//! Frames on a byte stream: what a stream transport (TCP, a pipe, stdio) adds
//! so the frames it carries keep their boundaries.
//!
//! Each frame is `[len: u32 LE][len bytes]`. The transport owns the limit on
//! `len`: [`DEFAULT_MAX_FRAME`] unless it is configured otherwise. This module
//! declares the layout and that default once. The blocking
//! [`Tcp`](crate::transport::Tcp) and the tokio one
//! (`async_connection::tokio::Tcp`) both read them from here.
//!
//! `no_std`, allocation-free, and sans-IO: it parses and writes bytes, and
//! the host reads and writes the stream.

use crate::contract::{BufMut, RuntimeError};

/// The default frame limit, in bytes (16 MiB), not counting the prefix. Every
/// Comline runtime, in every language, uses it unless configured otherwise, so
/// peers nobody configured agree.
///
/// A transport refuses a length prefix above its limit before allocating for
/// it: a peer claiming a 4 GiB frame should not cost 4 GiB.
pub const DEFAULT_MAX_FRAME: usize = 16 * 1024 * 1024;

/// The length prefix's size in bytes: a `u32`, little-endian.
pub const PREFIX_LEN: usize = 4;

/// The largest frame the length prefix can declare: `u32::MAX` bytes (on a
/// target whose `usize` holds it). A stream transport's limit can be raised
/// up to this, and no further.
pub const PREFIX_CEILING: usize = if usize::BITS >= u32::BITS {
    u32::MAX as usize
} else {
    usize::MAX
};

/// Write the length prefix for a frame of `len` bytes. Refuses a `len` above
/// `max` (normally [`DEFAULT_MAX_FRAME`]), or one a `u32` can't hold, with
/// [`RuntimeError::FrameTooLarge`], writing nothing.
pub fn put_prefix(len: usize, max: usize, out: &mut dyn BufMut) -> Result<(), RuntimeError> {
    if len > max {
        return Err(RuntimeError::FrameTooLarge);
    }
    let len = u32::try_from(len).map_err(|_| RuntimeError::FrameTooLarge)?;
    out.put_u32_le(len);
    Ok(())
}

/// The frame length a complete prefix declares, or `None` if it is above
/// `max`.
pub fn frame_len(prefix: [u8; PREFIX_LEN], max: usize) -> Option<usize> {
    let len = usize::try_from(u32::from_le_bytes(prefix)).ok()?;
    (len <= max).then_some(len)
}

/// Where the first frame in a stream buffer is, from [`split_frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    /// No whole frame yet. `needed` is the total number of bytes, prefix
    /// included and counted from the start of the buffer, that the frame
    /// takes once the prefix is complete; `0` while the prefix itself is
    /// still incomplete.
    NeedMore { needed: usize },
    /// A whole frame: its bytes are `buf[start..start + len]`, and the caller
    /// consumes `start + len` bytes.
    Frame { start: usize, len: usize },
    /// The prefix declares a frame larger than `max`. The stream can't be
    /// resynchronised past it, so the connection should be dropped. A caller
    /// reports it as [`RuntimeError::FrameTooLarge`].
    TooLarge,
}

