#!/usr/bin/env python3
"""Tests for what no recording holds.

    python3 -m unittest discover -s tools/prototype -p 'test_*.py'

The recordings are the test of the prototype, and `translate.py --check` runs them all. This file
covers cases that were never recorded: a Codex question marked secret, a question asked in a message
that outlives its turn, a translator that follows one thread, Claude Code's answer to a question with
several choices or with typed text, and what Claude Code's client learns before the session has an id.
The messages below are written by hand, in the shapes the recordings have: for Codex, the shape
`item/tool/requestUserInput` has in `fixtures/recordings/codex/plan_mode.jsonl`; for Claude Code, the
shapes in `fixtures/recordings/claude/question.jsonl` and `early_info.jsonl`.
"""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import translate  # noqa: E402

PASSWORD = "hunter2"


def question(secret: bool) -> dict:
    return {
        "dir": "in",
        "msg": {
            "id": 7,
            "method": "item/tool/requestUserInput",
            "params": {
                "threadId": "th1",
                "turnId": "t1",
                "itemId": "call1",
                "questions": [
                    {"id": "name", "header": "Name", "question": "Who are you?", "isOther": True, "isSecret": False, "options": []},
                    {"id": "password", "header": "Password", "question": "Password?", "isOther": True, "isSecret": secret, "options": []},
                ],
            },
        },
    }


ANSWER = {"dir": "out", "msg": {"id": 7, "result": {"answers": {"name": {"answers": ["Alice"]}, "password": {"answers": [PASSWORD]}}}}}
TURN_ENDS = {"dir": "in", "msg": {"method": "turn/completed", "params": {"threadId": "th1", "turn": {"id": "t1", "status": "interrupted", "items": [], "error": None}}}}


def events(*records: dict) -> list[dict]:
    codex = translate.Codex()
    for record in records:
        codex.feed(record)
    return [e for e in codex.events if e["type"] == "question"]


class SecretQuestion(unittest.TestCase):
    def test_a_waiting_secret_question_keeps_the_request_as_its_native_message(self):
        (asked,) = events(question(secret=True))
        self.assertEqual(asked["state"], "waiting")
        self.assertEqual(asked["native"]["payload"]["itemId"], "call1")
        self.assertEqual([q["secret"] for q in asked["body"]["questions"]], [False, True])

    def test_an_answered_secret_question_does_not_repeat_the_answer(self):
        _, answered = events(question(secret=True), ANSWER)
        self.assertEqual(answered["state"], "done")
        self.assertIsNone(answered["native"])
        self.assertEqual(answered["body"]["answer"], [{"question": "name", "options": [], "text": "Alice"}, {"question": "password"}])
        self.assertNotIn(PASSWORD, json.dumps(answered))

    def test_a_cancelled_secret_question_carries_no_native_message(self):
        _, cancelled = events(question(secret=True), TURN_ENDS)
        self.assertEqual(cancelled["state"], "cancelled")
        self.assertIsNone(cancelled["native"])

    def test_a_question_with_nothing_secret_is_unchanged(self):
        _, answered = events(question(secret=False), ANSWER)
        self.assertEqual(answered["body"]["answer"][1], {"question": "password", "options": [], "text": PASSWORD})
        self.assertEqual(answered["native"]["payload"]["answers"]["password"], {"answers": [PASSWORD]})


ASKS_IN_A_MESSAGE = {"dir": "in", "msg": {"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1", "item": {
    "type": "agentMessage", "id": "msg1", "text": "Hello or Hi?", "phase": "final_answer",
    "questions": [{"title": "Which greeting?", "options": ["Hello", "Hi"]}]}}}}
TURN_STARTED = {"dir": "in", "msg": {"method": "turn/started", "params": {"threadId": "th1", "turn": {"id": "t1", "status": "inProgress"}}}}
TURN_DONE = {"dir": "in", "msg": {"method": "turn/completed", "params": {"threadId": "th1", "turn": {"id": "t1", "status": "completed"}}}}


PICK = {"dir": "out", "msg": {"id": 100, "method": "thread/start", "params": {}}}  # the client asks for a session


def thread(tid: str) -> dict:
    """Codex's reply to PICK, which picks the session."""
    return {"dir": "in", "msg": {"id": 100, "result": {"thread": {"id": tid, "cliVersion": "0.169.0"}}}}


