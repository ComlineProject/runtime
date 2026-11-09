use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::contract::{
    BufMut, Call, ConnectionFraming, Envelope, Frame, Framing, ProtocolNumber, Request,
    RequestCall, RuntimeError, WireFormat,
};

mod read;
mod write;

use read::{
    envelope_of, is_peer_refusal, method_of, one_outcome, params_of, parse, present, request_id,
    ErrIn, Version,
};
use write::{put_request, put_request_body, put_u64};
#[cfg(test)]
mod tests;

/// [JSON-RPC 2.0](https://www.jsonrpc.org/specification) framing — a
/// **name-oriented**, human-readable frame. Pair it with
/// [`Json`](crate::format::Json).
///
/// A payload is written into the frame's own JSON text, and that isn't
/// checked: writing it costs every frame the same, and a peer refuses what it
/// can't read. So bytes given to `encode_request_body` or a reply must be one
/// JSON value, and params an array, an object or nothing. Anything else is a
/// frame a peer refuses or, if it's still JSON (`[],"x":1`), one where what
/// follows the first value is read as other members and lost.
///
/// Request:  `{"jsonrpc":"2.0","method":<name>,"params":<params>,"id":<n>}`,
///           without `params` when there are none
/// Response: `{"jsonrpc":"2.0","result":<r>,"id":<n>}` or
///           `{"jsonrpc":"2.0","error":{"code":<ordinal>,"message":...,"data":<body>},"id":<n>}`
///
/// A raised schema error maps to a JSON-RPC `error` object whose `code` is the
/// schema-global ordinal and whose `data` is the serialised error struct. A
/// provider fault maps to JSON-RPC's own reserved `-32603 Internal error`,
/// which is what a non-Comline JSON-RPC peer already expects to see — the
/// ordinal space stays entirely non-negative, so the two can never collide.
///
/// It is also a [`ConnectionFraming`], for calls in both directions: a
/// message with a `method` is a request, any other a reply. A request names
/// no protocol, only its method - bare (`send`) or qualified
/// (`chat.Chat/send`), as the schema's `method_names` chose - which the
/// receiver looks up among the protocols it provides. A one-way call is a
/// notification: it has no `id`, and gets no reply.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonRpcFraming;

