//! Compact request / response framing.
//!
//! - **Request**  — `[call_id: u16 LE] [request_id: u64 LE] [params …]`
//! - **Response** — `[request_id: u64 LE] [envelope …]`, where the envelope is
//!   the tag-byte form from [`Envelope`](crate::contract::Envelope).
//!
//! Datagram-oriented: one frame per message. A length-prefixed stream framing
//! for byte-stream transports (TCP) layers on top of this.

use crate::contract::BufMut;

const REQUEST_HEADER: usize = 2 + 8;
const RESPONSE_HEADER: usize = 8;

/// Write a request frame.
pub fn encode_request(call_id: u16, request_id: u64, params: &[u8], out: &mut dyn BufMut) {
    out.put_u16_le(call_id);
    out.put_u64_le(request_id);
    out.put_slice(params);
}

/// `(call_id, request_id, params)` — `params` borrows `frame`. `None` if the
/// frame is shorter than the header.
pub fn decode_request(frame: &[u8]) -> Option<(u16, u64, &[u8])> {
    let (head, params) = frame.split_at_checked(REQUEST_HEADER)?;
    let call_id = u16::from_le_bytes([head[0], head[1]]);
    let request_id = u64::from_le_bytes(head[2..10].try_into().ok()?);
    Some((call_id, request_id, params))
}

/// Write a response frame around an already-encoded envelope.
pub fn encode_response(request_id: u64, envelope: &[u8], out: &mut dyn BufMut) {
    out.put_u64_le(request_id);
    out.put_slice(envelope);
}

/// `(request_id, envelope_bytes)` — the envelope borrows `frame`; parse it with
/// [`Envelope::decode`](crate::contract::Envelope::decode). `None` if truncated.
pub fn decode_response(frame: &[u8]) -> Option<(u64, &[u8])> {
    let (head, envelope) = frame.split_at_checked(RESPONSE_HEADER)?;
    let request_id = u64::from_le_bytes(head.try_into().ok()?);
    Some((request_id, envelope))
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn request_round_trips() {
        let mut frame = Vec::new();
        encode_request(3, 0x0102_0304_0506_0708, b"args", &mut frame);
        assert_eq!(decode_request(&frame), Some((3, 0x0102_0304_0506_0708, &b"args"[..])));
    }

    #[test]
    fn response_round_trips() {
        let mut frame = Vec::new();
        encode_response(42, b"\x00payload", &mut frame);
        assert_eq!(decode_response(&frame), Some((42, &b"\x00payload"[..])));
    }

    #[test]
    fn truncated_frames_are_rejected() {
        assert_eq!(decode_request(&[0, 0, 0]), None);
        assert_eq!(decode_response(&[0, 0, 0]), None);
    }
}
