#!/usr/bin/env python3
"""Unit tests for tools/release/plan_release.py (Phase 4.2, #222)."""

from __future__ import annotations

import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

RELEASE_DIR = Path(__file__).resolve().parent
CI_DIR = RELEASE_DIR.parent / "ci"
for p in (str(RELEASE_DIR), str(CI_DIR)):
    if p not in sys.path:
        sys.path.insert(0, p)

import plan_release as planner  # noqa: E402
from test_check_socket_compat import make_minimal_repo  # noqa: E402


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def commit_file(repo: Path, rel: str, content: str, message: str) -> str:
    path = repo / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    git(repo, "add", rel)
    git(repo, "commit", "-m", message)
    return git(repo, "rev-parse", "HEAD")


def make_git_repo(version: str = "0.1.0") -> tuple[tempfile.TemporaryDirectory[str], Path]:
    tmp = tempfile.TemporaryDirectory()
    repo = Path(tmp.name)
    make_minimal_repo(repo, version=version)
    git(repo, "init", "-b", "master")
    git(repo, "config", "user.name", "Test Author")
    git(repo, "config", "user.email", "test@example.com")
    git(repo, "config", "commit.gpgsign", "false")
    git(repo, "add", ".")
    git(
        repo,
        "commit",
        "-m",
        "Initial tree\n\nLocal-Checks: test\nLocal-Review: human\nRelease-Impact: none\n",
    )
    return tmp, repo


