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

### Removing or renaming a benchmark

A benchmark the base had and the head doesn't run also fails the gate, so a
slowed benchmark can't get through by being renamed or dropped in the same
pull request. When that's intended, append a `[[removed]]` entry, under the
same rules (this pull request's `pr`, needed, append-only):

```toml
[[removed]]
pr = 123
bench = "handshake/check"
renamed_to = "handshake/verify"   # only for a rename
reason = "the check is now called verify (#121)"
```

A renamed benchmark is compared under its new id against its old id's base
count, so it's still held to 1%, or to an `[[accepted]]` entry for the new id.
A new benchmark that isn't a rename has nothing to regress from, and is
recorded only.

### Where it shows

The accepted regressions are listed, with their base and head counts, in the
job summary, as are removals and renames, and each entry is annotated in the
pull request's diff. Nothing
else accepts a regression: no label, setting or default.
