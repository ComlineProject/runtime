// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The byte ranges `comline-core` computes for a type, held to the real
//! encoders: what `sizes_vs_core` (the fuzz target) and
//! [`tests/sizes.rs`](../tests/sizes.rs) (its proptest twin) both run.
//!
//! A [`Schema`] is a bounded type graph (a root [`Ty`] and the declarations
//! it names) that becomes what the compiler freezes: [`TypeRef`]s, with a
//! [`Lookup`] over the declarations, so core sizes it exactly as it sizes a
//! compiled package's types. [`read_schema`] reads one from the front of an
//! input, one byte per choice, like [`read_type`](crate::read_type) does for
//! a conformance [`Type`](comline_runtime_conformance::Type):
//!
//! - scalars, with the 128-bit integers, `char`, `string` and `unit`;
//! - `char[max N]` and `char[N]`, `T[]`, `T[max N]` and `T[N]`, with
//!   bounds at the formats' tiers (a header grows at 15/16, 31/32, 127/128,
//!   255/256, 16383/16384, 65535/65536, 2^21) and past what a header can say;
//! - up to six declarations: structs (0 to 20 fields, some `optional`), unit
//!   enums (0 to 200 variants, names up to 300 bytes) and `type` aliases;
//! - recursion, through an `optional` field or an array only, so every type
//!   has a value, under `@max_depth` 1 to 4 or none; a required field names
//!   an earlier declaration.
//!
//! A value of it comes from [`Gen`], in a [`Style`]: the simplest, a random
//! one, or one at the bounds (full-length strings of the worst characters
//! for a format, the longest counts, the widest numbers and variants). Each
//! is a [`Value`] in the specification's typed notation, which
//! [`encode_as`] writes with the real encoder.
//!
//! [`check_schema`] asserts, for `packed@1`, `msgpack@1` and `json@1`:
//!
//! - core sizes the type exactly when the generator can build a value of it;
//! - each value is of the type ([`Schema::conforms`]), encodes to a length in
//!   `[min, max]`, and `encoded_len` says that length;
//! - core's witnesses are values of the type too, the least one encodes to
//!   exactly `min` bytes and the most one to exactly `max`.
//!
//! That is soundness (no value escapes the range) and tightness (both ends
//! are reached), with no rule of the specification trusted on review.

use std::borrow::Cow;

use arbitrary::Unstructured;
use serde::Deserialize as _;
use comline_core::schema::ir::frozen::types::{PrimitiveType, TypeRef};
use comline_core::schema::ir::size::{self, Decl, Lookup, Member, Range, Resolved};
use comline_runtime::contract::{FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED};
use comline_runtime_conformance::{encode_as, encoded_len_as, Value, Wire};

pub const FORMATS: [&str; 3] = [FORMAT_PACKED, FORMAT_MSGPACK, FORMAT_JSON];

/// Declarations in a schema, at most.
pub const MAX_DECLS: usize = 6;
/// `@max_depth`, at most.
pub const MAX_RECURSION: u64 = 4;
/// How much a check may build, in values and witnesses. Most inputs are
/// [`Scale::Small`]: building a witness (a `serde_json::Value` a node, which
/// costs hundreds of bytes) is what a run spends its time on. One in sixteen
/// is [`Scale::Big`], which reaches the tiers that need a lot of items or
/// characters: a MessagePack array of 65536 items (a five-byte header), a
/// string of 2^21 characters (a four-byte varint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Small,
    Big,
}

impl Scale {
    /// Past it, arrays and strings are cut short.
    fn soft_items(self) -> u64 {
        match self {
            Scale::Small => 1 << 13,
            Scale::Big => 1 << 19,
        }
    }

    fn soft_chars(self) -> u64 {
        match self {
            Scale::Small => 1 << 15,
            Scale::Big => 1 << 22,
        }
    }

    /// Past it, the value is dropped.
    fn hard_items(self) -> u64 {
        match self {
            Scale::Small => 1 << 17,
            Scale::Big => 1 << 20,
        }
    }

    fn hard_chars(self) -> u64 {
        match self {
            Scale::Small => 1 << 18,
            Scale::Big => 1 << 23,
        }
    }
}

/// What a witness may take to build (a node or a character each), when the
/// type's biggest value is mostly nodes and the scale is big.
pub const WITNESS_BUDGET: usize = 1 << 18;
/// What it may take when the biggest value is made of characters, which cost
/// a byte or four: this reaches a string of 2^21 characters, a four-byte
/// length.
pub const WITNESS_BUDGET_CHARS: usize = 1 << 23;
/// What it may take at the small scale.
pub const WITNESS_BUDGET_SMALL: usize = 1 << 12;

/// A length up to this is picked as it is; past it, one time in eight.
const HEAVY: u64 = 1 << 17;

