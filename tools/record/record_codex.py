#!/usr/bin/env python3
"""Record `codex app-server` sessions: every JSON message in both directions.

Usage:
    record_codex.py <scenario> <project-dir> <output.jsonl>
    record_codex.py --list

Each output line is {"t": <ms since start>, "dir": "out"|"in", "msg": <the JSON message>}.
"out" is what this client sent to the app server; "in" is what the app server sent back.

The script has no dependencies. It needs the `codex` command on PATH and a signed-in Codex.
Raw recordings contain local paths and account details. Scrub them before sharing.
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading
import time
from pathlib import Path

TURN_TIMEOUT = 240


class AppServer:
    def __init__(self, out_path: Path):
        self.out = out_path.open("w", encoding="utf-8")
        self.t0 = time.monotonic()
        self.lock = threading.Lock()
        self.cond = threading.Condition()
        self.inbox: list[dict] = []  # notifications and server requests, in order
        self.responses: dict[int, dict] = {}
        self.next_id = 1
        self.request_handler = None  # callable(msg) -> result dict, or None to leave unanswered
        self.proc = subprocess.Popen(
            ["codex", "app-server", "--disable", "hooks"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        threading.Thread(target=self._read, daemon=True).start()

    # -- wire ---------------------------------------------------------------

    def _record(self, direction: str, msg: dict):
        with self.lock:
            self.out.write(
                json.dumps({"t": int((time.monotonic() - self.t0) * 1000), "dir": direction, "msg": msg}) + "\n"
            )
            self.out.flush()

    def _send(self, msg: dict):
        self._record("out", msg)
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def _read(self):
        for line in self.proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                self._record("in", {"_unparsed": line})
                continue
            self._record("in", msg)
            if "method" not in msg and "id" in msg:
                with self.cond:
                    self.responses[msg["id"]] = msg
                    self.cond.notify_all()
                continue
            if "id" in msg and self.request_handler:
                result = self.request_handler(msg)
                if result is not None:
                    self._send({"id": msg["id"], "result": result})
            with self.cond:
                self.inbox.append(msg)
                self.cond.notify_all()

    # -- client API ---------------------------------------------------------

    def request(self, method: str, params: dict | None = None, timeout: float = 60) -> dict:
        rid = self.next_id
        self.next_id += 1
        self._send({"id": rid, "method": method, "params": params or {}})
        deadline = time.monotonic() + timeout
        with self.cond:
            while rid not in self.responses:
                left = deadline - time.monotonic()
                if left <= 0:
                    raise TimeoutError(f"no response to {method}")
                self.cond.wait(left)
            return self.responses.pop(rid)

    def notify(self, method: str, params: dict | None = None):
        msg = {"method": method}
        if params is not None:
            msg["params"] = params
        self._send(msg)

    def wait_for(self, predicate, timeout: float = TURN_TIMEOUT, start: int = 0) -> tuple[int, dict]:
        """Wait for the first inbound message at or after index `start` that satisfies `predicate`."""
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
                    raise TimeoutError("timed out waiting for a message")
                self.cond.wait(left)

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()
        self.out.close()

    # -- helpers ------------------------------------------------------------

    def initialize(self):
        self.request(
            "initialize",
            {
                "clientInfo": {"name": "marmot-agents-recorder", "title": "Recorder", "version": "0.0.0"},
                "capabilities": {"experimentalApi": True},
            },
        )
        self.notify("initialized")

    def start_thread(self, cwd: str, approval="never", sandbox="workspace-write", ephemeral=True) -> str:
        r = self.request(
            "thread/start",
            {"cwd": cwd, "approvalPolicy": approval, "sandbox": sandbox, "ephemeral": ephemeral},
        )
        return r["result"]["thread"]["id"]

    def start_turn(self, thread_id: str, text: str, **extra) -> tuple[str, int]:
        mark = len(self.inbox)
        r = self.request(
            "turn/start",
            {"threadId": thread_id, "effort": "low", "input": [text_input(text)], **extra},
        )
        if "error" in r:
            return "", mark
        return r["result"]["turn"]["id"], mark

    def finish_turn(self, mark: int, timeout: float = TURN_TIMEOUT) -> dict:
        _, msg = self.wait_for(lambda m: m.get("method") == "turn/completed", timeout=timeout, start=mark)
        return msg

    def turn(self, thread_id: str, text: str, **extra) -> dict:
        _, mark = self.start_turn(thread_id, text, **extra)
        return self.finish_turn(mark)


def text_input(text: str) -> dict:
    return {"type": "text", "text": text, "text_elements": []}


def is_item(msg: dict, method: str, item_type: str) -> bool:
    return msg.get("method") == method and (msg.get("params", {}).get("item", {}) or {}).get("type") == item_type


def answer_with(decision: str):
    """A request handler that answers approvals with `decision` and questions with their first option."""

    def handler(msg: dict):
        method = msg.get("method", "")
        if method.endswith("/requestApproval"):
            return {"decision": decision}
        if method == "item/tool/requestUserInput":
            answers = {}
            for q in msg.get("params", {}).get("questions", []):
                options = q.get("options") or []
                label = options[0].get("label") if options else "yes"
                answers[q.get("id", "q")] = {"answers": [label]}
            return {"answers": answers}
        return None

    return handler


# -- scenarios ----------------------------------------------------------------


def simple(s: AppServer, cwd: str):
    """One turn with a short text answer and no tools."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(t, "Reply with exactly: Hello from the recording.")


