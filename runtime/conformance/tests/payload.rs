// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What each framing's `payload` fact claims, over bytes no list of cases
//! could cover.
//!
//! - **`opaque`:** whatever the payload, the frame gives it back exactly.
//! - **`json`, on what it must carry:** params that are an array or an object,
//!   and a result that's any JSON value, come back as written.
//! - **`json`, on what it needn't:** a frame holding any other payload is
//!   refused or still reads as the frame it was written as, with its method
//!   and id. A payload can lose what follows its first value, since unknown
//!   members are JSON-RPC's to carry, but it can't change what the frame calls
//!   or answers.
//!
//! A run is short by default; `PROPTEST_CASES` raises the count per property
//! (the nightly job sets 100000).

use comline_runtime::contract::{
    ConnectionFraming, Datagram2Framing, Datagram3Framing, DatagramFraming, Envelope, Frame,
    Framing, Request, RequestCall,
};
use comline_runtime::framing::JsonRpcFraming;
use proptest::prelude::*;
use serde_json::Value as Json;

const CALL: u16 = 7;
const METHOD: &str = "m";
const ID: u64 = 42;

/// Any JSON value, as text.
fn json_value() -> impl Strategy<Value = Json> {
    let leaf = prop_oneof![
        Just(Json::Null),
        any::<bool>().prop_map(Json::from),
        any::<i64>().prop_map(Json::from),
        any::<u64>().prop_map(Json::from),
        "[ -~]{0,8}".prop_map(Json::from),
    ];
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Json::Array),
            prop::collection::btree_map("[a-z]{0,4}", inner, 0..4)
                .prop_map(|m| Json::Object(m.into_iter().collect())),
        ]
    })
}

/// A JSON value's text, and what could follow it in a frame.
fn spliced() -> impl Strategy<Value = Vec<u8>> {
    let member = prop::sample::select(&["id", "method", "params", "result", "error", "jsonrpc", "x"][..]);
    (json_value(), prop::collection::vec((member, json_value()), 1..3)).prop_map(|(first, rest)| {
        let mut text = first.to_string();
        for (name, value) in rest {
            text.push_str(&format!(",\"{name}\":{value}"));
        }
        text.into_bytes()
    })
}

/// Bytes a framing that embeds JSON has no reason to expect: any bytes, JSON's
/// own punctuation in any order, and JSON values with members after them.
fn hostile() -> impl Strategy<Value = Vec<u8>> {
    let punctuation = prop::sample::select(&b"[]{},:\"1 n"[..]);
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..32),
        prop::collection::vec(punctuation, 0..16),
        spliced(),
    ]
}

fn called(request: &Request<'_>) -> bool {
    request.request_id == ID && matches!(request.call, RequestCall::Name(name) if name == METHOD)
}

fn jsonrpc_request(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    Framing::encode_request_body(&JsonRpcFraming, CALL, METHOD, ID, payload, &mut frame);
    frame
}

fn jsonrpc_result(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    Framing::encode_response_ok(&JsonRpcFraming, ID, payload, &mut frame);
    frame
}

fn jsonrpc_error_data(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    Framing::encode_response_err(&JsonRpcFraming, ID, 2, payload, &mut frame);
    frame
}

fn config() -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 64;
    }
    config
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn opaque_framings_give_any_payload_back(payload in prop::collection::vec(any::<u8>(), 0..64)) {
        let mut frame = Vec::new();
        DatagramFraming.encode_request_body(CALL, METHOD, ID, &payload, &mut frame);
        let request = DatagramFraming.decode_request(&frame).expect("a datagram@1 request");
        prop_assert_eq!(request.params, &payload[..]);
        prop_assert_eq!(request.request_id, ID);

        let mut frame = Vec::new();
        DatagramFraming.encode_response_ok(ID, &payload, &mut frame);
        prop_assert_eq!(
            DatagramFraming.decode_response(&frame),
            Some((ID, Envelope::Ok(&payload)))
        );

        let mut frame = Vec::new();
        Datagram2Framing.encode_request_body(1, CALL, METHOD, ID, false, &payload, &mut frame);
        match Datagram2Framing.decode(&frame) {
            Some(Frame::Request { request, .. }) => {
                prop_assert_eq!(request.params, &payload[..]);
                prop_assert_eq!(request.request_id, ID);
            }
            other => prop_assert!(false, "datagram@2 read {:?}", other),
        }

        let mut frame = Vec::new();
        Datagram3Framing.encode_request_body(1, CALL, METHOD, ID, false, &payload, &mut frame);
        match Datagram3Framing.decode(&frame) {
            Some(Frame::Request { request, .. }) => {
                prop_assert_eq!(request.params, &payload[..]);
                prop_assert_eq!(request.request_id, ID);
            }
            other => prop_assert!(false, "datagram@3 read {:?}", other),
        }
    }

    #[test]
    fn jsonrpc_gives_structured_params_back(value in json_value()) {
        let params = value.to_string();
        let structured = value.is_array() || value.is_object() || value.is_null();
        let frame = jsonrpc_request(params.as_bytes());
        match Framing::decode_request(&JsonRpcFraming, &frame) {
            Some(request) => {
                prop_assert!(structured, "read scalar params {}", params);
                prop_assert!(called(&request));
                prop_assert_eq!(request.params, params.as_bytes());
            }
            None => prop_assert!(!structured, "refused params {}", params),
        }
    }

    #[test]
    fn jsonrpc_gives_any_result_back(value in json_value()) {
        let payload = value.to_string();
        let frame = jsonrpc_result(payload.as_bytes());
        prop_assert_eq!(
            Framing::decode_response(&JsonRpcFraming, &frame),
            Some((ID, Envelope::Ok(payload.as_bytes())))
        );
    }

    #[test]
    fn a_payload_cant_change_a_jsonrpc_frame(payload in hostile()) {
        let frame = jsonrpc_request(&payload);
        if let Some(request) = Framing::decode_request(&JsonRpcFraming, &frame) {
            prop_assert!(called(&request), "{:?} read as {:?}", String::from_utf8_lossy(&frame), request);
        }
        match ConnectionFraming::decode(&JsonRpcFraming, &frame) {
            Some(Frame::Request { request, one_way, .. }) => {
                prop_assert!(called(&request) && !one_way, "{:?}", String::from_utf8_lossy(&frame));
            }
            Some(other) => prop_assert!(false, "a request read as {:?}", other),
            None => {}
        }

        for frame in [jsonrpc_result(&payload), jsonrpc_error_data(&payload)] {
            if let Some((id, _)) = Framing::decode_response(&JsonRpcFraming, &frame) {
                prop_assert_eq!(id, ID, "{:?}", String::from_utf8_lossy(&frame));
            }
        }
    }

    /// A result can't turn into an error or an error into a result, nor one
    /// error's ordinal into another's.
    #[test]
    fn a_payload_cant_change_a_jsonrpc_outcome(payload in hostile()) {
        if let Some((_, envelope)) = Framing::decode_response(&JsonRpcFraming, &jsonrpc_result(&payload)) {
            prop_assert!(matches!(envelope, Envelope::Ok(_)), "{:?}", envelope);
        }
        if let Some((_, envelope)) = Framing::decode_response(&JsonRpcFraming, &jsonrpc_error_data(&payload)) {
            prop_assert!(matches!(envelope, Envelope::Err { id: 2, .. }), "{:?}", envelope);
        }
    }
}
