# Cue Specs

CUE definitions of every file in the corpus, and the JSON Schemas generated
from them (`gen/`), which the files name in `$schema` for editors. The
benches' schema is generated into `benches/bench.schema.json` instead, so a
runtime that vendors `benches/` alone has it.

| Definition | Files | Spec |
| --- | --- | --- |
| `#Bench` | `benches/*.json` | [`bench.cue`](bench.cue) |
| `#CompilerCases` | `compiler/cases/*.json` | [`compiler.cue`](compiler.cue) |
| `#Features` | `features.json` | [`features.cue`](features.cue) |
| `#Profile` | `profiles/*.json` | [`profile.cue`](profile.cue) |
| `#Report` | `reports/<runtime>.json`, in each runtime's repository | [`report.cue`](report.cue) |
| `#Support` | `support/<runtime>.json`, in each runtime's repository | [`support.cue`](support.cue) |
| `#Vectors` | `vectors/*.json` | [`vectors.cue`](vectors.cue) |

Values and types (`#Value`, `#Type`, `#SizeType`) are in [`types.cue`](types.cue), and what
the others share in [`common.cue`](common.cue). Which facts a profile states,
and their values, come from [the catalog](../catalog/readme.md), which the
specs import: the CUE module (`cue.mod/`) is the repository's root, so run
`cue` anywhere inside it. `$schema` is optional in every
file: a third party's needn't have one.

## Building and verifying

`./check.sh` regenerates `gen/` and `benches/bench.schema.json` from the
specs, and `../features.json` and `../catalog/gen/catalog.json` from the
catalog, and vets every file. It also checks that `profiles/` and `vectors/`
hold exactly what the catalog names, and, with [`jq`](https://jqlang.github.io/jq/)
([`sizes.jq`](sizes.jq)), that each format's size rules are proven by its
size vectors: every rule lists cases that exist and show it, every case proves
a rule, and every witness is as long as its case says. It generates
`../compiler/cases/` from `../compiler/src/` (the [compiler cases](../compiler/readme.md)),
and, with [`compiler.jq`](compiler.jq), holds them to what their schema can't say.
It also holds each framing's `protocol` fact to the cases it names
([`protocol.jq`](protocol.jq)). CI runs `./check.sh --check`, which fails
instead when a generated file is stale. It needs
[`cue`](https://cuelang.org) at the version `.github/workflows/check.yml`
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
