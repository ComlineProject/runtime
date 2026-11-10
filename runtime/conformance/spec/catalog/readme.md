# The catalog

Every name that more than one Comline program has to agree on, defined once.
Each entry has a stable id, a title and a description, and where they apply
a value domain, aliases, a status and plain references to other entries.
Sets of names are queries over the entries, so they can't drift from them.

| Kind | Ids | File |
| --- | --- | --- |
| Guarantees | `schema_agreement`, `bounded_size`, ... | [`guarantees.cue`](guarantees.cue) |
| Facts, with their values | `struct_fields`, `calls`, `schema_agreement`, ... | [`facts.cue`](facts.cue) |
| Official encodings, with their aliases | `packed@1`, `jsonrpc@1`, `comline@2`, `none`, ... | [`encodings.cue`](encodings.cue) |
| Size rules of the formats | `packed@1`'s `sizes`, ... | [`encodings.cue`](encodings.cue), [`schema.cue`](schema.cue) |
| Wire features | `name-hash`, `framing-datagram3`, `sizes-packed`, ... | [`features.cue`](features.cue) |
| Sets | `formats`, `framing_facts`, `aliases`, ... | [`sets.cue`](sets.cue) |

What an entry may say, and the rules across entries, are in
[`schema.cue`](schema.cue):

- An id is a published name, so it never changes. A rename adds an entry
  and deprecates the old one (`status: "deprecated"`, `replaced_by`).
- An alias resolves to exactly one encoding, and is never another
  encoding's name. Where core freezes another spelling than the wire name
  (`frozen_as`: `jsonrpc`, `datagram`), that spelling is an alias too.
- A reference (`establishes`, `feature`) names an entry that exists.
- A handshake's facts are booleans.
- A guarantee's `scopes` say where an `@unverified` marker acknowledging it
  may sit: any of `package`, `protocol`, `function`, `argument`, `field` and
  `type`, outermost to innermost, each at most once. They follow from where
  its cause is: what the handshake, a policy or the encoding gives up
  (`schema_agreement`, `version_honesty`, `encoding_conformance`, ...) is
  settled for a package or a protocol, so those are its scopes; what a type
  gives up (`bounded_size`) can also be acknowledged on a function, an
  argument, a field or the type itself, and must include `type`. The schema
  checks both, and `sets.scopes` lists the vocabulary.
- `since` is the spec version that introduced an entry, where the changelog
  names one.

## Size rules

A format entry may carry `sizes`: how many bytes each kind of value takes, as
data, so a compiler can compute the least and most bytes a type takes in that
format, and a runtime be held to it. The three official formats have them; a
custom format may declare them in its profile (`sizes`, in the same shape),
and one that doesn't has no size, which nothing guesses. Every rule lists the
vector cases that prove it (`proven_by`, in the feature `sizes.feature` names:
`vectors/sizes-<format>.json`), and `specs/check.sh` refuses one with none,
or one whose cases don't exist or don't show it. The vocabulary is closed; the
schema is [`schema.cue`](schema.cue). Sizes are in bytes, a *count* is of items
and a *length* of bytes.

**`scalars`** gives `[min, max]` bytes, `min` and `max` both reached by some value,
for each primitive, named as a schema spells it (`s8` where a vector's type says
`i8`): `bool`, `u8`, `u16`, `u32`, `u64`, `u128`, `s8`, `s16`, `s32`, `s64`,
`s128`, `f32`, `f64`, `char` (one Unicode scalar value) and `unit`. Where a
rule has `upper_bound`, `max` is only a bound that no value is known to reach.

**A header** is what a count or a length costs ahead of what it measures:

| `kind` | Bytes |
| --- | --- |
| `none` | 0 |
| `varint_leb128` | the number as an unsigned LEB128 varint, 7 bits a byte: 1 byte up to 127, 2 up to 16,383, 3 up to 2,097,151, 4 up to 268,435,455, 5 up to 2^35 - 1 (counts and lengths are at most 2^32 - 1) |
| `msgpack_str` | a MessagePack string header on a length: 1 byte up to 31, 2 up to 255, 3 up to 65,535, 5 above |
| `msgpack_array` | a MessagePack array header on a count: 1 byte up to 15, 3 up to 65,535, 5 above |
| `delimited` | `before` bytes, then the items with `between` bytes between each two, then `after` bytes: the same whatever the count, plus `between` for each item after the first (JSON: `[`, `]`, `,`) |

