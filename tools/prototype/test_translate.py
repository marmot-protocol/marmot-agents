#!/usr/bin/env python3
"""Tests for what no recording holds.

    python3 -m unittest discover -s tools/prototype -p 'test_*.py'

The recordings are the test of the prototype, and `translate.py --check` runs them all. This file
covers cases that were never recorded: a Codex question marked secret, a question asked in a message
that outlives its turn, and a translator that follows one thread. The messages below are
written by hand, in the shape `item/tool/requestUserInput` has in
`fixtures/recordings/codex/plan_mode.jsonl`.
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


if __name__ == "__main__":
    unittest.main()
