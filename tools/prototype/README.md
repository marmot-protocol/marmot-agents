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

# Every recording: how many events, the session items, which native call each control was, what was
# not sent, and anything with no rule
python3 tools/prototype/translate.py --check fixtures/recordings/codex fixtures/recordings/claude \
    fixtures/recordings/hermes fixtures/recordings/openclaw fixtures/recordings/acp/opencode
```

It has a translator for each harness that has recordings: Codex, Claude Code, Hermes, OpenClaw, and
any agent over ACP, recorded with OpenCode. It picks one
from what is in the file, not from the path. What each showed is in
[`docs/design/event-model.md`](../../docs/design/event-model.md#checked-against-real-sessions).

`--check` exits with an error if any native message has no rule. Run it after adding a recording. A new kind of
message then needs a decision: an item type, or the not-sent list.

When the event model changes, change this script with it and run `--check` again. If the two disagree, the
recordings decide.

It also writes the events the Codex, Claude Code and ACP drivers are tested against, beside the recordings
([D-074](../../docs/decisions.md#d-074-every-driver-is-tested-by-replay-and-by-one-end-to-end-scenario)):

```sh
python3 tools/prototype/translate.py --expected fixtures/recordings/codex           # write them
python3 tools/prototype/translate.py --expected fixtures/recordings/codex --check   # fail if out of date
python3 tools/prototype/translate.py --expected fixtures/recordings/claude --check  # the same for Claude Code
python3 tools/prototype/translate.py --expected fixtures/recordings/acp/opencode --check  # and for ACP
```

A case that no recording holds is tested with messages written by hand, in
[`test_translate.py`](test_translate.py). Today those are a Codex question marked secret, a question asked in a
message that outlives its turn, a translator that follows one thread, Claude Code's answer to a question with
several choices or with typed text, and what Claude Code's client learns before the session has an id.

```sh
python3 -m unittest discover -s tools/prototype -p 'test_*.py'
```