/// Find the first length-prefixed frame at the start of `buf`, refusing one
/// longer than `max` (normally [`DEFAULT_MAX_FRAME`]). Bytes after that frame are left
/// for the next call.
pub fn split_frame(buf: &[u8], max: usize) -> Split {
    let Some(prefix) = buf.first_chunk::<PREFIX_LEN>() else {
        return Split::NeedMore { needed: 0 };
    };
    let Some(len) = frame_len(*prefix, max) else {
        return Split::TooLarge;
    };
    let Some(total) = PREFIX_LEN.checked_add(len) else {
        return Split::TooLarge;
    };
    if buf.len() < total {
        Split::NeedMore { needed: total }
    } else {
        Split::Frame {
            start: PREFIX_LEN,
            len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::SliceBuf;

    fn prefixed(len: u32) -> [u8; PREFIX_LEN] {
        len.to_le_bytes()
    }

    #[test]
    fn max_frame_fits_the_prefix() {
        assert!(u32::try_from(DEFAULT_MAX_FRAME).is_ok());
    }

    #[test]
    fn exact_frame() {
        let buf = [3, 0, 0, 0, b'a', b'b', b'c'];
        assert_eq!(
            split_frame(&buf, DEFAULT_MAX_FRAME),
            Split::Frame { start: 4, len: 3 }
        );
    }

    #[test]
    fn partial_prefix_needs_more_without_a_size() {
        for n in 0..PREFIX_LEN {
            let buf = &[3, 0, 0, 0][..n];
            assert_eq!(
                split_frame(buf, DEFAULT_MAX_FRAME),
                Split::NeedMore { needed: 0 },
                "{n} prefix bytes"
            );
        }
    }

    #[test]
    fn partial_body_needs_the_total() {
        let buf = [3, 0, 0, 0, b'a'];
        assert_eq!(
            split_frame(&buf, DEFAULT_MAX_FRAME),
            Split::NeedMore { needed: 7 }
        );
        assert_eq!(
            split_frame(&buf[..4], DEFAULT_MAX_FRAME),
            Split::NeedMore { needed: 7 }
        );
    }

    #[test]
    fn two_frames_in_one_buffer() {
        let buf = [2, 0, 0, 0, b'a', b'b', 1, 0, 0, 0, b'c'];
        let Split::Frame { start, len } = split_frame(&buf, DEFAULT_MAX_FRAME) else {
            panic!("first frame");
        };
        assert_eq!(&buf[start..start + len], b"ab");
        let rest = &buf[start + len..];
        assert_eq!(
            split_frame(rest, DEFAULT_MAX_FRAME),
            Split::Frame { start: 4, len: 1 }
        );
        let Split::Frame { start, len } = split_frame(rest, DEFAULT_MAX_FRAME) else {
            unreachable!()
        };
        assert_eq!(&rest[start..start + len], b"c");
        assert!(rest[start + len..].is_empty());
    }

    #[test]
    fn zero_length_frame() {
        assert_eq!(
            split_frame(&[0, 0, 0, 0], DEFAULT_MAX_FRAME),
            Split::Frame { start: 4, len: 0 }
        );
        assert_eq!(
            split_frame(&[0, 0, 0, 0, 9], DEFAULT_MAX_FRAME),
            Split::Frame { start: 4, len: 0 }
        );
    }

    #[test]
    fn max_is_accepted_and_one_more_is_too_large() {
        let at_max = prefixed(DEFAULT_MAX_FRAME as u32);
        assert_eq!(
            split_frame(&at_max, DEFAULT_MAX_FRAME),
            Split::NeedMore {
                needed: PREFIX_LEN + DEFAULT_MAX_FRAME
            }
        );
        let over = prefixed(DEFAULT_MAX_FRAME as u32 + 1);
        assert_eq!(split_frame(&over, DEFAULT_MAX_FRAME), Split::TooLarge);
        assert_eq!(
            split_frame(&prefixed(u32::MAX), DEFAULT_MAX_FRAME),
            Split::TooLarge
        );

        // A caller's own, smaller ceiling.
        assert_eq!(
            split_frame(&[2, 0, 0, 0, b'a', b'b'], 2),
            Split::Frame { start: 4, len: 2 }
        );
        assert_eq!(split_frame(&[3, 0, 0, 0], 2), Split::TooLarge);
    }

    #[test]
    fn put_prefix_writes_u32_le_and_refuses_over_max() {
        let mut bytes = [0u8; PREFIX_LEN];
        let mut out = SliceBuf::new(&mut bytes);
        put_prefix(0x0001_0203, DEFAULT_MAX_FRAME, &mut out).unwrap();
        assert_eq!(out.written(), &[3, 2, 1, 0]);

        let mut out = SliceBuf::new(&mut bytes);
        put_prefix(DEFAULT_MAX_FRAME, DEFAULT_MAX_FRAME, &mut out).unwrap();
        assert_eq!(out.written(), &prefixed(DEFAULT_MAX_FRAME as u32));

        let mut out = SliceBuf::new(&mut bytes);
        assert_eq!(
            put_prefix(DEFAULT_MAX_FRAME + 1, DEFAULT_MAX_FRAME, &mut out),
            Err(RuntimeError::FrameTooLarge)
        );
        assert!(out.written().is_empty());
    }

    #[test]
    fn put_prefix_takes_the_callers_limit() {
        let mut bytes = [0u8; PREFIX_LEN];
        let mut out = SliceBuf::new(&mut bytes);
        put_prefix(2, 2, &mut out).unwrap();
        assert_eq!(out.written(), &[2, 0, 0, 0]);

        let mut out = SliceBuf::new(&mut bytes);
        assert_eq!(put_prefix(3, 2, &mut out), Err(RuntimeError::FrameTooLarge));
        assert!(out.written().is_empty());

        // A limit above what the prefix can hold still stops at `u32::MAX`.
        let mut out = SliceBuf::new(&mut bytes);
        put_prefix(u32::MAX as usize, usize::MAX, &mut out).unwrap();
        assert_eq!(out.written(), &prefixed(u32::MAX));
        #[cfg(target_pointer_width = "64")]
        {
            let mut out = SliceBuf::new(&mut bytes);
            assert_eq!(
                put_prefix(u32::MAX as usize + 1, usize::MAX, &mut out),
                Err(RuntimeError::FrameTooLarge)
            );
            assert!(out.written().is_empty());
        }
    }

    #[test]
    fn put_prefix_round_trips_through_split_frame() {
        for len in [0usize, 1, 255, 256, 65_536, DEFAULT_MAX_FRAME] {
            let mut prefix = [0u8; PREFIX_LEN];
            put_prefix(len, DEFAULT_MAX_FRAME, &mut SliceBuf::new(&mut prefix)).unwrap();
            assert_eq!(frame_len(prefix, DEFAULT_MAX_FRAME), Some(len));
        }
    }
}
