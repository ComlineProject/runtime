//! Connections: calls in both directions over one transport.
//!
//! [`Client`](crate::client::Client) and [`Server`](crate::serve::Server)
//! work in lockstep: one end asks, the other answers. On a [`Connection`]
//! either end can call the other, a side waiting for a reply keeps serving
//! the peer's requests (so calls nest: a handler can call back the peer that
//! called it), and several protocols share the connection, each under its
//! [`ProtocolNumber`]. It needs a [`ConnectionFraming`], whose frames say
//! whether they're a request or a reply: [`Datagram2Framing`] (`datagram@2`),
//! or [`JsonRpcFraming`](crate::framing::JsonRpcFraming), whose requests name
//! only their method, so the protocol is the one this side provides with a
//! call by that name.
//!
//! Two layers:
//! - [`State`] is the transport-independent core: it hands out request ids,
//!   tracks the calls waiting for a reply, and turns a received frame into an
//!   [`Event`]. It does no I/O, so a blocking, an async or a `no_std` driver
//!   can share it.
//! - [`Connection`] is the blocking driver: it owns the transport, routes
//!   requests to the providers registered on it, and replies to the calls
//!   waiting on them.
//!
//! Who dialled and who answered only matters for the handshake
//! ([`connect`](Connection::connect) runs the consumer's side,
//! [`accept`](Connection::accept) the provider's); after that both ends are
//! the same.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::any::{Any, TypeId};
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use serde::Serialize;

use crate::contract::{
    BufMut, Call, ConnectionFraming, ConnectionId, Datagram2Framing, Dispatch, Envelope, Frame,
    Kind, Outcome, Peer, ProtocolNumber, Reply, Request, RequestCall, RuntimeError, WireFormat,
};
use crate::handshake::{ComlineHandshake, Facts, Handshake};
use crate::transport::Transport;

// ── the transport-independent core ─────────────────────────────────────────

/// What a received frame means for this side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'f> {
    /// The peer calls `protocol` on this side (or, when the framing names
    /// only the call, whichever protocol has it); answer with
    /// [`State::encode_reply`] unless the call is one-way.
    Request {
        protocol: Option<ProtocolNumber>,
        request: Request<'f>,
        /// The frame asked for no reply: send none, whatever the handler
        /// returns.
        one_way: bool,
    },
    /// The reply to the innermost call this side is waiting on.
    Reply {
        request_id: u64,
        envelope: Envelope<'f>,
    },
    /// A reply to a call nobody waits on any more (it timed out). Drop it.
    Stale,
}

/// The calls one side of a connection has made and is waiting on, and the
/// request ids it hands out. No I/O: bytes in, [`Event`]s out.
///
/// For the blocking driver (`State::default()`) calls nest, so the waiting
/// calls form a stack, and a reply has to answer the innermost one: the peer
/// can't answer an outer call while it is still waiting on its own nested
/// call to this side. The async driver has several calls in flight at once,
/// answered in any order ([`unordered`](Self::unordered)).
///
/// Either way it makes room for waiting calls as they come, unless it was
/// given a fixed room ([`with_capacity`](Self::with_capacity)).
#[derive(Debug)]
pub struct State {
    next_id: u64,
    waiting: Vec<u64>,
    /// How many calls may wait at once; `usize::MAX` grows as needed.
    capacity: usize,
    unordered: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            next_id: 0,
            waiting: Vec::new(),
            capacity: usize::MAX,
            unordered: false,
        }
    }
}

impl State {
    /// A state whose waiting calls may be answered in any order.
    pub fn unordered() -> Self {
        Self {
            unordered: true,
            ..Self::default()
        }
    }

