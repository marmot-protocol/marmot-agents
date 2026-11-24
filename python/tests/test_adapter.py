"""The adapter's own work, without Hermes: what it asks Hermes in the background, and where the answer
goes. Hermes's `gateway` module is replaced by a stand-in that holds only what the adapter imports."""

import asyncio
import dataclasses
import enum
import json
import logging
import os
import sys
import tempfile
import threading
import time
import types
import unittest
from pathlib import Path
from unittest import mock


def stand_in_for_gateway():
    config = types.ModuleType("gateway.config")
    base = types.ModuleType("gateway.platforms.base")

    class Platform:
        def __init__(self, value):
            self.value = value

    class BasePlatformAdapter:
        def __init__(self, config, platform):
            self.config, self.platform = config, platform

        async def send_image(self, chat_id, image_url, caption=None, reply_to=None, metadata=None):
            # Hermes's own: the address as text.
            return await self.send(chat_id=chat_id, content=image_url, reply_to=reply_to, metadata=metadata)

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

    config.Platform = Platform
    base.BasePlatformAdapter, base.MessageType, base.MessageEvent, base.SendResult = (
        BasePlatformAdapter, MessageType, MessageEvent, SendResult)
    for name, module in (("gateway", types.ModuleType("gateway")), ("gateway.config", config),
                         ("gateway.platforms", types.ModuleType("gateway.platforms")),
                         ("gateway.platforms.base", base)):
        sys.modules.setdefault(name, module)


stand_in_for_gateway()
from gateway.platforms.base import MessageEvent, MessageType  # noqa: E402
from marmot_hermes import adapter as adapter_module  # noqa: E402
from marmot_hermes.adapter import MarmotAdapter  # noqa: E402
from marmot_hermes.core import CoreConnection, Refusal  # noqa: E402
from protocol import event_problems, problems  # noqa: E402


def checked_send(test):
    """Keeps each event the adapter sends in `test.sent`, and what is not as the core's types say
    (`fixtures/socket`) in `test.invalid`."""
    test.sent, test.invalid = [], []

    def send(event):
        test.invalid += [f"an event: {p}" for p in event_problems(json.loads(json.dumps(event)))]
        test.sent.append(event)
    return send


@dataclasses.dataclass
class Source:
    chat_id: str
    user_id: str
    chat_type: str = "group"
    platform: str = "marmot"


