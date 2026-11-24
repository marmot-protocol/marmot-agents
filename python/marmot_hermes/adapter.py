"""The Marmot platform for Hermes: the part of the plugin that runs inside Hermes.

Hermes sees a chat platform called `marmot`. The adapter connects to marmot-agent's local socket,
hands Hermes each prompt as a chat message, and records every call Hermes makes on the platform as a
line, exactly as `tools/record/hermes` records it, for the connector to translate. It sends nothing
anywhere else.

A file Hermes hands the platform to send (`send_document`, `send_image_file`, `send_video`,
`send_voice`, `send_multiple_images`: a `MEDIA:` path in the agent's answer, or a file another part of
Hermes delivers) becomes the core's `send_file`, one request for each file, with the path as Hermes gave
it. The core decides whether it may be sent, from the folders its settings allow (D-090, D-099), and
sends it to the chat as an encrypted attachment. A caption is sent first, as the agent's words; a batch keeps one caption, its
first, as today's connector does.

Who may prompt and who may approve is the core's to decide (D-010). The core lets a message through
only when its sender may send it, so Hermes is set up to take whatever reaches it: every user is
allowed, and anyone in a chat may answer its approvals (provisional, Q-22).

Environment:
    MARMOT_AGENT_HOME   where the core's socket and the plugin's token are: `socket/marmot-agent.sock`
                        and `connector.token` (the agent's home, or a folder holding just those two).
                        Without it, the platform's `home` setting says, `platforms.marmot.home` in
                        Hermes's `config.yaml`, as `install/hermes.sh` writes it
    HERMES_VERSION      Hermes's release, which Hermes does not report to a plugin (D-044)
    MARMOT_LOG          optional; a file the plugin appends its own log lines to, and nothing else does

It logs what went wrong by kind only: never a message, a prompt, a key or a path (D-071). Each line is
one of the fixed descriptions written in this plugin, at most with an event's item type. They go to
Hermes's log, and to `MARMOT_LOG` if it is set, which is the only log `python/hermes/run.sh` keeps.
"""

from __future__ import annotations

import asyncio
import dataclasses
import enum
import functools
import inspect
import logging
import os
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import unquote, urlsplit

from gateway.config import Platform
from gateway.platforms.base import BasePlatformAdapter, MessageEvent, MessageType, SendResult

from .connector import Connector
from .core import CoreConnection, HelloRefused, Refused, line_of
from .translate import VERSION, usable_settings

PLATFORM = "marmot"
PLUGIN_VERSION = "0.1.0"
RECONNECT_SECONDS = 2.0
log = logging.getLogger("marmot_hermes")

# Methods of the base adapter through which Hermes hands a platform something. Each is wrapped so that
# the call is recorded and then carried out by the base implementation. Typing calls are not recorded:
# they are four fifths of what Hermes calls, and never become events.
RECORDED_PREFIXES = (
    "send", "edit_message", "delete_message", "play_tts", "begin_streaming_tts", "write_streaming_tts",
    "finish_streaming_tts", "abort_streaming_tts", "emit_", "set_status_text", "on_processing_start",
    "format_tool_event", "format_tool_preview", "render_message_event", "create_handoff_thread",
)
NOT_RECORDED = {"send_typing", "send"}

# What Hermes can do, as the core and the apps read it (D-058).
CAPABILITIES = {
    "interrupt": True,
    # A message sent during a turn is folded into it: Hermes steers (D-040).
    "mid_turn": "fold",
    # Hermes keeps its own sessions, one for each chat and sender.
    "sessions": "own",
    "resume": True,
    "approvals": True,
    "files_in": "none",
    # Typed commands that answer an approval or change what the agent may do without asking (D-032,
    # D-047), as read in Hermes's source at the pinned commit (`docs/reference/hermes-messages.md`).
    "approval_commands": [{"command": c, "counts": "start"} for c in
                          ("/approve", "/deny", "/yolo", "/approvals", "/memory", "/skills", "/codex-runtime")],
}


def jsonable(v, depth=0):
    """Anything Hermes passes, as JSON: its structure kept, what cannot be kept named by its type."""
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


def diagnose(kind: str) -> None:
    """Appends one of the plugin's own log lines to `MARMOT_LOG`, when it is set."""
    path = os.environ.get("MARMOT_LOG")
    if not path:
        return
    try:
        with open(path, "a", encoding="utf-8") as f:
            f.write(f"{datetime.now(timezone.utc).isoformat(timespec='seconds')} marmot: {kind}\n")
    except OSError:
        pass


