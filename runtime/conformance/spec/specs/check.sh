#!/bin/sh
# Generates gen/*.schema.json (and benches/bench.schema.json, which ships with
# the benches so a vendored copy of them is whole) from the specs, and
# features.json, catalog/gen/catalog.json and rust/src/catalog.rs from the
# catalog, and compiler/cases/*.json from compiler/src, and vets every corpus
# file against them (and, with jq, holds the size rules to their vectors and the
# compiler cases to what their schema can't say). `--check` fails instead of
# writing when a generated file is stale.
set -eu
cd "$(dirname "$0")"

check=false
[ "${1:-}" = "--check" ] && check=true
failed=0

generate() { # <definition> <path>
    if $check; then
        if ! cue def --out jsonschema -e "#$1" . | cmp -s - "$2"; then
            echo "$2 is out of date: run specs/check.sh" >&2
            failed=1
        fi
    else
        cue def --out jsonschema -e "#$1" -o "$2" -f .
    fi
}

vet() { # <definition> <file>...
    definition=$1
    shift
    for file in "$@"; do
        # `json:` because `@` in a file name otherwise reads as a package path.
        if ! cue vet -c -d "#$definition" . "json:" "$file"; then
            echo "$file doesn't match #$definition" >&2
            failed=1
        fi
    done
}

generate Bench ../benches/bench.schema.json
generate CompilerCases gen/compiler.schema.json
generate Features gen/features.schema.json
generate Profile gen/profile.schema.json
generate Report gen/report.schema.json
generate Support gen/support.schema.json
generate Vectors gen/vectors.schema.json

# What the catalog exports: features.json, and the whole catalog as JSON for
# readers without CUE.
export_json() { # <expression> <path>
    if $check; then
        if ! cue export ../catalog -e "$1" | cmp -s - "$2"; then
            echo "$2 is out of date: run specs/check.sh" >&2
            failed=1
        fi
    else
        cue export ../catalog -e "$1" -o "$2" -f
    fi
}

cue vet -c ../catalog || failed=1
export_json features_json ../features.json
export_json catalog ../catalog/gen/catalog.json

# The catalog as Rust data, for the comline-catalog crate (rust/), with the
# spec's version from VERSION. Tabs become spaces, as Rust is written.
rust_catalog() {
    cue export ../catalog -t "version=$(cat ../VERSION)" -e rust_catalog --out text | expand -t 4
}
if $check; then
    if ! rust_catalog | cmp -s - ../rust/src/catalog.rs; then
        echo "../rust/src/catalog.rs is out of date: run specs/check.sh" >&2
        failed=1
    fi
else
    rust_catalog > ../rust/src/catalog.rs
fi

# The compiler cases, written compactly in compiler/src, as the JSON runners
# read (compiler/readme.md): one file per group.
groups=$(cue eval ../compiler/src -e 'strings.Join([for g, _ in groups {g}], "\n")' --out text)
for group in $groups; do
    if $check; then
        if ! cue export ../compiler/src -e "out.\"$group\"" | cmp -s - "../compiler/cases/$group.json"; then
            echo "../compiler/cases/$group.json is out of date: run specs/check.sh" >&2
            failed=1
        fi
    else
        mkdir -p ../compiler/cases
        cue export ../compiler/src -e "out.\"$group\"" -o "../compiler/cases/$group.json" -f
    fi
done

