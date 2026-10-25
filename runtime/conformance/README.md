# Runtime wire conformance

Byte-exact vectors for Comline's wire contract, which every runtime must
reproduce, and the Rust runtime's proof that it does.

## The corpus

- [`features.json`](features.json): the wire features, with stable ids:
  `name-hash`, `handshake`, `envelope`, `framing-datagram`, `framing-datagram2`,
  `framing-jsonrpc`, `format-json`, `format-msgpack` and `format-packed`.
- `vectors/<feature>.json`: that feature's cases.

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
| `envelope` | `kind`: `ok` with `payload`, `err` with `id` and `body`, or `fault` | encode and decode |
| `request` | `framing`, `call_id`, `method`, `request_id`, `format`, `params`, and `one_way` when `true` | encode and decode |
| `response` | `framing`, `request_id`, and an envelope's fields | encode and decode |
| `encode` | `format`, `value` | encode |
| `decode` | `format`, `type`, `value` | decode: `bytes` read as `type` give `value` |

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
- **`"expect": "reject"`:** decoding `bytes` must fail. The input then names
  only the framing, where there is one.
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
  `"f64"`, `"string"` and `"unit"`;
- `{"option": T}`, `{"array": T}` for `T[]`, and `{"fixed": {"of": T, "len": N}}`
  for `T[N]`;
- `{"struct": {"name": "Point", "fields": [["x", "i32"], ["y", "i32"]]}}`;
- `{"enum": {"name": "Color", "variants": ["Red", "Green", "Blue"]}}`.

The reference decodes by that description through `TypeSeed`
([`src/seed.rs`](src/seed.rs)). It makes exactly the calls a plain `serde`
derive makes, as generated Rust declares its types, so the cases prove
generated code, not a hand-written decoder.
[`tests/seed_vs_derive.rs`](tests/seed_vs_derive.rs) holds it to real
derives, on valid and mutated bytes in every format.

A value is typed, so a runtime knows which width and shape to encode:
`{"u8": 255}`, `{"u64": "18446744073709551615"}` (and `u128`, `i128`, also as
decimal strings), `{"string": "x"}`, `{"tuple": [...]}` for `T[N]`,
`{"unit": null}`,
`{"array": [...]}`, `{"option": null}`,
`{"struct": {"name": "Point", "fields": [["x", {"i32": 1}]]}}` and
`{"unit_variant": {"enum": "Color", "index": 1, "variant": "Green"}}`. A value
encodes the way a generated type of the same shape does: a struct is an object
in JSON and an array in MessagePack.

### Where the vectors come from

The Rust runtime is the reference for now. [`src/lib.rs`](src/lib.rs) computes
every case from `comline-runtime`, and `vectors_are_current` fails when
`vectors/` differs. A changed vector is a wire-protocol change and shows up in
review as one.

```sh
CONFORMANCE_BLESS=1 cargo test -p comline-runtime-conformance
```

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

Rust's claims are in [`support/rust.json`](support/rust.json). The runner
`rust_support_is_proven` writes [`reports/rust.json`](reports/rust.json): the
claims, with the cases that prove each. The report is drift-checked like the
vectors, and the docs' language reference is rendered from it.

## Encoding profiles

[`profiles/`](profiles) holds one profile per official encoding name
(`json@1.json`, `msgpack@1.json`, `packed@1.json`, `datagram@1.json`,
`datagram@2.json`, `jsonrpc@1.json`): facts
about how it encodes, which the version comparison turns into bumps. A format
states `struct_fields`, `unknown_field`, `missing_optional`, `enum_variants`,
`unknown_variant` and `integer_width`; a framing states `calls` and `errors`.

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

## Another runtime

Vendor `features.json` and `vectors/` at a pinned commit of this repository.
Drive your runtime through the same ops, check your claims by the rules
above, and write a report of the same shape. comline-typescript does this in
`runtime/test/conformance/`.

Behavioural features (transports, timeouts, one-way calls) aren't here: a
vector can't prove them. They're for live cross-runtime calls, later.

### Asking the reference directly

Beyond the fixed vectors, [`comline-reference`](../reference/README.md) is
`comline-runtime` as a process: it answers any `op`/`input`/`bytes` question
in this corpus's encoding, newline-delimited JSON on stdin and stdout, so a
fuzzer in another language can compare its runtime with the reference on
inputs no vector holds.
