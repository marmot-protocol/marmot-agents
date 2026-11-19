#!/usr/bin/env python3
"""Design prototype: translate a recorded harness session into the proposed agent events.

This is not driver code. It exists to test the event model in docs/design/event-model.md
against real recordings: every field in an agent event has to come from somewhere, and every
native message has to be either translated or deliberately not sent.

Usage:
    translate.py <recording.jsonl>             print the agent events, one JSON object per line
    translate.py <recording.jsonl> --brief     the same, without ids and native payloads
    translate.py --check <recordings-dir>...   translate every recording and report coverage

The check also reports the session items, and which native call each control was seen as.

An agent event:

    {
      "v": 0,                      draft version of this shape
      "harness": "codex",          which harness produced it
      "session": "...",            the harness's session or thread id
      "turn": "...",               the turn it belongs to
      "item": "...",               stable id; a later event with the same id replaces this one
      "seq": 7,                    order within the session
      "type": "tool",              one of ITEM_TYPES
      "state": "running",          one of STATES
      "title": "Running a command",
      "text": "python3 greet.py",  the plain rendering, readable on its own
      "body": {...},               fields specific to the item type
      "native": {"kind": "...", "payload": {...}}
    }

It has no dependencies.
"""

from __future__ import annotations

import collections
import json
import sys
from pathlib import Path

ITEM_TYPES = [
    "turn",  # a turn started or ended
    "message",  # text the agent says
    "reasoning",  # the agent's thinking
    "tool",  # a tool the agent used; body.kind says which sort
    "approval",  # the agent wants permission for one action
    "question",  # the agent asks the person something
    "plan",  # the agent's plan or to-do list
    "task",  # work that runs beside or beyond the turn: a subagent, a background job
    "notice",  # compaction, waiting, a retry, a model change
    "error",  # something failed
    "session",  # a session with the harness: its settings, its own actions and its slash commands
]
STATES = ["running", "waiting", "done", "failed", "declined", "cancelled"]
TOOL_KINDS = ["command", "edit", "read", "search", "fetch", "mcp", "subagent", "other"]

MAX_TEXT = 600
MAX_NATIVE = 4000


def clip(s, n=MAX_TEXT):
    s = "" if s is None else str(s)
    return s if len(s) <= n else s[: n - 1] + "…"


def clip_native(payload):
    """Keep the native payload whole when it is small; otherwise say how big it was."""
    raw = json.dumps(payload, ensure_ascii=False)
    if len(raw) <= MAX_NATIVE:
        return payload
    return {"_truncated": True, "_bytes": len(raw.encode("utf-8"))}


class Translator:
    harness = "?"

    def __init__(self):
        self.events: list[dict] = []
        self.session = None
        self.turn = None
        self.seq = 0
        self.open: dict[str, dict] = {}  # item id -> last event, for items not yet finished
        self.not_sent = collections.Counter()
        self.unmapped = collections.Counter()
        self.sessions: dict[str, str] = {}  # session item -> its body as last sent
        self.controls = collections.Counter()  # (control, the native call it was seen as)

    def emit(self, item, type_, state, title, text="", body=None, native_kind="", native=None):
        assert type_ in ITEM_TYPES and state in STATES, (type_, state)
        self.seq += 1
        ev = {
            "v": 0,
            "harness": self.harness,
            "session": self.session,
            "turn": self.turn,
            "item": item,
            "seq": self.seq,
            "type": type_,
            "state": state,
            "title": title,
            "text": clip(text),
            "body": body or {},
            "native": {"kind": native_kind, "payload": clip_native(native)},
        }
        self.events.append(ev)
        if state in ("running", "waiting"):
            self.open[item] = ev
        else:
            self.open.pop(item, None)
        return ev

    def close_open_items(self, reason):
        """A turn ended. Anything still running will never report again, so close it."""
        for item, ev in list(self.open.items()):
            if ev["type"] in ("turn", "task", "session"):
                continue  # tasks and sessions may outlive the turn; the turn closes itself
            self.emit(
                item, ev["type"], "cancelled", ev["title"], ev["text"], ev["body"],
                native_kind="(none: closed by the driver)", native={"reason": reason},
            )

    def session_item(self, sid, settings, native_kind, native, actions=(), commands=()):
        """A session started, or something about it changed. Sent only when the body differs from the
        last one sent, so a harness that repeats its settings on every turn produces no edits.

        A setting is {"id", "label", "value", "options", "approve"}. "options" is None when the
        harness does not say what it accepts. "approve" marks a setting that decides what the agent
        may do without asking, which needs the right to approve (D-032)."""
        item = f"session:{sid}"
        body = {"settings": settings, "actions": list(actions), "commands": list(commands)}
        key = json.dumps(body, sort_keys=True)
        if self.sessions.get(item) == key:
            return
        self.sessions[item] = key
        text = ", ".join(f"{x['label']}: {x['value']}" for x in settings if x.get("value") is not None)
        turn, self.turn = self.turn, None  # a session belongs to no turn
        self.emit(item, "session", "running", "Session", text, body, native_kind, native)
        self.turn = turn

    def end_session(self, sid, reason):
        """Nothing a harness sent in any recording says a session is over. The driver ends the item
        itself, when the chat's session is replaced."""
        item = f"session:{sid}"
        prev = self.open.get(item)
        if prev is None:
            return
        turn, self.turn = self.turn, None
        self.emit(item, "session", "done", "Session ended", prev["text"], prev["body"],
                  "(none: closed by the driver)", {"reason": reason})
        self.turn = turn

    @staticmethod
    def setting(id_, label, value, options=None, approve=False):
        return {"id": id_, "label": label, "value": value, "options": options, "approve": approve}

    def control(self, name, native):
        self.controls[(name, native)] += 1

    def skip(self, kind):
        self.not_sent[kind] += 1

    def unknown(self, kind):
        self.unmapped[kind] += 1


# ---------------------------------------------------------------------------- Codex


