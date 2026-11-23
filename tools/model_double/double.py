#!/usr/bin/env python3
"""A model double: a local server that speaks the model providers' wire formats, so a real harness runs
against it with only the model faked (docs/design/verification.md, "A model double", and D-127).

Usage:
    double.py scripted <script.json>
    double.py replay   <cassette.jsonl>
    double.py record   <cassette.jsonl> --upstream URL --key-file FILE --model NAME --harness NAME --version V

It listens on 127.0.0.1 (`--port`, any free one by default) and prints its address, `http://127.0.0.1:<port>`,
as the first line of its output. It answers:

- Anthropic Messages, `POST …/v1/messages` (and `…/count_tokens`), as Claude Code sends them;
- OpenAI Responses, `POST …/responses`, as Codex sends them;
- OpenAI Chat Completions, `POST …/chat/completions`, as OpenCode, Goose, Pi, Hermes and OpenClaw send them;
- `GET …/models`, a list with one model, `scripted`;
- `GET /_double/log`, what it was asked, as keys (below), and whether each was answered (with each request's
  whole body too, given `--keep-bodies`). Every POST is in it: a token count as `count_tokens`, and one to a
  path it does not serve as `other`, answered 404 and marked unmatched.

What it prints, on its error output, is never anything a request holds: no prompt, path, host name or key.
An unanswered request is printed by its format, `kind`, `after` and `last`, and why it was not answered;
with `--verbose`, an answered one the same way. Only the log holds a prompt, and a body.

Every request is read into one neutral form, whatever its format: the conversation as user, assistant and
tool messages, and whether tools were offered. A reply is neutral too, a list of parts (`{"text": …}`,
`{"thinking": …}`, `{"call": name, "input": {…}}`), and is written out in the format that was asked.

What identifies a request, its key, is loose on purpose, so a new harness version still matches:

- `kind`: `main` when the request offers tools, `side` when it offers none (a title, a summary, a check);
- `prompt`: the text of the last user messages, those since the model last spoke or a tool's results came,
  with the blocks a harness wraps around a person's words (`<system-reminder>…</…>`, `<environment_context>…</…>`
  and any other tagged block) removed and spaces collapsed;
- `after`: how many model replies the conversation already holds since that prompt;
- `last`: what the model is answering: `prompt`, or `results:` and the names of the tools whose results came.

The system prompt, the tool definitions, ids, models, headers and every other field are not part of it.

Modes:

- scripted: the script says what the model does. `{"prompts": [{"when": "<words in the prompt>",
  "replies": [[part, …], …]}], "side": [part, …]}`: the first entry whose `when` is in the prompt answers, its
  reply number `after` (or its last, for any later one, when the entry has `"repeat": true`); a side request
  gets `side` (`ok` if none).
- replay: a cassette answers. A request matches the exchange with the same key, all four parts; if none has
  it, it is refused with status 400 and logged as unmatched, with what differs from the nearest exchange.
- record: each request is sent on to a real model at `--upstream`, an OpenAI-compatible Chat Completions
  endpoint (a request in another format is translated to it), not streamed, and the reply is kept as a
  cassette. The key is read from `--key-file`; run it in a container that has that file mounted read-only.

A cassette is JSON lines: a first line `{"cassette": 1, "harness", "version", "recorded", "format"}`, then
one `{"key": {…}, "reply": [part, …]}` per exchange. It holds no header, no key and no request body.
It holds one reply for each key: recording refuses a second, different one (502, logged as an error), and
replay refuses to start on a cassette that has one.

It uses the standard library only.
"""

from __future__ import annotations

import argparse
import datetime
import http.server
import json
import re
import sys
import threading
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

Json = dict[str, Any]

# ---------------------------------------------------------------------------------------------------------
# Reading a request into the neutral form
# ---------------------------------------------------------------------------------------------------------

