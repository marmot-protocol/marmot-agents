"""The adapter's own work, without Hermes: what it asks Hermes in the background, and where the answer
goes. Hermes's `gateway` module is replaced by a stand-in that holds only what the adapter imports."""

import asyncio
import dataclasses
import enum
import logging
import os
import sys
import tempfile
import threading
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

    config.Platform = Platform
    base.BasePlatformAdapter, base.MessageType, base.MessageEvent, base.SendResult = (
        BasePlatformAdapter, MessageType, MessageEvent, SendResult)
    for name, module in (("gateway", types.ModuleType("gateway")), ("gateway.config", config),
                         ("gateway.platforms", types.ModuleType("gateway.platforms")),
                         ("gateway.platforms.base", base)):
        sys.modules.setdefault(name, module)


stand_in_for_gateway()
from gateway.platforms.base import MessageEvent, MessageType  # noqa: E402
from marmot_hermes.adapter import MarmotAdapter  # noqa: E402


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
        self.sent = []
        self.adapter.connector.send_event = self.sent.append
        self.gates = {"u-1": threading.Event(), "u-2": threading.Event()}
        self.failing = False
        self.adapter.get_session_model_settings = self.settings

    async def asyncTearDown(self):
        for gate in self.gates.values():
            gate.set()
        await asyncio.gather(*self.adapter.reading)
        self.folder.cleanup()

    def settings(self, source):
        """Hermes's answer, in a worker thread, once the test lets it through."""
        self.gates[source.user_id].wait(5)
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

    async def test_a_read_overtaken_by_a_new_session_or_failed_is_not_kept(self):
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


if __name__ == "__main__":
    unittest.main()
