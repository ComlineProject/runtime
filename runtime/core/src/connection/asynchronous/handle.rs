use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use futures_channel::oneshot;
use futures_util::future::{select, Either};
use serde::Serialize;

use crate::connection::Extensions;
use crate::contract::{
    Call, ConnectionFraming, ConnectionId, Datagram2Framing, ProtocolNumber, RuntimeError,
    WireFormat,
};

use super::driver::{Command, Shared};
use super::peer::{AsyncPeer, OwnedEnvelope};

/// A handle to an [`AsyncConnection`](crate::connection::AsyncConnection): call the peer from any task. Cheap
/// to clone. What a handler is given is one too, whose calls count as
/// nested.
pub struct AsyncHandle<W, F = Datagram2Framing> {
    pub(super) shared: Arc<Shared<W, F>>,
    pub(super) from_handler: bool,
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
    /// End the connection: [`run`](crate::connection::AsyncConnection::run) sends what's
    /// queued, closes the transport and returns.
    pub fn close(&self) {
        let _ = self.shared.commands.unbounded_send(Command::Close);
    }

    /// Frame a call and queue it for sending. One over the connection's
    /// limit is [`RuntimeError::FrameTooLarge`], with nothing spent.
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
        if frame.len() > self.shared.max_frame() {
            // Refused before it waits or is queued: this call fails alone.
            calls.state.retract(request_id);
            return Err(RuntimeError::FrameTooLarge);
        }
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
        if self
            .shared
            .commands
            .unbounded_send(Command::Frame(frame))
            .is_err()
        {
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
