# Cue Specs

CUE definitions of every file in the corpus, and the JSON Schemas generated
from them (`gen/`), which the files name in `$schema` for editors.

| Definition | Files | Spec |
| --- | --- | --- |
| `#Bench` | `benches/*.json` | [`bench.cue`](bench.cue) |
| `#Features` | `features.json` | [`features.cue`](features.cue) |
| `#Profile` | `profiles/*.json` | [`profile.cue`](profile.cue) |
| `#Report` | `reports/<runtime>.json` | [`report.cue`](report.cue) |
| `#Support` | `support/*.json` | [`support.cue`](support.cue) |
| `#Vectors` | `vectors/*.json` | [`vectors.cue`](vectors.cue) |

Values and types (`#Value`, `#Type`) are in [`types.cue`](types.cue), and what
the others share in [`common.cue`](common.cue). `$schema` is optional in every
file: a third party's needn't have one.

## Building and verifying

`./check.sh` regenerates `gen/` and vets every file, and CI runs
`./check.sh --check`, which fails instead when `gen/` is stale. It needs
[`cue`](https://cuelang.org) at the version `.github/workflows/build_test.yml`
pins.

By hand, from this directory, for one schema and one file (`json:` because `@`
in a file name otherwise reads as a package path):

```sh
cue def --out jsonschema -e '#Profile' -o gen/profile.schema.json -f .
cue vet -c -d '#Profile' . json: ../profiles/json@1.json
```

`-c` matters: without it, a missing required field passes.

## What the specs can't say

`cargo test -p comline-runtime-conformance` holds the rest: that a fact's cases
show its value, that a generated number fits its type, that `"self"` is inside
a `recursive` type, and that a case's bytes are what its input encodes to.

## Writing a definition

- Quote a label that would shadow a predeclared type (`"bool"!: bool`): unquoted,
  the value refers to the field itself.
- Spell a closed struct out in full, rather than with `&` or an embedded
  definition, which the JSON Schema export gets wrong.
- Keep patterns to what both RE2 and ECMAScript read: editors validate with the
  latter, so no `(?s)`.
