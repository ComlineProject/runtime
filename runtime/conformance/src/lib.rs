// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Wire conformance for Comline runtimes.
//!
//! The corpus is a set of byte-exact vectors, one file per wire feature
//! (`vectors/<feature>.json`), that every runtime must reproduce. This crate
//! holds the vector model, computes the vectors from `comline-runtime` (the
//! reference for now, see [`reference_cases`]), and runs them against
//! `comline-runtime` through [`run`]. Another runtime reads the same files and
//! drives itself the same way.

use std::collections::BTreeMap;

mod intern;
pub mod runner;
mod seed;

pub use seed::{check, TypeSeed};

use serde::de::Error as _;
use serde::ser::{SerializeSeq, SerializeStruct, SerializeTuple};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use comline_runtime::contract::stream::{self, Split, DEFAULT_MAX_FRAME, PREFIX_LEN};
use comline_runtime::contract::SliceBuf;
use comline_runtime::contract::{
    name_hash, Call, ConnectionFraming, Datagram2Framing, DatagramFraming, Envelope, Frame,
    Framing, HandshakeFrame, RequestCall, WireFormat, FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED,
    FRAMING_DATAGRAM, FRAMING_DATAGRAM2, FRAMING_JSONRPC,
};
use comline_runtime::format::{Json, MsgPack, Packed};
use comline_runtime::framing::JsonRpcFraming;

/// A value as a vector states it: typed, so a runtime knows which width or
/// shape to encode. 64-bit integers are decimal strings, so a JSON reader
/// that holds numbers as doubles keeps them exact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    #[serde(with = "decimal")]
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    #[serde(with = "decimal")]
    I64(i64),
    #[serde(with = "decimal")]
    U128(u128),
    #[serde(with = "decimal")]
    I128(i128),
    F32(f32),
    F64(f64),
    String(String),
    Array(Vec<Value>),
    /// A fixed-size array, `T[N]`: its length is in the type, so formats
    /// that can leave it out do.
    Tuple(Vec<Value>),
    Option(Option<Box<Value>>),
    Struct {
        name: String,
        fields: Vec<(String, Value)>,
    },
    UnitVariant {
        #[serde(rename = "enum")]
        enum_name: String,
        index: u32,
        variant: String,
    },
    /// `()`: a function's empty return, written as `{"unit": null}`.
    Unit(()),
}

/// Serializes a [`Value`] the way a generated type with the same shape
/// serializes: a struct through `serialize_struct`, an enum variant through
/// `serialize_unit_variant`, and so on.
pub struct Wire<'a>(pub &'a Value);

impl Serialize for Wire<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Bool(v) => s.serialize_bool(*v),
            Value::U8(v) => s.serialize_u8(*v),
            Value::U16(v) => s.serialize_u16(*v),
            Value::U32(v) => s.serialize_u32(*v),
            Value::U64(v) => s.serialize_u64(*v),
            Value::I8(v) => s.serialize_i8(*v),
            Value::I16(v) => s.serialize_i16(*v),
            Value::I32(v) => s.serialize_i32(*v),
            Value::I64(v) => s.serialize_i64(*v),
            Value::U128(v) => s.serialize_u128(*v),
            Value::I128(v) => s.serialize_i128(*v),
            Value::F32(v) => s.serialize_f32(*v),
            Value::F64(v) => s.serialize_f64(*v),
            Value::String(v) => s.serialize_str(v),
            Value::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&Wire(item))?;
                }
                seq.end()
            }
            Value::Tuple(items) => {
                let mut tuple = s.serialize_tuple(items.len())?;
                for item in items {
                    tuple.serialize_element(&Wire(item))?;
                }
                tuple.end()
            }
            Value::Option(None) => s.serialize_none(),
            Value::Option(Some(v)) => s.serialize_some(&Wire(v)),
            Value::Struct { name, fields } => {
                // Generated types have `'static` names; interning them keeps
                // the impl honest to that.
                let mut st = s.serialize_struct(intern::name(name), fields.len())?;
                for (field, value) in fields {
                    st.serialize_field(intern::name(field), &Wire(value))?;
                }
                st.end()
            }
            Value::UnitVariant {
                enum_name,
                index,
                variant,
            } => s.serialize_unit_variant(intern::name(enum_name), *index, intern::name(variant)),
            Value::Unit(()) => s.serialize_unit(),
        }
    }
}

mod decimal {
    use super::*;

    pub fn serialize<T: ToString, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, T, D>(d: D) -> Result<T, D::Error>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
        D: Deserializer<'de>,
    {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}

/// Bytes as lowercase hex, in vectors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hex(pub Vec<u8>);

impl Serialize for Hex {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(
            &self
                .0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        )
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() % 2 != 0 {
            return Err(D::Error::custom("odd-length hex"));
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(D::Error::custom))
            .collect::<Result<_, _>>()
            .map(Hex)
    }
}

/// One vector: an operation's input and the exact bytes it must produce and
/// decode from, or (`expect: "reject"`) bytes the operation must refuse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub op: Op,
    /// The operation's input; for a rejection, only what selects the decoder
    /// (a framing's name).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub input: serde_json::Value,
    pub bytes: Hex,
    #[serde(default, skip_serializing_if = "Expect::is_exact")]
    pub expect: Expect,
    #[serde(default, skip_serializing_if = "Compare::is_bytes")]
    pub compare: Compare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// `{name}` → the hash, 8 bytes little-endian. Encode only.
    NameHash,
    /// `{ir_hash, wire_format, framing, capabilities}`, names unhashed.
    Handshake,
    /// `{ir_hash, wire_format, framing, capabilities}`, one end's own
    /// declaration (`comline@1`'s), and `bytes`, the frame its peer sent.
    /// Decoding checks the peer's frame against the input: it's accepted,
    /// and gives the input back, when the schema fingerprint, format and
    /// framing agree (capability bits may differ), and refused otherwise.
    /// Decode only. A handshake profile's facts are about this.
    HandshakeCheck,
    /// A [`Reply`].
    Envelope,
    /// `{framing, call_id, method, request_id, format, params}`. Decoding
    /// yields the call (the id for datagram framing, the method for
    /// JSON-RPC), the request id, and `params` encoded in `format`. A framing
    /// that carries the protocol (`datagram@2`) has a `protocol` number too.
    Request,
    /// `{framing, request_id}` and a [`Reply`], plus `protocol` for a
    /// framing that carries it.
    Response,
    /// `{format, value}` → the encoded value. Encode only.
    Encode,
    /// `{format, type, value}`: decoding `bytes` as `type` gives `value`.
    /// Decode only: the bytes were written by a different type (a field
    /// more, a variant reordered), which is what a profile fact is about.
    /// A rejection's input is `{format, type}`.
    Decode,
    /// `{len, max}` → the 4-byte length prefix of a `len`-byte frame on a
    /// byte stream, under the limit `max`. Decoding reads the prefix back.
    /// A rejection's bytes are a prefix over `max`; its input is
    /// `{len, max}` too, and encoding it must fail as well.
    StreamPrefix,
    /// `{max, frames, rest, needed}` → a byte stream: each of `frames`
    /// (hex) behind its length prefix, then `rest`, the start of a frame
    /// not yet whole. Decoding splits the stream frame by frame under the
    /// limit `max`; `needed` is how many bytes the incomplete frame in
    /// `rest` takes in all, prefix included, or 0 while its prefix is
    /// incomplete. A rejection's bytes declare a frame over `max`; its
    /// input is `{max}`.
    StreamSplit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// Encoding the input gives `bytes`, and decoding `bytes` gives the input.
    #[default]
    Exact,
    /// Decoding `bytes` fails.
    Reject,
}

