#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

"""Turns iai-callgrind's summaries into `reports/rust.bench.json` and a
Markdown comparison against the base, and decides the instruction-count gate.

Reads every `summary.json` that `cargo bench -p comline-runtime --bench runtime
-- --save-summary=json` wrote under `<home>/comline-runtime/runtime/`, where
`<home>` is iai-callgrind's output directory (`target/iai` unless moved), and
names each benchmark by its id from `runtime/conformance/benches/README.md`.

    bench-report.py --home DIR --commit SHA --toolchain TEXT \
        --report OUT.json [--markdown OUT.md] \
        [--limit PCT --pr N --ledger HEAD.toml --base-ledger BASE.toml]

With `--limit`, it's the gate: a benchmark whose count grew by more than
`--limit` percent over the base fails, unless this pull request accepts it in
the ledger (`runtime/core/benches/accepted-regressions.toml`, see its header).
Only entries the pull request adds count: entries already on the base are
history, and loosen nothing. The exit status is the verdict: 0 passes, 1
fails.
"""

import argparse
import json
import math
import pathlib
import sys
import tomllib

FORMATS = {"json": "json@1", "msgpack": "msgpack@1", "packed": "packed@1"}
FRAMINGS = {"datagram1": "datagram@1", "datagram2": "datagram@2", "jsonrpc1": "jsonrpc@1"}
ENTRY_KEYS = {"pr", "bench", "max_percent", "reason"}


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


def change(head, base):
    """The growth from `base` to `head`, in percent."""
    if base is None:
        return None
    if base == 0:
        return 0.0 if head == 0 else math.inf
    return (head - base) / base * 100


# ── the ledger ──────────────────────────────────────────────────────────────


def read_ledger(path):
    """The ledger's entries, as dicts; none if there's no ledger."""
    if path is None or not path.exists():
        return []
    entries = tomllib.loads(path.read_text()).get("accepted", [])
    if not isinstance(entries, list):
        raise ValueError(f"{path}: `accepted` must be an array of tables ([[accepted]])")
    return entries


def ledger_line(path, bench):
    """The line of the last entry naming `bench`, for annotations."""
    if path is None or not path.exists():
        return None
    needle = json.dumps(bench)
    found = None
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if line.split("#")[0].strip().startswith("bench") and needle in line:
            found = number
    return found


def new_entries(head, base, pr, limit, problems):
    """This pull request's entries, by bench, after checking the ledger.

    The base's entries must all still be there, unchanged: the ledger is
    append-only, so it stays the record of every cost that was accepted.
    """
    def key(entry):
        return json.dumps(entry, sort_keys=True, default=str)

    head_keys = {key(e) for e in head}
    for entry in base:
        if key(entry) not in head_keys:
            problems.append(
                (entry.get("bench"), "the ledger is append-only: this pull request "
                 f"changes or removes an entry the base has (PR {entry.get('pr')})"))
    base_keys = {key(e) for e in base}
    added = {}
    for entry in head:
        if key(entry) in base_keys:
            continue
        bench = entry.get("bench")
        missing = ENTRY_KEYS - entry.keys()
        extra = entry.keys() - ENTRY_KEYS
        if missing or extra:
            problems.append((bench, f"an entry needs exactly {sorted(ENTRY_KEYS)}; "
                             f"missing {sorted(missing)}, unknown {sorted(extra)}"))
            continue
        if not isinstance(bench, str):
            problems.append((None, "`bench` must be a benchmark id string"))
            continue
        if entry["pr"] != pr:
            problems.append((bench, f"`pr = {entry['pr']}`, but this is pull request "
                             f"{pr}: an entry only counts in the pull request that adds it"))
            continue
        cap = entry["max_percent"]
        if isinstance(cap, bool) or not isinstance(cap, (int, float)) or cap <= limit:
            problems.append((bench, f"`max_percent` must be a number above the limit "
                             f"({limit:g}%), got {cap!r}"))
            continue
        if not isinstance(entry["reason"], str) or not entry["reason"].strip():
            problems.append((bench, "`reason` must say why the cost is accepted"))
            continue
        if bench in added:
            problems.append((bench, "this pull request accepts the same benchmark twice"))
            continue
        added[bench] = entry
    return added


