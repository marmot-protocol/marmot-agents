#!/usr/bin/env python3
"""A stand-in for `pi --mode rpc`, for the driver's tests. It speaks the records the recordings in
fixtures/recordings/pi show, and nothing else, and needs no model.

What a prompt does depends on its words:

- "sleep": runs a `bash` tool that waits until the run is aborted, or for 30 seconds;
- "ask": an extension asks Yes or No before a tool, and the run ends with what was answered;
- "hangup": with "ask", the process closes its standard input before it asks, so no answer can
  reach it, and exits a second after asking;
- "die": the process exits in the middle of a tool, as a crash would;
- "crash": with "sleep", the process exits when asked to abort, before it responds;
- "/ask": an extension command that asks Yes or No, and is acknowledged only once answered, as
  Pi 1.6.0 acknowledges a command only after its handler returns;
- "/veto": an extension command, after which an extension cancels any new session;
- "auto": Pi compacts the conversation of its own accord before it replies;
- any other prompt that starts with "/": an extension command, handled with no run;
- "file:<path>": calls the driver's `send_file` tool with <path>, as Pi does once it has loaded the
  driver's extension (`--extension <file>`, which must be the driver's own); the reply is "sent:"
  and the tool's result, and each answer the tool gets is added to `.fake-file-replies`;
- anything else: one text reply, "ok: <the prompt>".

Setting the thinking level to "crash" makes the process exit before it responds. As Pi does, a
setting changed to the value it has makes no `thinking_level_changed`.

`compact` takes half a second, in the background, as a model call would.

As Pi does, the fake writes a session's file (in `.fake-sessions` in the working folder) only once
the session holds a message. `--version` prints a version. `--session <id>` resumes that session,
and exits if it has no file; `--session-id <id>` resumes it, or starts an empty session with that
id; an id that starts with "other" comes up as a session of another id. `--ask-at-start` makes an
extension ask a question as Pi starts, before it reads anything, as Pi 1.6.0 does when an extension
asks in its start-up handler; it then waits for good. Each fake writes its process id to
`.fake-pid`, so a test can see that it was stopped.
"""

import json
import os
import sys
import threading
import time
import uuid

out_lock = threading.Lock()
state = {"session": None, "thinking": "low", "model": "m1", "running": False, "aborted": False,
         "steered": [], "answer": None, "crash_on_abort": False, "veto": False}
wake = threading.Condition()


def send(record):
    with out_lock:
        sys.stdout.write(json.dumps(record) + "\n")
        sys.stdout.flush()


def respond(cmd, success=True, data=None, error=None):
    record = {"id": cmd.get("id"), "type": "response", "command": cmd["type"], "success": success}
    if data is not None:
        record["data"] = data
    if error is not None:
        record["error"] = error
    send(record)


def model(model_id):
    return {"id": model_id, "name": model_id, "provider": "fake"}


def now():
    return int(time.time() * 1000)


def assistant(content, stop="stop"):
    message = {"role": "assistant", "content": content, "usage": {"input": 10, "output": 2},
               "stopReason": stop, "timestamp": now()}
    send({"type": "message_start", "message": {**message, "content": [], "stopReason": "pending"}})
    send({"type": "message_end", "message": message})


def wait_until(predicate, seconds):
    deadline = time.time() + seconds
    with wake:
        while not predicate() and time.time() < deadline:
            wake.wait(0.05)


def keep_session():
    os.makedirs(".fake-sessions", exist_ok=True)
    open(os.path.join(".fake-sessions", state["session"]), "w").close()


def compact(cmd):
    time.sleep(0.5)
    send({"type": "compaction_start", "reason": "manual"})
    send({"type": "compaction_end", "reason": "manual", "result": {"summary": "s"}, "aborted": False,
          "willRetry": False})
    respond(cmd, data={"summary": "s"})


