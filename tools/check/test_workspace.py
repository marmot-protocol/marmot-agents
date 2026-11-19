#!/usr/bin/env python3
"""Tests for workspace.py.

    python3 -m unittest discover -s tools/check -p 'test_*.py'
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import workspace  # noqa: E402

ROOT = """
[workspace]
members = ["crates/*"]
[workspace.package]
publish = false
[workspace.lints.rust]
unsafe_code = "forbid"
"""

INHERITS = "".join(f"{key}.workspace = true\n" for key in workspace.INHERITED)


def crate(name: str, lints: str = "[lints]\nworkspace = true\n") -> str:
    return f'[package]\nname = "{name}"\n{INHERITS}{lints}'


def problems_in(files: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as folder:
        root = Path(folder)
        for name, text in {"Cargo.toml": ROOT, **files}.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        return workspace.check(root)


class Workspace(unittest.TestCase):
    def test_a_crate_that_inherits_everything_passes(self):
        self.assertEqual(problems_in({"crates/marmot-agent-a/Cargo.toml": crate("marmot-agent-a")}), [])

    def test_a_crate_without_the_lints_fails(self):
        found = problems_in({"crates/marmot-agent-a/Cargo.toml": crate("marmot-agent-a", lints="")})
        self.assertEqual(
            found, ["crates/marmot-agent-a/Cargo.toml: needs `[lints] workspace = true` and no lints of its own"]
        )

    def test_a_crate_inside_another_crate_is_checked_too(self):
        found = problems_in(
            {
                "crates/marmot-agent-a/Cargo.toml": crate("marmot-agent-a"),
                "crates/marmot-agent-a/marmot-agent-a-support/Cargo.toml": crate("marmot-agent-a-support", lints=""),
            }
        )
        self.assertEqual(
            found,
            [
                "crates/marmot-agent-a/marmot-agent-a-support/Cargo.toml: "
                "needs `[lints] workspace = true` and no lints of its own"
            ],
        )

    def test_a_crate_in_the_root_manifest_fails(self):
        found = problems_in(
            {
                "Cargo.toml": ROOT + '[package]\nname = "marmot-agent"\n',
                "crates/marmot-agent-a/Cargo.toml": crate("marmot-agent-a"),
            }
        )
        self.assertEqual(found, ["Cargo.toml: the root manifest is a crate; crates live under crates/"])

    def test_a_build_folder_is_not_read(self):
        found = problems_in(
            {
                "crates/marmot-agent-a/Cargo.toml": crate("marmot-agent-a"),
                "target/package/other/Cargo.toml": crate("other", lints=""),
            }
        )
        self.assertEqual(found, [])


if __name__ == "__main__":
    unittest.main()
