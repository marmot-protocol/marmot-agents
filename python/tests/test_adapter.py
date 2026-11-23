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
from marmot_hermes.adapter import MarmotAdapter  # noqa: E402
from marmot_hermes.core import CoreConnection  # noqa: E402
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


if __name__ == "__main__":
    unittest.main()
