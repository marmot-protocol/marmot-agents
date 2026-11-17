"""A Hermes platform plugin that records what Hermes hands a platform.

It plays the part of a chat network. It feeds Hermes scripted messages from named senders in
pretend chats, and writes down every call Hermes makes on the platform adapter: the method,
its arguments and what the default implementation returned. Nothing is sent anywhere.

Each output line is {"t": <ms>, "dir": "in"|"out", "via": ..., "msg": ...}:

    dir "out", via "inbound"   a scripted chat message this plugin handed to Hermes
    dir "in",  via "hook"      Hermes called a method on the adapter
    dir "out", via "result"    what that method returned to Hermes
    dir "out", via "note"      a marker written by the recorder itself

Environment:
    RECORDER_SCENARIO   path to a JSON file: {"steps": [...]}. A step is one of:
                          {"chat", "user", "text", ...}   a chat message; waits for the turn to end
                                                          unless "wait" is false
                          {"wait_for": "<method>"}        wait until Hermes calls that adapter method
                          {"wait_for_text": "<words>"}    wait until Hermes sends a message containing them
                          {"press": "<choice>", "user"}   press a button on the last approval card
                          {"wait_turn": true}             wait for the running turn to end
                          {"sleep": <seconds>}
    RECORDER_APPROVAL_BUTTONS  set to 1 to claim native approval buttons, so that Hermes hands over
                          a structured approval card where it would otherwise send text
    RECORDER_OUT        path of the .jsonl file to write
    RECORDER_TURN_TIMEOUT  seconds to wait for Hermes to finish a turn (default 900)
"""

from __future__ import annotations

import asyncio
import dataclasses
import enum
import functools
import inspect
import json
import os
import signal
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

from gateway.config import Platform
from gateway.platforms.base import BasePlatformAdapter, MessageEvent, MessageType, SendResult

PLATFORM = "recorder"

# Methods of the base adapter that Hermes calls to hand something to a platform. Each is
# wrapped so that the call is recorded and then carried out by the base implementation,
# which is what a platform with no special support would do.
RECORDED_PREFIXES = (
    "send", "edit_message", "delete_message", "play_tts", "begin_streaming_tts", "write_streaming_tts",
    "finish_streaming_tts", "abort_streaming_tts", "emit_", "stop_typing", "set_status_text",
    "on_processing_", "format_tool_event", "format_tool_preview", "render_message_event",
    "create_handoff_thread",
)


def jsonable(v, depth=0):
    """Turn anything Hermes passes into JSON, keeping structure and dropping what cannot be kept."""
    if depth > 6:
        return f"<{type(v).__name__}>"
    if v is None or isinstance(v, (bool, int, float, str)):
        return v
    if isinstance(v, enum.Enum):
        return v.value
    if isinstance(v, bytes):
        return {"_bytes": len(v)}
    if isinstance(v, datetime):
        return v.isoformat()
    if isinstance(v, Path):
        return str(v)
    if dataclasses.is_dataclass(v) and not isinstance(v, type):
        return {"_type": type(v).__name__, **{f.name: jsonable(getattr(v, f.name), depth + 1) for f in dataclasses.fields(v)}}
    if isinstance(v, dict):
        return {str(k): jsonable(x, depth + 1) for k, x in list(v.items())[:200]}
    if isinstance(v, (list, tuple, set, frozenset)):
        return [jsonable(x, depth + 1) for x in list(v)[:200]]
    if callable(v):
        return f"<callable {getattr(v, '__name__', type(v).__name__)}>"
    return f"<{type(v).__name__}>"


