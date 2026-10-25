//! The async driver for connections, on any executor.
//!
//! [`Connection`](crate::connection::Connection) is blocking: one thread
//! serves, and calls the peer only between requests. An [`AsyncConnection`]
//! is a future the application spawns ([`run`](AsyncConnection::run)) plus
//! cloneable [`AsyncHandle`]s, so any task can call the peer while the
//! connection serves it. It shares the transport-independent
//! [`State`] with the blocking driver, and needs nothing from an executor
//! but polling: handlers run inside `run`'s future, not as spawned tasks.
//!
//! - **Transport:** anything that's a [`Stream`] of frames and a [`Sink`]
//!   for them ([`AsyncTransport`]). [`async_duplex`] is in-memory; the
//!   `tokio` feature adds TCP ([`tokio::Tcp`]).
//! - **Providers** implement [`AsyncDispatch`]; their handlers get an
//!   [`AsyncPeer`] to call back the peer that called them.
//! - **Order:** the peer's requests are handled one at a time, in order,
//!   except while every running handler waits on a call to the peer: then
//!   the next starts, since it may be that call's nested request.
//!   [`concurrent`](AsyncConnection::concurrent) lets several run at once.
//! - **Timeouts** need a clock, which the executor has:
//!   [`set_timer`](AsyncConnection::set_timer). Without one,
//!   [`AsyncPeer::call_with_timeout`] waits. Dropping a call's future gives up
//!   on it too, so the executor's own timeout works as well.

use std::collections::VecDeque;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use alloc::collections::BTreeMap;
use futures_channel::{mpsc, oneshot};
use futures_util::future::{select, Either};
use futures_util::stream::FuturesUnordered;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::Serialize;

use crate::connection::{call_index, Event, Extensions, State};
use crate::contract::{
    Call, ConnectionFraming, ConnectionId, Datagram2Framing, Envelope, HandshakeFrame, Kind,
    Outcome, ProtocolNumber, RuntimeError, WireFormat,
};

// ── transports ─────────────────────────────────────────────────────────────

/// An async frame channel: a [`Stream`] of received frames (ending, or
/// yielding an error, once the peer is gone) and a [`Sink`] to send them.
pub trait AsyncTransport:
    Stream<Item = Result<Vec<u8>, RuntimeError>> + Sink<Vec<u8>, Error = RuntimeError> + Send + Unpin + 'static
{
}

impl<T> AsyncTransport for T where
    T: Stream<Item = Result<Vec<u8>, RuntimeError>>
        + Sink<Vec<u8>, Error = RuntimeError>
        + Send
        + Unpin
        + 'static
{
}

/// An in-process [`AsyncTransport`], for tests and same-binary setups.
/// Dropping or closing one end ends the other's stream.
pub struct AsyncInMemory {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

/// A crossed pair: what `a` sends, `b` receives, and vice versa.
pub fn async_duplex() -> (AsyncInMemory, AsyncInMemory) {
    let (a_tx, a_rx) = mpsc::unbounded();
    let (b_tx, b_rx) = mpsc::unbounded();
    (AsyncInMemory { tx: a_tx, rx: b_rx }, AsyncInMemory { tx: b_tx, rx: a_rx })
}

impl Stream for AsyncInMemory {
    type Item = Result<Vec<u8>, RuntimeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_next_unpin(cx).map(|frame| frame.map(Ok))
    }
}

impl Sink<Vec<u8>> for AsyncInMemory {
    type Error = RuntimeError;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, frame: Vec<u8>) -> Result<(), RuntimeError> {
        self.tx.unbounded_send(frame).map_err(|_| RuntimeError::Transport)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
        self.tx.close_channel();
        Poll::Ready(Ok(()))
    }
}

// ── what calls and handlers exchange ───────────────────────────────────────

/// A boxed, `Send` future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A clock for call timeouts: a future that completes after the duration.
/// With tokio, [`tokio::timer`].
pub type Timer = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// A reply that owns its bytes: what an async call returns, since it crosses
/// from the driver to the calling task. [`envelope`](Self::envelope) borrows
/// it as the blocking driver's [`Envelope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedEnvelope {
    Ok(Vec<u8>),
    Err { id: u16, body: Vec<u8> },
    Fault,
}