class Codex(Translator):
    harness = "codex"
    NOT_SENT = {
        "initialized", "thread/started", "thread/status/changed", "thread/settings/updated",
        "thread/tokenUsage/updated", "account/updated", "account/rateLimits/updated",
        "remoteControl/status/changed", "mcpServer/startupStatus/updated", "turn/moderationMetadata",
        "turn/diff/updated", "serverRequest/resolved", "skills/changed", "thread/name/updated",
        "thread/closed", "thread/environment/connected", "model/verification",
        "thread/readState/changed", "thread/goal/cleared", "thread/goal/updated", "deprecationNotice",
    }

    def __init__(self):
        super().__init__()
        self.pending: dict = {}  # JSON-RPC request id -> what the client asked or was asked
        self.usage = None
        self.last_request = None  # the client's most recent request, to tell who started a turn
        self.settings: dict = {}  # what Codex has said about the thread's settings, merged
        self.models = None  # model ids, once the client has asked for the list
        self.modes = None  # collaboration modes, once the client has asked for the list

    def feed(self, rec):
        m, out = rec["msg"], rec["dir"] == "out"
        if "_recorder" in m:
            if "new one started" in m["_recorder"]:
                self.harness_stopped()
            return
        method = m.get("method")
        if out:
            if method:
                self.pending[("c", m.get("id"))] = method
                if "id" in m:
                    self.last_request = method
                p = m.get("params") or {}
                if method == "turn/interrupt":
                    self.control("interrupt", "turn/interrupt")
                elif method == "thread/start":
                    self.control("new_session", "thread/start")
                elif method == "thread/resume":
                    self.control("(resume a session)", "thread/resume")
                elif method == "thread/compact/start":
                    self.control("(action) compact", "thread/compact/start")
                elif method in ("thread/settings/update", "turn/settings/update"):
                    for k in sorted(set(p) - {"threadId", "turnId"}):
                        self.control(f"set {k}", method)
                elif method == "turn/start":
                    for k in ("model", "collaborationMode", "approvalPolicy", "sandboxPolicy"):
                        if k in p:
                            self.control(f"set {k}", f"turn/start with {k}")
                elif method == "turn/steer":
                    self.control("(message during a turn)", "turn/steer")
                if method == "turn/steer":
                    return self.skip("client: turn/steer")  # the harness echoes it as a userMessage
                return self.skip(f"client: {method}")
            return self.answer(m)  # the client's response to a server request
        if method is None:
            return self.response(m)
        p = m.get("params") or {}
        if "delta" in method.lower():
            return self.skip("partial text and output (…/delta)")
        if method in self.NOT_SENT:
            if method == "thread/tokenUsage/updated":
                self.usage = (p.get("tokenUsage") or {}).get("last")
            if method == "thread/started":
                self.use_thread((p.get("thread") or {}).get("id"))
                return self.thread_settings(method, p.get("thread") or {}, p)
            if method == "thread/settings/updated":
                return self.thread_settings(method, p.get("threadSettings") or {}, p)
            return self.skip(method)
        handler = {
            "turn/started": self.turn_started,
            "turn/completed": self.turn_completed,
            "item/started": self.item,
            "item/completed": self.item,
            "item/commandExecution/requestApproval": self.approval,
            "item/fileChange/requestApproval": self.approval,
            "item/permissions/requestApproval": self.approval,
            "item/tool/requestUserInput": self.user_input,
            "mcpServer/elicitation/request": self.elicitation,
            "turn/plan/updated": self.plan,
            "thread/compacted": self.compacted,
            "model/rerouted": self.rerouted,
            "error": self.error_notification,
            "warning": self.warning,
        }.get(method)
        if handler is None:
            return self.unknown(method)
        handler(method, m, p)

    def harness_stopped(self):
        """The harness process ended. Nothing it was doing will ever report again, so the driver
        closes the open items and the open turn itself."""
        self.close_open_items("the harness process ended")
        turn_item = f"turn:{self.turn}"
        if turn_item in self.open:
            prev = self.open[turn_item]
            self.emit(turn_item, "turn", "cancelled", "Interrupted", "The harness stopped before the turn finished.",
                      prev["body"], native_kind="(none: closed by the driver)", native={"reason": "the harness process ended"})
        self.pending.clear()

    # -- turns

    def turn_started(self, method, m, p):
        self.turn = (p.get("turn") or {}).get("id")
        self.started_by = "user" if self.last_request == "turn/start" else "control"
        self.emit(f"turn:{self.turn}", "turn", "running", "Working", body={"started_by": self.started_by},
                  native_kind=method, native=p)

    def turn_completed(self, method, m, p):
        turn = p.get("turn") or {}
        status = turn.get("status")
        self.close_open_items(f"the turn ended with status {status}")
        state = {"completed": "done", "interrupted": "cancelled", "failed": "failed"}.get(status, "done")
        title = {"done": "Finished", "cancelled": "Interrupted", "failed": "Failed"}[state]
        err = turn.get("error")
        body = {"started_by": getattr(self, "started_by", "user"), "duration_ms": turn.get("durationMs")}
        if self.usage:
            body["usage"] = {"input_tokens": self.usage.get("inputTokens"), "output_tokens": self.usage.get("outputTokens")}
        self.emit(f"turn:{turn.get('id')}", "turn", state, title, text=(err or {}).get("message", "") if err else "",
                  body=body, native_kind=method, native=p)
        self.usage = None

    # -- items

    def item(self, method, m, p):
        it = p.get("item") or {}
        kind, started = it.get("type"), method == "item/started"
        native_kind = f"{method} [{kind}]"
        iid = it.get("id")
        if kind == "userMessage":
            return self.skip("echo of the user's message (userMessage item)")
        if kind == "agentMessage":
            if started:
                return self.skip("start of a message, before it has text")
            phase = {"final_answer": "final", "commentary": "commentary"}.get(it.get("phase"), it.get("phase"))
            self.emit(iid, "message", "done", "Message", it.get("text"), {"phase": phase}, native_kind, it)
            if it.get("questions"):
                qs = [
                    {"id": f"q{n}", "text": q.get("title"), "multiple": False, "allow_text": True,
                     "options": [{"id": f"o{k}", "label": o} for k, o in enumerate(q.get("options") or [])]}
                    for n, q in enumerate(it["questions"])
                ]
                self.emit(f"{iid}:question", "question", "waiting", "Question", qs[0]["text"],
                          {"answer_via": "message", "questions": qs}, native_kind, it)
            return
        if kind == "reasoning":
            text = "\n".join(str(x.get("text", x) if isinstance(x, dict) else x) for x in (it.get("summary") or []) + (it.get("content") or []))
            if started or not text.strip():
                return self.skip("reasoning item with no text")
            return self.emit(iid, "reasoning", "done", "Thinking", text, {}, native_kind, it)
        if kind == "commandExecution":
            state = "running" if started else {"completed": "done", "failed": "failed", "declined": "declined"}.get(it.get("status"), "done")
            actions = [a.get("type") for a in it.get("commandActions") or []]
            tool_kind = "read" if actions and all(a == "read" for a in actions) else "search" if "search" in actions else "command"
            body = {"kind": tool_kind, "name": "shell", "command": it.get("command"), "cwd": it.get("cwd")}
            if not started:
                body.update(exit_code=it.get("exitCode"), duration_ms=it.get("durationMs"), output=clip(it.get("aggregatedOutput"), 2000))
            title = {"running": "Running a command", "done": "Ran a command", "failed": "Command failed", "declined": "Command not allowed"}[state]
            return self.emit(iid, "tool", state, title, it.get("command"), body, native_kind, it)
        if kind == "fileChange":
            state = "running" if started else {"completed": "done", "failed": "failed", "declined": "declined"}.get(it.get("status"), "done")
            changes = [{"path": c.get("path"), "change": (c.get("kind") or {}).get("type") if isinstance(c.get("kind"), dict) else c.get("kind"),
                        "diff": clip(c.get("diff"), 2000)} for c in it.get("changes") or []]
            title = {"running": "Editing files", "done": "Edited files", "failed": "Edit failed", "declined": "Edit not allowed"}[state]
            return self.emit(iid, "tool", state, title, ", ".join(c["path"] or "" for c in changes),
                             {"kind": "edit", "name": "apply_patch", "changes": changes}, native_kind, it)
        if kind == "mcpToolCall":
            state = "running" if started else {"completed": "done", "failed": "failed"}.get(it.get("status"), "done")
            name = f"{it.get('server')}.{it.get('tool')}"
            body = {"kind": "mcp", "name": name, "input": it.get("arguments")}
            if not started:
                body["output"] = clip(json.dumps(it.get("result"), ensure_ascii=False), 2000) if it.get("result") is not None else None
                body["error"] = it.get("error")
            return self.emit(iid, "tool", state, "Using a tool" if started else "Used a tool", name, body, native_kind, it)
        if kind == "webSearch":
            state = "running" if started else "done"
            return self.emit(iid, "tool", state, "Searching the web" if started else "Searched the web", it.get("query"),
                             {"kind": "search", "name": "web_search", "query": it.get("query")}, native_kind, it)
        if kind == "plan":
            if started:
                return self.skip("start of a plan, before it has text")
            return self.emit(iid, "plan", "done", "Plan", it.get("text"), {"text": it.get("text")}, native_kind, it)
        if kind == "sleep":
            state = "running" if started else "done"
            return self.emit(iid, "notice", state, "Waiting", f"Waiting up to {round((it.get('durationMs') or 0) / 1000)} seconds",
                             {"kind": "waiting", "duration_ms": it.get("durationMs")}, native_kind, it)
        if kind == "contextCompaction":
            return self.emit(iid, "notice", "running" if started else "done", "Compacting the conversation", "",
                             {"kind": "compaction"}, native_kind, it)
        if kind == "imageView":
            return self.emit(iid, "tool", "running" if started else "done", "Viewing an image", it.get("path"),
                             {"kind": "read", "name": "view_image", "path": it.get("path")}, native_kind, it)
        self.unknown(native_kind)

    # -- approvals and questions

    def approval(self, method, m, p):
        rid = m.get("id")
        offered = p.get("availableDecisions")
        choices, native_choices = [], {}
        for n, d in enumerate(offered or ["accept", "decline"]):
            key = d if isinstance(d, str) else next(iter(d))
            cid = f"c{n}"
            native_choices[cid] = d
            label = {"accept": "Allow once", "acceptForSession": "Allow for this session", "decline": "Deny", "cancel": "Deny and stop",
                     "acceptWithExecpolicyAmendment": "Allow, and don't ask again for this command"}.get(key, key)
            choices.append({"id": cid, "label": label, "effect": "allow" if key.startswith("accept") else "deny"})
        if not any(native_choices[c] == "decline" for c in native_choices):
            cid = f"c{len(choices)}"
            native_choices[cid] = "decline"  # accepted by Codex although it does not list it
            choices.append({"id": cid, "label": "Deny", "effect": "deny"})
        what = "command" if "commandExecution" in method else "file change" if "fileChange" in method else "permission"
        item = f"approval:{rid}"
        self.pending[("s", rid)] = {"item": item, "native_choices": native_choices, "subject": p.get("itemId"), "what": what, "choices": choices}
        self.emit(item, "approval", "waiting", f"Allow this {what}?", p.get("command") or p.get("reason") or "",
                  {"subject": p.get("itemId"), "choices": choices}, f"{method} (request)", p)

    def elicitation(self, method, m, p):
        """An MCP server asks the user for something. Codex also uses this to approve MCP tool calls."""
        rid, meta = m.get("id"), p.get("_meta") or {}
        if meta.get("codex_approval_kind") == "mcp_tool_call":
            native_choices = {"c0": "accept", "c1": "decline"}
            choices = [{"id": "c0", "label": "Allow once", "effect": "allow"}, {"id": "c1", "label": "Deny", "effect": "deny"}]
            item = f"approval:{rid}"
            self.pending[("s", rid)] = {"item": item, "native_choices": native_choices, "choices": choices, "decision_key": "action"}
            subject = next((i for i, ev in self.open.items() if ev["type"] == "tool" and ev["body"].get("kind") == "mcp"), None)
            return self.emit(item, "approval", "waiting", "Allow this tool?", p.get("message", ""),
                             {"subject": subject, "choices": choices}, f"{method} (request)", p)
        props = (p.get("requestedSchema") or {}).get("properties") or {}
        qs = [{"id": k, "text": v.get("description") or v.get("title") or k, "multiple": False, "allow_text": True, "options": []} for k, v in props.items()]
        item = f"question:{rid}"
        self.pending[("s", rid)] = {"item": item, "questions": qs}
        self.emit(item, "question", "waiting", "Question", p.get("message", ""),
                  {"answer_via": "request", "questions": qs}, f"{method} (request)", p)

    def user_input(self, method, m, p):
        rid = m.get("id")
        qs = [{"id": q.get("id"), "header": q.get("header"), "text": q.get("question"), "multiple": False,
               "allow_text": bool(q.get("isOther")), "secret": bool(q.get("isSecret")),
               "options": [{"id": f"o{k}", "label": o.get("label"), "description": o.get("description")} for k, o in enumerate(q.get("options") or [])]}
              for q in p.get("questions") or []]
        item = f"question:{rid}"
        self.pending[("s", rid)] = {"item": item, "questions": qs}
        self.emit(item, "question", "waiting", "Question", qs[0]["text"] if qs else "",
                  {"answer_via": "request", "subject": p.get("itemId"), "questions": qs}, f"{method} (request)", p)

    def answer(self, m):
        """The client answered a request from the harness."""
        req = self.pending.pop(("s", m.get("id")), None)
        if req is None:
            return self.skip("client: response")
        result = m.get("result") or {}
        prev = self.open.get(req["item"])
        if prev is None:
            return
        if prev["type"] == "approval":
            decision = result.get(req.get("decision_key", "decision"))
            chosen = next((c for c, d in req["native_choices"].items() if d == decision), None)
            label = next((c["label"] for c in req["choices"] if c["id"] == chosen), str(decision))
            effect = next((c["effect"] for c in req["choices"] if c["id"] == chosen), "deny")
            self.emit(req["item"], "approval", "done" if effect == "allow" else "declined", prev["title"], f"Answered: {label}",
                      {**prev["body"], "answer": {"choice": chosen}}, "response to the approval request", result)
        else:
            self.emit(req["item"], "question", "done", prev["title"], "Answered",
                      {**prev["body"], "answer": result.get("answers")}, "response to the question request", result)

    def response(self, m):
        method = self.pending.pop(("c", m.get("id")), None)
        if "error" in m:
            err = m["error"] or {}
            return self.emit(f"error:{m.get('id')}", "error", "failed", "The harness refused a request", err.get("message", ""),
                             {"request": method, "code": err.get("code")}, f"error response to {method}", err)
        result = m.get("result") or {}
        if method == "model/list":
            self.models = [x.get("id") for x in result.get("data") or []]
        if method == "collaborationMode/list":
            self.modes = [x.get("mode") for x in result.get("data") or []]
        if method in ("thread/start", "thread/resume") and isinstance(result.get("thread"), dict):
            self.use_thread(result["thread"].get("id"))
            return self.thread_settings(f"response to {method}", result, result)
        self.skip(f"response to {method}")

    def use_thread(self, thread_id):
        """A second thread replaces the first as this chat's session. Codex sends nothing to say the
        first is over, so the driver ends its session item."""
        if self.session and thread_id != self.session:
            self.end_session(self.session, "a new thread was started")
            self.settings = {}
        self.session = thread_id

    def thread_settings(self, native_kind, src, native):
        """Codex states a thread's settings in three places, under different names: the response to
        thread/start, thread/started, and thread/settings/updated. The driver merges them."""
        sandbox = src.get("sandboxPolicy") or src.get("sandbox")
        for key, value in {
            "model": src.get("model"),
            "effort": src.get("effort") or src.get("reasoningEffort"),
            "approval_policy": src.get("approvalPolicy"),
            "sandbox": sandbox.get("type") if isinstance(sandbox, dict) else sandbox,
            "mode": (src.get("collaborationMode") or {}).get("mode"),
        }.items():
            if value is not None:
                self.settings[key] = value
        g = self.settings.get
        before = len(self.events)
        self.session_item(self.session, [
            self.setting("model", "Model", g("model"), self.models),
            self.setting("effort", "Reasoning effort", g("effort")),
            self.setting("mode", "Mode", g("mode"), self.modes),
            self.setting("approval_policy", "Approval policy", g("approval_policy"), approve=True),
            self.setting("sandbox", "Sandbox", g("sandbox"), approve=True),
        ], native_kind, native, actions=[{"id": "compact", "label": "Compact the conversation"}])
        if len(self.events) == before:
            self.skip(f"{native_kind}, nothing new for the session item")

    # -- other

    def plan(self, method, m, p):
        entries = [{"text": e.get("step"), "state": {"pending": "pending", "inProgress": "running", "completed": "done"}.get(e.get("status"), e.get("status"))}
                   for e in p.get("plan") or []]
        text = "\n".join(f"[{'x' if e['state'] == 'done' else ' '}] {e['text']}" for e in entries)
        done = entries and all(e["state"] == "done" for e in entries)
        self.emit(f"plan:{p.get('turnId')}", "plan", "done" if done else "running", "Plan", text,
                  {"explanation": p.get("explanation"), "entries": entries}, method, p)

    def compacted(self, method, m, p):
        self.emit(f"compaction:{self.seq}", "notice", "done", "Conversation compacted", "", {"kind": "compaction"}, method, p)

    def rerouted(self, method, m, p):
        self.emit(f"model:{self.seq}", "notice", "done", "Model changed", f"{p.get('fromModel')} to {p.get('toModel')}",
                  {"kind": "model_changed", "reason": p.get("reason")}, method, p)

    def warning(self, method, m, p):
        self.emit(f"warning:{self.seq}", "notice", "done", "Warning", p.get("message", ""), {"kind": "warning"}, method, p)

    def error_notification(self, method, m, p):
        err = p.get("error") or {}
        self.emit(f"error:{self.seq}", "error", "failed", "Error", err.get("message", ""),
                  {"will_retry": p.get("willRetry")}, method, p)


# ---------------------------------------------------------------------------- Claude


