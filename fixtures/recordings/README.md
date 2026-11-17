# Recorded harness sessions

These are real sessions with Codex and Claude Code, recorded message by message in both directions. They exist
so that the event model and the drivers are checked against what the harnesses actually send, not against what
we assume they send.

What the recordings show, and what it means for the design, is written up in
[`docs/reference/harness-messages.md`](../../docs/reference/harness-messages.md).

## What was recorded

| | Codex | Claude Code |
| --- | --- | --- |
| Interface | `codex app-server`, JSON-RPC over stdio | Claude Agent SDK, `query()` |
| Version | codex-cli 0.169.0 | `@anthropic-ai/claude-agent-sdk` 0.3.321, Claude Code 2.1.321 |
| Model | `gpt-6.1-sol`, low effort | `claude-haiku-5-5` |
| Recorded on | 2026-11-17 | 2026-11-17 |
| Recorder | [`tools/record/record_codex.py`](../../tools/record/record_codex.py) | [`tools/record/claude/record_claude.ts`](../../tools/record/claude/record_claude.ts) |

Every session ran in a throwaway copy of [`tools/record/demo`](../../tools/record/demo): three small files.
The prompts are scripted and are in the recorder sources.

## Scenarios

46 recordings: 22 with Codex and 24 with Claude Code.

| Scenario | What it exercises | Codex | Claude |
| --- | --- | --- | --- |
| `simple` | A text answer, no tools | yes | yes |
| `streaming` | The same, with partial messages on | always on | yes |
| `command` | A shell command | yes | yes |
| `file_edit` | A file edit, no approval needed | yes | yes |
| `failing_command` | A command that exits with an error | yes | yes |
| `approval_allowed` | A command needs approval; allowed | yes | yes |
| `approval_denied` | A command needs approval; declined | yes | yes |
| `file_approval` | A file change needs approval; allowed | yes | yes |
| `question` | The agent asks a multiple-choice question | as a message | yes |
| `plan_mode` | Plan mode: a question, then a plan | yes | yes |
| `plan`, `plan_tool` | The agent is asked to keep a to-do list | no list appeared | no list appeared |
| `reasoning` | The agent's thinking | empty | yes |
| `interrupt` | The user interrupts a running command | yes | yes |
| `steer` | The user sends a message mid-turn | yes | yes |
| `two_turns` | Two turns in one session | yes | yes |
| `resume` | A session resumed after a restart | yes | yes |
| `compact` | The conversation is compacted between turns | yes | yes |
| `model_switch` | The model is changed between turns | yes | yes |
| `slash_command` | The commands are listed and one is run | not in this interface | yes |
| `subagent` | The agent hands work to a subagent | not recorded | yes |
| `mcp_tool` | A tool on an MCP server | yes, with approval | yes |
| `web_search` | A web search | yes | yes |
| `image` | The user attaches an image | yes | yes |
| `bad_model` | A model that does not exist | yes | yes |

Things that did not go as scripted, and are useful for that reason:

- **Codex `question`.** In its default mode Codex did not use a question request. It sent an ordinary message
  that carries a `questions` field, then waited with three 60-second `sleep` items and gave up. The real
  question request appears in `plan_mode`.
- **`plan` and `plan_tool`.** Neither harness produced a to-do list, even when told to. Both simply did the
  work. A plan as text appears in `plan_mode`.
- **Codex `reasoning`.** The reasoning items are empty, at high effort with detailed summaries requested.
- **Claude thinking.** Thinking blocks are empty in every recording except `reasoning`, which switches
  summarised thinking on.
- **Claude `steer`.** The message sent mid-turn ended the running turn and started a new one.
- **Claude `subagent`.** The subagent ran in the background. The turn finished while it was still working, and
  more messages and a second result arrived afterwards.
- **Claude `interrupt` and `bad_model`.** The turn ends with an error result, and the SDK's `query()` throws.
  For `bad_model` the result's subtype is `success` with `is_error` set.
- **Codex `file_approval`.** Codex asked to run a command as well as to change the file, so the recording has
  two approvals.
- **Codex `mcp_tool`.** The approval arrives as an MCP elicitation request. With approvals switched off, Codex
  refuses the call.
- **Codex `compact`.** Compaction runs as a turn of its own.
- **Codex `resume`.** The recording has a marker line, `{"_recorder": ...}`, where the app server was stopped
  and started again.

## File format

One JSON object per line.

| Field | Meaning |
| --- | --- |
| `t` | Milliseconds since the recording started |
| `dir` | `out`: the client sent it to the harness. `in`: the harness sent it to the client |
| `via` | Claude only. How it travelled; see below |
| `msg` | The message itself, unchanged apart from scrubbing |

Codex messages are JSON-RPC: a request has `id` and `method`, a notification has `method` only, and a response
has `id` with `result` or `error`.

For Claude, `via` is one of:

| `via` | With `dir` | Meaning |
| --- | --- | --- |
| `message` | `in` | A message yielded by `query()` |
| `callback` | `in` | A permission request delivered to `canUseTool` |
| `callback` | `out` | The client's answer to that request |
| `prompt` | `out` | A user message the client pushed into the session |
| `call` | `out` | A control method the client called, such as `query` or `interrupt` |
| `call` | `in` | What that call returned |

## What was scrubbed

The recorders write raw files that contain local paths and account details.
[`tools/record/scrub.py`](../../tools/record/scrub.py) produced the files here. It:

- replaced local paths with `/work/demo`, `/work/tmp` and `/home/user`, and the machine and user names with
  `host` and `user`;
- neutralised account and machine details: plan, rate limits, installation id, user agent, credential expiry
  and moderation metadata. The messages are still present, with strings set to `redacted` and numbers to `0`;
- emptied Claude's list of connected MCP servers and removed MCP tool names from its tool list. This also
  removes the demo server from the start-up message of the `mcp_tool` recording; the tool call itself is intact;
- replaced thinking signatures with `removed`.

Session, thread, turn and item ids are real. The text of prompts, replies and tool output is real; it is about
the demo project only.

The scrubber scans its own output and fails if a path, e-mail address, host name or user name is left. Run it
again after any new recording, and read the result before committing.

## Not recorded yet

- A to-do list that changes during a turn
- A rate limit, an expired sign-in, and a harness crash
- A file attachment other than an image
- A subagent in Codex
- OpenCode, Pi, Goose, Hermes and OpenClaw
