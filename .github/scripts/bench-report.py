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
        [--limit PCT --pr N --base-home DIR \
         --ledger HEAD.toml --base-ledger BASE.toml]

With `--limit`, it's the gate. `--base-home` holds the base's own summaries,
which say what benchmarks the base had and their counts. Then:

- a benchmark whose count grew by more than `--limit` percent over the base
  fails, unless this pull request accepts it (`[[accepted]]`);
- a benchmark the base had and the head doesn't run fails, unless this pull
  request removes it (`[[removed]]`). One removed with `renamed_to` is
  compared under its new id against its old count, like any other;
- a benchmark that's new, and not a rename, is recorded only.

The ledger is `runtime/core/benches/accepted-regressions.toml` (see its
header). Only entries the pull request adds count: entries already on the base
are history, and loosen nothing. The exit status is the verdict: 0 passes, 1
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
# Each table of the ledger: its required keys, and its optional ones.
TABLES = {
    "accepted": ({"pr", "bench", "max_percent", "reason"}, set()),
    "removed": ({"pr", "bench", "reason"}, {"renamed_to"}),
}


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


def summaries(home):
    """`{id: (count, baseline count or None)}` for every summary under `home`."""
    root = home / "comline-runtime" / "runtime"
    found = {}
    for path in sorted(root.glob("**/summary.json")):
        summary = json.loads(path.read_text())
        found[bench_id(summary)] = instructions(summary)
    return found


def change(head, base):
    """The growth from `base` to `head`, in percent."""
    if base is None:
        return None
    if base == 0:
        return 0.0 if head == 0 else math.inf
    return (head - base) / base * 100


# ── the ledger ──────────────────────────────────────────────────────────────


def read_ledger(path):
    """`{table: [entry, ...]}`; empty tables if there's no ledger."""
    ledger = {table: [] for table in TABLES}
    if path is None or not path.exists():
        return ledger
    data = tomllib.loads(path.read_text())
    unknown = data.keys() - TABLES.keys()
    if unknown:
        raise ValueError(f"{path}: unknown tables {sorted(unknown)}")
    for table in TABLES:
        entries = data.get(table, [])
        if not isinstance(entries, list) or not all(isinstance(e, dict) for e in entries):
            raise ValueError(f"{path}: `{table}` must be an array of tables ([[{table}]])")
        ledger[table] = entries
    return ledger


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


def new_entries(table, head, base, pr, problems):
    """This pull request's entries in `table`, by bench, with their keys
    checked.

    The base's entries must all still be there, unchanged: the ledger is
    append-only, so it stays the record of every cost that was accepted.
    """
    def key(entry):
        return json.dumps(entry, sort_keys=True, default=str)

    required, optional = TABLES[table]
    head_keys = {key(e) for e in head}
    for entry in base:
        if key(entry) not in head_keys:
            problems.append(
                (entry.get("bench"), f"the ledger is append-only: this pull request "
                 f"changes or removes a [[{table}]] entry the base has "
                 f"(PR {entry.get('pr')})"))
    base_keys = {key(e) for e in base}
    added = {}
    for entry in head:
        if key(entry) in base_keys:
            continue
        bench = entry.get("bench")
        missing = required - entry.keys()
        extra = entry.keys() - required - optional
        if missing or extra:
            problems.append((bench, f"a [[{table}]] entry needs {sorted(required)}"
                             f"{f', may have {sorted(optional)}' if optional else ''}; "
                             f"missing {sorted(missing)}, unknown {sorted(extra)}"))
            continue
        if not isinstance(bench, str):
            problems.append((None, f"[[{table}]]: `bench` must be a benchmark id string"))
            continue
        if entry["pr"] != pr:
            problems.append((bench, f"[[{table}]] `pr = {entry['pr']}`, but this is pull "
                             f"request {pr}: an entry only counts in the pull request "
                             "that adds it"))
            continue
        if not isinstance(entry["reason"], str) or not entry["reason"].strip():
            problems.append((bench, f"[[{table}]]: `reason` must say why"))
            continue
        if bench in added:
            problems.append((bench, f"this pull request has two [[{table}]] entries for it"))
            continue
        added[bench] = entry
    return added


def check_caps(accepted, limit, problems):
    for bench, entry in list(accepted.items()):
        cap = entry["max_percent"]
        if isinstance(cap, bool) or not isinstance(cap, (int, float)) or cap <= limit:
            problems.append((bench, f"`max_percent` must be a number above the limit "
                             f"({limit:g}%), got {cap!r}"))
            del accepted[bench]


# ── the verdict ─────────────────────────────────────────────────────────────


