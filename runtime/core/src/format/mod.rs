//! [`WireFormat`](crate::contract::WireFormat) implementations.
//!
//! [`Packed`] (`packed@1`, the default) needs neither `std` nor `alloc`.
//! [`Json`] and [`MsgPack`] are `std`-gated: `serde_json` / `rmp-serde` want
//! `std::io`.

#[cfg(feature = "std")]
use std::io::Write;

#[cfg(feature = "std")]
use crate::contract::BufMut;

#[cfg(feature = "std")]
mod json;
#[cfg(feature = "std")]
mod msgpack;
mod packed;

#[cfg(feature = "std")]
pub use json::Json;
#[cfg(feature = "std")]
pub use msgpack::MsgPack;
pub use packed::Packed;

#[cfg(feature = "std")]
/// `std::io::Write` over a [`BufMut`], so an `encode` serialises straight into
/// the caller's buffer with no intermediate `Vec`.
pub(crate) struct BufWriter<'a>(pub &'a mut dyn BufMut);

#[cfg(feature = "std")]
impl Write for BufWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.put_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