TAGGED = re.compile(r"<([A-Za-z_][\w-]*)\b[^>]*>.*?</\1\s*>", re.DOTALL)


def prompt_text(text: str) -> str:
    """A person's words, without the tagged blocks a harness puts around them, with spaces collapsed."""
    return " ".join(TAGGED.sub(" ", text).split())


def _texts(content: Any, kinds: tuple[str, ...] = ("text", "input_text", "output_text")) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(p.get("text", "") for p in content if isinstance(p, dict) and p.get("type") in kinds)
    return ""


class Msg:
    """One message of the neutral conversation: `user` (a person's text), `assistant` (a model reply,
    with the names of the tools it called), or `tool` (the results of tools, by name)."""

    def __init__(self, role: str, text: str = "", names: list[str] | None = None) -> None:
        self.role = role
        self.text = text
        self.names = names or []


def _push(messages: list[Msg], role: str, text: str = "", names: list[str] | None = None) -> None:
    """Adds to the conversation, merging into the last message when it has the same role and is not a user's:
    a model reply split over several items, or several tool results, is one message."""
    if messages and messages[-1].role == role and role != "user":
        messages[-1].names += names or []
        messages[-1].text += text
    else:
        messages.append(Msg(role, text, names))


def read_anthropic(body: Json) -> tuple[list[Msg], bool]:
    messages: list[Msg] = []
    calls: dict[str, str] = {}
    for m in body.get("messages", []):
        content = m.get("content")
        if m.get("role") == "assistant":
            blocks = content if isinstance(content, list) else [{"type": "text", "text": content or ""}]
            names = []
            for b in blocks:
                if b.get("type") == "tool_use":
                    calls[b.get("id", "")] = b.get("name", "")
                    names.append(b.get("name", ""))
            _push(messages, "assistant", _texts(blocks), names)
            continue
        if m.get("role") != "user":
            continue  # Claude Code puts its environment in a message with the role "system"
        blocks = content if isinstance(content, list) else [{"type": "text", "text": content or ""}]
        results = [calls.get(b.get("tool_use_id", ""), "?") for b in blocks if b.get("type") == "tool_result"]
        text = _texts(blocks)
        if results:
            _push(messages, "tool", "", results)
            # Words a person sent beside the results are a prompt of their own; a reminder alone is not.
            if not prompt_text(text):
                continue
        _push(messages, "user", text)
    return messages, bool(body.get("tools"))


def read_responses(body: Json) -> tuple[list[Msg], bool]:
    messages: list[Msg] = []
    calls: dict[str, str] = {}
    items = body.get("input", [])
    if isinstance(items, str):
        items = [{"type": "message", "role": "user", "content": items}]
    for it in items:
        kind = it.get("type", "message")
        if kind == "message":
            role = it.get("role")
            if role == "user":
                _push(messages, "user", _texts(it.get("content")))
            elif role == "assistant":
                _push(messages, "assistant", _texts(it.get("content")))
            # developer and system messages are instructions, not the conversation.
        elif kind == "reasoning":
            _push(messages, "assistant")
        elif kind in ("function_call", "custom_tool_call", "local_shell_call"):
            name = it.get("name", kind)
            calls[it.get("call_id", "")] = name
            _push(messages, "assistant", "", [name])
        elif kind.endswith("_call_output"):
            _push(messages, "tool", "", [calls.get(it.get("call_id", ""), "?")])
    return messages, bool(body.get("tools"))


def read_chat(body: Json) -> tuple[list[Msg], bool]:
    messages: list[Msg] = []
    calls: dict[str, str] = {}
    for m in body.get("messages", []):
        role = m.get("role")
        if role == "user":
            _push(messages, "user", _texts(m.get("content")))
        elif role == "assistant":
            names = []
            for c in m.get("tool_calls") or []:
                name = c.get("function", {}).get("name", "")
                calls[c.get("id", "")] = name
                names.append(name)
            _push(messages, "assistant", _texts(m.get("content")), names)
        elif role == "tool":
            _push(messages, "tool", "", [calls.get(m.get("tool_call_id", ""), "?")])
    return messages, bool(body.get("tools"))


