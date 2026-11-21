#!/usr/bin/env python3
"""Summarise the structure of recordings: every kind of message, how often it appears, and
its fields with their types. Values are not printed, except for a few enumeration fields.

Usage:
    inventory.py <recordings-dir>          e.g. fixtures/recordings/codex
    inventory.py <recordings-dir> --kinds  only the list of kinds and counts

It has no dependencies.
"""

from __future__ import annotations

import collections
import json
import re
import sys
from pathlib import Path

ENUM_FIELDS = {"type", "subtype", "status", "stop_reason", "role", "priority", "behavior", "decision", "state", "phase"}
# Free-form objects, or objects keyed by an id: note that they exist, never expand them.
OPAQUE = {"input", "updatedInput", "tool_use_result", "wire_tool_inputs", "wire_ingest_context", "metadata"}


class Node:
    def __init__(self):
        self.types, self.fields, self.elem, self.seen = set(), {}, None, 0
        self.enum = collections.Counter()

    def add(self, v, key=None, depth=0):
        self.seen += 1
        if isinstance(v, dict):
            self.types.add("object")
            if depth < 5 and key not in OPAQUE:
                for k, x in v.items():
                    self.fields.setdefault(k, Node()).add(x, k, depth + 1)
        elif isinstance(v, list):
            self.types.add("array")
            if depth < 5 and key not in OPAQUE:
                self.elem = self.elem or Node()
                for x in v[:50]:
                    self.elem.add(x, key, depth + 1)
        elif isinstance(v, bool):
            self.types.add("boolean")
        elif v is None:
            self.types.add("null")
        elif isinstance(v, str):
            self.types.add("string")
            if key in ENUM_FIELDS and re.fullmatch(r"[A-Za-z0-9_.\-]{1,40}", v):
                self.enum[v] += 1
        else:
            self.types.add("number")

    def render(self, name, parent_seen, indent, out):
        optional = "" if self.seen >= parent_seen else "?"
        line = "  " * indent + f"{name}{optional}: " + "|".join(sorted(self.types))
        if self.enum and len(self.enum) <= 16:
            line += " = " + ", ".join(k for k, _ in self.enum.most_common())
        if name in OPAQUE:
            line += " (free-form; not expanded)"
        out.append(line)
        if len(self.fields) > 60:
            out[-1] += " (many keys; not expanded)"
            return
        for k in sorted(self.fields):
            self.fields[k].render(k, self.seen, indent + 1, out)
        if self.elem is not None and (self.elem.fields or self.elem.enum):
            self.elem.render("[]", self.elem.seen, indent + 1, out)


def kind_of(rec: dict) -> str:
    m = rec["msg"]
    arrow = "client to harness" if rec["dir"] == "out" else "harness to client"
    if "via" not in rec and "type" in m:  # Pi's RPC records
        name = str(m["type"])
        if m["type"] == "response":
            name += f" to {m.get('command')}" + ("" if m.get("success") else " (refused)")
        elif m["type"] in ("message_start", "message_end") and isinstance(m.get("message"), dict):
            name += f" [{m['message'].get('role')}]"
        elif m["type"] == "message_update":
            name += f" [{(m.get('assistantMessageEvent') or {}).get('type')}]"
        elif m["type"] in ("extension_ui_request", "extension_ui_response"):
            name += f" [{m.get('method', 'answer')}]"
        return f"{arrow}: {name}"
    if "_recorder" in m:
        return f"{arrow}: recorder note"
    if "method" in m and "via" not in rec:  # Codex JSON-RPC
        item = (m.get("params") or {}).get("item") if isinstance(m.get("params"), dict) else None
        name = m["method"] + (f" [{item['type']}]" if isinstance(item, dict) and item.get("type") else "")
        if "id" in m:
            name += " (request)"
        return f"{arrow}: {name}"
    if "via" not in rec:
        return f"{arrow}: " + ("response" if "result" in m else "error response" if "error" in m else "other")
    name = str(m.get("type") or m.get("kind") or m.get("method"))
    if m.get("subtype"):
        name += f" / {m['subtype']}"
    return f"{arrow} ({rec['via']}): {name}"


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    root, kinds_only = Path(sys.argv[1]), "--kinds" in sys.argv
    kinds: dict[str, Node] = {}
    counts, scenarios = collections.Counter(), collections.defaultdict(set)
    for path in sorted(root.glob("*.jsonl")):
        for line in path.read_text(encoding="utf-8").splitlines():
            rec = json.loads(line)
            k = kind_of(rec)
            counts[k] += 1
            scenarios[k].add(path.stem)
            kinds.setdefault(k, Node()).add(rec["msg"])
    print(f"{sum(counts.values())} messages in {len(list(root.glob('*.jsonl')))} recordings under {root}\n")
    for k, n in sorted(counts.items(), key=lambda x: (x[0].split(":")[0], -x[1])):
        where = "all" if len(scenarios[k]) >= len(list(root.glob("*.jsonl"))) else ", ".join(sorted(scenarios[k]))
        print(f"{n:5d}  {k}   [{where}]")
    if kinds_only:
        return 0
    for k, n in sorted(counts.items(), key=lambda x: (x[0].split(":")[0], -x[1])):
        print(f"\n### {k}  ({n})")
        out: list[str] = []
        kinds[k].render("msg", kinds[k].seen, 0, out)
        print("\n".join(out[1:]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