impl Op {
    /// Whether a case of this op only asks to decode its bytes, never to
    /// encode its input.
    pub fn decode_only(self) -> bool {
        matches!(self, Op::Decode | Op::HandshakeCheck)
    }
}

impl Expect {
    fn is_exact(&self) -> bool {
        *self == Expect::Exact
    }
}

/// How a runtime's output must match a case's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compare {
    /// Byte for byte.
    #[default]
    Bytes,
    /// As JSON values (RFC 8259), for bytes that are JSON text: whitespace
    /// and object member order don't matter, and numbers compare as exact
    /// decimals, so `1` equals `1.0` but `9007199254740992` doesn't equal
    /// `9007199254740993`. The same holds for JSON carried inside the frame:
    /// JSON-RPC params, results and error data. `comline-runtime`, the
    /// reference, reproduces the bytes exactly, except for a `found-*` case
    /// that spells a string another way.
    Json,
}

impl Compare {
    fn is_bytes(&self) -> bool {
        *self == Compare::Bytes
    }
}

/// A response's outcome, as `envelope` and `response` inputs carry it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Ok { payload: Hex },
    Err { id: u16, body: Hex },
    Fault,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct NameHashIn {
    name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct HandshakeIn {
    #[serde(with = "decimal")]
    ir_hash: u64,
    wire_format: String,
    framing: String,
    capabilities: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RequestIn {
    framing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protocol: Option<u16>,
    call_id: u16,
    method: String,
    #[serde(with = "decimal")]
    request_id: u64,
    /// A one-way call, on a connection: JSON-RPC sends it as a notification,
    /// with no `id` (and decoding one gives `request_id` 0).
    #[serde(default, skip_serializing_if = "is_false")]
    one_way: bool,
    format: String,
    params: Value,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ResponseIn {
    framing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protocol: Option<u16>,
    #[serde(with = "decimal")]
    request_id: u64,
    #[serde(flatten)]
    reply: Reply,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StreamPrefixIn {
    len: u64,
    max: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StreamSplitIn {
    max: u64,
    frames: Vec<Hex>,
    rest: Hex,
    needed: u64,
}

/// A stream limit as the runtime takes it: a `max` past `usize` holds every
/// frame `usize` can.
fn stream_max(max: u64) -> usize {
    usize::try_from(max).unwrap_or(usize::MAX)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct EncodeIn {
    format: String,
    value: Value,
}

/// A type a `decode` case reads into, described so any runtime can build a
/// reader for it: a struct's fields in order, an enum's variants in order.
/// It covers every shape a generated Rust type can have; unions, which the
/// Rust generator refuses, aren't here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    String,
    Unit,
    Option(Box<Type>),
    /// `T[]`.
    Array(Box<Type>),
    /// `T[N]`.
    Fixed {
        of: Box<Type>,
        len: usize,
    },
    Struct {
        name: String,
        fields: Vec<(String, Type)>,
    },
    Enum {
        name: String,
        variants: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DecodeIn {
    format: String,
    #[serde(rename = "type")]
    ty: Type,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FramingIn {
    framing: String,
}

/// An encoding's profile (`profiles/<name>.json`): facts about how it
/// encodes, which the version comparison turns into bumps. Each fact names
/// the vector cases that prove it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub kind: ProfileKind,
    pub facts: BTreeMap<String, Fact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Format,
    Framing,
    /// A handshake: what it establishes between the two ends. A fact it
    /// leaves out, or states `false`, isn't established, and the schema has
    /// to give it up (`@unverified`).
    Handshake,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    /// The fact's value. A handshake's facts are JSON booleans, read as
    /// `"true"` / `"false"`; every other fact is a string.
    #[serde(with = "fact_value")]
    pub value: String,
    /// The cases that prove it. A handshake fact stated `false` claims
    /// nothing, so it names none.
    #[serde(default)]
    pub proven_by: Vec<String>,
}

/// A fact's value: a string, or a JSON boolean (a handshake's facts), held
/// as `"true"` / `"false"` and written back as a boolean.
mod fact_value {
    use serde::{Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Written {
        Bool(bool),
        Str(String),
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(match Written::deserialize(d)? {
            Written::Bool(b) => b.to_string(),
            Written::Str(s) => s,
        })
    }

    pub fn serialize<S: Serializer>(value: &str, s: S) -> Result<S::Ok, S::Error> {
        match value {
            "true" => s.serialize_bool(true),
            "false" => s.serialize_bool(false),
            other => s.serialize_str(other),
        }
    }
}

impl ProfileKind {
    /// Every fact a profile of this kind states, with the values it may
    /// take. The set is fixed: a new fact is a change to the comparison too.
    pub fn facts(self) -> &'static [(&'static str, &'static [&'static str])] {
        match self {
            ProfileKind::Format => &[
                ("struct_fields", &["by_position", "by_name"]),
                ("unknown_field", &["rejected", "ignored"]),
                ("missing_optional", &["rejected", "absent"]),
                ("enum_variants", &["by_name", "by_index"]),
                ("unknown_variant", &["rejected", "tolerated"]),
                ("integer_width", &["by_value", "exact"]),
            ],
            ProfileKind::Framing => &[
                ("calls", &["by_index", "by_name", "by_id"]),
                ("errors", &["by_ordinal", "by_name"]),
            ],
            ProfileKind::Handshake => &[
                ("schema_agreement", &["true", "false"]),
                ("encoding_agreement", &["true", "false"]),
            ],
        }
    }

    /// Whether a profile of this kind must state every fact. A handshake's
    /// may leave one out, which reads as `false`: not established.
    pub fn states_every_fact(self) -> bool {
        !matches!(self, ProfileKind::Handshake)
    }
}

/// Whether `cases` (a fact's `proven_by`, in order) show `value` for
/// `fact`, beyond passing: a rejection where the fact says rejected, the
/// decoded value where it says by name or by position.
pub fn fact_shown(fact: &str, value: &str, cases: &[&Case]) -> Result<(), String> {
    if matches!(fact, "schema_agreement" | "encoding_agreement") {
        return handshake_fact_shown(fact, value, cases);
    }
    let decoded = |case: &Case| -> Option<Value> {
        serde_json::from_value::<DecodeIn>(case.input.clone())
            .ok()?
            .value
    };
    let rejected = |case: &Case| case.expect == Expect::Reject;
    let first = cases.first().ok_or("no proving case")?;
    let point = |x: i32, y: i32| Value::Struct {
        name: "Point".into(),
        fields: vec![("x".into(), Value::I32(x)), ("y".into(), Value::I32(y))],
    };
    let shown = match (fact, value) {
        ("struct_fields", "by_name") => decoded(first) == Some(point(1, -2)),
        ("struct_fields", "by_position") => decoded(first) == Some(point(-2, 1)),
        ("enum_variants", "by_name") => {
            matches!(decoded(first), Some(Value::UnitVariant { ref variant, .. }) if variant == "Green")
        }
        ("enum_variants", "by_index") => {
            matches!(decoded(first), Some(Value::UnitVariant { ref variant, .. }) if variant == "Red")
        }
        ("unknown_field" | "missing_optional" | "unknown_variant", "rejected") => rejected(first),
        ("unknown_field", "ignored")
        | ("missing_optional", "absent")
        | ("unknown_variant", "tolerated") => !rejected(first),
        ("integer_width", "by_value") => {
            cases.len() == 2 && !rejected(cases[0]) && rejected(cases[1])
        }
        ("integer_width", "exact") => cases.len() == 2 && cases.iter().all(|c| rejected(c)),
        // Shown by the cases passing: the framing carries the call and the
        // error that way, and decoding reads them back.
        ("calls" | "errors", _) => true,
        _ => return Err(format!("no check for `{fact}: {value}`")),
    };
    if shown {
        Ok(())
    } else {
        Err(format!("its cases don't show `{fact}: {value}`"))
    }
}

/// A handshake fact: `false` claims nothing. `true` is shown by
/// `handshake_check` cases: one where the peer agrees, accepted, and, for
/// each thing the fact covers, one where the peer's frame differs from the
/// input in only that (the schema fingerprint; the format; the framing),
/// refused.
fn handshake_fact_shown(fact: &str, value: &str, cases: &[&Case]) -> Result<(), String> {
    if value == "false" {
        return Ok(());
    }
    // (ir_hash, format, framing) differ, of the peer's frame from the input.
    let differs = |case: &Case| -> Option<(bool, bool, bool)> {
        if case.op != Op::HandshakeCheck {
            return None;
        }
        let i: HandshakeIn = serde_json::from_value(case.input.clone()).ok()?;
        let ours = HandshakeFrame::new(i.ir_hash, &i.wire_format, &i.framing, i.capabilities);
        let peer = HandshakeFrame::decode(&case.bytes.0)?;
        Some((
            ours.ir_hash != peer.ir_hash,
            ours.wire_format != peer.wire_format,
            ours.framing != peer.framing,
        ))
    };
    let shows = |want: (bool, bool, bool), expect: Expect| {
        cases
            .iter()
            .any(|c| c.expect == expect && differs(c) == Some(want))
    };
    let agrees = shows((false, false, false), Expect::Exact);
    let refused = match fact {
        "schema_agreement" => shows((true, false, false), Expect::Reject),
        _ => {
            shows((false, true, false), Expect::Reject)
                && shows((false, false, true), Expect::Reject)
        }
    };
    if agrees && refused {
        Ok(())
    } else {
        Err(format!("its cases don't show `{fact}: {value}`"))
    }
}

/// A feature of the wire contract, from the corpus's `features.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureInfo {
    pub id: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Features {
    pub features: Vec<FeatureInfo>,
}

/// One `vectors/<feature>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vectors {
    pub feature: String,
    pub cases: Vec<Case>,
}

/// What a runtime claims for a feature (`support/<runtime>.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Supported,
    /// Every case passes but those listed in `failing`, which must fail.
    Partial,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking: Option<String>,
    /// For a partial claim: the cases known to fail.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Support {
    pub runtime: String,
    pub support: BTreeMap<String, Claim>,
}

/// A proven claim, in `reports/<runtime>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenClaim {
    #[serde(flatten)]
    pub claim: Claim,
    pub proven_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub runtime: String,
    pub support: BTreeMap<String, ProvenClaim>,
}

/// What running one case against a runtime gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail(String),
    /// The runtime has no implementation of something the case needs: a
    /// framing or format it doesn't ship.
    NotImplemented(String),
}

// --- The reference: vectors computed from `comline-runtime` ---

fn encode_with(format: &str, value: &Value) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match format {
        FORMAT_JSON => Json.encode(&Wire(value), &mut out),
        FORMAT_MSGPACK => MsgPack.encode(&Wire(value), &mut out),
        FORMAT_PACKED => Packed.encode(&Wire(value), &mut out),
        _ => return None,
    }
    .ok()?;
    Some(out)
}

