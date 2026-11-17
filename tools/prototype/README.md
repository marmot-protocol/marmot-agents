# Prototype translator

`translate.py` turns a recorded harness session into the agent events proposed in
[`docs/design/event-model.md`](../../docs/design/event-model.md).

It is a design tool, not driver code. The real drivers will be written in Rust and TypeScript. This script
exists so that the event model is tested against real recordings: every field of an agent event has to come from
somewhere, and every native message has to be either translated or deliberately not sent.

```sh
# One recording, readable at a glance
python3 tools/prototype/translate.py fixtures/recordings/codex/approval_allowed.jsonl --brief

# One recording, in full, with ids and native payloads
python3 tools/prototype/translate.py fixtures/recordings/claude/question.jsonl

# Every recording: how many events, what was not sent, and anything with no rule
python3 tools/prototype/translate.py --check fixtures/recordings/codex fixtures/recordings/claude
```

`--check` exits with an error if any native message has no rule. Run it after adding a recording. A new kind of
message then needs a decision: an item type, or the not-sent list.

When the event model changes, change this script with it and run `--check` again. If the two disagree, the
recordings decide.
