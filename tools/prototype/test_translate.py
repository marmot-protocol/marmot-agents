#!/usr/bin/env python3
"""Tests for what no recording holds.

    python3 -m unittest discover -s tools/prototype -p 'test_*.py'

The recordings are the test of the prototype, and `translate.py --check` runs them all. This file
covers a case that was never recorded: a Codex question marked secret. The messages below are
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


if __name__ == "__main__":
    unittest.main()