impl OwnedEnvelope {
    pub fn envelope(&self) -> Envelope<'_> {
        match self {
            OwnedEnvelope::Ok(payload) => Envelope::Ok(payload),
            OwnedEnvelope::Err { id, body } => Envelope::Err { id: *id, body },
            OwnedEnvelope::Fault => Envelope::Fault,
        }
    }
}

impl From<Envelope<'_>> for OwnedEnvelope {
    fn from(envelope: Envelope<'_>) -> Self {
        match envelope {
            Envelope::Ok(payload) => OwnedEnvelope::Ok(payload.to_vec()),
            Envelope::Err { id, body } => OwnedEnvelope::Err {
                id,
                body: body.to_vec(),
            },
            Envelope::Fault => OwnedEnvelope::Fault,
        }
    }
}

/// What an async handler produced: the [`Reply`](crate::contract::Reply) of
/// the blocking side, owned, so it can be returned from a future.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    outcome: Outcome,
    body: Vec<u8>,
}

impl Answer {
    /// A success: `payload` is the serialised return value.
    pub fn ok(payload: Vec<u8>) -> Self {
        Self { outcome: Outcome::Ok, body: payload }
    }

    /// A raised schema error: `body` is the serialised error struct, `id`
    /// its ordinal.
    pub fn err(id: u16, body: Vec<u8>) -> Self {
        Self { outcome: Outcome::Err(id), body }
    }

    /// A failure the schema doesn't describe.
    pub fn fault() -> Self {
        Self { outcome: Outcome::Fault, body: Vec::new() }
    }

    /// A one-way call's: nothing is sent back.
    pub fn none() -> Self {
        Self { outcome: Outcome::None, body: Vec::new() }
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    fn envelope(&self) -> Option<Envelope<'_>> {
        match self.outcome {
            Outcome::None => None,
            Outcome::Ok => Some(Envelope::Ok(&self.body)),
            Outcome::Err(id) => Some(Envelope::Err { id, body: &self.body }),
            Outcome::Fault => Some(Envelope::Fault),
        }
    }
}

/// The other end of an async connection, as this side's code sees it: what
/// a generated async sender calls through, and what an async handler is
/// given. [`AsyncHandle`] implements it.
pub trait AsyncPeer: Send + Sync {
    /// The wire format both ends agreed on.
    type Format: WireFormat + Send + Sync;

    /// The format, to decode a reply or a request's params with.
    fn format(&self) -> &Self::Format;

    /// This connection's id.
    fn id(&self) -> ConnectionId;

    /// Call `call` on the peer's `protocol`. The request is sent before the
    /// future is first polled; awaiting it waits for the reply. Dropping the
    /// future gives up on the call, and a late reply is dropped.
    fn call<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> impl Future<Output = Result<OwnedEnvelope, RuntimeError>> + Send;

    /// [`call`](Self::call), but give up after `timeout` with
    /// [`RuntimeError::Timeout`], using the connection's
    /// [`Timer`](AsyncConnection::set_timer). Without one it waits.
    fn call_with_timeout<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Duration,
    ) -> impl Future<Output = Result<OwnedEnvelope, RuntimeError>> + Send;

    /// Send a one-way call: no reply is awaited.
    fn notify<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(), RuntimeError>;

    /// Typed data the application keeps on this connection. A lock: drop
    /// the guard before awaiting.
    fn extensions(&self) -> MutexGuard<'_, Extensions>;
}

/// The provider side of a protocol on an [`AsyncConnection`]: the async
/// counterpart of [`Dispatch`](crate::contract::Dispatch). The generated
/// `<Proto>AsyncDispatcher` implements it.
pub trait AsyncDispatch: Send + Sync + 'static {
    /// The protocol's function names, in declaration order.
    fn calls(&self) -> &'static [&'static str];

    /// Run the handler for `call` with its encoded `params`, decoded with
    /// `peer.format()`.
    fn dispatch<'a, P: AsyncPeer>(
        &'a self,
        call: Kind,
        params: &'a [u8],
        peer: &'a P,
    ) -> impl Future<Output = Result<Answer, RuntimeError>> + Send + 'a;
}

// ── the connection ─────────────────────────────────────────────────────────

