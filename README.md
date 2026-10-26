# Published reports

This branch is written by CI only; don't push to it by hand. It holds the
latest verification reports of `main`, which the docs
(Reference → Languages → Rust) fetch from here:

- `rust.bench.json`: instruction counts of the runtime's and the C ABI's
  benches, from `.github/workflows/bench.yml` on every push to `main`.
  Format: `runtime/conformance/benches/README.md` on `main`.
- `rust.fuzz.json`: the nightly fuzz run, from `.github/workflows/fuzz.yml`.
  Format: `runtime/fuzz/README.md` on `main`.

Each report names the `main` commit it measured. Every publish is one commit
here, so `git log -- rust.bench.json` is the series. The publishing script is
`.github/scripts/publish-report.py` on `main`.