def local_file(url) -> str | None:
    """The path of a `file://` address, or None for any other."""
    parts = urlsplit(str(url))
    if parts.scheme != "file" or parts.netloc not in ("", "localhost"):
        return None
    return unquote(parts.path)


def chat_of(params: dict) -> str | None:
    """The chat a call is about: named directly, or by the message event or approval it carries."""
    for holder in (params, params.get("event"), params.get("prompt")):
        if isinstance(holder, dict):
            if holder.get("chat_id"):
                return str(holder["chat_id"])
            source = holder.get("source")
            if isinstance(source, dict) and source.get("chat_id"):
                return str(source["chat_id"])
    return None


def fit(event: dict) -> dict | None:
    """An event as it fits on one line of the socket (D-033): without its native message if it must,
    then with its text cut in the middle. The core applies the 64 KB limit (D-057)."""
    if line_of({"jsonrpc": "2.0", "method": "event", "params": event}) is not None:
        return event
    event = {k: v for k, v in event.items() if k != "native"}
    for _ in range(8):
        if line_of({"jsonrpc": "2.0", "method": "event", "params": event}) is not None:
            return event
        text = event.get("text") or ""
        if len(text) < 64:
            break
        keep = len(text) // 4
        event = {**event, "text": text[:keep] + "\n[… cut to fit …]\n" + text[-keep:]}
    return None


def recorded(name, base_fn):
    """Wraps a base-adapter method: record the call, run the base implementation, record the result."""
    signature = inspect.signature(base_fn)

    def describe(self, args, kwargs):
        try:
            bound = signature.bind(self, *args, **kwargs)
            params = {k: jsonable(v) for k, v in bound.arguments.items() if k != "self"}
        except TypeError:
            params = {"args": jsonable(args), "kwargs": jsonable(kwargs)}
        chat = chat_of(params)
        self.note({"dir": "in", "via": "hook", "msg": {"method": name, "params": params}}, chat)
        return chat

    if inspect.iscoroutinefunction(base_fn):
        @functools.wraps(base_fn)
        async def wrapper(self, *args, **kwargs):
            chat = describe(self, args, kwargs)
            result = await base_fn(self, *args, **kwargs)
            self.note({"dir": "out", "via": "result", "msg": {"method": name, "result": jsonable(result)}}, chat)
            return result
    else:
        @functools.wraps(base_fn)
        def wrapper(self, *args, **kwargs):
            chat = describe(self, args, kwargs)
            result = base_fn(self, *args, **kwargs)
            self.note({"dir": "out", "via": "result", "msg": {"method": name, "result": jsonable(result)}}, chat)
            return result
    return wrapper