class ModelReads(unittest.IsolatedAsyncioTestCase):
    """Hermes says a session's model only when asked. The adapter asks in the background when a
    session's first turn starts, so two senders' reads, or a read and a new session, can overlap."""

    async def asyncSetUp(self):
        self.folder = tempfile.TemporaryDirectory()
        with mock.patch.dict("os.environ", {"HERMES_HOME": self.folder.name}):
            self.adapter = MarmotAdapter({})
        self.adapter.loop = asyncio.get_running_loop()
        self.adapter.connector.send_event = checked_send(self)
        self.gates = {"u-1": threading.Event(), "u-2": threading.Event()}
        self.failing = False
        self.adapter.get_session_model_settings = self.settings

    async def asyncTearDown(self):
        for gate in self.gates.values():
            gate.set()
        await asyncio.gather(*self.adapter.reading)
        self.folder.cleanup()
        self.assertEqual(self.invalid, [], "every event is as the core's types say")

    def settings(self, source):
        """Hermes's answer, in a worker thread, once the test lets it through."""
        self.gates[source.user_id].wait(5)
        if self.failing == "unusable":
            return None
        if self.failing == "blank":
            # Hermes's getter with no model configured: the session named, its model blank.
            return {"session_key": f"agent:main:marmot:group:c-2:{source.user_id}", "model": "",
                    "provider": "custom", "base_url": ""}
        if self.failing:
            raise RuntimeError("Hermes failed")
        return {"session_key": f"agent:main:marmot:group:c-2:{source.user_id}", "model": f"model-{source.user_id}",
                "provider": "custom", "base_url": "https://example.invalid"}

    async def start(self, message_id, user):
        event = MessageEvent(text="hello", message_type=MessageType.TEXT, source=Source("c-2", user),
                             message_id=message_id)
        await self.adapter.on_processing_start(event)

    async def answered(self, user):
        """Lets Hermes answer the read for `user`, and waits until the adapter has taken it."""
        before = len(self.adapter.reading)
        self.gates[user].set()
        while len(self.adapter.reading) >= before:
            await asyncio.sleep(0.01)

    def shown(self, session):
        return [e["text"] for e in self.sent if e["type"] == "session" and e["session"] == session][-1]

    async def test_overlapping_reads_reach_their_own_sessions(self):
        await self.start("m-a", "u-1")
        await self.start("m-b", "u-2")
        self.assertEqual(len(self.adapter.reading), 2)
        # Alice's read comes back while Bob's turn is the latest.
        await self.answered("u-1")
        await self.answered("u-2")
        self.assertEqual(self.shown("group:c-2:u-1"), "Model: model-u-1, Provider: custom")
        self.assertEqual(self.shown("group:c-2:u-2"), "Model: model-u-2, Provider: custom")

    async def test_each_read_and_its_answer_carry_one_id_of_their_own(self):
        """Two reads of one session may be under way at once, as one at a turn's start and one after a
        `/model`: each answer names its own read, so neither is taken for the other."""
        lines = []
        record = self.adapter.connector.record
        self.adapter.connector.record = lambda line, chat: (lines.append(line), record(line, chat))
        await self.start("m-a", "u-1")
        self.adapter.read_settings(Source("c-2", "u-1"))
        await self.answered("u-1")
        await asyncio.gather(*self.adapter.reading)
        reads = [(line["dir"], line["msg"].get("id")) for line in lines
                 if line["msg"].get("method") == "get_session_model_settings"]
        asked = [r for d, r in reads if d == "out"]
        self.assertEqual(len(set(asked)), 2)
        self.assertEqual(sorted(r for d, r in reads if d == "in"), sorted(asked))

    async def test_a_read_overtaken_by_a_new_session_failed_or_unusable_is_not_kept(self):
        connector = self.adapter.connector
        await self.start("m-a", "u-1")
        # Alice starts a new session while the read of her first one is under way.
        connector.record({"dir": "out", "via": "inbound", "msg": {
            "message_id": "n-1", "chat": "c-2", "chat_type": "group", "user": "u-1", "text": "/new"}}, "c-2")
        connector.record({"dir": "in", "via": "hook", "msg": {"method": "send", "params": {
            "chat_id": "c-2", "content": "✨ Session reset! Starting fresh.", "reply_to": "n-1",
            "metadata": {"notify": True}}}}, "c-2")
        self.assertEqual(self.shown("group:c-2:u-1#2"), "")
        await self.answered("u-1")
        self.assertEqual(self.shown("group:c-2:u-1#2"), "")
        # The new session's first turn asks again; when that fails, the next turn asks once more.
        self.gates["u-1"].clear()
        self.failing = True
        await self.start("m-c", "u-1")
        await self.answered("u-1")
        self.assertEqual(self.shown("group:c-2:u-1#2"), "")
        self.assertTrue(connector.wants_model("group", "c-2", "u-1"))
        # An answer that is no use ends its read too. No read that has ended is kept.
        self.gates["u-1"].clear()
        self.failing = "unusable"
        self.adapter.read_settings(Source("c-2", "u-1"))
        await self.answered("u-1")
        self.assertEqual(self.shown("group:c-2:u-1#2"), "")
        self.assertEqual(connector.chats["c-2"].translator.reads, {})

    async def test_a_first_read_with_no_model_is_asked_again(self):
        """A session's first read that names it but gives a blank model shows nothing, and the next turn
        asks again, as after a read that failed."""
        connector = self.adapter.connector
        self.failing = "blank"
        await self.start("m-a", "u-1")
        await self.answered("u-1")
        self.assertEqual(self.shown("group:c-2:u-1"), "")
        self.assertEqual(connector.chats["c-2"].translator.reads, {})
        self.gates["u-1"].clear()
        self.failing = False
        await self.start("m-b", "u-1")
        await self.answered("u-1")
        self.assertEqual(self.shown("group:c-2:u-1"), "Model: model-u-1, Provider: custom")


class Phrases(unittest.TestCase):
    """Hermes answers a command in text from its catalog: the adapter reads it from Hermes's own `t`."""

    def test_a_phrase_is_hermess_own(self):
        i18n = types.ModuleType("agent.i18n")
        i18n.t = lambda key, **fields: f"{key}|{fields['model']}"
        with tempfile.TemporaryDirectory() as home, mock.patch.dict("os.environ", {"HERMES_HOME": home}), \
                mock.patch.dict(sys.modules, {"agent": types.ModuleType("agent"), "agent.i18n": i18n}):
            adapter = MarmotAdapter({})
            self.assertEqual(adapter.phrase("gateway.model.switched", model="m"), "gateway.model.switched|m")


