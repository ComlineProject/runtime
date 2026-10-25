// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Properties of the reference, over random types and values.
//!
//! - **Round trip:** a value of a random [`Type`], written by [`Wire`] and
//!   read back by [`decode_typed`], is the same value, in every format.
//!   Floats compare by their bits.
//! - **`packed@1` is canonical:** any bytes that decode as a type re-encode to
//!   exactly those bytes.
//! - **Decoded values are values of the type:** whatever a format accepts,
//!   valid or mutated, passes [`check`].
//! - **Seed against derive:** the fixed mirrors of `tests/seed_vs_derive.rs`
//!   (in `common/mod.rs`) on proptest's mutations and on random bytes.
//!
//! A run is short by default, so `cargo test` stays fast; `PROPTEST_CASES`
//! raises the count per property (the nightly job sets 100000).
//!
//! The values each format can carry:
//!
//! - **JSON has no NaN or infinity.** `serde_json` writes a non-finite `f32`
//!   or `f64` as `null`, which then doesn't decode as a float, so JSON values
//!   are finite. `packed@1` and MessagePack carry every bit pattern, NaN
//!   payloads included, and get them all.
//! - **JSON and MessagePack can't tell `Some(None)` from `None`,** nor
//!   `Some(())` from `None`: `Option<Option<T>>` and `Option<()>` write
//!   both as `null` (`nil`), as for any `serde` type. Their generated types
//!   never put an option or unit straight in an option, so for them the
//!   generator wraps such an inner type in an array.
//! - **No format has arrays of data-less elements.** Core refuses `T[]`
//!   when `T` carries no data (unit, `T[0]`, a struct of only such fields),
//!   at schema build, so no generated type has one; runtime#29. The
//!   generator gives such an element a `bool` field beside it. `packed@1`
//!   refuses a count past the bytes left even then, which the vector
//!   `format-packed/reject-count-past-input` pins.

mod common;

use comline_runtime::contract::{FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED};
use comline_runtime_conformance::{check, decode_typed, Type, Value};
use proptest::prelude::*;
use proptest::sample::Index;

/// Struct, field, enum and variant names come from fixed pools: the
/// interner leaks one entry per distinct name and per distinct list.
const STRUCT_NAMES: [&str; 4] = ["Point", "Player", "Line", "Empty"];
const ENUM_NAMES: [&str; 2] = ["Color", "Class"];
/// A struct's fields are a prefix of these, so they never repeat.
const FIELD_NAMES: [&str; 6] = ["id", "name", "x", "y", "tags", "label"];
/// An enum's variants are a prefix of these.
const VARIANT_NAMES: [&str; 5] = ["Red", "Green", "Blue", "Warrior", "Mage"];

const MAX_FIELDS: usize = 6;
const MAX_ELEMENTS: usize = 8;
/// `T[N]` past 32 needs `serde_big_array` in generated Rust.
const MAX_FIXED: usize = 40;
/// A fixed array of something that isn't a scalar stays short, so nesting
/// them doesn't multiply into a huge value.
const MAX_FIXED_NESTED: usize = 4;

fn config() -> ProptestConfig {
    let mut config = ProptestConfig::default();
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 64;
    }
    config
}

/// What a format can carry, which shapes the values drawn for it.
#[derive(Clone, Copy, Debug)]
struct Carries {
    /// NaN and infinity.
    non_finite: bool,
    /// `Some(None)` and `Some(())`, apart from `None`.
    nested_null: bool,
}

fn carries(format: &str) -> Carries {
    match format {
        FORMAT_JSON => Carries {
            non_finite: false,
            nested_null: false,
        },
        FORMAT_MSGPACK => Carries {
            non_finite: true,
            nested_null: false,
        },
        _ => Carries {
            non_finite: true,
            nested_null: true,
        },
    }
}

/// Whether a value of `ty` takes any bytes: unit, `T[0]`, a fixed array of
/// such, and a struct of only such fields (or none) don't. An option does.
fn carries_data(ty: &Type) -> bool {
    match ty {
        Type::Unit => false,
        Type::Fixed { of, len } => *len > 0 && carries_data(of),
        Type::Struct { fields, .. } => fields.iter().any(|(_, t)| carries_data(t)),
        _ => true,
    }
}