/// The sizes where a header grows, and the ones around them.
const TIERS: [u64; 24] = [
    0, 1, 2, 3, 7, 8, 15, 16, 17, 31, 32, 33, 127, 128, 255, 256, 16383, 16384, 65535, 65536,
    65537, 2097151, 2097152, 2097153,
];
/// A bound: the tiers, and bounds past what any header says (2^32 - 1).
const BOUNDS: [u64; 30] = [
    0, 1, 2, 3, 7, 8, 15, 16, 17, 31, 32, 63, 64, 127, 128, 255, 256, 4095, 4096, 16383, 16384,
    65535, 65536, 65537, 2097151, 2097152, 1 << 30, u32::MAX as u64, 1 << 32, 5_000_000_000,
];
/// The length of a fixed array of scalars, and of a fixed string.
const FIXED_BIG: [u64; 14] = [
    1, 2, 3, 4, 8, 15, 16, 17, 32, 255, 256, 65535, 65536, 65537,
];
const FIXED_SMALL: [u64; 9] = [1, 2, 3, 4, 8, 15, 16, 17, 33];
const FIXED_CHARS: [u64; 14] = [0, 1, 2, 3, 8, 31, 32, 33, 255, 256, 4097, 16384, 65536, 70000];
const FIELD_COUNTS: [usize; 14] = [0, 1, 1, 2, 2, 3, 4, 5, 6, 8, 15, 16, 17, 20];
const VARIANT_COUNTS: [usize; 16] = [1, 1, 2, 3, 4, 5, 8, 15, 16, 17, 31, 127, 128, 129, 130, 200];
/// Field names; the index follows, so they are distinct in a struct and few
/// in all (the conformance crate's interner leaks one entry per name).
const FIELD_NAMES: [&str; 10] = [
    "x",
    "y",
    "id",
    "label",
    "flag",
    "maybe",
    "rows",
    "precise",
    "name_of_thirty_or_so_bytes_long",
    "a_rather_long_name_that_is_well_over_thirty_one_bytes_long_for_json_keys",
];
/// How many `n` pad a variant's name.
const VARIANT_PADS: [usize; 8] = [0, 0, 1, 5, 25, 26, 40, 254];

/// A primitive type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scalar {
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
    Char,
    Unit,
}

const SCALARS: [Scalar; 16] = [
    Scalar::Bool,
    Scalar::U8,
    Scalar::U16,
    Scalar::U32,
    Scalar::U64,
    Scalar::U128,
    Scalar::I8,
    Scalar::I16,
    Scalar::I32,
    Scalar::I64,
    Scalar::I128,
    Scalar::F32,
    Scalar::F64,
    Scalar::String,
    Scalar::Char,
    Scalar::Unit,
];

/// A type, as a schema writes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Scalar(Scalar),
    /// `char[max N]`.
    CharMax(u64),
    /// `char[N]`.
    CharN(u64),
    /// `T[]` or `T[max N]`.
    Array(Box<Ty>, Option<u64>),
    /// `T[N]`.
    Fixed(Box<Ty>, u64),
    /// A declaration, by its index in [`Schema::decls`].
    Named(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Struct(Vec<Field>),
    /// Unit variants only.
    Enum(Vec<String>),
    /// `type NAME = TARGET`.
    Alias(Ty),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declared {
    pub name: String,
    pub body: Body,
    /// `@max_depth`, on a struct or an alias.
    pub max_depth: Option<u64>,
}

/// A bounded type graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub decls: Vec<Declared>,
    pub root: Ty,
}

impl Ty {
    /// What the compiler freezes this type as.
    pub fn to_ref(&self, schema: &Schema) -> TypeRef {
        let prim = |p| TypeRef::Primitive(p);
        let chars = |length, max| TypeRef::Array {
            element: Box::new(prim(PrimitiveType::Char)),
            length,
            max,
        };
        match self {
            Ty::Scalar(s) => match s {
                Scalar::Bool => prim(PrimitiveType::Bool),
                Scalar::U8 => prim(PrimitiveType::U8),
                Scalar::U16 => prim(PrimitiveType::U16),
                Scalar::U32 => prim(PrimitiveType::U32),
                Scalar::U64 => prim(PrimitiveType::U64),
                Scalar::U128 => prim(PrimitiveType::U128),
                Scalar::I8 => prim(PrimitiveType::S8),
                Scalar::I16 => prim(PrimitiveType::S16),
                Scalar::I32 => prim(PrimitiveType::S32),
                Scalar::I64 => prim(PrimitiveType::S64),
                Scalar::I128 => prim(PrimitiveType::S128),
                Scalar::F32 => prim(PrimitiveType::F32),
                Scalar::F64 => prim(PrimitiveType::F64),
                Scalar::String => prim(PrimitiveType::String),
                Scalar::Char => prim(PrimitiveType::Char),
                Scalar::Unit => TypeRef::Unit,
            },
            Ty::CharMax(n) => chars(None, Some(*n)),
            Ty::CharN(n) => chars(Some(*n), None),
            Ty::Array(of, max) => TypeRef::Array {
                element: Box::new(of.to_ref(schema)),
                length: None,
                max: *max,
            },
            Ty::Fixed(of, n) => TypeRef::Array {
                element: Box::new(of.to_ref(schema)),
                length: Some(*n),
                max: None,
            },
            Ty::Named(at) => TypeRef::Named(schema.decls[*at].name.clone()),
        }
    }
}

/// Core resolves the names in a schema's types through it, as it does a
/// compiled package's.
impl Lookup for Schema {
    type Scope = ();

    fn resolve(&self, _: (), name: &str) -> Option<Resolved<'_, ()>> {
        let at = self.decls.iter().position(|d| d.name == name)?;
        let d = &self.decls[at];
        let decl = match &d.body {
            Body::Struct(fields) => Decl::Struct {
                name: Cow::Borrowed(&d.name),
                fields: fields
                    .iter()
                    .map(|f| Member {
                        name: Cow::Borrowed(&f.name),
                        ty: Cow::Owned(f.ty.to_ref(self)),
                        optional: f.optional,
                    })
                    .collect(),
                max_depth: d.max_depth,
            },
            Body::Enum(variants) => Decl::Enum {
                name: Cow::Borrowed(&d.name),
                variants: variants.iter().map(|v| Cow::Borrowed(v.as_str())).collect(),
            },
            Body::Alias(target) => Decl::Alias {
                target: Cow::Owned(target.to_ref(self)),
                max_depth: d.max_depth,
            },
        };
        Some(Resolved { key: at + 1, scope: (), decl })
    }
}

