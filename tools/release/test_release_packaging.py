#!/usr/bin/env python3
"""Unit tests for tools/release packaging, manifest, and release resolution helpers (Phase 4.3, #222)."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

RELEASE_DIR = Path(__file__).resolve().parent
REPO_ROOT = RELEASE_DIR.parents[1]
CI_DIR = REPO_ROOT / "tools" / "ci"
for p in (str(RELEASE_DIR), str(CI_DIR)):
    if p not in sys.path:
        sys.path.insert(0, p)

import resolve_release as rr  # noqa: E402
import write_manifest as wm  # noqa: E402
from test_plan_release import commit_file, git, make_git_repo  # noqa: E402


class ConnectorPackagingTests(unittest.TestCase):
    def test_package_connector_builds_expected_archives_for_each_track(self) -> None:
        expected_members = {
            "claude-code": (
                "marmot-agent-claude-code/LICENSE",
                "marmot-agent-claude-code/packages/claude-code/package.json",
                "marmot-agent-claude-code/packages/claude-code/package-lock.json",
                "marmot-agent-claude-code/packages/claude-code/src/main.ts",
            ),
            "openclaw": (
                "marmot-agent-openclaw/LICENSE",
                "marmot-agent-openclaw/packages/openclaw/package.json",
                "marmot-agent-openclaw/packages/openclaw/openclaw.plugin.json",
                "marmot-agent-openclaw/packages/openclaw/src/index.ts",
            ),
            "hermes": (
                "marmot-agent-hermes/LICENSE",
                "marmot-agent-hermes/python/pyproject.toml",
                "marmot-agent-hermes/python/marmot_hermes/plugin.yaml",
                "marmot-agent-hermes/python/marmot_hermes/__init__.py",
            ),
        }
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            for track, required in expected_members.items():
                with self.subTest(track=track):
                    proc = subprocess.run(
                        ["bash", str(RELEASE_DIR / "package_connector.sh"), track, str(out)],
                        cwd=REPO_ROOT,
                        capture_output=True,
                        text=True,
                        check=True,
                    )
                    archive = out / f"marmot-agent-{track}.tar.gz"
                    self.assertEqual(proc.stdout.strip(), str(archive))
                    self.assertTrue(archive.is_file())
                    with tarfile.open(archive, "r:gz") as tf:
                        names = set(tf.getnames())
                    for member in required:
                        self.assertIn(member, names)
                    self.assertFalse(any("node_modules" in n or "__pycache__" in n for n in names))

    def test_package_connector_rejects_unknown_track(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            proc = subprocess.run(
                ["bash", str(RELEASE_DIR / "package_connector.sh"), "unknown", tmp],
                cwd=REPO_ROOT,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("unknown connector track", proc.stderr)


class WriteManifestTests(unittest.TestCase):
    def test_writes_sha256sums_and_latest_json_for_stable_and_rc(self) -> None:
        cases = (
            ("openclaw", "0.1.0-rc.1", "openclaw-v0.1.0-rc.1", True),
            ("marmot-agent", "0.1.0", "v0.1.0", False),
        )
        for track, version, tag, expected_pre in cases:
            with self.subTest(tag=tag), tempfile.TemporaryDirectory() as tmp:
                dist = Path(tmp)
                payload = f"tarball bytes for {tag}\n".encode()
                asset_name = f"marmot-agent-{track}.tar.gz"
                (dist / asset_name).write_bytes(payload)
                expected_sha = hashlib.sha256(payload).hexdigest()

                code = wm.main(
                    [
                        "--dir",
                        str(dist),
                        "--track",
                        track,
                        "--version",
                        version,
                        "--tag",
                        tag,
                        "--commit",
                        "a" * 40,
                        "--socket-protocol",
                        "marmot.agent-control.v3",
                    ]
                )
                self.assertEqual(code, 0)

                sums = (dist / "SHA256SUMS").read_text(encoding="utf-8")
                self.assertEqual(sums, f"{expected_sha}  {asset_name}\n")

                manifest = json.loads((dist / "latest.json").read_text(encoding="utf-8"))
                self.assertEqual(manifest["schema_version"], 1)
                self.assertEqual(manifest["track"], track)
                self.assertEqual(manifest["version"], version)
                self.assertEqual(manifest["tag"], tag)
                self.assertEqual(manifest["prerelease"], expected_pre)
                self.assertEqual(manifest["socket_protocol"], "marmot.agent-control.v3")
                self.assertEqual(
                    manifest["assets"],
                    [{"name": asset_name, "sha256": expected_sha, "size": len(payload)}],
                )

    def test_rejects_empty_dir_and_invalid_version(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp)
            self.assertEqual(
                wm.main(
                    [
                        "--dir",
                        str(dist),
                        "--track",
                        "hermes",
                        "--version",
                        "0.1.0",
                        "--tag",
                        "hermes-v0.1.0",
                        "--commit",
                        "b" * 40,
                        "--socket-protocol",
                        "marmot.agent-control.v3",
                    ]
                ),
                1,
            )
            (dist / "marmot-agent-hermes.tar.gz").write_bytes(b"x")
            self.assertEqual(
                wm.main(
                    [
                        "--dir",
                        str(dist),
                        "--track",
                        "hermes",
                        "--version",
                        "0.0.0",
                        "--tag",
                        "hermes-v0.0.0",
                        "--commit",
                        "b" * 40,
                        "--socket-protocol",
                        "marmot.agent-control.v3",
                    ]
                ),
                1,
            )


class ResolveReleaseTests(unittest.TestCase):
    def test_resolve_tag_validates_manifest_match_and_track_notes(self) -> None:
        tmp, repo = make_git_repo("0.1.0")
        try:
            sha = git(repo, "rev-parse", "HEAD")
            meta, notes = rr.resolve_tag(repo, "openclaw-v0.1.0", sha)
            self.assertEqual(meta["track"], "openclaw")
            self.assertEqual(meta["version"], "0.1.0")
            self.assertFalse(meta["prerelease"])
            self.assertTrue(meta["should_build"])
            self.assertTrue(meta["verify_existing_tag"])
            self.assertIn("Standalone `openclaw` connector archive", notes)

            rc_meta, _ = rr.resolve_tag(repo, "v0.1.0-rc.1", sha)
            self.assertEqual(rc_meta["track"], "marmot-agent")
            self.assertTrue(rc_meta["prerelease"])

            with self.assertRaisesRegex(ValueError, "does not match"):
                rr.resolve_tag(repo, "openclaw-v0.2.0", sha)
        finally:
            tmp.cleanup()

    def test_dispatch_rc_rejects_planned_bump_ahead_of_manifest_and_allows_matching_manifest(self) -> None:
        tmp, repo = make_git_repo("0.1.0")
        try:
            # Initial RC when manifest is 0.1.0 and no tag exists yet succeeds (0.1.0-rc.1 matches 0.1.0).
            meta, _, notes = rr.resolve_dispatch(
                repo, ref="HEAD", track="hermes", channel="rc", dry_run=False
            )
            self.assertTrue(meta["should_build"])
            self.assertEqual(meta["tag"], "hermes-v0.1.0-rc.1")
            self.assertFalse(meta["verify_existing_tag"])
            self.assertIn("hermes-v0.1.0-rc.1", notes)

            # Once hermes-v0.1.0 is tagged and a patch lands without bumping manifests, RC cut fails closed.
            git(repo, "tag", "hermes-v0.1.0")
            commit_file(
                repo,
                "python/marmot_hermes/fix.py",
                "X = 1\n",
                "Fix Hermes reconnect\n\nLocal-Checks: unittest\nLocal-Review: human\nRelease-Impact: patch\n",
            )
            # Dry run still prints the plan cleanly.
            dry_meta, dry_summary, _ = rr.resolve_dispatch(
                repo, ref="HEAD", track="hermes", channel="rc", dry_run=True
            )
            self.assertFalse(dry_meta["should_build"])
            self.assertEqual(dry_meta["tag"], "hermes-v0.1.1-rc.1")
            self.assertIn("hermes-v0.1.1-rc.1", dry_summary)

            # Non-dry-run RC refuses to ship 0.1.0 artifacts under a 0.1.1-rc.1 tag.
            with self.assertRaisesRegex(ValueError, "manifest is still at 0.1.0"):
                rr.resolve_dispatch(repo, ref="HEAD", track="hermes", channel="rc", dry_run=False)

            # Stable dispatch always refuses unsigned tag creation and prints git tag -s command.
            with self.assertRaisesRegex(ValueError, "git tag -s hermes-v0.1.1"):
                rr.resolve_dispatch(repo, ref="HEAD", track="hermes", channel="stable", dry_run=False)
        finally:
            tmp.cleanup()


if __name__ == "__main__":
    unittest.main()
