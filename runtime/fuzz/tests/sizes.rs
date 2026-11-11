// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The twin of the `sizes_vs_core` fuzz target, on stable and by `cargo test`
//! (`PROPTEST_CASES` raises the count): `comline-core`'s byte ranges and
//! witnesses for a type, held to the real encoders in `packed@1`, `msgpack@1`
//! and `json@1`. The check is `comline_runtime_fuzz::sizes`'s, the same one
//! the target runs.
//!
//! - Random type graphs and values (what the fuzz target reads from its
//!   input), and every scalar over its whole range.
//! - The tiers the specification's size vectors can't store, because their
//!   values are too large: a MessagePack array of 65536 items (a five-byte
//!   header), a string or array whose length takes a three- and a four-byte
//!   varint (2^21 characters), and the longest JSON floats.
//!
//! Run in release: the large values are megabytes.

use comline_core::schema::ir::size;
use comline_runtime::contract::{FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED};
use comline_runtime_conformance::{encode_as, encoded_len_as, Value, Wire};
use comline_runtime_fuzz::sizes::{
    check_input, check_type, Body, Declared, Field, Gen, Reach, Scalar, Scale, Schema, Style, Ty,
    FORMATS,
};
use proptest::prelude::*;

fn alone(ty: Ty) -> Schema {
    Schema { decls: vec![], root: ty }
}

/// `values` held to core's range for `ty`, with its witnesses encoded to
/// exactly `min` and `max`.
fn hold(schema: &Schema, ty: &Ty, values: &[(&str, Value)]) -> Reach {
    let values: Vec<(&str, &Value)> = values.iter().map(|(w, v)| (*w, v)).collect();
    let mut reach = Reach::default();
    check_type(schema, ty, Some(true), &values, Scale::Big, &mut reach).unwrap_or_else(|e| panic!("{e}"));
    reach
}

fn len(format: &str, value: &Value) -> usize {
    let bytes = encode_as(format, &Wire(value)).expect("the runtime encodes it");
    assert_eq!(encoded_len_as(format, &Wire(value)), Some(bytes.len()), "{format}: encoded_len");
    bytes.len()
}

fn range(schema: &Schema, ty: &Ty, format: &str) -> size::Range {
    let rules = size::rules(format).unwrap();
    size::range(&ty.to_ref(schema), rules, schema, ()).expect("core sizes it")
}

proptest! {
    /// What the fuzz target does: a schema from the front of the bytes,
    /// values from the rest.
    #[test]
    fn core_ranges_hold_for_random_schemas(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        check_input(&data).map_err(|e| TestCaseError::fail(e.to_string()))?;
    }

    #[test]
    fn integers_stay_in_range(
        a in any::<u8>(), b in any::<u16>(), c in any::<u32>(), d in any::<u64>(), e in any::<u128>(),
        f in any::<i8>(), g in any::<i16>(), h in any::<i32>(), i in any::<i64>(), j in any::<i128>(),
        k in any::<bool>(),
    ) {
        for (ty, value) in [
            (Scalar::U8, Value::U8(a)), (Scalar::U16, Value::U16(b)), (Scalar::U32, Value::U32(c)),
            (Scalar::U64, Value::U64(d)), (Scalar::U128, Value::U128(e)),
            (Scalar::I8, Value::I8(f)), (Scalar::I16, Value::I16(g)), (Scalar::I32, Value::I32(h)),
            (Scalar::I64, Value::I64(i)), (Scalar::I128, Value::I128(j)),
            (Scalar::Bool, Value::Bool(k)),
        ] {
            let ty = Ty::Scalar(ty);
            let mut reach = Reach::default();
            check_type(&alone(ty.clone()), &ty, Some(true), &[("a value", &value)], Scale::Big, &mut reach)
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
        }
    }

    /// Every float the encoders write is within the range, JSON's longest
    /// included.
    #[test]
    fn floats_stay_in_range(x in any::<f64>(), y in any::<f32>()) {
        for (ty, value) in [(Scalar::F64, Value::F64(x)), (Scalar::F32, Value::F32(y))] {
            let ty = Ty::Scalar(ty);
            let mut reach = Reach::default();
            check_type(&alone(ty.clone()), &ty, Some(true), &[("a float", &value)], Scale::Big, &mut reach)
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
        }
    }

    /// Strings of any characters, under a bound just enough, and exactly.
    #[test]
    fn strings_stay_in_range(
        chars in prop::collection::vec(any::<char>(), 0..200),
        spare in 0u64..3,
        c in any::<char>(),
    ) {
        let s: String = chars.iter().collect();
        let n = chars.len() as u64;
        for (ty, value) in [
            (Ty::Scalar(Scalar::String), Value::String(s.clone())),
            (Ty::CharMax(n + spare), Value::String(s.clone())),
            (Ty::CharN(n), Value::String(s.clone())),
            (Ty::Scalar(Scalar::Char), Value::Char(c)),
        ] {
            let mut reach = Reach::default();
            check_type(&alone(ty.clone()), &ty, Some(true), &[("a string", &value)], Scale::Big, &mut reach)
                .map_err(|e| TestCaseError::fail(e.to_string()))?;
        }
    }
}

