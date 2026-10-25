# Contributing to comline-runtime

Contributions are MPL-2.0 (see the [README](README.md#license)). Pull requests
go to `main` from a branch.

## Benchmarks and the instruction-count gate

The runtime's benchmarks are in [`runtime/core/benches`](runtime/core/benches),
measuring the work [`runtime/conformance/benches`](runtime/conformance/benches/README.md)
defines. They count instructions with Callgrind, through `iai-callgrind`; run
them with `cargo bench -p comline-runtime` (they need Valgrind, and
`iai-callgrind-runner` at the version the dev-dependency pins).

On every pull request, CI ([`bench.yml`](.github/workflows/bench.yml)) runs
them on the base and on the head, and **fails if any benchmark's instruction
count grows by more than 1%**. The job summary has the full comparison.

### When a fix costs instructions

No standard is lowered to let a change through: not the limit, and not the
tests. But a correctness fix can legitimately cost instructions. When it does,
the pull request accepts that cost explicitly, in
[`runtime/core/benches/accepted-regressions.toml`](runtime/core/benches/accepted-regressions.toml),
by appending one entry per benchmark:

```toml
[[accepted]]
pr = 123
bench = "handshake/check"
max_percent = 2.37
reason = "check the peer's ids in constant time (#120)"
```

- `pr` is this pull request's number, and `bench` the benchmark's id, as the
  job summary names it.
- `max_percent` is the most it may grow. When the gate fails, the job summary
  prints the entry to paste, with the measured growth rounded up to 0.01%.
- `reason` says why the cost is the price of the change; the review is where
  that's agreed.

The rules, which the gate enforces:

- An entry counts only in the pull request that adds it. Once merged, later
  pull requests compare against a base that already includes the cost, and
  the entry loosens nothing for them; it stays as the record.
- It covers that one benchmark, up to `max_percent`. Another benchmark, or a
  bigger growth, still fails.
- It must be needed: an entry for a benchmark that stayed within 1% fails.
- The ledger is append-only: changing or removing an existing entry fails.

The accepted regressions are listed, with their base and head counts, in the
job summary, and annotated on the entry in the pull request's diff. Nothing
else accepts a regression: no label, setting or default.
