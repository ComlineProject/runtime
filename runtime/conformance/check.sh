#!/bin/sh
# Vets this runtime's own corpus files, reports/ and support/, against the
# vendored specification's specs and the schemas generated from them. The
# specification vets everything else in its own CI; `vendor-spec.sh --check`
# keeps spec/ identical to it.
#
# `cue` runs inside spec/, which is the specs' CUE module (its cue.mod/):
# outside it, their imports don't resolve.
set -eu
cd "$(dirname "$0")/spec"

failed=0

vet() { # <definition> <file>...
    definition=$1
    shift
    for file in "$@"; do
        # `json:` because `@` in a file name otherwise reads as a package path.
        if ! cue vet -c -d "#$definition" ./specs "json:" "$file"; then
            echo "$file doesn't match #$definition" >&2
            failed=1
        fi
    done
}

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

vet Report ../reports/rust.json
vet Support ../support/*.json

# The generated schemas say the same as the specs.
through specs/gen/report.schema.json ../reports/rust.json
through specs/gen/support.schema.json ../support/*.json

exit $failed
