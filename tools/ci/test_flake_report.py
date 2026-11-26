"""Tests for flake_report.py, the flaky job tracker (D-143)."""

import json
import os
import subprocess
import tempfile
import unittest

import flake_report
import passed
from flake_report import (GitHub, ListingTooLong, StateError, code, fit, job_names, read_state,
                          render)

REPO = "owner/name"
WORKFLOWS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".github",
                         "workflows")
# The names the jobs of this checkout's ci.yml and nightly.yml run under, as on the default branch.
TRUSTED = flake_report.trusted_names(WORKFLOWS)


def collect(github, state, since, workflows=flake_report.WORKFLOWS):
    flake_report.collect(github, state, since, TRUSTED, workflows)


def job(id, name, attempt, conclusion, started="2026-11-20T10:00:00Z"):
    return {"id": id, "name": name, "run_attempt": attempt, "conclusion": conclusion,
            "started_at": started}


def run(id, attempt, status="completed", event="pull_request", created="2026-11-20T09:00:00Z"):
    return {"id": id, "run_attempt": attempt, "status": status, "event": event,
            "created_at": created}


class FakeGitHub:
    """Runs and jobs as GitHub would give them, counting the runs whose jobs were listed."""

    def __init__(self, runs, jobs):
        self._runs, self._jobs = runs, jobs
        self.jobs_listed = []

    def runs(self, workflow, since):
        return [r for r in self._runs.get(workflow, []) if r["created_at"][:10] >= since]

    def jobs(self, run):
        self.jobs_listed.append(run)
        jobs = self._jobs[run]
        if isinstance(jobs, Exception):
            raise jobs
        return jobs


def empty():
    return flake_report.empty_state()


def jobs_of(state):
    return {(e["workflow"], e["job"]): e for e in state["jobs"]}


def counted(state):
    return sorted((e["workflow"], e["job"], e["count"]) for e in state["jobs"])


class Calls(unittest.TestCase):
    """GitHub's listings through `gh api`, with a stand-in for the process it starts."""

    def github(self, pages=None, returncode=0):
        calls = []

        def run(argv, **kwargs):
            calls.append(argv[-1])
            out = json.dumps({"jobs": pages[len(calls) - 1]}).encode() if pages else b""
            return subprocess.CompletedProcess(argv, returncode, out)
        github = GitHub(REPO, run=run)
        github.calls = calls
        return github

    def test_a_listing_is_read_page_by_page_until_a_short_page(self):
        full = [job(n, "Python", 1, "success") for n in range(flake_report.PER_PAGE)]
        github = self.github([full, full[:3]])
        self.assertEqual(len(github.jobs(7)), flake_report.PER_PAGE + 3)
        self.assertEqual(github.calls, [
            f"repos/{REPO}/actions/runs/7/jobs?filter=all&per_page=100&page=1",
            f"repos/{REPO}/actions/runs/7/jobs?filter=all&per_page=100&page=2"])

    def test_a_listing_longer_than_the_pages_it_reads_fails_rather_than_count_part(self):
        full = [job(n, "Python", 1, "success") for n in range(flake_report.PER_PAGE)]
        github = self.github([full] * flake_report.MAX_PAGES)
        with self.assertRaises(ListingTooLong):
            github.jobs(7)
        self.assertEqual(len(github.calls), flake_report.MAX_PAGES)

    def test_a_failed_call_raises(self):
        # gh writes its own message to the job's log.
        with self.assertRaises(subprocess.CalledProcessError):
            self.github(returncode=1).jobs(7)