    /// A state with room for `capacity` waiting calls, reserved now, for a
    /// caller that mustn't allocate once it's set up (a binding through the
    /// C ABI). A call that would wait past that is [`RuntimeError::Busy`].
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            waiting: Vec::with_capacity(capacity),
            capacity,
            ..Self::default()
        }
    }

    /// [`with_capacity`](Self::with_capacity), answered in any order.
    pub fn unordered_with_capacity(capacity: usize) -> Self {
        Self {
            unordered: true,
            ..Self::with_capacity(capacity)
        }
    }

    /// Whether a call that expects a reply would find no room to wait.
    fn full(&self, expects_reply: bool) -> bool {
        expects_reply && self.waiting.len() >= self.capacity
    }

    /// Frame a call into `out` and return its request id. A call that
    /// `expects_reply` is waited on until [`receive`](Self::receive) sees its
    /// reply or [`abandon`](Self::abandon) gives up on it. With no room for it
    /// to wait it's [`RuntimeError::Busy`], and nothing is spent; a failed
    /// encode spends its id.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_request<F, W, P>(
        &mut self,
        framing: &F,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        format: &W,
        expects_reply: bool,
        out: &mut Vec<u8>,
    ) -> Result<u64, RuntimeError>
    where
        F: ConnectionFraming,
        W: WireFormat,
        P: Serialize + ?Sized,
    {
        if self.full(expects_reply) {
            return Err(RuntimeError::Busy);
        }
        let request_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        out.clear();
        framing.encode_request(protocol, call, request_id, !expects_reply, params, format, out)?;
        if expects_reply {
            self.waiting.push(request_id);
        }
        Ok(request_id)
    }

    /// Frame a call around `params` already serialised in the connection's
    /// wire format ([`ConnectionFraming::encode_request_body`]), appending to
    /// `out`, and return its request id.
    ///
    /// Nothing is committed unless the whole frame was written: when `out`
    /// dropped a write ([`BufMut::overflowed`]) it's
    /// [`RuntimeError::BufferTooSmall`], no id is spent and nothing waits, so
    /// the caller can retry with more room. With no room for the call to wait
    /// it's [`RuntimeError::Busy`], before anything is written.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_request_body<F: ConnectionFraming>(
        &mut self,
        framing: &F,
        protocol: ProtocolNumber,
        call_id: u16,
        name: &str,
        params: &[u8],
        expects_reply: bool,
        out: &mut dyn BufMut,
    ) -> Result<u64, RuntimeError> {
        if self.full(expects_reply) {
            return Err(RuntimeError::Busy);
        }
        let request_id = self.next_id;
        framing.encode_request_body(
            protocol,
            call_id,
            name,
            request_id,
            !expects_reply,
            params,
            out,
        );
        if out.overflowed() {
            return Err(RuntimeError::BufferTooSmall);
        }
        self.next_id = request_id.wrapping_add(1);
        if expects_reply {
            self.waiting.push(request_id);
        }
        Ok(request_id)
    }

    /// Frame the reply to the peer's request `request_id` into `out`.
    pub fn encode_reply<F: ConnectionFraming>(
        &self,
        framing: &F,
        protocol: ProtocolNumber,
        request_id: u64,
        reply: &Envelope<'_>,
        out: &mut Vec<u8>,
    ) {
        out.clear();
        framing.encode_reply(protocol, request_id, reply, out);
    }

    /// What `frame` means. A malformed frame, or (unless
    /// [`unordered`](Self::unordered)) a reply that skips the innermost
    /// waiting call, is [`RuntimeError::Framing`].
    pub fn receive<'f, F: ConnectionFraming>(
        &mut self,
        framing: &F,
        frame: &'f [u8],
    ) -> Result<Event<'f>, RuntimeError> {
        match framing.decode(frame).ok_or(RuntimeError::Framing)? {
            Frame::Request { protocol, request, one_way } => Ok(Event::Request { protocol, request, one_way }),
            Frame::Reply { request_id, envelope, .. } => {
                if self.unordered {
                    return Ok(match self.waiting.iter().position(|&id| id == request_id) {
                        Some(at) => {
                            self.waiting.swap_remove(at);
                            Event::Reply { request_id, envelope }
                        }
                        None => Event::Stale,
                    });
                }
                if self.waiting.last() == Some(&request_id) {
                    self.waiting.pop();
                    Ok(Event::Reply { request_id, envelope })
                } else if self.waiting.contains(&request_id) {
                    Err(RuntimeError::Framing)
                } else {
                    Ok(Event::Stale)
                }
            }
        }
    }

    /// Stop waiting on `request_id` (it timed out); a late reply to it is
    /// then [`Event::Stale`].
    pub fn abandon(&mut self, request_id: u64) {
        self.waiting.retain(|&id| id != request_id);
    }

    /// Whether any call is waiting on a reply.
    pub fn is_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }
}

// ── per-connection data ────────────────────────────────────────────────────

/// Typed data kept on a connection, one value per type, like
/// `http::Extensions`: what the application knows about the peer (a session,
/// a user id) or keeps for it (a rate-limit bucket). Values are `Send`, so
/// an async connection's handlers on any thread can reach them.
#[derive(Default)]
pub struct Extensions {
    values: BTreeMap<TypeId, Box<dyn Any + Send>>,
}

