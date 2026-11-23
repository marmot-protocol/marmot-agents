"""The Hermes plugin's translator: what Hermes hands a platform, as agent events.

It reads each line as `tools/record/hermes` records it, `{"dir", "via", "msg"}`, and returns the agent
events the line makes, in the shape the item types read (`crates/marmot-agent-events`). It does no
I/O. Its rules are the prototype's (`tools/prototype/translate.py`, class `Hermes`), and its test
replays every Hermes recording against the prototype's expected events (D-074).

Hermes hands a platform formatted text, not tool events. A line about a tool, or a heartbeat, is
Hermes's running commentary: a message in the commentary phase. What Hermes says for itself is a
notice. Approvals, questions and requests to confirm a command arrive in a structured form and become
`approval` and `question` items (D-048). Hermes names no session to a platform, so a session is named
after the chat and the sender, and counted when Hermes replaces it. In a group Hermes keeps a session
for each sender, so turns can overlap: what names its session or its turn (an approval card, a
question, the end of a turn, a reply to a message) goes to that turn, and the rest to the latest.

The native message of an event is the whole line's `msg` (D-059). An event the translator makes up,
such as an item it closes because the turn ended, carries none.
"""

from __future__ import annotations

import collections
import json

# Hermes reports no version to a plugin, so the connector supplies it (D-044). The recordings were
# made with this release, commit 40d1e95f.
VERSION = "0.23.2"

NOT_SENT = {"send_typing", "stop_typing", "connect", "disconnect", "get_chat_info", "format_tool_preview",
            "format_tool_event", "send_final_ledgered", "send_exec_approval", "render_message_event"}
# What Hermes says for itself that a person should know. Its answer to a slash command is that
# command's output instead, whatever its first character.
WARNINGS = ("📬", "↪", "⚠️ Your message was interrupted", "⏸")
# Plugin hooks that say nothing a platform call does not already say.
HOOKS_NOT_SENT = {"pre_command", "on_session_end", "on_session_finalize", "on_session_open", "agent_loop_stopped",
                  "pre_approval_request", "post_approval_response"}
# What a person types to answer Hermes's request to confirm one of their own commands, in the order of
# the options the question offers.
CONFIRM = ("/approve", "/always", "/cancel")
CONFIRM_LABELS = ("Approve once", "Always approve", "Cancel")
# What a person types to answer an approval Hermes sent as text, in the order of its choices.
TEXT_CHOICES = [("Allow once", "allow", "/approve"), ("Allow for this session", "allow", "/approve session"),
                ("Always allow", "allow", "/approve always"), ("Deny", "deny", "/deny")]
EFFORTS = ("none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra")
OPEN = ("running", "waiting")


def command(name, description=None, hint=None):
    """A slash command as a session item lists it: always a name and a hint (D-045)."""
    return {"name": name, "description": description, "hint": hint or ""}


def setting(id_, label, value, options=None, approve=False):
    return {"id": id_, "label": label, "value": value, "options": options, "approve": approve}


def shaped(value, key=None):
    """An event as the item types write it: no null, no `secret` or `approve` that is false, and no
    empty list of options in an answer. The native message is kept exactly as it is."""
    if isinstance(value, dict):
        out = {}
        for k, v in value.items():
            if key == "native" and k == "payload":
                out[k] = v
                continue
            if v is None or (k in ("secret", "approve") and v is False) or (key == "answer" and k == "options" and v == []):
                continue
            out[k] = shaped(v, k)
        return out
    if isinstance(value, list):
        return [shaped(v, key) for v in value]
    return value


def reply_to_question(question, text):
    """What a typed answer records (D-059): the option it names, by its number or its label, or else
    the text as typed. Only ASCII digits are a number: `²` passes `isdigit()` but is not one."""
    options = question.get("options") or []
    chosen = next((o for o in options if o["label"] == text), None)
    if chosen is None and text.isascii() and text.isdigit() and 1 <= int(text) <= len(options):
        chosen = options[int(text) - 1]
    if chosen is not None:
        return {"question": question["id"], "options": [chosen["id"]]}
    return {"question": question["id"], "options": [], "text": text}


def key_of(source):
    """A Hermes session's chat and sender, as Hermes keeps one session for each."""
    return f"{source.get('chat_type')}:{source.get('chat_id')}:{source.get('user_id')}"


