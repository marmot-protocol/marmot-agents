#!/usr/bin/env python3
"""Always-on revert-only Bisect Agent for marmot-protocol/marmot-agents (D-168, #222).

Watches completed `.github/workflows/canary.yml` runs on `master`, classifies failures into
four buckets (`infra`, `known_flake`, `suspected_race`, `deterministic_regression`),
bisects deterministic regressions over `canary..tip` by exporting each commit via
`git archive` and running checks inside a `bwrap` (bubblewrap) read-only-root + PID/IPC/mount
namespace sandbox with bot credentials and the control checkout masked out, opens a signed
`revert` PR when `git revert` applies cleanly and passes the exact failed check, and enters
Meltdown Mode (`tree-state` = `"red"` + White Noise / Marmot alert) whenever automated
recovery cannot safely fix `master`.

When `tree-state` is `"red"` and a completed `canary.yml` run genuinely advanced `canary`
to cover `master` (ignoring skipped runs), automatically resets `tree-state` back to `"green"`.

Supports `--mode dry-run` (default; persists local dry-run state so timer ticks do not
repeat work, while performing zero remote mutations) and `--mode live`.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import fcntl
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Iterator, Optional

sys.path.insert(0, str(Path(__file__).resolve().parent))
import flake_report  # noqa: E402

DEFAULT_REPO = "marmot-protocol/marmot-agents"
CANARY_WORKFLOW = "canary.yml"
CANARY_ISSUE_TITLE = "Canary: full gate failed"
FLAKE_ISSUE_TITLE = flake_report.TITLE
HEX_SHA_RE = re.compile(r"^[0-9a-f]{40}$")

# Jobs that never indicate an independent code failure on their own.
PURE_WRAPPER_JOBS = {
    "Report canary failure",
    "Fast-forward canary",
    "Check whether master moved",
    "Tier and changed paths",
    "CI / Tier and changed paths",
}

# Gate jobs that fail whenever an inner job fails, but also run `tools/ci` tests themselves.
GATE_JOBS = {"CI passed", "CI / CI passed"}

# Test/compiler failure markers that indicate a real failure rather than an infra outage.
REAL_FAILURE_MARKERS = [
    re.compile(r"test result: FAILED", re.I),
    re.compile(r"error\[E\d{4}\]:"),
    re.compile(r"FAILED \(failures=|FAILED \(errors="),
    re.compile(r"AssertionError|panicked at|exit status: 1", re.I),
]

# Curated infrastructure / runner / network outage patterns (Bucket 1).
INFRA_PATTERNS = [
    re.compile(r"The runner has received a shutdown signal", re.I),
    re.compile(r"The self-hosted runner:.*lost communication", re.I),
    re.compile(r"lost communication with the server", re.I),
    re.compile(r"No space left on device|ENOSPC", re.I),
    re.compile(r"HTTP\s+(502|503|504|429)\b", re.I),
    re.compile(r"API rate limit exceeded|secondary rate limit", re.I),
    re.compile(r"Failed to restore.*cache|Cache service responded with", re.I),
    re.compile(
        r"(crates\.io|index\.crates\.io|registry\.npmjs\.org|pypi\.org|ghcr\.io|registry-1\.docker\.io|github\.com)"
        r".*(timed out|connection reset|could not resolve|TLS|502|503|504|ECONNRESET|ETIMEDOUT|EAI_AGAIN)",
        re.I,
    ),
    re.compile(r"error: failed to get `.*` as a dependency.*Spurious network error", re.I),
    re.compile(r"Could not resolve host:", re.I),
    re.compile(r"The operation was canceled\.", re.I),
]

# Allowlisted environment variables forwarded to checked-out repository test commands.
ALLOWED_CHECK_ENV_KEYS = (
    "PATH",
    "ODIN",
    "RUSTC_WRAPPER",
    "SCCACHE_DIR",
    "SCCACHE_CACHE_SIZE",
    "CARGO_PROFILE_DEV_DEBUG",
    "LANG",
    "LC_ALL",
    "USER",
)


@dataclass(frozen=True)
class JobSpec:
    command: str
    required_bins: tuple[str, ...] = ()


# Maps normalized CI job names (with leading "CI / " stripped) to local reproduction commands.
# Note: test_bisect_agent verifies that every job name in .github/workflows/ci.yml is present
# here; keep the commands aligned with ci.yml when workflow steps change.
JOB_SPECS: dict[str, JobSpec] = {
    "Rust format and Clippy": JobSpec(
        command="cargo fmt --all --check && cargo clippy --locked --workspace --all-targets --all-features -- -D warnings",
        required_bins=("cargo",),
    ),
    "Rust tests (core)": JobSpec(
        command="npm ci --prefix packages/claude-code && cargo test --locked --all-features -p marmot-agent",
        required_bins=("cargo", "npm", "codex"),
    ),
    "Rust tests (drivers and the rest)": JobSpec(
        command="cargo test --locked --all-features --workspace --exclude marmot-agent --exclude marmot-agent-sim",
        required_bins=("cargo",),
    ),
    "Simulator": JobSpec(
        command=(
            "cargo test --locked --release -p marmot-agent-sim --all-features --no-run && "
            "MARMOT_AGENT_SIM_RUNS=200 timeout --kill-after=30s 4m "
            "cargo test --locked --release -p marmot-agent-sim --all-features"
        ),
        required_bins=("cargo",),
    ),
    "Move-over tests (Docker)": JobSpec(
        # Requires a host Docker daemon socket, which the bwrap sandbox intentionally masks
        # so checked-out code cannot escape via `docker run -v /:/host`. Treated as locally
        # unbisectable (retries canary once, then escalates to Meltdown on a second failure).
        command="",
        required_bins=(),
    ),
    "Conformance (socket)": JobSpec(
        command="npm ci --prefix packages/claude-code && npm ci --prefix packages/openclaw && cargo test --locked --all-features -p marmot-agent --test conformance_socket -- --ignored",
        required_bins=("cargo", "npm", "python3"),
    ),
    "Kani proofs": JobSpec(
        command="cargo kani -p marmot-agent --output-format terse",
        required_bins=("cargo-kani",),
    ),
    "Recordings": JobSpec(
        command=(
            "python3 -m unittest discover -s tools/prototype -p 'test_*.py' && "
            "python3 -m unittest discover -s tools/model_double -p 'test_*.py' && "
            "python3 tools/model_double/check_cassettes.py fixtures/cassettes --offline && "
            "python3 tools/prototype/translate.py --check "
            "fixtures/recordings/codex fixtures/recordings/claude "
            "fixtures/recordings/hermes fixtures/recordings/openclaw "
            "fixtures/recordings/acp/opencode fixtures/recordings/acp/goose fixtures/recordings/acp/cursor "
            "fixtures/recordings/pi && "
            "python3 tools/prototype/translate.py --expected fixtures/recordings/codex --check && "
            "python3 tools/prototype/translate.py --expected fixtures/recordings/claude --check && "
            "python3 tools/prototype/translate.py --expected fixtures/recordings/acp/opencode --check && "
            "python3 tools/prototype/translate.py --expected fixtures/recordings/acp/goose --check && "
            "python3 tools/prototype/translate.py --expected fixtures/recordings/acp/cursor --check && "
            "python3 tools/prototype/translate.py --expected fixtures/recordings/pi --check && "
            "python3 tools/prototype/translate.py --expected fixtures/recordings/openclaw --check && "
            "python3 tools/prototype/translate.py --expected fixtures/recordings/hermes --check"
        ),
        required_bins=("python3",),
    ),
    "TypeScript (claude-code)": JobSpec(
        command="(cd packages/claude-code && npm ci && npm run check && TMPDIR=/tmp npm test)",
        required_bins=("npm", "node"),
    ),
    "TypeScript (openclaw)": JobSpec(
        command="(cd packages/openclaw && npm ci && npm run check && TMPDIR=/tmp npm test)",
        required_bins=("npm", "node"),
    ),
    "Python": JobSpec(
        command=(
            "(cd python && python3 -m unittest discover -s tests) && "
            "python3 tools/ci/check_socket_compat.py && "
            "python3 -m unittest discover -s tools/release -p 'test_*.py'"
        ),
        required_bins=("python3",),
    ),
    "Install scripts": JobSpec(
        command=(
            "shellcheck -x install/*.sh tools/release/archive.sh tools/release/package.sh "
            "tools/release/package_connector.sh tools/tla/run.sh && "
            "TMPDIR=/tmp python3 -m unittest discover -v -s install -p 'test_*.py'"
        ),
        required_bins=("shellcheck", "python3"),
    ),
    "TLA+ models (fair)": JobSpec(
        command="TLC_WORKERS=4 python3 tools/tla/check.py --model CoreLifecycle --only fair",
        required_bins=("java", "python3"),
    ),
    "TLA+ models (give-up)": JobSpec(
        command="TLC_WORKERS=4 python3 tools/tla/check.py --model CoreLifecycle --only give-up-closes-item",
        required_bins=("java", "python3"),
    ),
    "TLA+ models (the other cases)": JobSpec(
        command="TLC_WORKERS=4 python3 tools/tla/check.py --model CoreLifecycle --except fair give-up-closes-item",
        required_bins=("java", "python3"),
    ),
    "TLA+ models (SocketProtocol)": JobSpec(
        command="TLC_WORKERS=4 python3 tools/tla/check.py --model SocketProtocol",
        required_bins=("java", "python3"),
    ),
    "TLA+ models (DriverTransport)": JobSpec(
        command="TLC_WORKERS=4 python3 tools/tla/check.py --model DriverTransport",
        required_bins=("java", "python3"),
    ),
    "TLA+ models (InstallUpgrade and ActivityHoldBack)": JobSpec(
        command="TLC_WORKERS=4 python3 tools/tla/check.py --model InstallUpgrade ActivityHoldBack",
        required_bins=("java", "python3"),
    ),
    "Documents": JobSpec(
        command="lychee .",
        required_bins=("lychee",),
    ),
    "CI passed": JobSpec(
        command="python3 -m unittest discover -s tools/ci -p 'test_*.py'",
        required_bins=("python3",),
    ),
}


def _pr_check_state(rollup: list[dict] | None) -> str:
    """Summarize GitHub `statusCheckRollup` entries as `'failed'`, `'pending'`, or `'passed'`."""
    if not rollup:
        return "pending"
    saw_pending = False
    for item in rollup:
        if not isinstance(item, dict):
            continue
        conclusion = str(item.get("conclusion") or "").upper()
        status = str(item.get("status") or "").upper()
        state = str(item.get("state") or "").upper()
        if conclusion in {"FAILURE", "TIMED_OUT", "CANCELLED", "ACTION_REQUIRED", "STARTUP_FAILURE", "STALE"} or state in {"FAILURE", "ERROR"}:
            return "failed"
        if status in {"QUEUED", "IN_PROGRESS", "PENDING", "WAITING", "REQUESTED"} or state == "PENDING":
            saw_pending = True
    return "pending" if saw_pending else "passed"


def _normalize_pr_info(pr: dict) -> dict:
    return {
        "url": str(pr.get("url") or ""),
        "state": str(pr.get("state") or "OPEN").upper(),
        "auto_merge": pr.get("autoMergeRequest") is not None,
        # Note: GitHub computes `mergeable` lazily and may return UNKNOWN on the first read;
        # pending revert monitoring re-checks on subsequent ticks until merged or blocked.
        "mergeable": str(pr.get("mergeable") or "UNKNOWN").upper(),
        "checks": _pr_check_state(pr.get("statusCheckRollup")),
    }


def utc_now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def normalize_job_name(name: str) -> str:
    return name[5:] if name.startswith("CI / ") else name


def validate_sha(sha: str) -> str:
    s = sha.strip().lower()
    if not HEX_SHA_RE.match(s):
        raise ValueError(f"invalid 40-hex commit SHA: {sha!r}")
    return s


def parse_flake_tracker_jobs(issue_body: str) -> set[str]:
    """Extract job names with recorded flakes from `Flaky job tracker` (D-143) via flake_report."""
    if not issue_body:
        return set()
    try:
        state = flake_report.read_state(issue_body)
    except flake_report.StateError:
        return set()
    flaky: set[str] = set()
    for entry in state.get("jobs", []):
        if isinstance(entry, dict) and isinstance(entry.get("count"), int) and entry["count"] > 0:
            job_name = entry.get("job")
            if isinstance(job_name, str) and job_name and job_name != flake_report.NOT_ON_DEFAULT_BRANCH:
                flaky.add(job_name)
                flaky.add(normalize_job_name(job_name))
    return flaky


def is_infra_failure(conclusion: str, log_tail: str) -> Optional[str]:
    """Return the matched infra reason if log/conclusion looks like infrastructure failure.

    Real test/compiler failure markers take precedence so an earlier transient warning
    (e.g. cache restore error) cannot mask a real test failure.
    """
    text = log_tail or ""
    if any(p.search(text) for p in REAL_FAILURE_MARKERS):
        return None
    for pat in INFRA_PATTERNS:
        m = pat.search(text)
        if m:
            return f"matched infra signature: {m.group(0)}"
    if conclusion == "timed_out":
        return "job timed_out with no test/compiler failure marker"
    return None


def detect_sandbox_backend(which_fn: Callable[[str], Optional[str]] = shutil.which) -> Optional[str]:
    """Detect working `bwrap` (bubblewrap) with read-only root and PID/IPC namespace isolation."""
    if which_fn("bwrap"):
        probe = subprocess.run(
            [
                "bwrap",
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--unshare-pid",
                "--proc",
                "/proc",
                "--unshare-ipc",
                "--die-with-parent",
                "--",
                "true",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=10,
            check=False,
        )
        if probe.returncode == 0:
            return "bwrap"
    return None


@dataclass
class RunInfo:
    run_id: int
    head_sha: str
    conclusion: str
    html_url: str


@dataclass
class FailedJob:
    job_id: int
    name: str
    conclusion: str
    html_url: str
    log_tail: str = ""


@dataclass
class JobClassification:
    job: FailedJob
    bucket: str
    reason: str
    spec: Optional[JobSpec] = None
    passes_tip: int = 0
    fails_tip: int = 0
    tip_tail: str = ""


@dataclass
class AgentResult:
    action: str
    bucket: Optional[str] = None
    dry_run: bool = True
    run_id: Optional[int] = None
    head_sha: Optional[str] = None
    canary_sha: Optional[str] = None
    failed_job: Optional[str] = None
    culprit_sha: Optional[str] = None
    revert_branch: Optional[str] = None
    revert_pr_url: Optional[str] = None
    tree_state: Optional[str] = None
    alert_message: Optional[str] = None
    details: dict = field(default_factory=dict)


class GitHubClient:
    """Thin wrapper over `gh` CLI so tests can inject a fake client."""

    def __init__(self, repo: str, run_fn: Callable = subprocess.run):
        self.repo = repo
        self._run = run_fn

    def _gh(self, args: list[str], input_text: Optional[str] = None, check: bool = True) -> subprocess.CompletedProcess:
        return self._run(
            ["gh", *args],
            input=input_text,
            text=True,
            capture_output=True,
            timeout=120,
            check=check,
        )

    def iter_completed_canary_runs(self, per_page: int = 30, max_pages: int = 10) -> Iterator[RunInfo]:
        """Yield completed canary.yml runs on master newest-first across pages."""
        for page in range(1, max_pages + 1):
            out = self._gh(
                [
                    "api",
                    f"repos/{self.repo}/actions/workflows/{CANARY_WORKFLOW}/runs"
                    f"?branch=master&status=completed&per_page={per_page}&page={page}",
                ]
            ).stdout
            runs = json.loads(out).get("workflow_runs", [])
            if not runs:
                return
            for r in runs:
                conclusion = r.get("conclusion") or ""
                if conclusion in {"success", "failure", "timed_out"}:
                    yield RunInfo(
                        run_id=int(r["id"]),
                        head_sha=validate_sha(r["head_sha"]),
                        conclusion=conclusion,
                        html_url=str(r.get("html_url") or ""),
                    )
            if len(runs) < per_page:
                return

    def latest_failed_canary_run(self) -> Optional[RunInfo]:
        """Query `status=failure` directly so an unresolved failure is never hidden by >300 skip runs."""
        out = self._gh(
            [
                "api",
                f"repos/{self.repo}/actions/workflows/{CANARY_WORKFLOW}/runs"
                "?branch=master&status=failure&per_page=10",
            ]
        ).stdout
        for r in json.loads(out).get("workflow_runs", []):
            conclusion = r.get("conclusion") or ""
            if conclusion in {"failure", "timed_out"}:
                return RunInfo(
                    run_id=int(r["id"]),
                    head_sha=validate_sha(r["head_sha"]),
                    conclusion=conclusion,
                    html_url=str(r.get("html_url") or ""),
                )
        return None

    def failed_jobs_for_run(self, run_id: int) -> list[FailedJob]:
        out = self._gh(["api", "--paginate", f"repos/{self.repo}/actions/runs/{run_id}/jobs?per_page=100"]).stdout
        jobs: list[dict] = []
        dec = json.JSONDecoder()
        idx = 0
        s = out.strip()
        while idx < len(s):
            obj, end = dec.raw_decode(s, idx)
            jobs.extend(obj.get("jobs", []))
            idx = end
            while idx < len(s) and s[idx].isspace():
                idx += 1

        inner_failed: list[FailedJob] = []
        gate_failed: list[FailedJob] = []
        for j in jobs:
            conclusion = j.get("conclusion") or ""
            name = j.get("name") or ""
            if conclusion not in {"failure", "timed_out"}:
                continue
            if name in PURE_WRAPPER_JOBS:
                continue
            fj = FailedJob(
                job_id=int(j["id"]),
                name=name,
                conclusion=conclusion,
                html_url=str(j.get("html_url") or ""),
                log_tail=self.job_log_tail(int(j["id"])),
            )
            if name in GATE_JOBS:
                gate_failed.append(fj)
            else:
                inner_failed.append(fj)
        return inner_failed if inner_failed else gate_failed

    def job_log_tail(self, job_id: int, max_chars: int = 16000) -> str:
        proc = self._gh(["api", f"repos/{self.repo}/actions/jobs/{job_id}/logs"], check=False)
        if proc.returncode != 0:
            return ""
        text = proc.stdout or ""
        return text[-max_chars:] if len(text) > max_chars else text

    def flaky_job_names(self) -> set[str]:
        proc = self._gh(
            [
                "issue",
                "list",
                "--repo",
                self.repo,
                "--state",
                "open",
                "--search",
                f'"{FLAKE_ISSUE_TITLE}" in:title',
                "--json",
                "title,body",
                "--limit",
                "5",
            ],
            check=False,
        )
        if proc.returncode != 0:
            return set()
        for item in json.loads(proc.stdout or "[]"):
            if item.get("title") == FLAKE_ISSUE_TITLE:
                return parse_flake_tracker_jobs(item.get("body") or "")
        return set()

    def open_revert_pr_for_sha(self, culprit_sha: str) -> Optional[dict]:
        short = culprit_sha[:9]
        expected_branch = f"bisect/revert-{short}"
        proc = self._gh(
            [
                "pr",
                "list",
                "--repo",
                self.repo,
                "--state",
                "open",
                "--json",
                "url,state,headRefName,body,autoMergeRequest,mergeable,statusCheckRollup",
                "--limit",
                "100",
            ],
            check=False,
        )
        if proc.returncode != 0:
            return None
        for pr in json.loads(proc.stdout or "[]"):
            ref = pr.get("headRefName") or ""
            body = pr.get("body") or ""
            if ref == expected_branch or (ref.startswith("bisect/revert-") and culprit_sha in body):
                return _normalize_pr_info(pr)
        return None

    def inspect_pr(self, pr_url: str) -> Optional[dict]:
        proc = self._gh(
            [
                "pr",
                "view",
                pr_url,
                "--repo",
                self.repo,
                "--json",
                "url,state,autoMergeRequest,mergeable,statusCheckRollup",
            ],
            check=False,
        )
        if proc.returncode != 0:
            return None
        try:
            return _normalize_pr_info(json.loads(proc.stdout))
        except json.JSONDecodeError:
            return None

    def enable_auto_merge(self, pr_url: str) -> bool:
        proc = self._gh(["pr", "merge", pr_url, "--repo", self.repo, "--auto", "--squash"], check=False)
        return proc.returncode == 0

    def dispatch_canary(self, force: bool = True) -> None:
        args = ["workflow", "run", CANARY_WORKFLOW, "--repo", self.repo, "--ref", "master"]
        if force:
            args += ["-f", "force=true"]
        self._gh(args)

    def comment_canary_issue(self, body: str) -> None:
        proc = self._gh(
            [
                "issue",
                "list",
                "--repo",
                self.repo,
                "--state",
                "open",
                "--search",
                f'"{CANARY_ISSUE_TITLE}" in:title',
                "--json",
                "number,title",
                "--limit",
                "10",
            ],
            check=False,
        )
        number = None
        if proc.returncode == 0:
            for item in json.loads(proc.stdout or "[]"):
                if item.get("title") == CANARY_ISSUE_TITLE:
                    number = int(item["number"])
                    break
        if number is not None:
            self._gh(["issue", "comment", str(number), "--repo", self.repo, "--body", body])
        else:
            self._gh(["issue", "create", "--repo", self.repo, "--title", CANARY_ISSUE_TITLE, "--body", body])

    def open_revert_pr(self, branch: str, title: str, body: str) -> tuple[str, bool]:
        args = [
            "pr",
            "create",
            "--repo",
            self.repo,
            "--base",
            "master",
            "--head",
            branch,
            "--title",
            title,
            "--body",
            body,
            "--label",
            "revert",
            "--label",
            "impact:patch",
        ]
        proc = self._gh(args, check=False)
        if proc.returncode != 0:
            proc = self._gh(args[:-4], check=True)
        pr_url = proc.stdout.strip().splitlines()[-1].strip()
        return pr_url, self.enable_auto_merge(pr_url)


def send_marmot_alert(message: str, dry_run: bool, alert_cmd: Optional[str] = None) -> dict:
    """Deliver a Meltdown alert via White Noise / Marmot CLI (`MARMOT_ALERT_CMD` on stdin)."""
    cmd = alert_cmd if alert_cmd is not None else os.environ.get("MARMOT_ALERT_CMD", "").strip()
    record = {"sent": False, "dry_run": dry_run, "channel": "command" if cmd else "none", "message": message}
    if cmd and not dry_run:
        proc = subprocess.run(
            shlex.split(cmd),
            input=message,
            text=True,
            capture_output=True,
            timeout=60,
            check=False,
        )
        record["sent"] = proc.returncode == 0
        record["returncode"] = proc.returncode
    return record


class BisectAgent:
    def __init__(
        self,
        repo_dir: Path,
        worktrees_dir: Path,
        state_dir: Path,
        gh: GitHubClient,
        dry_run: bool = True,
        repro_attempts: int = 2,
        step_timeout_sec: int = 1200,
        sign_commits: bool = True,
        require_sandbox: bool = True,
        extra_masked_dirs: tuple[Path, ...] = (),
        job_specs: Optional[dict[str, JobSpec]] = None,
        which_fn: Callable[[str], Optional[str]] = shutil.which,
        alert_fn: Callable[[str, bool], dict] = send_marmot_alert,
    ):
        self.repo_dir = Path(repo_dir).resolve()
        self.worktrees_dir = Path(worktrees_dir).resolve()
        self.state_dir = Path(state_dir).resolve()
        self.gh = gh
        self.dry_run = dry_run
        self.repro_attempts = max(1, repro_attempts)
        self.step_timeout_sec = step_timeout_sec
        self.sign_commits = sign_commits
        self.require_sandbox = require_sandbox
        self.extra_masked_dirs = tuple(Path(p).resolve() for p in extra_masked_dirs)
        self.job_specs = job_specs if job_specs is not None else JOB_SPECS
        self._which = which_fn
        self._alert = alert_fn
        self._sandbox_backend = detect_sandbox_backend(which_fn)

        # Validate directory separation before creating directories so rejected layouts leave nothing behind.
        self._validate_directory_separation()
        self.worktrees_dir.mkdir(parents=True, exist_ok=True)
        self.state_dir.mkdir(parents=True, exist_ok=True)
        # Keep dry-run local state separate from live state so dry-run ticks never pollute live.
        self.state_file = self.state_dir / ("bisect-state.dry-run.json" if dry_run else "bisect-state.json")

    def _sccache_dir(self) -> Optional[Path]:
        raw = os.environ.get("SCCACHE_DIR", "").strip()
        return Path(raw).resolve() if raw else None

    def _validate_directory_separation(self) -> None:
        """Reject overlapping writable/sensitive directory layouts so sandbox masks are never bypassed."""
        cargo_dir, rustup_dir = self._sandbox_cache_dirs(create=False)
        writable: list[Path] = [self.worktrees_dir, cargo_dir, rustup_dir]
        sccache_dir = self._sccache_dir()
        if sccache_dir is not None:
            writable.append(sccache_dir)
        sensitive = [self.repo_dir, self.state_dir, *self.extra_masked_dirs]
        real_home = os.environ.get("HOME")
        if real_home:
            hp = Path(real_home).resolve()
            sensitive += [hp / ".ssh", hp / ".config", hp / ".gnupg", hp / ".cargo"]
        for w in writable:
            for s in sensitive:
                if w == s or s in w.parents or w in s.parents:
                    raise ValueError(
                        f"writable bisect directory {w} overlaps sensitive masked path {s}; "
                        "place --worktrees-dir and cache directories outside repo/state/credential paths"
                    )

    def _git(self, args: list[str], cwd: Optional[Path] = None, check: bool = True) -> subprocess.CompletedProcess:
        return subprocess.run(
            ["git", *args],
            cwd=str(cwd or self.repo_dir),
            text=True,
            capture_output=True,
            timeout=120,
            check=check,
        )

    def _load_state(self) -> dict:
        if self.state_file.is_file():
            try:
                return json.loads(self.state_file.read_text(encoding="utf-8"))
            except json.JSONDecodeError:
                pass
        return {"last_handled_run_id": None, "last_failed_sha": None, "consecutive_failures": 0}

    def _record_handled(
        self,
        state: dict,
        run_id: int,
        consecutive_failures: int,
        last_failed_sha: Optional[str],
        pending_revert_pr: Optional[dict] = None,
    ) -> None:
        # Local state is persisted in both dry-run and live modes (zero remote mutations in dry-run)
        # so timer ticks do not repeat expensive bisects and dry-run can verify 2nd-failure escalation.
        state["last_handled_run_id"] = run_id
        state["consecutive_failures"] = consecutive_failures
        state["last_failed_sha"] = last_failed_sha
        state["pending_revert_pr"] = pending_revert_pr
        tmp = self.state_file.with_suffix(".tmp")
        tmp.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        tmp.replace(self.state_file)

    def _remove_worktree(self, path: Path) -> None:
        self._git(["worktree", "remove", "--force", str(path)], check=False)
        shutil.rmtree(path, ignore_errors=True)

    def _is_ancestor(self, older: str, newer: str) -> bool:
        return self._git(["merge-base", "--is-ancestor", older, newer], check=False).returncode == 0

    def _fetch_refs(self) -> tuple[str, str, str]:
        self._git(["fetch", "--prune", "origin", "master", "canary"])
        self._git(["fetch", "--prune", "origin", "tree-state"], check=False)
        master_sha = validate_sha(self._git(["rev-parse", "origin/master"]).stdout)
        canary_sha = validate_sha(self._git(["rev-parse", "origin/canary"]).stdout)
        return master_sha, canary_sha, self._read_tree_state()

    def _read_tree_state(self) -> str:
        proc = self._git(["show", "origin/tree-state:state.json"], check=False)
        if proc.returncode != 0:
            return "unknown"
        try:
            return str(json.loads(proc.stdout).get("state") or "unknown")
        except json.JSONDecodeError:
            return "unknown"

    def update_tree_state(self, new_state: str, reason: str, run_url: Optional[str]) -> str:
        """Update remote `tree-state` branch. Returns `'updated'`, `'already_set'`, or `'failed'`."""
        cur_state = self._read_tree_state()
        if cur_state == new_state:
            return "already_set"
        payload = {
            "schema_version": 1,
            "state": new_state,
            "updated_at": utc_now_iso(),
            "reason": reason,
            "run_url": run_url,
        }
        if self.dry_run:
            return "updated"
        ts_wt = self.worktrees_dir / "tree-state"
        self._remove_worktree(ts_wt)
        try:
            fetch = self._git(["fetch", "origin", "tree-state"], check=False)
            if fetch.returncode != 0:
                return "failed"
            if self._read_tree_state() == new_state:
                return "already_set"
            add_wt = self._git(["worktree", "add", str(ts_wt), "-B", "tree-state", "origin/tree-state"], check=False)
            if add_wt.returncode != 0:
                return "failed"
            (ts_wt / "state.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
            self._git(["add", "state.json"], cwd=ts_wt)
            commit_cmd = ["commit"] + (["-S"] if self.sign_commits else [])
            commit_cmd += ["-m", f"Set tree-state to {new_state}: {reason}"]
            if self._git(commit_cmd, cwd=ts_wt, check=False).returncode != 0:
                return "failed"
            if self._git(["push", "origin", "tree-state"], cwd=ts_wt, check=False).returncode != 0:
                return "failed"
            return "updated"
        finally:
            self._remove_worktree(ts_wt)

    def _masked_paths(self) -> tuple[list[Path], list[Path]]:
        """Directories (masked via tmpfs) and files (masked via /dev/null) hidden from check code."""
        dirs: list[Path] = [self.repo_dir, self.state_dir, *self.extra_masked_dirs]
        files: list[Path] = [
            Path("/run/docker.sock"),
            Path("/var/run/docker.sock"),
        ]
        real_home = os.environ.get("HOME")
        if real_home:
            hp = Path(real_home).resolve()
            for sub in (".ssh", ".config", ".gnupg", ".cargo"):
                dirs.append(hp / sub)
            for fname in (".gitconfig", ".git-credentials", ".netrc"):
                files.append(hp / fname)
        run_user = Path(f"/run/user/{os.getuid()}")
        dirs.append(run_user)
        dirs.append(Path("/run/podman"))
        existing_dirs = [d for d in dict.fromkeys(dirs) if d.is_dir()]
        existing_files = [f for f in dict.fromkeys(files) if f.exists()]
        return existing_dirs, existing_files

    def _sandbox_cache_dirs(self, create: bool = True) -> tuple[Path, Path]:
        """Dedicated credential-free writable CARGO_HOME and RUSTUP_HOME directories."""
        cargo_dir = (self.worktrees_dir / "cache/cargo").resolve()
        rustup_dir = (self.worktrees_dir / "cache/rustup").resolve()
        env_cargo = os.environ.get("BISECT_CARGO_HOME", "").strip()
        env_rustup = os.environ.get("BISECT_RUSTUP_HOME", "").strip()
        if env_cargo:
            cargo_dir = Path(env_cargo).resolve()
        if env_rustup:
            rustup_dir = Path(env_rustup).resolve()
        if create:
            cargo_dir.mkdir(parents=True, exist_ok=True)
            rustup_dir.mkdir(parents=True, exist_ok=True)
        return cargo_dir, rustup_dir

    def _check_env(self) -> dict[str, str]:
        """Build a minimal allowlisted environment without bot tokens, SSH agent, or alert hooks."""
        env: dict[str, str] = {}
        for key in ALLOWED_CHECK_ENV_KEYS:
            val = os.environ.get(key)
            if val is not None:
                env[key] = val
        cargo_dir, rustup_dir = self._sandbox_cache_dirs()
        cargo_bin = cargo_dir / "bin"
        if cargo_bin.is_dir():
            cur_path = env.get("PATH", "")
            parts = cur_path.split(os.pathsep) if cur_path else []
            if str(cargo_bin) not in parts:
                env["PATH"] = os.pathsep.join([str(cargo_bin), *parts])
        env["CARGO_HOME"] = str(cargo_dir)
        env["RUSTUP_HOME"] = str(rustup_dir)
        sandbox_home = (self.worktrees_dir / ".check-home").resolve()
        env["HOME"] = str(sandbox_home)
        env["XDG_CONFIG_HOME"] = str(sandbox_home / ".config")
        env["TMPDIR"] = "/tmp"
        env["CARGO_TERM_COLOR"] = "never"
        env["CARGO_INCREMENTAL"] = "0"
        return env

    def _export_tree(self, source_repo: Path, ref: str, dest: Path) -> None:
        """Export a commit via `git archive` so checked-out code has no `.git` pointer to repo_dir."""
        shutil.rmtree(dest, ignore_errors=True)
        dest.mkdir(parents=True, exist_ok=True)
        archive = subprocess.Popen(
            ["git", "archive", "--format=tar", ref],
            cwd=str(source_repo),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        assert archive.stdout is not None
        extract = subprocess.run(
            ["tar", "-xf", "-", "-C", str(dest)],
            stdin=archive.stdout,
            capture_output=True,
            timeout=120,
            check=False,
        )
        archive.stdout.close()
        _, arch_err = archive.communicate(timeout=120)
        if archive.returncode != 0 or extract.returncode != 0:
            raise RuntimeError(f"git archive export failed: {arch_err.decode(errors='replace')} {extract.stderr.decode(errors='replace')}")

    def _build_sandbox_argv(self, export_dir: Path, command: str) -> list[str]:
        backend = self._sandbox_backend
        if backend != "bwrap":
            if self.require_sandbox:
                raise RuntimeError(
                    "bwrap (bubblewrap) with read-only root and PID/IPC namespace isolation is unavailable; "
                    "refusing to run repository code without OS filesystem isolation"
                )
            return ["bash", "-euo", "pipefail", "-c", command]

        self._validate_directory_separation()
        mask_dirs, mask_files = self._masked_paths()
        sandbox_home = (self.worktrees_dir / ".check-home").resolve()
        cargo_dir, rustup_dir = self._sandbox_cache_dirs()
        writable_dirs: list[Path] = [export_dir.resolve(), sandbox_home, cargo_dir, rustup_dir]
        sccache_dir = self._sccache_dir()
        if sccache_dir is not None and sccache_dir.is_dir():
            writable_dirs.append(sccache_dir)

        # Read-only host root + private /tmp, PID, and IPC namespaces + tmpfs/dev-null masks over
        # credentials, container daemon sockets, and control-plane directories + explicit writable
        # build/cache binds (credential-free CARGO_HOME, RUSTUP_HOME, per-check HOME, and sccache).
        argv = [
            "bwrap",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--unshare-pid",
            "--proc",
            "/proc",
            "--unshare-ipc",
            "--die-with-parent",
        ]
        # Never silently skip any mask: directory separation was enforced at __init__.
        for d in mask_dirs:
            argv += ["--tmpfs", str(d)]
        # Preserve read-only toolchain shims if cargo/rustup live under $HOME/.cargo/bin.
        real_home = os.environ.get("HOME")
        if real_home:
            host_cargo_bin = (Path(real_home).resolve() / ".cargo/bin")
            if host_cargo_bin.is_dir():
                argv += ["--ro-bind", str(host_cargo_bin), str(host_cargo_bin)]
        for f in mask_files:
            argv += ["--ro-bind", "/dev/null", str(f)]
        for w in dict.fromkeys(writable_dirs):
            argv += ["--bind", str(w), str(w)]
        # Keep toolchain shims in CARGO_HOME/bin read-only so checked-out code cannot overwrite them,
        # and mask crates.io credentials/config overrides inside cargo_dir.
        cargo_bin = cargo_dir / "bin"
        if cargo_bin.is_dir():
            argv += ["--ro-bind", str(cargo_bin), str(cargo_bin)]
        for masked_name in ("credentials.toml", "credentials", "config.toml", "config"):
            masked_file = cargo_dir / masked_name
            if masked_file.exists():
                argv += ["--ro-bind", "/dev/null", str(masked_file)]
        argv += ["--chdir", str(export_dir), "--", "bash", "-euo", "pipefail", "-c", command]
        return argv

    def _can_run_locally(self, spec: JobSpec) -> tuple[bool, list[str]]:
        if not spec.command:
            return (False, list(spec.required_bins))
        missing = [b for b in spec.required_bins if self._which(b) is None]
        return (len(missing) == 0, missing)

    def _run_check_in_export(self, source_repo: Path, ref: str, command: str) -> tuple[bool, str]:
        export_dir = self.worktrees_dir / "sandbox-export"
        sandbox_home = (self.worktrees_dir / ".check-home").resolve()
        try:
            # Recreate .check-home fresh every run so no checked-out commit leaks state to the next.
            shutil.rmtree(sandbox_home, ignore_errors=True)
            sandbox_home.mkdir(parents=True, exist_ok=True)
            self._export_tree(source_repo, ref, export_dir)
            argv = self._build_sandbox_argv(export_dir, command)
            proc = subprocess.run(
                argv,
                cwd=str(export_dir),
                env=self._check_env(),
                text=True,
                capture_output=True,
                timeout=self.step_timeout_sec,
                check=False,
            )
            combined = (proc.stdout or "") + "\n" + (proc.stderr or "")
            return (proc.returncode == 0, combined[-4000:])
        except subprocess.TimeoutExpired as exc:
            return (False, f"command timed out after {self.step_timeout_sec}s: {exc}")
        finally:
            shutil.rmtree(export_dir, ignore_errors=True)
            shutil.rmtree(sandbox_home, ignore_errors=True)

    def _reproduce_on_sha(self, sha: str, command: str, attempts: int) -> tuple[int, int, str]:
        """Run command `attempts` times on clean sandboxed exports of `sha`."""
        passes = 0
        fails = 0
        last_tail = ""
        for _ in range(attempts):
            ok, tail = self._run_check_in_export(self.repo_dir, sha, command)
            last_tail = tail
            if ok:
                passes += 1
            else:
                fails += 1
        return passes, fails, last_tail

    def _classify_job(self, job: FailedJob, run_info: RunInfo, canary_sha: str, flaky_names: set[str]) -> JobClassification:
        norm_name = normalize_job_name(job.name)
        infra_reason = is_infra_failure(job.conclusion, job.log_tail)
        if infra_reason is not None:
            return JobClassification(job=job, bucket="infra", reason=infra_reason)

        spec = self.job_specs.get(norm_name)
        can_run, missing_bins = self._can_run_locally(spec) if spec else (False, [])
        is_known_flake = (job.name in flaky_names) or (norm_name in flaky_names)

        if not spec or not can_run:
            if is_known_flake:
                return JobClassification(job=job, bucket="known_flake", reason="listed in Flaky job tracker (not locally runnable)")
            reason = (
                f"no local repro command for job {job.name}"
                if not spec
                else f"missing local tools {missing_bins} to bisect {job.name}"
            )
            return JobClassification(job=job, bucket="unmapped", reason=reason)

        attempts = 1 if is_known_flake else self.repro_attempts
        passes_tip, fails_tip, tip_tail = self._reproduce_on_sha(run_info.head_sha, spec.command, attempts)
        if passes_tip > 0:
            if is_known_flake:
                return JobClassification(
                    job=job,
                    bucket="known_flake",
                    reason="listed in Flaky job tracker and did not reproduce deterministically",
                    spec=spec,
                    passes_tip=passes_tip,
                    fails_tip=fails_tip,
                    tip_tail=tip_tail,
                )
            return JobClassification(
                job=job,
                bucket="suspected_race",
                reason=f"passed {passes_tip}/{attempts} locally on {run_info.head_sha[:9]}",
                spec=spec,
                passes_tip=passes_tip,
                fails_tip=fails_tip,
                tip_tail=tip_tail,
            )

        passes_base, fails_base, _ = self._reproduce_on_sha(canary_sha, spec.command, 1)
        if fails_base > 0 or passes_base == 0:
            return JobClassification(
                job=job,
                bucket="baseline_broken",
                reason=f"baseline canary {canary_sha[:9]} also fails `{job.name}` locally",
                spec=spec,
                passes_tip=passes_tip,
                fails_tip=fails_tip,
                tip_tail=tip_tail,
            )

        return JobClassification(
            job=job,
            bucket="deterministic_regression",
            reason=f"failed {fails_tip}/{attempts} on {run_info.head_sha[:9]} and passed on canary {canary_sha[:9]}",
            spec=spec,
            passes_tip=passes_tip,
            fails_tip=fails_tip,
            tip_tail=tip_tail,
        )

    def _bisect_culprit(self, good_sha: str, bad_sha: str, command: str, max_steps: int = 24) -> Optional[str]:
        """Drive `git bisect` step-by-step, running each candidate SHA in a clean sandboxed export."""
        count_proc = self._git(["rev-list", "--count", f"{good_sha}..{bad_sha}"], check=False)
        if count_proc.returncode == 0 and count_proc.stdout.strip() == "1":
            return validate_sha(bad_sha)
        wt = self.worktrees_dir / "bisect"
        self._remove_worktree(wt)
        try:
            self._git(["worktree", "add", "--detach", str(wt), bad_sha])
            step = self._git(["bisect", "start", bad_sha, good_sha], cwd=wt)
            output = (step.stdout or "") + "\n" + (step.stderr or "")
            for _ in range(max_steps):
                m = re.search(r"^([0-9a-f]{40}) is the first bad commit$", output, re.M)
                if m:
                    return validate_sha(m.group(1))
                cur_sha = validate_sha(self._git(["rev-parse", "HEAD"], cwd=wt).stdout)
                ok, tail = self._run_check_in_export(self.repo_dir, cur_sha, command)
                if not ok and "command timed out" in tail:
                    verdict = "skip"
                else:
                    verdict = "good" if ok else "bad"
                step = self._git(["bisect", verdict], cwd=wt, check=False)
                output = (step.stdout or "") + "\n" + (step.stderr or "")
                if step.returncode != 0 and "is the first bad commit" not in output:
                    return None
            return None
        finally:
            self._git(["bisect", "reset"], cwd=wt, check=False)
            self._remove_worktree(wt)

    def _attempt_clean_revert(
        self,
        master_sha: str,
        culprit_sha: str,
        job_name: str,
        command: str,
        run_info: RunInfo,
    ) -> tuple[bool, str, Optional[str], Optional[str]]:
        """Try `git revert` of `culprit_sha` onto `master_sha` and prove `command` passes."""
        short = culprit_sha[:9]
        branch = f"bisect/revert-{short}"
        subject = self._git(["log", "-1", "--format=%s", culprit_sha]).stdout.strip()
        wt = self.worktrees_dir / "revert"
        self._remove_worktree(wt)
        try:
            self._git(["worktree", "add", "-B", branch, str(wt), master_sha])
            rev = self._git(["revert", "--no-edit", culprit_sha], cwd=wt, check=False)
            if rev.returncode != 0:
                self._git(["revert", "--abort"], cwd=wt, check=False)
                return False, "revert_conflict", branch, None

            ok, tail = self._run_check_in_export(wt, "HEAD", command)
            if not ok:
                return False, f"revert_still_fails: {tail[-500:]}", branch, None

            commit_msg = (
                f'Revert "{subject}"\n\n'
                f"This reverts commit {culprit_sha}.\n\n"
                f"Automated revert by the marmot-agents bisect agent after canary.yml failed\n"
                f"in `{job_name}` ({run_info.html_url}). Bisected `canary..master` and verified\n"
                f"the exact failed check fails on {short}, passes on canary, and passes on\n"
                f"the reverted tree.\n\n"
                f"Local-Checks: {command}\n"
                f"Local-Review: exempt (automated bisect revert)\n"
                f"Release-Impact: patch\n"
            )
            amend_cmd = ["commit", "--amend"] + (["-S"] if self.sign_commits else []) + ["-m", commit_msg]
            self._git(amend_cmd, cwd=wt)

            pr_title = f'Revert "{subject}" (canary bisect)'
            pr_body = (
                f"## Automated Revert (Bisect Agent)\n\n"
                f"- **Failing canary run:** {run_info.html_url}\n"
                f"- **Failing job:** `{job_name}`\n"
                f"- **Bisected culprit commit:** `{culprit_sha}` (`{subject}`)\n"
                f"- **Verification command:** `{command}`\n"
                f"- **Result on reverted tree:** PASS\n\n"
                f"This PR was opened automatically by the revert-only bisect agent (#222).\n"
            )
            if self.dry_run:
                return True, "dry_run_clean_revert_verified", branch, None

            # Fetch existing remote revert branch (if any) so --force-with-lease knows its ref.
            self._git(["fetch", "origin", branch], cwd=wt, check=False)
            self._git(["push", "--force-with-lease", "-u", "origin", branch], cwd=wt)
            pr_url, auto_merge_ok = self.gh.open_revert_pr(branch, pr_title, pr_body)
            if not auto_merge_ok:
                return False, f"revert_auto_merge_failed ({pr_url})", branch, pr_url
            return True, "revert_pr_opened", branch, pr_url
        finally:
            self._remove_worktree(wt)

    def _enter_meltdown(
        self,
        reason: str,
        bucket: str,
        run_info: RunInfo,
        canary_sha: str,
        failed_job: Optional[str] = None,
        culprit_sha: Optional[str] = None,
        extra: Optional[dict] = None,
    ) -> tuple[AgentResult, bool]:
        """Attempt Meltdown activation. Returns `(result, activated)`."""
        ts_status = self.update_tree_state("red", reason, run_info.html_url)
        if ts_status == "failed":
            details = {"reason": reason, "tree_state_error": "failed to update origin/tree-state"}
            if extra:
                details.update(extra)
            return (
                AgentResult(
                    action="meltdown_failed",
                    bucket=bucket,
                    dry_run=self.dry_run,
                    run_id=run_info.run_id,
                    head_sha=run_info.head_sha,
                    canary_sha=canary_sha,
                    failed_job=failed_job,
                    culprit_sha=culprit_sha,
                    tree_state=self._read_tree_state(),
                    details=details,
                ),
                False,
            )

        alert_text = (
            f"[MELTDOWN] {self.gh.repo} tree-state set to RED\n"
            f"Reason: {reason}\n"
            f"Bucket: {bucket}\n"
            f"Canary run: {run_info.html_url}\n"
            f"Range: {canary_sha[:9]}..{run_info.head_sha[:9]}\n"
            f"Job: {failed_job or 'unknown'}\n"
            + (f"Culprit: {culprit_sha}\n" if culprit_sha else "")
            + "Recovery: land a PR labeled `tree-fix` or `revert` and verify canary."
        )
        alert_record = self._alert(alert_text, self.dry_run)
        if not self.dry_run and alert_record.get("channel") != "none" and not alert_record.get("sent"):
            print(f"bisect_agent: warning: Meltdown alert command failed: {alert_record}", file=sys.stderr)
        if not self.dry_run:
            self.gh.comment_canary_issue(
                f"### Meltdown Mode Activated (`tree-state` = `red`)\n\n"
                f"- **Reason:** {reason}\n"
                f"- **Bucket:** `{bucket}`\n"
                f"- **Canary run:** {run_info.html_url}\n"
                f"- **Range:** `{canary_sha[:9]}..{run_info.head_sha[:9]}`\n"
                f"- **Job:** `{failed_job or 'unknown'}`\n"
                + (f"- **Culprit:** `{culprit_sha}`\n" if culprit_sha else "")
                + "\n`tree-state` is now `red` (enforced on team PRs once the Phase 3 gate is active). "
                "Label recovery PRs `tree-fix` or `revert`."
            )
        details = {"reason": reason, "tree_state_status": ts_status, "alert": alert_record}
        if extra:
            details.update(extra)
        return (
            AgentResult(
                action="meltdown",
                bucket=bucket,
                dry_run=self.dry_run,
                run_id=run_info.run_id,
                head_sha=run_info.head_sha,
                canary_sha=canary_sha,
                failed_job=failed_job,
                culprit_sha=culprit_sha,
                tree_state="red",
                alert_message=alert_text,
                details=details,
            ),
            True,
        )

    def _select_actionable_run(self, canary_sha: str) -> tuple[Optional[RunInfo], bool]:
        """Find the newest non-skipped completed canary run (walking past skipped successes).

        Falls back to a direct `status=failure` query if the paginated window contains only
        unverified skipped `success` runs so an unresolved failure is never hidden even after
        weeks of hourly retry-cap skips. Returns `(run_info, saw_any_run)`.
        """
        saw_any = False
        for run in self.gh.iter_completed_canary_runs():
            saw_any = True
            if run.conclusion == "success":
                # A skipped canary run (e.g. 2-failure retry cap on a broken SHA) ends `success`
                # without advancing `canary`. Skip past it to find the underlying failure.
                if not self._is_ancestor(run.head_sha, canary_sha):
                    continue
                return run, True
            return run, True
        if saw_any:
            fallback_fail = self.gh.latest_failed_canary_run()
            if fallback_fail is not None and not self._is_ancestor(fallback_fail.head_sha, canary_sha):
                return fallback_fail, True
        return None, saw_any

    def run_once(self) -> AgentResult:
        lock_path = self.state_dir / "bisect.lock"
        with open(lock_path, "w", encoding="utf-8") as lock_fh:
            try:
                fcntl.flock(lock_fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return AgentResult(action="skipped_locked", dry_run=self.dry_run)
            return self._run_once_locked()

    def _monitor_pending_revert(
        self,
        state: dict,
        pending: dict,
        run_info: RunInfo,
        canary_sha: str,
        tree_state: str,
        consecutive: int,
    ) -> Optional[AgentResult]:
        """Check an open automated revert PR on every tick until it merges or becomes blocked."""
        pr_url = pending["url"]
        culprit = pending["culprit_sha"]
        job_name = pending.get("job_name")
        pr = self.gh.inspect_pr(pr_url)
        if pr is None:
            return None
        pr_state = pr.get("state", "OPEN")
        if pr_state == "MERGED":
            # Recovery PR landed; persist cleared pending marker and let normal handling proceed.
            self._record_handled(
                state,
                state.get("last_handled_run_id") or run_info.run_id,
                int(state.get("consecutive_failures", 0)),
                state.get("last_failed_sha"),
                pending_revert_pr=None,
            )
            return None
        if pr_state != "OPEN":
            reason = f"automated revert PR {pr_url} was closed without merging (state={pr_state})"
            status = "revert_pr_closed"
        elif pr.get("mergeable") == "CONFLICTING":
            reason = f"automated revert PR {pr_url} for culprit {culprit[:9]} has merge conflicts"
            status = "revert_pr_conflicting"
        elif pr.get("checks") == "failed":
            reason = f"automated revert PR {pr_url} for culprit {culprit[:9]} failed CI checks"
            status = "revert_pr_checks_failed"
        elif not pr.get("auto_merge") and not self.dry_run and not self.gh.enable_auto_merge(pr_url):
            reason = f"could not enable auto-merge on automated revert PR {pr_url} for culprit {culprit[:9]}"
            status = "revert_auto_merge_failed"
        else:
            return AgentResult(
                action="noop_revert_pending",
                bucket="deterministic_regression",
                dry_run=self.dry_run,
                run_id=run_info.run_id,
                head_sha=run_info.head_sha,
                canary_sha=canary_sha,
                failed_job=job_name,
                culprit_sha=culprit,
                revert_pr_url=pr_url,
                tree_state=tree_state,
                details={"checks": pr.get("checks"), "mergeable": pr.get("mergeable")},
            )

        res, activated = self._enter_meltdown(
            reason=reason,
            bucket="deterministic_regression",
            run_info=run_info,
            canary_sha=canary_sha,
            failed_job=job_name,
            culprit_sha=culprit,
            extra={"revert_pr_url": pr_url, "revert_status": status},
        )
        if activated:
            self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha, pending_revert_pr=None)
        return res

    def _run_once_locked(self) -> AgentResult:
        state = self._load_state()
        master_sha, canary_sha, tree_state = self._fetch_refs()
        run_info, saw_any = self._select_actionable_run(canary_sha)
        if run_info is None:
            return AgentResult(
                action="noop_skipped_canary_run" if saw_any else "noop_no_runs",
                dry_run=self.dry_run,
                head_sha=master_sha,
                canary_sha=canary_sha,
                tree_state=tree_state,
            )

        if run_info.conclusion == "success":
            recovered = False
            if tree_state == "red" and self._is_ancestor(master_sha, canary_sha):
                ts_status = self.update_tree_state(
                    "green",
                    f"canary verified {run_info.head_sha[:9]}",
                    run_info.html_url,
                )
                if ts_status == "failed":
                    return AgentResult(
                        action="recovery_failed",
                        dry_run=self.dry_run,
                        run_id=run_info.run_id,
                        head_sha=run_info.head_sha,
                        canary_sha=canary_sha,
                        tree_state=tree_state,
                    )
                recovered = ts_status == "updated"
            self._record_handled(state, run_info.run_id, 0, None, pending_revert_pr=None)
            return AgentResult(
                action="recovered_green" if recovered else "noop_green",
                dry_run=self.dry_run,
                run_id=run_info.run_id,
                head_sha=run_info.head_sha,
                canary_sha=canary_sha,
                tree_state="green" if recovered else tree_state,
            )

        # Monitor any pending automated revert PR BEFORE the handled-run early return so a PR
        # whose CI fails or becomes conflicting escalates to Meltdown even on the same canary run.
        pending_pr = state.get("pending_revert_pr")
        if isinstance(pending_pr, dict) and pending_pr.get("url"):
            monitored = self._monitor_pending_revert(
                state,
                pending_pr,
                run_info,
                canary_sha,
                tree_state,
                max(1, int(state.get("consecutive_failures", 1))),
            )
            if monitored is not None:
                return monitored

        if state.get("last_handled_run_id") == run_info.run_id:
            return AgentResult(
                action="noop_already_handled",
                dry_run=self.dry_run,
                run_id=run_info.run_id,
                head_sha=run_info.head_sha,
                canary_sha=canary_sha,
                tree_state=tree_state,
            )

        if self._is_ancestor(run_info.head_sha, canary_sha):
            self._record_handled(state, run_info.run_id, 0, None)
            return AgentResult(
                action="noop_superseded_by_canary",
                dry_run=self.dry_run,
                run_id=run_info.run_id,
                head_sha=run_info.head_sha,
                canary_sha=canary_sha,
                tree_state=tree_state,
            )

        # Reset per-SHA failure count when a new SHA fails (unless tree is already red).
        prev_count = int(state.get("consecutive_failures", 0))
        if state.get("last_failed_sha") != run_info.head_sha and tree_state != "red":
            prev_count = 0
        consecutive = prev_count + 1

        failed_jobs = self.gh.failed_jobs_for_run(run_info.run_id)
        if not failed_jobs:
            reason = f"canary run {run_info.run_id} failed with no identifiable inner failed job"
            if consecutive < 2:
                if not self.dry_run:
                    self.gh.dispatch_canary(force=True)
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha)
                return AgentResult(
                    action="retry_canary",
                    bucket="infra",
                    dry_run=self.dry_run,
                    run_id=run_info.run_id,
                    head_sha=run_info.head_sha,
                    canary_sha=canary_sha,
                    tree_state=tree_state,
                    details={"reason": reason},
                )
            res, activated = self._enter_meltdown(reason=reason, bucket="infra", run_info=run_info, canary_sha=canary_sha)
            if activated:
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha)
            return res

        # Classify failed jobs, stopping early as soon as a deterministic regression is confirmed
        # so a compile error breaking multiple Rust jobs does not run redundant builds.
        flaky_names = self.gh.flaky_job_names()
        classifications: list[JobClassification] = []
        # Check cheap/infra/non-flaky jobs first so a deterministic regression surfaces quickly.
        ordered_jobs = sorted(
            failed_jobs,
            key=lambda j: (
                (j.name in flaky_names or normalize_job_name(j.name) in flaky_names),
                normalize_job_name(j.name) not in self.job_specs,
            ),
        )
        for job in ordered_jobs:
            c = self._classify_job(job, run_info, canary_sha, flaky_names)
            classifications.append(c)
            if c.bucket == "deterministic_regression":
                break

        priority = {
            "deterministic_regression": 0,
            "baseline_broken": 1,
            "suspected_race": 2,
            "unmapped": 3,
            "known_flake": 4,
            "infra": 5,
        }
        chosen = min(classifications, key=lambda c: priority.get(c.bucket, 99))

        if chosen.bucket in {"infra", "known_flake", "suspected_race", "unmapped"}:
            if consecutive < 2:
                if not self.dry_run:
                    self.gh.dispatch_canary(force=True)
                    if chosen.bucket == "suspected_race":
                        self.gh.comment_canary_issue(
                            f"Bisect agent classified `{chosen.job.name}` on `{run_info.head_sha[:9]}` "
                            f"as **suspected new race** ({chosen.reason}). Dispatched one canary retry."
                        )
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha)
                return AgentResult(
                    action="retry_canary",
                    bucket=chosen.bucket,
                    dry_run=self.dry_run,
                    run_id=run_info.run_id,
                    head_sha=run_info.head_sha,
                    canary_sha=canary_sha,
                    failed_job=chosen.job.name,
                    tree_state=tree_state,
                    details={"reason": chosen.reason},
                )
            res, activated = self._enter_meltdown(
                reason=f"{chosen.bucket} in `{chosen.job.name}` failed canary {consecutive} consecutive times ({chosen.reason})",
                bucket=chosen.bucket,
                run_info=run_info,
                canary_sha=canary_sha,
                failed_job=chosen.job.name,
            )
            if activated:
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha)
            return res

        if chosen.bucket == "baseline_broken":
            res, activated = self._enter_meltdown(
                reason=f"{chosen.reason} (environmental or upstream breakage)",
                bucket="baseline_broken",
                run_info=run_info,
                canary_sha=canary_sha,
                failed_job=chosen.job.name,
            )
            if activated:
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha)
            return res

        # Bucket 4: Deterministic regression over canary..run_info.head_sha.
        assert chosen.spec is not None
        if master_sha != run_info.head_sha:
            passes_master, fails_master, _ = self._reproduce_on_sha(master_sha, chosen.spec.command, 1)
            if passes_master > 0 and fails_master == 0:
                self._record_handled(state, run_info.run_id, 0, None)
                return AgentResult(
                    action="noop_already_fixed_on_master",
                    bucket="deterministic_regression",
                    dry_run=self.dry_run,
                    run_id=run_info.run_id,
                    head_sha=master_sha,
                    canary_sha=canary_sha,
                    failed_job=chosen.job.name,
                    tree_state=tree_state,
                    details={"failed_run_sha": run_info.head_sha},
                )

        culprit = self._bisect_culprit(canary_sha, run_info.head_sha, chosen.spec.command)
        if not culprit:
            res, activated = self._enter_meltdown(
                reason=f"git bisect over {canary_sha[:9]}..{run_info.head_sha[:9]} could not isolate a single culprit for `{chosen.job.name}`",
                bucket="deterministic_regression",
                run_info=run_info,
                canary_sha=canary_sha,
                failed_job=chosen.job.name,
            )
            if activated:
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha)
            return res

        existing_pr = self.gh.open_revert_pr_for_sha(culprit)
        if existing_pr:
            pr_url = existing_pr["url"]
            if existing_pr.get("mergeable") == "CONFLICTING":
                reason = f"existing revert PR {pr_url} for culprit {culprit[:9]} has merge conflicts"
                status = "revert_pr_conflicting"
            elif existing_pr.get("checks") == "failed":
                reason = f"existing revert PR {pr_url} for culprit {culprit[:9]} failed CI checks"
                status = "revert_pr_checks_failed"
            elif not existing_pr.get("auto_merge") and not self.dry_run and not self.gh.enable_auto_merge(pr_url):
                reason = f"could not enable auto-merge on existing revert PR {pr_url} for culprit {culprit[:9]}"
                status = "revert_auto_merge_failed"
            else:
                pending_meta = {"url": pr_url, "culprit_sha": culprit, "run_id": run_info.run_id, "job_name": chosen.job.name}
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha, pending_revert_pr=pending_meta)
                return AgentResult(
                    action="noop_revert_already_open",
                    bucket="deterministic_regression",
                    dry_run=self.dry_run,
                    run_id=run_info.run_id,
                    head_sha=run_info.head_sha,
                    canary_sha=canary_sha,
                    failed_job=chosen.job.name,
                    culprit_sha=culprit,
                    revert_pr_url=pr_url,
                    tree_state=tree_state,
                )
            res, activated = self._enter_meltdown(
                reason=reason,
                bucket="deterministic_regression",
                run_info=run_info,
                canary_sha=canary_sha,
                failed_job=chosen.job.name,
                culprit_sha=culprit,
                extra={"revert_pr_url": pr_url, "revert_status": status},
            )
            if activated:
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha, pending_revert_pr=None)
            return res

        reverted_ok, rev_status, branch, pr_url = self._attempt_clean_revert(
            master_sha=master_sha,
            culprit_sha=culprit,
            job_name=chosen.job.name,
            command=chosen.spec.command,
            run_info=run_info,
        )
        if not reverted_ok:
            res, activated = self._enter_meltdown(
                reason=f"could not cleanly auto-revert culprit {culprit[:9]} for `{chosen.job.name}` ({rev_status})",
                bucket="deterministic_regression",
                run_info=run_info,
                canary_sha=canary_sha,
                failed_job=chosen.job.name,
                culprit_sha=culprit,
                extra={"revert_status": rev_status},
            )
            if activated:
                self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha, pending_revert_pr=None)
            return res

        pending_meta = (
            {"url": pr_url, "culprit_sha": culprit, "run_id": run_info.run_id, "job_name": chosen.job.name}
            if pr_url
            else None
        )
        self._record_handled(state, run_info.run_id, consecutive, run_info.head_sha, pending_revert_pr=pending_meta)
        return AgentResult(
            action="revert_pr_opened" if not self.dry_run else "dry_run_revert_verified",
            bucket="deterministic_regression",
            dry_run=self.dry_run,
            run_id=run_info.run_id,
            head_sha=run_info.head_sha,
            canary_sha=canary_sha,
            failed_job=chosen.job.name,
            culprit_sha=culprit,
            revert_branch=branch,
            revert_pr_url=pr_url,
            tree_state=tree_state,
            details={"revert_status": rev_status, "tip_tail": chosen.tip_tail[-500:]},
        )


def parse_args(argv: list[str]) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Revert-only Bisect Agent for marmot-agents (#222).")
    p.add_argument("--repo", default=DEFAULT_REPO, help="GitHub repository (owner/name)")
    p.add_argument("--repo-dir", type=Path, required=True, help="Local git checkout of marmot-agents")
    p.add_argument("--worktrees-dir", type=Path, required=True, help="Directory for isolated worktrees")
    p.add_argument("--state-dir", type=Path, required=True, help="Directory for bisect state and lock")
    p.add_argument(
        "--mode",
        choices=("dry-run", "live"),
        default="dry-run",
        help="dry-run performs zero remote mutations; live opens revert PRs and updates tree-state",
    )
    p.add_argument("--repro-attempts", type=int, default=2, help="Local reproduction attempts on tip")
    p.add_argument("--step-timeout", type=int, default=1200, help="Per-command timeout in seconds")
    p.add_argument("--no-sign", action="store_true", help="Disable git commit -S (for local testing only)")
    p.add_argument(
        "--allow-unsandboxed",
        action="store_true",
        help="Allow running checks without bwrap mount-namespace isolation (testing only)",
    )
    return p.parse_args(argv)


def main(argv: Optional[list[str]] = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    gh = GitHubClient(args.repo)
    agent = BisectAgent(
        repo_dir=args.repo_dir,
        worktrees_dir=args.worktrees_dir,
        state_dir=args.state_dir,
        gh=gh,
        dry_run=(args.mode != "live"),
        repro_attempts=args.repro_attempts,
        step_timeout_sec=args.step_timeout,
        sign_commits=(not args.no_sign),
        require_sandbox=(not args.allow_unsandboxed),
    )
    result = agent.run_once()
    print(json.dumps(dataclasses.asdict(result), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
