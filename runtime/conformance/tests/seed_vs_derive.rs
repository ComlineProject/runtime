// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `TypeSeed` decodes exactly as a generated type does.
//!
//! Each mirror below is declared as the Rust generator declares types: plain
//! `serde` derives, `serde_big_array` past 32 elements. For every format, the
//! bytes of sample values, every single-byte mutation of them, every
//! truncation and a trailing byte go through both the derive and the seed.
//! Both must accept or both refuse, and what they accept must be the same
//! value. Two decodes are compared by their `packed@1` encoding, which is
//! positional and deterministic.

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

const FORMATS: [&str; 3] = [FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED];

fn encode(format: &str, value: &impl Serialize) -> Vec<u8> {
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

/// The inputs a decode is tried on: each sample's bytes, every single-byte
/// mutation, every truncation, and a byte appended.
fn inputs(valid: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut all = Vec::new();
    for bytes in valid {
        all.push(bytes.clone());
        for i in 0..bytes.len() {
            for flip in [0x01, 0x20, 0x80, 0xff] {
                let mut mutated = bytes.clone();
                mutated[i] ^= flip;
                all.push(mutated);
            }
            all.push(bytes[..i].to_vec());
        }
        for extra in [0x00, 0x01, b' ', b'0'] {
            let mut longer = bytes.clone();
            longer.push(extra);
            all.push(longer);
        }
    }
    all
}

/// Holds the seed for `ty` to the derive `T` on `samples` and on `extra`
/// inputs (hand-written ones, per format).
fn agrees<T: Serialize + DeserializeOwned>(ty: Type, samples: &[Value], extra: &[(&str, &[u8])]) {
    for format in FORMATS {
        let valid: Vec<Vec<u8>> = samples.iter().map(|v| encode(format, &Wire(v))).collect();
        let mut tried = inputs(&valid);
        tried.extend(
            extra
                .iter()
                .filter(|(f, _)| *f == format)
                .map(|(_, b)| b.to_vec()),
        );
        for bytes in tried {
            let derived = derive_decode::<T>(format, &bytes);
            let seeded = decode_typed(format, &ty, &bytes).expect("a known format");
            match (&derived, &seeded) {
                (None, None) => {}
                (Some(d), Some(s)) => assert_eq!(
                    encode(FORMAT_PACKED, d),
                    encode(FORMAT_PACKED, &Wire(s)),
                    "{format}: derive and seed read {bytes:02x?} differently ({ty:?})"
                ),
                _ => panic!(
                    "{format}: derive {} and seed {} {bytes:02x?} ({ty:?})",
                    if derived.is_some() {
                        "accepts"
                    } else {
                        "refuses"
                    },
                    if seeded.is_some() {
                        "accepts"
                    } else {
                        "refuses"
                    },
                ),
            }
        }
    }
}

fn structure(name: &str, fields: Vec<(&str, Type)>) -> Type {
    Type::Struct {
        name: name.into(),
        fields: fields.into_iter().map(|(f, t)| (f.into(), t)).collect(),
    }
}

fn value(name: &str, fields: Vec<(&str, Value)>) -> Value {
    Value::Struct {
        name: name.into(),
        fields: fields.into_iter().map(|(f, v)| (f.into(), v)).collect(),
    }
}

fn some(v: Value) -> Value {
    Value::Option(Some(Box::new(v)))
}

fn opt(t: Type) -> Type {
    Type::Option(Box::new(t))
}

fn fixed(of: Type, len: usize) -> Type {
    Type::Fixed {
        of: Box::new(of),
        len,
    }
}

#[test]
fn point() {
    agrees::<Point>(
        structure("Point", vec![("x", Type::I32), ("y", Type::I32)]),
        &[value(
            "Point",
            vec![("x", Value::I32(1)), ("y", Value::I32(-2))],
        )],
        &[
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
    );
}

#[test]
fn tagged() {
    agrees::<Tagged>(
        structure(
            "Tagged",
            vec![("id", Type::U32), ("label", opt(Type::String))],
        ),
        &[
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
        &[
            (FORMAT_JSON, br#"{"id":1}"#),
            (FORMAT_JSON, br#"{"label":"x"}"#),
            (FORMAT_JSON, br#"{"id":1,"label":null,"label":"x"}"#),
            (FORMAT_JSON, br#"{"id":4294967296}"#),
            (FORMAT_JSON, br#"{"id":1.0}"#),
            (FORMAT_JSON, br#"{"id":-1}"#),
        ],
    );
}

#[test]
fn color() {
    let ty = Type::Enum {
        name: "Color".into(),
        variants: vec!["Red".into(), "Green".into(), "Blue".into()],
    };
    let variant = |index: u32, name: &str| Value::UnitVariant {
        enum_name: "Color".into(),
        index,
        variant: name.into(),
    };
    agrees::<Color>(
        ty,
        &[variant(0, "Red"), variant(2, "Blue")],
        &[
            (FORMAT_JSON, br#""Purple""#),
            (FORMAT_JSON, br#"1"#),
            (FORMAT_JSON, br#"{"Green":null}"#),
            (FORMAT_PACKED, &[3]),
            (FORMAT_PACKED, &[0x80, 0x01]),
        ],
    );
}

#[test]
fn scalars() {
    let ty = structure(
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
    );
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
    agrees::<Scalars>(ty, &[sample(u128::MAX, u64::MAX), sample(7, 1 << 53)], &[]);
}

#[test]
fn bag() {
    let ty = structure(
        "Bag",
        vec![
            ("required", Type::String),
            ("maybe", opt(Type::U32)),
            ("many", Type::Array(Box::new(Type::String))),
            ("maybe_many", opt(Type::Array(Box::new(Type::U32)))),
        ],
    );
    let strings = |n: usize| Value::Array((0..n).map(|i| Value::String(i.to_string())).collect());
    agrees::<Bag>(
        ty,
        &[
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
        &[(FORMAT_JSON, br#"{"required":"r","many":[]}"#)],
    );
}

#[test]
fn sample_with_fixed_arrays() {
    let ty = structure(
        "Sample",
        vec![
            ("ratio", Type::F32),
            ("precise", Type::F64),
            ("rgba", fixed(Type::U8, 4)),
            ("digest", fixed(Type::U8, 40)),
            ("rows", Type::Array(Box::new(fixed(Type::U16, 3)))),
            ("maybe_rgba", opt(fixed(Type::U8, 4))),
        ],
    );
    let bytes = |n: u8| Value::Tuple((0..n).map(Value::U8).collect());
    agrees::<Sample>(
        ty,
        &[value(
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
        &[],
    );
}

#[test]
fn errors_and_unit() {
    agrees::<TooLong>(
        structure("TooLong", vec![("limit", Type::U32)]),
        &[value("TooLong", vec![("limit", Value::U32(9))])],
        &[],
    );
    agrees::<Denied>(
        structure("Denied", vec![]),
        &[value("Denied", vec![])],
        &[(FORMAT_JSON, br#"{"x":1}"#), (FORMAT_JSON, br#"[]"#)],
    );
    agrees::<()>(Type::Unit, &[Value::Unit(())], &[(FORMAT_JSON, b"{}")]);
}