fn byte(u: &mut Unstructured) -> u8 {
    u.arbitrary::<u8>().unwrap_or(0)
}

fn pick<T: Copy>(u: &mut Unstructured, pool: &[T]) -> T {
    pool[byte(u) as usize % pool.len()]
}

/// A bound (`char[max N]`, `T[max N]`): at a tier, or a few characters.
fn read_bound(u: &mut Unstructured) -> u64 {
    match byte(u) % 8 {
        0 => byte(u) as u64 % 100,
        _ => pick(u, &BOUNDS),
    }
}

/// A type; `me` is the declaration being written (`decls.len()` for the
/// root), whose earlier declarations a required field may name, and a
/// `guarded` one, behind an `optional` field or an array, any.
fn read_ty(u: &mut Unstructured, decls: usize, me: usize, guarded: bool, depth: u8) -> Ty {
    let names: Vec<usize> = (0..decls).filter(|at| *at < me || guarded).collect();
    let tag = byte(u) % 16;
    let scalar = |u: &mut Unstructured| Ty::Scalar(pick(u, &SCALARS));
    if depth == 0 && tag > 9 {
        return scalar(u);
    }
    match tag {
        0..=4 => scalar(u),
        5 => Ty::CharMax(read_bound(u)),
        6 => Ty::CharN(pick(u, &FIXED_CHARS)),
        7 | 8 => {
            let of = read_ty(u, decls, me, true, depth - 1);
            let max = match byte(u) % 3 {
                0 => None,
                _ => Some(read_bound(u)),
            };
            match (of, max) {
                // An array of `char` is a string: that is how the IR has it.
                (Ty::Scalar(Scalar::Char), None) => Ty::Scalar(Scalar::String),
                (Ty::Scalar(Scalar::Char), Some(max)) => Ty::CharMax(max),
                (of, max) => Ty::Array(Box::new(of), max),
            }
        }
        9 | 10 => {
            let of = read_ty(u, decls, me, guarded, depth - 1);
            let big = matches!(of, Ty::Scalar(_));
            let n = if big { pick(u, &FIXED_BIG) } else { pick(u, &FIXED_SMALL) };
            match of {
                Ty::Scalar(Scalar::Char) => Ty::CharN(n),
                of => Ty::Fixed(Box::new(of), n),
            }
        }
        _ if names.is_empty() => scalar(u),
        _ => Ty::Named(pick(u, &names)),
    }
}

/// The schema at the front of `u`.
pub fn read_schema(u: &mut Unstructured) -> Schema {
    let n = byte(u) as usize % (MAX_DECLS + 1);
    let mut decls = Vec::with_capacity(n);
    for at in 0..n {
        let max_depth = match byte(u) as u64 % (MAX_RECURSION + 1) {
            0 => None,
            k => Some(k),
        };
        let (name, body) = match byte(u) % 8 {
            0..=4 => {
                let count = pick(u, &FIELD_COUNTS);
                let fields = (0..count)
                    .map(|i| {
                        let optional = byte(u) % 3 == 0;
                        Field {
                            name: format!("{}{i}", pick(u, &FIELD_NAMES)),
                            ty: read_ty(u, n, at, optional, 2),
                            optional,
                        }
                    })
                    .collect();
                (format!("S{at}"), Body::Struct(fields))
            }
            5 | 6 => {
                // One time in sixteen, none: no value of it exists.
                let count = if byte(u) % 16 == 0 { 0 } else { pick(u, &VARIANT_COUNTS) };
                let pad = pick(u, &VARIANT_PADS);
                let variants = (0..count)
                    .map(|i| format!("V{i}{}", "n".repeat(if i % 5 == 0 { pad } else { 0 })))
                    .collect();
                (format!("E{at}"), Body::Enum(variants))
            }
            _ => (format!("A{at}"), Body::Alias(read_ty(u, n, at, false, 2))),
        };
        decls.push(Declared { name, body, max_depth });
    }
    let root = read_ty(u, n, n, false, 3);
    let mut schema = Schema { decls, root };
    schema.carry_data_in_arrays();
    schema
}

impl Schema {
    /// Whether a value of `ty` takes no bytes in `packed@1`: `unit`, an
    /// empty struct, a struct or alias of only such.
    fn dataless(&self, ty: &Ty, seen: &mut Vec<usize>) -> bool {
        match ty {
            Ty::Scalar(s) => *s == Scalar::Unit,
            Ty::CharMax(_) | Ty::CharN(_) | Ty::Array(..) => false,
            Ty::Fixed(of, n) => *n == 0 || self.dataless(of, seen),
            Ty::Named(at) => {
                if seen.contains(at) {
                    return false;
                }
                seen.push(*at);
                let out = match &self.decls[*at].body {
                    Body::Struct(fields) => {
                        fields.iter().all(|f| !f.optional && self.dataless(&f.ty, seen))
                    }
                    Body::Enum(_) => false,
                    Body::Alias(target) => self.dataless(target, seen),
                };
                seen.pop();
                out
            }
        }
    }

