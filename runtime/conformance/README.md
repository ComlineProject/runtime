# Runtime wire conformance

The Rust runtime's proof that it implements the
[Comline specification](https://github.com/ComlineProject/specification): its
byte-exact vectors, the profiles of the official encodings, and the ops
protocol. The case format, the ops, how answers are judged and the spec's
changes are all in the specification's README.

## The vendored spec

[`spec/`](spec/README.md) is the specification at the revision in
[`spec/REV`](spec/REV), unchanged. [`vendor-spec.sh`](vendor-spec.sh) writes
it:

```sh
runtime/conformance/vendor-spec.sh <rev>    # vendor <rev> and pin it
runtime/conformance/vendor-spec.sh --check  # CI: spec/ still matches spec/REV
```

`runner::SPEC_VERSION` is the vendored `spec/VERSION`.

What stays here is the Rust runtime's own: [`support/rust.json`](support/rust.json)
and [`reports/rust.json`](reports/rust.json). [`check.sh`](check.sh) vets them
against the vendored specs.

## Where the vectors come from

The Rust runtime is the reference implementation. [`src/lib.rs`](src/lib.rs)
computes every case from `comline-runtime`, and `vectors_are_current` fails
when `spec/vectors/` differs. A changed vector is a wire-protocol change.

The spec leads, so a wire change goes:

1. Change the runtime, then write its output into the vendored copy:

   ```sh
   CONFORMANCE_BLESS=1 cargo test -p comline-runtime-conformance
   ```

2. Propose the changed vectors (and profiles, specs, `VERSION` and the
   changelog) to the specification. `vendor-spec.sh --check` fails here until
   it merges.
3. Re-vendor at the merged revision.

A `decode` case's reader type is built through `TypeSeed`
([`src/seed.rs`](src/seed.rs)), which makes exactly the calls a plain `serde`
derive makes, so the cases prove generated code, not a hand-written decoder.
[`tests/seed_vs_derive.rs`](tests/seed_vs_derive.rs) holds it to real
derives, on valid and mutated bytes in every format.

[`tests/properties.rs`](tests/properties.rs) checks the reference's properties
over random types and values with `proptest`, and
[`runtime/fuzz`](../fuzz/README.md) fuzzes it with `cargo-fuzz`. Cases they
find become `found-*` vectors; the fuzz README says how.

`profiles_are_proven` checks that every case each profile fact names exists,
passes against `comline-runtime`, and shows the stated value.

## Support, and its proof

Rust's claims are in [`support/rust.json`](support/rust.json), held to the
vectors by the specification's rules. `rust_support_is_proven` writes
[`reports/rust.json`](reports/rust.json): the claims, with the cases that
prove each. The report is drift-checked like the vectors, and the docs'
language reference is rendered from it.

## The runner as a library

Other crates depend on this one by git rev
(`comline-runtime-conformance`, `runtime/conformance`). Its
[`runner`](src/runner.rs) module:

- `load_corpus(dir)`: `vectors/*.json` and `profiles/*.json` under `dir`, a
  copy of the specification such as `spec/`; `load_vectors(path)` and
  `load_profile(path)` for single files;
- `Corpus::cases_for(profile)`: every case of the features its facts are
  proven from; `Corpus::prove(profile, outcomes)`: each fact, proven or why
  not;
- `questions(case)`, `judge(case, answers)` and `byte_diff(got, expected)`;
- the `Implementation` trait, with `InProcess` (`comline-runtime`) and
  `Lines` (any reader and writer speaking the ops protocol, such as a
  child process's stdout and stdin); `run_all(cases, implementation)`.

[`comline-reference`](../reference/README.md) is `comline-runtime` as a
process speaking the ops protocol, for comparing another runtime with the
reference on inputs no vector holds.
