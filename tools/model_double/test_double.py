"""The model double's own tests: how it reads each wire format into a key, how each mode answers, and that
what it writes parses as each format's stream. Run with:

    python3 -m unittest discover -s tools/model_double -p 'test_*.py'
"""

import contextlib
import datetime
import io
import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.request
from pathlib import Path

import check_cassettes
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

    def test_anthropic_keeps_words_sent_beside_tool_results(self):
        # A person's words after a tool's results, in the same message, are the new prompt.
        body = anthropic(
            {"role": "user", "content": "Run `ls`"},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "a", "name": "Bash", "input": {}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a", "content": "x"},
                                         {"type": "text", "text": REMINDER + "Summarize only"}]},
        )
        self.assertEqual(double.key_of("anthropic", body),
                         {"kind": "main", "prompt": "Summarize only", "after": 0, "last": "prompt"})

    def test_context_beside_the_prompt_is_part_of_it(self):
        body = {"tools": [{"type": "function"}], "messages": [
            {"role": "user", "content": "Run `ls`"},
            {"role": "user", "content": "Sender: someone"},
        ]}
        self.assertEqual(double.key_of("chat", body)["prompt"], "Run `ls` Sender: someone")

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

    def test_replay_matches_the_whole_key_first(self):
        # Two exchanges that differ only in what the model answers: each is answered by its own.
        replay = double.Replay([{"key": {**self.key, "last": "results:Read"}, "reply": [{"text": "a"}]},
                                {"key": self.key, "reply": [{"text": "b"}]}])
        self.assertEqual(replay.answer("anthropic", self.key, {}), [{"text": "b"}])
        self.assertEqual(replay.answer("anthropic", {**self.key, "last": "results:Read"}, {}), [{"text": "a"}])

    def test_replay_refuses_a_cassette_with_two_replies_for_one_key(self):
        # Only the first would ever be served, and the second would pass as used.
        with self.assertRaisesRegex(ValueError, "exchanges 1 and 3"):
            double.Replay([{"key": self.key, "reply": [{"text": "a"}]},
                           {"key": {**self.key, "after": 0}, "reply": [{"text": "a"}]},
                           {"key": self.key, "reply": [{"text": "b"}]}])
        same = {"key": self.key, "reply": [{"text": "a"}]}
        double.Replay([same, same])


