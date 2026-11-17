# Reference: what the MDK connectors do today

This page is detailed on purpose. It records how the existing connectors in
[MDK](https://github.com/marmot-protocol/mdk) behave, so that the new design can be checked against them and so
that nobody has to repeat the inventory.

- **Read at:** MDK commit `a2d3486ac`, on 2026-11-17.
- **How:** two code inventories done by research agents, with file and line references. The claims marked
  "checked" were then confirmed directly in the code. The rest were not re-checked.
- **Paths** are relative to the MDK repository root. Line numbers will drift.

## Layout in MDK

| Path | What it is |
| --- | --- |
| `crates/agent-connector` | The `wn-agent` daemon: a Unix-socket server around `MarmotAppRuntime` |
| `crates/agent-control` | The socket protocol types, `marmot.agent-control.v2` |
| `crates/agent-stream-compose` | Composition of live text previews |
| `crates/transport-quic-stream`, `crates/transport-quic-broker` | The QUIC preview side channel |
| `integrations/terminal-harness` | Shared Rust runtime for the five terminal harnesses |
| `integrations/{claude,codex,opencode,pi,goose}/marmot` | One small Rust binary per terminal harness |
| `integrations/hermes/marmot` | Hermes platform plugin, Python |
| `integrations/openclaw/marmot` | OpenClaw channel plugin, TypeScript |
| `integrations/custom-client.md` | Guide for third parties who write their own socket client |

Sizes, including tests: `agent-connector` about 27,000 lines, `terminal-harness` about 16,000, the Hermes plugin
about 32,000 and the OpenClaw plugin about 32,000.

## The socket protocol today

`AgentControlRequest` in `crates/agent-control/src/lib.rs` has about fifty request types. Grouped:

- **Chat:** `SendFinal`, `EditMessage`, `DeleteMessage`, `SendReaction`, `RemoveReaction`, `SendMedia`,
  `DownloadMedia`, attachment requests, `TimelineList`, `TimelineMessageGet`, `SubscribeInbound`.
- **Live preview:** `StreamBegin`, `StreamAppend`, `StreamStatus`, `StreamProgress`, `StreamFinish`,
  `StreamFinalize`, `StreamCancel`.
- **Agent activity:** `SendAgentActivity` (kind 1201), `SendAgentOperationEvent` (kind 1202),
  `SendGroupSystemEvent`.
- **Account, groups, allow-list, invite policy, relay lists, maintenance, diagnostics, debug.**

`SendAgentOperationEvent` already has an open shape: `event_type`, `status`, `operation_id`, `run_id`,
`turn_id`, `name`, `text`, `preview`, a free-form `details` JSON value, `sequence`, `ok`, `duration_ms` and a
reply reference (lines 444 to 460, checked). The apps receive timeline updates for kinds 1201 and 1202
(`crates/marmot-uniffi/src/conversions/timeline.rs`). Only the Hermes plugin sends it, and only with status
"started".

## Terminal harnesses

All five work the same way.

- **One process per turn.** The prompt is written to stdin and stdin is then closed
  (`integrations/terminal-harness/src/process.rs`). Nothing can be sent to the harness after that, so approvals
  and questions cannot be answered.
- **The shared event type has no room for agent activity.** `ParsedEvent` has only `Session`, `Text`, `Error`,
  `FailedWithoutSideEffects`, `AttachmentNotProcessed` and `Ignored` (`process.rs:306-331`, checked). Whatever a
  harness parser recognises, only these can pass.
- **This is deliberate.** The `AGENTS.md` files for Claude, Codex, Pi and Goose each say never to expose
  reasoning or tool output (checked). Decision
  [D-008](../decisions.md#d-008-pass-everything-through) reverses this.
- **Inbound is text and media only.** Reply context, mentions, edits and reactions are dropped before the
  harness sees them (`bridge.rs`).

| Harness | How it is invoked | What reaches the chat | What is dropped |
| --- | --- | --- | --- |
| Claude Code | `claude -p --output-format stream-json --verbose`, with `--session-id` or `--resume` | Assistant text, joined per message | Thinking, tool use, tool results, subagent output, cost and usage |
| Codex | `codex exec [resume] --json -` | Completed `agent_message` items; files via a completion manifest | Reasoning, commands, file changes, to-do items, usage |
| OpenCode | `opencode run --format json` | Text parts | Tool parts, reasoning, step events |
| Pi | `pi --mode json` | Assistant text at message end | Thinking, tool calls, deltas |
| Goose | `goose run --instructions - --output-format stream-json` | Message text, joined per message | Thinking, tool requests and responses, notifications |

Errors reach the chat only as a short label such as `max_turns`, and only when the turn produced no text.

## OpenClaw plugin

- **Final answers only.** `integrations/openclaw/marmot/src/dispatch.ts:611-613` returns early for any delivery
  that is not `final`, and block streaming is disabled at `:645-648` with the comment "Keep the initial contract
  final-only" (checked).
- **No agent activity.** `client.sendAgentOperationEvent` exists and nothing calls it.
- **Message tool:** only `delete`, `edit` and `react`.
- **No path for approvals, cards, buttons or other interactive output.**
- **Inbound:** reply quote, media, history and extra context are passed in. Every conversation is reported to
  the host as a group, including two-person chats.

## Hermes plugin

The richest of the seven, in `integrations/hermes/marmot/adapter.py`.

| Hermes gives | The plugin does |
| --- | --- |
| Final text | `send_final` |
| Streamed partial text | QUIC preview, only when QUIC candidates are configured; otherwise it fails |
| Tool event | Sent as an operation event with status "started"; no result, success flag or duration |
| Commentary | `send_agent_activity` (kind 1201) |
| Approval request | Rendered as plain text and sent as an ordinary message. "The host supplies no request id" (`:4056`, checked). Answered by typed `/approve` or `/deny`, or a reaction |
| Status text, typing | Optional presence reactions; the status phrase is dropped |
| Editing a sent message | Refused |
| Media | `send_media`; one caption kept for a batch of images |
| Clarifying questions, buttons, model picker | No override found, so Hermes's text fallback applies (inferred) |

Inbound: every message is given to Hermes as text in a "group" chat. Reply context, attachments and twenty
messages of history are passed in. Edits, deletes and reactions become quiet context for the next turn.

## The QUIC preview stack

Live text previews were meant to travel over a QUIC side channel through a broker
(`crates/transport-quic-stream`, `crates/transport-quic-broker`, `crates/agent-stream-compose`). According to
the project owner it has never worked reliably and is not in use. Only Hermes can use it, and only when
configured. See [D-012](../decisions.md#d-012-word-by-word-streaming-is-not-required-now) and
[Q-14](../open-questions.md#q-14-what-happens-to-the-quic-preview-stack).

## Things the new design must not lose

These work today and are easy to forget.

- **Separate identity per connector.** Each installed connector has its own `wn-agent` process, home, socket,
  service and Marmot identity by default (`integrations/AGENTS.md`).
- **Invite policy.** `wn-agent` joins a group only when the inviter is on an allow-list
  (`crates/agent-connector/src/invite_policy.rs`).
- **Media confinement.** Outbound media is read only from allowed directories, without following symlinks.
- **Durable sends.** Replies carry a token so that a retry after a crash does not send twice.
- **Recovery after lag.** `SubscribeInbound` replays missed messages from storage.
- **Installers.** One per connector under `scripts/`, in the `wn-agent-v*` release family, with checksums.
- **Custom clients.** Third parties are told to build on the v2 socket protocol.