class QuestionInAMessage(unittest.TestCase):
    """A question answered by the person's next message waits past the end of its turn."""

    def states(self, *records):
        return [(e["state"], e["session"], e["turn"]) for e in events(*records)]

    def test_the_next_message_answers_it(self):
        nxt = {"dir": "out", "msg": {"id": 3, "method": "turn/start", "params": {"threadId": "th1", "input": []}}}
        self.assertEqual(self.states(PICK, thread("th1"), TURN_STARTED, ASKS_IN_A_MESSAGE, TURN_DONE, nxt),
                         [("waiting", "th1", "t1"), ("done", "th1", "t1")])

    def test_a_new_session_leaves_it_unanswered_where_it_was_asked(self):
        self.assertEqual(self.states(PICK, thread("th1"), TURN_STARTED, ASKS_IN_A_MESSAGE, TURN_DONE, PICK, thread("th2")),
                         [("waiting", "th1", "t1"), ("cancelled", "th1", "t1")])


class QuestionOutlivingItsTurn(unittest.TestCase):
    def test_closed_in_its_own_turn_when_the_harness_stops(self):
        codex = translate.Codex()
        for record in (PICK, thread("th1"), TURN_STARTED, ASKS_IN_A_MESSAGE, TURN_DONE):
            codex.feed(record)
        codex.harness_stopped()
        last = [e for e in codex.events if e["type"] == "question"][-1]
        self.assertEqual((last["state"], last["session"], last["turn"]), ("cancelled", "th1", "t1"))


class OneThread(unittest.TestCase):
    """A translator follows the chat's session. The Rust driver has the same cases in `tests/cases.rs`."""

    def test_a_message_for_another_thread_makes_no_event_and_is_counted(self):
        def warn(tid):
            return {"dir": "in", "msg": {"method": "warning", "params": {"threadId": tid, "message": "careful"}}}

        codex = translate.Codex()
        for record in (PICK, thread("th1"), warn("th1"), PICK, thread("th2"), warn("th2"), warn("th1")):
            codex.feed(record)
        self.assertEqual([(e["session"], e["seq"]) for e in codex.events],
                         [("th1", 1), ("th1", 2), ("th1", 3), ("th2", 1), ("th2", 2)])
        self.assertEqual(codex.other_threads, 1)

    def test_a_session_the_chat_goes_back_to_runs_again(self):
        codex = translate.Codex()
        for record in (PICK, thread("th1"), PICK, thread("th2"), PICK, thread("th1")):
            codex.feed(record)
        self.assertEqual([e["state"] for e in codex.events if e["item"] == "session:th1"], ["running", "done", "running"])


    def test_a_refused_request_stays_where_it_was_sent(self):
        update = {"dir": "out", "msg": {"id": 7, "method": "turn/settings/update",
                                        "params": {"threadId": "th1", "turnId": "t1", "model": "m2"}}}
        refused = {"dir": "in", "msg": {"id": 7, "error": {"code": -32600, "message": "no such turn"}}}
        for later in ([TURN_DONE], [TURN_DONE, PICK, thread("th2")]):
            codex = translate.Codex()
            for record in (PICK, thread("th1"), TURN_STARTED, update, *later, refused):
                codex.feed(record)
            last = codex.events[-1]
            self.assertEqual((last["type"], last["session"], last["turn"]), ("error", "th1", "t1"))

    def test_an_mcp_approval_names_the_tool_it_is_for(self):
        def tool(iid, name):
            return {"dir": "in", "msg": {"method": "item/started", "params": {"threadId": "th1", "turnId": "t1", "item": {
                "type": "mcpToolCall", "id": iid, "server": "demo", "tool": name, "status": "inProgress"}}}}

        def approval(rid, **meta):
            return {"dir": "in", "msg": {"id": rid, "method": "mcpServer/elicitation/request", "params": {
                "threadId": "th1", "turnId": "t1", "serverName": "demo", "message": "Allow?",
                "_meta": {"codex_approval_kind": "mcp_tool_call", **meta}}}}

        codex = translate.Codex()
        # Without the tool's name there are two it could be, and it names neither.
        for record in (TURN_STARTED, tool("a", "define_word"), tool("b", "spell_word"),
                       approval(0, tool_name="spell_word"), approval(1)):
            codex.feed(record)
        self.assertEqual([e["body"]["subject"] for e in codex.events if e["type"] == "approval"], ["b", None])

    def test_a_turn_the_old_session_was_asked_for_is_not_the_new_sessions(self):
        compact = {"dir": "out", "msg": {"id": 8, "method": "thread/compact/start", "params": {"threadId": "th1"}}}
        turn2 = {"dir": "in", "msg": {"method": "turn/started", "params": {"threadId": "th2", "turn": {"id": "t2"}}}}
        codex = translate.Codex()
        for record in (PICK, thread("th1"), compact, PICK, thread("th2"), turn2):
            codex.feed(record)
        self.assertEqual(codex.events[-1]["body"]["started_by"], "agent")

    def test_token_usage_for_another_thread_is_not_counted_in_this_turn(self):
        def usage(tid, out):
            return {"dir": "in", "msg": {"method": "thread/tokenUsage/updated", "params": {
                "threadId": tid, "turnId": "t1",
                "tokenUsage": {"total": {"inputTokens": 0, "outputTokens": out}, "last": {"inputTokens": 0, "outputTokens": out}}}}}

        codex = translate.Codex()
        for record in (PICK, thread("th1"), TURN_STARTED, usage("th9", 5000), usage("th1", 40), TURN_DONE):
            codex.feed(record)
        self.assertEqual(codex.events[-1]["body"]["usage"]["output_tokens"], 40)
        self.assertEqual(codex.other_threads, 1)



