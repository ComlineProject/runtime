#!/bin/sh
# Writes the seed corpora (corpus/<target>/) from the conformance vectors and
# the seed-vs-derive mirrors. Generated, so not committed.
#
#   runtime/fuzz/seed-corpus.sh [corpus-dir]
#
# Uses CARGO_TARGET_DIR if set, like any cargo command.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec cargo run --quiet --manifest-path "$here/Cargo.toml" --example seed_corpus -- "${1:-$here/corpus}"
