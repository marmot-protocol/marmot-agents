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
        self._spawn()

    def _spawn(self):
        self.proc = subprocess.Popen(
            ["codex", "app-server", "--disable", "hooks"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        threading.Thread(target=self._read, args=(self.proc,), daemon=True).start()

    def restart(self, hard: bool = False):
        """Stop the app server and start a fresh one, writing to the same recording.

        A hard restart kills the process without warning, as a crash would.
        """
        how = "killed" if hard else "stopped"
        self._record("out", {"_recorder": f"the app server process was {how} and a new one started"})
        try:
            if hard:
                self.proc.kill()
            else:
                self.proc.stdin.close()
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

    def start_thread(self, cwd: str, approval="never", sandbox="workspace-write", ephemeral=True, **extra) -> str:
        r = self.request(
            "thread/start",
            {"cwd": cwd, "approvalPolicy": approval, "sandbox": sandbox, "ephemeral": ephemeral, **extra},
        )
        self.model = r["result"].get("model")
        return r["result"]["thread"]["id"]

    def start_turn(self, thread_id: str, text: str, inputs: list | None = None, **extra) -> tuple[str, int]:
        mark = len(self.inbox)
        r = self.request(
            "turn/start",
            {"threadId": thread_id, "effort": "low", "input": [*(inputs or []), text_input(text)], **extra},
        )
        if "error" in r:
            return "", mark
        return r["result"]["turn"]["id"], mark

    def finish_turn(self, mark: int, timeout: float = TURN_TIMEOUT) -> dict:
        _, msg = self.wait_for(lambda m: m.get("method") == "turn/completed", timeout=timeout, start=mark)
        return msg

    def turn(self, thread_id: str, text: str, **extra) -> dict | None:
        turn_id, mark = self.start_turn(thread_id, text, **extra)
        if not turn_id:
            return None  # the request itself was refused; the error response is in the recording
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
        if method == "mcpServer/elicitation/request":
            return {"action": "accept", "content": {}}
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


def resume(s: AppServer, cwd: str):
    """A session is resumed after the app server restarts. Leaves one thread in the Codex history."""
    s.initialize()
    t = s.start_thread(cwd, ephemeral=False)
    s.turn(t, "Remember the word 'marmot'. Reply with: Noted.")
    s.restart()
    s.initialize()
    s.request("thread/resume", {"threadId": t, "cwd": cwd})
    s.turn(t, "What word did I ask you to remember? Reply with just the word.")


def plan_tool(s: AppServer, cwd: str):
    """The agent is told to call its plan tool."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(
        t,
        "First call the update_plan tool with exactly three steps: read greet.py, read test_greet.py, report. "
        "Then do each step, calling update_plan again to mark it completed. Finish with one sentence.",
        effort="medium",
    )


def plan_mode(s: AppServer, cwd: str):
    """Plan mode: the agent may ask questions and proposes a plan instead of acting."""
    s.request_handler = answer_with("accept")
    s.initialize()
    t = s.start_thread(cwd)
    mode = {"mode": "plan", "settings": {"model": s.model, "reasoning_effort": "low", "developer_instructions": None}}
    s.turn(
        t,
        "I want a farewell() function added to greet.py. Ask me which farewell word to use, offering "
        "'Goodbye' and 'Bye', then give me your plan. Do not edit any files.",
        collaborationMode=mode,
    )


def reasoning(s: AppServer, cwd: str):
    """A turn at high effort with detailed reasoning summaries."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(
        t,
        "Without running any commands: how many times does the letter r appear in 'strawberry marmot burrow'? "
        "Work it out carefully, then give the number.",
        effort="high",
        summary="detailed",
    )


def web_search(s: AppServer, cwd: str):
    """The agent searches the web."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(t, "Search the web for the latest stable release of the Rust programming language and tell me its version number.")


def image(s: AppServer, cwd: str):
    """The user attaches an image."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(
        t,
        "What colour is the attached image? Answer with one word.",
        inputs=[{"type": "localImage", "path": str(Path(cwd) / "red.png")}],
    )


def mcp_tool(s: AppServer, cwd: str):
    """The agent calls a tool on an MCP server."""
    s.request_handler = answer_with("accept")
    server = str(Path(__file__).resolve().parent / "demo_mcp_server.py")
    s.initialize()
    # With approval policy "never", Codex refuses the call outright: an MCP tool call needs approval.
    t = s.start_thread(cwd, approval="untrusted", config={"mcp_servers": {"demo": {"command": "python3", "args": [server]}}})
    s.turn(t, "Use the define_word tool from the demo MCP server to define 'marmot', then tell me the definition.")


def compact(s: AppServer, cwd: str):
    """The client asks for the conversation to be compacted between two turns."""
    s.initialize()
    t = s.start_thread(cwd)
    s.turn(t, "Remember the word 'marmot'. Reply with: Noted.")
    mark = len(s.inbox)
    s.request("thread/compact/start", {"threadId": t})
    try:
        s.wait_for(lambda m: m.get("method") in ("thread/compacted", "turn/completed"), timeout=120, start=mark)
    except TimeoutError:
        pass
    time.sleep(2)
    s.turn(t, "What word did I ask you to remember? Reply with just the word.")