def stand_in_for_slash_confirm():
    """Hermes's `tools.slash_confirm`, as the pinned commit has it: one confirmation waits in a session,
    a new one replaces it, and `resolve` runs the one it names and no other, once, while it is fresh, by
    Hermes's clock (`time.time`). What its handler gives that is not text comes back as None, as when it
    ran none."""
    module = types.ModuleType("tools.slash_confirm")
    module.waiting, module.DEFAULT_TIMEOUT_SECONDS = {}, 300

    def register(session_key, confirm_id, command, handler, authority=None):
        module.waiting[session_key] = {"confirm_id": confirm_id, "command": command, "handler": handler,
                                       "created_at": time.time(), "authority": authority}

    def get_pending(session_key):
        entry = module.waiting.get(session_key)
        return dict(entry) if entry else None

    def clear_if_stale(session_key, timeout=300):
        entry = module.waiting.get(session_key)
        stale = bool(entry and time.time() - entry["created_at"] > timeout)
        if stale:
            module.waiting.pop(session_key)
        return stale

    async def resolve(session_key, confirm_id, choice, timeout=300):
        entry = module.waiting.get(session_key)
        if not entry or entry["confirm_id"] != confirm_id:
            return None
        module.waiting.pop(session_key)
        if time.time() - entry["created_at"] > timeout:
            return None
        result = await entry["handler"](choice)
        return result if isinstance(result, str) else None

    module.register, module.get_pending, module.clear_if_stale, module.resolve = (
        register, get_pending, clear_if_stale, resolve)
    return module


def stand_in_for_approval_actor(asked):
    """Hermes's `gateway.approval_actor`, as far as a confirmation goes: with an admin list for the chat's
    scope (`group_allow_admin_from` in the platform's `extra`), only an admin may answer, as the answer
    runs an admin-only command. Each check is kept in `asked`."""
    module = types.ModuleType("gateway.approval_actor")

    def slash_confirm_refusal(session_key, confirm_id, actor_id, *, admin_ids=(), extra=None, admitted=True):
        asked.append((session_key, confirm_id, actor_id, extra))
        admins = (extra or {}).get("group_allow_admin_from") or []
        return None if not admins or actor_id in admins else "\0admin-only:new"

    def refusal_text(requester):
        return f"⛔ /{requester.split(':')[-1]} is admin-only here."

    module.slash_confirm_refusal, module.refusal_text = slash_confirm_refusal, refusal_text
    return module


class Clock:
    """Hermes's clock, read in turn: each reading is the next of `readings`, and then the last again."""

    def __init__(self, *readings):
        self.readings = list(readings)

    def __call__(self):
        return self.readings.pop(0) if len(self.readings) > 1 else self.readings[0]


