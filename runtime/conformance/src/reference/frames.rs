// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use comline_runtime::contract::stream::{DEFAULT_MAX_FRAME, PREFIX_LEN};
use comline_runtime::contract::{FORMAT_JSON, FORMAT_PACKED};

use crate::{
    EncodeIn, Framed, Hex, Input, Reply, RequestIn, ResponseIn, StreamPrefixIn, StreamSplit,
    StreamSplitIn, Value,
};

pub(super) fn request(
    id: String,
    framing: &str,
    call_id: u16,
    method: &str,
    request_id: u64,
    params: &Value,
) -> (String, Input) {
    let input = RequestIn {
        framing: framing.into(),
        protocol: None,
        call_id,
        method: method.into(),
        request_id,
        one_way: false,
        format: FORMAT_JSON.into(),
        params: params.clone(),
    };
    (id, Input::Request(Framed::Full(input)))
}

/// The same call with its params in `format`, for a framing that carries them
/// as bytes.
pub(super) fn with_format((id, mut input): (String, Input), format: &str) -> (String, Input) {
    if let Input::Request(Framed::Full(i)) = &mut input {
        i.format = format.into();
    }
    (id, input)
}

pub(super) fn response(
    id: String,
    framing: &str,
    request_id: u64,
    reply: Reply,
) -> (String, Input) {
    let input = ResponseIn {
        framing: framing.into(),
        protocol: None,
        request_id,
        reply,
    };
    (id, Input::Response(Framed::Full(input)))
}

/// Sample values for a wire format: each scalar at its edges, then the
/// shapes generated types take.
pub(super) fn format_cases(feature_id: &str, format: &str) -> Vec<(String, Input)> {
    let point = |x: i32, y: i32| Value::Struct {
        name: "Point".into(),
        fields: vec![("x".into(), Value::I32(x)), ("y".into(), Value::I32(y))],
    };
    let samples: Vec<(&str, Value)> = vec![
        ("bool-true", Value::Bool(true)),
        ("bool-false", Value::Bool(false)),
        ("u8-max", Value::U8(u8::MAX)),
        ("u16-max", Value::U16(u16::MAX)),
        ("u32-max", Value::U32(u32::MAX)),
        ("u64-small", Value::U64(7)),
        ("u64-above-2-53", Value::U64((1 << 53) + 1)),
        ("u64-max", Value::U64(u64::MAX)),
        ("i8-min", Value::I8(i8::MIN)),
        ("i16-min", Value::I16(i16::MIN)),
        ("i32-min", Value::I32(i32::MIN)),
        ("i64-min", Value::I64(i64::MIN)),
        ("f64-fraction", Value::F64(-0.25)),
        ("f64-integral", Value::F64(1.0)),
        ("string-empty", Value::String(String::new())),
        (
            "string-unicode",
            Value::String("héllo \"wörld\" ✓\n".into()),
        ),
        ("array-empty", Value::Array(vec![])),
        (
            "array-u8",
            Value::Array(vec![Value::U8(1), Value::U8(2), Value::U8(3)]),
        ),
        ("option-none", Value::Option(None)),
        ("option-some", Value::Option(Some(Box::new(Value::U32(5))))),
        ("struct", point(1, -2)),
        (
            "struct-nested",
            Value::Struct {
                name: "Line".into(),
                fields: vec![
                    ("from".into(), point(0, 0)),
                    ("to".into(), point(3, 4)),
                    (
                        "label".into(),
                        Value::Option(Some(Box::new(Value::String("diag".into())))),
                    ),
                    ("tags".into(), Value::Array(vec![Value::String("a".into())])),
                ],
            },
        ),
        (
            "struct-optional-none",
            Value::Struct {
                name: "Tagged".into(),
                fields: vec![
                    ("id".into(), Value::U32(1)),
                    ("label".into(), Value::Option(None)),
                ],
            },
        ),
        (
            "enum-unit-variant",
            Value::UnitVariant {
                enum_name: "Color".into(),
                index: 1,
                variant: "Green".into(),
            },
        ),
        ("u128-max", Value::U128(u128::MAX)),
        ("i128-min", Value::I128(i128::MIN)),
        (
            "tuple-u8",
            Value::Tuple(vec![Value::U8(1), Value::U8(2), Value::U8(3), Value::U8(4)]),
        ),
        // Longer than 32: generated Rust needs `serde_big_array` for it.
        ("tuple-u8-33", Value::Tuple(vec![Value::U8(7); 33])),
        (
            "array-of-options",
            Value::Array(vec![
                Value::Option(Some(Box::new(Value::U16(1)))),
                Value::Option(None),
            ]),
        ),
        ("unit", Value::Unit(())),
        // A single `char`, 1 to 4 bytes of UTF-8: raw in `packed@1`, a
        // one-character string in `msgpack@1` and `json@1`.
        ("char-ascii", Value::Char('a')),
        ("char-2-byte", Value::Char('é')),
        ("char-3-byte", Value::Char('€')),
        ("char-4-byte", Value::Char('😀')),
    ];
    samples
        .into_iter()
        .map(|(id, value)| {
            let input = EncodeIn {
                format: format.into(),
                ty: None,
                value,
            };
            (format!("{feature_id}/{id}"), Input::Encode(input))
        })
        .collect()
}