    /// Core refuses an array of a type that carries no data
    /// (`validation::array-of-dataless-type`, ComlineProject/core#141): such
    /// an element becomes a `u8`.
    fn carry_data_in_arrays(&mut self) {
        fn walk(schema: &Schema, ty: &mut Ty) {
            match ty {
                Ty::Array(of, _) | Ty::Fixed(of, _) => {
                    if schema.dataless(of, &mut vec![]) {
                        **of = Ty::Scalar(Scalar::U8);
                    }
                    walk(schema, of);
                }
                _ => {}
            }
        }
        let snapshot = self.clone();
        for d in &mut self.decls {
            match &mut d.body {
                Body::Struct(fields) => fields.iter_mut().for_each(|f| walk(&snapshot, &mut f.ty)),
                Body::Alias(target) => walk(&snapshot, target),
                Body::Enum(_) => {}
            }
        }
        walk(&snapshot, &mut self.root);
    }

    /// Whether `value` is of `ty`: its shape, its bounds and its nesting.
    pub fn conforms(&self, ty: &Ty, value: &Value) -> Result<(), String> {
        self.conforms_in(ty, value, &mut vec![])
    }

    fn conforms_in(&self, ty: &Ty, value: &Value, stack: &mut Vec<usize>) -> Result<(), String> {
        let wrong = |what: &str| Err(format!("{what}: {}", shown(&format!("{value:?}"))));
        match (ty, value) {
            (Ty::Scalar(s), v) => {
                let ok = matches!(
                    (s, v),
                    (Scalar::Bool, Value::Bool(_))
                        | (Scalar::U8, Value::U8(_))
                        | (Scalar::U16, Value::U16(_))
                        | (Scalar::U32, Value::U32(_))
                        | (Scalar::U64, Value::U64(_))
                        | (Scalar::U128, Value::U128(_))
                        | (Scalar::I8, Value::I8(_))
                        | (Scalar::I16, Value::I16(_))
                        | (Scalar::I32, Value::I32(_))
                        | (Scalar::I64, Value::I64(_))
                        | (Scalar::I128, Value::I128(_))
                        | (Scalar::F32, Value::F32(_))
                        | (Scalar::F64, Value::F64(_))
                        | (Scalar::String, Value::String(_))
                        | (Scalar::Char, Value::Char(_))
                        | (Scalar::Unit, Value::Unit(()))
                );
                if ok { Ok(()) } else { wrong(&format!("not a {s:?}")) }
            }
            (Ty::CharMax(max), Value::String(s)) => {
                let chars = s.chars().count() as u64;
                if chars <= *max { Ok(()) } else { Err(format!("{chars} characters, over char[max {max}]")) }
            }
            (Ty::CharN(n), Value::String(s)) => {
                let chars = s.chars().count() as u64;
                if chars == *n { Ok(()) } else { Err(format!("{chars} characters, not char[{n}]")) }
            }
            (Ty::Array(of, max), Value::Array(items)) => {
                if max.is_some_and(|m| items.len() as u64 > m) {
                    return Err(format!("{} items, over the bound {max:?}", items.len()));
                }
                items.iter().try_for_each(|item| self.conforms_in(of, item, stack))
            }
            (Ty::Fixed(of, n), Value::Tuple(items)) => {
                if items.len() as u64 != *n {
                    return Err(format!("{} items, not T[{n}]", items.len()));
                }
                items.iter().try_for_each(|item| self.conforms_in(of, item, stack))
            }
            (Ty::Named(at), value) => {
                let d = &self.decls[*at];
                match (&d.body, value) {
                    (Body::Enum(variants), Value::UnitVariant { enum_name, index, variant }) => {
                        let ok = *enum_name == d.name
                            && variants.get(*index as usize).is_some_and(|v| v == variant);
                        if ok { Ok(()) } else { wrong(&format!("not a variant of {}", d.name)) }
                    }
                    (Body::Struct(fields), Value::Struct { name, fields: given }) => {
                        let level = stack.iter().filter(|k| **k == *at).count() as u64 + 1;
                        if d.max_depth.is_some_and(|limit| level > limit) {
                            return Err(format!("{} nested {level} deep, over its @max_depth", d.name));
                        }
                        if *name != d.name || fields.len() != given.len() {
                            return wrong(&format!("not a {}", d.name));
                        }
                        stack.push(*at);
                        let out = fields.iter().zip(given).try_for_each(|(f, (given_name, v))| {
                            if *given_name != f.name {
                                return Err(format!("field {given_name}, not {}", f.name));
                            }
                            match (f.optional, v) {
                                (true, Value::Option(None)) => Ok(()),
                                (true, Value::Option(Some(v))) => self.conforms_in(&f.ty, v, stack),
                                (true, _) => Err(format!("{} isn't an option", f.name)),
                                (false, v) => self.conforms_in(&f.ty, v, stack),
                            }
                        });
                        stack.pop();
                        out
                    }
                    (Body::Alias(target), value) => {
                        let level = stack.iter().filter(|k| **k == *at).count() as u64 + 1;
                        if d.max_depth.is_some_and(|limit| level > limit) {
                            return Err(format!("{} nested {level} deep, over its @max_depth", d.name));
                        }
                        stack.push(*at);
                        let out = self.conforms_in(target, value, stack);
                        stack.pop();
                        out
                    }
                    _ => wrong(&format!("not a {}", d.name)),
                }
            }
            _ => wrong(&format!("not a {ty:?}")),
        }
    }