/// `T[]`, or, when `T` carries no data, an array of a struct holding a
/// `bool` and `T`: core refuses arrays of data-less types, runtime#29.
fn array_of(t: Type) -> Type {
    let element = if carries_data(&t) {
        t
    } else {
        Type::Struct {
            name: STRUCT_NAMES[2].into(),
            fields: vec![
                (FIELD_NAMES[0].into(), Type::Bool),
                (FIELD_NAMES[1].into(), t),
            ],
        }
    };
    Type::Array(Box::new(element))
}

fn scalar(ty: &Type) -> bool {
    !matches!(
        ty,
        Type::Option(_) | Type::Array(_) | Type::Fixed { .. } | Type::Struct { .. }
    )
}

fn type_strategy(c: Carries) -> impl Strategy<Value = Type> {
    let leaf = prop_oneof![
        Just(Type::Bool),
        Just(Type::U8),
        Just(Type::U16),
        Just(Type::U32),
        Just(Type::U64),
        Just(Type::U128),
        Just(Type::I8),
        Just(Type::I16),
        Just(Type::I32),
        Just(Type::I64),
        Just(Type::I128),
        Just(Type::F32),
        Just(Type::F64),
        Just(Type::String),
        Just(Type::Unit),
        (0..ENUM_NAMES.len(), 1..=VARIANT_NAMES.len()).prop_map(|(name, n)| Type::Enum {
            name: ENUM_NAMES[name].into(),
            variants: VARIANT_NAMES[..n].iter().map(|v| v.to_string()).collect(),
        }),
    ];
    // Depth 4: at most four levels of option, array, fixed array or struct
    // above a scalar.
    leaf.prop_recursive(4, 48, MAX_FIELDS as u32, move |inner| {
        prop_oneof![
            inner.clone().prop_map(move |t| {
                let t = match t {
                    Type::Option(_) | Type::Unit if !c.nested_null => array_of(t),
                    t => t,
                };
                Type::Option(Box::new(t))
            }),
            inner.clone().prop_map(array_of),
            inner.clone().prop_flat_map(|t| {
                let max = if scalar(&t) {
                    MAX_FIXED
                } else {
                    MAX_FIXED_NESTED
                };
                (Just(t), 0..=max).prop_map(|(t, len)| Type::Fixed {
                    of: Box::new(t),
                    len,
                })
            }),
            (
                0..STRUCT_NAMES.len(),
                prop::collection::vec(inner, 0..=MAX_FIELDS)
            )
                .prop_map(|(name, fields)| Type::Struct {
                    name: STRUCT_NAMES[name].into(),
                    fields: FIELD_NAMES
                        .iter()
                        .map(|f| f.to_string())
                        .zip(fields)
                        .collect(),
                }),
        ]
    })
}

/// A whole range, with its edges drawn more often than chance would.
macro_rules! int {
    ($t:ty, $variant:ident) => {
        prop_oneof![
            Just(<$t>::MIN),
            Just(<$t>::MAX),
            Just(0 as $t),
            Just(1 as $t),
            any::<$t>(),
            any::<$t>(),
            any::<$t>(),
        ]
        .prop_map(Value::$variant)
        .boxed()
    };
}

fn f32_strategy(c: Carries) -> BoxedStrategy<f32> {
    let edges = prop_oneof![
        Just(0.0f32),
        Just(-0.0),
        Just(f32::MIN),
        Just(f32::MAX),
        Just(f32::MIN_POSITIVE),
        Just(f32::from_bits(1)),
        Just(f32::EPSILON),
        Just(1.5),
    ];
    if c.non_finite {
        prop_oneof![
            edges,
            Just(f32::NAN),
            Just(f32::INFINITY),
            Just(f32::NEG_INFINITY),
            // Any bit pattern: NaN payloads, subnormals, everything.
            any::<u32>().prop_map(f32::from_bits),
            any::<u32>().prop_map(f32::from_bits),
        ]
        .boxed()
    } else {
        prop_oneof![
            edges,
            any::<u32>()
                .prop_map(f32::from_bits)
                .prop_filter("JSON floats are finite", |f| f.is_finite()),
            any::<u32>()
                .prop_map(f32::from_bits)
                .prop_filter("JSON floats are finite", |f| f.is_finite()),
        ]
        .boxed()
    }
}