class Collect(unittest.TestCase):
    def test_a_job_that_failed_and_then_passed_is_counted_once(self):
        github = FakeGitHub(
            {"ci.yml": [run(1, 2)]},
            {1: [job(10, "Rust tests (core)", 1, "failure"),
                 job(11, "Python", 1, "success"),
                 job(12, "Rust tests (core)", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(github.jobs_listed, [1], "nothing is read twice")
        self.assertEqual(counted(state), [("ci.yml", "Rust tests (core)", 1)])
        entry = jobs_of(state)[("ci.yml", "Rust tests (core)")]
        self.assertEqual(entry["events"], {"pull_request": 1})
        self.assertEqual((entry["first"], entry["last"]), ("2026-11-20", "2026-11-20"))
        self.assertEqual(entry["days"], {"2026-11-20": 1})
        self.assertEqual(entry["latest"], [1, 10, "2026-11-20T10:00:00Z"])

    def test_each_leg_of_a_matrix_and_each_workflow_counts_apart(self):
        github = FakeGitHub(
            {"ci.yml": [run(1, 2)], "nightly.yml": [run(2, 2, event="schedule")]},
            {1: [job(10, "TypeScript (claude-code)", 1, "failure"),
                 job(11, "TypeScript (openclaw)", 1, "failure"),
                 job(12, "TypeScript (claude-code)", 2, "success"),
                 job(13, "TypeScript (openclaw)", 2, "success")],
             2: [job(20, "Coverage report", 1, "failure"),
                 job(21, "Coverage report", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10")
        self.assertEqual(counted(state), [("ci.yml", "TypeScript (claude-code)", 1),
                                          ("ci.yml", "TypeScript (openclaw)", 1),
                                          ("nightly.yml", "Coverage report", 1)])
        self.assertEqual(jobs_of(state)[("nightly.yml", "Coverage report")]["events"],
                         {"schedule": 1})

    def test_a_job_that_failed_again_is_counted_only_once_it_passes(self):
        jobs = [job(10, "Python", 1, "failure"), job(12, "Python", 2, "failure")]
        github = FakeGitHub({"ci.yml": [run(1, 2)]}, {1: jobs})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(state["jobs"], [], "it may be a real failure")
        github._runs["ci.yml"] = [run(1, 3)]
        jobs.append(job(13, "Python", 3, "success"))
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(counted(state), [("ci.yml", "Python", 2)])
        self.assertEqual(state["runs"]["1"]["counted"], [10, 12])

    def test_a_job_that_timed_out_and_then_passed_is_counted(self):
        github = FakeGitHub(
            {"ci.yml": [run(1, 2)]},
            {1: [job(10, "Simulator", 1, "timed_out"), job(11, "Simulator", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(counted(state), [("ci.yml", "Simulator", 1)])

    def test_the_gate_counts_only_when_no_other_job_of_its_attempt_failed(self):
        github = FakeGitHub(
            {"ci.yml": [run(1, 2), run(2, 2), run(3, 2)]},
            {1: [job(10, "CI passed", 1, "failure"), job(11, "Python", 1, "failure"),
                 job(12, "Python", 2, "success"), job(13, "CI passed", 2, "success")],
             2: [job(20, "CI passed", 1, "failure"), job(21, "Python", 1, "cancelled"),
                 job(22, "Python", 2, "success"), job(23, "CI passed", 2, "success")],
             3: [job(30, "CI passed", 1, "failure"), job(31, "Python", 1, "success"),
                 job(33, "CI passed", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(counted(state), [("ci.yml", "CI passed", 1), ("ci.yml", "Python", 1)])
        self.assertEqual(jobs_of(state)[("ci.yml", "CI passed")]["latest"][:2], [3, 30])

    def test_the_gate_reads_each_other_jobs_latest_result_up_to_its_attempt(self):
        # Two jobs fail; only Python is run again and passes, and the gate fails again on Rust's
        # earlier result; then Rust is run again and the gate passes. Neither failure of the gate
        # was its own.
        github = FakeGitHub(
            {"ci.yml": [run(1, 3)]},
            {1: [job(10, "Python", 1, "failure"), job(11, "Rust tests (core)", 1, "failure"),
                 job(12, "CI passed", 1, "failure"),
                 job(20, "Python", 2, "success"), job(22, "CI passed", 2, "failure"),
                 job(31, "Rust tests (core)", 3, "success"), job(32, "CI passed", 3, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(counted(state), [("ci.yml", "Python", 1),
                                          ("ci.yml", "Rust tests (core)", 1)])

    def test_the_nightly_report_job_is_not_a_gate_and_counts(self):
        name = "Open an issue for each job that failed"
        github = FakeGitHub(
            {"nightly.yml": [run(1, 2, event="schedule")]},
            {1: [job(10, name, 1, "failure"), job(11, "Simulator on new seeds", 1, "failure"),
                 job(12, "Simulator on new seeds", 2, "success"), job(13, name, 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("nightly.yml",))
        self.assertEqual(counted(state), [("nightly.yml", name, 1),
                                          ("nightly.yml", "Simulator on new seeds", 1)])

    def test_runs_not_run_again_cancelled_or_still_running_are_not_read(self):
        github = FakeGitHub(
            {"ci.yml": [run(1, 1), run(2, 2, status="in_progress"), run(3, 2)]},
            {2: [job(20, "Python", 1, "failure"), job(21, "Python", 2, "success")],
             3: [job(30, "Python", 1, "cancelled"), job(31, "Python", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(github.jobs_listed, [3])
        self.assertEqual(state["jobs"], [])
        self.assertNotIn("2", state["runs"], "read once it has finished")

    def test_a_listing_that_cannot_be_read_fails_and_leaves_the_run_for_next_time(self):
        github = FakeGitHub({"ci.yml": [run(1, 2)]}, {1: ListingTooLong("too long")})
        state = empty()
        with self.assertRaises(ListingTooLong):
            collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(state["runs"], {})

    def test_the_last_job_is_the_latest_one_whatever_its_link_sorts_as(self):
        # GitHub lists the newest run first; run 1000 is newer than run 999.
        github = FakeGitHub(
            {"ci.yml": [run(1000, 2), run(999, 2)]},
            {1000: [job(10, "Python", 1, "failure", "2026-11-20T12:00:00Z"),
                    job(11, "Python", 2, "success")],
             999: [job(9, "Python", 1, "failure", "2026-11-20T08:00:00Z"),
                   job(12, "Python", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        self.assertEqual(state["jobs"][0]["latest"][:2], [1000, 10])
        body = render(state, "2026-11-24", 100, REPO)
        self.assertIn(f"https://github.com/{REPO}/actions/runs/1000/job/10", body)
        self.assertNotIn("runs/999", body)

    def test_runs_older_than_the_window_are_forgotten_and_their_counts_kept(self):
        github = FakeGitHub(
            {"ci.yml": [run(1, 2)]},
            {1: [job(10, "Python", 1, "failure"), job(11, "Python", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        collect(github, state, "2026-11-25", workflows=("ci.yml",))
        self.assertEqual(state["runs"], {})
        self.assertEqual(counted(state), [("ci.yml", "Python", 1)])


def entry(name, count, last, workflow="ci.yml"):
    return {"workflow": workflow, "job": name, "count": count, "first": "2026-01-01",
            "last": last, "days": {last: count}, "events": {"push": count},
            "latest": [1, 2, f"{last}T00:00:00Z"]}


def state_with(*jobs):
    state = empty()
    state["jobs"] = [entry(*j) for j in jobs]
    return state


class Inventory(unittest.TestCase):
    """The names read from the default branch's workflow files."""

    def test_ci_yml_gives_the_names_its_gate_expects(self):
        with open(os.path.join(WORKFLOWS, "ci.yml"), encoding="utf-8") as f:
            text = f.read()
        # The gate's list, EXPECTED, is a block of names under its key, a name a line.
        lines = text.splitlines()
        (at,) = [n for n, line in enumerate(lines) if line.strip() == "EXPECTED: |"]
        indent = len(lines[at]) - len(lines[at].lstrip())
        block = []
        for line in lines[at + 1:]:
            if line.strip() and len(line) - len(line.lstrip()) <= indent:
                break
            block.append(line)
        expected = {name for name, _ in passed.parse_expected("\n".join(block))}
        self.assertEqual(TRUSTED["ci.yml"], expected | {"CI passed"})

    def test_nightly_yml_gives_its_jobs_names(self):
        self.assertEqual(TRUSTED["nightly.yml"], {
            "Resolve canary commit", "Simulator on new seeds", "TLA+ models on larger bounds",
            "Coverage report", "Mutation testing, one shard", "The model double's cassettes",
            "Property tests at high counts", "Fuzzing",
            "Open an issue for each job that failed"})

    def test_canary_yml_gives_its_own_and_called_ci_job_names(self):
        expected_called = {f"CI / {name}" for name in TRUSTED["ci.yml"]}
        self.assertEqual(TRUSTED["canary.yml"], {
            "Check whether master moved", "Fast-forward canary", "Report canary failure",
        } | expected_called)

    def test_canary_gate_counts_only_when_no_other_job_of_its_attempt_failed(self):
        github = FakeGitHub(
            {"canary.yml": [run(1, 2, event="schedule"), run(2, 2, event="schedule")]},
            {1: [job(10, "CI / CI passed", 1, "failure"),
                 job(11, "CI / Rust tests (core)", 1, "failure"),
                 job(12, "CI / Rust tests (core)", 2, "success"),
                 job(13, "CI / CI passed", 2, "success")],
             2: [job(20, "CI / CI passed", 1, "failure"),
                 job(21, "CI / Rust tests (core)", 1, "success"),
                 job(22, "CI / CI passed", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("canary.yml",))
        self.assertEqual(counted(state), [
            ("canary.yml", "CI / CI passed", 1),
            ("canary.yml", "CI / Rust tests (core)", 1),
        ])

    def test_the_forms_a_workflow_names_its_jobs_in(self):
        text = """name: X
on:
  push:
jobs:
  plain:
    name: A plain name # and a comment
    runs-on: ubuntu-24.04
    steps:
      - name: not a job
        run: |
          name: not a job either
  quoted:
    name: "A quoted: name"
  unnamed:
    runs-on: ubuntu-24.04
  listed:
    name: Listed (${{ matrix.part }})
    strategy:
      matrix:
        part: [one, "two"]
  block:
    name: Block (${{matrix.part}}, ${{ matrix.os }})
    strategy:
      fail-fast: false
      matrix:
        part:
          - three
        os: [a, b]
  included:
    name: Included (${{ matrix.part }})
    strategy:
      matrix:
        include:
          - part: four
            cases: --a
          - cases: --b
  computed:
    name: Computed (${{ needs.a.outputs.b }})
  from_json:
    name: From JSON (${{ matrix.part }})
    strategy:
      matrix: ${{ fromJSON(needs.a.outputs.legs) }}
"""
        self.assertEqual(job_names(text), {
            "A plain name", "A quoted: name", "unnamed", "Listed (one)", "Listed (two)",
            "Block (three, a)", "Block (three, b)", "Included (four)"})

    def test_a_workflow_with_no_job_names_fails(self):
        with tempfile.TemporaryDirectory() as folder:
            for workflow in flake_report.WORKFLOWS:
                with open(os.path.join(folder, workflow), "w", encoding="utf-8") as f:
                    f.write("name: X\non:\n  push:\n")
            with self.assertRaises(flake_report.InventoryError):
                flake_report.trusted_names(folder)


class State(unittest.TestCase):
    def test_the_state_reads_back_from_the_body_it_is_written_in(self):
        state = state_with(("a", 2, "2026-11-20"))
        state["runs"]["1"] = {"attempt": 2, "created": "2026-11-20", "counted": [10]}
        self.assertEqual(read_state(render(state, "2026-11-24", 100, REPO)), state)

    def test_a_name_holding_the_marker_or_a_comment_end_cannot_reach_the_state(self):
        name = f"{flake_report.STATE_START} --> <b>x</b> --"
        state = state_with((name, 1, "2026-11-20"))
        body = render(state, "2026-11-24", 100, REPO)
        # In the table it is code, shown as written; only the comment that ends the body is read.
        self.assertIn(code(name), body)
        comment = body[body.rindex(flake_report.STATE_START):]
        self.assertEqual((comment.count("<"), comment.count(">"), comment.count("--")), (1, 1, 2))
        self.assertEqual(read_state(body), state)

    def test_an_issue_written_back_by_github_reads_the_same(self):
        state = state_with(("a", 2, "2026-11-20"))
        body = render(state, "2026-11-24", 100, REPO).replace("\n", "\r\n") + "\n"
        self.assertEqual(read_state(body), state)

    def test_a_body_whose_state_is_missing_malformed_misplaced_or_another_version_fails(self):
        good = render(state_with(("a", 1, "2026-11-20")), "2026-11-24", 100, REPO)
        start = flake_report.STATE_START
        for body in ("", " \n", "written by hand",
                     good.replace(start, "<!-- flake-tracker-stat"),
                     good.replace('"version":1', '"version":0'),
                     good.replace('"jobs":[', '"jobs":[}'),
                     good + "\nand a line written after it\n",
                     f"{start}\n{{}}\n-->\n",
                     good.replace('"count":1', '"count":"1"'),
                     good.replace('"count":1', '"count":1,"extra":0')):
            with self.subTest(body=body[-80:]), self.assertRaises(StateError):
                read_state(body)

    def test_the_state_is_one_compact_line(self):
        body = render(state_with(("a", 1, "2026-11-20")), "2026-11-24", 100, REPO)
        line = body.splitlines()[-2]
        self.assertNotIn(" ", line)
        self.assertEqual(json.loads(line), read_state(body))


class Body(unittest.TestCase):
    def test_recent_flakes_rank_first_then_all_then_the_latest(self):
        state = state_with(("old_and_many", 9, "2026-01-01"), ("recent", 1, "2026-11-20"),
                           ("recent_twice", 2, "2026-11-21"), ("also_recent", 1, "2026-11-22"))
        body = render(state, "2026-11-24", 100, REPO)
        order = ["recent_twice", "also_recent", "recent", "old_and_many"]
        self.assertEqual(sorted(order, key=lambda name: body.index(f"`{name}`")), order)

    def test_a_row_holds_only_names_counts_days_kinds_and_a_link(self):
        state = state_with(("TypeScript (claude-code)", 3, "2026-11-20"))
        state["jobs"][0]["events"] = {"merge_group": 2, "pull_request": 1}
        body = render(state, "2026-11-24", 100, REPO)
        self.assertIn("| 1 | `ci.yml` | `TypeScript (claude-code)` | 3 | 3 | merge queue 2, "
                      "pull request 1 | 2026-01-01 | 2026-11-20 | "
                      f"[job](https://github.com/{REPO}/actions/runs/1/job/2) |", body)

    def test_a_name_shows_as_written_and_keeps_the_table_whole(self):
        self.assertEqual(code("plain"), "`plain`")
        self.assertEqual(code("a `b` c"), "`` a `b` c ``")
        self.assertEqual(code("``x``"), "``` ``x`` ```")
        self.assertEqual(code("a | b <tag> @someone #1"), "`a \\| b <tag> @someone #1`")
        name = "a job named with `.` | and a pipe"
        state = state_with((name, 1, "2026-11-20"))
        body = render(state, "2026-11-24", 100, REPO)
        (row,) = [line for line in body.splitlines() if line.startswith("| 1 |")]
        self.assertIn(f"`` {name.replace('|', chr(92) + '|')} ``", row)
        self.assertEqual(row.replace("\\|", "").count("|"), 10, "nine cells")
        self.assertEqual(jobs_of(read_state(body))[("ci.yml", name)]["job"], name)

    def test_a_name_not_on_the_default_branch_is_counted_under_one_label_and_never_kept(self):
        # A pull request's CI runs the pull request's own ci.yml, so it can name a job anything.
        names = ["Python \x1b[1m from a test's output", "Rust tests (${{ needs.x.outputs.y }})"]
        github = FakeGitHub(
            {"ci.yml": [run(1, 2), run(2, 2)]},
            {1: [job(10, names[0], 1, "failure"), job(11, names[0], 2, "success")],
             2: [job(20, names[1], 1, "timed_out", "2026-11-21T10:00:00Z"),
                 job(21, names[1], 2, "success"),
                 job(22, "Python", 1, "failure"), job(23, "Python", 2, "success")]})
        state = empty()
        collect(github, state, "2026-11-10", workflows=("ci.yml",))
        label = flake_report.NOT_ON_DEFAULT_BRANCH
        self.assertEqual(counted(state), [("ci.yml", label, 2), ("ci.yml", "Python", 1)])
        self.assertEqual(jobs_of(state)[("ci.yml", label)]["latest"][:2], [2, 20])
        body = render(state, "2026-11-24", 100, REPO)
        self.assertIn(f"| {code(label)} | 2 | 2 |", body)
        self.assertIn(f"https://github.com/{REPO}/actions/runs/2/job/20", body)
        for name in names:
            self.assertNotIn(name, body)
            self.assertNotIn(name, json.dumps(state))
        for part in ("x1b", "\x1b", "outputs", "test's output"):
            self.assertNotIn(part, body)

    def test_the_table_is_trimmed_to_fit_and_no_count_is_lost(self):
        jobs = [(f"job_{n:04}_" + "x" * 60, 1 + n % 3, f"2026-{1 + n % 12:02}-01") for n in range(200)]
        state = state_with(*jobs)
        body = fit(state, "2026-11-24", REPO)
        self.assertLessEqual(len(body), flake_report.MAX_BODY)
        kept = read_state(body)
        self.assertEqual(kept, state)
        self.assertEqual(sum(e["count"] for e in kept["jobs"]), sum(j[1] for j in jobs))
        self.assertIn("more jobs are counted and left out of the table", body)

    def test_a_state_too_large_for_an_issue_fails_rather_than_lose_counts(self):
        state = state_with(*[(f"job_{n:05}_" + "x" * 280, 1, "2026-11-20") for n in range(400)])
        with self.assertRaises(StateError):
            fit(state, "2026-11-24", REPO)

    def test_days_older_than_the_ranking_are_folded_into_the_counts(self):
        state = state_with(("a", 1, "2026-09-01"))
        state["jobs"][0]["days"]["2026-11-20"] = 2
        state["jobs"][0]["count"] = 3
        flake_report.prune(state, "2026-11-24")
        self.assertEqual(state["jobs"][0]["days"], {"2026-11-20": 2})
        self.assertEqual(state["jobs"][0]["count"], 3)


class Main(unittest.TestCase):
    def main(self, github, previous=None):
        with tempfile.TemporaryDirectory() as folder:
            before = os.path.join(folder, "previous.md")
            if previous is not None:
                with open(before, "w", encoding="utf-8") as f:
                    f.write(previous)
            out = os.path.join(folder, "body.md")
            argv = ["--repo", REPO, "--previous", before, "--output", out, "--today", "2026-11-24",
                    "--workflows", WORKFLOWS]
            status = flake_report.main(argv, github)
            if not os.path.exists(out):
                return status, None
            with open(out, encoding="utf-8") as f:
                return status, f.read()

    def test_a_first_run_writes_a_body_a_second_reads_back(self):
        github = FakeGitHub(
            {"ci.yml": [run(1, 2, event="merge_group")]},
            {1: [job(10, "Rust tests (core)", 1, "failure"),
                 job(11, "Rust tests (core)", 2, "success")]})
        status, body = self.main(github)
        self.assertEqual(status, 0)
        self.assertIn("| `Rust tests (core)` | 1 | 1 | merge queue 1 |", body)
        self.assertIn("#184", body)
        status, again = self.main(github, previous=body)
        self.assertEqual((status, again), (0, body), "counted once")

    def test_a_previous_body_it_cannot_read_writes_nothing(self):
        github = FakeGitHub({"ci.yml": []}, {})
        # An issue was found, so its body is a file, even when the body was cleared: only no
        # file, no issue, starts a new count.
        for previous in ("# Flaky job tracker\n\nedited by hand\n", "", " \n\t\n", "null\n"):
            with self.subTest(previous=previous):
                status, body = self.main(github, previous=previous)
                self.assertNotEqual(status, 0)
                self.assertIsNone(body)


if __name__ == "__main__":
    unittest.main()
