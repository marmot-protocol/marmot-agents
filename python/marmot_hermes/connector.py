"""The Hermes plugin's transport: it serves the core's `prompt`, `answer` and `control` requests by
handing them to Hermes, and turns what Hermes hands the platform into agent events for the core.

Every call Hermes makes on the platform, and everything the plugin hands Hermes, is recorded as a
line, exactly as `tools/record/hermes` records it, and fed to the chat's translator. The events it
makes go to the core with the chat they belong to, the prompt a turn responds to, and the requests
whose effect they show (D-028, D-058).

For Hermes a control is a typed command (`docs/reference/hermes-messages.md`): an interrupt is
`/stop`, a new session `/new` and a new model `/model <id>`, each typed in the session's own chat by
its own sender. An approval is answered through Hermes's approval card, and a question through the
call Hermes gives a platform for it; each is settled once Hermes has taken the answer. A request to
confirm a command is answered by its id (D-146), and is Hermes's: it closes only on evidence, Hermes's reply
to an answer, a newer one in its session, or Hermes's own limit passing, never because Hermes no longer
holds it, which says nothing of why. The command it confirms is settled by the answer that cancels it or
the event that shows it replaced or expired, or else by its own effect (D-155).

Nothing here touches Hermes directly: `hermes` is the adapter, or a stand-in for it in the tests.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import secrets
import tempfile
from pathlib import Path

from .core import Refusal
from .translate import CONFIRM_CHOICES, CONFIRM_LOOK, OPEN, VERSION, Translator, names_session

log = logging.getLogger("marmot_hermes")

# Hermes can report a turn complete and start it again a moment later, for the first message of a
# session (`docs/reference/hermes-messages.md`). The end of a turn that showed nothing is held this
# long, and dropped if the turn starts again meanwhile.
HOLD_SECONDS = 10.0
# Hermes answers a command only in text, which it makes from its catalog (`agent.i18n.t`) in the language
# it is set to: no field says what became of the command. The plugin knows the reply by its key, from the
# same catalog. Hermes took a `/model <id>` or a `/model reset` when it replies with one of these
# (`gateway/slash_commands_model.py`); anything else is a refusal.
MODEL_TAKEN = ("gateway.model.switched", "gateway.model.reset_done", "gateway.model.reset_none")
# And it replaced the session when it replies to a `/new` with one of these (`gateway/slash_commands_session.py`).
RESET_TAKEN = ("gateway.reset.header_default", "gateway.reset.header_new", "gateway.reset.header_titled")
# Stands for a value in a reply, as each field of the catalog's text.
MARK = "\x00"
# How long a control waits for its effect, from when the plugin handed it to Hermes, before the core
# settles it as having none (D-087). A control still waiting after this settles nothing.
EFFECT_SECONDS = 120.0
# Hermes's own limit for a confirmation (`tools.slash_confirm.DEFAULT_TIMEOUT_SECONDS`, 300 at the pinned
# commit): it never runs one older, and drops one silently. The plugin counts from when Hermes offers it,
# which is after Hermes started counting, and a second more, so that by Hermes's clock it has passed too.
EXPIRY_SECONDS = 301.0
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
        # Resets waiting for the session they replace to end, one for each chat and sender: {"key", its
        # session count then, "request", "at": when Hermes was handed it, None until then}
        self.new_session: list[dict] = []
        # Model switches waiting for the read after Hermes took them, one a session: {"request", "session",
        # "taken": Hermes said it took it, "at": as for a reset}
        self.sets: list[dict] = []
        # Request to confirm a command -> the command it confirms, as `Connector.awaiting` holds it. Hermes
        # keeps one in each session (`Translator.confirm_waiting`), so in a group each sender can have one.
        self.confirming: dict[str, tuple] = {}
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
        self.confirming.clear()
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
        # A command typed for a request -> (its chat, the request, whether Hermes's reply settles it, the
        # chat and sender it was typed as). `replied(None)` says the command will not run.
        self.awaiting: dict[str, tuple[str, str, object, str]] = {}
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
        settled_by_reply = []
        if line.get("via") == "hook":
            p = m.get("params") or {}
            if m.get("method") == "_send_exec_approval_prompt":
                prompt = obj(p.get("prompt"))
                self.cards[f"approval:{prompt.get('request_id')}"] = (prompt.get("session_key"), prompt.get("request_id"))
            if m.get("method") == "send_slash_confirm":
                item = f"confirm:{p.get('confirm_id')}"
                self.asked_to_confirm(chat, text_of(p.get("session_key")), item)
                asyncio.get_running_loop().call_later(EXPIRY_SECONDS, self.expired, chat, item)
            if m.get("method") == "send":
                content = text_of(p.get("content"))
                settled_by_reply += self.typed_answer_replied(chat, text_of(p.get("reply_to")), content)
                awaiting = self.awaiting.pop(text_of(p.get("reply_to")), None)
                if awaiting is not None and awaiting[0] == chat.id:
                    asking = self.asking(chat, awaiting[3], content)
                    if asking is not None:
                        # Hermes asks to confirm the command, in reply to it: the answer decides its outcome.
                        chat.confirming[asking] = awaiting
                    elif awaiting[2](content):
                        settled_by_reply.append(awaiting[1])
                if content.startswith(LOST_TURN):
                    self.discard(chat, text_of(p.get("reply_to")))
        if line.get("via") == "press" and m.get("confirm_id") is not None and "said" in m:
            # Hermes's reply to the answer, which it handed the plugin, is the outcome of the command that
            # request confirmed, and of no other, when it says the command was refused. Otherwise the
            # command's effect settles it, as any other's, or, failing one, the core (D-087, D-155).
            awaiting = chat.confirming.pop(f"confirm:{m['confirm_id']}", None)
            said = text_of(m.get("said"))
            if awaiting is not None and said and awaiting[2](said):
                settled_by_reply.append(awaiting[1])
        before = dict(self.generations)
        events = chat.translator.feed(line)
        model_read = chat.translator.model_read
        self.save_generations(before)
        settled_by_reply = [r for r in settled_by_reply if r]
        if settled_by_reply:
            # Hermes's reply is the command's outcome: the notice that shows it settles the request.
            reply = next((e for e in events if e["type"] == "notice"), None)
            if reply is not None:
                chat.by_item.setdefault(reply["item"], []).extend(settled_by_reply)
        self.dispatch(chat, events, model_read)

    def core_gone(self) -> None:
        """The connection to the core went: see `Chat.core_gone`. A command typed for a request of that
        core settles nothing now."""
        for chat in self.chats.values():
            chat.core_gone()
        self.awaiting.clear()

    # -- Hermes's requests to confirm a command, kept by Hermes's session and id (D-155)

    def dropped(self, chat: Chat, item: str) -> None:
        """The confirmation `item` will not run its command: cancelled, replaced or expired. The event that
        closes it settles that command."""
        awaiting = chat.confirming.pop(item, None)
        if awaiting is not None and awaiting[2](None) and awaiting[1]:
            chat.by_item.setdefault(item, []).append(awaiting[1])

    def asked_to_confirm(self, chat: Chat, session_key: str, item: str) -> None:
        """Hermes asks to confirm a command in the session `session_key`: one that Hermes held there before is
        replaced, and will not run its command."""
        replaced = chat.translator.confirm_waiting.get(session_key)
        if replaced is not None and replaced != item:
            self.dropped(chat, replaced)

    def asking(self, chat: Chat, key: str, content: str) -> str | None:
        """The confirmation that Hermes's reply `content`, to a command typed in the session of `key`, asks:
        Hermes sends its question as its reply to the command (`_request_slash_confirm`), so the command now
        waits for the answer, and not for those words. None when the reply is not one."""
        translator = chat.translator
        return next((item for session_key, item in translator.confirm_waiting.items()
                     if names_session(session_key, key) and item not in chat.confirming
                     and (translator.open.get(item) or {}).get("text") == content), None)

    def expired(self, chat: Chat, item: str) -> None:
        """Hermes's own limit for the confirmation `item` passed (`EXPIRY_SECONDS`): Hermes will never run it,
        and it shows as cancelled, unless something closed it first."""
        if item in chat.translator.open:
            self.dropped(chat, item)
            self.record({"dir": "in", "via": "expired", "msg": {"confirm_id": item.removeprefix("confirm:")}}, chat.id)

    def typed_answer_replied(self, chat: Chat, message: str, reply: str) -> list[str]:
        """Hermes replies `reply` to `message`, a typed answer to the confirmation shown waiting in its sender's
        session (`Translator.typed_confirm`). Hermes is asked which one it holds there, and that is recorded:
        when it still holds that one, or cannot say, the answer did not answer it (Hermes refused the sender,
        or an approval took the word). Otherwise the reply shows it answered. A Cancel settles the command it
        was for, which will not run; one that runs is settled by its effect, or by `reply` when it says the
        command was refused, which is returned (D-155)."""
        translator = chat.translator
        item, choice = translator.typed_confirm.get(message, (None, None))
        if item not in translator.open:
            return []
        session_key = translator.confirm_key[item]
        look = {"method": CONFIRM_LOOK, "params": {"session_key": session_key, "message": message}}
        try:
            held = self.hermes.confirm_waiting(session_key)
            look["result"] = None if held is None else {"confirm_id": held}
        except Exception:  # the look runs inside Hermes's own call on the platform, which must not fail for it
            self.warn("Hermes's waiting confirmation could not be read")
            held, look["error"] = item.removeprefix("confirm:"), "unreadable"
        self.record({"dir": "in", "via": "call", "msg": look}, chat.id)
        if f"confirm:{held}" == item:
            return []
        if choice == "cancel":
            self.dropped(chat, item)
            return []
        awaiting = chat.confirming.pop(item, None)
        return [awaiting[1]] if awaiting is not None and awaiting[2](reply) else []

    def discard(self, chat: Chat, reply_to: str) -> None:
        """Hermes paused a session because a crash cut a turn off. The core has already closed that
        turn as interrupted, and a prompt is never run twice (D-037, D-070), so the plugin drops the
        lost turn, as the person would with `/discard`. The message Hermes saved then runs."""
        source = self.sources.get(reply_to) or self.sources.get(chat.translator.turn or "")
        if source is None:
            return self.warn("a paused session could not be resumed")
        asyncio.ensure_future(self.type(source[0], source[1], "/discard"))

    def dispatch(self, chat: Chat, events: list[dict], model_read: bool = False) -> None:
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
                # A switch is settled only by the session as a read begun after Hermes took it shows it: the
                # translator applies no read begun before (`Translator.model_switched`). A session that shows
                # the model asked for before then says nothing of the switch.
                shown = [s for s in chat.sets if s["session"] == session and s["taken"] and model_read]
                settles += [s["request"] for s in shown]
                chat.sets = [s for s in chat.sets if s not in shown]
                # A reset is settled by the first session event after Hermes replaced that sender's
                # session: the old session's end, or, when it was not shown since starting, the new one.
                replaced = [r for r in chat.new_session
                            if chat.translator.generation.get(r["key"], 1) > r["generation"]]
                settles += [r["request"] for r in replaced]
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
        self.awaiting[message_id] = (chat.id, request, replied, self.key_of(where, sender))
        try:
            await self.type(where, sender, text, message_id)
        except BaseException:
            self.awaiting.pop(message_id, None)
            raise

    def taken_by(self, keys) -> re.Pattern | None:
        """The replies of Hermes's that say it took a command: the catalog's texts of `keys`, each value in
        them any one line. None when Hermes knows none of them."""
        said = []
        for key in keys:
            try:
                text = self.hermes.phrase(key, model=MARK, title=MARK)
            except Exception:
                text = None
            if isinstance(text, str) and text and text != key:  # Hermes gives back a key it does not know
                said.append("[^\n]*".join(re.escape(part) for part in text.split(MARK)))
        return re.compile(f"(?:{'|'.join(said)})(?:\n|$)") if said else None

    def reserve(self, waiting: list[dict], pending: dict, by: str, what: str) -> None:
        """Holds `pending[by]` until `pending` settles, or until the core stops waiting for its effect,
        `EFFECT_SECONDS` after it was handed over. One effect can settle only one request."""
        now = asyncio.get_running_loop().time()
        waiting[:] = [w for w in waiting if w["at"] is None or now - w["at"] < EFFECT_SECONDS]
        if any(w[by] == pending[by] for w in waiting):
            raise Refusal("busy", f"A {what} is already on its way.")
        if pending["request"]:
            waiting.append(pending)

    async def handed(self, waiting: list[dict], pending: dict, work) -> None:
        """Hands Hermes a control held by `reserve`. The core's wait for its effect starts once it is
        handed over (D-087), and so does the plugin's; if Hermes does not take it, nothing is held."""
        try:
            await work
        except BaseException:
            if pending in waiting:
                waiting.remove(pending)
            raise
        pending["at"] = asyncio.get_running_loop().time()

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
                await self.confirm(chat, item, request, by, CONFIRM_CHOICES[options[chosen[0]]])
                return
            parts = [asked["options"][options[o]]["label"] for o in chosen]
            if text_of(reply.get("text")):
                parts.append(reply["text"])
            if len(parts) != 1:
                raise Refusal("invalid", "Hermes takes one answer to a question.")
            # Hermes's own call for a platform's answer: it says whether the question was still
            # waiting, and the question is closed only once Hermes has taken the answer.
            clarify_id = item.removeprefix("question:")
            if not self.hermes.resolve_question(clarify_id, parts[0], text_of(by.get("id"))):
                raise Refusal("not_waiting")
            chat.answered.add(item)
            if request:
                chat.by_item.setdefault(item, []).append(request)
            press = {"clarify_id": clarify_id, "text": parts[0], "user": by.get("id"), "resolved": True}
            self.record({"dir": "out", "via": "press", "msg": press}, chat.id)
            return
        raise Refusal("unknown_item")

    async def confirm(self, chat: Chat, item: str, request: str, by: dict, choice: str) -> None:
        """Answers Hermes's request to confirm a command through Hermes's own call, which answers only the
        confirmation it names, by its id (D-146). A typed `/approve` would answer whatever Hermes waits on
        when it reads it: an approval that started meanwhile, or another command's confirmation. One that
        no longer waits is refused, and nothing else is answered. The item counts as answered from the
        call on, so a second answer is refused, unless Hermes did not take it.

        The core's request is answered as soon as Hermes has taken the confirmation: the command it
        confirms, which can take a while (a `/new` closes the old session's tools first), runs on and
        shows its outcome when it finishes, and a core that goes away meanwhile does not cancel it
        (D-058)."""
        session_key = chat.translator.confirm_key.get(item)
        if not session_key:
            raise Refusal("not_waiting")
        confirm_id = item.removeprefix("confirm:")
        chat.answered.add(item)
        if request:
            chat.by_item.setdefault(item, []).append(request)
        try:
            running = self.hermes.resolve_confirm(session_key, confirm_id, choice, text_of(by.get("id")))
        except BaseException:
            chat.answered.discard(item)
            unsettle(chat.by_item, item, request)
            raise
        if running is None:
            # Hermes holds it no longer, which says nothing of why: the question shows what closes it (D-155).
            chat.answered.discard(item)
            unsettle(chat.by_item, item, request)
            raise Refusal("not_waiting")
        if choice == "cancel":
            # The command will not run: the answer that shows so settles it.
            self.dropped(chat, item)
        taken = {"confirm_id": confirm_id, "choice": choice, "user": by.get("id"), "session_key": session_key}
        self.record({"dir": "out", "via": "press", "msg": taken}, chat.id)
        if running.done():
            self.confirm_said(chat, confirm_id, session_key, running)
        else:
            self.background(self.confirm_finished(chat, confirm_id, session_key, running))

    async def confirm_finished(self, chat: Chat, confirm_id: str, session_key: str, running) -> None:
        await asyncio.wait({running})
        self.confirm_said(chat, confirm_id, session_key, running)

    def confirm_said(self, chat: Chat, confirm_id: str, session_key: str, running) -> None:
        """What Hermes said to the answer to its request to confirm a command, once the command finished,
        which shows its outcome. When Hermes's call failed, nothing does, and the core settles what waited
        on it (D-087)."""
        if running.cancelled() or running.exception() is not None:
            # D-071: the failure's words may hold a path or the command's, so only its kind is logged.
            kind = "cancelled" if running.cancelled() else type(running.exception()).__name__
            log.warning("Hermes's call to answer a confirmation failed: %s", kind)
            return
        said = {"confirm_id": confirm_id, "session_key": session_key, "said": running.result()}
        self.record({"dir": "out", "via": "press", "msg": said}, chat.id)

    async def typed_answer(self, chat: Chat, item: str, request: str, where: dict, sender: dict, text: str) -> None:
        """Types an answer. The item counts as answered from then on, so a second answer is refused, unless
        Hermes did not take it: then it waits for another, and the command it confirms waits for the reply
        to that one."""
        chat.answered.add(item)
        if request:
            chat.by_item.setdefault(item, []).append(request)
        self.typed_n += 1
        message_id = f"hermes-{self.boot}-{self.typed_n}"
        try:
            await self.type(where, sender, text, message_id)
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
            taken = self.taken_by(RESET_TAKEN)
            if taken is None:
                raise Refusal("not_supported", "This Hermes does not say how it answers /new.")
            # One reset shows as one session's end, which can settle only one request.
            pending = {"key": key, "generation": chat.translator.generation.get(key, 1), "request": request,
                       "at": None}
            self.reserve(chat.new_session, pending, "key", "new session")

            def replied(said: str | None) -> bool:
                # A reset Hermes took is settled by the end of the session it replaced. Anything else is
                # a refusal, or a question of its own: no session was replaced. None: it will not run.
                if (said is not None and taken.match(said)) or pending not in chat.new_session:
                    return False
                chat.new_session.remove(pending)
                return True
            await self.handed(chat.new_session, pending, self.command(chat, where, sender, "/new", request, replied))
            return
        if kind == "set":
            return await self.set(request, by, action)
        raise Refusal("not_supported")

    async def set(self, request: str, by: dict, action: dict) -> None:
        """Only the model can be set from the chat for now: by `/model <id>`, which Hermes answers in
        text. When the reply says Hermes took it, the plugin asks Hermes for the session's model, and the
        session item that read shows settles the request; any other reply is a refusal, and settles it."""
        session, setting, value = text_of(action.get("session")), action.get("setting"), action.get("value")
        if setting != "model":
            raise Refusal("not_supported")
        if not isinstance(value, str) or not value.strip() or len(value.split()) != 1:
            raise Refusal("invalid")
        chat = next((c for c in self.chats.values() if f"session:{session}" in c.translator.last), None)
        if chat is None:
            raise Refusal("nothing_to_do")
        where, sender = self.source_of(chat, session, by)
        taken = self.taken_by(MODEL_TAKEN)
        if taken is None:
            raise Refusal("not_supported", "This Hermes does not say how it answers /model.")
        # The model read after a switch settles it, and a read shows only the model then: one read cannot
        # tell two switches apart.
        pending = {"request": request, "session": session, "taken": False, "at": None}
        self.reserve(chat.sets, pending, "session", "change of model")

        def replied(said: str | None) -> bool:
            if said is not None and taken.match(said):
                # Hermes took it: the session's model, read from now on, shows it, and settles the request.
                pending["taken"] = True
                chat.translator.model_switched(self.key_of(where, sender))
                self.background(self.hermes.read_model_settings(where, sender))
                return False
            if pending not in chat.sets:
                return False
            chat.sets.remove(pending)
            return True
        await self.handed(chat.sets, pending, self.command(chat, where, sender, f"/model {value}", request, replied))