    /// How many nodes and characters the largest value of `ty` has, as far
    /// as core builds a witness of it; saturating, and cut off after
    /// `steps` declarations.
    fn weight(&self, ty: &Ty, stack: &mut Vec<usize>, steps: &mut u32) -> (u64, u64) {
        let times = |(nodes, chars): (u64, u64), n: u64| (nodes.saturating_mul(n), chars.saturating_mul(n));
        match ty {
            Ty::Scalar(_) => (1, 0),
            Ty::CharMax(n) | Ty::CharN(n) => (1, (*n).min(u32::MAX as u64)),
            // No largest value, so no witness: only the least one is built.
            Ty::Array(_, None) => (1, 0),
            Ty::Array(of, Some(n)) | Ty::Fixed(of, n) => {
                let (nodes, chars) = times(self.weight(of, stack, steps), (*n).min(u32::MAX as u64));
                (nodes.saturating_add(1), chars)
            }
            Ty::Named(at) => {
                let d = &self.decls[*at];
                if let Body::Enum(_) = d.body {
                    return (1, 0);
                }
                let level = stack.iter().filter(|k| **k == *at).count() as u64 + 1;
                if *steps == 0 || d.max_depth.is_some_and(|limit| level > limit) || (d.max_depth.is_none() && level > 1) {
                    return (0, 0);
                }
                *steps -= 1;
                stack.push(*at);
                let out = match &d.body {
                    Body::Struct(fields) => fields.iter().fold((1u64, 0u64), |(nodes, chars), f| {
                        let (n, c) = self.weight(&f.ty, stack, steps);
                        (nodes.saturating_add(n).saturating_add(f.optional as u64), chars.saturating_add(c))
                    }),
                    Body::Alias(target) => self.weight(target, stack, steps),
                    Body::Enum(_) => (1, 0),
                };
                stack.pop();
                out
            }
        }
    }

    /// What core may spend building a witness of `ty`: enough for its
    /// largest value, up to [`WITNESS_BUDGET_CHARS`] when that is mostly
    /// characters and [`WITNESS_BUDGET`] when it is nodes.
    pub fn witness_budget(&self, ty: &Ty, scale: Scale) -> usize {
        if scale == Scale::Small {
            return WITNESS_BUDGET_SMALL;
        }
        let (nodes, chars) = self.weight(ty, &mut vec![], &mut 4096);
        if nodes > WITNESS_BUDGET as u64 {
            return WITNESS_BUDGET;
        }
        (nodes.saturating_add(chars) as usize).clamp(WITNESS_BUDGET, WITNESS_BUDGET_CHARS)
    }
}

/// How a value is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// The simplest: zeros, empty, absent. It exists exactly when core can
    /// size the type.
    Smallest,
    /// Anywhere in the type, with the numbers, lengths and characters
    /// where an encoding changes.
    Random,
    /// At the bounds: full-length strings and arrays of the widest numbers
    /// and the worst characters for a format, present options, the longest
    /// and the last variant.
    Largest,
}

/// A value too big to build here.
#[derive(Debug)]
pub struct Overrun;

/// Builds values of a [`Schema`]'s types from the bytes of an input.
pub struct Gen<'s, 'u, 'd> {
    schema: &'s Schema,
    u: &'u mut Unstructured<'d>,
    style: Style,
    scale: Scale,
    items: u64,
    chars: u64,
    stack: Vec<usize>,
}

impl<'s, 'u, 'd> Gen<'s, 'u, 'd> {
    pub fn new(schema: &'s Schema, u: &'u mut Unstructured<'d>, style: Style, scale: Scale) -> Self {
        Gen { schema, u, style, scale, items: 0, chars: 0, stack: vec![] }
    }

    fn byte(&mut self) -> u8 {
        byte(self.u)
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if lo >= hi {
            return lo;
        }
        self.u.int_in_range(lo..=hi).unwrap_or(lo)
    }

    fn spend_item(&mut self) -> Result<(), Overrun> {
        self.items += 1;
        if self.items > self.scale.hard_items() { Err(Overrun) } else { Ok(()) }
    }

    /// A count or length in `lo..=hi`.
    fn length(&mut self, lo: u64, hi: u64) -> u64 {
        let hi = hi.max(lo);
        if lo == hi {
            return lo;
        }
        let arm = match self.style {
            Style::Smallest => 0,
            Style::Largest => [1, 1, 1, 3][self.byte() as usize % 4],
            Style::Random => [0, 1, 2, 3, 4, 4][self.byte() as usize % 6],
        };
        match arm {
            0 => lo,
            1 if hi <= HEAVY || self.byte() % 8 == 0 => hi,
            2 => hi - 1,
            4 => {
                let top = lo.saturating_add(16).min(hi);
                self.range(lo, top)
            }
            // A tier, or the bound when none lies within.
            _ => {
                let heavy = self.byte() % 8 == 0;
                let within: Vec<u64> =
                    TIERS.iter().copied().filter(|t| (lo..=hi).contains(t) && (*t <= HEAVY || heavy)).collect();
                if within.is_empty() { hi.min(HEAVY).max(lo) } else { within[self.byte() as usize % within.len()] }
            }
        }
    }

