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

/// Params serialised by the caller frame exactly as `encode_request`
/// frames them, as a call and as a notification. None at all leave
/// `params` out, whether `()`, empty bytes or `null`.
#[test]
fn a_request_framed_around_its_params_bytes_matches() {
    let f = JsonRpcFraming;
    for (params, bytes) in [
        (&(7u32, "x") as &dyn erased::Params, &br#"[7,"x"]"#[..]),
        (&(), &b""[..]),
    ] {
        for one_way in [false, true] {
            let (mut generic, mut body) = (Vec::new(), Vec::new());
            params.encode(&f, one_way, &mut generic);
            ConnectionFraming::encode_request_body(&f, 0, 0, "greet", 1, one_way, bytes, &mut body);
            assert_eq!(body, generic, "one_way: {one_way}");
            assert!(serde_json::from_slice::<serde_json::Value>(&body).is_ok());
        }
        let (mut generic, mut body) = (Vec::new(), Vec::new());
        params.encode(&f, false, &mut generic);
        Framing::encode_request_body(&f, 0, "greet", 1, bytes, &mut body);
        assert_eq!(body, generic);
    }
}

/// Lets the test above hold params of different types in one list.
mod erased {
    use super::*;

    pub trait Params {
        fn encode(&self, f: &JsonRpcFraming, one_way: bool, out: &mut Vec<u8>);
    }

    impl<P: Serialize> Params for P {
        fn encode(&self, f: &JsonRpcFraming, one_way: bool, out: &mut Vec<u8>) {
            ConnectionFraming::encode_request(
                f,
                0,
                Call::new(0, "greet"),
                1,
                one_way,
                self,
                &Json,
                out,
            )
            .unwrap();
        }
    }
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

/// Frames JSON-RPC 2.0 forbids, refused by every decoder (runtime#75).
#[test]
fn frames_that_arent_json_rpc_2_are_refused() {
    let f = JsonRpcFraming;
    let requests: [&[u8]; 5] = [
        br#"{"jsonrpc":"1.0","method":"greet","params":[],"id":1}"#,
        br#"{"method":"greet","params":[],"id":1}"#,
        br#"{"jsonrpc":"2.0","method":"greet","params":"x","id":1}"#,
        br#"{"jsonrpc":"2.0","method":"greet","params":7,"id":1}"#,
        // Not a notification: one still gets a reply.
        br#"{"jsonrpc":"2.0","method":"greet","params":[],"id":null}"#,
    ];
    for frame in requests {
        assert!(
            f.decode_request(frame).is_none(),
            "{}",
            str::from_utf8(frame).unwrap()
        );
        assert!(
            ConnectionFraming::decode(&f, frame).is_none(),
            "{}",
            str::from_utf8(frame).unwrap()
        );
    }
    let responses: [&[u8]; 4] = [
        br#"{"jsonrpc":"2.0","result":1,"error":{"code":1,"message":"x"},"id":1}"#,
        br#"{"jsonrpc":"2.0","error":{"code":1},"id":1}"#,
        br#"{"jsonrpc":"2.0","error":{"code":1,"message":7},"id":1}"#,
        br#"{"result":1,"id":1}"#,
    ];
    for frame in responses {
        assert!(
            f.decode_response(frame).is_none(),
            "{}",
            str::from_utf8(frame).unwrap()
        );
        assert!(
            ConnectionFraming::decode(&f, frame).is_none(),
            "{}",
            str::from_utf8(frame).unwrap()
        );
    }
    // Params as an object, or none written, still decode.
    assert!(f
        .decode_request(br#"{"jsonrpc":"2.0","method":"greet","params":{"a":1},"id":1}"#)
        .is_some());
    assert!(f
        .decode_request(br#"{"jsonrpc":"2.0","method":"greet","id":1}"#)
        .is_some());
}

#[test]
fn a_code_that_cannot_be_an_ordinal_decodes_as_a_fault() {
    let f = JsonRpcFraming;
    // Any other reserved JSON-RPC code, and a code too wide for a `u16`.
    for frame in [
        br#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":1}"#.to_vec(),
        br#"{"jsonrpc":"2.0","error":{"code":70000,"message":"x"},"id":1}"#.to_vec(),
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
    Framing::encode_request(&f, Call::new(0, "send"), 4, &("hi",), &Json, &mut lockstep).unwrap();
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
    // And what its outcome is: `null` is one, and nothing isn't.
    assert_eq!(f.decode(br#"{"jsonrpc":"2.0","id":2}"#), None);
    assert_eq!(f.decode(br#"{"jsonrpc":"2.0","error":null,"id":2}"#), None);
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
            format!(r#"{{"jsonrpc":"2.0","method":{quoted},"id":1}}"#)
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
            format!(r#"{{"jsonrpc":"2.0","method":{quoted}}}"#)
        );
    }
}

/// A format that writes these slices, whatever the value.
struct Writes<'a>(&'a [&'a str]);

impl WireFormat for Writes<'_> {
    fn name(&self) -> &'static str {
        "writes"
    }
    fn encode<T: Serialize + ?Sized>(
        &self,
        _: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError> {
        self.0
            .iter()
            .for_each(|text| out.put_slice(text.as_bytes()));
        Ok(())
    }
    fn decode<'de, T: Deserialize<'de>>(&self, _: &'de [u8]) -> Result<T, RuntimeError> {
        Err(RuntimeError::Serialization)
    }
}

/// What decides is what the format writes, however it splits it: a value
/// that serialises as none doesn't drop params a format writes anyway.
#[test]
fn params_are_left_out_only_when_the_format_writes_none() {
    let with = |written: &[&str]| {
        let mut frame = Vec::new();
        Framing::encode_request(
            &JsonRpcFraming,
            Call::new(0, "m"),
            1,
            &(),
            &Writes(written),
            &mut frame,
        )
        .unwrap();
        String::from_utf8(frame).unwrap()
    };
    let none = r#"{"jsonrpc":"2.0","method":"m","id":1}"#;
    assert_eq!(with(&[]), none);
    assert_eq!(with(&[""]), none);
    assert_eq!(with(&["null"]), none);
    assert_eq!(with(&["nu", "ll"]), none);
    assert_eq!(with(&["n", "", "u", "l", "l"]), none);
    for (written, params) in [
        (&["[1,2]"][..], "[1,2]"),
        (&["[", "1", "]"][..], "[1]"),
        (&["nul"][..], "nul"),
        (&["nu", "x"][..], "nux"),
        (&["null", "x"][..], "nullx"),
        (&["nullx"][..], "nullx"),
    ] {
        assert_eq!(
            with(written),
            format!(r#"{{"jsonrpc":"2.0","method":"m","params":{params},"id":1}}"#)
        );
    }
}

/// Whatever serialises as `null` in JSON leaves `params` out, in both
/// ways of framing a request. Anything else keeps them, empty ones too.
#[test]
fn only_what_serialises_as_null_is_no_params() {
    #[derive(Serialize)]
    struct UnitStruct;
    #[derive(Serialize)]
    struct Empty {}
    #[derive(Serialize)]
    struct Wrapper(());
    fn framed<P: Serialize>(params: &P) -> String {
        let mut typed = Vec::new();
        Framing::encode_request(
            &JsonRpcFraming,
            Call::new(0, "m"),
            1,
            params,
            &Json,
            &mut typed,
        )
        .unwrap();
        let bytes = serde_json::to_vec(params).unwrap();
        let mut body = Vec::new();
        Framing::encode_request_body(&JsonRpcFraming, 0, "m", 1, &bytes, &mut body);
        assert_eq!(typed, body);
        String::from_utf8(typed).unwrap()
    }
    let none = r#"{"jsonrpc":"2.0","method":"m","id":1}"#;
    assert_eq!(framed(&()), none);
    assert_eq!(framed(&UnitStruct), none);
    assert_eq!(framed(&Wrapper(())), none);
    assert_eq!(framed(&None::<u8>), none);
    assert_eq!(framed(&Some(())), none);
    assert_eq!(
        framed(&Empty {}),
        r#"{"jsonrpc":"2.0","method":"m","params":{},"id":1}"#
    );
    assert_eq!(
        framed(&[0u8; 0]),
        r#"{"jsonrpc":"2.0","method":"m","params":[],"id":1}"#
    );
}

/// An error with a `null` id is the peer refusing, and nothing else is.
#[test]
fn only_an_error_with_a_null_id_is_a_peer_refusal() {
    let f = JsonRpcFraming;
    for frame in [
        r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":null}"#,
        r#"{"id":null,"error":{"code":-32600,"message":"x","data":[1]},"jsonrpc":"2.0"}"#,
        r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"x"},"x":1,"id":null}"#,
        r#"{"jsonrpc":"2.0","result":null,"error":{"code":-32700,"message":"x"},"id":null}"#,
    ] {
        assert!(
            Framing::decode_response(&f, frame.as_bytes()).is_none(),
            "{frame}"
        );
        assert!(
            ConnectionFraming::decode(&f, frame.as_bytes()).is_none(),
            "{frame}"
        );
        assert!(Framing::is_peer_refusal(&f, frame.as_bytes()), "{frame}");
        assert!(
            ConnectionFraming::is_peer_refusal(&f, frame.as_bytes()),
            "{frame}"
        );
    }
    for frame in [
        // Not null, or no id at all.
        r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"x"},"id":1}"#,
        r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"x"}}"#,
        // Not an error.
        r#"{"jsonrpc":"2.0","result":1,"id":null}"#,
        r#"{"jsonrpc":"2.0","error":null,"id":null}"#,
        r#"{"jsonrpc":"2.0","id":null}"#,
        r#"{"jsonrpc":"2.0","result":1,"error":{"code":1,"message":"x"},"id":null}"#,
        // Not JSON-RPC 2.0's error, or its version.
        r#"{"jsonrpc":"2.0","error":{"code":-32700},"id":null}"#,
        r#"{"jsonrpc":"1.0","error":{"code":-32700,"message":"x"},"id":null}"#,
        r#"{"error":{"code":-32700,"message":"x"},"id":null}"#,
        // Written twice.
        r#"{"jsonrpc":"2.0","error":{"code":1,"message":"x"},"id":null,"id":null}"#,
        // A request with a null id.
        r#"{"jsonrpc":"2.0","method":"m","params":[],"id":null}"#,
        "",
        "{",
    ] {
        assert!(!Framing::is_peer_refusal(&f, frame.as_bytes()), "{frame}");
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