impl Extensions {
    /// Store `value`, returning the one of its type stored before.
    pub fn insert<T: Send + 'static>(&mut self, value: T) -> Option<T> {
        self.values
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|old| old.downcast().ok().map(|old| *old))
    }

    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.values.get(&TypeId::of::<T>())?.downcast_ref()
    }

    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.values.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }

    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.values
            .remove(&TypeId::of::<T>())
            .and_then(|old| old.downcast().ok().map(|old| *old))
    }
}

impl core::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Extensions").field("len", &self.values.len()).finish()
    }
}

pub(crate) fn next_connection_id() -> ConnectionId {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    ConnectionId(NEXT.fetch_add(1, Ordering::Relaxed) as u64)
}

// ── the blocking driver ────────────────────────────────────────────────────

/// One end of a connection, over a blocking [`Transport`]. Generic over the
/// [`ConnectionFraming`]; defaults to `datagram@2`.
///
/// Register what this side provides with [`provide`](Self::provide), then
/// either [`serve`](Self::serve) the peer, or call it through the [`Peer`]
/// methods: a call waits for its reply and serves the peer's requests
/// meanwhile. Handlers get a [`Handle`] to call the peer back.
///
/// One side does one thing at a time: a request is handled to the end before
/// the next frame is read, and a call made while serving finishes before the
/// request that made it. To call the peer from another thread or task while
/// this one serves, use the async driver
/// ([`AsyncConnection`](crate::async_connection::AsyncConnection)).
pub struct Connection<T, W, F = Datagram2Framing> {
    link: Link<T, W, F>,
    providers: Providers<T, W, F>,
}

/// The parts of a [`Connection`] a handler borrows mutably while the
/// providers stay borrowed for nested requests.
struct Link<T, W, F> {
    transport: T,
    format: W,
    framing: F,
    state: State,
    id: ConnectionId,
    extensions: Extensions,
    send: Vec<u8>,
    recv: Vec<u8>,
    body: Vec<u8>,
}

/// This side's providers, by protocol number.
struct Providers<T, W, F> {
    table: Vec<Option<BoxedProvider<T, W, F>>>,
}

impl<T, W, F> Providers<T, W, F> {
    /// The provider a request is for, and its call's index there.
    fn find(
        &self,
        protocol: Option<ProtocolNumber>,
        call: RequestCall<'_>,
    ) -> Option<(ProtocolNumber, &dyn Provider<T, W, F>, u16)> {
        let numbers = match protocol {
            Some(number) => usize::from(number)..usize::from(number) + 1,
            None => 0..self.table.len(),
        };
        numbers.into_iter().find_map(|number| {
            let provider = self.table.get(number)?.as_deref()?;
            let index = call_index(call, provider.calls())?;
            Some((ProtocolNumber::try_from(number).ok()?, provider, index))
        })
    }
}

/// `call`'s index among `calls`: its id, or its name's position.
pub(crate) fn call_index(call: RequestCall<'_>, calls: &[&str]) -> Option<u16> {
    match call {
        RequestCall::Id(id) => Some(id),
        RequestCall::Name(name) => calls.iter().position(|c| *c == name).map(|i| i as u16),
    }
}

type BoxedProvider<T, W, F> = Box<dyn Provider<T, W, F>>;

/// A [`Dispatch`] with its generic method fixed to this connection's types,
/// so different protocols' dispatchers fit one table.
trait Provider<T, W, F> {
    fn calls(&self) -> &'static [&'static str];
    fn dispatch(
        &self,
        call: Kind,
        params: &[u8],
        reply: &mut Reply,
        peer: &mut Handle<'_, T, W, F>,
    ) -> Result<(), RuntimeError>;
}

impl<D, T, W, F> Provider<T, W, F> for D
where
    D: Dispatch,
    T: Transport,
    W: WireFormat,
    F: ConnectionFraming,
{
    fn calls(&self) -> &'static [&'static str] {
        Dispatch::calls(self)
    }

    fn dispatch(
        &self,
        call: Kind,
        params: &[u8],
        reply: &mut Reply,
        peer: &mut Handle<'_, T, W, F>,
    ) -> Result<(), RuntimeError> {
        self.dispatch_with(call, params, reply, peer)
    }
}

/// What a handler on a [`Connection`] is given: the connection it was called
/// on, to call the peer back or reach its [`Extensions`]. While a call made
/// through it waits, the peer's requests keep being served.
pub struct Handle<'a, T, W, F> {
    link: &'a mut Link<T, W, F>,
    providers: &'a Providers<T, W, F>,
}