/// Shapes only `packed@1` vectors carry: widths the other formats don't
/// keep apart (`f32`), a fixed-size array, and the proposal's example.
pub(super) fn packed_cases() -> Vec<(String, Input)> {
    let class = |index: u32, variant: &str| Value::UnitVariant {
        enum_name: "Class".into(),
        index,
        variant: variant.into(),
    };
    let samples: Vec<(&str, Value)> = vec![
        ("f32", Value::F32(1.5)),
        ("u16-small", Value::U16(1)),
        (
            "tuple-f32",
            Value::Tuple(vec![Value::F32(1.0), Value::F32(2.0)]),
        ),
        // 300 elements: a count that takes two varint bytes.
        (
            "array-two-byte-count",
            Value::Array(vec![Value::U8(0); 300]),
        ),
        ("enum-first-variant", class(0, "Warrior")),
        (
            "struct-player",
            Value::Struct {
                name: "Player".into(),
                fields: vec![
                    ("id".into(), Value::U32(7)),
                    ("hp".into(), Value::U16(100)),
                    ("name".into(), Value::String("ab".into())),
                    ("guild".into(), Value::Option(None)),
                    ("class".into(), class(1, "Mage")),
                    (
                        "pos".into(),
                        Value::Tuple(vec![Value::F32(1.0), Value::F32(2.0)]),
                    ),
                ],
            },
        ),
    ];
    samples
        .into_iter()
        .map(|(id, value)| {
            let input = EncodeIn {
                format: FORMAT_PACKED.into(),
                ty: None,
                value,
            };
            (format!("format-packed/{id}"), Input::Encode(input))
        })
        .collect()
}

// --- Decoding what another version wrote ---

/// Stream framing (`contract::stream`): length prefixes, and streams split
/// into frames, at the default limit and at a small one, on both sides of
/// each.
pub(super) fn stream_cases() -> Vec<(String, Input)> {
    let default = DEFAULT_MAX_FRAME as u64;
    let ceiling = u64::from(u32::MAX);
    let prefix = |id: &str, len: u64, max: u64| {
        (
            format!("stream/{id}"),
            Input::StreamPrefix(StreamPrefixIn { len, max }),
        )
    };
    let split = |id: &str, max: u64, frames: &[&[u8]], rest: &[u8], needed: u64| {
        let frames: Vec<Hex> = frames.iter().map(|f| Hex(f.to_vec())).collect();
        (
            format!("stream/{id}"),
            Input::StreamSplit(StreamSplit::Frames(StreamSplitIn {
                max,
                frames,
                rest: Hex(rest.to_vec()),
                needed,
            })),
        )
    };
    let at_default_max = (DEFAULT_MAX_FRAME as u32).to_le_bytes();
    vec![
        prefix("prefix-zero", 0, default),
        prefix("prefix-small", 3, default),
        prefix("prefix-le", 0x0001_0203, default),
        prefix("prefix-default-max", default, default),
        prefix("prefix-small-max", 2, 2),
        prefix("prefix-u32-max", ceiling, ceiling),
        split("split-empty", default, &[], &[], 0),
        split("split-partial-prefix-1", default, &[], &[3], 0),
        split("split-partial-prefix-3", default, &[], &[3, 0, 0], 0),
        split("split-prefix-only", default, &[], &[3, 0, 0, 0], 7),
        split("split-partial-body", default, &[], &[3, 0, 0, 0, b'a'], 7),
        split("split-one-frame", default, &[b"abc"], &[], 0),
        split("split-zero-length", default, &[b""], &[], 0),
        split("split-two-frames", default, &[b"ab", b"c"], &[], 0),
        split("split-frame-then-partial", default, &[b"ab"], &[1, 0], 0),
        split("split-small-max", 2, &[b"ab", b""], &[2, 0, 0, 0, b'c'], 6),
        split(
            "split-default-max-prefix",
            default,
            &[],
            &at_default_max,
            PREFIX_LEN as u64 + default,
        ),
    ]
}