# profiles/ and vectors/ hold exactly what the catalog names.
same_names() { # <what> <catalog set> <files' names>
    if [ "$(printf '%s\n' "$2" | sort)" != "$(printf '%s\n' "$3" | sort)" ]; then
        echo "$1 differ from the catalog's: expected" $2 "- found" $3 >&2
        failed=1
    fi
}
names_in() { # <directory>
    for file in "$1"/*.json; do basename "$file" .json; done
}
same_names profiles "$(cue export ../catalog -e 'strings.Join(sets.profiled, "\n")' --out text)" "$(names_in ../profiles)"
same_names vectors "$(cue export ../catalog -e 'strings.Join(sets.features, "\n")' --out text)" "$(names_in ../vectors)"
# compiler/cases/ holds exactly the groups compiler/src declares.
same_names "compiler cases" "$groups" "$(names_in ../compiler/cases)"
for file in ../profiles/*.json; do
    if ! cue vet -c -d '#OfficialProfile' ../catalog "json:" "$file"; then
        echo "$file isn't the profile the catalog gives its encoding" >&2
        failed=1
    fi
    if [ "$(cue export "json:" "$file" -e name --out text)" != "$(basename "$file" .json)" ]; then
        echo "$file names another encoding" >&2
        failed=1
    fi
done

# A format's size rules are proven by its size vectors: every rule lists cases
# that exist and show it, every case proves a rule, and every witness is as long
# as its case says (`specs/sizes.jq`).
for file in ../profiles/*.json; do
    feature=$(jq -r '.sizes.feature // empty' "$file")
    [ -n "$feature" ] || continue
    if ! problems=$(jq -n -r --slurpfile profile "$file" --slurpfile vectors "../vectors/$feature.json" -f sizes.jq); then
        echo "$file or ../vectors/$feature.json can't be read as size rules and vectors" >&2
        failed=1
    elif [ -n "$problems" ]; then
        echo "$file's size rules aren't proven by ../vectors/$feature.json:" >&2
        echo "$problems" >&2
        failed=1
    fi
done

# A framing's `protocol` is shown by its cases: `named` by requests whose bytes
# hold the protocol's number, `unnamed` by requests that give none
# (`specs/protocol.jq`).
for file in ../profiles/*.json; do
    [ "$(jq -r '.kind' "$file")" = framing ] || continue
    if ! problems=$(jq -s -r --slurpfile profile "$file" -f protocol.jq ../vectors/framing-*.json); then
        echo "$file or the framing vectors can't be read as a protocol fact and vectors" >&2
        failed=1
    elif [ -n "$problems" ]; then
        echo "$file's protocol isn't proven by its vectors:" >&2
        echo "$problems" >&2
        failed=1
    fi
done

# Not the schema the benches ship with.
vet Bench $(ls ../benches/*.json | grep -v '/bench\.schema\.json$')
vet Features ../features.json
vet Profile ../profiles/*.json
vet Vectors ../vectors/*.json
vet CompilerCases ../compiler/cases/*.json

# What the compiler cases' schema can't say (`specs/compiler.jq`).
for file in ../compiler/cases/*.json; do
    if ! problems=$(jq -r -f compiler.jq "$file"); then
        echo "$file can't be read as compiler cases" >&2
        failed=1
    elif [ -n "$problems" ]; then
        echo "$file has problems:" >&2
        echo "$problems" >&2
        failed=1
    fi
done

# `reports/` and `support/` live in each runtime's repository, which vets them
# against its vendored copy of these specs.

# The generated schemas say the same as the specs.
through() { # <schema path> <file>...
    schema=$1
    shift
    for file in "$@"; do
        if ! cue vet -c "jsonschema:" "$schema" "json:" "$file"; then
            echo "$file doesn't match $schema" >&2
            failed=1
        fi
    done
}

# Not `formats.json` and the `format-*` vectors: CUE's importer misreads a
# nested `option` in these schemas, which standard validators read correctly.
through ../benches/bench.schema.json ../benches/connection.json ../benches/framings.json ../benches/handshake.json
through gen/features.schema.json ../features.json
through gen/profile.schema.json ../profiles/*.json
through gen/compiler.schema.json ../compiler/cases/*.json
through gen/vectors.schema.json ../vectors/envelope.json ../vectors/framing-*.json ../vectors/handshake*.json \
    ../vectors/name-hash.json ../vectors/stream.json

exit $failed