class Confirmations(unittest.IsolatedAsyncioTestCase):
    """The adapter answers Hermes's request to confirm a command by its id, through Hermes's own call
    (D-146): never another confirmation, never one that no longer waits, and never for someone Hermes's
    own check refuses."""

    async def asyncSetUp(self):
        self.module = stand_in_for_slash_confirm()
        self.checks = []
        patch = mock.patch.dict(sys.modules, {"tools": types.ModuleType("tools"), "tools.slash_confirm": self.module,
                                              "gateway.approval_actor": stand_in_for_approval_actor(self.checks)})
        patch.start()
        self.addCleanup(patch.stop)
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.extra = {}
        with mock.patch.dict("os.environ", {"HERMES_HOME": self.folder.name}):
            self.adapter = MarmotAdapter(types.SimpleNamespace(extra=self.extra))
        self.ran = []

    def asks(self, confirm_id, says="✨ Session reset! Starting fresh.", at=0.0, pause=None):
        """Hermes registers a confirmation whose handler, once run, pauses on `pause` if given, as a `/new`
        closing an old session's tools can, before it says `says`."""
        async def handler(choice):
            self.ran.append((confirm_id, choice))
            if pause is not None:
                await pause.wait()
                self.ran.append((confirm_id, "finished"))
            return says
        with mock.patch("time.time", return_value=at):
            self.module.register("agent:main:marmot:dm:c-1", confirm_id, "new", handler)

    async def resolve(self, confirm_id, choice, actor="u-1", now=1.0):
        clock = now if isinstance(now, Clock) else Clock(now)
        with mock.patch("time.time", clock):
            running = self.adapter.resolve_confirm("agent:main:marmot:dm:c-1", confirm_id, choice, actor)
            return None if running is None else await running

    async def test_hermes_has_taken_the_confirmation_when_the_call_returns_and_the_command_runs_on(self):
        """D-146, D-058. The call returns once Hermes has taken the confirmation, before the command it
        confirms finishes: the core is answered at once, and what Hermes says to it comes later."""
        pause = asyncio.Event()
        self.asks("k-1", pause=pause)
        with mock.patch("time.time", return_value=1.0):
            running = self.adapter.resolve_confirm("agent:main:marmot:dm:c-1", "k-1", "once", "u-1")
        self.assertIsNotNone(running)
        self.assertIsNone(self.module.get_pending("agent:main:marmot:dm:c-1"), "Hermes took it")
        self.assertEqual((self.ran, running.done()), ([("k-1", "once")], False), "the command runs on")
        pause.set()
        self.assertEqual(await running, "✨ Session reset! Starting fresh.")
        self.assertEqual(self.ran, [("k-1", "once"), ("k-1", "finished")])

    async def test_only_the_confirmation_named_is_answered(self):
        self.asks("k-1")
        self.asks("k-2")
        self.assertIsNone(await self.resolve("k-1", "once"))
        self.assertEqual(self.ran, [], "a confirmation another replaced runs nothing, nor the other")
        said = await self.resolve("k-2", "cancel")
        self.assertEqual((said, self.ran), ("✨ Session reset! Starting fresh.", [("k-2", "cancel")]))
        self.assertIsNone(await self.resolve("k-2", "once"))
        self.assertEqual(self.ran, [("k-2", "cancel")], "once only")

    async def test_one_that_waited_too_long_is_not_run(self):
        self.asks("k-1")
        self.assertIsNone(await self.resolve("k-1", "once", now=301))
        self.assertEqual((self.ran, self.module.waiting), ([], {}))

    async def test_one_that_expires_on_the_way_to_hermes_is_not_waiting(self):
        """Fresh when the plugin looks, and too old a moment later when Hermes looks: Hermes runs nothing
        and says None, as it would for a handler that said nothing. The plugin's own look comes earlier
        than Hermes's limit, so what it lets through is what Hermes runs."""
        self.asks("k-1")
        self.assertIsNone(await self.resolve("k-1", "once", now=Clock(299.9995, 300.0001)))
        self.assertEqual(self.ran, [])
        # Fresh at each of the plugin's looks, and too old by Hermes's: the plugin looks a few seconds early.
        self.asks("k-2")
        self.assertIsNone(await self.resolve("k-2", "once", now=Clock(299.9995, 299.9996, 300.0001)))
        self.assertEqual(self.ran, [])
        self.assertEqual(self.checks, [], "nothing past the look")
        # Within a few seconds of Hermes's limit the plugin answers nothing, so a moment's stall between
        # its look and Hermes's, such as a second, does not cross Hermes's limit.
        self.asks("k-3")
        self.assertIsNone(await self.resolve("k-3", "once", now=Clock(298.99, 298.99, 300.01)))
        self.assertEqual((self.ran, self.checks), ([], []))

    async def test_one_hermes_finds_too_old_after_the_plugin_let_it_through_gives_no_reply(self):
        """Fresh by the plugin's look, a few seconds early, and too old by Hermes's after a longer stall:
        Hermes runs nothing and says None, as it does for a handler that said nothing, and the plugin
        cannot tell the two apart. It says only that Hermes gave no reply, never that the command ran."""
        self.asks("k-1")
        self.assertEqual(await self.resolve("k-1", "once", now=Clock(294.9, 294.9, 300.01)), "")
        self.assertEqual((self.ran, self.module.waiting), ([], {}), "Hermes ran nothing")

    async def test_a_confirmation_hermes_says_nothing_to_is_still_answered(self):
        """A handler that ran and said nothing, or gave Hermes something other than text: Hermes says None,
        and the plugin says it was answered with no reply."""
        self.asks("k-1", says=None)
        self.assertEqual(await self.resolve("k-1", "once"), "")
        self.assertEqual(self.ran, [("k-1", "once")])
        self.asks("k-2", says=object())
        self.assertEqual(await self.resolve("k-2", "once"), "")

    async def test_hermes_own_check_of_who_may_answer_is_asked_first(self):
        """D-146. With an admin list for the chat, Hermes lets only an admin answer, as its own platforms
        ask (`gateway.approval_actor.slash_confirm_refusal`): a person the core lets prompt is refused, and
        the confirmation still waits for someone who may."""
        self.extra["group_allow_admin_from"] = ["u-admin"]
        self.asks("k-1")
        with self.assertRaises(Refusal) as refused:
            await self.resolve("k-1", "once", actor="u-1")
        self.assertEqual((refused.exception.reason, refused.exception.detail), ("not_supported", "⛔ /new is admin-only here."))
        self.assertEqual(self.ran, [])
        self.assertEqual(self.module.get_pending("agent:main:marmot:dm:c-1")["confirm_id"], "k-1", "still waiting")
        self.assertEqual(self.checks, [("agent:main:marmot:dm:c-1", "k-1", "u-1", self.extra)])
        self.assertEqual(await self.resolve("k-1", "once", actor="u-admin"), "✨ Session reset! Starting fresh.")
        self.assertEqual(self.ran, [("k-1", "once")])

    async def test_a_command_that_runs_long_neither_holds_the_core_nor_stops_when_it_goes(self):
        """The core's `answer`, through the adapter's real connection, is answered as soon as Hermes takes
        the confirmation, while the command runs on, and the core going away does not cancel the command
        (D-058)."""
        self.adapter.loop = asyncio.get_running_loop()
        self.adapter.connector.send_event = checked_send(self)
        pause = asyncio.Event()
        self.asks("k-1", pause=pause)
        self.adapter.connector.record({"dir": "in", "via": "hook", "msg": {"method": "send_slash_confirm", "params": {
            "chat_id": "c-1", "title": "/new", "message": "Confirm /new", "session_key": "agent:main:marmot:dm:c-1",
            "confirm_id": "k-1", "metadata": None}}}, "c-1")
        core = AnsweringCore({"request": "r-2", "item": "confirm:k-1", "by": {"id": "u-1", "name": "Alice"},
                              "given": {"questions": [{"question": "q0", "options": ["o0"]}]}})
        socket = str(Path(self.folder.name) / "core.sock")
        server = await asyncio.start_unix_server(core.serve, socket)
        self.addAsyncCleanup(server.wait_closed)
        self.addCleanup(server.close)
        self.adapter.core = connection = await CoreConnection.open(socket)
        with mock.patch("time.time", return_value=1.0):
            await connection.hello({"token": "t", "capabilities": {}}, self.adapter.connector.serve)
            answered = await asyncio.wait_for(core.answered, 2)
        self.assertEqual(answered["result"], {}, "answered while the command still runs")
        self.assertEqual(self.ran, [("k-1", "once")])
        await asyncio.wait_for(connection.done(), 2)
        pause.set()
        for _ in range(5):
            await asyncio.sleep(0)
        self.assertEqual(self.ran, [("k-1", "once"), ("k-1", "finished")], "the core going away cancelled nothing")
        self.assertEqual(self.invalid, [])