class Claude(Translator):
    harness = "claude"
    TOOL_KINDS = {
        "Bash": "command", "Edit": "edit", "Write": "edit", "NotebookEdit": "edit", "Read": "read",
        "Grep": "search", "Glob": "search", "ToolSearch": "search", "WebSearch": "search",
        "WebFetch": "fetch", "Agent": "subagent", "Task": "subagent",
    }
    NOT_TOOLS = {"AskUserQuestion", "ExitPlanMode"}
    NOT_SENT_SYSTEM = {"thinking_tokens", "background_tasks_changed", "session_title_changed", "vcs_state_changed",
                       "code_change_published", "commands_changed", "hook_started", "hook_response"}

    def __init__(self):
        super().__init__()
        self.prompted = False  # a user prompt has been sent since the last result
        self.turn_n = 0
        self.tools: dict[str, dict] = {}  # tool_use id -> last body
        self.approvals: dict[str, str] = {}  # tool name -> open approval item (answers carry no id)
        self.settings: dict = {}
        self.commands: list = []
        self.models = None  # model names, once the client has asked for them
        self.session_key = None  # the driver's own id for the session item
        self.sessions_started = 0

    def feed(self, rec):
        m, via, out = rec["msg"], rec.get("via"), rec["dir"] == "out"
        if via == "note":
            return self.skip("recorder note")
        if via == "prompt":
            self.prompted = True
            content = (m.get("message") or {}).get("content")
            if isinstance(content, str) and content.startswith("/"):
                self.control("(slash command, as a message)", content.split()[0])
            if m.get("priority"):
                self.control("(message during a turn)", "a user message with priority")
            return self.skip("client: user message" + (" (steer)" if m.get("priority") else ""))
        if via == "call":
            if "error" in m:
                return self.skip("client library: query() threw at the end of a failed turn")
            method = m.get("method")
            if out:
                if method == "query":
                    resumed = "resume" in (m.get("options") or {})
                    self.control("(resume a session)" if resumed else "new_session",
                                 "query() with resume" if resumed else "query()")
                    if not resumed:
                        # Claude Code sends nothing to say the earlier session is over.
                        if self.session_key:
                            self.end_session(self.session_key, "a new session was started")
                        self.sessions_started += 1
                        self.session = self.session_key = None
                        self.settings, self.commands = {}, []
                elif method == "setPermissionMode":
                    self.control("set permission mode", "setPermissionMode()")
                elif method in ("interrupt", "abort"):
                    self.control("interrupt", f"{method}()")
                elif method == "setModel":
                    self.control("set model", "setModel()")
            elif method in ("supportedCommands", "supportedModels", "initializationResult") and "response" in m:
                # The client asked, which it can do before any prompt is sent.
                r = m["response"]
                r = {"commands": r} if method == "supportedCommands" else {"models": r} if method == "supportedModels" else r
                if isinstance(r.get("commands"), list):
                    self.commands = [{"name": c.get("name"), "description": c.get("description"), "hint": c.get("argumentHint")}
                                     for c in r["commands"]]
                if isinstance(r.get("models"), list):
                    self.models = [x.get("value") for x in r["models"]]
                if r.get("current_permission_mode"):
                    self.settings["permission_mode"] = r["current_permission_mode"]
                return self.announce(f"result of {method}()", {k: len(v) if isinstance(v, list) else v for k, v in r.items()
                                                                  if k in ("commands", "models", "current_permission_mode")})
            return self.skip(f"client: {method}()" if out else f"result of {method}()")
        if via == "callback":
            return self.permission_answer(m) if out else self.permission_request(m)
        t, st = m.get("type"), m.get("subtype")
        if t == "system":
            return self.system(st, m)
        if t == "assistant":
            return self.assistant(m)
        if t == "user":
            return self.user(m)
        if t == "result":
            return self.result(st, m)
        if t in ("stream_event", "rate_limit_event", "tool_progress", "command_lifecycle"):
            return self.skip({"stream_event": "partial text (stream_event)"}.get(t, t))
        self.unknown(f"{t}/{st}" if st else str(t))

    # -- turns

    def announce(self, native_kind, native):
        g = self.settings.get
        before = len(self.events)
        if self.session_key is None:  # the harness has not named the session yet
            self.session_key = self.session or f"not-yet-named:{self.sessions_started}"
        self.session_item(self.session_key, [
            self.setting("model", "Model", g("model"), self.models),
            self.setting("permission_mode", "Permission mode", g("permission_mode"), approve=True),
        ], native_kind, native, commands=self.commands)
        if len(self.events) == before:
            self.skip(f"{native_kind}, nothing new for the session item")

    def init(self, m):
        """Claude Code repeats this at the start of every turn. It is the only place the model, the
        permission mode and the names of the slash commands are stated."""
        self.session = m.get("session_id")
        self.settings.update({"model": m.get("model"), "permission_mode": m.get("permissionMode")})
        if not self.commands:  # the init message has names only; keep fuller entries the client asked for
            self.commands = [{"name": n} for n in m.get("slash_commands") or []]
        self.announce("system / init", m)

    def start_turn(self, m):
        self.turn_n += 1
        self.session = m.get("session_id")
        self.turn = f"{self.session}:{self.turn_n}"
        by = "user" if self.prompted else "agent"
        self.prompted = False
        self.emit(f"turn:{self.turn}", "turn", "running", "Working", body={"started_by": by, "model": m.get("model")},
                  native_kind="system / init", native=m)

    def result(self, st, m):
        ok = st == "success" and not m.get("is_error")
        interrupted = st == "error_during_execution"
        self.close_open_items(f"the turn ended with result {st}")
        if self.turn is None:  # a failure before any turn began
            self.turn_n += 1
            self.session = m.get("session_id")
            self.turn = f"{self.session}:{self.turn_n}"
        state = "done" if ok else "cancelled" if interrupted else "failed"
        title = {"done": "Finished", "cancelled": "Interrupted", "failed": "Failed"}[state]
        usage = m.get("usage") or {}
        started = self.open.get(f"turn:{self.turn}", {}).get("body", {}).get("started_by", "user")
        body = {"started_by": started, "duration_ms": m.get("duration_ms"),
                "usage": {"input_tokens": usage.get("input_tokens"), "output_tokens": usage.get("output_tokens"), "cost_usd": m.get("total_cost_usd")}}
        text = "" if ok else clip("; ".join(map(str, m.get("errors") or [])) or str(m.get("result") or ""))
        self.emit(f"turn:{self.turn}", "turn", state, title, text, body, f"result / {st}", m)
        self.turn = None

    # -- content

    def ensure_turn(self, m):
        if self.turn is None:  # messages that arrive after a result, from background work
            self.start_turn({"session_id": m.get("session_id"), "model": None})

    def assistant(self, m):
        self.ensure_turn(m)
        parent = m.get("parent_tool_use_id")
        for n, b in enumerate((m.get("message") or {}).get("content") or []):
            bt = b.get("type")
            base = {"parent": parent} if parent else {}
            if bt == "text":
                self.emit(f"{m.get('uuid')}:{n}", "message", "done", "Message", b.get("text"), base, "assistant [text]", b)
            elif bt == "thinking":
                if not (b.get("thinking") or "").strip():
                    self.skip("thinking block with no text")
                    continue
                self.emit(f"{m.get('uuid')}:{n}", "reasoning", "done", "Thinking", b.get("thinking"), base, "assistant [thinking]", b)
            elif bt == "tool_use":
                name, inp = b.get("name"), b.get("input") or {}
                if name in self.NOT_TOOLS:
                    # A question or a plan, dressed as a tool call. It becomes one item, when the
                    # request arrives through the permission callback, not a tool call as well.
                    self.tools[b.get("id")] = {"kind": "internal", "name": name}
                    self.skip(f"tool call that is really a question or a plan ({name})")
                    continue
                kind = "mcp" if str(name).startswith("mcp__") else self.TOOL_KINDS.get(name, "other")
                body = {**base, "kind": kind, "name": name}
                if kind == "command":
                    body["command"] = inp.get("command")
                elif kind in ("edit", "read"):
                    body["path"] = inp.get("file_path")
                elif kind == "search":
                    body["query"] = inp.get("query") or inp.get("pattern")
                else:
                    body["input"] = clip_native(inp)
                text = body.get("command") or body.get("path") or body.get("query") or inp.get("description") or name
                self.tools[b.get("id")] = body
                self.emit(b.get("id"), "tool", "running", self.tool_title(kind, "running"), text, body, f"assistant [tool_use: {name}]", b)
            else:
                self.unknown(f"assistant block [{bt}]")

    @staticmethod
    def tool_title(kind, state):
        verbs = {"command": ("Running a command", "Ran a command"), "edit": ("Editing a file", "Edited a file"),
                 "read": ("Reading a file", "Read a file"), "search": ("Searching", "Searched"), "fetch": ("Fetching", "Fetched"),
                 "mcp": ("Using a tool", "Used a tool"), "subagent": ("Starting a subagent", "Started a subagent"),
                 "other": ("Using a tool", "Used a tool")}[kind]
        return verbs[0] if state == "running" else verbs[1] if state == "done" else f"{verbs[0]}: {state}"

    def user(self, m):
        content = (m.get("message") or {}).get("content")
        if isinstance(content, str) and content.startswith("<local-command-stdout>"):
            self.ensure_turn(m)
            text = content.replace("<local-command-stdout>", "").replace("</local-command-stdout>", "").strip()
            return self.emit(f"command-output:{m.get('uuid')}", "notice", "done", "Command output", text,
                             {"kind": "command_output"}, "user [local command output]", m)
        if isinstance(content, str) and content.startswith("This session is being continued from a previous conversation"):
            return self.skip("the compaction summary, which is for the model")
        if not isinstance(content, list):
            return self.skip("echo of the user's message")
        for b in content:
            if b.get("type") != "tool_result":
                self.skip("echo of the user's message")
                continue
            tid = b.get("tool_use_id")
            body = dict(self.tools.get(tid) or {"kind": "other", "name": "?"})
            if body.get("kind") == "internal":
                self.skip(f"result of a tool call that is really a question or a plan ({body.get('name')})")
                continue
            c = b.get("content")
            out = c if isinstance(c, str) else "\n".join(x.get("text", "") for x in c or [] if isinstance(x, dict))
            prev = self.open.get(tid)
            state = "failed" if b.get("is_error") else "done"
            if prev is None and tid in self.tools:
                continue  # already closed, for example declined
            body["output"] = clip(out, 2000)
            result = m.get("tool_use_result")
            if isinstance(result, dict) and body.get("kind") == "edit" and result.get("structuredPatch") is not None:
                body["patch"] = clip_native(result.get("structuredPatch"))
            text = (prev or {}).get("text", "")
            self.emit(tid, "tool", state, self.tool_title(body.get("kind", "other"), state), text, body,
                      "user [tool_result]", {"block": b, "tool_use_result": result})

    # -- approvals and questions

    def permission_request(self, m):
        tool, inp, opts = m.get("toolName"), m.get("input") or {}, m.get("options") or {}
        subject = opts.get("toolUseID")
        if tool == "AskUserQuestion":
            qs = [{"id": f"q{n}", "header": q.get("header"), "text": q.get("question"), "multiple": bool(q.get("multiSelect")),
                   "allow_text": True,
                   "options": [{"id": f"o{k}", "label": o.get("label"), "description": o.get("description")} for k, o in enumerate(q.get("options") or [])]}
                  for n, q in enumerate(inp.get("questions") or [])]
            item = f"question:{opts.get('requestId')}"
            self.approvals[tool] = item
            return self.emit(item, "question", "waiting", "Question", qs[0]["text"] if qs else "",
                             {"answer_via": "request", "subject": subject, "questions": qs}, "canUseTool [AskUserQuestion]", m)
        if tool == "ExitPlanMode" and inp.get("plan"):
            self.emit(f"plan:{subject}", "plan", "done", "Plan", inp.get("plan"), {"text": inp.get("plan")}, "canUseTool [ExitPlanMode]", m)
        choices = [{"id": "c0", "label": "Allow once", "effect": "allow"}]
        for n, s in enumerate(opts.get("suggestions") or []):
            label = "Allow, and don't ask again" if s.get("type") == "addRules" else f"Allow, and switch to {s.get('mode')} mode" if s.get("type") == "setMode" else "Allow, with a rule"
            choices.append({"id": f"c{n + 1}", "label": label, "effect": "allow"})
        choices.append({"id": f"c{len(choices)}", "label": "Deny", "effect": "deny"})
        item = f"approval:{opts.get('requestId')}"
        self.approvals[tool] = item
        text = inp.get("command") or inp.get("file_path") or opts.get("description") or tool
        what = "plan" if tool == "ExitPlanMode" else "action"
        self.emit(item, "approval", "waiting", f"Allow this {what}?", text, {"subject": subject, "tool": tool, "choices": choices},
                  f"canUseTool [{tool}]", m)

    def permission_answer(self, m):
        item = self.approvals.pop(m.get("toolName"), None)
        prev = self.open.get(item) if item else None
        if prev is None:
            return self.skip("client: permission result")
        res = m.get("result") or {}
        if prev["type"] == "question":
            return self.emit(item, "question", "done", prev["title"], "Answered",
                             {**prev["body"], "answer": (res.get("updatedInput") or {}).get("answers")},
                             "permission result for AskUserQuestion", res)
        allow = res.get("behavior") == "allow"
        chosen = "c0" if allow else prev["body"]["choices"][-1]["id"]
        self.emit(item, "approval", "done" if allow else "declined", prev["title"], "Answered: " + ("Allow once" if allow else "Deny"),
                  {**prev["body"], "answer": {"choice": chosen}}, "permission result", res)
        if not allow and prev["body"].get("subject") in self.open:
            tool_ev = self.open[prev["body"]["subject"]]
            self.emit(tool_ev["item"], "tool", "declined", self.tool_title(tool_ev["body"].get("kind", "other"), "declined"),
                      tool_ev["text"], tool_ev["body"], "(none: closed by the driver)", {"reason": "the approval was denied"})

    # -- system messages

    def system(self, st, m):
        if st == "init":
            self.init(m)
            if self.turn is None:
                return self.start_turn(m)
            return
        if st in self.NOT_SENT_SYSTEM:
            return self.skip(f"system / {st}")
        if st == "status":
            status = m.get("status")
            if status == "compacting":
                self.ensure_turn(m)
                return self.emit(f"compaction:{m.get('uuid')}", "notice", "running", "Compacting the conversation", "",
                                 {"kind": "compaction"}, "system / status", m)
            if m.get("permissionMode") and self.session:
                self.settings["permission_mode"] = m["permissionMode"]
                return self.announce("system / status", m)
            return self.skip("system / status")
        if st == "compact_boundary":
            self.ensure_turn(m)
            for item, ev in list(self.open.items()):
                if ev["type"] == "notice" and ev["body"].get("kind") == "compaction":
                    self.emit(item, "notice", "done", "Conversation compacted", "", ev["body"], "system / compact_boundary", m)
                    return
            return self.emit(f"compaction:{m.get('uuid')}", "notice", "done", "Conversation compacted", "", {"kind": "compaction"},
                             "system / compact_boundary", m)
        if st == "api_retry":
            self.ensure_turn(m)
            return self.emit(f"retry:{m.get('uuid')}", "notice", "done", "Retrying", "", {"kind": "retry", "attempt": m.get("attempt")},
                             "system / api_retry", m)
        if st in ("task_started", "task_progress", "task_updated", "task_notification"):
            self.ensure_turn(m)
            item = f"task:{m.get('task_id')}"
            prev = self.open.get(item, {})
            body = dict(prev.get("body") or {})
            body.update({k: v for k, v in {
                "kind": "subagent" if m.get("subagent_type") else body.get("kind") or m.get("task_type") or "background",
                "subject": m.get("tool_use_id") or body.get("subject"),
                "description": m.get("description") or body.get("description"),
                "usage": m.get("usage") or body.get("usage"),
            }.items() if v is not None})
            state = "running"
            if st == "task_notification":
                state = {"completed": "done", "failed": "failed", "stopped": "cancelled", "killed": "cancelled"}.get(m.get("status"), "done")
            title = {"running": "Background task", "done": "Background task finished", "failed": "Background task failed",
                     "cancelled": "Background task stopped"}[state]
            text = m.get("summary") or body.get("description") or ""
            return self.emit(item, "task", state, title, text, body, f"system / {st}", m)
        self.unknown(f"system / {st}")


