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
question, the end of a turn, a reply to a message) goes to that turn, and the rest to the latest. A
request to confirm a command is kept by Hermes's session key and its id, as Hermes keeps one in each
session: it waits until it is answered, a newer one replaces it in its session, or Hermes drops it, and
never closes with a turn. The plugin's look at what Hermes holds (`CONFIRM_LOOK`) says which (D-155).

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
# The options of Hermes's request to confirm one of a person's own commands, which they can also type as
# `/approve`, `/always` and `/cancel`, among others (`CONFIRM_WORDS`).
CONFIRM_LABELS = ("Approve once", "Always approve", "Cancel")
# The same answers, as the plugin hands them to Hermes's own call for them (`tools.slash_confirm.resolve`),
# which answers only the confirmation it names (D-146).
CONFIRM_CHOICES = ("once", "always", "cancel")
# Everything Hermes reads as a typed answer to a confirmation in the sender's own session, as the pinned
# commit reads it (`_hm_slash_confirm_reply` in `gateway/run_inbound.py`): a command by these names, or
# these words alone, in English, which Hermes always accepts beside the language it is set to
# (`approval.inputs.confirm_*`). A word of another language that Hermes takes shows as a confirmation
# dropped, not answered (D-155).
CONFIRM_COMMANDS = {"approve": "once", "yes": "once", "ok": "once", "confirm": "once", "always": "always",
                    "remember": "always", "cancel": "cancel", "no": "cancel", "deny": "cancel", "nevermind": "cancel"}
CONFIRM_WORDS = {**{w: "once" for w in ("approve", "approve once", "once", "yes", "ok", "confirm")},
                 **{w: "always" for w in ("always", "always approve", "remember")},
                 **{w: "cancel" for w in ("cancel", "nevermind", "no", "deny")}}
# The plugin's look at which confirmation Hermes holds in a session (`tools.slash_confirm.get_pending`).
CONFIRM_LOOK = "tools.slash_confirm.get_pending"
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


def confirm_choice(text) -> str | None:
    """The answer Hermes reads in `text` when a confirmation waits in the sender's session (`CONFIRM_WORDS`),
    as `tools.slash_confirm.resolve` takes it, or None when Hermes reads none."""
    text = str(text or "").strip()
    if text.startswith("/"):
        name = text.split(maxsplit=1)[0][1:].lower().split("@", 1)[0]
        if name in CONFIRM_COMMANDS:
            return CONFIRM_COMMANDS[name]
    return CONFIRM_WORDS.get(text.lstrip("!/").lower())


def usable_settings(result) -> bool:
    """Whether Hermes's answer to a read of a session's model has one: its getter can name the session
    and give a blank model, when none is configured (`gateway/platforms/session_model.py`)."""
    return isinstance(result, dict) and isinstance(result.get("model"), str) and bool(result["model"].strip())


def key_of(source):
    """A Hermes session's chat and sender, as Hermes keeps one session for each."""
    return f"{source.get('chat_type')}:{source.get('chat_id')}:{source.get('user_id')}"