class ApprovalChoices(unittest.TestCase):
    """A choice says what it does only when the driver knows the decision. The Rust driver has the same
    case in `tests/send.rs`."""

    def test_a_choice_does_what_its_decision_does(self):
        rule = lambda action: {"applyNetworkPolicyAmendment": {"network_policy_amendment": {"action": action, "host": "example.org"}}}  # noqa: E731
        codex = translate.Codex()
        codex.feed({"dir": "in", "msg": {"id": 0, "method": "item/commandExecution/requestApproval", "params": {
            "threadId": "th1", "turnId": "t1", "command": "curl example.org",
            "availableDecisions": ["accept", rule("allow"), rule("deny"), {"somethingNew": {}}, "cancel"]}}})
        choices = [(c["id"], c["label"], c["effect"]) for c in codex.events[-1]["body"]["choices"]]
        self.assertEqual(choices, [("c0", "Allow once", "allow"), ("c1", "Always allow example.org", "allow"),
                                   ("c2", "Always deny example.org", "deny"), ("c4", "Deny and stop", "deny"),
                                   ("c5", "Deny", "deny")])


class ClaudeQuestionAnswer(unittest.TestCase):
    """Claude Code answers a question with each question's text and the labels chosen. The item records
    ids instead (D-059). Only a single choice is recorded, in `fixtures/recordings/claude/question.jsonl`."""

    QUESTIONS = [
        {"question": "Which files?", "header": "Files", "multiSelect": True,
         "options": [{"label": "a.py", "description": ""}, {"label": "b.py", "description": ""}, {"label": "c.py", "description": ""}]},
        {"question": "Which greeting?", "header": "Greeting", "multiSelect": False,
         "options": [{"label": "Hello", "description": ""}, {"label": "Hi", "description": ""}]},
        {"question": "Anything else?", "header": "More", "multiSelect": False, "options": [{"label": "No", "description": ""}]},
    ]

    def answer(self, answers: dict) -> list:
        claude = translate.Claude()
        claude.feed({"dir": "in", "via": "callback", "msg": {
            "kind": "can_use_tool", "toolName": "AskUserQuestion", "input": {"questions": self.QUESTIONS},
            "options": {"toolUseID": "toolu_1", "requestId": "r1"}}})
        claude.feed({"dir": "out", "via": "callback", "msg": {
            "kind": "permission_result", "toolName": "AskUserQuestion",
            "result": {"behavior": "allow", "updatedInput": {"questions": self.QUESTIONS, "answers": answers}}}})
        return translate.crate_shape(claude.events[-1])["body"]["answer"]

    def test_labels_become_the_ids_of_the_options_offered(self):
        self.assertEqual(self.answer({"Which files?": "a.py, c.py", "Which greeting?": "Hi"}),
                         [{"question": "q0", "options": ["o0", "o2"]}, {"question": "q1", "options": ["o1"]}])

    def test_what_matches_no_label_is_typed_text(self):
        self.assertEqual(self.answer({"Which files?": "b.py, d.py", "Which greeting?": "Howdy"}),
                         [{"question": "q0", "options": ["o1"], "text": "d.py"}, {"question": "q1", "text": "Howdy"}])