READERS = {"anthropic": read_anthropic, "responses": read_responses, "chat": read_chat}


def key_of(fmt: str, body: Json) -> Json:
    """What identifies a request (see the module's notes)."""
    messages, tools = READERS[fmt](body)
    at = None
    for i in range(len(messages) - 1, -1, -1):
        if messages[i].role == "user" and prompt_text(messages[i].text):
            at = i
            break
    prompt = ""
    if at is not None:
        # The person's words and what a harness puts beside them, before or after, in messages of their own.
        first = at
        while first > 0 and messages[first - 1].role == "user":
            first -= 1
        prompt = " ".join(t for t in (prompt_text(m.text) for m in messages[first : at + 1]) if t)
    later = messages[at + 1 :] if at is not None else messages
    after = sum(1 for m in later if m.role == "assistant")
    last = "prompt"
    if messages and messages[-1].role == "tool":
        last = "results:" + ",".join(sorted(messages[-1].names))
    elif messages and messages[-1].role == "assistant":
        last = "assistant"
    return {"kind": "main" if tools else "side", "prompt": prompt, "after": after, "last": last}


# ---------------------------------------------------------------------------------------------------------
# Writing a neutral reply in each format
# ---------------------------------------------------------------------------------------------------------

USAGE_IN, USAGE_OUT = 10, 5


class Ids:
    def __init__(self) -> None:
        self.n = 0
        self.lock = threading.Lock()

    def next(self, prefix: str) -> str:
        with self.lock:
            self.n += 1
            return f"{prefix}{self.n:04d}"


IDS = Ids()


def sse(event: str | None, data: Json | str) -> str:
    text = data if isinstance(data, str) else json.dumps(data)
    return (f"event: {event}\n" if event else "") + f"data: {text}\n\n"


def anthropic_reply(parts: list[Json], body: Json) -> tuple[str, str]:
    mid = IDS.next("msg_double_")
    blocks = []
    for p in parts:
        if "thinking" in p:
            blocks.append({"type": "thinking", "thinking": p["thinking"], "signature": "double"})
        elif "text" in p:
            blocks.append({"type": "text", "text": p["text"]})
        elif "call" in p:
            blocks.append({"type": "tool_use", "id": IDS.next("toolu_double_"), "name": p["call"],
                           "input": p.get("input", {})})
    stop = "tool_use" if any(b["type"] == "tool_use" for b in blocks) else "end_turn"
    usage = {"input_tokens": USAGE_IN, "output_tokens": USAGE_OUT}
    message = {"id": mid, "type": "message", "role": "assistant", "model": body.get("model", "scripted"),
               "content": blocks, "stop_reason": stop, "stop_sequence": None, "usage": usage}
    if not body.get("stream"):
        return "application/json", json.dumps(message)
    out = sse("message_start", {"type": "message_start", "message": {**message, "content": [],
                                                                     "stop_reason": None}})
    for i, b in enumerate(blocks):
        if b["type"] == "thinking":
            start, deltas = {"type": "thinking", "thinking": "", "signature": ""}, [
                {"type": "thinking_delta", "thinking": b["thinking"]},
                {"type": "signature_delta", "signature": b["signature"]}]
        elif b["type"] == "text":
            start, deltas = {"type": "text", "text": ""}, [{"type": "text_delta", "text": b["text"]}]
        else:
            start = {"type": "tool_use", "id": b["id"], "name": b["name"], "input": {}}
            deltas = [{"type": "input_json_delta", "partial_json": json.dumps(b["input"])}]
        out += sse("content_block_start", {"type": "content_block_start", "index": i, "content_block": start})
        for d in deltas:
            out += sse("content_block_delta", {"type": "content_block_delta", "index": i, "delta": d})
        out += sse("content_block_stop", {"type": "content_block_stop", "index": i})
    out += sse("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": None},
                                 "usage": {"output_tokens": USAGE_OUT}})
    out += sse("message_stop", {"type": "message_stop"})
    return "text/event-stream", out


