"""The Hermes and OpenClaw recorders publish only a complete capture, and keep raw files owner-only (#38).

Each test runs the real `record.sh` with a stand-in for `docker` on the PATH, which plays the container:
it writes what the scenario's mode says into the folder mounted at /out, prints a log, and exits. No
harness runs and nothing is recorded. Every value is made up.
"""

import os
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

HERE = Path(__file__).parent
KEY = "fabricated-model-key-0000"
FINISHED = '{"t": 2, "dir": "out", "via": "note", "msg": {"finished": "ok"}}'
STARTED = '{"t": 1, "dir": "out", "via": "note", "msg": {"scenario": "simple", "steps": 1}}'

# The stand-in container. FAKE_MODE: ok, failed (exits nonzero), unfinished (no closing note),
# none (writes nothing). It also notes the mode of the folder that holds /out.
DOCKER = textwrap.dedent(f"""\
    #!/bin/sh
    [ "$1" = run ] || exit 0
    out=
    while [ $# -gt 0 ]; do
        case "$1" in -v) case "$2" in *:/out) out=${{2%:/out}} ;; esac; shift ;; esac
        shift
    done
    stat -c %a "$(dirname "$out")" > "$FAKE_NOTES"
    echo "gateway started"
    echo "a line with the key {KEY} in it"
    case "$FAKE_MODE" in
        ok) printf '%s\\n%s\\n' '{STARTED}' '{FINISHED}' > "$out/recording.jsonl" ;;
        failed) printf '%s\\n' '{STARTED}' > "$out/recording.jsonl"; exit 3 ;;
        unfinished) printf '%s\\n' '{STARTED}' > "$out/recording.jsonl" ;;
    esac
    """)


class Recorder:
    script: Path

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        bin_dir = self.tmp / "bin"
        bin_dir.mkdir()
        (bin_dir / "docker").write_text(DOCKER)
        (bin_dir / "docker").chmod(0o755)
        (self.tmp / "key").write_text(KEY + "\n")
        (self.tmp / "scenario.json").write_text('{"env": {}}\n')
        self.out = self.tmp / "dest" / "simple.jsonl"
        self.log = self.tmp / "gateway.log"
        self.notes = self.tmp / "notes"

    def record(self, mode: str) -> subprocess.CompletedProcess:
        env = {
            **os.environ,
            "PATH": f"{self.tmp / 'bin'}:{os.environ['PATH']}",
            "MODEL_BASE_URL": "http://model.invalid/v1",
            "MODEL_NAME": "fabricated-model",
            "MODEL_KEY_FILE": str(self.tmp / "key"),
            "RECORD_LOG": str(self.log),
            "FAKE_MODE": mode,
            "FAKE_NOTES": str(self.notes),
        }
        return subprocess.run(["sh", str(self.script), str(self.tmp / "scenario.json"), str(self.out)],
                              env=env, capture_output=True, text=True)

    def mode(self, path: Path) -> int:
        return stat.S_IMODE(path.stat().st_mode)

    def test_a_complete_capture_is_published_owner_only(self):
        # Contract: the recording, its log and the folder the container writes into are the owner's alone.
        # Regression: the folder holding the raw log and capture was world-readable (0777), and so was the
        # log, kept with the caller's umask or the mode of a file already there.
        self.log.write_text("an earlier log\n")
        self.log.chmod(0o644)
        result = self.record("ok")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.out.read_text(), f"{STARTED}\n{FINISHED}\n")
        self.assertEqual(self.mode(self.out), 0o600)
        self.assertEqual(self.mode(self.log), 0o600)
        self.assertEqual(self.notes.read_text().strip(), "700")
        self.assertIn("gateway started", self.log.read_text())
        self.assertNotIn(KEY, self.log.read_text())

    def test_a_failed_or_incomplete_capture_fails_and_leaves_the_destination(self):
        # Regression: a failed container run, or one that wrote a partial recording or none, exited 0,
        # and a partial recording replaced the existing one.
        for mode in ("failed", "unfinished", "none"):
            with self.subTest(mode=mode):
                self.out.parent.mkdir(exist_ok=True)
                self.out.write_text("existing\n")
                result = self.record(mode)
                self.assertNotEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.out.read_text(), "existing\n")
                self.assertEqual(sorted(p.name for p in self.out.parent.iterdir()), ["simple.jsonl"])
                # The log is kept for finding out why, still without the key.
                self.assertEqual(self.mode(self.log), 0o600)
                self.assertNotIn(KEY, self.log.read_text())


class Hermes(Recorder, unittest.TestCase):
    script = HERE / "hermes" / "record.sh"


class OpenClaw(Recorder, unittest.TestCase):
    script = HERE / "openclaw" / "record.sh"


if __name__ == "__main__":
    unittest.main()
