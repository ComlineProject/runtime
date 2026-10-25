// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the fuzz targets share: reading a format and a [`Type`] from the
//! front of a fuzz input, and writing them back, so the seed corpus can turn
//! a vector into an input that reaches the same type.
//!
//! A type is read one byte per choice, from the front, with
//! [`Unstructured`]'s `u8` (an exhausted input reads as zero), in prefix
//! order:
//!
//! | tag (byte % 20) | then |
//! |---|---|
//! | 0..=14 | nothing: bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64, string, unit |
//! | 15 | enum: a name byte, a count byte (0..=5), a name byte per variant |
//! | 16 | option: the inner type |
//! | 17 | array: the element type |
//! | 18 | fixed array: the element type, then a length byte |
//! | 19 | struct: a name byte, a count byte (0..=6), a name byte and a type per field |
//!
//! Depth stops at [`MAX_DEPTH`]: past it, a tag reads as a scalar. A fixed
//! array is up to 40 long over a scalar and up to 4 over anything else, so
//! nesting stays small. Names come from fixed pools, since the conformance
//! crate's interner leaks one entry per distinct name and name list.
//!
//! An array whose element carries no data (unit, `T[0]`, a struct of only
//! such fields) is never produced: core refuses those at schema build
//! (ComlineProject/core#141, `validation::array-of-dataless-type`). Its
//! element becomes a struct of a `bool` and that element, as in
//! `conformance/tests/properties.rs`.

use arbitrary::Unstructured;
use comline_runtime::contract::{WireFormat, FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED};
use comline_runtime::format::{Json, MsgPack, Packed};
use comline_runtime_conformance::{Type, Value, Wire};

pub const FORMATS: [&str; 3] = [FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED];

pub const MAX_DEPTH: u32 = 4;
pub const MAX_FIELDS: usize = 6;
pub const MAX_VARIANTS: usize = 5;
pub const MAX_FIXED: usize = 40;
pub const MAX_FIXED_NESTED: usize = 4;

/// Struct and enum names. They never reach the wire.
pub const TYPE_NAMES: [&str; 8] = [
    "Point", "Tagged", "Line", "Player", "Empty", "Color", "Class", "Sample",
];
/// Field names: the vectors' and the mirrors', so a seed from them decodes
/// by name in JSON.
pub const FIELD_NAMES: [&str; 32] = [
    "x",
    "y",
    "z",
    "id",
    "label",
    "name",
    "flag",
    "small",
    "medium",
    "large",
    "huge",
    "signed_small",
    "signed_large",
    "required",
    "maybe",
    "many",
    "maybe_many",
    "ratio",
    "precise",
    "rgba",
    "digest",
    "rows",
    "maybe_rgba",
    "limit",
    "from",
    "to",
    "tags",
    "hp",
    "guild",
    "class",
    "pos",
    "body",
];
pub const VARIANT_NAMES: [&str; 8] = [
    "Red", "Green", "Blue", "Purple", "Warrior", "Mage", "Rogue", "None",
];

const SCALARS: [Type; 15] = [
    Type::Bool,
    Type::U8,
    Type::U16,
    Type::U32,
    Type::U64,
    Type::U128,
    Type::I8,
    Type::I16,
    Type::I32,
    Type::I64,
    Type::I128,
    Type::F32,
    Type::F64,
    Type::String,
    Type::Unit,
];
const TAG_ENUM: u8 = 15;
const TAG_OPTION: u8 = 16;
const TAG_ARRAY: u8 = 17;
const TAG_FIXED: u8 = 18;
const TAG_STRUCT: u8 = 19;
const TAGS: u8 = 20;

fn byte(u: &mut Unstructured) -> u8 {
    u.arbitrary::<u8>().unwrap_or(0)
}

fn pick<'a>(pool: &[&'a str], b: u8) -> &'a str {
    pool[b as usize % pool.len()]
}

/// The format named by the next byte.
pub fn read_format(u: &mut Unstructured) -> &'static str {
    FORMATS[byte(u) as usize % FORMATS.len()]
}

/// The type at the front of `u`.
pub fn read_type(u: &mut Unstructured) -> Type {
    read_type_at(u, MAX_DEPTH)
}

fn read_type_at(u: &mut Unstructured, depth: u32) -> Type {
    let mut tag = byte(u) % TAGS;
    if depth == 0 && tag > TAG_ENUM {
        tag %= TAG_ENUM;
    }
    match tag {
        TAG_ENUM => {
            let name = pick(&TYPE_NAMES, byte(u));
            let n = byte(u) as usize % (MAX_VARIANTS + 1);
            Type::Enum {
                name: name.into(),
                variants: (0..n)
                    .map(|_| pick(&VARIANT_NAMES, byte(u)).into())
                    .collect(),
            }
        }
        TAG_OPTION => Type::Option(Box::new(read_type_at(u, depth - 1))),
        TAG_ARRAY => array_of(read_type_at(u, depth - 1)),
        TAG_FIXED => {
            let of = read_type_at(u, depth - 1);
            let max = if scalar(&of) {
                MAX_FIXED
            } else {
                MAX_FIXED_NESTED
            };
            let len = byte(u) as usize % (max + 1);
            Type::Fixed {
                of: Box::new(of),
                len,
            }
        }
        TAG_STRUCT => {
            let name = pick(&TYPE_NAMES, byte(u));
            let n = byte(u) as usize % (MAX_FIELDS + 1);
            Type::Struct {
                name: name.into(),
                fields: (0..n)
                    .map(|_| {
                        let field = pick(&FIELD_NAMES, byte(u)).to_string();
                        (field, read_type_at(u, depth - 1))
                    })
                    .collect(),
            }
        }
        scalar => SCALARS[scalar as usize].clone(),
    }
}

