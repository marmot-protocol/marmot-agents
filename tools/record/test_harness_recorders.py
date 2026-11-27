"""The Codex, ACP, Pi and Claude Code recorders publish only a finished run, and keep raw output owner-only (#248).

Each test runs the real recorder on its `simple` scenario against a stand-in for the harness: a small
script that speaks just enough of the harness's JSON lines (Codex, ACP, Pi behind a stand-in for
`docker`), or a stand-in for the Claude Agent SDK. No harness runs and nothing is recorded. Every value
is made up.

The Claude recorder runs under Node, which strips its types, from a copy beside the stand-in SDK, so
that `@anthropic-ai/claude-agent-sdk` resolves to the stand-in.
"""

import json
import os
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

HERE = Path(__file__).parent

# The stand-in harness: fake.py <protocol> <mode> <notes-file>. It answers each request of the simple
# scenario. Mode ok finishes the turn; failed refuses to start a session, which the scenario does not
# expect; hang notes that the turn started and never ends it.
HARNESS = textwrap.dedent("""\
    import json, sys
    protocol, mode, notes = sys.argv[1:4]
    if protocol == "pi" and sys.argv[-1] == "--version":
        print("0.0.0-fabricated")
        sys.exit(0)
    RESULTS = {
        "initialize": {},
        "thread/start": {"thread": {"id": "thread-1"}, "model": "fabricated-model"},
        "turn/start": {"turn": {"id": "turn-1"}},
        "session/new": {"sessionId": "session-1"},
        "session/prompt": {"stopReason": "end_turn"},
    }
    TURNS = ("turn/start", "session/prompt", "prompt")
    def say(msg):
        print(json.dumps(msg), flush=True)
    for line in sys.stdin:
        msg = json.loads(line)
        kind = msg.get("type") if protocol == "pi" else msg.get("method")
        if "id" not in msg or kind is None:
            continue
        if kind in TURNS and mode == "hang":
            open(notes, "w").write("turn started")
            continue
        if protocol == "pi":
            say({"type": "response", "id": msg["id"], "command": kind, "success": True, "data": {}})
        elif mode == "failed" and kind in ("thread/start", "session/new"):
            say({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32603, "message": "fabricated"}})
        else:
            say({"jsonrpc": "2.0", "id": msg["id"], "result": RESULTS[kind]})
        if kind == "turn/start":
            say({"method": "turn/completed", "params": {"threadId": "thread-1", "turn": {"status": "completed"}}})
        if kind == "prompt":
            say({"type": "agent_settled"})
    """)

# tools/pi/pi.sh runs Pi with `docker run`; this stand-in runs the stand-in harness instead.
DOCKER = textwrap.dedent("""\
    #!/bin/sh
    [ "$1" = run ] || exit 0
    exec python3 "$FAKE_HARNESS" pi "$FAKE_MODE" "$FAKE_NOTES" "$@"
    """)

# The stand-in Claude Agent SDK. Mode ok answers each prompt with a result; error ends the query with an
# error, as several recorded scenarios do on purpose; failed throws when the query starts; hang never
# ends the turn; timeout never ends it either, with the recorder's timers cut short so that its own
# time limit is reached at once.
SDK = textwrap.dedent("""\
    import { writeFileSync } from "node:fs";
    const mode = process.env.FAKE_MODE;
    if (mode === "timeout") {
      const real = globalThis.setTimeout;
      globalThis.setTimeout = (f, ms, ...rest) => real(f, Math.min(ms, 50), ...rest);
    }
    export function query({ prompt }) {
      if (mode === "failed") throw new Error("fabricated failure");
      let stop;
      const stopped = new Promise((r) => (stop = r));
      const q = (async function* () {
        for await (const _ of prompt) {
          yield { type: "system", subtype: "init", session_id: "session-1" };
          if (mode === "hang" || mode === "timeout") {
            writeFileSync(process.env.FAKE_NOTES, "turn started");
            await stopped;
            return;
          }
          if (mode === "error") throw new Error("fabricated error result");
          yield { type: "result", subtype: "success", session_id: "session-1" };
        }
      })();
      q.interrupt = async () => stop();
      return q;
    }
    """)


