//! The byte-frame transport. Message-oriented — `send` / `recv` move whole
//! request / response frames (see [`wire`](crate::wire)). Sync, to pair with
//! the sync [`Dispatch`](crate::contract::Dispatch) and [`Server`](crate::serve::Server).

use alloc::vec::Vec;

use crate::contract::RuntimeError;

/// A duplex frame channel. One end's `send` is the other end's `recv`.
pub trait Transport {
    /// Send one frame.
    fn send(&mut self, frame: &[u8]) -> Result<(), RuntimeError>;

    /// Receive the next frame into `buf` (the caller clears and reuses it).
    /// `Err(RuntimeError::Transport)` once the peer is gone.
    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), RuntimeError>;
}

#[cfg(feature = "std")]
mod in_memory {
    use std::sync::mpsc::{channel, Receiver, Sender};

    use super::{Transport, Vec};
    use crate::contract::RuntimeError;

    /// An in-process [`Transport`] — for tests, examples, and same-binary
    /// consumer/provider setups.
    pub struct InMemory {
        tx: Sender<Vec<u8>>,
        rx: Receiver<Vec<u8>>,
    }

    /// A crossed pair: what `a` sends, `b` receives, and vice versa.
    pub fn duplex() -> (InMemory, InMemory) {
        let (a_tx, a_rx) = channel();
        let (b_tx, b_rx) = channel();
        (InMemory { tx: a_tx, rx: b_rx }, InMemory { tx: b_tx, rx: a_rx })
    }

    impl Transport for InMemory {
        fn send(&mut self, frame: &[u8]) -> Result<(), RuntimeError> {
            self.tx
                .send(frame.to_vec())
                .map_err(|_| RuntimeError::Transport)
        }

        fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), RuntimeError> {
            let frame = self.rx.recv().map_err(|_| RuntimeError::Transport)?;
            buf.clear();
            buf.extend_from_slice(&frame);
            Ok(())
        }
    }
}

#[cfg(feature = "std")]
pub use in_memory::{duplex, InMemory};