def command(s: AppServer, cwd: str):
    """One turn that runs a shell command and reports its output."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(t, "Run `python3 greet.py` and tell me what it printed.")


def file_edit(s: AppServer, cwd: str):
    """One turn that edits a file, with no approval needed."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(t, "In greet.py, make greet() end its result with an exclamation mark. Then run `python3 greet.py`.")


def plan(s: AppServer, cwd: str):
    """One turn where the agent keeps a plan while it works."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(
        t,
        "Use your plan tool to write a three-step plan, then carry it out, updating the plan as you go: "
        "1) read greet.py, 2) read test_greet.py, 3) tell me in one sentence whether the test would pass.",
    )


def approval_allowed(s: AppServer, cwd: str):
    """A command needs approval and the user allows it."""
    s.request_handler = answer_with("accept")
    s.initialize()
    t = s.start_thread(cwd, approval="untrusted", sandbox="read-only")
    s.turn(t, "Run `python3 greet.py` and tell me what it printed.")


def approval_denied(s: AppServer, cwd: str):
    """A command needs approval and the user declines it."""
    s.request_handler = answer_with("decline")
    s.initialize()
    t = s.start_thread(cwd, approval="untrusted", sandbox="read-only")
    s.turn(t, "Run `python3 greet.py` and tell me what it printed. If you are not allowed, say so and stop.")


def file_approval(s: AppServer, cwd: str):
    """A file change needs approval and the user allows it."""
    s.request_handler = answer_with("accept")
    s.initialize()
    t = s.start_thread(cwd, approval="untrusted", sandbox="read-only")
    s.turn(t, "Add a one-line comment at the top of greet.py that says `# demo`. Do nothing else.")


def question(s: AppServer, cwd: str):
    """The agent asks the user a multiple-choice question."""
    s.request_handler = answer_with("accept")
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(
        t,
        "Before doing anything, ask me one multiple-choice question with your request_user_input tool: "
        "which greeting should greet.py use, 'Hello' or 'Hi'? Then tell me which I chose. Do not edit files.",
    )


def interrupt(s: AppServer, cwd: str):
    """The user interrupts a turn while a command is running."""
    s.initialize()
    t = s.start_thread(cwd)
    turn_id, mark = s.start_turn(t, "Run `sleep 60` and then tell me it finished.")
    s.wait_for(lambda m: is_item(m, "item/started", "commandExecution"), timeout=120, start=mark)
    time.sleep(2)
    s.request("turn/interrupt", {"threadId": t, "turnId": turn_id})
    s.finish_turn(mark, timeout=60)


def steer(s: AppServer, cwd: str):
    """The user sends a new message while a turn is running."""
    s.initialize()
    t = s.start_thread(cwd)
    turn_id, mark = s.start_turn(t, "Run `sleep 8`, then tell me the name of the function defined in greet.py.")
    s.wait_for(lambda m: is_item(m, "item/started", "commandExecution"), timeout=120, start=mark)
    s.request(
        "turn/steer",
        {"threadId": t, "expectedTurnId": turn_id, "input": [text_input("Also tell me how many lines greet.py has.")]},
    )
    s.finish_turn(mark)


def failing_command(s: AppServer, cwd: str):
    """A command exits with an error."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(t, "Run `python3 -m pytest -q test_greet.py` once and report the result in one sentence. Do not fix anything.")


def two_turns(s: AppServer, cwd: str):
    """Two turns in one thread."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(t, "Remember the word 'marmot'. Reply with: Noted.")
    s.turn(t, "What word did I ask you to remember? Reply with just the word.")


SCENARIOS = {
    f.__name__: f
    for f in [
        simple,
        command,
        file_edit,
        plan,
        approval_allowed,
        approval_denied,
        file_approval,
        question,
        interrupt,
        steer,
        failing_command,
        two_turns,
    ]
}


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "--list":
        for name, f in SCENARIOS.items():
            print(f"{name}: {f.__doc__}")
        return
    if len(sys.argv) != 4 or sys.argv[1] not in SCENARIOS:
        sys.exit(__doc__)
    name, cwd, out = sys.argv[1], str(Path(sys.argv[2]).resolve()), Path(sys.argv[3])
    s = AppServer(out)
    try:
        SCENARIOS[name](s, cwd)
        time.sleep(1.0)  # let trailing notifications arrive
        print(f"{name}: ok")
    except Exception as e:  # noqa: BLE001 - report and keep what was recorded
        print(f"{name}: FAILED: {type(e).__name__}: {e}")
    finally:
        s.close()


if __name__ == "__main__":
    main()