class Translator:
    """One chat's Hermes calls, as agent events.

    `ids` is put in front of the ids of items Hermes gives no id of its own, such as a message, so
    they never repeat after a restart. `generations` counts the sessions each chat and sender has had;
    the connector keeps it across restarts, so a replaced session's name is never taken again."""

    def __init__(self, version=VERSION, ids="", generations=None):
        self.version = version
        self.ids = ids
        self.generation = {} if generations is None else generations
        self.seq = 0  # every event made, which names the items Hermes gives no id
        self.seq_in = collections.Counter()  # session -> its events, for each event's `seq`
        self.open: dict[str, dict] = {}  # item -> its last event, while it is running or waiting
        self.last: dict[str, dict] = {}  # item -> its last event
        self.unknown = collections.Counter()
        self.session = None
        self.turn = None
        self.msg = None
        self.out: list[dict] = []
        self.sessions: dict[str, str] = {}  # session item -> its body as last sent
        self.sent: dict[str, dict] = {}  # the id Hermes was given for a message it sent -> its event
        self.last_send = None
        self.approval = None
        self.natives: dict[str, list] = {}  # approval item -> Hermes's value for each of its choices
        self.question = None
        self.confirm = None
        self.confirm_command = None
        self.final_next = False
        self.key = None
        self.turn_command = None
        self.settings: dict[str, dict] = {}
        self.hermes_ids: dict[str, str] = {}
        self.commands: list[dict] = []
        self.typed: dict[str, tuple] = {}
        self.typed_key: dict[str, str] = {}  # message typed -> the chat and sender whose session it reaches
        # A read of a session's model -> (the chat and sender, their session count and switches then). A read
        # the plugin named by an id is kept by it; one it did not, as in a recording, by the chat and sender.
        self.reads: dict[str, tuple[str, int, int]] = {}
        self.switches = collections.Counter()  # chat and sender -> the model switches Hermes took
        self.switched: set[str] = set()  # chats and senders whose next model read follows a switch
        self.model_read = False  # the line read was a model read that the session now shows
        self.reset_by_hook = False
        self.turn_key: dict[str, str] = {}  # turn -> the chat and sender whose session it runs in, oldest first
        self.questions: dict[str, str | None] = {}  # question item -> the chat and sender it asks, if known

    # -- making events

    def emit(self, item, type_, state, title, text="", body=None, native=True, owner=None):
        """`native` is True for the line being read, or None for an event the translator made up."""
        session, turn = owner if owner is not None else (self.session, self.turn)
        self.seq += 1
        self.seq_in[session] += 1
        ev = {"harness": "hermes", "session": session, "turn": turn, "item": item, "seq": self.seq_in[session],
              "type": type_, "state": state, "title": title, "text": "" if text is None else str(text),
              "body": body or {}}
        if native is not None:
            ev["native"] = {"kind": self.kind, "payload": self.msg}
        ev = shaped(ev)
        self.out.append(ev)
        self.last[item] = ev
        if state in OPEN:
            self.open[item] = ev
        else:
            self.open.pop(item, None)
        return ev

    def close_open_items(self, keep_turns=()):
        """A turn ended: anything still open will never report again (the turn closes itself), except
        what belongs to another turn still running."""
        for item, ev in list(self.open.items()):
            if ev["type"] in ("turn", "task", "session") or ev.get("turn") in keep_turns:
                continue
            self.emit(item, ev["type"], "cancelled", ev["title"], ev["text"], ev["body"], native=None,
                      owner=(ev.get("session"), ev.get("turn")))

    def session_item(self, name, native=True, always=False):
        """The session item of `name`, sent only when its body changed, unless `always`."""
        item = f"session:{name}"
        body = {"harness_version": self.version, "settings": list(self.settings.setdefault(name, {}).values()),
                "actions": [], "commands": list(self.commands)}
        key = json.dumps(body, sort_keys=True)
        if self.sessions.get(item) == key and not always:
            return
        self.sessions[item] = key
        text = ", ".join(f"{x['label']}: {x['value']}" for x in body["settings"] if x.get("value") is not None)
        self.emit(item, "session", "running", "Session", text, body, native, owner=(name, None))

    # -- reading

    def feed(self, line) -> list[dict]:
        """The events one recorded line makes."""
        self.out = []
        self.model_read = False
        m, via = line["msg"], line.get("via")
        self.msg = m
        # The native message's kind is what the line is: the method Hermes called on the adapter, the
        # plugin hook it called, the call the plugin made, or the plugin's own "inbound" or "press".
        self.kind = str(m.get("hook") if via == "plugin_hook" else m.get("method") if via in ("hook", "call") else via)
        if via == "inbound":
            self.inbound(m)
        elif via == "press":
            self.press(m)
        elif via == "plugin_hook":
            self.plugin_hook(m)
        elif via == "call":
            if line.get("dir") == "in":
                self.asked(m.get("method"), m)
            else:
                src = (m.get("params") or {}).get("source")
                if src and m.get("method") == "get_session_model_settings":
                    key = key_of(src)
                    read = str(m["id"]) if "id" in m else key
                    self.reads[read] = (key, self.generation.get(key, 1), self.switches[key])
                elif src:
                    self.key = key_of(src)
        elif via == "result":
            method = m.get("method")
            if method in ("send", "_send_exec_approval_prompt") and self.last_send is not None:
                mid = (m.get("result") or {}).get("message_id")
                if mid:
                    self.sent[mid] = self.last_send
        elif via == "hook":
            self.hook(m)
        elif via != "note":
            self.unknown[f"line via {via}"] += 1
        return self.out

    def hook(self, m):
        method, p = m.get("method"), m.get("params") or {}
        if method in NOT_SENT:
            if method == "send_final_ledgered":
                self.final_next = True
            return
        handler = {
            "on_processing_start": self.turn_start, "on_processing_complete": self.turn_end, "send": self.send,
            "edit_message": self.edit, "send_clarify": self.clarify, "_send_exec_approval_prompt": self.approval_card,
            "send_slash_confirm": self.confirm_card, "send_model_picker": self.model_picker,
        }.get(method)
        if handler is None:
            self.unknown[str(method)] += 1
            return
        handler(p)

    # -- what people send

    def typed_control(self, word, args):
        first = args[0] if args else ""
        if word == "/stop":
            return "interrupt", word
        if word in ("/new", "/reset"):
            return "new_session", word
        if word == "/model" and first and not first.startswith("-"):
            return "set model", word
        if word == "/reasoning" and first in EFFORTS:
            return "set reasoning", word
        if word == "/yolo":
            return "set (permission) yolo", word
        if word == "/approvals" and first:
            return "set (permission) approvals", word
        if word in ("/memory", "/skills") and first in ("approval", "mode") and len(args) > 1:
            return "set (permission) write approval", word
        return "(slash command)", word

    def inbound(self, m):
        text = str(m.get("text") or "").strip()
        word, *args = text.split() or [""]
        confirming = bool(self.confirm and self.confirm in self.open and word in CONFIRM)
        if not confirming and word.startswith("/") and word not in ("/approve", "/deny"):
            self.typed[m.get("message_id")] = self.typed_control(word, args)
        if m.get("chat_type"):
            # The plugin says what kind of chat a message is typed in, so a reply to it reaches the
            # sender's own session, whichever turn is the latest.
            self.typed_key[m.get("message_id")] = key_of({"chat_type": m["chat_type"], "chat_id": m.get("chat"),
                                                          "user_id": m.get("user")})
        if confirming or (self.approval and word in ("/approve", "/deny")) or word.startswith("/"):
            # A slash command is never the answer to a question: Hermes leaves the question waiting.
            return
        item = self.question_for(m.get("chat"), m.get("user"))
        if item is not None:
            self.answered(item, text, m.get("user"))

    def question_for(self, chat, user):
        """The latest open question that a message from `user` in `chat` answers: Hermes takes a
        message as the answer to a question of the sender's own session."""
        for item in reversed(list(self.questions)):
            key = self.questions[item]
            if item in self.open and (key is None or key.endswith(f":{chat}:{user}")):
                return item
        return None

    def answered(self, item, text, user=None):
        prev = self.open[item]
        q = prev["body"]["questions"][0]
        self.emit(item, "question", "done", prev["title"], "Answered", {**prev["body"], "answer": [reply_to_question(q, text)]},
                  owner=(prev.get("session"), prev.get("turn")))
        self.questions.pop(item, None)
        if self.question == item:
            self.question = None

    def press(self, m):
        """An answer the plugin handed Hermes directly, which Hermes took: a press on an approval
        card, by its request id, or the answer to a question, by its id."""
        if "pick_model" in m:
            return
        if m.get("clarify_id") is not None:
            item = f"question:{m['clarify_id']}"
            if item in self.open:
                self.answered(item, str(m.get("text") or ""), m.get("user"))
            return
        item = f"approval:{m['request_id']}" if m.get("request_id") else self.approval
        prev = self.open.get(item) if item else None
        if prev is None:
            return
        natives = self.natives.get(item) or []
        chosen = next((c for c, n in zip(prev["body"]["choices"], natives) if n == m.get("choice")), None)
        allow = bool(chosen and chosen["effect"] == "allow")
        self.emit(item, "approval", "done" if allow else "declined", prev["title"],
                  "Answered: " + (chosen["label"] if chosen else str(m.get("choice"))),
                  {**prev["body"], "answer": {"choice": chosen["id"] if chosen else None}},
                  owner=(prev.get("session"), prev.get("turn")))
        if self.approval == item:
            self.approval = None

    # -- sessions

    def name(self, key):
        n = self.generation.setdefault(key, 1)
        return key if n == 1 else f"{key}#{n}"

    def model_switched(self, key):
        """Hermes took a switch of model for the session of `key`: a read of its model started before
        says nothing of now, and the next read shows the session."""
        self.switches[key] += 1
        self.switched.add(key)

    def session_changed(self, key, native=True):
        self.session_item(self.name(key), native)

    def put(self, key, id_, label, value, options=None, approve=False):
        known = self.settings.setdefault(self.name(key), {})
        if options is None and id_ in known:
            options = known[id_]["options"]
        known[id_] = setting(id_, label, value, options, approve)

    def session_replaced(self, key):
        name = self.name(key)
        prev = self.last.get(f"session:{name}")
        if prev is not None and prev["state"] == "running":
            self.emit(prev["item"], "session", "done", "Session ended", prev["text"], prev["body"], owner=(name, None))
        self.generation[key] = self.generation.get(key, 1) + 1
        self.session_changed(key)
        if self.turn is not None and self.key == key:
            self.session = self.name(key)

    def plugin_hook(self, m):
        hook, p = m.get("hook"), m.get("params") or {}
        if hook == "on_session_start":
            if self.key is None:
                return
            self.hermes_ids[p.get("session_id")] = self.key
            self.put(self.key, "model", "Model", p.get("model"))
            return self.session_changed(self.key)
        if hook == "on_session_reset":
            key = self.hermes_ids.get(p.get("old_session_id")) or self.key
            if key is None:
                self.unknown["on_session_reset for a chat the translator cannot name"] += 1
                return
            self.hermes_ids[p.get("new_session_id")] = key
            self.reset_by_hook = True
            return self.session_replaced(key)
        if hook not in HOOKS_NOT_SENT:
            self.unknown[f"plugin hook {hook}"] += 1

    def asked(self, method, m):
        if "error" in m:
            return
        result = m.get("result")
        if method == "hermes_cli.commands.COMMAND_REGISTRY":
            self.commands = [command(c.get("name"), c.get("description"), c.get("args_hint"))
                             for c in result or [] if c.get("gateway_available")]
            # The sessions this chat has shown: the count of sessions is shared with other chats.
            for key in list(self.generation):
                if f"session:{self.name(key)}" in self.sessions:
                    self.session_changed(key)
        elif method == "get_session_model_settings" and isinstance(result, dict):
            # The session Hermes names (`agent:main:<platform>:<chat type>:<chat>:<sender>`), as it was
            # when asked: a read that a new session, or a switch of model, overtook says nothing of now.
            named = str(result.get("session_key"))
            read = str(m["id"]) if "id" in m else next(
                (r for r in reversed(list(self.reads)) if named.endswith(":" + self.reads[r][0])), None)
            if read not in self.reads:
                return
            key, generation, switches = self.reads.pop(read)
            if (not named.endswith(":" + key) or generation != self.generation.get(key, 1)
                    or switches != self.switches[key]):
                return
            self.put(key, "model", "Model", result.get("model"))
            self.put(key, "provider", "Provider", result.get("provider"))
            self.model_read = True
            # The read after a switch shows the session even when nothing changed, as a reset to the
            # model shown: it is the switch's outcome.
            self.session_item(self.name(key), always=key in self.switched)
            self.switched.discard(key)
        elif method == "list_session_model_picker_providers" and isinstance(result, list):
            self.model_options(self.key, result)
        elif method == "on_model_selected":
            self.emit(f"sent:{self.ids}{self.seq + 1}", "notice", "done", "Command output", result,
                      {"kind": "command_output"})

    def model_options(self, key, providers):
        current = next((p for p in providers if p.get("is_current")), None)
        options = [x for p in providers for x in p.get("models") or []]
        known = self.settings.setdefault(self.name(key), {})
        self.put(key, "model", "Model", (known.get("model") or {}).get("value"), options)
        if current:
            self.put(key, "provider", "Provider", current.get("slug"), [p.get("slug") for p in providers])
        self.session_changed(key)

    def model_picker(self, p):
        if self.key is not None:
            known = self.settings.setdefault(self.name(self.key), {})
            if p.get("current_model") and "model" not in known:
                self.put(self.key, "model", "Model", p.get("current_model"))
            self.model_options(self.key, p.get("providers") or [])
        self.last_send = None

    # -- turns

    def turn_start(self, p):
        ev = p.get("event") or {}
        self.key = key_of(ev.get("source") or {})
        self.session = self.name(self.key)
        text = str(ev.get("text") or "").strip()
        self.turn_command = text.split()[0] if text.startswith("/") else None
        self.session_changed(self.key, native=None)
        self.turn = ev.get("message_id")
        self.turn_key.pop(self.turn, None)
        self.turn_key[self.turn] = self.key
        self.emit(f"turn:{self.turn}", "turn", "running", "Working", body={"started_by": "user"})

    def turn_end(self, p):
        """The end of the turn Hermes names by its message, or else of the latest turn."""
        turn = p.get("message_id") if p.get("message_id") in self.turn_key else self.turn
        if turn is None:
            return
        latest = turn == self.turn
        others = [t for t in self.turn_key if t != turn and f"turn:{t}" in self.open]
        outcome = p.get("outcome")
        state = {"success": "done", "failure": "failed", "cancelled": "cancelled"}.get(outcome, "done")
        still = [i for i, e in self.open.items()
                 if e["type"] not in ("turn", "task", "session") and e.get("turn") not in others]
        if state != "done" or not still:
            self.close_open_items(keep_turns=others)
        title = {"done": "Finished", "failed": "Failed", "cancelled": "Interrupted"}[state]
        session = self.session if latest else self.open.get(f"turn:{turn}", {}).get("session", self.name(self.turn_key[turn]))
        self.emit(f"turn:{turn}", "turn", state, title, body={"started_by": "user"}, owner=(session, turn))
        if latest and others:
            # Another sender's turn is still running: what follows is its.
            self.turn, self.key = others[-1], self.turn_key[others[-1]]
            self.session = self.open[f"turn:{self.turn}"]["session"]

    def owner_of(self, turn):
        """The session and turn to give an event of `turn` when that turn is running and is not the
        latest, or else None: the latest turn's."""
        ev = self.open.get(f"turn:{turn}") if turn is not None and turn != self.turn else None
        return (ev["session"], turn) if ev is not None else None

    def turn_of_session(self, session_key):
        """The running turn of the Hermes session `session_key`, which Hermes names after the chat and
        the sender (`agent:main:<platform>:<chat type>:<chat>:<sender>`), if one is running."""
        for turn, key in reversed(list(self.turn_key.items())):
            if session_key and str(session_key).endswith(":" + key) and f"turn:{turn}" in self.open:
                return turn
        return None

    # -- text

    def command_reply(self, item, content, typed, key):
        """Hermes's reply to a command typed in the session of `key`."""
        title = "Command output"
        if content.startswith("⛔"):
            title = "Not allowed"
        elif content.startswith("✨"):
            if not self.reset_by_hook:
                self.session_replaced(key)
            self.reset_by_hook = False
        elif typed and typed[0].startswith("set "):
            words = content.split()
            if typed[0] == "set (permission) yolo" and "YOLO" in content:
                on = "**ON**" in content
                self.put(key, "yolo", "Skip approvals", "on" if on else "off", ["on", "off"], approve=True)
            elif typed[0] == "set (permission) approvals" and content.startswith("Approval mode:") and len(words) > 2:
                self.put(key, "approvals", "Approval mode", words[2], approve=True)
            elif typed[0] == "set reasoning" and "`" in content:
                self.put(key, "reasoning", "Reasoning effort", content.split("`")[1])
            elif typed[0] == "set (permission) write approval" and ".write_approval set to '" in content:
                what = content.split(".write_approval")[0].split()[-1]
                self.put(key, f"{what}_write_approval", f"Ask before writing {what}", content.split("'")[1],
                         ["on", "off"], approve=True)
            self.session_changed(key)
        self.last_send = self.emit(item, "notice", "done", title, content, {"kind": "command_output"})

    def send(self, p):
        content = str(p.get("content") or "")
        meta = p.get("metadata") or {}
        item = f"sent:{self.ids}{self.seq + 1}"
        final, self.final_next = self.final_next, False
        if meta.get("is_approval_prompt") or content.startswith("⚠️ **Hermes wants"):
            self.approval = item
            self.natives[item] = [native for _, _, native in TEXT_CHOICES]
            choices = [{"id": f"c{n}", "label": label, "effect": effect}
                       for n, (label, effect, _) in enumerate(TEXT_CHOICES)]
            self.last_send = self.emit(item, "approval", "waiting", "Allow this command?", content, {"choices": choices})
            return
        if content.startswith(("✅ Command approved", "❌ Command denied", "⌛ Approval timed out")) and self.approval in self.open:
            prev = self.open[self.approval]
            state = "done" if content.startswith("✅") else "declined" if content.startswith("❌") else "cancelled"
            self.last_send = self.emit(self.approval, "approval", state, prev["title"], content, prev["body"])
            self.approval = None
            return
        if content.startswith("❓") and self.question in self.open:
            self.last_send = None
            return
        if self.confirm in self.open:
            prev = self.open[self.confirm]
            if content == prev["text"]:
                self.last_send = prev
                return
            if self.turn_command in CONFIRM:
                q = prev["body"]["questions"][0]
                chosen = q["options"][CONFIRM.index(self.turn_command)]
                self.emit(self.confirm, "question", "done", prev["title"], f"Answered: {chosen['label']}",
                          {**prev["body"], "answer": [{"question": q["id"], "options": [chosen["id"]]}]})
                self.confirm, self.turn_command = None, self.confirm_command
        typed = self.typed.get(p.get("reply_to"))
        key = self.typed_key.get(p.get("reply_to")) or self.key
        if self.turn_command and (meta.get("notify") or final):
            return self.command_reply(item, content, typed, key)
        if typed:
            return self.command_reply(item, content, typed, key)
        if content.startswith(WARNINGS):
            self.last_send = self.emit(item, "notice", "done", "Notice", content, {"kind": "warning"})
            return
        phase = "final" if (final or meta.get("notify")) else "commentary"
        owner = self.owner_of(p.get("reply_to") or meta.get("reply_to_message_id"))
        self.last_send = self.emit(item, "message", "done", "Message", content, {"phase": phase}, owner=owner)

    def edit(self, p):
        prev = self.sent.get(p.get("message_id"))
        if prev is None:
            self.unknown["edit_message for a message that was not sent"] += 1
            return
        self.emit(prev["item"], prev["type"], prev["state"], prev["title"], p.get("content"), prev["body"],
                  owner=(prev.get("session"), prev.get("turn")))

    # -- the structured calls

    def clarify(self, p):
        qs = [{"id": "q0", "text": p.get("question"), "multiple": False, "allow_text": True,
               "options": [{"id": f"o{k}", "label": c} for k, c in enumerate(p.get("choices") or [])]}]
        self.question = f"question:{p.get('clarify_id')}"
        turn = self.turn_of_session(p.get("session_key"))
        self.questions[self.question] = self.turn_key.get(turn, self.key)
        self.emit(self.question, "question", "waiting", "Question", p.get("question"),
                  {"answer_via": "message", "questions": qs}, owner=self.owner_of(turn))

    def approval_card(self, p):
        prompt = p.get("prompt") or {}
        actions = prompt.get("actions") or []
        self.approval = f"approval:{prompt.get('request_id')}"
        self.natives[self.approval] = [choice for _label, choice, _style in actions]
        choices = [{"id": f"c{n}", "label": label, "effect": "deny" if choice == "deny" else "allow"}
                   for n, (label, choice, _style) in enumerate(actions)]
        owner = self.owner_of(self.turn_of_session(prompt.get("session_key")))
        self.last_send = self.emit(self.approval, "approval", "waiting", "Allow this command?", prompt.get("command"),
                                   {"choices": choices}, owner=owner)

    def confirm_card(self, p):
        options = [{"id": f"o{n}", "label": label} for n, label in enumerate(CONFIRM_LABELS)]
        self.confirm = f"confirm:{p.get('confirm_id')}"
        self.confirm_command = p.get("title")
        qs = [{"id": "q0", "text": p.get("message"), "multiple": False, "allow_text": False, "options": options}]
        self.emit(self.confirm, "question", "waiting", "Confirm this command?", p.get("message"),
                  {"answer_via": "message", "questions": qs})
