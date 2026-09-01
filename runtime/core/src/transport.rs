//! The byte-frame transport. Message-oriented — `send` / `recv` move whole
//! request / response frames (see [`wire`](crate::wire)). Sync, to pair with
//! the sync [`Dispatch`](crate::contract::Dispatch) and [`Server`](crate::serve::Server).
//!
//! A datagram medium (`InMemory`, and later UDP) carries one frame per message
//! natively. A byte stream ([`Tcp`]) has no message boundaries, so it adds a
//! `u32` length prefix per frame.

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

#[cfg(feature = "std")]
mod tcp {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};

    use super::{Transport, Vec};
    use crate::contract::RuntimeError;

    /// Reject a length prefix larger than this before allocating for it — a
    /// peer claiming a 4 GiB frame should not cost 4 GiB. Frames are datagrams
    /// (one call), so the ceiling is generous.
    const MAX_FRAME: usize = 16 * 1024 * 1024;

    /// A [`Transport`] over a TCP byte stream. Each frame is `[len: u32 LE]
    /// [frame bytes]`; `recv` reads exactly one.
    pub struct Tcp {
        stream: TcpStream,
        len: [u8; 4],
    }

    impl Tcp {
        /// Wrap an established stream (e.g. from `TcpListener::accept`).
        pub fn new(stream: TcpStream) -> Self {
            Self {
                stream,
                len: [0; 4],
            }
        }

        /// Connect to `addr`.
        pub fn connect(addr: impl ToSocketAddrs) -> Result<Self, RuntimeError> {
            TcpStream::connect(addr)
                .map(Self::new)
                .map_err(|_| RuntimeError::Transport)
        }

        /// The wrapped stream.
        pub fn stream(&self) -> &TcpStream {
            &self.stream
        }
    }

    impl Transport for Tcp {
        fn send(&mut self, frame: &[u8]) -> Result<(), RuntimeError> {
            let len = u32::try_from(frame.len()).map_err(|_| RuntimeError::Framing)?;
            self.stream
                .write_all(&len.to_le_bytes())
                .and_then(|()| self.stream.write_all(frame))
                .and_then(|()| self.stream.flush())
                .map_err(|_| RuntimeError::Transport)
        }

        fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), RuntimeError> {
            self.stream
                .read_exact(&mut self.len)
                .map_err(|_| RuntimeError::Transport)?;
            let len = u32::from_le_bytes(self.len) as usize;
            if len > MAX_FRAME {
                return Err(RuntimeError::Framing);
            }
            buf.clear();
            buf.resize(len, 0);
            self.stream
                .read_exact(buf)
                .map_err(|_| RuntimeError::Transport)
        }
    }
}

#[cfg(feature = "std")]
pub use tcp::Tcp;