class AdminChecks(unittest.TestCase):
    """D-151. Before it answers an approval card or an agent's question, the adapter asks Hermes's own
    check of who may answer it (`gateway.approval_actor`), with the chat's Hermes settings, as Hermes's
    own platforms do: someone Hermes refuses is refused with Hermes's words, and nothing is answered."""

    def setUp(self):
        self.asked, self.resolved = [], []
        actor = types.ModuleType("gateway.approval_actor")
        admins = lambda extra: (extra or {}).get("group_allow_admin_from") or []  # noqa: E731

        def approval_refusal(session_key, actor_id, *, admin_ids=(), request_id=None, extra=None, choice=None,
                             admitted=True):
            self.asked.append(("approval", session_key, actor_id, request_id, choice, extra))
            return None if not admins(extra) or actor_id in admins(extra) else "\0admin-only:approve"

        def clarify_refusal(clarify_id, actor_id, *, admin_ids=(), extra=None, admitted=True):
            self.asked.append(("clarify", clarify_id, actor_id, extra))
            return None if not admins(extra) or actor_id in admins(extra) else "Alice"

        actor.approval_refusal, actor.clarify_refusal = approval_refusal, clarify_refusal
        actor.refusal_text = lambda r: f"⛔ /{r.split(':')[-1]} is admin-only here." if r.startswith("\0") else f"Only {r} can answer this."
        approval = types.ModuleType("tools.approval")

        def resolve_gateway_approval_card(session_key, choice, request_id, *, actor_id=None, admin_ids=(),
                                          extra=None, admitted=True):
            self.resolved.append(("card", session_key, choice, request_id, actor_id, extra))
            return 1
        approval.resolve_gateway_approval_card = resolve_gateway_approval_card
        clarify = types.ModuleType("tools.clarify_gateway")

        def resolve_gateway_clarify(clarify_id, text):
            self.resolved.append(("clarify", clarify_id, text))
            return True
        clarify.resolve_gateway_clarify = resolve_gateway_clarify
        patch = mock.patch.dict(sys.modules, {"tools": types.ModuleType("tools"), "tools.approval": approval,
                                              "tools.clarify_gateway": clarify, "gateway.approval_actor": actor})
        patch.start()
        self.addCleanup(patch.stop)
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.extra = {}
        with mock.patch.dict("os.environ", {"HERMES_HOME": self.folder.name}):
            self.adapter = MarmotAdapter(types.SimpleNamespace(extra=self.extra))

    def test_an_approval_card_is_answered_only_for_someone_hermes_lets_answer_it(self):
        self.extra["group_allow_admin_from"] = ["u-admin"]
        with self.assertRaises(Refusal) as refused:
            self.adapter.resolve_card("agent:main:marmot:group:c-2:u-1", "once", "rq", "u-2")
        self.assertEqual((refused.exception.reason, refused.exception.detail), ("not_supported", "⛔ /approve is admin-only here."))
        self.assertEqual(self.resolved, [], "nothing answered: the card still waits")
        self.assertEqual(self.asked, [("approval", "agent:main:marmot:group:c-2:u-1", "u-2", "rq", "once", self.extra)])
        self.assertEqual(self.adapter.resolve_card("agent:main:marmot:group:c-2:u-1", "once", "rq", "u-admin"), 1)
        self.assertEqual(self.resolved, [("card", "agent:main:marmot:group:c-2:u-1", "once", "rq", "u-admin", self.extra)],
                         "Hermes's own call is given the chat's settings too")

    def test_a_question_is_answered_only_for_someone_hermes_lets_answer_it(self):
        self.extra["group_allow_admin_from"] = ["u-admin"]
        with self.assertRaises(Refusal) as refused:
            self.adapter.resolve_question("k-1", "blue", "u-2")
        self.assertEqual((refused.exception.reason, refused.exception.detail), ("not_supported", "Only Alice can answer this."))
        self.assertEqual(self.resolved, [])
        self.assertTrue(self.adapter.resolve_question("k-1", "blue", "u-admin"))
        self.assertEqual(self.resolved, [("clarify", "k-1", "blue")])
        self.assertEqual(self.asked[-1], ("clarify", "k-1", "u-admin", self.extra))