def pair(head, base, removed, problems):
    """The rows to judge, `(bench, head, base, renamed_from)`, with each
    rename's new id given its old id's base count; and the removed benchmarks
    `(bench, base, entry)`. A base benchmark the head lacks, with no
    `[[removed]]` entry, fails here."""
    renamed = {}
    for old, entry in removed.items():
        new = entry.get("renamed_to")
        if old not in base:
            problems.append((old, "removed, but the base has no benchmark with this id"))
        elif old in head:
            problems.append((old, "removed, but the head still runs it"))
        elif new is not None:
            if not isinstance(new, str) or new not in head:
                problems.append((old, f"`renamed_to = {new!r}`, but the head runs no "
                                 "benchmark with that id"))
            elif new in base:
                problems.append((old, f"`renamed_to = {json.dumps(new)}`, but the base "
                                 "already had that id; a rename must be to a new one"))
            elif new in renamed:
                problems.append((old, f"`renamed_to = {json.dumps(new)}`, and so is "
                                 f"`{renamed[new]}`"))
            else:
                renamed[new] = old
    for bench in sorted(base.keys() - head.keys()):
        if bench not in removed:
            problems.append((bench, "the base has this benchmark and the head doesn't run "
                             "it; if that's intended, say so with a [[removed]] entry "
                             "(with `renamed_to` for a rename)"))
    rows = []
    for bench, (count, baseline) in sorted(head.items()):
        old = renamed.get(bench)
        if old is not None:
            rows.append((bench, count, base[old][0], old))
        else:
            rows.append((bench, count, baseline, None))
    gone = [(b, base[b][0], e) for b, e in sorted(removed.items())
            if b in base and b not in head]
    return rows, gone


def judge(rows, accepted, limit, problems):
    """Each compared row's verdict: `ok`, `accepted` or `fails`."""
    verdicts = {}
    used = set()
    for bench, head, base, renamed_from in rows:
        pct = change(head, base)
        if pct is None or pct <= limit:
            verdicts[bench] = "ok"
            continue
        what = f"grew {pct:+.2f}%" + (f" from `{renamed_from}`" if renamed_from else "")
        entry = accepted.get(bench)
        if entry is None:
            verdicts[bench] = "fails"
            problems.append((bench, f"{what}, beyond the limit ({limit:g}%), and this "
                             "pull request doesn't accept it"))
        elif pct > entry["max_percent"]:
            verdicts[bench] = "fails"
            used.add(bench)
            problems.append((bench, f"{what}, beyond the {entry['max_percent']}% this "
                             "pull request accepts"))
        else:
            verdicts[bench] = "accepted"
            used.add(bench)
    known = {r[0] for r in rows}
    for bench in accepted:
        if bench not in known:
            problems.append((bench, "accepted, but no benchmark has this id"))
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
    args.add_argument("--base-home", type=pathlib.Path,
                      help="the base's own summaries; absent if the base has no benches")
    args.add_argument("--ledger", type=pathlib.Path, help="the head's ledger")
    args.add_argument("--base-ledger", type=pathlib.Path, help="the base's ledger")
    a = args.parse_args()

    head = summaries(a.home)
    if not head:
        sys.exit(f"no summaries under {a.home}")

    report = {
        "runtime": "rust",
        "commit": a.commit,
        "toolchain": a.toolchain,
        "metric": "instructions",
        "benches": {bench: {"ir": count} for bench, (count, _) in sorted(head.items())},
    }
    a.report.parent.mkdir(parents=True, exist_ok=True)
    a.report.write_text(json.dumps(report, indent=2) + "\n")

    problems = []
    accepted, gone, verdicts = {}, [], {}
    rows = [(b, c, base, None) for b, (c, base) in sorted(head.items())]
    if a.limit is not None:
        if a.pr is None:
            sys.exit("--limit needs --pr")
        try:
            head_ledger = read_ledger(a.ledger)
            base_ledger = read_ledger(a.base_ledger)
        except (ValueError, tomllib.TOMLDecodeError) as error:
            problems.append((None, f"the ledger doesn't parse: {error}"))
            head_ledger = base_ledger = read_ledger(None)
        new = {t: new_entries(t, head_ledger[t], base_ledger[t], a.pr, problems)
               for t in TABLES}
        accepted, removed = new["accepted"], new["removed"]
        check_caps(accepted, a.limit, problems)
        base = summaries(a.base_home) if a.base_home and a.base_home.exists() else {}
        if base:
            rows, gone = pair(head, base, removed, problems)
        else:
            for bench in [*accepted, *removed]:
                problems.append((bench, "the base has no benchmarks to compare with"))
            accepted = {}
        verdicts = judge(rows, accepted, a.limit, problems)
        a.entries = {**removed, **accepted}

    if a.markdown:
        a.markdown.write_text(markdown(rows, gone, verdicts, accepted, problems, a))
    annotate(rows, gone, verdicts, accepted, problems, a)
    return 1 if problems else 0


