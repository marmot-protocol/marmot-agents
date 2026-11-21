"""Every Hermes recording, replayed through the translator, must give the events the prototype gives
for it (`fixtures/recordings/hermes/expected`, written by `tools/prototype/translate.py --expected`).
The two were written apart, from the same design; where they disagree, the recording and the event
model decide which is wrong (D-074)."""

import json
import unittest
from pathlib import Path

from marmot_hermes.translate import Translator

FOLDER = Path(__file__).resolve().parents[2] / "fixtures" / "recordings" / "hermes"


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


if __name__ == "__main__":
    unittest.main()