def send_file(text):
    """The driver's own tool, as its extension runs it: an input dialog with the request's title.
    Without the extension, Pi has no such tool."""
    path = text.split("file:", 1)[1].split()[0]
    if not extension_loaded():
        return "no send_file tool"
    call = f"call-{uuid.uuid4()}"
    assistant([{"type": "toolCall", "id": call, "name": "send_file", "arguments": {"path": path}}], "toolUse")
    send({"type": "tool_execution_start", "toolCallId": call, "toolName": "send_file", "args": {"path": path}})
    send({"type": "extension_ui_request", "id": str(uuid.uuid4()), "method": "input",
          "title": "marmot-agent send_file", "placeholder": json.dumps({"path": path})})
    wait_until(lambda: state["answer"] is not None, 30)
    answer = json.loads(state["answer"] or "{}")
    with open(".fake-file-replies", "a") as f:
        f.write(json.dumps(answer) + "\n")
    result = {"content": [{"type": "text", "text": answer.get("text", "Not sent.")}]}
    send({"type": "tool_execution_end", "toolCallId": call, "toolName": "send_file", "result": result,
          "isError": answer.get("sent") is not True})
    return f"sent: {answer.get('text')}"


def extension_loaded():
    """Whether Pi was started with the driver's extension: a file that registers the tool."""
    args = sys.argv[1:]
    if "--extension" not in args:
        return False
    try:
        with open(args[args.index("--extension") + 1]) as f:
            source = f.read()
    except OSError:
        return False
    return 'name: "send_file"' in source and "marmot-agent send_file" in source


def run(text):
    keep_session()
    send({"type": "agent_start"})
    user = {"role": "user", "content": [{"type": "text", "text": text}], "timestamp": now()}
    send({"type": "message_start", "message": user})
    send({"type": "message_end", "message": user})
    reply = f"ok: {text}"
    if "file:" in text:
        reply = send_file(text)
    if "auto" in text:
        send({"type": "compaction_start", "reason": "threshold"})
        send({"type": "compaction_end", "reason": "threshold", "result": {"summary": "s"},
              "aborted": False, "willRetry": False})
    if "sleep" in text or "die" in text or "ask" in text:
        call = f"call-{uuid.uuid4()}"
        assistant([{"type": "toolCall", "id": call, "name": "bash", "arguments": {"command": "sleep 30"}}], "toolUse")
        if "ask" in text:
            asked = str(uuid.uuid4())
            send({"type": "extension_ui_request", "id": asked, "method": "select",
                  "title": "Allow sleep 30?", "options": ["Yes", "No"]})
            wait_until(lambda: state["answer"] is not None, 1 if "hangup" in text else 30)
            if "hangup" in text:
                os._exit(0)
            allowed = state["answer"] == "Yes"
            reply = "allowed" if allowed else "denied"
        send({"type": "tool_execution_start", "toolCallId": call, "toolName": "bash", "args": {"command": "sleep 30"}})
        if "die" in text:
            time.sleep(0.2)
            sys.stdout.flush()
            os._exit(1)
        if "sleep" in text:
            wait_until(lambda: state["aborted"], 30)
        aborted = state["aborted"]
        result = {"content": [{"type": "text", "text": "Command aborted" if aborted else ""}]}
        send({"type": "tool_execution_end", "toolCallId": call, "toolName": "bash", "result": result,
              "isError": aborted})
        if aborted:
            assistant([], "aborted")
            finish(True)
            return
    for steered in list(state["steered"]):
        message = {"role": "user", "content": [{"type": "text", "text": steered}], "timestamp": now()}
        send({"type": "message_start", "message": message})
        send({"type": "message_end", "message": message})
        reply += f" and {steered}"
    state["steered"].clear()
    assistant([{"type": "text", "text": reply}])
    finish(False)


def ask_command(cmd):
    """An extension command that asks first: Pi acknowledges the prompt once its handler returns."""
    state["answer"] = None
    send({"type": "extension_ui_request", "id": str(uuid.uuid4()), "method": "confirm",
          "title": "Go ahead?", "message": "The command asks first."})
    wait_until(lambda: state["answer"] is not None, 30)
    respond(cmd, data={"disposition": "handled"})


