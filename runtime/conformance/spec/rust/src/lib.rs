// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Comline specification's catalog of shared names, as Rust constants:
//! the guarantees, the facts and the values each may take, the official
//! encodings with their aliases and their size rules, and the wire features,
//! with the sets they form. `catalog/` in the specification is the source; [`catalog`]'s data is
//! generated from it by `specs/check.sh`, and CI fails when it's stale.
//!
//! The ids are published names: they never change. A rename adds an entry and
//! deprecates the old one ([`Status::Deprecated`], `replaced_by`).

#![no_std]

mod catalog;

pub use catalog::{
    encoding, fact, feature, guarantee, sets, ENCODINGS, FACTS, FEATURES, GUARANTEES,
    SPEC_VERSION,
};

/// What a fact applies to, and what kind of encoding a name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Format,
    Framing,
    Handshake,
}

impl Kind {
    /// As the catalog and profiles write it: `format`, `framing`, `handshake`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Kind::Format => "format",
            Kind::Framing => "framing",
            Kind::Handshake => "handshake",
        }
    }
}

/// What gives a guarantee up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The handshake.
    Handshake,
    /// The format and framing.
    Encoding,
    /// A handshake policy.
    Policy,
    /// A type.
    Type,
}

/// Where an `@unverified` marker may sit, outermost to innermost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// The whole package.
    Package,
    /// A protocol in it.
    Protocol,
    /// A function of a protocol.
    Function,
    /// An argument of a function.
    Argument,
    /// A field of a type.
    Field,
    /// A type.
    Type,
}

impl Scope {
    /// As the catalog writes it: `package`, `protocol`, `function`,
    /// `argument`, `field`, `type`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Scope::Package => "package",
            Scope::Protocol => "protocol",
            Scope::Function => "function",
            Scope::Argument => "argument",
            Scope::Field => "field",
            Scope::Type => "type",
        }
    }
}

/// Where an entry stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Stable,
    Experimental,
    /// Kept for what already uses it; `replaced_by` names what to use instead.
    Deprecated,
}

/// A guarantee Comline gives between peers, which a protocol can give up
/// with `@unverified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guarantee {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub comes_from: Origin,
    /// Where an `@unverified` marker acknowledging it may sit, outermost to
    /// innermost.
    pub scopes: &'static [Scope],
    pub status: Status,
    pub replaced_by: Option<&'static str>,
    /// The spec version that introduced it, where the changelog names one.
    pub since: Option<&'static str>,
}

impl Guarantee {
    /// Whether an `@unverified` marker acknowledging this guarantee may sit
    /// at `scope`.
    pub fn allows_scope(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }
}

/// One value a fact may take, and what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Value {
    pub id: &'static str,
    pub meaning: &'static str,
}

/// A fact an encoding's profile states. A handshake's facts are booleans
/// (`"true"`, `"false"`), and `true` establishes the guarantee named in
/// `establishes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fact {
    pub id: &'static str,
    pub applies_to: Kind,
    pub title: &'static str,
    pub description: &'static str,
    pub values: &'static [Value],
    pub establishes: Option<&'static str>,
    pub status: Status,
    pub replaced_by: Option<&'static str>,
    pub since: Option<&'static str>,
}

impl Fact {
    /// Whether `value` is one this fact may take.
    pub fn allows(&self, value: &str) -> bool {
        self.values.iter().any(|v| v.id == value)
    }
}

/// An official encoding, by its wire name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    pub id: &'static str,
    pub kind: Kind,
    pub title: &'static str,
    pub description: &'static str,
    /// Other spellings a schema may write, resolved to `id`.
    pub aliases: &'static [&'static str],
    /// The spelling core freezes into the IR, where it isn't `id`.
    pub frozen_as: Option<&'static str>,
    /// Whether `profiles/<id>.json` states its facts.
    pub profile: bool,
    /// The feature whose vectors prove it.
    pub feature: Option<&'static str>,
    /// How many bytes each kind of value takes: a format's, and only an
    /// official format's. `None` is no size rules, not a size of zero.
    pub sizes: Option<&'static Sizes>,
    pub tags: &'static [&'static str],
    pub status: Status,
    pub replaced_by: Option<&'static str>,
    pub since: Option<&'static str>,
}