impl Framing for JsonRpcFraming {
    fn name(&self) -> &'static str {
        crate::contract::FRAMING_JSONRPC
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
        put_request(call.name, Some(request_id), params, fmt, out)
    }

    fn encode_request_body(
        &self,
        _call_id: u16,
        name: &str,
        request_id: u64,
        params: &[u8],
        out: &mut dyn BufMut,
    ) {
        put_request_body(name, Some(request_id), params, out);
    }

    fn decode_request<'f>(&self, frame: &'f [u8]) -> Option<Request<'f>> {
        #[derive(Deserialize)]
        struct ReqIn<'a> {
            #[serde(rename = "jsonrpc")]
            _version: Version,
            #[serde(borrow)]
            method: &'a RawValue,
            #[serde(borrow, default)]
            params: Option<&'a RawValue>,
            #[serde(default, deserialize_with = "request_id")]
            id: Option<u64>,
        }
        let r: ReqIn = parse(frame)?;
        Some(Request {
            call: RequestCall::Name(method_of(r.method)?),
            request_id: r.id.unwrap_or(0),
            params: params_of(r.params)?,
        })
    }

    fn encode_response_ok(&self, request_id: u64, payload: &[u8], out: &mut dyn BufMut) {
        out.put_slice(br#"{"jsonrpc":"2.0","result":"#);
        out.put_slice(if payload.is_empty() { b"null" } else { payload });
        out.put_slice(br#","id":"#);
        put_u64(request_id, out);
        out.put_slice(b"}");
    }

    fn encode_response_err(&self, request_id: u64, id: u16, body: &[u8], out: &mut dyn BufMut) {
        out.put_slice(br#"{"jsonrpc":"2.0","error":{"code":"#);
        put_u64(u64::from(id), out);
        out.put_slice(br#","message":"application error","data":"#);
        out.put_slice(if body.is_empty() { b"null" } else { body });
        out.put_slice(br#"},"id":"#);
        put_u64(request_id, out);
        out.put_slice(b"}");
    }

    fn encode_response_fault(&self, request_id: u64, out: &mut dyn BufMut) {
        out.put_slice(
            br#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"},"id":"#,
        );
        put_u64(request_id, out);
        out.put_slice(b"}");
    }

    fn decode_response<'f>(&self, frame: &'f [u8]) -> Option<(u64, Envelope<'f>)> {
        #[derive(Deserialize)]
        struct RespIn<'a> {
            #[serde(rename = "jsonrpc")]
            _version: Version,
            #[serde(borrow, default, deserialize_with = "present")]
            result: Option<&'a RawValue>,
            #[serde(borrow, default)]
            error: Option<ErrIn<'a>>,
            id: u64,
        }
        let r: RespIn = parse(frame)?;
        one_outcome(r.result, r.error.as_ref())?;
        Some((r.id, envelope_of(r.result, r.error)))
    }

    fn is_peer_refusal(&self, frame: &[u8]) -> bool {
        is_peer_refusal(frame)
    }
}

impl ConnectionFraming for JsonRpcFraming {
    fn name(&self) -> &'static str {
        crate::contract::FRAMING_JSONRPC
    }

    fn encode_request<W, P>(
        &self,
        _protocol: ProtocolNumber,
        call: Call,
        request_id: u64,
        one_way: bool,
        params: &P,
        fmt: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>
    where
        W: WireFormat,
        P: Serialize + ?Sized,
    {
        // A notification has no `id`, so the peer sends nothing back.
        let id = (!one_way).then_some(request_id);
        put_request(call.name, id, params, fmt, out)
    }

    fn encode_request_body(
        &self,
        _protocol: ProtocolNumber,
        _call_id: u16,
        name: &str,
        request_id: u64,
        one_way: bool,
        params: &[u8],
        out: &mut dyn BufMut,
    ) {
        let id = (!one_way).then_some(request_id);
        put_request_body(name, id, params, out);
    }

    fn encode_reply(
        &self,
        _protocol: ProtocolNumber,
        request_id: u64,
        reply: &Envelope<'_>,
        out: &mut dyn BufMut,
    ) {
        match *reply {
            Envelope::Ok(payload) => self.encode_response_ok(request_id, payload, out),
            Envelope::Err { id, body } => self.encode_response_err(request_id, id, body, out),
            Envelope::Fault => self.encode_response_fault(request_id, out),
        }
    }

    fn decode<'f>(&self, frame: &'f [u8]) -> Option<Frame<'f>> {
        #[derive(Deserialize)]
        struct MessageIn<'a> {
            #[serde(rename = "jsonrpc")]
            _version: Version,
            #[serde(borrow, default)]
            method: Option<&'a RawValue>,
            #[serde(borrow, default)]
            params: Option<&'a RawValue>,
            #[serde(default, deserialize_with = "request_id")]
            id: Option<u64>,
            #[serde(borrow, default, deserialize_with = "present")]
            result: Option<&'a RawValue>,
            #[serde(borrow, default)]
            error: Option<ErrIn<'a>>,
        }
        let m: MessageIn = parse(frame)?;
        Some(match m.method {
            Some(method) => Frame::Request {
                protocol: None,
                request: Request {
                    call: RequestCall::Name(method_of(method)?),
                    request_id: m.id.unwrap_or(0),
                    params: params_of(m.params)?,
                },
                one_way: m.id.is_none(),
            },
            None => {
                one_outcome(m.result, m.error.as_ref())?;
                Frame::Reply {
                    protocol: None,
                    request_id: m.id?,
                    envelope: envelope_of(m.result, m.error),
                }
            }
        })
    }

    fn is_peer_refusal(&self, frame: &[u8]) -> bool {
        is_peer_refusal(frame)
    }
}
