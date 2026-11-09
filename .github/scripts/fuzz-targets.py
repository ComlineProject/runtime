#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

"""The cargo-fuzz targets, from the one list: runtime/fuzz/Cargo.toml's
`[[bin]]` entries.

Prints them as a JSON list, which fuzz.yml's matrix and the report's
`expect` read, and fails when runtime/fuzz/README.md's targets table names
any other set, so adding a target is one edit to Cargo.toml plus its row.

    .github/scripts/fuzz-targets.py
"""

import json
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
FUZZ = ROOT / "runtime" / "fuzz"


def targets():
    with open(FUZZ / "Cargo.toml", "rb") as f:
        return [b["name"] for b in tomllib.load(f).get("bin", [])]


def documented():
    """The targets the README's `## Targets` table has a row for."""
    text = (FUZZ / "README.md").read_text()
    section = text.split("\n## Targets\n", 1)[1].split("\n## ", 1)[0]
    return re.findall(r"^\| `([a-z0-9_]+)` \|", section, re.MULTILINE)


def main():
    listed = targets()
    rows = documented()
    if sorted(rows) != sorted(listed):
        missing = sorted(set(listed) - set(rows))
        extra = sorted(set(rows) - set(listed))
        print(
            "runtime/fuzz/README.md's targets table differs from Cargo.toml's [[bin]]s:"
            + (f" no row for {', '.join(missing)}." if missing else "")
            + (f" a row for {', '.join(extra)}, which isn't a target." if extra else ""),
            file=sys.stderr,
        )
        sys.exit(1)
    print(json.dumps(listed))


if __name__ == "__main__":
    main()
