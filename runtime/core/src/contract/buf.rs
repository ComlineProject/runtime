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

    /// Whether a write was dropped for want of room. A buffer that grows
    /// never drops one.
    fn overflowed(&self) -> bool {
        false
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
    #[inline]
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            len: 0,
            overflowed: false,
        }
    }

    /// The bytes written so far.
    #[inline]
    pub fn written(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Whether a write was dropped for want of capacity.
    #[inline]
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Reset the cursor for the next call. Does not touch the backing bytes;
    /// pair with a `zeroize` pass where hardening is on.
    #[inline]
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

    fn overflowed(&self) -> bool {
        self.overflowed
    }
}

/// A [`BufMut`] that keeps nothing and only counts: [`len`](CountBuf::len) is
/// the number of bytes an encode would have written. Allocation-free.
///
/// For reporting the exact size needed after a [`SliceBuf`] overflowed: encode
/// the same frame again into a `CountBuf`. Not for the success path, which
/// writes once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CountBuf {
    len: usize,
}

impl CountBuf {
    pub const fn new() -> Self {
        Self { len: 0 }
    }

    /// The bytes counted so far. Saturates at `usize::MAX` rather than wrap.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing has been counted.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reset the count to 0.
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

impl BufMut for CountBuf {
    fn put_slice(&mut self, bytes: &[u8]) {
        self.len = self.len.saturating_add(bytes.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_buf_counts_every_put() {
        let mut count = CountBuf::new();
        assert!(count.is_empty());
        count.put_slice(b"abc");
        count.put_u8(1);
        count.put_u16_le(2);
        count.put_u32_le(3);
        count.put_u64_le(4);
        count.put_slice(&[]);
        assert_eq!(count.len(), 3 + 1 + 2 + 4 + 8);
        count.clear();
        assert_eq!(count.len(), 0);
    }

    #[test]
    fn count_buf_saturates() {
        let mut count = CountBuf {
            len: usize::MAX - 1,
        };
        count.put_slice(b"xyz");
        assert_eq!(count.len(), usize::MAX);
    }
}
