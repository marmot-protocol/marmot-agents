"""The flaky job tracker (D-143): one issue that counts the CI jobs that failed and then passed
when they were run again.

A job that fails in one attempt of a run and passes in a later attempt of the same run failed on
the same commit and then passed: it is flaky. This script lists the recent runs of `ci.yml`,
`canary.yml` and `nightly.yml` that were run again (pull requests, hourly canary and the nightly
runs alike), and the jobs of each attempt, and counts each job that failed or timed out in one
attempt and passed in a later one, by its workflow and its name (a matrix leg's name holds its leg,
and a reusable-workflow call keeps its `<caller> / ` prefix). The gate, `CI passed` (or `CI / CI
passed` inside canary), fails whenever another job does: it counts only when every other job's
latest result up to its attempt passed (a job not run again keeps its earlier result).

It reads no log. A log mixes what the runner printed with what the tests printed, so a test could
put its own text into the public issue; which test flaked is for machine-readable results to say
(#184). Only a job's workflow and name, its counts, the days it was first and last seen, the kind
of run it flaked in and a link to its last such job reach the issue, as nightly.yml's issues hold
only names, links and dates.

A job's name is shown only if the default branch's workflow file names a job so (`--workflows`,
the default branch's `.github/workflows`, with each matrix leg's name filled in). A pull
request's CI runs the pull request's own workflow, which can name a job anything, even from a
test's output; such a job is counted as NOT_ON_DEFAULT_BRANCH, with its link, and its own name is
never kept.

What it keeps is in the issue's body, below the table, in an HTML comment that ends the body: each
job's count, its flakes by day over the days it is ranked by, the latest attempt it failed in, and
the runs it has read. So the issue is its own store, and nothing is kept elsewhere. Each time, it
reads the body it wrote last, adds what is new, and writes the body again; a job is counted once,
however often the script runs. If that body cannot be read back, or what it keeps no longer fits
an issue, the script fails and writes nothing, so the issue keeps what it had.

Usage:

    flake_report.py --repo OWNER/NAME --previous BODY --output BODY [--workflows DIR] [--days 14]

BODY is the issue's body as Markdown; `--previous` names a file that is missing when there is no
issue yet, and only then is the count started afresh: an issue's body that is empty fails. DIR
holds the default branch's ci.yml, canary.yml and nightly.yml. GitHub is called through `gh api`,
with the token in GH_TOKEN.
"""

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys

TITLE = "Flaky job tracker"
WORKFLOWS = ("ci.yml", "canary.yml", "nightly.yml")
LOCAL_WORKFLOW = "./.github/workflows/"
# These fail whenever another job does, so only a failure of their own counts.
GATES = {"CI passed", "CI / CI passed"}
# A job that hit its `timeout-minutes` ends `timed_out`; one that a step failed ends `failure`.
FAILED = {"failure", "timed_out"}
# A gate's failure is its own only when no other job's latest result up to it is one of these.
NOT_PASSED = FAILED | {"cancelled"}
STATE_START = "<!-- flake-tracker-state"
STATE_END = "-->"
STATE_VERSION = 1
# An issue's body holds at most 65,536 characters.
MAX_BODY = 65000
# What a job is shown as when the default branch's workflows give no job its name: a pull request's
# CI runs the pull request's own workflow, which can name a job anything (D-143).
NOT_ON_DEFAULT_BRANCH = "(a job not on the default branch)"
TOP = 100
RECENT_DAYS = 30
PER_PAGE = 100
MAX_PAGES = 10
EVENTS = {
    "pull_request": "pull request",
    "merge_group": "merge queue",
    "push": "master",
    "schedule": "scheduled",
    "workflow_dispatch": "by hand",
}
MATRIX = re.compile(r"\$\{\{\s*matrix\.([A-Za-z0-9_-]+)\s*\}\}")


class ListingTooLong(RuntimeError):
    """A listing longer than the script reads, which it does not count from part of."""