/// A MessagePack array header is five bytes from 65536 items, three below
/// it, and one up to 15: the value, and core's witnesses, hit each.
#[test]
fn msgpack_array_headers_of_every_width() {
    for (count, header) in [(15, 1), (16, 3), (65535, 3), (65536, 5), (65537, 5)] {
        let ty = Ty::Array(Box::new(Ty::Scalar(Scalar::U8)), Some(count));
        let schema = alone(ty.clone());
        let value = Value::Array(vec![Value::U8(255); count as usize]);
        let reach = hold(&schema, &ty, &[("a full array", value.clone())]);
        // The witnesses are values the budget builds: this is a node each.
        assert!(reach.exact_max >= FORMATS.len() as u32, "{count}: no max witness: {reach:?}");
        let bytes = encode_as(FORMAT_MSGPACK, &Wire(&value)).unwrap();
        let tag = match header {
            1 => 0x90 | count as u8,
            3 => 0xdc,
            _ => 0xdd,
        };
        assert_eq!(bytes[0], tag, "{count} items");
        assert_eq!(bytes.len(), header + count as usize * 2, "{count} items, `cc ff` each");
        // The same count, unbounded: no max, so only the range's floor.
        let unbounded = Ty::Array(Box::new(Ty::Scalar(Scalar::U8)), None);
        let r = range(&alone(unbounded.clone()), &unbounded, FORMAT_MSGPACK);
        assert_eq!(r.max, None);
        assert!(bytes.len() as u64 >= r.min);
    }
}

/// A string's length is a varint of one to four bytes (up to 127, 16383,
/// 2^21 - 1, 2^28 - 1) in `packed@1`, and a `str` header of one to five in
/// `msgpack@1`; the longest string a type allows is a witness core builds,
/// and it must be exact in every format.
#[test]
fn string_length_headers_of_every_width() {
    // (characters, the varint's bytes for the longest string in `packed@1`)
    for (chars, varint) in [
        (31u64, 1),
        (32, 2),
        (127, 2),
        (4095, 2),
        (4096, 3),
        (16384, 3),
        (524287, 3),
        (524288, 4),
        (1 << 21, 4),
    ] {
        let ty = Ty::CharMax(chars);
        let schema = alone(ty.clone());
        let longest = Value::String("\u{1F600}".repeat(chars as usize));
        let reach = hold(&schema, &ty, &[("the longest string", longest.clone())]);
        assert!(reach.exact_max >= FORMATS.len() as u32, "char[max {chars}]: no max witness: {reach:?}");
        let r = range(&schema, &ty, FORMAT_PACKED);
        assert_eq!(r.max, Some(chars * 4 + varint));
        assert_eq!(len(FORMAT_PACKED, &longest) as u64, chars * 4 + varint, "char[max {chars}] in packed@1");
        // And one character at a time, `a` each: a different header.
        let ascii = Value::String("a".repeat(chars as usize));
        hold(&schema, &ty, &[("an ASCII string", ascii)]);
    }
}

