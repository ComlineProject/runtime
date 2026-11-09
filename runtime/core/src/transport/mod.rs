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

#[cfg(feature = "async")]
mod asynchronous;
#[cfg(feature = "std")]
mod in_memory;
#[cfg(feature = "std")]
mod tcp;
/// Tokio's TCP as an [`AsyncTransport`], and its clock as a [`Timer`](crate::connection::Timer).
#[cfg(feature = "tokio")]
pub mod tokio;

#[cfg(feature = "async")]
pub use asynchronous::{async_duplex, AsyncInMemory, AsyncTransport};
#[cfg(feature = "std")]
pub use in_memory::{duplex, InMemory};
#[cfg(feature = "std")]
pub use tcp::Tcp;