def model_switch(s: AppServer, cwd: str):
    """The client lists the models and runs the second turn on a different one."""
    s.initialize()
    models = s.request("model/list", {}).get("result", {}).get("data", [])
    t = s.start_thread(cwd)
    s.turn(t, "Reply with exactly: first.")
    other = next((m.get("model") or m.get("id") for m in models if (m.get("model") or m.get("id")) != s.model), None)
    s.turn(t, "Reply with exactly: second.", **({"model": other} if other else {}))


def bad_model(s: AppServer, cwd: str):
    """A turn is started with a model that does not exist."""
    s.initialize()
    t = s.start_thread(cwd)
    _, mark = s.start_turn(t, "Reply with exactly: hello.", model="no-such-model")
    try:
        s.finish_turn(mark, timeout=60)
    except TimeoutError:
        pass


def approval_unanswered(s: AppServer, cwd: str):
    """A command needs approval and nobody answers. After six minutes the client interrupts."""
    s.request_handler = lambda msg: None  # leave every request from the harness unanswered
    s.initialize()
    t = s.start_thread(cwd, approval="untrusted", sandbox="read-only")
    turn_id, mark = s.start_turn(t, "Run `python3 greet.py` and tell me what it printed.")
    s.wait_for(lambda m: str(m.get("method", "")).endswith("/requestApproval"), timeout=180, start=mark)
    try:
        s.finish_turn(mark, timeout=360)  # does the harness give up on its own?
        return
    except TimeoutError:
        pass
    s._record("out", {"_recorder": "six minutes passed with the approval unanswered; the client interrupts"})
    s.request("turn/interrupt", {"threadId": t, "turnId": turn_id})
    s.finish_turn(mark, timeout=60)


def restart_mid_turn(s: AppServer, cwd: str):
    """The app server is killed while a command is running, then the thread is resumed.
    Leaves one thread in the Codex history."""
    s.initialize()
    t = s.start_thread(cwd, ephemeral=False)
    _, mark = s.start_turn(t, "Run `sleep 45 && echo finished` and then tell me what it printed.")
    s.wait_for(lambda m: is_item(m, "item/started", "commandExecution"), timeout=120, start=mark)
    time.sleep(3)
    s.restart(hard=True)
    s.initialize()
    s.request("thread/resume", {"threadId": t, "cwd": cwd})
    time.sleep(5)
    s.turn(t, "You were stopped and restarted. What happened to the command you were running? Answer in one sentence.")


def settings_update(s: AppServer, cwd: str):
    """The client changes the thread's settings between turns with thread/settings/update: first the
    model, then the approval policy and the sandbox. The command after that needs approval."""
    s.request_handler = answer_with("accept")
    s.initialize()
    models = s.request("model/list", {}).get("result", {}).get("data", [])
    s.request("collaborationMode/list", {})
    t = s.start_thread(cwd)
    s.turn(t, "Reply with exactly: first.")
    other = next((m.get("model") or m.get("id") for m in models if (m.get("model") or m.get("id")) != s.model), None)
    if other:
        s.request("thread/settings/update", {"threadId": t, "model": other})
        time.sleep(3)  # does Codex report the change before any prompt is sent?
    s.request(
        "thread/settings/update",
        {"threadId": t, "approvalPolicy": "untrusted", "sandboxPolicy": {"type": "readOnly", "networkAccess": False}},
    )
    time.sleep(3)
    s.turn(t, "Run `python3 greet.py` and tell me what it printed.")


def settings_mid_turn(s: AppServer, cwd: str):
    """The client changes the reasoning effort of a turn while it is running, with turn/settings/update."""
    s.initialize()
    t = s.start_thread(cwd)
    turn_id, mark = s.start_turn(t, "Run `sleep 8 && echo finished` and then tell me what it printed.")
    s.wait_for(lambda m: is_item(m, "item/started", "commandExecution"), timeout=120, start=mark)
    s.request("turn/settings/update", {"threadId": t, "turnId": turn_id, "effort": "medium"})
    s.finish_turn(mark)


def new_session(s: AppServer, cwd: str):
    """A second thread is started for the same project, after the client lets go of the first."""
    s.initialize()
    a = s.start_thread(cwd)
    s.turn(a, "Remember the word 'marmot'. Reply with: Noted.")
    s.request("thread/unsubscribe", {"threadId": a})
    time.sleep(2)
    b = s.start_thread(cwd)
    s.turn(b, "What word did I ask you to remember? If you do not know, say so in one sentence.")


def resume_with_settings(s: AppServer, cwd: str):
    """A session is resumed after the app server restarts, and the client sends the approval policy
    and the sandbox again with the resume. Leaves one thread in the Codex history."""
    s.initialize()
    t = s.start_thread(cwd, ephemeral=False)
    s.turn(t, "Remember the word 'marmot'. Reply with: Noted.")
    s.restart()
    s.initialize()
    s.request("thread/resume", {"threadId": t, "cwd": cwd, "approvalPolicy": "never", "sandbox": "workspace-write"})
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
        resume,
        plan_tool,
        plan_mode,
        reasoning,
        web_search,
        image,
        mcp_tool,
        compact,
        model_switch,
        bad_model,
        approval_unanswered,
        restart_mid_turn,
        settings_update,
        settings_mid_turn,
        new_session,
        resume_with_settings,
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
