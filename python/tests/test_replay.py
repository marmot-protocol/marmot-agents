"""Every Hermes recording, replayed through the translator, must give the events the prototype gives
for it (`fixtures/recordings/hermes/expected`, written by `tools/prototype/translate.py --expected`).
The two were written apart, from the same design; where they disagree, the recording and the event
model decide which is wrong (D-074)."""

import json
import sys
import unittest
from pathlib import Path

from marmot_hermes.translate import Translator

FOLDER = Path(__file__).resolve().parents[2] / "fixtures" / "recordings" / "hermes"
sys.path.insert(0, str(FOLDER.parents[2] / "tools" / "prototype"))
import translate as prototype  # noqa: E402


def lines(path):
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


class Replay(unittest.TestCase):
    def test_the_recordings_are_here(self):
        self.assertTrue(sorted(FOLDER.glob("*.jsonl")), "no recordings found")

    def test_each_recording_gives_the_prototypes_events(self):
        for path in sorted(FOLDER.glob("*.jsonl")):
            with self.subTest(recording=path.name):
                translator = Translator()
                made = []
                for line in lines(path):
                    for event in translator.feed(line):
                        if "native" in event:
                            # The native message is the whole line Hermes's call was recorded as (D-059).
                            self.assertIs(event["native"]["payload"], line["msg"])
                        made.append(event)
                self.assertEqual(dict(translator.unknown), {}, "every line has a rule")
                want = lines(FOLDER / "expected" / path.name)
                self.assertEqual(json.loads(json.dumps(made, ensure_ascii=False)), want)


    def test_a_confirmation_answered_by_its_id_gives_the_prototypes_events(self):
        """No recording holds it: the plugin answers Hermes's request to confirm `/new` through Hermes's
        own call (D-146), in place of the typed `/approve` of `new_session.jsonl`, and records that Hermes
        took it and, once the reset is done, what Hermes said to it. The answer, the old session's end, the new session and Hermes's reply follow, outside
        any turn. Bob, in the same group, has a turn running when Alice's answer is taken: what follows
        is his turn's, and then Alice's own, as recorded."""
        recorded = lines(FOLDER / "new_session.jsonl")
        typed = next(i for i, line in enumerate(recorded)
                     if line.get("via") == "inbound" and line["msg"].get("text") == "/approve")
        after = next(i for i, line in enumerate(recorded) if i > typed and line.get("via") == "inbound")
        # Hermes takes the answer, and once the reset is done, the plugin records what Hermes said to it.
        press = [{"dir": "out", "via": "press", "msg": {
                     "confirm_id": "990865760537", "choice": "once", "user": "alice",
                     "session_key": "agent:main:recorder:group:family:alice"}},
                 {"dir": "out", "via": "press", "msg": {
                     "confirm_id": "990865760537", "session_key": "agent:main:recorder:group:family:alice",
                     "said": "✨ Session reset! Starting fresh."}}]
        source = {**recorded[typed + 1]["msg"]["params"]["event"]["source"], "user_id": "bob", "user_name": "Bob",
                  "message_id": "in-b"}
        bob = [{"dir": "out", "via": "inbound", "msg": {"message_id": "in-b", "chat": "family", "user": "bob",
                                                          "user_name": "Bob", "text": "Hello."}},
               {"dir": "in", "via": "hook", "msg": {"method": "on_processing_start", "params": {"event": {
                   "_type": "MessageEvent", "text": "Hello.", "message_type": "text", "source": source,
                   "message_id": "in-b"}}}}]
        bob_ends = [{"dir": "in", "via": "hook", "msg": {"method": "send", "params": {
                        "chat_id": "family", "content": "Hi, Bob.", "reply_to": "in-b", "metadata": {"notify": True}}}},
                    {"dir": "in", "via": "hook", "msg": {"method": "on_processing_complete", "params": {
                        "message_id": "in-b", "outcome": "success"}}}]
        fed = recorded[:typed] + bob + press + bob_ends + recorded[after:]
        translator, made = Translator(), []
        for line in fed:
            made += translator.feed(line)
        hermes = prototype.Hermes()
        for line in fed:
            hermes.feed(line)
        want = [prototype.crate_shape(e) for e in hermes.events]
        self.assertEqual(json.loads(json.dumps(made, ensure_ascii=False)), json.loads(json.dumps(want, ensure_ascii=False)))
        answered = next(i for i, e in enumerate(made) if e["item"] == "confirm:990865760537" and e["state"] == "done")
        self.assertEqual([(e["item"], e["session"], e.get("turn")) for e in made[answered:answered + 4]],
                         [("confirm:990865760537", "group:family:alice", "in-9"),
                          ("session:group:family:alice", "group:family:alice", None),
                          ("session:group:family:alice#2", "group:family:alice#2", None),
                          ("sent:23", "group:family:alice#2", None)])
        bobs = [(e["item"], e["session"], e.get("turn")) for e in made[answered + 4:] if e.get("turn") == "in-b"]
        self.assertEqual(bobs, [("sent:26", "group:family:bob", "in-b"), ("turn:in-b", "group:family:bob", "in-b")])
        alices = {e["session"] for e in made[answered + 4:] if e.get("turn") not in (None, "in-b")}
        self.assertEqual(alices, {"group:family:alice#2"}, "Alice's next turns are in her new session")


if __name__ == "__main__":
    unittest.main()
