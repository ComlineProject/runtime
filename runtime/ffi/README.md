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