class AnsweringCore:
    """The core's side of the socket, on a real Unix socket: it takes the hello, sends one `answer`, keeps
    the plugin's response to it in `answered`, and goes away."""

    def __init__(self, answer):
        self.answer = answer
        self.answered = asyncio.get_running_loop().create_future()

    async def serve(self, reader, writer):
        hello = json.loads(await reader.readline())
        writer.write(json.dumps({"jsonrpc": "2.0", "id": hello["id"], "result": {}}).encode() + b"\n")
        writer.write(json.dumps({"jsonrpc": "2.0", "id": 7, "method": "answer", "params": self.answer}).encode() + b"\n")
        while line := await reader.readline():
            m = json.loads(line)
            if m.get("id") == 7 and "method" not in m:
                self.answered.set_result(m)
                break
        writer.close()


class FakeCore:
    """The core's side of the socket, on a real Unix socket: it takes the hello, and answers each
    `send_file` with `answer(path)`, None for sent or a (reason, detail) refusal."""

    def __init__(self, answer):
        self.answer = answer
        self.asked = []
        self.invalid = []

    async def serve(self, reader, writer):
        hello = json.loads(await reader.readline())
        writer.write(json.dumps({"jsonrpc": "2.0", "id": hello["id"], "result": {}}).encode() + b"\n")
        while line := await reader.readline():
            m = json.loads(line)
            if m.get("method") != "send_file":
                continue
            self.asked.append(m["params"])
            self.invalid += [f"from the plugin: {p}" for p in problems("FromConnector", m)]
            no = self.answer(m["params"]["path"])
            reply = {"jsonrpc": "2.0", "id": m["id"], "result": {}} if no is None else {
                "jsonrpc": "2.0", "id": m["id"], "error": {"code": -32000, "message": no[0], "data": {
                    "reason": no[0], "detail": no[1]}}}
            writer.write(json.dumps(reply).encode() + b"\n")


