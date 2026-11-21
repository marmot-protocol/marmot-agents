"""The plugin's side of the core's local socket (`docs/design/socket-protocol.md`, "The messages, as
built"): JSON-RPC 2.0 over a Unix socket, one message per line, no line over 4 MB either way (D-033).

The plugin opens with a hello. Then the core sends `prompt`, `answer` and `control` requests, which the
plugin answers with `{}` once it has handed them to Hermes, or with a refusal; and the plugin sends the
core `event` notifications (D-034, D-058). Requests are served one at a time, in the order they came.

Nothing here writes what a message holds to a log (D-071).
"""

from __future__ import annotations

import asyncio
import json

PROTOCOL = "marmot.agent-control.v3"
MAX_MESSAGE = 4 * 1024 * 1024
HELLO = 1  # the hello's id: the only request the plugin sends
REASONS = ("not_supported", "busy", "unknown_item", "not_waiting", "already_answered", "nothing_to_do", "invalid",
           "failed")


class Refusal(Exception):
    """A request not taken, with one of the eight reasons. The detail is shown in the chat."""

    def __init__(self, reason: str, detail: str | None = None):
        super().__init__(reason)
        assert reason in REASONS, reason
        self.reason = reason
        self.detail = detail


class HelloRefused(Exception):
    """The core refused the hello, or the connection went before it answered."""


def line_of(message) -> bytes | None:
    """A message as one line, or None when it is over the limit."""
    data = json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    return None if len(data) > MAX_MESSAGE else data + b"\n"


class CoreConnection:
    """One connection to the core. Its methods run on the event loop it was opened on, except
    `notify`, which may be called from any thread."""

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        self.reader = reader
        self.writer = writer
        self.loop = asyncio.get_running_loop()
        self.closed = False
        self.welcome: asyncio.Future = self.loop.create_future()
        self.serve = None

    @classmethod
    async def open(cls, path: str) -> CoreConnection:
        reader, writer = await asyncio.open_unix_connection(path, limit=MAX_MESSAGE + 1)
        return cls(reader, writer)

    async def hello(self, params: dict, serve) -> dict:
        """Gives the hello and serves the core's requests with `serve(method, params)` until the
        connection closes. Returns the core's answer to the hello."""
        self.serve = serve
        self.write({"jsonrpc": "2.0", "id": HELLO, "method": "hello", "params": {"protocol": PROTOCOL, **params}})
        self.reading = asyncio.ensure_future(self.read())
        return await self.welcome

    def notify(self, method: str, params) -> bool:
        """Sends a notification. One over the limit is not sent, and False says so."""
        data = line_of({"jsonrpc": "2.0", "method": method, "params": params})
        if data is None or self.closed:
            return False
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is self.loop:
            self.writer.write(data)
        else:
            self.loop.call_soon_threadsafe(self.writer.write, data)
        return True

    def write(self, message) -> None:
        data = line_of(message)
        if data is not None and not self.closed:
            self.writer.write(data)

    def close(self) -> None:
        if not self.closed:
            self.closed = True
            self.writer.close()

    async def done(self) -> None:
        """Waits until the connection has closed, from either side."""
        await asyncio.shield(self.reading)

    async def read(self) -> None:
        queue: asyncio.Queue = asyncio.Queue()
        serving = asyncio.ensure_future(self.answer_each(queue))
        try:
            while True:
                try:
                    raw = await self.reader.readuntil(b"\n")
                except asyncio.LimitOverrunError as over:
                    # A line over the limit is passed over as it arrives.
                    await self.skip_line(over.consumed)
                    continue
                except (asyncio.IncompleteReadError, ConnectionError, OSError):
                    break
                self.message(raw, queue)
        finally:
            self.closed = True
            if not self.welcome.done():
                self.welcome.set_exception(HelloRefused("the connection closed"))
            serving.cancel()
            try:
                self.writer.close()
            except Exception:
                pass

    async def skip_line(self, consumed: int) -> None:
        await self.reader.readexactly(consumed)
        while True:
            try:
                await self.reader.readuntil(b"\n")
                return
            except asyncio.LimitOverrunError as over:
                await self.reader.readexactly(over.consumed)

    def message(self, raw: bytes, queue: asyncio.Queue) -> None:
        try:
            m = json.loads(raw)
        except ValueError:
            return
        if not isinstance(m, dict) or m.get("jsonrpc") != "2.0":
            return
        method, id_ = m.get("method"), m.get("id")
        if isinstance(method, str):
            if id_ is None:
                return  # the core sends the plugin no notifications
            params = m.get("params")
            queue.put_nowait((id_, method, params if isinstance(params, dict) else {}))
            return
        if id_ == HELLO and not self.welcome.done():
            if "result" in m and "error" not in m:
                self.welcome.set_result(m["result"] if isinstance(m["result"], dict) else {})
            else:
                self.welcome.set_exception(HelloRefused("the core refused the hello"))

    async def answer_each(self, queue: asyncio.Queue) -> None:
        while True:
            id_, method, params = await queue.get()
            try:
                await self.serve(method, params)
                self.write({"jsonrpc": "2.0", "id": id_, "result": {}})
            except Exception as e:  # anything else that goes wrong refuses the request as failed
                refusal = e if isinstance(e, Refusal) else Refusal("failed")
                data = {"reason": refusal.reason}
                if refusal.detail is not None:
                    data["detail"] = refusal.detail
                self.write({"jsonrpc": "2.0", "id": id_, "error": {"code": -32000, "message": refusal.reason, "data": data}})
