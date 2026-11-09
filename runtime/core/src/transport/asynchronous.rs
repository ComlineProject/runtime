use std::pin::Pin;
use std::task::{Context, Poll};

use futures_channel::mpsc;
use futures_util::{Sink, Stream, StreamExt};

use crate::contract::stream::DEFAULT_MAX_FRAME;
use crate::contract::RuntimeError;

/// An async frame channel: a [`Stream`] of received frames (ending, or
/// yielding an error, once the peer is gone) and a [`Sink`] to send them.
pub trait AsyncTransport:
    Stream<Item = Result<Vec<u8>, RuntimeError>>
    + Sink<Vec<u8>, Error = RuntimeError>
    + Send
    + Unpin
    + 'static
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
///
/// Like the blocking [`InMemory`](crate::transport::InMemory), it holds
/// frames to a limit, by default [`DEFAULT_MAX_FRAME`], as a TCP transport
/// does: a send over it is refused, and a received frame over it is refused
/// and closes this end. It has no ceiling of its own.
pub struct AsyncInMemory {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    max_frame: usize,
    closed: bool,
}

/// A crossed pair: what `a` sends, `b` receives, and vice versa.
pub fn async_duplex() -> (AsyncInMemory, AsyncInMemory) {
    let (a_tx, a_rx) = mpsc::unbounded();
    let (b_tx, b_rx) = mpsc::unbounded();
    (
        AsyncInMemory::new(a_tx, b_rx),
        AsyncInMemory::new(b_tx, a_rx),
    )
}

impl AsyncInMemory {
    fn new(tx: mpsc::UnboundedSender<Vec<u8>>, rx: mpsc::UnboundedReceiver<Vec<u8>>) -> Self {
        Self {
            tx,
            rx,
            max_frame: DEFAULT_MAX_FRAME,
            closed: false,
        }
    }

    /// The same end, holding frames to `max_frame` bytes instead.
    pub fn with_max_frame(mut self, max_frame: usize) -> Self {
        self.max_frame = max_frame;
        self
    }
}

impl Stream for AsyncInMemory {
    type Item = Result<Vec<u8>, RuntimeError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.closed {
            return Poll::Ready(Some(Err(RuntimeError::Transport)));
        }
        let max_frame = self.max_frame;
        match self.rx.poll_next_unpin(cx) {
            Poll::Ready(Some(frame)) if frame.len() > max_frame => {
                self.closed = true;
                Poll::Ready(Some(Err(RuntimeError::FrameTooLarge)))
            }
            other => other.map(|frame| frame.map(Ok)),
        }
    }
}

impl Sink<Vec<u8>> for AsyncInMemory {
    type Error = RuntimeError;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, frame: Vec<u8>) -> Result<(), RuntimeError> {
        if self.closed {
            return Err(RuntimeError::Transport);
        }
        if frame.len() > self.max_frame {
            return Err(RuntimeError::FrameTooLarge);
        }
        self.tx
            .unbounded_send(frame)
            .map_err(|_| RuntimeError::Transport)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), RuntimeError>> {
        self.tx.close_channel();
        Poll::Ready(Ok(()))
    }
}
