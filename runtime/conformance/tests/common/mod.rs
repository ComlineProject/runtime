// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The fixed mirrors that `TypeSeed` is held to: types declared as the Rust
//! generator declares them (plain `serde` derives, `serde_big_array` past 32
//! elements), each with its [`Type`] description, sample values and inputs
//! written by hand.
//!
//! Shared by `tests/seed_vs_derive.rs` (a deterministic set of mutations),
//! `tests/properties.rs` (proptest's mutations) and the `seed_vs_derive`
//! fuzz target in `runtime/fuzz`, which includes this file by path.

#![allow(dead_code)]

use comline_runtime::contract::{WireFormat, FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED};
use comline_runtime::format::{Json, MsgPack, Packed};
use comline_runtime_conformance::{decode_typed, Type, Value, Wire};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Serialize, Deserialize)]
struct Tagged {
    id: u32,
    label: Option<String>,
}

#[derive(Serialize, Deserialize)]
enum Color {
    Red,
    Green,
    Blue,
}

#[derive(Serialize, Deserialize)]
struct Scalars {
    flag: bool,
    small: u8,
    medium: u32,
    large: u64,
    huge: u128,
    signed_small: i8,
    signed_large: i64,
    name: String,
}

#[derive(Serialize, Deserialize)]
struct Bag {
    required: String,
    maybe: Option<u32>,
    many: Vec<String>,
    maybe_many: Option<Vec<u32>>,
}

#[derive(Serialize, Deserialize)]
struct Sample {
    ratio: f32,
    precise: f64,
    rgba: [u8; 4],
    #[serde(with = "serde_big_array::BigArray")]
    digest: [u8; 40],
    rows: Vec<[u16; 3]>,
    maybe_rgba: Option<[u8; 4]>,
}

#[derive(Serialize, Deserialize)]
struct TooLong {
    limit: u32,
}

#[derive(Serialize, Deserialize)]
struct Denied {}

pub const FORMATS: [&str; 3] = [FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED];

pub fn encode(format: &str, value: &impl Serialize) -> Vec<u8> {
    let mut out = Vec::new();
    match format {
        FORMAT_JSON => Json.encode(value, &mut out),
        FORMAT_MSGPACK => MsgPack.encode(value, &mut out),
        FORMAT_PACKED => Packed.encode(value, &mut out),
        _ => unreachable!(),
    }
    .expect("samples encode");
    out
}

fn derive_decode<T: DeserializeOwned>(format: &str, bytes: &[u8]) -> Option<T> {
    match format {
        FORMAT_JSON => Json.decode(bytes).ok(),
        FORMAT_MSGPACK => MsgPack.decode(bytes).ok(),
        FORMAT_PACKED => Packed.decode(bytes).ok(),
        _ => unreachable!(),
    }
}

/// Decodes as the derive `T` would, then writes what it read as `packed@1`,
/// which is positional and deterministic, so two decodes compare as bytes.
fn derived<T: Serialize + DeserializeOwned>(format: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    derive_decode::<T>(format, bytes).map(|value| encode(FORMAT_PACKED, &value))
}

/// A derived type and its run-time description.
pub struct Mirror {
    pub name: &'static str,
    pub ty: Type,
    pub samples: Vec<Value>,
    /// Inputs written by hand, per format.
    pub extra: Vec<(&'static str, &'static [u8])>,
    derive: fn(&str, &[u8]) -> Option<Vec<u8>>,
}

impl Mirror {
    /// The samples' bytes in `format`.
    pub fn valid(&self, format: &str) -> Vec<Vec<u8>> {
        self.samples
            .iter()
            .map(|v| encode(format, &Wire(v)))
            .collect()
    }

    /// Decodes `bytes` through the derive and through the seed: both must
    /// accept or both refuse, and what they accept must be the same value.
    pub fn agrees_on(&self, format: &str, bytes: &[u8]) -> Result<(), String> {
        let derived = (self.derive)(format, bytes);
        let seeded = decode_typed(format, &self.ty, bytes)
            .expect("a known format")
            .map(|v| encode(FORMAT_PACKED, &Wire(&v)));
        let says = |accepts: bool| if accepts { "accepts" } else { "refuses" };
        match (&derived, &seeded) {
            (None, None) => Ok(()),
            (Some(d), Some(s)) if d == s => Ok(()),
            (Some(_), Some(_)) => Err(format!(
                "{format}: derive and seed read {bytes:02x?} differently ({})",
                self.name
            )),
            _ => Err(format!(
                "{format}: derive {} and seed {} {bytes:02x?} ({})",
                says(derived.is_some()),
                says(seeded.is_some()),
                self.name
            )),
        }
    }
}

