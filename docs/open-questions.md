# Open questions

These points are not decided. Each has the current thinking, which is a proposal and nothing more. When one is
settled, add it to [`decisions.md`](decisions.md) and remove it from this page.

The first three block the most other work.

## The event model

### Q-01 What exactly is in an agent event?

[`design/event-model.md`](design/event-model.md) proposes an envelope and a list of item types. Still to settle:
the exact item types, the fields of each, the event kind numbers, and the size limits.

### Q-02 What happens to large output?

A tool result can be megabytes. Relays limit event size, and every durable event is stored on every member's
devices. Options are to cut output off with a marker, to send it as an encrypted attachment, or both.

### Q-03 Where are the event kinds specified?

The kinds are Marmot events, so their definition probably belongs in the
[Marmot specification](https://github.com/marmot-protocol/marmot), with MDK exposing them to the apps. The
ownership, and how much is specification versus app convention, is not settled.

### Q-04 What carries over from the earlier approvals design?

MDK issue 858 and Marmot pull request 503 design approvals and single-choice questions as a strict protocol with
a durable lifecycle. It covers only two-person chats and requires an acknowledgement that harnesses do not give.
The current thinking is to keep its rules about authentication (an answer is bound to the exact request, only
permitted people count, a bare "yes" never counts, answers never reach the model) and to drop its lifecycle
guarantees. Not yet decided.

## The socket protocol

### Q-05 How is the agent administered?

Administration means setting up the account, publishing its profile and relay lists, choosing who may invite and
prompt it, and checking its health. In v2 any connected client can do all of this.

The core is the only process that can open the account, so even a command-line tool has to ask the running core.
Administration therefore needs messages on a socket. The question is who may send them.

The current thinking:

- **Standing settings live in a configuration file** that the core reads: who may prompt and approve
  ([D-010](decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)), who may invite, relays and
  profile.
- **Actions and status go through the core's own command line**, which talks to the running core using an
  administration section of v3.
- **Connectors cannot use that section.** A harness that could change who may prompt it could be talked into
  widening its own access.
- **Later, the owner can administer from the chat.**

Open within this: whether administration gets its own socket or a separate credential on the same one, and
whether creating a group counts as administration or as an ordinary chat action.

### Q-06 How does a program prove it may connect?

Today the socket relies on file permissions, the same user id, or a bearer token that grants everything. v3
authenticates once in the hello ([D-020](decisions.md#d-020-the-socket-speaks-agent-control-v3-with-a-clean-cutover)).
What the credential is, and how a connector's rights differ from an administrator's
([Q-05](#q-05-how-is-the-agent-administered)), is open.

## Conversation handling

### Q-07 How does a chat map to a harness session?

One session per chat, per thread, or chosen by the user? How are sessions started, resumed and listed from the
chat? What happens when one host runs several harnesses, or one group contains several agents?

### Q-08 Does each agent keep its own Marmot identity?

Today each installed connector has its own account and identity. Keeping that is assumed but not decided.

### Q-09 Who owns the queue when a message arrives mid-turn?

Harnesses differ: some steer, some queue, some reject. The current thinking is that the core owns the queue and
each driver says what it supports.

### Q-10 How are permissions changed from the chat?

D-010 wants this later. The mechanism is open, and so is how a client learns who may prompt, so that it can show
this before someone tries.

## Privacy and volume

### Q-11 What keeps sensitive output from spreading?

Everything the agent does is visible to every member and stays in their history (D-009). Tool output can contain
secrets from the owner's machine. Whether the owner gets a per-chat detail setting, and what the default is, is
open.

### Q-12 How many events is too many?

A busy turn can produce hundreds of tool events. Each durable one is an encrypted group message. Whether some
activity should be combined, summarised, or sent as short-lived events is open.

### Q-13 What may be logged?

This is a private messenger. The starting assumption is that logs never contain message text, prompts, model
output, identifiers or keys. The exact rules are not written yet.

## Streaming

### Q-14 What happens to the QUIC preview stack?

D-012 makes word-by-word streaming low priority. Whether to repair the QUIC stack later, replace it with
short-lived group messages, or remove it is open.

## Drivers

### Q-15 Which library does the Codex driver build on?

OpenAI's own protocol crates are official but unpublished and pull in much of the Codex workspace. The community
`codex-codes` crate is lighter. A short trial of both should settle it.

### Q-16 How does a driver say what it supports?

The current thinking is a set of capability flags per driver, so the core and the clients adapt without checking
the harness name. The exact flags are open.

### Q-17 How are drivers tested?

The current thinking is to record real sessions and replay them through the driver, as T3 Code does. Not decided.

## The repository and the move from MDK

### Q-18 What is the core program called?

The name of the core program is not chosen. Today's daemon is `wn-agent`.

### Q-19 How do existing installs move over?

Account homes, identities, allow-lists, services and installers exist for every current connector. The cutover
plan, and when the old code leaves MDK, are open.

### Q-20 What do the apps need?

Rich rendering needs work in the White Noise apps. Which item types come first, and who does that work, is open.