class InventoryError(RuntimeError):
    """The default branch's workflow files gave no job names to trust."""


class StateError(RuntimeError):
    """The issue's body cannot be read back, or what it keeps no longer fits an issue."""


class GitHub:
    """The few GitHub API calls the tracker makes, through `gh api`."""

    def __init__(self, repo, run=subprocess.run):
        self.repo, self._run = repo, run

    def call(self, path):
        # gh writes why a call failed to the job's log; a failed call raises CalledProcessError.
        result = self._run(["gh", "api", "-X", "GET", path], stdout=subprocess.PIPE, timeout=300)
        result.check_returncode()
        return result.stdout

    def pages(self, path, key):
        """Every item of a listing, page by page until a short one.

        Raises ListingTooLong if the listing is longer than MAX_PAGES pages, rather than count
        from part of it.
        """
        items = []
        for page in range(1, MAX_PAGES + 1):
            batch = json.loads(self.call(f"{path}&per_page={PER_PAGE}&page={page}")).get(key, [])
            items += batch
            if len(batch) < PER_PAGE:
                return items
        raise ListingTooLong(f"{path}: more than {MAX_PAGES * PER_PAGE} items; none counted")

    def runs(self, workflow, since):
        """The workflow's runs created since `since` (a date), newest first."""
        return self.pages(f"repos/{self.repo}/actions/workflows/{workflow}/runs"
                          f"?created=%3E%3D{since}", "workflow_runs")

    def jobs(self, run):
        """Every job of every attempt of the run, each with its `run_attempt`."""
        return self.pages(f"repos/{self.repo}/actions/runs/{run}/jobs?filter=all", "jobs")


def _lines(text):
    """The file's lines that hold something, as (indent, text), with no comment on a line alone."""
    out = []
    for line in text.splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith("#"):
            out.append((len(line) - len(line.lstrip(" ")), stripped))
    return out


def _scalar(text):
    """A plain, quoted or `[a, b]` value; None for one this reader does not read."""
    if text[:1] in ("'", '"'):
        end = text.find(text[0], 1)
        return text[1:end] if end > 0 else None
    text = re.split(r"\s#", text, maxsplit=1)[0].strip()
    if text.startswith("["):
        if not text.endswith("]"):
            return None
        items = [_scalar(item.strip()) for item in text[1:-1].split(",") if item.strip()]
        return None if None in items or any(isinstance(i, list) for i in items) else items
    if text[:1] in ("{", "|", ">", "&", "*", "!"):
        return None
    return text


KEY = re.compile(r"([A-Za-z0-9_.-]+):(?:\s+(.*))?$")


def _block(lines, at, indent):
    """The mapping or list that starts at line `at`, at `indent`, and the line after it.

    It reads the YAML the workflows here are written in: mappings, lists, plain, quoted and `[a, b]`
    values. A block scalar (`run: |`) and a value it does not read are None, and lines more
    indented than a value it has read are skipped, so nothing in a script is taken for a key.
    """
    if at < len(lines) and lines[at][0] == indent and lines[at][1].startswith("- "):
        items = []
        while at < len(lines) and lines[at][0] == indent and lines[at][1].startswith("- "):
            rest = lines[at][1][2:].strip()
            if KEY.match(rest):
                lines[at] = (indent + 2, rest)
                item, at = _block(lines, at, indent + 2)
            else:
                item, at = _scalar(rest), at + 1
                while at < len(lines) and lines[at][0] > indent:
                    at += 1
            items.append(item)
        return items, at
    mapping = {}
    while at < len(lines) and lines[at][0] == indent and not lines[at][1].startswith("- "):
        key = KEY.match(lines[at][1])
        if not key:
            return mapping, at
        rest, at = key.group(2), at + 1
        nested = at < len(lines) and (lines[at][0] > indent or (
            lines[at][0] == indent and lines[at][1].startswith("- ")))
        if not rest and nested:
            mapping[key.group(1)], at = _block(lines, at, lines[at][0])
        else:
            mapping[key.group(1)] = _scalar(rest) if rest else None
        while at < len(lines) and lines[at][0] > indent:
            at += 1
    return mapping, at