# ---------------------------------------------------------------------------- Hermes


class Hermes(Translator):
    """Hermes hands a platform formatted text, not tool events. So most of what it sends can only
    become a message or a notice. Approvals and questions are the exception.

    A session is the other exception, and only if the plugin asks. Hermes will say which model a
    chat uses, and list the models and the commands, when the plugin calls for them. It reports a
    session being replaced through a plugin hook. Everything else about a session is text."""

    harness = "hermes"
    NOT_SENT = {"send_typing", "stop_typing", "connect", "disconnect", "get_chat_info", "format_tool_preview",
                "format_tool_event", "send_final_ledgered", "send_exec_approval", "render_message_event"}
    # A line Hermes writes about a tool it is running. There is no structured form of this.
    TOOL_LINE = ("💻", "🔍", "🔎", "⚙️", "🐍", "📄", "🌐", "🧠", "✏️", "📝")
    NOTICES = [
        ("📬", "runtime"), ("⏳", "progress"), ("↪", "steered"), ("⚡ Stopped", "stopped"),
        ("📖", "command_output"), ("📊", "command_output"), ("⚠️ Your message was interrupted", "runtime"),
    ]
    # Plugin hooks that say nothing a platform call does not already say, or nothing the model needs.
    HOOKS_NOT_SENT = {
        "pre_command": "names a slash command that is about to run",
        "on_session_end": "the end of a turn, which on_processing_complete also reports",
        "on_session_finalize": "the old session is being closed; on_session_reset follows",
        "on_session_open": "a session was opened",
        "agent_loop_stopped": "a turn was interrupted, which on_processing_complete also reports",
        "pre_approval_request": "an approval is about to be asked, which the prompt also says",
        "post_approval_response": "an approval was answered, which the outcome message also says",
    }
    # What a person can type to answer Hermes's request to confirm one of their own commands.
    CONFIRM = {"/approve": "once", "/always": "always", "/cancel": "cancel"}
    EFFORTS = ("none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra")

    def __init__(self):
        super().__init__()
        self.sent: dict[str, dict] = {}  # Hermes's id for a message it sent -> the event it became
        self.last_send = None
        self.approval = None  # the open approval item, if any
        self.question = None  # the open question item, if any
        self.confirm = None  # the open request to confirm a slash command, if any
        self.final_next = False
        self.key = None  # the chat and sender of the turn in progress, or of the last call
        self.turn_command = None  # the slash command that started the turn in progress, if one did
        self.generation: dict[str, int] = {}  # chat and sender -> how many sessions they have had
        self.settings: dict[str, dict] = {}  # session name -> setting id -> the setting
        self.hermes_ids: dict[str, str] = {}  # Hermes's own session id -> chat and sender
        self.commands: list[dict] = []  # Hermes's command list, if the plugin asked for it
        self.typed: dict[str, tuple] = {}  # message id of a typed command -> the control it was counted as
        self.reset_by_hook = False

    def feed(self, rec):
        m, via = rec["msg"], rec.get("via")
        if via == "note":
            return self.skip("recorder note")
        if via == "inbound":
            return self.inbound(m)
        if via == "press":
            return self.press(m)
        if via == "plugin_hook":
            return self.plugin_hook(m)
        method = m.get("method")
        if via == "call":
            return self.asked(method, m) if rec["dir"] == "in" else self.ask(method, m)
        if via == "result":
            if method in ("send", "_send_exec_approval_prompt") and self.last_send is not None:
                mid = (m.get("result") or {}).get("message_id")
                if mid:
                    self.sent[mid] = self.last_send
            return self.skip(f"result of {method}")
        p = m.get("params") or {}
        if method in self.NOT_SENT:
            if method == "send_final_ledgered":
                self.final_next = True
            return self.skip(method)
        handler = {
            "on_processing_start": self.turn_start, "on_processing_complete": self.turn_end, "send": self.send,
            "edit_message": self.edit, "send_clarify": self.clarify, "_send_exec_approval_prompt": self.approval_card,
            "send_slash_confirm": self.confirm_card, "send_model_picker": self.model_picker,
        }.get(method)
        if handler is None:
            return self.unknown(str(method))
        handler(method, p)

    # -- what people send

    def typed_control(self, word, args):
        """Which control a typed slash command is. Hermes has no other way to give one."""
        first = args[0] if args else ""
        if word == "/stop":
            return "interrupt", "/stop, as a message"
        if word in ("/new", "/reset"):
            return "new_session", f"{word}, as a message"
        if word == "/model" and first and not first.startswith("-"):
            return "set model", "/model reset, as a message" if first == "reset" else "/model <id>, as a message"
        if word == "/reasoning" and first in self.EFFORTS:
            return "set reasoning", "/reasoning <level>, as a message"
        # The next three change what the agent may do without asking.
        if word == "/yolo":
            return "set (permission) yolo", "/yolo, as a message"
        if word == "/approvals" and first:
            return "set (permission) approvals", "/approvals <mode>, as a message"
        if word in ("/memory", "/skills") and first in ("approval", "mode") and len(args) > 1:
            return "set (permission) write approval", f"{word} approval <on|off>, as a message"
        return "(slash command, as a message)", word

    def inbound(self, m):
        text = str(m.get("text") or "").strip()
        word, *args = text.split()
        confirming = bool(self.confirm and self.confirm in self.open and word in self.CONFIRM)
        if confirming:
            self.control("(confirm a command, typed)", word)
        elif word in ("/approve", "/deny"):
            # "/approve session" and "/approve always" leave a standing approval behind.
            scope = f" {args[0]}" if word == "/approve" and args and args[0] in ("session", "always") else ""
            self.control("(answer an approval, typed)", word + scope)
        elif word.startswith("/"):
            self.typed[m.get("message_id")] = self.typed_control(word, args)
            self.control(*self.typed[m.get("message_id")])
        if confirming:
            return self.skip("client: answer to a command confirmation, as a slash command")
        if self.approval and word in ("/approve", "/deny"):
            return self.skip("client: answer to an approval, as a slash command")
        if self.question and self.question in self.open:
            prev = self.open[self.question]
            self.emit(self.question, "question", "done", prev["title"], "Answered",
                      {**prev["body"], "answer": text}, "inbound message, taken as the answer", m)
            self.question = None
            return
        self.skip("client: slash command" if word.startswith("/") else "client: chat message")

    def press(self, m):
        if "pick_model" in m:
            self.control("set model", "on_model_selected(), from the model picker")
            return self.skip("client: a pick from the model picker")
        item = self.approval
        prev = self.open.get(item) if item else None
        if prev is None:
            return self.skip("client: button press")
        chosen = next((c for c in prev["body"]["choices"] if c.get("native") == m.get("choice")), None)
        allow = bool(chosen and chosen["effect"] == "allow")
        self.emit(item, "approval", "done" if allow else "declined", prev["title"],
                  "Answered: " + (chosen["label"] if chosen else str(m.get("choice"))),
                  {**prev["body"], "answer": {"choice": chosen["id"] if chosen else None}}, "button press", m)
        self.approval = None

    # -- sessions

    @staticmethod
    def key_of(src):
        return f"{src.get('chat_type')}:{src.get('chat_id')}:{src.get('user_id')}"

    def name(self, key):
        """Hermes hands a platform no session id. The driver names a session after the chat and the
        sender, and counts when Hermes replaces it."""
        n = self.generation.setdefault(key, 1)
        return key if n == 1 else f"{key}#{n}"

    def session_changed(self, key, native_kind, native):
        """Send the session item of this chat and sender, if anything in it is new."""
        name = self.name(key)
        settings = list(self.settings.setdefault(name, {}).values())
        session, self.session = self.session, name
        self.session_item(name, settings, native_kind, native, commands=self.commands)
        self.session = session

    def put(self, key, id_, label, value, options=None, approve=False):
        known = self.settings.setdefault(self.name(key), {})
        if options is None and id_ in known:
            options = known[id_]["options"]  # keep the accepted values Hermes gave earlier
        known[id_] = self.setting(id_, label, value, options, approve)

    def session_replaced(self, key, native_kind, native):
        """Hermes ended this chat's session and started another. The session key stays the same."""
        name = self.name(key)
        prev = next((e for e in reversed(self.events) if e["item"] == f"session:{name}"), None)
        if prev is not None and prev["state"] == "running":
            turn, session, self.turn, self.session = self.turn, self.session, None, name
            self.emit(prev["item"], "session", "done", "Session ended", prev["text"], prev["body"], native_kind, native)
            self.turn, self.session = turn, session
        self.generation[key] = self.generation.get(key, 1) + 1
        self.session_changed(key, native_kind, native)
        if self.turn is not None and self.key == key:
            self.session = self.name(key)

    def plugin_hook(self, m):
        """Hooks a Hermes plugin may register beside its platform. None is called unless it does."""
        hook, p = m.get("hook"), m.get("params") or {}
        if hook == "on_session_start":
            # The first turn of a session. It names Hermes's session id and the model, and not the
            # chat: the driver takes it to belong to the turn in progress.
            if self.key is None:
                return self.skip("on_session_start with no turn in progress")
            self.hermes_ids[p.get("session_id")] = self.key
            self.put(self.key, "model", "Model", p.get("model"))
            self.session_changed(self.key, hook, p)
            return self.skip("on_session_start, kept for the session item")
        if hook == "on_session_reset":
            # It names the old and the new session id, and again not the chat.
            key = self.hermes_ids.get(p.get("old_session_id")) or self.key
            if key is None:
                return self.unknown("on_session_reset for a chat the driver cannot name")
            self.hermes_ids[p.get("new_session_id")] = key
            self.reset_by_hook = True
            return self.session_replaced(key, hook, p)
        if hook in self.HOOKS_NOT_SENT:
            return self.skip(f"plugin hook {hook}: {self.HOOKS_NOT_SENT[hook]}")
        self.unknown(f"plugin hook {hook}")

    def ask(self, method, m):
        """The plugin asked Hermes for something as data."""
        src = (m.get("params") or {}).get("source")
        if src:
            self.key = self.key_of(src)
        self.skip(f"client: {method}")

    def asked(self, method, m):
        if "error" in m:
            return self.skip(f"error result of {method}")
        result = m.get("result")
        if method == "hermes_cli.commands.COMMAND_REGISTRY":
            self.commands = [{"name": f"/{c['name']}", "description": c.get("description"), "hint": c.get("args_hint") or None}
                             for c in result or [] if c.get("gateway_available")]
            for key in self.generation:
                self.session_changed(key, method, {"commands": len(self.commands)})
        elif method == "get_session_model_settings" and isinstance(result, dict):
            self.put(self.key, "model", "Model", result.get("model"))
            self.put(self.key, "provider", "Provider", result.get("provider"))
            self.session_changed(self.key, method, result)
        elif method == "list_session_model_picker_providers" and isinstance(result, list):
            self.model_options(self.key, result, method)
        elif method == "on_model_selected":
            # What the picker's callback returned: text for the chat. The new model is not in it as
            # a field; the driver asks for the model settings to learn it.
            item = f"sent:{self.seq + 1}"
            return self.emit(item, "notice", "done", "Command output", result, {"kind": "command_output", "command": "/model"},
                             f"result of {method}", m)
        self.skip(f"result of {method}")

    def model_options(self, key, providers, native_kind):
        """The models Hermes will accept, as a list. It gives one only when asked."""
        current = next((p for p in providers if p.get("is_current")), None)
        options = [x for p in providers for x in p.get("models") or []]
        known = self.settings.setdefault(self.name(key), {})
        self.put(key, "model", "Model", (known.get("model") or {}).get("value"), options)
        if current:
            self.put(key, "provider", "Provider", current.get("slug"), [p.get("slug") for p in providers])
        self.session_changed(key, native_kind, {"providers": [{k: v for k, v in p.items() if k != "models"} for p in providers]})

    def model_picker(self, method, p):
        """The platform claimed a model picker, so Hermes hands over the models where it would
        otherwise send them as text. The list goes into the session item; nothing else is sent."""
        if self.key is not None:
            known = self.settings.setdefault(self.name(self.key), {})
            if p.get("current_model") and "model" not in known:
                self.put(self.key, "model", "Model", p.get("current_model"))
            self.model_options(self.key, p.get("providers") or [], method)
        self.last_send = None
        self.skip("send_model_picker: the models as a list, kept for the session item")

    # -- turns

    def turn_start(self, method, p):
        ev = p.get("event") or {}
        src = ev.get("source") or {}
        self.key = self.key_of(src)
        self.session = self.name(self.key)
        text = str(ev.get("text") or "").strip()
        self.turn_command = text.split()[0] if text.startswith("/") else None
        # Hermes tells a platform nothing about a session unless the plugin asks: no id, no model, no
        # list of commands. The driver names it after the chat and the sender.
        self.session_changed(self.key, "(none: made by the driver)", {"reason": "first turn seen for this chat and sender"})
        self.turn = ev.get("message_id")
        # Hermes can report a turn complete and start it again a moment later. The same item id
        # means the second start simply replaces the first "done".
        self.emit(f"turn:{self.turn}", "turn", "running", "Working", body={"started_by": "user"}, native_kind=method, native=ev)

    def turn_end(self, method, p):
        if self.turn is None:
            return self.skip("on_processing_complete with no turn open")
        outcome = p.get("outcome")
        state = {"success": "done", "failure": "failed", "cancelled": "cancelled"}.get(outcome, "done")
        still = [i for i, e in self.open.items() if e["type"] not in ("turn", "task", "session")]
        if state != "done" or not still:
            self.close_open_items(f"the turn ended with outcome {outcome}")
        title = {"done": "Finished", "failed": "Failed", "cancelled": "Interrupted"}[state]
        self.emit(f"turn:{self.turn}", "turn", state, title, body={"started_by": "user"}, native_kind=method, native=p)

    # -- everything that arrives as text

    def command_reply(self, item, content, p):
        """Hermes's own answer to a slash command. What it says exists only as wording, so a setting
        the answer confirms is read from the words, and from the command that was typed."""
        command, typed = self.turn_command, self.typed.get(p.get("reply_to"))
        body = {"kind": "command_output", "command": command}
        if content.startswith("⛔"):
            # Hermes refused: the sender is not on its own list of admins for slash commands.
            body["kind"] = "refused"
            if typed and typed[0] != "(slash command, as a message)":
                self.controls[typed] -= 1
                if not self.controls[typed]:
                    del self.controls[typed]
                self.control("(slash command, refused)", command)
            self.last_send = self.emit(item, "notice", "done", "Not allowed", content, body, "send [refusal of a slash command]", p)
            return
        if content.startswith("✨"):
            # A new session. Without the plugin hook this message is the only sign of it.
            body["kind"] = "new_session"
            if not self.reset_by_hook:
                self.session_replaced(self.key, "send [reply to /new, read from its wording]", p)
            self.reset_by_hook = False
        elif typed and typed[0].startswith("set "):
            words = content.split()
            if typed[0] == "set (permission) yolo" and "YOLO" in content:
                on = "**ON**" in content
                self.put(self.key, "yolo", "Skip approvals", "on" if on else "off", ["on", "off"], approve=True)
            elif typed[0] == "set (permission) approvals" and content.startswith("Approval mode:") and len(words) > 2:
                self.put(self.key, "approvals", "Approval mode", words[2], approve=True)
            elif typed[0] == "set reasoning" and "`" in content:
                self.put(self.key, "reasoning", "Reasoning effort", content.split("`")[1])
            elif typed[0] == "set (permission) write approval" and ".write_approval set to '" in content:
                what = content.split(".write_approval")[0].split()[-1]
                self.put(self.key, f"{what}_write_approval", f"Ask before writing {what}", content.split("'")[1],
                         ["on", "off"], approve=True)
            self.session_changed(self.key, f"send [reply to {command}, read from its wording]", p)
        self.last_send = self.emit(item, "notice", "done", "Command output", content, body, f"send [reply to {command}]", p)

    def send(self, method, p):
        content = str(p.get("content") or "")
        meta = p.get("metadata") or {}
        self.seq_hint = None
        item = f"sent:{self.seq + 1}"
        final, self.final_next = self.final_next, False
        if meta.get("is_approval_prompt") or content.startswith("⚠️ **Hermes wants"):
            choices = [{"id": "c0", "label": "Allow once", "effect": "allow", "native": "/approve"},
                       {"id": "c1", "label": "Allow for this session", "effect": "allow", "native": "/approve session"},
                       {"id": "c2", "label": "Always allow", "effect": "allow", "native": "/approve always"},
                       {"id": "c3", "label": "Deny", "effect": "deny", "native": "/deny"}]
            self.approval = item
            self.last_send = self.emit(item, "approval", "waiting", "Allow this command?", content,
                                       {"answer_via": "message", "choices": choices}, "send [approval prompt as text]", p)
            return
        if content.startswith(("✅ Command approved", "❌ Command denied", "⌛ Approval timed out")) and self.approval in self.open:
            prev = self.open[self.approval]
            state = "done" if content.startswith("✅") else "declined" if content.startswith("❌") else "cancelled"
            self.last_send = self.emit(self.approval, "approval", state, prev["title"], content, prev["body"],
                                       "send [approval outcome as text]", p)
            self.approval = None
            return
        if content.startswith("❓") and self.question in self.open:
            self.last_send = None
            return self.skip("send: a question as text, already sent in structured form")
        if self.confirm in self.open:
            prev = self.open[self.confirm]
            if content == prev["text"]:
                self.last_send = prev
                return self.skip("send: a command confirmation as text, already sent in structured form")
            if self.turn_command in self.CONFIRM:
                # The answer to the confirmation. Hermes's reply is the command's own outcome.
                choice = self.CONFIRM[self.turn_command]
                chosen = next(c for c in prev["body"]["choices"] if c["native"] == self.turn_command)
                self.emit(self.confirm, "approval", "declined" if choice == "cancel" else "done", prev["title"],
                          f"Answered: {chosen['label']}", {**prev["body"], "answer": {"choice": chosen["id"]}},
                          f"send [outcome of {prev['body']['command']}, after {self.turn_command}]", p)
                self.confirm, self.turn_command = None, prev["body"]["command"]
        if self.turn_command and (meta.get("notify") or final):
            # Hermes marks its answer to a slash command as it marks an agent's final reply. A command
            # that runs the model was not recorded, so its reply is not told apart here.
            return self.command_reply(item, content, p)
        for prefix, kind in self.NOTICES:
            if content.startswith(prefix):
                self.last_send = self.emit(item, "notice", "done", "Notice", content, {"kind": kind}, f"send [{kind} notice]", p)
                return
        if content.startswith(self.TOOL_LINE):
            self.last_send = self.emit(item, "notice", "done", "Activity", content, {"kind": "activity"}, "send [tool progress line]", p)
            return
        phase = "final" if (final or meta.get("notify")) else "commentary"
        self.last_send = self.emit(item, "message", "done", "Message", content, {"phase": phase}, f"send [{phase} text]", p)

    def edit(self, method, p):
        prev = self.sent.get(p.get("message_id"))
        if prev is None:
            return self.unknown("edit_message for a message that was not sent")
        self.emit(prev["item"], prev["type"], prev["state"], prev["title"], p.get("content"), prev["body"], method, p)

    # -- the structured calls

    def clarify(self, method, p):
        qs = [{"id": "q0", "text": p.get("question"), "multiple": False, "allow_text": True,
               "options": [{"id": f"o{k}", "label": c} for k, c in enumerate(p.get("choices") or [])]}]
        self.question = f"question:{p.get('clarify_id')}"
        self.emit(self.question, "question", "waiting", "Question", p.get("question"),
                  {"answer_via": "message", "questions": qs}, method, p)

    def approval_card(self, method, p):
        prompt = p.get("prompt") or {}
        choices = [{"id": f"c{n}", "label": label, "effect": "deny" if choice == "deny" else "allow", "native": choice}
                   for n, (label, choice, _style) in enumerate(prompt.get("actions") or [])]
        self.approval = f"approval:{prompt.get('request_id')}"
        self.last_send = self.emit(self.approval, "approval", "waiting", "Allow this command?", prompt.get("command"),
                                   {"answer_via": "request", "reason": prompt.get("description"), "choices": choices}, method, prompt)

    def confirm_card(self, method, p):
        """Hermes asks the person to confirm one of their own slash commands, such as /new. It is an
        approval of what a person asked for, not of what the agent wants to do. A platform without
        buttons gets the same text through send straight after, and the answer is a typed command."""
        choices = [{"id": "c0", "label": "Approve once", "effect": "allow", "native": "/approve"},
                   {"id": "c1", "label": "Always approve", "effect": "allow", "native": "/always"},
                   {"id": "c2", "label": "Cancel", "effect": "deny", "native": "/cancel"}]
        self.confirm = f"confirm:{p.get('confirm_id')}"
        self.emit(self.confirm, "approval", "waiting", "Confirm this command?", p.get("message"),
                  {"answer_via": "message", "subject": "command", "command": p.get("title"), "choices": choices}, method, p)


