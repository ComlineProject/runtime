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

mod case;
mod comline2;
mod engine;
mod intern;
mod json_equal;
mod profile;
mod reference;
pub mod runner;
mod seed;

pub use case::{
    hex, Case, Compare, DecodeIn, EncodeIn, Expect, Framed, HandshakeIn, Hex, Input, NameHashIn, Op,
    Reply, RequestIn, ResponseIn, StreamPrefixIn, StreamSplit, StreamSplitIn,
};
pub use comline2::{NegotiateIn, NegotiateResult, Step};
pub use engine::{answer, decode_typed, encode_as, run, Answer};
pub(crate) use engine::to_json;
pub use json_equal::json_equal;
pub use profile::{
    fact_shown, Calls, EnumVariants, Errors, Fact, Facts, FormatFacts, FramingFacts,
    HandshakeFacts, IntegerWidth, MissingOptional, Profile, ProfileKind, Stated, StructFields,
    UnknownField, UnknownVariant,
};
pub use reference::reference_cases;
pub use seed::{check, TypeSeed};

use serde::de::Error as _;
use serde::ser::{SerializeSeq, SerializeStruct, SerializeTuple};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
    /// A single `char`: one Unicode scalar value, `{"char": "é"}`.
    Char(char),
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
            Value::Char(v) => s.serialize_char(*v),
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

/// A 64- or 128-bit integer as a decimal string, so a JSON reader that holds
/// numbers as doubles keeps it exact.
pub mod decimal {
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
    /// A single `char`: one Unicode scalar value.
    Char,
    Unit,
    /// `char[max N]`: a string of at most `max` characters (Unicode scalar
    /// values, not bytes).
    BoundedString {
        max: u32,
    },
    Option(Box<Type>),
    /// `T[]`.
    Array(Box<Type>),
    /// `T[N]`.
    Fixed {
        of: Box<Type>,
        len: usize,
    },
    /// `T[max N]`: at most `max` items.
    BoundedArray {
        of: Box<Type>,
        max: u32,
    },
    /// A type that can contain itself, `@max_depth = max_depth`: `of`, in
    /// which `"self"` stands for the recursive type again. The outermost
    /// value is level 1; a value at level `max_depth + 1` is refused.
    Recursive {
        max_depth: u32,
        of: Box<Type>,
    },
    /// The nearest enclosing [`Recursive`](Type::Recursive) type.
    #[serde(rename = "self")]
    SelfRef,
    Struct {
        name: String,
        fields: Vec<(String, Type)>,
    },
    Enum {
        name: String,
        variants: Vec<String>,
    },
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
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub features: Vec<FeatureInfo>,
}

/// One `vectors/<feature>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vectors {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
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
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
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
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
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

/// The JSON a vectors, features, support or report file holds, as written.
pub fn render<T: Serialize>(value: &T) -> String {
    let mut out = serde_json::to_string_pretty(value).expect("conformance files serialize");
    out.push('\n');
    out
}

