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

### Which requests each client uses

Counted on 2026-11-17 by searching each client's source for every request type's name, tests excluded.

| Client | Request types used, of 54 |
| --- | --- |
| Hermes plugin | 31 |
| OpenClaw plugin | 26 |
| Terminal harness | 15 |
| The custom-client guide | 4: `subscribe_inbound`, `send_final`, `account_list`, `group_create` |
| None of the above | 17 |

- **Used by both plugins:** inbound subscription, history, send, edit, delete, reactions, media, the seven
  preview-stream requests, account list, profile publish and lookup, group info and profile update, and the
  invite allow-list.
- **Used by Hermes only:** relay-list reads and edits, agent activity, diagnostics.
- **Used by the terminal harness only:** group creation, maintenance status and run, a debug request.
- **Used by none:** account creation, KeyPackage publication, group system events, group creation with admins,
  leave, group admin changes, six maintenance requests, invite-policy get and set, debug injection, and three
  attachment requests.

According to the project owner, every client of v2 in use today is one of the project's own.

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
[D-067](../decisions.md#d-067-the-quic-preview-stack-is-removed), which removes it.

## Where an install keeps what

Read at MDK commit `bf4778a` on 2026-11-21, in `scripts/install-terminal-harness-marmot.sh`,
`scripts/install-hermes-marmot.sh`, `scripts/install-openclaw-marmot.sh`, `crates/agent-connector/src/allowlist.rs`,
`crates/agent-connector/src/invite_policy.rs`, `crates/agent-connector/src/lib.rs`,
`crates/agent-connector/src/bin/wn-agent.rs`, `integrations/terminal-harness/src/config.rs`,
`integrations/openclaw/marmot/src/sender-policy.ts` and `integrations/hermes/marmot/{adapter,diagnostics,doctor}.py`.
This is what `marmot-agent import` reads
([D-089](../decisions.md#d-089-an-install-moves-over-by-importing-its-account-home-in-place)). Not checked
against a real install; the import test runs `agent-connector` itself at that commit.

| Installer | Home | Account label | `wn-agent` service (systemd / launchd) | Harness service |
| --- | --- | --- | --- | --- |
| Claude Code | `~/.marmot-agents/claude` | `claude-harness-agent` | `wn-agent-claude` / `org.marmot.wn-agent.claude` | `wn-claude` / `org.marmot.wn-claude` |
| Codex | `~/.marmot-agents/codex` | `codex-harness-agent` | `wn-agent-codex` / `org.marmot.wn-agent.codex` | `wn-codex` / `org.marmot.wn-codex` |
| OpenCode | `~/.marmot-agents/harnesses` | `terminal-harness-agent` | `wn-agent-harnesses` / `org.marmot.wn-agent.harnesses` | `wn-opencode` / `org.marmot.wn-opencode` |
| Pi | `~/.marmot-agents/pi` | `pi-harness-agent` | `wn-agent-pi` / `org.marmot.wn-agent.pi` | `wn-pi` / `org.marmot.wn-pi` |
| Goose | `~/.marmot-agents/goose` | `goose-harness-agent` | `wn-agent-goose` / `org.marmot.wn-agent.goose` | `wn-goose` / `org.marmot.wn-goose` |
| Hermes | `~/.marmot-agents/hermes` | `hermes-agent` | `wn-agent-hermes` / `org.marmot.wn-agent.hermes` | none: the plugin runs in Hermes's gateway |
| OpenClaw | `~/.marmot-agents/openclaw` | `openclaw-agent` | `wn-agent-openclaw` / `org.marmot.wn-agent.openclaw` | none: the plugin runs in OpenClaw's gateway |
| `wn-agent` alone | `~/.marmot-agents/standalone` | `standalone-agent` | `wn-agent-standalone` / `org.marmot.wn-agent.standalone` | none |

Every name above can be changed when installing (`MARMOT_HOME`, `MARMOT_AGENT_LABEL`,
`MARMOT_AGENT_SERVICE_NAME` and others), so the import finds services by the home they run on, not by name.

- **The home** is an MDK account home, opened by `wn-agent` exactly as the core opens one:
  `MarmotApp::try_with_relays_and_account_home_and_config` with `AccountHome::open(home)` and
  `MarmotAppConfig::white_noise()`. One identity per home by default. Opening it takes MDK's root runtime
  lease (`MarmotRootRuntimeLease`, a lock on `.marmot-runtime.lock` in the home), held while `wn-agent` runs.
- **Relays** are not in the home. They are `--relay` arguments on the `wn-agent` service's command line, each
  split at commas, then `MARMOT_RELAYS` (or the older `MARMOT_RELAY` when `MARMOT_RELAYS` is not set), then
  `wss://relay.eu.whitenoise.chat` and `wss://relay.us.whitenoise.chat`. With `--no-default-relays` and none
  named, `wn-agent` runs with no relays at all, offline, which its help calls "Offline/test only"
  (`resolve_relays` in `crates/agent-connector/src/bootstrap.rs`). The import refuses such an install, since
  the core needs relays.
- **Files `wn-agent` may send** come only from its `--media-allowed-root` folders, which may be repeated. With
  none, it sends no file. The terminal harnesses' installers set none. At MDK commit `7f7dcc6` (read on
  2026-11-22), the Hermes and OpenClaw installers set one, `dev/outbound-media` in the home
  (`MARMOT_OUTBOUND_MEDIA_DIR`), which the import leaves out, being inside the home.
- **The invite allow-list** is `dev/agent-allowlist/<account id>.json` in the home: `account_id_hex`,
  `invite_policy` (`deny`, `allowlist`, `any_authenticated_direct` or `any_authenticated`, `allowlist` by
  default) and `welcomer_account_ids_hex`, which must be there. A damaged record, or one naming another account,
  is read as empty, which lets nobody invite. An entry counts only if it is the welcomer's key exactly, in
  lowercase hex; an npub never matches. `allowlist` lets the listed people invite into any group;
  `any_authenticated_direct` lets anyone invite into a group of two and ignores the list;
  `any_authenticated` lets anyone invite.
- **Who may prompt a terminal harness** is `WN_<HARNESS>_ALLOWED_SENDERS_HEX` (for OpenCode also the older
  `WN_OPENCODE_ADMIN_HEX`) in the harness's environment. The installer writes it into the harness service
  (`Environment=` lines, or `EnvironmentVariables` in the LaunchAgent) and into `dev/wn-<harness>.env` in the
  home, which is only for running the harness by hand. It sets it to the welcomers unless told otherwise. The
  harness refuses to start when it is missing, empty or holds any entry that is not 64 hex digits (either
  case); npubs are not taken. The same environment holds the harness program (`WN_<HARNESS>_BIN`), the
  execution profile (`MARMOT_HARNESS_EXECUTION_PROFILE`, `inherit` by default) and time and reply limits.
- **Who may write to Hermes** is `MARMOT_ALLOWED_USERS` and `MARMOT_ALLOW_ALL_USERS` in `$HERMES_HOME/.env`
  (`~/.hermes` by default), which Hermes loads over its own environment. Hermes also reads `allowed_users`,
  `allowed_users_hex` and `allow_all_users` from its `config.yaml`, and combines the two; the import does not
  follow that and treats such a config as unclear.
- **Who may write to OpenClaw** is the `senderPolicy` (`allowedUsers`, `allowAll`) of the account the plugin
  runs, in `$OPENCLAW_HOME/.openclaw/openclaw.json` (`$HOME` by default), and otherwise the same two variables
  in the gateway's environment. Without `channels.marmot.accounts` that is `channels.marmot.senderPolicy`;
  with it, each account runs on `channels.marmot.accounts.<id>.senderPolicy`, and a policy at the channel's
  root applies to none of them (checked in `integrations/openclaw/marmot/src/channel.ts` and
  `sender-policy.ts` at MDK commit `c210e7d9`, on 2026-11-23). The plugin parses it strictly: any field besides those two, an `allowAll` that is not
  a boolean, or any `allowedUsers` entry that is not 64 hex digits makes the whole policy invalid, which denies
  every sender, even with `allowAll: true`. An empty list without `allowAll` denies everyone too.
- **The socket** is `dev/wn-agent.sock` in the home, and its token `control.token`. The core's own are
  `socket/marmot-agent.sock` and `connector.token`, so the two never collide.

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
