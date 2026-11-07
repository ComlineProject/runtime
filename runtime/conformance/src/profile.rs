// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An encoding's profile (`profiles/<name>.json`): facts about how it
//! encodes, which the version comparison turns into bumps. Each fact names
//! the vector cases that prove it.

use serde::{Deserialize, Serialize};

use comline_runtime::contract::{
    HandshakeFrame, FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_DATAGRAM3, FRAMING_JSONRPC,
};

use crate::comline2;
use crate::engine::encode_with;
use crate::{Case, Expect, Framed, Input, Op, Reply, RequestIn, ResponseIn, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
    #[serde(flatten)]
    pub facts: Facts,
}

impl Profile {
    pub fn kind(&self) -> ProfileKind {
        match self.facts {
            Facts::Format(_) => ProfileKind::Format,
            Facts::Framing(_) => ProfileKind::Framing,
            Facts::Handshake(_) => ProfileKind::Handshake,
        }
    }
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

/// What a profile of each kind states. The set is fixed: a new fact is a
/// change to the comparison too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "facts", rename_all = "snake_case")]
pub enum Facts {
    Format(FormatFacts),
    Framing(FramingFacts),
    Handshake(HandshakeFacts),
}

impl Facts {
    /// Every fact stated, by name, with the cases that prove it.
    pub fn stated(&self) -> Vec<(Fact, &[String])> {
        fn of<T: Copy>(fact: fn(T) -> Fact, stated: &Stated<T>) -> (Fact, &[String]) {
            (fact(stated.value), &stated.proven_by)
        }
        match self {
            Facts::Format(f) => vec![
                of(Fact::EnumVariants, &f.enum_variants),
                of(Fact::IntegerWidth, &f.integer_width),
                of(Fact::MissingOptional, &f.missing_optional),
                of(Fact::Output, &f.output),
                of(Fact::StructFields, &f.struct_fields),
                of(Fact::UnknownField, &f.unknown_field),
                of(Fact::UnknownVariant, &f.unknown_variant),
            ],
            Facts::Framing(f) => vec![
                of(Fact::Calls, &f.calls),
                of(Fact::Errors, &f.errors),
                of(Fact::Payload, &f.payload),
            ],
            Facts::Handshake(f) => [
                f.encoding_agreement
                    .as_ref()
                    .map(|s| of(Fact::EncodingAgreement, s)),
                f.schema_agreement
                    .as_ref()
                    .map(|s| of(Fact::SchemaAgreement, s)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        }
    }
}

/// The profile of an official encoding, as shipped in `profiles/`.
fn official(name: &str) -> Option<Profile> {
    let text = match name {
        FORMAT_JSON => include_str!("../profiles/json@1.json"),
        FORMAT_MSGPACK => include_str!("../profiles/msgpack@1.json"),
        FORMAT_PACKED => include_str!("../profiles/packed@1.json"),
        FRAMING_DATAGRAM => include_str!("../profiles/datagram@1.json"),
        FRAMING_DATAGRAM2 => include_str!("../profiles/datagram@2.json"),
        FRAMING_DATAGRAM3 => include_str!("../profiles/datagram@3.json"),
        FRAMING_JSONRPC => include_str!("../profiles/jsonrpc@1.json"),
        _ => return None,
    };
    Some(serde_json::from_str(text).expect("an official profile parses"))
}

/// What the official framing `name` requires of a payload.
pub(crate) fn official_payload(name: &str) -> Option<Payload> {
    match official(name)?.facts {
        Facts::Framing(f) => Some(f.payload.value),
        _ => None,
    }
}

/// What the official format `name` writes.
pub(crate) fn official_output(name: &str) -> Option<Output> {
    match official(name)?.facts {
        Facts::Format(f) => Some(f.output.value),
        _ => None,
    }
}

/// A format states every fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatFacts {
    pub struct_fields: Stated<StructFields>,
    pub unknown_field: Stated<UnknownField>,
    pub missing_optional: Stated<MissingOptional>,
    pub enum_variants: Stated<EnumVariants>,
    pub unknown_variant: Stated<UnknownVariant>,
    pub integer_width: Stated<IntegerWidth>,
    pub output: Stated<Output>,
}

/// A framing states every fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramingFacts {
    pub calls: Stated<Calls>,
    pub errors: Stated<Errors>,
    pub payload: Stated<Payload>,
}

/// A handshake may leave a fact out, which reads as `false`: not established.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_agreement: Option<Stated<bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_agreement: Option<Stated<bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stated<T> {
    pub value: T,
    /// The cases that prove it. A handshake fact stated `false` claims
    /// nothing, so it names none.
    #[serde(default)]
    pub proven_by: Vec<String>,
}

