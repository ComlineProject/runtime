# Fuzzing the reference

Coverage-guided fuzz targets (`cargo-fuzz`, libFuzzer) against
`comline-runtime` and the conformance crate's `TypeSeed`: the Rust runtime is
the reference every other runtime is checked against, so its decoders must be
strict, canonical and panic-free.
[`conformance/tests/properties.rs`](../conformance/tests/properties.rs)
covers the structured side with `proptest` (round trips over random types and
values), and runs on stable with `cargo test`.

This crate is outside the workspace and needs nightly.

## Running

```sh
cargo install cargo-fuzz --locked
runtime/fuzz/seed-corpus.sh                  # writes runtime/fuzz/corpus/<target>/
cargo +nightly fuzz run --fuzz-dir runtime/fuzz decode_no_panic -- \
    -max_total_time=120 -rss_limit_mb=2048 -malloc_limit_mb=512
```

- Run from the repository root; `--fuzz-dir` points at this crate.
- Pass `--target-dir <dir>` to keep the instrumented build apart from the
  workspace's `target/`: it is built with sanitizer flags, and sharing a
  directory rebuilds both.
- **Always pass `-rss_limit_mb` and `-malloc_limit_mb`.** Bounded allocation is
  part of what `decode_no_panic` checks: a single allocation past
  `-malloc_limit_mb` is a crash, and so is the process passing `-rss_limit_mb`.
- `PROPTEST_CASES=100000 cargo test --release -p comline-runtime-conformance --test properties`
  is the property tests' long run.

## Targets

| Target | Input | Property |
|---|---|---|
| `decode_no_panic` | a format, a type, bytes | `decode_typed` never panics, stays within the memory limits, and anything it accepts passes `check` |
| `packed_canonical` | a type, bytes | bytes that decode as the type in `packed@1` re-encode to exactly themselves |
| `framing_decode` | bytes | through `datagram@1` request and response decode, `datagram@2`, JSON-RPC request, response and connection decode, `Envelope::decode` and `HandshakeFrame::decode`; no panic, and see below |
| `seed_vs_derive` | a format, a mirror, bytes | `TypeSeed` accepts and refuses what the mirror's `serde` derive does, and reads the same value; the mirrors are [`conformance/tests/common`](../conformance/tests/common/mod.rs) |

**What `framing_decode` checks.**
- The binary frames are canonical, so what decodes re-encodes to exactly the
  input.
- Two decoders read a prefix and ignore the rest: a `Fault` envelope (its tag
  alone) and the fixed-size handshake. They behave the same way in
  comline-typescript. Their re-encoding must be a prefix of the input.
- JSON-RPC isn't byte-canonical: whitespace, member order and escapes vary. A
  response or reply is re-encoded and decoded again, and must decode the same.
  JSON-RPC requests are only decoded, since encoding needs a `'static` method
  name.

**The input grammar.** A type is read from the front of the input, one byte
per choice (see [`src/lib.rs`](src/lib.rs)). This is deliberately not a derived
`Arbitrary`: [`examples/seed_corpus.rs`](examples/seed_corpus.rs) writes any
vector's type back in it, so the seeds reach the vectors' shapes.

The generated types have the same bounds as the property tests:
- depth 4;
- up to 6 fields per struct and 5 variants per enum;
- fixed arrays up to 40 over a scalar and up to 4 over anything else;
- names from fixed pools, since the interner leaks one entry per distinct name.

Neither generator produces an array of a type that carries no data (unit,
`T[0]`, a struct of only such fields). Core refuses such a schema
(ComlineProject/core#141, `validation::array-of-dataless-type`). `packed@1`
refuses a count past the bytes left even for such an element, and the vector
`format-packed/reject-count-past-input` pins that.

## Seed corpora

`seed-corpus.sh` runs `examples/seed_corpus.rs`, which writes the following into
`corpus/<target>/`, prefixed as each target reads them:
- every conformance vector's bytes, from `reference_cases()`;
- every mirror's samples, in every format.

The corpora are generated, so they aren't committed (`.gitignore`), and neither
are `artifacts/`.

## CI

[`.github/workflows/fuzz.yml`](../../.github/workflows/fuzz.yml):

- **Every pull request:** each target runs for 60 seconds, seeded from the
  vectors.
- **Nightly:**
  - each target runs for 10 minutes on a corpus cached between runs;
  - the property tests run with `PROPTEST_CASES=100000`.

  A failure uploads the crash and opens an issue titled after the target,
  unless one with that title is already open. It doesn't block anyone's pull
  request.

## A finding becomes a vector

1. **Minimize it.**

   ```sh
   cargo +nightly fuzz tmin --fuzz-dir runtime/fuzz <target> artifacts/<target>/crash-…
   ```

   Proptest shrinks by itself and saves the seed in
   `conformance/tests/properties.proptest-regressions`.
2. **Decide whether it's a bug in the runtime or the seed, or in the target.**
   Never weaken an assertion to make a finding go away. If the behaviour is
   intended, the property was wrong, and its doc comment says why.
3. **For a runtime or seed bug:**
   - fix it, with a unit test where the bug is;
   - add the minimized input to `reference_cases()` in
     [`conformance/src/lib.rs`](../conformance/src/lib.rs) as a `found-*` case
     (`found_cases()`), such as `format-json/found-f64-exact-digits`. Its
     expected bytes, value or refusal come from the reference like every other
     case.
   - bless the vectors: `CONFORMANCE_BLESS=1 cargo test -p comline-runtime-conformance`.

   From then on every runtime checks the case on every run.
4. **For behaviour that is intended but was implicit,** pin it with a
   vector, as `format-packed/reject-count-past-input` does.

### Found so far

- `format-json/found-f64-exact-digits`: `serde_json`'s default float parser
  read `4.520618856987477e-279` back one ulp off. `comline-runtime` now
  enables serde_json's `float_roundtrip` feature, which makes float parsing
  slower (serde_json estimates about 2x).
- `format-packed/reject-count-past-input`: `packed@1` couldn't read back an
  array of data-less elements, such as `[Empty {}]`, which is `01`. Core now
  refuses such schemas (ComlineProject/core#141), and the decoder stays strict.