    fn uint(&mut self, bits: u32) -> u128 {
        let max = if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 };
        if self.style == Style::Smallest {
            return 0;
        }
        let kind = self.byte();
        let kind = if self.style == Style::Largest { [1, 5, 2][kind as usize % 3] } else { kind % 6 };
        let b = self.byte() as u32 % bits;
        let r = match kind {
            0 => 0,
            1 => max,
            2 => 1u128 << b,
            3 => (1u128 << b) - 1,
            4 => (1u128 << b) + 1,
            _ => self.u.arbitrary::<u128>().unwrap_or(0),
        };
        r & max
    }

    fn sint(&mut self, bits: u32) -> i128 {
        let negative = self.style == Style::Largest || self.byte() % 2 == 1;
        let mag = self.uint(bits);
        let half = 1u128 << (bits - 1);
        if negative {
            let m = mag.min(half);
            (m as i128).wrapping_neg()
        } else {
            (mag & (half - 1)) as i128
        }
    }

    fn f64(&mut self) -> f64 {
        const SPECIAL: [f64; 14] = [
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            5e-324,
            1e15,
            1e16,
            1e17,
            1e-5,
            1e-6,
            1e21,
            0.1 + 0.2,
            -1.2610470545525327e-05,
            -2.2250738585072014e-308,
        ];
        if self.style == Style::Smallest {
            return 0.0;
        }
        let bits = self.u.arbitrary::<u64>().unwrap_or(0);
        let random = f64::from_bits(bits);
        let random = if random.is_finite() { random } else { -1.5 };
        let arm = if self.style == Style::Largest { [1, 1, 3][self.byte() as usize % 3] } else { self.byte() % 7 };
        match arm {
            0 => [0.0, -0.0][self.byte() as usize % 2],
            1 => random,
            2 => (bits as i32) as f64 * 10f64.powi(self.byte() as i32 % 40 - 20),
            3 => SPECIAL[self.byte() as usize % SPECIAL.len()],
            4 => [f64::NAN, f64::INFINITY, f64::NEG_INFINITY][self.byte() as usize % 3],
            _ => bits as f64,
        }
    }

    fn f32(&mut self) -> f32 {
        const SPECIAL: [f32; 12] = [
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            -f32::MIN_POSITIVE,
            1e-45,
            16777216.0,
            1e-5,
            1e-6,
            1e16,
            1e17,
            -6233676316672.0,
            0.1 + 0.2,
        ];
        if self.style == Style::Smallest {
            return 0.0;
        }
        let bits = self.u.arbitrary::<u32>().unwrap_or(0);
        let random = f32::from_bits(bits);
        let random = if random.is_finite() { random } else { -1.5 };
        let arm = if self.style == Style::Largest { [1, 1, 3][self.byte() as usize % 3] } else { self.byte() % 7 };
        match arm {
            0 => [0.0, -0.0][self.byte() as usize % 2],
            1 => random,
            2 => (bits as i32) as f32 * 10f32.powi(self.byte() as i32 % 20 - 10),
            3 => SPECIAL[self.byte() as usize % SPECIAL.len()],
            4 => [f32::NAN, f32::INFINITY, f32::NEG_INFINITY][self.byte() as usize % 3],
            _ => bits as f32,
        }
    }

    /// A character of every width an encoding tells apart.
    fn char(&mut self) -> char {
        if self.style == Style::Smallest {
            return 'a';
        }
        let class = if self.style == Style::Largest { [6, 7, 8, 9, 1][self.byte() as usize % 5] } else { self.byte() % 11 };
        let code = match class {
            0 => self.range(0x20, 0x7e),
            1 => self.range(0, 0x1f),
            2 => ['"', '\\', '\u{8}', '\t', '\n', '\u{c}', '\r'][self.byte() as usize % 7] as u64,
            3 => 0x7f,
            4 => self.range(0x80, 0x7ff),
            5 => {
                let c = self.range(0x800, 0xffff - 0x800);
                if c >= 0xd800 { c + 0x800 } else { c }
            }
            6 => self.range(0x10000, 0x10ffff),
            7 => 1,
            8 => 0x10ffff,
            9 => 0x1f600,
            _ => [0x800, 0xffff, 0x80, 0x7ff, 0x10000][self.byte() as usize % 5],
        };
        char::from_u32(code as u32).unwrap_or('a')
    }

    /// `lo` to `hi` characters, from a few of them repeated.
    fn string(&mut self, lo: u64, hi: u64) -> Result<String, Overrun> {
        let soft = self.scale.soft_chars().saturating_sub(self.chars);
        let n = self.length(lo, hi.min(soft.max(lo)));
        self.chars = self.chars.saturating_add(n);
        if self.chars > self.scale.hard_chars() {
            return Err(Overrun);
        }
        let alphabet: Vec<char> = (0..1 + self.byte() % 3).map(|_| self.char()).collect();
        let mut s = String::with_capacity(n as usize);
        for i in 0..n as usize {
            s.push(alphabet[i % alphabet.len()]);
        }
        Ok(s)
    }

    /// A value of `ty`; `None` where no value of it exists here (a type that
    /// contains itself, past its `@max_depth`).
    pub fn value(&mut self, ty: &Ty) -> Result<Option<Value>, Overrun> {
        self.spend_item()?;
        Ok(Some(match ty {
            Ty::Scalar(s) => match s {
                Scalar::Bool => Value::Bool(self.style != Style::Smallest && self.byte() % 2 == 1),
                Scalar::U8 => Value::U8(self.uint(8) as u8),
                Scalar::U16 => Value::U16(self.uint(16) as u16),
                Scalar::U32 => Value::U32(self.uint(32) as u32),
                Scalar::U64 => Value::U64(self.uint(64) as u64),
                Scalar::U128 => Value::U128(self.uint(128)),
                Scalar::I8 => Value::I8(self.sint(8) as i8),
                Scalar::I16 => Value::I16(self.sint(16) as i16),
                Scalar::I32 => Value::I32(self.sint(32) as i32),
                Scalar::I64 => Value::I64(self.sint(64) as i64),
                Scalar::I128 => Value::I128(self.sint(128)),
                Scalar::F32 => Value::F32(self.f32()),
                Scalar::F64 => Value::F64(self.f64()),
                Scalar::String => Value::String(self.string(0, u64::MAX)?),
                Scalar::Char => Value::Char(self.char()),
                Scalar::Unit => Value::Unit(()),
            },
            Ty::CharMax(max) => Value::String(self.string(0, *max)?),
            Ty::CharN(n) => {
                if *n > self.scale.hard_chars() {
                    return Err(Overrun);
                }
                Value::String(self.string(*n, *n)?)
            }
            Ty::Array(of, max) => {
                let soft = self.scale.soft_items().saturating_sub(self.items);
                let hi = max.unwrap_or(u64::MAX).min(soft);
                let n = self.length(0, hi);
                let mut items = Vec::with_capacity(n.min(1 << 16) as usize);
                for _ in 0..n {
                    match self.value(of)? {
                        Some(item) => items.push(item),
                        // None of the element exists here: none are written.
                        None => break,
                    }
                }
                Value::Array(items)
            }
            Ty::Fixed(of, n) => {
                if *n > self.scale.hard_items() {
                    return Err(Overrun);
                }
                let mut items = Vec::with_capacity((*n).min(1 << 16) as usize);
                for _ in 0..*n {
                    match self.value(of)? {
                        Some(item) => items.push(item),
                        None => return Ok(None),
                    }
                }
                Value::Tuple(items)
            }
            Ty::Named(at) => {
                let schema = self.schema;
                let d = &schema.decls[*at];
                let level = self.stack.iter().filter(|k| **k == *at).count() as u64 + 1;
                // A declaration without a limit nests as far as the stack
                // goes, then is cut short where it can be (an option, an array).
                let cut = d.max_depth.is_some_and(|limit| level > limit) || self.stack.len() >= 24;
                match &d.body {
                    Body::Enum(variants) => {
                        if variants.is_empty() {
                            return Ok(None);
                        }
                        let last = variants.len() as u64 - 1;
                        let at = match self.style {
                            Style::Smallest => 0,
                            Style::Largest => {
                                // The last variant, or the one with the longest name.
                                if self.byte() % 2 == 0 {
                                    last
                                } else {
                                    (0..variants.len()).max_by_key(|i| variants[*i].len()).unwrap_or(0) as u64
                                }
                            }
                            Style::Random => self.range(0, last),
                        };
                        Value::UnitVariant {
                            enum_name: d.name.clone(),
                            index: at as u32,
                            variant: variants[at as usize].clone(),
                        }
                    }
                    Body::Alias(target) => {
                        if cut {
                            return Ok(None);
                        }
                        self.stack.push(*at);
                        let out = self.value(target);
                        self.stack.pop();
                        return out;
                    }
                    Body::Struct(fields) => {
                        if cut {
                            return Ok(None);
                        }
                        self.stack.push(*at);
                        let mut given = Vec::with_capacity(fields.len());
                        for f in fields {
                            let value = if f.optional {
                                let present = match self.style {
                                    Style::Smallest => false,
                                    // Present, until the value is large enough.
                                    Style::Largest => self.items < self.scale.soft_items() / 4,
                                    Style::Random => self.byte() % 2 == 0,
                                };
                                let inner = if present { self.value(&f.ty) } else { Ok(None) };
                                match inner {
                                    Ok(inner) => Value::Option(inner.map(Box::new)),
                                    Err(e) => {
                                        self.stack.pop();
                                        return Err(e);
                                    }
                                }
                            } else {
                                match self.value(&f.ty) {
                                    Ok(Some(v)) => v,
                                    other => {
                                        self.stack.pop();
                                        return other;
                                    }
                                }
                            };
                            given.push((f.name.clone(), value));
                        }
                        self.stack.pop();
                        Value::Struct { name: d.name.clone(), fields: given }
                    }
                }
            }
        }))
    }
}

