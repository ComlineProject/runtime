// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An encoding's profile (`profiles/<name>.json`): facts about how it
//! encodes, which the version comparison turns into bumps. Each fact names
//! the vector cases that prove it.

use serde::{Deserialize, Serialize};

use comline_runtime::contract::HandshakeFrame;

use crate::comline2;
use crate::{Case, Expect, Input, Op, Value};

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
                of(Fact::StructFields, &f.struct_fields),
                of(Fact::UnknownField, &f.unknown_field),
                of(Fact::UnknownVariant, &f.unknown_variant),
            ],
            Facts::Framing(f) => vec![of(Fact::Calls, &f.calls), of(Fact::Errors, &f.errors)],
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
}

/// A framing states every fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramingFacts {
    pub calls: Stated<Calls>,
    pub errors: Stated<Errors>,
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
        // Shown by the cases passing: the framing carries the call and the
        // error that way, and decoding reads them back.
        Fact::Calls(_) | Fact::Errors(_) => true,
        Fact::SchemaAgreement(_) | Fact::EncodingAgreement(_) => {
            unreachable!("handshake facts aren't shown by decoding")
        }
    }
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
        let read = profile("framing", json!({ "calls": by_id, "errors": by_name })).unwrap();
        assert_eq!(read.kind(), ProfileKind::Framing);
        let stated = read.facts.stated();
        assert_eq!(stated[0].0, Fact::Calls(Calls::ById));
        assert_eq!(
            (stated[1].0.name(), stated[1].0.value()),
            ("errors", "by_name")
        );

        // A fact missing, an unknown fact, an unknown value.
        assert!(profile("framing", json!({ "calls": by_id })).is_err());
        let more = json!({ "calls": by_id, "errors": by_name, "more": by_name });
        assert!(profile("framing", more).is_err());
        let wrong = json!({ "calls": { "value": "by_ordinal" }, "errors": by_name });
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
    fn a_profile_writes_back_as_it_was_read() {
        let file = json!({
            "$schema": "s", "name": "x@1", "kind": "handshake",
            "facts": { "schema_agreement": { "value": false, "proven_by": [] } }
        });
        let read: Profile = serde_json::from_value(file.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), file);
    }
}