impl<T: Transport, W: WireFormat> Connection<T, W, Datagram2Framing> {
    /// Dial: run the `comline@1` handshake's consumer side over `transport`,
    /// with `datagram@2`. Refuses (`RuntimeError::Handshake`) on a schema,
    /// format or framing mismatch.
    pub fn connect(transport: T, format: W, ir_hash: u64) -> Result<Self, RuntimeError> {
        Self::connect_with(transport, format, Datagram2Framing, &mut ComlineHandshake, ir_hash)
    }

    /// Answer: run the `comline@1` handshake's provider side over
    /// `transport`, with `datagram@2`.
    pub fn accept(transport: T, format: W, ir_hash: u64) -> Result<Self, RuntimeError> {
        Self::accept_with(transport, format, Datagram2Framing, &mut ComlineHandshake, ir_hash)
    }

    /// A connection over `transport` **without a handshake**, with
    /// `datagram@2`. A mismatch then surfaces later as a decode or framing
    /// error.
    pub fn new_unverified(transport: T, format: W) -> Self {
        Self::with_framing_unverified(transport, format, Datagram2Framing)
    }
}

impl<T: Transport, W: WireFormat, F: ConnectionFraming> Connection<T, W, F> {
    /// [`new_unverified`](Connection::new_unverified) with another framing.
    pub fn with_framing_unverified(transport: T, format: W, framing: F) -> Self {
        Self {
            link: Link {
                transport,
                format,
                framing,
                state: State::default(),
                id: next_connection_id(),
                extensions: Extensions::default(),
                send: Vec::new(),
                recv: Vec::new(),
                body: Vec::new(),
            },
            providers: Providers { table: Vec::new() },
        }
    }

    /// Dial with `handshake` ([`Handshake::initiate`]).
    pub fn connect_with<H: Handshake>(
        mut transport: T,
        format: W,
        framing: F,
        handshake: &mut H,
        ir_hash: u64,
    ) -> Result<Self, RuntimeError> {
        let local = Facts::new(ir_hash, format.name(), framing.name());
        handshake.initiate(&mut transport, &local)?;
        Ok(Self::with_framing_unverified(transport, format, framing))
    }

    /// Answer with `handshake` ([`Handshake::respond`]).
    pub fn accept_with<H: Handshake>(
        mut transport: T,
        format: W,
        framing: F,
        handshake: &mut H,
        ir_hash: u64,
    ) -> Result<Self, RuntimeError> {
        let local = Facts::new(ir_hash, format.name(), framing.name());
        handshake.respond(&mut transport, &local)?;
        Ok(Self::with_framing_unverified(transport, format, framing))
    }

    /// Provide `protocol` on this connection: the peer's requests to it go to
    /// `dispatcher`. Replaces one already registered for it.
    pub fn provide<D: Dispatch + 'static>(&mut self, protocol: ProtocolNumber, dispatcher: D) {
        let index = usize::from(protocol);
        if self.providers.table.len() <= index {
            self.providers.table.resize_with(index + 1, || None);
        }
        self.providers.table[index] = Some(Box::new(dispatcher));
    }

    /// Handle the peer's next frame. `Ok(true)`: one was handled;
    /// `Ok(false)`: the transport closed.
    pub fn serve_one(&mut self) -> Result<bool, RuntimeError> {
        if self.link.transport.recv(&mut self.link.recv).is_err() {
            return Ok(false);
        }
        self.link.handle_frame(&self.providers)?;
        Ok(true)
    }

    /// Serve the peer until the transport closes.
    pub fn serve(&mut self) -> Result<(), RuntimeError> {
        while self.serve_one()? {}
        Ok(())
    }

    /// The underlying transport, e.g. to close it or read its peer address.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.link.transport
    }

    /// Consume the connection, returning its transport.
    pub fn into_transport(self) -> T {
        self.link.transport
    }
}

impl<T: Transport, W: WireFormat, F: ConnectionFraming> Peer for Connection<T, W, F> {
    type Format = W;

    fn format(&self) -> &W {
        &self.link.format
    }

    fn id(&self) -> ConnectionId {
        self.link.id
    }

    fn call<P: Serialize + ?Sized>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(Envelope<'_>, &W), RuntimeError> {
        self.link.call(&self.providers, protocol, call, params, None)
    }

    fn call_with_timeout<P: Serialize + ?Sized>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Duration,
    ) -> Result<(Envelope<'_>, &W), RuntimeError> {
        self.link.call(&self.providers, protocol, call, params, Some(timeout))
    }

    fn notify<P: Serialize + ?Sized>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(), RuntimeError> {
        self.link.notify(protocol, call, params)
    }

    fn extensions(&mut self) -> &mut Extensions {
        &mut self.link.extensions
    }
}

