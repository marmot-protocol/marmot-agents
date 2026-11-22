#!/usr/bin/env python3
"""A stand-in for an agent over ACP (`opencode acp`, `goose acp`), for the import test
(`import_coding.rs`): it needs no model and no account.

    stand_in_acp.py <name> acp

It names itself <name> when greeted, and refuses to start unless its own arguments are exactly
`acp`, as the import writes the command. Each prompt is answered "ok: " and the prompt's text, in
words, and the prompt's text is added, as one JSON string per line, to `.stand-in-prompts` in the
folder it was started in, so a test sees every prompt it heard. Anything else asked of it is
refused.
"""

import json
import sys


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main():
    if len(sys.argv) != 3 or sys.argv[2] != "acp":
        sys.stderr.write("usage: stand_in_acp.py <name> acp\n")
        return 2
    name = sys.argv[1]
    sessions = 0
    messages = 0
    for line in sys.stdin:
        try:
            frame = json.loads(line)
        except ValueError:
            continue
        method = frame.get("method")
        if "id" not in frame or method is None:
            continue  # a notification, such as session/cancel, or a reply
        ident, params = frame["id"], frame.get("params") or {}
        if method == "initialize":
            send({"jsonrpc": "2.0", "id": ident, "result": {
                "protocolVersion": 1,
                "agentCapabilities": {"loadSession": False, "mcpCapabilities": {"http": False}},
                "agentInfo": {"name": name, "version": "0.0.0-stand-in"},
            }})
        elif method == "session/new":
            sessions += 1
            send({"jsonrpc": "2.0", "id": ident, "result": {"sessionId": f"s{sessions}"}})
        elif method == "session/prompt":
            text = "".join(
                block.get("text", "") for block in params.get("prompt", [])
                if block.get("type") == "text"
            )
            with open(".stand-in-prompts", "a") as heard:
                heard.write(json.dumps(text) + "\n")
            messages += 1
            send({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": params.get("sessionId"),
                "update": {"sessionUpdate": "agent_message_chunk", "messageId": f"m{messages}",
                           "content": {"type": "text", "text": f"ok: {text}"}},
            }})
            send({"jsonrpc": "2.0", "id": ident, "result": {"stopReason": "end_turn"}})
        else:
            send({"jsonrpc": "2.0", "id": ident,
                  "error": {"code": -32601, "message": "not in the stand-in"}})
    return 0


if __name__ == "__main__":
    sys.exit(main())
