use serde::Serialize;

use crate::contract::{BufMut, Call, Envelope, Request, RequestCall, RuntimeError, WireFormat};

/// A protocol's number on one connection. Under `comline@1` it's the
/// protocol's position in the schema, which both ends share because their
/// IR is identical; `comline@2` will hand numbers out at the handshake.
pub type ProtocolNumber = u16;

/// How a [`Connection`](crate::connection::Connection) puts calls on the wire
/// when either end may start one. Unlike [`Framing`](crate::contract::Framing),
/// whose frames only make sense in lockstep, every frame here says whether
/// it's a request or a reply, and which protocol it belongs to.
pub trait ConnectionFraming: Default {
    /// A stable name, folded into the handshake.
    fn name(&self) -> &'static str;

    /// Frame a request to `protocol`: the call, its id, and its params
    /// (serialised with `fmt`).
    fn encode_request<W, P>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        request_id: u64,
        params: &P,
        fmt: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>
    where
        W: WireFormat,
        P: Serialize + ?Sized;

    /// Frame the reply to request `request_id`, made by `protocol`.
    fn encode_reply(
        &self,
        protocol: ProtocolNumber,
        request_id: u64,
        reply: &Envelope<'_>,
        out: &mut dyn BufMut,
    );

    /// Parse a frame of either kind. Borrows `frame`.
    fn decode<'f>(&self, frame: &'f [u8]) -> Option<Frame<'f>>;
}

/// A decoded [`ConnectionFraming`] frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame<'f> {
    /// The peer calls one of this side's protocols.
    Request {
        protocol: ProtocolNumber,
        request: Request<'f>,
    },
    /// The peer answers a call this side made.
    Reply {
        protocol: ProtocolNumber,
        request_id: u64,
        envelope: Envelope<'f>,
    },
}

// ── datagram@2 ─────────────────────────────────────────────────────────────

const KIND_REQUEST: u8 = 0;
const KIND_REPLY: u8 = 1;

/// The Comline datagram framing, version 2: `datagram@1` plus a kind byte and
/// the protocol number, so both ends can start calls and several protocols
/// share a connection. `no_std`, allocation-free.
///
/// - Request: `[0][protocol varint][call_id u16 LE][request_id u64 LE][params…]`
/// - Reply: `[1][protocol varint][request_id u64 LE][envelope…]`
///
/// The protocol number is an unsigned LEB128 varint: one byte below 128.
/// Request ids are per direction: a reply's id is one the receiving side
/// handed out.
#[derive(Debug, Default, Clone, Copy)]
pub struct Datagram2Framing;

impl ConnectionFraming for Datagram2Framing {
    fn name(&self) -> &'static str {
        crate::contract::FRAMING_DATAGRAM2
    }

    fn encode_request<W, P>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        request_id: u64,
        params: &P,
        fmt: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>
    where
        W: WireFormat,
        P: Serialize + ?Sized,
    {
        out.put_u8(KIND_REQUEST);
        put_varint(protocol, out);
        out.put_u16_le(call.id);
        out.put_u64_le(request_id);
        fmt.encode(params, out)
    }

    fn encode_reply(
        &self,
        protocol: ProtocolNumber,
        request_id: u64,
        reply: &Envelope<'_>,
        out: &mut dyn BufMut,
    ) {
        out.put_u8(KIND_REPLY);
        put_varint(protocol, out);
        out.put_u64_le(request_id);
        reply.encode(out);
    }

    fn decode<'f>(&self, frame: &'f [u8]) -> Option<Frame<'f>> {
        let (&kind, rest) = frame.split_first()?;
        let (protocol, rest) = take_varint(rest)?;
        match kind {
            KIND_REQUEST => {
                let (head, params) = rest.split_at_checked(10)?;
                let call = u16::from_le_bytes([head[0], head[1]]);
                let request_id = u64::from_le_bytes(head[2..10].try_into().ok()?);
                Some(Frame::Request {
                    protocol,
                    request: Request {
                        call: RequestCall::Id(call),
                        request_id,
                        params,
                    },
                })
            }
            KIND_REPLY => {
                let (head, envelope) = rest.split_at_checked(8)?;
                Some(Frame::Reply {
                    protocol,
                    request_id: u64::from_le_bytes(head.try_into().ok()?),
                    envelope: Envelope::decode(envelope)?,
                })
            }
            _ => None,
        }
    }
}

fn put_varint(mut value: ProtocolNumber, out: &mut dyn BufMut) {
    while value >= 0x80 {
        out.put_u8((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.put_u8(value as u8);
}

/// An unsigned LEB128 number that fits a [`ProtocolNumber`], in its shortest
/// form, and the bytes after it.
fn take_varint(bytes: &[u8]) -> Option<(ProtocolNumber, &[u8])> {
    let mut value: u32 = 0;
    for (i, &byte) in bytes.iter().enumerate().take(3) {
        value |= u32::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            // A trailing zero byte would be a second spelling of a shorter
            // number.
            if i > 0 && byte == 0 {
                return None;
            }
            return Some((ProtocolNumber::try_from(value).ok()?, &bytes[i + 1..]));
        }
    }
    None
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn varints_round_trip_in_their_shortest_form() {
        for (value, bytes) in [
            (0u16, &[0x00][..]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (300, &[0xac, 0x02]),
            (u16::MAX, &[0xff, 0xff, 0x03]),
        ] {
            let mut out = Vec::new();
            put_varint(value, &mut out);
            assert_eq!(out, bytes, "{value}");
            assert_eq!(take_varint(&out), Some((value, &[][..])));
        }
        assert_eq!(take_varint(&[0x80, 0x00]), None, "overlong");
        assert_eq!(take_varint(&[0xff, 0xff, 0x04]), None, "past u16");
        assert_eq!(take_varint(&[0x80]), None, "truncated");
    }

    #[test]
    fn a_reply_round_trips() {
        let f = Datagram2Framing;
        for envelope in [
            Envelope::Ok(b"payload"),
            Envelope::Err { id: 2, body: b"fields" },
            Envelope::Fault,
        ] {
            let mut frame = Vec::new();
            f.encode_reply(200, 7, &envelope, &mut frame);
            assert_eq!(frame[0], KIND_REPLY);
            assert_eq!(
                f.decode(&frame),
                Some(Frame::Reply { protocol: 200, request_id: 7, envelope })
            );
        }
    }

    #[test]
    fn a_request_decodes() {
        let mut frame = Vec::from([KIND_REQUEST, 3]);
        frame.extend_from_slice(&5u16.to_le_bytes());
        frame.extend_from_slice(&42u64.to_le_bytes());
        frame.extend_from_slice(b"args");
        assert_eq!(
            Datagram2Framing.decode(&frame),
            Some(Frame::Request {
                protocol: 3,
                request: Request { call: RequestCall::Id(5), request_id: 42, params: b"args" },
            })
        );
    }

    #[test]
    fn malformed_frames_are_rejected() {
        let f = Datagram2Framing;
        assert_eq!(f.decode(&[]), None);
        assert_eq!(f.decode(&[2, 0]), None, "unknown kind");
        assert_eq!(f.decode(&[KIND_REQUEST, 0, 1, 0]), None, "short request");
        assert_eq!(f.decode(&[KIND_REPLY, 0, 0, 0, 0, 0, 0, 0, 0, 0]), None, "no envelope");
    }
}