/// What the driver and every handle share.
struct Shared<W, F> {
    format: W,
    framing: F,
    id: ConnectionId,
    calls: Mutex<Calls>,
    commands: mpsc::UnboundedSender<Command>,
    extensions: Mutex<Extensions>,
    /// Calls to the peer that handlers are waiting on.
    handlers_waiting: AtomicUsize,
    timer: OnceLock<Timer>,
}

struct Calls {
    state: State,
    waiting: BTreeMap<u64, oneshot::Sender<OwnedEnvelope>>,
    open: bool,
}

enum Command {
    Frame(Vec<u8>),
    Close,
}

/// A protocol's provider, with its handlers' futures boxed so different
/// protocols' dispatchers fit one table.
trait Provider<W, F>: Send + Sync {
    fn calls(&self) -> &'static [&'static str];
    fn dispatch(
        self: Arc<Self>,
        call: Kind,
        params: Vec<u8>,
        peer: AsyncHandle<W, F>,
    ) -> BoxFuture<'static, Result<Answer, RuntimeError>>;
}

impl<D, W, F> Provider<W, F> for D
where
    D: AsyncDispatch,
    W: WireFormat + Send + Sync + 'static,
    F: ConnectionFraming + Send + Sync + 'static,
{
    fn calls(&self) -> &'static [&'static str] {
        AsyncDispatch::calls(self)
    }

    fn dispatch(
        self: Arc<Self>,
        call: Kind,
        params: Vec<u8>,
        peer: AsyncHandle<W, F>,
    ) -> BoxFuture<'static, Result<Answer, RuntimeError>> {
        Box::pin(async move { AsyncDispatch::dispatch(&*self, call, &params, &peer).await })
    }
}

type Providers<W, F> = Vec<Option<Arc<dyn Provider<W, F>>>>;

/// One end of a connection, driven by a future: register what this side
/// provides with [`provide`](Self::provide), take [`handle`](Self::handle)s
/// to call the peer, then spawn [`run`](Self::run) on any executor.
///
/// ```ignore
/// let mut conn = AsyncConnection::accept(transport, Json, CONNECTION_IR_HASH).await?;
/// ChatAsyncDispatcher(ChatImpl).provide_on(&mut conn);
/// let events = conn.handle();
/// tokio::spawn(conn.run());
/// EventsAsyncSender::new(&events).message("pushed").await?;
/// ```
pub struct AsyncConnection<T, W, F = Datagram2Framing> {
    transport: T,
    shared: Arc<Shared<W, F>>,
    commands: mpsc::UnboundedReceiver<Command>,
    providers: Providers<W, F>,
    limit: usize,
}

impl<T, W> AsyncConnection<T, W, Datagram2Framing>
where
    T: AsyncTransport,
    W: WireFormat + Send + Sync + 'static,
{
    /// Dial: run the `comline@1` handshake's consumer side over
    /// `transport`, with `datagram@2`. Refuses (`RuntimeError::Handshake`) on
    /// a schema, format or framing mismatch.
    pub async fn connect(transport: T, format: W, ir_hash: u64) -> Result<Self, RuntimeError> {
        Self::handshake(transport, format, Datagram2Framing, ir_hash).await
    }

    /// Answer: run the `comline@1` handshake's provider side. It's
    /// symmetric, so the same exchange as [`connect`](Self::connect).
    pub async fn accept(transport: T, format: W, ir_hash: u64) -> Result<Self, RuntimeError> {
        Self::handshake(transport, format, Datagram2Framing, ir_hash).await
    }

    /// A connection over `transport` **without a handshake**, with
    /// `datagram@2`.
    pub fn new_unverified(transport: T, format: W) -> Self {
        Self::with_framing_unverified(transport, format, Datagram2Framing)
    }
}

