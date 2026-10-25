# Benchmark definitions

The work every runtime's benchmarks measure, so the numbers compare across
languages. Vendor this directory with the vectors; it uses the vectors' own
value and type encoding (see [the corpus README](../README.md)).

| File | Benchmarks |
|---|---|
| [`formats.json`](formats.json) | encode and decode, per format, per shape |
| [`framings.json`](framings.json) | framing encode and decode, per framing, per payload size |
| [`handshake.json`](handshake.json) | the handshake frame: encode, decode, check |
| [`connection.json`](connection.json) | one connection step: issue a call, receive its reply |

## `formats.json`

- `formats`: the formats each shape is measured in.
- `shapes`: each has an `id`, a `description`, a `type` (as a `decode` case
  states it) and a `value` of that type (as an `encode` case states it).

| `id` | What it stresses |
|---|---|
| `small-scalar` | five fixed-width scalars: per-message overhead |
| `string-heavy` | strings, with non-ASCII text, a quote and a newline to escape |
| `nested` | structs three deep, options present and absent |
| `large-array` | 1000 small structs |
| `enum-heavy` | unit-variant enums, alone, optional and in lists |

**Encode** a value of the shape, as a generated type holds it, into bytes.
**Decode** those bytes back into the generated type. Build the value and the
bytes outside the measurement, and measure the code that generated types run:
in Rust that's a `serde` derive, not the corpus's `Value`.

## `framings.json`

The measured frame is an `ok` response to request `request_id` (`42`), on
protocol `protocol` (`1`) for a framing that carries one (`datagram@2`). Its
payload, already encoded, is `size` bytes for each of `payload_sizes`: a JSON
string, `"`, then `size - 2` copies of `payload_fill` (`a`), then `"`, so it's
valid JSON for `jsonrpc@1` too.

**Encode** frames the payload. **Decode** parses the frame back into its
request id and envelope.

## `handshake.json`

`frame` is a handshake frame, with names unhashed as in a `handshake` case.
**Encode** writes it, **decode** reads its bytes back, and **check** compares
it with an identical frame from the peer.

## `connection.json`

One step on a fresh connection state: frame a call to `call` on `protocol`
with the `params` shape's value as its params, in `framing` and `format`,
waiting on a reply; then receive the `reply` to it (an `ok`, with the hex
`payload`). Both halves are measured together.

## Benchmark ids

Results name each benchmark by an id built from the definitions:

| Benchmark | Id |
|---|---|
| format | `format/<encode\|decode>/<shape>/<format>`, e.g. `format/decode/nested/packed@1` |
| framing | `framing/<encode\|decode>/<framing>/<size>`, e.g. `framing/encode/jsonrpc@1/4096` |
| handshake | `handshake/<encode\|decode\|check>` |
| connection | `connection/step` |

## Results: `reports/<lang>.bench.json`

```json
{
  "runtime": "rust",
  "commit": "<the measured commit>",
  "toolchain": "rustc 1.100.0-nightly (787af2b8c 2026-08-25)",
  "metric": "instructions",
  "benches": {
    "format/encode/small-scalar/packed@1": { "ir": 123 }
  }
}
```

- `metric` says what the numbers count. Rust counts instructions (`ir`, with
  Callgrind), which only compare against the same runtime on another commit.
- `benches` has one entry per id above.

Rust's CI writes it as an artifact on every pull request; see
[`runtime/core/benches`](../../core/benches) for how the Rust benches run.