def _custom_tools(body: Json) -> set[str]:
    return {t.get("name", "") for t in body.get("tools", []) if t.get("type") == "custom"}


def responses_reply(parts: list[Json], body: Json) -> tuple[str, str]:
    rid = IDS.next("resp_double_")
    custom = _custom_tools(body)
    items = []
    for p in parts:
        if "thinking" in p:
            items.append({"type": "reasoning", "id": IDS.next("rs_double_"),
                          "summary": [{"type": "summary_text", "text": p["thinking"]}]})
        elif "text" in p:
            items.append({"type": "message", "id": IDS.next("msg_double_"), "role": "assistant",
                          "status": "completed",
                          "content": [{"type": "output_text", "text": p["text"], "annotations": []}]})
        elif "call" in p and p["call"] in custom:
            items.append({"type": "custom_tool_call", "id": IDS.next("ctc_double_"), "status": "completed",
                          "call_id": IDS.next("call_double_"), "name": p["call"],
                          "input": p.get("input", {}).get("input", "")})
        elif "call" in p:
            items.append({"type": "function_call", "id": IDS.next("fc_double_"), "status": "completed",
                          "call_id": IDS.next("call_double_"), "name": p["call"],
                          "arguments": json.dumps(p.get("input", {}))})
    usage = {"input_tokens": USAGE_IN, "input_tokens_details": {"cached_tokens": 0},
             "output_tokens": USAGE_OUT, "output_tokens_details": {"reasoning_tokens": 0},
             "total_tokens": USAGE_IN + USAGE_OUT}
    response = {"id": rid, "object": "response", "created_at": 0, "status": "completed",
                "model": body.get("model", "scripted"), "output": items, "usage": usage}
    if not body.get("stream"):
        return "application/json", json.dumps(response)
    seq = iter(range(1_000_000))
    out = sse("response.created", {"type": "response.created", "sequence_number": next(seq),
                                   "response": {**response, "status": "in_progress", "output": []}})
    for i, it in enumerate(items):
        added = {**it, "status": "in_progress"}
        if it["type"] == "message":
            added["content"] = []
        out += sse("response.output_item.added", {"type": "response.output_item.added", "output_index": i,
                                                  "sequence_number": next(seq), "item": added})
        if it["type"] == "message":
            text = it["content"][0]["text"]
            out += sse("response.output_text.delta", {"type": "response.output_text.delta", "item_id": it["id"],
                                                      "output_index": i, "content_index": 0, "delta": text,
                                                      "sequence_number": next(seq)})
        elif it["type"] == "reasoning":
            text = it["summary"][0]["text"]
            out += sse("response.reasoning_summary_text.delta", {
                "type": "response.reasoning_summary_text.delta", "item_id": it["id"], "output_index": i,
                "summary_index": 0, "delta": text, "sequence_number": next(seq)})
        out += sse("response.output_item.done", {"type": "response.output_item.done", "output_index": i,
                                                 "sequence_number": next(seq), "item": it})
    out += sse("response.completed", {"type": "response.completed", "sequence_number": next(seq),
                                      "response": response})
    return "text/event-stream", out


