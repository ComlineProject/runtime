// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use comline_runtime::contract::stream::DEFAULT_MAX_FRAME;
use comline_runtime::contract::{
    Call, ConnectionFraming, Datagram2Framing, Datagram3Framing, Envelope, HandshakeFrame,
    FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};
use comline_runtime::format::Json;

use crate::{Case, DecodeIn, Framed, Input, StreamPrefixIn, StreamSplit, Type};

use super::decode::point_type;
use super::jsonrpc::{accept_params_null, non_json_payloads};
use super::reject;

/// Bytes a runtime must refuse, per feature.
pub(super) fn rejections(feature: &str) -> Vec<Case> {
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
                "framing-jsonrpc/reject-response-no-outcome",
                response_for(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","id":1}"#,
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
