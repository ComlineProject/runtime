// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use comline_runtime::contract::{FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED};

use crate::engine::encode_with;
use crate::{decode_typed, Case, Compare, DecodeIn, EncodeIn, Expect, Hex, Input, Type, Value};

use super::{exact, reject};

pub(super) fn point_type() -> Type {
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

/// Bytes `written` encodes to, read back as `ty`: an exact case with the
/// value the runtime decoded, or a rejection if it refused them.
fn decode_case(id: String, format: &str, written: &Value, ty: Type) -> Case {
    let bytes = encode_with(format, written).expect("reference formats encode");
    match decode_typed(format, &ty, &bytes) {
        Ok(Some(value)) => Case {
            id,
            input: Input::Decode(DecodeIn {
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
            Input::Decode(DecodeIn {
                format: format.into(),
                ty,
                value: None,
            }),
            &bytes,
        ),
        Err(outcome) => panic!("reference case `{id}` doesn't decode: {outcome:?}"),
    }
}

// --- A single `char`, and bounds (core#98) ---

/// `bytes` read as `ty`: an exact `decode` case with what the reference
/// decoded, or a rejection, which `rejected` says it must be.
fn decode_bytes_case(id: String, format: &str, bytes: &[u8], ty: Type, rejected: bool) -> Case {
    let value = decode_typed(format, &ty, bytes)
        .unwrap_or_else(|outcome| panic!("reference case `{id}` doesn't decode: {outcome:?}"));
    assert_eq!(value.is_none(), rejected, "reference case `{id}`");
    let case = |value| {
        Input::Decode(DecodeIn {
            format: format.into(),
            ty: ty.clone(),
            value,
        })
    };
    match value {
        Some(value) => Case {
            id,
            input: case(Some(value)),
            bytes: Hex(bytes.to_vec()),
            expect: Expect::Exact,
            compare: Compare::Bytes,
        },
        None => reject(&id, case(None), bytes),
    }
}

/// A single `char` read back, and bytes that aren't exactly one character.
pub(super) fn char_cases(feature_id: &str, format: &str) -> Vec<Case> {
    let p = |id: &str| format!("{feature_id}/{id}");
    let mut cases: Vec<Case> = [
        ("ascii", 'a'),
        ("2-byte", 'é'),
        ("3-byte", '€'),
        ("4-byte", '😀'),
    ]
    .into_iter()
    .map(|(id, c)| {
        decode_case(
            p(&format!("decode-char-{id}")),
            format,
            &Value::Char(c),
            Type::Char,
        )
    })
    .collect();
    let bad: &[(&str, &[u8])] = match format {
        FORMAT_PACKED => &[
            ("empty", b""),
            ("continuation-byte", b"\x80"),
            ("truncated", b"\xc3"),
            ("overlong", b"\xc0\x80"),
            ("surrogate", b"\xed\xa0\x80"),
            ("past-max", b"\xf4\x90\x80\x80"),
            // A character, then one more byte: a value uses every byte.
            ("two", b"ab"),
        ],
        FORMAT_MSGPACK => &[
            ("empty", b"\xa0"),
            ("two", b"\xa2ab"),
            ("overlong", b"\xa2\xc0\x80"),
            ("surrogate", b"\xa3\xed\xa0\x80"),
            ("bin", b"\xc4\x01a"),
            ("integer", b"\x61"),
        ],
        FORMAT_JSON => &[
            ("empty", b"\"\""),
            ("two", b"\"ab\""),
            ("lone-surrogate", b"\"\\ud800\""),
            ("integer", b"97"),
        ],
        _ => &[],
    };
    cases.extend(bad.iter().map(|(id, bytes)| {
        decode_bytes_case(
            p(&format!("decode-char-reject-{id}")),
            format,
            bytes,
            Type::Char,
            true,
        )
    }));
    cases
}

/// Bounds: a value at its type's maximum, one over it, multi-byte
/// characters near it, and the depth limit, on decode and on encode.
pub(super) fn bound_cases(feature_id: &str, format: &str) -> Vec<Case> {
    let p = |id: &str| format!("{feature_id}/{id}");
    let string = |s: &str| Value::String(s.into());
    let chars = |max| Type::BoundedString { max };
    let u8s = |items: &[u8]| Value::Array(items.iter().copied().map(Value::U8).collect());
    let items = |of: Type, max| Type::BoundedArray {
        of: Box::new(of),
        max,
    };
    // `@max_depth = 3`: `struct Node { next: optional Node }`.
    let chain_type = || Type::Recursive {
        max_depth: 3,
        of: Box::new(Type::Struct {
            name: "Node".into(),
            fields: vec![("next".into(), Type::Option(Box::new(Type::SelfRef)))],
        }),
    };
    let chain = |n: usize| {
        (0..n).fold(Value::Option(None), |next, _| {
            Value::Option(Some(Box::new(Value::Struct {
                name: "Node".into(),
                fields: vec![("next".into(), next)],
            })))
        })
    };
    let chain = |n: usize| match chain(n) {
        Value::Option(Some(node)) => *node,
        _ => unreachable!("n > 0"),
    };
    // `@max_depth = 2`: `struct Tree { children: Tree[max 2] }`.
    let tree_type = || Type::Recursive {
        max_depth: 2,
        of: Box::new(Type::Struct {
            name: "Tree".into(),
            fields: vec![("children".into(), items(Type::SelfRef, 2))],
        }),
    };
    let tree = |children: Vec<Value>| Value::Struct {
        name: "Tree".into(),
        fields: vec![("children".into(), Value::Array(children))],
    };
    let leaf = || tree(vec![]);

    let read: Vec<(&str, Value, Type)> = vec![
        ("chars-empty", string(""), chars(4)),
        ("chars-at-max", string("abcd"), chars(4)),
        ("chars-over-max", string("abcde"), chars(4)),
        // 4 characters in 10 bytes: under by characters, over by bytes.
        ("chars-multibyte-at-max", string("é€😀a"), chars(4)),
        // 4 characters in 16 bytes, 4 × N: the most N characters take.
        ("chars-4-byte-at-max", string("😀😀😀😀"), chars(4)),
        // 5 characters in 10 bytes: over by characters, under 4 × N bytes.
        ("chars-multibyte-over-max", string("ééééé"), chars(4)),
        // 17 bytes: a length over 4 × N, refused before it's read.
        ("chars-bytes-over-4n", string("😀😀😀😀a"), chars(4)),
        ("chars-max-0", string(""), chars(0)),
        ("chars-max-0-over", string("a"), chars(0)),
        ("items-empty", u8s(&[]), items(Type::U8, 3)),
        ("items-at-max", u8s(&[1, 2, 3]), items(Type::U8, 3)),
        ("items-over-max", u8s(&[1, 2, 3, 4]), items(Type::U8, 3)),
        (
            "items-of-chars-at-max",
            Value::Array(vec![string("ab"), string("é😀")]),
            items(chars(2), 2),
        ),
        (
            "items-of-chars-over-max",
            Value::Array(vec![string("ab"), string("abc")]),
            items(chars(2), 2),
        ),
        ("depth-at-max", chain(3), chain_type()),
        ("depth-over-max", chain(4), chain_type()),
        ("depth-tree-at-max", tree(vec![leaf(), leaf()]), tree_type()),
        (
            "depth-tree-over-max",
            tree(vec![leaf(), tree(vec![leaf()])]),
            tree_type(),
        ),
    ];
    let mut cases: Vec<Case> = read
        .iter()
        .map(|(id, written, ty)| {
            decode_case(p(&format!("decode-{id}")), format, written, ty.clone())
        })
        .collect();
    for case in &cases {
        let over = case.id.contains("over");
        assert_eq!(
            case.expect == Expect::Reject,
            over,
            "reference case `{}`",
            case.id
        );
    }

    // The same values encoded as their types: within the bounds, the bytes;
    // over them, refused before anything is written.
    for (id, value, ty) in read {
        let id = p(&format!("encode-{id}"));
        let input = Input::Encode(EncodeIn {
            format: format.into(),
            ty: Some(ty),
            value,
        });
        cases.push(if id.contains("over") {
            reject(&id, input, &[])
        } else {
            exact(id, input)
        });
    }
    cases
}

/// What a newer or older peer wrote, read by today's types: the cases an
/// encoding profile's facts point to.
pub(super) fn decode_cases(feature_id: &str, format: &str) -> Vec<Case> {
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
    let bytes = |n: u8| Value::Tuple((1..=n).map(Value::U8).collect());
    let fixed = |len: usize| Type::Fixed {
        of: Box::new(Type::U8),
        len,
    };
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
        // The shapes beyond the scalars a generated type can have.
        decode_case(
            p("decode-u128"),
            format,
            &Value::U128(u128::MAX),
            Type::U128,
        ),
        decode_case(
            p("decode-i128"),
            format,
            &Value::I128(i128::MIN),
            Type::I128,
        ),
        decode_case(p("decode-unit"), format, &Value::Unit(()), Type::Unit),
        decode_case(p("decode-fixed-array"), format, &bytes(4), fixed(4)),
        decode_case(p("decode-fixed-array-33"), format, &bytes(33), fixed(33)),
        // A fixed array written one element short, or one too long.
        decode_case(p("decode-fixed-array-short"), format, &bytes(3), fixed(4)),
        decode_case(p("decode-fixed-array-long"), format, &bytes(5), fixed(4)),
        decode_case(
            p("decode-array-of-options"),
            format,
            &Value::Array(vec![
                Value::Option(Some(Box::new(Value::U16(1)))),
                Value::Option(None),
            ]),
            Type::Array(Box::new(Type::Option(Box::new(Type::U16)))),
        ),
    ]
}

/// Inputs the property tests and fuzzers found (`tests/properties.rs`,
/// `runtime/fuzz`), minimized, as `found-*` cases. See `runtime/fuzz/README.md`.
pub(super) fn found_cases(feature_id: &str, format: &str) -> Vec<Case> {
    let p = |id: &str| format!("{feature_id}/found-{id}");
    // A value that must read back as itself, not only as whatever the
    // reference happened to decode.
    let exact = |id: &str, written: Value, ty: Type| {
        let case = decode_case(p(id), format, &written, ty);
        let Input::Decode(DecodeIn { value, .. }) = &case.input else {
            unreachable!("`decode_case` makes `decode` cases");
        };
        assert_eq!(
            value.as_ref(),
            Some(&written),
            "`{}` doesn't read back",
            case.id
        );
        case
    };
    match format {
        // serde_json's default float parser can be one ulp off; the
        // runtime enables `float_roundtrip`, which reads every `f64` that
        // was written back exactly.
        FORMAT_JSON => vec![exact(
            "f64-exact-digits",
            Value::F64(4.520618856987477e-279),
            Type::F64,
        )],
        _ => vec![],
    }
}
