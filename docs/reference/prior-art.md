# Reference: what other projects do

This page is detailed on purpose. It records what was learned from two projects that solve a similar problem,
so that the lessons can be reused and the research does not have to be repeated.

- **Read on:** 2026-11-17, from the public repositories.
- **How:** research agents read the source and reported with file references. Their findings were not
  independently re-checked. Items marked "inferred" are conclusions, not statements found in the source.
- **File paths** are in the named repository and will drift.

## What we took from each

| Project | What it is | What we took |
| --- | --- | --- |
| [T3 Code](https://github.com/pingdotgg/t3code) | A coding app that drives many harnesses | The overall structure: native drivers behind one interface, ACP for the long tail, one event model, capability flags, recorded-session tests |
| [Buzz](https://github.com/block/buzz) | A Nostr-based workspace for people and agents | The envelope idea and some runner design. Its event vocabulary did not fit |
| [Agent Client Protocol](https://github.com/agentclientprotocol/agent-client-protocol) | An open standard between editors and agents | The default driver, and its list of what harnesses have in common |

ACP is covered in [`harness-interfaces.md`](harness-interfaces.md). How these and other products show subagents,
and how chat threads work, is in [`subagents-prior-art.md`](subagents-prior-art.md) (read 2026-11-23).

## T3 Code

MIT licence. Read at commit `f79ab797`.

### How it drives harnesses

Every connection is long-lived and two-way.

| Harness | Interface |
| --- | --- |
| Codex | `codex app-server`, JSON-RPC over stdio |
| Claude Code | `@anthropic-ai/claude-agent-sdk`, fed by a streaming input queue |
| Cursor | `@cursor/sdk`, in process |
| OpenCode | `opencode serve`, HTTP and server-sent events; separate adapters for versions 1 and 2 |
| Pi | `pi` RPC mode |
| Grok, Kiro and others | ACP, each with a thin "flavour" |
| Anything else | A generic ACP driver, from the ACP registry or a local command |

Its stated rule is that an ACP agent starts as a registry entry and gets a dedicated flavour "only when it needs
behavior the spec does not cover" (`docs/internals/adding-a-provider.md`).

### One interface for every adapter

`packages/provider-core/src/server/ProviderAdapter.ts` defines it.

- **Required:** an event stream, `ensureThread`, `resumeThread`, `startTurn`, `steerTurn`, `interruptTurn`,
  `respondToRuntimeRequest`, `readThreadSnapshot`, `rollbackThread`, `forkThread`.
- **Optional:** `resumeTurn`, `compactThread`, `applyModelSelection`, `injectHistory`, `stopSubagent`,
  `stopTask`, and others added for single harnesses.

### One event model

Events are whole-entity updates, not deltas: the adapter sends the current state of a thing and the receiver
replaces what it had. Turn items include:

`user_message`, `assistant_message`, `reasoning`, `proposed_plan`, `todo_list`, `user_input_request`,
`approval_request`, `command_execution`, `file_change`, `file_search`, `web_search`, `dynamic_tool`, `subagent`,
`compaction`, `error`, `handoff`, `fork`, `checkpoint`.

Raw native events are not stored next to the common ones. They go to a bounded diagnostic log, and each common
item keeps a reference to its native id. We chose differently: our events carry the native payload
([D-007](../decisions.md#d-007-one-shared-event-vocabulary)).

### How it avoids the lowest common denominator

- **Capability flags.** About sixty, in twelve groups (sessions, threads, turns, streaming, tools, approvals,
  planning, subagents, context, checkpoints, identity, runtime policy). The stated rule is "Orchestration and
  clients read capabilities, never the driver kind."
- **Opaque options.** Model, effort and mode settings are published by each adapter as descriptors and drawn
  generically. Approval and question choice ids pass through unchanged.
- **Defined fallbacks.** When a capability is missing, policy decides what happens instead; for example a fork
  falls back to handing the context to a new thread.

### Controls

Steer, queue and interrupt are generic requests. The server owns the queue and decides from the capabilities
whether to steer natively, interrupt and restart, or queue.

| Action | Codex | Claude Code | Pi | ACP |
| --- | --- | --- | --- | --- |
| Steer | `turn/steer` | A user message with priority "now" | A prompt with steering behaviour | None, unless the flavour adds it |
| Interrupt | `turn/interrupt` | `query.interrupt()` | `abort` | `session/cancel` |
| Approvals, questions | JSON-RPC reply | `canUseTool` | `extension_ui_response` | `session/request_permission` |

Slash commands are advertised per harness and sent as prompt text.

### Testing

Recorded raw sessions are replayed through the real adapter. There are about a hundred scenario directories and
a recorder script per harness. The same scenarios (simple turn, several turns, queued turn, interrupt, steering)
run against every harness. Their documentation calls hand-written frames "a guess about the protocol".

### What going native costs them

| Adapter | Lines |
| --- | --- |
| Claude Code | about 14,600 |
| Shared ACP | about 10,000, plus 3,000 for the session runtime |
| Codex | about 8,900 |
| OpenCode | about 4,500 and 5,500 for its two versions |
| Pi | about 4,400 |
| Cursor | about 3,300 |
| An ACP flavour | 260 to 650 |

Upstream changes are constant: OpenCode needs two adapters and Cursor was rewritten once. The session interface
keeps gaining optional methods for single-harness features.

### Reuse

The adapters are private, unpublished packages built on the Effect framework and T3 Code's own event model, so
they cannot be used as a dependency. They are a reference.

## Buzz

Apache-2.0. A Rust monorepo with a relay server, an agent runner (`buzz-acp`), a command-line tool, and desktop,
web and mobile clients.

### How it drives harnesses

`buzz-acp` speaks ACP over stdio to every harness, with a hand-written client and no ACP library. Goose runs as
`goose acp`; Codex and Claude Code run through the ACP organisation's adapters. Other harnesses are command
presets. Harness-specific cases live inside the one client: how the system prompt is passed, a Goose-only
steering method, Codex configuration merging.

Its newer application (`block/buzz-app`) drives Codex, Claude Code and Pi through native plugins and keeps ACP
for Goose. This suggests that ACP alone did not give full fidelity (inferred).

### What goes on the wire

- **Channel messages** are ordinary chat messages that the agent posts itself by calling a command-line tool.
  Its base prompt says "Your reasoning and tool calls are invisible".
- **Agent activity** goes only to the agent's owner, in one short-lived event kind (24200). The content is
  encrypted to the owner and is an envelope: sequence, time, kind, session, turn and a payload.
- **The payload is the raw ACP message.** Text, thinking, tool calls, plans and usage are ACP JSON, and the
  client parses ACP directly.
- **There is no vocabulary for approvals, questions or session control.**

### Controls

- **Prompt:** a chat message that mentions the agent.
- **Interrupt:** a `!cancel` message or an owner control frame.
- **Steer:** a follow-up message goes to the harness's steering method when it has one, otherwise the turn is
  cancelled and the messages are merged.
- **Model switch:** an owner control frame.
- **Approvals:** always granted automatically. No person is asked.

### Permissions and privacy

- Each agent has its own key, tied to its owner by a signed tag.
- Who may prompt is set per agent: owner only (the default), an allow-list, anyone, or nobody.
- Only the owner can send controls.
- Channel content is plain text on one trusted relay. Only owner-directed data is encrypted, pairwise.

### What transfers and what does not

- **Transfers as an idea:** one envelope with sequence, turn, session and a payload, instead of a new event kind
  for every activity.
- **Transfers as a reference:** the runner's handling of timeouts, cancel-and-drain and steering; its session
  scoping and queue-merging logic; its "who may prompt" setting, which matches
  [D-010](../decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve).
- **Does not transfer:** activity is visible to the owner only, where ours is visible to the group
  ([D-009](../decisions.md#d-009-groups-are-first-class-and-every-member-sees-agent-activity)); there is no
  approval or question vocabulary; authorisation depends on its relay's database; the main files are over
  ten thousand lines each.

## Mesh LLM

[Mesh LLM](https://goose-docs.ai/blog/2026/04/20/mesh-llm) is peer-to-peer sharing of model inference. It is not
related to connectors and nothing was taken from it.

## Lessons carried into the design

1. Drive important harnesses natively; use ACP for the rest; hide the difference behind one interface.
2. Common events are whole items that replace their earlier state.
3. Drivers declare capabilities; nothing above them checks a harness's name.
4. Choice ids from the harness pass through unchanged.
5. Steer, queue and interrupt are generic requests that each driver carries out in its own way.
6. Test drivers by replaying recorded sessions.
7. Send agent activity to the whole group, with the native payload attached, unlike both projects above.
