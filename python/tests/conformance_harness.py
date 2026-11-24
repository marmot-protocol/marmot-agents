"""The Hermes plugin for the driver conformance suite (`crates/marmot-agent-conformance`, "the socket
connectors"): the plugin's own adapter and connector, on the core's real socket, with a scripted
Hermes behind them in place of the real one. No model and no network.

    MARMOT_AGENT_HOME=<the agent's home> python3 tests/conformance_harness.py   # from python/

Hermes's `gateway` module is replaced by a stand-in that holds what the adapter imports, as in
`test_adapter.py`, and what Hermes itself does with a message is played by `ScriptedAdapter`: it reads
the same words in a prompt as every fake of the suite (`#ask` an approval card, `#quiz` a question,
`#sleep` a turn that runs until `/stop`, `#die` a crash), and echoes the prompt in its reply.

The suite drives it through a control channel on its standard input and output, one JSON object a
line, which the plugin uses for nothing else (its log goes to standard error):

- out `{"heard": {...}}`: what this Hermes understood, in the suite's shapes (`Heard`);
- out `{"frame": <kind>, "wait": <bool>}`: each call the plugin made on Hermes, after what Hermes heard
  in it. With `wait`, Hermes goes on only once the suite answers `{"go": true}`, or `{"die": true}`,
  when it dies there;
- the process ending is Hermes dying, as the plugin runs inside Hermes.
"""

from __future__ import annotations

import asyncio
import dataclasses
import enum
import itertools
import json
import os
import sys
import types
from pathlib import Path

HERE = Path(__file__).resolve().parent


def stand_in_for_gateway():
    """Hermes's `gateway` modules, holding what the adapter imports, with the base adapter's helpers the
    adapter calls; the scripted Hermes below plays the rest."""
    config = types.ModuleType("gateway.config")
    base = types.ModuleType("gateway.platforms.base")

    class Platform:
        def __init__(self, value):
            self.value = value

    @dataclasses.dataclass
    class SessionSource:
        platform: str
        chat_id: str
        chat_name: str | None
        chat_type: str
        user_id: str
        user_name: str | None
        message_id: str | None = None

    class MessageType(enum.Enum):
        TEXT = "text"

    @dataclasses.dataclass
    class MessageEvent:
        text: str
        message_type: MessageType
        source: object
        raw_message: object = None
        message_id: str | None = None
        timestamp: object = None

    @dataclasses.dataclass
    class SendResult:
        success: bool
        message_id: str | None = None
        error: str | None = None
        raw_response: object = None

    class BasePlatformAdapter:
        def __init__(self, config, platform):
            self.config, self.platform = config, platform

        def build_source(self, chat_id, chat_name=None, chat_type="dm", user_id=None, user_name=None,
                         message_id=None, **_):
            return SessionSource("marmot", chat_id, chat_name, chat_type, user_id, user_name, message_id)

        def _mark_connected(self):
            pass

        def _mark_disconnected(self):
            pass

        async def send_clarify(self, chat_id, question, choices, clarify_id, session_key, metadata=None):
            return SendResult(success=True, message_id=f"clarify-{clarify_id}")

    config.Platform = Platform
    base.BasePlatformAdapter, base.MessageType, base.MessageEvent, base.SendResult = (
        BasePlatformAdapter, MessageType, MessageEvent, SendResult)
    for name, module in (("gateway", types.ModuleType("gateway")), ("gateway.config", config),
                         ("gateway.platforms", types.ModuleType("gateway.platforms")),
                         ("gateway.platforms.base", base)):
        sys.modules.setdefault(name, module)


stand_in_for_gateway()
sys.path.insert(0, str(HERE.parent))
from marmot_hermes import adapter as adapter_module  # noqa: E402
from marmot_hermes.adapter import MarmotAdapter  # noqa: E402

PHRASES = json.loads((HERE / "hermes_phrases.json").read_text(encoding="utf-8"))["phrases"]
# The plugin connects again this soon after the core goes, here: Hermes's own wait is two seconds.
adapter_module.RECONNECT_SECONDS = 0.1
IDS = itertools.count(1)


class Control:
    """The suite's side of the channel: what Hermes heard, and each call it took, out; the suite's word
    on a call Hermes waits on, in."""

    def __init__(self):
        self.verdicts: asyncio.Queue = asyncio.Queue()

    @staticmethod
    def out(message: dict) -> None:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()

    def heard(self, kind: str, **fields) -> None:
        self.out({"heard": {kind: fields}})

    async def frame(self, kind: str) -> None:
        """A call Hermes takes, and goes on with once the suite says; it dies there if the suite says so."""
        self.out({"frame": kind, "wait": True})
        verdict = await self.verdicts.get()
        if verdict.get("die"):
            os._exit(1)

    def frame_now(self, kind: str) -> None:
        """A call Hermes answers at once, as the plugin waits for it without yielding: counted only."""
        self.out({"frame": kind, "wait": False})

    async def read(self) -> None:
        loop = asyncio.get_running_loop()
        reader = asyncio.StreamReader()
        await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
        while line := await reader.readline():
            self.verdicts.put_nowait(json.loads(line))
        # The suite has gone: so does Hermes.
        os._exit(0)


