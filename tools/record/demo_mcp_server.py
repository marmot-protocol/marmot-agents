#!/usr/bin/env python3
"""A minimal MCP server over stdio, with one tool, for recording MCP tool calls.

It offers `define_word`, which returns a fixed definition. It speaks just enough of the
Model Context Protocol for a harness to list and call that tool. No dependencies.
"""

import json
import sys

DEFINITIONS = {"marmot": "A large ground squirrel that lives in burrows and whistles when alarmed."}
TOOL = {
    "name": "define_word",
    "description": "Return the dictionary definition of a word.",
    "inputSchema": {
        "type": "object",
        "properties": {"word": {"type": "string", "description": "The word to define."}},
        "required": ["word"],
    },
}


def handle(msg: dict):
    method, params = msg.get("method"), msg.get("params") or {}
    if method == "initialize":
        return {
            "protocolVersion": params.get("protocolVersion", "2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "demo", "version": "0.0.0"},
        }
    if method == "tools/list":
        return {"tools": [TOOL]}
    if method == "tools/call":
        word = str((params.get("arguments") or {}).get("word", "")).lower()
        text = DEFINITIONS.get(word, f"No definition found for '{word}'.")
        return {"content": [{"type": "text", "text": text}], "isError": False}
    if method == "ping":
        return {}
    return None


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if "id" not in msg:
            continue  # a notification
        result = handle(msg)
        if result is None:
            reply = {"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32601, "message": "Method not found"}}
        else:
            reply = {"jsonrpc": "2.0", "id": msg["id"], "result": result}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
