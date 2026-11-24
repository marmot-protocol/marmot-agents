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
        own call (D-146), in place of the typed `/approve` of `new_session.jsonl`, and records what Hermes
        said to it. The answer, the old session's end, the new session and Hermes's reply follow, outside
        any turn."""
        recorded = lines(FOLDER / "new_session.jsonl")
        typed = next(i for i, line in enumerate(recorded)
                     if line.get("via") == "inbound" and line["msg"].get("text") == "/approve")
        press = {"dir": "out", "via": "press", "msg": {
            "confirm_id": "990865760537", "choice": "once", "user": "alice",
            "session_key": "agent:main:recorder:group:family:alice", "said": "✨ Session reset! Starting fresh."}}
        fed = recorded[:typed] + [press]
        translator, made = Translator(), []
        for line in fed:
            made += translator.feed(line)
        hermes = prototype.Hermes()
        for line in fed:
            hermes.feed(line)
        want = [prototype.crate_shape(e) for e in hermes.events]
        self.assertEqual(json.loads(json.dumps(made, ensure_ascii=False)), json.loads(json.dumps(want, ensure_ascii=False)))
        self.assertEqual([(e["item"], e["state"], e.get("turn")) for e in made[-4:]],
                         [("confirm:990865760537", "done", "in-9"), ("session:group:family:alice", "done", None),
                          ("session:group:family:alice#2", "running", None), ("sent:21", "done", None)])


if __name__ == "__main__":
    unittest.main()