/// At most 600 characters of `s`, for a failure message.
fn shown(s: &str) -> String {
    match s.char_indices().nth(600) {
        Some((end, _)) => format!("{}... ({} bytes)", &s[..end], s.len()),
        None => s.to_string(),
    }
}

/// What a check did, so a test can see it reached something.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reach {
    /// Types core sized, in a format each.
    pub sized: u32,
    /// Values encoded and held to a range.
    pub values: u32,
    /// Witnesses encoded and held to exactly `min` or `max`.
    pub exact_min: u32,
    pub exact_max: u32,
    /// The longest value encoded, in bytes.
    pub longest: u64,
}

impl Reach {
    pub fn add(&mut self, other: &Reach) {
        self.sized += other.sized;
        self.values += other.values;
        self.exact_min += other.exact_min;
        self.exact_max += other.exact_max;
        self.longest = self.longest.max(other.longest);
    }
}

/// A disagreement between core's range and an encoder, or a broken
/// generator or witness, described.
#[derive(Debug)]
pub struct Disagreement(pub String);

impl std::fmt::Display for Disagreement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Disagreement {}

macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(Disagreement(format!($($arg)*)))
    };
}

/// Encodes `value` in `format` and holds its length to `range`.
fn hold(
    format: &str,
    range: &Range,
    value: &Value,
    what: &str,
    reach: &mut Reach,
) -> Result<u64, Disagreement> {
    let Some(bytes) = encode_as(format, &Wire(value)) else {
        bail!("{format}: the runtime refused to encode {what}: {}", shown(&format!("{value:?}")));
    };
    let len = bytes.len() as u64;
    let counted = encoded_len_as(format, &Wire(value));
    if counted != Some(bytes.len()) {
        bail!("{format}: encoded_len of {what} is {counted:?}, it encodes to {len} bytes");
    }
    if len < range.min || range.max.is_some_and(|max| len > max) {
        bail!(
            "{format}: {what} encodes to {len} bytes, outside core's range {}..={:?}: {}",
            range.min,
            range.max,
            shown(&format!("{value:?}"))
        );
    }
    reach.values += 1;
    reach.longest = reach.longest.max(len);
    Ok(len)
}

