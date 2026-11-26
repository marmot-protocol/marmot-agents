"""Tests for tools/ci/bisect_agent.py (Phase 2 of #222).

Exercises all four failure buckets (`infra`, `known_flake`, `suspected_race`,
`deterministic_regression`), real `git bisect` and `git revert` over temporary
git repositories, paginated skipped-canary scanning via the production GitHubClient,
failed `tree-state` update retry safety, OS mount-namespace isolation of credentials
and control repo, dry-run state persistence, per-SHA failure counters, multi-job
prioritization, already-fixed master protection, standalone `CI passed` bisection,
and `tree-state` red->green auto-recovery.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bisect_agent as ba  # noqa: E402
import flake_report as fr  # noqa: E402

SANDBOX_BACKEND = ba.detect_sandbox_backend()


def _git(cwd: Path, *args: str) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=str(cwd),
        text=True,
        capture_output=True,
        check=True,
    )
    return proc.stdout.strip()


class FakeGitHub:
    """In-memory stand-in for GitHubClient."""

    def __init__(
        self,
        runs: list[ba.RunInfo] | None = None,
        run_info: ba.RunInfo | None = None,
        failed_run_fallback: ba.RunInfo | None = None,
        failed_jobs: list[ba.FailedJob] | None = None,
        flaky_jobs: set[str] | None = None,
        existing_revert_pr: dict | str | None = None,
        auto_merge_ok: bool = True,
        open_prs: list[dict] | None = None,
        disable_auto_merge_results: dict[str, bool | Exception] | None = None,
        list_open_prs_error: Exception | None = None,
    ):
        self.repo = "marmot-protocol/marmot-agents"
        self._runs = list(runs) if runs is not None else ([run_info] if run_info is not None else [])
        self._failed_run_fallback = failed_run_fallback
        self._failed_jobs = failed_jobs or []
        self._flaky_jobs = flaky_jobs or set()
        if isinstance(existing_revert_pr, str):
            self._existing_revert_pr = {
                "url": existing_revert_pr,
                "state": "OPEN",
                "auto_merge": True,
                "mergeable": "MERGEABLE",
                "checks": "pending",
            }
        else:
            self._existing_revert_pr = existing_revert_pr
        self._inspected_pr: dict | None = None
        self._auto_merge_ok = auto_merge_ok
        self._open_prs = list(open_prs) if open_prs is not None else []
        self._disable_auto_merge_results = dict(disable_auto_merge_results or {})
        self._list_open_prs_error = list_open_prs_error
        self.dispatched: list[bool] = []
        self.comments: list[str] = []
        self.opened_prs: list[dict] = []
        self.auto_merge_calls: list[str] = []
        self.list_open_prs_calls: list[str] = []
        self.disable_auto_merge_calls: list[str] = []

    def iter_completed_canary_runs(self, per_page: int = 30, max_pages: int = 10):
        yield from self._runs

    def latest_failed_canary_run(self):
        return self._failed_run_fallback

    def failed_jobs_for_run(self, run_id: int):
        inner = [j for j in self._failed_jobs if j.name not in ba.PURE_WRAPPER_JOBS and j.name not in ba.GATE_JOBS]
        gates = [j for j in self._failed_jobs if j.name in ba.GATE_JOBS]
        return inner if inner else gates

    def flaky_job_names(self):
        return set(self._flaky_jobs)

    def open_revert_pr_for_sha(self, culprit_sha: str):
        return self._existing_revert_pr

    def inspect_pr(self, pr_url: str):
        return self._inspected_pr

    def enable_auto_merge(self, pr_url: str) -> bool:
        self.auto_merge_calls.append(pr_url)
        return self._auto_merge_ok

    def list_open_prs(self, base: str = "master") -> list[dict]:
        self.list_open_prs_calls.append(base)
        if self._list_open_prs_error is not None:
            raise self._list_open_prs_error
        return list(self._open_prs)

    def disable_auto_merge(self, pr_url: str) -> bool:
        self.disable_auto_merge_calls.append(pr_url)
        outcome = self._disable_auto_merge_results.get(pr_url, True)
        if isinstance(outcome, Exception):
            raise outcome
        return bool(outcome)

    def dispatch_canary(self, force: bool = True):
        self.dispatched.append(force)

    def comment_canary_issue(self, body: str):
        self.comments.append(body)

    def open_revert_pr(self, branch: str, title: str, body: str) -> tuple[str, bool]:
        url = f"https://github.com/{self.repo}/pull/999"
        self.opened_prs.append({"branch": branch, "title": title, "body": body, "url": url})
        return url, self.enable_auto_merge(url)


class TempRepoMixin:
    """Creates a real bare origin + working clone with master, canary, and tree-state branches."""

    def make_repo(self, tree_state: str = "green"):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        origin = root / "origin.git"
        work = root / "seed"
        repo = root / "repo"
        worktrees = root / "worktrees"
        state_dir = root / "state"

        origin.mkdir()
        _git(origin, "init", "--bare", "-b", "master")
        _git(root, "clone", str(origin), str(work))
        _git(work, "config", "user.name", "Test Bot")
        _git(work, "config", "user.email", "bot@example.invalid")
        _git(work, "config", "commit.gpgsign", "false")

        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
        (work / "app.txt").write_text("v1\n", encoding="utf-8")
        _git(work, "add", "check.sh", "app.txt")
        _git(work, "commit", "-m", "Initial green commit")
        base_sha = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")
        _git(work, "push", "origin", f"{base_sha}:refs/heads/canary")

        _git(work, "checkout", "--orphan", "tree-state")
        _git(work, "rm", "-rf", ".")
        (work / "state.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "state": tree_state,
                    "updated_at": "2026-11-26T14:00:00Z",
                    "reason": "initial",
                    "run_url": None,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        _git(work, "add", "state.json")
        _git(work, "commit", "-m", f"Init tree-state {tree_state}")
        _git(work, "push", "origin", "tree-state")
        _git(work, "checkout", "master")

        _git(root, "clone", str(origin), str(repo))
        _git(repo, "config", "user.name", "Test Bot")
        _git(repo, "config", "user.email", "bot@example.invalid")
        _git(repo, "config", "commit.gpgsign", "false")
        return root, origin, work, repo, worktrees, state_dir, base_sha


class UnitParsingTests(unittest.TestCase):
    def test_parse_flake_tracker_jobs_matches_real_flake_report_state(self):
        state = fr.empty_state()
        trusted = {"canary.yml": {"CI / Rust tests (core)"}, "ci.yml": {"Install scripts"}}
        fr.record(
            state,
            "canary.yml",
            {"id": 10, "created_at": "2026-11-26T00:00:00Z", "event": "schedule"},
            {"id": 100, "name": "CI / Rust tests (core)", "started_at": "2026-11-26T00:01:00Z"},
            trusted,
        )
        fr.record(
            state,
            "ci.yml",
            {"id": 11, "created_at": "2026-11-25T00:00:00Z", "event": "pull_request"},
            {"id": 101, "name": "Install scripts", "started_at": "2026-11-25T00:01:00Z"},
            trusted,
        )
        body = f"# Flaky job tracker\n\n{fr.write_state(state)}"
        flaky = ba.parse_flake_tracker_jobs(body)
        self.assertIn("CI / Rust tests (core)", flaky)
        self.assertIn("Rust tests (core)", flaky)
        self.assertIn("Install scripts", flaky)
        self.assertNotIn("Python", flaky)

    def test_infra_classification_prefers_real_failure_markers(self):
        self.assertIsNotNone(ba.is_infra_failure("failure", "Error: The runner has received a shutdown signal."))
        self.assertIsNotNone(ba.is_infra_failure("failure", "curl: (22) HTTP 503 Service Unavailable"))
        self.assertIsNotNone(ba.is_infra_failure("timed_out", "Compiling mdk v0.1.0\nStill building..."))
        self.assertIsNone(ba.is_infra_failure("failure", "Failed to restore cache\ntest result: FAILED. 1 failed"))
        self.assertIsNone(ba.is_infra_failure("timed_out", "test result: FAILED. 1 failed"))
        self.assertIsNone(ba.is_infra_failure("failure", "AssertionError: expected 1 == 2"))

    def test_job_specs_cover_every_non_wrapper_job_in_ci_yml(self):
        ci_yml = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
        ci_jobs = fr.job_names(ci_yml.read_text(encoding="utf-8"))
        missing = sorted(j for j in ci_jobs if j not in ba.PURE_WRAPPER_JOBS and j not in ba.JOB_SPECS)
        self.assertEqual(missing, [], f"ci.yml jobs missing from bisect_agent.JOB_SPECS: {missing}")

    def test_simulator_spec_compiles_before_timed_run(self):
        cmd = ba.JOB_SPECS["Simulator"].command
        self.assertIn("--no-run &&", cmd)
        self.assertLess(cmd.index("--no-run"), cmd.index("timeout "))

    def test_recordings_spec_includes_offline_cassette_check(self):
        cmd = ba.JOB_SPECS["Recordings"].command
        self.assertIn("python3 tools/model_double/check_cassettes.py fixtures/cassettes --offline", cmd)

    def test_normalize_pr_info_handles_rollup_conclusions(self):
        p_fail = ba._normalize_pr_info({
            "url": "https://example/pr/1",
            "state": "OPEN",
            "autoMergeRequest": {"enabledAt": "2026-11-26T00:00:00Z"},
            "mergeable": "MERGEABLE",
            "statusCheckRollup": [{"conclusion": "STARTUP_FAILURE"}],
        })
        self.assertEqual(p_fail["checks"], "failed")
        self.assertTrue(p_fail["auto_merge"])

        p_pend = ba._normalize_pr_info({
            "url": "https://example/pr/2",
            "state": "OPEN",
            "autoMergeRequest": None,
            "mergeable": "UNKNOWN",
            "statusCheckRollup": [{"status": "IN_PROGRESS", "conclusion": ""}],
        })
        self.assertEqual(p_pend["checks"], "pending")
        self.assertFalse(p_pend["auto_merge"])


class BisectAgentBucketTests(TempRepoMixin, unittest.TestCase):
    def _agent(
        self,
        repo,
        worktrees,
        state_dir,
        gh,
        dry_run=True,
        job_specs=None,
        alerts=None,
        require_sandbox=None,
        extra_masked_dirs=(),
    ):
        def record_alert(msg: str, is_dry: bool) -> dict:
            if alerts is not None:
                alerts.append((msg, is_dry))
            return {"sent": not is_dry, "dry_run": is_dry, "channel": "test", "message": msg}

        if require_sandbox is None:
            require_sandbox = SANDBOX_BACKEND is not None
        return ba.BisectAgent(
            repo_dir=repo,
            worktrees_dir=worktrees,
            state_dir=state_dir,
            gh=gh,
            dry_run=dry_run,
            repro_attempts=2,
            step_timeout_sec=30,
            sign_commits=False,
            require_sandbox=require_sandbox,
            extra_masked_dirs=extra_masked_dirs,
            job_specs=job_specs or {"Python": ba.JobSpec(command="bash check.sh", required_bins=("bash",))},
            alert_fn=record_alert,
        )

    def test_production_github_client_paginates_past_skipped_success_to_failed_run(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Deterministic break")
        broken_sha = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        # Simulate production GitHubClient paginated API: page 1 has 30 skipped successes on broken_sha,
        # page 2 has the two earlier failures that triggered the retry-cap skips.
        page1_runs = [
            {"id": 900 + i, "head_sha": broken_sha, "conclusion": "success", "html_url": f"https://example/runs/{900 + i}"}
            for i in range(30)
        ]
        page2_runs = [
            {"id": 850, "head_sha": broken_sha, "conclusion": "failure", "html_url": "https://example/runs/850"},
            {"id": 849, "head_sha": broken_sha, "conclusion": "failure", "html_url": "https://example/runs/849"},
        ]

        def fake_gh_run(cmd, **kwargs):
            arg_str = " ".join(cmd)
            if "&page=1" in arg_str:
                return subprocess.CompletedProcess(cmd, 0, json.dumps({"workflow_runs": page1_runs}), "")
            if "&page=2" in arg_str:
                return subprocess.CompletedProcess(cmd, 0, json.dumps({"workflow_runs": page2_runs}), "")
            if "/runs/850/jobs" in arg_str:
                return subprocess.CompletedProcess(
                    cmd,
                    0,
                    json.dumps({"jobs": [{"id": 5001, "name": "CI / Python", "conclusion": "failure", "html_url": "https://example/jobs/5001"}]}),
                    "",
                )
            if "/jobs/5001/logs" in arg_str:
                return subprocess.CompletedProcess(cmd, 0, "AssertionError: deterministic\n", "")
            if "issue list" in arg_str or "pr list" in arg_str:
                return subprocess.CompletedProcess(cmd, 0, "[]", "")
            raise AssertionError(f"unexpected gh call: {cmd}")

        gh = ba.GitHubClient("marmot-protocol/marmot-agents", run_fn=fake_gh_run)
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=True).run_once()
        self.assertEqual(res.run_id, 850)
        self.assertEqual(res.bucket, "deterministic_regression")
        self.assertEqual(res.action, "dry_run_revert_verified")
        self.assertEqual(res.culprit_sha, broken_sha)

    def test_failed_tree_state_update_does_not_mark_run_handled_and_retries_next_tick(self):
        _, origin, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "note.txt").write_text("x\n", encoding="utf-8")
        _git(work, "add", "note.txt")
        _git(work, "commit", "-m", "Innocent commit")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh = FakeGitHub(
            run_info=ba.RunInfo(701, tip, "failure", "https://example/runs/701"),
            failed_jobs=[ba.FailedJob(71, "CI / Python", "failure", "https://example/jobs/71", "FAILED")],
        )
        # First tick: simulate tree-state update failing (e.g. transient network/fetch failure).
        alerts: list[tuple[str, bool]] = []
        agent1 = self._agent(
            repo,
            worktrees,
            state_dir,
            gh,
            dry_run=False,
            job_specs={"Python": ba.JobSpec(command="exit 1", required_bins=("bash",))},
            alerts=alerts,
        )
        agent1.update_tree_state = lambda *a, **kw: "failed"  # type: ignore[method-assign]
        r1 = agent1.run_once()
        self.assertEqual(r1.action, "meltdown_failed")
        self.assertEqual(alerts, [])
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "green")

        # Second tick on the SAME run_id retries Meltdown and succeeds once remote is reachable.
        agent2 = self._agent(
            repo,
            worktrees,
            state_dir,
            gh,
            dry_run=False,
            job_specs={"Python": ba.JobSpec(command="exit 1", required_bins=("bash",))},
            alerts=alerts,
        )
        r2 = agent2.run_once()
        self.assertEqual(r2.action, "meltdown")
        self.assertEqual(len(alerts), 1)
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "red")

    def test_status_failure_fallback_finds_unresolved_failure_past_300_skipped_runs(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Break check long ago")
        broken_sha = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        # 310 skipped success runs (exceeding the 10-page/300-run window) with the real failure
        # returned only by `latest_failed_canary_run()` (`status=failure`).
        skipped_runs = [
            ba.RunInfo(2000 + i, broken_sha, "success", f"https://example/runs/{2000 + i}")
            for i in range(310)
        ]
        gh = FakeGitHub(
            runs=skipped_runs,
            failed_run_fallback=ba.RunInfo(1500, broken_sha, "failure", "https://example/runs/1500"),
            failed_jobs=[ba.FailedJob(1501, "CI / Python", "failure", "https://example/jobs/1501", "AssertionError")],
        )
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=True).run_once()
        self.assertEqual(res.run_id, 1500)
        self.assertEqual(res.bucket, "deterministic_regression")
        self.assertEqual(res.action, "dry_run_revert_verified")
        self.assertEqual(res.culprit_sha, broken_sha)

    def test_sandbox_argv_and_bwrap_runtime_block_credentials_proc_umount_and_host_writes(self):
        root, _, _, repo, worktrees, state_dir, base_sha = self.make_repo()
        secret_dir = root / "sentinel-creds"
        secret_dir.mkdir()
        sentinel = secret_dir / "token.txt"
        sentinel.write_text("TOP_SECRET_BOT_TOKEN\n", encoding="utf-8")
        host_ro_target = root / "host-ro-canary.txt"
        host_ro_target.write_text("untouched\n", encoding="utf-8")

        # 1. Fail-closed check when bwrap is unavailable, plus overlap rejection (runs on all hosts).
        agent_no_bwrap = self._agent(repo, worktrees, state_dir, FakeGitHub(), dry_run=True, require_sandbox=True)
        agent_no_bwrap._sandbox_backend = None
        with self.assertRaises(RuntimeError):
            agent_no_bwrap._build_sandbox_argv(worktrees / "sandbox-export", "true")
        with self.assertRaises(ValueError):
            self._agent(repo, repo / ".bisect-worktrees", state_dir, FakeGitHub(), dry_run=True, require_sandbox=False)
        self.assertFalse((repo / ".bisect-worktrees").exists())
        with self.assertRaises(ValueError):
            self._agent(repo, state_dir / "worktrees", state_dir, FakeGitHub(), dry_run=True, require_sandbox=False)
        # SCCACHE_DIR overlapping (or containing!) a masked directory is also rejected.
        prev_sccache = os.environ.get("SCCACHE_DIR")
        os.environ["SCCACHE_DIR"] = str(root)
        try:
            with self.assertRaises(ValueError):
                self._agent(repo, worktrees, state_dir, FakeGitHub(), dry_run=True, require_sandbox=False)
        finally:
            if prev_sccache is None:
                os.environ.pop("SCCACHE_DIR", None)
            else:
                os.environ["SCCACHE_DIR"] = prev_sccache

        # 2. Verify exact bwrap flags unconditionally (read-only root, PID/IPC isolation, masks).
        agent_argv = self._agent(
            repo,
            worktrees,
            state_dir,
            FakeGitHub(),
            dry_run=True,
            require_sandbox=True,
            extra_masked_dirs=(secret_dir,),
        )
        agent_argv._sandbox_backend = "bwrap"
        argv = agent_argv._build_sandbox_argv(worktrees / "sandbox-export", "true")
        self.assertEqual(
            argv[:11],
            ["bwrap", "--ro-bind", "/", "/", "--dev", "/dev", "--tmpfs", "/tmp", "--unshare-pid", "--proc", "/proc"],
        )
        self.assertIn("--unshare-ipc", argv)
        self.assertIn("--die-with-parent", argv)
        self.assertIn(str(secret_dir), argv)
        self.assertIn(str(repo), argv)
        # Dedicated writable credential-free CARGO_HOME and RUSTUP_HOME are bound explicitly.
        cargo_dir, rustup_dir = agent_argv._sandbox_cache_dirs()
        self.assertIn(str(cargo_dir), argv)
        self.assertIn(str(rustup_dir), argv)
        # Docker job is intentionally not locally runnable without an isolated daemon.
        can_run_docker, _ = agent_argv._can_run_locally(ba.JOB_SPECS["Move-over tests (Docker)"])
        self.assertFalse(can_run_docker)

        # 3. Runtime bwrap proof: blocks direct read, /proc/<parent>/root, /proc/<parent>/environ,
        #    umount of masks, host Docker socket access, and writes outside writable binds, while
        #    allowing writes to the dedicated credential-free CARGO_HOME/RUSTUP_HOME caches and
        #    resetting .check-home between runs.
        if SANDBOX_BACKEND != "bwrap":
            self.skipTest("bwrap is not available on this host")

        prev_sentinel_env = os.environ.get("BISECT_TEST_SENTINEL_SECRET")
        os.environ["BISECT_TEST_SENTINEL_SECRET"] = "TOP_SECRET_ENV_VALUE"
        try:
            agent = self._agent(
                repo,
                worktrees,
                state_dir,
                FakeGitHub(),
                dry_run=True,
                require_sandbox=True,
                extra_masked_dirs=(secret_dir,),
            )
            # Drop a fake credentials.toml in cargo_dir to prove it is masked to /dev/null.
            cargo_dir, _ = agent._sandbox_cache_dirs()
            (cargo_dir / "credentials.toml").write_text("token = 'CRATES_SECRET'\n", encoding="utf-8")
            parent_pid = os.getpid()
            probe_cmd = " && ".join([
                f"test ! -e {sentinel}",
                f"test ! -e /proc/{parent_pid}/root{sentinel}",
                f"test ! -e /proc/{parent_pid}/environ",
                f"(! umount {secret_dir} 2>/dev/null)",
                f"test ! -e {sentinel}",
                f"test ! -e {host_ro_target}",
                "(! (touch /usr/.ro-probe 2>/dev/null))",
                "(! test -S /run/docker.sock)",
                "(! test -S /var/run/docker.sock)",
                'test ! -s "$CARGO_HOME/credentials.toml"',
                'mkdir -p "$CARGO_HOME/registry" && echo ok > "$CARGO_HOME/registry/.write-test"',
                'mkdir -p "$RUSTUP_HOME/tmp" && echo ok > "$RUSTUP_HOME/tmp/.write-test"',
                'if command -v cargo >/dev/null 2>&1; then cargo --version >/dev/null; fi',
                'echo dirty > "$HOME/.persisted-probe"',
                f"test ! -e {repo / '.git/config'}",
                "test ! -e .git",
                "test -f check.sh",
            ])
            ok, tail = agent._run_check_in_export(repo, base_sha, probe_cmd)
            self.assertTrue(ok, f"bwrap sandbox failed runtime isolation probe:\n{tail}")
            self.assertEqual(host_ro_target.read_text(encoding="utf-8"), "untouched\n")
            # Confirm .check-home is wiped between runs so check state cannot leak across commits.
            ok2, tail2 = agent._run_check_in_export(repo, base_sha, 'test ! -e "$HOME/.persisted-probe"')
            self.assertTrue(ok2, f".check-home persisted across checks:\n{tail2}")
        finally:
            if prev_sentinel_env is None:
                os.environ.pop("BISECT_TEST_SENTINEL_SECRET", None)
            else:
                os.environ["BISECT_TEST_SENTINEL_SECRET"] = prev_sentinel_env

    def test_dry_run_persists_separate_state_and_escalates_on_second_failure(self):
        _, origin, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "note.txt").write_text("hi\n", encoding="utf-8")
        _git(work, "add", "note.txt")
        _git(work, "commit", "-m", "Harmless")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh1 = FakeGitHub(
            run_info=ba.RunInfo(601, tip, "failure", "https://example/runs/601"),
            failed_jobs=[ba.FailedJob(61, "CI / Python", "failure", "https://example/jobs/61", "HTTP 503")],
        )
        r1 = self._agent(repo, worktrees, state_dir, gh1, dry_run=True).run_once()
        self.assertEqual(r1.action, "retry_canary")
        self.assertEqual(gh1.dispatched, [])

        # Repeating dry-run on the SAME run_id is a no-op (no endless re-runs on timer ticks).
        r1_again = self._agent(repo, worktrees, state_dir, gh1, dry_run=True).run_once()
        self.assertEqual(r1_again.action, "noop_already_handled")

        # Second failure on the same SHA in dry-run shows Meltdown escalation without mutating origin!
        gh2 = FakeGitHub(
            run_info=ba.RunInfo(602, tip, "failure", "https://example/runs/602"),
            failed_jobs=[ba.FailedJob(62, "CI / Python", "failure", "https://example/jobs/62", "HTTP 503")],
        )
        r2 = self._agent(repo, worktrees, state_dir, gh2, dry_run=True).run_once()
        self.assertEqual(r2.action, "meltdown")
        self.assertEqual(r2.tree_state, "red")
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "green")

        # Live state remains untouched and independent of dry-run state.
        r_live = self._agent(repo, worktrees, state_dir, gh1, dry_run=False).run_once()
        self.assertEqual(r_live.action, "retry_canary")
        self.assertEqual(gh1.dispatched, [True])

    def test_per_sha_consecutive_failures_resets_on_new_sha(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "a.txt").write_text("a\n", encoding="utf-8")
        _git(work, "add", "a.txt")
        _git(work, "commit", "-m", "Commit A")
        sha_a = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh_a = FakeGitHub(
            run_info=ba.RunInfo(610, sha_a, "failure", "https://example/runs/610"),
            failed_jobs=[ba.FailedJob(1, "CI / Python", "failure", "https://example/jobs/1", "HTTP 503")],
        )
        self.assertEqual(self._agent(repo, worktrees, state_dir, gh_a, dry_run=False).run_once().action, "retry_canary")

        (work / "b.txt").write_text("b\n", encoding="utf-8")
        _git(work, "add", "b.txt")
        _git(work, "commit", "-m", "Commit B")
        sha_b = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        # First infra blip on a NEW commit B retries once instead of jumping straight to Meltdown.
        gh_b = FakeGitHub(
            run_info=ba.RunInfo(611, sha_b, "failure", "https://example/runs/611"),
            failed_jobs=[ba.FailedJob(2, "CI / Python", "failure", "https://example/jobs/2", "HTTP 503")],
        )
        self.assertEqual(self._agent(repo, worktrees, state_dir, gh_b, dry_run=False).run_once().action, "retry_canary")

    def test_empty_failed_jobs_retries_once_then_melts_down(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "a.txt").write_text("a\n", encoding="utf-8")
        _git(work, "add", "a.txt")
        _git(work, "commit", "-m", "Commit A")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh1 = FakeGitHub(run_info=ba.RunInfo(620, tip, "failure", "https://example/runs/620"), failed_jobs=[])
        self.assertEqual(self._agent(repo, worktrees, state_dir, gh1, dry_run=False).run_once().action, "retry_canary")

        gh2 = FakeGitHub(run_info=ba.RunInfo(621, tip, "failure", "https://example/runs/621"), failed_jobs=[])
        self.assertEqual(self._agent(repo, worktrees, state_dir, gh2, dry_run=False).run_once().action, "meltdown")

    def test_bucket2_known_flake_retries_once_then_melts_down_and_promotes_if_deterministic(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "note.txt").write_text("hi\n", encoding="utf-8")
        _git(work, "add", "note.txt")
        _git(work, "commit", "-m", "Harmless change")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh1 = FakeGitHub(
            run_info=ba.RunInfo(111, tip, "failure", "https://example/runs/111"),
            failed_jobs=[ba.FailedJob(11, "CI / Python", "failure", "https://example/jobs/11", "AssertionError: transient")],
            flaky_jobs={"Python"},
        )
        r1 = self._agent(repo, worktrees, state_dir, gh1, dry_run=False).run_once()
        self.assertEqual(r1.bucket, "known_flake")
        self.assertEqual(r1.action, "retry_canary")

        gh2 = FakeGitHub(
            run_info=ba.RunInfo(112, tip, "failure", "https://example/runs/112"),
            failed_jobs=[ba.FailedJob(12, "CI / Python", "failure", "https://example/jobs/12", "AssertionError: transient")],
            flaky_jobs={"Python"},
        )
        r2 = self._agent(repo, worktrees, state_dir, gh2, dry_run=False).run_once()
        self.assertEqual(r2.bucket, "known_flake")
        self.assertEqual(r2.action, "meltdown")

        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Break check deterministically")
        culprit = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")
        gh3 = FakeGitHub(
            run_info=ba.RunInfo(113, culprit, "failure", "https://example/runs/113"),
            failed_jobs=[ba.FailedJob(13, "CI / Python", "failure", "https://example/jobs/13", "AssertionError: deterministic")],
            flaky_jobs={"Python"},
        )
        r3 = self._agent(repo, worktrees, state_dir, gh3, dry_run=True).run_once()
        self.assertEqual(r3.bucket, "deterministic_regression")
        self.assertEqual(r3.action, "dry_run_revert_verified")
        self.assertEqual(r3.culprit_sha, culprit)

    def test_known_flake_does_not_mask_deterministic_regression_in_same_run(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Break Python check")
        culprit = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        specs = {
            "Install scripts": ba.JobSpec(command="exit 0", required_bins=("bash",)),
            "Python": ba.JobSpec(command="bash check.sh", required_bins=("bash",)),
        }
        gh = FakeGitHub(
            run_info=ba.RunInfo(120, culprit, "failure", "https://example/runs/120"),
            failed_jobs=[
                ba.FailedJob(21, "CI / Install scripts", "failure", "https://example/jobs/21", "AssertionError: flake"),
                ba.FailedJob(22, "CI / Python", "failure", "https://example/jobs/22", "AssertionError: real bug"),
            ],
            flaky_jobs={"Install scripts"},
        )
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=True, job_specs=specs).run_once()
        self.assertEqual(res.bucket, "deterministic_regression")
        self.assertEqual(res.failed_job, "CI / Python")
        self.assertEqual(res.culprit_sha, culprit)

    def test_standalone_ci_passed_failure_is_bisected(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Break tools/ci test")
        culprit = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh = FakeGitHub(
            run_info=ba.RunInfo(125, culprit, "failure", "https://example/runs/125"),
            failed_jobs=[ba.FailedJob(25, "CI / CI passed", "failure", "https://example/jobs/25", "FAILED (failures=1)")],
        )
        specs = {"CI passed": ba.JobSpec(command="bash check.sh", required_bins=("bash",))}
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=True, job_specs=specs).run_once()
        self.assertEqual(res.bucket, "deterministic_regression")
        self.assertEqual(res.failed_job, "CI / CI passed")
        self.assertEqual(res.culprit_sha, culprit)

    def test_bucket3_suspected_race_retries_then_melts_down_on_second_failure(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "note.txt").write_text("hi\n", encoding="utf-8")
        _git(work, "add", "note.txt")
        _git(work, "commit", "-m", "Unrelated change")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        alerts: list[tuple[str, bool]] = []
        gh1 = FakeGitHub(
            run_info=ba.RunInfo(104, tip, "failure", "https://example/runs/104"),
            failed_jobs=[ba.FailedJob(4, "CI / Python", "failure", "https://example/jobs/4", "AssertionError: race")],
        )
        res1 = self._agent(repo, worktrees, state_dir, gh1, dry_run=False, alerts=alerts).run_once()
        self.assertEqual(res1.bucket, "suspected_race")
        self.assertEqual(res1.action, "retry_canary")
        self.assertEqual(gh1.dispatched, [True])

        gh2 = FakeGitHub(
            run_info=ba.RunInfo(105, tip, "failure", "https://example/runs/105"),
            failed_jobs=[ba.FailedJob(5, "CI / Python", "failure", "https://example/jobs/5", "AssertionError: race again")],
        )
        res2 = self._agent(repo, worktrees, state_dir, gh2, dry_run=False, alerts=alerts).run_once()
        self.assertEqual(res2.bucket, "suspected_race")
        self.assertEqual(res2.action, "meltdown")
        self.assertEqual(res2.tree_state, "red")
        self.assertEqual(len(alerts), 1)
        self.assertIn("[MELTDOWN]", alerts[0][0])

    def test_bucket4_deterministic_bisect_and_clean_revert_dry_run_and_live(self):
        _, origin, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "good1.txt").write_text("1\n", encoding="utf-8")
        _git(work, "add", "good1.txt")
        _git(work, "commit", "-m", "Good commit 1")
        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Introduce deterministic bug")
        culprit = _git(work, "rev-parse", "HEAD")
        (work / "good2.txt").write_text("2\n", encoding="utf-8")
        _git(work, "add", "good2.txt")
        _git(work, "commit", "-m", "Good commit 2")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh = FakeGitHub(
            run_info=ba.RunInfo(201, tip, "failure", "https://example/runs/201"),
            failed_jobs=[ba.FailedJob(20, "CI / Python", "failure", "https://example/jobs/20", "FAILED")],
        )
        res_dry = self._agent(repo, worktrees, state_dir, gh, dry_run=True).run_once()
        self.assertEqual(res_dry.bucket, "deterministic_regression")
        self.assertEqual(res_dry.action, "dry_run_revert_verified")
        self.assertEqual(res_dry.culprit_sha, culprit)
        self.assertEqual(gh.opened_prs, [])
        self.assertNotIn(res_dry.revert_branch, _git(origin, "branch", "-a"))

        res_live = self._agent(repo, worktrees, state_dir, gh, dry_run=False).run_once()
        self.assertEqual(res_live.action, "revert_pr_opened")
        self.assertEqual(res_live.culprit_sha, culprit)
        self.assertEqual(len(gh.opened_prs), 1)
        self.assertIn(res_live.revert_branch, _git(origin, "branch", "-a"))
        msg = _git(origin, "log", "-1", "--format=%B", res_live.revert_branch)
        self.assertIn("Local-Checks: bash check.sh", msg)
        self.assertIn("Local-Review: exempt (automated bisect revert)", msg)
        self.assertIn("Release-Impact: patch", msg)

    def test_already_fixed_on_current_master_skips_revert(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "flag.txt").write_text("bad\n", encoding="utf-8")
        (work / "check.sh").write_text("#!/usr/bin/env bash\n[ \"$(cat flag.txt)\" = ok ]\n", encoding="utf-8")
        _git(work, "add", "flag.txt", "check.sh")
        _git(work, "commit", "-m", "Break check")
        broken_sha = _git(work, "rev-parse", "HEAD")
        (work / "flag.txt").write_text("ok\n", encoding="utf-8")
        _git(work, "add", "flag.txt")
        _git(work, "commit", "-m", "Fix flag on master")
        _git(work, "push", "origin", "master")

        gh = FakeGitHub(
            run_info=ba.RunInfo(210, broken_sha, "failure", "https://example/runs/210"),
            failed_jobs=[ba.FailedJob(210, "CI / Python", "failure", "https://example/jobs/210", "FAILED")],
        )
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=False).run_once()
        self.assertEqual(res.action, "noop_already_fixed_on_master")
        self.assertEqual(gh.opened_prs, [])

    def test_bucket4_revert_conflict_enters_meltdown_and_alerts(self):
        _, origin, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "check.sh").write_text("#!/usr/bin/env bash\necho broken\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Break check.sh")
        culprit = _git(work, "rev-parse", "HEAD")
        (work / "check.sh").write_text("#!/usr/bin/env bash\necho still broken differently\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Modify same line in check.sh")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        alerts: list[tuple[str, bool]] = []
        gh = FakeGitHub(
            run_info=ba.RunInfo(202, tip, "failure", "https://example/runs/202"),
            failed_jobs=[ba.FailedJob(21, "CI / Python", "failure", "https://example/jobs/21", "FAILED")],
        )
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=False, alerts=alerts).run_once()
        self.assertEqual(res.bucket, "deterministic_regression")
        self.assertEqual(res.action, "meltdown")
        self.assertEqual(res.culprit_sha, culprit)
        self.assertEqual(res.tree_state, "red")
        self.assertEqual(res.details.get("revert_status"), "revert_conflict")
        self.assertEqual(len(alerts), 1)
        self.assertIn("revert_conflict", alerts[0][0])
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "red")

    def test_bisect_plumbing_handles_skipped_commits_and_ambiguity(self):
        _, _, work, repo, worktrees, state_dir, base_sha = self.make_repo()
        shas: list[str] = [base_sha]
        for idx in range(1, 5):
            (work / f"step-{idx}.txt").write_text(f"{idx}\n", encoding="utf-8")
            if idx == 3:
                (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
            _git(work, "add", ".")
            _git(work, "commit", "-m", f"Step {idx}")
            shas.append(_git(work, "rev-parse", "HEAD"))
        _git(work, "push", "origin", "master")
        _git(repo, "fetch", "origin", "master", "canary")

        agent = self._agent(repo, worktrees, state_dir, FakeGitHub(), dry_run=True)

        # Skipping an unrelated commit (step 1) still isolates culprit (step 3).
        orig_run = agent._run_check_in_export
        agent._run_check_in_export = lambda sr, ref, cmd: (  # type: ignore[method-assign]
            (False, "command timed out after 30s") if ref == shas[1] else orig_run(sr, ref, cmd)
        )
        self.assertEqual(agent._bisect_culprit(shas[0], shas[4], "bash check.sh"), shas[3])

        # Skipping the culprit's direct parent (step 2) makes step 2 vs step 3 ambiguous -> None.
        agent._run_check_in_export = lambda sr, ref, cmd: (  # type: ignore[method-assign]
            (False, "command timed out after 30s") if ref == shas[2] else orig_run(sr, ref, cmd)
        )
        self.assertIsNone(agent._bisect_culprit(shas[0], shas[4], "bash check.sh"))

    def test_skipped_canary_success_does_not_clear_meltdown(self):
        _, origin, work, repo, worktrees, state_dir, _ = self.make_repo(tree_state="red")
        (work / "broken.txt").write_text("x\n", encoding="utf-8")
        _git(work, "add", "broken.txt")
        _git(work, "commit", "-m", "Unverified broken commit")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh = FakeGitHub(run_info=ba.RunInfo(300, tip, "success", "https://example/runs/300"))
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=False).run_once()
        self.assertEqual(res.action, "noop_skipped_canary_run")
        self.assertEqual(res.tree_state, "red")
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "red")

    def test_tree_state_auto_recovers_red_to_green_on_verified_canary_success(self):
        _, origin, _, repo, worktrees, state_dir, base_sha = self.make_repo(tree_state="red")
        gh = FakeGitHub(run_info=ba.RunInfo(301, base_sha, "success", "https://example/runs/301"))
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=False).run_once()
        self.assertEqual(res.action, "recovered_green")
        self.assertEqual(res.tree_state, "green")
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "green")

    def test_baseline_broken_enters_meltdown_without_blaming_innocent_commit(self):
        _, origin, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "note.txt").write_text("innocent\n", encoding="utf-8")
        _git(work, "add", "note.txt")
        _git(work, "commit", "-m", "Innocent commit")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh = FakeGitHub(
            run_info=ba.RunInfo(401, tip, "failure", "https://example/runs/401"),
            failed_jobs=[ba.FailedJob(41, "CI / Python", "failure", "https://example/jobs/41", "FAILED")],
        )
        res = self._agent(
            repo,
            worktrees,
            state_dir,
            gh,
            dry_run=False,
            job_specs={"Python": ba.JobSpec(command="exit 1", required_bins=("bash",))},
        ).run_once()
        self.assertEqual(res.bucket, "baseline_broken")
        self.assertEqual(res.action, "meltdown")
        self.assertEqual(gh.opened_prs, [])
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "red")

    def test_existing_open_revert_pr_is_not_duplicated(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Break check")
        culprit = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh = FakeGitHub(
            run_info=ba.RunInfo(402, culprit, "failure", "https://example/runs/402"),
            failed_jobs=[ba.FailedJob(42, "CI / Python", "failure", "https://example/jobs/42", "FAILED")],
            existing_revert_pr="https://github.com/marmot-protocol/marmot-agents/pull/888",
        )
        res = self._agent(repo, worktrees, state_dir, gh, dry_run=False).run_once()
        self.assertEqual(res.action, "noop_revert_already_open")
        self.assertEqual(res.revert_pr_url, "https://github.com/marmot-protocol/marmot-agents/pull/888")
        self.assertEqual(gh.opened_prs, [])

    def test_failed_auto_merge_and_conflicting_existing_revert_pr_escalate_to_meltdown(self):
        _, origin, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Break check")
        culprit = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        # 1. Failed auto-merge activation on a newly opened revert PR escalates to Meltdown.
        gh_fail_am = FakeGitHub(
            run_info=ba.RunInfo(410, culprit, "failure", "https://example/runs/410"),
            failed_jobs=[ba.FailedJob(410, "CI / Python", "failure", "https://example/jobs/410", "FAILED")],
            auto_merge_ok=False,
        )
        r1 = self._agent(repo, worktrees, state_dir, gh_fail_am, dry_run=False).run_once()
        self.assertEqual(r1.action, "meltdown")
        self.assertIn("revert_auto_merge_failed", r1.details.get("revert_status", ""))
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "red")

        # 2. Existing open revert PR without auto-merge re-enables auto-merge cleanly.
        gh_reenable = FakeGitHub(
            run_info=ba.RunInfo(411, culprit, "failure", "https://example/runs/411"),
            failed_jobs=[ba.FailedJob(411, "CI / Python", "failure", "https://example/jobs/411", "FAILED")],
            existing_revert_pr={
                "url": "https://github.com/marmot-protocol/marmot-agents/pull/888",
                "state": "OPEN",
                "auto_merge": False,
                "mergeable": "MERGEABLE",
                "checks": "pending",
            },
            auto_merge_ok=True,
        )
        r_re = self._agent(repo, worktrees, state_dir, gh_reenable, dry_run=False).run_once()
        self.assertEqual(r_re.action, "noop_revert_already_open")
        self.assertEqual(gh_reenable.auto_merge_calls, ["https://github.com/marmot-protocol/marmot-agents/pull/888"])

        # 3. Subsequent ticks on the SAME run_id keep monitoring the pending revert PR:
        #    pending -> noop_revert_pending; CI failure -> Meltdown!
        gh_reenable._inspected_pr = {
            "url": "https://github.com/marmot-protocol/marmot-agents/pull/888",
            "state": "OPEN",
            "auto_merge": True,
            "mergeable": "MERGEABLE",
            "checks": "pending",
        }
        r_pend = self._agent(repo, worktrees, state_dir, gh_reenable, dry_run=False).run_once()
        self.assertEqual(r_pend.action, "noop_revert_pending")

        gh_reenable._inspected_pr = {
            "url": "https://github.com/marmot-protocol/marmot-agents/pull/888",
            "state": "OPEN",
            "auto_merge": True,
            "mergeable": "MERGEABLE",
            "checks": "failed",
        }
        r_fail_ci = self._agent(repo, worktrees, state_dir, gh_reenable, dry_run=False).run_once()
        self.assertEqual(r_fail_ci.action, "meltdown")
        self.assertEqual(r_fail_ci.details.get("revert_status"), "revert_pr_checks_failed")

        # 4. Newly opened revert PR monitored on the SAME run_id escalates when it becomes CONFLICTING.
        _, origin2, work2, repo2, worktrees2, state_dir2, _ = self.make_repo()
        (work2 / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work2, "add", "check.sh")
        _git(work2, "commit", "-m", "Break check 2")
        culprit2 = _git(work2, "rev-parse", "HEAD")
        _git(work2, "push", "origin", "master")

        gh_live = FakeGitHub(
            run_info=ba.RunInfo(412, culprit2, "failure", "https://example/runs/412"),
            failed_jobs=[ba.FailedJob(412, "CI / Python", "failure", "https://example/jobs/412", "FAILED")],
        )
        r_open = self._agent(repo2, worktrees2, state_dir2, gh_live, dry_run=False).run_once()
        self.assertEqual(r_open.action, "revert_pr_opened")
        gh_live._inspected_pr = {
            "url": r_open.revert_pr_url,
            "state": "OPEN",
            "auto_merge": True,
            "mergeable": "CONFLICTING",
            "checks": "pending",
        }
        r_conf = self._agent(repo2, worktrees2, state_dir2, gh_live, dry_run=False).run_once()
        self.assertEqual(r_conf.action, "meltdown")
        self.assertEqual(r_conf.details.get("revert_status"), "revert_pr_conflicting")
        self.assertEqual(json.loads(_git(origin2, "show", "tree-state:state.json"))["state"], "red")

    def test_unmapped_job_retries_once_then_melts_down(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "note.txt").write_text("x\n", encoding="utf-8")
        _git(work, "add", "note.txt")
        _git(work, "commit", "-m", "Some commit")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        gh1 = FakeGitHub(
            run_info=ba.RunInfo(501, tip, "failure", "https://example/runs/501"),
            failed_jobs=[ba.FailedJob(51, "CI / Mystery Job", "failure", "https://example/jobs/51", "FAILED")],
        )
        r1 = self._agent(repo, worktrees, state_dir, gh1, dry_run=False).run_once()
        self.assertEqual(r1.bucket, "unmapped")
        self.assertEqual(r1.action, "retry_canary")

        gh2 = FakeGitHub(
            run_info=ba.RunInfo(502, tip, "failure", "https://example/runs/502"),
            failed_jobs=[ba.FailedJob(52, "CI / Mystery Job", "failure", "https://example/jobs/52", "FAILED")],
        )
        r2 = self._agent(repo, worktrees, state_dir, gh2, dry_run=False).run_once()
        self.assertEqual(r2.bucket, "unmapped")
        self.assertEqual(r2.action, "meltdown")

    def test_check_env_strips_credentials_and_isolates_home(self):
        _, _, _, repo, worktrees, state_dir, _ = self.make_repo()
        agent = self._agent(repo, worktrees, state_dir, FakeGitHub(), dry_run=True)
        prev = {k: os.environ.get(k) for k in ("GH_TOKEN", "GITHUB_TOKEN", "SSH_AUTH_SOCK", "MARMOT_ALERT_CMD")}
        try:
            os.environ["GH_TOKEN"] = "secret-gh"
            os.environ["GITHUB_TOKEN"] = "secret-gha"
            os.environ["SSH_AUTH_SOCK"] = "/tmp/fake-ssh.sock"
            os.environ["MARMOT_ALERT_CMD"] = "echo secret"
            env = agent._check_env()
        finally:
            for k, v in prev.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v
        for secret_key in ("GH_TOKEN", "GITHUB_TOKEN", "SSH_AUTH_SOCK", "MARMOT_ALERT_CMD"):
            self.assertNotIn(secret_key, env)
        self.assertEqual(env["HOME"], str(worktrees / ".check-home"))

    def test_hardening_edge_cases_timeout_sandbox_flake_pr_create_and_run_id(self):
        _, _, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work, "add", "check.sh")
        _git(work, "commit", "-m", "Break check")
        culprit = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        # 1. Missing required bwrap escalates cleanly to Meltdown instead of raising RuntimeError.
        gh1 = FakeGitHub(
            run_info=ba.RunInfo(801, culprit, "failure", "https://example/runs/801"),
            failed_jobs=[ba.FailedJob(81, "CI / Python", "failure", "https://example/jobs/81", "FAILED")],
        )
        ag_no_bwrap = self._agent(repo, worktrees, state_dir / "s1", gh1, dry_run=True, require_sandbox=True)
        ag_no_bwrap._sandbox_backend = None
        r1 = ag_no_bwrap.run_once()
        self.assertEqual(r1.action, "meltdown")
        self.assertEqual(r1.bucket, "baseline_broken")

        # 2. Supervisor timeouts on tip are NOT counted as deterministic regressions.
        gh2 = FakeGitHub(
            run_info=ba.RunInfo(802, culprit, "failure", "https://example/runs/802"),
            failed_jobs=[ba.FailedJob(82, "CI / Python", "failure", "https://example/jobs/82", "FAILED")],
        )
        ag_to = self._agent(repo, worktrees, state_dir / "s2", gh2, dry_run=True)
        ag_to._run_check_in_export = lambda *a, **kw: (False, "command timed out after 30s")  # type: ignore[method-assign]
        r2 = ag_to.run_once()
        self.assertEqual(r2.bucket, "infra")
        self.assertEqual(r2.action, "retry_canary")

        # 3. Tracked flake failing 1st attempt and passing 2nd stays known_flake (not promoted).
        gh3 = FakeGitHub(
            run_info=ba.RunInfo(803, culprit, "failure", "https://example/runs/803"),
            failed_jobs=[ba.FailedJob(83, "CI / Python", "failure", "https://example/jobs/83", "FAILED")],
            flaky_jobs={"Python"},
        )
        ag_fl = self._agent(repo, worktrees, state_dir / "s3", gh3, dry_run=True)
        seq = iter([(False, "flake once"), (True, "ok second")])
        ag_fl._run_check_in_export = lambda *a, **kw: next(seq)  # type: ignore[method-assign]
        r3 = ag_fl.run_once()
        self.assertEqual(r3.bucket, "known_flake")

        # 4. PR creation RuntimeError after branch push escalates cleanly to Meltdown.
        gh4 = FakeGitHub(
            run_info=ba.RunInfo(804, culprit, "failure", "https://example/runs/804"),
            failed_jobs=[ba.FailedJob(84, "CI / Python", "failure", "https://example/jobs/84", "FAILED")],
        )
        def _boom(*a, **kw):
            raise RuntimeError("gh pr create failed")
        gh4.open_revert_pr = _boom  # type: ignore[method-assign]
        r4 = self._agent(repo, worktrees, state_dir / "s4", gh4, dry_run=False).run_once()
        self.assertEqual(r4.action, "meltdown")
        self.assertIn("revert_pr_create_failed", str(r4.details.get("revert_status")))

        # 5. Failed older pending revert PR does NOT mark a newer canary run_id as handled.
        _, _, work2, repo2, worktrees2, state_dir2, _ = self.make_repo()
        (work2 / "check.sh").write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
        _git(work2, "add", "check.sh")
        _git(work2, "commit", "-m", "Break 1")
        c1 = _git(work2, "rev-parse", "HEAD")
        _git(work2, "push", "origin", "master")
        gh_a = FakeGitHub(
            run_info=ba.RunInfo(901, c1, "failure", "https://example/runs/901"),
            failed_jobs=[ba.FailedJob(91, "CI / Python", "failure", "https://example/jobs/91", "FAILED")],
        )
        r_open = self._agent(repo2, worktrees2, state_dir2, gh_a, dry_run=False).run_once()
        self.assertEqual(r_open.action, "revert_pr_opened")

        gh_b = FakeGitHub(
            run_info=ba.RunInfo(902, c1, "failure", "https://example/runs/902"),
            failed_jobs=[ba.FailedJob(92, "CI / Python", "failure", "https://example/jobs/92", "FAILED")],
        )
        gh_b._inspected_pr = {
            "url": r_open.revert_pr_url,
            "state": "OPEN",
            "auto_merge": True,
            "mergeable": "MERGEABLE",
            "checks": "failed",
        }
        ag_b = self._agent(repo2, worktrees2, state_dir2, gh_b, dry_run=False)
        r_melt = ag_b.run_once()
        self.assertEqual(r_melt.action, "meltdown")
        self.assertEqual(ag_b._load_state().get("last_handled_run_id"), 901)

    def test_github_client_list_open_prs_paginates_and_disables_auto_merge(self):
        # Contract: GitHubClient.list_open_prs paginates concatenated JSON arrays for base=master
        # and normalizes labels/auto_merge; disable_auto_merge invokes gh pr merge --disable-auto.
        page1 = [
            {
                "number": 101,
                "html_url": "https://github.com/marmot-protocol/marmot-agents/pull/101",
                "labels": [{"name": "enhancement"}],
                "auto_merge": {"merge_method": "squash"},
            },
        ]
        page2 = [
            {
                "number": 102,
                "html_url": "https://github.com/marmot-protocol/marmot-agents/pull/102",
                "labels": [{"name": "tree-fix"}],
                "auto_merge": {"merge_method": "squash"},
            },
            {
                "number": 103,
                "html_url": "https://github.com/marmot-protocol/marmot-agents/pull/103",
                "labels": [],
                "auto_merge": None,
            },
        ]
        seen_cmds: list[list[str]] = []

        def fake_gh_run(cmd, **kwargs):
            seen_cmds.append(list(cmd))
            if "pulls?state=open&base=master&per_page=100" in " ".join(cmd):
                return subprocess.CompletedProcess(cmd, 0, json.dumps(page1) + "\n" + json.dumps(page2), "")
            if "--disable-auto" in cmd:
                rc = 0 if cmd[3].endswith("/101") else 1
                return subprocess.CompletedProcess(cmd, rc, "", "failed" if rc else "")
            raise AssertionError(f"unexpected gh call: {cmd}")

        gh = ba.GitHubClient("marmot-protocol/marmot-agents", run_fn=fake_gh_run)
        prs = gh.list_open_prs(base="master")
        self.assertEqual(
            prs,
            [
                {
                    "number": 101,
                    "url": "https://github.com/marmot-protocol/marmot-agents/pull/101",
                    "labels": ["enhancement"],
                    "auto_merge": True,
                },
                {
                    "number": 102,
                    "url": "https://github.com/marmot-protocol/marmot-agents/pull/102",
                    "labels": ["tree-fix"],
                    "auto_merge": True,
                },
                {
                    "number": 103,
                    "url": "https://github.com/marmot-protocol/marmot-agents/pull/103",
                    "labels": [],
                    "auto_merge": False,
                },
            ],
        )
        self.assertTrue(gh.disable_auto_merge("https://github.com/marmot-protocol/marmot-agents/pull/101"))
        self.assertFalse(gh.disable_auto_merge("https://github.com/marmot-protocol/marmot-agents/pull/999"))
        self.assertIn("--paginate", seen_cmds[0])
        self.assertEqual(
            seen_cmds[1],
            [
                "gh",
                "pr",
                "merge",
                "https://github.com/marmot-protocol/marmot-agents/pull/101",
                "--repo",
                "marmot-protocol/marmot-agents",
                "--disable-auto",
            ],
        )

    def test_open_revert_pr_preserves_revert_label_when_impact_label_fails(self):
        # Contract: open_revert_pr falls back to `--label revert` before dropping all labels so
        # automated revert PRs keep their Meltdown override label if `impact:patch` is unavailable.
        seen_cmds: list[list[str]] = []

        def fake_gh_run(cmd, **kwargs):
            seen_cmds.append(list(cmd))
            if cmd[:3] == ["gh", "pr", "create"]:
                if "impact:patch" in cmd:
                    return subprocess.CompletedProcess(cmd, 1, "", "label impact:patch not found")
                return subprocess.CompletedProcess(cmd, 0, "https://github.com/marmot-protocol/marmot-agents/pull/555\n", "")
            if cmd[:3] == ["gh", "pr", "merge"]:
                return subprocess.CompletedProcess(cmd, 0, "", "")
            raise AssertionError(f"unexpected gh call: {cmd}")

        gh = ba.GitHubClient("marmot-protocol/marmot-agents", run_fn=fake_gh_run)
        url, am_ok = gh.open_revert_pr("bisect/revert-abcdef123", "Revert bad", "body")
        self.assertEqual(url, "https://github.com/marmot-protocol/marmot-agents/pull/555")
        self.assertTrue(am_ok)
        self.assertIn("revert", seen_cmds[1])
        self.assertNotIn("impact:patch", seen_cmds[1])

    def test_meltdown_disarms_non_recovery_auto_merge_prs_and_respects_dry_run_and_best_effort(self):
        # Contract: activating Meltdown disarms auto-merge on open non-recovery PRs targeting master,
        # skips `tree-fix` / `revert` and non-auto-merge PRs, performs zero mutations in dry-run,
        # and preserves tree-state=red + alert + issue comment even when disarming or listing fails.
        _, origin, work, repo, worktrees, state_dir, _ = self.make_repo()
        (work / "note.txt").write_text("innocent\n", encoding="utf-8")
        _git(work, "add", "note.txt")
        _git(work, "commit", "-m", "Innocent commit")
        tip = _git(work, "rev-parse", "HEAD")
        _git(work, "push", "origin", "master")

        pr_normal = "https://github.com/marmot-protocol/marmot-agents/pull/301"
        pr_fail = "https://github.com/marmot-protocol/marmot-agents/pull/302"
        pr_boom = "https://github.com/marmot-protocol/marmot-agents/pull/303"
        pr_tree_fix = "https://github.com/marmot-protocol/marmot-agents/pull/304"
        pr_revert = "https://github.com/marmot-protocol/marmot-agents/pull/305"
        pr_no_am = "https://github.com/marmot-protocol/marmot-agents/pull/306"
        open_prs = [
            {"number": 301, "url": pr_normal, "labels": ["enhancement"], "auto_merge": True},
            {"number": 302, "url": pr_fail, "labels": [], "auto_merge": True},
            {"number": 303, "url": pr_boom, "labels": ["bug"], "auto_merge": True},
            {"number": 304, "url": pr_tree_fix, "labels": ["tree-fix"], "auto_merge": True},
            {"number": 305, "url": pr_revert, "labels": ["revert", "impact:patch"], "auto_merge": True},
            {"number": 306, "url": pr_no_am, "labels": [], "auto_merge": False},
        ]

        # 1. Dry-run Meltdown: zero remote mutations, records candidates in would_disarm.
        gh_dry = FakeGitHub(
            run_info=ba.RunInfo(710, tip, "failure", "https://example/runs/710"),
            failed_jobs=[ba.FailedJob(710, "CI / Python", "failure", "https://example/jobs/710", "FAILED")],
            open_prs=open_prs,
        )
        alerts_dry: list[tuple[str, bool]] = []
        r_dry = self._agent(
            repo,
            worktrees,
            state_dir / "dry",
            gh_dry,
            dry_run=True,
            job_specs={"Python": ba.JobSpec(command="exit 1", required_bins=("bash",))},
            alerts=alerts_dry,
        ).run_once()
        self.assertEqual(r_dry.action, "meltdown")
        self.assertEqual(gh_dry.list_open_prs_calls, ["master"])
        self.assertEqual(gh_dry.disable_auto_merge_calls, [])
        self.assertEqual(r_dry.details["auto_merge_disarm"]["would_disarm"], [pr_normal, pr_fail, pr_boom])
        self.assertEqual(r_dry.details["auto_merge_disarm"]["disarmed"], [])
        self.assertEqual(r_dry.details["auto_merge_disarm"]["skipped_recovery"], [pr_tree_fix, pr_revert])
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "green")

        # 2. Live Meltdown: disarms non-recovery PRs best-effort; partial failure still disarms others,
        #    sets tree-state=red, sends alert, and comments on canary issue.
        gh_live = FakeGitHub(
            run_info=ba.RunInfo(711, tip, "failure", "https://example/runs/711"),
            failed_jobs=[ba.FailedJob(711, "CI / Python", "failure", "https://example/jobs/711", "FAILED")],
            open_prs=open_prs,
            disable_auto_merge_results={pr_fail: False, pr_boom: RuntimeError("transient API error")},
        )
        alerts_live: list[tuple[str, bool]] = []
        r_live = self._agent(
            repo,
            worktrees,
            state_dir / "live",
            gh_live,
            dry_run=False,
            job_specs={"Python": ba.JobSpec(command="exit 1", required_bins=("bash",))},
            alerts=alerts_live,
        ).run_once()
        self.assertEqual(r_live.action, "meltdown")
        self.assertEqual(r_live.tree_state, "red")
        self.assertEqual(gh_live.disable_auto_merge_calls, [pr_normal, pr_fail, pr_boom])
        self.assertEqual(r_live.details["auto_merge_disarm"]["disarmed"], [pr_normal])
        self.assertEqual(r_live.details["auto_merge_disarm"]["failed"], [pr_fail, pr_boom])
        self.assertEqual(r_live.details["auto_merge_disarm"]["skipped_recovery"], [pr_tree_fix, pr_revert])
        self.assertEqual(len(alerts_live), 1)
        self.assertIn(pr_normal, alerts_live[0][0])
        self.assertIn(pr_fail, alerts_live[0][0])
        self.assertEqual(len(gh_live.comments), 1)
        self.assertIn(pr_normal, gh_live.comments[0])
        self.assertIn(pr_fail, gh_live.comments[0])
        self.assertEqual(json.loads(_git(origin, "show", "tree-state:state.json"))["state"], "red")

        # 3. Listing failure still activates Meltdown, sends alert, and comments on canary issue.
        _, origin2, work2, repo2, worktrees2, state_dir2, _ = self.make_repo()
        (work2 / "note.txt").write_text("innocent\n", encoding="utf-8")
        _git(work2, "add", "note.txt")
        _git(work2, "commit", "-m", "Innocent commit 2")
        tip2 = _git(work2, "rev-parse", "HEAD")
        _git(work2, "push", "origin", "master")
        gh_err = FakeGitHub(
            run_info=ba.RunInfo(712, tip2, "failure", "https://example/runs/712"),
            failed_jobs=[ba.FailedJob(712, "CI / Python", "failure", "https://example/jobs/712", "FAILED")],
            list_open_prs_error=RuntimeError("GitHub 502"),
        )
        alerts_err: list[tuple[str, bool]] = []
        r_err = self._agent(
            repo2,
            worktrees2,
            state_dir2,
            gh_err,
            dry_run=False,
            job_specs={"Python": ba.JobSpec(command="exit 1", required_bins=("bash",))},
            alerts=alerts_err,
        ).run_once()
        self.assertEqual(r_err.action, "meltdown")
        self.assertEqual(len(alerts_err), 1)
        self.assertEqual(len(gh_err.comments), 1)
        self.assertIn("GitHub 502", r_err.details["auto_merge_disarm"].get("list_error", ""))
        self.assertEqual(json.loads(_git(origin2, "show", "tree-state:state.json"))["state"], "red")


def _extract_ci_step_run_script(step_name: str) -> str:
    """Extract the `run: |` shell script for a named step in `.github/workflows/ci.yml`."""
    ci_yml = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
    lines = ci_yml.read_text(encoding="utf-8").splitlines()
    target = f"- name: {step_name}"
    start = None
    for idx, line in enumerate(lines):
        if line.strip() == target:
            start = idx + 1
            break
    if start is None:
        raise AssertionError(f"step {step_name!r} not found in ci.yml")
    run_idx = None
    run_indent = 0
    for idx in range(start, len(lines)):
        stripped = lines[idx].strip()
        if stripped.startswith("- name:") or stripped.startswith("- uses:"):
            break
        if stripped == "run: |":
            run_idx = idx + 1
            run_indent = len(lines[idx]) - len(lines[idx].lstrip(" "))
            break
    if run_idx is None:
        raise AssertionError(f"`run: |` block not found for step {step_name!r}")
    body: list[str] = []
    for line in lines[run_idx:]:
        if line.strip() and (len(line) - len(line.lstrip(" "))) <= run_indent:
            break
        body.append(line)
    import textwrap

    return textwrap.dedent("\n".join(body)) + "\n"


class TreeStateGateScriptTests(unittest.TestCase):
    """Run the real `Check tree-state gate` bash step from `.github/workflows/ci.yml` against fake `gh`/`sleep`."""

    def _run_gate(self, tree_state: str, has_tree_override: str, label_responses: list[tuple[int, str]]):
        import base64

        script = _extract_ci_step_run_script("Check tree-state gate")
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        bin_dir = root / "bin"
        bin_dir.mkdir()
        state_b64 = base64.b64encode(
            json.dumps({"state": tree_state, "reason": "test reason"}).encode("utf-8")
        ).decode("ascii")
        calls_file = root / "calls.jsonl"
        resp_file = root / "label_responses.json"
        resp_file.write_text(json.dumps(label_responses), encoding="utf-8")
        idx_file = root / "label_idx.txt"
        idx_file.write_text("0", encoding="utf-8")

        fake_gh = bin_dir / "gh"
        fake_gh.write_text(
            f"""#!{sys.executable}