@dataclasses.dataclass
class Turn:
    event: object
    session: str
    text: str
    script: str


class ScriptedAdapter(MarmotAdapter):
    """The plugin's adapter, with what Hermes does played by a script."""

    def __init__(self, control: Control):
        super().__init__({})
        self.control = control
        self.connector.hold = 0.05
        self.turns: dict[str, Turn] = {}  # session -> its running turn
        self.cards: dict[str, Turn] = {}  # request id -> the turn whose approval it is
        self.clarifies: dict[str, Turn] = {}  # clarify id -> the turn that asked

    # -- what the plugin asks of Hermes

    async def handle_message(self, event) -> None:
        source = event.source
        session = f"{source.chat_type}:{source.chat_id}:{source.user_id}"
        text = event.text or ""
        # Hermes has the message once it is handed over; the suite's word comes after, so a connection
        # it drops there drops after delivery.
        if text.startswith("/"):
            await self.control.frame("handle_message")
            return await self.command(event, session, text)
        self.control.heard("prompt", session=session, text=text, refused=False)
        await self.control.frame("handle_message")
        if session in self.turns:
            return  # Hermes steers a running turn with it
        script = next((w for w in ("#ask", "#quiz", "#sleep", "#die") if w in text), "")
        turn = Turn(event, session, text, script)
        self.turns[session] = turn
        await self.on_processing_start(event)
        if script == "#ask":
            request = f"rq-{next(IDS)}"
            self.cards[request] = turn
            prompt = {"chat_id": source.chat_id, "session_key": self.key(source), "command": text,
                      "description": "a command", "request_id": request,
                      "actions": [["Allow Once", "once", "primary"], ["Deny", "deny", "danger"]]}
            await self._send_exec_approval_prompt(prompt)
        elif script == "#quiz":
            clarify = f"cl-{next(IDS)}"
            self.clarifies[clarify] = turn
            await self.send_clarify(source.chat_id, f"{text}?", ["yes", "no"], clarify, self.key(source))
        elif script == "#die":
            await self.send(source.chat_id, "Running a command.", reply_to=event.message_id)
            os._exit(1)
        elif script != "#sleep":
            await self.finish(turn)

    async def command(self, event, session: str, text: str) -> None:
        source = event.source
        word = text.split()[0]
        if word == "/stop" and session in self.turns:
            self.control.heard("control", session=session, what="interrupt")
            turn = self.turns.pop(session)
            await self.send(source.chat_id, "⚡ Stopped. You can continue this session.", reply_to=event.message_id,
                            metadata={"notify": True})
            await self.on_processing_complete(turn.event, "cancelled")
            return
        self.control.heard("control", session=session, what=word.lstrip("/"))
        await self.send(source.chat_id, f"Unknown command {word}.", reply_to=event.message_id, metadata={"notify": True})

    async def finish(self, turn: Turn, outcome: str = "success") -> None:
        if self.turns.get(turn.session) is turn:
            del self.turns[turn.session]
        await self.send(turn.event.source.chat_id, f"Done: {turn.text}", reply_to=turn.event.message_id,
                        metadata={"notify": True})
        await self.on_processing_complete(turn.event, outcome)

    @staticmethod
    def key(source) -> str:
        """Hermes's name for the session: a group's after the chat and the sender, a direct chat's after the
        chat alone, as `fixtures/recordings/hermes` shows."""
        if source.chat_type == "group":
            return f"agent:main:marmot:group:{source.chat_id}:{source.user_id}"
        return f"agent:main:marmot:dm:{source.chat_id}"

    def resolve_card(self, session_key, choice, request_id, actor) -> int:
        self.control.frame_now("resolve_card")
        turn = self.cards.pop(request_id, None)
        if turn is None or self.turns.get(turn.session) is not turn:
            return 0
        self.control.heard("answer", session=turn.session, allow=choice != "deny")
        asyncio.ensure_future(self.finish(turn))
        return 1

    def resolve_question(self, clarify_id, text) -> bool:
        self.control.frame_now("resolve_question")
        turn = self.clarifies.pop(clarify_id, None)
        if turn is None or self.turns.get(turn.session) is not turn:
            return False
        self.control.heard("answer", session=turn.session, text=text)
        asyncio.ensure_future(self.finish(turn))
        return True

    def get_session_model_settings(self, source):
        return {"session_key": self.key(source), "model": "scripted", "provider": "custom"}

    def phrase(self, key: str, **fields) -> str:
        return PHRASES.get(key, key).format(**fields)

    def read_commands(self) -> None:
        self.connector.commands_read([{"name": "new", "description": "Start a new session.", "args_hint": "",
                                       "gateway_available": True}])


async def main() -> None:
    control = Control()
    reading = asyncio.ensure_future(control.read())
    hermes = ScriptedAdapter(control)
    await hermes.connect()
    await reading


if __name__ == "__main__":
    asyncio.run(main())
