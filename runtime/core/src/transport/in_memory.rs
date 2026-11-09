use std::sync::mpsc::{channel, Receiver, Sender};

use super::{Transport, Vec, DEFAULT_MAX_FRAME};
use crate::contract::RuntimeError;

/// An in-process [`Transport`] — for tests, examples, and same-binary
/// consumer/provider setups.
///
/// It holds frames to the same limit a [`Tcp`](super::Tcp) does, by
/// default [`DEFAULT_MAX_FRAME`], so what works in memory works over a
/// socket: a send over it is refused with nothing sent, and a received
/// frame over it (the peer's limit is higher) is refused and closes this
/// end. A channel has no ceiling of its own:
/// [`with_max_frame`](Self::with_max_frame) takes any limit.
pub struct InMemory {
    tx: Sender<Vec<u8>>,
    rx: Receiver<Vec<u8>>,
    max_frame: usize,
    closed: bool,
}

/// A crossed pair: what `a` sends, `b` receives, and vice versa.
pub fn duplex() -> (InMemory, InMemory) {
    let (a_tx, a_rx) = channel();
    let (b_tx, b_rx) = channel();
    (InMemory::new(a_tx, b_rx), InMemory::new(b_tx, a_rx))
}

impl InMemory {
    fn new(tx: Sender<Vec<u8>>, rx: Receiver<Vec<u8>>) -> Self {
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

    /// A received frame over the limit closes this end.
    fn take(&mut self, frame: Vec<u8>, buf: &mut Vec<u8>) -> Result<(), RuntimeError> {
        if frame.len() > self.max_frame {
            self.closed = true;
            return Err(RuntimeError::FrameTooLarge);
        }
        buf.clear();
        buf.extend_from_slice(&frame);
        Ok(())
    }

    /// Non-blocking receive: `Ok(true)` if a frame was read into `buf`,
    /// `Ok(false)` if the peer has sent nothing (yet). For single-threaded
    /// pumping, and for asserting a one-way call drew no reply.
    pub fn try_recv(&mut self, buf: &mut Vec<u8>) -> Result<bool, RuntimeError> {
        use std::sync::mpsc::TryRecvError;
        if self.closed {
            return Err(RuntimeError::Transport);
        }
        match self.rx.try_recv() {
            Ok(frame) => self.take(frame, buf).map(|()| true),
            Err(TryRecvError::Empty) => Ok(false),
            Err(TryRecvError::Disconnected) => Err(RuntimeError::Transport),
        }
    }
}

impl Transport for InMemory {
    fn send(&mut self, frame: &[u8]) -> Result<(), RuntimeError> {
        if self.closed {
            return Err(RuntimeError::Transport);
        }
        if frame.len() > self.max_frame {
            return Err(RuntimeError::FrameTooLarge);
        }
        self.tx
            .send(frame.to_vec())
            .map_err(|_| RuntimeError::Transport)
    }

    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), RuntimeError> {
        if self.closed {
            return Err(RuntimeError::Transport);
        }
        let frame = self.rx.recv().map_err(|_| RuntimeError::Transport)?;
        self.take(frame, buf)
    }

    fn recv_timeout(
        &mut self,
        buf: &mut Vec<u8>,
        timeout: super::Duration,
    ) -> Result<bool, RuntimeError> {
        use std::sync::mpsc::RecvTimeoutError;
        if self.closed {
            return Err(RuntimeError::Transport);
        }
        match self.rx.recv_timeout(timeout) {
            Ok(frame) => self.take(frame, buf).map(|()| true),
            Err(RecvTimeoutError::Timeout) => Ok(false),
            Err(RecvTimeoutError::Disconnected) => Err(RuntimeError::Transport),
        }
    }

    fn max_frame(&self) -> usize {
        self.max_frame
    }
}