def _legs(matrix):
    """Each leg of a matrix, as the values it sets; more than GitHub runs is harmless, since every
    value comes from the file."""
    if not isinstance(matrix, dict):
        return []
    axes = {k: v for k, v in matrix.items() if k not in ("include", "exclude")
            and isinstance(v, list) and all(isinstance(i, str) for i in v)}
    combos = [{}]
    for key, values in axes.items():
        combos = [dict(c, **{key: v}) for c in combos for v in values]
    include = [i for i in matrix.get("include") or [] if isinstance(i, dict)]
    return combos + include + [dict(c, **i) for c in combos for i in include]


def _expand_job_name(name, job):
    """Expand a job's name across its matrix legs, or return it directly when unparameterised."""
    if "${{" not in name:
        return [name]
    strategy = job.get("strategy")
    out = []
    for leg in _legs(strategy.get("matrix") if isinstance(strategy, dict) else None):
        filled = MATRIX.sub(lambda m: leg.get(m.group(1), "${{"), name)
        if "${{" not in filled and all(isinstance(leg.get(k), str) for k in
                                       MATRIX.findall(name)):
            out.append(filled)
    return out


def job_names(text):
    """The direct job names a workflow runs under, as GitHub shows them, read from its file.

    Each non-reusable job's `name`, or its key when it has none, with `${{ matrix.KEY }}` filled
    from each leg of its `strategy.matrix`. A name with any other expression, or a matrix this
    reader cannot read, gives no name: the tracker then shows that job only as
    NOT_ON_DEFAULT_BRANCH.
    """
    workflow, _ = _block(_lines(text), 0, 0)
    names = set()
    jobs = workflow.get("jobs")
    for key, job in (jobs.items() if isinstance(jobs, dict) else []):
        if not isinstance(job, dict) or isinstance(job.get("uses"), str):
            continue
        name = job.get("name", key)
        if isinstance(name, str):
            names.update(_expand_job_name(name, job))
    return names


def _local_workflow_calls(text):
    """Return `(caller_name, callee_filename)` pairs for jobs calling `./.github/workflows/<file>`."""
    workflow, _ = _block(_lines(text), 0, 0)
    calls = []
    jobs = workflow.get("jobs")
    for key, job in (jobs.items() if isinstance(jobs, dict) else []):
        if not isinstance(job, dict):
            continue
        uses = job.get("uses")
        if not (isinstance(uses, str) and uses.startswith(LOCAL_WORKFLOW)):
            continue
        callee = uses[len(LOCAL_WORKFLOW):]
        name = job.get("name", key)
        if isinstance(name, str) and os.path.basename(callee) == callee and callee not in ("", ".", ".."):
            for caller_name in _expand_job_name(name, job):
                calls.append((caller_name, callee))
    return calls


def trusted_names(folder, workflows=WORKFLOWS):
    """The job names of each workflow, read from its file in `folder`, the default branch's.

    Expands one level of local reusable-workflow calls (`uses: ./.github/workflows/<file>`) with
    GitHub's `<caller> / <callee>` job-name prefix. Raises InventoryError when a workflow gives
    none, rather than show every job as unknown.
    """
    trusted = {}
    for workflow in workflows:
        with open(os.path.join(folder, workflow), encoding="utf-8") as f:
            text = f.read()
        names = job_names(text)
        for caller_name, callee in _local_workflow_calls(text):
            with open(os.path.join(folder, callee), encoding="utf-8") as f:
                for callee_name in job_names(f.read()):
                    names.add(f"{caller_name} / {callee_name}")
        trusted[workflow] = names
        if not trusted[workflow]:
            raise InventoryError(f"{folder}/{workflow} names no job")
    return trusted


def empty_state():
    return {"version": STATE_VERSION, "runs": {}, "jobs": []}