fn f64_strategy(c: Carries) -> BoxedStrategy<f64> {
    let edges = prop_oneof![
        Just(0.0f64),
        Just(-0.0),
        Just(f64::MIN),
        Just(f64::MAX),
        Just(f64::MIN_POSITIVE),
        Just(f64::from_bits(1)),
        Just(f64::EPSILON),
        Just(0.1),
        Just(9007199254740993.0),
    ];
    if c.non_finite {
        prop_oneof![
            edges,
            Just(f64::NAN),
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
            any::<u64>().prop_map(f64::from_bits),
            any::<u64>().prop_map(f64::from_bits),
        ]
        .boxed()
    } else {
        prop_oneof![
            edges,
            any::<u64>()
                .prop_map(f64::from_bits)
                .prop_filter("JSON floats are finite", |f| f.is_finite()),
            any::<u64>()
                .prop_map(f64::from_bits)
                .prop_filter("JSON floats are finite", |f| f.is_finite()),
        ]
        .boxed()
    }
}

/// A value of `ty`.
fn value_strategy(ty: &Type, c: Carries) -> BoxedStrategy<Value> {
    match ty {
        Type::Bool => any::<bool>().prop_map(Value::Bool).boxed(),
        Type::U8 => int!(u8, U8),
        Type::U16 => int!(u16, U16),
        Type::U32 => int!(u32, U32),
        Type::U64 => int!(u64, U64),
        Type::U128 => int!(u128, U128),
        Type::I8 => int!(i8, I8),
        Type::I16 => int!(i16, I16),
        Type::I32 => int!(i32, I32),
        Type::I64 => int!(i64, I64),
        Type::I128 => int!(i128, I128),
        Type::F32 => f32_strategy(c).prop_map(Value::F32).boxed(),
        Type::F64 => f64_strategy(c).prop_map(Value::F64).boxed(),
        Type::String => prop::collection::vec(any::<char>(), 0..16)
            .prop_map(|chars| Value::String(chars.into_iter().collect()))
            .boxed(),
        Type::Unit => Just(Value::Unit(())).boxed(),
        Type::Option(inner) => prop_oneof![
            Just(Value::Option(None)),
            value_strategy(inner, c).prop_map(|v| Value::Option(Some(Box::new(v)))),
        ]
        .boxed(),
        Type::Array(element) => prop::collection::vec(value_strategy(element, c), 0..=MAX_ELEMENTS)
            .prop_map(Value::Array)
            .boxed(),
        Type::Fixed { of, len } => prop::collection::vec(value_strategy(of, c), *len)
            .prop_map(Value::Tuple)
            .boxed(),
        Type::Struct { name, fields } => {
            let names: Vec<String> = fields.iter().map(|(f, _)| f.clone()).collect();
            let name = name.clone();
            fields
                .iter()
                .map(|(_, t)| value_strategy(t, c))
                .collect::<Vec<_>>()
                .prop_map(move |values| Value::Struct {
                    name: name.clone(),
                    fields: names.iter().cloned().zip(values).collect(),
                })
                .boxed()
        }
        Type::Enum { name, variants } => {
            let (name, variants) = (name.clone(), variants.clone());
            (0..variants.len())
                .prop_map(move |i| Value::UnitVariant {
                    enum_name: name.clone(),
                    index: i as u32,
                    variant: variants[i].clone(),
                })
                .boxed()
        }
    }
}

/// A random type and a value of it, as `format` can carry.
fn typed_value(format: &'static str) -> impl Strategy<Value = (Type, Value)> {
    let c = carries(format);
    type_strategy(c).prop_flat_map(move |ty| {
        let value = value_strategy(&ty, c);
        (Just(ty), value)
    })
}

/// Equal, with floats compared by their bits.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::F32(x), Value::F32(y)) => x.to_bits() == y.to_bits(),
        (Value::F64(x), Value::F64(y)) => x.to_bits() == y.to_bits(),
        (Value::Array(xs), Value::Array(ys)) | (Value::Tuple(xs), Value::Tuple(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same(x, y))
        }
        (Value::Option(Some(x)), Value::Option(Some(y))) => same(x, y),
        (
            Value::Struct {
                name: xn,
                fields: xs,
            },
            Value::Struct {
                name: yn,
                fields: ys,
            },
        ) => {
            xn == yn
                && xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys)
                    .all(|((xf, x), (yf, y))| xf == yf && same(x, y))
        }
        _ => a == b,
    }
}

/// Changes to valid bytes: bytes flipped, then a cut, then bytes appended.
#[derive(Debug, Clone)]
struct Mutation {
    flips: Vec<(Index, u8)>,
    cut: Option<Index>,
    append: Vec<u8>,
}

impl Mutation {
    fn apply(&self, bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        if !out.is_empty() {
            for (at, xor) in &self.flips {
                let i = at.index(out.len());
                out[i] ^= xor;
            }
        }
        if let Some(cut) = &self.cut {
            out.truncate(cut.index(out.len() + 1));
        }
        out.extend_from_slice(&self.append);
        out
    }
}

