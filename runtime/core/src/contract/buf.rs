//! The byte sink an encode step writes into. The call system owns one and
//! reuses it — reset, not reallocated, per call.

/// A minimal, `no_std` append target. `Vec<u8>` implements it (with `alloc`);
/// [`SliceBuf`] wraps a fixed `&mut [u8]` for the no-`alloc` tier.
pub trait BufMut {
    /// Append `bytes`.
    fn put_slice(&mut self, bytes: &[u8]);

    fn put_u8(&mut self, byte: u8) {
        self.put_slice(&[byte]);
    }
    fn put_u16_le(&mut self, value: u16) {
        self.put_slice(&value.to_le_bytes());
    }
    fn put_u32_le(&mut self, value: u32) {
        self.put_slice(&value.to_le_bytes());
    }
    fn put_u64_le(&mut self, value: u64) {
        self.put_slice(&value.to_le_bytes());
    }
}

#[cfg(feature = "alloc")]
impl BufMut for alloc::vec::Vec<u8> {
    fn put_slice(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

/// A fixed `&mut [u8]` with a write cursor. A `put_*` that would exceed
/// capacity writes nothing and sets [`overflowed`](SliceBuf::overflowed) — the
/// caller checks it once after encoding rather than on every write.
pub struct SliceBuf<'a> {
    buf: &'a mut [u8],
    len: usize,
    overflowed: bool,
}

impl<'a> SliceBuf<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            len: 0,
            overflowed: false,
        }
    }

    /// The bytes written so far.
    pub fn written(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Whether a write was dropped for want of capacity.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Reset the cursor for the next call. Does not touch the backing bytes;
    /// pair with a `zeroize` pass where hardening is on.
    pub fn clear(&mut self) {
        self.len = 0;
        self.overflowed = false;
    }
}

impl BufMut for SliceBuf<'_> {
    fn put_slice(&mut self, bytes: &[u8]) {
        let Some(end) = self.len.checked_add(bytes.len()) else {
            self.overflowed = true;
            return;
        };
        if end > self.buf.len() {
            self.overflowed = true;
            return;
        }
        self.buf[self.len..end].copy_from_slice(bytes);
        self.len = end;
    }
}