impl Encoding {
    /// The spelling core freezes: `frozen_as`, or else the wire name.
    pub const fn frozen(&self) -> &'static str {
        match self.frozen_as {
            Some(spelling) => spelling,
            None => self.id,
        }
    }
}

/// A wire feature: a set of vector cases, `vectors/<id>.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub status: Status,
    pub replaced_by: Option<&'static str>,
    pub since: Option<&'static str>,
}

/// A primitive type, as the size rules name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primitive {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    S8,
    S16,
    S32,
    S64,
    S128,
    F32,
    F64,
    Char,
    Unit,
}

impl Primitive {
    /// Every primitive, in the order the catalog lists them.
    pub const ALL: [Primitive; 15] = [
        Primitive::Bool,
        Primitive::U8,
        Primitive::U16,
        Primitive::U32,
        Primitive::U64,
        Primitive::U128,
        Primitive::S8,
        Primitive::S16,
        Primitive::S32,
        Primitive::S64,
        Primitive::S128,
        Primitive::F32,
        Primitive::F64,
        Primitive::Char,
        Primitive::Unit,
    ];

    /// As a schema and the catalog write it: `bool`, `u8`, `s64`, `char`...
    pub const fn as_str(self) -> &'static str {
        match self {
            Primitive::Bool => "bool",
            Primitive::U8 => "u8",
            Primitive::U16 => "u16",
            Primitive::U32 => "u32",
            Primitive::U64 => "u64",
            Primitive::U128 => "u128",
            Primitive::S8 => "s8",
            Primitive::S16 => "s16",
            Primitive::S32 => "s32",
            Primitive::S64 => "s64",
            Primitive::S128 => "s128",
            Primitive::F32 => "f32",
            Primitive::F64 => "f64",
            Primitive::Char => "char",
            Primitive::Unit => "unit",
        }
    }
}

/// What a count or a length costs on the wire, ahead of what it measures.
/// `catalog/readme.md` defines each kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Header {
    /// Nothing: the count is in the type, or isn't said.
    None,
    /// The count as an unsigned LEB128 varint: 7 bits a byte.
    VarintLeb128,
    /// A MessagePack string header on a length in bytes: 1 byte up to 31, 2
    /// up to 255, 3 up to 65535, 5 above.
    MsgpackStr,
    /// A MessagePack array header on an item count: 1 byte up to 15, 3 up to
    /// 65535, 5 above.
    MsgpackArray,
    /// Delimiters around the items, and a separator between two: `before`
    /// bytes, the items with `between` bytes between each two, `after` bytes.
    Delimited { before: u64, after: u64, between: u64 },
}

impl Header {
    /// The bytes before and after what `count` measures, apart from the
    /// separators between items (see [`Header::between`]).
    pub const fn len(&self, count: u64) -> u64 {
        match *self {
            Header::None => 0,
            Header::VarintLeb128 => {
                let mut bytes = 1;
                // 7 bits a byte, so 10 bytes hold 64 bits.
                while bytes < 10 && count >> (7 * bytes) != 0 {
                    bytes += 1;
                }
                bytes
            }
            Header::MsgpackStr => match count {
                0..=31 => 1,
                32..=255 => 2,
                256..=65535 => 3,
                _ => 5,
            },
            Header::MsgpackArray => match count {
                0..=15 => 1,
                16..=65535 => 3,
                _ => 5,
            },
            Header::Delimited { before, after, .. } => before + after,
        }
    }

    /// The bytes between two consecutive items: only a delimited sequence
    /// has any.
    pub const fn between(&self) -> u64 {
        match *self {
            Header::Delimited { between, .. } => between,
            _ => 0,
        }
    }
}

/// How a string's characters are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// UTF-8: 1 to 4 bytes a character.
    Utf8,
    /// A JSON string's contents: 1 to 6 bytes a character (`\u0001` is 6).
    JsonEscaped,
}

impl Body {
    /// The most bytes one character takes.
    pub const fn max_per_char(self) -> u64 {
        match self {
            Body::Utf8 => 4,
            Body::JsonEscaped => 6,
        }
    }
}

