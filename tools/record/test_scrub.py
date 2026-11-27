"""scrub.py publishes a batch only when all of it passes its check (#38).

Every value here is made up. Each test runs the real script on a throwaway raw folder.
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRUB = Path(__file__).with_name("scrub.py")
# Made up, and of the shapes the check looks for.
EMAIL = "pat.example@example.org"
HOME = "/home/fabricated-person"


def record(msg: dict) -> str:
    return json.dumps({"t": 1, "dir": "in", "msg": msg})


class Scrub(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.raw, self.out = self.tmp / "raw", self.tmp / "out"
        (self.raw / "codex").mkdir(parents=True)
        (self.out / "codex").mkdir(parents=True)

    def write(self, name: str, *lines: str):
        (self.raw / "codex" / name).write_text("\n".join(lines) + "\n", encoding="utf-8")

    def scrub(self, *maps: str, env=None) -> subprocess.CompletedProcess:
        args = [sys.executable, str(SCRUB), str(self.raw), str(self.out)]
        for m in maps:
            args += ["--map", m]
        return subprocess.run(args, capture_output=True, text=True, env=env)

    def test_a_rejected_batch_publishes_nothing_and_keeps_existing_files(self):
        # Contract: the destination changes only when the whole batch passes. Regression: each file
        # was written before the check, so a rejected batch left files that must not be committed.
        old = self.out / "codex" / "a.jsonl"
        old.write_text("existing\n", encoding="utf-8")
        self.write("a.jsonl", record({"method": "clean"}))
        self.write("b.jsonl", record({"method": "clean"}), record({"text": f"mail {EMAIL}"}))
        result = self.scrub()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(old.read_text(encoding="utf-8"), "existing\n")
        self.assertEqual(sorted(p.name for p in (self.out / "codex").iterdir()), ["a.jsonl"])

    def test_the_diagnostic_names_file_line_and_kind_but_not_the_value(self):
        # Regression: the diagnostic quoted what it found, so the private value reached the terminal.
        self.write("b.jsonl", record({"method": "clean"}), record({"text": f"mail {EMAIL}"}))
        result = self.scrub()
        self.assertEqual(result.returncode, 1)
        self.assertIn("codex/b.jsonl line 2: an e-mail address", result.stderr)
        self.assertNotIn(EMAIL, result.stderr + result.stdout)
        self.assertNotIn("example.org", result.stderr + result.stdout)

    def test_an_object_key_is_checked(self):
        # Regression: only values were checked, so a path used as a key passed.
        self.write("a.jsonl", record({"changes": {f"{HOME}/notes.txt": {"kind": "add"}}}))
        result = self.scrub()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("codex/a.jsonl line 1: a home folder, in a key", result.stderr)
        self.assertNotIn(HOME, result.stderr + result.stdout)
        self.assertFalse((self.out / "codex" / "a.jsonl").exists())

    def test_an_object_key_is_scrubbed_like_a_value(self):
        # Contract: a mapped path is replaced wherever it appears, so a key holding one can pass.
        self.write("a.jsonl", record({"changes": {f"{HOME}/notes.txt": {"path": f"{HOME}/notes.txt"}}}))
        result = self.scrub(f"{HOME}=/home/user")
        self.assertEqual(result.returncode, 0, result.stderr)
        msg = json.loads((self.out / "codex" / "a.jsonl").read_text(encoding="utf-8"))["msg"]
        self.assertEqual(msg, {"changes": {"/home/user/notes.txt": {"path": "/home/user/notes.txt"}}})

    def test_a_cassette_header_is_kept_for_a_user_named_like_the_harness(self):
        # Contract: a cassette header's own fields are written as they are and do not fail the check.
        header = {"cassette": 1, "harness": "codex", "version": "0.0.0", "recorded": "2026-01-01", "format": "x"}
        (self.raw / "cassettes" / "codex").mkdir(parents=True)
        (self.raw / "cassettes" / "codex" / "a.jsonl").write_text(json.dumps(header) + "\n", encoding="utf-8")
        result = self.scrub(env={**os.environ, "USER": "codex"})
        self.assertEqual(result.returncode, 0, result.stderr)
        written = (self.out / "cassettes" / "codex" / "a.jsonl").read_text(encoding="utf-8")
        self.assertEqual(json.loads(written), header)

    def test_a_clean_batch_replaces_existing_files(self):
        old = self.out / "codex" / "a.jsonl"
        old.write_text("existing\n", encoding="utf-8")
        self.write("a.jsonl", record({"method": "clean"}))
        result = self.scrub()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(old.read_text(encoding="utf-8"), record({"method": "clean"}) + "\n")


if __name__ == "__main__":
    unittest.main()
