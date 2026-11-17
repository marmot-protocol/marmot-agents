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

50 recordings of coding harnesses: 24 with Codex and 26 with Claude Code. Hermes has 14 more and OpenClaw 26,
described below: [Hermes](#hermes), [OpenClaw](#openclaw).

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
| `approval_unanswered` | An approval nobody answers for six minutes, then an interrupt | yes | yes |
| `restart_mid_turn` | The harness is stopped while a command runs, then the session is resumed | yes | yes |

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
- **`approval_unanswered`.** Neither harness gave up. Both waited the full six minutes for an answer, in
  silence, until the recorder interrupted the turn.
- **Codex `restart_mid_turn`.** After the app server was killed, resuming the thread showed the earlier turn as
  interrupted, and the running command was missing from its history.
- **Claude `restart_mid_turn`.** The stop arrived just after the turn had finished and handed the command to the
  background. So the recording shows a background task cut short, not a reply cut short. On resume, Claude Code
  first reported the stopped task and an empty turn, and then answered.
- **Codex `resume`.** The recording has a marker line, `{"_recorder": ...}`, where the app server was stopped
  and started again.

## Hermes

Hermes is a gateway, not a coding harness, so it is recorded differently. A plugin plays the part of a chat
network: it feeds Hermes scripted messages from named senders, and records every call Hermes makes back.
What the recordings show is in [`docs/reference/hermes-messages.md`](../../docs/reference/hermes-messages.md).

| | |
| --- | --- |
| Runtime | Hermes Agent, commit `40d1e95f` (the canary release of 2026-11-17, v0.23.2), in a container |
| Model | `z-ai/glm-5.3-flash` through an OpenAI-compatible endpoint |
| Recorded on | 2026-11-17 |
| Recorder | [`tools/record/hermes`](../../tools/record/README.md#hermes) |

| Scenario | What it exercises |
| --- | --- |
| `simple` | One message in a group, one reply |
| `direct_message` | One message in a direct chat |
| `two_senders` | Two people in one group; the second asks about what the first said |
| `reply` | A reply to another person's earlier message |
| `tool_call` | The agent runs a command |
| `approval_allowed`, `approval_denied` | An approval sent as text, answered with `/approve` and `/deny` |
| `approval_buttons_allowed`, `approval_buttons_denied` | The same, with the platform claiming buttons, answered by a press |
| `question` | A question with choices, answered by a chat message |
| `mid_turn_message` | A second message while the agent is working. Also holds an approval that timed out |
| `stop` | `/stop` during a turn |
| `commands` | `/help` and `/status` |
| `not_allowed` | A message from someone who is not on the allow-list, then one from someone who is |

In these files `via` is one of:

| `via` | With `dir` | Meaning |
| --- | --- | --- |
| `inbound` | `out` | A scripted chat message handed to Hermes |
| `hook` | `in` | Hermes called a method on the platform adapter |
| `result` | `out` | What that method returned to Hermes |
| `press` | `out` | A scripted press of an approval button |
| `note` | `out` | A marker written by the recorder |

About four fifths of the lines are `send_typing` calls. The container held no account, key or relay of ours, so
only paths needed scrubbing.

## OpenClaw

OpenClaw is a gateway too, and is recorded the same way: a plugin plays the part of a chat network. In
OpenClaw the plugin also asks for things: which session a chat belongs to, whether a sender may speak, and for
the turn to run. Those calls and their answers are recorded as well.
What the recordings show is in [`docs/reference/openclaw-messages.md`](../../docs/reference/openclaw-messages.md).

| | |
| --- | --- |
| Runtime | OpenClaw 2026.11.5 (build `e13cf15`), the newest stable release on the day, in a container |
| Model | `z-ai/glm-5.3-flash:free` through an OpenAI-compatible endpoint |
| Recorded on | 2026-11-17 |
| Recorder | [`tools/record/openclaw`](../../tools/record/README.md#openclaw) |

| Scenario | What it exercises |
| --- | --- |
| `simple` | One message in a group, one reply |
| `direct_message` | One message in a direct chat |
| `two_senders` | Two people in one group; the second asks about what the first said |
| `two_direct_messages` | Two people in separate direct chats; the second asks about the first |
| `reply`, `reply_quoted` | A reply to an earlier message, given by its id only, and given with the quoted text |
| `tool_call` | The agent runs a command |
| `tool_call_plain` | The same, with no progress callbacks and OpenClaw's own defaults |
| `reasoning` | The model's thinking, with reasoning set to stream |
| `approval_not_forwarded` | A command needs approval, with OpenClaw's defaults |
| `approval_allowed`, `approval_denied` | The approval forwarded to the chat as a message, answered with `/approve` |
| `approval_cards_allowed`, `approval_cards_denied` | The same, with the plugin claiming an approval capability, answered by a press |
| `approval_press_unclaimed` | A press without that capability, then `/approve` |
| `question`, `question_text_answer` | A question with choices, answered by a press, and by a chat message |
| `mid_turn_message` | A second message while the agent is working |
| `stop` | `/stop` during a turn |
| `commands` | `/help` and `/status` |
| `not_allowed` | A message from someone who is not on the group's allow-list, then one from someone who is |
| `pairing` | A direct message from a stranger, under OpenClaw's pairing policy |
| `not_mentioned` | A group that requires a mention: a message without one, then one with |
| `message_actions`, `message_edit_delete` | The agent uses its message tool to react, send, edit and delete |
| `model_refused` | The model provider refuses the request, twice |

Things that did not go as scripted, and are useful for that reason:

- **`approval_not_forwarded`.** No prompt reached the chat. The command waited until the recording ended.
  An approval reaches a chat only when the owner switches forwarding on, which the other approval scenarios do.
- **`approval_press_unclaimed`.** The press was refused with `approval not found`.
- **`question_text_answer`.** The chat message was not taken as the answer. It waited fifteen minutes until
  the question expired, and then ran as a turn of its own.
- **`question` and `question_text_answer`.** The model's first call of the question tool had the wrong
  arguments and was rejected. The second worked.
- **`mid_turn_message`.** The second message was not folded into the running turn. It waited and ran afterwards.
- **`two_direct_messages`.** Both people were given the same session. The agent declined to repeat what the
  first had said.
- **`message_actions`.** The turn ended after the agent's `send`, before it tried to edit or delete.
- **`message_edit_delete`.** OpenClaw refused the edit before it reached the plugin. The delete went through.
- **`reply`.** Asked to remember three codes, the agent also wrote them to a file, so the recording holds file
  tools as well.
- **`reasoning`.** Thinking text arrived here. In the other scenarios, with reasoning merely on, none did.
- **`approval_denied`.** Recorded twice. The first time the model declined to run the command at all, so
  nothing needed approval; that run was discarded.

In these files `via` is one of:

| `via` | With `dir` | Meaning |
| --- | --- | --- |
| `inbound` | `out` | A scripted chat message handed to OpenClaw |
| `hook` | `in` | OpenClaw called the plugin: an adapter method or a callback |
| `result` | `out` | What that method returned to OpenClaw |
| `call` | `out` | The plugin called OpenClaw: for a route, a decision on the sender, the message context, or to run the turn |
| `call` | `in` | What that call returned |
| `press` | `out` | A scripted press of an approval or question button |
| `note` | `out` | A marker written by the recorder |

A third of the lines are the typing signal and the receive path's own progress log. The container held no
account, key or relay of ours. The recorder never writes OpenClaw's configuration, which is where the model
key is named; it appears as `<config>`. The scrubber replaces the machine's uptime in the output of `/status`.

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
- Claude Code stopped while it is in the middle of writing a reply
- A file attachment other than an image
- A subagent in Codex
- OpenCode, Pi and Goose
- Hermes with media, reactions, scheduled jobs and its message tools
- OpenClaw with media, polls, threads, scheduled jobs, subagents, plugin approvals and the pairing code exchange
