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

use serde::de::Error as _;
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use comline_runtime::contract::{
    name_hash, Call, DatagramFraming, Envelope, Framing, Handshake, RequestCall, WireFormat,
    FORMAT_JSON, FORMAT_MSGPACK, FRAMING_DATAGRAM, FRAMING_JSONRPC,
};
use comline_runtime::format::{Json, MsgPack};
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
    F64(f64),
    String(String),
    Array(Vec<Value>),
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
            Value::F64(v) => s.serialize_f64(*v),
            Value::String(v) => s.serialize_str(v),
            Value::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&Wire(item))?;
                }
                seq.end()
            }
            Value::Option(None) => s.serialize_none(),
            Value::Option(Some(v)) => s.serialize_some(&Wire(v)),
            Value::Struct { name, fields } => {
                // Generated types have `'static` names; leaking the few a
                // test run sees keeps the impl honest to them.
                let mut st = s.serialize_struct(leak(name), fields.len())?;
                for (field, value) in fields {
                    st.serialize_field(leak(field), &Wire(value))?;
                }
                st.end()
            }
            Value::UnitVariant {
                enum_name,
                index,
                variant,
            } => s.serialize_unit_variant(leak(enum_name), *index, leak(variant)),
        }
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
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
    /// A [`Reply`].
    Envelope,
    /// `{framing, call_id, method, request_id, format, params}`. Decoding
    /// yields the call (the id for datagram framing, the method for
    /// JSON-RPC), the request id, and `params` encoded in `format`.
    Request,
    /// `{framing, request_id}` and a [`Reply`].
    Response,
    /// `{format, value}` → the encoded value. Encode only.
    Encode,
    /// `{format, type, value}`: decoding `bytes` as `type` gives `value`.
    /// Decode only: the bytes were written by a different type (a field
    /// more, a variant reordered), which is what a profile fact is about.
    /// A rejection's input is `{format, type}`.
    Decode,
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
    /// reference, reproduces the bytes exactly.
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
    call_id: u16,
    method: String,
    #[serde(with = "decimal")]
    request_id: u64,
    format: String,
    params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ResponseIn {
    framing: String,
    #[serde(with = "decimal")]
    request_id: u64,
    #[serde(flatten)]
    reply: Reply,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct EncodeIn {
    format: String,
    value: Value,
}

/// A type a `decode` case reads into, described so any runtime can build a
/// reader for it: a struct's fields in order, an enum's variants in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    U32,
    I32,
    String,
    Option(Box<Type>),
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub value: String,
    pub proven_by: Vec<String>,
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
        }
    }
}