def valid(state):
    """Whether the state is in the shape this version writes."""
    def day(text):
        return isinstance(text, str) and re.fullmatch(r"\d{4}-\d\d-\d\d", text) is not None

    def count(n):
        return isinstance(n, int) and not isinstance(n, bool) and n >= 0
    if not isinstance(state, dict) or set(state) != {"version", "runs", "jobs"}:
        return False
    if not isinstance(state["runs"], dict) or not isinstance(state["jobs"], list):
        return False
    for kept in state["runs"].values():
        if not (isinstance(kept, dict) and set(kept) == {"attempt", "created", "counted"}
                and count(kept["attempt"]) and day(kept["created"])
                and isinstance(kept["counted"], list) and all(map(count, kept["counted"]))):
            return False
    for entry in state["jobs"]:
        if not (isinstance(entry, dict)
                and set(entry) == {"workflow", "job", "count", "first", "last", "days", "events",
                                   "latest"}
                and isinstance(entry["workflow"], str) and isinstance(entry["job"], str)
                and count(entry["count"]) and day(entry["first"]) and day(entry["last"])
                and isinstance(entry["days"], dict) and all(map(day, entry["days"]))
                and all(map(count, entry["days"].values()))
                and isinstance(entry["events"], dict)
                and all(map(count, entry["events"].values()))
                and isinstance(entry["latest"], list) and len(entry["latest"]) == 3
                and count(entry["latest"][0]) and count(entry["latest"][1])
                and isinstance(entry["latest"][2], str)):
            return False
    return True


STATE = re.compile(re.escape(STATE_START) + r"\n([^\n]*)\n" + re.escape(STATE_END) + r"\s*\Z")


def read_state(body):
    """What the tracker kept in the issue's body: the comment that ends it.

    None, when there is no issue, is an empty state. A body that does not end in a state this
    version can read, an empty one too, raises StateError, so nothing is written over it.
    """
    if body is None:
        return empty_state()
    body = body.replace("\r\n", "\n")
    match = STATE.search(body)
    if not match:
        raise StateError("the tracker's body does not end with its state")
    try:
        state = json.loads(match.group(1))
    except ValueError as error:
        raise StateError(f"the tracker's state is not JSON: {error}") from None
    if not isinstance(state, dict) or state.get("version") != STATE_VERSION:
        raise StateError(f"the tracker's state is not version {STATE_VERSION}")
    if not valid(state):
        raise StateError("the tracker's state is not in the shape it is written in")
    return state


def write_state(state):
    # On one line, with no "<", ">" or "--", so nothing in a name can end the comment or start
    # another; they are only ever inside a JSON string, where an escape reads the same.
    text = (json.dumps(state, separators=(",", ":"), sort_keys=True)
            .replace("<", "\\u003c").replace(">", "\\u003e").replace("--", "-\\u002d"))
    return f"{STATE_START}\n{text}\n{STATE_END}\n"


def day(timestamp):
    return timestamp[:10]


def record(state, workflow, run, job, trusted):
    """Counts one failure of one job in one attempt of a run.

    The job is kept by its name only when the default branch's workflow names a job so; any other
    name is never kept, and the job is counted as NOT_ON_DEFAULT_BRANCH.
    """
    started = job.get("started_at") or run["created_at"]
    when, event = day(started), run.get("event", "")
    latest = [run["id"], job["id"], started]
    name = job["name"] if job["name"] in trusted.get(workflow, ()) else NOT_ON_DEFAULT_BRANCH
    entry = next((e for e in state["jobs"] if (e["workflow"], e["job"]) == (workflow, name)), None)
    if entry is None:
        entry = {"workflow": workflow, "job": name, "count": 0, "first": when, "last": when,
                 "days": {}, "events": {}, "latest": latest}
        state["jobs"].append(entry)
    entry["count"] += 1
    entry["first"] = min(entry["first"], when)
    entry["last"] = max(entry["last"], when)
    entry["days"][when] = entry["days"].get(when, 0) + 1
    entry["events"][event] = entry["events"].get(event, 0) + 1
    # The latest job by when it started, then by run and job, never by how its link sorts.
    if (latest[2], latest[0], latest[1]) >= (entry["latest"][2], entry["latest"][0],
                                             entry["latest"][1]):
        entry["latest"] = latest