import json, sys
from pathlib import Path
args = sys.argv[1:]
with open({str(calls_file)!r}, "a", encoding="utf-8") as fh:
    fh.write(json.dumps({{"cmd": "gh", "args": args}}) + "\\n")
joined = " ".join(args)
if "contents/state.json?ref=tree-state" in joined:
    print({state_b64!r})
    raise SystemExit(0)
if "/labels?per_page=100" in joined:
    idx_p = Path({str(idx_file)!r})
    idx = int(idx_p.read_text(encoding="utf-8"))
    resps = json.loads(Path({str(resp_file)!r}).read_text(encoding="utf-8"))
    idx_p.write_text(str(idx + 1), encoding="utf-8")
    rc, body = resps[idx] if idx < len(resps) else (0, "[]")
    if body:
        sys.stdout.write(body)
    raise SystemExit(rc)
sys.stderr.write(f"unexpected gh call: {{args}}\\n")
raise SystemExit(2)
""",
            encoding="utf-8",
        )
        fake_gh.chmod(0o755)

        fake_sleep = bin_dir / "sleep"
        fake_sleep.write_text(
            f"""#!{sys.executable}
import json, sys
with open({str(calls_file)!r}, "a", encoding="utf-8") as fh:
    fh.write(json.dumps({{"cmd": "sleep", "args": sys.argv[1:]}}) + "\\n")