impl<T, W, F> AsyncConnection<T, W, F>
where
    T: AsyncTransport,
    W: WireFormat + Send + Sync + 'static,
    F: ConnectionFraming + Send + Sync + 'static,
{
    /// [`new_unverified`](AsyncConnection::new_unverified) with another
    /// framing.
    pub fn with_framing_unverified(transport: T, format: W, framing: F) -> Self {
        let (commands_tx, commands) = mpsc::unbounded();
        Self {
            transport,
            shared: Arc::new(Shared {
                format,
                framing,
                id: crate::connection::next_connection_id(),
                calls: Mutex::new(Calls {
                    state: State::unordered(),
                    waiting: BTreeMap::new(),
                    open: true,
                }),
                commands: commands_tx,
                extensions: Mutex::new(Extensions::default()),
                handlers_waiting: AtomicUsize::new(0),
                timer: OnceLock::new(),
            }),
            commands,
            providers: Vec::new(),
            limit: 1,
        }
    }

    /// The `comline@1` exchange over `transport` with another framing:
    /// each end sends its handshake frame and checks the peer's.
    pub async fn handshake(mut transport: T, format: W, framing: F, ir_hash: u64) -> Result<Self, RuntimeError> {
        let ours = HandshakeFrame::new(ir_hash, format.name(), framing.name(), 0);
        let mut frame = Vec::new();
        ours.encode(&mut frame);
        transport.send(frame).await?;
        let theirs = transport.next().await.ok_or(RuntimeError::Transport)??;
        ours.check(&HandshakeFrame::decode(&theirs).ok_or(RuntimeError::Handshake)?)?;
        Ok(Self::with_framing_unverified(transport, format, framing))
    }

    /// Provide `protocol` on this connection: the peer's requests to it go to
    /// `dispatcher`. Replaces one already registered for it.
    pub fn provide<D: AsyncDispatch>(&mut self, protocol: ProtocolNumber, dispatcher: D) {
        let index = usize::from(protocol);
        if self.providers.len() <= index {
            self.providers.resize_with(index + 1, || None);
        }
        self.providers[index] = Some(Arc::new(dispatcher));
    }

    /// Let up to `limit` of the peer's requests run at once, for protocols
    /// that don't rely on order. The default, 1, handles them in order.
    pub fn concurrent(&mut self, limit: usize) {
        self.limit = limit.max(1);
    }

    /// The clock [`AsyncPeer::call_with_timeout`] uses. Set once; later calls
    /// are ignored.
    pub fn set_timer(&mut self, timer: Timer) {
        let _ = self.shared.timer.set(timer);
    }

    /// A handle to call the peer with, from any task. Calls made before
    /// [`run`](Self::run) starts are sent once it does.
    pub fn handle(&self) -> AsyncHandle<W, F> {
        AsyncHandle {
            shared: Arc::clone(&self.shared),
            from_handler: false,
        }
    }

    /// This connection's id.
    pub fn id(&self) -> ConnectionId {
        self.shared.id
    }

    /// Drive the connection until the transport closes or a handle
    /// [`close`](AsyncHandle::close)s it. Calls still waiting then fail with
    /// [`RuntimeError::Transport`]. A malformed frame ends it with
    /// [`RuntimeError::Framing`].
    pub async fn run(self) -> Result<(), RuntimeError> {
        let mut driver = Driver {
            transport: self.transport,
            shared: self.shared,
            commands: self.commands,
            providers: self.providers,
            limit: self.limit,
            queue: VecDeque::new(),
            running: FuturesUnordered::new(),
            outbox: VecDeque::new(),
            unflushed: false,
            closing: false,
        };
        let ended = poll_fn(|cx| driver.poll(cx)).await;
        let mut calls = driver.shared.calls.lock().unwrap();
        calls.open = false;
        // Dropping the senders fails the calls still waiting.
        calls.waiting.clear();
        ended
    }
}

/// A peer request waiting to start.
struct Incoming<W, F> {
    protocol: ProtocolNumber,
    request_id: u64,
    /// The frame asked for no reply.
    one_way: bool,
    target: Option<(Arc<dyn Provider<W, F>>, u16)>,
    params: Vec<u8>,
}

struct Driver<T, W, F> {
    transport: T,
    shared: Arc<Shared<W, F>>,
    commands: mpsc::UnboundedReceiver<Command>,
    providers: Providers<W, F>,
    limit: usize,
    queue: VecDeque<Incoming<W, F>>,
    /// Handlers running, each yielding its reply frame, if any.
    running: FuturesUnordered<BoxFuture<'static, Option<Vec<u8>>>>,
    outbox: VecDeque<Vec<u8>>,
    unflushed: bool,
    closing: bool,
}