def flaked(jobs):
    """The jobs that failed in one attempt and passed in a later one, oldest attempt first.

    The gate's failure in an attempt counts only when every other job's latest result up to that
    attempt passed: a job that was not run again in it keeps its earlier result, which the gate
    read.
    """
    passed, results = {}, {}
    for job in jobs:
        attempt = job.get("run_attempt", 1)
        if job.get("conclusion") == "success":
            passed[job["name"]] = max(passed.get(job["name"], 0), attempt)
        if job["name"] not in GATES:
            results.setdefault(job["name"], []).append((attempt, job.get("conclusion")))

    def others_failed(attempt):
        latest = [max((r for r in rs if r[0] <= attempt), key=lambda r: r[0])
                  for rs in results.values()
                  if any(r[0] <= attempt for r in rs)]
        return any(conclusion in NOT_PASSED for _, conclusion in latest)
    out = []
    for job in jobs:
        attempt = job.get("run_attempt", 1)
        if job.get("conclusion") not in FAILED or passed.get(job["name"], 0) <= attempt:
            continue
        if job["name"] in GATES and others_failed(attempt):
            continue
        out.append(job)
    return out


def collect(github, state, since, trusted, workflows=WORKFLOWS):
    """Adds what the runs since `since` (a date) show to `state`, naming only the jobs whose names
    are in `trusted`, each workflow's from the default branch.

    A listing that cannot be read raises, and the run it is in is not marked as read, so the next
    pass reads it again.
    """
    for workflow in workflows:
        for run in github.runs(workflow, since):
            key = str(run["id"])
            kept = state["runs"].get(key, {"attempt": 1, "counted": []})
            # A run is read again only once it has been run again since it was last read.
            if run.get("run_attempt", 1) <= 1 or run.get("status") != "completed":
                continue
            if kept["attempt"] >= run["run_attempt"]:
                continue
            counted = set(kept["counted"])
            for job in flaked(github.jobs(run["id"])):
                if job["id"] not in counted:
                    record(state, workflow, run, job, trusted)
                    counted.add(job["id"])
            state["runs"][key] = {"attempt": run["run_attempt"], "created": day(run["created_at"]),
                                  "counted": sorted(counted)}
    # A run older than the window is not listed again, so what was kept of it can go.
    state["runs"] = {k: v for k, v in state["runs"].items() if v["created"] >= since}


def recent_since(today):
    return (dt.date.fromisoformat(today) - dt.timedelta(days=RECENT_DAYS)).isoformat()


def prune(state, today):
    """Drops the flakes by day older than the days a job is ranked by; its count keeps them."""
    recent = recent_since(today)
    for entry in state["jobs"]:
        entry["days"] = {d: n for d, n in entry["days"].items() if d > recent}


def code(text):
    """The text as inline code in a table cell, shown as written.

    The fence is one backtick longer than the longest run inside, padded when the text starts or
    ends with one, and `|` is escaped, which GitHub undoes inside a table's code.
    """
    longest = max((len(run) for run in re.findall(r"`+", text)), default=0)
    fence = "`" * (longest + 1)
    pad = " " if text.startswith("`") or text.endswith("`") or longest else ""
    return f"{fence}{pad}{text.replace('|', chr(92) + '|')}{pad}{fence}"


def rows(state, today):
    """The jobs, with their flakes in the last days: most of those first, then most in all,
    then the latest seen."""
    recent = recent_since(today)
    out = [(e, sum(n for d, n in e["days"].items() if d > recent)) for e in state["jobs"]]
    out.sort(key=lambda r: (r[0]["workflow"], r[0]["job"]))
    out.sort(key=lambda r: r[0]["last"], reverse=True)
    out.sort(key=lambda r: (-r[1], -r[0]["count"]))
    return out


