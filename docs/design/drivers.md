# Drivers

Status: draft. Which driver each harness gets is decided
([D-005](../decisions.md#d-005-which-driver-each-harness-gets)), and so is the preference for official
libraries ([D-006](../decisions.md#d-006-prefer-official-interfaces-and-libraries)), and so are the trait and
the capability flags ([D-058](../decisions.md#d-058-the-driver-trait-and-the-capability-flags)). The testing
approach is a proposal.

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

## The driver trait

Every Rust driver implements one trait. A first version exists as code, in
[`crates/marmot-agent-driver`](../../crates/marmot-agent-driver). It has the operations of
[the socket's driver section](socket-protocol.md#the-driver-section) and no others, because a program on the
socket does the same job as messages:

- say what the harness is, and what it can do (capabilities);
- take a prompt;
- take an answer to a waiting request;
- take a control: interrupt, new session, change a setting, or one of the harness's own actions;
- hand back the next agent event.

Three things a driver does are not operations of their own. A prompt names its session, so opening and
resuming one happen when a prompt arrives. A message sent during a turn is a prompt like any other, so there
is no "steer". And "new session" is a control.

It is decided ([D-058](../decisions.md#d-058-the-driver-trait-and-the-capability-flags)).

## Capabilities

Each driver declares what its harness can do: interrupt a turn, what it does with a message sent during one,
resume a session, take attachments, and so on. The core and the clients read these flags and never check a
harness's name. A driver states every flag; none is optional.

The flags, with what Codex states for each, are in
[D-058](../decisions.md#d-058-the-driver-trait-and-the-capability-flags).

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