impl<T, W, F> Driver<T, W, F>
where
    T: AsyncTransport,
    W: WireFormat + Send + Sync + 'static,
    F: ConnectionFraming + Send + Sync + 'static,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
        loop {
            let mut progressed = false;

            while let Poll::Ready(command) = self.commands.poll_next_unpin(cx) {
                progressed = true;
                match command {
                    Some(Command::Frame(frame)) => self.outbox.push_back(frame),
                    Some(Command::Close) | None => {
                        self.closing = true;
                        break;
                    }
                }
            }

            if !self.closing {
                loop {
                    match Pin::new(&mut self.transport).poll_next(cx) {
                        Poll::Ready(Some(Ok(frame))) => {
                            progressed = true;
                            self.receive(&frame)?;
                        }
                        // The peer is gone.
                        Poll::Ready(Some(Err(_)) | None) => return Poll::Ready(Ok(())),
                        Poll::Pending => break,
                    }
                }
            }

            progressed |= self.start_queued();

            while let Poll::Ready(Some(reply)) = self.running.poll_next_unpin(cx) {
                progressed = true;
                self.outbox.extend(reply);
            }

            match self.poll_write(cx) {
                Poll::Ready(Ok(())) => {
                    if self.closing {
                        return Pin::new(&mut self.transport).poll_close(cx).map(|_| Ok(()));
                    }
                }
                // A send that fails means the peer is gone, as a failed read.
                Poll::Ready(Err(_)) => return Poll::Ready(Ok(())),
                Poll::Pending => {}
            }

            if !progressed {
                return Poll::Pending;
            }
        }
    }

    /// Route one received frame: a reply to its waiting call, a request to
    /// the queue.
    fn receive(&mut self, frame: &[u8]) -> Result<(), RuntimeError> {
        let mut calls = self.shared.calls.lock().unwrap();
        match calls.state.receive(&self.shared.framing, frame)? {
            Event::Reply { request_id, envelope } => {
                if let Some(waiter) = calls.waiting.remove(&request_id) {
                    // The caller may have given up meanwhile.
                    let _ = waiter.send(envelope.into());
                }
            }
            Event::Stale => {}
            Event::Request { protocol, request, one_way } => {
                // By number, or, when the framing names only the call, the
                // protocol that has a call by that name.
                let numbers = match protocol {
                    Some(number) => usize::from(number)..usize::from(number) + 1,
                    None => 0..self.providers.len(),
                };
                let found = numbers.into_iter().find_map(|number| {
                    let provider = self.providers.get(number)?.as_ref()?;
                    let index = call_index(request.call, provider.calls())?;
                    Some((ProtocolNumber::try_from(number).ok()?, Arc::clone(provider), index))
                });
                self.queue.push_back(Incoming {
                    protocol: found.as_ref().map(|(number, ..)| *number).or(protocol).unwrap_or_default(),
                    request_id: request.request_id,
                    one_way,
                    target: found.map(|(_, provider, index)| (provider, index)),
                    params: request.params.to_vec(),
                });
            }
        }
        Ok(())
    }

    /// Start queued requests while fewer than `limit` run, or while every
    /// running handler waits on the peer (the next may be what it waits for).
    fn start_queued(&mut self) -> bool {
        let mut started = false;
        while !self.queue.is_empty() {
            let running = self.running.len();
            let all_waiting = self.shared.handlers_waiting.load(Ordering::SeqCst) >= running;
            if running >= self.limit && !all_waiting {
                break;
            }
            let incoming = self.queue.pop_front().unwrap();
            self.running.push(self.serve(incoming));
            started = true;
        }
        started
    }

    fn serve(&self, incoming: Incoming<W, F>) -> BoxFuture<'static, Option<Vec<u8>>> {
        let shared = Arc::clone(&self.shared);
        Box::pin(async move {
            let answer = match incoming.target {
                Some((provider, index)) => {
                    let peer = AsyncHandle {
                        shared: Arc::clone(&shared),
                        from_handler: true,
                    };
                    provider
                        .dispatch(Kind::Id(index), incoming.params, peer)
                        .await
                        .unwrap_or_else(|_| Answer::fault())
                }
                // A protocol or call this side doesn't have: the caller
                // hears a fault.
                None => Answer::fault(),
            };
            if incoming.one_way {
                return None;
            }
            let envelope = answer.envelope()?;
            let mut frame = Vec::new();
            shared
                .framing
                .encode_reply(incoming.protocol, incoming.request_id, &envelope, &mut frame);
            Some(frame)
        })
    }

    fn poll_write(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
        while !self.outbox.is_empty() {
            match Pin::new(&mut self.transport).poll_ready(cx) {
                Poll::Ready(Ok(())) => {}
                other => return other,
            }
            let frame = self.outbox.pop_front().unwrap();
            Pin::new(&mut self.transport).start_send(frame)?;
            self.unflushed = true;
        }
        if self.unflushed {
            match Pin::new(&mut self.transport).poll_flush(cx) {
                Poll::Ready(Ok(())) => self.unflushed = false,
                other => return other,
            }
        }
        Poll::Ready(Ok(()))
    }
}