class Cassettes(unittest.TestCase):
    key = Modes.key

    def test_a_second_reply_for_one_key_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            cassette = double.Cassette(Path(folder) / "c.jsonl", {"cassette": 1})
            cassette.keep(self.key, [{"text": "a"}])
            cassette.keep(self.key, [{"text": "a"}])
            with self.assertRaises(double.Conflict):
                cassette.keep(self.key, [{"text": "b"}])
            _, exchanges = double.load_cassette(Path(folder) / "c.jsonl")
        self.assertEqual(exchanges, [{"key": self.key, "reply": [{"text": "a"}]}])

    def test_an_exchange_that_failed_to_be_written_is_written_on_a_retry(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "later" / "c.jsonl"
            cassette = double.Cassette(path, {"cassette": 1})
            with self.assertRaises(OSError):
                cassette.keep(self.key, [{"text": "a"}])
            path.parent.mkdir()
            cassette.keep(self.key, [{"text": "a"}])
            header, exchanges = double.load_cassette(path)
        self.assertEqual((header, exchanges), ({"cassette": 1}, [{"key": self.key, "reply": [{"text": "a"}]}]))

    def test_a_line_separator_in_a_reply_is_not_a_record_boundary(self):
        reply = [{"text": "one\u2028two\u2029three\x85four"}]
        with tempfile.TemporaryDirectory() as folder:
            cassette = double.Cassette(Path(folder) / "c.jsonl", {"cassette": 1})
            cassette.keep(self.key, reply)
            _, exchanges = double.load_cassette(Path(folder) / "c.jsonl")
        self.assertEqual(exchanges, [{"key": self.key, "reply": reply}])


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


def post(url, path, body):
    request = urllib.request.Request(url + path, data=json.dumps(body).encode())
    with urllib.request.urlopen(request) as r:
        return r.read().decode()


class Serving:
    """A double on the loopback address, stopped on leaving."""

    def __init__(self, mode, save=None, keep_bodies=False, verbose=False):
        self.server = double.Server(0, mode, save)
        self.server.keep_bodies = keep_bodies
        self.server.verbose = verbose
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"

    def __enter__(self):
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()

    def log(self):
        with urllib.request.urlopen(self.url + "/_double/log") as r:
            return json.loads(r.read())


# What a request holds that must never reach a log line: a person's words, a path, a host name.
PRIVATE = ["Run `ls` in secret-folder", "/home/someone", "somehost.example"]


def chat_request(prompt):
    return {"stream": True, "tools": [{"type": "function"}],
            "messages": [{"role": "system", "content": "cwd /home/someone on somehost.example"},
                         {"role": "user", "content": prompt}]}


class Served(unittest.TestCase):
    """The server itself, on the loopback address: a request in, a stream out, and the log."""

    def test_scripted_server(self):
        script = double.Scripted({"prompts": [{"when": "`ls`", "replies": [[{"text": "hi"}]]}]})
        with Serving(script) as served:
            self.assertIn("data: [DONE]", post(served.url, "/v1/chat/completions", chat_request("Run `ls`")))
            with self.assertRaises(urllib.error.HTTPError) as refused:
                post(served.url, "/v1/chat/completions", chat_request("something else"))
            self.assertEqual(refused.exception.code, 400)
            log = served.log()
        self.assertEqual([e.get("unmatched") is None for e in log], [True, False])
        self.assertNotIn("body", log[0])

    def test_nothing_of_a_request_is_printed(self):
        script = double.Scripted({"prompts": [{"when": "secret-folder", "replies": [[{"text": "hi"}]]}]})
        printed = io.StringIO()
        with contextlib.redirect_stderr(printed), Serving(script, keep_bodies=True, verbose=True) as served:
            post(served.url, "/v1/chat/completions", chat_request(PRIVATE[0]))
            answered = chat_request(PRIVATE[0])
            answered["messages"].append({"role": "assistant", "content": "hi"})
            for asked in [answered, chat_request("Run ls on somehost.example")]:
                with self.assertRaises(urllib.error.HTTPError):
                    post(served.url, "/v1/chat/completions", asked)
            log = served.log()
        lines = printed.getvalue().splitlines()
        self.assertEqual(len(lines), 3, lines)
        self.assertIn("unmatched chat main request, after 1, answering assistant", lines[1])
        for line in lines:
            for private in PRIVATE + ["secret"]:
                self.assertNotIn(private, line)
        # The log a test reads keeps each body whole.
        self.assertTrue(all(PRIVATE[1] in e["body"] for e in log))

    def test_every_post_is_logged(self):
        with Serving(double.Scripted({}), keep_bodies=True) as served:
            post(served.url, "/v1/messages/count_tokens", {"messages": [{"role": "user", "content": "one"}]})
            with self.assertRaises(urllib.error.HTTPError) as refused:
                post(served.url, "/v1/somewhere/else", {"input": "two"})
            self.assertEqual(refused.exception.code, 404)
            log = served.log()
        self.assertEqual([e["format"] for e in log], ["count_tokens", "other"])
        self.assertIn("one", log[0]["body"])
        self.assertIn("two", log[1]["body"])
        # A POST to a path it does not serve was not answered, so a test that checks for that fails.
        self.assertEqual([e.get("unmatched") for e in log], [None, "not a path the double serves"])

    def test_record_keeps_what_the_model_said(self):
        upstream = double.Scripted({"prompts": [{"when": "`ls`", "replies": [[{"text": "hi"}]]}]})
        with tempfile.TemporaryDirectory() as folder, Serving(upstream, keep_bodies=True) as model:
            key = Path(folder) / "key"
            key.write_text("not-a-key\n")
            cassette = double.Cassette(Path(folder) / "c.jsonl", {"cassette": 1, "harness": "h"})
            with Serving(double.Record(model.url + "/v1", key, "m"), cassette) as served:
                body = anthropic({"role": "user", "content": "Run `ls`"})
                self.assertIn("message_stop", post(served.url, "/v1/messages", body))
            sent = json.loads(model.log()[0]["body"])
            header, exchanges = double.load_cassette(Path(folder) / "c.jsonl")
        self.assertEqual((sent["model"], sent["stream"]), ("m", False))
        self.assertEqual(header["harness"], "h")
        self.assertEqual(exchanges, [{"key": {"kind": "main", "prompt": "Run `ls`", "after": 0, "last": "prompt"},
                                      "reply": [{"text": "hi"}]}])


class Scrubbing(unittest.TestCase):
    def test_the_recipe_in_the_readme_scrubs_a_cassette(self):
        # The layout README.md's recipe uses: cassettes/<harness>/<scenario>.jsonl, scrubbed whole.
        scrub = Path(__file__).parent.parent / "record" / "scrub.py"
        with tempfile.TemporaryDirectory() as folder:
            raw, clean = Path(folder) / "scrub", Path(folder) / "clean"
            (raw / "cassettes" / "claude").mkdir(parents=True)
            header = {"cassette": 1, "harness": "claude-code", "version": "0", "recorded": "2026-11-23"}
            exchange = {"key": {"kind": "main", "prompt": f"Run it in {folder}/demo", "after": 0, "last": "prompt"},
                        "reply": [{"text": "done"}]}
            (raw / "cassettes" / "claude" / "approval.jsonl").write_text(
                json.dumps(header) + "\n" + json.dumps(exchange) + "\n")
            subprocess.run([sys.executable, scrub, raw, clean, "--map", f"{folder}/demo=/work/demo"],
                           check=True, capture_output=True)
            header_out, exchanges = double.load_cassette(clean / "cassettes" / "claude" / "approval.jsonl")
        self.assertEqual(header_out, header)
        self.assertEqual(exchanges[0]["key"]["prompt"], "Run it in /work/demo")

    def test_the_header_keeps_its_harness_and_a_reply_its_line_separators(self):
        # A user named like the harness: its name in a reply is scrubbed, the header's harness is not.
        scrub = Path(__file__).parent.parent / "record" / "scrub.py"
        with tempfile.TemporaryDirectory() as folder:
            raw, clean = Path(folder) / "scrub", Path(folder) / "clean"
            (raw / "cassettes" / "claude").mkdir(parents=True)
            header = {"cassette": 1, "harness": "claude-code", "version": "0", "recorded": "2026-11-23",
                      "format": "anthropic"}
            exchange = {"key": {"kind": "main", "prompt": "hello", "after": 0, "last": "prompt"},
                        "reply": [{"text": "hi claude\u2028bye"}]}
            (raw / "cassettes" / "claude" / "a.jsonl").write_text(
                json.dumps(header) + "\n" + json.dumps(exchange, ensure_ascii=False) + "\n", encoding="utf-8")
            env = {**os.environ, "USER": "claude", "LOGNAME": "claude"}
            run = subprocess.run([sys.executable, scrub, raw, clean], capture_output=True, text=True, env=env)
            self.assertEqual(run.returncode, 0, run.stderr)
            header_out, exchanges = double.load_cassette(clean / "cassettes" / "claude" / "a.jsonl")
        self.assertEqual(header_out, header)
        self.assertEqual(exchanges[0]["reply"], [{"text": "hi user\u2028bye"}])


class Staleness(unittest.TestCase):
    def test_old_cassettes_and_newer_releases_are_reported(self):
        with tempfile.TemporaryDirectory() as folder:
            for name, harness, version, day in [("a.jsonl", "claude-code", "2.1.326", "2026-11-01"),
                                                ("b.jsonl", "codex", "0.171.0", "2026-11-20"),
                                                ("c.jsonl", "hermes", "1.0", "2026-11-20")]:
                header = {"cassette": 1, "harness": harness, "version": version, "recorded": day}
                (Path(folder) / name).write_text(json.dumps(header) + "\n")
            latest = {"@anthropic-ai/claude-code": "2.1.330", "@openai/codex": "0.171.0"}.__getitem__
            found = check_cassettes.check(Path(folder), datetime.date(2026, 11, 23), 14, latest)
        self.assertEqual(found, [
            "stale: a.jsonl was recorded 22 days ago, on 2026-11-01; record it again",
            "newer: claude-code 2.1.330 is out; cassettes were recorded with 2.1.326",
            "unknown: no release lookup for hermes; check it by hand",
        ])

    def test_it_fails_only_when_asked_to(self):
        # A pull request reports what it finds; the nightly run, with --strict, fails on it.
        script = Path(__file__).parent / "check_cassettes.py"
        with tempfile.TemporaryDirectory() as folder:
            header = {"cassette": 1, "harness": "claude-code", "version": "0", "recorded": "2026-11-01"}
            (Path(folder) / "a.jsonl").write_text(json.dumps(header) + "\n")
            run = [sys.executable, script, folder, "--offline", "--today", "2026-11-23"]
            report = subprocess.run(run, capture_output=True, text=True)
            strict = subprocess.run(run + ["--strict"], capture_output=True, text=True)
            fresh = subprocess.run(run[:-1] + ["2026-11-02", "--strict"], capture_output=True, text=True)
        self.assertEqual(report.returncode, 0)
        self.assertIn("stale: a.jsonl", report.stdout)
        self.assertEqual((strict.returncode, strict.stdout), (1, report.stdout))
        self.assertEqual((fresh.returncode, fresh.stdout), (0, ""))

    def test_only_a_stale_cassette_fails_the_nightly_run(self):
        # A finding that is not about age is printed and fails nothing, under --strict too: a harness
        # that drifts fails its replay (D-127). A harness with no release lookup needs no network.
        script = Path(__file__).parent / "check_cassettes.py"
        with tempfile.TemporaryDirectory() as folder:
            header = {"cassette": 1, "harness": "hermes", "version": "1.0", "recorded": "2026-11-20"}
            (Path(folder) / "a.jsonl").write_text(json.dumps(header) + "\n")
            run = [sys.executable, script, folder, "--strict", "--today"]
            fresh = subprocess.run(run + ["2026-11-23"], capture_output=True, text=True)
            stale = subprocess.run(run + ["2026-12-23"], capture_output=True, text=True)
        self.assertEqual(fresh.returncode, 0)
        self.assertIn("unknown: no release lookup for hermes", fresh.stdout)
        self.assertEqual(stale.returncode, 1)
        self.assertIn("stale: a.jsonl", stale.stdout)

    def test_a_folder_that_is_not_there_fails(self):
        # A mistyped folder is an error, even without --strict; an empty one finds nothing.
        script = Path(__file__).parent / "check_cassettes.py"
        with tempfile.TemporaryDirectory() as folder:
            missing = subprocess.run([sys.executable, script, f"{folder}/nothing", "--offline"],
                                     capture_output=True, text=True)
            empty = subprocess.run([sys.executable, script, folder, "--offline", "--strict"],
                                   capture_output=True, text=True)
        self.assertEqual(missing.returncode, 2)
        self.assertIn("not a folder", missing.stderr)
        self.assertEqual((empty.returncode, empty.stdout), (0, ""))

    def test_versions(self):
        self.assertTrue(check_cassettes.newer("2.1.330", "2.1.326"))
        self.assertTrue(check_cassettes.newer("0.172.0", "0.171.0"))
        self.assertFalse(check_cassettes.newer("0.171.0", "0.171.0"))
        self.assertTrue(check_cassettes.newer("1.0.0", "1.0.0-alpha.1"))
        self.assertFalse(check_cassettes.newer("0.9.10", "0.10.0"))
        # Build metadata has no part in precedence; pre-releases of one version compare by their identifiers.
        self.assertFalse(check_cassettes.newer("1.2.3", "1.2.3+build.7"))
        self.assertFalse(check_cassettes.newer("1.2.3+build.8", "1.2.3+build.7"))
        self.assertTrue(check_cassettes.newer("1.2.4", "1.2.3+build.7"))
        self.assertTrue(check_cassettes.newer("1.0.0-beta.2", "1.0.0-beta.1"))
        self.assertTrue(check_cassettes.newer("1.0.0-beta.11", "1.0.0-beta.2"))
        self.assertTrue(check_cassettes.newer("1.0.0-beta", "1.0.0-alpha.1"))
        self.assertTrue(check_cassettes.newer("1.0.0-alpha.1", "1.0.0-alpha"))
        self.assertTrue(check_cassettes.newer("1.0.0-alpha.beta", "1.0.0-alpha.1"))
        self.assertFalse(check_cassettes.newer("1.0.0-rc.1", "1.0.0"))


if __name__ == "__main__":
    unittest.main()