class Recorder:
    """Runs one recorder. A subclass says how to start it and which modes its stand-in plays."""

    failing_modes: tuple = ("failed",)
    env: dict = {}

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.harness = self.tmp / "fake.py"
        self.harness.write_text(HARNESS)
        self.project = self.tmp / "project"
        self.project.mkdir()
        self.dest = self.tmp / "dest"
        self.dest.mkdir()
        self.dest.chmod(0o755)
        self.out = self.dest / "simple.jsonl"
        self.notes = self.tmp / "notes"

    def command(self, mode: str) -> list[str]:
        raise NotImplementedError

    def start(self, mode: str) -> subprocess.Popen:
        env = {
            **os.environ,
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "FAKE_MODE": mode,
            "FAKE_NOTES": str(self.notes),
            "FAKE_HARNESS": str(self.harness),
            **self.env,
        }
        return subprocess.Popen(self.command(mode), env=env, cwd=self.tmp, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, preexec_fn=lambda: os.umask(0o022))

    def record(self, mode: str) -> tuple[int, str]:
        proc = self.start(mode)
        stdout, stderr = proc.communicate(timeout=60)
        return proc.returncode, stdout + stderr

    def mode(self, path: Path) -> int:
        return stat.S_IMODE(path.lstat().st_mode)

    def assert_left_as_it_was(self):
        self.assertEqual(self.out.read_text(), "existing\n")
        self.assertEqual(sorted(p.name for p in self.dest.iterdir()), ["simple.jsonl"])

    def test_a_finished_run_is_published_owner_only(self):
        # Contract: a run that finishes its scenario replaces the destination with the whole recording,
        # readable by the owner alone, and leaves nothing else beside it.
        # Regression: the recording was written with the caller's umask (here 022, so world-readable).
        self.out.write_text("existing\n")
        self.out.chmod(0o644)
        code, output = self.record("ok")
        self.assertEqual(code, 0, output)
        records = [json.loads(line) for line in self.out.read_text().splitlines()]
        self.assertTrue(any(r["dir"] == "in" for r in records), records)
        self.assertEqual(self.mode(self.out), 0o600)
        self.assertEqual(sorted(p.name for p in self.dest.iterdir()), ["simple.jsonl"])

    def test_a_failed_run_fails_and_leaves_the_destination(self):
        # Regression: a scenario that failed exited 0, and its partial recording replaced the existing one.
        for mode in self.failing_modes:
            with self.subTest(mode=mode):
                self.out.write_text("existing\n")
                code, output = self.record(mode)
                self.assertNotEqual(code, 0, output)
                self.assert_left_as_it_was()

    def test_an_interrupted_run_is_owner_only_while_it_runs_and_leaves_the_destination(self):
        # Contract: while the harness runs, what is recorded so far sits in an owner-only folder beside the
        # destination, and an interrupted run removes it and leaves the destination as it was.
        # Regression: the recorder streamed into the destination itself, with the caller's umask.
        self.out.write_text("existing\n")
        proc = self.start("hang")
        self.addCleanup(proc.kill)
        deadline = time.monotonic() + 30
        while not self.notes.exists():
            if proc.poll() is not None:
                self.fail(f"the recorder exited early: {proc.communicate()}")
            self.assertLess(time.monotonic(), deadline, "the stand-in never saw the turn start")
            time.sleep(0.05)
        self.assertEqual(self.out.read_text(), "existing\n")
        work = [p for p in self.dest.iterdir() if p != self.out]
        self.assertEqual(len(work), 1, work)
        self.assertEqual(self.mode(work[0]), 0o700)
        raw = list(work[0].iterdir())
        self.assertEqual([self.mode(p) for p in raw], [0o600])
        self.assertEqual(json.loads(raw[0].read_text().splitlines()[0])["dir"], "out")
        proc.send_signal(signal.SIGINT)
        proc.communicate(timeout=30)
        self.assertNotEqual(proc.returncode, 0)
        self.assert_left_as_it_was()


class Codex(Recorder, unittest.TestCase):
    def setUp(self):
        super().setUp()
        codex = self.bin / "codex"
        codex.write_text(f'#!/bin/sh\nexec python3 "{self.harness}" codex "$FAKE_MODE" "$FAKE_NOTES"\n')
        codex.chmod(0o755)

    def command(self, mode):
        return [sys.executable, str(HERE / "record_codex.py"), "simple", str(self.project), str(self.out)]


class Acp(Recorder, unittest.TestCase):
    def command(self, mode):
        # The agent gets none of the recorder's environment but PATH, so the mode goes in its command.
        agent = shlex.join(["python3", str(self.harness), "acp", mode, str(self.notes)])
        return [sys.executable, str(HERE / "record_acp.py"), "simple", str(self.project), str(self.out),
                "--agent", agent, "--home", str(self.tmp / "home")]


class Pi(Recorder, unittest.TestCase):
    # Pi's simple scenario has no request whose refusal it notices, so only an interrupted run fails.
    failing_modes = ()

    def setUp(self):
        super().setUp()
        (self.bin / "docker").write_text(DOCKER)
        (self.bin / "docker").chmod(0o755)
        (self.tmp / "key").write_text("fabricated-model-key-0000\n")
        self.env = {"MODEL_BASE_URL": "http://model.invalid/v1", "MODEL_NAME": "fabricated-model",
                    "MODEL_KEY_FILE": str(self.tmp / "key")}

    def command(self, mode):
        return [sys.executable, str(HERE / "record_pi.py"), "simple", str(self.project), str(self.out)]


class Claude(Recorder, unittest.TestCase):
    failing_modes = ("failed", "timeout")

    def setUp(self):
        super().setUp()
        rec = self.tmp / "claude"
        rec.mkdir()
        shutil.copy(HERE / "claude" / "record_claude.ts", rec)
        shutil.copy(HERE / "claude" / "package.json", rec)
        sdk = rec / "node_modules" / "@anthropic-ai" / "claude-agent-sdk"
        sdk.mkdir(parents=True)
        (sdk / "package.json").write_text('{"name": "@anthropic-ai/claude-agent-sdk", "type": "module", '
                                          '"exports": "./index.js"}\n')
        (sdk / "index.js").write_text(SDK)

    def command(self, mode):
        return ["node", str(self.tmp / "claude" / "record_claude.ts"), "simple", str(self.project), str(self.out)]

    def test_a_run_the_harness_ends_with_an_error_is_still_published(self):
        # Contract: an error the harness ends a query with is part of what a scenario records (the committed
        # interrupt, approval_unanswered, bad_model and restart_mid_turn recordings end that way), so the run
        # counts as finished and is published.
        code, output = self.record("error")
        self.assertEqual(code, 0, output)
        self.assertIn("ended with an error", output)
        self.assertIn("fabricated error result", self.out.read_text())
        self.assertEqual(self.mode(self.out), 0o600)


if __name__ == "__main__":
    unittest.main()
