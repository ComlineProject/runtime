// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The reference: every case in `vectors/`, computed from `comline-runtime`.

mod decode;
mod frames;
mod handshake;
mod jsonrpc;
mod rejections;

use comline_runtime::contract::{
    FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};

use crate::engine::encode;
use crate::{
    comline2, Case, Compare, Expect, Framed, Hex, Input, NameHashIn, Reply, Value, Vectors,
};

use decode::{bound_cases, char_cases, decode_cases, found_cases};
use frames::{format_cases, packed_cases, request, response, stream_cases, with_format};
use handshake::{handshake, handshake_check_cases};
use jsonrpc::{found_jsonrpc_cases, jsonrpc_peer_refusals, jsonrpc_unknown_members};
use rejections::rejections;

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
            ("envelope/fault".into(), Input::Envelope(Some(Reply::Fault))),
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
            let (id, mut input) = request(
                p("notification-no-params"),
                framing,
                0,
                "ping",
                0,
                &Value::Unit(()),
            );
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
                binary.push(with_format(
                    request(id, framing, 3, "greet", 42, &params),
                    format,
                ));
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
                    request(
                        p("request-params-msgpack"),
                        FRAMING_DATAGRAM2,
                        3,
                        "greet",
                        42,
                        &params,
                    ),
                    FORMAT_MSGPACK,
                ),
                0,
            ),
            on(
                with_format(
                    request(
                        p("request-params-packed"),
                        FRAMING_DATAGRAM2,
                        3,
                        "greet",
                        42,
                        &params,
                    ),
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
            d3(
                request(p("request"), FRAMING_DATAGRAM3, 3, "greet", 42, &params),
                0,
            ),
            // The protocol is 300, two varint bytes, and the call 1, one: no
            // function number can be mistaken for the protocol's bytes.
            d3(
                request(
                    p("request-two-byte-protocol"),
                    FRAMING_DATAGRAM3,
                    1,
                    "ping",
                    5,
                    &Value::Array(vec![]),
                ),
                300,
            ),
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
                    request(
                        p("request-params-msgpack"),
                        FRAMING_DATAGRAM3,
                        3,
                        "greet",
                        42,
                        &params,
                    ),
                    FORMAT_MSGPACK,
                ),
                0,
            ),
            d3(
                with_format(
                    request(
                        p("request-params-packed"),
                        FRAMING_DATAGRAM3,
                        3,
                        "greet",
                        42,
                        &params,
                    ),
                    FORMAT_PACKED,
                ),
                0,
            ),
            d3(
                response(
                    p("response-ok"),
                    FRAMING_DATAGRAM3,
                    7,
                    ok(br#"{"body":"hi"}"#),
                ),
                0,
            ),
            d3(
                response(
                    p("response-err"),
                    FRAMING_DATAGRAM3,
                    7,
                    err(2, br#"{"why":"no"}"#),
                ),
                1,
            ),
            d3(
                response(p("response-fault"), FRAMING_DATAGRAM3, 9, Reply::Fault),
                200,
            ),
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
        let read_the_other_way = directions(&v.feature, &v.cases);
        v.cases.extend(read_the_other_way);
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
                v.cases.extend(jsonrpc_peer_refusals());
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

pub(super) fn exact(id: String, input: Input) -> Case {
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

pub(super) fn reject(id: &str, input: Input, bytes: &[u8]) -> Case {
    Case {
        id: id.into(),
        input,
        bytes: Hex(bytes.to_vec()),
        expect: Expect::Reject,
        compare: Compare::Bytes,
    }
}

pub(super) fn ok(payload: &[u8]) -> Reply {
    Reply::Ok {
        payload: Hex(payload.to_vec()),
    }
}

pub(super) fn err(id: u16, body: &[u8]) -> Reply {
    Reply::Err {
        id,
        body: Hex(body.to_vec()),
    }
}

/// A framing's own frames read the other way, which `directions` is proven by.
/// `datagram@1`'s don't say which way they go: its request has a response's
/// shape, so it reads as one. The others' say, so each is refused as the other.
fn directions(feature: &str, cases: &[Case]) -> Vec<Case> {
    let framing = match feature {
        "framing-datagram" => FRAMING_DATAGRAM,
        "framing-datagram2" => FRAMING_DATAGRAM2,
        "framing-datagram3" => FRAMING_DATAGRAM3,
        "framing-jsonrpc" => FRAMING_JSONRPC,
        _ => return vec![],
    };
    let p = |id: &str| format!("{feature}/{id}");
    let bytes_of = |id: &str| {
        let id = p(id);
        let case = cases
            .iter()
            .find(|c| c.id == id)
            .expect("the framing's frame");
        case.bytes.0.clone()
    };
    let (request_frame, response_frame) = (bytes_of("request"), bytes_of("response-ok"));
    if framing == FRAMING_DATAGRAM {
        let (head, rest) = request_frame.split_at(8);
        let request_id = u64::from_le_bytes(head.try_into().expect("eight bytes"));
        let Some((&0, payload)) = rest.split_first() else {
            panic!("a request that doesn't read as an `ok`");
        };
        let (id, input) = response(
            p("accept-request-as-response"),
            framing,
            request_id,
            ok(payload),
        );
        return vec![Case {
            id,
            input,
            bytes: Hex(request_frame),
            expect: Expect::Accept,
            compare: Compare::Bytes,
        }];
    }
    let framing = || framing.to_string();
    vec![
        reject(
            &p("reject-response-as-request"),
            Input::Request(Framed::Selector { framing: framing() }),
            &response_frame,
        ),
        reject(
            &p("reject-request-as-response"),
            Input::Response(Framed::Selector { framing: framing() }),
            &request_frame,
        ),
    ]
}