macro_rules! fact_values {
    ($($name:ident { $($variant:ident = $text:literal),+ $(,)? })+) => {$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }
    )+};
}

fact_values! {
    StructFields { ByPosition = "by_position", ByName = "by_name" }
    UnknownField { Rejected = "rejected", Ignored = "ignored" }
    MissingOptional { Rejected = "rejected", Absent = "absent" }
    EnumVariants { ByName = "by_name", ByIndex = "by_index" }
    UnknownVariant { Rejected = "rejected", Tolerated = "tolerated" }
    IntegerWidth { ByValue = "by_value", Exact = "exact" }
    Calls { ByIndex = "by_index", ByName = "by_name", ById = "by_id" }
    Errors { ByOrdinal = "by_ordinal", ByName = "by_name" }
    Output { Bytes = "bytes", Json = "json" }
    Payload { Opaque = "opaque", Json = "json" }
}

impl Payload {
    /// Whether this framing can carry what a format with `output` writes: an
    /// opaque one carries anything, and one that embeds a grammar carries
    /// that grammar. This is the whole rule, so it names no encoding.
    pub fn carries(self, output: Output) -> bool {
        match self {
            Payload::Opaque => true,
            Payload::Json => output == Output::Json,
        }
    }

    /// Whether this framing can carry `params`, a request's encoded params.
    /// JSON-RPC 2.0 has them be an array or an object when given at all;
    /// `null`, like nothing at all, is none, which `jsonrpc@1` leaves out.
    pub fn carries_params(self, params: &[u8]) -> bool {
        match self {
            Payload::Opaque => true,
            Payload::Json => {
                let structured = matches!(params.first(), Some(b'[' | b'{')) || params == b"null";
                params.is_empty() || (structured && is_json(params))
            }
        }
    }
}

fn is_json(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde::de::IgnoredAny>(bytes).is_ok()
}

/// One stated fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact {
    StructFields(StructFields),
    UnknownField(UnknownField),
    MissingOptional(MissingOptional),
    EnumVariants(EnumVariants),
    UnknownVariant(UnknownVariant),
    IntegerWidth(IntegerWidth),
    Calls(Calls),
    Errors(Errors),
    Output(Output),
    Payload(Payload),
    SchemaAgreement(bool),
    EncodingAgreement(bool),
}

impl Fact {
    pub fn name(self) -> &'static str {
        match self {
            Fact::StructFields(_) => "struct_fields",
            Fact::UnknownField(_) => "unknown_field",
            Fact::MissingOptional(_) => "missing_optional",
            Fact::EnumVariants(_) => "enum_variants",
            Fact::UnknownVariant(_) => "unknown_variant",
            Fact::IntegerWidth(_) => "integer_width",
            Fact::Calls(_) => "calls",
            Fact::Errors(_) => "errors",
            Fact::Output(_) => "output",
            Fact::Payload(_) => "payload",
            Fact::SchemaAgreement(_) => "schema_agreement",
            Fact::EncodingAgreement(_) => "encoding_agreement",
        }
    }

    /// The value as the file spells it.
    pub fn value(self) -> &'static str {
        match self {
            Fact::StructFields(v) => v.as_str(),
            Fact::UnknownField(v) => v.as_str(),
            Fact::MissingOptional(v) => v.as_str(),
            Fact::EnumVariants(v) => v.as_str(),
            Fact::UnknownVariant(v) => v.as_str(),
            Fact::IntegerWidth(v) => v.as_str(),
            Fact::Calls(v) => v.as_str(),
            Fact::Errors(v) => v.as_str(),
            Fact::Output(v) => v.as_str(),
            Fact::Payload(v) => v.as_str(),
            Fact::SchemaAgreement(true) | Fact::EncodingAgreement(true) => "true",
            Fact::SchemaAgreement(false) | Fact::EncodingAgreement(false) => "false",
        }
    }
}