class MarmotAdapter(BasePlatformAdapter):
    def __init__(self, config):
        super().__init__(config, Platform(PLATFORM))
        extra = getattr(config, "extra", None)
        setting = extra.get("home") if isinstance(extra, dict) else None
        self.home = Path(os.environ.get("MARMOT_AGENT_HOME") or (setting if isinstance(setting, str) else ""))
        hermes_home = Path(os.environ.get("HERMES_HOME") or Path.home() / ".hermes")
        self.connector = Connector(self, self.send_to_core, os.environ.get("HERMES_VERSION") or VERSION,
                                   generations=hermes_home / "marmot" / "sessions.json", warn=self.warn)
        self.core: CoreConnection | None = None
        self.loop: asyncio.AbstractEventLoop | None = None
        self.task = None
        self.reading: set[asyncio.Task] = set()  # reads of a session's model, kept until they end
        self.reads_n = 0  # names each read of a session's model, and its answer
        self.n = 0

    @staticmethod
    def warn(kind: str) -> None:
        log.warning("marmot: %s", kind)
        diagnose(kind)

    def note(self, line: dict, chat: str | None) -> None:
        """Hands a line to the connector, on the event loop."""
        loop = self.loop
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if loop is None or running is loop:
            self.connector.record(line, chat)
        else:
            loop.call_soon_threadsafe(self.connector.record, line, chat)

    def send_to_core(self, event: dict) -> None:
        core = self.core
        fitted = fit(event)
        if fitted is not event:
            self.warn(f"an event over the size limit was cut to fit ({event.get('type')})")
        if core is None or fitted is None or not core.notify("event", fitted):
            self.warn(f"an event could not be sent ({event.get('type')})")

    # -- the core's socket

    async def run_core(self) -> None:
        """Keeps a connection to the core, and connects again when it goes."""
        while True:
            try:
                token = (self.home / "connector.token").read_text(encoding="utf-8").strip()
                core = await CoreConnection.open(str(self.home / "socket" / "marmot-agent.sock"))
            except OSError:
                self.warn("the core's socket or token could not be opened")
                await asyncio.sleep(RECONNECT_SECONDS)
                continue
            try:
                self.core = core
                await core.hello({
                    "connector": {"name": "marmot-agent-hermes", "version": PLUGIN_VERSION},
                    "harness": {"name": "hermes", "version": self.connector.version},
                    "token": token,
                    "capabilities": CAPABILITIES,
                }, self.connector.serve)
                log.info("marmot: connected to the core")
                diagnose("connected to the core")
                await core.done()
                self.warn("the core closed the socket")
            except HelloRefused:
                self.warn("the core refused the hello")
            finally:
                core.close()
                self.core = None
                # That core closed what was open and settled what it handed over: the next starts
                # from what it knows.
                self.connector.core_gone()
            await asyncio.sleep(RECONNECT_SECONDS)

    # -- what a platform must implement

    async def connect(self, *, is_reconnect: bool = False) -> bool:
        self.loop = asyncio.get_running_loop()
        if self.connector.commands is None:
            self.read_commands()
        self._mark_connected()
        if self.task is None:
            self.task = asyncio.create_task(self.run_core())
        return True

    async def disconnect(self) -> None:
        self._mark_disconnected()
        if self.task is not None:
            self.task.cancel()
            self.task = None
        if self.core is not None:
            self.core.close()

    async def send(self, chat_id, content, reply_to=None, metadata=None, **kwargs):
        self.n += 1
        result = SendResult(success=True, message_id=f"out-{self.n}")
        self.note({"dir": "in", "via": "hook", "msg": {"method": "send", "params": {
            "chat_id": chat_id, "content": content, "reply_to": reply_to, "metadata": jsonable(metadata),
            **jsonable(kwargs)}}}, str(chat_id))
        self.note({"dir": "out", "via": "result", "msg": {"method": "send", "result": jsonable(result)}}, str(chat_id))
        return result

    async def get_chat_info(self, chat_id):
        return {"name": str(chat_id), "type": "group", "chat_id": chat_id}

    async def on_processing_complete(self, event, outcome):
        message_id = getattr(event, "message_id", None)
        self.note({"dir": "in", "via": "hook", "msg": {"method": "on_processing_complete", "params": {
            "message_id": message_id, "outcome": jsonable(outcome)}}}, self.connector.message_chat.get(str(message_id)))

    async def on_processing_start(self, event):
        params = {"event": jsonable(event)}
        chat = chat_of(params)
        self.note({"dir": "in", "via": "hook", "msg": {"method": "on_processing_start", "params": params}}, chat)
        source = getattr(event, "source", None)
        session = source is not None and self.connector.wants_model(
            jsonable(getattr(source, "chat_type", None)), getattr(source, "chat_id", None),
            getattr(source, "user_id", None))
        if session:
            # Hermes says which model a session uses only when asked. Ask once for each session.
            self.read_settings(source, session)

    async def _send_exec_approval_prompt(self, prompt):
        """Claims approval buttons, so that Hermes hands over an approval as a card, with an id for
        the request and its choices, instead of as text."""
        params = {"prompt": jsonable(prompt)}
        chat = chat_of(params)
        self.note({"dir": "in", "via": "hook", "msg": {"method": "_send_exec_approval_prompt", "params": params}}, chat)
        self.n += 1
        result = SendResult(success=True, message_id=f"out-{self.n}")
        self.note({"dir": "out", "via": "result", "msg": {"method": "_send_exec_approval_prompt",
                                                            "result": jsonable(result)}}, chat)
        return result

    # -- files the agent sends (D-090)

    async def send_document(self, chat_id, file_path, caption=None, file_name=None, reply_to=None, metadata=None,
                            **kwargs):
        return await self.send_files(chat_id, [file_path], caption, reply_to, metadata)

    async def send_image_file(self, chat_id, image_path, caption=None, reply_to=None, metadata=None, **kwargs):
        return await self.send_files(chat_id, [image_path], caption, reply_to, metadata)

    async def send_video(self, chat_id, video_path, caption=None, reply_to=None, metadata=None, **kwargs):
        return await self.send_files(chat_id, [video_path], caption, reply_to, metadata)

    async def send_voice(self, chat_id, audio_path, caption=None, reply_to=None, metadata=None, **kwargs):
        return await self.send_files(chat_id, [audio_path], caption, reply_to, metadata)

    async def send_multiple_images(self, chat_id, images, metadata=None, human_delay=0.0):
        """Local images go to the core; an image on the web is left to Hermes's own way, which sends its
        address as text. The batch keeps one caption: the first a local image has."""
        files, others = [], []
        for url, alt in images or []:
            path = local_file(url)
            if path is None:
                others.append((url, alt))
            else:
                files.append((path, alt))
        caption = next((alt for _, alt in files if str(alt or "").strip()), None)
        result = None
        if files:
            result = await self.send_files(chat_id, [p for p, _ in files], caption, None, metadata)
        delivered = result is not None and result.success
        for url, alt in others:
            other = await self.send_image(chat_id=chat_id, image_url=url, caption=alt or None, metadata=metadata)
            delivered = delivered or bool(getattr(other, "success", False))
        if not images:
            return SendResult(success=False, error="no images to send")
        if delivered:
            return SendResult(success=True, message_id=result.message_id if result and result.success else None)
        return result if result is not None else SendResult(success=False, error="all images failed to send")

    async def send_files(self, chat_id, paths, caption, reply_to, metadata) -> SendResult:
        """Asks the core to send each file, in turn, after the caption. Sent when at least one went, as
        Hermes counts a batch. A path goes as Hermes gave it: Hermes resolves each one it delivers to a
        whole path first (`validate_media_delivery_path`), and the core refuses one that is not whole.
        A refusal the core answered is shown in the chat by the core, naming no path, so Hermes is told
        not to add its own notice, which would name the file."""
        chat = str(chat_id)
        if caption is not None and str(caption).strip():
            await self.send(chat_id, str(caption), reply_to=reply_to, metadata=metadata)
        sent, refused = 0, None
        for path in paths:
            try:
                await self.ask_core("send_file", {"chat": chat, "path": str(path)})
                sent += 1
            except Refused as e:
                self.warn(f"a file was not sent ({e.reason})")
                refused = refused or e
        if sent:
            self.n += 1
            return SendResult(success=True, message_id=f"out-{self.n}")
        result = SendResult(success=False, error=(refused.detail if refused else None) or "The file was not sent.")
        if refused is not None and refused.answered:
            result.raw_response = {"failure_notice_sent": True}
        return result

    async def ask_core(self, method: str, params: dict) -> None:
        """A request to the core, from whichever event loop Hermes calls on: it runs on the connection's."""
        core = self.core
        if core is None or core.closed:
            raise Refused("failed", "The agent is not connected to its core.")
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is core.loop:
            return await core.request(method, params)
        return await asyncio.wrap_future(asyncio.run_coroutine_threadsafe(core.request(method, params), core.loop))

    # -- what the connector asks of Hermes

    async def inject(self, chat: dict, sender: dict, message_id: str, text: str, reply_to: dict | None) -> None:
        source = self.build_source(
            chat_id=str(chat.get("id")), chat_name=chat.get("name") or None,
            chat_type="group" if chat.get("group") else "dm",
            user_id=str(sender.get("id")), user_name=sender.get("name") or None, message_id=message_id)
        event = MessageEvent(text=text, message_type=MessageType.TEXT, source=source, raw_message=None,
                             message_id=message_id, timestamp=datetime.now(timezone.utc))
        if reply_to:
            author = reply_to.get("author") if isinstance(reply_to.get("author"), dict) else {}
            event.reply_to_message_id = reply_to.get("message")
            event.reply_to_text = reply_to.get("text")
            event.reply_to_author_id = author.get("id")
            event.reply_to_author_name = author.get("name")
        await self.handle_message(event)

    def resolve_card(self, session_key: str, choice: str, request_id: str, actor: str) -> int:
        from tools.approval import resolve_gateway_approval_card

        return int(resolve_gateway_approval_card(session_key, choice, request_id, actor_id=actor) or 0)

    def resolve_question(self, clarify_id: str, text: str) -> bool:
        """Answers a question the way Hermes asks a platform to: False when it no longer waits."""
        from tools.clarify_gateway import resolve_gateway_clarify

        return bool(resolve_gateway_clarify(clarify_id, text))

    async def resolve_confirm(self, session_key: str, confirm_id: str, choice: str) -> str | None:
        """Answers Hermes's request to confirm a command, the one named by `confirm_id` and no other, as
        Hermes's own button platforms do (`tools.slash_confirm`, D-146). None when it no longer waits:
        answered, replaced by another, or expired. Otherwise what Hermes says to the answer, which it hands
        back instead of sending; empty when it says nothing."""
        from tools import slash_confirm

        # As Hermes does with the next message: a confirmation older than it waits is dropped, not run.
        slash_confirm.clear_if_stale(session_key)
        waiting = slash_confirm.get_pending(session_key)
        if not waiting or str(waiting.get("confirm_id")) != confirm_id:
            return None
        said = await slash_confirm.resolve(session_key, confirm_id, choice)
        return "" if said is None else str(said)

    def phrase(self, key: str, **fields) -> str:
        """What Hermes says under `key` of its catalog, in the language it is set to, as its commands
        make their replies. It answers a command only in text, and this is how the plugin knows which."""
        from agent.i18n import t

        return t(key, **fields)

    def read_commands(self) -> None:
        """Reads Hermes's slash commands once, for the session items. Hermes's own platform adapters
        build their command menus from this registry, and a plugin runs in the same process."""
        try:
            from hermes_cli.commands import COMMAND_REGISTRY, _is_gateway_available

            commands = [{**jsonable(c), "gateway_available": bool(_is_gateway_available(c))} for c in COMMAND_REGISTRY]
        except Exception:
            return self.warn("Hermes's command list could not be read")
        self.connector.commands_read(commands)

    async def read_model_settings(self, chat: dict, sender: dict) -> None:
        source = self.build_source(chat_id=str(chat.get("id")), chat_name=chat.get("name") or None,
                                   chat_type="group" if chat.get("group") else "dm",
                                   user_id=str(sender.get("id")), user_name=sender.get("name") or None)
        await self.read_settings(source)

    def read_settings(self, source, session: str | None = None) -> asyncio.Task:
        """Asks Hermes for the model of the session of `source`, in the background. The request is
        recorded at once, under an id its answer carries too, and the translator applies the answer to
        the session Hermes names in it, if that session was not replaced, nor its model switched, since.
        `session` is the connector's name for it, if it chose to ask."""
        chat = str(source.chat_id)
        method = "get_session_model_settings"
        self.reads_n += 1
        read = f"read-{self.reads_n}"
        self.note({"dir": "out", "via": "call", "msg": {"method": method, "id": read,
                                                         "params": {"source": jsonable(source)}}}, chat)
        task = asyncio.ensure_future(self.settings_of(source, chat, session, read))
        self.reading.add(task)
        task.add_done_callback(self.reading.discard)
        return task

    async def settings_of(self, source, chat: str, session: str | None, read: str) -> None:
        method = "get_session_model_settings"
        try:
            result = await asyncio.to_thread(self.get_session_model_settings, source)
        except Exception as e:
            self.note({"dir": "in", "via": "call", "msg": {"method": method, "id": read,
                                                            "error": type(e).__name__}}, chat)
            if session:
                self.connector.model_not_read(session)
            return
        result = jsonable(result)
        if isinstance(result, dict):
            # The model's address is the owner's configuration, and the chat has no need of it.
            result.pop("base_url", None)
        self.note({"dir": "in", "via": "call", "msg": {"method": method, "id": read, "result": result}}, chat)
        if session and not usable_settings(result):
            # An answer with no model shows nothing: the next turn asks again, as after a failure.
            self.connector.model_not_read(session)


for _name in dir(BasePlatformAdapter):
    if _name.startswith("_") or not _name.startswith(RECORDED_PREFIXES) or _name in NOT_RECORDED \
            or _name in MarmotAdapter.__dict__:
        continue
    _attr = inspect.getattr_static(BasePlatformAdapter, _name)
    if inspect.isfunction(_attr):
        setattr(MarmotAdapter, _name, recorded(_name, _attr))


def register(ctx):
    """Registers the platform with Hermes."""
    ctx.register_platform(
        name=PLATFORM,
        label="Marmot",
        adapter_factory=lambda config: MarmotAdapter(config),
        check_fn=lambda: True,
        is_connected=lambda config: True,
        validate_config=lambda config: True,
        allowed_users_env="MARMOT_ALLOWED_USERS",
        allow_all_env="MARMOT_ALLOW_ALL_USERS",
        max_message_length=0,
        platform_hint="You are chatting through Marmot, a private messenger. Chat and user ids are opaque.",
        emoji="",
    )