def names_session(named, key) -> bool:
    """Whether Hermes's name for a session, `named`, is the session of `key` (`key_of`). Hermes names a
    group's session after the chat and the sender, `agent:main:<platform>:group:<chat>:<sender>`, and a
    direct chat's after the chat alone, `agent:main:<platform>:dm:<chat>`, as `fixtures/recordings/hermes`
    shows: a direct chat has one person in it."""
    named = str(named or "")
    if not named:
        return False
    if named.endswith(":" + key):
        return True
    kind, _, rest = key.partition(":")
    chat = rest.rpartition(":")[0]
    return kind == "dm" and bool(chat) and named.endswith(f":dm:{chat}")


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
        # Hermes keeps one confirmation in a session, and a new one replaces it (`tools.slash_confirm`): its
        # session key -> the one the chat shows waiting there, and each one -> its session key and command.
        self.confirm_waiting: dict[str, str] = {}
        self.confirm_key: dict[str, str] = {}
        self.confirm_commands: dict[str, str] = {}
        # A message typed in a session with a confirmation waiting -> that confirmation, and the answer
        # Hermes reads in it, if any (`confirm_choice`)
        self.typed_confirm: dict[str, tuple[str, str | None]] = {}
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
        what belongs to another turn still running, and a confirmation, which waits in Hermes's session,
        not in a turn, until it is answered, replaced or dropped (D-155)."""
        waiting = set(self.confirm_waiting.values())
        for item, ev in list(self.open.items()):
            if ev["type"] in ("turn", "task", "session") or ev.get("turn") in keep_turns or item in waiting:
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
        key = None
        if m.get("chat_type"):
            # The plugin says what kind of chat a message is typed in, so a reply to it reaches the
            # sender's own session, whichever turn is the latest.
            key = key_of({"chat_type": m["chat_type"], "chat_id": m.get("chat"), "user_id": m.get("user")})
            self.typed_key[m.get("message_id")] = key
        # Hermes reads a typed answer as the answer to the confirmation in the sender's own session.
        asked = self.confirm_of(key, m.get("chat"), m.get("user"))
        choice = confirm_choice(text) if asked else None
        if asked:
            self.typed_confirm[m.get("message_id")] = (asked, choice)
        confirming = choice is not None and word.startswith("/")
        if not confirming and word.startswith("/") and word not in ("/approve", "/deny"):
            self.typed[m.get("message_id")] = self.typed_control(word, args)
        if confirming or (self.approval and word in ("/approve", "/deny")) or word.startswith("/"):
            # A slash command is never the answer to a question: Hermes leaves the question waiting.
            return
        item = self.question_for(m.get("chat"), m.get("user"))
        if item is not None:
            self.answered(item, text, m.get("user"))

    def confirm_of(self, key, chat, user):
        """The confirmation the chat shows waiting in the session of the sender of a message: by its chat and
        sender (`key_of`) when the plugin says them, or else, as in a recording, by the chat and sender."""
        for session_key, item in reversed(list(self.confirm_waiting.items())):
            if item not in self.open:
                continue
            if key is not None and names_session(session_key, key):
                return item
            if key is None and (str(session_key).endswith(f":{chat}:{user}") or str(session_key).endswith(f":dm:{chat}")):
                return item
        return None

    def confirm_done(self, item, choice, native=True, message=None):
        """Hermes holds the confirmation `item` no longer: answered with `choice`, or, with none, replaced or
        dropped, so it shows as cancelled. Either way it waits no more. Answered by a `message` typed, it
        shows in that message's turn, as Hermes's reply to it does."""
        prev = self.open.get(item)
        key = self.confirm_key.get(item)
        if self.confirm_waiting.get(key) == item:
            del self.confirm_waiting[key]
        for typed in [t for t, (asked, _) in self.typed_confirm.items() if asked == item]:
            del self.typed_confirm[typed]
        if prev is None:
            return
        owner = (prev.get("session"), prev.get("turn")) if message is None else self.owner_of(message)
        if choice in CONFIRM_CHOICES:
            q = prev["body"]["questions"][0]
            chosen = q["options"][CONFIRM_CHOICES.index(choice)]
            self.emit(item, "question", "done", prev["title"], f"Answered: {chosen['label']}",
                      {**prev["body"], "answer": [{"question": q["id"], "options": [chosen["id"]]}]},
                      native=native, owner=owner)
        else:
            self.emit(item, "question", "cancelled", prev["title"], prev["text"], prev["body"], native=native,
                      owner=owner)

    def looked(self, m):
        """The plugin asked Hermes which confirmation waits in a session (`CONFIRM_LOOK`), after `message`
        reached it, when one shows waiting there. When it is not the one shown, Hermes holds that one no
        longer: the message answered it, when Hermes reads an answer in it, or else Hermes replaced or
        dropped it. When it is, the message answered nothing."""
        p = m.get("params") or {}
        item = self.confirm_waiting.get(p.get("session_key"))
        if item is None or item not in self.open:
            return
        result = m.get("result")
        waiting = result.get("confirm_id") if isinstance(result, dict) else None
        asked, choice = self.typed_confirm.get(str(p.get("message")), (None, None))
        if waiting is not None and f"confirm:{waiting}" == item:
            self.typed_confirm.pop(str(p.get("message")), None)
            return
        answered = asked == item and choice is not None
        self.confirm_done(item, choice if answered else None, message=str(p.get("message")) if answered else None)
        if answered:
            # What Hermes says next to that message is the confirmed command's.
            self.turn_command = self.confirm_commands.get(item)

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
        card, by its request id, the answer to a question, by its id, or to a request to confirm a
        command, by its id, with what Hermes said to it."""
        if "pick_model" in m:
            return
        if m.get("confirm_id") is not None:
            return self.confirmed(m)
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
        result = m.get("result")
        if method == "get_session_model_settings" and "id" in m and ("error" in m or not usable_settings(result)):
            # Hermes's answer ends the read it names, even when it is of no use, as one with no model is: it
            # shows nothing and settles nothing. A read the plugin did not name, as in a recording, is kept
            # by its chat and sender, and the next read of theirs replaces it.
            self.reads.pop(str(m["id"]), None)
        if "error" in m:
            return
        if method == CONFIRM_LOOK:
            return self.looked(m)
        if method == "hermes_cli.commands.COMMAND_REGISTRY":
            self.commands = [command(c.get("name"), c.get("description"), c.get("args_hint"))
                             for c in result or [] if c.get("gateway_available")]
            # The sessions this chat has shown: the count of sessions is shared with other chats.
            for key in list(self.generation):
                if f"session:{self.name(key)}" in self.sessions:
                    self.session_changed(key)
        elif method == "get_session_model_settings" and usable_settings(result):
            # The session Hermes names (`names_session`), as it was
            # when asked: a read that a new session, or a switch of model, overtook says nothing of now.
            named = str(result.get("session_key"))
            read = str(m["id"]) if "id" in m else next(
                (r for r in reversed(list(self.reads)) if names_session(named, self.reads[r][0])), None)
            if read not in self.reads:
                return
            key, generation, switches = self.reads.pop(read)
            if (not names_session(named, key) or generation != self.generation.get(key, 1)
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
        """The running turn of the Hermes session `session_key` (`names_session`), if one is running."""
        for turn, key in reversed(list(self.turn_key.items())):
            if names_session(session_key, key) and f"turn:{turn}" in self.open:
                return turn
        return None

    def key_named(self, session_key):
        """The chat and sender (`key_of`) of the Hermes session `session_key`, of those this chat has had a
        turn or a session of, or None. In a group each sender has a session: the latest is not theirs."""
        known = [*reversed(list(self.turn_key.values())), *self.generation]
        return next((key for key in known if names_session(session_key, key)), None)

    # -- text

    def confirmed(self, m):
        """Hermes took the answer to its request to confirm a command (D-146): the question is answered.
        Once the command finishes, what Hermes says to the answer (`said`), which it hands the plugin and
        not the chat, is the command's output, in the session of the command, outside any turn: the
        answer started none."""
        item = f"confirm:{m['confirm_id']}"
        if "said" not in m:
            if item in self.open and m.get("choice") in CONFIRM_CHOICES:
                self.confirm_done(item, m["choice"])
            return
        answered = self.last.get(item)
        if answered is None or answered["state"] != "done":
            return
        key = self.key_named(m.get("session_key")) or str(answered.get("session") or "").split("#")[0]
        if key:
            # Hermes gave no reply: nothing else shows the outcome, so a notice says so. Hermes says the same
            # when it ran nothing, having found the confirmation too old after all, so it never says it ran.
            said = str(m.get("said") or "") or f"Hermes gave no reply to {self.confirm_commands.get(item) or 'the command'}."
            self.command_reply(f"sent:{self.ids}{self.seq + 1}", said, None, key, alone=True)

    def command_reply(self, item, content, typed, key, alone=False):
        """Hermes's reply to a command typed in the session of `key`: in the latest turn, or `alone`, in
        that session outside any turn."""
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
        owner = (self.name(key), None) if alone else None
        self.last_send = self.emit(item, "notice", "done", title, content, {"kind": "command_output"}, owner=owner)

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
        typed = self.typed.get(p.get("reply_to"))
        key = self.typed_key.get(p.get("reply_to")) or self.key
        for asking in self.confirm_waiting.values():
            prev = self.open.get(asking)
            if (prev is not None and content == prev["text"]
                    and (key is None or names_session(self.confirm_key.get(asking), key))):
                # Hermes's request to confirm, as text, after it was handed over as one.
                self.last_send = prev
                return
        # The reply to a typed answer to a confirmation, when the plugin did not look at Hermes first, as in
        # a recording: the confirmation is answered, and what follows is the confirmed command's.
        answer = p.get("reply_to") or self.turn or ""
        asked, choice = self.typed_confirm.get(answer, (None, None))
        if asked in self.open and choice is not None:
            self.confirm_done(asked, choice, message=answer)
            self.turn_command = self.confirm_commands.get(asked)
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
        """Hermes asks to confirm a command in a session: a confirmation it already held there is replaced,
        and will never run (D-155)."""
        options = [{"id": f"o{n}", "label": label} for n, label in enumerate(CONFIRM_LABELS)]
        item = f"confirm:{p.get('confirm_id')}"
        session_key = str(p.get("session_key") or "")
        replaced = self.confirm_waiting.get(session_key)
        if replaced is not None and replaced != item:
            self.confirm_done(replaced, None, native=None)
        self.confirm_waiting[session_key] = item
        self.confirm_key[item] = session_key
        self.confirm_commands[item] = p.get("title")
        qs = [{"id": "q0", "text": p.get("message"), "multiple": False, "allow_text": False, "options": options}]
        # The session Hermes asks in, which in a group need not be the latest turn's.
        key = self.key_named(p.get("session_key"))
        owner = None if key is None else (self.name(key), self.turn_of_session(p.get("session_key")))
        self.emit(item, "question", "waiting", "Confirm this command?", p.get("message"),
                  {"answer_via": "message", "questions": qs}, owner=owner)