fn scalar(ty: &Type) -> bool {
    !matches!(
        ty,
        Type::Option(_) | Type::Array(_) | Type::Fixed { .. } | Type::Struct { .. }
    )
}

/// Whether a value of `ty` takes any bytes.
pub fn carries_data(ty: &Type) -> bool {
    match ty {
        Type::Unit => false,
        Type::Fixed { of, len } => *len > 0 && carries_data(of),
        Type::Struct { fields, .. } => fields.iter().any(|(_, t)| carries_data(t)),
        _ => true,
    }
}

/// `T[]`, or an array of a `bool` and `T` when `T` carries no data.
pub fn array_of(t: Type) -> Type {
    let element = if carries_data(&t) {
        t
    } else {
        Type::Struct {
            name: "Line".into(),
            fields: vec![("flag".into(), Type::Bool), ("label".into(), t)],
        }
    };
    Type::Array(Box::new(element))
}

/// Writes `format`'s byte, for the seed corpus.
pub fn write_format(format: &str, out: &mut Vec<u8>) {
    let i = FORMATS.iter().position(|f| *f == format).unwrap_or(0);
    out.push(i as u8);
}

fn index_of(pool: &[&str], name: &str) -> u8 {
    pool.iter().position(|n| *n == name).unwrap_or(0) as u8
}

/// Writes the bytes [`read_type`] reads back as `ty`, as near as the
/// grammar allows: names outside the pools become the pool's first, and
/// sizes past the limits are cut.
pub fn write_type(ty: &Type, out: &mut Vec<u8>) {
    match ty {
        Type::Enum { name, variants } => {
            out.extend([TAG_ENUM, index_of(&TYPE_NAMES, name)]);
            let variants = &variants[..variants.len().min(MAX_VARIANTS)];
            out.push(variants.len() as u8);
            out.extend(variants.iter().map(|v| index_of(&VARIANT_NAMES, v)));
        }
        Type::Option(inner) => {
            out.push(TAG_OPTION);
            write_type(inner, out);
        }
        Type::Array(element) => {
            out.push(TAG_ARRAY);
            write_type(element, out);
        }
        Type::Fixed { of, len } => {
            out.push(TAG_FIXED);
            write_type(of, out);
            out.push((*len).min(MAX_FIXED) as u8);
        }
        Type::Struct { name, fields } => {
            out.extend([TAG_STRUCT, index_of(&TYPE_NAMES, name)]);
            let fields = &fields[..fields.len().min(MAX_FIELDS)];
            out.push(fields.len() as u8);
            for (field, t) in fields {
                out.push(index_of(&FIELD_NAMES, field));
                write_type(t, out);
            }
        }
        scalar => {
            let i = SCALARS.iter().position(|s| s == scalar).unwrap_or(0);
            out.push(i as u8);
        }
    }
}

/// A type `value` is a value of: an empty array's element is `u8`, and an
/// enum's other variants are made up around the one it holds.
pub fn type_of(value: &Value) -> Type {
    match value {
        Value::Bool(_) => Type::Bool,
        Value::U8(_) => Type::U8,
        Value::U16(_) => Type::U16,
        Value::U32(_) => Type::U32,
        Value::U64(_) => Type::U64,
        Value::U128(_) => Type::U128,
        Value::I8(_) => Type::I8,
        Value::I16(_) => Type::I16,
        Value::I32(_) => Type::I32,
        Value::I64(_) => Type::I64,
        Value::I128(_) => Type::I128,
        Value::F32(_) => Type::F32,
        Value::F64(_) => Type::F64,
        Value::String(_) => Type::String,
        Value::Unit(()) => Type::Unit,
        Value::Option(v) => Type::Option(Box::new(v.as_deref().map_or(Type::U8, type_of))),
        Value::Array(items) => Type::Array(Box::new(items.first().map_or(Type::U8, type_of))),
        Value::Tuple(items) => Type::Fixed {
            of: Box::new(items.first().map_or(Type::U8, type_of)),
            len: items.len(),
        },
        Value::Struct { name, fields } => Type::Struct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(f, v)| (f.clone(), type_of(v)))
                .collect(),
        },
        Value::UnitVariant {
            enum_name,
            index,
            variant,
        } => Type::Enum {
            name: enum_name.clone(),
            variants: (0..=*index)
                .map(|i| {
                    if i == *index {
                        variant.clone()
                    } else {
                        VARIANT_NAMES[i as usize % VARIANT_NAMES.len()].to_string()
                    }
                })
                .collect(),
        },
    }
}

/// `value` in `format`.
pub fn encode(format: &str, value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    match format {
        FORMAT_JSON => Json.encode(&Wire(value), &mut out),
        FORMAT_MSGPACK => MsgPack.encode(&Wire(value), &mut out),
        FORMAT_PACKED => Packed.encode(&Wire(value), &mut out),
        other => panic!("no format `{other}`"),
    }
    .expect("a value encodes");
    out
}