""",
            encoding="utf-8",
        )
        fake_sleep.chmod(0o755)

        env = os.environ.copy()
        env["PATH"] = f"{bin_dir}:{env.get('PATH', '')}"
        env["GH_TOKEN"] = "test-token"
        env["REPO"] = "marmot-protocol/marmot-agents"
        env["PR_NUMBER"] = "234"
        env["HAS_TREE_OVERRIDE"] = has_tree_override
        proc = subprocess.run(["bash", "-c", script], env=env, text=True, capture_output=True, check=False)
        calls = (
            [json.loads(line) for line in calls_file.read_text(encoding="utf-8").splitlines() if line.strip()]
            if calls_file.exists()
            else []
        )
        return proc, calls

    def test_green_tree_state_adds_zero_label_retries_or_sleeps(self):
        proc, calls = self._run_gate("green", "false", [])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual([c["cmd"] for c in calls], ["gh"])

    def test_red_tree_state_allows_delayed_revert_label_after_transient_api_error(self):
        # Catches the `gh pr create --label revert` race where labels attach after the opened event.
        proc, calls = self._run_gate(
            "red",
            "false",
            [
                (1, ""),
                (0, json.dumps([{"name": "revert"}])),
            ],
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("allowed via tree-fix/revert label", proc.stdout)
        sleeps = [c["args"][0] for c in calls if c["cmd"] == "sleep"]
        self.assertEqual(len(sleeps), 2)

    def test_red_tree_state_allows_delayed_tree_fix_label(self):
        proc, calls = self._run_gate(
            "red",
            "false",
            [
                (0, "[]"),
                (0, json.dumps([{"name": "tree-fix"}])),
            ],
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("allowed via tree-fix/revert label", proc.stdout)

    def test_red_tree_state_fails_closed_when_no_recovery_label_arrives(self):
        proc, calls = self._run_gate("red", "false", [(0, "[]"), (0, "[]"), (0, "[]")])
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("Meltdown Mode", proc.stdout)
        sleeps = [c["args"][0] for c in calls if c["cmd"] == "sleep"]
        self.assertGreaterEqual(len(sleeps), 2)

    def test_red_tree_state_fails_closed_when_all_label_rechecks_fail(self):
        proc, calls = self._run_gate("red", "false", [(1, ""), (1, ""), (1, "")])
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("Meltdown Mode", proc.stdout)
        sleeps = [c["args"][0] for c in calls if c["cmd"] == "sleep"]
        self.assertEqual(len(sleeps), 3)

    def test_check_tree_state_gate_step_binds_pr_number_env(self):
        ci_yml = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
        text = ci_yml.read_text(encoding="utf-8")
        idx = text.index("- name: Check tree-state gate")
        step_block = text[idx : text.index("run: |", idx)]
        self.assertIn("PR_NUMBER: ${{ github.event.pull_request.number }}", step_block)

    def test_red_tree_state_with_existing_override_does_not_sleep(self):
        proc, calls = self._run_gate("red", "true", [])
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual([c["cmd"] for c in calls], ["gh"])


if __name__ == "__main__":
    unittest.main()