impl<T: Transport, W: WireFormat, F: ConnectionFraming> Peer for Handle<'_, T, W, F> {
    type Format = W;

    fn format(&self) -> &W {
        &self.link.format
    }

    fn id(&self) -> ConnectionId {
        self.link.id
    }

    fn call<P: Serialize + ?Sized>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(Envelope<'_>, &W), RuntimeError> {
        self.link.call(self.providers, protocol, call, params, None)
    }

    fn call_with_timeout<P: Serialize + ?Sized>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Duration,
    ) -> Result<(Envelope<'_>, &W), RuntimeError> {
        self.link.call(self.providers, protocol, call, params, Some(timeout))
    }

    fn notify<P: Serialize + ?Sized>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(), RuntimeError> {
        self.link.notify(protocol, call, params)
    }

    fn extensions(&mut self) -> &mut Extensions {
        &mut self.link.extensions
    }
}

impl<T: Transport, W: WireFormat, F: ConnectionFraming> Link<T, W, F> {
    fn call<P: Serialize + ?Sized>(
        &mut self,
        providers: &Providers<T, W, F>,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Option<Duration>,
    ) -> Result<(Envelope<'_>, &W), RuntimeError> {
        let request_id = self.state.encode_request(
            &self.framing,
            protocol,
            call,
            params,
            &self.format,
            true,
            &mut self.send,
        )?;
        if let Err(error) = self.transport.send(&self.send) {
            self.state.abandon(request_id);
            return Err(error);
        }
        self.wait(providers, request_id, timeout)?;
        match self.framing.decode(&self.recv) {
            Some(Frame::Reply { envelope, .. }) => Ok((envelope, &self.format)),
            _ => Err(RuntimeError::Framing),
        }
    }

    fn notify<P: Serialize + ?Sized>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(), RuntimeError> {
        self.state.encode_request(
            &self.framing,
            protocol,
            call,
            params,
            &self.format,
            false,
            &mut self.send,
        )?;
        self.transport.send(&self.send)
    }

    /// Read frames, serving the peer's requests, until the reply to
    /// `request_id` is in `self.recv`.
    fn wait(
        &mut self,
        providers: &Providers<T, W, F>,
        request_id: u64,
        timeout: Option<Duration>,
    ) -> Result<(), RuntimeError> {
        #[cfg(feature = "std")]
        let deadline = timeout.map(|t| std::time::Instant::now() + t);
        loop {
            let received = match timeout {
                None => self.transport.recv(&mut self.recv).map(|()| true),
                Some(_timeout) => {
                    #[cfg(feature = "std")]
                    let _timeout = deadline
                        .unwrap()
                        .saturating_duration_since(std::time::Instant::now());
                    self.transport.recv_timeout(&mut self.recv, _timeout)
                }
            };
            match received {
                Ok(true) => {}
                Ok(false) => {
                    self.state.abandon(request_id);
                    return Err(RuntimeError::Timeout);
                }
                Err(error) => {
                    self.state.abandon(request_id);
                    return Err(error);
                }
            }
            if self.handle_frame(providers)? == Some(request_id) {
                return Ok(());
            }
        }
    }

    /// Act on the frame in `self.recv`: serve a request, or report which
    /// waiting call a reply completes (leaving it in `self.recv`).
    fn handle_frame(&mut self, providers: &Providers<T, W, F>) -> Result<Option<u64>, RuntimeError> {
        // A handler may receive frames of its own (a nested call), so the
        // request's bytes move out of `recv` while it runs.
        let frame = core::mem::take(&mut self.recv);
        let handled = match self.state.receive(&self.framing, &frame) {
            Ok(Event::Request { protocol, request, one_way }) => {
                self.serve_request(providers, protocol, request, one_way).map(|()| None)
            }
            Ok(Event::Reply { request_id, .. }) => Ok(Some(request_id)),
            Ok(Event::Stale) => Ok(None),
            Err(error) => Err(error),
        };
        self.recv = frame;
        handled
    }

