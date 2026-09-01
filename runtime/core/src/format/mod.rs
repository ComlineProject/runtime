//! [`WireFormat`](crate::contract::WireFormat) implementations.
//!
//! `std`-gated for now — `rmp-serde` / `serde_json` want `std::io`. A `no_std`
//! path comes later.

use std::io::Write;

use crate::contract::BufMut;

mod json;
mod msgpack;

pub use json::Json;
pub use msgpack::MsgPack;

/// `std::io::Write` over a [`BufMut`], so an `encode` serialises straight into
/// the caller's buffer with no intermediate `Vec`.
pub(crate) struct BufWriter<'a>(pub &'a mut dyn BufMut);

impl Write for BufWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.put_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
