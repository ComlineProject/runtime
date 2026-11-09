use serde::Serialize;

use super::datagram2::{Datagram2Framing, KIND_REQUEST};
use super::{put_varint, take_varint, ConnectionFraming, Frame, ProtocolNumber};
use crate::contract::{BufMut, Call, Envelope, Request, RequestCall, RuntimeError, WireFormat};

/// The Comline datagram framing, version 3: `datagram@2` with a call
/// addressed by wire numbers (`calls = by_id`, runtime#15). A request names
/// its protocol and its function by the numbers core assigns them
/// (core#127), both as varints, so reordering or inserting functions no
/// longer moves what a call addresses. Replies are `datagram@2`'s.
/// `no_std`, allocation-free.
///
/// - Request: `[0][protocol varint][function varint][request_id u64 LE][params…]`
/// - Reply: `[1][protocol varint][request_id u64 LE][envelope…]`
///
/// Both numbers are `u16`s, as unsigned LEB128 varints in their shortest
/// form: one byte below 128, at most three. The [`Call`]'s `id` is the
/// function's wire number.
#[derive(Debug, Default, Clone, Copy)]
pub struct Datagram3Framing;

impl ConnectionFraming for Datagram3Framing {
    fn name(&self) -> &'static str {
        crate::contract::FRAMING_DATAGRAM3
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
        put_varint(call.id, out);
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
        put_varint(call_id, out);
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
        Datagram2Framing.encode_reply(protocol, request_id, reply, out)
    }

    fn decode<'f>(&self, frame: &'f [u8]) -> Option<Frame<'f>> {
        let (&kind, rest) = frame.split_first()?;
        if kind != KIND_REQUEST {
            return Datagram2Framing.decode(frame);
        }
        let (protocol, rest) = take_varint(rest)?;
        let (function, rest) = take_varint(rest)?;
        let (head, params) = rest.split_at_checked(8)?;
        Some(Frame::Request {
            protocol: Some(protocol),
            request: Request {
                call: RequestCall::Id(function),
                request_id: u64::from_le_bytes(head.try_into().ok()?),
                params,
            },
            one_way: false,
        })
    }
}
