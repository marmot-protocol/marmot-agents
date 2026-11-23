"""The Hermes plugin's transport: it serves the core's `prompt`, `answer` and `control` requests by
handing them to Hermes, and turns what Hermes hands the platform into agent events for the core.

Every call Hermes makes on the platform, and everything the plugin hands Hermes, is recorded as a
line, exactly as `tools/record/hermes` records it, and fed to the chat's translator. The events it
makes go to the core with the chat they belong to, the prompt a turn responds to, and the requests
whose effect they show (D-028, D-058).

For Hermes a control is a typed command (`docs/reference/hermes-messages.md`): an interrupt is
`/stop`, a new session `/new` and a new model `/model <id>`, each typed in the session's own chat by
its own sender. An approval is answered through Hermes's approval card, and a question through the
call Hermes gives a platform for it; each is settled once Hermes has taken the answer.

Nothing here touches Hermes directly: `hermes` is the adapter, or a stand-in for it in the tests.
"""

from __future__ import annotations

import asyncio
import json
import os
import secrets
import tempfile
from pathlib import Path

from .core import Refusal
from .translate import CONFIRM, OPEN, VERSION, Translator

# Hermes can report a turn complete and start it again a moment later, for the first message of a
# session (`docs/reference/hermes-messages.md`). The end of a turn that showed nothing is held this
# long, and dropped if the turn starts again meanwhile.
HOLD_SECONDS = 10.0
# What Hermes says when it took a `/model <id>`: the session's model, read then, shows the switch.
MODEL_SWITCHED = "Model switched to"
# What Hermes says, on the first message after it restarted, about a session whose turn a crash cut off.
# The session stays paused until someone sends `/discard`, which drops the lost turn without running it
# again, or `/reset` (read in Hermes's source, `gateway/session_ingress.py`).
LOST_TURN = "⏸️ This conversation is paused: a previous turn did not finish when Hermes restarted"


def text_of(value) -> str:
    return value if isinstance(value, str) else ""


def obj(value) -> dict:
    return value if isinstance(value, dict) else {}


def unsettle(waiting: dict[str, list[str]], key: str, request: str) -> None:
    """Takes a request back off what waits for `key`'s effect."""
    left = [r for r in waiting.get(key, []) if r != request]
    if left:
        waiting[key] = left
    else:
        waiting.pop(key, None)


class Chat:
    """One chat: its translator, and what waits for an effect."""

    def __init__(self, id_: str, translator: Translator):
        self.id = id_
        self.translator = translator
        self.by_item: dict[str, list[str]] = {}  # requests settled by an item's next event
        self.by_turn: dict[str, list[str]] = {}  # interrupts, settled by the end of the turn
        self.new_session: list[tuple[str, int, str]] = []  # (chat and sender, their session count then, the request)
        self.sets: list[dict] = []  # `set` controls waiting for a session event showing the value
        self.answered: set[str] = set()
        self.held = None  # (the event, its timer) for a turn's end that showed nothing
        self.quiet: dict[str, bool] = {}  # turn -> nothing but the turn itself was shown since it started
        self.closed: set[str] = set()  # items a core closed when its connection went, never sent again

    def core_gone(self) -> None:
        """The connection to the core went, and that core closed every item open in the chat and settled
        every request it handed over (D-058, D-070). Hermes goes on. What it says next of those items,
        and a turn's end held back, would change rows the chat shows closed; what waits for an effect
        names requests already settled. The next connection starts from what the next core knows: those
        items are not sent again, and nothing waits. A session item is the session's, which goes on, and
        is shown again with its next change."""
        if self.held is not None:
            held, timer = self.held
            timer.cancel()
            self.held = None
            self.closed.add(held["item"])
        self.closed.update(item for item, ev in self.translator.open.items() if ev["type"] != "session")
        self.translator.sessions.clear()
        self.by_item.clear()
        self.by_turn.clear()
        self.new_session.clear()
        self.sets.clear()
        self.answered.clear()
        self.quiet.clear()


