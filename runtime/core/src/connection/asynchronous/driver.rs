use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use alloc::collections::BTreeMap;
use futures_channel::{mpsc, oneshot};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;

use crate::connection::blocking::call_index;
use crate::connection::{Event, Extensions, State};
use crate::contract::{
    ConnectionFraming, ConnectionId, Envelope, Kind, ProtocolNumber, RuntimeError, WireFormat,
};

use crate::transport::AsyncTransport;

use super::handle::AsyncHandle;
use super::peer::{Answer, AsyncDispatch, BoxFuture, OwnedEnvelope, Timer};

/// What the driver and every handle share.
pub(super) struct Shared<W, F> {
    pub(super) format: W,
    pub(super) framing: F,
    pub(super) id: ConnectionId,
    pub(super) calls: Mutex<Calls>,
    pub(super) commands: mpsc::UnboundedSender<Command>,
    pub(super) extensions: Mutex<Extensions>,
    /// Calls to the peer that handlers are waiting on.
    pub(super) handlers_waiting: AtomicUsize,
    pub(super) timer: OnceLock<Timer>,
    /// The largest frame this side sends.
    pub(super) max_frame: AtomicUsize,
}

impl<W, F> Shared<W, F> {
    pub(super) fn max_frame(&self) -> usize {
        self.max_frame.load(Ordering::Relaxed)
    }
}

pub(super) struct Calls {
    pub(super) state: State,
    pub(super) waiting: BTreeMap<u64, oneshot::Sender<OwnedEnvelope>>,
    pub(super) open: bool,
}

pub(super) enum Command {
    Frame(Vec<u8>),
    Close,
}

/// A protocol's provider, with its handlers' futures boxed so different
/// protocols' dispatchers fit one table.
pub(super) trait Provider<W, F>: Send + Sync {
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

pub(super) type Providers<W, F> = Vec<Option<Arc<dyn Provider<W, F>>>>;

/// A peer request waiting to start.
pub(super) struct Incoming<W, F> {
    protocol: ProtocolNumber,
    request_id: u64,
    /// The frame asked for no reply.
    one_way: bool,
    target: Option<(Arc<dyn Provider<W, F>>, u16)>,
    params: Vec<u8>,
}

pub(super) struct Driver<T, W, F> {
    pub(super) transport: T,
    pub(super) shared: Arc<Shared<W, F>>,
    pub(super) commands: mpsc::UnboundedReceiver<Command>,
    pub(super) providers: Providers<W, F>,
    pub(super) limit: usize,
    pub(super) queue: VecDeque<Incoming<W, F>>,
    /// Handlers running, each yielding its reply frame, if any.
    pub(super) running: FuturesUnordered<BoxFuture<'static, Option<Vec<u8>>>>,
    pub(super) outbox: VecDeque<Vec<u8>>,
    pub(super) unflushed: bool,
    pub(super) closing: bool,
}

impl<T, W, F> Driver<T, W, F>
where
    T: AsyncTransport,
    W: WireFormat + Send + Sync + 'static,
    F: ConnectionFraming + Send + Sync + 'static,
{
    pub(super) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
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
                        Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                        // The peer is gone.
                        Poll::Ready(None) => return Poll::Ready(Ok(())),
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
                // A send that fails means the peer is gone, as a closed
                // stream. A frame the transport refused is this side's.
                Poll::Ready(Err(RuntimeError::Transport)) => return Poll::Ready(Ok(())),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
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
            Event::Reply {
                request_id,
                envelope,
            } => {
                if let Some(waiter) = calls.waiting.remove(&request_id) {
                    // The caller may have given up meanwhile.
                    let _ = waiter.send(envelope.into());
                }
            }
            Event::Stale => {}
            Event::Request {
                protocol,
                request,
                one_way,
            } => {
                // By number, or, when the framing names only the call, the
                // protocol that has a call by that name.
                let numbers = match protocol {
                    Some(number) => usize::from(number)..usize::from(number) + 1,
                    None => 0..self.providers.len(),
                };
                let found = numbers.into_iter().find_map(|number| {
                    let provider = self.providers.get(number)?.as_ref()?;
                    let index = call_index(request.call, provider.calls())?;
                    Some((
                        ProtocolNumber::try_from(number).ok()?,
                        Arc::clone(provider),
                        index,
                    ))
                });
                self.queue.push_back(Incoming {
                    protocol: found
                        .as_ref()
                        .map(|(number, ..)| *number)
                        .or(protocol)
                        .unwrap_or_default(),
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
            shared.framing.encode_reply(
                incoming.protocol,
                incoming.request_id,
                &envelope,
                &mut frame,
            );
            if frame.len() > shared.max_frame() {
                // Too large to send: the caller hears a fault, not silence.
                frame.clear();
                shared.framing.encode_reply(
                    incoming.protocol,
                    incoming.request_id,
                    &Envelope::Fault,
                    &mut frame,
                );
            }
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