class PlanReleaseTests(unittest.TestCase):
    def test_initial_release_when_no_tags_exist(self) -> None:
        tmp, repo = make_git_repo("0.1.0")
        try:
            plan, errors = planner.plan_releases(repo, ref="HEAD", channel="stable")
            self.assertEqual(errors, [])
            self.assertEqual(set(plan["tracks"]), {"marmot-agent", "claude-code", "openclaw", "hermes"})
            for name, prefix in (
                ("marmot-agent", "v"),
                ("claude-code", "claude-code-v"),
                ("openclaw", "openclaw-v"),
                ("hermes", "hermes-v"),
            ):
                info = plan["tracks"][name]
                self.assertTrue(info["needs_release"], name)
                self.assertIsNone(info["last_tag"])
                self.assertEqual(info["next_version"], "0.1.0")
                self.assertEqual(info["next_tag"], f"{prefix}0.1.0")

            rc_plan, rc_errors = planner.plan_releases(repo, ref="HEAD", channel="rc")
            self.assertEqual(rc_errors, [])
            self.assertEqual(rc_plan["tracks"]["openclaw"]["next_tag"], "openclaw-v0.1.0-rc.1")
        finally:
            tmp.cleanup()

    def test_path_isolation_only_bumps_affected_track(self) -> None:
        tmp, repo = make_git_repo("0.1.0")
        try:
            for tag in ("v0.1.0", "claude-code-v0.1.0", "openclaw-v0.1.0", "hermes-v0.1.0"):
                git(repo, "tag", tag)

            # Docs-only commit with Release-Impact: none affects no track.
            commit_file(
                repo,
                "docs/notes.md",
                "hello\n",
                "Docs tweak\n\nLocal-Checks: lychee\nLocal-Review: human\nRelease-Impact: none\n",
            )
            # OpenClaw fix with Release-Impact: patch only bumps openclaw.
            commit_file(
                repo,
                "packages/openclaw/src/fix.ts",
                "export const x = 1;\n",
                "Fix OpenClaw reconnect\n\n"
                "Local-Checks: npm test\n"
                "Local-Review: claude + codex merge @ 1234567\n"
                "Release-Impact: patch\n"
                "Changelog: Fix reconnect backoff in OpenClaw plugin\n\n"
                "---------\n\n"
                "Co-authored-by: Bob <bob@example.com>\n",
            )
            # Hermes test-only / internal commit marked Release-Impact: none bumps nothing.
            commit_file(
                repo,
                "python/marmot_hermes/note.txt",
                "comment\n",
                "Hermes comment\n\nLocal-Checks: unittest\nLocal-Review: human\nRelease-Impact: none\n",
            )

            plan, errors = planner.plan_releases(repo, ref="HEAD")
            self.assertEqual(errors, [])
            self.assertFalse(plan["tracks"]["marmot-agent"]["needs_release"])
            self.assertFalse(plan["tracks"]["claude-code"]["needs_release"])
            self.assertFalse(plan["tracks"]["hermes"]["needs_release"])

            oc = plan["tracks"]["openclaw"]
            self.assertTrue(oc["needs_release"])
            self.assertEqual(oc["last_tag"], "openclaw-v0.1.0")
            self.assertEqual(oc["effective_impact"], "patch")
            self.assertEqual(oc["next_version"], "0.1.1")
            self.assertEqual(oc["next_tag"], "openclaw-v0.1.1")
            self.assertTrue(any("Fix reconnect backoff in OpenClaw plugin" in line for line in oc["changelog"]))
        finally:
            tmp.cleanup()

    def test_minor_beats_patch_and_rc_increments_across_existing_rc_tags(self) -> None:
        tmp, repo = make_git_repo("0.2.0")
        try:
            git(repo, "tag", "claude-code-v0.2.0")
            git(repo, "tag", "claude-code-v0.3.0-rc.1")
            git(repo, "tag", "claude-code-v0.3.0-rc.2")

            commit_file(
                repo,
                "packages/claude-code/src/a.ts",
                "export const a = 1;\n",
                "Patch fix\n\nLocal-Checks: npm test\nLocal-Review: human\nRelease-Impact: patch\n",
            )
            commit_file(
                repo,
                "packages/claude-code/src/b.ts",
                "export const b = 2;\n",
                "New capability\n\nLocal-Checks: npm test\nLocal-Review: human\nRelease-Impact: minor\n",
            )

            stable, errors = planner.plan_releases(repo, ref="HEAD", track="claude-code", channel="stable")
            self.assertEqual(errors, [])
            self.assertEqual(stable["tracks"]["claude-code"]["next_tag"], "claude-code-v0.3.0")

            rc, rc_errors = planner.plan_releases(repo, ref="HEAD", track="claude-code", channel="rc")
            self.assertEqual(rc_errors, [])
            self.assertEqual(rc["tracks"]["claude-code"]["next_tag"], "claude-code-v0.3.0-rc.3")
        finally:
            tmp.cleanup()

    def test_missing_or_duplicate_trailer_defaults_conservatively_to_patch(self) -> None:
        tmp, repo = make_git_repo("0.1.0")
        try:
            git(repo, "tag", "hermes-v0.1.0")
            commit_file(repo, "python/marmot_hermes/extra.py", "X = 1\n", "Legacy commit without trailers")
            plan, errors = planner.plan_releases(repo, ref="HEAD", track="hermes")
            self.assertEqual(errors, [])
            info = plan["tracks"]["hermes"]
            self.assertTrue(info["needs_release"])
            self.assertEqual(info["next_version"], "0.1.1")
            self.assertTrue(info["commits"][0]["missing_trailer"])

            # Conflicting duplicate Release-Impact trailers (none + minor) must not suppress release.
            git(repo, "tag", "openclaw-v0.1.0")
            commit_file(
                repo,
                "packages/openclaw/src/dup.ts",
                "export const dup = true;\n",
                "Conflicting impact trailers\n\n"
                "Local-Checks: npm test\n"
                "Local-Review: human\n"
                "Release-Impact: none\n"
                "Release-Impact: minor\n",
            )
            oc_plan, oc_errors = planner.plan_releases(repo, ref="HEAD", track="openclaw")
            self.assertEqual(oc_errors, [])
            oc = oc_plan["tracks"]["openclaw"]
            self.assertTrue(oc["needs_release"])
            self.assertEqual(oc["next_version"], "0.1.1")
            self.assertTrue(oc["commits"][0]["missing_trailer"])
        finally:
            tmp.cleanup()

    def test_manifest_version_with_semver_build_metadata_is_accepted(self) -> None:
        tmp, repo = make_git_repo("0.1.0+build.7")
        try:
            plan, errors = planner.plan_releases(repo, ref="HEAD")
            self.assertEqual(errors, [])
            self.assertEqual(plan["tracks"]["marmot-agent"]["next_version"], "0.1.0")
            self.assertEqual(plan["tracks"]["openclaw"]["next_tag"], "openclaw-v0.1.0")
        finally:
            tmp.cleanup()

    def test_additive_socket_schema_sets_minor_floor_on_core_only(self) -> None:
        tmp, repo = make_git_repo("0.1.0")
        try:
            for tag in ("v0.1.0", "claude-code-v0.1.0", "openclaw-v0.1.0", "hermes-v0.1.0"):
                git(repo, "tag", tag)

            schema_path = repo / "fixtures/socket/schema.json"
            schema = json.loads(schema_path.read_text(encoding="utf-8"))
            schema["$defs"]["Prompt"]["properties"]["priority"] = {"type": "string"}
            commit_file(
                repo,
                "fixtures/socket/schema.json",
                json.dumps(schema, indent=2),
                "Add optional priority field\n\nLocal-Checks: cargo test\nLocal-Review: human\nRelease-Impact: patch\n",
            )

            plan, errors = planner.plan_releases(repo, ref="HEAD")
            self.assertEqual(errors, [])
            core = plan["tracks"]["marmot-agent"]
            self.assertEqual(core["trailer_impact"], "patch")
            self.assertEqual(core["mechanical_floor"], "minor")
            self.assertEqual(core["effective_impact"], "minor")
            self.assertEqual(core["next_tag"], "v0.2.0")
            self.assertFalse(plan["tracks"]["openclaw"]["needs_release"])
        finally:
            tmp.cleanup()

    def test_protocol_bump_sets_minor_floor_on_all_tracks_and_breaking_fails(self) -> None:
        tmp, repo = make_git_repo("0.1.0")
        try:
            for tag in ("v0.1.0", "claude-code-v0.1.0", "openclaw-v0.1.0", "hermes-v0.1.0"):
                git(repo, "tag", tag)

            # Breaking schema change without protocol bump fails closed.
            schema_path = repo / "fixtures/socket/schema.json"
            bad = json.loads(schema_path.read_text(encoding="utf-8"))
            del bad["$defs"]["Prompt"]["properties"]["text"]
            commit_file(
                repo,
                "fixtures/socket/schema.json",
                json.dumps(bad, indent=2),
                "Accidental breaking schema\n\nLocal-Checks: test\nLocal-Review: human\nRelease-Impact: patch\n",
            )
            _, bad_errors = planner.plan_releases(repo, ref="HEAD", track="marmot-agent")
            self.assertTrue(any("breaking change" in e for e in bad_errors), bad_errors)

            # Protocol bump to v4 applies minor floor to every track.
            good_v4 = json.loads(schema_path.read_text(encoding="utf-8"))
            good_v4["title"] = "marmot.agent-control.v4"
            good_v4["$defs"]["HelloParams"]["properties"]["protocol"]["const"] = "marmot.agent-control.v4"
            commit_file(
                repo,
                "fixtures/socket/schema.json",
                json.dumps(good_v4, indent=2),
                "Bump socket protocol to v4\n\nLocal-Checks: test\nLocal-Review: human\nRelease-Impact: none\n",
            )
            plan, errors = planner.plan_releases(repo, ref="HEAD")
            self.assertEqual(errors, [])
            for name in ("marmot-agent", "claude-code", "openclaw", "hermes"):
                self.assertEqual(plan["tracks"][name]["mechanical_floor"], "minor", name)
                self.assertEqual(plan["tracks"][name]["next_version"], "0.2.0", name)
        finally:
            tmp.cleanup()

    def test_cli_json_and_markdown_output_on_live_repo(self) -> None:
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = planner.main(["--root", str(planner.ROOT), "--json"])
        self.assertEqual(code, 0)
        parsed = json.loads(buf.getvalue())
        self.assertTrue(parsed["ok"])
        expected_protocol = json.loads(
            (planner.ROOT / "fixtures/socket/schema.json").read_text(encoding="utf-8")
        )["title"]
        self.assertEqual(parsed["socket_protocol"], expected_protocol)
        self.assertEqual(set(parsed["tracks"]), {"marmot-agent", "claude-code", "openclaw", "hermes"})

        md_buf = io.StringIO()
        with redirect_stdout(md_buf):
            code = planner.main(["--root", str(planner.ROOT)])
        self.assertEqual(code, 0)
        self.assertIn("# Release plan", md_buf.getvalue())


if __name__ == "__main__":
    unittest.main()
