"""The check behind CI's one required job, `CI passed` (D-169, replacing D-130).

`needs.<job>.result` is not enough: GitHub can report a matrix job as passed when one of its legs
was cancelled before a runner took it. So the gate also lists the run's jobs, as GitHub's API gives
them, and checks them against the jobs the tier expects:

- every job in the list that the tier expects ran and passed;
- every job in the run finished, and passed or was skipped;
- every job in the run that passed is in the list, so the list keeps up with ci.yml.

The list is one job's name per line, as the run shows it (a matrix leg with its part). A name may
be followed by a condition tag:
- `[fast]`: expected only in the fast Team PR gate;
- `[slow]`: expected whenever the run is not the fast Team PR gate (external/fork PR, `ci:full`,
  manual dispatch, or hourly `canary`);
- `[full]`: expected only in the full gate;
- `[models]`: expected only when the TLA+ models run.
Any untagged job is expected in every tier.

Usage, with the list in the environment variable EXPECTED:

    passed.py --jobs JOBS --fast true|false --full true|false --models true|false [--prefix PREFIX]

JOBS holds one JSON object per line, each with the job's `name`, `status` and `conclusion`, as
`gh api .../actions/runs/RUN/jobs --jq '.jobs[] | {name, status, conclusion}'` writes them.
When called from another workflow via `workflow_call`, pass `--prefix` (for example `"CI / "`)
to filter to the called workflow's jobs and strip the caller prefix before checking EXPECTED.
"""

import argparse
import json
import os
import sys

GATE = "CI passed"
CONDITIONS = ("fast", "slow", "full", "models")


def parse_expected(text):
    """The list as (name, condition) pairs; the condition is None or one of CONDITIONS."""
    expected = []
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        condition = None
        if line.endswith("]") and " [" in line:
            line, _, tag = line.rpartition(" [")
            condition = tag[:-1]
            if condition not in CONDITIONS:
                raise ValueError(f"unknown condition [{condition}] in the list of expected jobs")
        expected.append((line, condition))
    if not expected:
        raise ValueError("the list of expected jobs is empty")
    return expected


def check(expected_text, jobs, full, models, prefix="", fast=False):
    """What is wrong with the run's `jobs`, as messages: none when the gate passes."""
    if fast and (full or models):
        return [f"invalid tier combination: fast=True cannot combine with full={full}, models={models}"]
    expected = parse_expected(expected_text)
    applies = {
        None: True,
        "fast": fast,
        "slow": not fast,
        "full": full,
        "models": models,
    }
    listed = {name for name, _ in expected}
    errors = []
    if prefix:
        scoped = [
            dict(j, name=j["name"][len(prefix):])
            for j in jobs
            if j["name"].startswith(prefix)
        ]
        if not scoped:
            return [f"no jobs in this run matched prefix {prefix!r}"]
    else:
        scoped = jobs
    others = [j for j in scoped if j["name"] != GATE]
    for j in others:
        name, status, conclusion = j["name"], j["status"], j["conclusion"]
        if status != "completed":
            errors.append(f"{name}: not finished ({status})")
        elif conclusion not in ("success", "skipped"):
            errors.append(f"{name}: {conclusion}")
        elif conclusion == "success" and name not in listed:
            errors.append(f"{name}: passed, but is not in the list of expected jobs in ci.yml")
    for name, condition in expected:
        if not applies[condition]:
            continue
        found = [j for j in others if j["name"] == name]
        if not found:
            errors.append(f"{name}: never ran, though this tier needs it")
        for j in found:
            if j["status"] == "completed" and j["conclusion"] == "skipped":
                errors.append(f"{name}: skipped, though this tier needs it")
    return errors


def flag(value):
    if value not in ("true", "false"):
        raise argparse.ArgumentTypeError(f"expected true or false, not {value!r}")
    return value == "true"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--jobs", required=True, help="the run's jobs, one JSON object per line")
    parser.add_argument("--fast", required=True, type=flag, help="whether this is the fast Team PR gate")
    parser.add_argument("--full", required=True, type=flag, help="whether this is the full gate")
    parser.add_argument("--models", required=True, type=flag, help="whether the models run")
    parser.add_argument("--prefix", default="", help="caller job prefix to filter and strip (workflow_call)")
    args = parser.parse_args()
    with open(args.jobs, encoding="utf-8") as f:
        jobs = [json.loads(line) for line in f if line.strip()]
    for j in jobs:
        print(f"{j['name']}: {j['status']}, {j['conclusion']}")
    errors = check(
        os.environ.get("EXPECTED", ""),
        jobs,
        args.full,
        args.models,
        prefix=args.prefix,
        fast=args.fast,
    )
    for error in errors:
        print(f"::error::{error}")
    if errors:
        return 1
    print(f"Every job the tier expects passed (fast: {args.fast}, full: {args.full}, models: {args.models}).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