fn framing_of(name: &str) -> Option<AnyFraming> {
    match name {
        FRAMING_DATAGRAM => Some(AnyFraming::Datagram(DatagramFraming)),
        FRAMING_JSONRPC => Some(AnyFraming::JsonRpc(JsonRpcFraming)),
        _ => None,
    }
}

enum AnyFraming {
    Datagram(DatagramFraming),
    JsonRpc(JsonRpcFraming),
}

macro_rules! with_framing {
    ($f:expr, |$x:ident| $body:expr) => {
        match $f {
            AnyFraming::Datagram($x) => $body,
            AnyFraming::JsonRpc($x) => $body,
        }
    };
}

fn encode_reply(reply: &Reply, out: &mut Vec<u8>) {
    match reply {
        Reply::Ok { payload } => Envelope::encode_ok(&payload.0, out),
        Reply::Err { id, body } => Envelope::encode_err(*id, &body.0, out),
        Reply::Fault => Envelope::encode_fault(out),
    }
}

fn envelope_of(reply: &Reply) -> Envelope<'_> {
    match reply {
        Reply::Ok { payload } => Envelope::Ok(&payload.0),
        Reply::Err { id, body } => Envelope::Err {
            id: *id,
            body: &body.0,
        },
        Reply::Fault => Envelope::Fault,
    }
}

fn reply_of(envelope: Envelope) -> Reply {
    match envelope {
        Envelope::Ok(payload) => Reply::Ok {
            payload: Hex(payload.to_vec()),
        },
        Envelope::Err { id, body } => Reply::Err {
            id,
            body: Hex(body.to_vec()),
        },
        Envelope::Fault => Reply::Fault,
    }
}

/// A request's frame as a caller that serialises the params itself makes
/// it: the params with the format alone, then `encode_request_body`, once
/// per trait the framing frames requests through. Each must equal what
/// [`encode`] writes. The method name is borrowed, not interned, as a
/// binding's is.
fn encode_request_bodies(
    input: &serde_json::Value,
) -> Result<Vec<(&'static str, Vec<u8>)>, Outcome> {
    let i: RequestIn = parse(input)?;
    let mut params = Vec::new();
    match i.format.as_str() {
        FORMAT_JSON => Json.encode(&Wire(&i.params), &mut params),
        FORMAT_MSGPACK => MsgPack.encode(&Wire(&i.params), &mut params),
        other => return Err(Outcome::NotImplemented(format!("format `{other}`"))),
    }
    .map_err(|e| Outcome::Fail(format!("encoding the params failed: {e:?}")))?;
    let (call, name, id) = (i.call_id, i.method.as_str(), i.request_id);
    let framed = |encode: &dyn Fn(&mut Vec<u8>)| {
        let mut out = Vec::new();
        encode(&mut out);
        out
    };
    let mut bodies = Vec::new();
    match i.framing.as_str() {
        FRAMING_DATAGRAM => bodies.push((
            "Framing::encode_request_body",
            framed(&|out| DatagramFraming.encode_request_body(call, name, id, &params, out)),
        )),
        FRAMING_DATAGRAM2 => {
            let protocol = i
                .protocol
                .ok_or_else(|| Outcome::Fail("no `protocol`".into()))?;
            bodies.push((
                "ConnectionFraming::encode_request_body",
                framed(&|out| {
                    Datagram2Framing
                        .encode_request_body(protocol, call, name, id, i.one_way, &params, out)
                }),
            ));
        }
        FRAMING_JSONRPC => {
            if !i.one_way {
                bodies.push((
                    "Framing::encode_request_body",
                    framed(&|out| {
                        Framing::encode_request_body(&JsonRpcFraming, call, name, id, &params, out)
                    }),
                ));
            }
            bodies.push((
                "ConnectionFraming::encode_request_body",
                framed(&|out| {
                    ConnectionFraming::encode_request_body(
                        &JsonRpcFraming,
                        0,
                        call,
                        name,
                        id,
                        i.one_way,
                        &params,
                        out,
                    )
                }),
            ));
        }
        other => return Err(Outcome::NotImplemented(format!("framing `{other}`"))),
    }
    Ok(bodies)
}