// ── handles ────────────────────────────────────────────────────────────────

/// A handle to an [`AsyncConnection`]: call the peer from any task. Cheap
/// to clone. What a handler is given is one too, whose calls count as
/// nested.
pub struct AsyncHandle<W, F = Datagram2Framing> {
    shared: Arc<Shared<W, F>>,
    from_handler: bool,
}

impl<W, F> Clone for AsyncHandle<W, F> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            from_handler: self.from_handler,
        }
    }
}

impl<W, F> AsyncHandle<W, F>
where
    W: WireFormat + Send + Sync + 'static,
    F: ConnectionFraming + Send + Sync + 'static,
{
    /// End the connection: [`run`](AsyncConnection::run) sends what's
    /// queued, closes the transport and returns.
    pub fn close(&self) {
        let _ = self.shared.commands.unbounded_send(Command::Close);
    }

    /// Frame a call and queue it for sending.
    fn start<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        expects_reply: bool,
    ) -> Result<Option<Started<W, F>>, RuntimeError> {
        let mut frame = Vec::new();
        let mut calls = self.shared.calls.lock().unwrap();
        if !calls.open {
            return Err(RuntimeError::Transport);
        }
        let request_id = calls.state.encode_request(
            &self.shared.framing,
            protocol,
            call,
            params,
            &self.shared.format,
            expects_reply,
            &mut frame,
        )?;
        let waiting = expects_reply.then(|| {
            let (tx, rx) = oneshot::channel();
            calls.waiting.insert(request_id, tx);
            if self.from_handler {
                // Counted before the frame goes out, so the driver sees it
                // when it next decides whether to start a queued request.
                self.shared.handlers_waiting.fetch_add(1, Ordering::SeqCst);
            }
            let guard = Waiting {
                shared: Arc::clone(&self.shared),
                request_id,
                counted: self.from_handler,
                done: false,
            };
            (guard, rx)
        });
        drop(calls);
        if self.shared.commands.unbounded_send(Command::Frame(frame)).is_err() {
            // The driver is gone; `waiting`'s guard cleans up.
            return Err(RuntimeError::Transport);
        }
        Ok(waiting)
    }

    fn call_inner<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Option<Duration>,
    ) -> impl Future<Output = Result<OwnedEnvelope, RuntimeError>> + Send {
        let started = self.start(protocol, call, params, true);
        let sleep = timeout.and_then(|t| self.shared.timer.get().map(|timer| timer(t)));
        async move {
            let (mut guard, reply) = started?.expect("a call that expects a reply waits on it");
            let reply = match sleep {
                None => reply.await,
                Some(sleep) => match select(reply, sleep).await {
                    Either::Left((reply, _)) => reply,
                    Either::Right(_) => return Err(RuntimeError::Timeout),
                },
            };
            // A dropped sender means the connection ended.
            let reply = reply.map_err(|_| RuntimeError::Transport)?;
            guard.done = true;
            Ok(reply)
        }
    }
}

impl<W, F> AsyncPeer for AsyncHandle<W, F>
where
    W: WireFormat + Send + Sync + 'static,
    F: ConnectionFraming + Send + Sync + 'static,
{
    type Format = W;

    fn format(&self) -> &W {
        &self.shared.format
    }

    fn id(&self) -> ConnectionId {
        self.shared.id
    }

    fn call<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> impl Future<Output = Result<OwnedEnvelope, RuntimeError>> + Send {
        self.call_inner(protocol, call, params, None)
    }

    fn call_with_timeout<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Duration,
    ) -> impl Future<Output = Result<OwnedEnvelope, RuntimeError>> + Send {
        self.call_inner(protocol, call, params, Some(timeout))
    }

    fn notify<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(), RuntimeError> {
        self.start(protocol, call, params, false).map(|_| ())
    }

    fn extensions(&self) -> MutexGuard<'_, Extensions> {
        self.shared.extensions.lock().unwrap()
    }
}