class ClaudeSessionItem(unittest.TestCase):
    """A session item carries the session's id (D-030). Claude Code gives it only when the first turn
    starts, so what the client asks for before then waits for it."""

    def test_nothing_is_sent_before_the_session_is_named(self):
        claude = translate.Claude()
        claude.feed({"dir": "out", "via": "call", "msg": {"method": "query", "options": {}}})
        claude.feed({"dir": "in", "via": "call", "msg": {"method": "supportedCommands", "response": [
            {"name": "compact", "description": "Compact", "argumentHint": "<instructions>"}, {"name": "cost", "description": "Cost"}]}})
        self.assertEqual(claude.events, [])
        claude.feed({"dir": "in", "via": "message", "msg": {
            "type": "system", "subtype": "init", "session_id": "s1", "claude_code_version": "9.9.9",
            "model": "m", "permissionMode": "default", "slash_commands": ["compact", "cost"]}})
        session = translate.crate_shape(claude.events[0])
        self.assertEqual((session["type"], session["session"]), ("session", "s1"))
        self.assertEqual(session["body"]["harness_version"], "9.9.9")
        self.assertEqual(session["body"]["commands"], [{"name": "compact", "description": "Compact", "hint": "<instructions>"},
                                                       {"name": "cost", "description": "Cost", "hint": ""}])

    def test_a_command_known_by_name_only_has_an_empty_hint(self):
        claude = translate.Claude()
        claude.feed({"dir": "in", "via": "message", "msg": {
            "type": "system", "subtype": "init", "session_id": "s1", "claude_code_version": "9.9.9", "slash_commands": ["cost"]}})
        self.assertEqual(translate.crate_shape(claude.events[0])["body"]["commands"], [{"name": "cost", "hint": ""}])


class ClaudeApprovals(unittest.TestCase):
    """A reply settles the approval it answers, by the callback's request id when the driver recorded
    it, and records the choice the person made."""

    @staticmethod
    def ask(claude, request, command, suggestions=()):
        claude.feed({"dir": "in", "via": "callback", "msg": {
            "kind": "can_use_tool", "toolName": "Bash", "input": {"command": command},
            "options": {"toolUseID": f"tool-{request}", "requestId": request, "suggestions": list(suggestions)}}})

    @staticmethod
    def reply(claude, request, choice, behavior="allow"):
        claude.feed({"dir": "out", "via": "callback", "msg": {
            "kind": "permission_result", "toolName": "Bash", "requestId": request, "choice": choice,
            "result": {"behavior": behavior}}})
        return translate.crate_shape(claude.events[-1])

    def test_two_waiting_for_one_tool_are_answered_each_by_its_own_reply(self):
        claude = translate.Claude()
        self.ask(claude, "r1", "ls")
        self.ask(claude, "r2", "pwd")
        self.assertEqual(self.reply(claude, "r2", "c0")["item"], "approval:r2")
        self.assertEqual(self.reply(claude, "r1", "c0")["item"], "approval:r1")

    def test_a_standing_choice_is_recorded_as_that_choice(self):
        claude = translate.Claude()
        self.ask(claude, "r1", "ls", suggestions=[{"type": "addRules"}])
        answered = self.reply(claude, "r1", "c1")
        self.assertEqual(answered["body"]["answer"], {"choice": "c1"})
        self.assertEqual(answered["text"], "Answered: Allow, and don't ask again")


class ClaudeResumedTurns(unittest.TestCase):
    """A driver that restarts resumes the session with a fresh translator. Its turns must not take the
    item ids of the turns before, or they would edit those turns' rows."""

    @staticmethod
    def first_turn(uuid):
        claude = translate.Claude()
        claude.feed({"dir": "in", "via": "message", "msg": {
            "type": "system", "subtype": "init", "session_id": "s1", "uuid": uuid, "claude_code_version": "9.9.9"}})
        return next(e["item"] for e in claude.events if e["type"] == "turn")

    def test_a_resumed_sessions_first_turn_has_its_own_id(self):
        self.assertNotEqual(self.first_turn("u-before"), self.first_turn("u-after"))



if __name__ == "__main__":
    unittest.main()
