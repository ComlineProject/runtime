mod driver;
mod handle;
mod peer;

pub use handle::AsyncHandle;
pub use peer::{Answer, AsyncDispatch, AsyncPeer, BoxFuture, OwnedEnvelope, Timer};

use std::collections::VecDeque;
use std::future::poll_fn;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use alloc::collections::BTreeMap;
use futures_channel::mpsc;
use futures_util::stream::FuturesUnordered;

use crate::connection::extensions::next_connection_id;
use crate::connection::{Extensions, State};
use crate::contract::stream::DEFAULT_MAX_FRAME;
use crate::contract::{
    ConnectionFraming, ConnectionId, Datagram2Framing, ProtocolNumber, RuntimeError, WireFormat,
};
use crate::handshake::{AsyncHandshake, ComlineHandshake, Facts};
use crate::transport::AsyncTransport;

use driver::{Calls, Command, Driver, Providers, Shared};

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
        Self::connect_with(
            transport,
            format,
            Datagram2Framing,
            &mut ComlineHandshake,
            ir_hash,
        )
        .await
    }

    /// Answer: run the `comline@1` handshake's provider side. It's
    /// symmetric, so the same exchange as [`connect`](Self::connect).
    pub async fn accept(transport: T, format: W, ir_hash: u64) -> Result<Self, RuntimeError> {
        Self::accept_with(
            transport,
            format,
            Datagram2Framing,
            &mut ComlineHandshake,
            ir_hash,
        )
        .await
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
                id: next_connection_id(),
                calls: Mutex::new(Calls {
                    state: State::unordered(),
                    waiting: BTreeMap::new(),
                    open: true,
                }),
                commands: commands_tx,
                extensions: Mutex::new(Extensions::default()),
                handlers_waiting: AtomicUsize::new(0),
                timer: OnceLock::new(),
                max_frame: AtomicUsize::new(DEFAULT_MAX_FRAME),
            }),
            commands,
            providers: Vec::new(),
            limit: 1,
        }
    }

    /// The `comline@1` exchange over `transport` with another framing:
    /// each end sends its handshake frame and checks the peer's. The
    /// exchange is symmetric, so this is [`connect_with`](Self::connect_with)
    /// (or [`accept_with`](Self::accept_with)) with [`ComlineHandshake`].
    pub async fn handshake(
        transport: T,
        format: W,
        framing: F,
        ir_hash: u64,
    ) -> Result<Self, RuntimeError> {
        Self::connect_with(transport, format, framing, &mut ComlineHandshake, ir_hash).await
    }

    /// Dial with `handshake` ([`AsyncHandshake::initiate`]).
    pub async fn connect_with<H: AsyncHandshake>(
        mut transport: T,
        format: W,
        framing: F,
        handshake: &mut H,
        ir_hash: u64,
    ) -> Result<Self, RuntimeError> {
        let local = Facts::new(ir_hash, format.name(), framing.name());
        handshake.initiate(&mut transport, &local).await?;
        Ok(Self::with_framing_unverified(transport, format, framing))
    }

    /// Answer with `handshake` ([`AsyncHandshake::respond`]).
    pub async fn accept_with<H: AsyncHandshake>(
        mut transport: T,
        format: W,
        framing: F,
        handshake: &mut H,
        ir_hash: u64,
    ) -> Result<Self, RuntimeError> {
        let local = Facts::new(ir_hash, format.name(), framing.name());
        handshake.respond(&mut transport, &local).await?;
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

    /// The largest frame, in bytes, this side sends; by default
    /// [`DEFAULT_MAX_FRAME`]. Set it to the transport's own limit (the tokio
    /// [`Tcp::with_max_frame`](crate::transport::tokio::Tcp::with_max_frame),
    /// [`AsyncInMemory::with_max_frame`](crate::transport::AsyncInMemory::with_max_frame)) when that isn't the default.
    ///
    /// - A call over it fails with [`RuntimeError::FrameTooLarge`] before
    ///   anything is queued or a request id is spent; the connection keeps
    ///   working.
    /// - A handler's reply over it is sent as a fault, so the caller hears
    ///   [`RuntimeError::Provider`]. This runtime doesn't log; a provider
    ///   that wants to know checks its replies' size itself.
    ///
    /// A frame the transport refuses to send all the same (its limit is
    /// lower than this) ends [`run`](Self::run) with
    /// [`RuntimeError::FrameTooLarge`].
    pub fn set_max_frame(&mut self, max_frame: usize) {
        self.shared.max_frame.store(max_frame, Ordering::Relaxed);
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
    /// [`close`](AsyncHandle::close)s it, then `Ok(())`. Calls still waiting
    /// then fail with [`RuntimeError::Transport`].
    ///
    /// It ends with the error instead when the transport fails to receive
    /// (a frame over its limit is [`RuntimeError::FrameTooLarge`]), or a
    /// received frame is malformed ([`RuntimeError::Framing`]) or the peer
    /// says it couldn't read a request ([`RuntimeError::PeerRefused`]). A failed
    /// send means the peer is gone, as a closed stream does.
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
