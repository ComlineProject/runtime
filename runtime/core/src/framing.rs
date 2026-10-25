//! Alternative [`Framing`](crate::contract::Framing) implementations.
//!
//! The Comline datagram framing is `DatagramFraming` in `contract`; this module
//! holds ones that need `alloc` and extra deps (the `jsonrpc` feature, which
//! `std` implies).

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::contract::{
    BufMut, Call, ConnectionFraming, Envelope, Frame, Framing, MethodName, ProtocolNumber, Request,
    RequestCall, RuntimeError, WireFormat,
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

#[derive(Deserialize)]
struct ErrIn<'a> {
    code: i64,
    #[serde(borrow, default)]
    data: Option<&'a RawValue>,
}

/// A response's envelope, from its `result` or `error`.
fn envelope_of<'f>(result: Option<&'f RawValue>, error: Option<ErrIn<'f>>) -> Envelope<'f> {
    match error {
        Some(e) => match u16::try_from(e.code) {
            Ok(id) => Envelope::Err {
                id,
                body: e.data.map(|d| d.get().as_bytes()).unwrap_or(b"null"),
            },
            // An ordinal is a `u16`, so it is always in `0..=65535`. Anything
            // outside that — JSON-RPC's reserved negatives, `-32603` among
            // them, or a code too wide to be an ordinal — is not a schema
            // error this wire can name, so it is a fault.
            Err(_) => Envelope::Fault,
        },
        None => Envelope::Ok(result.map(|p| p.get().as_bytes()).unwrap_or(b"null")),
    }
}

/// Reads a frame as `T`. JSON text is UTF-8 (RFC 8259 §8.1), all of it:
/// serde_json checks only the strings it reads, and skips a member `T`
/// ignores (`jsonrpc`, `error.message`, an unknown one) without looking, so
/// the whole frame is checked first. Read from a `str`, serde_json then
/// doesn't check its strings again.
fn parse<'f, T: Deserialize<'f>>(frame: &'f [u8]) -> Option<T> {
    serde_json::from_str(core::str::from_utf8(frame).ok()?).ok()
}

/// The method a request names, from its `method` member, which must be a
/// string. It's read as written, so a name in any JSON spelling is borrowed
/// from the frame rather than copied out to unescape it.
fn method_of(method: &RawValue) -> Option<MethodName<'_>> {
    let text = method.get().strip_prefix('"')?.strip_suffix('"')?;
    MethodName::from_json(text)
}

/// Writes `{"jsonrpc":"2.0","method":<name>,"params":`, with the name a
/// JSON string escaped as serde_json escapes one: `"` and `\` by backslash,
/// control characters as `\b`, `\f`, `\n`, `\r`, `\t` or `\u00XX`, and
/// nothing else. A generated name needs no escape, and goes out in one slice.
fn put_method(name: &str, out: &mut dyn BufMut) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.put_slice(br#"{"jsonrpc":"2.0","method":""#);
    let bytes = name.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let control;
        let escaped: &[u8] = match b {
            b'"' => br#"\""#,
            b'\\' => br"\\",
            b'\n' => br"\n",
            b'\r' => br"\r",
            b'\t' => br"\t",
            0x08 => br"\b",
            0x0c => br"\f",
            0x00..=0x1f => {
                control = [
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(b >> 4)],
                    HEX[usize::from(b & 0xf)],
                ];
                &control
            }
            _ => continue,
        };
        out.put_slice(&bytes[start..i]);
        out.put_slice(escaped);
        start = i + 1;
    }
    out.put_slice(&bytes[start..]);
    out.put_slice(br#"","params":"#);
}

