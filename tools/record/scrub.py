#!/usr/bin/env python3
"""Scrub raw recordings so they can be committed.

Usage:
    scrub.py <raw-dir> <out-dir> --map <local-path>=<replacement> [--map ...]

`<raw-dir>` holds one folder per harness (`codex`, `claude`, `hermes`, `openclaw`), each with `*.jsonl`
files as written by the recorders, and `acp`, which holds one such folder per ACP agent, such as
`acp/opencode`. The same tree is written to `<out-dir>`. The script then scans its own output and exits with an error
if anything that looks private is left.

What it does:

- Replaces local paths, and the dash-encoded form of each that Claude Code uses for folder
  names. Give the most specific first, for example
  `--map /tmp/rec/demo=/work/demo --map /home/alice=/home/user`.
- Replaces the machine's host name and the current user name.
- Blanks account and machine details: plan, rate limits, installation id, user agent,
  moderation metadata.
- Empties the list of connected MCP servers and removes MCP tool names.
- Replaces thinking signatures and other opaque blobs with a short marker.

It has no dependencies.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import socket
import sys
from pathlib import Path

# Messages whose values describe the account or the machine. Their structure is kept and
# every value is neutralised: strings become "redacted", numbers 0.
NEUTRALISE_CODEX = {
    "account/updated",
    "account/rateLimits/updated",
    "remoteControl/status/changed",
    "turn/moderationMetadata",
}
NEUTRALISE_CLAUDE_TYPES = {"rate_limit_event"}
BLOB_KEYS = {"signature", "encrypted_content", "encryptedContent"}


def neutralise(v):
    if isinstance(v, dict):
        return {k: neutralise(x) for k, x in v.items()}
    if isinstance(v, list):
        return [neutralise(x) for x in v]
    if isinstance(v, bool) or v is None:
        return v
    if isinstance(v, (int, float)):
        return 0
    return "redacted"


class Scrubber:
    def __init__(self, maps: list[tuple[str, str]]):
        # Claude Code names per-project folders after the working directory with every
        # non-alphanumeric character turned into "-". Replace that form of each path too.
        encoded = [(re.sub(r"[^A-Za-z0-9]", "-", old), re.sub(r"[^A-Za-z0-9]", "-", new)) for old, new in maps]
        self.maps = maps + encoded
        host = socket.gethostname()
        user = os.environ.get("USER") or os.environ.get("LOGNAME") or ""
        self.words = [(w, r) for w, r in ((host, "host"), (user, "user")) if len(w) >= 3]

    def text(self, s: str) -> str:
        for old, new in self.maps:
            s = s.replace(old, new)
        for word, new in self.words:
            s = re.sub(rf"(?<![A-Za-z0-9]){re.escape(word)}(?![A-Za-z0-9])", new, s)
        return s

    def walk(self, v, key=None):
        if isinstance(v, dict):
            return {k: self.walk(x, k) for k, x in v.items()}
        if isinstance(v, list):
            return [self.walk(x, key) for x in v]
        if isinstance(v, str):
            if key in BLOB_KEYS and len(v) > 24:
                return "removed"
            if key in ("userAgent", "credential_expires_at"):
                return "redacted"
            return self.text(v)
        return v

    def codex(self, msg: dict) -> dict:
        if msg.get("method") in NEUTRALISE_CODEX and isinstance(msg.get("params"), dict):
            msg = {**msg, "params": neutralise(msg["params"])}
        return self.walk(msg)

    def acp(self, msg: dict) -> dict:
        # The ACP recorder runs the agent with a throwaway home folder and no account of ours, so only
        # paths and names need replacing.
        return self.walk(msg)

    def hermes(self, msg: dict) -> dict:
        # The Hermes container holds no account of ours, so only paths and names need replacing.
        return self.walk(msg)

    def openclaw(self, msg: dict) -> dict:
        # The OpenClaw container holds no account of ours. Its status command reports how long
        # the machine has been up, which describes the machine and not the session.
        def uptime(v):
            if isinstance(v, dict):
                return {k: uptime(x) for k, x in v.items()}
            if isinstance(v, list):
                return [uptime(x) for x in v]
            return MACHINE_UPTIME.sub("system 0h", v) if isinstance(v, str) else v

        return uptime(self.walk(msg))

    def claude(self, msg: dict) -> dict:
        if msg.get("type") in NEUTRALISE_CLAUDE_TYPES:
            keep = {k: msg[k] for k in ("type", "uuid", "session_id") if k in msg}
            msg = {**neutralise(msg), **keep}
        if msg.get("type") == "system" and msg.get("subtype") == "init":
            msg = dict(msg)
            msg["mcp_servers"] = []
            msg["tools"] = [t for t in msg.get("tools", []) if not str(t).startswith("mcp__")]
            if "apiKeySource" in msg:
                msg["apiKeySource"] = "redacted"
        return self.walk(msg)


MACHINE_UPTIME = re.compile(r"system \d+[dhms](?: \d+[dhms])*")

SUSPECT = re.compile(
    r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+\.[A-Za-z]{2,}"  # an e-mail address
    r"|/home/(?!user\b)[A-Za-z0-9_.-]+"  # a home directory other than the placeholder
    r"|/Users/[A-Za-z0-9_.-]+"
    r"|/mnt/|/tmp/claude|/private/var"
    r"|claude\.ai [A-Z]"  # a connected account's MCP server name
)


# Public, generic addresses that harnesses mention in their own text.
ALLOWED = {"noreply@anthropic.com"}


def strings(v):
    if isinstance(v, dict):
        for x in v.values():
            yield from strings(x)
    elif isinstance(v, list):
        for x in v:
            yield from strings(x)
    elif isinstance(v, str):
        yield v


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("raw", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--map", action="append", default=[], metavar="LOCAL=REPLACEMENT")
    args = ap.parse_args()
    maps = [tuple(m.split("=", 1)) for m in args.map]
    s = Scrubber(maps)
    problems = 0
    for harness in ("codex", "claude", "hermes", "openclaw", "acp"):
        if not (args.raw / harness).is_dir():
            continue
        pattern = "*/*.jsonl" if harness == "acp" else "*.jsonl"  # acp/<agent>/<scenario>.jsonl
        for src in sorted((args.raw / harness).glob(pattern)):
            rel = src.relative_to(args.raw / harness)
            (args.out / harness / rel.parent).mkdir(parents=True, exist_ok=True)
            lines = []
            for line in src.read_text(encoding="utf-8").splitlines():
                rec = json.loads(line)
                rec["msg"] = getattr(s, harness)(rec["msg"])
                lines.append(json.dumps(rec, ensure_ascii=False, sort_keys=False))
                for text in strings(rec["msg"]):
                    for word, _ in s.words:
                        if re.search(rf"(?<![A-Za-z0-9]){re.escape(word)}(?![A-Za-z0-9])", text):
                            problems += 1
                            print(f"LEFT BEHIND in {harness}/{rel}: the user or host name", file=sys.stderr)
                    for old, _ in s.maps:
                        if old in text:
                            problems += 1
                            print(f"LEFT BEHIND in {harness}/{rel}: a mapped path", file=sys.stderr)
                    m = next((x for x in SUSPECT.finditer(text) if x.group(0) not in ALLOWED), None)
                    if m:
                        problems += 1
                        print(f"LEFT BEHIND in {harness}/{rel}: {m.group(0)[:60]}", file=sys.stderr)
            (args.out / harness / rel).write_text("\n".join(lines) + "\n", encoding="utf-8")
            print(f"{harness}/{rel}: {len(lines)} messages")
    if problems:
        print(f"{problems} problem(s). Do not commit this output.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
