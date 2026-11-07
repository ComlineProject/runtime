#!/bin/sh
# Generates gen/*.schema.json from the specs and vets every corpus file against
# them. `--check` fails instead of writing when the generated schemas are stale.
set -eu
cd "$(dirname "$0")"

check=false
[ "${1:-}" = "--check" ] && check=true
failed=0

generate() { # <definition> <file>
    if $check; then
        if ! cue def --out jsonschema -e "#$1" . | cmp -s - "gen/$2.schema.json"; then
            echo "gen/$2.schema.json is out of date: run specs/check.sh" >&2
            failed=1
        fi
    else
        cue def --out jsonschema -e "#$1" -o "gen/$2.schema.json" -f .
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

generate Bench bench
generate Features features
generate Profile profile
generate Report report
generate Support support
generate Vectors vectors

vet Bench ../benches/*.json
vet Features ../features.json
vet Profile ../profiles/*.json
vet Report ../reports/rust.json
vet Support ../support/*.json
vet Vectors ../vectors/*.json

# The generated schemas say the same as the specs.
through() { # <schema> <file>...
    schema=$1
    shift
    for file in "$@"; do
        if ! cue vet -c "jsonschema:" "gen/$schema.schema.json" "json:" "$file"; then
            echo "$file doesn't match gen/$schema.schema.json" >&2
            failed=1
        fi
    done
}

# Not `formats.json` and the `format-*` vectors: CUE's importer misreads a
# nested `option` in these schemas, which standard validators read correctly.
through bench ../benches/connection.json ../benches/framings.json ../benches/handshake.json
through features ../features.json
through profile ../profiles/*.json
through report ../reports/rust.json
through support ../support/*.json
through vectors ../vectors/envelope.json ../vectors/framing-*.json ../vectors/handshake*.json \
    ../vectors/name-hash.json ../vectors/stream.json

exit $failed