/// Writes `n` in decimal, as `u64`'s `Display` writes it, from a buffer on
/// the stack: 20 digits hold `u64::MAX`.
fn put_u64(n: u64, out: &mut dyn BufMut) {
    let mut digits = [0u8; 20];
    let mut start = digits.len();
    let mut n = n;
    loop {
        start -= 1;
        digits[start] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.put_slice(&digits[start..]);
}

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
        put_method(call.name, out);
        fmt.encode(params, out)?;
        out.put_slice(br#","id":"#);
        put_u64(request_id, out);
        out.put_slice(b"}");
        Ok(())
    }

    fn decode_request<'f>(&self, frame: &'f [u8]) -> Option<Request<'f>> {
        #[derive(Deserialize)]
        struct ReqIn<'a> {
            #[serde(borrow)]
            method: &'a RawValue,
            #[serde(borrow, default)]
            params: Option<&'a RawValue>,
            #[serde(default)]
            id: Option<u64>,
        }
        let r: ReqIn = parse(frame)?;
        Some(Request {
            call: RequestCall::Name(method_of(r.method)?),
            request_id: r.id.unwrap_or(0),
            params: r.params.map(|p| p.get().as_bytes()).unwrap_or(b"null"),
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
            #[serde(borrow, default)]
            result: Option<&'a RawValue>,
            #[serde(borrow, default)]
            error: Option<ErrIn<'a>>,
            id: u64,
        }
        let r: RespIn = parse(frame)?;
        Some((r.id, envelope_of(r.result, r.error)))
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
        if !one_way {
            return Framing::encode_request(self, call, request_id, params, fmt, out);
        }
        // A notification: no `id`, so the peer sends nothing back.
        put_method(call.name, out);
        fmt.encode(params, out)?;
        out.put_slice(b"}");
        Ok(())
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
            #[serde(borrow, default)]
            method: Option<&'a RawValue>,
            #[serde(borrow, default)]
            params: Option<&'a RawValue>,
            #[serde(default)]
            id: Option<u64>,
            #[serde(borrow, default)]
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
                    params: m.params.map(|p| p.get().as_bytes()).unwrap_or(b"null"),
                },
                one_way: m.id.is_none(),
            },
            None => Frame::Reply {
                protocol: None,
                request_id: m.id?,
                envelope: envelope_of(m.result, m.error),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    use core::str;

    use super::*;
    #[cfg(feature = "std")]
    use crate::format::Json;

    /// Without `std` there is no [`crate::format::Json`]: a stand-in that
    /// writes what it does, through `serde_json`'s `alloc` API.
    #[cfg(not(feature = "std"))]
    struct Json;

    #[cfg(not(feature = "std"))]
    impl WireFormat for Json {
        fn name(&self) -> &'static str {
            crate::contract::FORMAT_JSON
        }

        fn encode<T: Serialize + ?Sized>(
            &self,
            value: &T,
            out: &mut dyn BufMut,
        ) -> Result<(), RuntimeError> {
            let bytes = serde_json::to_vec(value).map_err(|_| RuntimeError::Serialization)?;
            out.put_slice(&bytes);
            Ok(())
        }

        fn decode<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> Result<T, RuntimeError> {
            serde_json::from_slice(bytes).map_err(|_| RuntimeError::Serialization)
        }
    }

    /// What [`put_u64`] writes.
    fn decimal(n: u64) -> Vec<u8> {
        let mut out = Vec::new();
        put_u64(n, &mut out);
        out
    }

    #[test]
    fn an_id_is_written_in_decimal() {
        for (n, text) in [
            (0, "0"),
            (1, "1"),
            (9, "9"),
            (10, "10"),
            (u64::from(u16::MAX), "65535"),
            (u64::MAX, "18446744073709551615"),
        ] {
            assert_eq!(decimal(n), text.as_bytes(), "{n}");
        }
        // Each power of ten, and one either side of it: every digit count,
        // and every carry.
        for exp in 0..=19 {
            let p = 10u64.pow(exp);
            for n in [p - 1, p, p + 1] {
                assert_eq!(decimal(n), n.to_string().as_bytes(), "{n}");
            }
        }
    }

    #[test]
    fn request_round_trips() {
        let f = JsonRpcFraming;
        let mut frame = Vec::new();
        Framing::encode_request(
            &f,
            Call::new(0, "greet"),
            1,
            &(7u32, "x"),
            &Json,
            &mut frame,
        )
        .unwrap();
        assert_eq!(
            str::from_utf8(&frame).unwrap(),
            r#"{"jsonrpc":"2.0","method":"greet","params":[7,"x"],"id":1}"#
        );

        let req = f.decode_request(&frame).unwrap();
        assert_eq!(req.call, RequestCall::Name("greet".into()));
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
    fn fault_response_uses_the_jsonrpc_internal_error_code() {
        let f = JsonRpcFraming;
        let mut frame = Vec::new();
        f.encode_response_fault(9, &mut frame);
        assert_eq!(
            str::from_utf8(&frame).unwrap(),
            r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"},"id":9}"#
        );
        assert_eq!(f.decode_response(&frame), Some((9, Envelope::Fault)));
    }

    #[test]
    fn a_code_that_cannot_be_an_ordinal_decodes_as_a_fault() {
        let f = JsonRpcFraming;
        // Any other reserved JSON-RPC code, and a code too wide for a `u16`.
        for frame in [
            br#"{"jsonrpc":"2.0","error":{"code":-32700},"id":1}"#.to_vec(),
            br#"{"jsonrpc":"2.0","error":{"code":70000},"id":1}"#.to_vec(),
        ] {
            assert_eq!(f.decode_response(&frame), Some((1, Envelope::Fault)));
        }
    }

    #[test]
    fn on_a_connection_a_one_way_call_is_a_notification() {
        let f = JsonRpcFraming;
        let mut frame = Vec::new();
        ConnectionFraming::encode_request(
            &f,
            0,
            Call::new(0, "chat.Chat/pushed"),
            4,
            true,
            &("hi",),
            &Json,
            &mut frame,
        )
        .unwrap();
        assert_eq!(
            str::from_utf8(&frame).unwrap(),
            r#"{"jsonrpc":"2.0","method":"chat.Chat/pushed","params":["hi"]}"#
        );
        assert_eq!(
            f.decode(&frame),
            Some(Frame::Request {
                protocol: None,
                request: Request {
                    call: RequestCall::Name("chat.Chat/pushed".into()),
                    request_id: 0,
                    params: br#"["hi"]"#
                },
                one_way: true,
            })
        );

        // A call that wants a reply keeps its id, as in lockstep.
        let mut call = Vec::new();
        ConnectionFraming::encode_request(
            &f,
            0,
            Call::new(0, "send"),
            4,
            false,
            &("hi",),
            &Json,
            &mut call,
        )
        .unwrap();
        let mut lockstep = Vec::new();
        Framing::encode_request(&f, Call::new(0, "send"), 4, &("hi",), &Json, &mut lockstep)
            .unwrap();
        assert_eq!(call, lockstep);
        assert!(matches!(
            f.decode(&call),
            Some(Frame::Request {
                request: Request { request_id: 4, .. },
                one_way: false,
                ..
            })
        ));
    }

    #[test]
    fn on_a_connection_anything_without_a_method_is_a_reply() {
        let f = JsonRpcFraming;
        for envelope in [
            Envelope::Ok(br#"{"body":"hi"}"#),
            Envelope::Err {
                id: 3,
                body: br#"{"why":"no"}"#,
            },
            Envelope::Fault,
        ] {
            let mut frame = Vec::new();
            f.encode_reply(0, 9, &envelope, &mut frame);
            assert_eq!(
                f.decode(&frame),
                Some(Frame::Reply {
                    protocol: None,
                    request_id: 9,
                    envelope
                })
            );
        }
        assert_eq!(
            f.decode(br#"{"jsonrpc":"2.0","result":null,"id":2}"#),
            Some(Frame::Reply {
                protocol: None,
                request_id: 2,
                envelope: Envelope::Ok(b"null")
            })
        );
        // A reply has to say which call it answers.
        assert_eq!(f.decode(br#"{"jsonrpc":"2.0","result":1}"#), None);
        assert_eq!(f.decode(b"[1,2]"), None);
    }

    #[test]
    fn err_response_carries_the_ordinal() {
        let f = JsonRpcFraming;
        let mut frame = Vec::new();
        f.encode_response_err(9, 3, br#"{"why":"no"}"#, &mut frame);
        assert_eq!(
            f.decode_response(&frame),
            Some((
                9,
                Envelope::Err {
                    id: 3,
                    body: br#"{"why":"no"}"#
                }
            ))
        );
    }
    /// The name a decoded request calls, as text.
    fn method_of(call: RequestCall<'_>) -> String {
        match call {
            RequestCall::Name(name) => name.to_string(),
            RequestCall::Id(id) => panic!("a JSON-RPC request named call {id}"),
        }
    }

    #[test]
    fn a_method_name_reads_in_any_json_spelling() {
        let f = JsonRpcFraming;
        for (method, name) in [
            (r#""greet""#, "greet"),
            // PHP's `json_encode` escapes `/` by default (#41).
            (r#""app.Chat\/count""#, "app.Chat/count"),
            (r#""\u0067reet""#, "greet"),
            (r#""\u00e9t\u00C9""#, "\u{e9}t\u{c9}"),
            (r#""\ud83d\ude00""#, "\u{1f600}"),
            (
                r#""a\"b\\c\n\t\r\b\f\u0001""#,
                "a\"b\\c\n\t\r\u{8}\u{c}\u{1}",
            ),
        ] {
            let request = format!(r#"{{"jsonrpc":"2.0","method":{method},"params":[],"id":1}}"#);
            let req = f
                .decode_request(request.as_bytes())
                .unwrap_or_else(|| panic!("refused {request}"));
            assert_eq!(method_of(req.call), name, "{request}");

            let Some(Frame::Request {
                request: req,
                one_way: false,
                ..
            }) = f.decode(request.as_bytes())
            else {
                panic!("a connection refused {request}");
            };
            assert_eq!(method_of(req.call), name, "{request}");
        }
    }

    #[test]
    fn a_method_name_that_is_not_a_valid_string_is_refused() {
        let f = JsonRpcFraming;
        for method in [
            r#""\ud800""#,
            r#""\udc00x""#,
            r#""\ud83dx""#,
            r#""\x""#,
            "5",
            "[]",
            "\"a\u{1}\"",
        ] {
            let request = format!(r#"{{"jsonrpc":"2.0","method":{method},"params":[],"id":1}}"#);
            assert_eq!(f.decode_request(request.as_bytes()), None, "{request}");
            assert_eq!(f.decode(request.as_bytes()), None, "{request}");
        }
    }

    #[test]
    fn a_method_name_is_written_as_serde_json_writes_it() {
        let f = JsonRpcFraming;
        for name in [
            "greet",
            "a\"b",
            "a\\b",
            "line\nbreak",
            "\u{0}\u{1}\u{1f}\u{7f}",
            "\u{8}\u{c}\t\r",
            "\u{e9}/\u{1f600}",
        ] {
            let quoted = serde_json::to_string(name).unwrap();
            let mut frame = Vec::new();
            Framing::encode_request(&f, Call::new(0, name), 1, &(), &Json, &mut frame).unwrap();
            assert_eq!(
                str::from_utf8(&frame).unwrap(),
                format!(r#"{{"jsonrpc":"2.0","method":{quoted},"params":null,"id":1}}"#)
            );
            assert_eq!(method_of(f.decode_request(&frame).unwrap().call), name);

            let mut notification = Vec::new();
            ConnectionFraming::encode_request(
                &f,
                0,
                Call::new(0, name),
                1,
                true,
                &(),
                &Json,
                &mut notification,
            )
            .unwrap();
            assert_eq!(
                str::from_utf8(&notification).unwrap(),
                format!(r#"{{"jsonrpc":"2.0","method":{quoted},"params":null}}"#)
            );
        }
    }

    #[test]
    fn invalid_utf8_anywhere_refuses_the_frame() {
        let f = JsonRpcFraming;
        // `\xff` is never UTF-8. Each frame is otherwise valid, with the bad
        // byte in a member the framing reads or one it ignores (#42).
        let responses: [&[u8]; 5] = [
            b"{\"jsonrpc\":\"2.\xff\",\"result\":1,\"id\":1}",
            b"{\"jsonrpc\":\"2.0\",\"result\":1,\"x\":\"\xff\",\"id\":1}",
            b"{\"jsonrpc\":\"2.0\",\"error\":{\"code\":1,\"message\":\"\xff\"},\"id\":1}",
            b"{\"jsonrpc\":\"2.0\",\"result\":\"\xff\",\"id\":1}",
            b"{\"jsonrpc\":\"2.0\",\"result\":1,\"\xff\":0,\"id\":1}",
        ];
        for frame in responses {
            assert_eq!(f.decode_response(frame), None, "{frame:?}");
            assert_eq!(f.decode(frame), None, "{frame:?}");
        }
        let requests: [&[u8]; 4] = [
            b"{\"jsonrpc\":\"2.\xff\",\"method\":\"greet\",\"params\":[],\"id\":1}",
            b"{\"jsonrpc\":\"2.0\",\"method\":\"greet\",\"params\":[],\"x\":\"\xff\",\"id\":1}",
            b"{\"jsonrpc\":\"2.0\",\"method\":\"greet\",\"params\":[\"\xff\"],\"id\":1}",
            b"{\"jsonrpc\":\"2.0\",\"method\":\"gr\xffeet\",\"params\":[],\"id\":1}",
        ];
        for frame in requests {
            assert_eq!(f.decode_request(frame), None, "{frame:?}");
            assert_eq!(f.decode(frame), None, "{frame:?}");
        }
    }
}
