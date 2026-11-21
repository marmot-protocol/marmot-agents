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

57 recordings of coding harnesses: 28 with Codex and 29 with Claude Code. Hermes has 27 more and OpenClaw 39,
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
| `subagent` | The agent hands work to a subagent | in [`codex/subagents`](#codex-subagents) | yes |
| `mcp_tool` | A tool on an MCP server | yes, with approval | yes |
| `web_search` | A web search | yes | yes |
| `image` | The user attaches an image | yes | yes |
| `bad_model` | A model that does not exist | yes | yes |
| `approval_unanswered` | An approval nobody answers for six minutes, then an interrupt | yes | yes |
| `restart_mid_turn` | The harness is stopped while a command runs, then the session is resumed | yes | yes |
| `new_session` | A second session for the same project, without resuming the first | yes | yes |
| `settings_update` | The client changes the model, then the approval policy and the sandbox, between turns | yes | |
| `settings_mid_turn` | The client changes the reasoning effort of a turn while it runs | yes, refused | |
| `resume_with_settings` | A session is resumed, with the approval policy and the sandbox sent again | yes | |
| `permission_mode_switch` | The client changes the permission mode between two turns | | yes |
| `early_info` | Before any prompt, the client asks what the session offers and waits five seconds | | yes |

The last six rows were recorded on 2026-11-18, to check the three shared controls and the `session` item.

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

## Codex subagents

Six more Codex sessions, in [`codex/subagents`](codex/subagents), recorded on 2026-11-20 with codex-cli 0.169.0.
Each prompt asks Codex to use subagents. They run on `gpt-6.1-sol` at low effort, except `subagent_v1`, which
uses `gpt-5.6-luna`, the one model offered that runs Codex's first multi-agent runtime. The threads are kept
(`ephemeral: false`), because a subagent could not start from a throwaway thread, so each run left its threads
in the recording machine's Codex history.

| Scenario | What it exercises |
| --- | --- |
| `subagent` | One subagent, waited for |
| `subagent_v1` | The same, on the first multi-agent runtime |
| `subagent_nested` | A subagent that starts one of its own |
| `subagent_approval` | A subagent's command needs approval; allowed |
| `subagent_interrupt` | The parent's turn is interrupted while the subagent runs a command |
| `subagent_continue` | A later turn sends more work to the same subagent |

They are in a folder of their own because neither the prototype nor the Codex driver translates subagents yet:
how a chat shows them is being designed
([Q-30](../../docs/open-questions.md#q-30-how-does-a-chat-show-an-agents-subagents)). What they show is in
[harness messages](../../docs/reference/harness-messages.md#subagents).

## Hermes

Hermes is a gateway, not a coding harness, so it is recorded differently. A plugin plays the part of a chat
network: it feeds Hermes scripted messages from named senders, and records every call Hermes makes back.
What the recordings show is in [`docs/reference/hermes-messages.md`](../../docs/reference/hermes-messages.md).

| | |
| --- | --- |
| Runtime | Hermes Agent, commit `40d1e95f` (the canary release of 2026-11-17, v0.23.2), in a container |
| Model | `z-ai/glm-5.3-flash` through an OpenAI-compatible endpoint. On 2026-11-18 its free route, `z-ai/glm-5.3-flash:free` |
| Recorded on | 2026-11-17: the first 14. 2026-11-18: the 13 under [Controls and sessions](#hermes-controls-and-sessions) |
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

<a id="hermes-controls-and-sessions"></a>

**Controls and sessions**, recorded on 2026-11-18. In these the plugin also registers nine plugin hooks, such
as `on_session_reset` and `pre_command`. What they show is in
[`docs/reference/hermes-messages.md`](../../docs/reference/hermes-messages.md#controls-and-sessions).

| Scenario | What it exercises |
| --- | --- |
| `new_session` | A turn, `/new`, and a turn that asks what the agent remembers |
| `model_switch` | `/model` to list, `/model <id>` to switch to another model, a turn, `/model reset` |
| `model_picker` | The same switch, with the platform claiming a model picker, made by a pick |
| `setting_reasoning` | `/reasoning` to show, `/reasoning low` to change, a turn |
| `yolo` | `/yolo`, a command that would need approval, `/yolo` again, the same command |
| `approvals_off` | `/approvals off`, a command that would need approval, `/approvals manual` |
| `approve_session` | An approval answered with `/approve session`, then the same command again |
| `approve_always` | An approval answered with `/approve always`, then the same command from another person |
| `write_approval` | `/memory approval` on and off, `/skills approval`, and three commands that only show a setting |
| `command_not_admin` | Hermes's own list of admins for slash commands: one person refused five commands, one allowed |
| `control_catalog` | The plugin asks Hermes for its commands and a chat's model settings as data, then `/commands`, `/model`, `/reasoning`, `/approvals`, `/whoami` |
| `approvals_unordered` | Five commands sent before the first was answered |
| `smart_approved` | A command that would need approval, with Hermes's default approval mode |

Things that did not go as scripted, and are useful for that reason:

- **`new_session`.** `/new` did not start a session. Hermes asked for confirmation first, and the scenario
  answers `/approve`. The agent then still knew the code word, because it had saved it to its memory.
- **`approvals_unordered`.** Recorded as `approvals_off`. Hermes took 19 seconds to answer the first command,
  the recorder did not wait, and the five commands were answered out of order. The approval mode ended as
  `manual`, so the command was asked about, and the approval timed out. `approvals_off` was recorded again
  with a wait for each reply. The file was renamed, and the recorder's own note in it with the name.
- **`smart_approved`.** Recorded as `approve_session`. No approval reached the chat: Hermes's default mode,
  `smart`, has a second model decide, and it approved the command. The scenario then waited ten minutes for a
  prompt that never came. `approve_session` and `approve_always` were recorded again with the mode set to
  `manual`. The file was renamed in the same way.
- **`model_picker`.** The list Hermes handed over held one model. The pick was a model that was not on it,
  and Hermes accepted it.

In these files `via` is one of:

| `via` | With `dir` | Meaning |
| --- | --- | --- |
| `inbound` | `out` | A scripted chat message handed to Hermes |
| `hook` | `in` | Hermes called a method on the platform adapter |
| `result` | `out` | What that method returned to Hermes |
| `plugin_hook` | `in` | Hermes called a plugin hook that the recorder registered |
| `call` | `out` | The plugin asked Hermes for something as data: its commands, or a chat's model settings |
| `call` | `in` | What that returned. Also what the model picker's callback returned |
| `press` | `out` | A scripted press of an approval button, or a pick from the model picker |
| `note` | `out` | A marker written by the recorder |

About four fifths of the lines of the first 14 recordings are `send_typing` calls. The container held no
account, key or relay of ours, so only paths needed scrubbing. The recordings of 2026-11-18 name the model
endpoint, which is also the default in the recorder's script.

## OpenClaw

OpenClaw is a gateway too, and is recorded the same way: a plugin plays the part of a chat network. In
OpenClaw the plugin also asks for things: which session a chat belongs to, whether a sender may speak, and for
the turn to run. Those calls and their answers are recorded as well.
What the recordings show is in [`docs/reference/openclaw-messages.md`](../../docs/reference/openclaw-messages.md).

| | |
| --- | --- |
| Runtime | OpenClaw 2026.11.5 (build `e13cf15`), the newest stable release on 2026-11-17, in a container |
| Model | `z-ai/glm-5.3-flash:free` through an OpenAI-compatible endpoint |
| Recorded on | 2026-11-17: the first 26. 2026-11-18: the 13 under [Controls and sessions](#openclaw-controls-and-sessions) |
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

<a id="openclaw-controls-and-sessions"></a>

**Controls and sessions**, recorded on 2026-11-18. In these the plugin also registers three typed plugin
hooks: `session_start`, `session_end` and `before_reset`. What they show is in
[`docs/reference/openclaw-messages.md`](../../docs/reference/openclaw-messages.md#controls-and-sessions).

| Scenario | What it exercises |
| --- | --- |
| `new_session`, `reset_session` | A turn, `/new` or `/reset`, and a turn that asks what the agent remembers |
| `model_switch` | `/models` to list, `/model <provider/model>` to switch to another model, a turn, `/model default`, and a model that does not exist |
| `settings` | `/think high`, `/verbose off`, `/reasoning off`, a turn, and a value that does not exist |
| `elevated` | `/elevated full` from someone not allowed to use it and from someone who is, then a command that needs approval |
| `exec_directive` | `/exec security=full ask=off` in front of a request for a command that needs approval, and alone |
| `exec_directive_once` | The same directive and request, then the request again without the directive |
| `exec_directive_unauthorized` | The same directive and request from someone who may not use commands |
| `exec_directive_mid_message` | The same directive at the end of a request, then in the middle of one |
| `approve_always` | An approval answered with `allow-always`, then the same command from another person |
| `owner_commands` | Twelve commands that are owner-only or switched off, from someone who is not the owner |
| `owner_config` | The same person made the owner: `/debug set` and `/config set` on the command mode |
| `control_catalog` | The plugin asks OpenClaw for its commands, its models and a session's stored entry as data, then ten commands that show a setting |

Things that did not go as scripted, and are useful for that reason:

- **`new_session`, `reset_session`.** The session started afresh, and neither its key nor its id changed.
- **`exec_directive`.** The directive worked: the command ran with no approval. The scenario then waited four
  minutes for a prompt, and its `/approve unknown deny` was answered as if it were about something else.
- **`elevated`.** `/elevated full` was accepted and the approval was still asked. The agent in these
  recordings is not sandboxed.
- **`approve_always`.** The model first chained the command with another, and OpenClaw refused that outright
  with no approval. Its second try was the plain command.
- **`model_switch`.** OpenClaw accepted a model that is not configured.
- **`owner_config`.** `/debug set` on the command mode was accepted and changed nothing. `/config set` on the
  same value did.
- **`exec_directive_unauthorized`.** The message was not treated as plain text. OpenClaw dropped it whole,
  with no reply.
- **`exec_directive_mid_message`.** The directive worked in both places: each command ran with no approval.
  The second turn then ended with OpenClaw's "the request took too long" after the command had run.

In these files `via` is one of:

| `via` | With `dir` | Meaning |
| --- | --- | --- |
| `inbound` | `out` | A scripted chat message handed to OpenClaw |
| `hook` | `in` | OpenClaw called the plugin: an adapter method or a callback |
| `result` | `out` | What that method returned to OpenClaw |
| `call` | `out` | The plugin called OpenClaw: for a route, a decision on the sender, the message context, or to run the turn. Also, through the plugin SDK, for its commands, its models or a session's stored entry |
| `call` | `in` | What that call returned |
| `plugin_hook` | `in` | OpenClaw called a typed plugin hook that the recorder registered |
| `press` | `out` | A scripted press of an approval or question button |
| `note` | `out` | A marker written by the recorder |

A third of the lines of the first 26 recordings are the typing signal and the receive path's own progress
log. The container held no account, key or relay of ours. The recorder never writes OpenClaw's configuration,
which is where the model key is named; it appears as `<config>`. The scrubber replaces the machine's uptime in
the output of `/status`.

In the recordings of 2026-11-18 a stored session entry is written without its two largest parts, the skills
and the report on the system prompt: each appears as `_omitted_bytes` with its size. The recorder writes
`<redacted>` for any text value whose field name looks like a secret, and that catches one harmless field of
a stored entry, `contextTokensSource`. The recordings name the model endpoint, which is also the default in
the recorder's script. One command is deliberately not in them: `/model status` answers with
the model key in a masked form that keeps its first and last eight characters.

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

`codex/expected/` holds, for each Codex recording, the agent events the Codex driver must produce from it, one
per line, as the Rust item types write them. `claude/expected/` holds the same for the Claude Code driver. The
prototype writes them (`python3 tools/prototype/translate.py --expected fixtures/recordings/codex`, and the same
with `claude`), and CI checks that they are current.

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
- OpenCode, Pi and Goose
- Hermes with media, reactions, scheduled jobs and its message tools
- OpenClaw with media, polls, threads, scheduled jobs, subagents, plugin approvals and the pairing code exchange
- For both gateways: resuming an older session, compacting, and a standing approval or a changed setting across
  a restart
- OpenClaw with a sandboxed agent, with a session permission mode, and with `/bash` switched on