def finish(aborted):
    send({"type": "agent_end", "messages": [], "willRetry": False})
    send({"type": "agent_settled", "aborted": aborted})
    with wake:
        state["running"] = False
        wake.notify_all()


def main():
    args = sys.argv[1:]
    if "--version" in args:
        print("0.0.0-fake")
        return
    with open(".fake-pid", "w") as f:
        f.write(str(os.getpid()))
    if "--session" in args and args[args.index("--session") + 1].startswith("other"):
        state["session"] = str(uuid.uuid4())
    elif "--session" in args:
        state["session"] = args[args.index("--session") + 1]
        if not os.path.exists(os.path.join(".fake-sessions", state["session"])):
            print("No session found", file=sys.stderr)
            sys.exit(1)
    elif "--session-id" in args:
        state["session"] = args[args.index("--session-id") + 1]
    else:
        state["session"] = str(uuid.uuid4())
    if "--ask-at-start" in args:
        send({"type": "extension_ui_request", "id": str(uuid.uuid4()), "method": "confirm",
              "title": "Trust this folder?", "message": "An extension asks as Pi starts."})
        while True:
            time.sleep(60)
    for line in sys.stdin:
        if not line.strip():
            continue
        cmd = json.loads(line)
        t = cmd.get("type")
        if t == "get_state":
            respond(cmd, data={"model": model(state["model"]), "thinkingLevel": state["thinking"],
                               "isStreaming": state["running"], "sessionId": state["session"]})
        elif t == "get_available_models":
            respond(cmd, data={"models": [model("m1"), model("m2")]})
        elif t == "get_available_thinking_levels":
            respond(cmd, data={"levels": ["off", "low", "high"]})
        elif t == "get_commands":
            respond(cmd, data={"commands": [{"name": "fake", "description": "A fake command", "source": "extension"}]})
        elif t == "prompt":
            text = cmd.get("message", "")
            if text == "/ask":
                threading.Thread(target=ask_command, args=(cmd,), daemon=True).start()
            elif text.startswith("/"):
                state["veto"] = state["veto"] or text == "/veto"
                respond(cmd, data={"disposition": "handled"})
            elif "hangup" in text:
                state["running"] = True
                respond(cmd, data={"disposition": "started"})
                os.close(0)  # sys.stdin does not own it
                run(text)
            elif state["running"]:
                state["steered"].append(text)
                respond(cmd, data={"disposition": "queued"})
            else:
                state["running"], state["aborted"], state["answer"] = True, False, None
                state["crash_on_abort"] = "crash" in text
                respond(cmd, data={"disposition": "started"})
                threading.Thread(target=run, args=(text,), daemon=True).start()
        elif t == "abort":
            if state["crash_on_abort"]:
                os._exit(1)
            with wake:
                state["aborted"] = True
                wake.notify_all()
            wait_until(lambda: not state["running"], 30)
            respond(cmd)
        elif t == "extension_ui_response":
            with wake:
                state["answer"] = cmd["confirmed"] if "confirmed" in cmd else cmd.get("value")
                wake.notify_all()
        elif t == "new_session":
            if not state["veto"]:
                state["session"] = str(uuid.uuid4())
            respond(cmd, data={"cancelled": state["veto"]})
        elif t == "set_thinking_level":
            if cmd["level"] == "crash":
                os._exit(1)
            if cmd["level"] != state["thinking"]:
                state["thinking"] = cmd["level"]
                send({"type": "thinking_level_changed", "level": cmd["level"]})
            respond(cmd)
        elif t == "set_model":
            if cmd.get("modelId") in ("m1", "m2"):
                state["model"] = cmd["modelId"]
                respond(cmd, data=model(cmd["modelId"]))
            else:
                respond(cmd, success=False, error=f"Model not found: fake/{cmd.get('modelId')}")
        elif t == "compact":
            threading.Thread(target=compact, args=(cmd,), daemon=True).start()
        else:
            respond(cmd, success=False, error=f"Unknown command: {t}")


if __name__ == "__main__":
    main()