# ── the verdict ─────────────────────────────────────────────────────────────


def judge(rows, accepted, limit, problems):
    """Each compared row's verdict: `ok`, `accepted` or `fails`; and which of
    the pull request's entries went unused."""
    verdicts = {}
    used = set()
    for bench, head, base in rows:
        pct = change(head, base)
        if pct is None or pct <= limit:
            verdicts[bench] = "ok"
            continue
        entry = accepted.get(bench)
        if entry is None:
            verdicts[bench] = "fails"
            problems.append((bench, f"grew {pct:+.2f}%, beyond the limit ({limit:g}%), "
                             "and this pull request doesn't accept it"))
        elif pct > entry["max_percent"]:
            verdicts[bench] = "fails"
            used.add(bench)
            problems.append((bench, f"grew {pct:+.2f}%, beyond the "
                             f"{entry['max_percent']}% this pull request accepts"))
        else:
            verdicts[bench] = "accepted"
            used.add(bench)
    known = {bench for bench, _, _ in rows}
    for bench in accepted:
        if bench not in known:
            problems.append((bench, "no benchmark has this id"))
        elif bench not in used:
            problems.append((bench, f"accepted, but it didn't grow beyond the limit "
                             f"({limit:g}%): remove the entry, the ledger records only "
                             "costs that were paid"))
    return verdicts


def main():
    args = argparse.ArgumentParser()
    args.add_argument("--home", required=True, type=pathlib.Path)
    args.add_argument("--commit", required=True)
    args.add_argument("--toolchain", required=True)
    args.add_argument("--report", required=True, type=pathlib.Path)
    args.add_argument("--markdown", type=pathlib.Path)
    args.add_argument("--limit", type=float, help="the gate's limit, in percent")
    args.add_argument("--pr", type=int, help="this pull request's number")
    args.add_argument("--ledger", type=pathlib.Path, help="the head's ledger")
    args.add_argument("--base-ledger", type=pathlib.Path, help="the base's ledger")
    a = args.parse_args()

    root = a.home / "comline-runtime" / "runtime"
    rows = []
    for path in sorted(root.glob("**/summary.json")):
        summary = json.loads(path.read_text())
        head, base = instructions(summary)
        rows.append((bench_id(summary), head, base))
    if not rows:
        sys.exit(f"no summaries under {root}")
    rows.sort()

    report = {
        "runtime": "rust",
        "commit": a.commit,
        "toolchain": a.toolchain,
        "metric": "instructions",
        "benches": {bench: {"ir": head} for bench, head, _ in rows},
    }
    a.report.parent.mkdir(parents=True, exist_ok=True)
    a.report.write_text(json.dumps(report, indent=2) + "\n")

    problems = []
    accepted, verdicts = {}, {}
    if a.limit is not None:
        if a.pr is None:
            sys.exit("--limit needs --pr")
        try:
            head_ledger = read_ledger(a.ledger)
            base_ledger = read_ledger(a.base_ledger)
        except (ValueError, tomllib.TOMLDecodeError) as error:
            problems.append((None, f"the ledger doesn't parse: {error}"))
            head_ledger = base_ledger = []
        accepted = new_entries(head_ledger, base_ledger, a.pr, a.limit, problems)
        if not any(base is not None for _, _, base in rows):
            for bench in accepted:
                problems.append((bench, "the base has no benchmarks to compare with"))
            accepted = {}
        verdicts = judge(rows, accepted, a.limit, problems)

    if a.markdown:
        a.markdown.write_text(markdown(rows, verdicts, accepted, problems, a))
    annotate(rows, verdicts, accepted, problems, a)
    return 1 if problems else 0


def annotate(rows, verdicts, accepted, problems, a):
    """Workflow commands, so the gate's findings show on the pull request's
    files and checks, not only in the job summary."""
    if a.ledger is None:
        return
    counts = {bench: (head, base) for bench, head, base in rows}
    for bench, entry in accepted.items():
        if verdicts.get(bench) != "accepted":
            continue
        head, base = counts[bench]
        where = location(a.ledger, bench)
        print(f"::warning {where}title=Accepted regression::{bench}: "
              f"{base:,} -> {head:,} instructions ({change(head, base):+.2f}%, "
              f"accepted up to {entry['max_percent']}%): {entry['reason']}")
    for bench, message in problems:
        where = location(a.ledger, bench) if bench in accepted else ""
        name = f"{bench}: " if bench else ""
        print(f"::error {where}title=Instruction-count gate::{name}{message}")


