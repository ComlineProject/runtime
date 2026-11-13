// Generated from the catalog's CUE by `specs/check.sh`: don't edit by hand.

use crate::{
    Body, Encoding, EnumSize, EnumTag, Fact, Feature, Guarantee, Header, Kind, Origin, OptionSize, Scalars, Scope,
    SeqSize, Sizes, Status, StringSize, StructSize, Value, Width,
};

pub const SPEC_VERSION: &str = r#"0.11"#;

/// Every guarantee, in the catalog's order.
pub const GUARANTEES: &[Guarantee] = &[
    Guarantee {
        id: r#"schema_agreement"#,
        title: r#"schema agreement"#,
        description: r#"Both ends were built from compatible schemas, which the handshake checks."#,
        comes_from: Origin::Handshake,
        scopes: &[Scope::Package, Scope::Protocol],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Guarantee {
        id: r#"encoding_agreement"#,
        title: r#"encoding agreement"#,
        description: r#"Both ends use the same format and framing, which the handshake checks."#,
        comes_from: Origin::Handshake,
        scopes: &[Scope::Package, Scope::Protocol],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Guarantee {
        id: r#"encoding_conformance"#,
        title: r#"encoding conformance"#,
        description: r#"An implementation encodes what its name promises, proven by conformance vectors."#,
        comes_from: Origin::Encoding,
        scopes: &[Scope::Package, Scope::Protocol],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Guarantee {
        id: r#"version_honesty"#,
        title: r#"version honesty"#,
        description: r#"A version bump says what the change breaks, as the version comparison works it out."#,
        comes_from: Origin::Policy,
        scopes: &[Scope::Package, Scope::Protocol],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Guarantee {
        id: r#"bounded_size"#,
        title: r#"bounded size"#,
        description: r#"Every value has a maximum size, so a decoder refuses an oversized one before allocating it."#,
        comes_from: Origin::Type,
        scopes: &[Scope::Protocol, Scope::Function, Scope::Argument, Scope::Field, Scope::Type],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
];

/// Every fact, in the order profiles list them.
pub const FACTS: &[Fact] = &[
    Fact {
        id: r#"struct_fields"#,
        applies_to: Kind::Format,
        title: r#"struct fields"#,
        description: r#"How a struct's fields are written."#,
        values: &[Value { id: r#"by_position"#, meaning: r#"back to back, in declaration order"# }, Value { id: r#"by_name"#, meaning: r#"each keyed by its name"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Fact {
        id: r#"unknown_field"#,
        applies_to: Kind::Format,
        title: r#"unknown field"#,
        description: r#"What a reader does with a field its type doesn't have."#,
        values: &[Value { id: r#"rejected"#, meaning: r#"refuses the value"# }, Value { id: r#"ignored"#, meaning: r#"skips the field"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Fact {
        id: r#"missing_optional"#,
        applies_to: Kind::Format,
        title: r#"missing optional"#,
        description: r#"What a reader does when an optional field isn't there."#,
        values: &[Value { id: r#"rejected"#, meaning: r#"refuses the value"# }, Value { id: r#"absent"#, meaning: r#"reads it as absent"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Fact {
        id: r#"enum_variants"#,
        applies_to: Kind::Format,
        title: r#"enum variants"#,
        description: r#"How an enum's variant is written."#,
        values: &[Value { id: r#"by_name"#, meaning: r#"by its name"# }, Value { id: r#"by_index"#, meaning: r#"by its position in the declaration"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Fact {
        id: r#"unknown_variant"#,
        applies_to: Kind::Format,
        title: r#"unknown variant"#,
        description: r#"What a reader does with a variant its type doesn't have."#,
        values: &[Value { id: r#"rejected"#, meaning: r#"refuses the value"# }, Value { id: r#"tolerated"#, meaning: r#"reads it without failing"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Fact {
        id: r#"integer_width"#,
        applies_to: Kind::Format,
        title: r#"integer width"#,
        description: r#"Whether an integer reads into another width."#,
        values: &[Value { id: r#"by_value"#, meaning: r#"any width the value fits"# }, Value { id: r#"exact"#, meaning: r#"only the width it was written as"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Fact {
        id: r#"output"#,
        applies_to: Kind::Format,
        title: r#"output"#,
        description: r#"The grammar a format writes in, which a framing's `payload` must take."#,
        values: &[Value { id: r#"bytes"#, meaning: r#"bytes in no grammar a framing reads"# }, Value { id: r#"json"#, meaning: r#"JSON text"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.7"#),
    },
    Fact {
        id: r#"calls"#,
        applies_to: Kind::Framing,
        title: r#"calls"#,
        description: r#"How a request names the function it calls."#,
        values: &[Value { id: r#"by_index"#, meaning: r#"by the function's position"# }, Value { id: r#"by_name"#, meaning: r#"by the function's name"# }, Value { id: r#"by_id"#, meaning: r#"by the protocol's and function's wire numbers"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Fact {
        id: r#"directions"#,
        applies_to: Kind::Framing,
        title: r#"directions"#,
        description: r#"Whether a frame says if it's a request or a response."#,
        values: &[Value { id: r#"both"#, meaning: r#"it does, so calls can go both ways on one transport"# }, Value { id: r#"one"#, meaning: r#"it doesn't"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.10"#),
    },
    Fact {
        id: r#"errors"#,
        applies_to: Kind::Framing,
        title: r#"errors"#,
        description: r#"How an error response names its error."#,
        values: &[Value { id: r#"by_ordinal"#, meaning: r#"by its position in the declaration"# }, Value { id: r#"by_name"#, meaning: r#"by its name"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Fact {
        id: r#"payload"#,
        applies_to: Kind::Framing,
        title: r#"payload"#,
        description: r#"What a framing needs of a payload, which a format's `output` must fit."#,
        values: &[Value { id: r#"opaque"#, meaning: r#"any bytes, carried as they are"# }, Value { id: r#"json"#, meaning: r#"JSON text, written into the frame"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.7"#),
    },
    Fact {
        id: r#"protocol"#,
        applies_to: Kind::Framing,
        title: r#"protocol"#,
        description: r#"Whether a request's frame names the protocol it calls into."#,
        values: &[Value { id: r#"named"#, meaning: r#"it does, by number or wire id, so several protocols can share one transport"# }, Value { id: r#"unnamed"#, meaning: r#"it doesn't: a call names only its function"# }],
        establishes: None,
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.11"#),
    },
    Fact {
        id: r#"schema_agreement"#,
        applies_to: Kind::Handshake,
        title: r#"schema agreement"#,
        description: r#"Whether the handshake checks both ends were built from compatible schemas."#,
        values: &[Value { id: r#"true"#, meaning: r#"it does, and refuses a peer that wasn't"# }, Value { id: r#"false"#, meaning: r#"it doesn't"# }],
        establishes: Some(r#"schema_agreement"#),
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.2"#),
    },
    Fact {
        id: r#"encoding_agreement"#,
        applies_to: Kind::Handshake,
        title: r#"encoding agreement"#,
        description: r#"Whether the handshake checks both ends use the same format and framing."#,
        values: &[Value { id: r#"true"#, meaning: r#"it does, and refuses a peer that doesn't"# }, Value { id: r#"false"#, meaning: r#"it doesn't"# }],
        establishes: Some(r#"encoding_agreement"#),
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.2"#),
    },
];

const SIZES_PACKED_1: Sizes = Sizes {
    feature: r#"sizes-packed"#,
    scalars: Scalars {
        bool: Width { min: 1, max: 1, upper_bound: false, proven_by: &[r#"sizes-packed/bool"#] },
        u8: Width { min: 1, max: 1, upper_bound: false, proven_by: &[r#"sizes-packed/u8"#] },
        u16: Width { min: 2, max: 2, upper_bound: false, proven_by: &[r#"sizes-packed/u16"#] },
        u32: Width { min: 4, max: 4, upper_bound: false, proven_by: &[r#"sizes-packed/u32"#] },
        u64: Width { min: 8, max: 8, upper_bound: false, proven_by: &[r#"sizes-packed/u64"#] },
        u128: Width { min: 16, max: 16, upper_bound: false, proven_by: &[r#"sizes-packed/u128"#] },
        s8: Width { min: 1, max: 1, upper_bound: false, proven_by: &[r#"sizes-packed/i8"#] },
        s16: Width { min: 2, max: 2, upper_bound: false, proven_by: &[r#"sizes-packed/i16"#] },
        s32: Width { min: 4, max: 4, upper_bound: false, proven_by: &[r#"sizes-packed/i32"#] },
        s64: Width { min: 8, max: 8, upper_bound: false, proven_by: &[r#"sizes-packed/i64"#] },
        s128: Width { min: 16, max: 16, upper_bound: false, proven_by: &[r#"sizes-packed/i128"#] },
        f32: Width { min: 4, max: 4, upper_bound: false, proven_by: &[r#"sizes-packed/f32"#] },
        f64: Width { min: 8, max: 8, upper_bound: false, proven_by: &[r#"sizes-packed/f64"#] },
        char: Width { min: 1, max: 4, upper_bound: false, proven_by: &[r#"sizes-packed/char"#] },
        unit: Width { min: 0, max: 0, upper_bound: false, proven_by: &[r#"sizes-packed/unit"#] },
    },
    string: StringSize {
        header: Header::VarintLeb128,
        body: Body::Utf8,
        proven_by: &[r#"sizes-packed/string-unbounded"#, r#"sizes-packed/string-max-0"#, r#"sizes-packed/string-max-1"#, r#"sizes-packed/string-max-3"#, r#"sizes-packed/string-max-7"#, r#"sizes-packed/string-max-8"#, r#"sizes-packed/string-max-31"#, r#"sizes-packed/string-max-32"#, r#"sizes-packed/string-max-63"#, r#"sizes-packed/string-max-64"#, r#"sizes-packed/string-max-4095"#, r#"sizes-packed/string-max-4096"#, r#"sizes-packed/string-fixed-3"#, r#"sizes-packed/string-fixed-8"#, r#"sizes-packed/string-fixed-32"#],
    },
    array: SeqSize { header: Header::VarintLeb128, proven_by: &[r#"sizes-packed/array-unbounded"#, r#"sizes-packed/array-unbounded-strings"#, r#"sizes-packed/array-max-3"#, r#"sizes-packed/array-max-127"#, r#"sizes-packed/array-max-128"#, r#"sizes-packed/array-max-2-of-strings"#, r#"sizes-packed/array-max-3-of-options"#, r#"sizes-packed/recursive-tree-2"#] },
    fixed_array: SeqSize { header: Header::None, proven_by: &[r#"sizes-packed/fixed-4"#, r#"sizes-packed/fixed-1"#, r#"sizes-packed/fixed-2-of-strings"#, r#"sizes-packed/fixed-3-of-options"#] },
    option: OptionSize { none: 1, some: 1, proven_by: &[r#"sizes-packed/option-u8"#, r#"sizes-packed/option-bool"#, r#"sizes-packed/option-string"#, r#"sizes-packed/option-max-3-chars"#, r#"sizes-packed/option-struct"#, r#"sizes-packed/recursive-chain-3"#] },
    struct_: StructSize { header: Header::None, key_extra: None, proven_by: &[r#"sizes-packed/struct-point"#, r#"sizes-packed/struct-empty"#, r#"sizes-packed/struct-mixed"#, r#"sizes-packed/struct-nested"#, r#"sizes-packed/struct-15-fields"#, r#"sizes-packed/struct-16-fields"#, r#"sizes-packed/struct-long-names"#, r#"sizes-packed/recursive-chain-3"#, r#"sizes-packed/recursive-tree-2"#] },
    enum_: EnumSize { tag: EnumTag::VarintIndex, proven_by: &[r#"sizes-packed/enum-color"#, r#"sizes-packed/enum-single"#, r#"sizes-packed/enum-129-variants"#, r#"sizes-packed/enum-long-names"#] },
};

const SIZES_JSON_1: Sizes = Sizes {
    feature: r#"sizes-json"#,
    scalars: Scalars {
        bool: Width { min: 4, max: 5, upper_bound: false, proven_by: &[r#"sizes-json/bool"#] },
        u8: Width { min: 1, max: 3, upper_bound: false, proven_by: &[r#"sizes-json/u8"#] },
        u16: Width { min: 1, max: 5, upper_bound: false, proven_by: &[r#"sizes-json/u16"#] },
        u32: Width { min: 1, max: 10, upper_bound: false, proven_by: &[r#"sizes-json/u32"#] },
        u64: Width { min: 1, max: 20, upper_bound: false, proven_by: &[r#"sizes-json/u64"#] },
        u128: Width { min: 1, max: 39, upper_bound: false, proven_by: &[r#"sizes-json/u128"#] },
        s8: Width { min: 1, max: 4, upper_bound: false, proven_by: &[r#"sizes-json/i8"#] },
        s16: Width { min: 1, max: 6, upper_bound: false, proven_by: &[r#"sizes-json/i16"#] },
        s32: Width { min: 1, max: 11, upper_bound: false, proven_by: &[r#"sizes-json/i32"#] },
        s64: Width { min: 1, max: 20, upper_bound: false, proven_by: &[r#"sizes-json/i64"#] },
        s128: Width { min: 1, max: 40, upper_bound: false, proven_by: &[r#"sizes-json/i128"#] },
        f32: Width { min: 3, max: 16, upper_bound: false, proven_by: &[r#"sizes-json/f32"#] },
        f64: Width { min: 3, max: 24, upper_bound: false, proven_by: &[r#"sizes-json/f64"#] },
        char: Width { min: 3, max: 8, upper_bound: false, proven_by: &[r#"sizes-json/char"#] },
        unit: Width { min: 4, max: 4, upper_bound: false, proven_by: &[r#"sizes-json/unit"#] },
    },
    string: StringSize {
        header: Header::Delimited { before: 1, after: 1, between: 0 },
        body: Body::JsonEscaped,
        proven_by: &[r#"sizes-json/string-unbounded"#, r#"sizes-json/string-max-0"#, r#"sizes-json/string-max-1"#, r#"sizes-json/string-max-3"#, r#"sizes-json/string-max-7"#, r#"sizes-json/string-max-8"#, r#"sizes-json/string-max-31"#, r#"sizes-json/string-max-32"#, r#"sizes-json/string-max-63"#, r#"sizes-json/string-max-64"#, r#"sizes-json/string-fixed-3"#, r#"sizes-json/string-fixed-8"#, r#"sizes-json/string-fixed-32"#],
    },
    array: SeqSize { header: Header::Delimited { before: 1, after: 1, between: 1 }, proven_by: &[r#"sizes-json/array-unbounded"#, r#"sizes-json/array-unbounded-strings"#, r#"sizes-json/array-max-0"#, r#"sizes-json/array-max-1"#, r#"sizes-json/array-max-3"#, r#"sizes-json/array-max-2-of-strings"#, r#"sizes-json/array-max-3-of-options"#, r#"sizes-json/recursive-tree-2"#] },
    fixed_array: SeqSize { header: Header::Delimited { before: 1, after: 1, between: 1 }, proven_by: &[r#"sizes-json/fixed-4"#, r#"sizes-json/fixed-1"#, r#"sizes-json/fixed-2-of-strings"#, r#"sizes-json/fixed-3-of-options"#] },
    option: OptionSize { none: 4, some: 0, proven_by: &[r#"sizes-json/option-u8"#, r#"sizes-json/option-bool"#, r#"sizes-json/option-string"#, r#"sizes-json/option-max-3-chars"#, r#"sizes-json/option-struct"#, r#"sizes-json/recursive-chain-3"#] },
    struct_: StructSize { header: Header::Delimited { before: 1, after: 1, between: 1 }, key_extra: Some(3), proven_by: &[r#"sizes-json/struct-point"#, r#"sizes-json/struct-empty"#, r#"sizes-json/struct-mixed"#, r#"sizes-json/struct-nested"#, r#"sizes-json/struct-15-fields"#, r#"sizes-json/struct-16-fields"#, r#"sizes-json/struct-long-names"#, r#"sizes-json/recursive-chain-3"#, r#"sizes-json/recursive-tree-2"#] },
    enum_: EnumSize { tag: EnumTag::Name(Header::Delimited { before: 1, after: 1, between: 0 }), proven_by: &[r#"sizes-json/enum-color"#, r#"sizes-json/enum-single"#, r#"sizes-json/enum-129-variants"#, r#"sizes-json/enum-long-names"#] },
};

const SIZES_MSGPACK_1: Sizes = Sizes {
    feature: r#"sizes-msgpack"#,
    scalars: Scalars {
        bool: Width { min: 1, max: 1, upper_bound: false, proven_by: &[r#"sizes-msgpack/bool"#] },
        u8: Width { min: 1, max: 2, upper_bound: false, proven_by: &[r#"sizes-msgpack/u8"#] },
        u16: Width { min: 1, max: 3, upper_bound: false, proven_by: &[r#"sizes-msgpack/u16"#] },
        u32: Width { min: 1, max: 5, upper_bound: false, proven_by: &[r#"sizes-msgpack/u32"#] },
        u64: Width { min: 1, max: 9, upper_bound: false, proven_by: &[r#"sizes-msgpack/u64"#] },
        u128: Width { min: 18, max: 18, upper_bound: false, proven_by: &[r#"sizes-msgpack/u128"#] },
        s8: Width { min: 1, max: 2, upper_bound: false, proven_by: &[r#"sizes-msgpack/i8"#] },
        s16: Width { min: 1, max: 3, upper_bound: false, proven_by: &[r#"sizes-msgpack/i16"#] },
        s32: Width { min: 1, max: 5, upper_bound: false, proven_by: &[r#"sizes-msgpack/i32"#] },
        s64: Width { min: 1, max: 9, upper_bound: false, proven_by: &[r#"sizes-msgpack/i64"#] },
        s128: Width { min: 18, max: 18, upper_bound: false, proven_by: &[r#"sizes-msgpack/i128"#] },
        f32: Width { min: 5, max: 5, upper_bound: false, proven_by: &[r#"sizes-msgpack/f32"#] },
        f64: Width { min: 9, max: 9, upper_bound: false, proven_by: &[r#"sizes-msgpack/f64"#] },
        char: Width { min: 2, max: 5, upper_bound: false, proven_by: &[r#"sizes-msgpack/char"#] },
        unit: Width { min: 1, max: 1, upper_bound: false, proven_by: &[r#"sizes-msgpack/unit"#] },
    },
    string: StringSize {
        header: Header::MsgpackStr,
        body: Body::Utf8,
        proven_by: &[r#"sizes-msgpack/string-unbounded"#, r#"sizes-msgpack/string-max-0"#, r#"sizes-msgpack/string-max-1"#, r#"sizes-msgpack/string-max-3"#, r#"sizes-msgpack/string-max-7"#, r#"sizes-msgpack/string-max-8"#, r#"sizes-msgpack/string-max-31"#, r#"sizes-msgpack/string-max-32"#, r#"sizes-msgpack/string-max-63"#, r#"sizes-msgpack/string-max-64"#, r#"sizes-msgpack/string-max-16384"#, r#"sizes-msgpack/string-fixed-3"#, r#"sizes-msgpack/string-fixed-8"#, r#"sizes-msgpack/string-fixed-32"#],
    },
    array: SeqSize { header: Header::MsgpackArray, proven_by: &[r#"sizes-msgpack/array-unbounded"#, r#"sizes-msgpack/array-unbounded-strings"#, r#"sizes-msgpack/array-max-3"#, r#"sizes-msgpack/array-max-15"#, r#"sizes-msgpack/array-max-16"#, r#"sizes-msgpack/array-max-2-of-strings"#, r#"sizes-msgpack/array-max-3-of-options"#, r#"sizes-msgpack/recursive-tree-2"#] },
    fixed_array: SeqSize { header: Header::MsgpackArray, proven_by: &[r#"sizes-msgpack/fixed-4"#, r#"sizes-msgpack/fixed-15"#, r#"sizes-msgpack/fixed-16"#, r#"sizes-msgpack/fixed-1"#, r#"sizes-msgpack/fixed-2-of-strings"#, r#"sizes-msgpack/fixed-3-of-options"#] },
    option: OptionSize { none: 1, some: 0, proven_by: &[r#"sizes-msgpack/option-u8"#, r#"sizes-msgpack/option-bool"#, r#"sizes-msgpack/option-string"#, r#"sizes-msgpack/option-max-3-chars"#, r#"sizes-msgpack/option-struct"#, r#"sizes-msgpack/recursive-chain-3"#] },
    struct_: StructSize { header: Header::MsgpackArray, key_extra: None, proven_by: &[r#"sizes-msgpack/struct-point"#, r#"sizes-msgpack/struct-empty"#, r#"sizes-msgpack/struct-mixed"#, r#"sizes-msgpack/struct-nested"#, r#"sizes-msgpack/struct-15-fields"#, r#"sizes-msgpack/struct-16-fields"#, r#"sizes-msgpack/struct-long-names"#, r#"sizes-msgpack/recursive-chain-3"#, r#"sizes-msgpack/recursive-tree-2"#] },
    enum_: EnumSize { tag: EnumTag::Name(Header::MsgpackStr), proven_by: &[r#"sizes-msgpack/enum-color"#, r#"sizes-msgpack/enum-single"#, r#"sizes-msgpack/enum-129-variants"#, r#"sizes-msgpack/enum-long-names"#] },
};

/// Every official encoding, by its wire name.
pub const ENCODINGS: &[Encoding] = &[
    Encoding {
        id: r#"packed@1"#,
        kind: Kind::Format,
        title: r#"packed"#,
        description: r#"Fixed-width fields back to back, varint lengths, no tags."#,
        aliases: &[],
        frozen_as: None,
        profile: true,
        feature: Some(r#"format-packed"#),
        sizes: Some(&SIZES_PACKED_1),
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Encoding {
        id: r#"json@1"#,
        kind: Kind::Format,
        title: r#"JSON"#,
        description: r#"Values as UTF-8 JSON: a struct as an object."#,
        aliases: &[],
        frozen_as: None,
        profile: true,
        feature: Some(r#"format-json"#),
        sizes: Some(&SIZES_JSON_1),
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Encoding {
        id: r#"msgpack@1"#,
        kind: Kind::Format,
        title: r#"MessagePack"#,
        description: r#"Values as MessagePack: a struct as an array."#,
        aliases: &[],
        frozen_as: None,
        profile: true,
        feature: Some(r#"format-msgpack"#),
        sizes: Some(&SIZES_MSGPACK_1),
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Encoding {
        id: r#"jsonrpc@1"#,
        kind: Kind::Framing,
        title: r#"JSON-RPC"#,
        description: r#"JSON-RPC 2.0 requests and responses."#,
        aliases: &[r#"jsonrpc"#, r#"json-rpc"#, r#"jsonrpc-2.0"#],
        frozen_as: Some(r#"jsonrpc"#),
        profile: true,
        feature: Some(r#"framing-jsonrpc"#),
        sizes: None,
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Encoding {
        id: r#"datagram@1"#,
        kind: Kind::Framing,
        title: r#"datagram"#,
        description: r#"Comline's own binary framing of requests and responses."#,
        aliases: &[r#"datagram"#, r#"comline.datagram"#],
        frozen_as: Some(r#"datagram"#),
        profile: true,
        feature: Some(r#"framing-datagram"#),
        sizes: None,
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Encoding {
        id: r#"datagram@2"#,
        kind: Kind::Framing,
        title: r#"datagram@2"#,
        description: r#"The datagram framing with a request/reply kind and a protocol number, for calls both ways and several protocols on one connection."#,
        aliases: &[],
        frozen_as: None,
        profile: true,
        feature: Some(r#"framing-datagram2"#),
        sizes: None,
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Encoding {
        id: r#"datagram@3"#,
        kind: Kind::Framing,
        title: r#"datagram@3"#,
        description: r#"The datagram framing that addresses a call by its protocol's and function's wire numbers."#,
        aliases: &[],
        frozen_as: None,
        profile: true,
        feature: Some(r#"framing-datagram3"#),
        sizes: None,
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.4"#),
    },
    Encoding {
        id: r#"comline@1"#,
        kind: Kind::Handshake,
        title: r#"comline@1"#,
        description: r#"The fixed frame each end sends first: magic, version, IR hash, wire format, framing and capabilities."#,
        aliases: &[],
        frozen_as: None,
        profile: true,
        feature: Some(r#"handshake"#),
        sizes: None,
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Encoding {
        id: r#"comline@2"#,
        kind: Kind::Handshake,
        title: r#"comline@2"#,
        description: r#"The compatibility-checking handshake: lines, shape hashes and wire numbers instead of an exact IR match."#,
        aliases: &[],
        frozen_as: None,
        profile: true,
        feature: Some(r#"handshake-comline2"#),
        sizes: None,
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.4"#),
    },
    Encoding {
        id: r#"none"#,
        kind: Kind::Handshake,
        title: r#"no handshake"#,
        description: r#"No frame and nothing checked. Unversioned, since there is nothing to evolve; it establishes neither agreement."#,
        aliases: &[],
        frozen_as: None,
        profile: false,
        feature: None,
        sizes: None,
        tags: &[r#"official"#],
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
];

/// Every wire feature, in the order `features.json` lists them.
pub const FEATURES: &[Feature] = &[
    Feature {
        id: r#"name-hash"#,
        title: r#"name hash"#,
        description: r#"FNV-1a 64 of a wire format or framing name, as the handshake carries it"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"handshake"#,
        title: r#"handshake"#,
        description: r#"the fixed 31-byte frame each side sends first: magic, version, IR hash, wire format, framing, capabilities"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"envelope"#,
        title: r#"envelope"#,
        description: r#"a response's outcome: ok with a payload, an error ordinal with its body, or a provider fault"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"framing-datagram"#,
        title: r#"datagram framing"#,
        description: r#"Comline's own binary framing of requests and responses"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"framing-datagram2"#,
        title: r#"datagram@2 framing"#,
        description: r#"the datagram framing with a request/reply kind and a protocol number, for calls in both directions and several protocols on one connection"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"framing-datagram3"#,
        title: r#"datagram@3 framing"#,
        description: r#"the datagram framing that addresses a call by its protocol's and function's wire numbers, both varints (calls = by_id); replies as datagram@2"#,
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.4"#),
    },
    Feature {
        id: r#"handshake-comline2"#,
        title: r#"comline@2 handshake"#,
        description: r#"the compatibility-checking handshake: its hello, answer and verdict messages, and its negotiation steps under every parameter, with every refusal reason"#,
        status: Status::Stable,
        replaced_by: None,
        since: Some(r#"0.4"#),
    },
    Feature {
        id: r#"stream"#,
        title: r#"stream framing"#,
        description: r#"frames on a byte stream (TCP, a pipe): each behind a u32 little-endian length prefix, refused over the limit, by default 16 MiB (16777216 bytes) in every runtime"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"framing-jsonrpc"#,
        title: r#"JSON-RPC framing"#,
        description: r#"JSON-RPC 2.0 requests and responses, a fault as -32603"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"format-json"#,
        title: r#"JSON wire format"#,
        description: r#"values as UTF-8 JSON: a struct as an object"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"format-msgpack"#,
        title: r#"MessagePack wire format"#,
        description: r#"values as MessagePack: a struct as an array"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"format-packed"#,
        title: r#"packed wire format"#,
        description: r#"values as packed@1: fixed-width fields back to back, varint lengths, no tags"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"sizes-packed"#,
        title: r#"packed sizes"#,
        description: r#"how many bytes each kind of value takes in packed@1, each rule proven by values written to exactly its minimum and maximum"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"sizes-msgpack"#,
        title: r#"MessagePack sizes"#,
        description: r#"how many bytes each kind of value takes in msgpack@1, each rule proven by values written to exactly its minimum and maximum"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
    Feature {
        id: r#"sizes-json"#,
        title: r#"JSON sizes"#,
        description: r#"how many bytes each kind of value takes in json@1, each rule proven by values written to exactly its minimum and maximum, or by a declared upper bound"#,
        status: Status::Stable,
        replaced_by: None,
        since: None,
    },
];

/// Guarantee ids.
pub mod guarantee {
    pub const SCHEMA_AGREEMENT: &str = r#"schema_agreement"#;
    pub const ENCODING_AGREEMENT: &str = r#"encoding_agreement"#;
    pub const ENCODING_CONFORMANCE: &str = r#"encoding_conformance"#;
    pub const VERSION_HONESTY: &str = r#"version_honesty"#;
    pub const BOUNDED_SIZE: &str = r#"bounded_size"#;
}

/// Fact ids.
pub mod fact {
    pub const STRUCT_FIELDS: &str = r#"struct_fields"#;
    pub const UNKNOWN_FIELD: &str = r#"unknown_field"#;
    pub const MISSING_OPTIONAL: &str = r#"missing_optional"#;
    pub const ENUM_VARIANTS: &str = r#"enum_variants"#;
    pub const UNKNOWN_VARIANT: &str = r#"unknown_variant"#;
    pub const INTEGER_WIDTH: &str = r#"integer_width"#;
    pub const OUTPUT: &str = r#"output"#;
    pub const CALLS: &str = r#"calls"#;
    pub const DIRECTIONS: &str = r#"directions"#;
    pub const ERRORS: &str = r#"errors"#;
    pub const PAYLOAD: &str = r#"payload"#;
    pub const PROTOCOL: &str = r#"protocol"#;
    pub const SCHEMA_AGREEMENT: &str = r#"schema_agreement"#;
    pub const ENCODING_AGREEMENT: &str = r#"encoding_agreement"#;
}

/// Encoding wire names.
pub mod encoding {
    pub const PACKED_1: &str = r#"packed@1"#;
    pub const JSON_1: &str = r#"json@1"#;
    pub const MSGPACK_1: &str = r#"msgpack@1"#;
    pub const JSONRPC_1: &str = r#"jsonrpc@1"#;
    pub const DATAGRAM_1: &str = r#"datagram@1"#;
    pub const DATAGRAM_2: &str = r#"datagram@2"#;
    pub const DATAGRAM_3: &str = r#"datagram@3"#;
    pub const COMLINE_1: &str = r#"comline@1"#;
    pub const COMLINE_2: &str = r#"comline@2"#;
    pub const NONE: &str = r#"none"#;
}

/// Feature ids.
pub mod feature {
    pub const NAME_HASH: &str = r#"name-hash"#;
    pub const HANDSHAKE: &str = r#"handshake"#;
    pub const ENVELOPE: &str = r#"envelope"#;
    pub const FRAMING_DATAGRAM: &str = r#"framing-datagram"#;
    pub const FRAMING_DATAGRAM2: &str = r#"framing-datagram2"#;
    pub const FRAMING_DATAGRAM3: &str = r#"framing-datagram3"#;
    pub const HANDSHAKE_COMLINE2: &str = r#"handshake-comline2"#;
    pub const STREAM: &str = r#"stream"#;
    pub const FRAMING_JSONRPC: &str = r#"framing-jsonrpc"#;
    pub const FORMAT_JSON: &str = r#"format-json"#;
    pub const FORMAT_MSGPACK: &str = r#"format-msgpack"#;
    pub const FORMAT_PACKED: &str = r#"format-packed"#;
    pub const SIZES_PACKED: &str = r#"sizes-packed"#;
    pub const SIZES_MSGPACK: &str = r#"sizes-msgpack"#;
    pub const SIZES_JSON: &str = r#"sizes-json"#;
}

/// The catalog's sets.
pub mod sets {
    pub const GUARANTEES: &[&str] = &[r#"schema_agreement"#, r#"encoding_agreement"#, r#"encoding_conformance"#, r#"version_honesty"#, r#"bounded_size"#];
    pub const SCOPES: &[&str] = &[r#"package"#, r#"protocol"#, r#"function"#, r#"argument"#, r#"field"#, r#"type"#];
    pub const FORMATS: &[&str] = &[r#"packed@1"#, r#"json@1"#, r#"msgpack@1"#];
    pub const FRAMINGS: &[&str] = &[r#"jsonrpc@1"#, r#"datagram@1"#, r#"datagram@2"#, r#"datagram@3"#];
    pub const HANDSHAKES: &[&str] = &[r#"comline@1"#, r#"comline@2"#, r#"none"#];
    pub const OFFICIAL: &[&str] = &[r#"packed@1"#, r#"json@1"#, r#"msgpack@1"#, r#"jsonrpc@1"#, r#"datagram@1"#, r#"datagram@2"#, r#"datagram@3"#, r#"comline@1"#, r#"comline@2"#, r#"none"#];
    pub const PROFILED: &[&str] = &[r#"packed@1"#, r#"json@1"#, r#"msgpack@1"#, r#"jsonrpc@1"#, r#"datagram@1"#, r#"datagram@2"#, r#"datagram@3"#, r#"comline@1"#, r#"comline@2"#];
    pub const FORMAT_FACTS: &[&str] = &[r#"struct_fields"#, r#"unknown_field"#, r#"missing_optional"#, r#"enum_variants"#, r#"unknown_variant"#, r#"integer_width"#, r#"output"#];
    pub const FRAMING_FACTS: &[&str] = &[r#"calls"#, r#"directions"#, r#"errors"#, r#"payload"#, r#"protocol"#];
    pub const HANDSHAKE_FACTS: &[&str] = &[r#"schema_agreement"#, r#"encoding_agreement"#];
    pub const FEATURES: &[&str] = &[r#"name-hash"#, r#"handshake"#, r#"envelope"#, r#"framing-datagram"#, r#"framing-datagram2"#, r#"framing-datagram3"#, r#"handshake-comline2"#, r#"stream"#, r#"framing-jsonrpc"#, r#"format-json"#, r#"format-msgpack"#, r#"format-packed"#, r#"sizes-packed"#, r#"sizes-msgpack"#, r#"sizes-json"#];
    /// Every alias, with the wire name it resolves to.
    pub const ALIASES: &[(&str, &str)] = &[(r#"jsonrpc"#, r#"jsonrpc@1"#), (r#"json-rpc"#, r#"jsonrpc@1"#), (r#"jsonrpc-2.0"#, r#"jsonrpc@1"#), (r#"datagram"#, r#"datagram@1"#), (r#"comline.datagram"#, r#"datagram@1"#)];
}

