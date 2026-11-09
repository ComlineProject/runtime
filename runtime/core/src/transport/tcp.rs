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
    fn recv_timeout(&mut self, buf: &mut Vec<u8>, timeout: Duration) -> Result<bool, RuntimeError> {
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
        let timed_out =
            |e: &std::io::Error| matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut);
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