**`string`** (a string, `char[max N]` or `char[N]`): a `header` on the *length in
bytes*, then the characters as `body`: `utf8`, 1 to 4 bytes a character, or
`json_escaped`, the contents of a JSON string as `serde_json` writes it, 1 to 6
bytes a character: a character is itself in UTF-8 (1 to 4 bytes), except `"` and
`\` (2 bytes: `\"`, `\\`), the controls U+0008, U+0009, U+000A, U+000C and U+000D
(2 bytes: `\b`, `\t`, `\n`, `\f`, `\r`), and the other controls U+0000 to U+001F
(6 bytes: `\u00XX`, in lower case). Nothing above U+001F is escaped, DEL included.
A bounded string's most bytes is its characters' most times its bound, and the
header is of that length: `char[max N]` is `header(4N) + 4N` bytes at most in UTF-8.
A string takes `header(0)` bytes at least, and `char[N]` `header(N) + N`.

**`array`** (`T[]`, `T[max N]`): a `header` on the item count, then the items.
**`fixed_array`** (`T[N]`): a `header` on N, then the items; where the header is
`none` the count isn't written at all.

**`option`**: `none` bytes for an absent value, and `some` bytes and then the value
for a present one. `packed@1` has `none` and `some` of 1 (a tag byte). `msgpack@1`
and `json@1` write an absent value as `nil` or `null` (`none` 1 and 4) and a
present one as the value alone (`some` 0), so an option's range is that of
`min(none, value)` to `max(none, value)`.

**`struct`**: a `header` on the field count, then the fields, in declaration
order. With `key_extra`, which goes with the fact `struct_fields = by_name`,
each field also takes its name's UTF-8 bytes and `key_extra` more (JSON: 2
quotes and a colon, 3). A field name is written as it is, and has no escapes.

**`enum`** (variants without fields): `tag` `varint_index` is the variant's index as
a varint (`enum_variants = by_index`), and `name` is the variant's name as a
string behind a `header` on its length (`by_name`); in JSON the header is a
delimited pair of quotes. The size of an enum is that of its shortest and longest
variant.

**Recursion** has no rule of its own: a type that contains itself under
`@max_depth = D` takes the sizes of the rules above to depth D, where the value
at level D + 1 can't be: an option there is absent, an array empty.

Not here yet, because no generator writes one (generated Rust refuses a union,
and an enum has only unit variants): unions, and enums with fields. Nor a map,
which no type is, nor a MessagePack header on one. JSON `NaN` and infinity are
written as `null`, a value the rules' range for a float contains, and no
vector writes.

**Where a rule can't be reached exactly** it says so: `upper_bound`. None of the
official formats has one, as every `max` is reached by a witness. A JSON float's
text is the shortest that reads back as the same number, as `serde_json` writes it
(the `ryu` and `zmij` of its versions agree on every float in positional
notation, which the witnesses keep to).

## What reads it

- [`specs/profile.cue`](../specs/profile.cue) takes each kind's facts and
  their values from here, so a profile is held to the catalog's domains.
- [`features.json`](../features.json) is generated from `features`.
- [`gen/catalog.json`](gen/catalog.json) is the whole catalog as JSON, for
  readers without CUE: the Observatory, the docs, generators in other
  languages.
- The **`comline-catalog`** Rust crate ([`rust/`](../rust)) holds it as
  constants and tables, `no_std` and without dependencies: core, the runtime
  and the language server depend on it by git rev. Its types and lookups are
  written by hand in `rust/src/lib.rs`; its data, `rust/src/catalog.rs`, is
  generated by [`rust.cue`](rust.cue). An id's constant is its name in upper
  case, with anything but letters and digits as `_`: `packed@1` is
  `encoding::PACKED_1`.
- `specs/check.sh` checks that `profiles/` holds exactly the encodings with a
  profile, each of its encoding's kind and, for a format with `sizes`, stating
  the same, and `vectors/` exactly the features.

A third party's encoding isn't an entry here: its package declares it in
`config.idp`, in the same shape, and the same rules judge it.

## Changing it

Edit the `.cue` files, then run `specs/check.sh` to regenerate
`features.json`, `gen/catalog.json` and `rust/src/catalog.rs`. CI runs
`specs/check.sh --check`, which fails when any of them is stale.

Order matters where it shows: facts are listed in the order profiles list
them, and features in the order `features.json` does.
