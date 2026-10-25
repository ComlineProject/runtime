# comline-reference

The Rust runtime, `comline-runtime`, as a process that answers encode and
decode questions. A runtime in another language checks itself against it,
from a fuzzer or a test, without linking any Rust: it asks what the reference
encodes an input to, or whether it accepts some bytes and as what, and
compares with its own answer.

It speaks the [conformance corpus](../conformance/README.md)'s own encoding,
so any vector case's `op`, `input` and `bytes` make a valid request.

## Running it

```sh
cargo build --release -p comline-reference
target/release/comline-reference
```

Build it at the same commit as the vectors you vendor (`VECTORS_REV`), so it
answers the way they do. It reads requests from stdin until stdin ends, then
exits 0.

## The protocol

Newline-delimited JSON on stdin and stdout. Each line is one request object,
or a JSON array of them (a batch). Each line gets one reply line of the same
shape: an object, or an array in the same order. The reply is flushed after
each line.

A request is `{"id": ..., "op": ..., "input": {...}, "bytes": "..."}`:

- `id` is any JSON value, echoed in the reply;
- `op` is a corpus op (`name_hash`, `handshake`, `envelope`, `request`,
  `response`, `encode`, `decode`), `check` or `version`;
- `input` is the op's input, as a vector case states it;
- `bytes`, lowercase hex, is optional.

### Encoding

With no `bytes`, the reply is what `input` encodes to:

```json
{"id": 1, "op": "envelope", "input": {"kind": "err", "id": 258, "body": "6669656c6473"}}
→ {"id": 1, "bytes": "0102016669656c6473"}
```

### Decoding

With `bytes`, the reply says whether they decode and, if they do, to what
input:

```json
{"id": 2, "op": "envelope", "input": {"kind": "err", "id": 258, "body": "6669656c6473"}, "bytes": "0102016669656c6473"}
→ {"id": 2, "accept": true, "input": {"kind": "err", "id": 258, "body": "6669656c6473"}}

{"id": 3, "op": "request", "input": {"framing": "datagram@1"}, "bytes": ""}
→ {"id": 3, "accept": false}
```

- A refusal carries no reason: what runtimes must agree on is *whether*
  bytes are refused.
- The input selects the decoder, and supplies what the bytes don't carry, as
  in the corpus: a `decode` needs `format` and `type`, a rejection needs only
  the framing, a datagram request takes its method name from the input.
- `decode` is decode-only: with no `bytes` it gets an `error`. `name_hash` and
  `encode` are encode-only: given `bytes`, they get an `error`
  (`"name_hash is encode-only: send it without bytes"`), never a refusal.

### Other replies

- `{"id": ..., "not_implemented": "..."}`: the reference has no
  implementation of a format or framing the request names.
- `{"id": ..., "error": "..."}`: the request is malformed. A line that isn't
  JSON, or a batch element that isn't an object, gets one with `id` `null`.
  A malformed line never stops the process.

### `check`

Whether a value is a value of a type, in the corpus's type and value
encoding, so a fuzzer can validate a pair it generated before asking about
it:

```json
{"id": 4, "op": "check", "input": {"type": "u8", "value": {"u8": 1}}}
→ {"id": 4, "ok": true}

{"id": 5, "op": "check", "input": {"type": "u8", "value": {"u16": 1}}}
→ {"id": 5, "ok": false, "why": "U16(1) isn't a U8"}
```

`why` is for a human; its text isn't part of the protocol.

### `version`

```json
{"id": 6, "op": "version"}
→ {"id": 6, "version": {"protocol": 1, "crate": "0.1.0",
     "ops": ["name_hash", "handshake", "envelope", "request", "response", "encode", "decode", "check", "version"],
     "formats": ["json@1", "msgpack@1", "packed@1"],
     "framings": ["datagram@1", "datagram@2", "jsonrpc@1"]}}
```

### Batches

```json
[{"id": "a", "op": "version"}, {"id": "b", "op": "name_hash", "input": {"name": "x"}}]
→ [{"id": "a", "version": {...}}, {"id": "b", "bytes": "..."}]
```

A batch costs one round trip, so a fuzzer sends a whole batch of inputs per
line.

## Stability

The protocol is `protocol: 1`. Under it, ops and reply fields are only ever
added. A change to an existing op's request or reply is a new protocol
version. `ops`, `formats` and `framings` in `version` say what this build
answers.

The answers are `comline-runtime`'s, the same ones the vectors are blessed
from. `tests/process.rs` replays every vector through the built binary.