class Files(unittest.IsolatedAsyncioTestCase):
    """What Hermes hands a platform to send a file (`send_document`, `send_image_file`, `send_video`,
    `send_voice`, `send_multiple_images`) becomes the core's `send_file`, one request for each file, and
    the answer goes back to Hermes as its SendResult (D-090). The core decides what may be sent."""

    OUTSIDE = ("not_allowed", "The file is outside the folders the agent may send files from.")
    NOT_WHOLE = ("invalid", "The agent did not give a whole path.")

    async def asyncSetUp(self):
        self.folder = tempfile.TemporaryDirectory()
        with mock.patch.dict("os.environ", {"HERMES_HOME": self.folder.name}):
            self.adapter = MarmotAdapter({})
        self.adapter.loop = asyncio.get_running_loop()
        self.adapter.connector.send_event = checked_send(self)
        self.fake = FakeCore(lambda path: self.NOT_WHOLE if not path.startswith("/") else
                             self.OUTSIDE if path.startswith("/etc/") else None)
        socket = str(Path(self.folder.name) / "core.sock")
        self.server = await asyncio.start_unix_server(self.fake.serve, socket)
        self.adapter.core = self.core = await CoreConnection.open(socket)
        await self.adapter.core.hello({"token": "t", "capabilities": {}}, self.adapter.connector.serve)

    async def asyncTearDown(self):
        self.core.close()
        await self.core.done()
        self.server.close()
        await self.server.wait_closed()
        self.folder.cleanup()
        self.assertEqual(self.invalid, [], "every event is as the core's types say")
        self.assertEqual(self.fake.invalid, [], "every send_file is as the core's types say")

    def words(self):
        return [e["text"] for e in self.sent if e["type"] == "message"]

    async def test_a_file_is_sent_and_hermes_is_told(self):
        result = await self.adapter.send_document(chat_id="c-1", file_path="/work/report.pdf")
        self.assertEqual(self.fake.asked, [{"chat": "c-1", "path": "/work/report.pdf"}])
        self.assertTrue(result.success)
        self.assertEqual(self.words(), [])

    async def test_a_refused_file_fails_with_the_cores_reason_and_hermes_adds_no_notice_of_its_own(self):
        result = await self.adapter.send_image_file(chat_id="c-1", image_path="/etc/passwd")
        self.assertFalse(result.success)
        self.assertEqual(result.error, self.OUTSIDE[1])
        # The core shows its own refused notice in the chat; Hermes's would name the file.
        self.assertEqual(result.raw_response, {"failure_notice_sent": True})

    async def test_a_batch_sends_each_file_once_with_one_caption_as_the_agents_words(self):
        images = [("file:///work/one%20chart.png", ""), ("file:///work/two.png", "Two charts"),
                  ("file:///etc/three.png", "Another caption")]
        result = await self.adapter.send_multiple_images(chat_id="c-1", images=images)
        self.assertEqual([a["path"] for a in self.fake.asked], ["/work/one chart.png", "/work/two.png", "/etc/three.png"])
        # One sent is enough for Hermes to count the batch delivered.
        self.assertTrue(result.success)
        self.assertEqual(self.words(), ["Two charts"])

    async def test_a_path_that_is_not_whole_goes_as_it_is_and_the_core_refuses_it(self):
        # Hermes hands a platform only whole paths, once its own checks resolved them: no folder of
        # Hermes's is guessed at for one that is not, from its environment or the plugin's own.
        with mock.patch.dict("os.environ", {"TERMINAL_CWD": "/work/project"}):
            video = await self.adapter.send_video(chat_id="c-1", video_path="out/clip.mp4", caption="The clip")
            voice = await self.adapter.send_voice(chat_id="c-1", audio_path="~/notes.ogg")
        self.assertEqual([a["path"] for a in self.fake.asked], ["out/clip.mp4", "~/notes.ogg"])
        for result in (video, voice):
            self.assertFalse(result.success)
            self.assertEqual(result.raw_response, {"failure_notice_sent": True})
        self.assertEqual(self.words(), ["The clip"])

    async def test_an_image_on_the_web_is_left_to_hermes(self):
        result = await self.adapter.send_multiple_images(chat_id="c-1", images=[("https://example.invalid/a.png", "")])
        self.assertEqual(self.fake.asked, [])
        self.assertTrue(result.success)
        self.assertEqual(self.words(), ["https://example.invalid/a.png"])

    async def test_with_no_core_nothing_is_sent_and_hermes_says_so(self):
        self.adapter.core.close()
        self.adapter.core = None
        result = await self.adapter.send_document(chat_id="c-1", file_path="/work/report.pdf")
        self.assertFalse(result.success)
        self.assertNotEqual(result.raw_response, {"failure_notice_sent": True})

    async def test_a_send_from_another_thread_reaches_the_core(self):
        def elsewhere():
            return asyncio.run(self.adapter.send_document(chat_id="c-1", file_path="/work/a.txt"))

        result = await asyncio.to_thread(elsewhere)
        self.assertTrue(result.success)
        self.assertEqual(self.fake.asked, [{"chat": "c-1", "path": "/work/a.txt"}])


