// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The shared benchmark definitions (`benches/`) are well formed, and the
//! Rust benches (`runtime/core/benches/`) measure exactly what they define.
//!
//! - Every shape's value fits its type and round-trips through `Wire` and
//!   `decode_typed` in every format.
//! - Each hand-written mirror type in `core/benches/definitions` encodes to
//!   exactly the bytes its shape's value does, in every format.
//! - The framing, handshake and connection definitions match the constants
//!   the benches use.

use std::path::{Path, PathBuf};

use comline_runtime::contract::{
    WireFormat, FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_JSONRPC,
};
use comline_runtime::format::{Json, MsgPack, Packed};
use comline_runtime_conformance::{check, decode_typed, Type, Value, Wire};
use serde::{Deserialize, Serialize};

#[path = "../../core/benches/definitions/mod.rs"]
mod definitions;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Formats {
    formats: Vec<String>,
    shapes: Vec<Shape>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Shape {
    id: String,
    description: String,
    #[serde(rename = "type")]
    ty: Type,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Framings {
    framings: Vec<String>,
    payload_sizes: Vec<usize>,
    frame: FramingFrame,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FramingFrame {
    kind: String,
    request_id: String,
    protocol: u16,
    payload_fill: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Handshake {
    frame: HandshakeFrame,
    ops: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandshakeFrame {
    ir_hash: String,
    wire_format: String,
    framing: String,
    capabilities: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    framing: String,
    format: String,
    protocol: u16,
    call: ConnectionCall,
    params: String,
    reply: ConnectionReply,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionCall {
    id: u16,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionReply {
    kind: String,
    payload: String,
}

const FORMATS: [&str; 3] = [FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED];

fn benches_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("benches")
}

fn read<T: for<'de> Deserialize<'de>>(name: &str) -> T {
    let path = benches_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()))
}

fn encode(format: &str, value: &(impl Serialize + ?Sized)) -> Vec<u8> {
    let mut out = Vec::new();
    match format {
        FORMAT_JSON => Json.encode(value, &mut out),
        FORMAT_MSGPACK => MsgPack.encode(value, &mut out),
        FORMAT_PACKED => Packed.encode(value, &mut out),
        other => panic!("unknown format `{other}`"),
    }
    .expect("bench values encode");
    out
}

/// The mirror of shape `id`, encoded in `format`.
fn mirror_bytes(id: &str, format: &str) -> Vec<u8> {
    match id {
        "small-scalar" => encode(format, &definitions::small_scalar()),
        "string-heavy" => encode(format, &definitions::string_heavy()),
        "nested" => encode(format, &definitions::nested()),
        "large-array" => encode(format, &definitions::large_array()),
        "enum-heavy" => encode(format, &definitions::enum_heavy()),
        other => panic!("shape `{other}` has no mirror in core/benches/definitions"),
    }
}

#[test]
fn shapes_round_trip_in_every_format() {
    let formats: Formats = read("formats.json");
    assert_eq!(formats.formats, FORMATS);
    let ids: Vec<&str> = formats.shapes.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "small-scalar",
            "string-heavy",
            "nested",
            "large-array",
            "enum-heavy"
        ]
    );
    for shape in &formats.shapes {
        assert!(
            !shape.description.is_empty(),
            "{}: no description",
            shape.id
        );
        check(&shape.value, &shape.ty).unwrap_or_else(|e| panic!("{}: {e}", shape.id));
        for format in FORMATS {
            let bytes = encode(format, &Wire(&shape.value));
            let decoded = decode_typed(format, &shape.ty, &bytes)
                .unwrap_or_else(|e| panic!("{} in {format}: {e:?}", shape.id));
            assert_eq!(
                decoded.as_ref(),
                Some(&shape.value),
                "{} doesn't round-trip in {format}",
                shape.id
            );
        }
    }
    let Value::Array(items) = &formats.shapes[3].value else {
        panic!("large-array isn't an array");
    };
    assert_eq!(items.len(), definitions::LARGE_ARRAY_LEN);
}

#[test]
fn mirrors_encode_as_their_definitions() {
    let formats: Formats = read("formats.json");
    for shape in &formats.shapes {
        for format in FORMATS {
            assert_eq!(
                mirror_bytes(&shape.id, format),
                encode(format, &Wire(&shape.value)),
                "the mirror of {} encodes differently in {format}",
                shape.id
            );
        }
    }
}

#[test]
fn framing_definitions_match_the_benches() {
    let framings: Framings = read("framings.json");
    assert_eq!(
        framings.framings,
        [FRAMING_DATAGRAM, FRAMING_DATAGRAM2, FRAMING_JSONRPC]
    );
    assert_eq!(framings.payload_sizes, definitions::PAYLOAD_SIZES);
    assert_eq!(framings.frame.kind, "ok");
    assert_eq!(
        framings.frame.request_id,
        definitions::FRAME_REQUEST_ID.to_string()
    );
    assert_eq!(framings.frame.protocol, definitions::FRAME_PROTOCOL);
    assert_eq!(framings.frame.payload_fill, "a");
    for size in definitions::PAYLOAD_SIZES {
        let payload = definitions::payload(size);
        assert_eq!(payload.len(), size);
        let text: serde_json::Value =
            serde_json::from_slice(&payload).expect("the payload is JSON");
        assert_eq!(text, serde_json::Value::String("a".repeat(size - 2)));
    }
}

#[test]
fn handshake_definition_matches_the_benches() {
    let handshake: Handshake = read("handshake.json");
    let frame = &handshake.frame;
    assert_eq!(frame.ir_hash, definitions::HANDSHAKE_IR_HASH.to_string());
    assert_eq!(frame.wire_format, definitions::HANDSHAKE_WIRE_FORMAT);
    assert_eq!(frame.framing, definitions::HANDSHAKE_FRAMING);
    assert_eq!(frame.capabilities, definitions::HANDSHAKE_CAPABILITIES);
    assert_eq!(handshake.ops, ["encode", "decode", "check"]);
}

#[test]
fn connection_definition_matches_the_benches() {
    let connection: Connection = read("connection.json");
    assert_eq!(connection.framing, definitions::CONNECTION_FRAMING);
    assert_eq!(connection.format, definitions::CONNECTION_FORMAT);
    assert_eq!(connection.protocol, definitions::CONNECTION_PROTOCOL);
    assert_eq!(
        (connection.call.id, connection.call.name.as_str()),
        definitions::CONNECTION_CALL
    );
    assert_eq!(connection.params, definitions::CONNECTION_PARAMS);
    assert_eq!(connection.reply.kind, "ok");
    assert_eq!(connection.reply.payload, "");
}
