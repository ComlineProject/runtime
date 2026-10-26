//! The byte-frame transport. Message-oriented — `send` / `recv` move whole
//! request / response frames (see [`wire`](crate::wire)). Sync, to pair with
//! the sync [`Dispatch`](crate::contract::Dispatch) and [`Server`](crate::serve::Server).
//!
//! A datagram medium (`InMemory`, and later UDP) carries one frame per message
//! natively. A byte stream ([`Tcp`]) has no message boundaries, so it adds a
//! `u32` length prefix per frame.
//!
//! **Frame limits.** A transport owns the limit on a frame's size
//! ([`Transport::max_frame`]): by default
//! [`DEFAULT_MAX_FRAME`] (16 MiB), the same in every Comline runtime, and
//! configurable on each transport here (`with_max_frame`) up to what its
//! framing can express.
//! - Sending a frame over it is [`RuntimeError::FrameTooLarge`], with nothing
//!   written: the transport keeps working.
//! - Receiving one is [`RuntimeError::FrameTooLarge`] too, refused before
//!   anything is allocated for it. A byte stream is out of step past it, so
//!   the transport closes, and later calls return [`RuntimeError::Transport`].

use core::time::Duration;

use alloc::vec::Vec;

use crate::contract::stream::DEFAULT_MAX_FRAME;
use crate::contract::RuntimeError;

/// A duplex frame channel. One end's `send` is the other end's `recv`.
pub trait Transport {
    /// Send one frame.
    fn send(&mut self, frame: &[u8]) -> Result<(), RuntimeError>;

    /// Receive the next frame into `buf` (the caller clears and reuses it).
    /// `Err(RuntimeError::Transport)` once the peer is gone.
    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), RuntimeError>;

    /// Receive the next frame, waiting at most `timeout`. `Ok(true)` — a
    /// frame was read into `buf`; `Ok(false)` — the timeout elapsed first.
    ///
    /// The default **blocks**, ignoring `timeout` — a transport with no clock
    /// still works, it just can't honour a per-call deadline. `std` transports
    /// override this. [`Client::call_with_timeout`](crate::client::Client::call_with_timeout)
    /// is the caller.
    fn recv_timeout(&mut self, buf: &mut Vec<u8>, timeout: Duration) -> Result<bool, RuntimeError> {
        let _ = timeout;
        self.recv(buf)?;
        Ok(true)
    }

    /// The largest frame, in bytes, this transport sends or receives. A
    /// connection checks an outgoing frame against it before spending a
    /// request id, so one oversized call fails alone, and answers a reply
    /// over it with a fault.
    ///
    /// The default is [`DEFAULT_MAX_FRAME`]; a transport that enforces
    /// another limit returns it.
    fn max_frame(&self) -> usize {
        DEFAULT_MAX_FRAME
    }
}

#[cfg(feature = "std")]
mod in_memory {
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
}

#[cfg(feature = "std")]
pub use in_memory::{duplex, InMemory};

#[cfg(feature = "std")]
mod tcp {
    use std::io::{ErrorKind, Read, Write};
    use std::net::{Shutdown, TcpStream, ToSocketAddrs};

    use super::{Duration, Transport, Vec};
    use crate::contract::stream::{self, DEFAULT_MAX_FRAME, PREFIX_CEILING, PREFIX_LEN};
    use crate::contract::RuntimeError;

    /// A [`Transport`] over a TCP byte stream. Each frame is `[len: u32 LE]
    /// [frame bytes]` ([`contract::stream`](crate::contract::stream)); `recv`
    /// reads exactly one.
    ///
    /// Frames are held to a limit, by default
    /// [`DEFAULT_MAX_FRAME`](crate::contract::stream::DEFAULT_MAX_FRAME)
    /// ([`with_max_frame`](Self::with_max_frame) sets another):
    /// - `send` refuses a frame over it with
    ///   [`RuntimeError::FrameTooLarge`] and writes nothing;
    /// - `recv` refuses a length prefix over it with
    ///   [`RuntimeError::FrameTooLarge`] before allocating or reading the
    ///   body. The stream is out of step past that prefix, so the `Tcp`
    ///   shuts the socket down, and every later call returns
    ///   [`RuntimeError::Transport`].
    pub struct Tcp {
        stream: TcpStream,
        len: [u8; PREFIX_LEN],
        max_frame: usize,
        closed: bool,
    }