/// Whether `cases` (a fact's `proven_by`, in order) show `fact`, beyond
/// passing: a rejection where the fact says rejected, the decoded value
/// where it says by name or by position.
pub fn fact_shown(fact: Fact, cases: &[&Case]) -> Result<(), String> {
    let shown = match fact {
        Fact::SchemaAgreement(false) | Fact::EncodingAgreement(false) => true,
        Fact::SchemaAgreement(true) => agreement_shown(true, cases),
        Fact::EncodingAgreement(true) => agreement_shown(false, cases),
        _ => {
            let first = cases.first().ok_or("no proving case")?;
            decoding_fact_shown(fact, first, cases)
        }
    };
    if shown {
        Ok(())
    } else {
        Err(format!(
            "its cases don't show `{}: {}`",
            fact.name(),
            fact.value()
        ))
    }
}

fn decoding_fact_shown(fact: Fact, first: &Case, cases: &[&Case]) -> bool {
    let decoded = match &first.input {
        Input::Decode(i) => i.value.as_ref(),
        _ => None,
    };
    let rejected = first.expect == Expect::Reject;
    let point = |x: i32, y: i32| Value::Struct {
        name: "Point".into(),
        fields: vec![("x".into(), Value::I32(x)), ("y".into(), Value::I32(y))],
    };
    let variant =
        |name: &str| matches!(decoded, Some(Value::UnitVariant { variant, .. }) if variant == name);
    match fact {
        Fact::StructFields(StructFields::ByName) => decoded == Some(&point(1, -2)),
        Fact::StructFields(StructFields::ByPosition) => decoded == Some(&point(-2, 1)),
        Fact::EnumVariants(EnumVariants::ByName) => variant("Green"),
        Fact::EnumVariants(EnumVariants::ByIndex) => variant("Red"),
        Fact::UnknownField(UnknownField::Rejected)
        | Fact::MissingOptional(MissingOptional::Rejected)
        | Fact::UnknownVariant(UnknownVariant::Rejected) => rejected,
        Fact::UnknownField(UnknownField::Ignored)
        | Fact::MissingOptional(MissingOptional::Absent)
        | Fact::UnknownVariant(UnknownVariant::Tolerated) => !rejected,
        Fact::IntegerWidth(IntegerWidth::ByValue) => {
            cases.len() == 2 && !rejected && cases[1].expect == Expect::Reject
        }
        Fact::IntegerWidth(IntegerWidth::Exact) => {
            cases.len() == 2 && cases.iter().all(|c| c.expect == Expect::Reject)
        }
        Fact::Calls(calls) => calls_shown(calls, cases),
        Fact::Errors(_) => cases.iter().any(|c| carries_error(c)),
        Fact::Output(output) => output_shown(output, cases),
        Fact::Payload(Payload::Opaque) => cases.iter().any(|c| carries_non_json(c)),
        Fact::Payload(Payload::Json) => embedding_shown(cases),
        Fact::SchemaAgreement(_) | Fact::EncodingAgreement(_) => {
            unreachable!("handshake facts aren't shown by decoding")
        }
    }
}

/// `calls` is shown by the frames: each is an exact request, whose bytes hold
/// its method's name for `by_name` and don't otherwise. Whether a number is an
/// index or a wire number (`by_index`, `by_id`) isn't in the bytes, so the
/// profile's word stands for that.
fn calls_shown(calls: Calls, cases: &[&Case]) -> bool {
    let holds_name = |bytes: &[u8], name: &[u8]| {
        !name.is_empty() && bytes.windows(name.len()).any(|window| window == name)
    };
    cases.iter().all(|case| match &case.input {
        Input::Request(Framed::Full(i)) if case.expect == Expect::Exact => {
            holds_name(&case.bytes.0, i.method.as_bytes()) == (calls == Calls::ByName)
        }
        _ => false,
    })
}

