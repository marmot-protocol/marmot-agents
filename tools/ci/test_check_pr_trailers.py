#!/usr/bin/env python3
"""Unit tests for tools/ci/check_pr_trailers.py."""

from __future__ import annotations

import io
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_pr_trailers as trailers  # noqa: E402


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


class PrTrailerTests(unittest.TestCase):
    def make_repo(self) -> tuple[tempfile.TemporaryDirectory[str], Path, str]:
        tmp = tempfile.TemporaryDirectory()
        repo = Path(tmp.name)
        git(repo, "init", "-b", "master")
        git(repo, "config", "user.name", "Test Author")
        git(repo, "config", "user.email", "test@example.com")
        git(repo, "config", "commit.gpgsign", "false")
        (repo / "README.md").write_text("# marmot-agents\n", encoding="utf-8")
        git(repo, "add", "README.md")
        git(repo, "commit", "-m", "Initial commit")
        base = git(repo, "rev-parse", "HEAD")
        return tmp, repo, base

    def commit_file(self, repo: Path, rel_path: str, content: str, message: str) -> str:
        path = repo / rel_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        git(repo, "add", rel_path)
        git(repo, "commit", "-m", message)
        return git(repo, "rev-parse", "HEAD")

    def test_valid_cross_model_human_exempt_folded_changelog_and_coauthored_trailers(self) -> None:
        cases = [
            (
                "Add feature\n\n"
                "Local-Checks: cargo fmt --all --check, cargo clippy, cargo test\n"
                "Local-Review: claude + codex merge @ 37a9067\n"
                "Release-Impact: patch\n"
                "Changelog: Fix reconnect race on idle socket\n",
                "patch",
            ),
            (
                "Doc tweak\n\n"
                "Local-Checks: lychee .\n"
                "Local-Review: human (@reviewer)\n"
                "Release-Impact: none\n",
                "none",
            ),
            (
                'Revert "bad commit"\n\n'
                "This reverts commit abcdef1.\n\n"
                "Local-Checks: bisect verified on mdk-ci-1\n"
                "Local-Review: exempt (revert)\n"
                "Release-Impact: Patch\n",
                "patch",
            ),
            (
                "Folded trailer value, mixed case seats, and multiple Co-authored-by lines\n\n"
                "Local-Checks: cargo fmt --all --check,\n"
                "  cargo clippy --locked --workspace --all-targets --all-features -- -D warnings,\n"
                "  cargo test --locked --workspace --all-features\n"
                "Local-Review: Antigravity + Codex merge @ 0123456789abcdef\n"
                "Release-Impact: MINOR\n"
                "Co-authored-by: Alice <alice@example.com>\n"
                "Co-authored-by: Bob <bob@example.com>\n",
                "minor",
            ),
            (
                "Markdown horizontal rule before valid trailer block\n\n"
                "Section 1\n\n"
                "---\n\n"
                "Section 2\n\n"
                "Local-Checks: cargo fmt --all --check\n"
                "Local-Review: human\n"
                "Release-Impact: none\n",
                "none",
            ),
        ]
        for message, expected_impact in cases:
            with self.subTest(first_line=message.splitlines()[0]):
                tmp, repo, base = self.make_repo()
                try:
                    head = self.commit_file(repo, "note.txt", expected_impact, message)
                    impact, errors = trailers.validate_range(repo, base, head)
                    self.assertEqual(errors, [])
                    self.assertEqual(impact, expected_impact)
                finally:
                    tmp.cleanup()

    def test_rejects_missing_placeholder_duplicate_and_same_seat_trailers(self) -> None:
        bad_messages = [
            ("No trailers at all", ["Local-Checks", "Local-Review", "Release-Impact"]),
            (
                "Placeholder values\n\n"
                "Local-Checks: todo\n"
                "Local-Review: tbd\n"
                "Release-Impact: major\n"
                "Changelog: n/a\n",
                [
                    "invalid 'Local-Checks:'",
                    "invalid 'Local-Review:'",
                    "invalid 'Release-Impact:'",
                    "invalid 'Changelog:'",
                ],
            ),
            (
                "Same seat twice (case-insensitive)\n\n"
                "Local-Checks: cargo fmt\n"
                "Local-Review: Claude + claude merge @ 37a9067\n"
                "Release-Impact: none\n",
                ["two distinct seats"],
            ),
            (
                "Placeholder seat name\n\n"
                "Local-Checks: cargo fmt\n"
                "Local-Review: tbd + codex merge @ 37a9067\n"
                "Release-Impact: none\n",
                ["placeholder seat name"],
            ),
            (
                "Duplicate trailer\n\n"
                "Local-Checks: cargo fmt\n"
                "Local-Review: human\n"
                "Release-Impact: patch\n"
                "Release-Impact: minor\n",
                ["duplicate trailer"],
            ),
            (
                "Trailer-shaped example before --- divider followed by trailing prose\n\n"
                "Local-Checks: cargo fmt\n"
                "Local-Review: human\n"
                "Release-Impact: none\n\n"
                "---\n\n"
                "Trailing prose after a Markdown rule.\n",
                ["Local-Checks", "Local-Review", "Release-Impact"],
            ),
        ]
        for message, expected_substrings in bad_messages:
            with self.subTest(first_line=message.splitlines()[0]):
                tmp, repo, base = self.make_repo()
                try:
                    head = self.commit_file(repo, "note.txt", "x", message)
                    _, errors = trailers.validate_range(repo, base, head)
                    joined = "\n".join(errors)
                    for sub in expected_substrings:
                        self.assertIn(sub, joined)
                finally:
                    tmp.cleanup()

    def test_synthetic_pr_merge_commit_with_head_parents(self) -> None:
        """Matches GitHub Actions pull_request checkout (`fetch-depth: 2`, HEAD^1=base, HEAD^2=pr_tip)."""
        tmp, repo, _ = self.make_repo()
        try:
            git(repo, "checkout", "-b", "feature")
            self.commit_file(repo, "a.txt", "1", "WIP commit without trailers")
            self.commit_file(
                repo,
                "b.txt",
                "2",
                "Final PR commit\n\n"
                "Local-Checks: python3 -m unittest discover -s tools/ci -p 'test_*.py'\n"
                "Local-Review: claude + codex merge @ 1234567\n"
                "Release-Impact: none\n",
            )
            git(repo, "checkout", "master")
            git(repo, "merge", "--no-ff", "feature", "-m", "Merge feature into master")
            impact, errors = trailers.validate_range(repo, "HEAD^1", "HEAD^2")
            self.assertEqual(errors, [])
            self.assertEqual(impact, "none")
        finally:
            tmp.cleanup()

    def test_cli_strict_vs_advisory_mode(self) -> None:
        tmp, repo, base = self.make_repo()
        try:
            head = self.commit_file(repo, "a.txt", "1", "Missing trailers")
            stderr = io.StringIO()
            with redirect_stderr(stderr):
                code = trailers.main(["--root", str(repo), "--base", base, "--head", head])
            self.assertEqual(code, 1)
            self.assertIn("missing required trailer", stderr.getvalue())

            stdout = io.StringIO()
            with redirect_stdout(stdout):
                code = trailers.main(
                    ["--root", str(repo), "--base", base, "--head", head, "--advisory"]
                )
            self.assertEqual(code, 0)
            self.assertIn("::warning title=PR trailers::", stdout.getvalue())
        finally:
            tmp.cleanup()


if __name__ == "__main__":
    unittest.main()
