//! Alternative [`Framing`](crate::contract::Framing) implementations.
//!
//! The Comline datagram framing is `DatagramFraming` in `contract`; this module
//! holds ones that need `std` / extra deps.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::contract::{
    BufMut, Call, Envelope, Framing, Request, RequestCall, RuntimeError, WireFormat,
};

/// [JSON-RPC 2.0](https://www.jsonrpc.org/specification) framing — a
/// **name-oriented**, human-readable frame. Pair it with
/// [`Json`](crate::format::Json).
///
/// Request:  `{"jsonrpc":"2.0","method":<name>,"params":<params>,"id":<n>}`
/// Response: `{"jsonrpc":"2.0","result":<r>,"id":<n>}` or
///           `{"jsonrpc":"2.0","error":{"code":<ordinal>,"message":...,"data":<body>},"id":<n>}`
///
/// A raised schema error maps to a JSON-RPC `error` object whose `code` is the
/// schema-global ordinal and whose `data` is the serialised error struct.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonRpcFraming;

fn u64_bytes(n: u64) -> impl AsRef<[u8]> {
    // small, no itoa dep
    let s = n.to_string();
    s.into_bytes()
}

impl Framing for JsonRpcFraming {
    fn name(&self) -> &'static str {
        "jsonrpc-2.0"
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
        // method names are generated identifiers — no JSON escaping needed.
        out.put_slice(br#"{"jsonrpc":"2.0","method":""#);
        out.put_slice(call.name.as_bytes());
        out.put_slice(br#"","params":"#);
        fmt.encode(params, out)?;
        out.put_slice(br#","id":"#);
        out.put_slice(u64_bytes(request_id).as_ref());
        out.put_slice(b"}");
        Ok(())
    }

    fn decode_request<'f>(&self, frame: &'f [u8]) -> Option<Request<'f>> {
        #[derive(Deserialize)]
        struct ReqIn<'a> {
            method: &'a str,
            #[serde(borrow, default)]
            params: Option<&'a RawValue>,
            #[serde(default)]
            id: Option<u64>,
        }
        let r: ReqIn = serde_json::from_slice(frame).ok()?;
        Some(Request {
            call: RequestCall::Name(r.method),
            request_id: r.id.unwrap_or(0),
            params: r.params.map(|p| p.get().as_bytes()).unwrap_or(b"null"),
        })
    }

    fn encode_response_ok(&self, request_id: u64, payload: &[u8], out: &mut dyn BufMut) {
        out.put_slice(br#"{"jsonrpc":"2.0","result":"#);
        out.put_slice(if payload.is_empty() { b"null" } else { payload });
        out.put_slice(br#","id":"#);
        out.put_slice(u64_bytes(request_id).as_ref());
        out.put_slice(b"}");
    }

    fn encode_response_err(&self, request_id: u64, id: u16, body: &[u8], out: &mut dyn BufMut) {
        out.put_slice(br#"{"jsonrpc":"2.0","error":{"code":"#);
        out.put_slice(u64_bytes(u64::from(id)).as_ref());
        out.put_slice(br#","message":"application error","data":"#);
        out.put_slice(if body.is_empty() { b"null" } else { body });
        out.put_slice(br#"},"id":"#);
        out.put_slice(u64_bytes(request_id).as_ref());
        out.put_slice(b"}");
    }

    fn decode_response<'f>(&self, frame: &'f [u8]) -> Option<(u64, Envelope<'f>)> {
        #[derive(Deserialize)]
        struct ErrIn<'a> {
            code: i64,
            #[serde(borrow, default)]
            data: Option<&'a RawValue>,
        }
        #[derive(Deserialize)]
        struct RespIn<'a> {
            #[serde(borrow, default)]
            result: Option<&'a RawValue>,
            #[serde(borrow, default)]
            error: Option<ErrIn<'a>>,
            id: u64,
        }
        let r: RespIn = serde_json::from_slice(frame).ok()?;
        if let Some(e) = r.error {
            Some((
                r.id,
                Envelope::Err {
                    id: e.code as u16,
                    body: e.data.map(|d| d.get().as_bytes()).unwrap_or(b"null"),
                },
            ))
        } else {
            Some((
                r.id,
                Envelope::Ok(r.result.map(|p| p.get().as_bytes()).unwrap_or(b"null")),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Json;

    #[test]
    fn request_round_trips() {
        let f = JsonRpcFraming;
        let mut frame = Vec::new();
        f.encode_request(Call::new(0, "greet"), 1, &(7u32, "x"), &Json, &mut frame)
            .unwrap();
        assert_eq!(
            std::str::from_utf8(&frame).unwrap(),
            r#"{"jsonrpc":"2.0","method":"greet","params":[7,"x"],"id":1}"#
        );

        let req = f.decode_request(&frame).unwrap();
        assert_eq!(req.call, RequestCall::Name("greet"));
        assert_eq!(req.request_id, 1);
        assert_eq!(req.params, br#"[7,"x"]"#);
    }

    #[test]
    fn ok_response_round_trips() {
        let f = JsonRpcFraming;
        let mut frame = Vec::new();
        f.encode_response_ok(9, br#"{"body":"hi"}"#, &mut frame);
        assert_eq!(
            f.decode_response(&frame),
            Some((9, Envelope::Ok(br#"{"body":"hi"}"#)))
        );
    }

    #[test]
    fn err_response_carries_the_ordinal() {
        let f = JsonRpcFraming;
        let mut frame = Vec::new();
        f.encode_response_err(9, 3, br#"{"why":"no"}"#, &mut frame);
        assert_eq!(
            f.decode_response(&frame),
            Some((9, Envelope::Err { id: 3, body: br#"{"why":"no"}"# }))
        );
    }
}