/// Whether `cases` (a fact's `proven_by`, in order) show `value` for
/// `fact`, beyond passing: a rejection where the fact says rejected, the
/// decoded value where it says by name or by position.
pub fn fact_shown(fact: &str, value: &str, cases: &[&Case]) -> Result<(), String> {
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
            Handshake::new(i.ir_hash, &i.wire_format, &i.framing, i.capabilities).encode(&mut out);
        }
        Op::Envelope => encode_reply(&parse(input)?, &mut out),
        Op::Request => {
            let i: RequestIn = parse(input)?;
            let framing = framing_of(&i.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", i.framing)))?;
            let call = Call::new(i.call_id, leak(&i.method));
            let encoded = match i.format.as_str() {
                FORMAT_JSON => with_framing!(framing, |f| f.encode_request(
                    call,
                    i.request_id,
                    &Wire(&i.params),
                    &Json,
                    &mut out
                )),
                FORMAT_MSGPACK => with_framing!(framing, |f| f.encode_request(
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
        Op::Decode => {
            let i: DecodeIn = parse(input)?;
            Ok(decode_as(&i.format, &i.ty, bytes)?.map(|value| {
                to_json(&DecodeIn {
                    format: i.format,
                    ty: i.ty,
                    value: Some(value),
                })
            }))
        }
        Op::Handshake => {
            let Some(h) = Handshake::decode(bytes) else {
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
        Op::Envelope => match Envelope::decode(bytes) {
            Some(e) => json(to_json(&reply_of(e))),
            None => Ok(None),
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
            let Some(req) = with_framing!(framing, |f| f.decode_request(bytes)) else {
                return Ok(None);
            };
            let (call_id, method) = match req.call {
                RequestCall::Id(id) => (id, i.method.clone()),
                RequestCall::Name(name) => (i.call_id, name.to_owned()),
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
            json(serde_json::json!({
                "framing": i.framing,
                "call_id": call_id,
                "method": method,
                "request_id": req.request_id.to_string(),
                "format": i.format,
                "params": params,
            }))
        }
        Op::Response => {
            let f: FramingIn = parse(input)?;
            let framing = framing_of(&f.framing)
                .ok_or_else(|| Outcome::NotImplemented(format!("framing `{}`", f.framing)))?;
            match with_framing!(framing, |x| x.decode_response(bytes)) {
                Some((request_id, envelope)) => json(to_json(&ResponseIn {
                    framing: f.framing,
                    request_id,
                    reply: reply_of(envelope),
                })),
                None => Ok(None),
            }
        }
    }
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
        return match decode(case.op, &case.input, bytes)? {
            None => Ok(()),
            Some(got) => Err(Outcome::Fail(format!(
                "accepted bytes it must reject, as {got}"
            ))),
        };
    }

    if case.op != Op::Decode {
        let encoded = encode(case.op, &case.input)?;
        if &encoded != bytes {
            return Err(Outcome::Fail(format!(
                "encodes to {}, expected {}",
                to_json(&Hex(encoded)),
                to_json(&case.bytes),
            )));
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
            FRAMING_DATAGRAM,
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
        feature(
            feature_id,
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
        );
    }

    feature("format-json", format_cases("format-json", FORMAT_JSON));
    feature(
        "format-msgpack",
        format_cases("format-msgpack", FORMAT_MSGPACK),
    );

    for v in &mut vectors {
        v.cases.extend(rejections(&v.feature));
        match v.feature.as_str() {
            "format-json" => v.cases.extend(decode_cases("format-json", FORMAT_JSON)),
            "format-msgpack" => v
                .cases
                .extend(decode_cases("format-msgpack", FORMAT_MSGPACK)),
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
        call_id,
        method: method.into(),
        request_id,
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

// --- Decoding what another version wrote ---

/// The types `decode` cases read into, as generated code declares them:
/// plain `serde` derives, nothing added. Decoding through them is what makes
/// a case prove how generated code behaves, not how a hand-written decoder
/// would.
mod readers {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    #[derive(Deserialize)]
    pub struct Tagged {
        pub id: u32,
        pub label: Option<String>,
    }

    #[derive(Deserialize)]
    pub enum Color {
        Red,
        Green,
        Blue,
    }
}

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

fn decode_with<T: for<'de> Deserialize<'de>>(
    format: &str,
    bytes: &[u8],
) -> Result<Option<T>, Outcome> {
    match format {
        FORMAT_JSON => Ok(Json.decode(bytes).ok()),
        FORMAT_MSGPACK => Ok(MsgPack.decode(bytes).ok()),
        other => Err(Outcome::NotImplemented(format!("format `{other}`"))),
    }
}

/// Decodes `bytes` as `ty` through the reader generated code would have;
/// `Ok(None)` when the format refuses them.
fn decode_as(format: &str, ty: &Type, bytes: &[u8]) -> Result<Option<Value>, Outcome> {
    let point = |p: readers::Point| Value::Struct {
        name: "Point".into(),
        fields: vec![("x".into(), Value::I32(p.x)), ("y".into(), Value::I32(p.y))],
    };
    let tagged = |t: readers::Tagged| Value::Struct {
        name: "Tagged".into(),
        fields: vec![
            ("id".into(), Value::U32(t.id)),
            (
                "label".into(),
                Value::Option(t.label.map(|l| Box::new(Value::String(l)))),
            ),
        ],
    };
    let color = |c: readers::Color| {
        let (index, variant) = match c {
            readers::Color::Red => (0, "Red"),
            readers::Color::Green => (1, "Green"),
            readers::Color::Blue => (2, "Blue"),
        };
        Value::UnitVariant {
            enum_name: "Color".into(),
            index,
            variant: variant.into(),
        }
    };
    Ok(if *ty == Type::U32 {
        decode_with::<u32>(format, bytes)?.map(Value::U32)
    } else if *ty == point_type() {
        decode_with(format, bytes)?.map(point)
    } else if *ty == tagged_type() {
        decode_with(format, bytes)?.map(tagged)
    } else if *ty == color_type() {
        decode_with(format, bytes)?.map(color)
    } else {
        return Err(Outcome::NotImplemented(format!(
            "a reader for {}",
            to_json(ty)
        )));
    })
}

/// Bytes `written` encodes to, read back as `ty`: an exact case with the
/// value the runtime decoded, or a rejection if it refused them.
fn decode_case(id: String, format: &str, written: &Value, ty: Type) -> Case {
    let bytes = encode_with(format, written).expect("reference formats encode");
    match decode_as(format, &ty, &bytes) {
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
    ]
}

/// Bytes a runtime must refuse, per feature.
fn rejections(feature: &str) -> Vec<Case> {
    let none = serde_json::Value::Null;
    let mut handshake = Vec::new();
    Handshake::new(1, FORMAT_JSON, FRAMING_DATAGRAM, 0).encode(&mut handshake);
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
