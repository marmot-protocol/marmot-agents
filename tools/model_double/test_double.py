"""The model double's own tests: how it reads each wire format into a key, how each mode answers, and that
what it writes parses as each format's stream. Run with:

    python3 -m unittest discover -s tools/model_double -p 'test_*.py'
"""

import json
import tempfile
import threading
import unittest
import urllib.request
from pathlib import Path

import double

REMINDER = "<system-reminder>\nSomething the harness adds.\n</system-reminder>\n"


def anthropic(*messages, tools=True):
    return {"model": "m", "stream": True, "messages": list(messages),
            "tools": [{"name": "Bash", "input_schema": {"type": "object"}}] if tools else []}


class Keys(unittest.TestCase):
    def test_anthropic_skips_reminders_and_the_system_message(self):
        body = anthropic(
            {"role": "user", "content": [{"type": "text", "text": REMINDER}, {"type": "text", "text": "Run `ls`"}]},
            {"role": "system", "content": "# Environment\nwhere it runs"},
        )
        self.assertEqual(double.key_of("anthropic", body),
                         {"kind": "main", "prompt": "Run `ls`", "after": 0, "last": "prompt"})

    def test_anthropic_counts_replies_and_names_results(self):
        body = anthropic(
            {"role": "user", "content": "Run `ls`"},
            {"role": "assistant", "content": [{"type": "thinking", "thinking": "t"},
                                              {"type": "tool_use", "id": "a", "name": "Bash", "input": {}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a", "content": "x"},
                                         {"type": "text", "text": REMINDER}]},
        )
        self.assertEqual(double.key_of("anthropic", body),
                         {"kind": "main", "prompt": "Run `ls`", "after": 1, "last": "results:Bash"})

    def test_a_request_with_no_tools_is_a_side_one(self):
        body = anthropic({"role": "user", "content": "Write a title"}, tools=False)
        self.assertEqual(double.key_of("anthropic", body)["kind"], "side")

    def test_responses_skips_instructions_and_context(self):
        body = {"tools": [{"type": "function", "name": "exec_command"}], "input": [
            {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "rules"}]},
            {"type": "message", "role": "user",
             "content": [{"type": "input_text", "text": "<environment_context>\n<cwd>/w</cwd>\n</environment_context>"}]},
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Run `ls`"}]},
            {"type": "reasoning", "summary": []},
            {"type": "function_call", "name": "exec_command", "call_id": "c", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c", "output": "x"},
        ]}
        self.assertEqual(double.key_of("responses", body),
                         {"kind": "main", "prompt": "Run `ls`", "after": 1, "last": "results:exec_command"})

    def test_chat(self):
        body = {"tools": [{"type": "function", "function": {"name": "bash"}}], "messages": [
            {"role": "system", "content": "rules"},
            {"role": "user", "content": "Run `ls`"},
            {"role": "assistant", "content": None,
             "tool_calls": [{"id": "c", "function": {"name": "bash", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "c", "content": "x"},
            {"role": "assistant", "content": "done"},
        ]}
        self.assertEqual(double.key_of("chat", body),
                         {"kind": "main", "prompt": "Run `ls`", "after": 2, "last": "assistant"})


class Modes(unittest.TestCase):
    key = {"kind": "main", "prompt": "Run `ls` now", "after": 1, "last": "results:Bash"}

    def test_scripted_answers_by_the_reply_number(self):
        script = double.Scripted({"prompts": [{"when": "`ls`", "replies": [[{"text": "a"}], [{"text": "b"}]]}]})
        self.assertEqual(script.answer("anthropic", self.key, {}), [{"text": "b"}])
        with self.assertRaises(double.Unmatched):
            script.answer("anthropic", {**self.key, "after": 2}, {})
        with self.assertRaises(double.Unmatched):
            script.answer("anthropic", {**self.key, "prompt": "something else"}, {})
        side = {**self.key, "kind": "side"}
        self.assertEqual(script.answer("anthropic", side, {}), [{"text": "ok"}])

    def test_replay_refuses_what_changed(self):
        replay = double.Replay([{"key": self.key, "reply": [{"text": "b"}]}])
        self.assertEqual(replay.answer("anthropic", self.key, {}), [{"text": "b"}])
        with self.assertRaisesRegex(double.Unmatched, "results:Shell"):
            replay.answer("anthropic", {**self.key, "last": "results:Shell"}, {})
        with self.assertRaises(double.Unmatched):
            replay.answer("anthropic", {**self.key, "after": 0}, {})


def events(text):
    """The data of each server-sent event."""
    return [json.loads(line[6:]) for line in text.splitlines()
            if line.startswith("data: ") and line != "data: [DONE]"]


PARTS = [{"thinking": "t"}, {"text": "hi"}, {"call": "Bash", "input": {"command": "ls"}}]


class Writers(unittest.TestCase):
    def test_anthropic(self):
        kind, text = double.anthropic_reply(PARTS, {"stream": True})
        self.assertEqual(kind, "text/event-stream")
        got = events(text)
        self.assertEqual([e["type"] for e in got][:2], ["message_start", "content_block_start"])
        self.assertEqual(got[-2]["delta"]["stop_reason"], "tool_use")
        call = [e for e in got if e.get("delta", {}).get("type") == "input_json_delta"]
        self.assertEqual(json.loads(call[0]["delta"]["partial_json"]), {"command": "ls"})

    def test_responses(self):
        _, text = double.responses_reply(PARTS, {"stream": True})
        got = events(text)
        self.assertEqual(got[-1]["type"], "response.completed")
        output = got[-1]["response"]["output"]
        self.assertEqual([o["type"] for o in output], ["reasoning", "message", "function_call"])
        self.assertEqual(json.loads(output[2]["arguments"]), {"command": "ls"})

    def test_responses_custom_tool(self):
        body = {"tools": [{"type": "custom", "name": "apply_patch"}]}
        _, text = double.responses_reply([{"call": "apply_patch", "input": {"input": "*** Begin"}}], body)
        item = json.loads(text)["output"][0]
        self.assertEqual((item["type"], item["input"]), ("custom_tool_call", "*** Begin"))

    def test_chat(self):
        _, text = double.chat_reply(PARTS, {"stream": True})
        self.assertTrue(text.rstrip().endswith("data: [DONE]"))
        deltas = [e["choices"][0]["delta"] for e in events(text) if e["choices"]]
        self.assertIn({"reasoning_content": "t"}, deltas)
        self.assertEqual(deltas[-2]["tool_calls"][0]["function"]["name"], "Bash")


class Translation(unittest.TestCase):
    def test_anthropic_to_chat(self):
        body = anthropic(
            {"role": "user", "content": "Run `ls`"},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "a", "name": "Bash", "input": {"c": 1}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a", "content": "x"}]},
        )
        body["system"] = [{"type": "text", "text": "rules"}]
        chat = double.to_chat("anthropic", body)
        self.assertEqual([m["role"] for m in chat["messages"]], ["system", "user", "assistant", "tool"])
        self.assertEqual(chat["messages"][3]["tool_call_id"], "a")
        self.assertEqual(chat["tools"][0]["function"]["name"], "Bash")

    def test_responses_to_chat(self):
        body = {"instructions": "rules", "tools": [{"type": "custom", "name": "apply_patch"}], "input": [
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Run"}]},
            {"type": "function_call", "name": "exec_command", "call_id": "c", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c", "output": "x"}]}
        chat = double.to_chat("responses", body)
        self.assertEqual([m["role"] for m in chat["messages"]], ["system", "user", "assistant", "tool"])
        self.assertEqual(chat["tools"][0]["function"]["parameters"]["required"], ["input"])

    def test_chat_reply_to_parts(self):
        reply = {"choices": [{"message": {"content": "hi", "reasoning_content": "t", "tool_calls": [
            {"id": "c", "function": {"name": "Bash", "arguments": "{\"command\": \"ls\"}"}}]}}]}
        self.assertEqual(double.parts_of_chat(reply), PARTS)


class Served(unittest.TestCase):
    """The server itself, on the loopback address: a request in, a stream out, and the log."""

    def test_scripted_server_and_cassette(self):
        with tempfile.TemporaryDirectory() as folder:
            saved = double.Cassette(Path(folder) / "c.jsonl", {"cassette": 1, "harness": "h"})
            script = double.Scripted({"prompts": [{"when": "`ls`", "replies": [[{"text": "hi"}]]}]})
            server = double.Server(0, script, saved)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            url = f"http://127.0.0.1:{server.server_address[1]}"
            try:
                body = {"stream": True, "tools": [{"type": "function"}],
                        "messages": [{"role": "user", "content": "Run `ls`"}]}
                request = urllib.request.Request(url + "/v1/chat/completions", data=json.dumps(body).encode())
                with urllib.request.urlopen(request) as r:
                    self.assertIn("data: [DONE]", r.read().decode())
                body["messages"][0]["content"] = "something else"
                request = urllib.request.Request(url + "/v1/chat/completions", data=json.dumps(body).encode())
                with self.assertRaises(urllib.error.HTTPError) as refused:
                    urllib.request.urlopen(request)
                self.assertEqual(refused.exception.code, 400)
                with urllib.request.urlopen(url + "/_double/log") as r:
                    log = json.loads(r.read())
                self.assertEqual([e.get("unmatched") is None for e in log], [True, False])
            finally:
                server.shutdown()
                server.server_close()
            header, exchanges = double.load_cassette(Path(folder) / "c.jsonl")
            self.assertEqual(header["harness"], "h")
            self.assertEqual(exchanges, [{"key": {"kind": "main", "prompt": "Run `ls`", "after": 0, "last": "prompt"},
                                          "reply": [{"text": "hi"}]}])


if __name__ == "__main__":
    unittest.main()
