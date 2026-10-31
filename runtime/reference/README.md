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
- `op` is a corpus op (`name_hash`, `handshake`, `handshake_check`,
  `envelope`, `request`, `response`, `encode`, `decode`, `stream_prefix`,
  `stream_split`), `check`,
  `version` or `session`;
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
- `decode` and `handshake_check` are decode-only: with no `bytes` they get an
  `error`. `name_hash` and
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
     "ops": ["name_hash", "handshake", "handshake_check", "envelope", "request", "response",
             "encode", "decode", "stream_prefix", "stream_split", "check", "version", "session"],
     "formats": ["json@1", "msgpack@1", "packed@1"],
     "framings": ["datagram@1", "datagram@2", "jsonrpc@1"]}}
```

### `session`

One side of a connection, driven through a list of events: the Rust
runtime's sans-IO connection state (`connection::State`) hands out request
ids, tracks the calls waiting on a reply, and says what each received frame
means. A runtime in another language runs the same events through its own
connection and compares the steps, which must be identical.

```json
{"id": 7, "op": "session", "input": {"framing": "datagram@2", "ordered": true, "events": [
  {"call": {"protocol": 1, "call_id": 2, "method": "outer", "params": "aa"}},
  {"call": {"protocol": 1, "call_id": 3, "method": "inner", "params": "bb"}},
  {"call": {"protocol": 1, "call_id": 4, "method": "note", "one_way": true, "params": ""}},
  {"receive": "0101000000000000000000"},
  {"receive": "000105000700000000000000cc"},
  {"reply": {"protocol": 1, "request_id": "7", "envelope": {"kind": "err", "id": 3, "body": "ee"}}},
  {"receive": "010101000000000000000011"},
  {"abandon": "0"},
  {"receive": "010100000000000000000022"},
  {"receive": "ff"}
]}}
→ {"id": 7, "steps": [
  {"frame": "000102000000000000000000aa", "request_id": "0"},
  {"frame": "000103000100000000000000bb", "request_id": "1"},
  {"frame": "000104000200000000000000", "request_id": "2"},
  {"error": "framing"},
  {"event": {"request": {"protocol": 1, "call_id": 5, "request_id": "7", "one_way": false, "params": "cc"}}},
  {"frame": "01010700000000000000010300ee"},
  {"event": {"reply": {"request_id": "1", "envelope": {"kind": "ok", "payload": "11"}}}},
  {},
  {"event": "stale"},
  {"error": "framing"}
]}
```

(The fourth event answers the outer call while the inner one still waits,
which an ordered connection refuses; the ninth answers the outer call after
it was abandoned.)

The input:

- `framing`: `datagram@2` or `jsonrpc@1`, the framings a connection uses.
  Any other is `not_implemented`.
- `ordered`: `true` for the nested, blocking connection, where a reply must
  answer the innermost waiting call (Rust's `State::default()`); `false`
  when calls are in flight at once and answered in any order
  (`State::unordered()`, the async driver).
- `events`, in order, each an object with one key:
  - `{"call": {"protocol", "call_id", "method", "one_way", "params"}}`: a
    local call. `params` is the body already encoded in the session's
    format, in hex, and goes into the frame as it is: under `jsonrpc@1` it
    has to be JSON text. `one_way` is optional (`false`); a one-way call is
    never waited on, and under `jsonrpc@1` it's a notification. `datagram@2`
    puts `protocol` and `call_id` on the wire, `jsonrpc@1` only `method`; all
    three are required either way.
  - `{"receive": "<hex frame>"}`: a frame from the peer.
  - `{"abandon": "<request id>"}`: stop waiting on a call (a timeout). An id
    nobody waits on changes nothing.
  - `{"reply": {"protocol", "request_id", "envelope"}}`: this side answers
    the peer's request `request_id`. `envelope` is in the vectors' form
    (`{"kind": "ok", "payload": "<hex>"}`, `{"kind": "err", "id": n, "body":
    "<hex>"}`, `{"kind": "fault"}`).

Request ids are decimal strings, as in the vectors. Ids start at 0 and go
up by one per call, one-way calls included.

The reply's `steps` has one entry per event:

- a `call`: `{"frame": "<hex>", "request_id": "<decimal>"}`;
- a `receive`:
  - `{"event": {"request": {"protocol", "call_id" | "method", "request_id", "one_way", "params"}}}`:
    the peer calls this side. `protocol` is there when the framing carries
    it (`datagram@2`). The call is `call_id` under `datagram@2` and `method`
    under `jsonrpc@1`. `one_way` is whether the frame asked for no reply,
    which only `jsonrpc@1` can say (a notification, whose `request_id` is
    `"0"`). `params` is the body as the frame carries it.
  - `{"event": {"reply": {"request_id", "envelope"}}}`: the reply to a
    waiting call, which stops waiting. `envelope` is in the vectors' form.
  - `{"event": "stale"}`: a reply to an id nobody waits on: never handed
    out, already answered, abandoned, or a one-way call's.
  - `{"error": "framing"}`: a malformed frame, or, when `ordered`, a reply
    that skips the innermost waiting call (both calls keep waiting). Either
    way nothing changes.
- an `abandon`: `{}`;
- a `reply`: `{"frame": "<hex>"}`.

`error` names a `RuntimeError` in snake case; `receive` only ever gives
`framing`. A malformed event (an unknown key, bad hex, a missing field)
makes the whole request an `error` reply, with no steps.

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