def seen_on(events):
    named = [(EVENTS.get(k, k), n) for k, n in events.items()]
    return ", ".join(f"{name} {n}" for name, n in sorted(named))


def render(state, today, top, repo):
    table = rows(state, today)
    lines = [
        f"# {TITLE}",
        "",
        "The CI jobs that failed and passed when they were run again, on the same commit: on pull "
        "requests, in hourly canary, and in the nightly runs. Written by "
        "`.github/workflows/flake-report.yml` from `tools/ci/flake_report.py` (D-143); edits by "
        "hand are lost the next time it runs, or stop it. It reads no log: follow the link to the "
        "job to see what failed. Naming the tests is #184.",
        "",
        f"Updated {today}. Ranked by flakes in the last {RECENT_DAYS} days, then in all.",
    ]
    if not table:
        lines += ["", "No job has flaked yet."]
    else:
        lines += [
            "",
            f"| # | Workflow | Job | Last {RECENT_DAYS} days | All | Seen on | First seen "
            "| Last seen | Last job |",
            "| ---: | --- | --- | ---: | ---: | --- | --- | --- | --- |",
        ]
        for rank, (entry, last_month) in enumerate(table[:top], 1):
            run, job, _ = entry["latest"]
            link = f"[job](https://github.com/{repo}/actions/runs/{run}/job/{job})"
            lines.append(
                f"| {rank} | {code(entry['workflow'])} | {code(entry['job'])} | {last_month} | "
                f"{entry['count']} | {seen_on(entry['events'])} | {entry['first']} | "
                f"{entry['last']} | {link} |")
        if len(table) > top:
            lines += ["", f"{len(table) - top} more jobs are counted and left out of the table."]
    return "\n".join(lines) + "\n\n" + write_state(state)


def fit(state, today, repo, top=TOP):
    """The body, with fewer rows in the table until it fits an issue; what is kept is never cut.

    Raises StateError when it does not fit even with no table.
    """
    while True:
        body = render(state, today, top, repo)
        if len(body) <= MAX_BODY:
            return body
        if top == 0:
            raise StateError(f"what the tracker keeps is {len(body)} characters, more than an "
                             f"issue holds ({MAX_BODY})")
        top = max(0, top - 10)


def main(argv=None, github=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", required=True, help="OWNER/NAME")
    parser.add_argument("--previous", required=True, help="the issue's body as it is, if any")
    parser.add_argument("--output", required=True, help="where to write the new body")
    parser.add_argument("--days", type=int, default=14, help="how far back to list runs")
    parser.add_argument("--workflows", default=".github/workflows",
                        help="the default branch's workflow files, whose job names may be shown")
    parser.add_argument("--today", help="the date to report as, YYYY-MM-DD (for tests)")
    args = parser.parse_args(argv)
    try:
        with open(args.previous, encoding="utf-8") as f:
            previous = f.read()
    except FileNotFoundError:
        # No issue is open; one that is has a body file, even an empty one.
        previous = None
    today = args.today or dt.datetime.now(dt.timezone.utc).date().isoformat()
    since = (dt.date.fromisoformat(today) - dt.timedelta(days=args.days)).isoformat()
    try:
        trusted = trusted_names(args.workflows)
        state = read_state(previous)
        collect(github or GitHub(args.repo), state, since, trusted)
        prune(state, today)
        body = fit(state, today, args.repo)
    except (InventoryError, StateError) as error:
        print(f"The tracker is left as it is: {error}.", file=sys.stderr)
        return 1
    with open(args.output, "w", encoding="utf-8") as f:
        f.write(body)
    counted = sum(e["count"] for e in state["jobs"])
    print(f"{len(state['jobs'])} jobs, {counted} flakes in all.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
