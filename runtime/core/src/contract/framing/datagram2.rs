use serde::Serialize;

use super::{put_varint, take_varint, ConnectionFraming, Frame, ProtocolNumber};
use crate::contract::{BufMut, Call, Envelope, Request, RequestCall, RuntimeError, WireFormat};

pub(super) const KIND_REQUEST: u8 = 0;
pub(super) const KIND_REPLY: u8 = 1;

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
        _one_way: bool,
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

    fn encode_request_body(
        &self,
        protocol: ProtocolNumber,
        call_id: u16,
        _name: &str,
        request_id: u64,
        _one_way: bool,
        params: &[u8],
        out: &mut dyn BufMut,
    ) {
        out.put_u8(KIND_REQUEST);
        put_varint(protocol, out);
        out.put_u16_le(call_id);
        out.put_u64_le(request_id);
        out.put_slice(params);
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
                    protocol: Some(protocol),
                    request: Request {
                        call: RequestCall::Id(call),
                        request_id,
                        params,
                    },
                    one_way: false,
                })
            }
            KIND_REPLY => {
                let (head, envelope) = rest.split_at_checked(8)?;
                Some(Frame::Reply {
                    protocol: Some(protocol),
                    request_id: u64::from_le_bytes(head.try_into().ok()?),
                    envelope: Envelope::decode(envelope)?,
                })
            }
            _ => None,
        }
    }
}
