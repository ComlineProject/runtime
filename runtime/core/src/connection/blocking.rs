use alloc::boxed::Box;
use alloc::vec::Vec;
use core::time::Duration;

use serde::Serialize;

use crate::contract::{
    Call, ConnectionFraming, ConnectionId, Datagram2Framing, Dispatch, Envelope, Frame, Kind,
    Outcome, Peer, ProtocolNumber, Reply, Request, RequestCall, RuntimeError, WireFormat,
};
use crate::handshake::{ComlineHandshake, Facts, Handshake};
use crate::transport::Transport;

use crate::connection::extensions::{next_connection_id, Extensions};
use crate::connection::state::{Event, State};

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
/// ([`AsyncConnection`](crate::connection::AsyncConnection)).
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
        Self::connect_with(
            transport,
            format,
            Datagram2Framing,
            &mut ComlineHandshake,
            ir_hash,
        )
    }

    /// Answer: run the `comline@1` handshake's provider side over
    /// `transport`, with `datagram@2`.
    pub fn accept(transport: T, format: W, ir_hash: u64) -> Result<Self, RuntimeError> {
        Self::accept_with(
            transport,
            format,
            Datagram2Framing,
            &mut ComlineHandshake,
            ir_hash,
        )
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
    /// `Ok(false)`: the transport closed. A frame over the transport's limit
    /// is [`RuntimeError::FrameTooLarge`], and the transport is closed after
    /// it.
    pub fn serve_one(&mut self) -> Result<bool, RuntimeError> {
        match self.link.transport.recv(&mut self.link.recv) {
            Ok(()) => {}
            Err(RuntimeError::Transport) => return Ok(false),
            Err(error) => return Err(error),
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
        self.link
            .call(&self.providers, protocol, call, params, None)
    }

    fn call_with_timeout<P: Serialize + ?Sized>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Duration,
    ) -> Result<(Envelope<'_>, &W), RuntimeError> {
        self.link
            .call(&self.providers, protocol, call, params, Some(timeout))
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
        self.link
            .call(self.providers, protocol, call, params, Some(timeout))
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
        self.check_size(request_id)?;
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
        let request_id = self.state.encode_request(
            &self.framing,
            protocol,
            call,
            params,
            &self.format,
            false,
            &mut self.send,
        )?;
        self.check_size(request_id)?;
        self.transport.send(&self.send)
    }

    /// Refuse the call just framed in `self.send` if it's over the
    /// transport's limit, taking it back: no request id is spent, nothing
    /// waits and nothing is sent, so the connection keeps working.
    fn check_size(&mut self, request_id: u64) -> Result<(), RuntimeError> {
        if self.send.len() > self.transport.max_frame() {
            self.state.retract(request_id);
            return Err(RuntimeError::FrameTooLarge);
        }
        Ok(())
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
    fn handle_frame(
        &mut self,
        providers: &Providers<T, W, F>,
    ) -> Result<Option<u64>, RuntimeError> {
        // A handler may receive frames of its own (a nested call), so the
        // request's bytes move out of `recv` while it runs.
        let frame = core::mem::take(&mut self.recv);
        let handled = match self.state.receive(&self.framing, &frame) {
            Ok(Event::Request {
                protocol,
                request,
                one_way,
            }) => self
                .serve_request(providers, protocol, request, one_way)
                .map(|()| None),
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
                    let mut handle = Handle {
                        link: self,
                        providers,
                    };
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
        let protocol = target
            .map(|(number, ..)| number)
            .or(protocol)
            .unwrap_or_default();

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
                if self.send.len() > self.transport.max_frame() {
                    // Too large to send: the caller hears a fault, not
                    // silence. This runtime doesn't log; a provider that
                    // wants to know checks its replies' size itself.
                    self.state.encode_reply(
                        &self.framing,
                        protocol,
                        request.request_id,
                        &Envelope::Fault,
                        &mut self.send,
                    );
                }
                self.transport.send(&self.send)
            }
            None => Ok(()),
        };
        self.body = body;
        sent
    }
}