class Connector:
    def __init__(self, hermes, send, version: str = VERSION, generations: Path | None = None,
                 hold: float = HOLD_SECONDS, warn=None):
        self.hermes = hermes
        self.send_event = send
        self.version = version
        self.hold = hold
        self.warn = warn or (lambda kind: None)
        self.boot = secrets.token_hex(4)
        self.chats: dict[str, Chat] = {}
        self.message_chat: dict[str, str] = {}  # message id -> its chat
        self.sources: dict[str, tuple[dict, dict]] = {}  # message id -> (chat, sender) it came from
        self.prompted: set[str] = set()  # the core's prompts not yet tied to a turn
        self.session_sender: dict[str, dict] = {}  # session -> the sender whose session it is
        self.cards: dict[str, tuple[str, str]] = {}  # approval item -> (Hermes's session key, request id)
        # A command typed for a request -> (its chat, the request, whether Hermes's reply settles it)
        self.awaiting: dict[str, tuple[str, str, object]] = {}
        self.tasks: set[asyncio.Task] = set()  # what the plugin asks Hermes in the background
        self.typed_n = 0
        self.commands: list | None = None  # Hermes's command registry, once the adapter has read it
        self.models_read: set[str] = set()  # sessions whose model has been asked for since starting
        self.generations_path = generations
        self.generations = self.load_generations()

    # -- the generations of each chat's sessions, kept across restarts

    def load_generations(self) -> dict:
        try:
            data = json.loads(self.generations_path.read_text(encoding="utf-8")) if self.generations_path else {}
        except (OSError, ValueError):
            data = {}
        return {k: v for k, v in data.items() if isinstance(k, str) and isinstance(v, int)} if isinstance(data, dict) else {}

    def save_generations(self, before: dict) -> None:
        if self.generations_path is None or before == self.generations:
            return
        try:
            self.generations_path.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile("w", dir=self.generations_path.parent, delete=False, encoding="utf-8") as f:
                json.dump(self.generations, f)
            os.replace(f.name, self.generations_path)
        except OSError:
            self.warn("the sessions file could not be written")

    # -- lines from Hermes, and from the plugin itself

    def chat(self, id_: str) -> Chat:
        chat = self.chats.get(id_)
        if chat is None:
            ids = f"{self.boot}-{len(self.chats) + 1}-"
            chat = Chat(id_, Translator(self.version, ids, self.generations))
            self.chats[id_] = chat
            if self.commands is not None:
                self.record(self.registry_line(), id_)
        return chat

    def registry_line(self) -> dict:
        return {"dir": "in", "via": "call", "msg": {"method": "hermes_cli.commands.COMMAND_REGISTRY", "result": self.commands}}

    def commands_read(self, commands: list) -> None:
        """Hermes's command registry, read once by the adapter: every chat's session items list it."""
        self.commands = commands
        for id_ in list(self.chats):
            self.record(self.registry_line(), id_)

    def wants_model(self, chat_type, chat_id, user_id) -> str | None:
        """The session to ask Hermes the model of, when a turn of this chat and sender starts: each
        session once, so a new session shows its model too. None when it was asked already."""
        key = f"{chat_type}:{chat_id}:{user_id}"
        session = f"{key}#{self.generations.get(key, 1)}"
        if session in self.models_read:
            return None
        self.models_read.add(session)
        return session

    def model_not_read(self, session: str) -> None:
        """Asking Hermes for the model of `session` failed: the next turn asks again."""
        self.models_read.discard(session)

    def record(self, line: dict, chat_id: str | None) -> None:
        """Feeds one line to its chat's translator and sends what it makes. A line that belongs to no
        chat the plugin knows is passed over."""
        if not chat_id:
            return
        chat = self.chat(chat_id)
        m = line.get("msg") or {}
        settled_by_reply = None
        if line.get("via") == "hook":
            p = m.get("params") or {}
            if m.get("method") == "_send_exec_approval_prompt":
                prompt = obj(p.get("prompt"))
                self.cards[f"approval:{prompt.get('request_id')}"] = (prompt.get("session_key"), prompt.get("request_id"))
            if m.get("method") == "send":
                awaiting = self.awaiting.pop(text_of(p.get("reply_to")), None)
                if awaiting is not None and awaiting[0] == chat.id and awaiting[2](text_of(p.get("content"))):
                    settled_by_reply = awaiting[1] or None
                if text_of(p.get("content")).startswith(LOST_TURN):
                    self.discard(chat, text_of(p.get("reply_to")))
        before = dict(self.generations)
        events = chat.translator.feed(line)
        self.save_generations(before)
        if settled_by_reply is not None:
            # Hermes's reply is the command's outcome: the notice that shows it settles the request.
            reply = next((e for e in events if e["type"] == "notice"), None)
            if reply is not None:
                chat.by_item.setdefault(reply["item"], []).append(settled_by_reply)
        self.dispatch(chat, events)

    def core_gone(self) -> None:
        """The connection to the core went: see `Chat.core_gone`. A command typed for a request of that
        core settles nothing now."""
        for chat in self.chats.values():
            chat.core_gone()
        self.awaiting.clear()

    def discard(self, chat: Chat, reply_to: str) -> None:
        """Hermes paused a session because a crash cut a turn off. The core has already closed that
        turn as interrupted, and a prompt is never run twice (D-037, D-070), so the plugin drops the
        lost turn, as the person would with `/discard`. The message Hermes saved then runs."""
        source = self.sources.get(reply_to) or self.sources.get(chat.translator.turn or "")
        if source is None:
            return self.warn("a paused session could not be resumed")
        asyncio.ensure_future(self.type(source[0], source[1], "/discard"))

    def dispatch(self, chat: Chat, events: list[dict]) -> None:
        for event in events:
            if event["item"] in chat.closed:
                continue
            settles: list[str] = []
            item, state, turn = event["item"], event["state"], event.get("turn")
            settles += chat.by_item.pop(item, [])
            open_ = state in OPEN
            if event["type"] == "turn" and not open_:
                settles += chat.by_turn.pop(turn, [])
            if event["type"] == "session":
                session = event.get("session")
                values = {s.get("id"): s.get("value") for s in event["body"].get("settings") or []}
                shown = [s for s in chat.sets if s["session"] == session and values.get(s["setting"]) == s["value"]]
                settles += [s["request"] for s in shown]
                chat.sets = [s for s in chat.sets if s not in shown]
                # A reset is settled by the first session event after Hermes replaced that sender's
                # session: the old session's end, or, when it was not shown since starting, the new one.
                replaced = [r for r in chat.new_session if chat.translator.generation.get(r[0], 1) > r[1]]
                settles += [request for _, _, request in replaced]
                chat.new_session = [r for r in chat.new_session if r not in replaced]
            out = {k: v for k, v in event.items() if k not in ("harness", "seq")}
            out["chat"] = chat.id
            if event["type"] == "turn" and turn in self.prompted:
                self.prompted.discard(turn)
                out["prompts"] = [turn]
            if settles:
                out["settles"] = settles
            if turn is not None and event["type"] == "turn":
                source = self.sources.get(turn)
                if source is not None and event.get("session"):
                    self.session_sender[event["session"]] = source[1]
            self.out(chat, out)

    def out(self, chat: Chat, event: dict) -> None:
        """Sends an event, holding back the end of a turn that showed nothing (`HOLD_SECONDS`)."""
        is_turn = event["type"] == "turn"
        if chat.held is not None:
            held, timer = chat.held
            if is_turn and event["item"] == held["item"] and event["state"] == "running":
                # Hermes started the turn again: the end that was held never happened.
                timer.cancel()
                chat.held = None
                chat.quiet[event["turn"]] = True
                return
            if is_turn or event.get("turn") == held.get("turn"):
                # Another turn, or more of this one: the held end stands, and goes first. Anything else,
                # such as the session item, goes past it.
                timer.cancel()
                chat.held = None
                self.send_held(chat, held)
        turn = event.get("turn")
        if is_turn and event["state"] == "running":
            chat.quiet[turn] = True
        elif turn is not None and event["type"] != "session":
            if not is_turn:
                chat.quiet[turn] = False
        if is_turn and event["state"] == "done" and chat.quiet.get(turn) and "settles" not in event and self.hold > 0:
            loop = asyncio.get_running_loop()
            chat.held = (event, loop.call_later(self.hold, self.release, chat))
            return
        self.send_event(event)

    def release(self, chat: Chat) -> None:
        if chat.held is not None:
            held, _ = chat.held
            chat.held = None
            self.send_held(chat, held)

    def send_held(self, chat: Chat, held: dict) -> None:
        """Sends a held end. Hermes reported it before any interrupt asked for while it was held, so it
        settles none: the turn had already finished, and the core settles such an interrupt as having
        no effect (D-087). An interrupt settles only if the turn starts again and ends."""
        chat.by_turn.pop(held.get("turn"), None)
        self.send_event(held)

    # -- requests from the core

    async def serve(self, method: str, params: dict) -> None:
        if method == "prompt":
            return await self.prompt(params)
        if method == "answer":
            return await self.answer(params)
        if method == "control":
            return await self.control(params)
        raise Refusal("not_supported")

    async def type(self, chat: dict, sender: dict, text: str, message_id: str | None = None,
                   reply_to: dict | None = None) -> str:
        """Hands Hermes a message, as typed by `sender` in `chat`, and records it first."""
        if message_id is None:
            self.typed_n += 1
            message_id = f"hermes-{self.boot}-{self.typed_n}"
        chat_id = text_of(chat.get("id"))
        self.message_chat[message_id] = chat_id
        self.sources[message_id] = (chat, sender)
        line = {"message_id": message_id, "chat": chat_id, "chat_type": "group" if chat.get("group") else "dm",
                "user": sender.get("id"), "user_name": sender.get("name"), "text": text}
        if reply_to:
            line["reply_to"] = reply_to.get("message")
        self.record({"dir": "out", "via": "inbound", "msg": line}, chat_id)
        await self.hermes.inject(chat, sender, message_id, text, reply_to)
        return message_id

    async def prompt(self, params: dict) -> None:
        message = text_of(params.get("message"))
        chat = obj(params.get("chat"))
        sender = obj(params.get("sender"))
        if not message or not text_of(chat.get("id")) or not text_of(sender.get("id")):
            raise Refusal("invalid")
        # Hermes keeps its own sessions (`sessions: own`): a session the core names is not used.
        self.prompted.add(message)
        reply = params.get("reply_to")
        await self.type(chat, sender, text_of(params.get("text")), message, obj(reply) if reply else None)

    def find(self, item: str) -> Chat | None:
        return next((c for c in self.chats.values() if item in c.translator.last), None)

    def source_of(self, chat: Chat, session: str | None, by: dict) -> tuple[dict, dict]:
        """The chat and the sender a command must come from to reach `session`. A session is named
        after its chat and sender (`<chat type>:<chat>:<sender>`, then `#<n>`), so the sender is
        known after a restart too. A session that names neither this chat nor a sender is refused."""
        known = next((s[0] for s in self.sources.values() if text_of(s[0].get("id")) == chat.id), None)
        if not session:
            return known or {"id": chat.id}, by
        chat_type, _, rest = session.split("#")[0].partition(":")
        chat_id, _, user_id = rest.rpartition(":")
        if chat_id != chat.id or not user_id:
            raise Refusal("nothing_to_do")
        sender = self.session_sender.get(session) or {"id": user_id}
        return known or {"id": chat.id, "group": chat_type == "group"}, sender

    def key_of(self, chat: dict, sender: dict) -> str:
        """The chat and sender a session is named after, as the translator names them."""
        return f"{'group' if chat.get('group') else 'dm'}:{chat.get('id')}:{sender.get('id')}"

    async def command(self, chat: Chat, where: dict, sender: dict, text: str, request: str, replied) -> None:
        """Types a command for `request`, which is taken once Hermes has it: the core waits only so long
        for a request to be taken, and Hermes may answer late or not at all. When Hermes replies,
        `replied(its words)` says whether the reply is the outcome, and the reply then settles the request.
        Otherwise an effect does, or, failing one, the core (D-087)."""
        self.typed_n += 1
        message_id = f"hermes-{self.boot}-{self.typed_n}"
        self.awaiting[message_id] = (chat.id, request, replied)
        try:
            await self.type(where, sender, text, message_id)
        except BaseException:
            self.awaiting.pop(message_id, None)
            raise

    def background(self, work) -> None:
        task = asyncio.ensure_future(work)
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)

    async def answer(self, params: dict) -> None:
        request = text_of(params.get("request"))
        item = text_of(params.get("item"))
        by = obj(params.get("by"))
        given = obj(params.get("given"))
        chat = self.find(item)
        if chat is None:
            raise Refusal("unknown_item")
        if item in chat.closed:
            raise Refusal("not_waiting")
        if item in chat.answered:
            raise Refusal("already_answered")
        prev = chat.translator.open.get(item)
        if prev is None or prev["state"] != "waiting":
            raise Refusal("not_waiting")
        where, sender = self.source_of(chat, prev.get("session"), by)
        if prev["type"] == "approval":
            natives = chat.translator.natives.get(item) or []
            choice = given.get("choice")
            ids = [c["id"] for c in prev["body"].get("choices") or []]
            if choice not in ids or ids.index(choice) >= len(natives):
                raise Refusal("invalid")
            native = natives[ids.index(choice)]
            if item in self.cards:
                session_key, request_id = self.cards[item]
                resolved = self.hermes.resolve_card(session_key, native, request_id, text_of(by.get("id")))
                if not resolved:
                    raise Refusal("not_waiting")
                chat.answered.add(item)
                if request:
                    chat.by_item.setdefault(item, []).append(request)
                press = {"choice": native, "user": by.get("id"), "request_id": request_id, "resolved": resolved}
                self.record({"dir": "out", "via": "press", "msg": press}, chat.id)
                return
            await self.typed_answer(chat, item, request, where, sender, native)
            return
        if prev["type"] == "question":
            questions = prev["body"].get("questions") or []
            replies = given.get("questions")
            if not isinstance(replies, list) or len(replies) != 1 or not questions:
                raise Refusal("invalid")
            reply, asked = obj(replies[0]), questions[0]
            if reply.get("question") != asked["id"]:
                raise Refusal("invalid")
            options = {o["id"]: i for i, o in enumerate(asked.get("options") or [])}
            chosen = [o for o in reply.get("options") or [] if o in options]
            if len(chosen) != len(reply.get("options") or []):
                raise Refusal("invalid")
            if item.startswith("confirm:"):
                if len(chosen) != 1:
                    raise Refusal("invalid")
                await self.typed_answer(chat, item, request, where, sender, CONFIRM[options[chosen[0]]])
                return
            parts = [asked["options"][options[o]]["label"] for o in chosen]
            if text_of(reply.get("text")):
                parts.append(reply["text"])
            if len(parts) != 1:
                raise Refusal("invalid", "Hermes takes one answer to a question.")
            # Hermes's own call for a platform's answer: it says whether the question was still
            # waiting, and the question is closed only once Hermes has taken the answer.
            clarify_id = item.removeprefix("question:")
            if not self.hermes.resolve_question(clarify_id, parts[0]):
                raise Refusal("not_waiting")
            chat.answered.add(item)
            if request:
                chat.by_item.setdefault(item, []).append(request)
            press = {"clarify_id": clarify_id, "text": parts[0], "user": by.get("id"), "resolved": True}
            self.record({"dir": "out", "via": "press", "msg": press}, chat.id)
            return
        raise Refusal("unknown_item")

    async def typed_answer(self, chat: Chat, item: str, request: str, where: dict, sender: dict, text: str) -> None:
        """Types an answer. The item counts as answered from then on, so a second answer is refused, unless
        Hermes did not take it: then it waits for another."""
        chat.answered.add(item)
        if request:
            chat.by_item.setdefault(item, []).append(request)
        try:
            await self.type(where, sender, text)
        except BaseException:
            chat.answered.discard(item)
            unsettle(chat.by_item, item, request)
            raise

    async def control(self, params: dict) -> None:
        request = text_of(params.get("request"))
        by = obj(params.get("by"))
        action = obj(params.get("action"))
        kind = action.get("type")
        if kind == "interrupt":
            turn = text_of(action.get("turn"))
            # A turn whose end is held (`HOLD_SECONDS`) still shows as running, and may start again.
            chat = next((c for c in self.chats.values() if (f"turn:{turn}" in c.translator.open
                         and f"turn:{turn}" not in c.closed)
                         or (c.held is not None and c.held[0].get("turn") == turn)), None)
            if chat is None or turn not in self.sources:
                raise Refusal("nothing_to_do")
            where, sender = self.sources[turn]
            if request:
                chat.by_turn.setdefault(turn, []).append(request)
            try:
                await self.type(where, sender, "/stop")
            except BaseException:
                unsettle(chat.by_turn, turn, request)
                raise
            return
        if kind == "new_session":
            where = obj(action.get("chat"))
            if not text_of(where.get("id")):
                raise Refusal("invalid")
            chat = self.chat(where["id"])
            session = action.get("session") if isinstance(action.get("session"), str) else None
            _, sender = self.source_of(chat, session, by)
            key = self.key_of(where, sender)
            pending = (key, chat.translator.generation.get(key, 1), request)
            if request:
                chat.new_session.append(pending)

            def replied(said: str) -> bool:
                # "✨" is Hermes's reset, which the end of the session it replaced shows. Anything else
                # is a refusal, or a question of its own: no session was replaced.
                if said.startswith("✨") or pending not in chat.new_session:
                    return False
                chat.new_session.remove(pending)
                return True
            try:
                await self.command(chat, where, sender, "/new", request, replied)
            except BaseException:
                if pending in chat.new_session:
                    chat.new_session.remove(pending)
                raise
            return
        if kind == "set":
            return await self.set(request, by, action)
        raise Refusal("not_supported")

    async def set(self, request: str, by: dict, action: dict) -> None:
        """Only the model can be set from the chat for now: by `/model <id>`, which Hermes answers in
        text. The plugin then asks Hermes for the session's model, which the session item shows."""
        session, setting, value = text_of(action.get("session")), action.get("setting"), action.get("value")
        if setting != "model":
            raise Refusal("not_supported")
        if not isinstance(value, str) or not value.strip() or len(value.split()) != 1:
            raise Refusal("invalid")
        chat = next((c for c in self.chats.values() if f"session:{session}" in c.translator.last), None)
        if chat is None:
            raise Refusal("nothing_to_do")
        where, sender = self.source_of(chat, session, by)
        pending = {"request": request, "session": session, "setting": "model", "value": value}
        if request:
            chat.sets.append(pending)

        def replied(said: str) -> bool:
            if said.startswith(MODEL_SWITCHED):
                # Hermes took it: the session's model, read now, shows it, and settles the request.
                self.background(self.hermes.read_model_settings(where, sender))
                return False
            if pending not in chat.sets:
                return False
            chat.sets.remove(pending)
            return True
        try:
            await self.command(chat, where, sender, f"/model {value}", request, replied)
        except BaseException:
            if pending in chat.sets:
                chat.sets.remove(pending)
            raise
