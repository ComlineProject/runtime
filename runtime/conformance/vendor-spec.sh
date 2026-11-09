#!/usr/bin/env bash
# Vendors ComlineProject/specification (the specs, vectors, profiles, benches,
# features.json and VERSION) into spec/, at a pinned commit.
#
#   vendor-spec.sh <rev>   # vendor <rev> and pin it in spec/REV
#   vendor-spec.sh --check # fail if spec/ differs from spec/REV's
#
# The spec leads: a wire change lands in specification first, and this
# repository vendors it. `CONFORMANCE_BLESS=1` writes into spec/ so the change
# can be proposed there; `--check` fails until it is merged and re-vendored.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
dest="$here/spec"
repo="https://github.com/ComlineProject/specification"

if [[ "${1:-}" == "--check" ]]; then
  rev="$(cat "$dest/REV")"
else
  rev="${1:?usage: vendor-spec.sh <rev> | --check}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
git clone --quiet --no-checkout "$repo" "$tmp/specification"
mkdir "$tmp/spec"
git -C "$tmp/specification" archive "$rev" ':!.github' | tar -x -C "$tmp/spec"

if [[ "${1:-}" == "--check" ]]; then
  if diff -r --exclude=REV "$tmp/spec" "$dest"; then
    echo "spec/ matches $repo@$rev"
  else
    echo "spec/ differs from $repo@$rev: re-vendor with vendor-spec.sh <rev>" >&2
    exit 1
  fi
  exit 0
fi

rm -rf "$dest"
mv "$tmp/spec" "$dest"
git -C "$tmp/specification" rev-parse "$rev^{commit}" > "$dest/REV"
echo "vendored $repo@$(cat "$dest/REV")"
echo "Now: cargo test -p comline-runtime-conformance"