def chat_reply(parts: list[Json], body: Json) -> tuple[str, str]:
    cid = IDS.next("chatcmpl-double-")
    text = "".join(p["text"] for p in parts if "text" in p)
    thinking = "".join(p["thinking"] for p in parts if "thinking" in p)
    calls = [{"id": IDS.next("call_double_"), "type": "function",
              "function": {"name": p["call"], "arguments": json.dumps(p.get("input", {}))}}
             for p in parts if "call" in p]
    finish = "tool_calls" if calls else "stop"
    usage = {"prompt_tokens": USAGE_IN, "completion_tokens": USAGE_OUT, "total_tokens": USAGE_IN + USAGE_OUT}
    model = body.get("model", "scripted")
    if not body.get("stream"):
        message: Json = {"role": "assistant", "content": text or None}
        if thinking:
            message["reasoning_content"] = thinking
        if calls:
            message["tool_calls"] = calls
        return "application/json", json.dumps({
            "id": cid, "object": "chat.completion", "created": 0, "model": model,
            "choices": [{"index": 0, "message": message, "finish_reason": finish}], "usage": usage})

    def chunk(delta: Json, finish_reason: str | None = None) -> str:
        return sse(None, {"id": cid, "object": "chat.completion.chunk", "created": 0, "model": model,
                          "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]})

    out = chunk({"role": "assistant", "content": ""})
    if thinking:
        out += chunk({"reasoning_content": thinking})
    if text:
        out += chunk({"content": text})
    for i, c in enumerate(calls):
        out += chunk({"tool_calls": [{"index": i, **c}]})
    out += chunk({}, finish)
    out += sse(None, {"id": cid, "object": "chat.completion.chunk", "created": 0, "model": model,
                      "choices": [], "usage": usage})
    out += sse(None, "[DONE]")
    return "text/event-stream", out


WRITERS = {"anthropic": anthropic_reply, "responses": responses_reply, "chat": chat_reply}

# ---------------------------------------------------------------------------------------------------------
# Record: translating a request to Chat Completions, and reading the reply back
# ---------------------------------------------------------------------------------------------------------


def _anthropic_to_chat(body: Json) -> Json:
    out: list[Json] = []
    system = body.get("system")
    if system:
        out.append({"role": "system", "content": _texts(system)})
    for m in body.get("messages", []):
        content = m.get("content")
        blocks = content if isinstance(content, list) else [{"type": "text", "text": content or ""}]
        if m.get("role") == "system":
            out.append({"role": "system", "content": _texts(blocks)})
            continue
        if m.get("role") == "assistant":
            calls = [{"id": b["id"], "type": "function",
                      "function": {"name": b["name"], "arguments": json.dumps(b.get("input", {}))}}
                     for b in blocks if b.get("type") == "tool_use"]
            msg: Json = {"role": "assistant", "content": _texts(blocks) or None}
            if calls:
                msg["tool_calls"] = calls
            out.append(msg)
            continue
        for b in blocks:
            if b.get("type") == "tool_result":
                out.append({"role": "tool", "tool_call_id": b.get("tool_use_id", ""),
                            "content": _texts(b.get("content")) or "(no output)"})
        text = _texts(blocks)
        if text:
            out.append({"role": "user", "content": text})
    tools = [{"type": "function", "function": {"name": t["name"], "description": t.get("description", ""),
                                               "parameters": t.get("input_schema", {"type": "object"})}}
             for t in body.get("tools", []) if "name" in t]
    chat: Json = {"messages": out, "max_tokens": min(int(body.get("max_tokens", 4096)), 8192)}
    if tools:
        chat["tools"] = tools
    return chat


def _responses_to_chat(body: Json) -> Json:
    out: list[Json] = []
    if body.get("instructions"):
        out.append({"role": "system", "content": body["instructions"]})
    items = body.get("input", [])
    if isinstance(items, str):
        items = [{"type": "message", "role": "user", "content": items}]
    for it in items:
        kind = it.get("type", "message")
        if kind == "message":
            role = {"developer": "system"}.get(it.get("role"), it.get("role"))
            out.append({"role": role, "content": _texts(it.get("content"))})
        elif kind in ("function_call", "custom_tool_call"):
            args = it.get("arguments") if kind == "function_call" else json.dumps({"input": it.get("input", "")})
            call = {"id": it.get("call_id", ""), "type": "function",
                    "function": {"name": it.get("name", ""), "arguments": args or "{}"}}
            if out and out[-1]["role"] == "assistant":
                out[-1].setdefault("tool_calls", []).append(call)
            else:
                out.append({"role": "assistant", "content": None, "tool_calls": [call]})
        elif kind.endswith("_call_output"):
            output = it.get("output", "")
            out.append({"role": "tool", "tool_call_id": it.get("call_id", ""),
                        "content": output if isinstance(output, str) else _texts(output)})
    tools = []
    for t in body.get("tools", []):
        if t.get("type") == "function":
            tools.append({"type": "function", "function": {"name": t["name"],
                                                           "description": t.get("description", ""),
                                                           "parameters": t.get("parameters", {"type": "object"})}})
        elif t.get("type") == "custom":
            tools.append({"type": "function", "function": {
                "name": t["name"], "description": t.get("description", ""),
                "parameters": {"type": "object", "properties": {"input": {"type": "string"}},
                               "required": ["input"]}}})
    chat: Json = {"messages": out}
    if tools:
        chat["tools"] = tools
    return chat


def to_chat(fmt: str, body: Json) -> Json:
    if fmt == "anthropic":
        return _anthropic_to_chat(body)
    if fmt == "responses":
        return _responses_to_chat(body)
    return {k: v for k, v in body.items() if k not in ("stream", "stream_options")}


def parts_of_chat(reply: Json) -> list[Json]:
    message = (reply.get("choices") or [{}])[0].get("message") or {}
    parts: list[Json] = []
    thinking = message.get("reasoning_content") or message.get("reasoning")
    if thinking:
        parts.append({"thinking": thinking})
    if message.get("content"):
        parts.append({"text": message["content"]})
    for c in message.get("tool_calls") or []:
        try:
            args = json.loads(c["function"].get("arguments") or "{}")
        except json.JSONDecodeError:
            args = {"input": c["function"].get("arguments")}
        parts.append({"call": c["function"]["name"], "input": args})
    return parts


# ---------------------------------------------------------------------------------------------------------
# The modes
# ---------------------------------------------------------------------------------------------------------


class Unmatched(Exception):
    pass


class Conflict(Exception):
    """A second, different reply for a key a cassette already holds: a replay could serve only the first."""


def _key_text(key: Json) -> str:
    return json.dumps(key, sort_keys=True)


class Cassette:
    def __init__(self, path: Path, header: Json) -> None:
        self.path = path
        self.header = header
        self.lock = threading.Lock()
        self.replies: dict[str, list[Json]] = {}  # what is written, by key

    def keep(self, key: Json, parts: list[Json]) -> None:
        line = json.dumps({"key": key, "reply": parts}, ensure_ascii=False)
        with self.lock:
            kept = self.replies.get(_key_text(key))
            if kept == parts:
                return
            if kept is not None:
                raise Conflict("the cassette already holds another reply for this request")
            # The whole file again when nothing is written yet, so a failed first write leaves no half of one.
            fresh = not self.replies
            with self.path.open("w" if fresh else "a", encoding="utf-8") as f:
                f.write((json.dumps(self.header) + "\n" if fresh else "") + line + "\n")
            self.replies[_key_text(key)] = parts


def load_cassette(path: Path) -> tuple[Json, list[Json]]:
    # Split on newlines alone: a reply may hold U+2028 and the like, which `splitlines` would split on.
    text = path.read_text(encoding="utf-8")
    lines = [json.loads(line) for line in text.split("\n") if line.strip()]
    return lines[0], lines[1:]


class Scripted:
    def __init__(self, script: Json) -> None:
        self.script = script

    def answer(self, fmt: str, key: Json, body: Json) -> list[Json]:
        if key["kind"] == "side":
            return self.script.get("side") or [{"text": "ok"}]
        for entry in self.script.get("prompts", []):
            if entry["when"] in key["prompt"]:
                replies = entry["replies"]
                if key["after"] < len(replies):
                    return replies[key["after"]]
                if entry.get("repeat"):
                    return replies[-1]
                raise Unmatched(f"entry {self.script['prompts'].index(entry)} of the script has "
                                f"{len(replies)} replies")
        raise Unmatched("no entry of the script is in the prompt")


class Replay:
    def __init__(self, exchanges: list[Json]) -> None:
        # Two replies for one key: only the first could be served, and the second would pass as used.
        first: dict[str, int] = {}
        for i, e in enumerate(exchanges):
            j = first.setdefault(_key_text(e["key"]), i)
            if exchanges[j]["reply"] != e["reply"]:
                raise ValueError(f"the cassette's exchanges {j + 1} and {i + 1} answer one request differently")
        self.exchanges = exchanges

    def answer(self, fmt: str, key: Json, body: Json) -> list[Json]:
        for e in self.exchanges:
            if e["key"] == key:
                return e["reply"]
        # Only to say why: the nearest exchanges, those that differ from it in `last` alone.
        near = [e["key"]["last"] for e in self.exchanges
                if all(e["key"][k] == key[k] for k in ("kind", "prompt", "after"))]
        if near:
            raise Unmatched(f"the cassette answers {' or '.join(map(repr, near))} here, "
                            f"the harness sent {key['last']!r}")
        raise Unmatched("the cassette has no exchange for it")


class Record:
    def __init__(self, upstream: str, key_file: Path, model: str) -> None:
        self.upstream = upstream.rstrip("/") + "/chat/completions"
        self.key = key_file.read_text().strip()
        self.model = model

    def answer(self, fmt: str, key: Json, body: Json) -> list[Json]:
        chat = to_chat(fmt, body)
        chat["model"] = self.model
        chat["stream"] = False
        request = urllib.request.Request(self.upstream, data=json.dumps(chat).encode(), method="POST",
                                         headers={"Authorization": f"Bearer {self.key}",
                                                  "Content-Type": "application/json",
                                                  "User-Agent": "model-double"})
        with urllib.request.urlopen(request, timeout=300) as r:
            return parts_of_chat(json.loads(r.read()))


def fmt_of(path: str) -> str | None:
    path = path.split("?", 1)[0].rstrip("/")
    if path.endswith("/messages"):
        return "anthropic"
    if path.endswith("/responses"):
        return "responses"
    if path.endswith("/chat/completions"):
        return "chat"
    return None


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, port: int, mode: Any, save: Cassette | None = None) -> None:
        super().__init__(("127.0.0.1", port), Handler)
        self.keep_bodies = False
        self.verbose = False
        self.mode = mode
        self.save = save
        self.log: list[Json] = []
        self.lock = threading.Lock()


class Handler(http.server.BaseHTTPRequestHandler):
    server: Server
    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002 - the base class's name
        pass

    def send(self, status: int, kind: str, text: str) -> None:
        data = text.encode()
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("request-id", "req_double")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802 - the base class's name
        path = self.path.split("?", 1)[0].rstrip("/")
        if path == "/_double/log":
            with self.server.lock:
                return self.send(200, "application/json", json.dumps(self.server.log))
        if path.endswith("/models"):
            listed = {"object": "list", "data": [{"id": "scripted", "object": "model", "owned_by": "double",
                                                  "created": 0}]}
            return self.send(200, "application/json", json.dumps(listed))
        self.send(404, "application/json", json.dumps({"error": {"message": "not served by the double"}}))

    def keep(self, entry: Json, raw: bytes) -> None:
        if self.server.keep_bodies:
            entry["body"] = raw.decode("utf-8", "replace")
        with self.server.lock:
            self.server.log.append(entry)

    def do_POST(self) -> None:  # noqa: N802 - the base class's name
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length)
        path = self.path.split("?", 1)[0].rstrip("/")
        if path.endswith("/count_tokens"):
            self.keep({"format": "count_tokens"}, raw)
            return self.send(200, "application/json", json.dumps({"input_tokens": USAGE_IN}))
        fmt = fmt_of(path)
        if fmt is None:
            # Kept, so a test that reads the bodies sees what a harness sent any other way, and unanswered.
            self.keep({"format": "other", "unmatched": "not a path the double serves"}, raw)
            print("model double: unmatched request to a path it does not serve", file=sys.stderr, flush=True)
            return self.send(404, "application/json", json.dumps({"error": {"message": "not served"}}))
        body = json.loads(raw or b"{}")
        key = key_of(fmt, body)
        entry = {"format": fmt, **key}
        # What is printed names the request by its shape only, never its prompt or anything else it holds.
        shape = f"{fmt} {key['kind']} request, after {key['after']}, answering {key['last']}"
        try:
            parts = self.server.mode.answer(fmt, key, body)
            if self.server.save is not None:
                self.server.save.keep(key, parts)
        except Unmatched as e:
            entry["unmatched"] = str(e)
            self.keep(entry, raw)
            print(f"model double: unmatched {shape}: {e}", file=sys.stderr, flush=True)
            # A 4xx: a harness retries a 5xx, Claude Code ten times.
            return self.send(400, "application/json",
                             json.dumps({"type": "error", "error": {"type": "double_unmatched", "message": str(e)}}))
        except Conflict as e:
            entry["error"] = str(e)
            self.keep(entry, raw)
            print(f"model double: not recorded, {shape}: {e}", file=sys.stderr, flush=True)
            return self.send(502, "application/json", json.dumps({"error": {"message": "not recorded"}}))
        except Exception as e:  # an upstream failure, or a failed write, while recording
            entry["error"] = type(e).__name__
            self.keep(entry, raw)
            # The kind and the status only: an upstream's message can name its address.
            status = f" {e.code}" if isinstance(e, urllib.error.HTTPError) else ""
            print(f"model double: upstream failed for {shape}: {type(e).__name__}{status}",
                  file=sys.stderr, flush=True)
            return self.send(502, "application/json", json.dumps({"error": {"message": "upstream failed"}}))
        self.keep(entry, raw)
        if self.server.verbose:
            print(f"model double: answered {shape}", file=sys.stderr, flush=True)
        kind, text = WRITERS[fmt](parts, body)
        self.send(200, kind, text)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("mode", choices=["scripted", "replay", "record"])
    parser.add_argument("file", type=Path, help="the script (scripted) or the cassette (replay, record)")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--harness", default="", help="record: the harness, for the cassette")
    parser.add_argument("--version", default="", help="record: the harness's version, for the cassette")
    parser.add_argument("--format", default="", help="record: the wire format the harness speaks")
    parser.add_argument("--upstream")
    parser.add_argument("--key-file", type=Path)
    parser.add_argument("--model")
    parser.add_argument("--verbose", action="store_true",
                        help="print each request's shape (format, kind, after, last) as it is answered")
    parser.add_argument("--keep-bodies", action="store_true", help="put each request's whole body in the log")
    args = parser.parse_args()

    header = {"cassette": 1, "harness": args.harness, "version": args.version,
              "recorded": datetime.date.today().isoformat(), "format": args.format}
    save = None
    if args.mode == "scripted":
        mode: Any = Scripted(json.loads(args.file.read_text()))
    elif args.mode == "replay":
        _, exchanges = load_cassette(args.file)
        try:
            mode = Replay(exchanges)
        except ValueError as e:
            parser.error(str(e))
    else:
        if not (args.upstream and args.key_file and args.model):
            parser.error("record needs --upstream, --key-file and --model")
        mode = Record(args.upstream, args.key_file, args.model)
        save = Cassette(args.file, header)
    server = Server(args.port, mode, save)
    server.keep_bodies = args.keep_bodies
    server.verbose = args.verbose
    print(f"http://127.0.0.1:{server.server_address[1]}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
