# Compiler conformance cases

Schema graphs as data, each with the outcome any compiler must reproduce:
core, the language server, or another tool, the way a runtime must reproduce
the [vectors](../README.md#the-corpus)' bytes. The cases say which guarantees a
protocol gives up, who acknowledged them and why, and which errors and
warnings a build reports, for the rules of
[the proposal](https://github.com/ComlineProject/docs/blob/proposals/shared-vocabulary/docs/docs/proposals/shared-vocabulary/granular-unverified.md)
that core implements today. They are
[core#224](https://github.com/ComlineProject/core/issues/224)'s, and the
outcomes are those of core at `d1b678a`.

| What | Where |
| --- | --- |
| The cases, one JSON file a group | [`cases/<group>.json`](cases) |
| The format of a file, in CUE, and as JSON Schema | [`specs/compiler.cue`](../specs/compiler.cue), [`specs/gen/compiler.schema.json`](../specs/gen/compiler.schema.json) |
| The cases' source, which the JSON is generated from | [`src/`](src) |
| What the schema can't say, checked by `jq` | [`specs/compiler.jq`](../specs/compiler.jq) |

`specs/check.sh` regenerates `cases/` from `src/` and vets it, and CI runs it
with `--check`, which fails when a file is stale. Edit `src/`, never `cases/`.

## A case

```json
{
  "id": "marker/missing-marker",
  "description": "a protocol with no handshake gives up two guarantees and has to say why",
  "packages": [
    {
      "name": "app",
      "config": "congregation app\nspecification_version = 1\nsettings = { annotations = { unverified = { allowed = true } } }\n",
      "schemas": [
        {
          "path": "src/api.ids",
          "text": "@handshake = \"none\"\nprotocol P {\n    function f();\n}\n"
        }
      ],
      "dependencies": []
    }
  ],
  "build": "app",
  "expect": {
    "builds": false,
    "diagnostics": [
      {
        "code": "unverified::missing-marker",
        "severity": "error"
      }
    ]
  }
}
```

- **`id`** is `<group>/<name>`, the group being the file's. An id is stable,
  as a vector's is.
- **`description`** is one line.
- **`packages`** are the packages of the case: the one to build and those it
  depends on, directly or not. Each has a `name`, which is its congregation's,
  its directory's and how its dependants say it in `use`; its `config.idp` as
  `config`, whole; its `schemas`, each a `path` under the package
  (`src/<namespace>.ids`, as on disk) and its `text`; and its `dependencies`:
  the packages it depends on by path (`../<name>`) and, for each, what the
  config accepts of its unverified protocols (`accept_unverified`, a protocol
  and the guarantee ids it gives up). The config text declares the same: the
  compiler reads that, and `dependencies` restates it so a runner can lay the
  packages out and a check can hold them to each other without reading
  `config.idp`. A package never depends on itself or in a cycle.
- **`build`** names the package to build.
- **`expect`** is the outcome, below.

Texts are written as they are, with the newlines JSON escapes. In a schema's
annotation a dictionary's entries are separated by commas
(`@unverified = { a = "..." , b = "..." }`); in `config.idp` by whitespace.

### What is expected

**`builds`** says whether the package builds.

For a build that **succeeds**:

- **`protocols`** lists every protocol of the package built, not those of its
  dependencies, and no others, each with:
  - **`given_up`**: the guarantee ids it gives up, the set frozen with it.
    An order says nothing. A protocol that gives up nothing has `[]`.
  - **`acknowledged`**, where the case asserts it: for each guarantee listed,
    the **`scope`** it was acknowledged at, `protocol` (the protocol's own
    `@unverified`, which wins where both say) or `package` (`config.idp`'s
    `unverified`, which core freezes with the protocol as
    `unverified_package`), and the **`reason`** shown for it. A guarantee not
    listed isn't asserted. Only a guarantee given up is listed.
- **`diagnostics`** are the warnings the build reports for the package's own
  schemas and config, and no others: a warning of a dependency is its authors'
  to act on, and is not the case's.

For a build that **fails**:

- **`diagnostics`** are the errors with a code, and no others.
- **`error_contains`**, for an error without a code, lists text its message
  contains. Core reports a dependency that isn't accepted as a plain error.
  The substrings are minimal and as stable as core's wording; they are
  **provisional**, and go when those errors get codes.
- A failing build says why with one or the other. It has no `protocols`, and
  no warnings: they aren't reported once a build fails.

A diagnostic is `{ "code", "severity" }`, matched on both and never on message
text, as a set: each once, in any order. The codes the corpus uses are a
closed list in [`specs/compiler.cue`](../specs/compiler.cue): a rule's code
that isn't there has no case yet. A case is written so that its outcome has
one cause, so which stage of a compiler stops the build first doesn't matter.

## Running a case

For each case in each file of `cases/`:

1. Write every package into its own directory, side by side:
   `<root>/<name>/config.idp` from `config`, and each schema at its `path`
   under `<root>/<name>/`.
2. Build the package named by `build`, as `comline build` would resolve its
   dependencies from the paths its config declares, and no further (no
   fetching, no code generation).
3. Compare: whether it built, then per `expect`. The frozen set and the
   acknowledgements are what the build froze with each protocol; the
   diagnostics are what it reported, by code and severity.

A tool that can only compile one package's sources in memory can't run the
cases with `dependencies`, which are in `acceptance` and `chain`.

## The corpus

| Group | Covers |
| --- | --- |
| `marker` | the gate (`annotations.unverified.allowed`); a marker required where a protocol gives something up (its own `@handshake`, the package's default); its reason; a marker where nothing is given up |
| `keyed` | `@unverified = { <guarantee> = "<reason>" }`: the exact set, an unknown id, a missing one, one listed that isn't given up, one listed twice |
| `package` | `unverified` in `config.idp`: the keyed and plain forms covering what a protocol takes from the package's settings; a protocol with its own `@handshake` not covered; `unverified::wrong-scope` for `bounded_size`; a guarantee no protocol gives up through it; the gate; a marker adding `bounded_size` |
| `bounded` | `bounded_size`: a string or list without a maximum, a type that contains itself without `@max_depth`, a type from another schema of the package that holds one, each unmarked (an error) and marked (given up), and the bounded variants |
| `acceptance` | a dependency's unverified protocols accepted under it in `config.idp`: not at all, exactly, for less, for more; the reasons the consumer is shown; the dependency judged by its own package |
| `chain` | three packages, `c` depending on `b` depending on `a`: a type holding `a`'s unbounded value, used in `c`; and the acceptance resolving `b` asks for `a`'s protocols too |

A case's `description` says what it shows. Each rule has a passing and a
failing variant.

### To come

Cases for rules core doesn't implement yet, which go in when it does:
acknowledgements at the inner scopes (function, argument, field and type),
an outer scope covering an inner one, `strict`, pinning an acceptance to a
chain of packages and its invalidation, and failing closed.

## Taking the cases

Vendor `compiler/cases/*.json` at a pinned revision of this repository, as core
does the size vectors (`core/tests/spec/`): a `REV` file with the commit, a
`vendor.sh --check` in CI that fails when the copy differs from it, and a test
that the `REV` is the `comline-catalog` revision core is pinned to, since the
cases name guarantees as the catalog does. `specs/gen/compiler.schema.json` is
the format as JSON Schema, for a reader that wants it; a reader ignores the
`$schema` each file names.

The files of a group are never split or renamed, and ids don't change. A
group or a case may be added, and a case's expected outcome changed, only as
the compiler's behaviour changes, in a pull request that says so.

## Writing a case

Cases are written in [`src/`](src), in CUE, where a package's files are
multi-line strings and a protocol's expected outcome a struct; the comment
at the top of [`src/lib.cue`](src/lib.cue) says how it becomes JSON. Run
`specs/check.sh` to regenerate `cases/`.

A case should have one cause: where two things are wrong at once, which one a
tool reports first can differ, so it isn't a good case. Derive the outcome from
the compiler's tests and code, and state in the pull request any you aren't
sure of.