/// The bytes a kind of value takes: at least `min`, at most `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Width {
    pub min: u64,
    pub max: u64,
    /// `max` is a bound that no witness reaches: the longest value may be
    /// shorter. Otherwise some value takes exactly `max` bytes.
    pub upper_bound: bool,
    /// The vector cases that prove it.
    pub proven_by: &'static [&'static str],
}

/// The bytes of each primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scalars {
    pub bool: Width,
    pub u8: Width,
    pub u16: Width,
    pub u32: Width,
    pub u64: Width,
    pub u128: Width,
    pub s8: Width,
    pub s16: Width,
    pub s32: Width,
    pub s64: Width,
    pub s128: Width,
    pub f32: Width,
    pub f64: Width,
    pub char: Width,
    pub unit: Width,
}

impl Scalars {
    /// The rule of one primitive.
    pub const fn of(&self, primitive: Primitive) -> &Width {
        match primitive {
            Primitive::Bool => &self.bool,
            Primitive::U8 => &self.u8,
            Primitive::U16 => &self.u16,
            Primitive::U32 => &self.u32,
            Primitive::U64 => &self.u64,
            Primitive::U128 => &self.u128,
            Primitive::S8 => &self.s8,
            Primitive::S16 => &self.s16,
            Primitive::S32 => &self.s32,
            Primitive::S64 => &self.s64,
            Primitive::S128 => &self.s128,
            Primitive::F32 => &self.f32,
            Primitive::F64 => &self.f64,
            Primitive::Char => &self.char,
            Primitive::Unit => &self.unit,
        }
    }
}

/// A string, a `char[max N]` or a `char[N]`: a header on the length in bytes,
/// then the characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StringSize {
    pub header: Header,
    pub body: Body,
    pub proven_by: &'static [&'static str],
}

/// An array, `T[]`, `T[max N]` or `T[N]`: a header on the item count, then
/// the items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeqSize {
    pub header: Header,
    pub proven_by: &'static [&'static str],
}

/// An option: `none` bytes when absent, `some` bytes and the value when
/// present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSize {
    pub none: u64,
    pub some: u64,
    pub proven_by: &'static [&'static str],
}

/// A struct: a header on the field count, then the fields, each with its
/// name's bytes and `key_extra` more when `key_extra` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructSize {
    pub header: Header,
    pub key_extra: Option<u64>,
    pub proven_by: &'static [&'static str],
}

/// How an enum's variant is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumTag {
    /// Its index, as a LEB128 varint.
    VarintIndex,
    /// Its name, behind a header on the name's length in bytes.
    Name(Header),
}

/// An enum of unit variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumSize {
    pub tag: EnumTag,
    pub proven_by: &'static [&'static str],
}

/// How many bytes each kind of value of a format takes, as data a type's
/// range is computed from. Every rule is proven by the vector cases it
/// lists, in `vectors/<feature>.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    /// The feature whose vectors prove the rules.
    pub feature: &'static str,
    pub scalars: Scalars,
    pub string: StringSize,
    pub array: SeqSize,
    pub fixed_array: SeqSize,
    pub option: OptionSize,
    pub struct_: StructSize,
    pub enum_: EnumSize,
}

/// The size rules of the official format whose wire name is `id`.
pub fn sizes(id: &str) -> Option<&'static Sizes> {
    encoding(id).and_then(|e| e.sizes)
}

/// The guarantee `id`.
pub fn guarantee(id: &str) -> Option<&'static Guarantee> {
    GUARANTEES.iter().find(|g| g.id == id)
}

/// The fact `id`.
pub fn fact(id: &str) -> Option<&'static Fact> {
    FACTS.iter().find(|f| f.id == id)
}

/// The facts a profile of `kind` states, in the order profiles list them.
pub fn facts_of(kind: Kind) -> impl Iterator<Item = &'static Fact> {
    FACTS.iter().filter(move |f| f.applies_to == kind)
}

/// The official encoding whose wire name is `id`.
pub fn encoding(id: &str) -> Option<&'static Encoding> {
    ENCODINGS.iter().find(|e| e.id == id)
}

/// The official encoding `name` spells: its wire name or one of its aliases.
pub fn resolve(name: &str) -> Option<&'static Encoding> {
    encoding(name).or_else(|| ENCODINGS.iter().find(|e| e.aliases.contains(&name)))
}