/// A string of exactly 2^21 bytes, where the varint takes a fourth byte, and
/// one byte fewer, where it doesn't.
#[test]
fn a_string_of_2_pow_21_bytes() {
    let ty = Ty::Scalar(Scalar::String);
    for (bytes, header) in [((1usize << 21) - 1, 3), (1 << 21, 4), ((1 << 21) + 1, 4)] {
        let value = Value::String("a".repeat(bytes));
        assert_eq!(len(FORMAT_PACKED, &value), header + bytes);
        // A `str32` header: one tag and a four-byte length.
        assert_eq!(len(FORMAT_MSGPACK, &value), 5 + bytes);
        assert_eq!(len(FORMAT_JSON, &value), 2 + bytes);
        hold(&alone(ty.clone()), &ty, &[("a long string", value)]);
    }
}

/// A count of 3 and 4 bytes: arrays past 16383 and 2^21 - 1 items.
#[test]
fn array_count_varints_of_every_width() {
    for (count, header) in [(16383u64, 2), (16384, 3), (2097151, 3), (2097152, 4)] {
        let ty = Ty::Array(Box::new(Ty::Scalar(Scalar::U8)), Some(count));
        let schema = alone(ty.clone());
        // Core builds no witness past its budget; the value alone is held.
        let value = Value::Array(vec![Value::U8(0); count as usize]);
        assert_eq!(len(FORMAT_PACKED, &value) as u64, header + count);
        let r = range(&schema, &ty, FORMAT_PACKED);
        assert_eq!(r.max, Some(header + count), "u8[max {count}] in packed@1");
        let r = range(&schema, &ty, FORMAT_MSGPACK);
        let tag = if count > 65535 { 5 } else { 3 };
        assert_eq!(r.max, Some(tag + count * 2), "u8[max {count}] in msgpack@1");
        hold(&schema, &ty, &[("a full array", value)]);
    }
}

/// The longest text `serde_json` writes for a float. Core's maximum for each
/// is one a witness reaches; a scan of the floats, spread over every
/// exponent, finds none longer, and reaches the maximum.
#[test]
fn the_longest_json_floats() {
    for (ty, longest) in [(Scalar::F32, 16usize), (Scalar::F64, 24)] {
        let ty = Ty::Scalar(ty);
        let schema = alone(ty.clone());
        let r = range(&schema, &ty, FORMAT_JSON);
        assert_eq!(r.max, Some(longest as u64), "{ty:?}");
        assert!(!r.upper_bound, "{ty:?}");
        let reach = hold(&schema, &ty, &[("zero", if ty == Ty::Scalar(Scalar::F32) { Value::F32(0.0) } else { Value::F64(0.0) })]);
        assert_eq!(reach.exact_max, FORMATS.len() as u32, "{ty:?}: every format has a max witness");
    }
    // Every f32 from a stride through the bit patterns (a million), both
    // signs; a xorshift over f64 bits.
    let mut longest = 0;
    for bits in (0..=u32::MAX).step_by(4093) {
        for bits in [bits, bits | 0x8000_0000] {
            let x = f32::from_bits(bits);
            if x.is_finite() {
                longest = longest.max(len(FORMAT_JSON, &Value::F32(x)));
            }
        }
    }
    assert!(longest <= 16, "an f32 takes {longest} bytes in json@1");
    assert_eq!(longest, 16, "the scan reaches the longest f32");
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut longest = 0;
    for _ in 0..1_000_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let x = f64::from_bits(state);
        if x.is_finite() {
            longest = longest.max(len(FORMAT_JSON, &Value::F64(x)));
        }
    }
    assert!(longest <= 24, "an f64 takes {longest} bytes in json@1");
    assert_eq!(longest, 24, "the scan reaches the longest f64");
    // NaN and infinity are written as `null`, which the range holds.
    for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(len(FORMAT_JSON, &Value::F64(x)), 4);
        assert_eq!(len(FORMAT_JSON, &Value::F32(x as f32)), 4);
    }
}

