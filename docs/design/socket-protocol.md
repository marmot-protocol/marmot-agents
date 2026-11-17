# The socket protocol

Status: draft. That there is a local socket is decided
([D-003](../decisions.md#d-003-the-boundary-to-other-languages-is-a-local-socket)). Everything else on this page
is a proposal. The open points are [Q-05 and Q-06](../open-questions.md#the-socket-protocol).

## What it is for

Some connectors cannot be compiled into the core, because the harness's interface exists only in another
language. They run as their own programs and talk to the core over a socket on the same machine.

| Program | Language | Why it is separate |
| --- | --- | --- |
| Claude Code driver | TypeScript | Built on the official Claude Agent SDK |
| Cursor driver | TypeScript | Built on the official Cursor SDK |
| Hermes plugin | Python | Must run inside Hermes |
| OpenClaw plugin | TypeScript | Must run inside OpenClaw |
| Third-party connectors, later | Any | [D-014](../decisions.md#d-014-outside-developers-can-write-connectors-later) |

## The principle

A program on the socket is a driver. It does over the socket exactly what a built-in driver does through the
Rust trait: it sends agent events up and receives prompts, answers and controls down. The core treats both the
same.

This means the socket carries the [event model](event-model.md) and nothing about Marmot itself. A program on
the socket never sees keys, ciphertext, relays or group state.

## What travels over it

**From the connector to the core:**

- what the connector is, and which capabilities it supports;
- agent events: every item the harness produces;
- the result of each control it was asked to carry out.

**From the core to the connector:**

- prompts, with who sent them, which chat they came from, what they reply to, and any attachments;
- answers to approval requests and questions;
- controls: interrupt, steer, start or resume a session, switch model or mode.

Hermes and OpenClaw decide for themselves how to route a prompt inside their runtime, so they need the sender
and the chat on every prompt. That is why those fields are part of the protocol and not an afterthought.

## What it replaces

MDK's `wn-agent` daemon already has a socket protocol, `marmot.agent-control.v2`: newline-delimited JSON over a
Unix socket. It has around fifty request types, most of them about chat messages, account management and
maintenance. Agent activity is a small corner of it.

The new protocol turns that around. Agent events and controls are the centre. Account and maintenance requests
belong to the core's own command line, not to connectors.

Whether this is a new version of the existing protocol or a fresh one, and what happens to clients written
against v2, is [Q-05](../open-questions.md#q-05-what-is-the-relationship-to-the-existing-socket-protocol).

## Proposed shape

- **Transport.** A Unix socket, readable only by the owner, as today.
- **Framing.** One JSON message per line, as today. It is easy to write from any language and easy to record
  for tests.
- **Versioning.** The connector states the protocol version it speaks when it connects. The core refuses a
  version it does not support with a clear error.
- **Capabilities.** The connector declares what its harness supports when it connects, in the same flags a
  built-in driver uses.
