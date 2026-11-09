// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use comline_runtime::contract::{FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_JSONRPC};

use crate::engine::{encode, encode_with};
use crate::{Case, Compare, Expect, Framed, Hex, Input, PeerRefusalIn, Reply, Value};

use super::frames::{request, response};
use super::{ok, reject};

/// JSON-RPC frames the reference writes differently but must read, found by
/// comline-typescript's differential tests (#41). They compare as JSON, so
/// encoding the input must give the same JSON value as `bytes`, spelled
/// however the runtime spells it.
pub(super) fn found_jsonrpc_cases() -> Vec<Case> {
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
pub(super) fn accept_params_null() -> Case {
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
pub(super) fn non_json_payloads() -> Vec<Case> {
    let request_for = || {
        Input::Request(Framed::Selector {
            framing: FRAMING_JSONRPC.into(),
        })
    };
    let response_for = || {
        Input::Response(Framed::Selector {
            framing: FRAMING_JSONRPC.into(),
        })
    };
    let params = Value::Array(vec![Value::U32(7), Value::String("x".into())]);
    let mut cases = Vec::new();
    for (name, format) in [("msgpack", FORMAT_MSGPACK), ("packed", FORMAT_PACKED)] {
        let bytes = encode_with(format, &params).expect("the params encode");
        let framed = |head: &str, tail: &str| [head.as_bytes(), &bytes, tail.as_bytes()].concat();
        cases.push(reject(
            &format!("framing-jsonrpc/reject-request-params-{name}"),
            request_for(),
            &framed(
                r#"{"jsonrpc":"2.0","method":"greet","params":"#,
                r#","id":42}"#,
            ),
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
pub(super) fn jsonrpc_unknown_members() -> Vec<Case> {
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

/// JSON-RPC 2.0 has a peer answer a request it can't read with an error whose
/// `id` is `null`, which answers no call. It's read as what it is, and is no
/// `response`; a `null` id on anything else is nothing.
pub(super) fn jsonrpc_peer_refusals() -> Vec<Case> {
    let case = |id: &str, bytes: &[u8], expect| Case {
        id: format!("framing-jsonrpc/{id}"),
        input: Input::PeerRefusal(PeerRefusalIn {
            framing: FRAMING_JSONRPC.into(),
        }),
        bytes: Hex(bytes.to_vec()),
        expect,
        compare: Compare::Bytes,
    };
    let not_a_response = |id: &str, bytes: &[u8]| {
        reject(
            &format!("framing-jsonrpc/{id}"),
            Input::Response(Framed::Selector {
                framing: FRAMING_JSONRPC.into(),
            }),
            bytes,
        )
    };
    let parse_error =
        br#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":null}"#;
    vec![
        case("peer-refusal-parse-error", parse_error, Expect::Exact),
        case(
            "peer-refusal-invalid-request",
            br#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request","data":{"why":"no"}},"id":null}"#,
            Expect::Exact,
        ),
        not_a_response("reject-response-peer-refusal", parse_error),
        case(
            "reject-peer-refusal-of-a-call",
            br#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":1}"#,
            Expect::Reject,
        ),
        case(
            "reject-peer-refusal-without-id",
            br#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"}}"#,
            Expect::Reject,
        ),
        case(
            "reject-peer-refusal-result",
            br#"{"jsonrpc":"2.0","result":1,"id":null}"#,
            Expect::Reject,
        ),
        case(
            "reject-peer-refusal-result-and-error",
            br#"{"jsonrpc":"2.0","result":1,"error":{"code":-32700,"message":"x"},"id":null}"#,
            Expect::Reject,
        ),
        case(
            "reject-peer-refusal-without-message",
            br#"{"jsonrpc":"2.0","error":{"code":-32700},"id":null}"#,
            Expect::Reject,
        ),
        case(
            "reject-peer-refusal-other-version",
            br#"{"jsonrpc":"1.0","error":{"code":-32700,"message":"x"},"id":null}"#,
            Expect::Reject,
        ),
        case(
            "reject-peer-refusal-repeated-id",
            br#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"x"},"id":null,"id":null}"#,
            Expect::Reject,
        ),
    ]
}
