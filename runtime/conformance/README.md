# Runtime wire conformance

Byte-exact vectors for Comline's wire contract, which every runtime must
reproduce, and the Rust runtime's proof that it does.

## The corpus

- [`features.json`](features.json): the wire features, with stable ids:
  `name-hash`, `handshake`, `envelope`, `framing-datagram`, `framing-jsonrpc`,
  `format-json` and `format-msgpack`.
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
| `request` | `framing`, `call_id`, `method`, `request_id`, `format`, `params` | encode and decode |
| `response` | `framing`, `request_id`, and an envelope's fields | encode and decode |
| `encode` | `format`, `value` | encode |

- **Encode and decode:** encoding the input gives exactly `bytes`, and decoding
  `bytes` gives the input back.
  - A datagram request carries the call id but not the method name; JSON-RPC
    carries the name but not the id. Decoding takes the other from the input.
  - Decoded request params must be exactly `params` encoded in `format`.
- **`"expect": "reject"`:** decoding `bytes` must fail. The input then names
  only the framing, where there is one.
- **Bytes** are lowercase hex.
- **64-bit integers** (`ir_hash`, `request_id`, `u64` and `i64` values) are
  decimal strings, so a reader that holds JSON numbers as doubles keeps them
  exact.

A value is typed, so a runtime knows which width and shape to encode:
`{"u8": 255}`, `{"u64": "18446744073709551615"}`, `{"string": "x"}`,
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

Each runtime declares, per feature, `supported` or `unsupported`, with an
optional `note` and `tracking` issue. A claim must hold against the vectors:

- **supported:** every case of the feature passes;
- **unsupported:** the runtime reports every case as not implemented. A
  runtime that passes them should claim the feature.

A feature with no claim, or no cases, fails.

Rust's claims are in [`support/rust.json`](support/rust.json). The runner
`rust_support_is_proven` writes [`reports/rust.json`](reports/rust.json): the
claims, with the cases that prove each. The report is drift-checked like the
vectors, and the docs' language reference is rendered from it.

## Another runtime

Vendor `features.json` and `vectors/` at a pinned commit of this repository.
Drive your runtime through the same ops, check your claims by the rules
above, and write a report of the same shape. comline-typescript does this in
`runtime/test/conformance/`.

Behavioural features (transports, timeouts, one-way calls) aren't here: a
vector can't prove them. They're for live cross-runtime calls, later.