def location(ledger, bench):
    line = ledger_line(ledger, bench) if bench else None
    if line is None:
        return f"file={ledger},"
    return f"file={ledger},line={line},"


def markdown(rows, verdicts, accepted, problems, a):
    compared = [r for r in rows if r[2] is not None]
    lines = ["## Instruction counts", ""]
    if not compared:
        lines += [
            "The base has no benchmarks to compare with, so these are recorded only.",
            "",
            "| Benchmark | Instructions |",
            "|---|---:|",
        ]
        lines += [f"| `{bench}` | {head:,} |" for bench, head, _ in rows]
        if problems:
            lines += ["", "### Gate failures", ""]
            lines += [f"- {f'`{b}`: ' if b else ''}{m}" for b, m in problems]
        return "\n".join(lines) + "\n"

    failing = [b for b, v in verdicts.items() if v == "fails"]
    passed = [b for b, v in verdicts.items() if v == "accepted"]
    if problems:
        lines.append(f"**The gate fails**: {len(failing)} beyond the limit (`ir` "
                     f"{a.limit:g}%) and not accepted; {len(problems)} "
                     f"problem{'s' if len(problems) != 1 else ''} in all.")
    elif passed:
        lines.append(f"**The gate passes with {len(passed)} accepted regression"
                     f"{'s' if len(passed) != 1 else ''}**; everything else is within "
                     f"the limit (`ir` {a.limit:g}%).")
    else:
        lines.append(f"No regression beyond the limit (`ir` {a.limit:g}%).")
    changed = [r for r in compared if r[1] != r[2]]
    lines.append(
        f"{len(changed)} of {len(compared)} compared benchmarks changed; "
        f"{len(rows) - len(compared)} are new."
    )

    counts = {bench: (head, base) for bench, head, base in rows}
    if passed:
        lines += [
            "",
            "### Accepted regressions",
            "",
            f"Accepted by this pull request in `{a.ledger}`; they count here only.",
            "",
            "| Benchmark | Base | Head | Change | Accepted up to | Reason |",
            "|---|---:|---:|---:|---:|---|",
        ]
        for bench in sorted(passed):
            head, base = counts[bench]
            entry = accepted[bench]
            reason = " ".join(entry["reason"].split()).replace("|", "\\|")
            lines.append(f"| `{bench}` | {base:,} | {head:,} | "
                         f"{change(head, base):+.2f}% | {entry['max_percent']}% | {reason} |")

    if problems:
        lines += ["", "### Gate failures", ""]
        lines += [f"- {f'`{b}`: ' if b else ''}{m}" for b, m in problems]
    if failing:
        lines += [
            "",
            "If the cost is the price of a fix, and reviewed as such, accept it by",
            f"appending to `{a.ledger}` (see its header):",
            "",
            "```toml",
        ]
        for bench in sorted(failing):
            head, base = counts[bench]
            cap = math.ceil(change(head, base) * 100) / 100
            lines += [
                "[[accepted]]",
                f"pr = {a.pr}",
                f"bench = {json.dumps(bench)}",
                f"max_percent = {cap}",
                'reason = "<why this cost is the price of the change>"',
                "",
            ]
        lines[-1] = "```"

    lines += ["", "### All benchmarks", "",
              "| Benchmark | Base | Head | Change |", "|---|---:|---:|---:|"]

    # Changed first, worst first; then the rest.
    def order(r):
        bench, head, base = r
        if base is None:
            return (2, 0, bench)
        return (0 if head != base else 1, -change(head, base), bench)

    for bench, head, base in sorted(rows, key=order):
        if base is None:
            lines.append(f"| `{bench}` | | {head:,} | new |")
            continue
        text = "=" if head == base else f"{change(head, base):+.2f}%"
        verdict = verdicts.get(bench)
        if verdict == "fails":
            text = f"**{text}** :x:"
        elif verdict == "accepted":
            text = f"{text} (accepted)"
        lines.append(f"| `{bench}` | {base:,} | {head:,} | {text} |")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    sys.exit(main())
