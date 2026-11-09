// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The shared benchmark definitions (`benches/`) are well formed, and the
//! Rust benches (`runtime/core/benches/`) measure exactly what they define.
//!
//! - A generated value expands to what its definition says, and every shape's
//!   value fits its type and round-trips through `Wire` and
//!   `decode_typed` in every format.
//! - Each hand-written mirror type in `core/benches/definitions` encodes to
//!   exactly the bytes its shape's value does, in every format.
//! - The framing, handshake and connection definitions match the constants
//!   the benches use.

use std::path::{Path, PathBuf};

use comline_runtime::contract::{
    FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};
use comline_runtime::handshake::comline2::Hello;
use comline_runtime_conformance::{
    check, decimal, decode_typed, encode_as, HandshakeIn, Hex, Reply, Type, Value, Wire,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value as JsonValue};

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
    #[serde(deserialize_with = "generated")]
    value: Value,
}

/// A shape's value: as a corpus value is written, or `generate`d. Expanded
/// here, it is a corpus value from then on.
fn generated<'de, D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    Value::deserialize(expand(JsonValue::deserialize(d)?).map_err(serde::de::Error::custom)?)
        .map_err(serde::de::Error::custom)
}

/// `{"generate": {"count": N, "item": ...}}` as the array of its `N` items,
/// in which each sequence holds its number for that item's index. Anything
/// else as it is.
fn expand(value: JsonValue) -> Result<JsonValue, String> {
    let Some(generate) = value.get("generate") else {
        return Ok(value);
    };
    let Generate { count, item } =
        Generate::deserialize(generate).map_err(|e| format!("bad `generate`: {e}"))?;
    let items = (0..count)
        .map(|index| instantiate(&item, index))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "array": items }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generate {
    count: i64,
    item: JsonValue,
}

/// A number per item: `start + step * index`, then `modulo` (Euclidean) if
/// it has one. Reading the result into its type refuses one that won't fit.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sequence {
    start: i64,
    step: i64,
    modulo: Option<i64>,
}

fn instantiate(item: &JsonValue, index: i64) -> Result<JsonValue, String> {
    Ok(match item {
        JsonValue::Object(fields) if fields.contains_key("start") => {
            let s = Sequence::deserialize(item).map_err(|e| format!("bad sequence: {e}"))?;
            let number = s.start + s.step * index;
            json!(s.modulo.map_or(number, |m| number.rem_euclid(m)))
        }
        JsonValue::Object(fields) => JsonValue::Object(
            fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), instantiate(v, index)?)))
                .collect::<Result<_, String>>()?,
        ),
        JsonValue::Array(items) => JsonValue::Array(
            items
                .iter()
                .map(|v| instantiate(v, index))
                .collect::<Result<_, _>>()?,
        ),
        other => other.clone(),
    })
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
    #[serde(with = "decimal")]
    request_id: u64,
    protocol: u16,
    payload_fill: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Handshake {
    frame: HandshakeIn,
    ops: Vec<HandshakeOp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HandshakeOp {
    Encode,
    Decode,
    Check,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Comline2Bench {
    hello: Hello,
    ops: Vec<Comline2Op>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Comline2Op {
    Encode,
    Decode,
    Answer,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    framing: String,
    format: String,
    protocol: u16,
    call: ConnectionCall,
    params: String,
    reply: Reply,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionCall {
    id: u16,
    name: String,
}

const FORMATS: [&str; 3] = [FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED];

fn benches_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("spec/benches")
}

fn read<T: for<'de> Deserialize<'de>>(name: &str) -> T {
    let path = benches_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    // `$schema` is for editors and `cue vet`; the rest is held to the structs.
    let mut json: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()));
    if let Some(fields) = json.as_object_mut() {
        fields.remove("$schema");
    }
    serde_json::from_value(json).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()))
}

fn encode(format: &str, value: &(impl Serialize + ?Sized)) -> Vec<u8> {
    encode_as(format, value).expect("bench values encode")
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
    let large = formats.shapes.iter().find(|s| s.id == "large-array");
    let Some(Value::Array(items)) = large.map(|s| &s.value) else {
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
        [FRAMING_DATAGRAM, FRAMING_DATAGRAM2, FRAMING_DATAGRAM3, FRAMING_JSONRPC]
    );
    assert_eq!(framings.payload_sizes, definitions::PAYLOAD_SIZES);
    assert_eq!(framings.frame.kind, "ok");
    assert_eq!(framings.frame.request_id, definitions::FRAME_REQUEST_ID);
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
    assert_eq!(frame.ir_hash, definitions::HANDSHAKE_IR_HASH);
    assert_eq!(frame.wire_format, definitions::HANDSHAKE_WIRE_FORMAT);
    assert_eq!(frame.framing, definitions::HANDSHAKE_FRAMING);
    assert_eq!(frame.capabilities, definitions::HANDSHAKE_CAPABILITIES);
    assert_eq!(
        handshake.ops,
        [HandshakeOp::Encode, HandshakeOp::Decode, HandshakeOp::Check]
    );
}

#[test]
fn comline2_definition_matches_the_benches() {
    let comline2: Comline2Bench = read("comline2.json");
    assert_eq!(comline2.hello, definitions::comline2_hello());
    assert_eq!(comline2.hello, comline2.hello.clone().canonical());
    assert_eq!(
        comline2.ops,
        [Comline2Op::Encode, Comline2Op::Decode, Comline2Op::Answer]
    );
}

#[test]
fn connection_definition_matches_the_benches() {
    let connection: Connection = read("connection.json");
    // The params name a shape, in another file.
    let formats: Formats = read("formats.json");
    assert!(
        formats.shapes.iter().any(|s| s.id == connection.params),
        "no shape `{}`",
        connection.params
    );
    assert_eq!(connection.framing, definitions::CONNECTION_FRAMING);
    assert_eq!(connection.format, definitions::CONNECTION_FORMAT);
    assert_eq!(connection.protocol, definitions::CONNECTION_PROTOCOL);
    assert_eq!(
        (connection.call.id, connection.call.name.as_str()),
        definitions::CONNECTION_CALL
    );
    assert_eq!(connection.params, definitions::CONNECTION_PARAMS);
    assert_eq!(connection.reply, Reply::Ok { payload: Hex(vec![]) });
}

#[test]
fn a_generated_value_expands_to_its_items() {
    let read = |value: JsonValue| {
        serde_json::from_value::<Shape>(json!({
            "id": "x", "description": "x", "type": "u8", "value": value,
        }))
        .map(|shape| shape.value)
    };
    let sequence = json!({ "start": 10, "step": -4, "modulo": 7 });
    assert_eq!(
        read(json!({ "generate": { "count": 4, "item": { "u8": sequence } } })).unwrap(),
        // 10, 6, 2, -2: a negative wraps to 5.
        Value::Array([3, 6, 2, 5].map(Value::U8).to_vec())
    );
    assert_eq!(
        read(json!({ "generate": { "count": 0, "item": { "u8": 1 } } })).unwrap(),
        Value::Array(vec![])
    );
    assert_eq!(read(json!({ "u8": 1 })).unwrap(), Value::U8(1));
    // A number that won't fit its type, a field a sequence hasn't got.
    let over = json!({ "generate": { "count": 2, "item": { "u8": { "start": 255, "step": 1 } } } });
    assert!(read(over).is_err());
    let more = json!({ "generate": { "count": 1, "item": { "u8": { "start": 0, "step": 1, "by": 2 } } } });
    assert!(read(more).is_err());
}