# ---------------------------------------------------------------------------- OpenClaw


class OpenClaw(Translator):
    """OpenClaw hands a channel plugin structured callbacks about tools, and two formatted copies
    of the same activity. Tool items are built from the structured callbacks. The copies are not
    sent, except where a copy holds something the callbacks lack: a question, and the result of a
    tool that is not a command.

    A recording is flat: it does not say which inbound.run a callback belongs to. A plugin knows,
    because each run is given its own callbacks. Where that matters, the translator goes by order,
    and says so in a comment.

    A session item holds the model and the thinking level, which OpenClaw names before each run.
    The rest of a session arrives only if the plugin asks through the plugin SDK: the stored
    session entry, the command list with the values each command accepts, and the models."""

    harness = "openclaw"
    NOT_SENT = {
        "dispatcher.onReplyStart": "typing signal",
        "onTypingController": "typing signal",
        "inbound.log": "receive-path log",
        "dispatcher.onDeliverySettled": "delivery bookkeeping",
        "dispatcher.onIdle": "delivery bookkeeping",
        "dispatcher.onSettled": "delivery bookkeeping",
        "dispatcher.onCleanup": "delivery bookkeeping",
        "onObservedReplyDelivery": "delivery bookkeeping",
        "onBlockReplyQueued": "delivery bookkeeping",
        "messaging.normalizeTarget": "address tidying",
        "messaging.targetResolver.looksLikeId": "address tidying",
        "messaging.targetResolver.resolveTarget": "address tidying",
        "messaging.inferTargetChatType": "address tidying",
        "groups.resolveRequireMention": "question to the plugin about the channel",
        "actions.describeMessageTool": "question to the plugin about the channel",
        "actions.supportsAction": "question to the plugin about the channel",
        "agentPrompt.messageToolHints": "question to the plugin about the channel",
        "approvalCapability.getActionAvailabilityState": "question to the plugin about approvals",
        "approvalCapability.authorizeActorAction": "question to the plugin about approvals",
        "gateway.startAccount": "the account started",
        "onAssistantMessageStart": "start of a message, before it has text",
        "onPartialReply": "partial text",
        "onToolResult": "formatted line about a tool, a copy of the tool delivery",
    }
    # Only the tool names seen in the recordings. A command is known by commandBearing, not by name.
    TOOL_KINDS = {"write": "edit", "edit": "edit"}
    DECISIONS = {"allow-once": "Allow once", "allow-always": "Always allow", "deny": "Deny"}
    BY_DRIVER = "(none: closed by the driver)"
    # Typed commands that change one setting of the session, and the setting each one changes.
    SETTING_COMMANDS = {"/think": "think", "/thinking": "think", "/t": "think", "/verbose": "verbose", "/v": "verbose",
                        "/reasoning": "reasoning", "/reason": "reasoning", "/fast": "fast", "/trace": "trace",
                        "/usage": "usage", "/queue": "queue", "/activation": "activation", "/send": "send"}
    # Fields of a stored session entry that are settings: field -> (id, label, needs the right to approve).
    ENTRY_SETTINGS = {
        "thinkingLevel": ("think", "Thinking", False), "verboseLevel": ("verbose", "Verbose", False),
        "reasoningLevel": ("reasoning", "Reasoning", False), "fastMode": ("fast", "Fast mode", False),
        "traceLevel": ("trace", "Trace", False), "sendPolicy": ("send", "Send policy", False),
        "elevatedLevel": ("elevated", "Elevated", True), "permissionMode": ("permission_mode", "Permission mode", True),
        "execHost": ("exec_host", "Exec host", True), "execNode": ("exec_node", "Exec node", True),
    }
    HOOKS_NOT_SENT = {
        "session_start": "a session started, which the next run also says",
        "before_reset": "the conversation that is about to be dropped",
    }

    def __init__(self):
        super().__init__()
        self.turn_n = 0
        self.turn_body: dict = {}
        self.driver_turn = False  # the open turn was opened by the driver, not reported by OpenClaw
        self.failed_turn = None  # the last turn, if it failed: its error text arrives after it
        self.route_session = None
        self.model = None
        self.thinking = ""  # the model's thinking so far
        self.tools: dict[str, dict] = {}  # toolCallId -> what is known about the call
        self.ending = None  # a tool that has ended and is held for one call, for its result line
        self.asking = None  # the ask_user call in progress
        self.inbound_text: dict[str, str] = {}  # message id -> what the person sent
        self.commands: list[str] = []  # slash commands handed to OpenClaw and not yet answered
        self.decisions: dict[str, str] = {}  # approval id -> the decision the client sent
        self.last_press = None
        self.sent: dict[str, dict] = {}  # the plugin's id for a posted message -> the event it became
        self.posting = None  # (method, event) of the call whose result will name the posted message
        self.denied: set[str] = set()  # tool calls whose approval was denied
        self.route_key = None  # the session key of the last route OpenClaw gave
        self.generation: dict[str, int] = {}  # session key -> how many sessions it has named
        self.known: dict[str, dict] = {}  # session name -> setting id -> the setting
        self.ended: set[str] = set()  # sessions OpenClaw has started afresh
        self.options: dict[str, list] = {}  # setting id -> the values OpenClaw accepts, if the plugin asked
        self.command_list: list[dict] = []  # OpenClaw's command list, if the plugin asked
        self.asked_key = None  # the session key the plugin last asked about
        self.last_run = None  # the text of the message last handed to inbound.run, until that call returns
        self.reset_by_hook = False

    def feed(self, rec):
        m, via, out = rec["msg"], rec.get("via"), rec["dir"] == "out"
        if via == "note":
            return self.skip("recorder note")
        method = m.get("method")
        if via == "result":
            return self.result(method, m)
        # A tool that is not a command reports its end, and its result arrives as text in the tool
        # delivery that follows. Any other call means no result line is coming.
        if self.ending and not (via == "hook" and method in ("onToolResult", "deliver")):
            self.finish_tool()
        if via == "inbound":
            return self.inbound(m)
        if via == "press":
            return self.press(m)
        if via == "plugin_hook":
            return self.plugin_hook(m)
        if via == "call":
            return self.call_out(method, m) if out else self.call_in(method, m)
        p = m.get("params")
        if method in self.NOT_SENT:
            return self.skip(f"{self.NOT_SENT[method]} ({method})")
        handler = {
            "onModelSelected": self.model_selected,
            "onPreparedAgentRunStart": self.run_start,
            "onAgentRunTerminalOutcome": self.run_end,
            "gateway.abort": self.account_stopped,
            "onItemEvent": self.item_event,
            "onToolStart": self.tool_start,
            "onCommandOutput": self.command_output,
            "onApprovalEvent": self.approval_event,
            "onReasoningStream": self.reasoning_stream,
            "onReasoningEnd": self.reasoning_end,
            "deliver": self.deliver,
            "message.send.payload": self.send_payload,
            "approvalCapability.render.exec.buildPendingPayload": self.approval_card,
            "approvalCapability.render.exec.buildResolvedPayload": self.approval_card_resolved,
            "actions.handleAction": self.handle_action,
        }.get(method)
        if handler is None:
            return self.unknown(str(method))
        handler(method, p)

    # -- what the plugin sends, and what its own calls return

    def typed_control(self, text):
        """Which control a typed command is. This counts what was typed. Whether OpenClaw accepted it
        is in the wording of its reply, or in the stored session entry, and nowhere else."""
        word, *args = text.split()
        first = args[0] if args else ""
        if word == "/stop":
            return "interrupt", "/stop, as a message"
        if word in ("/new", "/reset"):
            return "new_session", f"{word}, as a message"
        if word == "/model" and first and first not in ("list", "status"):
            return "set model", "/model default, as a message" if first == "default" else "/model <provider/model>, as a message"
        # The next four change what the agent may do without asking.
        if word in ("/elevated", "/elev") and first:
            return "set (permission) elevated", "/elevated <level>, as a message"
        if word == "/exec" and any(a.startswith(("security=", "ask=")) for a in args):
            return "set (permission) exec", "/exec security=… ask=…, with or without a message"
        if word in ("/config", "/debug", "/mcp", "/plugins", "/allowlist") and first in (
                "set", "unset", "reset", "add", "remove", "install", "enable", "disable"):
            return "(write configuration, typed)", f"{word} {first}"
        if word == "/bash" or text.startswith("!"):
            return "(run a host command, typed)", "/bash <command>" if word == "/bash" else "! <command>"
        if word == "/exec" and args:
            return "set exec host", "/exec host=…, as a message"
        if word in self.SETTING_COMMANDS and first:
            return f"set {self.SETTING_COMMANDS[word]}", f"/{self.SETTING_COMMANDS[word]} <value>, as a message"
        return "(slash command, as a message)", word

    def inbound(self, m):
        text = str(m.get("text") or "").strip()
        self.inbound_text[m.get("message_id")] = text
        word = text.split(" ")[0]
        if word == "/approve":
            # "allow-always" leaves a standing approval behind.
            self.control("(answer an approval, typed)", "/approve <id> allow-always" if "allow-always" in text.split() else word)
            return self.skip("client: answer to an approval, as a slash command")
        if text.startswith(("/", "!")):
            self.control(*self.typed_control(text))
        self.skip("client: slash command" if text.startswith("/") else
                  "client: host command" if text.startswith("!") else "client: chat message")

    def press(self, m):
        if "approvalId" in m:
            self.last_press = m.get("approvalId")
            return self.skip("client: button press on an approval")
        self.skip("client: button press on a question")

    def call_out(self, method, m):
        if method == "inbound.run":
            # Only a message that is run can be answered. A command is answered by OpenClaw itself.
            text = self.inbound_text.get((m.get("params") or {}).get("messageId"), "")
            self.last_run = text
            if text.startswith(("/", "!")):
                self.commands.append(text)
                parts = text.split()
                if parts[0] == "/approve" and len(parts) >= 3:
                    self.decisions[parts[1]] = parts[2]
        elif method == "session-store-runtime.getSessionEntryAsync":
            self.asked_key = (m.get("params") or {}).get("sessionKey")
        self.skip(f"client: {method}")

    def call_in(self, method, m):
        result = m.get("result") or {}
        if method == "routing.resolveAgentRoute":
            self.route_key = result.get("sessionKey")
            self.route_session = self.name(self.route_key)
            if self.turn is None:
                self.session = self.route_session
        elif method == "inbound.run":
            dropped = (result.get("dispatchResult") or {}).get("beforeAgentRunBlocked")
            if dropped and self.last_run in self.commands:
                # OpenClaw took the message as a command from someone who may not use commands. It ran
                # nothing and sent nothing, so no reply will answer it.
                self.commands.remove(self.last_run)
            self.last_run = None
            if dropped:
                return self.skip("result of inbound.run: OpenClaw dropped the message and sent nothing")
            if self.driver_turn:
                # With no run callbacks registered, the return of inbound.run is the only end there is.
                return self.close_turn("done", "result of inbound.run", result)
        elif method.startswith(("native-command-registry.", "models-provider-runtime.", "session-store-runtime.")):
            return self.asked(method, m.get("result"))
        elif method == "resolveApprovalOverGateway":
            if "error" in m:
                err = m["error"] or {}
                return self.emit(f"error:{self.seq + 1}", "error", "failed", "OpenClaw refused an answer", err.get("message", ""),
                                 {"request": method, "subject": f"approval:{self.last_press}"}, f"error result of {method}", err)
            approval = result.get("approval") or {}
            if not result.get("applied"):
                return self.unknown(f"{method} result that was not applied")
            return self.approval_resolved(approval.get("id"), f"result of {method}", result, approval.get("decision"))
        elif method == "questionGatewayRuntime.resolveOption":
            return self.question_answered(method, result)
        self.skip(f"result of {method}")

    def result(self, method, m):
        """What the plugin returned to OpenClaw. For a posted message that is the plugin's id for it."""
        res = m.get("result")
        if self.posting and self.posting[0] == method and isinstance(res, dict):
            mid = (res.get("messageId") or (res.get("details") or {}).get("messageId")
                   or (res.get("receipt") or {}).get("primaryPlatformMessageId"))
            if mid:
                self.sent[mid] = self.posting[1]
            self.posting = None
        self.skip(f"result of {method}")

    # -- sessions

    def name(self, key):
        """OpenClaw keeps a session's key and id when it starts the session afresh. So the driver
        counts: the second session under a key is named key#2."""
        n = self.generation.get(key, 1)
        return key if n == 1 or key is None else f"{key}#{n}"

    def put(self, name, id_, label, value, approve=False):
        self.known.setdefault(name, {})[id_] = self.setting(id_, label, value, self.options.get(id_), approve)

    def send_session(self, name, native_kind, native):
        """Send the session item, if anything in it is new."""
        if name not in self.known or name in self.ended:
            return
        for x in self.known[name].values():
            x["options"] = self.options.get(x["id"], x["options"])
        session, self.session = self.session, name
        self.session_item(name, list(self.known[name].values()), native_kind, native, commands=self.command_list)
        self.session = session

    def session_replaced(self, key, native_kind, native):
        """OpenClaw started this chat's session afresh. Its key and its id stay the same."""
        name = self.name(key)
        prev = next((e for e in reversed(self.events) if e["item"] == f"session:{name}"), None)
        if prev is not None and prev["state"] == "running":
            turn, session, self.turn, self.session = self.turn, self.session, None, name
            self.emit(prev["item"], "session", "done", "Session ended", prev["text"], prev["body"], native_kind, native)
            self.turn, self.session = turn, session
        self.ended.add(name)
        self.generation[key] = self.generation.get(key, 1) + 1
        name = self.name(key)
        self.known[name] = {}
        self.send_session(name, native_kind, native)
        if key == self.route_key:
            self.route_session = name
            if self.turn is None:
                self.session = name

    def plugin_hook(self, m):
        """Typed hooks a plugin may register beside its channel. None is called unless it does."""
        hook, event = m.get("hook"), m.get("event") or {}
        if hook == "session_end":
            if event.get("reason") in ("new", "reset"):
                self.reset_by_hook = True
                return self.session_replaced(event.get("sessionKey"), f"{hook} [{event.get('reason')}]", m)
            return self.skip(f"plugin hook session_end [{event.get('reason')}]: the gateway stopped, and the session is kept")
        if hook in self.HOOKS_NOT_SENT:
            return self.skip(f"plugin hook {hook}: {self.HOOKS_NOT_SENT[hook]}")
        self.unknown(f"plugin hook {hook}")

    def asked(self, method, result):
        """What the plugin SDK returned when the plugin asked for something as data."""
        if method.endswith(".listChatCommands") and isinstance(result, list):
            self.command_list = [{"name": (c.get("textAliases") or [f"/{c.get('key')}"])[0], "description": c.get("description"),
                                  "hint": " ".join(f"<{a.get('name')}>" for a in c.get("args") or []) or None} for c in result]
            for c in result:
                arg = (c.get("args") or [{}])[0]
                choices = arg.get("resolvedChoices") or arg.get("choices")
                if isinstance(choices, list) and len(c.get("args") or []) == 1:
                    self.options[c.get("key")] = [x.get("value") if isinstance(x, dict) else x for x in choices]
        elif method.endswith(".buildModelsProviderData") and isinstance(result, dict):
            self.options["model"] = [f"{prov}/{x}" for prov, models in (result.get("byProvider") or {}).items() for x in models]
            default = result.get("resolvedDefault") or {}
            self.default_model = f"{default.get('provider')}/{default.get('model')}" if default else None
        elif method.endswith(".getSessionEntryAsync") and isinstance(result, dict):
            name = self.name(self.asked_key)
            self.known.setdefault(name, {})  # the store has an entry, so there is a session to describe
            if result.get("providerOverride") and result.get("modelOverride"):
                self.put(name, "model", "Model", f"{result['providerOverride']}/{result['modelOverride']}")
            elif result.get("modelOverrideSource") == "default" and getattr(self, "default_model", None):
                # The session's own choice was cleared. The entry does not name the default it falls
                # back to; the model list does, if the plugin asked for it.
                self.put(name, "model", "Model", self.default_model)
            for field, (id_, label, approve) in self.ENTRY_SETTINGS.items():
                if field in result:
                    self.put(name, id_, label, result[field], approve)
            self.skip(f"result of {method}")
            return self.send_session(name, method, {k: v for k, v in result.items() if k in self.ENTRY_SETTINGS or "verride" in k})
        for name in list(self.known):  # the lists belong in the item of every session still running
            self.send_session(name, method, {"asked": method})
        self.skip(f"result of {method}")

    # -- turns

    def model_selected(self, method, p):
        self.model = f"{p.get('provider')}/{p.get('model')}"
        self.think = p.get("thinkLevel") or getattr(self, "think", None)
        self.model_native = p
        self.skip("onModelSelected, kept for the turn and the session item")

    def open_turn(self, turn=None):
        # OpenClaw names the model and the thinking level before each run. It says nothing else
        # about a session unless the plugin asks.
        if self.session:
            self.put(self.session, "model", "Model", self.model)
            self.put(self.session, "think", "Thinking", getattr(self, "think", None))
            self.send_session(self.session, "onModelSelected", getattr(self, "model_native", None))
        self.turn_n += 1
        self.turn = turn or f"{self.session}:{self.turn_n}"
        self.failed_turn = None
        self.turn_body = {"started_by": "user"}
        if self.model:
            self.turn_body["model"] = self.model

    def run_start(self, method, p):
        if self.turn is not None:
            return self.unknown("onPreparedAgentRunStart while a turn is open")
        key = (p.get("transcriptStart") or {}).get("sessionKey")
        self.session = self.name(key) if key else self.session
        if self.last_run in self.commands:
            # The message started a run, so it was a directive with a task, or a command the model
            # answers. OpenClaw sends no reply of its own for it.
            self.commands.remove(self.last_run)
        self.open_turn(p.get("runId"))
        self.emit(f"turn:{self.turn}", "turn", "running", "Working", body=self.turn_body, native_kind=method, native=p)

    def ensure_turn(self):
        """The agent spoke and no run was reported. The plugin registered no run callbacks, so the
        driver opens the turn itself."""
        if self.turn is None:
            self.open_turn()
            self.driver_turn = True
            self.emit(f"turn:{self.turn}", "turn", "running", "Working", body=self.turn_body,
                      native_kind="(none: opened by the driver)", native={"reason": "the agent spoke and no run was reported"})

    def run_end(self, method, p):
        state = {"completed": "done", "failed": "failed"}.get(p)
        if state is None:
            return self.unknown(f"onAgentRunTerminalOutcome [{p}]")
        if self.turn is None:
            if state != "failed":
                return self.unknown("onAgentRunTerminalOutcome with no turn open")
            self.open_turn()  # the run failed before OpenClaw reported it as started
        self.close_turn(state, method, p)

    def close_turn(self, state, native_kind, native, text=""):
        if self.thinking.strip():  # thinking that never reported its end
            self.emit(f"reasoning:{self.seq + 1}", "reasoning", "done", "Thinking", self.thinking, {}, self.BY_DRIVER,
                      {"reason": "the turn ended"})
        self.thinking = ""
        self.close_open_items(f"the turn ended: {state}")
        title = {"done": "Finished", "failed": "Failed", "cancelled": "Interrupted"}[state]
        self.emit(f"turn:{self.turn}", "turn", state, title, text, self.turn_body, native_kind, native)
        self.failed_turn = self.turn if state == "failed" else None
        self.turn, self.driver_turn, self.asking = None, False, None
        self.session = self.route_session or self.session

    def account_stopped(self, method, p):
        """OpenClaw asked the account to stop. Nothing in progress will report again."""
        if not [e for e in self.open.values() if e["type"] != "task"]:
            return self.skip(method)
        if self.turn is not None:
            return self.close_turn("cancelled", method, p, "The account stopped before the turn finished.")
        self.close_open_items("the account stopped")

    # -- tools

    def is_inner(self, tid):
        """A call made through dispatch_action arrives twice: under the model's id, and again
        under an id built from it. The second is the copy."""
        parts = str(tid).split(":")
        return len(parts) > 1 and parts[0] == "tool_call" and parts[1] in self.tools

    @staticmethod
    def tool_title(kind, state):
        if kind == "command":
            return {"running": "Running a command", "done": "Ran a command", "failed": "Command failed",
                    "declined": "Command not allowed"}[state]
        return Claude.tool_title(kind, state)

    def item_event(self, method, p):
        tid, phase, status = p.get("toolCallId"), p.get("phase"), p.get("status")
        if self.is_inner(tid):
            return self.skip("inner copy of a call made through dispatch_action")
        if phase == "start":
            self.tools[tid] = {"name": p.get("name"), "title": p.get("title"), "command": bool(p.get("commandBearing")), "mode": "tool"}
            return self.skip("onItemEvent start, kept until onToolStart gives the arguments")
        t = self.tools.get(tid)
        if t is None or "body" not in t:
            return self.unknown("onItemEvent for a tool call that never started")
        if phase == "update":
            return self.skip("onItemEvent update with nothing new")
        if phase != "end":
            return self.unknown(f"onItemEvent [{phase}]")
        if t.get("ended"):
            return self.skip("onItemEvent end, repeated")
        t["ended"] = True
        state = {"completed": "done", "failed": "failed", "blocked": "failed"}.get(status)
        if state is None:
            return self.unknown(f"onItemEvent end [{status}]")
        if t["mode"] == "question" and t.get("question"):
            if t["question"] not in self.open:
                return self.skip("end of a tool call that is really a question (ask_user)")
            # The tool ended and no answer went through the plugin. The result line says why, in
            # words; the callback does not.
            prev = self.open[t["question"]]
            return self.emit(prev["item"], "question", "cancelled", prev["title"], "Closed with no answer from the chat",
                             prev["body"], "onItemEvent [end of ask_user]", p)
        if t["mode"] == "send" and t.get("message"):
            if state == "done":
                return self.skip("end of a tool call that is really a message (message send)")
            prev = t["message"]
            return self.emit(prev["item"], "message", "failed", prev["title"], prev["text"], prev["body"], "onItemEvent [end of message send]", p)
        if t["command"]:
            return self.skip("onItemEvent end of a command, which closes on onCommandOutput")
        self.ending = {"tid": tid, "state": state, "native": p}  # sent by finish_tool, with the next call

    def tool_start(self, method, p):
        tid, phase = p.get("toolCallId"), p.get("phase")
        if self.is_inner(tid):
            return self.skip("inner copy of a call made through dispatch_action")
        if phase == "update":
            return self.skip("onToolStart update with no arguments")
        if phase != "start":
            return self.unknown(f"onToolStart [{phase}]")
        t = self.tools.setdefault(tid, {"name": p.get("name"), "title": None, "command": False, "mode": "tool"})
        name, args, wrapper = t["name"] or p.get("name"), p.get("args") or {}, None
        if p.get("name") == "dispatch_action":  # a wrapper: the real tool and its arguments are inside
            name, args, wrapper = args.get("id") or name, args.get("args") or {}, "dispatch_action"
        kind = "command" if t["command"] else self.TOOL_KINDS.get(name, "other")
        body = {"kind": kind, "name": name}
        if wrapper:
            body["called_through"] = wrapper
        if kind == "command":
            body["command"] = args.get("command")
        else:
            if kind == "edit":
                body["path"] = args.get("path")
            body["input"] = clip_native(args)
        t.update(name=name, args=args, body=body, text=body.get("command") or body.get("path") or t.get("title") or name)
        if name == "ask_user":
            # A question dressed as a tool call. It becomes a question when it reaches the chat. If
            # OpenClaw rejects the call first, it never does, and is sent as a failed tool.
            t["mode"], self.asking = "question", tid
            return self.skip("tool call that is really a question (ask_user)")
        if name == "message" and args.get("action") == "send":
            t["mode"] = "send"
            return self.skip("tool call that is really a message (message send)")
        self.emit(tid, "tool", "running", self.tool_title(kind, "running"), t["text"], body, f"{method} [{name}]", p)

    def command_output(self, method, p):
        tid, phase = p.get("toolCallId"), p.get("phase")
        if phase == "delta":
            return self.skip("partial output (onCommandOutput delta)")
        if phase != "end":
            return self.unknown(f"onCommandOutput [{phase}]")
        if "itemId" not in p:
            return self.skip("onCommandOutput end with no fields; a second call carries them")
        t = self.tools.get(tid)
        state = {"completed": "done", "failed": "failed"}.get(p.get("status"))
        if t is None or tid not in self.open or state is None:
            return self.unknown("onCommandOutput end for a command that is not open")
        if state == "failed" and tid in self.denied:
            state = "declined"
        t["body"] = {**t["body"], "cwd": p.get("cwd"), "exit_code": p.get("exitCode"), "duration_ms": p.get("durationMs"),
                     "output": clip(p.get("output"), 2000)}
        self.emit(tid, "tool", state, self.tool_title("command", state), t["text"], t["body"], f"{method} [end]", p)

    def finish_tool(self, output=None, delivery=None):
        """Close a tool that is not a command. Its result exists only as a formatted line."""
        e, self.ending = self.ending, None
        t = self.tools[e["tid"]]
        if output is not None:
            t["body"] = {**t["body"], "output": clip(output, 2000)}
        native = {"onItemEvent": e["native"], "deliver": delivery} if delivery is not None else e["native"]
        self.emit(e["tid"], "tool", e["state"], self.tool_title(t["body"]["kind"], e["state"]), t["text"], t["body"],
                  "onItemEvent [end]" + (" and its tool delivery" if delivery is not None else ""), native)

    def approval_event(self, method, p):
        tid, status = p.get("toolCallId"), p.get("status")
        t = self.tools.get(tid)
        if status == "failed":
            return self.skip("onApprovalEvent: the approval request failed, and the tool item already failed")
        if status != "denied" or t is None or "body" not in t:
            return self.unknown(f"onApprovalEvent [{status}]")
        prev = self.events_for(tid)
        if prev and prev["state"] == "declined":
            return self.skip("onApprovalEvent: the denial is already known from the approval")
        # The command was reported as failed one call ago. Only this call says a person said no.
        self.emit(tid, "tool", "declined", self.tool_title(t["body"]["kind"], "declined"), t["text"], t["body"], f"{method} [denied]", p)

    def events_for(self, item):
        return next((e for e in reversed(self.events) if e["item"] == item), None)

    # -- the agent's message tool

    def handle_action(self, method, p):
        """OpenClaw asks the plugin to carry out an action of the agent's message tool."""
        action, params = p.get("action"), p.get("params") or {}
        key = str(params.get("idempotencyKey") or "")
        tid = next((i for i in self.tools if key.endswith(f":{i}")), None)
        t = self.tools.get(tid)
        if t is None or "body" not in t:
            return self.unknown("actions.handleAction with no tool call")
        if action == "send" and t["mode"] == "send":
            body = {"sent_with": "message tool"}
            if t["args"].get("final") is not None:
                body["phase"] = "final" if t["args"]["final"] else "commentary"
            t["message"] = self.emit(f"message:{tid}", "message", "done", "Message", params.get("message"), body, f"{method} [send]", p)
            self.posting = (method, t["message"])
            return
        # react and delete. No item type says "the agent acted on the chat", so the call stays a
        # tool item. What it acted on is only here: the tool arguments name no message for a react.
        target = params.get("messageId") or (p.get("toolContext") or {}).get("currentMessageId")
        t["body"] = {**t["body"], "action": action, "target": {"chat": params.get("to"), "message": target}}
        if target in self.sent:
            t["body"]["subject"] = self.sent[target]["item"]
        self.skip("actions.handleAction: the plugin carries out an action that the tool item shows")

    # -- thinking

    def reasoning_stream(self, method, p):
        self.thinking = str(p.get("text") or "")
        self.skip("partial thinking (onReasoningStream)")

    def reasoning_end(self, method, p):
        text, self.thinking = self.thinking, ""
        if not text.strip():
            return self.skip("onReasoningEnd with no thinking text")
        self.emit(f"reasoning:{self.seq + 1}", "reasoning", "done", "Thinking", text, {},
                  f"{method}, with the last onReasoningStream", {"text": text})

    # -- everything that arrives through deliver

    def deliver(self, method, p):
        payload, info = p[0] or {}, p[1] or {}
        kind, text = info.get("kind"), payload.get("text")
        self.posting = None
        if kind == "tool":
            if (payload.get("channelData") or {}).get("askUser"):
                return self.question_asked(payload, p)
            if self.ending:
                return self.finish_tool(output=text, delivery=p)
            return self.skip("deliver [tool]: formatted line about a tool that is already an item")
        if self.ending:
            self.finish_tool()
        if payload.get("isError"):
            # A failed run reports its outcome first and its error text after.
            turn, self.turn = self.turn, self.turn or self.failed_turn
            self.emit(f"error:{self.seq + 1}", "error", "failed", "Error", text, {}, f"{method} [{kind}, isError]", p)
            self.turn = turn
            return
        if payload.get("isStatusNotice") and kind != "final" and "assistantMessageIndex" not in info:
            # OpenClaw's own line about a directive it took out of a message, sent before the run that
            # the rest of the message starts. "/exec security=full ask=off <task>" is answered this way.
            command = self.last_run.split()[0] if self.last_run and self.last_run.startswith("/") else None
            if self.last_run in self.commands:
                self.commands.remove(self.last_run)
            ev = self.emit(f"notice:{self.seq + 1}", "notice", "done", "Directive taken", text,
                           {"kind": "directive", "command": command}, f"{method} [{kind}, isStatusNotice]", p)
            self.posting = (method, ev)
            return
        if "assistantMessageIndex" in info:  # only the agent's own words carry a message index
            self.ensure_turn()
            ev = self.emit(f"message:{self.seq + 1}", "message", "done", "Message", text,
                           {"phase": "final"} if kind == "final" else {}, f"{method} [{kind}]", p)
            self.posting = (method, ev)
            return
        if kind == "final":
            return self.own_reply(method, text, p)
        self.unknown(f"deliver [{kind}] with no message index")

    def own_reply(self, method, text, p):
        """OpenClaw answers a slash command itself. The reply carries no mark of which command it
        answers, so it is taken as the answer to the oldest one still waiting."""
        if not self.commands:
            return self.unknown("deliver [final] that is not the agent's, with no command waiting")
        command = self.commands.pop(0).split()[0]
        if command != "/stop":
            body = {"kind": "command_output", "command": command}
            if command in ("/new", "/reset") and (p[0] or {}).get("isStatusNotice"):
                # The session was started afresh. Without the plugin hook this reply is the only sign.
                body["kind"] = "new_session"
                if not self.reset_by_hook:
                    self.session_replaced(self.route_key, f"{method} [final, isStatusNotice, reply to {command}]", p)
                self.reset_by_hook = False
            ev = self.emit(f"notice:{self.seq + 1}", "notice", "done", "Command output", text,
                           body, f"{method} [final, reply to {command}]", p)
            self.posting = (method, ev)
            return
        ev = self.emit(f"notice:{self.seq + 1}", "notice", "done", "Stopped", text, {"kind": "stopped", "command": command},
                       f"{method} [final, reply to {command}]", p)
        self.posting = (method, ev)
        if self.turn is not None:  # a stopped run reports no outcome
            self.close_turn("cancelled", self.BY_DRIVER, {"reason": "the run was stopped and reports no outcome"})

    # -- questions

    def question_asked(self, payload, p):
        ask = payload["channelData"]["askUser"]
        # The delivery names the question, not the tool call. It is taken to belong to the one
        # ask_user call in progress, whose arguments hold the question in full.
        t = self.tools.get(self.asking) or {}
        buttons = [b for blk in (payload.get("presentation") or {}).get("blocks") or [] if blk.get("type") == "buttons"
                   for b in blk.get("buttons") or []]
        allow_text = any((b.get("action") or {}).get("intent") == "custom-input" for b in buttons)
        qs = []
        for q in (t.get("args") or {}).get("questions") or []:
            options = [o if isinstance(o, dict) else {"label": o} for o in q.get("options") or []]
            qs.append({"id": q.get("id"), "header": q.get("header"), "text": q.get("question"), "multiple": bool(q.get("multiSelect")),
                       "allow_text": allow_text,
                       "options": [{"id": f"o{k}", "label": o.get("label"), "description": o.get("description"),
                                    "native": o.get("value", o.get("label"))} for k, o in enumerate(options)]})
        if not qs:
            qs = [{"id": "q0", "text": payload.get("text"), "multiple": False, "allow_text": allow_text,
                   "options": [{"id": f"o{k}", "label": v, "native": v} for k, v in enumerate(ask.get("optionValues") or [])]}]
        item = f"question:{ask.get('questionId')}"
        t["question"] = item
        self.emit(item, "question", "waiting", "Question", qs[0]["text"],
                  {"answer_via": "request", "subject": self.asking, "questions": qs}, "deliver [tool, channelData.askUser]", p)

    def question_answered(self, method, result):
        item = (self.tools.get(self.asking) or {}).get("question")
        prev = self.open.get(item) if item else None
        if prev is None or result.get("status") != "answered":
            return self.unknown(f"{method} result [{result.get('status')}]")
        answer = {}
        for q in prev["body"]["questions"]:
            if q["id"] == result.get("questionId"):
                answer[q["id"]] = [o["id"] for o in q["options"] if o["native"] == result.get("optionValue")] or result.get("optionValue")
        self.emit(item, "question", "done", prev["title"], "Answered", {**prev["body"], "answer": answer}, f"result of {method}", result)

    # -- approvals

    def choices(self, allowed):
        return [{"id": f"c{n}", "label": self.DECISIONS.get(d, d), "effect": "deny" if d == "deny" else "allow", "native": d}
                for n, d in enumerate(allowed or [])]

    def approval_card(self, method, p):
        """The plugin claims the approval capability, so the request arrives with its fields."""
        req = p.get("request") or {}
        inner = req.get("request") or {}
        self.emit(f"approval:{req.get('id')}", "approval", "waiting", "Allow this command?", inner.get("command"),
                  {"answer_via": "request", "subject": inner.get("toolCallId"), "choices": self.choices(inner.get("allowedDecisions")),
                   "expires_at_ms": req.get("expiresAtMs")}, method, p)

    def approval_card_resolved(self, method, p):
        resolved = p.get("resolved") or {}
        self.approval_resolved(resolved.get("id"), method, p, resolved.get("decision"))

    def send_payload(self, method, p):
        """A message OpenClaw sends outside a reply. In the recordings every one is an approval."""
        payload = p.get("payload") or {}
        approval = (payload.get("channelData") or {}).get("execApproval")
        if not approval:
            return self.unknown("message.send.payload that is not an approval")
        aid, state = approval.get("approvalId"), approval.get("state")
        if state == "resolved":
            return self.approval_resolved(aid, f"{method} [execApproval resolved]", p)
        if state != "pending":
            return self.unknown(f"message.send.payload [execApproval {state}]")
        if f"approval:{aid}" in self.open:
            return self.skip("message.send.payload: an approval prompt as text, already an item")
        # Without the approval capability the prompt names no tool call. If exactly one command is
        # running, the approval is taken to be about it.
        running = [i for i, e in self.open.items() if e["type"] == "tool" and e["body"].get("kind") == "command"]
        body = {"answer_via": "message", "subject": running[0] if len(running) == 1 else None,
                "choices": self.choices(approval.get("allowedDecisions"))}
        if body["subject"]:
            body["subject_inferred"] = True
        ev = self.emit(f"approval:{aid}", "approval", "waiting", "Allow this command?", payload.get("text"), body,
                       f"{method} [execApproval pending]", p)
        self.posting = (method, ev)

    def approval_resolved(self, aid, native_kind, native, decision=None):
        """An approval can report its outcome three times. The first closes the item."""
        prev = self.open.get(f"approval:{aid}")
        if prev is None:
            return self.skip(f"{native_kind.split(' [')[0]}: an approval outcome that is already known")
        # The resolved message does not say which way. The decision is the one the client sent.
        decision = decision or self.decisions.get(aid)
        chosen = next((c for c in prev["body"]["choices"] if c["native"] == decision), None)
        if chosen is None:
            return self.unknown("an approval resolved, and which way is only in the wording")
        allow = chosen["effect"] == "allow"
        self.emit(prev["item"], "approval", "done" if allow else "declined", prev["title"], f"Answered: {chosen['label']}",
                  {**prev["body"], "answer": {"choice": chosen["id"]}}, native_kind, native)
        if not allow and prev["body"].get("subject"):
            self.denied.add(prev["body"]["subject"])