/// A sent call's guard, and where its reply arrives.
type Started<W, F> = (Waiting<W, F>, oneshot::Receiver<OwnedEnvelope>);

/// A call waiting on its reply. Dropped before the reply came (a timeout, a
/// dropped future), it gives the call up, so a late reply is stale.
struct Waiting<W, F> {
    shared: Arc<Shared<W, F>>,
    request_id: u64,
    counted: bool,
    done: bool,
}

impl<W, F> Drop for Waiting<W, F> {
    fn drop(&mut self) {
        if !self.done {
            let mut calls = self.shared.calls.lock().unwrap();
            calls.state.abandon(self.request_id);
            calls.waiting.remove(&self.request_id);
        }
        if self.counted {
            self.shared.handlers_waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

// ── tokio ──────────────────────────────────────────────────────────────────

/// Tokio's TCP as an [`AsyncTransport`], and its clock as a [`Timer`].
#[cfg(feature = "tokio")]
pub mod tokio {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use futures_util::{Sink, Stream};
    use ::tokio::net::{TcpStream, ToSocketAddrs};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    use super::Timer;
    use crate::contract::stream::{MAX_FRAME, PREFIX_LEN};
    use crate::contract::RuntimeError;

    /// An [`AsyncTransport`](super::AsyncTransport) over a tokio TCP stream,
    /// framed as the blocking [`Tcp`](crate::transport::Tcp) is (`[len: u32
    /// LE][frame]`, [`contract::stream`](crate::contract::stream)), so the two
    /// talk to each other.
    pub struct Tcp {
        framed: Framed<TcpStream, LengthDelimitedCodec>,
    }

    impl Tcp {
        /// Wrap an established stream (e.g. from `TcpListener::accept`).
        pub fn new(stream: TcpStream) -> Self {
            let codec = LengthDelimitedCodec::builder()
                .length_field_length(PREFIX_LEN)
                .little_endian()
                .max_frame_length(MAX_FRAME)
                .new_codec();
            Self {
                framed: Framed::new(stream, codec),
            }
        }

        /// Connect to `addr`.
        pub async fn connect(addr: impl ToSocketAddrs) -> Result<Self, RuntimeError> {
            TcpStream::connect(addr)
                .await
                .map(Self::new)
                .map_err(|_| RuntimeError::Transport)
        }

        /// The wrapped stream.
        pub fn stream(&self) -> &TcpStream {
            self.framed.get_ref()
        }
    }

    impl Stream for Tcp {
        type Item = Result<Vec<u8>, RuntimeError>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Pin::new(&mut self.framed).poll_next(cx).map(|frame| {
                frame.map(|frame| frame.map(|bytes| bytes.to_vec()).map_err(|_| RuntimeError::Transport))
            })
        }
    }

    impl Sink<Vec<u8>> for Tcp {
        type Error = RuntimeError;

        fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
            Sink::<Bytes>::poll_ready(Pin::new(&mut self.framed), cx).map_err(|_| RuntimeError::Transport)
        }

        fn start_send(mut self: Pin<&mut Self>, frame: Vec<u8>) -> Result<(), RuntimeError> {
            Pin::new(&mut self.framed)
                .start_send(Bytes::from(frame))
                .map_err(|_| RuntimeError::Transport)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
            Sink::<Bytes>::poll_flush(Pin::new(&mut self.framed), cx).map_err(|_| RuntimeError::Transport)
        }

        fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
            Sink::<Bytes>::poll_close(Pin::new(&mut self.framed), cx).map_err(|_| RuntimeError::Transport)
        }
    }

    /// Tokio's clock, for [`AsyncConnection::set_timer`](super::AsyncConnection::set_timer).
    pub fn timer() -> Timer {
        Arc::new(|duration| Box::pin(::tokio::time::sleep(duration)))
    }
}