/// A case that shows how a framing carries an error: an exact error response.
/// A response's input holds only the error's ordinal, so whether the frame has
/// that or a name (`by_ordinal`, `by_name`) rests on the case passing.
fn carries_error(case: &Case) -> bool {
    case.expect == Expect::Exact
        && matches!(
            &case.input,
            Input::Response(Framed::Full(ResponseIn {
                reply: Reply::Err { .. },
                ..
            })) | Input::Envelope(Some(Reply::Err { .. }))
        )
}

/// `output` is shown by exact `encode` cases: each one's bytes are JSON for
/// `json`, and for `bytes`, at least one's aren't. No list of cases shows that
/// a format only ever writes JSON, which is `tests/payload.rs`'s to check.
fn output_shown(output: Output, cases: &[&Case]) -> bool {
    let encodes = cases
        .iter()
        .all(|c| c.expect == Expect::Exact && c.op() == Op::Encode);
    encodes
        && match output {
            Output::Json => cases.iter().all(|c| is_json(&c.bytes.0)),
            Output::Bytes => cases.iter().any(|c| !is_json(&c.bytes.0)),
        }
}

/// A request's params as its format writes them, for a format this ships.
fn params_of(i: &RequestIn) -> Option<Vec<u8>> {
    encode_with(&i.format, &i.params)
}

/// An exact request whose params aren't JSON: passing it shows the framing
/// carries them.
fn carries_non_json(case: &Case) -> bool {
    match &case.input {
        Input::Request(Framed::Full(i)) if case.expect == Expect::Exact => {
            params_of(i).is_some_and(|params| !is_json(&params))
        }
        _ => false,
    }
}

/// `payload: json` is shown by a pair: an exact request with JSON params, and
/// a refusal of that same frame with something that isn't JSON in their place.
/// A framing that carries its payload opaquely would read that frame, so the
/// refusal can only come from what the payload is.
fn embedding_shown(cases: &[&Case]) -> bool {
    let frames: Vec<(&[u8], Vec<u8>)> = cases
        .iter()
        .filter_map(|c| match &c.input {
            Input::Request(Framed::Full(i)) if c.expect == Expect::Exact => {
                Some((&c.bytes.0[..], params_of(i)?))
            }
            _ => None,
        })
        .collect();
    cases.iter().any(|c| {
        c.expect == Expect::Reject
            && c.op() == Op::Request
            && frames.iter().any(|(frame, params)| {
                let Some(at) = frame.windows(params.len()).position(|w| w == params) else {
                    return false;
                };
                let (head, tail) = (&frame[..at], &frame[at + params.len()..]);
                let bytes = &c.bytes.0[..];
                bytes.len() >= head.len() + tail.len()
                    && bytes.starts_with(head)
                    && bytes.ends_with(tail)
                    && !is_json(&bytes[head.len()..bytes.len() - tail.len()])
            })
    })
}