fn mutation() -> impl Strategy<Value = Mutation> {
    (
        prop::collection::vec((any::<Index>(), 1..=u8::MAX), 0..4),
        prop::option::weighted(0.3, any::<Index>()),
        prop::collection::vec(any::<u8>(), 0..3),
    )
        .prop_map(|(flips, cut, append)| Mutation { flips, cut, append })
}

/// Bytes to decode: a valid encoding mutated, or random bytes.
#[derive(Debug, Clone)]
enum Input {
    Mutated(Mutation),
    Random(Vec<u8>),
}

impl Input {
    fn bytes(&self, valid: &[u8]) -> Vec<u8> {
        match self {
            Input::Mutated(m) => m.apply(valid),
            Input::Random(bytes) => bytes.clone(),
        }
    }
}

fn input() -> impl Strategy<Value = Input> {
    prop_oneof![
        3 => mutation().prop_map(Input::Mutated),
        1 => prop::collection::vec(any::<u8>(), 0..64).prop_map(Input::Random),
    ]
}

fn encode(format: &str, value: &Value) -> Vec<u8> {
    common::encode(format, &comline_runtime_conformance::Wire(value))
}

fn round_trips(format: &str, ty: &Type, value: &Value) -> Result<(), TestCaseError> {
    prop_assert_eq!(check(value, ty), Ok(()));
    let bytes = encode(format, value);
    let decoded = decode_typed(format, ty, &bytes).expect("a known format");
    prop_assert!(
        decoded.as_ref().is_some_and(|d| same(d, value)),
        "{} wrote {:?} as {:02x?}, which reads back as {:?}",
        format,
        value,
        bytes,
        decoded
    );
    Ok(())
}

/// Whatever `format` accepts of `bytes` is a value of `ty`.
fn decodes_to_a_value(
    format: &str,
    ty: &Type,
    bytes: &[u8],
) -> Result<Option<Value>, TestCaseError> {
    let decoded = decode_typed(format, ty, bytes).expect("a known format");
    if let Some(d) = &decoded {
        prop_assert_eq!(check(d, ty), Ok(()));
    }
    Ok(decoded)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn json_round_trips((ty, value) in typed_value(FORMAT_JSON)) {
        round_trips(FORMAT_JSON, &ty, &value)?;
    }

    #[test]
    fn msgpack_round_trips((ty, value) in typed_value(FORMAT_MSGPACK)) {
        round_trips(FORMAT_MSGPACK, &ty, &value)?;
    }

    #[test]
    fn packed_round_trips((ty, value) in typed_value(FORMAT_PACKED)) {
        round_trips(FORMAT_PACKED, &ty, &value)?;
    }

    /// `packed@1` has one encoding per value: bytes that decode re-encode
    /// to themselves.
    #[test]
    fn packed_is_canonical((ty, value) in typed_value(FORMAT_PACKED), input in input()) {
        let bytes = input.bytes(&encode(FORMAT_PACKED, &value));
        if let Some(decoded) = decodes_to_a_value(FORMAT_PACKED, &ty, &bytes)? {
            let again = encode(FORMAT_PACKED, &decoded);
            prop_assert_eq!(again, bytes);
        }
    }

    #[test]
    fn json_decodes_only_values((ty, value) in typed_value(FORMAT_JSON), input in input()) {
        decodes_to_a_value(FORMAT_JSON, &ty, &input.bytes(&encode(FORMAT_JSON, &value)))?;
    }

    #[test]
    fn msgpack_decodes_only_values((ty, value) in typed_value(FORMAT_MSGPACK), input in input()) {
        decodes_to_a_value(FORMAT_MSGPACK, &ty, &input.bytes(&encode(FORMAT_MSGPACK, &value)))?;
    }

    /// The seed accepts and refuses what a derive does, on the mirrors'
    /// samples mutated and on random bytes.
    #[test]
    fn seed_agrees_with_derive(
        mirror in any::<Index>(),
        sample in any::<Index>(),
        format in prop::sample::select(common::FORMATS.to_vec()),
        input in input(),
    ) {
        let mirrors = common::mirrors();
        let m = &mirrors[mirror.index(mirrors.len())];
        let valid = m.valid(format);
        let bytes = input.bytes(&valid[sample.index(valid.len())]);
        if let Err(why) = m.agrees_on(format, &bytes) {
            return Err(TestCaseError::fail(why));
        }
    }
}
