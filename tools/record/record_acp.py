#!/usr/bin/env python3
"""Record sessions with an agent that speaks the Agent Client Protocol (ACP): every JSON message in both directions.

Usage:
    record_acp.py <scenario> <project-dir> <output.jsonl> [--agent "opencode acp"] [--home DIR] [--config JSON]

For Goose, in a container: --agent "tools/goose/goose-acp.sh <model-key-file>".
For Cursor, on its own sign-in:
    --agent "cursor-agent acp" --agent-auto "cursor-agent --force acp" --own-home
    record_acp.py --list

Each output line is {"t": <ms since start>, "dir": "out"|"in", "msg": <the JSON message>}.
"out" is what this client sent to the agent; "in" is what the agent sent back. The recorder's own
markers are "out" lines whose message is {"_recorder": "..."}.

The recorder plays an ACP client over the agent's standard input and output, as an editor would. It
says it can neither read and write files nor run terminals for the agent, so the agent does both
itself and asks the client only for permission.

The agent runs with HOME and the XDG folders pointed at a throwaway folder, so it reads none of your
own settings, skills, plugins or sign-ins, and its sessions stay out of your history. It gets no other
variable of yours either, apart from PATH and the locale. Give `--home` to keep that folder between
runs; otherwise a new one is made beside the project folder and removed. `--config` is passed to
OpenCode as OPENCODE_CONFIG_CONTENT; the default picks a free model.

The script has no dependencies. Raw recordings contain local paths. Scrub them before sharing.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

TURN_TIMEOUT = 240
PROTOCOL_VERSION = 1
# The cheapest OpenCode model that worked on 2026-11-21: a free one, which needs no account.
DEFAULT_CONFIG = {"model": "opencode/deepseek-v4.1-flash-free"}
# Approval scenarios: OpenCode asks before every shell command and every edit.
ASK = {"permission": {"bash": "ask", "edit": "ask"}}
# The only variables the agent gets from the recorder's own environment. Everything else it is given
# is set below: its throwaway home and its configuration. Nothing else of the machine's reaches it.
# MODEL_* choose the model of an agent run in a container, such as Goose by `tools/goose/goose-acp.sh`.
PASSED_THROUGH = ("PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "MODEL_HOST", "MODEL_BASE_PATH", "MODEL_NAME")


class Agent:
    def __init__(self, out_path: Path, command: list[str], env: dict, cwd: str):
        self.out = out_path.open("w", encoding="utf-8")
        self.t0 = time.monotonic()
        self.lock = threading.Lock()
        self.cond = threading.Condition()
        self.inbox: list[dict] = []  # notifications and agent requests, in order
        self.responses: dict[int, dict] = {}
        self.next_id = 0
        self.request_handler = None  # callable(msg) -> result dict, or None to leave unanswered
        self.command, self.env, self.cwd = command, env, cwd
        self._spawn()

    def _spawn(self):
        self.proc = subprocess.Popen(
            self.command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            env=self.env,
            cwd=self.cwd,
        )
        threading.Thread(target=self._read, args=(self.proc,), daemon=True).start()

    def restart(self, hard: bool = False):
        """Stop the agent and start a fresh one, writing to the same recording. A hard restart kills
        the process without warning, as a crash would."""
        how = "killed" if hard else "stopped"
        self._record("out", {"_recorder": f"the agent process was {how} and a new one started"})
        try:
            if hard:
                self.proc.kill()
            else:
                self.proc.stdin.close()
                self.proc.terminate()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()
        with self.cond:
            self.responses.clear()
        self._spawn()

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

    def _read(self, proc):
        for line in proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                self._record("in", {"_unparsed": line})
                continue
            if proc is not self.proc:
                continue  # a stopped agent's last words
            self._record("in", msg)
            if "method" not in msg and "id" in msg:
                with self.cond:
                    self.responses[msg["id"]] = msg
                    self.cond.notify_all()
                continue
            if "id" in msg:
                result = self.request_handler(msg) if self.request_handler else None
                if result is not None:
                    self._send({"jsonrpc": "2.0", "id": msg["id"], "result": result})
            with self.cond:
                self.inbox.append(msg)
                self.cond.notify_all()

    # -- client API ---------------------------------------------------------

    def send_request(self, method: str, params: dict) -> int:
        rid = self.next_id
        self.next_id += 1
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        return rid

    def wait_response(self, rid: int, timeout: float = 60) -> dict:
        deadline = time.monotonic() + timeout
        with self.cond:
            while rid not in self.responses:
                left = deadline - time.monotonic()
                if left <= 0:
                    raise TimeoutError(f"no response to request {rid}")
                self.cond.wait(left)
            return self.responses.pop(rid)

    def request(self, method: str, params: dict, timeout: float = 60) -> dict:
        return self.wait_response(self.send_request(method, params), timeout)

    def notify(self, method: str, params: dict):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

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
            self.proc.terminate()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()
        self.out.close()

    # -- helpers ------------------------------------------------------------

    def initialize(self, **capabilities) -> dict:
        r = self.request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": {"readTextFile": False, "writeTextFile": False},
                    "terminal": False,
                    **capabilities,
                },
                "clientInfo": {"name": "marmot-agents-recorder", "title": "Recorder", "version": "0.0.0"},
            },
        )
        return r.get("result") or {}

    def new_session(self, cwd: str) -> str:
        r = self.request("session/new", {"cwd": cwd, "mcpServers": []})
        return r["result"]["sessionId"]

    def start_prompt(self, sid: str, text: str) -> tuple[int, int]:
        mark = len(self.inbox)
        rid = self.send_request("session/prompt", {"sessionId": sid, "prompt": [{"type": "text", "text": text}]})
        return rid, mark

    def prompt(self, sid: str, text: str, timeout: float = TURN_TIMEOUT) -> dict:
        rid, _ = self.start_prompt(sid, text)
        return self.wait_response(rid, timeout)


def update_of(msg: dict, kind: str) -> dict | None:
    """The update a session/update notification carries, if it is of this kind."""
    if msg.get("method") != "session/update":
        return None
    u = (msg.get("params") or {}).get("update") or {}
    return u if u.get("sessionUpdate") == kind else None


def answer_with(kind: str):
    """A request handler that answers each permission request with the first option of this kind."""

    def handler(msg: dict):
        if msg.get("method") == "elicitation/create":
            # A form: each field that offers choices gets its first, as a person picking the first
            # option. A free-text field is left empty.
            schema = (msg.get("params") or {}).get("requestedSchema") or {}
            content = {}
            for name, field in (schema.get("properties") or {}).items():
                choices = field.get("enum") or [o.get("const") for o in field.get("oneOf") or []]
                items = field.get("items") or {}
                many = items.get("enum") or [o.get("const") for o in items.get("anyOf") or items.get("oneOf") or []]
                if choices:
                    content[name] = choices[0]
                elif many:
                    content[name] = [many[0]]
            return {"action": "accept", "content": content}
        if msg.get("method") != "session/request_permission":
            return None
        options = (msg.get("params") or {}).get("options") or []
        chosen = next((o for o in options if o.get("kind") == kind), None)
        if chosen is None:
            return {"outcome": {"outcome": "cancelled"}}
        return {"outcome": {"outcome": "selected", "optionId": chosen["optionId"]}}

    return handler


# -- scenarios ----------------------------------------------------------------
# Each takes the agent and the project folder. A scenario that needs OpenCode configured differently
# says so in its `config` attribute.


def simple(a: Agent, cwd: str):
    """One turn with a short text answer and no tools."""
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "Reply with exactly: Hello from the recording.")


def command(a: Agent, cwd: str):
    """One turn that runs a shell command and reports its output."""
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "Run `python3 greet.py` and tell me what it printed.")


def file_edit(a: Agent, cwd: str):
    """One turn that edits a file, with no approval needed."""
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "In greet.py, make greet() end its result with an exclamation mark. Then run `python3 greet.py`.")


def plan(a: Agent, cwd: str):
    """One turn where the agent keeps a to-do list while it works."""
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(
        s,
        "Use your todo list tool to write a three-step plan, then carry it out, updating the list as you go: "
        "1) read greet.py, 2) read test_greet.py, 3) tell me in one sentence whether the test would pass.",
    )


def approval_allowed(a: Agent, cwd: str):
    """A command needs approval and the user allows it once."""
    a.request_handler = answer_with("allow_once")
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "Run `python3 greet.py` and tell me what it printed.")


approval_allowed.config = ASK
approval_allowed.ask = True


def approval_denied(a: Agent, cwd: str):
    """A command needs approval and the user rejects it."""
    a.request_handler = answer_with("reject_once")
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "Run `python3 greet.py` and tell me what it printed. If you are not allowed, say so and stop.")


approval_denied.config = ASK
approval_denied.ask = True


def interrupt(a: Agent, cwd: str):
    """The user cancels a turn while a command is running."""
    a.initialize()
    s = a.new_session(cwd)
    rid, mark = a.start_prompt(s, "Run `sleep 60` and then tell me it finished.")
    # A tool call has started: OpenCode says `in_progress`; Goose sends the call with no status at all.
    a.wait_for(lambda m: update_of(m, "tool_call") is not None
               or (update_of(m, "tool_call_update") or {}).get("status") == "in_progress",
               timeout=120, start=mark)
    time.sleep(2)
    a.notify("session/cancel", {"sessionId": s})
    a.wait_response(rid, timeout=60)


def question(a: Agent, cwd: str):
    """The agent is asked to put a multiple-choice question to the user with its question tool. The client
    says it can show forms, which OpenCode needs before it asks."""
    a.request_handler = answer_with("allow_once")
    a.initialize(elicitation={"form": {}})
    s = a.new_session(cwd)
    a.prompt(
        s,
        "Before doing anything, ask me one multiple-choice question with your question tool: which greeting "
        "should greet.py use, 'Hello' or 'Hi'? Then tell me which I chose. Do not edit files.",
        timeout=120,
    )


def failing_command(a: Agent, cwd: str):
    """A command exits with an error."""
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "Run `python3 -m pytest -q test_greet.py` once and report the result in one sentence. Do not fix anything.")


def new_session(a: Agent, cwd: str):
    """A second session for the same project on the same connection, without resuming the first."""
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "Remember the word 'marmot'. Reply with: Noted.")
    s2 = a.new_session(cwd)
    a.prompt(s2, "What word did I ask you to remember? If you do not know, reply with: I don't know.")


def resume(a: Agent, cwd: str):
    """A session is resumed with session/resume after the agent restarts. The agent replays nothing."""
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "Remember the word 'marmot'. Reply with: Noted.")
    a.restart()
    a.initialize()
    a.request("session/resume", {"sessionId": s, "cwd": cwd, "mcpServers": []})
    a.prompt(s, "What word did I ask you to remember? Reply with just the word.")


def load(a: Agent, cwd: str):
    """A session is loaded with session/load after the agent restarts. The agent replays the conversation."""
    a.initialize()
    s = a.new_session(cwd)
    a.prompt(s, "Run `python3 greet.py`, then reply with what it printed.")
    a.restart()
    a.initialize()
    a.request("session/load", {"sessionId": s, "cwd": cwd, "mcpServers": []})
    a.prompt(s, "What did the command print? Reply with just its output.")


SCENARIOS = {
    f.__name__: f
    for f in [
        simple,
        command,
        file_edit,
        plan,
        approval_allowed,
        approval_denied,
        interrupt,
        question,
        failing_command,
        new_session,
        resume,
        load,
    ]
}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("scenario", nargs="?")
    ap.add_argument("project", nargs="?")
    ap.add_argument("output", nargs="?", type=Path)
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--agent", default="opencode acp", help="the command that starts the agent")
    ap.add_argument("--home", help="the throwaway folder the agent uses as HOME; kept after the run")
    ap.add_argument("--config", help="OpenCode configuration as JSON, merged over the scenario's")
    ap.add_argument("--agent-auto", help="the command for scenarios that need no approval, for an agent that "
                                         "asks unless started otherwise, such as `cursor-agent --force acp`")
    ap.add_argument("--own-home", action="store_true",
                    help="run the agent with your own HOME, for one signed in through its own command, such as "
                         "Cursor; its sessions then go into your history")
    args = ap.parse_args()
    if args.list:
        for name, f in SCENARIOS.items():
            print(f"{name}: {f.__doc__}")
        return
    if not (args.scenario in SCENARIOS and args.project and args.output):
        sys.exit(__doc__)
    cwd = str(Path(args.project).resolve())
    # Beside the project, so the scrubber's map for the work folder covers the paths the agent
    # writes into its answers, such as where it kept a command's output.
    home = Path(args.home).resolve() if args.home else Path(tempfile.mkdtemp(prefix="acp-home-", dir=Path(cwd).parent))
    f = SCENARIOS[args.scenario]
    config = {**DEFAULT_CONFIG, **getattr(f, "config", {}), **(json.loads(args.config) if args.config else {})}
    homes = {"HOME": os.environ.get("HOME", "")} if args.own_home else {
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_DATA_HOME": str(home / ".local/share"),
        "XDG_STATE_HOME": str(home / ".local/state"),
        "XDG_CACHE_HOME": str(home / ".cache"),
    }
    env = {
        **{k: os.environ[k] for k in PASSED_THROUGH if k in os.environ},
        **homes,
        "OPENCODE_CONFIG_CONTENT": json.dumps(config),
        "OPENCODE_DISABLE_AUTOUPDATE": "1",
        # For an agent without OpenCode's configuration: ask before every tool. Goose's wrapper reads it.
        **({"RECORD_ASK": "1"} if getattr(f, "ask", False) else {}),
    }
    command = args.agent_auto if args.agent_auto and not getattr(f, "ask", False) else args.agent
    a = Agent(args.output, shlex.split(command), env, cwd)
    try:
        f(a, cwd)
        time.sleep(1.0)  # let trailing notifications arrive
        print(f"{args.scenario}: ok")
    except Exception as e:  # noqa: BLE001 - report and keep what was recorded
        print(f"{args.scenario}: FAILED: {type(e).__name__}: {e}")
    finally:
        a.close()
        if not args.home:
            shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