    impl Tcp {
        /// Wrap an established stream (e.g. from `TcpListener::accept`).
        pub fn new(stream: TcpStream) -> Self {
            Self {
                stream,
                len: [0; PREFIX_LEN],
                max_frame: DEFAULT_MAX_FRAME,
                closed: false,
            }
        }

        /// Connect to `addr`.
        pub fn connect(addr: impl ToSocketAddrs) -> Result<Self, RuntimeError> {
            TcpStream::connect(addr)
                .map(Self::new)
                .map_err(|_| RuntimeError::Transport)
        }

        /// Hold frames to `max_frame` bytes instead of
        /// [`DEFAULT_MAX_FRAME`](crate::contract::stream::DEFAULT_MAX_FRAME),
        /// lower or higher. A limit above what the `u32` prefix can declare
        /// ([`PREFIX_CEILING`](crate::contract::stream::PREFIX_CEILING)) is
        /// refused with [`RuntimeError::FrameTooLarge`].
        ///
        /// Both ends need a limit at least as large as the frames they
        /// exchange: a peer refusing a frame closes the connection.
        pub fn with_max_frame(mut self, max_frame: usize) -> Result<Self, RuntimeError> {
            if max_frame > PREFIX_CEILING {
                return Err(RuntimeError::FrameTooLarge);
            }
            self.max_frame = max_frame;
            Ok(self)
        }

        /// The wrapped stream.
        pub fn stream(&self) -> &TcpStream {
            &self.stream
        }

        /// The frame length a received prefix declares, or, over the limit,
        /// the end of this transport.
        fn frame_len(&mut self) -> Result<usize, RuntimeError> {
            match stream::frame_len(self.len, self.max_frame) {
                Some(len) => Ok(len),
                None => {
                    self.closed = true;
                    let _ = self.stream.shutdown(Shutdown::Both);
                    Err(RuntimeError::FrameTooLarge)
                }
            }
        }

        fn open(&self) -> Result<(), RuntimeError> {
            if self.closed {
                Err(RuntimeError::Transport)
            } else {
                Ok(())
            }
        }
    }

    impl Transport for Tcp {
        fn send(&mut self, frame: &[u8]) -> Result<(), RuntimeError> {
            self.open()?;
            let mut prefix = [0u8; PREFIX_LEN];
            stream::put_prefix(
                frame.len(),
                self.max_frame,
                &mut crate::contract::SliceBuf::new(&mut prefix),
            )?;
            self.stream
                .write_all(&prefix)
                .and_then(|()| self.stream.write_all(frame))
                .and_then(|()| self.stream.flush())
                .map_err(|_| RuntimeError::Transport)
        }

        fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), RuntimeError> {
            self.open()?;
            self.stream
                .read_exact(&mut self.len)
                .map_err(|_| RuntimeError::Transport)?;
            let len = self.frame_len()?;
            buf.clear();
            buf.resize(len, 0);
            self.stream
                .read_exact(buf)
                .map_err(|_| RuntimeError::Transport)
        }

        /// Best-effort: sets a read timeout for the length-prefix and body
        /// reads, then clears it. A timeout part-way through a frame leaves the
        /// stream desynced — treat a timed-out call as fatal to the connection
        /// and drop the [`Tcp`].
        fn recv_timeout(
            &mut self,
            buf: &mut Vec<u8>,
            timeout: Duration,
        ) -> Result<bool, RuntimeError> {
            self.open()?;
            self.stream
                .set_read_timeout(Some(timeout))
                .map_err(|_| RuntimeError::Transport)?;
            let outcome = self.recv_within(buf);
            let _ = self.stream.set_read_timeout(None);
            outcome
        }

        fn max_frame(&self) -> usize {
            self.max_frame
        }
    }

    impl Tcp {
        fn recv_within(&mut self, buf: &mut Vec<u8>) -> Result<bool, RuntimeError> {
            let timed_out = |e: &std::io::Error| {
                matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
            };
            if let Err(e) = self.stream.read_exact(&mut self.len) {
                return if timed_out(&e) {
                    Ok(false)
                } else {
                    Err(RuntimeError::Transport)
                };
            }
            let len = self.frame_len()?;
            buf.clear();
            buf.resize(len, 0);
            match self.stream.read_exact(buf) {
                Ok(()) => Ok(true),
                Err(e) if timed_out(&e) => Ok(false),
                Err(_) => Err(RuntimeError::Transport),
            }
        }
    }
}

#[cfg(feature = "std")]
pub use tcp::Tcp;