    fn serve_request(
        &mut self,
        providers: &Providers<T, W, F>,
        protocol: Option<ProtocolNumber>,
        request: Request<'_>,
        one_way: bool,
    ) -> Result<(), RuntimeError> {
        let mut body = core::mem::take(&mut self.body);
        body.clear();
        let target = providers.find(protocol, request.call);
        let outcome = {
            let mut reply = Reply::new(&mut body);
            let dispatched = match target {
                Some((_, provider, index)) => {
                    let mut handle = Handle { link: self, providers };
                    provider
                        .dispatch(Kind::Id(index), request.params, &mut reply, &mut handle)
                        .map(|()| reply.outcome())
                }
                // A protocol or call this side doesn't provide: the caller
                // hears a fault, as for an unknown call (see
                // `Server::serve_one`).
                None => Err(RuntimeError::UnknownCall),
            };
            dispatched.unwrap_or(Outcome::Fault)
        };
        let protocol = target.map(|(number, ..)| number).or(protocol).unwrap_or_default();

        let envelope = match outcome {
            _ if one_way => None,
            Outcome::None => None,
            Outcome::Ok => Some(Envelope::Ok(&body)),
            Outcome::Err(id) => Some(Envelope::Err { id, body: &body }),
            Outcome::Fault => Some(Envelope::Fault),
        };
        let sent = match envelope {
            Some(envelope) => {
                self.state.encode_reply(
                    &self.framing,
                    protocol,
                    request.request_id,
                    &envelope,
                    &mut self.send,
                );
                self.transport.send(&self.send)
            }
            None => Ok(()),
        };
        self.body = body;
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_must_answer_the_innermost_waiting_call() {
        let framing = Datagram2Framing;
        let mut state = State::default();
        let mut out = Vec::new();
        let outer = state
            .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
            .unwrap();
        let inner = state
            .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
            .unwrap();

        let reply = |id| {
            let mut frame = Vec::new();
            framing.encode_reply(0, id, &Envelope::Ok(b""), &mut frame);
            frame
        };
        assert_eq!(state.receive(&framing, &reply(outer)), Err(RuntimeError::Framing));
        assert!(matches!(state.receive(&framing, &reply(inner)), Ok(Event::Reply { .. })));
        assert!(matches!(state.receive(&framing, &reply(outer)), Ok(Event::Reply { .. })));
        assert!(!state.is_waiting());
        assert_eq!(state.receive(&framing, &reply(99)), Ok(Event::Stale));
    }

    #[test]
    fn an_unordered_state_takes_replies_in_any_order() {
        let framing = Datagram2Framing;
        let mut state = State::unordered();
        let mut out = Vec::new();
        let first = state
            .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
            .unwrap();
        let second = state
            .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
            .unwrap();
        let reply = |id| {
            let mut frame = Vec::new();
            framing.encode_reply(0, id, &Envelope::Ok(b""), &mut frame);
            frame
        };
        assert!(matches!(state.receive(&framing, &reply(first)), Ok(Event::Reply { request_id, .. }) if request_id == first));
        assert!(matches!(state.receive(&framing, &reply(second)), Ok(Event::Reply { .. })));
        assert_eq!(state.receive(&framing, &reply(first)), Ok(Event::Stale));
        assert!(!state.is_waiting());
    }

    #[test]
    fn an_abandoned_call_s_late_reply_is_stale() {
        let framing = Datagram2Framing;
        let mut state = State::default();
        let mut out = Vec::new();
        let id = state
            .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
            .unwrap();
        state.abandon(id);
        let mut frame = Vec::new();
        framing.encode_reply(0, id, &Envelope::Fault, &mut frame);
        assert_eq!(state.receive(&framing, &frame), Ok(Event::Stale));
    }

    #[test]
    fn extensions_hold_one_value_per_type() {
        struct Session(u32);
        let mut extensions = Extensions::default();
        assert!(extensions.insert(Session(1)).is_none());
        assert_eq!(extensions.insert(Session(2)).map(|s| s.0), Some(1));
        extensions.get_mut::<Session>().unwrap().0 += 1;
        assert_eq!(extensions.get::<Session>().map(|s| s.0), Some(3));
        assert_eq!(extensions.remove::<Session>().map(|s| s.0), Some(3));
        assert!(extensions.get::<Session>().is_none());
    }

    /// A format that writes nothing, for framing-only tests.
    struct Unit;

    impl WireFormat for Unit {
        fn name(&self) -> &'static str {
            "unit"
        }
        fn encode<T: Serialize + ?Sized>(
            &self,
            _: &T,
            _: &mut dyn crate::contract::BufMut,
        ) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn decode<'de, T: serde::Deserialize<'de>>(&self, _: &'de [u8]) -> Result<T, RuntimeError> {
            Err(RuntimeError::Serialization)
        }
    }
}
