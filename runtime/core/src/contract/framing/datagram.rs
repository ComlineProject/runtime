use serde::Serialize;

use super::{Call, Framing, Request, RequestCall};
use crate::contract::{BufMut, Envelope, RuntimeError, WireFormat};

/// `no_std`, allocation-free. Request `[call_id u16 LE][request_id u64 LE]
/// [params…]`; response `[request_id u64 LE][envelope…]` where the envelope is
/// the tag-byte [`Envelope`] form.
#[derive(Debug, Default, Clone, Copy)]
pub struct DatagramFraming;

impl Framing for DatagramFraming {
    fn name(&self) -> &'static str {
        crate::contract::FRAMING_DATAGRAM
    }

    fn encode_request<W, P>(
        &self,
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
        out.put_u16_le(call.id);
        out.put_u64_le(request_id);
        fmt.encode(params, out)
    }

    fn encode_request_body(
        &self,
        call_id: u16,
        _name: &str,
        request_id: u64,
        params: &[u8],
        out: &mut dyn BufMut,
    ) {
        out.put_u16_le(call_id);
        out.put_u64_le(request_id);
        out.put_slice(params);
    }

    fn decode_request<'f>(&self, frame: &'f [u8]) -> Option<Request<'f>> {
        let (head, params) = frame.split_at_checked(10)?;
        let id = u16::from_le_bytes([head[0], head[1]]);
        let request_id = u64::from_le_bytes(head[2..10].try_into().ok()?);
        Some(Request {
            call: RequestCall::Id(id),
            request_id,
            params,
        })
    }

    fn encode_response_ok(&self, request_id: u64, payload: &[u8], out: &mut dyn BufMut) {
        out.put_u64_le(request_id);
        Envelope::encode_ok(payload, out);
    }

    fn encode_response_err(&self, request_id: u64, id: u16, body: &[u8], out: &mut dyn BufMut) {
        out.put_u64_le(request_id);
        Envelope::encode_err(id, body, out);
    }

    fn encode_response_fault(&self, request_id: u64, out: &mut dyn BufMut) {
        out.put_u64_le(request_id);
        Envelope::encode_fault(out);
    }

    fn decode_response<'f>(&self, frame: &'f [u8]) -> Option<(u64, Envelope<'f>)> {
        let (head, rest) = frame.split_at_checked(8)?;
        let request_id = u64::from_le_bytes(head.try_into().ok()?);
        Some((request_id, Envelope::decode(rest)?))
    }
}