/// An enum of more than 128 variants: the index is a two-byte varint from the
/// 129th, and a name is as long as it is; every variant is held.
#[test]
fn an_enum_of_200_variants() {
    let variants: Vec<String> =
        (0..200).map(|i| format!("V{i}{}", "n".repeat(if i % 5 == 0 { 254 } else { 0 }))).collect();
    let schema = Schema {
        decls: vec![Declared { name: "E0".into(), body: Body::Enum(variants.clone()), max_depth: None }],
        root: Ty::Named(0),
    };
    let ty = Ty::Named(0);
    let values: Vec<(&str, Value)> = variants
        .iter()
        .enumerate()
        .map(|(i, v)| {
            ("a variant", Value::UnitVariant { enum_name: "E0".into(), index: i as u32, variant: v.clone() })
        })
        .collect();
    hold(&schema, &ty, &values);
    let at = |i: usize| len(FORMAT_PACKED, &values[i].1);
    assert_eq!((at(127), at(128)), (1, 2));
}

/// A struct that contains itself under `@max_depth`: the deepest value, the
/// shallowest, and one past the limit that isn't a value.
#[test]
fn recursion_to_the_limit() {
    let node = |next: Option<Value>| Value::Struct {
        name: "S0".into(),
        fields: vec![
            ("label0".into(), Value::String("\u{1F600}\u{1F600}\u{1F600}".into())),
            ("next1".into(), Value::Option(next.map(Box::new))),
        ],
    };
    for limit in 1..=4u64 {
        let schema = Schema {
            decls: vec![Declared {
                name: "S0".into(),
                body: Body::Struct(vec![
                    Field { name: "label0".into(), ty: Ty::CharMax(3), optional: false },
                    Field { name: "next1".into(), ty: Ty::Named(0), optional: true },
                ]),
                max_depth: Some(limit),
            }],
            root: Ty::Named(0),
        };
        let ty = Ty::Named(0);
        let mut deepest = node(None);
        for _ in 1..limit {
            deepest = node(Some(deepest));
        }
        let values = [("the deepest", deepest.clone()), ("the shallowest", node(None))];
        let reach = hold(&schema, &ty, &values);
        assert_eq!(reach.exact_max, FORMATS.len() as u32, "@max_depth {limit}");
        // A level past the limit is not of the type, and the check knows it.
        let past = node(Some(deepest));
        assert!(schema.conforms(&ty, &past).is_err(), "@max_depth {limit}");
    }
}

/// The fuzz target reaches the tiers: its generator, over a few inputs,
/// makes the arrays and strings the vectors can't store.
#[test]
fn the_generator_reaches_the_large_tiers() {
    let reaches = |ty: Ty, at_least: usize, count: fn(&Value) -> usize| {
        let schema = alone(ty.clone());
        (0u8..=255).any(|seed| {
            let data = [seed; 64];
            let mut u = arbitrary::Unstructured::new(&data);
            matches!(Gen::new(&schema, &mut u, Style::Largest, Scale::Big).value(&ty), Ok(Some(v)) if count(&v) >= at_least)
        })
    };
    let items = |v: &Value| match v {
        Value::Array(items) => items.len(),
        _ => 0,
    };
    let chars = |v: &Value| match v {
        Value::String(s) => s.chars().count(),
        _ => 0,
    };
    assert!(reaches(Ty::Array(Box::new(Ty::Scalar(Scalar::U8)), Some(70000)), 65536, items));
    assert!(reaches(Ty::Array(Box::new(Ty::Scalar(Scalar::U8)), None), 65536, items));
    assert!(reaches(Ty::CharMax(1 << 21), 1 << 21, chars));
    assert!(reaches(Ty::Scalar(Scalar::String), 1 << 16, chars));
}