/// A handshake fact stated `true` (`schema`: `schema_agreement`) is shown by
/// `handshake_check` cases: one where the peer agrees, accepted, and, for
/// each thing the fact covers, one where the peer's frame differs from the
/// input in only that (the schema fingerprint; the format; the framing),
/// refused.
fn agreement_shown(schema: bool, cases: &[&Case]) -> bool {
    if cases.iter().any(|c| c.op() == Op::Comline2Negotiate) {
        return comline2::agreement_shown(schema, cases);
    }
    // (ir_hash, format, framing) differ, of the peer's frame from the input.
    let differs = |case: &Case| -> Option<(bool, bool, bool)> {
        let Input::HandshakeCheck(i) = &case.input else {
            return None;
        };
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
    let refused = if schema {
        shows((true, false, false), Expect::Reject)
    } else {
        shows((false, true, false), Expect::Reject) && shows((false, false, true), Expect::Reject)
    };
    agrees && refused
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(kind: &str, facts: serde_json::Value) -> Result<Profile, serde_json::Error> {
        serde_json::from_value(json!({ "name": "x@1", "kind": kind, "facts": facts }))
    }

    #[test]
    fn a_kind_states_its_facts_with_their_values() {
        let by_id = json!({ "value": "by_id", "proven_by": ["a/b"] });
        let by_name = json!({ "value": "by_name", "proven_by": ["a/c"] });
        let opaque = json!({ "value": "opaque", "proven_by": ["a/d"] });
        let read = profile(
            "framing",
            json!({ "calls": by_id, "errors": by_name, "payload": opaque }),
        )
        .unwrap();
        assert_eq!(read.kind(), ProfileKind::Framing);
        let stated = read.facts.stated();
        assert_eq!(stated[0].0, Fact::Calls(Calls::ById));
        assert_eq!(
            (stated[1].0.name(), stated[1].0.value()),
            ("errors", "by_name")
        );
        assert_eq!(stated[2].0, Fact::Payload(Payload::Opaque));

        // A fact missing, an unknown fact, an unknown value.
        assert!(profile("framing", json!({ "calls": by_id, "errors": by_name })).is_err());
        let more = json!({ "calls": by_id, "errors": by_name, "payload": opaque, "more": by_name });
        assert!(profile("framing", more).is_err());
        let wrong = json!({ "calls": { "value": "by_ordinal" }, "errors": by_name, "payload": opaque });
        assert!(profile("framing", wrong).is_err());
    }

    #[test]
    fn a_handshake_fact_is_a_boolean_it_may_leave_out() {
        let read = profile("handshake", json!({})).unwrap();
        assert!(read.facts.stated().is_empty());
        assert!(profile(
            "handshake",
            json!({ "schema_agreement": { "value": "true" } })
        )
        .is_err());
    }

    #[test]
    fn calls_and_errors_are_shown_by_the_frames() {
        let case = |input, bytes: &[u8], expect| Case {
            id: "a/b".into(),
            input,
            bytes: crate::Hex(bytes.to_vec()),
            expect,
            compare: crate::Compare::Bytes,
        };
        let request = |method: &str| {
            Input::parse(
                Op::Request,
                &json!({
                    "framing": "f", "call_id": 1, "method": method, "request_id": "1",
                    "format": "json@1", "params": { "unit": null },
                }),
            )
            .unwrap()
        };
        let named = case(request("greet"), b"REQ greet", Expect::Exact);
        let numbered = case(request("greet"), &[1, 0, 1], Expect::Exact);
        let shown = |calls, case: &Case| fact_shown(Fact::Calls(calls), &[case]).is_ok();
        assert!(shown(Calls::ByName, &named));
        assert!(!shown(Calls::ById, &named));
        assert!(!shown(Calls::ByIndex, &named));
        assert!(shown(Calls::ById, &numbered));
        assert!(shown(Calls::ByIndex, &numbered));
        assert!(!shown(Calls::ByName, &numbered));
        // A rejection, or a selector, says nothing about calls.
        let refused = case(request("greet"), b"REQ greet", Expect::Reject);
        assert!(!shown(Calls::ByName, &refused));
        let selector = Input::parse(Op::Request, &json!({ "framing": "f" })).unwrap();
        assert!(!shown(Calls::ById, &case(selector, &[1], Expect::Reject)));
        assert!(fact_shown(Fact::Calls(Calls::ById), &[]).is_err());

        let reply = |kind: serde_json::Value| Input::parse(Op::Envelope, &kind).unwrap();
        let err = reply(json!({ "kind": "err", "id": 1, "body": "" }));
        let errors = Fact::Errors(Errors::ByOrdinal);
        assert!(fact_shown(errors, &[&case(err, &[1], Expect::Exact)]).is_ok());
        let fault = reply(json!({ "kind": "fault" }));
        assert!(fact_shown(errors, &[&case(fault, &[2], Expect::Exact)]).is_err());
    }

    #[test]
    fn a_profile_writes_back_as_it_was_read() {
        let file = json!({
            "$schema": "s", "name": "x@1", "kind": "handshake",
            "facts": { "schema_agreement": { "value": false, "proven_by": [] } }
        });
        let read: Profile = serde_json::from_value(file.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), file);
    }
}
