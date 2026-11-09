#!/bin/sh
# Generates gen/*.schema.json (and benches/bench.schema.json, which ships with
# the benches so a vendored copy of them is whole) from the specs and vets
# every corpus file against them. `--check` fails instead of writing when the
# generated schemas are stale.
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
generate Features gen/features.schema.json
generate Profile gen/profile.schema.json
generate Report gen/report.schema.json
generate Support gen/support.schema.json
generate Vectors gen/vectors.schema.json

# Not the schema the benches ship with.
vet Bench $(ls ../benches/*.json | grep -v '/bench\.schema\.json$')
vet Features ../features.json
vet Profile ../profiles/*.json
vet Vectors ../vectors/*.json

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
through gen/vectors.schema.json ../vectors/envelope.json ../vectors/framing-*.json ../vectors/handshake*.json \
    ../vectors/name-hash.json ../vectors/stream.json

exit $failed