/// The feature `id`.
pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_constant_resolves() {
        for id in sets::GUARANTEES {
            assert!(guarantee(id).is_some(), "{id}");
        }
        for id in sets::OFFICIAL {
            assert!(encoding(id).is_some(), "{id}");
        }
        for id in sets::FEATURES {
            assert!(feature(id).is_some(), "{id}");
        }
        assert_eq!(guarantee(guarantee::BOUNDED_SIZE).unwrap().title, "bounded size");
        assert_eq!(encoding(encoding::PACKED_1).unwrap().kind, Kind::Format);
    }

    #[test]
    fn aliases_resolve_to_their_wire_name() {
        for (alias, id) in sets::ALIASES {
            assert_eq!(resolve(alias).map(|e| e.id), Some(*id), "{alias}");
        }
        assert_eq!(resolve("json-rpc").unwrap().id, encoding::JSONRPC_1);
        assert_eq!(resolve(encoding::DATAGRAM_1).unwrap().frozen(), "datagram");
        assert_eq!(resolve(encoding::PACKED_1).unwrap().frozen(), encoding::PACKED_1);
        assert!(resolve("packed@2").is_none());
    }

    #[test]
    fn facts_match_their_sets_in_order() {
        let ids = |kind| facts_of(kind).map(|f| f.id);
        assert!(ids(Kind::Format).eq(sets::FORMAT_FACTS.iter().copied()));
        assert!(ids(Kind::Framing).eq(sets::FRAMING_FACTS.iter().copied()));
        assert!(ids(Kind::Handshake).eq(sets::HANDSHAKE_FACTS.iter().copied()));
        assert!(fact(fact::CALLS).unwrap().allows("by_id"));
        assert!(!fact(fact::CALLS).unwrap().allows("by_guess"));
    }

    #[test]
    fn handshake_facts_establish_guarantees() {
        for f in facts_of(Kind::Handshake) {
            let g = f.establishes.expect("a handshake fact establishes a guarantee");
            assert!(guarantee(g).is_some(), "{g}");
            assert!(f.allows("true") && f.allows("false"));
        }
    }

    #[test]
    fn every_guarantee_has_scopes_in_order() {
        let rank = |s: &Scope| sets::SCOPES.iter().position(|id| *id == s.as_str()).unwrap();
        for g in GUARANTEES {
            assert!(!g.scopes.is_empty(), "{}", g.id);
            assert!(g.scopes.windows(2).all(|w| rank(&w[0]) < rank(&w[1])), "{}", g.id);
            assert_eq!(g.comes_from == Origin::Type, g.allows_scope(Scope::Type), "{}", g.id);
        }
    }

    #[test]
    fn a_type_guarantee_reaches_below_the_protocol() {
        let bounded = guarantee(guarantee::BOUNDED_SIZE).unwrap();
        assert!(bounded.allows_scope(Scope::Field));
        assert!(!bounded.allows_scope(Scope::Package));
        let agreement = guarantee(guarantee::SCHEMA_AGREEMENT).unwrap();
        assert!(agreement.allows_scope(Scope::Package) && agreement.allows_scope(Scope::Protocol));
        assert!(!agreement.allows_scope(Scope::Field));
    }

    #[test]
    fn scopes_match_their_set() {
        let all = [
            Scope::Package,
            Scope::Protocol,
            Scope::Function,
            Scope::Argument,
            Scope::Field,
            Scope::Type,
        ];
        assert!(all.iter().map(|s| s.as_str()).eq(sets::SCOPES.iter().copied()));
    }

    fn sizes_of(id: &str) -> &'static Sizes {
        sizes(id).expect("an official format has size rules")
    }

    fn range(width: &Width) -> (u64, u64) {
        (width.min, width.max)
    }

    #[test]
    fn scalar_widths_are_what_each_format_writes() {
        let packed = sizes_of(encoding::PACKED_1);
        assert_eq!(range(&packed.scalars.u32), (4, 4));
        assert_eq!(range(&packed.scalars.char), (1, 4));
        assert_eq!(range(&packed.scalars.unit), (0, 0));
        let msgpack = sizes_of(encoding::MSGPACK_1);
        assert_eq!(range(&msgpack.scalars.u64), (1, 9));
        assert_eq!(range(&msgpack.scalars.u128), (18, 18));
        let json = sizes_of(encoding::JSON_1);
        assert_eq!(range(json.scalars.of(Primitive::S64)), (1, 20));
        assert_eq!(range(&json.scalars.f64), (3, 24));
        assert_eq!(range(&json.scalars.f32), (3, 16));
        assert_eq!(range(&json.scalars.char), (3, 8));
    }

    #[test]
    fn no_official_rule_is_only_an_upper_bound() {
        for id in sets::FORMATS {
            for p in Primitive::ALL {
                assert!(!sizes_of(id).scalars.of(p).upper_bound, "{id} {}", p.as_str());
            }
        }
    }

    #[test]
    fn only_the_official_formats_have_sizes() {
        for e in ENCODINGS {
            assert_eq!(e.sizes.is_some(), e.kind == Kind::Format, "{}", e.id);
        }
        for id in sets::FORMATS {
            assert!(sizes(id).is_some(), "{id}");
        }
        assert!(sizes(encoding::DATAGRAM_3).is_none());
        assert!(sizes("com.acme.custom-format").is_none());
    }

    #[test]
    fn every_rule_is_proven_by_cases_in_its_feature() {
        for id in sets::FORMATS {
            let s = sizes_of(id);
            assert!(feature(s.feature).is_some(), "{id}");
            let prefix_ok = |proofs: &[&str]| {
                !proofs.is_empty() && proofs.iter().all(|c| c.strip_prefix(s.feature).is_some_and(|r| r.starts_with('/')))
            };
            for p in Primitive::ALL {
                let w = s.scalars.of(p);
                assert!(w.min <= w.max && prefix_ok(w.proven_by), "{id} {}", p.as_str());
            }
            assert!(prefix_ok(s.string.proven_by), "{id} string");
            assert!(prefix_ok(s.array.proven_by), "{id} array");
            assert!(prefix_ok(s.fixed_array.proven_by), "{id} fixed_array");
            assert!(prefix_ok(s.option.proven_by), "{id} option");
            assert!(prefix_ok(s.struct_.proven_by), "{id} struct");
            assert!(prefix_ok(s.enum_.proven_by), "{id} enum");
        }
    }

    #[test]
    fn headers_grow_with_the_count() {
        for (count, expected) in [(0, 1), (127, 1), (128, 2), (16383, 2), (16384, 3), (2097151, 3), (2097152, 4), (u64::MAX, 10)] {
            assert_eq!(Header::VarintLeb128.len(count), expected, "varint {count}");
        }
        for (count, expected) in [(0, 1), (31, 1), (32, 2), (255, 2), (256, 3), (65535, 3), (65536, 5)] {
            assert_eq!(Header::MsgpackStr.len(count), expected, "str {count}");
        }
        for (count, expected) in [(0, 1), (15, 1), (16, 3), (65535, 3), (65536, 5)] {
            assert_eq!(Header::MsgpackArray.len(count), expected, "array {count}");
        }
        let delimited = Header::Delimited { before: 1, after: 1, between: 1 };
        assert_eq!((delimited.len(0), delimited.len(1000), delimited.between()), (2, 2, 1));
        assert_eq!((Header::None.len(7), Header::None.between()), (0, 0));
    }

    #[test]
    fn each_format_writes_structs_and_enums_as_its_facts_say() {
        let packed = sizes_of(encoding::PACKED_1);
        assert_eq!(packed.struct_.header, Header::None);
        assert_eq!(packed.struct_.key_extra, None);
        assert_eq!(packed.enum_.tag, EnumTag::VarintIndex);
        let msgpack = sizes_of(encoding::MSGPACK_1);
        assert_eq!(msgpack.struct_.header, Header::MsgpackArray);
        assert_eq!(msgpack.enum_.tag, EnumTag::Name(Header::MsgpackStr));
        let json = sizes_of(encoding::JSON_1);
        assert_eq!(json.struct_.key_extra, Some(3));
        assert_eq!(json.string.body, Body::JsonEscaped);
        assert_eq!((json.option.none, json.option.some), (4, 0));
        assert_eq!((packed.option.none, packed.option.some), (1, 1));
    }

    #[test]
    fn the_spec_version_is_a_version() {
        let (major, minor) = SPEC_VERSION.split_once('.').unwrap();
        assert!(major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok());
    }
}