/// Encodes `input` for `op` with `comline-runtime`.
fn encode(op: Op, input: &serde_json::Value) -> Result<Vec<u8>, Outcome> {
    let mut out = Vec::new();
    match op {
        Op::NameHash => {
            let i: NameHashIn = parse(input)?;
            out.extend_from_slice(&name_hash(&i.name).to_le_bytes());
        }
        Op::Handshake => {
            let i: HandshakeIn = parse(input)?;
            HandshakeFrame::new(i.ir_hash, &i.wire_format, &i.framing, i.capabilities)
                .encode(&mut out);
        }
        Op::Envelope => encode_reply(&parse(input)?, &mut out),
        Op::Request => {
            let i: RequestIn = parse(input)?;
            if i.framing == FRAMING_DATAGRAM2 {
                let protocol = i
                    .protocol
                    .ok_or_else(|| Outcome::Fail("no `protocol`".into()))?;
                let call = Call::new(i.call_id, intern::name(&i.method));
                let f = Datagram2Framing;
                let encoded = match i.format.as_str() {
                    FORMAT_JSON => f.encode_request(
                        protocol,
                        call,
                        i.request_id,
                        i.one_way,
                        &Wire(&i.params),
                        &Json,
                        &mut out,
                    ),
                    FORMAT_MSGPACK => f.encode_request(
                        protocol,
                        call,
                        i.request_id,
                        i.one_way,
                        &Wire(&i.params),
                        &MsgPack,
                        &mut out,
                    ),
                    other => return Err(Outcome::NotImplemented(format!("format `{other}`"))),
                };
                encoded.map_err(|e| Outcome::Fail(format!("encoding failed: {e:?}")))?;
                return Ok(out);
            }
            if i.one_way && i.framing == FRAMING_JSONRPC {
                // Only a connection sends one-way calls as notifications.
                let call = Call::new(i.call_id, intern::name(&i.method));
                let f = JsonRpcFraming;
                let encoded = match i.format.as_str() {
                    FORMAT_JSON => ConnectionFraming::encode_request(
                        &f,
                        0,
                        call,
                        i.request_id,
                        true,
                        &Wire(&i.params),
                        &Json,
                        &mut out,
                    ),
                    other => return Err(Outcome::NotImplemented(format!("format `{other}`"))),
                };
                encoded.map_err(|e| Outcome::Fail(format!("encoding failed: {e:?}")))?;
                return Ok(out);
            }
            let framing = framing_of(&i.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", i.framing)))?;
            let call = Call::new(i.call_id, intern::name(&i.method));
            let encoded = match i.format.as_str() {
                FORMAT_JSON => with_framing!(framing, |f| Framing::encode_request(
                    &f,
                    call,
                    i.request_id,
                    &Wire(&i.params),
                    &Json,
                    &mut out
                )),
                FORMAT_MSGPACK => with_framing!(framing, |f| Framing::encode_request(
                    &f,
                    call,
                    i.request_id,
                    &Wire(&i.params),
                    &MsgPack,
                    &mut out
                )),
                other => return Err(Outcome::NotImplemented(format!("format `{other}`"))),
            };
            encoded.map_err(|e| Outcome::Fail(format!("encoding failed: {e:?}")))?;
        }
        Op::Response => {
            let i: ResponseIn = parse(input)?;
            if i.framing == FRAMING_DATAGRAM2 {
                let protocol = i
                    .protocol
                    .ok_or_else(|| Outcome::Fail("no `protocol`".into()))?;
                Datagram2Framing.encode_reply(
                    protocol,
                    i.request_id,
                    &envelope_of(&i.reply),
                    &mut out,
                );
                return Ok(out);
            }
            let framing = framing_of(&i.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", i.framing)))?;
            with_framing!(framing, |f| match &i.reply {
                Reply::Ok { payload } => f.encode_response_ok(i.request_id, &payload.0, &mut out),
                Reply::Err { id, body } => {
                    f.encode_response_err(i.request_id, *id, &body.0, &mut out)
                }
                Reply::Fault => f.encode_response_fault(i.request_id, &mut out),
            });
        }
        Op::Encode => {
            let i: EncodeIn = parse(input)?;
            return encode_with(&i.format, &i.value)
                .ok_or_else(|| Outcome::NotImplemented(format!("format `{}`", i.format)));
        }
        Op::Decode => return Err(Outcome::Fail("`decode` cases are decode only".into())),
        Op::HandshakeCheck => {
            return Err(Outcome::Fail(
                "`handshake_check` cases are decode only".into(),
            ))
        }
        Op::StreamPrefix => {
            let i: StreamPrefixIn = parse(input)?;
            let len = usize::try_from(i.len)
                .map_err(|_| Outcome::Fail(format!("refused: a {}-byte frame", i.len)))?;
            let mut prefix = [0u8; PREFIX_LEN];
            stream::put_prefix(len, stream_max(i.max), &mut SliceBuf::new(&mut prefix))
                .map_err(|e| Outcome::Fail(format!("refused: {e}")))?;
            out.extend_from_slice(&prefix);
        }
        Op::StreamSplit => {
            let i: StreamSplitIn = parse(input)?;
            for frame in &i.frames {
                let mut prefix = [0u8; PREFIX_LEN];
                stream::put_prefix(
                    frame.0.len(),
                    stream_max(i.max),
                    &mut SliceBuf::new(&mut prefix),
                )
                .map_err(|e| Outcome::Fail(format!("refused: {e}")))?;
                out.extend_from_slice(&prefix);
                out.extend_from_slice(&frame.0);
            }
            out.extend_from_slice(&i.rest.0);
        }
    }
    Ok(out)
}

/// Decodes `bytes` for `op` with `comline-runtime`, as the input that would
/// encode to them; `Ok(None)` when the runtime refuses the bytes.
fn decode(
    op: Op,
    input: &serde_json::Value,
    bytes: &[u8],
) -> Result<Option<serde_json::Value>, Outcome> {
    let json = |v: serde_json::Value| -> Result<Option<serde_json::Value>, Outcome> { Ok(Some(v)) };
    match op {
        Op::NameHash | Op::Encode => Ok(None),
        Op::StreamPrefix => {
            #[derive(Deserialize)]
            struct Max {
                max: u64,
            }
            let i: Max = parse(input)?;
            let Ok(prefix) = <[u8; PREFIX_LEN]>::try_from(bytes) else {
                return Ok(None);
            };
            Ok(stream::frame_len(prefix, stream_max(i.max)).map(|len| {
                to_json(&StreamPrefixIn {
                    len: len as u64,
                    max: i.max,
                })
            }))
        }
        Op::StreamSplit => {
            #[derive(Deserialize)]
            struct Max {
                max: u64,
            }
            let i: Max = parse(input)?;
            let mut frames = Vec::new();
            let mut at = 0;
            loop {
                match stream::split_frame(&bytes[at..], stream_max(i.max)) {
                    Split::Frame { start, len } => {
                        frames.push(Hex(bytes[at + start..at + start + len].to_vec()));
                        at += start + len;
                    }
                    Split::NeedMore { needed } => {
                        return json(to_json(&StreamSplitIn {
                            max: i.max,
                            frames,
                            rest: Hex(bytes[at..].to_vec()),
                            needed: needed as u64,
                        }));
                    }
                    Split::TooLarge => return Ok(None),
                }
            }
        }
        Op::Decode => {
            let i: DecodeIn = parse(input)?;
            Ok(decode_typed(&i.format, &i.ty, bytes)?.map(|value| {
                to_json(&DecodeIn {
                    format: i.format,
                    ty: i.ty,
                    value: Some(value),
                })
            }))
        }
        Op::Handshake => {
            let Some(h) = HandshakeFrame::decode(bytes) else {
                return Ok(None);
            };
            // The frame carries hashes; compare them against the input's names.
            let i: Option<HandshakeIn> = serde_json::from_value(input.clone()).ok();
            let named = |hash: u64, name: Option<&String>| match name {
                Some(n) if name_hash(n) == hash => n.clone(),
                _ => format!("{hash:#018x}"),
            };
            json(serde_json::json!({
                "ir_hash": h.ir_hash.to_string(),
                "wire_format": named(h.wire_format, i.as_ref().map(|i| &i.wire_format)),
                "framing": named(h.framing, i.as_ref().map(|i| &i.framing)),
                "capabilities": h.capabilities,
            }))
        }
        Op::HandshakeCheck => {
            let i: HandshakeIn = parse(input)?;
            let ours = HandshakeFrame::new(i.ir_hash, &i.wire_format, &i.framing, i.capabilities);
            match ours.check_bytes(bytes) {
                Ok(_) => json(input.clone()),
                Err(_) => Ok(None),
            }
        }
        Op::Envelope => match Envelope::decode(bytes) {
            Some(e) => json(to_json(&reply_of(e))),
            None => Ok(None),
        },
        Op::Request if is_datagram2(input) => {
            let Some(Frame::Request {
                protocol: Some(protocol),
                request,
                ..
            }) = Datagram2Framing.decode(bytes)
            else {
                return Ok(None);
            };
            let Ok(i) = serde_json::from_value::<RequestIn>(input.clone()) else {
                return Ok(Some(serde_json::json!({ "accepted": true })));
            };
            let params = encode_with(&i.format, &i.params)
                .ok_or_else(|| Outcome::NotImplemented(format!("format `{}`", i.format)))?;
            let params = if request.params == params.as_slice() {
                to_json(&i.params)
            } else {
                serde_json::json!({ "undecoded": to_json(&Hex(request.params.to_vec())) })
            };
            let RequestCall::Id(call_id) = request.call else {
                return Ok(None);
            };
            json(serde_json::json!({
                "framing": i.framing,
                "protocol": protocol,
                "call_id": call_id,
                "method": i.method,
                "request_id": request.request_id.to_string(),
                "format": i.format,
                "params": params,
            }))
        }
        Op::Response if is_datagram2(input) => match Datagram2Framing.decode(bytes) {
            Some(Frame::Reply {
                protocol,
                request_id,
                envelope,
            }) => json(to_json(&ResponseIn {
                framing: FRAMING_DATAGRAM2.into(),
                protocol,
                request_id,
                reply: reply_of(envelope),
            })),
            _ => Ok(None),
        },
        Op::Request => {
            let i: RequestIn = match serde_json::from_value(input.clone()) {
                Ok(i) => i,
                Err(_) => {
                    return decode_reject_only(input, |f| {
                        with_framing!(f, |f| f.decode_request(bytes).is_some())
                    })
                }
            };
            let framing = framing_of(&i.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", i.framing)))?;
            // A notification is a connection's frame: read it as one, which
            // says whether the frame wanted a reply.
            let (req, one_way) = if i.one_way && i.framing == FRAMING_JSONRPC {
                match ConnectionFraming::decode(&JsonRpcFraming, bytes) {
                    Some(Frame::Request {
                        request, one_way, ..
                    }) => (request, one_way),
                    _ => return Ok(None),
                }
            } else {
                match with_framing!(framing, |f| f.decode_request(bytes)) {
                    Some(req) => (req, false),
                    None => return Ok(None),
                }
            };
            let (call_id, method) = match req.call {
                RequestCall::Id(id) => (id, i.method.clone()),
                RequestCall::Name(name) => (i.call_id, name.to_string()),
            };
            let params = encode_with(&i.format, &i.params)
                .ok_or_else(|| Outcome::NotImplemented(format!("format `{}`", i.format)))?;
            // Decoding gives params as bytes; they stand for the input's
            // value only when they're its exact encoding.
            let params = if req.params == params.as_slice() {
                to_json(&i.params)
            } else {
                serde_json::json!({ "undecoded": to_json(&Hex(req.params.to_vec())) })
            };
            let mut decoded = serde_json::json!({
                "framing": i.framing,
                "call_id": call_id,
                "method": method,
                "request_id": req.request_id.to_string(),
                "format": i.format,
                "params": params,
            });
            if one_way {
                decoded["one_way"] = true.into();
            }
            json(decoded)
        }
        Op::Response => {
            let f: FramingIn = parse(input)?;
            let framing = framing_of(&f.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", f.framing)))?;
            match with_framing!(framing, |x| x.decode_response(bytes)) {
                Some((request_id, envelope)) => json(to_json(&ResponseIn {
                    framing: f.framing,
                    protocol: None,
                    request_id,
                    reply: reply_of(envelope),
                })),
                None => Ok(None),
            }
        }
    }
}

fn is_datagram2(input: &serde_json::Value) -> bool {
    input["framing"] == FRAMING_DATAGRAM2
}

/// For a rejection case, whose input names only the framing: whether the
/// framing accepted the bytes.
fn decode_reject_only<F>(
    input: &serde_json::Value,
    accepts: F,
) -> Result<Option<serde_json::Value>, Outcome>
where
    F: FnOnce(AnyFraming) -> bool,
{
    let f: FramingIn = parse(input)?;
    let framing = framing_of(&f.framing)
        .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", f.framing)))?;
    Ok(accepts(framing).then(|| serde_json::json!({ "accepted": true })))
}

fn parse<T: for<'de> Deserialize<'de>>(input: &serde_json::Value) -> Result<T, Outcome> {
    serde_json::from_value(input.clone()).map_err(|e| Outcome::Fail(format!("bad input: {e}")))
}

fn to_json<T: Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).expect("vector types serialize")
}

/// The reference's answer to one question: what `comline-runtime` encodes
/// an input to, or what it decodes bytes to.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The input encodes to these bytes.
    Bytes(Vec<u8>),
    /// The bytes decode, to this input (in the case's own form).
    Accept(serde_json::Value),
    /// The runtime refuses the bytes.
    Reject,
    /// The runtime has no implementation of something the question needs.
    NotImplemented(String),
    /// The question itself is malformed.
    Bad(String),
}

/// Asks `comline-runtime` one question, as a case states it: with no
/// `bytes`, what `input` encodes to for `op`; with `bytes`, whether they
/// decode, and to what.
pub fn answer(op: Op, input: &serde_json::Value, bytes: Option<&[u8]>) -> Answer {
    let answered = match bytes {
        None => encode(op, input).map(Answer::Bytes),
        Some(bytes) => {
            decode(op, input, bytes).map(|decoded| decoded.map_or(Answer::Reject, Answer::Accept))
        }
    };
    answered.unwrap_or_else(|outcome| match outcome {
        Outcome::NotImplemented(what) => Answer::NotImplemented(what),
        Outcome::Fail(why) => Answer::Bad(why),
        Outcome::Pass => unreachable!("encode and decode fail with a reason"),
    })
}

/// Runs one case against `comline-runtime`.
pub fn run(case: &Case) -> Outcome {
    match run_inner(case) {
        Ok(()) => Outcome::Pass,
        Err(outcome) => outcome,
    }
}

fn run_inner(case: &Case) -> Result<(), Outcome> {
    let bytes = &case.bytes.0;
    if case.expect == Expect::Reject {
        if case.op == Op::StreamPrefix {
            if let Ok(encoded) = encode(case.op, &case.input) {
                return Err(Outcome::Fail(format!(
                    "prefixed a frame over the limit, as {}",
                    to_json(&Hex(encoded))
                )));
            }
        }
        return match decode(case.op, &case.input, bytes)? {
            None => Ok(()),
            Some(got) => Err(Outcome::Fail(format!(
                "accepted bytes it must reject, as {got}"
            ))),
        };
    }

    if !case.op.decode_only() {
        let same = |encoded: &[u8]| {
            encoded == bytes || (case.compare == Compare::Json && json_equal(encoded, bytes))
        };
        let encoded = encode(case.op, &case.input)?;
        if !same(&encoded) {
            return Err(Outcome::Fail(format!(
                "encodes to {}, expected {}",
                to_json(&Hex(encoded)),
                to_json(&case.bytes),
            )));
        }
        if case.op == Op::Request {
            for (path, encoded) in encode_request_bodies(&case.input)? {
                if !same(&encoded) {
                    return Err(Outcome::Fail(format!(
                        "{path} encodes to {}, expected {}",
                        to_json(&Hex(encoded)),
                        to_json(&case.bytes),
                    )));
                }
            }
        }
        if matches!(case.op, Op::NameHash | Op::Encode) {
            return Ok(());
        }
    }
    match decode(case.op, &case.input, bytes)? {
        Some(got) if got == case.input => Ok(()),
        Some(got) => Err(Outcome::Fail(format!(
            "decodes to {got}, expected {}",
            case.input
        ))),
        None => Err(Outcome::Fail("refused bytes it must decode".into())),
    }
}

/// Whether two JSON texts are the same JSON value, for a case that compares
/// as JSON whose bytes aren't the reference's own spelling: whitespace and
/// member order don't matter, strings compare by the text they hold, and
/// numbers compare as exact decimals (`1` equals `1.0` and `10e-1`, but
/// `9007199254740992` doesn't equal `9007199254740993`).
pub fn json_equal(a: &[u8], b: &[u8]) -> bool {
    // serde_json decides what is valid JSON (and caps the depth); the tree
    // below only exists because it can't keep a number's exact digits.
    if serde_json::from_slice::<serde::de::IgnoredAny>(a).is_err()
        || serde_json::from_slice::<serde::de::IgnoredAny>(b).is_err()
    {
        return false;
    }
    match (exact_json(a), exact_json(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// A JSON value whose numbers keep their exact decimal value.
#[derive(PartialEq)]
enum Exact {
    Null,
    Bool(bool),
    /// Normalised: sign, significant digits without a leading or trailing
    /// zero, and the power of ten they are scaled by. Zero is `(false, "", 0)`.
    Number(bool, String, i64),
    String(String),
    Array(Vec<Exact>),
    Object(std::collections::BTreeMap<String, Exact>),
}

/// Parses JSON text already known to be valid; `None` on a number whose
/// exponent doesn't fit in an `i64`.
fn exact_json(text: &[u8]) -> Option<Exact> {
    let mut at = 0;
    let value = exact_value(text, &mut at)?;
    Some(value)
}

fn exact_skip(text: &[u8], at: &mut usize) {
    while matches!(text.get(*at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        *at += 1;
    }
}

fn exact_string(text: &[u8], at: &mut usize) -> Option<String> {
    let start = *at;
    *at += 1;
    while *text.get(*at)? != b'"' {
        *at += if text[*at] == b'\\' { 2 } else { 1 };
    }
    *at += 1;
    serde_json::from_slice(&text[start..*at]).ok()
}

fn exact_value(text: &[u8], at: &mut usize) -> Option<Exact> {
    exact_skip(text, at);
    let value = match *text.get(*at)? {
        b'n' => {
            *at += 4;
            Exact::Null
        }
        b't' => {
            *at += 4;
            Exact::Bool(true)
        }
        b'f' => {
            *at += 5;
            Exact::Bool(false)
        }
        b'"' => Exact::String(exact_string(text, at)?),
        b'[' => {
            *at += 1;
            let mut items = Vec::new();
            loop {
                exact_skip(text, at);
                if text[*at] == b']' {
                    *at += 1;
                    break;
                }
                if text[*at] == b',' {
                    *at += 1;
                }
                items.push(exact_value(text, at)?);
            }
            Exact::Array(items)
        }
        b'{' => {
            *at += 1;
            let mut members = std::collections::BTreeMap::new();
            loop {
                exact_skip(text, at);
                match text[*at] {
                    b'}' => {
                        *at += 1;
                        break;
                    }
                    b',' => {
                        *at += 1;
                        continue;
                    }
                    _ => {}
                }
                let key = exact_string(text, at)?;
                exact_skip(text, at);
                *at += 1; // ':'
                members.insert(key, exact_value(text, at)?);
            }
            Exact::Object(members)
        }
        _ => {
            let start = *at;
            while matches!(
                text.get(*at),
                Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
            ) {
                *at += 1;
            }
            exact_number(std::str::from_utf8(&text[start..*at]).ok()?)?
        }
    };
    Some(value)
}

fn exact_number(number: &str) -> Option<Exact> {
    let (negative, number) = match number.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, number),
    };
    let (mantissa, exponent) = match number.find(['e', 'E']) {
        Some(at) => (&number[..at], number[at + 1..].parse::<i64>().ok()?),
        None => (number, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let mut scale = exponent.checked_sub(fraction.len() as i64)?;
    let digits = digits.trim_start_matches('0');
    let trimmed = digits.trim_end_matches('0');
    scale = scale.checked_add((digits.len() - trimmed.len()) as i64)?;
    if trimmed.is_empty() {
        return Some(Exact::Number(false, String::new(), 0));
    }
    Some(Exact::Number(negative, trimmed.to_string(), scale))
}

/// Every feature's vectors, computed from `comline-runtime`: what
/// `vectors/` must hold. A change here is a wire-protocol change.
pub fn reference_cases() -> Vec<Vectors> {
    let mut vectors = Vec::new();
    let mut feature = |id: &str, cases: Vec<(String, Op, serde_json::Value)>| {
        vectors.push(Vectors {
            feature: id.into(),
            cases: cases
                .into_iter()
                .map(|(id, op, input)| exact(id, op, input))
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
            FRAMING_JSONRPC,
            "com.acme.custom-format",
        ]
        .iter()
        .map(|name| {
            let id = if name.is_empty() { "empty" } else { name };
            (
                format!("name-hash/{id}"),
                Op::NameHash,
                serde_json::json!({ "name": name }),
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
            ("envelope/ok".into(), Op::Envelope, to_json(&ok(b"payload"))),
            ("envelope/ok-empty".into(), Op::Envelope, to_json(&ok(b""))),
            (
                "envelope/err".into(),
                Op::Envelope,
                to_json(&err(0x0102, b"fields")),
            ),
            (
                "envelope/err-empty-body".into(),
                Op::Envelope,
                to_json(&err(0, b"")),
            ),
            (
                "envelope/fault".into(),
                Op::Envelope,
                to_json(&Reply::Fault),
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
            let (id, op, mut input) = request(
                p("notification"),
                framing,
                0,
                "chat.ChatClient/pushed",
                0,
                &said,
            );
            input["one_way"] = true.into();
            connection.push((id, op, input));
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
            ]
            .concat(),
        );
    }

    let p = |id: &str| format!("framing-datagram2/{id}");
    let on = |(id, op, mut input): (String, Op, serde_json::Value), protocol: u16| {
        input["protocol"] = protocol.into();
        (id, op, input)
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
        match v.feature.as_str() {
            "format-json" => {
                v.cases.extend(decode_cases("format-json", FORMAT_JSON));
                v.cases.extend(found_cases("format-json", FORMAT_JSON));
            }
            "format-msgpack" => v
                .cases
                .extend(decode_cases("format-msgpack", FORMAT_MSGPACK)),
            "format-packed" => v.cases.extend(decode_cases("format-packed", FORMAT_PACKED)),
            "framing-jsonrpc" => v.cases.extend(found_jsonrpc_cases()),
            "handshake" => v.cases.extend(handshake_check_cases()),
            _ => {}
        }
    }
    vectors
}

fn exact(id: String, op: Op, input: serde_json::Value) -> Case {
    let bytes = match encode(op, &input) {
        Ok(bytes) => bytes,
        Err(outcome) => panic!("reference case `{id}` doesn't encode: {outcome:?}"),
    };
    let json_text = match op {
        Op::Encode => input["format"] == FORMAT_JSON,
        Op::Request | Op::Response => input["framing"] == FRAMING_JSONRPC,
        _ => false,
    };
    Case {
        id,
        op,
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

fn reject(id: &str, op: Op, input: serde_json::Value, bytes: &[u8]) -> Case {
    Case {
        id: id.into(),
        op,
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
) -> (String, Op, serde_json::Value) {
    let input = HandshakeIn {
        ir_hash,
        wire_format: wire_format.into(),
        framing: framing.into(),
        capabilities,
    };
    (id.into(), Op::Handshake, to_json(&input))
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
    let input = to_json(&ours);
    let check = |id: &str, bytes: Vec<u8>| {
        let case = Case {
            id: format!("handshake/{id}"),
            op: Op::HandshakeCheck,
            input: input.clone(),
            bytes: Hex(bytes),
            expect: Expect::Exact,
            compare: Compare::Bytes,
        };
        match decode(Op::HandshakeCheck, &case.input, &case.bytes.0) {
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
) -> (String, Op, serde_json::Value) {
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
    (id, Op::Request, to_json(&input))
}

fn response(
    id: String,
    framing: &str,
    request_id: u64,
    reply: Reply,
) -> (String, Op, serde_json::Value) {
    let input = ResponseIn {
        framing: framing.into(),
        protocol: None,
        request_id,
        reply,
    };
    (id, Op::Response, to_json(&input))
}

/// Sample values for a wire format: each scalar at its edges, then the
/// shapes generated types take.
fn format_cases(feature_id: &str, format: &str) -> Vec<(String, Op, serde_json::Value)> {
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
    ];
    samples
        .into_iter()
        .map(|(id, value)| {
            let input = EncodeIn {
                format: format.into(),
                value,
            };
            (format!("{feature_id}/{id}"), Op::Encode, to_json(&input))
        })
        .collect()
}

/// Shapes only `packed@1` vectors carry: widths the other formats don't
/// keep apart (`f32`), a fixed-size array, and the proposal's example.
fn packed_cases() -> Vec<(String, Op, serde_json::Value)> {
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
                value,
            };
            (format!("format-packed/{id}"), Op::Encode, to_json(&input))
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

/// Decodes `bytes` as `ty` the way a generated type with that shape would,
/// through [`TypeSeed`]; `Ok(None)` when the format refuses them.
pub fn decode_typed(format: &str, ty: &Type, bytes: &[u8]) -> Result<Option<Value>, Outcome> {
    let seed = TypeSeed(ty);
    Ok(match format {
        FORMAT_JSON => Json.decode_seed(seed, bytes).ok(),
        FORMAT_MSGPACK => MsgPack.decode_seed(seed, bytes).ok(),
        FORMAT_PACKED => Packed.decode_seed(seed, bytes).ok(),
        other => return Err(Outcome::NotImplemented(format!("format `{other}`"))),
    })
}

/// Bytes `written` encodes to, read back as `ty`: an exact case with the
/// value the runtime decoded, or a rejection if it refused them.
fn decode_case(id: String, format: &str, written: &Value, ty: Type) -> Case {
    let bytes = encode_with(format, written).expect("reference formats encode");
    match decode_typed(format, &ty, &bytes) {
        Ok(Some(value)) => Case {
            id,
            op: Op::Decode,
            input: to_json(&DecodeIn {
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
            Op::Decode,
            to_json(&DecodeIn {
                format: format.into(),
                ty,
                value: None,
            }),
            &bytes,
        ),
        Err(outcome) => panic!("reference case `{id}` doesn't decode: {outcome:?}"),
    }
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
        let decoded = serde_json::from_value::<DecodeIn>(case.input.clone())
            .ok()
            .and_then(|i| i.value);
        assert_eq!(decoded, Some(written), "`{}` doesn't read back", case.id);
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
        let (id, op, input) = request(
            format!("framing-jsonrpc/found-{id}"),
            FRAMING_JSONRPC,
            0,
            method,
            1,
            &Value::Array(vec![]),
        );
        let case = Case {
            id,
            op,
            input,
            bytes: Hex(bytes.to_vec()),
            expect: Expect::Exact,
            compare: Compare::Json,
        };
        assert_ne!(
            encode(op, &case.input).ok().as_deref(),
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

/// Stream framing (`contract::stream`): length prefixes, and streams split
/// into frames, at the default limit and at a small one, on both sides of
/// each.
fn stream_cases() -> Vec<(String, Op, serde_json::Value)> {
    let default = DEFAULT_MAX_FRAME as u64;
    let ceiling = u64::from(u32::MAX);
    let prefix = |id: &str, len: u64, max: u64| {
        (
            format!("stream/{id}"),
            Op::StreamPrefix,
            serde_json::json!({ "len": len, "max": max }),
        )
    };
    let split = |id: &str, max: u64, frames: &[&[u8]], rest: &[u8], needed: u64| {
        let frames: Vec<Hex> = frames.iter().map(|f| Hex(f.to_vec())).collect();
        (
            format!("stream/{id}"),
            Op::StreamSplit,
            to_json(&StreamSplitIn {
                max,
                frames,
                rest: Hex(rest.to_vec()),
                needed,
            }),
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
    let none = serde_json::Value::Null;
    let mut handshake = Vec::new();
    HandshakeFrame::new(1, FORMAT_JSON, FRAMING_DATAGRAM, 0).encode(&mut handshake);
    let framing = |name: &str| serde_json::json!({ "framing": name });
    match feature {
        "handshake" => {
            let mut bad_magic = handshake.clone();
            bad_magic[0] = b'X';
            let mut bad_version = handshake.clone();
            bad_version[2] = 2;
            vec![
                reject("handshake/reject-empty", Op::Handshake, none.clone(), &[]),
                reject(
                    "handshake/reject-truncated",
                    Op::Handshake,
                    none.clone(),
                    &handshake[..30],
                ),
                reject(
                    "handshake/reject-bad-magic",
                    Op::Handshake,
                    none.clone(),
                    &bad_magic,
                ),
                reject(
                    "handshake/reject-bad-version",
                    Op::Handshake,
                    none,
                    &bad_version,
                ),
            ]
        }
        "stream" => {
            let default = DEFAULT_MAX_FRAME as u64;
            let over_default = (DEFAULT_MAX_FRAME as u32 + 1).to_le_bytes();
            let prefix = |len: u64, max: u64| serde_json::json!({ "len": len, "max": max });
            let max = |max: u64| serde_json::json!({ "max": max });
            vec![
                reject(
                    "stream/reject-prefix-over-default-max",
                    Op::StreamPrefix,
                    prefix(default + 1, default),
                    &over_default,
                ),
                reject(
                    "stream/reject-prefix-over-small-max",
                    Op::StreamPrefix,
                    prefix(3, 2),
                    &[3, 0, 0, 0],
                ),
                reject(
                    "stream/reject-prefix-u32-max-over-default-max",
                    Op::StreamPrefix,
                    prefix(u64::from(u32::MAX), default),
                    &[0xff; 4],
                ),
                reject(
                    "stream/reject-split-over-default-max",
                    Op::StreamSplit,
                    max(default),
                    &over_default,
                ),
                reject(
                    "stream/reject-split-u32-max",
                    Op::StreamSplit,
                    max(default),
                    &[0xff; 4],
                ),
                reject(
                    "stream/reject-split-over-small-max",
                    Op::StreamSplit,
                    max(2),
                    &[3, 0, 0, 0, b'a', b'b', b'c'],
                ),
                reject(
                    "stream/reject-split-frame-then-over-max",
                    Op::StreamSplit,
                    max(2),
                    &[1, 0, 0, 0, b'a', 3, 0, 0, 0],
                ),
            ]
        }
        "envelope" => vec![
            reject("envelope/reject-empty", Op::Envelope, none.clone(), &[]),
            reject(
                "envelope/reject-unknown-tag",
                Op::Envelope,
                none.clone(),
                &[3],
            ),
            reject("envelope/reject-truncated-err", Op::Envelope, none, &[1, 0]),
        ],
        "framing-datagram" => vec![
            reject(
                "framing-datagram/reject-short-request",
                Op::Request,
                framing(FRAMING_DATAGRAM),
                &[3, 0, 42, 0, 0, 0, 0, 0, 0],
            ),
            reject(
                "framing-datagram/reject-short-response",
                Op::Response,
                framing(FRAMING_DATAGRAM),
                &[7, 0, 0, 0, 0, 0, 0],
            ),
            reject(
                "framing-datagram/reject-response-without-envelope",
                Op::Response,
                framing(FRAMING_DATAGRAM),
                &[7, 0, 0, 0, 0, 0, 0, 0],
            ),
        ],
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
            let d2 = || framing(FRAMING_DATAGRAM2);
            vec![
                reject(
                    "framing-datagram2/reject-unknown-kind",
                    Op::Request,
                    d2(),
                    &unknown_kind,
                ),
                reject(
                    "framing-datagram2/reject-overlong-protocol",
                    Op::Request,
                    d2(),
                    &overlong,
                ),
                reject(
                    "framing-datagram2/reject-reply-as-request",
                    Op::Request,
                    d2(),
                    &reply,
                ),
                reject(
                    "framing-datagram2/reject-request-as-reply",
                    Op::Response,
                    d2(),
                    &request,
                ),
                reject(
                    "framing-datagram2/reject-short-request",
                    Op::Request,
                    d2(),
                    &request[..9],
                ),
                reject(
                    "framing-datagram2/reject-reply-without-envelope",
                    Op::Response,
                    d2(),
                    &reply[..10],
                ),
            ]
        }
        "format-packed" => {
            let decode = |ty: Type| {
                to_json(&DecodeIn {
                    format: FORMAT_PACKED.into(),
                    ty,
                    value: None,
                })
            };
            let r = |id: &str, ty: Type, bytes: &[u8]| {
                reject(
                    &format!("format-packed/{id}"),
                    Op::Decode,
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
                Op::Request,
                framing(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.0\",",
            ),
            reject(
                "framing-jsonrpc/reject-request-without-method",
                Op::Request,
                framing(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","params":[],"id":1}"#,
            ),
            reject(
                "framing-jsonrpc/reject-response-without-id",
                Op::Response,
                framing(FRAMING_JSONRPC),
                br#"{"jsonrpc":"2.0","result":1}"#,
            ),
            // Found by comline-typescript's differential tests (#42): JSON
            // text is UTF-8 throughout, so `\xff` is refused even in a member
            // the framing ignores.
            reject(
                "framing-jsonrpc/found-reject-request-invalid-utf8-in-jsonrpc",
                Op::Request,
                framing(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.\xff\",\"method\":\"greet\",\"params\":[],\"id\":1}",
            ),
            reject(
                "framing-jsonrpc/found-reject-request-invalid-utf8-in-unknown-member",
                Op::Request,
                framing(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.0\",\"method\":\"greet\",\"params\":[],\"x\":\"\xff\",\"id\":1}",
            ),
            reject(
                "framing-jsonrpc/found-reject-response-invalid-utf8-in-jsonrpc",
                Op::Response,
                framing(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.\xff\",\"result\":1,\"id\":1}",
            ),
            reject(
                "framing-jsonrpc/found-reject-response-invalid-utf8-in-unknown-member",
                Op::Response,
                framing(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.0\",\"result\":1,\"x\":\"\xff\",\"id\":1}",
            ),
            reject(
                "framing-jsonrpc/found-reject-response-invalid-utf8-in-error-message",
                Op::Response,
                framing(FRAMING_JSONRPC),
                b"{\"jsonrpc\":\"2.0\",\"error\":{\"code\":1,\"message\":\"\xff\"},\"id\":1}",
            ),
        ],
        _ => vec![],
    }
}

/// The JSON a vectors, features, support or report file holds, as written.
pub fn render<T: Serialize>(value: &T) -> String {
    let mut out = serde_json::to_string_pretty(value).expect("conformance files serialize");
    out.push('\n');
    out
}

#[cfg(test)]
mod json_equal_tests {
    use super::json_equal;

    fn same(a: &str, b: &str) -> bool {
        let forward = json_equal(a.as_bytes(), b.as_bytes());
        assert_eq!(forward, json_equal(b.as_bytes(), a.as_bytes()), "{a} / {b}");
        forward
    }

    #[test]
    fn numbers_compare_as_exact_decimals() {
        for (a, b) in [
            ("1", "1.0"),
            ("1", "10e-1"),
            ("1", "0.1e1"),
            ("1", "1E0"),
            ("100", "1e2"),
            ("0", "-0"),
            ("0", "0.000"),
            ("-1.50", "-15e-1"),
            ("9007199254740993", "9007199254740993.0"),
            ("[1, {\"a\": 2.0}]", "[1.0,{\"a\":2}]"),
        ] {
            assert!(same(a, b), "{a} should equal {b}");
        }
    }

    #[test]
    fn different_numbers_differ() {
        for (a, b) in [
            ("1", "2"),
            ("1", "-1"),
            ("1", "1.0001"),
            ("1", "10"),
            ("9007199254740992", "9007199254740993"),
            ("0.1", "0.10000000000000001"),
            ("1e400", "1e401"),
        ] {
            assert!(!same(a, b), "{a} should differ from {b}");
        }
    }

    #[test]
    fn structure_and_strings() {
        assert!(same(
            r#"{"a":1,"b":[true,null]}"#,
            r#" { "b" : [ true , null ] , "a" : 1.0 } "#
        ));
        assert!(same(r#""app.Chat\/count""#, r#""app.Chat/count""#));
        assert!(same(r#""é""#, "\"é\""));
        assert!(!same(r#"{"a":1}"#, r#"{"a":1,"b":1}"#));
        assert!(!same("[1,2]", "[2,1]"));
        assert!(!same("true", "\"true\""));
        assert!(!same("1", "\"1\""));
    }

    #[test]
    fn invalid_json_equals_nothing() {
        assert!(!json_equal(b"1.", b"1.0"));
        assert!(!json_equal(b"{", b"{"));
        assert!(!json_equal(b"", b""));
    }
}