/// Core's range and witnesses for `ty`, held to the encoders in every
/// format, and `values` held to the range. `inhabited` is whether a value
/// of `ty` exists (the simplest was built), where that is known.
pub fn check_type(
    schema: &Schema,
    ty: &Ty,
    inhabited: Option<bool>,
    values: &[(&str, &Value)],
    scale: Scale,
    reach: &mut Reach,
) -> Result<(), Disagreement> {
    let tyref = ty.to_ref(schema);
    for format in FORMATS {
        let rules = size::rules(format).expect("the official format has size rules");
        let range = size::range(&tyref, rules, schema, ());
        let Some(range) = range else {
            if inhabited == Some(true) {
                bail!("{format}: core can't size {ty:?}, a type with a value");
            }
            continue;
        };
        if inhabited == Some(false) {
            bail!("{format}: core sizes {ty:?} as {range:?}, a type with no value");
        }
        reach.sized += 1;
        for (what, value) in values {
            schema.conforms(ty, value).map_err(|why| Disagreement(format!("the generator made {what}, not a {ty:?}: {why}")))?;
            hold(format, &range, value, what, reach)?;
        }
        let budget = schema.witness_budget(ty, scale);
        let Some(w) = size::witnesses(&tyref, rules, schema, (), budget) else {
            continue;
        };
        let min = witness(&w.min).map_err(|e| Disagreement(format!("{format}: the min witness: {e}")))?;
        schema.conforms(ty, &min).map_err(|why| Disagreement(format!("{format}: core's min witness isn't a {ty:?}: {why}")))?;
        let len = hold(format, &range, &min, "the min witness", reach)?;
        if len != range.min {
            bail!(
                "{format}: the min witness encodes to {len} bytes, core's min is {}: {}",
                range.min,
                shown(&format!("{min:?}"))
            );
        }
        reach.exact_min += 1;
        if let Some(max) = &w.max {
            let max = witness(max).map_err(|e| Disagreement(format!("{format}: the max witness: {e}")))?;
            schema.conforms(ty, &max).map_err(|why| Disagreement(format!("{format}: core's max witness isn't a {ty:?}: {why}")))?;
            let len = hold(format, &range, &max, "the max witness", reach)?;
            if Some(len) != range.max || range.upper_bound {
                bail!(
                    "{format}: the max witness encodes to {len} bytes, core's max is {:?}{}: {}",
                    range.max,
                    if range.upper_bound { " (an upper bound, which has no witness)" } else { "" },
                    shown(&format!("{max:?}"))
                );
            }
            reach.exact_max += 1;
        }
    }
    Ok(())
}

/// A witness as the conformance crate's [`Value`].
fn witness(value: &serde_json::Value) -> Result<Value, String> {
    Value::deserialize(value).map_err(|e| format!("{e}: {}", shown(&value.to_string())))
}

/// Everything `sizes_vs_core` asserts about `schema`: its root and each
/// declaration have core's range and witnesses held to the encoders, and
/// three values of the root (the simplest, a random one and one at the
/// bounds, from the rest of `u`) held to the range.
pub fn check_schema(schema: &Schema, u: &mut Unstructured, scale: Scale) -> Result<Reach, Disagreement> {
    let mut reach = Reach::default();
    let mut tys = vec![schema.root.clone()];
    tys.extend((0..schema.decls.len()).map(Ty::Named));
    for (at, ty) in tys.iter().enumerate() {
        let mut values = vec![];
        let simplest = Gen::new(schema, u, Style::Smallest, scale).value(ty);
        let inhabited = match &simplest {
            Ok(v) => Some(v.is_some()),
            // Too big to build here: unknown.
            Err(_) => None,
        };
        if let Ok(Some(v)) = simplest {
            values.push(("the simplest value", v));
        }
        // The root also gets values of its own; a declaration, its witnesses.
        if at == 0 && inhabited == Some(true) {
            for (what, style) in [("a random value", Style::Random), ("a value at the bounds", Style::Largest)] {
                match Gen::new(schema, u, style, scale).value(ty) {
                    Ok(Some(v)) => values.push((what, v)),
                    // Too big to build here, or none (past @max_depth).
                    _ => {}
                }
            }
        }
        let shown_values: Vec<(&str, &Value)> = values.iter().map(|(w, v)| (*w, v)).collect();
        check_type(schema, ty, inhabited, &shown_values, scale, &mut reach)
            .map_err(|e| Disagreement(format!("{e}\n  the type: {ty:?}\n  the schema: {schema:?}")))?;
    }
    Ok(reach)
}

/// The whole check on one input: its scale from the first byte, a schema
/// from what follows, values from the rest.
pub fn check_input(data: &[u8]) -> Result<Reach, Disagreement> {
    let mut u = Unstructured::new(data);
    let scale = if byte(&mut u) % 16 == 0 { Scale::Big } else { Scale::Small };
    let schema = read_schema(&mut u);
    check_schema(&schema, &mut u, scale)
}
