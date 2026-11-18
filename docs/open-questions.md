# Open questions

These points are not decided. Each has the current thinking, which is a proposal and nothing more. When one is
settled, add it to [`decisions.md`](decisions.md) and remove it from this page.

The first three block the most other work.

## The event model

### Q-01 What exactly is in an agent event?

[`design/event-model.md`](design/event-model.md) is now a concrete draft: ten item types, the fields of each,
and ten rules for drivers. A prototype applies it to 64 recorded sessions with Codex, Claude Code and
Hermes, with nothing left over. It is still a proposal. To settle: whether these are the right types and fields, and the exact
tags. How an event is carried is decided ([D-022](decisions.md#d-022-the-agents-words-are-ordinary-chat-messages)
to [D-026](decisions.md#d-026-an-agent-item-is-at-most-64-kb)). A to-do list
that changes during a turn has not been seen in a recording.

An eleventh type, `session`, was added on 2026-11-18
([D-030](decisions.md#d-030-a-session-is-an-item-the-eleventh-type)). The prototype does not produce it yet.

### Q-02 What happens to large output?

The limit is decided: an agent item is at most 64 KB ([D-026](decisions.md#d-026-an-agent-item-is-at-most-64-kb)).
What is still open is how the full output is fetched when it was cut.

A tool result can be megabytes. Relays limit event size, and every durable event is stored on every member's
devices. Options are to cut output off with a marker, to send it as an encrypted attachment, or both.

### Q-03 Where are the event kinds specified?

The carrier page ([`design/event-carrier.md`](design/event-carrier.md)) lists what the Marmot specification
and MDK would each need. Kind 1201 carries every agent item ([D-023](decisions.md#d-023-one-kind-carries-every-agent-item)), and kind 1202
carries answers and controls ([D-027](decisions.md#d-027-answers-and-controls-are-one-kind-of-event-kind-1202)).

The kinds are Marmot events, so their definition probably belongs in the
[Marmot specification](https://github.com/marmot-protocol/marmot), with MDK exposing them to the apps. The
ownership, and how much is specification versus app convention, is not settled.

### Q-23 How does an agent send a file to the chat?

The event model has no item for a file the agent produces: a report, a patch, an image. Today's Codex connector
can already send files back, so leaving this out would be a step backwards. To settle: the item itself, where
the file is stored, size limits, and the rule that keeps an agent from sending files it should not read. Raised
in [issue 4](https://github.com/marmot-protocol/marmot-agents/issues/4).

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

The project owner agreed with this direction on 2026-11-17 and chose not to fix its shape yet. It is to be
decided when drivers are being built against real harnesses, not before.

## Conversation handling

### Q-07 How does a chat map to a harness session?

Decided: each chat with an agent has its own session, and a new session can be started in the same chat
([D-035](decisions.md#d-035-each-harness-install-is-its-own-identity-with-its-own-sessions)).

Still open:

- **Older sessions.** How a person lists the sessions a chat has had, and goes back to one.
- **Gateways that choose their own sessions.** Hermes keeps one session per sender, even in a group, so a
  group chat has several. OpenClaw puts every direct chat in one session unless its plugin arranges otherwise,
  so the plugin has to arrange one per chat. Whether the core should accept Hermes's split is part of
  [Q-22](#q-22-whose-rules-govern-a-gateway).
- **Threads.** Whether a thread inside a chat gets its own session.

Raised in [issue 8](https://github.com/marmot-protocol/marmot-agents/issues/8).

### Q-22 Whose rules govern a gateway?

Hermes has its own allow-list, its own sessions (one per sender, even in a group) and its own notices, and it
drops a disallowed sender without a word. The core has the owner's settings
([D-010](decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)) and must tell a disallowed
person so. The current thinking is that the core checks first and Hermes is configured to allow whatever the
core lets through. Not decided. See [`reference/hermes-messages.md`](reference/hermes-messages.md).

OpenClaw was recorded on 2026-11-17, and differs on each point. This is evidence, not a decision. See
[`reference/openclaw-messages.md`](reference/openclaw-messages.md).

- **Who may speak.** OpenClaw does not drop anyone by itself. The plugin asks it for a verdict, supplying the
  policy and the allow-lists, and gets back "run", "drop", "skip" or "pairing required" with a reason. Acting
  on the verdict is the plugin's job, so the plugin could tell a disallowed person. The plugin could equally
  hand OpenClaw an open policy and let the core decide first.
- **Who may approve.** When the plugin claims OpenClaw's approval capability, OpenClaw asks the plugin whether
  a given sender may approve. That is a place to ask the core.
- **Sessions.** A group is one session shared by everyone in it. All direct chats share one session unless
  the plugin or the owner arranges otherwise.
- **Notices.** OpenClaw also speaks for itself, but marks what it says: by kind of delivery, by an error flag,
  and by structured data on approvals and questions.
- **What the owner must set.** An approval reaches the chat only if the owner has switched approval
  forwarding on in OpenClaw. Left off, the command waits unseen.

### Q-08 How do several agent identities run together?

Decided: each harness install is its own Marmot identity, and so its own contact
([D-035](decisions.md#d-035-each-harness-install-is-its-own-identity-with-its-own-sessions)). What is open is
how several of them run on one machine and behave in one group. This needs much more product work.

The rough shape, not decided:

- **Who answers.** In a one-to-one chat the agent always answers. In a group it answers only when it is
  mentioned or replied to. A message addressed to nobody gets no answer. The project owner has said this
  behaviour is the right one.
- **No loops.** Agents must not answer each other, and several must not answer at once.
- **The owner is not a required link.** Nothing forces an agent to state whose it is. An owner who wants to can
  say so in the agent's profile, for example "Max's Codex" in its name or description.

Still to work out:

- **One program or several.** One program on the machine could hold every identity, which is easier to run and
  to set up, provided each identity's keys and storage are kept apart. Or each identity could have its own
  program, as today. Leaning to one program.

  MDK supports both today ([`reference/mdk-multiple-identities.md`](reference/mdk-multiple-identities.md)). One
  process can run several accounts, each with its own encrypted databases, its own key and its own relay
  connections. But that separation is of files and data, not a security boundary: every running agent's key is
  in the same process memory, so a break-in to the program exposes all of them. With today's default store, each
  key is also a plain file beside the data it protects. So the choice is between easier running and stronger
  isolation, and where keys are kept matters more if one program holds them all.
- **How an agent knows another member is an agent**, so that it does not answer it, when no owner link is
  required. The proposal is a flag in the agent's own profile: a Nostr profile (kind 0) keeps its fields in its
  content, so a true-or-false "this is an agent" field can simply be added. Nostr already has a profile field
  named `bot` for automated accounts, which may be the one to use. A limit on back-and-forth replies would be a
  backstop either way.
- **Every agent in a group reads everything.** A member receives every message, mentioned or not. Five agents
  in a group means five harness installs holding that chat.
- **Names.** The identity is per install, so Claude Code on a laptop and on a server are two contacts.
- **Setup.** Each identity has to be created, named and invited. It should take one step, not five.

### Q-27 What happens to a turn when the core or a driver restarts?

People restart connectors for updates without waiting for a turn to finish, and programs crash. Whatever the
mechanism, a restart during a turn must not:

- lose the chat's saved session;
- run a prompt twice;
- leave the chat showing a turn that never ends.

How is open. The simple version: after a restart the chat shows the turn as interrupted, and a person says
"carry on". [Issue 9](https://github.com/marmot-protocol/marmot-agents/issues/9) proposes more: the core resumes
the session and runs a "continuation turn" that tells the agent to check what already happened. That means the
connector sends the model a prompt nobody wrote, which sits badly with staying true to the harness, so it is
parked as a possible later option. It also depends on things not designed yet: whether a harness even stops when
the core restarts depends on whether its driver is built in or a separate program.

Recordings now show what the two coding harnesses do
([`reference/harness-messages.md`](reference/harness-messages.md)). Stopped mid-turn and resumed, both keep the
session and neither reruns anything. Codex shows the earlier turn as interrupted, with the running command
missing from its history. Neither sends any message to say the turn ended, so the driver has to close it. That
supports the simple version.

### Q-09 Who owns the queue when a message arrives mid-turn?

Harnesses differ: some steer, some queue, some reject. The current thinking is that the core owns the queue and
each driver says what it supports.

Recordings show the difference is larger than supported or not: Codex folds a mid-turn message into the running
turn, and Claude Code ends the turn and starts a new one. See
[`reference/harness-messages.md`](reference/harness-messages.md#side-by-side).

The gateways differ too: Hermes folds the message into the running turn and says so, and OpenClaw held it
without a word and ran it as a turn of its own afterwards. See
[`reference/openclaw-messages.md`](reference/openclaw-messages.md#controls-are-chat-messages).

### Q-29 Which right does each control need?

Every action needs the right to prompt or the right to approve
([D-032](decisions.md#d-032-every-action-needs-one-of-two-rights)). Sending a prompt needs the first. Answering
an approval and changing the permission mode need the second. The rest is not settled:

| Action | Proposed | Why |
| --- | --- | --- |
| Interrupt | Prompt | Anyone who can start work can stop it |
| Start a new session | Prompt | |
| Change a setting other than the permission mode, such as the model | Prompt | |
| Use a harness's own action, such as compacting | Prompt | |
| Answer a question | Prompt | The answer is information for the agent, like a prompt |

[D-010](decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve) says the opposite as written: it
puts "use controls" with the right to approve. The reason to move them is that a harness's slash commands are
messages ([D-031](decisions.md#d-031-a-slash-command-is-a-message-for-the-harness)), and they include commands
that stop a turn, start a session and switch the model. Someone who may prompt can type those. A stricter rule
for the button than for the typed command would protect nothing.

Also to check: that each connector's list of commands needing the right to approve is complete. A harness can
add such a command in any release. Whether Hermes and OpenClaw have typed commands that change the permission
mode has not been recorded.

### Q-10 How are permissions changed from the chat?

D-010 wants this later. The mechanism is open, and so is how a client learns who may prompt, so that it can show
this before someone tries.

### Q-24 What may an agent do in the chat on its own?

A coding agent could act in Marmot during a turn, beyond producing output: rename the chat, create a new group,
react to a message. Hermes and OpenClaw agents already have tools of this kind. Whether coding agents should,
which actions, who may allow them, and how the harness asks the core for one are all open. This overlaps with
administration ([Q-05](#q-05-how-is-the-agent-administered)), which waits until drivers are being built against
real harnesses. Parked until the Hermes and OpenClaw recordings are in. Raised in
[issue 4](https://github.com/marmot-protocol/marmot-agents/issues/4).

### Q-25 Can something outside the chat and the harness start a turn?

The draft event model already records what started a turn: a person's message, the agent itself after
background work, or a control such as compaction. [Issue 6](https://github.com/marmot-protocol/marmot-agents/issues/6)
asks for one more: a tool on the owner's machine, such as a watcher for finished CI checks or a scheduled
check-in, that delivers a prompt into a chat's session.

It is not clear this is new. An agent that is building a feature can watch its own CI run as background work
and start a turn when it finishes, which the model already covers. The issue's author has been asked for a case
that the harness cannot handle itself.

If there is one, it is a new way in: whatever can submit a prompt gets the agent to act with the owner's
authority, without the checks on who may prompt
([D-010](decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)). It would have to be the owner's
alone, which makes it part of administration ([Q-05](#q-05-how-is-the-agent-administered)). Parked.

## Privacy and volume

### Q-11 What keeps sensitive output from spreading?

Everything the agent does is visible to every member and stays in their history (D-009). Tool output can contain
secrets from the owner's machine. Whether the owner gets a per-chat detail setting, and what the default is, is
open.

### Q-21 What happens to a secret answer?

A Codex question can be marked secret, for something like a password. Everything in a group is visible to every
member ([D-009](decisions.md#d-009-groups-are-first-class-and-every-member-sees-agent-activity)), so the answer
cannot travel as an ordinary event. Options are to refuse secret questions in a group, to allow them only when
the group is the owner alone, or to give them a separate path. Not decided.

### Q-12 How many events is too many?

A busy turn can produce hundreds of tool events. Each durable one is an encrypted group message. Whether some
activity should be combined, summarised, or sent as short-lived events is open.

One real number: in four days of one developer's use of Claude Code, a turn averaged about 12 tool calls,
7 reasoning blocks and 3.5 pieces of text, which is roughly 35 events if each tool call is sent twice. Those
were long, autonomous coding sessions; a short question would be far smaller.

### A proposal for Q-11 and Q-12, not decided

One filter could answer both: a setting the owner controls, per chat, for how much agent activity is sent.
[Issue 5](https://github.com/marmot-protocol/marmot-agents/issues/5) proposes three levels:

| Level | What is sent |
| --- | --- |
| Full | Everything. The default, as [D-008](decisions.md#d-008-pass-everything-through) says |
| Summary | Messages, approvals, questions, errors and turn results, plus one summary line per turn that is updated in place as the agent works |
| Final | Messages, approvals, questions and errors only |

The summary line is the strongest part. One item that keeps replacing itself ("ran 12 commands, edited 3 files")
would cut the volume a great deal, and it fits how items already replace their earlier state.

This needs more product thinking before anything is decided, and it does not change D-008. Points to work out:

- **Can the detail be seen, or recovered later?** That matters to people. If a lower level means the detail is
  never sent, nobody can open it up afterwards, the owner included. The alternatives are to send it where only
  the owner sees it, which sits badly with
  [D-009](decisions.md#d-009-groups-are-first-class-and-every-member-sees-agent-activity), or to keep it on the
  agent's machine to be fetched on request.
- **Are three fixed levels the right shape?** It could equally be a choice per kind of item.
- **Gateways already have their own setting.** Hermes and OpenClaw each decide how much tool activity they send.
  A filter in the core would sit on top of theirs, and the two should not fight.
- **It depends on how events are carried**, which is not designed yet.

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

The recorders and 50 recordings of coding harnesses exist: [`tools/record`](../tools/record/README.md) and
[`fixtures/recordings`](../fixtures/recordings/README.md). A prototype replays them through the proposed event
model ([`tools/prototype`](../tools/prototype/README.md)). There is no driver to replay them through yet.

## The repository and the move from MDK

### Q-18 What is the core program called?

The name of the core program is not chosen. Today's daemon is `wn-agent`.

### Q-19 How do existing installs move over?

Account homes, identities, allow-lists, services and installers exist for every current connector. The cutover
plan, and when the old code leaves MDK, are open.

The leading requirement, from [issue 10](https://github.com/marmot-protocol/marmot-agents/issues/10): an install
that moves over keeps its identity, its keys, its encryption state and every group it is in. Nobody has to
invite an agent again. This fits the identity direction in
[Q-08](#q-08-how-do-several-agent-identities-run-together): each existing account home is one identity, with nothing
to merge or split.

One caution about going back. Keeping the old service installed but stopped is cheap insurance, but it only
works until the new core first opens the store. MDK's storage upgrades run forward only: once a newer version
has upgraded a database, older programs refuse it. So a way back after that point needs a backup taken before
the first open, not just the old program.

A second caution applies to any backup and restore of an agent, not only to going back. Once a core has
processed group messages, sent anything or started a turn, restoring an older copy of its store could reuse
encryption keys or run a prompt twice. A restore is safe only if nothing of that kind happened in between.
Raised in [issue 12](https://github.com/marmot-protocol/marmot-agents/issues/12).

What an importer carries over, and whether each identity gets its own program, wait until cutover is planned.

### Q-28 How does someone know a release is genuine?

Today's agent releases in MDK are published with a checksum file beside each download. A checksum catches a
broken download, not a replaced one: whoever can replace a release file can replace its checksum too. The
releases carry no signature or attestation. Signing them will be needed eventually, both for MDK's current
agent releases and for whatever this repository ships, and it must come before any kind of automatic update.
Not scheduled. Raised in [issue 12](https://github.com/marmot-protocol/marmot-agents/issues/12).

### Q-20 What do the apps need?

Rich rendering needs work in the White Noise apps. Which item types come first, and who does that work, is open.