pub fn structure(name: &str, fields: Vec<(&str, Type)>) -> Type {
    Type::Struct {
        name: name.into(),
        fields: fields.into_iter().map(|(f, t)| (f.into(), t)).collect(),
    }
}

pub fn value(name: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Struct {
        name: name.into(),
        fields: fields.into_iter().map(|(f, v)| (f.into(), v)).collect(),
    }
}

pub fn some(v: Value) -> Value {
    Value::Option(Some(Box::new(v)))
}

pub fn opt(t: Type) -> Type {
    Type::Option(Box::new(t))
}

pub fn fixed(of: Type, len: usize) -> Type {
    Type::Fixed {
        of: Box::new(of),
        len,
    }
}

/// Every mirror.
pub fn mirrors() -> Vec<Mirror> {
    vec![
        point(),
        tagged(),
        color(),
        scalars(),
        bag(),
        sample(),
        too_long(),
        denied(),
        unit(),
    ]
}

/// The mirror called `name`.
pub fn mirror(name: &str) -> Mirror {
    mirrors()
        .into_iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no mirror `{name}`"))
}

fn point() -> Mirror {
    Mirror {
        name: "Point",
        ty: structure("Point", vec![("x", Type::I32), ("y", Type::I32)]),
        samples: vec![value(
            "Point",
            vec![("x", Value::I32(1)), ("y", Value::I32(-2))],
        )],
        extra: vec![
            (FORMAT_JSON, br#"{"y":-2,"x":1}"#),
            (FORMAT_JSON, br#"{"x":1,"y":-2,"z":5}"#),
            (FORMAT_JSON, br#"{"x":1,"x":2,"y":3}"#),
            (FORMAT_JSON, br#"{"x":1}"#),
            (FORMAT_JSON, br#"[1,-2]"#),
            (FORMAT_JSON, br#"[1,-2,3]"#),
            (FORMAT_JSON, br#"[1]"#),
            // MessagePack maps, which another writer may send for a struct:
            // keyed by name, by index, with an unknown index or name, and
            // with a key repeated.
            (FORMAT_MSGPACK, &[0x82, 0xa1, b'x', 0x01, 0xa1, b'y', 0xfe]),
            (FORMAT_MSGPACK, &[0x82, 0x00, 0x01, 0x01, 0xfe]),
            (FORMAT_MSGPACK, &[0x83, 0x05, 0x07, 0x00, 0x01, 0x01, 0xfe]),
            (
                FORMAT_MSGPACK,
                &[0x83, 0xa1, b'z', 0x05, 0x00, 0x01, 0x01, 0xfe],
            ),
            (FORMAT_MSGPACK, &[0x83, 0x00, 0x01, 0x00, 0x02, 0x01, 0xfe]),
            (FORMAT_MSGPACK, &[0x81, 0x00, 0x01]),
        ],
        derive: derived::<Point>,
    }
}

fn tagged() -> Mirror {
    Mirror {
        name: "Tagged",
        ty: structure(
            "Tagged",
            vec![("id", Type::U32), ("label", opt(Type::String))],
        ),
        samples: vec![
            value(
                "Tagged",
                vec![
                    ("id", Value::U32(1)),
                    ("label", some(Value::String("é".into()))),
                ],
            ),
            value(
                "Tagged",
                vec![("id", Value::U32(1)), ("label", Value::Option(None))],
            ),
        ],
        extra: vec![
            (FORMAT_JSON, br#"{"id":1}"#),
            (FORMAT_JSON, br#"{"label":"x"}"#),
            (FORMAT_JSON, br#"{"id":1,"label":null,"label":"x"}"#),
            (FORMAT_JSON, br#"{"id":4294967296}"#),
            (FORMAT_JSON, br#"{"id":1.0}"#),
            (FORMAT_JSON, br#"{"id":-1}"#),
        ],
        derive: derived::<Tagged>,
    }
}

fn color() -> Mirror {
    let variant = |index: u32, name: &str| Value::UnitVariant {
        enum_name: "Color".into(),
        index,
        variant: name.into(),
    };
    Mirror {
        name: "Color",
        ty: Type::Enum {
            name: "Color".into(),
            variants: vec!["Red".into(), "Green".into(), "Blue".into()],
        },
        samples: vec![variant(0, "Red"), variant(2, "Blue")],
        extra: vec![
            (FORMAT_JSON, br#""Purple""#),
            (FORMAT_JSON, br#"1"#),
            (FORMAT_JSON, br#"{"Green":null}"#),
            (FORMAT_PACKED, &[3]),
            (FORMAT_PACKED, &[0x80, 0x01]),
        ],
        derive: derived::<Color>,
    }
}

fn scalars() -> Mirror {
    let sample = |huge: u128, large: u64| {
        value(
            "Scalars",
            vec![
                ("flag", Value::Bool(true)),
                ("small", Value::U8(255)),
                ("medium", Value::U32(70_000)),
                ("large", Value::U64(large)),
                ("huge", Value::U128(huge)),
                ("signed_small", Value::I8(-128)),
                ("signed_large", Value::I64(i64::MIN)),
                ("name", Value::String("ab\"c".into())),
            ],
        )
    };
    Mirror {
        name: "Scalars",
        ty: structure(
            "Scalars",
            vec![
                ("flag", Type::Bool),
                ("small", Type::U8),
                ("medium", Type::U32),
                ("large", Type::U64),
                ("huge", Type::U128),
                ("signed_small", Type::I8),
                ("signed_large", Type::I64),
                ("name", Type::String),
            ],
        ),
        samples: vec![sample(u128::MAX, u64::MAX), sample(7, 1 << 53)],
        extra: vec![],
        derive: derived::<Scalars>,
    }
}

fn bag() -> Mirror {
    let strings = |n: usize| Value::Array((0..n).map(|i| Value::String(i.to_string())).collect());
    Mirror {
        name: "Bag",
        ty: structure(
            "Bag",
            vec![
                ("required", Type::String),
                ("maybe", opt(Type::U32)),
                ("many", Type::Array(Box::new(Type::String))),
                ("maybe_many", opt(Type::Array(Box::new(Type::U32)))),
            ],
        ),
        samples: vec![
            value(
                "Bag",
                vec![
                    ("required", Value::String("r".into())),
                    ("maybe", some(Value::U32(5))),
                    ("many", strings(3)),
                    ("maybe_many", some(Value::Array(vec![Value::U32(1)]))),
                ],
            ),
            value(
                "Bag",
                vec![
                    ("required", Value::String(String::new())),
                    ("maybe", Value::Option(None)),
                    ("many", strings(0)),
                    ("maybe_many", Value::Option(None)),
                ],
            ),
        ],
        extra: vec![(FORMAT_JSON, br#"{"required":"r","many":[]}"#)],
        derive: derived::<Bag>,
    }
}

fn sample() -> Mirror {
    let bytes = |n: u8| Value::Tuple((0..n).map(Value::U8).collect());
    Mirror {
        name: "Sample",
        ty: structure(
            "Sample",
            vec![
                ("ratio", Type::F32),
                ("precise", Type::F64),
                ("rgba", fixed(Type::U8, 4)),
                ("digest", fixed(Type::U8, 40)),
                ("rows", Type::Array(Box::new(fixed(Type::U16, 3)))),
                ("maybe_rgba", opt(fixed(Type::U8, 4))),
            ],
        ),
        samples: vec![value(
            "Sample",
            vec![
                ("ratio", Value::F32(0.5)),
                ("precise", Value::F64(-1.25)),
                ("rgba", bytes(4)),
                ("digest", bytes(40)),
                (
                    "rows",
                    Value::Array(vec![Value::Tuple(vec![
                        Value::U16(1),
                        Value::U16(2),
                        Value::U16(65535),
                    ])]),
                ),
                ("maybe_rgba", some(bytes(4))),
            ],
        )],
        extra: vec![],
        derive: derived::<Sample>,
    }
}

fn too_long() -> Mirror {
    Mirror {
        name: "TooLong",
        ty: structure("TooLong", vec![("limit", Type::U32)]),
        samples: vec![value("TooLong", vec![("limit", Value::U32(9))])],
        extra: vec![],
        derive: derived::<TooLong>,
    }
}

fn denied() -> Mirror {
    Mirror {
        name: "Denied",
        ty: structure("Denied", vec![]),
        samples: vec![value("Denied", vec![])],
        extra: vec![(FORMAT_JSON, br#"{"x":1}"#), (FORMAT_JSON, br#"[]"#)],
        derive: derived::<Denied>,
    }
}

fn unit() -> Mirror {
    Mirror {
        name: "()",
        ty: Type::Unit,
        samples: vec![Value::Unit(())],
        extra: vec![(FORMAT_JSON, b"{}")],
        derive: derived::<()>,
    }
}