# ---------------------------------------------------------------------------- driver


def translate(path: Path) -> Translator:
    recs = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    if any(r.get("via") == "hook" and r["msg"].get("method") == "gateway.startAccount" for r in recs):
        t = OpenClaw()  # Hermes and OpenClaw are both recorded as hooks; only OpenClaw starts an account
    elif any(r.get("via") == "hook" for r in recs):
        t = Hermes()
    elif any("via" in r for r in recs):
        t = Claude()
    else:
        t = Codex()
    for r in recs:
        t.feed(r)
    return t


def check(dirs: list[Path]) -> int:
    empty = [str(d) for d in dirs if not any(d.glob("*.jsonl"))]
    if empty or not dirs:  # a mistyped folder must not pass as "nothing without a rule"
        print("no recordings in:", ", ".join(empty) or "no folder was given")
        return 1
    total_native = total_events = 0
    unmapped, not_sent, types = collections.Counter(), collections.Counter(), collections.Counter()
    left_open = []
    sessions = collections.defaultdict(lambda: [0, 0, 0, 0])  # harness -> items, edits, largest body, recordings
    controls = collections.Counter()
    for d in dirs:
        for path in sorted(d.glob("*.jsonl")):
            t = translate(path)
            n = sum(1 for _ in path.read_text(encoding="utf-8").splitlines())
            total_native += n
            total_events += len(t.events)
            unmapped.update({f"{t.harness}: {k}": v for k, v in t.unmapped.items()})
            not_sent.update({f"{t.harness}: {k}": v for k, v in t.not_sent.items()})
            types.update(f"{e['type']}" for e in t.events)
            evs = [e for e in t.events if e["type"] == "session"]
            row = sessions[t.harness]
            row[0] += len(t.sessions)
            row[1] += len(evs) - len(t.sessions)
            row[2] = max([row[2]] + [len(json.dumps(e["body"], ensure_ascii=False).encode("utf-8")) for e in evs])
            row[3] += 1
            controls.update({(t.harness, *k): v for k, v in t.controls.items()})
            still = [e for e in t.open.values() if e["type"] not in ("task", "session")]
            if still:
                left_open.append((path.name, [f"{e['type']}:{e['state']}" for e in still]))
            print(f"{t.harness:8s} {path.stem:18s} {n:4d} native messages -> {len(t.events):3d} agent events")
    print(f"\n{total_native} native messages -> {total_events} agent events")
    print("\nagent events by type:")
    for k, v in types.most_common():
        print(f"  {v:4d}  {k}")
    print("\nsession items:")
    for h, (items, edits, largest, n) in sessions.items():
        print(f"  {h:8s} {n:3d} recordings  {items:3d} sessions  {edits:3d} edits  largest body {largest} bytes")
    print("\ncontrols, and the native call each was seen as:")
    for (h, name, native), v in sorted(controls.items()):
        print(f"  {v:4d}  {h:8s} {name:32s} {native}")
    print("\nnot sent, on purpose:")
    for k, v in sorted(not_sent.items(), key=lambda x: (-x[1], x[0])):
        print(f"  {v:4d}  {k}")
    print("\nitems left open at the end of a recording:", left_open or "none")
    print("\nnative messages with no translation:", "none" if not unmapped else "")
    for k, v in unmapped.most_common():
        print(f"  {v:4d}  {k}")
    return 1 if unmapped else 0


def main() -> int:
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return 2
    if args[0] == "--check":
        return check([Path(a) for a in args[1:]])
    brief = "--brief" in args
    for ev in translate(Path(next(a for a in args if not a.startswith("--")))).events:
        if brief:  # leave out the native payload and the ids, to read the flow at a glance
            ev = {k: ev[k] for k in ("seq", "type", "state", "title", "text", "body")}
        print(json.dumps(ev, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