class Diagnostics(unittest.TestCase):
    def test_the_plugins_lines_go_to_its_own_file_and_nothing_else_does(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "plugin.log"
            with mock.patch.dict("os.environ", {"MARMOT_LOG": str(path)}), self.assertLogs("marmot_hermes"):
                MarmotAdapter.warn("the core closed the socket")
                logging.getLogger("gateway").warning("marmot: what Hermes logs")
            lines = path.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(lines), 1)
            self.assertTrue(lines[0].endswith(" marmot: the core closed the socket"))


class Home(unittest.TestCase):
    """Where the core's socket is: MARMOT_AGENT_HOME, as the container sets it, or else the platform's
    `home` setting, as the installer writes it into the person's own Hermes."""

    def adapter(self, config, variable=None):
        with tempfile.TemporaryDirectory() as folder, mock.patch.dict("os.environ", {"HERMES_HOME": folder}):
            os.environ.pop("MARMOT_AGENT_HOME", None)
            if variable:
                os.environ["MARMOT_AGENT_HOME"] = variable
            return MarmotAdapter(config)

    def test_the_setting_names_the_home(self):
        config = types.SimpleNamespace(extra={"home": "/agents/a/home"})
        self.assertEqual(self.adapter(config).home, Path("/agents/a/home"))

    def test_the_environment_comes_first(self):
        config = types.SimpleNamespace(extra={"home": "/agents/a/home"})
        self.assertEqual(self.adapter(config, "/run/marmot-agent").home, Path("/run/marmot-agent"))


class Hello(unittest.IsolatedAsyncioTestCase):
    async def test_the_hello_is_what_the_core_reads(self):
        """The hello the adapter gives, against the core's own schema (`fixtures/socket`)."""
        with tempfile.TemporaryDirectory() as folder:
            home = Path(folder)
            (home / "socket").mkdir()
            (home / "connector.token").write_text("a-token\n", encoding="utf-8")
            heard = asyncio.get_running_loop().create_future()

            async def core(reader, writer):
                heard.set_result(json.loads(await reader.readline()))
                writer.close()

            server = await asyncio.start_unix_server(core, str(home / "socket" / "marmot-agent.sock"))
            with mock.patch.dict("os.environ", {"HERMES_HOME": folder, "MARMOT_AGENT_HOME": folder}):
                adapter = MarmotAdapter({})
            running = asyncio.ensure_future(adapter.run_core())
            try:
                hello = await asyncio.wait_for(heard, 5)
            finally:
                running.cancel()
                server.close()
            self.assertEqual(hello["params"]["token"], "a-token")
            self.assertEqual(problems("FromConnector", hello), [])

    async def test_each_connection_starts_from_what_its_core_knows(self):
        """When a connection goes, the connector forgets what that core was told, before the next hello."""
        with tempfile.TemporaryDirectory() as folder:
            home = Path(folder)
            (home / "socket").mkdir()
            (home / "connector.token").write_text("a-token\n", encoding="utf-8")
            hellos = asyncio.Queue()

            async def core(reader, writer):
                hello = json.loads(await reader.readline())
                writer.write(json.dumps({"jsonrpc": "2.0", "id": hello["id"], "result": {}}).encode() + b"\n")
                await writer.drain()
                await hellos.put(gone.call_count)
                writer.close()

            server = await asyncio.start_unix_server(core, str(home / "socket" / "marmot-agent.sock"))
            with mock.patch.dict("os.environ", {"HERMES_HOME": folder, "MARMOT_AGENT_HOME": folder}):
                adapter = MarmotAdapter({})
            gone = mock.Mock()
            adapter.connector.core_gone = gone
            with mock.patch.object(adapter_module, "RECONNECT_SECONDS", 0.01):
                running = asyncio.ensure_future(adapter.run_core())
                try:
                    first = await asyncio.wait_for(hellos.get(), 5)
                    second = await asyncio.wait_for(hellos.get(), 5)
                finally:
                    running.cancel()
                    server.close()
            self.assertEqual((first, second), (0, 1))


if __name__ == "__main__":
    unittest.main()