def annotate(rows, gone, verdicts, accepted, problems, a):
    """Workflow commands, so the gate's findings show on the pull request's
    files and checks, not only in the job summary."""
    if a.ledger is None:
        return
    for bench, head, base, renamed_from in rows:
        if verdicts.get(bench) != "accepted":
            continue
        entry = accepted[bench]
        was = f" (was `{renamed_from}`)" if renamed_from else ""
        print(f"::warning {location(a.ledger, bench)}title=Accepted regression::"
              f"{bench}{was}: {base:,} -> {head:,} instructions "
              f"({change(head, base):+.2f}%, accepted up to {entry['max_percent']}%): "
              f"{entry['reason']}")
    for bench, base, entry in gone:
        new = entry.get("renamed_to")
        what = f"renamed to {new}" if new else "removed"
        print(f"::notice {location(a.ledger, bench)}title=Benchmark {what}::"
              f"{bench} ({base:,} instructions on the base): {entry['reason']}")
    entries = getattr(a, "entries", {})
    for bench, message in problems:
        where = location(a.ledger, bench) if bench in entries else ""
        name = f"{bench}: " if bench else ""
        print(f"::error {where}title=Instruction-count gate::{name}{message}")


def location(ledger, bench):
    line = ledger_line(ledger, bench) if bench else None
    if line is None:
        return f"file={ledger},"
    return f"file={ledger},line={line},"


def cell(text):
    return " ".join(text.split()).replace("|", "\\|")


def markdown(rows, gone, verdicts, accepted, problems, a):
    compared = [r for r in rows if r[2] is not None]
    lines = ["## Instruction counts", ""]
    if not compared:
        lines += [
            "The base has no benchmarks to compare with, so these are recorded only."
            if a.limit is not None else "The head alone, with nothing to compare with.",
            "",
            "| Benchmark | Instructions |",
            "|---|---:|",
        ]
        lines += [f"| `{bench}` | {head:,} |" for bench, head, _, _ in rows]
        if problems:
            lines += ["", "### Gate failures", ""]
            lines += [f"- {f'`{b}`: ' if b else ''}{m}" for b, m in problems]
        return "\n".join(lines) + "\n"

    failing = [b for b, v in verdicts.items() if v == "fails"]
    passed = [r for r in rows if verdicts.get(r[0]) == "accepted"]
    if problems:
        lines.append(f"**The gate fails**: {len(problems)} "
                     f"problem{'s' if len(problems) != 1 else ''}, listed below; "
                     f"the limit is `ir` {a.limit:g}%.")
    elif passed:
        lines.append(f"**The gate passes with {len(passed)} accepted regression"
                     f"{'s' if len(passed) != 1 else ''}**; everything else is within "
                     f"the limit (`ir` {a.limit:g}%).")
    else:
        lines.append(f"No regression beyond the limit (`ir` {a.limit:g}%).")
    changed = [r for r in compared if r[1] != r[2]]
    lines.append(
        f"{len(changed)} of {len(compared)} compared benchmarks changed; "
        f"{len(rows) - len(compared)} are new; {len(gone)} removed or renamed."
    )

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
        for bench, head, base, renamed_from in passed:
            entry = accepted[bench]
            was = f" (was `{renamed_from}`)" if renamed_from else ""
            lines.append(f"| `{bench}`{was} | {base:,} | {head:,} | "
                         f"{change(head, base):+.2f}% | {entry['max_percent']}% | "
                         f"{cell(entry['reason'])} |")
    if gone:
        lines += [
            "",
            "### Removed and renamed benchmarks",
            "",
            f"Recorded by this pull request in `{a.ledger}`. A renamed one is "
            "compared under its new id against its old count.",
            "",
            "| Benchmark | Base | Now | Reason |",
            "|---|---:|---|---|",
        ]
        for bench, base, entry in gone:
            new = entry.get("renamed_to")
            now = f"`{new}`" if new else "removed"
            lines.append(f"| `{bench}` | {base:,} | {now} | {cell(entry['reason'])} |")

    if problems:
        lines += ["", "### Gate failures", ""]
        lines += [f"- {f'`{b}`: ' if b else ''}{m}" for b, m in problems]
    if failing:
        counts = {r[0]: r for r in rows}
        lines += [
            "",
            "If the cost is the price of a fix, and reviewed as such, accept it by",
            f"appending to `{a.ledger}` (see its header):",
            "",
            "```toml",
        ]
        for bench in sorted(failing):
            _, head, base, _ = counts[bench]
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
        bench, head, base, _ = r
        if base is None:
            return (2, 0, bench)
        return (0 if head != base else 1, -change(head, base), bench)

    for bench, head, base, renamed_from in sorted(rows, key=order):
        if base is None:
            lines.append(f"| `{bench}` | | {head:,} | new |")
            continue
        text = "=" if head == base else f"{change(head, base):+.2f}%"
        verdict = verdicts.get(bench)
        if verdict == "fails":
            text = f"**{text}** :x:"
        elif verdict == "accepted":
            text = f"{text} (accepted)"
        was = f" (was `{renamed_from}`)" if renamed_from else ""
        lines.append(f"| `{bench}`{was} | {base:,} | {head:,} | {text} |")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    sys.exit(main())
