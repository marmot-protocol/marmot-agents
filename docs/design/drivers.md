# Drivers

Status: draft. Which driver each harness gets is decided
([D-005](../decisions.md#d-005-which-driver-each-harness-gets)), and so is the preference for official
libraries ([D-006](../decisions.md#d-006-prefer-official-interfaces-and-libraries)). The trait, the capability
flags and the testing approach are proposals.

## What a driver does

A driver connects one harness to the core. It does two things:

1. **It speaks the harness's own interface.** It starts the harness, holds a two-way session open, and tracks
   the running turn and any requests the harness is waiting on.
2. **It translates.** Native events become [agent events](event-model.md). Answers and controls from the chat
   become native calls.

It does not decide who is allowed to do anything, and it does not touch Marmot. See
[`architecture.md`](architecture.md).

## The drivers

| Harness | Driver | Language | Interface it speaks | Builds on |
| --- | --- | --- | --- | --- |
| Codex | Native | Rust | `codex app-server`, JSON-RPC over stdio | Types of its own, with Codex's schema as the reference ([D-055](../decisions.md#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own)) |
| Pi | Native | Rust | `pi --mode rpc`, JSON lines over stdio | `pi-codes` |
| Goose | ACP, with a small Goose flavour | Rust | `goose acp` | `agent-client-protocol` |
| OpenCode | ACP | Rust | `opencode acp` | `agent-client-protocol` |
| Other ACP agents | ACP | Rust | Each agent's ACP command | `agent-client-protocol` |
| Claude Code | Native | TypeScript | The Claude Agent SDK | `@anthropic-ai/claude-agent-sdk` |
| Cursor | Native | TypeScript | The Cursor SDK | `@cursor/sdk` |
| Hermes | Plugin | Python | Hermes's platform plugin interface | Hermes itself |
| OpenClaw | Plugin | TypeScript | OpenClaw's channel plugin interface | OpenClaw itself |

The facts behind each choice are in [`../reference/harness-interfaces.md`](../reference/harness-interfaces.md).

### Why some are native and some use ACP

The Agent Client Protocol (ACP) is an open standard that many harnesses speak. One ACP driver covers all of
them, so a new harness can be a line of configuration.

ACP's standard part covers what harnesses have in common. What makes a harness distinctive mostly travels in
harness-specific extensions, or is missing. So:

- **A harness gets a native driver** when it matters enough that losing features is not acceptable, or when its
  ACP support is poor. That is Codex, Claude Code, Pi and Cursor.
- **Everything else uses the ACP driver.** Where a harness needs something the standard lacks, the ACP driver
  gets a small "flavour" for it, as Goose does for steering.

Because all drivers sit behind the same trait, a harness can move from ACP to a native driver later without any
change above the driver.

### Why Hermes and OpenClaw are plugins

Hermes and OpenClaw are not coding tools driven by one person. They are agent runtimes with their own idea of
users, channels and routing. ACP models one editor user per session, so it has no place for who sent a message
or which group it came from. Their own plugin interfaces do. Each plugin runs inside its runtime and connects to
the core over [the socket](socket-protocol.md).

### What Hermes can and cannot report

Recordings show that Hermes hands a platform formatted text, not tool events: a line such as "Running a
command" arrives through the same call as a reply. Only approvals and questions arrive in a structured form. So
the Hermes connector can report messages, notices, approvals and questions, and cannot report tool calls as
items without a change on the Hermes side. See
[`../reference/hermes-messages.md`](../reference/hermes-messages.md).

## Proposed: the driver trait

Every Rust driver implements one trait. In outline it needs to:

- say what the harness supports (capabilities);
- open a session, and resume an existing one;
- start a turn with a prompt;
- steer or interrupt a running turn;
- deliver an answer to a waiting request;
- change session settings such as model and mode;
- produce a stream of agent events.

Programs on the socket do the same things as messages. The exact trait is not designed yet.

## Proposed: capabilities

Each driver declares what its harness can do: steer mid-turn, resume, report usage, ask questions, and so on.
The core and the clients read these flags and never check a harness's name. When a capability is missing, the
core falls back in a defined way, for example interrupt-and-restart where steering is not supported.

## Proposed: testing with recorded sessions

Record a real session with a harness: every message in both directions. Replay it through the driver in a test
and check the agent events that come out. When a harness changes its interface, a new recording shows the
difference.

Hand-written test messages are a guess about what the harness sends. Recordings are what it sends.

## Adding a harness

1. **Does it speak ACP well?** Add it as a configuration entry for the ACP driver.
2. **Does it need something ACP lacks?** Add a flavour to the ACP driver for that one thing.
3. **Does it matter enough, or is its ACP support poor?** Write a native driver: in Rust if the interface is a
   documented wire protocol, or as a separate program over the socket if the official library is in another
   language.
4. **Is it a runtime with its own users and channels?** Write a plugin for it that connects over the socket.