class Recording:
    def __init__(self, path: str):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.file = self.path.open("w", encoding="utf-8")
        self.t0 = time.monotonic()
        self.calls: dict[str, int] = {}
        self.sent: list[str] = []
        self.changed = asyncio.Event()

    def write(self, direction: str, via: str, msg: dict):
        self.file.write(json.dumps({"t": int((time.monotonic() - self.t0) * 1000), "dir": direction, "via": via, "msg": msg}, ensure_ascii=False) + "\n")
        self.file.flush()
        if via == "hook":
            self.calls[msg.get("method", "")] = self.calls.get(msg.get("method", ""), 0) + 1
            if msg.get("method") == "send":
                self.sent.append(str((msg.get("params") or {}).get("content") or ""))
            self.changed.set()

    async def wait_until(self, condition, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while not condition():
            left = deadline - time.monotonic()
            if left <= 0:
                return False
            self.changed.clear()
            try:
                await asyncio.wait_for(self.changed.wait(), timeout=left)
            except asyncio.TimeoutError:
                return False
        return True

    async def wait_for(self, method: str, count: int, timeout: float) -> bool:
        """Wait until Hermes has called `method` more than `count` times."""
        return await self.wait_until(lambda: self.calls.get(method, 0) > count, timeout)


def recorded(name, base_fn):
    """Wrap a base-adapter method: record the call, run the base implementation, record the result."""
    signature = inspect.signature(base_fn)

    def describe(self, args, kwargs):
        try:
            bound = signature.bind(self, *args, **kwargs)
            params = {k: jsonable(v) for k, v in bound.arguments.items() if k != "self"}
        except TypeError:
            params = {"args": jsonable(args), "kwargs": jsonable(kwargs)}
        self.recording.write("in", "hook", {"method": name, "params": params})

    if inspect.iscoroutinefunction(base_fn):
        @functools.wraps(base_fn)
        async def wrapper(self, *args, **kwargs):
            describe(self, args, kwargs)
            try:
                result = await base_fn(self, *args, **kwargs)
            except Exception as exc:  # record, then let Hermes see the failure as it would
                self.recording.write("out", "result", {"method": name, "error": f"{type(exc).__name__}: {exc}"})
                raise
            self.recording.write("out", "result", {"method": name, "result": jsonable(result)})
            return result
    else:
        @functools.wraps(base_fn)
        def wrapper(self, *args, **kwargs):
            describe(self, args, kwargs)
            result = base_fn(self, *args, **kwargs)
            self.recording.write("out", "result", {"method": name, "result": jsonable(result)})
            return result
    return wrapper


class RecorderAdapter(BasePlatformAdapter):
    def __init__(self, config):
        super().__init__(config, Platform(PLATFORM))
        self.recording = Recording(os.environ.get("RECORDER_OUT", "/out/recording.jsonl"))
        self.scenario = json.loads(Path(os.environ["RECORDER_SCENARIO"]).read_text(encoding="utf-8"))
        self.turn_timeout = float(os.environ.get("RECORDER_TURN_TIMEOUT", "900"))
        self.turn_done = asyncio.Event()
        self.runner = None
        self.n = 0
        self.last_prompt = None

    # -- what a platform must implement

    async def connect(self, *, is_reconnect: bool = False) -> bool:
        self.recording.write("in", "hook", {"method": "connect", "params": {"is_reconnect": is_reconnect}})
        self._mark_connected()
        if self.runner is None:
            self.runner = asyncio.create_task(self.run_scenario())
        return True

    async def disconnect(self) -> None:
        self.recording.write("in", "hook", {"method": "disconnect", "params": {}})
        self._mark_disconnected()

    async def send(self, chat_id, content, reply_to=None, metadata=None, **kwargs):
        self.recording.write("in", "hook", {"method": "send", "params": {
            "chat_id": chat_id, "content": content, "reply_to": reply_to, "metadata": jsonable(metadata), **jsonable(kwargs)}})
        self.n += 1
        result = SendResult(success=True, message_id=f"out-{self.n}")
        self.recording.write("out", "result", {"method": "send", "result": jsonable(result)})
        return result

    async def get_chat_info(self, chat_id):
        self.recording.write("in", "hook", {"method": "get_chat_info", "params": {"chat_id": chat_id}})
        chat = (self.scenario.get("chats") or {}).get(chat_id, {})
        info = {"name": chat.get("name", chat_id), "type": chat.get("type", "group"), "chat_id": chat_id}
        self.recording.write("out", "result", {"method": "get_chat_info", "result": info})
        return info

    async def on_processing_complete(self, event, outcome):
        self.recording.write("in", "hook", {"method": "on_processing_complete", "params": {
            "message_id": getattr(event, "message_id", None), "outcome": jsonable(outcome)}})
        self.turn_done.set()

    async def wait_turn(self, settle: float) -> bool:
        """Wait for the running turn to end. Hermes can report a turn complete and then start it
        again a moment later, so wait until nothing is still in progress."""
        deadline = time.monotonic() + self.turn_timeout
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                return False
            try:
                await asyncio.wait_for(self.turn_done.wait(), timeout=left)
            except asyncio.TimeoutError:
                return False
            await asyncio.sleep(settle)
            calls = self.recording.calls
            if calls.get("on_processing_start", 0) <= calls.get("on_processing_complete", 0):
                return True
            self.turn_done.clear()

    # -- the scripted chat

    async def run_scenario(self):
        await asyncio.sleep(float(self.scenario.get("start_delay", 3)))
        self.recording.write("out", "note", {"scenario": self.scenario.get("name"), "steps": len(self.scenario["steps"])})
        status = "ok"
        try:
            for step in self.scenario["steps"]:
                if "sleep" in step:
                    await asyncio.sleep(float(step["sleep"]))
                    continue
                if "wait_for_text" in step:
                    words = step["wait_for_text"]
                    seen = await self.recording.wait_until(lambda: any(words in t for t in self.recording.sent),
                                                           float(step.get("timeout", self.turn_timeout)))
                    self.recording.write("out", "note", {"wait_for_text": words, "seen": seen})
                    if not seen:
                        status = f"Hermes never sent a message containing {words!r}"
                        break
                    await asyncio.sleep(float(step.get("settle", 1)))
                    continue
                if "press" in step:
                    from tools.approval import resolve_gateway_approval_card
                    prompt = self.last_prompt
                    resolved = resolve_gateway_approval_card(prompt.session_key, step["press"], prompt.request_id, actor_id=step.get("user"))
                    self.recording.write("out", "press", {"choice": step["press"], "user": step.get("user"),
                                                          "request_id": prompt.request_id, "resolved": resolved})
                    continue
                if step.get("wait_turn"):
                    if not await self.wait_turn(float(step.get("settle", 4))):
                        status = "timed out waiting for a turn to finish"
                        break
                    continue
                if "wait_for" in step:
                    method = step["wait_for"]
                    seen = await self.recording.wait_for(method, step.get("after", 0), float(step.get("timeout", self.turn_timeout)))
                    self.recording.write("out", "note", {"wait_for": method, "seen": seen})
                    if not seen and step.get("required", True):
                        status = f"Hermes never called {method}"
                        break
                    await asyncio.sleep(float(step.get("settle", 1)))
                    continue
                self.n += 1
                message_id = step.get("message_id") or f"in-{self.n}"
                chat = (self.scenario.get("chats") or {}).get(step["chat"], {})
                source = self.build_source(
                    chat_id=step["chat"], chat_name=chat.get("name", step["chat"]),
                    chat_type=step.get("chat_type") or chat.get("type", "group"),
                    user_id=step["user"], user_name=step.get("user_name", step["user"]), message_id=message_id,
                )
                event = MessageEvent(
                    text=step["text"], message_type=MessageType.TEXT, source=source, raw_message=step,
                    message_id=message_id, timestamp=datetime.now(timezone.utc),
                )
                if step.get("reply_to"):
                    event.reply_to_message_id = step["reply_to"]
                self.recording.write("out", "inbound", {"message_id": message_id, **step})
                self.turn_done.clear()
                await self.handle_message(event)
                if step.get("wait", True):
                    if not await self.wait_turn(float(step.get("settle", 4))):
                        status = "timed out waiting for a turn to finish"
                        self.recording.write("out", "note", {"timeout": message_id})
                        break
        except Exception as exc:
            status = f"failed: {type(exc).__name__}: {exc}"
        self.recording.write("out", "note", {"finished": status})
        os.kill(os.getpid(), signal.SIGTERM)


class RecorderAdapterWithButtons(RecorderAdapter):
    """Claims native approval buttons. Hermes checks for this override, and then hands over a
    structured approval card where it would otherwise send the approval as text."""

    async def _send_exec_approval_prompt(self, prompt):
        self.last_prompt = prompt
        self.recording.write("in", "hook", {"method": "_send_exec_approval_prompt", "params": {"prompt": jsonable(prompt)}})
        self.n += 1
        result = SendResult(success=True, message_id=f"out-{self.n}")
        self.recording.write("out", "result", {"method": "_send_exec_approval_prompt", "result": jsonable(result)})
        return result


for _name in dir(BasePlatformAdapter):
    if _name.startswith("_") or not _name.startswith(RECORDED_PREFIXES) or _name in RecorderAdapter.__dict__:
        continue
    _attr = inspect.getattr_static(BasePlatformAdapter, _name)
    if inspect.isfunction(_attr):
        setattr(RecorderAdapter, _name, recorded(_name, _attr))


def register(ctx):
    """Hermes plugin entry point."""
    ctx.register_platform(
        name=PLATFORM,
        label="Recorder",
        adapter_factory=lambda config: (RecorderAdapterWithButtons if os.environ.get("RECORDER_APPROVAL_BUTTONS") == "1" else RecorderAdapter)(config),
        check_fn=lambda: True,
        is_connected=lambda config: True,
        validate_config=lambda config: True,
        allowed_users_env="RECORDER_ALLOWED_USERS",
        allow_all_env="RECORDER_ALLOW_ALL_USERS",
        max_message_length=0,
        platform_hint="You are chatting through a test platform. Chat ids are group names and user ids are user names.",
        emoji="",
    )
