#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

"""Turns the fuzz workflow's logs into `rust.fuzz.json`; see
`runtime/fuzz/README.md` ("The report") for the format.

Each job writes a fragment, and the report job combines them:

    fuzz-report.py target --name T --toolchain TEXT --log LOG --corpus DIR \
        --artifacts DIR --wall-seconds S --out T.json
    fuzz-report.py proptest --log LOG --cases N --out proptest.json
    fuzz-report.py combine --commit SHA --run URL --fragments DIR \
        --report rust.fuzz.json [--expect T...] [--markdown OUT.md]

`target` reads libFuzzer's `-print_final_stats=1` lines and its
`Done N runs in S second(s)` line. libFuzzer prints the stats on a crash too,
but not `Done`, so the step's wall-clock time stands in for the seconds then.
A log with no stats (the target didn't build or start) gives a fragment with
`error` set and the numbers `null`. `proptest` reads the test harness's
`test NAME ... ok` lines.
"""

import argparse
import datetime
import json
import pathlib
import re
import sys

STAT = re.compile(r"^stat::(\w+):\s+(\d+)\s*$", re.M)
DONE = re.compile(r"^Done (\d+) runs in (\d+) second\(s\)\s*$", re.M)
TEST = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)\s*$", re.M)
# What libFuzzer names the inputs it saves on a finding.
FINDING = ("crash-", "leak-", "timeout-", "oom-", "slow-unit-")


def files(directory):
    d = pathlib.Path(directory)
    return [p for p in d.iterdir() if p.is_file()] if d.is_dir() else []


def target(a):
    log = pathlib.Path(a.log).read_text(errors="replace") if pathlib.Path(a.log).exists() else ""
    stats = {k: int(v) for k, v in STAT.findall(log)}
    done = DONE.findall(log)
    findings = sum(1 for p in files(a.artifacts) if p.name.startswith(FINDING))
    fragment = {
        "seconds": int(done[-1][1]) if done else (a.wall_seconds if stats else None),
        "executions": stats.get("number_of_executed_units"),
        "corpus": len(files(a.corpus)),
        "findings": findings,
    }
    if "number_of_executed_units" not in stats:
        fragment["error"] = "libFuzzer printed no final stats: the target didn't run"
    out = pathlib.Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"target": a.name, "toolchain": a.toolchain, **fragment}, indent=2) + "\n")


def proptest(a):
    log = pathlib.Path(a.log).read_text(errors="replace") if pathlib.Path(a.log).exists() else ""
    verdicts = {"ok": "pass", "FAILED": "fail"}
    properties = {name: verdicts[r] for name, r in TEST.findall(log) if r in verdicts}
    fragment = {"cases": a.cases, "properties": dict(sorted(properties.items()))}
    if not properties:
        fragment["error"] = "the test harness reported no properties: the tests didn't run"
    out = pathlib.Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(fragment, indent=2) + "\n")


def combine(a):
    targets, prop, toolchains = {}, None, set()
    for path in sorted(pathlib.Path(a.fragments).glob("**/*.json")):
        data = json.loads(path.read_text())
        if "target" in data:
            name = data.pop("target")
            toolchains.add(data.pop("toolchain"))
            targets[name] = data
        elif "properties" in data:
            prop = data
    if not targets:
        sys.exit(f"no target fragments under {a.fragments}")
    for name in a.expect or []:
        targets.setdefault(name, {"seconds": None, "executions": None, "corpus": None,
                                  "findings": None, "error": "the job left no fragment"})
    report = {
        "runtime": "rust",
        "commit": a.commit,
        "toolchain": " / ".join(sorted(toolchains)),
        "date": a.date or datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "run": a.run,
        "targets": dict(sorted(targets.items())),
        "proptest": prop,
    }
    a.report.parent.mkdir(parents=True, exist_ok=True)
    a.report.write_text(json.dumps(report, indent=2) + "\n")
    if a.markdown:
        a.markdown.write_text(markdown(report))


def number(n):
    return "—" if n is None else f"{n:,}"


def markdown(report):
    lines = [
        "## Fuzzing",
        "",
        f"Commit `{report['commit'][:12]}`, `{report['toolchain']}`, {report['date']}.",
        "",
        "| Target | Seconds | Executions | Corpus | Findings |",
        "|---|---:|---:|---:|---:|",
    ]
    for name, t in report["targets"].items():
        findings = number(t["findings"]) + (f" ({t['error']})" if t.get("error") else "")
        lines.append(f"| `{name}` | {number(t['seconds'])} | {number(t['executions'])} | "
                     f"{number(t['corpus'])} | {findings} |")
    prop = report["proptest"]
    lines.append("")
    if prop is None:
        lines.append("The property tests' long run isn't part of a pull request's run.")
    else:
        lines += [f"Property tests at {prop['cases']:,} cases each:", ""]
        lines += [f"- `{name}`: {verdict}" for name, verdict in prop["properties"].items()]
        if prop.get("error"):
            lines.append(f"- {prop['error']}")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    t = sub.add_parser("target")
    t.add_argument("--name", required=True)
    t.add_argument("--log", required=True)
    t.add_argument("--corpus", required=True)
    t.add_argument("--artifacts", required=True)
    t.add_argument("--wall-seconds", type=int, required=True)
    t.add_argument("--toolchain", required=True)
    t.add_argument("--out", required=True)

    p = sub.add_parser("proptest")
    p.add_argument("--log", required=True)
    p.add_argument("--cases", type=int, required=True)
    p.add_argument("--out", required=True)

    c = sub.add_parser("combine")
    c.add_argument("--commit", required=True)
    c.add_argument("--run", required=True)
    c.add_argument("--date", help="ISO 8601 UTC; defaults to now")
    c.add_argument("--fragments", required=True, type=pathlib.Path)
    c.add_argument("--expect", nargs="*", help="targets that must appear; missing ones get an error entry")
    c.add_argument("--report", required=True, type=pathlib.Path)
    c.add_argument("--markdown", type=pathlib.Path)

    a = parser.parse_args()
    {"target": target, "proptest": proptest, "combine": combine}[a.command](a)


if __name__ == "__main__":
    main()
