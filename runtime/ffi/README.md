# comline-runtime-ffi

The C ABI over `comline-runtime`: the `comline@1` handshake, the envelopes,
the connection framings (`datagram@2`, `jsonrpc@1`), connection state and
frame limits, for every language that isn't Rust. Codecs, transports, async
and dispatch stay in each host.

- `src/lib.rs`: the ABI, and the rules every function keeps.
- `include/comline.h`: generated from `src/` by cbindgen 0.29.2
  (`cbindgen.toml`) and checked in. CI fails when it drifts:
  `cbindgen --config cbindgen.toml --output include/comline.h`.
- `js/comline.mjs`, `js/comline.d.mts`: hand-written JS glue over the
  wasm32 build, with no dependencies. See [The wasm32 build](#the-wasm32-build).

## Proving the boundary

The conformance vectors (`runtime/conformance/vectors`) are replayed through
the ABI from three hosts, on the same cases with the same checks. Each
prints its count per file and fails on any mismatch or a case count that
differs from the vectors'.

**Rust** (`tests/vectors.rs`, through the `extern "C"` functions only):

```sh
cargo test -p comline-runtime-ffi
cargo +nightly miri test -p comline-runtime-ffi --features test-hooks
```

**The export** that C and Node read: one line per case, params pre-encoded
(the format is documented at the top of `examples/export_vectors.rs`):

```sh
cargo run -p comline-runtime-ffi --example export_vectors -- target/ffi-vectors
```

**C** (`tests/c/vectors.c`, C11 against `comline.h`): builds the libraries,
exports, and runs against the staticlib, the cdylib, and the staticlib
under ASan and UBSan. Each also runs alone (`static`, `dynamic`,
`sanitize`).

```sh
make -C runtime/ffi/tests/c          # CC=gcc or CC=clang
```

**Node** (`js/test/vectors.test.mjs`, `node --test`, built-ins only) against
the wasm32 build, after the export, once through each way of loading it
(`instantiate`, `instantiateSync` and `instantiateStreaming`, from bytes
and from a module):

```sh
cargo build -p comline-runtime-ffi --profile wasm --target wasm32-unknown-unknown
cargo build -p comline-runtime-ffi --profile wasm --target wasm32-unknown-unknown \
  --features test-hooks --target-dir "${CARGO_TARGET_DIR:-target}/test-hooks"
node --test runtime/ffi/js/test/*.test.mjs
```

The other Node tests are the glue's own (below): `poison.test.mjs` against
the `test-hooks` build, and `regions.test.mjs` and `bound.test.mjs`.

The C and Node harnesses look under `CARGO_TARGET_DIR` (or `target`);
`COMLINE_VECTORS`, `COMLINE_WASM` and `COMLINE_WASM_TEST_HOOKS` point Node
elsewhere, and `VECTORS` does the same for `make`.

**Against the core, on random sequences.** Beyond the vectors, the ABI is
run against `comline-runtime` itself: random configs and sequences (calls,
replies, abandons, peer frames, chunked deliveries, flipped bytes) go
through a `comline_connection` and through `State` and the framings
directly, which must give the same frames, events and refusals, while every
call is held to the ABI's rules (documented statuses, spans inside the
input, retry sizes, no request id spent on a refusal, no allocation after
`comline_connection_new`, except `jsonrpc@1`'s decode, runtime#58). The driver is `tests/common/drive.rs`; it runs on
proptest's sequences in `tests/differential.rs`, and on fuzzer input in the
`ffi_feed` (rules only) and `ffi_vs_core` targets in
[`runtime/fuzz`](../fuzz/README.md).

```sh
PROPTEST_CASES=100000 cargo test --release -p comline-runtime-ffi --test differential
cargo +nightly fuzz run --fuzz-dir runtime/fuzz ffi_vs_core
```

## What crossing costs

`runtime/ffi/bench` counts instructions (Callgrind, through
`iai-callgrind`) of `feed`, `encode_request` and `encode_reply` per framing
(`datagram@2`, `jsonrpc@1`), in message and stream mode, at the runtime
benches' payload sizes, and of `name_hash`, `handshake_encode` and
`handshake_check`. Each is measured twice: through its `extern "C"`
function (`ffi/abi/...`), and as the same work on `State` and the framings
directly (`ffi/core/...`). CI gates both like the runtime's benches (see
`CONTRIBUTING.md` at the repository's root), and its job summary pairs them
in a crossing-cost table. The benches are a package of their own so the ABI
is built as a host links it, without this crate's `test-hooks`.

```sh
cargo bench -p comline-runtime-ffi-bench   # needs Valgrind and iai-callgrind-runner
```

## The wasm32 build

The same `extern "C"` functions are the module's exports, with two more
for the glue, `comline_wasm_reserve_in` and `comline_wasm_reserve_out`. It
is built with the `wasm` profile (the root `Cargo.toml`), and only wasm32
is: native builds keep `release`.

**A trap poisons the instance, not just the handle.** On wasm32 a panic
doesn't unwind: it aborts, which is a trap (`WebAssembly.RuntimeError`)
that the ABI's `catch_unwind` can't catch, so the module never returns
`COMLINE_ERR_POISONED` itself, and the trap leaves the whole instance
unusable. `comline.mjs` marks the instance dead on a trap (or on a
`RangeError` from inside the module, the engine's stack exhaustion): that
call and every later one on the instance and on each of its connections
return `Status.ERR_POISONED` without entering the module. A binding drops
the instance and its connections and instantiates again. Other exceptions,
such as a `TypeError` for a bad argument, are thrown and poison nothing.
`js/test/poison.test.mjs` proves it on the `test-hooks` build, whose
`comline_test_panic_next` makes the next call on a handle panic.

**One crossing per call.** The glue keeps where the `in` and `out` regions
are and how big, and reserves again only when a call needs more. That
rests on the module's promise (`mod wasm` in `src/lib.rs`): a region moves,
grows or is freed only inside its own reserve call, and only when asked
for more than it holds. Memory may still grow in any call, which detaches
the glue's views but moves nothing, so the glue takes them again when
`memory.buffer` changes. `js/test/regions.test.mjs` holds the module to
the promise (through allocations, frees and memory growth) and the glue to
one crossing per call.

**Loading.** `instantiate(bytes | module)`; `instantiateSync(bytes |
module)` for Node and workers (a browser's main thread refuses to compile a
module this size synchronously); `instantiateStreaming(response)` for
browsers, which reads the whole body and compiles that instead when the
engine has no `WebAssembly.instantiateStreaming` or the response isn't
`application/wasm`. Each checks the ABI version and the layouts the same
way.

**Sizing an encode.** `encodeBound({ framing, stream, kind, bodyLen, ... })`
is the most bytes an encode can write, so `cap` never needs a retry. Given
everything the frame holds (`datagram@2`: `protocol`; `jsonrpc@1`: the
`name`, ids and error id) it is exact, from the framings' writers in
`comline-runtime`: `datagram@2`'s fixed header with its protocol varint,
and `jsonrpc@1`'s fixed members, its name escaped as serde_json escapes it,
and decimal ids. Given only lengths it is never smaller: a name byte
counted at 6 (`\u00XX`), an id at 20 digits, an error id at 5, a varint at
3. The vector harness encodes every case at the exact bound, which must be
what is written; `js/test/bound.test.mjs` does the same on random encodes
(every control character, quotes, backslashes, non-ASCII and lone
surrogates in names, reply ids up to `u64::MAX`, bodies to 70000 bytes,
stream and message mode), encodes at the loose bound, which must succeed
the first time, reports its slack, and catches a planted off-by-one.

**The profile.** `wasm` inherits `release` (`opt-level = 2`) with
`lto = "fat"`, `codegen-units = 1` and `panic = "abort"` (wasm32's own
behaviour, so the trap test holds unchanged). Measured on the glue's
round trip (two encodes and two feeds, a 230-byte body, Node 20; wall time
on a loaded machine, so only differences of 10% or more mean anything):

| build | `.wasm` | gzip -9 | `datagram@2` | `jsonrpc@1` |
|---|---|---|---|---|
| `release` (opt 2, 16 units, no fat LTO) | 208,930 | 77,971 | 4.7–5.5 µs | 5.9–6.4 µs |
| `wasm`, opt 2 (the default) | 197,771 | 76,325 | 4.6–5.5 µs | 6.1–7.2 µs |
| `wasm`, opt 3 | 198,061 | 76,345 | 4.6–5.5 µs | 6.8–7.0 µs |
| `wasm`, opt "s" | 174,152 | 63,568 | 4.8–5.0 µs | 7.4–7.6 µs |
| `wasm`, opt "z" | 187,903 | 66,517 | 5.3–5.9 µs | 9.4–9.9 µs |

The glue's call itself dominates these, so `opt-level` is left to the
TypeScript runtime's instruction counts, which measure what wall time
can't.

