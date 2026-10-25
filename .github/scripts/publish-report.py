#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

"""Commits reports to the `reports` branch, where the docs fetch them.

    publish-report.py [--branch reports] [--readme FILE] [--dry-run] REPORT...

Each REPORT lands at the branch's root under its own file name; the rest of
the branch is kept. `--readme` sets the branch's README.md. Every publish is
one commit on top of the branch, so its history is the series of reports
(`git log origin/reports -- rust.bench.json`), and a commit's raw URL keeps
pointing at that report.

The branch isn't the default branch, so no ruleset covers it, and nothing is
ever pushed to `main`. The workflows call this only on `push` to `main`, on
`schedule`, and on `workflow_dispatch` from `main`; never on a pull request.

A report names the `main` commit it measured (`commit`). One older than the
report already published (an ancestor of its commit) is skipped, so a slow
run can't replace a newer one; that needs the history, so check out with
`fetch-depth: 0`. A push that loses a race with another publish is retried on
top of it: each report is its own file, so they never conflict.

`--dry-run` builds the commit and prints what it would push, and pushes
nothing; it needs no write access.
"""

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time

REMOTE = "origin"
ATTEMPTS = 5
IDENTITY = {
    "GIT_AUTHOR_NAME": "github-actions[bot]",
    "GIT_AUTHOR_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
}
IDENTITY["GIT_COMMITTER_NAME"] = IDENTITY["GIT_AUTHOR_NAME"]
IDENTITY["GIT_COMMITTER_EMAIL"] = IDENTITY["GIT_AUTHOR_EMAIL"]


def git(*args, input=None, check=True):
    result = subprocess.run(["git", *args], input=input, capture_output=True, text=True,
                            env={**os.environ, **IDENTITY})
    if check and result.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed:\n{result.stderr}")
    return result


def fetch(branch):
    """The branch's head, or None if it doesn't exist yet."""
    ref = f"refs/remotes/{REMOTE}/{branch}"
    fetched = git("fetch", "--no-tags", REMOTE, f"+refs/heads/{branch}:{ref}", check=False)
    if fetched.returncode != 0:
        if "couldn't find remote ref" in fetched.stderr:
            return None
        sys.exit(f"fetching {branch} failed:\n{fetched.stderr}")
    return git("rev-parse", ref).stdout.strip()


def published_commit(parent, name):
    """The `commit` of the report `name` already on the branch, if any."""
    if parent is None:
        return None
    shown = git("show", f"{parent}:{name}", check=False)
    if shown.returncode != 0:
        return None
    try:
        return json.loads(shown.stdout).get("commit")
    except json.JSONDecodeError:
        return None


def is_older(ours, theirs):
    """Whether `ours` is a strict ancestor of `theirs`. An unknown commit
    (not in this clone's history) counts as not older."""
    if ours == theirs:
        return False
    return git("merge-base", "--is-ancestor", ours, theirs, check=False).returncode == 0


def build(parent, reports, readme):
    """`(commit, published)`: a commit on `parent` with `reports`, or None if
    there's nothing new; and the names it publishes."""
    entries = {}
    if parent is not None:
        for line in git("ls-tree", parent).stdout.splitlines():
            meta, name = line.split("\t", 1)
            entries[name] = meta
    published, commits = [], []
    for report in reports:
        ours = json.loads(report.read_text())["commit"]
        theirs = published_commit(parent, report.name)
        if theirs and is_older(ours, theirs):
            print(f"{report.name}: skipped, it measured {ours[:12]}, older than the "
                  f"published {theirs[:12]}")
            continue
        blob = git("hash-object", "-w", str(report)).stdout.strip()
        entries[report.name] = f"100644 blob {blob}"
        published.append(report.name)
        commits.append(ours)
    if not published:
        return None, []
    if readme is not None:
        blob = git("hash-object", "-w", str(readme)).stdout.strip()
        entries["README.md"] = f"100644 blob {blob}"
    tree = git("mktree", input="".join(f"{m}\t{n}\n" for n, m in sorted(entries.items()))).stdout.strip()
    if parent is not None and git("rev-parse", f"{parent}^{{tree}}").stdout.strip() == tree:
        print("The branch already has these reports.")
        return None, []
    run = os.environ.get("GITHUB_RUN_ID")
    url = (f"{os.environ.get('GITHUB_SERVER_URL')}/{os.environ.get('GITHUB_REPOSITORY')}"
           f"/actions/runs/{run}") if run else "a local run"
    message = (f"Publish {', '.join(published)} for main@{commits[0][:12]}\n\n"
               f"Measured at {', '.join(sorted(set(commits)))}, by {url}.\n")
    args = ["commit-tree", tree, "-F", "-"] + (["-p", parent] if parent else [])
    return git(*args, input=message).stdout.strip(), published


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("reports", nargs="+", type=pathlib.Path)
    parser.add_argument("--branch", default="reports")
    parser.add_argument("--readme", type=pathlib.Path)
    parser.add_argument("--dry-run", action="store_true")
    a = parser.parse_args()

    for attempt in range(1, ATTEMPTS + 1):
        parent = fetch(a.branch)
        print(f"{a.branch}: {parent or 'not created yet; this publish creates it'}")
        commit, published = build(parent, a.reports, a.readme)
        if commit is None:
            return 0
        print(f"Commit {commit} publishes {', '.join(published)}:")
        print(git("show", "--stat", "--format=%B", commit).stdout)
        if a.dry_run:
            print(f"Dry run: not pushing {commit} to {a.branch}.")
            return 0
        pushed = git("push", REMOTE, f"{commit}:refs/heads/{a.branch}", check=False)
        if pushed.returncode == 0:
            print(f"Pushed {commit} to {a.branch}.")
            return 0
        print(f"Push attempt {attempt} failed, retrying on the new head:\n{pushed.stderr}")
        time.sleep(attempt * 3)
    sys.exit(f"couldn't push to {a.branch} after {ATTEMPTS} attempts")


if __name__ == "__main__":
    sys.exit(main())
