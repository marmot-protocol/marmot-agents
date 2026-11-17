# The socket protocol

Status: draft. Decided: there is a local socket
([D-003](../decisions.md#d-003-the-boundary-to-other-languages-is-a-local-socket)), and it speaks
`marmot.agent-control.v3` with a hello, a driver section and a chat section, no live preview stream, and no
support for v2 ([D-020](../decisions.md#d-020-the-socket-speaks-agent-control-v3-with-a-clean-cutover)). The
individual messages are not designed yet. Administration and authentication are open:
[Q-05 and Q-06](../open-questions.md#the-socket-protocol).

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

A program on the socket never sees keys, ciphertext, relays or group state.

## The three parts of v3

### The hello

A connection opens with one exchange. The connector states the protocol version it speaks, what it is, and
which capabilities its harness supports, and it authenticates. The core refuses a version it does not support
with a clear error.

### The driver section

This is the [event model](event-model.md) and the driver trait, as messages.

**From the connector to the core:**

- agent events: every item the harness produces;
- the result of each control it was asked to carry out.

**From the core to the connector:**

- prompts, with who sent them, which chat they came from, what they reply to, and any attachments;
- answers to approval requests and questions;
- controls: interrupt, steer, start or resume a session, switch model or mode.

Hermes and OpenClaw decide for themselves how to route a prompt inside their runtime, so they need the sender
and the chat on every prompt. That is why those fields are part of the protocol and not an afterthought.

Recordings of Hermes confirm this and add to it
([`../reference/hermes-messages.md`](../reference/hermes-messages.md)):

- Hermes keeps one session per sender, even inside a group, so the sender decides which session a prompt joins.
- For a reply, Hermes needs the quoted text and its author on the prompt. It cannot look the message up.
- For Hermes, a control is a chat message that starts with a slash. Its connector turns "interrupt" into `/stop`.

### The chat section

Some agents act in the chat on their own initiative, outside a turn: Hermes and OpenClaw give their agents
message tools. The chat section lets a connector send, edit and delete ordinary messages, add and remove
reactions, send media, and read history.

The built-in drivers do not use this section in the current proposal: a coding harness speaks through its
turn. That may be too narrow. A coding agent may need to send a file
([Q-23](../open-questions.md#q-23-how-does-an-agent-send-a-file-to-the-chat)), and whether it may do more in the
chat on its own is open ([Q-24](../open-questions.md#q-24-what-may-an-agent-do-in-the-chat-on-its-own)).

## What it replaces

MDK's `wn-agent` daemon speaks `marmot.agent-control.v2`: one JSON message per line over a Unix socket. It has
54 request types, most of them about chat messages, account management and maintenance. Agent activity is a
small corner of it.

v3 turns that around. Agent events and controls are the centre. It keeps v2's socket and framing, because they
are easy to write from any language and easy to record for tests.

There is no bridge between the two. Every client of v2 is ours, so the connectors move to v3 in one cutover.

| In v2 | In v3 |
| --- | --- |
| Chat requests: send, edit, delete, react, media, history | The chat section |
| Agent activity and operation events | The driver section, in full |
| Live preview stream, seven requests | Left out until streaming is decided |
| Account, profile, relay lists, allow-list, invite policy, maintenance, diagnostics | Not available to connectors; see below |
| Debug requests | Not carried over |
| An authentication token in every message | Authentication once, in the hello |

## Administration

Open: [Q-05](../open-questions.md#q-05-how-is-the-agent-administered).

In v2 any connected client can administer the agent: change who may invite it, publish its profile, edit its
relay lists. In v3 a connector cannot. The harness is the thing being controlled, and it should not be able to
change the rules about who controls it.

The proposal is that the owner administers the agent through a configuration file and the core's own
command line, and later from the chat. The command line still has to reach the running core, so v3 gets an
administration section that only the owner's tools may use.
