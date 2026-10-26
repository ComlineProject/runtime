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
  wasm32 build, with no dependencies.

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
the release wasm32 build, after the export:

```sh
cargo build -p comline-runtime-ffi --release --target wasm32-unknown-unknown
node --test runtime/ffi/js/test/vectors.test.mjs
```

The C and Node harnesses look under `CARGO_TARGET_DIR` (or `target`);
`COMLINE_VECTORS` and `COMLINE_WASM` point Node elsewhere, and `VECTORS`
does the same for `make`.

**Against the core, on random sequences.** Beyond the vectors, the ABI is
run against `comline-runtime` itself: random configs and sequences (calls,
replies, abandons, peer frames, chunked deliveries, flipped bytes) go
through a `comline_connection` and through `State` and the framings
directly, which must give the same frames, events and refusals, while every
call is held to the ABI's rules (documented statuses, spans inside the
input, retry sizes, no request id spent on a refusal, no allocation after
`comline_connection_new`). The driver is `tests/common/drive.rs`; it runs on
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
