#!/usr/bin/env python3
"""Design prototype: translate a recorded harness session into the proposed agent events.

This is not driver code. It exists to test the event model in docs/design/event-model.md
against real recordings: every field in an agent event has to come from somewhere, and every
native message has to be either translated or deliberately not sent.

Usage:
    translate.py <recording.jsonl>             print the agent events, one JSON object per line
    translate.py <recording.jsonl> --brief     the same, without ids and native payloads
    translate.py --check <recordings-dir>...   translate every recording and report coverage

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
            if ev["type"] in ("turn", "task"):
                continue  # tasks may outlive the turn; the turn closes itself
            self.emit(
                item, ev["type"], "cancelled", ev["title"], ev["text"], ev["body"],
                native_kind="(none: closed by the driver)", native={"reason": reason},
            )

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

    def feed(self, rec):
        m, out = rec["msg"], rec["dir"] == "out"
        if "_recorder" in m:
            return
        method = m.get("method")
        if out:
            if method:
                self.pending[("c", m.get("id"))] = method
                if "id" in m:
                    self.last_request = method
                if method == "turn/steer":
                    return self.skip("client: turn/steer")  # a control; the harness echoes it as a userMessage
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
                self.session = (p.get("thread") or {}).get("id")
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
        if method in ("thread/start", "thread/resume") and isinstance(result.get("thread"), dict):
            self.session = result["thread"].get("id")
        self.skip(f"response to {method}")

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

    def feed(self, rec):
        m, via, out = rec["msg"], rec.get("via"), rec["dir"] == "out"
        if via == "prompt":
            self.prompted = True
            return self.skip("client: user message" + (" (steer)" if m.get("priority") else ""))
        if via == "call":
            if "error" in m:
                return self.skip("client library: query() threw at the end of a failed turn")
            return self.skip(f"client: {m.get('method')}()" if out else f"result of {m.get('method')}()")
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
            if self.turn is None:
                return self.start_turn(m)
            return self.skip("system / init inside a turn")
        if st in self.NOT_SENT_SYSTEM:
            return self.skip(f"system / {st}")
        if st == "status":
            status = m.get("status")
            if status == "compacting":
                self.ensure_turn(m)
                return self.emit(f"compaction:{m.get('uuid')}", "notice", "running", "Compacting the conversation", "",
                                 {"kind": "compaction"}, "system / status", m)
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


# ---------------------------------------------------------------------------- driver


def translate(path: Path) -> Translator:
    recs = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    t = Claude() if any("via" in r for r in recs) else Codex()
    for r in recs:
        t.feed(r)
    return t


def check(dirs: list[Path]) -> int:
    total_native = total_events = 0
    unmapped, not_sent, types = collections.Counter(), collections.Counter(), collections.Counter()
    left_open = []
    for d in dirs:
        for path in sorted(d.glob("*.jsonl")):
            t = translate(path)
            n = sum(1 for _ in path.read_text(encoding="utf-8").splitlines())
            total_native += n
            total_events += len(t.events)
            unmapped.update({f"{t.harness}: {k}": v for k, v in t.unmapped.items()})
            not_sent.update({f"{t.harness}: {k}": v for k, v in t.not_sent.items()})
            types.update(f"{e['type']}" for e in t.events)
            still = [e for e in t.open.values() if e["type"] != "task"]
            if still:
                left_open.append((path.name, [f"{e['type']}:{e['state']}" for e in still]))
            print(f"{t.harness:6s} {path.stem:18s} {n:4d} native messages -> {len(t.events):3d} agent events")
    print(f"\n{total_native} native messages -> {total_events} agent events")
    print("\nagent events by type:")
    for k, v in types.most_common():
        print(f"  {v:4d}  {k}")
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
