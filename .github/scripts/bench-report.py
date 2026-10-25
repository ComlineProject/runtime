#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

"""Turns iai-callgrind's summaries into `reports/rust.bench.json` and a
Markdown comparison against the base.

Reads every `summary.json` that `cargo bench -p comline-runtime --bench runtime
-- --save-summary=json` wrote under `<home>/comline-runtime/runtime/`, where
`<home>` is iai-callgrind's output directory (`target/iai` unless moved), and
names each benchmark by its id from `runtime/conformance/benches/README.md`.

    bench-report.py --home DIR --commit SHA --toolchain TEXT \
        --report OUT.json [--limit PCT] [--markdown OUT.md]
"""

import argparse
import json
import pathlib
import sys

FORMATS = {"json": "json@1", "msgpack": "msgpack@1", "packed": "packed@1"}
FRAMINGS = {"datagram1": "datagram@1", "datagram2": "datagram@2", "jsonrpc1": "jsonrpc@1"}


def bench_id(summary):
    """The definitions' id for one iai-callgrind benchmark."""
    group = summary["module_path"].split("::")[-2]
    function = summary["function_name"]
    case = summary.get("id") or ""
    op, _, rest = function.partition("_")
    if group == "formats" and case in FORMATS:
        return f"format/{op}/{rest.replace('_', '-')}/{FORMATS[case]}"
    if group == "framings" and rest in FRAMINGS and case.startswith("b"):
        return f"framing/{op}/{FRAMINGS[rest]}/{case[1:]}"
    if group == "handshake":
        return f"handshake/{rest}"
    if group == "connection":
        return f"connection/{rest}"
    return "/".join(p for p in (group, function, case) if p)


def instructions(summary):
    """`(head, base)` instruction counts; `base` is None with no baseline."""
    total = summary["profiles"][0]["summaries"]["total"]
    metrics = total["summary"]["Callgrind"]["Ir"]["metrics"]
    if "Both" in metrics:
        new, old = metrics["Both"]
        return new["Int"], old["Int"]
    if "Left" in metrics:
        return metrics["Left"]["Int"], None
    raise ValueError(f"no head count in {summary['module_path']}")


def regressed(summary):
    return bool(summary["profiles"][0]["summaries"]["total"]["regressions"])


def main():
    args = argparse.ArgumentParser()
    args.add_argument("--home", required=True, type=pathlib.Path)
    args.add_argument("--commit", required=True)
    args.add_argument("--toolchain", required=True)
    args.add_argument("--report", required=True, type=pathlib.Path)
    args.add_argument("--markdown", type=pathlib.Path)
    args.add_argument("--limit", help="the regression limit, for the Markdown")
    a = args.parse_args()

    root = a.home / "comline-runtime" / "runtime"
    rows = []
    for path in sorted(root.glob("**/summary.json")):
        summary = json.loads(path.read_text())
        head, base = instructions(summary)
        rows.append((bench_id(summary), head, base, regressed(summary)))
    if not rows:
        sys.exit(f"no summaries under {root}")
    rows.sort()

    report = {
        "runtime": "rust",
        "commit": a.commit,
        "toolchain": a.toolchain,
        "metric": "instructions",
        "benches": {bench: {"ir": head} for bench, head, _, _ in rows},
    }
    a.report.parent.mkdir(parents=True, exist_ok=True)
    a.report.write_text(json.dumps(report, indent=2) + "\n")

    if a.markdown:
        a.markdown.write_text(markdown(rows, a))


def markdown(rows, a):
    compared = [r for r in rows if r[2] is not None]
    worse = [r for r in rows if r[3]]
    lines = ["## Instruction counts", ""]
    if not compared:
        lines += [
            "The base has no benchmarks to compare with, so these are recorded only.",
            "",
            "| Benchmark | Instructions |",
            "|---|---:|",
        ]
        lines += [f"| `{bench}` | {head:,} |" for bench, head, _, _ in rows]
        return "\n".join(lines) + "\n"

    changed = [r for r in compared if r[1] != r[2]]
    if worse:
        lines.append(f"**{len(worse)} regressed** beyond the limit (`ir` {a.limit}).")
    else:
        lines.append(f"No regression beyond the limit (`ir` {a.limit}).")
    lines.append(
        f"{len(changed)} of {len(compared)} compared benchmarks changed; "
        f"{len(rows) - len(compared)} are new."
    )
    lines += ["", "| Benchmark | Base | Head | Change |", "|---|---:|---:|---:|"]
    # Changed first, worst first; then the rest.
    def order(r):
        bench, head, base, _ = r
        if base is None:
            return (2, 0, bench)
        change = (head - base) / base if base else 0
        return (0 if head != base else 1, -change, bench)

    for bench, head, base, bad in sorted(rows, key=order):
        if base is None:
            lines.append(f"| `{bench}` | | {head:,} | new |")
            continue
        pct = (head - base) / base * 100 if base else 0.0
        change = "=" if head == base else f"{pct:+.2f}%"
        if bad:
            change = f"**{change}** :x:"
        lines.append(f"| `{bench}` | {base:,} | {head:,} | {change} |")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    main()
