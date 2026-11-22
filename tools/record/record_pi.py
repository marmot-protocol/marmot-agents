#!/usr/bin/env python3
"""Record `pi --mode rpc` sessions: every JSON record in both directions.

Usage:
    record_pi.py <scenario> <project-dir> <output.jsonl> [model-key-file]
    record_pi.py --list

Each output line is {"t": <ms since start>, "dir": "out"|"in", "msg": <the JSON record>}.
"out" is what this client sent to Pi; "in" is what Pi sent back. A line whose message is
{"_recorder": ...} is a note the recorder wrote, such as that it started Pi again.

Pi runs in a throwaway container, through `tools/pi/pi.sh`, with the model key mounted
read-only. The model is MODEL_BASE_URL, MODEL_NAME and MODEL_KEY_FILE (or [model-key-file]), from the
environment or from tests.env at the top of the repository (tests.env.example says how); there is no
default. Its state (settings and sessions) is a fresh temporary folder for each recording.
The script has no dependencies besides Docker. Raw recordings contain local paths. Scrub them
before sharing.

The recorder opens each Pi as the driver does: it asks for the state, the models, the thinking
levels and the commands before the first prompt, and again after a new session.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import tests_env

TURN_TIMEOUT = 300
PI = Path(__file__).resolve().parent.parent / "pi" / "pi.sh"
# The example extension Pi ships that asks before a dangerous command, as installed in the image.
GATE = "/usr/local/lib/node_modules/@earendil-works/pi-coding-agent/examples/extensions/permission-gate.ts"


class Pi:
    def __init__(self, out_path: Path, cwd: str, key: str, state: str, extra: list[str] | None = None):
        self.out = out_path.open("w", encoding="utf-8")
        self.cwd, self.key, self.state = cwd, key, state
        self.t0 = time.monotonic()
        self.lock = threading.Lock()
        self.cond = threading.Condition()
        self.inbox: list[dict] = []  # everything Pi sent that is not a response, in order
        self.responses: dict[str, dict] = {}
        self.next_id = 1
        self.ui_handler = None  # callable(request) -> response fields, or None to leave unanswered
        self.session_id = None
        self._version()
        self._spawn(extra or [])

    def _version(self):
        """Pi does not say its version over RPC, so the driver asks `pi --version` once, and the
        recorder notes the answer the same way."""
        run = subprocess.run([str(PI), self.key, self.state, "--version"], cwd=self.cwd,
                             capture_output=True, text=True, check=False)
        self._record("out", {"_recorder": "pi --version", "version": run.stdout.strip() or None})

    def _spawn(self, extra: list[str]):
        self.proc = subprocess.Popen(
            [str(PI), self.key, self.state, "--mode", "rpc", *extra],
            cwd=self.cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        threading.Thread(target=self._read, args=(self.proc,), daemon=True).start()

    def restart(self, hard: bool = False, extra: list[str] | None = None):
        """Stop Pi and start a fresh one, writing to the same recording. A hard restart kills it
        without warning, as a crash would."""
        how = "killed" if hard else "stopped"
        self._record("out", {"_recorder": f"pi was {how} and a new one started"})
        try:
            if hard:
                self.proc.kill()
                self.proc.stdin.close()
            else:
                self.proc.stdin.close()
            self.proc.wait(timeout=30)
        except Exception:
            self.proc.kill()
        with self.cond:
            self.responses.clear()
        self._spawn(extra or [])

    # -- wire ---------------------------------------------------------------

    def _record(self, direction: str, msg: dict):
        with self.lock:
            line = {"t": int((time.monotonic() - self.t0) * 1000), "dir": direction, "msg": msg}
            self.out.write(json.dumps(line) + "\n")
            self.out.flush()

    def send(self, msg: dict):
        self._record("out", msg)
        self.proc.stdin.write((json.dumps(msg) + "\n").encode())
        self.proc.stdin.flush()

    def _read(self, proc):
        # Split on LF only: Unicode line separators are valid inside JSON strings.
        for raw in proc.stdout:
            line = raw.decode("utf-8").rstrip("\r\n")
            if not line.strip():
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                self._record("in", {"_unparsed": line})
                continue
            self._record("in", msg)
            if msg.get("type") == "response" and "id" in msg:
                with self.cond:
                    self.responses[msg["id"]] = msg
                    self.cond.notify_all()
                continue
            if msg.get("type") == "extension_ui_request" and self.ui_handler:
                answer = self.ui_handler(msg)
                if answer is not None:
                    self.send({"type": "extension_ui_response", "id": msg["id"], **answer})
            with self.cond:
                self.inbox.append(msg)
                self.cond.notify_all()

    # -- client API ---------------------------------------------------------

    def request(self, command: str, timeout: float = 120, **fields) -> dict:
        rid = str(self.next_id)
        self.next_id += 1
        self.send({"id": rid, "type": command, **fields})
        deadline = time.monotonic() + timeout
        with self.cond:
            while rid not in self.responses:
                left = deadline - time.monotonic()
                if left <= 0:
                    raise TimeoutError(f"no response to {command}")
                self.cond.wait(left)
            return self.responses.pop(rid)

    def wait_for(self, predicate, timeout: float = TURN_TIMEOUT, start: int = 0) -> tuple[int, dict]:
        """Wait for the first record at or after index `start` that satisfies `predicate`."""
        deadline = time.monotonic() + timeout
        i = start
        with self.cond:
            while True:
                while i < len(self.inbox):
                    if predicate(self.inbox[i]):
                        return i, self.inbox[i]
                    i += 1
                left = deadline - time.monotonic()
                if left <= 0:
                    raise TimeoutError("timed out waiting for a record")
                self.cond.wait(left)

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=30)
        except Exception:
            self.proc.kill()
        self.out.close()

    # -- what the driver does -----------------------------------------------

    def open(self):
        """What the driver asks before the first prompt, and after a new session."""
        state = self.request("get_state")
        self.session_id = (state.get("data") or {}).get("sessionId")
        self.request("get_available_models")
        self.request("get_available_thinking_levels")
        self.request("get_commands")

    def mark(self) -> int:
        with self.cond:
            return len(self.inbox)

    def prompt(self, text: str, **fields) -> int:
        mark = self.mark()
        self.request("prompt", message=text, **fields)
        return mark

    def settled(self, mark: int, timeout: float = TURN_TIMEOUT) -> dict:
        _, msg = self.wait_for(lambda m: m.get("type") == "agent_settled", timeout=timeout, start=mark)
        return msg

    def turn(self, text: str, **fields) -> dict:
        return self.settled(self.prompt(text, **fields))


def tool_started(name: str):
    return lambda m: m.get("type") == "tool_execution_start" and m.get("toolName") == name


def answer_with(value):
    """An extension UI handler that picks `value` in a selection and answers a confirmation with
    whether `value` is "Yes"; fire-and-forget requests get no answer."""

    def handler(msg: dict):
        method = msg.get("method")
        if method == "select":
            return {"value": value}
        if method == "confirm":
            return {"confirmed": value == "Yes"}
        if method in ("input", "editor"):
            return {"value": value}
        return None

    return handler


# -- scenarios ----------------------------------------------------------------


def simple(p: Pi):
    """One turn with a short text answer and no tools."""
    p.open()
    p.turn("Reply with exactly: Hello from the recording.")


def command(p: Pi):
    """One turn that runs a shell command and reports its output."""
    p.open()
    p.turn("Run `python3 greet.py` and tell me what it printed.")


def file_edit(p: Pi):
    """One turn that edits a file."""
    p.open()
    p.turn("In greet.py, make greet() end its result with an exclamation mark. Then run `python3 greet.py`.")


def failing_command(p: Pi):
    """A command exits with an error."""
    p.open()
    p.turn("Run `python3 -m pytest -q test_greet.py` once and report the result in one sentence. Do not fix anything.")


def two_turns(p: Pi):
    """Two turns in one session."""
    p.open()
    p.turn("Remember the word 'marmot'. Reply with: Noted.")
    p.turn("What word did I ask you to remember? Reply with just the word.")


def interrupt(p: Pi):
    """The user interrupts a turn while a command is running."""
    p.open()
    mark = p.prompt("Run `sleep 60` and then tell me it finished.")
    p.wait_for(tool_started("bash"), timeout=180, start=mark)
    time.sleep(2)
    p.request("abort", timeout=60)
    p.settled(mark, timeout=60)


def steer(p: Pi):
    """The user sends a new message while a turn is running. The driver sends it as a prompt that
    steers the running turn."""
    p.open()
    mark = p.prompt("Run `sleep 8`, then tell me the name of the function defined in greet.py.")
    p.wait_for(tool_started("bash"), timeout=180, start=mark)
    p.request("prompt", message="Also tell me how many lines greet.py has.", streamingBehavior="steer")
    p.settled(mark)


def new_session(p: Pi):
    """A turn, then a new session, then a turn in it."""
    p.open()
    p.turn("Remember the word 'marmot'. Reply with: Noted.")
    p.request("new_session")
    p.open()
    p.turn("What word did I ask you to remember? If you do not know, reply with: I do not know.")


def resume(p: Pi):
    """A session is resumed after Pi restarts."""
    p.open()
    p.turn("Remember the word 'marmot'. Reply with: Noted.")
    session = p.session_id
    p.restart(extra=["--session", session])
    p.open()
    p.turn("What word did I ask you to remember? Reply with just the word.")


def restart_mid_turn(p: Pi):
    """Pi is killed while a command is running, then the session is resumed."""
    p.open()
    mark = p.prompt("Run `echo ran >> runs.txt && sleep 20`, then reply with the word done.")
    p.wait_for(tool_started("bash"), timeout=180, start=mark)
    time.sleep(2)
    session = p.session_id
    p.restart(hard=True, extra=["--session", session])
    p.open()
    p.turn("Which shell command did you run last? Reply with only the command.")


def settings(p: Pi):
    """The client changes the thinking level between turns, and asks for the state again."""
    p.open()
    p.turn("Reply with exactly: one.")
    p.request("set_thinking_level", level="high")
    p.request("get_state")
    p.turn("Reply with exactly: two.")


def bad_model(p: Pi):
    """The client asks for a model that does not exist."""
    p.open()
    p.request("set_model", provider="numes", modelId="no-such-model")
    p.turn("Reply with exactly: still here.")


def compact(p: Pi):
    """The client asks for the conversation to be compacted between two turns."""
    p.open()
    p.turn("Remember the word 'marmot'. Reply with: Noted.")
    p.request("compact", timeout=TURN_TIMEOUT)
    p.turn("What word did I ask you to remember? Reply with just the word.")


def slash_command(p: Pi):
    """A prompt that starts with a slash, as the driver passes it: `/mcp` is a built-in extension's
    command, which Pi runs itself."""
    p.open()
    p.request("prompt", message="/mcp")
    time.sleep(3)
    p.turn("Reply with exactly: after the command.")


def gate_allowed(p: Pi):
    """An extension asks before a dangerous command, and the user allows it. Pi has no approval of
    its own; this is the example extension it ships, which asks through its extension UI."""
    p.ui_handler = answer_with("Yes")
    p.open()
    p.turn("Run exactly `rm -rf build` and nothing else, then reply with the word done.")


def gate_denied(p: Pi):
    """The same, and the user denies it."""
    p.ui_handler = answer_with("No")
    p.open()
    p.turn("Run exactly `rm -rf build` and nothing else, then reply with the word done. If you are not allowed, say so and stop.")


SCENARIOS = {
    f.__name__: f
    for f in [
        simple, command, file_edit, failing_command, two_turns, interrupt, steer, new_session, resume,
        restart_mid_turn, settings, bad_model, compact, slash_command, gate_allowed, gate_denied,
    ]
}
# Pi's own arguments for a scenario, beyond `--mode rpc`.
EXTRA = {"gate_allowed": ["--extension", GATE], "gate_denied": ["--extension", GATE]}


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "--list":
        for name, f in SCENARIOS.items():
            print(f"{name:18} {f.__doc__.splitlines()[0]}")
        return
    if len(sys.argv) not in (4, 5) or sys.argv[1] not in SCENARIOS:
        sys.exit(__doc__)
    tests_env.load()
    if len(sys.argv) == 5:
        os.environ["MODEL_KEY_FILE"] = sys.argv[4]
    tests_env.need(*tests_env.NAMES)
    name, project, out = sys.argv[1:4]
    key = os.environ["MODEL_KEY_FILE"]
    with tempfile.TemporaryDirectory(prefix="pi-state-") as state:
        pi = Pi(Path(out), str(Path(project).resolve()), str(Path(key).resolve()), state, EXTRA.get(name))
        try:
            SCENARIOS[name](pi)
        finally:
            pi.close()
            # Pi ran as this user, but make sure nothing it left behind stops the clean-up.
            subprocess.run(["chmod", "-R", "u+rwX", state], check=False)


if __name__ == "__main__":
    main()
