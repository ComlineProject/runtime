# Comline specification

What every Comline implementation is held to: the wire contract as byte-exact
conformance vectors, the facts each official encoding states and how they're
proven, and the formats of these files. A runtime, a third-party encoding or a
tool implements against this repository, and pins it like any other
dependency. Comline's own runtimes do the same.

Version: **0.10** ([`VERSION`](VERSION)). While Comline is in its early days
(0.x), a minor version may change anything, and the change is named in the
pull request that makes it and in [the changes](#changes) below.

The shared vocabulary of names (encodings, guarantees, facts, features and
the sets they form) will join this repository too: see
[the proposal](https://github.com/ComlineProject/docs/pull/162) and
[core#219](https://github.com/ComlineProject/core/issues/219).

## The corpus

- [`features.json`](features.json): the wire features, with stable ids:
  `name-hash`, `handshake`, `envelope`, `framing-datagram`, `framing-datagram2`,
  `framing-datagram3`, `handshake-comline2`, `framing-jsonrpc`, `stream`, `format-json`, `format-msgpack` and
  `format-packed`.
- `vectors/<feature>.json`: that feature's cases.
- [`benches/`](benches/README.md): the benchmarks every runtime runs, in the
  same value encoding.
- [`specs/`](specs/readme.md): the format of each of these files, in CUE, and
  the JSON Schemas generated from it, which each file names in `$schema`.

A case is one operation and the bytes it must produce:

```json
{
  "id": "envelope/err",
  "op": "envelope",
  "input": { "kind": "err", "id": 258, "body": "6669656c6473" },
  "bytes": "0102016669656c6473"
}
```

The ops, and what their `input` holds:

| `op` | `input` | checked |
|---|---|---|
| `name_hash` | `name` | the hash, 8 bytes little-endian |
| `handshake` | `ir_hash`, `wire_format`, `framing` (names, unhashed), `capabilities` | encode and decode |
| `handshake_check` | one end's `ir_hash`, `wire_format`, `framing`, `capabilities` | decode: `bytes`, the peer's `comline@1` frame, is accepted (giving the input back) when the fingerprint, format and framing agree, and refused when one differs |
| `envelope` | `kind`: `ok` with `payload`, `err` with `id` and `body`, or `fault` | encode and decode |
| `request` | `framing`, `call_id`, `method`, `request_id`, `format`, `params`, and `one_way` when `true` | encode and decode |
| `response` | `framing`, `request_id`, and an envelope's fields | encode and decode |
| `peer_refusal` | `framing` | decode: `bytes` are the peer saying it couldn't read a request, without saying which |
| `encode` | `format`, `value`, and `type` when its bounds matter | encode; a rejection's encode must fail |
| `decode` | `format`, `type`, `value` | decode: `bytes` read as `type` give `value` |
| `stream_prefix` | `len`, `max` | encode and decode: the 4-byte length prefix of a `len`-byte frame under the limit `max` |
| `comline2_message` | a `comline@2` message: `type` (`hello`, `answer` or `verdict`) and `message` | encode and decode |
| `comline2_negotiate` | `step`, and as the step needs `side`, `params`, `local` (this end's hello), `pending`; then `reply` and `result` | decode: `bytes`, the peer's message, run through `step` give exactly `reply` (the message this end sends, or `null`) and `result` (`accepted`, or `refused` with its reason and side) |
| `stream_split` | `max`, `frames`, `rest`, `needed` | encode and decode: a byte stream of `frames`, each behind its prefix, then `rest` |

- **Encode and decode:** encoding the input gives exactly `bytes`, and decoding
  `bytes` gives the input back.
  - A datagram request carries the call id but not the method name; JSON-RPC
    carries the name but not the id. Decoding takes the other from the input.
  - Decoded request params must be exactly `params` encoded in `format`.
  - `one_way` is a one-way call on a connection: JSON-RPC sends it as a
    notification, with no `id`, and decoding one gives `request_id` `"0"`.
- **`"compare": "json"`:** for bytes that are JSON text (JSON values and
  JSON-RPC frames), the output must equal `bytes` as JSON (RFC 8259) rather
  than byte for byte.
  - Whitespace and member order don't matter.
  - Numbers compare as exact decimals: `1` equals `1.0`, but
    `9007199254740992` doesn't equal `9007199254740993`.
  - The same holds for JSON inside a frame: JSON-RPC params, results and
    error data.
  - Strings compare by the text they hold, however they're escaped:
    `"app.Chat\/count"` equals `"app.Chat/count"`. Most such cases' bytes
    are the reference's own output, but a `found-*` case can spell one
    another way, which every runtime must read.
- **`"expect": "reject"`:** decoding `bytes` must fail. The input then names
  only the framing, where there is one.
  - An `encode` rejection is a value over its `type`'s bounds: encoding it
    must fail, before anything is written. Its `bytes` are empty and aren't
    decoded.
- **`"expect": "accept"`:** decoding `bytes` must give the input, and nothing
  asks to encode it: a frame a runtime must read but has no reason to write,
  such as one with a member JSON-RPC doesn't name.
- **Stream framing** (`stream`): a frame on a byte stream is
  `[len: u32 LE][len bytes]`, and a runtime refuses a `len` over its limit:
  by default 16 MiB (16,777,216 bytes) in every runtime, and `len == max` is
  accepted.
  - `stream_prefix`: a rejection's bytes are a prefix over `max`, and its
    input is `{len, max}`: decoding the bytes and encoding the input must
    both fail.
  - `stream_split`: decoding splits `bytes` frame by frame under `max`
    (`frames`, hex), until what's left (`rest`) isn't a whole frame.
    `needed` is the bytes that incomplete frame takes in all, prefix
    included, or 0 while its prefix is incomplete. A rejection's bytes
    declare a frame over `max`, possibly after whole frames; its input is
    `{max}`.
  - `len`, `max` and `needed` are plain JSON numbers: they stay below
    2^53.
- **Bytes** are lowercase hex.
- **64-bit integers** (`ir_hash`, `request_id`, `u64` and `i64` values) are
  decimal strings, so a reader that holds JSON numbers as doubles keeps them
  exact.

A `decode` case's bytes were written by a *different* type from the one it
reads into: a struct with a field more or one fewer, fields in another order,
an enum whose variants are ordered differently or that has one more, a wider
integer. They show what a reader generated from today's schema does with what
another version wrote, which is what [profiles](#encoding-profiles) are about.
`type` describes the reader so any runtime can build one, in every shape a
generated type can have:
- the scalars `"bool"`, `"u8"` to `"u128"`, `"i8"` to `"i128"`, `"f32"`,
  `"f64"`, `"string"`, `"char"` (one Unicode scalar value) and `"unit"`;
- `{"option": T}`, `{"array": T}` for `T[]`, and `{"fixed": {"of": T, "len": N}}`
  for `T[N]`;
- bounds (core#98): `{"bounded_string": {"max": N}}` for `char[max N]`, at
  most N characters (Unicode scalar values, not bytes);
  `{"bounded_array": {"of": T, "max": N}}` for `T[max N]`; and
  `{"recursive": {"max_depth": D, "of": T}}` for a type that can contain
  itself, under `@max_depth = D`, where `"self"` inside `T` stands for the
  recursive type again. The outermost value is level 1, and a value at level
  D + 1 is refused;
- `{"struct": {"name": "Point", "fields": [["x", "i32"], ["y", "i32"]]}}`;
- `{"enum": {"name": "Color", "variants": ["Red", "Green", "Blue"]}}`.

The reference decodes by that description through `TypeSeed`
([`seed.rs`](https://github.com/ComlineProject/runtime/blob/main/runtime/conformance/src/seed.rs) in the Rust reference). It makes exactly the calls a plain `serde`
derive makes, as generated Rust declares its types, so the cases prove
generated code, not a hand-written decoder.
[`seed_vs_derive.rs`](https://github.com/ComlineProject/runtime/blob/main/runtime/conformance/tests/seed_vs_derive.rs) holds it to real
derives, on valid and mutated bytes in every format.

[`properties.rs`](https://github.com/ComlineProject/runtime/blob/main/runtime/conformance/tests/properties.rs) checks the reference's properties
over random types and values with `proptest`, and
[`runtime/fuzz`](https://github.com/ComlineProject/runtime/blob/main/runtime/fuzz/README.md) fuzzes it with `cargo-fuzz`. Cases they
find become `found-*` vectors; the fuzz README says how.

A value is typed, so a runtime knows which width and shape to encode:
`{"u8": 255}`, `{"u64": "18446744073709551615"}` (and `u128`, `i128`, also as
decimal strings), `{"string": "x"}`, `{"char": "é"}`, `{"tuple": [...]}` for `T[N]`,
`{"unit": null}`,
`{"array": [...]}`, `{"option": null}`,
`{"struct": {"name": "Point", "fields": [["x", {"i32": 1}]]}}` and
`{"unit_variant": {"enum": "Color", "index": 1, "variant": "Green"}}`. A value
encodes the way a generated type of the same shape does: a struct is an object
in JSON and an array in MessagePack.


### Where the vectors come from, and changing the spec

This repository is the authority. A wire change starts here: a pull request
that changes the vectors, profiles or specs, and bumps [`VERSION`](VERSION)
with an entry under [the changes](#changes). Every runtime then follows,
Comline's own included.

The Rust runtime is the reference implementation. Its conformance crate
([`runtime/conformance`](https://github.com/ComlineProject/runtime/blob/main/runtime/conformance)) computes every case from
`comline-runtime`, and its `vectors_are_current` test fails when its output
differs from this repository's vectors at the revision it pins. So a spec
change that moves bytes lands here first, and the reference is changed to
match.

Each implementation vendors this repository at a pinned revision and checks in
CI that its copy hasn't drifted (see [Implementing the spec](#implementing-the-spec)).

## The spec

The case format above (`vectors/*.json`, `profiles/*.json`, the ops, values
and types) and the ops protocol
([`comline-reference`'s README](https://github.com/ComlineProject/runtime/blob/main/runtime/reference/README.md#the-protocol)) are a
public spec, at the version in [`VERSION`](VERSION). Third-party
encodings ship vectors in this format, and `comline conformance --impl`
drives their implementations over this protocol. While Comline is in its
early days (0.x), a minor version may change anything, and the change is
named in the pull request that makes it. The ops protocol keeps its own
number (`protocol: 1` in a `version` reply), under which ops and reply
fields are only ever added.

What an implementation answers, and how its answers are judged:

- It reads request lines on stdin and writes one reply line each on stdout,
  as `comline-reference` does, until stdin ends.
- `version` says what it implements: `protocol`, `ops`, and the encoding
  names it ships under `formats` and `framings` (namespaced names too,
  `com.acme.lines@1`).
- Each case poses the questions the Rust runner's `runner::questions` lists: a rejection asks
  to decode its bytes (a `stream_prefix` rejection also asks to encode its
  input, which must fail); any other case asks to encode its input (unless
  it's a `decode`, a `handshake_check` or a `comline2_negotiate`) and to decode its bytes (unless it's a `name_hash` or an
  `encode`). The questions never depend on earlier answers.
- `runner::judge` passes a case when every encode gives its bytes (as JSON,
  for `"compare": "json"`), every decode accepts and gives its input back
  exactly, and a rejection is refused. A `not_implemented` answer makes the
  case not implemented; an `error` fails it.
- A profile fact is proven when every case it names passes and shows its
  value (`fact_shown`). A framing's `calls` is shown by exact `request` cases
  whose bytes hold the method's name for `by_name` and don't otherwise; whether
  a number is an index or a wire number (`by_index`, `by_id`) isn't in the bytes,
  so the profile's word stands for that. Its `directions` is shown by real frames
  read the other way: for `both`, the bytes of an exact request refused as a
  response and those of an exact response refused as a request, and for `one`,
  such bytes accepted as the other. Its `errors` needs an exact error
  response; a response's input holds only the ordinal, so `by_ordinal` and
  `by_name` rest on the case passing. Its `payload` is shown by requests: an
  exact one with non-JSON params for `opaque`, and for `json`, an exact one with
  JSON params and a refusal of that same frame with non-JSON in their place. A
  format's `output` is shown by exact `encode` cases: all JSON for `json`, and
  at least one not for `bytes`. Its other facts are shown by `decode` cases of the
  `Point` and `Color` shapes the official profiles use. A handshake's facts
  are shown by `handshake_check` cases: one the peer agrees in, accepted,
  and, per thing the fact covers, one differing in only that, refused.

### Changes

- **0.10:** a framing states `directions`: `both`, when a frame says whether it's
  a request or a response so calls can go both ways on one transport, or `one`,
  when it doesn't. A profile without it is refused. New cases, on `datagram@2`,
  `@3` and `jsonrpc@1`: `reject-response-as-request` and
  `reject-request-as-response`; on `datagram@1`: `accept-request-as-response`.
  No frame's bytes change. `jsonrpc@1` refuses a response with neither `result`
  nor `error`, which JSON-RPC 2.0 doesn't allow and which it used to read as a
  `null` result, a request's frame among them; new case
  `reject-response-no-outcome`.
- **0.9:** new op `peer_refusal`: JSON-RPC 2.0's answer to a request a peer
  can't read, an error whose `id` is `null`, is read as that. It answers no call,
  so it stays no `response`, and a `null` id on anything else stays refused. It
  answers `not_implemented` on an implementation without the op.
- **0.8:** `jsonrpc@1` leaves `params` out of a request without any, where it
  wrote `"params": null`, which JSON-RPC 2.0 doesn't allow. Readers still take
  `null` as none. New cases: `request-no-params`, `notification-no-params`,
  `accept-request-params-null`.
- **0.7:** a framing states `payload` (`opaque` or `json`) and a format states
  `output` (`bytes` or `json`), and a profile without them is refused. New
  `"expect": "accept"`. New cases on `jsonrpc@1`: a member written twice is
  refused, in a request, a response and an error object; a payload in
  `msgpack@1` or `packed@1` is refused; unknown members are read. Request params
  a framing can't carry answer `not_implemented` by these facts, which replaces
  0.6's rule on `jsonrpc@1` by name, and now covers params that aren't an array,
  an object or `null`.
- **0.6:** `calls` and `errors` facts must be shown by their cases, where any
  passing case used to do: a `calls` naming anything but exact requests, or a
  `by_name` whose frames lack the method's name (or a number whose frames hold
  it), is unproven, and so is an `errors` naming no exact error response. New
  cases: request params in `msgpack@1` and `packed@1` on `datagram@1`, `@2` and
  `@3`; `name-hash/datagram@3` and `handshake/packed-datagram3`. Request params
  in a format other than `json@1` on `jsonrpc@1` aren't JSON-RPC, and answer
  `not_implemented`. New benchmarks: `datagram@3` framing, and `comline2.json`.
- **0.5:** the corpus's format is specified in [`specs/`](specs/readme.md),
  and every file may name its schema in a top-level `$schema`, which a reader
  ignores. The runner reads inputs and profiles by their shape, and refuses at
  load what it used to fail per case or per fact: an `input` with a field its
  op doesn't have or spelled another way (`"protocol": null`), a profile fact
  or value its kind doesn't have, a case id that isn't `<feature>/<name>` or
  appears twice, and hex that isn't lowercase. A decode's answer is compared
  by what it holds, so `1` gives back `{"f64": 1.0}`. Benchmark shapes may be
  `generate`d. No vector's bytes change.
- **0.4:** `datagram@3` and `comline@2` (runtime#15). New features
  `framing-datagram3` (a `request`'s `call_id` is the function's wire
  number, and `protocol` the protocol's) and `handshake-comline2`, with the
  ops `comline2_message` (encode and decode) and `comline2_negotiate`
  (decode only). A `comline@2` handshake fact is shown by
  `comline2_negotiate` cases: one accepted with every protocol, and
  refusals (of the connection or one protocol) for `line_mismatch` and
  `package_mismatch` (`schema_agreement`) or `encoding_mismatch` and
  `exchange_mismatch` (`encoding_agreement`). New profiles `datagram@3`
  (`calls = by_id`) and `comline@2`. Nothing earlier changes.
- **0.3:** bounds and `char` (core#98). New type descriptions `"char"`,
  `bounded_string`, `bounded_array`, `recursive` and `"self"`, and the value
  `{"char": ...}`. An `encode` case may carry a `type`, and an `encode`
  rejection asks only to encode, which must fail. Each format's vectors
  gain a single `char` (ASCII, 2-, 3- and 4-byte, and bytes that aren't one
  character), bounded strings and arrays at, under and over their maximum
  (multi-byte characters near it, a byte length over 4 × N), and the depth
  limit, decoded and encoded. No profile facts change.
- **0.2:** handshake profiles state `schema_agreement` and
  `encoding_agreement`, as JSON booleans (`"value": true`); a fact stated
  `false` names no cases, and a handshake profile may leave a fact out,
  which reads as `false`. New op `handshake_check` (decode only), whose
  cases prove them; `comline@1` has a profile.
- **0.1:** the first published version.

## Support, and its proof

Each runtime declares, per feature, `supported`, `partial` or `unsupported`,
with an optional `note` and `tracking` issue. A claim must hold against the
vectors:

- **supported:** every case of the feature passes;
- **partial:** every case passes except those listed in `failing`, which must
  fail. A listed case that starts passing fails the check, so the claim is
  upgraded as soon as it can be;
- **unsupported:** the runtime reports every case as not implemented. A
  runtime that passes them should claim the feature.

A feature with no claim, or no cases, fails.

Each runtime keeps its claims (`support/<runtime>.json`) and the report the
check writes (`reports/<runtime>.json`) in its own repository, in the formats
[`specs/support.cue`](specs/support.cue) and [`specs/report.cue`](specs/report.cue)
define. Rust's are in [`runtime/conformance`](https://github.com/ComlineProject/runtime/blob/main/runtime/conformance).


## Encoding profiles

[`profiles/`](profiles) holds one profile per official encoding name
(`json@1.json`, `msgpack@1.json`, `packed@1.json`, `datagram@1.json`,
`datagram@2.json`, `datagram@3.json`, `jsonrpc@1.json`, `comline@1.json`,
`comline@2.json`): facts
about how it encodes, which the version comparison turns into bumps. A format
states `struct_fields`, `unknown_field`, `missing_optional`, `enum_variants`,
`unknown_variant`, `integer_width` and `output`; a framing states `calls`,
`errors` and `payload`.

`payload` says what a framing needs of a payload, and `output` what a format
writes. A framing takes a format when its `payload` is `opaque` (it carries the
bytes as they are) or the two name the same grammar (`jsonrpc@1`'s is `json`).
Tools decide by these two facts alone, so a third-party encoding is held to the
same rule as an official one. `json` also bounds a request's params to what
JSON-RPC 2.0 allows: an array or an object, with `params` left out for none. Neither is
enforced on the wire: a runtime refuses what it can't read, and refusing a pair
that can't work is up to tools that know both, such as a schema compiler.

A handshake states what it establishes between the two ends, as booleans:
`schema_agreement` (both were built from the same schema) and
`encoding_agreement` (both speak the same format and framing). One it
doesn't state as `true` isn't established, and core makes a protocol using
the handshake give it up (`@unverified`). `comline@1` states both, proven by
the `handshake/check-*` cases: it accepts an agreeing peer and refuses one
whose fingerprint, format or framing differs. `comline@2` states both too,
proven by `handshake-comline2/offer-*` negotiations: a peer accepted, and
peers refused for a moved line, another package, other encodings or
another exchange.

```json
"schema_agreement": { "value": true, "proven_by": ["handshake/check-agree", "handshake/check-reject-ir-hash"] }
```

```json
"unknown_field": { "value": "rejected", "proven_by": ["format-msgpack/decode-unknown-field"] }
```

Every fact names the cases that prove it, and `profiles_are_proven` checks
that each named case exists, passes against `comline-runtime`, and shows the
stated value: a rejection where the fact says `rejected`, the decoded value
where it says `by_name` or `by_position`. A profile is fixed for its version:
changing a fact means a new `@N`.

Whether a one-way call gets a reply frame is behaviour, not bytes, so no
profile states it until live cross-runtime calls can prove it (see below).

## Implementing the spec

Vendor `features.json`, `vectors/` and `profiles/` at a pinned commit of this
repository, and `specs/gen/` too if you want their `$schema` to resolve; a
reader ignores it. `benches/` carries its own schema, so it vendors whole on
its own. Keep the pinned revision in your repository and check in CI that your
copy still matches it. comline-typescript does this in
`runtime/test/conformance/`.

Drive your implementation through the ops, check your claims by the rules
above, and write a report of the same shape.

Behavioural features (transports, timeouts, one-way calls) aren't here: a
vector can't prove them. They're for live cross-runtime calls, later.

### Asking the reference directly

Beyond the fixed vectors, [`comline-reference`](https://github.com/ComlineProject/runtime/blob/main/runtime/reference/README.md) is
`comline-runtime` as a process: it answers any `op`/`input`/`bytes` question
in this corpus's encoding, newline-delimited JSON on stdin and stdout, so a
fuzzer in another language can compare its implementation with the reference
on inputs no vector holds.

## License

MPL-2.0, see [LICENSE](LICENSE).
