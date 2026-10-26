// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Stream framing (`contract::stream`) on arbitrary bytes and limits. The
//! input is `[max: u32 LE][stream bytes]`.
//!
//! - `split_frame` never panics, and says exactly what the prefix declares:
//!   a `Frame` is within `max` and within the buffer, `NeedMore` counts the
//!   prefix in, and `TooLarge` only for a prefix over `max`.
//! - Splitting a whole buffer frame by frame consumes it in order, every
//!   frame within bounds, until what's left isn't a whole frame.
//! - `put_prefix` refuses only a length over `max`, and what it writes
//!   `split_frame` reads back as the same frame.

#![no_main]

use comline_runtime::contract::stream::{frame_len, put_prefix, split_frame, Split, PREFIX_LEN};
use comline_runtime::contract::{RuntimeError, SliceBuf};
use libfuzzer_sys::fuzz_target;

/// What `split_frame(buf, max)` must say, from the prefix alone.
fn check_split(buf: &[u8], max: usize) -> Split {
    let split = split_frame(buf, max);
    let Some(prefix) = buf.first_chunk::<PREFIX_LEN>() else {
        assert_eq!(split, Split::NeedMore { needed: 0 }, "{buf:02x?}");
        return split;
    };
    let declared = u32::from_le_bytes(*prefix) as usize;
    assert_eq!(
        frame_len(*prefix, max),
        (declared <= max).then_some(declared)
    );
    match split {
        Split::Frame { start, len } => {
            assert_eq!(start, PREFIX_LEN);
            assert_eq!(len, declared);
            assert!(len <= max, "a {len}-byte frame under max {max}");
            assert!(start + len <= buf.len(), "a frame past the buffer");
        }
        Split::NeedMore { needed } => {
            assert!(declared <= max, "a {declared}-byte frame under max {max}");
            assert_eq!(needed, PREFIX_LEN + declared);
            assert!(needed > buf.len());
        }
        Split::TooLarge => assert!(
            declared > max,
            "refused a {declared}-byte frame under max {max}"
        ),
    }
    split
}

fuzz_target!(|input: &[u8]| {
    let Some((max, bytes)) = input.split_first_chunk::<4>() else {
        return;
    };
    let max = u32::from_le_bytes(*max) as usize;

    // The whole buffer, frame by frame.
    let mut at = 0;
    loop {
        match check_split(&bytes[at..], max) {
            Split::Frame { start, len } => at += start + len,
            Split::NeedMore { .. } | Split::TooLarge => break,
        }
        assert!(at <= bytes.len());
    }

    // The buffer as one frame's body: prefixed under `max`, it splits back
    // to itself; over it, nothing is written.
    let mut prefix = [0u8; PREFIX_LEN];
    let mut out = SliceBuf::new(&mut prefix);
    match put_prefix(bytes.len(), max, &mut out) {
        Ok(()) => {
            assert!(bytes.len() <= max);
            assert_eq!(out.written().len(), PREFIX_LEN);
            let framed = [&prefix[..], bytes].concat();
            assert_eq!(
                check_split(&framed, max),
                Split::Frame {
                    start: PREFIX_LEN,
                    len: bytes.len()
                }
            );
            assert_eq!(frame_len(prefix, max), Some(bytes.len()));
        }
        Err(error) => {
            assert_eq!(error, RuntimeError::FrameTooLarge);
            assert!(bytes.len() > max);
            assert!(out.written().is_empty());
        }
    }
});
