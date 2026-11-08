# Published reports

This branch is written by CI only; don't push to it by hand. It holds the
latest verification reports of `main`, which the docs
(Reference → Languages → Rust) fetch from here:

- `rust.bench.json`: instruction counts of the runtime's and the C ABI's
  benches, from `.github/workflows/bench.yml` on every push to `main`.
  Format: `runtime/conformance/benches/README.md` on `main`.
- `rust.fuzz.json`: the nightly fuzz run, from `.github/workflows/fuzz.yml`.
  Format: `runtime/fuzz/README.md` on `main`.
- `rust.size.json`, `rust.coverage.json`, `rust.compile.json`: the C ABI
  crate's artifact sizes (cdylib, staticlib, wasm32), line/function coverage
  per crate, and a clean release build's seconds per crate, from
  `.github/workflows/reports.yml` on every push to `main`. Formats: the
  Observatory's `DATA.md` (`size`, `coverage`, `compile`).

Each report names the `main` commit it measured. Every publish is one commit
here, so `git log -- rust.bench.json` is the series. The publishing step is the shared
`ComlineProject/observatory/.github/actions/publish-report` action.
