// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The reference: every case in `vectors/`, computed from `comline-runtime`.

use comline_runtime::contract::stream::{DEFAULT_MAX_FRAME, PREFIX_LEN};
use comline_runtime::contract::{
    Call, ConnectionFraming, Datagram2Framing, Datagram3Framing, Envelope, HandshakeFrame,
    FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};
use comline_runtime::format::Json;

use crate::engine::{decode, encode, encode_with};
use crate::{
    comline2, decode_typed, Case, Compare, DecodeIn, EncodeIn, Expect, Framed, HandshakeIn, Hex,
    Input, NameHashIn, Reply, RequestIn, ResponseIn, StreamPrefixIn, StreamSplit, StreamSplitIn,
    Type, Value, Vectors,
};

/// Where a generated vectors file finds its schema.
const VECTORS_SCHEMA: &str = "../specs/gen/vectors.schema.json";

/// Every feature's vectors, computed from `comline-runtime`: what
/// `vectors/` must hold. A change here is a wire-protocol change.
pub fn reference_cases() -> Vec<Vectors> {
    let mut vectors = Vec::new();
    let mut feature = |id: &str, cases: Vec<(String, Input)>| {
        vectors.push(Vectors {
            schema: Some(VECTORS_SCHEMA.into()),
            feature: id.into(),
            cases: cases
                .into_iter()
                .map(|(id, input)| exact(id, input))
                .collect(),
        });
    };

    feature(
        "name-hash",
        [
            "",
            FORMAT_JSON,
            FORMAT_MSGPACK,
            FORMAT_PACKED,
            FRAMING_DATAGRAM,
            FRAMING_DATAGRAM2,
            FRAMING_DATAGRAM3,
            FRAMING_JSONRPC,
            "com.acme.custom-format",
        ]
        .iter()
        .map(|name| {
            let id = if name.is_empty() { "empty" } else { name };
            (
                format!("name-hash/{id}"),
                Input::NameHash(NameHashIn {
                    name: name.to_string(),
                }),
            )
        })
        .collect(),
    );

    feature(
        "handshake",
        vec![
            handshake(
                "handshake/json-datagram",
                0x0102_0304_0506_0708,
                FORMAT_JSON,
                FRAMING_DATAGRAM,
                0,
            ),
            handshake(
                "handshake/msgpack-datagram",
                0xdead_beef_0102_0304,
                FORMAT_MSGPACK,
                FRAMING_DATAGRAM,
                0b101,
            ),
            handshake(
                "handshake/msgpack-datagram2",
                0x0102_0304_0506_0708,
                FORMAT_MSGPACK,
                FRAMING_DATAGRAM2,
                0,
            ),
            handshake(
                "handshake/packed-datagram2",
                0x0102_0304_0506_0708,
                FORMAT_PACKED,
                FRAMING_DATAGRAM2,
                0,
            ),
            handshake(
                "handshake/packed-datagram3",
                0x0102_0304_0506_0708,
                FORMAT_PACKED,
                FRAMING_DATAGRAM3,
                0,
            ),
            handshake(
                "handshake/json-jsonrpc",
                u64::MAX,
                FORMAT_JSON,
                FRAMING_JSONRPC,
                u32::MAX,
            ),
        ],
    );

    feature(
        "envelope",
        vec![
            ("envelope/ok".into(), Input::Envelope(Some(ok(b"payload")))),
            ("envelope/ok-empty".into(), Input::Envelope(Some(ok(b"")))),
            (
                "envelope/err".into(),
                Input::Envelope(Some(err(0x0102, b"fields"))),
            ),
            (
                "envelope/err-empty-body".into(),
                Input::Envelope(Some(err(0, b""))),
            ),
            (
                "envelope/fault".into(),
                Input::Envelope(Some(Reply::Fault)),
            ),
        ],
    );

    let params = Value::Array(vec![Value::U32(7), Value::String("x".into())]);
    for (feature_id, framing) in [
        ("framing-datagram", FRAMING_DATAGRAM),
        ("framing-jsonrpc", FRAMING_JSONRPC),
    ] {
        let p = |id: &str| format!("{feature_id}/{id}");
        let mut connection = Vec::new();
        if framing == FRAMING_JSONRPC {
            // On a connection: a qualified method names its protocol, and a
            // one-way call is a notification, with no `id`.
            let said = Value::Array(vec![Value::String("hi".into())]);
            connection.push(request(
                p("request-qualified-method"),
                framing,
                0,
                "chat.Chat/send",
                5,
                &said,
            ));
            let (id, mut input) = request(
                p("notification"),
                framing,
                0,
                "chat.ChatClient/pushed",
                0,
                &said,
            );
            if let Input::Request(Framed::Full(i)) = &mut input {
                i.one_way = true;
            }
            connection.push((id, input));
            // JSON-RPC 2.0 has `params` omitted when there are none, as for a
            // call without arguments, and never `null`.
            connection.push(request(
                p("request-no-params"),
                framing,
                0,
                "ping",
                8,
                &Value::Unit(()),
            ));
            let (id, mut input) = request(p("notification-no-params"), framing, 0, "ping", 0, &Value::Unit(()));
            if let Input::Request(Framed::Full(i)) = &mut input {
                i.one_way = true;
            }
            connection.push((id, input));
            // Found by comline-typescript's differential tests (#41): a
            // name a JSON string can hold only escaped is written escaped.
            connection.push(request(
                p("found-request-method-needs-escapes"),
                framing,
                0,
                "app.Chat/say \"hi\" \\ \n\t\u{1}\u{1f}\u{7f} \u{e9}\u{1f600}",
                6,
                &said,
            ));
        }
        let mut binary = Vec::new();
        if framing == FRAMING_DATAGRAM {
            for (name, format) in [("msgpack", FORMAT_MSGPACK), ("packed", FORMAT_PACKED)] {
                let id = p(&format!("request-params-{name}"));
                binary.push(with_format(request(id, framing, 3, "greet", 42, &params), format));
            }
        }
        feature(
            feature_id,
            [
                vec![
                    request(p("request"), framing, 3, "greet", 42, &params),
                    request(
                        p("request-large-id"),
                        framing,
                        u16::MAX,
                        "ping",
                        u64::MAX,
                        &Value::Array(vec![]),
                    ),
                    response(p("response-ok"), framing, 7, ok(br#"{"body":"hi"}"#)),
                    // Payloads pass through as written, digits and all.
                    response(
                        p("response-ok-exact-numbers"),
                        framing,
                        8,
                        ok(br#"{"n":9007199254740993,"f":1.0}"#),
                    ),
                    response(p("response-err"), framing, 7, err(2, br#"{"why":"no"}"#)),
                    response(p("response-fault"), framing, 9, Reply::Fault),
                ],
                connection,
                binary,
            ]
            .concat(),
        );
    }

    let p = |id: &str| format!("framing-datagram2/{id}");
    let on = |(id, mut input): (String, Input), protocol: u16| {
        match &mut input {
            Input::Request(Framed::Full(i)) => i.protocol = Some(protocol),
            Input::Response(Framed::Full(i)) => i.protocol = Some(protocol),
            _ => unreachable!("only requests and responses carry a protocol"),
        }
        (id, input)
    };
    feature(
        "framing-datagram2",
        vec![
            on(
                request(p("request"), FRAMING_DATAGRAM2, 3, "greet", 42, &params),
                0,
            ),
            // 300 is two varint bytes, the largest protocol number three.
            on(
                request(
                    p("request-two-byte-protocol"),
                    FRAMING_DATAGRAM2,
                    1,
                    "ping",
                    5,
                    &Value::Array(vec![]),
                ),
                300,
            ),
            on(
                request(
                    p("request-large-ids"),
                    FRAMING_DATAGRAM2,
                    u16::MAX,
                    "ping",
                    u64::MAX,
                    &Value::Array(vec![]),
                ),
                u16::MAX,
            ),
            on(
                with_format(
                    request(p("request-params-msgpack"), FRAMING_DATAGRAM2, 3, "greet", 42, &params),
                    FORMAT_MSGPACK,
                ),
                0,
            ),
            on(
                with_format(
                    request(p("request-params-packed"), FRAMING_DATAGRAM2, 3, "greet", 42, &params),
                    FORMAT_PACKED,
                ),
                0,
            ),
            on(
                response(
                    p("response-ok"),
                    FRAMING_DATAGRAM2,
                    7,
                    ok(br#"{"body":"hi"}"#),
                ),
                0,
            ),
            on(
                response(
                    p("response-err"),
                    FRAMING_DATAGRAM2,
                    7,
                    err(2, br#"{"why":"no"}"#),
                ),
                1,
            ),
            on(
                response(p("response-fault"), FRAMING_DATAGRAM2, 9, Reply::Fault),
                200,
            ),
        ],
    );

    // datagram@3: calls by wire number, both numbers varints.
    let p = |id: &str| format!("framing-datagram3/{id}");
    let d3 = on;
    feature(
        "framing-datagram3",
        vec![
            d3(request(p("request"), FRAMING_DATAGRAM3, 3, "greet", 42, &params), 0),
            // 300 is two varint bytes, 65535 three.
            d3(
                request(
                    p("request-two-byte-numbers"),
                    FRAMING_DATAGRAM3,
                    300,
                    "ping",
                    5,
                    &Value::Array(vec![]),
                ),
                300,
            ),
            d3(
                request(
                    p("request-large-ids"),
                    FRAMING_DATAGRAM3,
                    u16::MAX,
                    "ping",
                    u64::MAX,
                    &Value::Array(vec![]),
                ),
                u16::MAX,
            ),
            d3(
                with_format(
                    request(p("request-params-msgpack"), FRAMING_DATAGRAM3, 3, "greet", 42, &params),
                    FORMAT_MSGPACK,
                ),
                0,
            ),
            d3(
                with_format(
                    request(p("request-params-packed"), FRAMING_DATAGRAM3, 3, "greet", 42, &params),
                    FORMAT_PACKED,
                ),
                0,
            ),
            d3(response(p("response-ok"), FRAMING_DATAGRAM3, 7, ok(br#"{"body":"hi"}"#)), 0),
            d3(response(p("response-err"), FRAMING_DATAGRAM3, 7, err(2, br#"{"why":"no"}"#)), 1),
            d3(response(p("response-fault"), FRAMING_DATAGRAM3, 9, Reply::Fault), 200),
        ],
    );

    feature("stream", stream_cases());

    feature("format-json", format_cases("format-json", FORMAT_JSON));
    feature(
        "format-msgpack",
        format_cases("format-msgpack", FORMAT_MSGPACK),
    );
    feature(
        "format-packed",
        [format_cases("format-packed", FORMAT_PACKED), packed_cases()].concat(),
    );

    for v in &mut vectors {
        v.cases.extend(rejections(&v.feature));
        if let Some(format) = match v.feature.as_str() {
            "format-json" => Some(FORMAT_JSON),
            "format-msgpack" => Some(FORMAT_MSGPACK),
            "format-packed" => Some(FORMAT_PACKED),
            _ => None,
        } {
            v.cases.extend(char_cases(&v.feature, format));
            v.cases.extend(bound_cases(&v.feature, format));
        }
        match v.feature.as_str() {
            "format-json" => {
                v.cases.extend(decode_cases("format-json", FORMAT_JSON));
                v.cases.extend(found_cases("format-json", FORMAT_JSON));
            }
            "format-msgpack" => v
                .cases
                .extend(decode_cases("format-msgpack", FORMAT_MSGPACK)),
            "format-packed" => v.cases.extend(decode_cases("format-packed", FORMAT_PACKED)),
            "framing-jsonrpc" => {
                v.cases.extend(found_jsonrpc_cases());
                v.cases.extend(jsonrpc_unknown_members());
            }
            "handshake" => v.cases.extend(handshake_check_cases()),
            _ => {}
        }
    }
    vectors.push(Vectors {
        schema: Some(VECTORS_SCHEMA.into()),
        feature: "handshake-comline2".into(),
        cases: comline2::cases(),
    });
    vectors
}

fn exact(id: String, input: Input) -> Case {
    let bytes = match encode(&input) {
        Ok(bytes) => bytes,
        Err(outcome) => panic!("reference case `{id}` doesn't encode: {outcome:?}"),
    };
    let json_text = match &input {
        Input::Encode(i) => i.format == FORMAT_JSON,
        Input::Request(Framed::Full(i)) => i.framing == FRAMING_JSONRPC,
        Input::Response(Framed::Full(i)) => i.framing == FRAMING_JSONRPC,
        _ => false,
    };
    Case {
        id,
        input,
        bytes: Hex(bytes),
        expect: Expect::Exact,
        compare: if json_text {
            Compare::Json
        } else {
            Compare::Bytes
        },
    }
}

fn reject(id: &str, input: Input, bytes: &[u8]) -> Case {
    Case {
        id: id.into(),
        input,
        bytes: Hex(bytes.to_vec()),
        expect: Expect::Reject,
        compare: Compare::Bytes,
    }
}

fn ok(payload: &[u8]) -> Reply {
    Reply::Ok {
        payload: Hex(payload.to_vec()),
    }
}

fn err(id: u16, body: &[u8]) -> Reply {
    Reply::Err {
        id,
        body: Hex(body.to_vec()),
    }
}

fn handshake(
    id: &str,
    ir_hash: u64,
    wire_format: &str,
    framing: &str,
    capabilities: u32,
) -> (String, Input) {
    let input = HandshakeIn {
        ir_hash,
        wire_format: wire_format.into(),
        framing: framing.into(),
        capabilities,
    };
    (id.into(), Input::Handshake(Some(input)))
}

/// One end's `comline@1` declaration against the frame its peer sent:
/// agreeing (capability bits differ, which is allowed), then a peer that
/// differs in the schema fingerprint, the format, or the framing alone. The
/// cases `comline@1`'s profile proves its facts by.
fn handshake_check_cases() -> Vec<Case> {
    let ours = HandshakeIn {
        ir_hash: 1,
        wire_format: FORMAT_JSON.into(),
        framing: FRAMING_DATAGRAM.into(),
        capabilities: 0,
    };
    let peer = |ir_hash: u64, format: &str, framing: &str, capabilities: u32| {
        let mut bytes = Vec::new();
        HandshakeFrame::new(ir_hash, format, framing, capabilities).encode(&mut bytes);
        bytes
    };
    let check = |id: &str, bytes: Vec<u8>| {
        let case = Case {
            id: format!("handshake/{id}"),
            input: Input::HandshakeCheck(ours.clone()),
            bytes: Hex(bytes),
            expect: Expect::Exact,
            compare: Compare::Bytes,
        };
        match decode(&case.input, &case.bytes.0) {
            Ok(Some(_)) => case,
            Ok(None) => Case {
                expect: Expect::Reject,
                ..case
            },
            Err(outcome) => panic!("reference case `{id}` doesn't check: {outcome:?}"),
        }
    };
    vec![
        check("check-agree", peer(1, FORMAT_JSON, FRAMING_DATAGRAM, 0b101)),
        check(
            "check-reject-ir-hash",
            peer(2, FORMAT_JSON, FRAMING_DATAGRAM, 0),
        ),
        check(
            "check-reject-format",
            peer(1, FORMAT_MSGPACK, FRAMING_DATAGRAM, 0),
        ),
        check(
            "check-reject-framing",
            peer(1, FORMAT_JSON, FRAMING_DATAGRAM2, 0),
        ),
    ]
}

fn request(
    id: String,
    framing: &str,
    call_id: u16,
    method: &str,
    request_id: u64,
    params: &Value,
) -> (String, Input) {
    let input = RequestIn {
        framing: framing.into(),
        protocol: None,
        call_id,
        method: method.into(),
        request_id,
        one_way: false,
        format: FORMAT_JSON.into(),
        params: params.clone(),
    };
    (id, Input::Request(Framed::Full(input)))
}

/// The same call with its params in `format`, for a framing that carries them
/// as bytes.
fn with_format((id, mut input): (String, Input), format: &str) -> (String, Input) {
    if let Input::Request(Framed::Full(i)) = &mut input {
        i.format = format.into();
    }
    (id, input)
}

fn response(
    id: String,
    framing: &str,
    request_id: u64,
    reply: Reply,
) -> (String, Input) {
    let input = ResponseIn {
        framing: framing.into(),
        protocol: None,
        request_id,
        reply,
    };
    (id, Input::Response(Framed::Full(input)))
}

/// Sample values for a wire format: each scalar at its edges, then the
/// shapes generated types take.
fn format_cases(feature_id: &str, format: &str) -> Vec<(String, Input)> {
    let point = |x: i32, y: i32| Value::Struct {
        name: "Point".into(),
        fields: vec![("x".into(), Value::I32(x)), ("y".into(), Value::I32(y))],
    };
    let samples: Vec<(&str, Value)> = vec![
        ("bool-true", Value::Bool(true)),
        ("bool-false", Value::Bool(false)),
        ("u8-max", Value::U8(u8::MAX)),
        ("u16-max", Value::U16(u16::MAX)),
        ("u32-max", Value::U32(u32::MAX)),
        ("u64-small", Value::U64(7)),
        ("u64-above-2-53", Value::U64((1 << 53) + 1)),
        ("u64-max", Value::U64(u64::MAX)),
        ("i8-min", Value::I8(i8::MIN)),
        ("i16-min", Value::I16(i16::MIN)),
        ("i32-min", Value::I32(i32::MIN)),
        ("i64-min", Value::I64(i64::MIN)),
        ("f64-fraction", Value::F64(-0.25)),
        ("f64-integral", Value::F64(1.0)),
        ("string-empty", Value::String(String::new())),
        (
            "string-unicode",
            Value::String("héllo \"wörld\" ✓\n".into()),
        ),
        ("array-empty", Value::Array(vec![])),
        (
            "array-u8",
            Value::Array(vec![Value::U8(1), Value::U8(2), Value::U8(3)]),
        ),
        ("option-none", Value::Option(None)),
        ("option-some", Value::Option(Some(Box::new(Value::U32(5))))),
        ("struct", point(1, -2)),
        (
            "struct-nested",
            Value::Struct {
                name: "Line".into(),
                fields: vec![
                    ("from".into(), point(0, 0)),
                    ("to".into(), point(3, 4)),
                    (
                        "label".into(),
                        Value::Option(Some(Box::new(Value::String("diag".into())))),
                    ),
                    ("tags".into(), Value::Array(vec![Value::String("a".into())])),
                ],
            },
        ),
        (
            "struct-optional-none",
            Value::Struct {
                name: "Tagged".into(),
                fields: vec![
                    ("id".into(), Value::U32(1)),
                    ("label".into(), Value::Option(None)),
                ],
            },
        ),
        (
            "enum-unit-variant",
            Value::UnitVariant {
                enum_name: "Color".into(),
                index: 1,
                variant: "Green".into(),
            },
        ),
        ("u128-max", Value::U128(u128::MAX)),
        ("i128-min", Value::I128(i128::MIN)),
        (
            "tuple-u8",
            Value::Tuple(vec![Value::U8(1), Value::U8(2), Value::U8(3), Value::U8(4)]),
        ),
        // Longer than 32: generated Rust needs `serde_big_array` for it.
        ("tuple-u8-33", Value::Tuple(vec![Value::U8(7); 33])),
        (
            "array-of-options",
            Value::Array(vec![
                Value::Option(Some(Box::new(Value::U16(1)))),
                Value::Option(None),
            ]),
        ),
        ("unit", Value::Unit(())),
        // A single `char`, 1 to 4 bytes of UTF-8: raw in `packed@1`, a
        // one-character string in `msgpack@1` and `json@1`.
        ("char-ascii", Value::Char('a')),
        ("char-2-byte", Value::Char('é')),
        ("char-3-byte", Value::Char('€')),
        ("char-4-byte", Value::Char('😀')),
    ];
    samples
        .into_iter()
        .map(|(id, value)| {
            let input = EncodeIn {
                format: format.into(),
                ty: None,
                value,
            };
            (format!("{feature_id}/{id}"), Input::Encode(input))
        })
        .collect()
}

/// Shapes only `packed@1` vectors carry: widths the other formats don't
/// keep apart (`f32`), a fixed-size array, and the proposal's example.
fn packed_cases() -> Vec<(String, Input)> {
    let class = |index: u32, variant: &str| Value::UnitVariant {
        enum_name: "Class".into(),
        index,
        variant: variant.into(),
    };
    let samples: Vec<(&str, Value)> = vec![
        ("f32", Value::F32(1.5)),
        ("u16-small", Value::U16(1)),
        (
            "tuple-f32",
            Value::Tuple(vec![Value::F32(1.0), Value::F32(2.0)]),
        ),
        // 300 elements: a count that takes two varint bytes.
        (
            "array-two-byte-count",
            Value::Array(vec![Value::U8(0); 300]),
        ),
        ("enum-first-variant", class(0, "Warrior")),
        (
            "struct-player",
            Value::Struct {
                name: "Player".into(),
                fields: vec![
                    ("id".into(), Value::U32(7)),
                    ("hp".into(), Value::U16(100)),
                    ("name".into(), Value::String("ab".into())),
                    ("guild".into(), Value::Option(None)),
                    ("class".into(), class(1, "Mage")),
                    (
                        "pos".into(),
                        Value::Tuple(vec![Value::F32(1.0), Value::F32(2.0)]),
                    ),
                ],
            },
        ),
    ];
    samples
        .into_iter()
        .map(|(id, value)| {
            let input = EncodeIn {
                format: FORMAT_PACKED.into(),
                ty: None,
                value,
            };
            (format!("format-packed/{id}"), Input::Encode(input))
        })
        .collect()
}

// --- Decoding what another version wrote ---

fn point_type() -> Type {
    Type::Struct {
        name: "Point".into(),
        fields: vec![("x".into(), Type::I32), ("y".into(), Type::I32)],
    }
}

fn tagged_type() -> Type {
    Type::Struct {
        name: "Tagged".into(),
        fields: vec![
            ("id".into(), Type::U32),
            ("label".into(), Type::Option(Box::new(Type::String))),
        ],
    }
}

fn color_type() -> Type {
    Type::Enum {
        name: "Color".into(),
        variants: vec!["Red".into(), "Green".into(), "Blue".into()],
    }
}

/// Bytes `written` encodes to, read back as `ty`: an exact case with the
/// value the runtime decoded, or a rejection if it refused them.
fn decode_case(id: String, format: &str, written: &Value, ty: Type) -> Case {
    let bytes = encode_with(format, written).expect("reference formats encode");
    match decode_typed(format, &ty, &bytes) {
        Ok(Some(value)) => Case {
            id,
            input: Input::Decode(DecodeIn {
                format: format.into(),
                ty,
                value: Some(value),
            }),
            bytes: Hex(bytes),
            expect: Expect::Exact,
            compare: Compare::Bytes,
        },
        Ok(None) => reject(
            &id,
            Input::Decode(DecodeIn {
                format: format.into(),
                ty,
                value: None,
            }),
            &bytes,
        ),
        Err(outcome) => panic!("reference case `{id}` doesn't decode: {outcome:?}"),
    }
}

// --- A single `char`, and bounds (core#98) ---

/// `bytes` read as `ty`: an exact `decode` case with what the reference
/// decoded, or a rejection, which `rejected` says it must be.
fn decode_bytes_case(id: String, format: &str, bytes: &[u8], ty: Type, rejected: bool) -> Case {
    let value = decode_typed(format, &ty, bytes)
        .unwrap_or_else(|outcome| panic!("reference case `{id}` doesn't decode: {outcome:?}"));
    assert_eq!(value.is_none(), rejected, "reference case `{id}`");
    let case = |value| {
        Input::Decode(DecodeIn {
            format: format.into(),
            ty: ty.clone(),
            value,
        })
    };
    match value {
        Some(value) => Case {
            id,
            input: case(Some(value)),
            bytes: Hex(bytes.to_vec()),
            expect: Expect::Exact,
            compare: Compare::Bytes,
        },
        None => reject(&id, case(None), bytes),
    }
}

/// A single `char` read back, and bytes that aren't exactly one character.
fn char_cases(feature_id: &str, format: &str) -> Vec<Case> {
    let p = |id: &str| format!("{feature_id}/{id}");
    let mut cases: Vec<Case> = [
        ("ascii", 'a'),
        ("2-byte", 'é'),
        ("3-byte", '€'),
        ("4-byte", '😀'),
    ]
    .into_iter()
    .map(|(id, c)| {
        decode_case(
            p(&format!("decode-char-{id}")),
            format,
            &Value::Char(c),
            Type::Char,
        )
    })
    .collect();
    let bad: &[(&str, &[u8])] = match format {
        FORMAT_PACKED => &[
            ("empty", b""),
            ("continuation-byte", b"\x80"),
            ("truncated", b"\xc3"),
            ("overlong", b"\xc0\x80"),
            ("surrogate", b"\xed\xa0\x80"),
            ("past-max", b"\xf4\x90\x80\x80"),
            // A character, then one more byte: a value uses every byte.
            ("two", b"ab"),
        ],
        FORMAT_MSGPACK => &[
            ("empty", b"\xa0"),
            ("two", b"\xa2ab"),
            ("overlong", b"\xa2\xc0\x80"),
            ("surrogate", b"\xa3\xed\xa0\x80"),
            ("bin", b"\xc4\x01a"),
            ("integer", b"\x61"),
        ],
        FORMAT_JSON => &[
            ("empty", b"\"\""),
            ("two", b"\"ab\""),
            ("lone-surrogate", b"\"\\ud800\""),
            ("integer", b"97"),
        ],
        _ => &[],
    };
    cases.extend(bad.iter().map(|(id, bytes)| {
        decode_bytes_case(
            p(&format!("decode-char-reject-{id}")),
            format,
            bytes,
            Type::Char,
            true,
        )
    }));
    cases
}

/// Bounds: a value at its type's maximum, one over it, multi-byte
/// characters near it, and the depth limit, on decode and on encode.
fn bound_cases(feature_id: &str, format: &str) -> Vec<Case> {
    let p = |id: &str| format!("{feature_id}/{id}");
    let string = |s: &str| Value::String(s.into());
    let chars = |max| Type::BoundedString { max };
    let u8s = |items: &[u8]| Value::Array(items.iter().copied().map(Value::U8).collect());
    let items = |of: Type, max| Type::BoundedArray {
        of: Box::new(of),
        max,
    };
    // `@max_depth = 3`: `struct Node { next: optional Node }`.
    let chain_type = || Type::Recursive {
        max_depth: 3,
        of: Box::new(Type::Struct {
            name: "Node".into(),
            fields: vec![("next".into(), Type::Option(Box::new(Type::SelfRef)))],
        }),
    };
    let chain = |n: usize| {
        (0..n).fold(Value::Option(None), |next, _| {
            Value::Option(Some(Box::new(Value::Struct {
                name: "Node".into(),
                fields: vec![("next".into(), next)],
            })))
        })
    };
    let chain = |n: usize| match chain(n) {
        Value::Option(Some(node)) => *node,
        _ => unreachable!("n > 0"),
    };
    // `@max_depth = 2`: `struct Tree { children: Tree[max 2] }`.
    let tree_type = || Type::Recursive {
        max_depth: 2,
        of: Box::new(Type::Struct {
            name: "Tree".into(),
            fields: vec![("children".into(), items(Type::SelfRef, 2))],
        }),
    };
    let tree = |children: Vec<Value>| Value::Struct {
        name: "Tree".into(),
        fields: vec![("children".into(), Value::Array(children))],
    };
    let leaf = || tree(vec![]);

    let read: Vec<(&str, Value, Type)> = vec![
        ("chars-empty", string(""), chars(4)),
        ("chars-at-max", string("abcd"), chars(4)),
        ("chars-over-max", string("abcde"), chars(4)),
        // 4 characters in 10 bytes: under by characters, over by bytes.
        ("chars-multibyte-at-max", string("é€😀a"), chars(4)),
        // 4 characters in 16 bytes, 4 × N: the most N characters take.
        ("chars-4-byte-at-max", string("😀😀😀😀"), chars(4)),
        // 5 characters in 10 bytes: over by characters, under 4 × N bytes.
        ("chars-multibyte-over-max", string("ééééé"), chars(4)),
        // 17 bytes: a length over 4 × N, refused before it's read.
        ("chars-bytes-over-4n", string("😀😀😀😀a"), chars(4)),
        ("chars-max-0", string(""), chars(0)),
        ("chars-max-0-over", string("a"), chars(0)),
        ("items-empty", u8s(&[]), items(Type::U8, 3)),
        ("items-at-max", u8s(&[1, 2, 3]), items(Type::U8, 3)),
        ("items-over-max", u8s(&[1, 2, 3, 4]), items(Type::U8, 3)),
        (
            "items-of-chars-at-max",
            Value::Array(vec![string("ab"), string("é😀")]),
            items(chars(2), 2),
        ),
        (
            "items-of-chars-over-max",
            Value::Array(vec![string("ab"), string("abc")]),
            items(chars(2), 2),
        ),
        ("depth-at-max", chain(3), chain_type()),
        ("depth-over-max", chain(4), chain_type()),
        ("depth-tree-at-max", tree(vec![leaf(), leaf()]), tree_type()),
        (
            "depth-tree-over-max",
            tree(vec![leaf(), tree(vec![leaf()])]),
            tree_type(),
        ),
    ];
    let mut cases: Vec<Case> = read
        .iter()
        .map(|(id, written, ty)| {
            decode_case(p(&format!("decode-{id}")), format, written, ty.clone())
        })
        .collect();
    for case in &cases {
        let over = case.id.contains("over");
        assert_eq!(
            case.expect == Expect::Reject,
            over,
            "reference case `{}`",
            case.id
        );
    }

    // The same values encoded as their types: within the bounds, the bytes;
    // over them, refused before anything is written.
    for (id, value, ty) in read {
        let id = p(&format!("encode-{id}"));
        let input = Input::Encode(EncodeIn {
            format: format.into(),
            ty: Some(ty),
            value,
        });
        cases.push(if id.contains("over") {
            reject(&id, input, &[])
        } else {
            exact(id, input)
        });
    }
    cases
}

/// What a newer or older peer wrote, read by today's types: the cases an
/// encoding profile's facts point to.
fn decode_cases(feature_id: &str, format: &str) -> Vec<Case> {
    let structure = |name: &str, fields: Vec<(&str, Value)>| Value::Struct {
        name: name.into(),
        fields: fields.into_iter().map(|(f, v)| (f.into(), v)).collect(),
    };
    let variant = |index: u32, variant: &str| Value::UnitVariant {
        enum_name: "Color".into(),
        index,
        variant: variant.into(),
    };
    let p = |id: &str| format!("{feature_id}/{id}");
    let bytes = |n: u8| Value::Tuple((1..=n).map(Value::U8).collect());
    let fixed = |len: usize| Type::Fixed {
        of: Box::new(Type::U8),
        len,
    };
    vec![
        // Written with the fields the other way round: by name, x stays 1;
        // by position, x reads the written y.
        decode_case(
            p("decode-fields-reordered"),
            format,
            &structure("Point", vec![("y", Value::I32(-2)), ("x", Value::I32(1))]),
            point_type(),
        ),
        // Written with a field this reader doesn't have.
        decode_case(
            p("decode-unknown-field"),
            format,
            &structure(
                "Point",
                vec![
                    ("x", Value::I32(1)),
                    ("y", Value::I32(-2)),
                    ("z", Value::I32(5)),
                ],
            ),
            point_type(),
        ),
        // Written before the optional `label` existed.
        decode_case(
            p("decode-missing-optional"),
            format,
            &structure("Tagged", vec![("id", Value::U32(1))]),
            tagged_type(),
        ),
        // Written by an enum where `Green` came first: by name it's still
        // Green; by index it would read as Red.
        decode_case(
            p("decode-variant-reordered"),
            format,
            &variant(0, "Green"),
            color_type(),
        ),
        // Written by an enum with a variant this reader doesn't have.
        decode_case(
            p("decode-unknown-variant"),
            format,
            &variant(3, "Purple"),
            color_type(),
        ),
        // A `u64` field read as `u32`: decodes while the value fits.
        decode_case(p("decode-wider-integer"), format, &Value::U64(7), Type::U32),
        decode_case(
            p("decode-wider-integer-overflow"),
            format,
            &Value::U64(1 << 40),
            Type::U32,
        ),
        // The shapes beyond the scalars a generated type can have.
        decode_case(
            p("decode-u128"),
            format,
            &Value::U128(u128::MAX),
            Type::U128,
        ),
        decode_case(
            p("decode-i128"),
            format,
            &Value::I128(i128::MIN),
            Type::I128,
        ),
        decode_case(p("decode-unit"), format, &Value::Unit(()), Type::Unit),
        decode_case(p("decode-fixed-array"), format, &bytes(4), fixed(4)),
        decode_case(p("decode-fixed-array-33"), format, &bytes(33), fixed(33)),
        // A fixed array written one element short, or one too long.
        decode_case(p("decode-fixed-array-short"), format, &bytes(3), fixed(4)),
        decode_case(p("decode-fixed-array-long"), format, &bytes(5), fixed(4)),
        decode_case(
            p("decode-array-of-options"),
            format,
            &Value::Array(vec![
                Value::Option(Some(Box::new(Value::U16(1)))),
                Value::Option(None),
            ]),
            Type::Array(Box::new(Type::Option(Box::new(Type::U16)))),
        ),
    ]
}

/// Inputs the property tests and fuzzers found (`tests/properties.rs`,
/// `runtime/fuzz`), minimized, as `found-*` cases. See `runtime/fuzz/README.md`.
fn found_cases(feature_id: &str, format: &str) -> Vec<Case> {
    let p = |id: &str| format!("{feature_id}/found-{id}");
    // A value that must read back as itself, not only as whatever the
    // reference happened to decode.
    let exact = |id: &str, written: Value, ty: Type| {
        let case = decode_case(p(id), format, &written, ty);
        let Input::Decode(DecodeIn { value, .. }) = &case.input else {
            unreachable!("`decode_case` makes `decode` cases");
        };
        assert_eq!(value.as_ref(), Some(&written), "`{}` doesn't read back", case.id);
        case
    };
    match format {
        // serde_json's default float parser can be one ulp off; the
        // runtime enables `float_roundtrip`, which reads every `f64` that
        // was written back exactly.
        FORMAT_JSON => vec![exact(
            "f64-exact-digits",
            Value::F64(4.520618856987477e-279),
            Type::F64,
        )],
        _ => vec![],
    }
}

/// JSON-RPC frames the reference writes differently but must read, found by
/// comline-typescript's differential tests (#41). They compare as JSON, so
/// encoding the input must give the same JSON value as `bytes`, spelled
/// however the runtime spells it.
fn found_jsonrpc_cases() -> Vec<Case> {
    let spelled = |id: &str, method: &str, bytes: &[u8]| {
        let (id, input) = request(
            format!("framing-jsonrpc/found-{id}"),
            FRAMING_JSONRPC,
            0,
            method,
            1,
            &Value::Array(vec![]),
        );
        let case = Case {
            id,
            input,
            bytes: Hex(bytes.to_vec()),
            expect: Expect::Exact,
            compare: Compare::Json,
        };
        assert_ne!(
            encode(&case.input).ok().as_deref(),
            Some(bytes),
            "`{}` is the reference's own spelling",
            case.id
        );
        case
    };
    vec![
        // `/` escaped, as PHP's `json_encode` writes it by default.
        spelled(
            "request-method-escaped-slash",
            "app.Chat/count",
            br#"{"jsonrpc":"2.0","method":"app.Chat\/count","params":[],"id":1}"#,
        ),
        spelled(
            "request-method-unicode-escapes",
            "greet\u{e9}\u{1f600}",
            br#"{"jsonrpc":"2.0","method":"\u0067reet\u00E9\ud83d\ude00","params":[],"id":1}"#,
        ),
    ]
}

/// `"params": null`, which this framing no longer writes: JSON-RPC 2.0 doesn't
/// allow it, but peers send it, and it can only mean none.
fn accept_params_null() -> Case {
    let (id, input) = request(
        "framing-jsonrpc/accept-request-params-null".into(),
        FRAMING_JSONRPC,
        0,
        "ping",
        8,
        &Value::Unit(()),
    );
    Case {
        id,
        input,
        bytes: Hex(br#"{"jsonrpc":"2.0","method":"ping","params":null,"id":8}"#.to_vec()),
        expect: Expect::Accept,
        compare: Compare::Bytes,
    }
}

/// `framing-jsonrpc/request` and its responses with the payload in a format
/// that isn't JSON: `jsonrpc@1` embeds a payload as a JSON value, so the frame
/// isn't JSON-RPC. `payload: json` is proven by these.
fn non_json_payloads() -> Vec<Case> {
    let request_for = || Input::Request(Framed::Selector { framing: FRAMING_JSONRPC.into() });
    let response_for = || Input::Response(Framed::Selector { framing: FRAMING_JSONRPC.into() });
    let params = Value::Array(vec![Value::U32(7), Value::String("x".into())]);
    let mut cases = Vec::new();
    for (name, format) in [("msgpack", FORMAT_MSGPACK), ("packed", FORMAT_PACKED)] {
        let bytes = encode_with(format, &params).expect("the params encode");
        let framed = |head: &str, tail: &str| [head.as_bytes(), &bytes, tail.as_bytes()].concat();
        cases.push(reject(
            &format!("framing-jsonrpc/reject-request-params-{name}"),
            request_for(),
            &framed(r#"{"jsonrpc":"2.0","method":"greet","params":"#, r#","id":42}"#),
        ));
        cases.push(reject(
            &format!("framing-jsonrpc/reject-response-result-{name}"),
            response_for(),
            &framed(r#"{"jsonrpc":"2.0","result":"#, r#","id":7}"#),
        ));
        cases.push(reject(
            &format!("framing-jsonrpc/reject-response-error-data-{name}"),
            response_for(),
            &framed(
                r#"{"jsonrpc":"2.0","error":{"code":2,"message":"application error","data":"#,
                r#"},"id":7}"#,
            ),
        ));
    }
    cases
}

/// Members JSON-RPC 2.0 doesn't name, in a frame otherwise like the reference's
/// own. The specification doesn't forbid them, so a reader ignores them and an
/// ordinary JSON-RPC peer can send them.
fn jsonrpc_unknown_members() -> Vec<Case> {
    let accept = |(id, input): (String, Input), bytes: &[u8]| Case {
        id,
        input,
        bytes: Hex(bytes.to_vec()),
        expect: Expect::Accept,
        compare: Compare::Bytes,
    };
    let params = Value::Array(vec![Value::U32(7), Value::String("x".into())]);
    let p = |id: &str| format!("framing-jsonrpc/{id}");
    vec![
        accept(
            request(p("accept-request-unknown-member"), FRAMING_JSONRPC, 0, "greet", 42, &params),
            br#"{"jsonrpc":"2.0","method":"greet","params":[7,"x"],"x":[1],"id":42}"#,
        ),
        accept(
            response(p("accept-response-unknown-member"), FRAMING_JSONRPC, 7, ok(br#"{"body":"hi"}"#)),
            br#"{"jsonrpc":"2.0","result":{"body":"hi"},"x":[1],"id":7}"#,
        ),
        accept(
            response(
                p("accept-response-error-unknown-member"),
                FRAMING_JSONRPC,
                7,
                Reply::Err { id: 2, body: Hex(br#"{"why":"no"}"#.to_vec()) },
            ),
            br#"{"jsonrpc":"2.0","error":{"code":2,"message":"application error","x":[1],"data":{"why":"no"}},"id":7}"#,
        ),
    ]
}

/// Stream framing (`contract::stream`): length prefixes, and streams split
/// into frames, at the default limit and at a small one, on both sides of
/// each.
fn stream_cases() -> Vec<(String, Input)> {
    let default = DEFAULT_MAX_FRAME as u64;
    let ceiling = u64::from(u32::MAX);
    let prefix = |id: &str, len: u64, max: u64| {
        (
            format!("stream/{id}"),
            Input::StreamPrefix(StreamPrefixIn { len, max }),
        )
    };
    let split = |id: &str, max: u64, frames: &[&[u8]], rest: &[u8], needed: u64| {
        let frames: Vec<Hex> = frames.iter().map(|f| Hex(f.to_vec())).collect();
        (
            format!("stream/{id}"),
            Input::StreamSplit(StreamSplit::Frames(StreamSplitIn {
                max,
                frames,
                rest: Hex(rest.to_vec()),
                needed,
            })),
        )
    };
    let at_default_max = (DEFAULT_MAX_FRAME as u32).to_le_bytes();
    vec![
        prefix("prefix-zero", 0, default),
        prefix("prefix-small", 3, default),
        prefix("prefix-le", 0x0001_0203, default),
        prefix("prefix-default-max", default, default),
        prefix("prefix-small-max", 2, 2),
        prefix("prefix-u32-max", ceiling, ceiling),
        split("split-empty", default, &[], &[], 0),
        split("split-partial-prefix-1", default, &[], &[3], 0),
        split("split-partial-prefix-3", default, &[], &[3, 0, 0], 0),
        split("split-prefix-only", default, &[], &[3, 0, 0, 0], 7),
        split("split-partial-body", default, &[], &[3, 0, 0, 0, b'a'], 7),
        split("split-one-frame", default, &[b"abc"], &[], 0),
        split("split-zero-length", default, &[b""], &[], 0),
        split("split-two-frames", default, &[b"ab", b"c"], &[], 0),
        split("split-frame-then-partial", default, &[b"ab"], &[1, 0], 0),
        split("split-small-max", 2, &[b"ab", b""], &[2, 0, 0, 0, b'c'], 6),
        split(
            "split-default-max-prefix",
            default,
            &[],
            &at_default_max,
            PREFIX_LEN as u64 + default,
        ),
    ]
}

/// Bytes a runtime must refuse, per feature.
fn rejections(feature: &str) -> Vec<Case> {
    let mut handshake = Vec::new();
    HandshakeFrame::new(1, FORMAT_JSON, FRAMING_DATAGRAM, 0).encode(&mut handshake);
    let request_for = |name: &str| {
        Input::Request(Framed::Selector {
            framing: name.into(),
        })
    };
    let response_for = |name: &str| {
        Input::Response(Framed::Selector {
            framing: name.into(),
        })
    };
    match feature {
        "handshake" => {
            let mut bad_magic = handshake.clone();
            bad_magic[0] = b'X';
            let mut bad_version = handshake.clone();
            bad_version[2] = 2;
            vec![
                reject("handshake/reject-empty", Input::Handshake(None), &[]),
                reject(
                    "handshake/reject-truncated",
                    Input::Handshake(None),
                    &handshake[..30],
                ),
                reject(
                    "handshake/reject-bad-magic",
                    Input::Handshake(None),
                    &bad_magic,
                ),
                reject(
                    "handshake/reject-bad-version",
                    Input::Handshake(None),
                    &bad_version,
                ),
            ]
        }
        "stream" => {
            let default = DEFAULT_MAX_FRAME as u64;
            let over_default = (DEFAULT_MAX_FRAME as u32 + 1).to_le_bytes();
            let prefix = |len: u64, max: u64| Input::StreamPrefix(StreamPrefixIn { len, max });
            let max = |max: u64| Input::StreamSplit(StreamSplit::Limit { max });
            vec![
                reject(
                    "stream/reject-prefix-over-default-max",
                    prefix(default + 1, default),
                    &over_default,
                ),
                reject(
                    "stream/reject-prefix-over-small-max",
                    prefix(3, 2),
                    &[3, 0, 0, 0],
                ),
                reject(
                    "stream/reject-prefix-u32-max-over-default-max",
                    prefix(u64::from(u32::MAX), default),
                    &[0xff; 4],
                ),
                reject(
                    "stream/reject-split-over-default-max",
                    max(default),
                    &over_default,
                ),
                reject(
                    "stream/reject-split-u32-max",
                    max(default),
                    &[0xff; 4],
                ),
                reject(
                    "stream/reject-split-over-small-max",
                    max(2),
                    &[3, 0, 0, 0, b'a', b'b', b'c'],
                ),
                reject(
                    "stream/reject-split-frame-then-over-max",
                    max(2),
                    &[1, 0, 0, 0, b'a', 3, 0, 0, 0],
                ),
            ]
        }
        "envelope" => vec![
            reject("envelope/reject-empty", Input::Envelope(None), &[]),
            reject(
                "envelope/reject-unknown-tag",
                Input::Envelope(None),
                &[3],
            ),
            reject("envelope/reject-truncated-err", Input::Envelope(None), &[1, 0]),
        ],
        "framing-datagram" => vec![
            reject(
                "framing-datagram/reject-short-request",
                request_for(FRAMING_DATAGRAM),
                &[3, 0, 42, 0, 0, 0, 0, 0, 0],
            ),
            reject(
                "framing-datagram/reject-short-response",
                response_for(FRAMING_DATAGRAM),
                &[7, 0, 0, 0, 0, 0, 0],
            ),
            reject(
                "framing-datagram/reject-response-without-envelope",
                response_for(FRAMING_DATAGRAM),
                &[7, 0, 0, 0, 0, 0, 0, 0],
            ),
        ],
        "framing-datagram3" => {
            let mut request = Vec::new();
            Datagram3Framing
                .encode_request(0, Call::from(3), 42, false, &(), &Json, &mut request)
                .unwrap();
            let mut reply = Vec::new();
            Datagram3Framing.encode_reply(0, 7, &Envelope::Fault, &mut reply);
            let d3 = || request_for(FRAMING_DATAGRAM3);
            let d3_reply = || response_for(FRAMING_DATAGRAM3);
            let raw = |head: &[u8]| {
                let mut b = head.to_vec();
                b.extend_from_slice(&request[3..]);
                b
            };
            let p = |id: &str| format!("framing-datagram3/{id}");
            vec![
                reject(&p("reject-unknown-kind"), d3(), &raw(&[2, 0, 3])),
                reject(&p("reject-overlong-protocol"), d3(), &raw(&[0, 0x80, 0x00, 3])),
                reject(&p("reject-overlong-function"), d3(), &raw(&[0, 0, 0x83, 0x00])),
                reject(&p("reject-function-past-u16"), d3(), &raw(&[0, 0, 0xff, 0xff, 0x04])),
                reject(&p("reject-reply-as-request"), d3(), &reply),
                reject(&p("reject-request-as-reply"), d3_reply(), &request),
                reject(&p("reject-short-request"), d3(), &request[..10]),
                reject(&p("reject-reply-without-envelope"), d3_reply(), &reply[..10]),
            ]
        }
        "framing-datagram2" => {
            let mut request = Vec::new();
            Datagram2Framing
                .encode_request(0, Call::from(3), 42, false, &(), &Json, &mut request)
                .unwrap();
            let mut reply = Vec::new();
            Datagram2Framing.encode_reply(0, 7, &Envelope::Fault, &mut reply);
            let mut overlong = vec![0, 0x80, 0x00];
            overlong.extend_from_slice(&request[2..]);
            let mut unknown_kind = request.clone();
            unknown_kind[0] = 2;
            let d2 = || request_for(FRAMING_DATAGRAM2);
            let d2_reply = || response_for(FRAMING_DATAGRAM2);
            vec![
                reject(
                    "framing-datagram2/reject-unknown-kind",
                    d2(),
                    &unknown_kind,
                ),
                reject(
                    "framing-datagram2/reject-overlong-protocol",
                    d2(),
                    &overlong,
                ),
                reject(
                    "framing-datagram2/reject-reply-as-request",
                    d2(),
                    &reply,
                ),
                reject(
                    "framing-datagram2/reject-request-as-reply",
                    d2_reply(),
                    &request,
                ),
                reject(
                    "framing-datagram2/reject-short-request",
                    d2(),
                    &request[..9],
                ),
                reject(
                    "framing-datagram2/reject-reply-without-envelope",
                    d2_reply(),
                    &reply[..10],
                ),
            ]
        }
        // A struct only in the shape its format writes (runtime#74): by name
        // in JSON, by position in MessagePack, however deeply nested.
        "format-json" | "format-msgpack" => {
            let format = if feature == "format-json" {
                FORMAT_JSON
            } else {
                FORMAT_MSGPACK
            };
            let r = |id: &str, ty: Type, bytes: &[u8]| {
                reject(
                    &format!("{feature}/{id}"),
                    Input::Decode(DecodeIn {
                        format: format.into(),
                        ty,
                        value: None,
                    }),
                    bytes,
                )
            };
            let points = Type::Array(Box::new(point_type()));
            if feature == "format-json" {
                vec![
                    r("reject-struct-by-position", point_type(), br#"[1,-2]"#),
                    r(
                        "reject-nested-struct-by-position",
                        points,
                        br#"[{"x":1,"y":-2},[3,4]]"#,
                    ),
                ]
            } else {
                vec![
                    // {"x": 1, "y": -2}
                    r(
                        "reject-struct-by-name",
                        point_type(),
                        &[0x82, 0xa1, b'x', 0x01, 0xa1, b'y', 0xfe],
                    ),
                    // [[1, -2], {"x": 3, "y": 4}]
                    r(
                        "reject-nested-struct-by-name",
                        points,
                        &[
                            0x92, 0x92, 0x01, 0xfe, 0x82, 0xa1, b'x', 0x03, 0xa1, b'y', 0x04,
                        ],
                    ),
                    // `bin` 1 "a", where a string is expected.
                    r("reject-string-as-bin", Type::String, &[0xc4, 0x01, b'a']),
                ]
            }
        }
        "format-packed" => {
            let decode = |ty: Type| {
                Input::Decode(DecodeIn {
                    format: FORMAT_PACKED.into(),
                    ty,
                    value: None,
                })
            };
            let r = |id: &str, ty: Type, bytes: &[u8]| {
                reject(
                    &format!("format-packed/{id}"),
                    decode(ty),
                    bytes,
                )
            };
            vec![
                r("reject-bool-not-0-or-1", Type::Bool, &[2]),
                r(
                    "reject-option-tag-not-0-or-1",
                    Type::Option(Box::new(Type::U32)),
                    &[2, 0, 0, 0, 0],
                ),
                r("reject-truncated-u32", Type::U32, &[1, 2, 3]),
                r("reject-trailing-bytes", Type::U32, &[1, 0, 0, 0, 9]),
                r("reject-overlong-length", Type::String, &[0x80, 0x00]),
                r("reject-length-past-input", Type::String, &[5, b'a', b'b']),
                r("reject-invalid-utf8", Type::String, &[2, 0xc3, 0x28]),
                // Found by tests/properties.rs (as a `found-*` case would be):
                // a count is never more than the bytes left, even for
                // elements that take none, so `[Empty {}]`'s `01` is refused.
                // Schemas can't declare such an array: core refuses arrays
                // of data-less types (ComlineProject/core#141).
                r(
                    "reject-count-past-input",
                    Type::Array(Box::new(Type::Struct {
                        name: "Empty".into(),
                        fields: vec![],
                    })),
                    &[1],
                ),
            ]
        }
        "framing-jsonrpc" => vec![
            reject(
                "framing-jsonrpc/reject-request-not-json",
                request_for(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.0\",",
            ),
            reject(
                "framing-jsonrpc/reject-request-without-method",
                request_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","params":[],"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-without-id",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","result":1}"#,
            ),
            // JSON-RPC 2.0's own rules (runtime#75): the version, structured
            // params, a reply to a null id, one outcome, an error's message.
            reject(
                "framing-jsonrpc/reject-request-other-version",
                request_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"1.0","method":"greet","params":[],"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-request-without-version",
                request_for(FRAMING_JSONRPC),
                br#"{"method":"greet","params":[],"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-request-scalar-params",
                request_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","method":"greet","params":"x","id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-request-null-id",
                request_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","method":"greet","params":[],"id":null}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-result-and-error",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","result":1,"error":{"code":1,"message":"x"},"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-error-without-message",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","error":{"code":1},"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-without-version",
                response_for(FRAMING_JSONRPC),
                br#"{"result":1,"id":1}"#,
            ),
            // A member written twice, which JSON leaves to the reader and a
            // last-wins reader gets wrong: it lets params that aren't one JSON
            // value overwrite a frame's `method`. Refused wherever it's read.
            // Written before 0.8, and by other peers: read as none.
            accept_params_null(),
            reject(
                "framing-jsonrpc/reject-request-repeated-jsonrpc",
                request_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","jsonrpc":"2.0","method":"greet","params":[],"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-request-repeated-method",
                request_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","method":"greet","params":[],"method":"other","id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-request-repeated-params",
                request_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","method":"greet","params":[],"params":[1],"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-request-repeated-id",
                request_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","method":"greet","id":1,"params":[],"id":2}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-repeated-jsonrpc",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","jsonrpc":"2.0","result":1,"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-repeated-result",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","result":1,"result":2,"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-repeated-id",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","result":1,"id":1,"id":2}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-repeated-error",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","error":{"code":1,"message":"x"},"error":{"code":2,"message":"y"},"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-repeated-error-code",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","error":{"code":1,"code":2,"message":"x"},"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-repeated-error-message",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","error":{"code":1,"message":"x","message":"y"},"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-repeated-error-data",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","error":{"code":1,"message":"x","data":1,"data":2},"id":1}"#,
            ),
            // Found by comline-typescript's differential tests (#42): JSON
            // text is UTF-8 throughout, so `\xff` is refused even in a member
            // the framing ignores.
            reject(
                "framing-jsonrpc/found-reject-request-invalid-utf8-in-jsonrpc",
                request_for(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.\xff\",\"method\":\"greet\",\"params\":[],\"id\":1}",
            ),
            reject(
                "framing-jsonrpc/found-reject-request-invalid-utf8-in-unknown-member",
                request_for(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.0\",\"method\":\"greet\",\"params\":[],\"x\":\"\xff\",\"id\":1}",
            ),
            reject(
                "framing-jsonrpc/found-reject-response-invalid-utf8-in-jsonrpc",
                response_for(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.\xff\",\"result\":1,\"id\":1}",
            ),
            reject(
                "framing-jsonrpc/found-reject-response-invalid-utf8-in-unknown-member",
                response_for(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.0\",\"result\":1,\"x\":\"\xff\",\"id\":1}",
            ),
            reject(
                "framing-jsonrpc/found-reject-response-invalid-utf8-in-error-message",
                response_for(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.0\",\"error\":{\"code\":1,\"message\":\"\xff\"},\"id\":1}",
            ),
        ]
        .into_iter()
        .chain(non_json_payloads())
        .collect(),
        _ => vec![],
    }
}
