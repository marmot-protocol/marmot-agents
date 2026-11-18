# Decisions

This is the record of what has been decided and why. It exists so that settled points are not argued again.

- Add an entry when a decision is made. Give it the next number, the date, the decision and the reason.
- Do not edit an old entry to change its meaning. Add a new entry that replaces it, and mark the old one
  "Replaced by D-0NN".
- A point that is not here is not decided. Proposals live in the `design/` documents and in
  [`open-questions.md`](open-questions.md).

All entries were decided by the project owner. Entries without a date were decided in the planning
discussion of 2026-11-17.

## D-001 Connectors live in this repository

**Decision.** The agent connectors are built here, not in MDK. MDK keeps only the Marmot event kinds that agents
send, because the apps need them to render agent activity.

**Why.** Harness interfaces change weekly; MDK releases on a slower, more careful cycle. The connectors also need
Python and TypeScript code and their own installers, which do not belong in the protocol implementation.

## D-002 One Rust core program, with MDK as a library

**Decision.** One Rust program holds the Marmot side (account, encryption, relays, storage) and the conversation
handling. It depends on MDK's Rust library. It does not call the MDK command-line tool.

**Why.** The program needs live subscriptions and sends in the same process. Going through a command-line tool
would put a text boundary in the middle of it.

## D-003 The boundary to other languages is a local socket

**Decision.** Code that must be written in another language talks to the core over a local socket with a defined
wire protocol. We do not link libraries across languages.

**Why.** Hermes plugins must be Python and OpenClaw plugins must be TypeScript. A socket works from any language.
Maintaining C interfaces and linked libraries for each one is more painful.

## D-004 Four layers, with drivers behind one trait

**Decision.** The work between a harness and the chat is split into four layers:

1. **Driver.** Speaks one harness's own interface and holds the live session. One per harness.
2. **Translation.** Turns native events into Marmot events and answers back again. One per harness.
3. **Conversation handling.** Who may prompt, which chat maps to which session, turn order. Written once.
4. **Marmot side.** Account, encryption, relays, storage. Written once, on MDK.

Rust drivers implement one shared trait. Whether a harness is driven natively or through ACP is invisible above
the driver.

**Why.** Layers 1 and 2 are different for every harness by nature. Layers 3 and 4 are the same for all of them.
Putting the line there keeps each driver small and lets a harness move from ACP to a native driver without
changing anything else.

## D-005 Which driver each harness gets

**Decision.**

| Harness | Driver | Language | Builds on |
| --- | --- | --- | --- |
| Codex | Native | Rust | `codex app-server`; OpenAI's protocol crates, or `codex-codes` if those are too heavy |
| Pi | Native | Rust | `pi --mode rpc`; the `pi-codes` crate |
| Goose, OpenCode, the long tail | ACP | Rust | The official `agent-client-protocol` crate |
| Claude Code | Native | TypeScript | The official `@anthropic-ai/claude-agent-sdk` |
| Cursor | Native | TypeScript | The official `@cursor/sdk` |
| Hermes | Its own plugin, over the socket | Python | Hermes's platform plugin interface |
| OpenClaw | Its own plugin, over the socket | TypeScript | OpenClaw's channel plugin interface |

**Why.** Claude Code and Codex are among the most used and lose features through ACP. Pi's only ACP adapter is a
stale third-party one. Cursor's own interface is its SDK. Goose is ACP-native and OpenCode's ACP support is
maintained by its vendor. ACP models one editor user per session, so it cannot carry who sent a message or which
group it came from; Hermes and OpenClaw need those, so they keep their own plugins.
See [`reference/harness-interfaces.md`](reference/harness-interfaces.md).

## D-006 Prefer official interfaces and libraries

**Decision.** Build each driver on the vendor's official interface or library where one exists. Use a community
Rust library only when it is clearly keeping up with the harness. Accept TypeScript and Python where the official
library requires them.

**Why.** Official libraries follow the harness as it changes. The cost is three languages, of which Python is
needed only for Hermes.

## D-007 One shared event vocabulary

**Decision.** Agents send one shared set of Marmot events, as unsigned Nostr-shaped events like the rest of
Marmot chat. Each event carries three things: an item type from the shared vocabulary, a plain rendering any
client can show, and the harness's native payload.

**Why.** Clients must be able to draw whatever arrives without knowing every harness. The shared item types and
the plain rendering give them that. The native payload keeps full fidelity for clients that want to do more.

## D-008 Pass everything through

**Decision.** Reasoning, tool calls, tool results, plans and other agent activity are sent to the chat. The old
rule in the MDK connectors, "never expose reasoning or tool output", does not apply here.

**Why.** The goal is to be true to what the agent is doing and give people the full picture.

## D-009 Groups are first-class, and every member sees agent activity

**Decision.** An agent in a group with several people is a first-class case, not an afterthought. Everyone in the
group sees the agent's activity and its details.

**Why.** People want to share an agent, for example one agent at home that answers a whole household.

## D-010 The owner sets who may prompt and who may approve

**Decision.**

- The agent's owner sets who in a group may prompt the agent: nobody else, named people, or everyone.
- The owner separately sets who may approve actions and use controls.
- Both default to the owner only.
- A person who addresses the agent without permission is told that they are not allowed to.
- The first version reads these settings from configuration on the agent's machine. Changing them from the chat
  is wanted later.

**Why.** Seeing and controlling are different. An owner may want others to watch an agent work without being able
to direct it or approve what it does on the owner's machine.

How each action maps onto the two rights is in [D-032](#d-032-every-action-needs-one-of-two-rights). The words
"and use controls" above are replaced by [D-039](#d-039-controls-need-the-right-to-prompt): controls need the
right to prompt.

## D-011 Full control from the phone

**Decision.** The aim is as much control from the chat as the harness gives at the terminal: interrupt, steer
mid-turn, start and resume sessions, switch model and mode, and run the harness's slash commands.

**Why.** The main user is a person controlling their own agents remotely. Anything they cannot do from the phone
sends them back to the terminal.

## D-012 Word-by-word streaming is not required now

**Decision.** Step-level events (a tool started, a tool finished, a message finished) are enough for the first
version. Word-by-word streaming needs a side channel and is lower priority.

**Why.** The existing side channel in MDK, the QUIC preview stack, has never worked reliably and is not in use.
Step-level events need no side channel.

## D-013 Keep every current connector, and expect more

**Decision.** Claude Code, Codex, OpenCode, Pi, Goose, Hermes and OpenClaw all stay supported, and more will be
added. Claude Code, Codex, Hermes and OpenClaw matter most.

**Why.** Broad availability is a goal. Small, easy-to-maintain drivers make it affordable and leave effort for
the harnesses with the most users.

## D-014 Outside developers can write connectors, later

**Decision.** Someone outside the project should eventually be able to write a connector without touching MDK.
This is a direction, not a requirement for the first version.

**Why.** The socket protocol already has to carry the full event model for Hermes and OpenClaw, so it is the
natural public boundary once it is stable.

## D-015 Build order

**Decision.**

1. The event model, the socket protocol and the core program.
2. In parallel: Codex, Claude Code, Hermes and OpenClaw.
3. The ACP driver, which brings Goose, OpenCode and the long tail.
4. Pi, then Cursor.

**Why.** Three of the four most important harnesses connect over the socket, so the event model and the socket
are on the critical path. Pi stays on the current MDK harness until its driver lands.

## D-016 Decisions and design live in this repository; work items live in issues

**Decision.** Decisions and design are Markdown files under `docs/`, changed by pull request. Work items are
GitHub issues, with a tracking issue once the event model is settled. The wiki is not used.

**Why.** Files are reviewed, versioned with the code, and visible to agents working in a checkout. A wiki is a
separate repository with no review. Issue threads spread a decision across comments and never show the current
answer.

## D-017 Who this is for

**Decision.** The main user is a person controlling their own agents from their phone. Sharing an agent with
other people in a group is also a first-class use (D-009).

**Why.** That is what people are asking for.

## D-018 Documents serve both people and agents

**Decision.** There are two kinds of document. People get short, plain pages with generated diagrams, starting
from a design overview that can be understood in three minutes. Agents may get long, detailed reference pages.
Diagrams are generated from a script, not drawn by hand.

**Why.** People need to step into the design gradually. Agents need exact detail. One page cannot do both well.

## D-019 MIT licence, public repository

**Decision.** The project is licensed under the MIT licence, with the Internet Privacy Foundation as copyright
holder, and the repository is public.

**Why.** It matches MDK and the rest of the organisation's repositories, and the design is meant to be read and
built on by others ([D-014](#d-014-outside-developers-can-write-connectors-later)).

## D-020 The socket speaks agent-control v3, with a clean cutover

**Decision.**

- The socket protocol is `marmot.agent-control.v3`. It keeps the conventions of v2: a Unix socket and one JSON
  message per line.
- The core does not speak v2 and there is no compatibility layer. The connectors move to v3 in one cutover.
- A connection opens with a hello: the connector states its protocol version, what it is and what it supports,
  and authenticates once.
- v3 has a **driver section**, which mirrors the driver trait, so a program on the socket is a remote driver.
- v3 has a **chat section** for ordinary messages: send, edit, delete, react, send media and read history.
  Hermes and OpenClaw need it because their agents use message tools on their own initiative.
- v3 has no live preview stream. That waits for the streaming question
  ([Q-14](open-questions.md#q-14-what-happens-to-the-quic-preview-stack)).

**Why.** Every existing client of v2 is ours and is being rewritten for the new design anyway, so compatibility
would protect nobody. v2 is built around chat messages and administration; of its 54 request types, 17 are used
by none of our clients and 7 belong to the preview stream. Starting clean keeps the new core from inheriting
that shape. See [`reference/current-connectors.md`](reference/current-connectors.md#which-requests-each-client-uses).

Decided 2026-11-17. How administration is done is still open
([Q-05](open-questions.md#q-05-how-is-the-agent-administered)).

## D-021 Approval behaviour belongs to the harness

**Decision.** Every harness has its own permission mode: ask about everything, ask about some things, or never
ask. That mode is set up on the harness. The core does not add approval modes of its own on top, per chat or
otherwise, and it does not answer an approval request itself. It passes requests to the chat and answers back.

What the chat gets is a control that changes the harness's own setting, as part of
[D-011](#d-011-full-control-from-the-phone). Who may use that control is covered by
[D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve).

What happens to a request that nobody answers is the harness's behaviour too. Hermes, for one, expires it after
five minutes and does not run the command.

**Why.** A second layer of permissions in the core would have to mimic each harness's own, and the two could
disagree. The harness already has the setting, and people already know it from using the harness directly.

Decided 2026-11-17, in response to [issue 7](https://github.com/marmot-protocol/marmot-agents/issues/7), which
proposed a per-chat approval mode in the core.


## D-022 The agent's words are ordinary chat messages

**Decision.** What an agent says to the people in a chat is sent as a normal chat message, kind 9, the same as
a person's message. It carries one extra tag naming the turn it belongs to. Everything else an agent produces
is an agent item ([D-023](#d-023-one-kind-carries-every-agent-item)).

**Why.** Every Marmot client already shows chat messages, so the agent's answer is readable everywhere, and
replies, reactions, search and alerts work with no extra work. Putting the words in a new kind would hide them
from any client that does not know it. See [`design/event-carrier.md`](design/event-carrier.md).

Decided 2026-11-17.

## D-023 One kind carries every agent item

**Decision.** Turns, tool calls, approvals, questions, plans, tasks, notices, errors and reasoning are all sent
as one kind of event, kind 1201. The item's type is a tag inside the event. Kind 1202 stays reserved.

This holds while the item types share one shape. Today they do: each carries the same parts and differs only in
its own fields. A type whose shape is really different gets its own kind.

**Why.** One kind keeps the rules for updating, ordering and size in one place, and gives a client that does not
support agents one thing to ignore.

Decided 2026-11-17. The kind's definition belongs in the Marmot specification
([Q-03](open-questions.md#q-03-where-are-the-event-kinds-specified)).

Kind 1202 is no longer reserved: it carries answers and controls
([D-027](#d-027-answers-and-controls-are-one-kind-of-event-kind-1202)). An eleventh type, `session`, was added by
[D-030](#d-030-a-session-is-an-item-the-eleventh-type).

## D-024 Who an agent event alerts

**Decision.** An agent event alerts people in three cases: the agent's final answer, an approval or a question
that is waiting for someone, and a turn that failed. Nothing else alerts: not tool calls, reasoning, plans,
tasks, notices, or a turn starting or finishing normally.

**Why.** A phone should buzz when a person is needed or the work is finished, and stay quiet while the agent
works. A waiting approval matters most, because Codex and Claude Code wait for an answer with no time limit.

A failed turn arrives as an edit of the turn item ([D-025](#d-025-an-agent-item-is-updated-with-an-edit)). That
edit alerts. No other edit does.

Decided 2026-11-17. Whether the agent's remarks during a turn alert is still open.

## D-025 An agent item is updated with an edit

**Decision.** An item's first event is a kind 1201 event. To change the item, the agent sends an edit, kind
1009, that points at that first event and carries the whole item again. The item is identified by the id of its
first event. There is no separate item id.

Edits get three additions for this, in the Marmot specification:

1. An edit may target an agent item, and then its tags replace the item's tags as well as its text.
2. An edit of an agent item carries a version number, and the highest version wins.
3. A client holds an edit whose item has not arrived yet, instead of dropping it.

**Why.** Marmot already has a way to change an event, and a second one would have to be specified, built and
tested beside it. Apps that have not been updated also behave better: they show each item once, not once per
update. The version number is needed because edits are ordered by a timestamp in whole seconds, and a tool can
start and finish within one second.

The cost: an edit means nothing without the event it points at, so someone added to a group in the middle of a
turn sees nothing of the items that began before they joined. See
[`design/event-carrier.md`](design/event-carrier.md#an-item-is-updated-with-an-edit).

Decided 2026-11-17.

## D-026 An agent item is at most 64 KB

**Decision.** An agent item is at most 64 KB. When one would be larger, the driver first leaves out the native
message, then cuts the output, keeping the start and the end, and says in the event what it did.

**Why.** 64 KB carried every one of 16,841 tool outputs measured in real use; 16 KB would have cut one in 70.
Storage is not a constraint at these sizes. The only constraint is relays that accept small events: an item
near the limit is about 86 KB by the time it reaches a relay. We accept that. 64 KB is not large, and we want
relays to accept more.

Decided 2026-11-17. How the full output is fetched when it was cut is still open
([Q-02](open-questions.md#q-02-what-happens-to-large-output)).

## D-027 Answers and controls are one kind of event, kind 1202

**Decision.** What a person's app sends back to an agent is an event of kind 1202. That covers an answer to an
approval or a question, and a control such as "interrupt". The event carries:

- the agent it is for, in a `p` tag;
- what it acts on, in an `e` tag that points at the item's first event;
- the action, in a tag: `answer`, `interrupt`, `new_session` or `set`;
- the action's details, as JSON in a tag;
- a plain rendering, such as "Allow once", in `content`.

An agent ignores one that does not name it. An answer or a control is not a row in the chat, alerts nobody and
does not count as unread.

**Why.** Answers and controls have one shape, so one kind is enough. Sent as chat messages they would alert
people, show as chat in every client, and could reach the model as a prompt by mistake.

Decided 2026-11-18. This replaces the line in [D-023](#d-023-one-kind-carries-every-agent-item) that kept kind
1202 reserved. The exact tags are not fixed, and the kind's definition belongs in the Marmot specification
([Q-03](open-questions.md#q-03-where-are-the-event-kinds-specified)). See
[`design/answers-and-controls.md`](design/answers-and-controls.md).

## D-028 An answer or a control is a request, and the agent's reply settles it

**Decision.** Sending an answer or a control changes nothing by itself. The agent resolves each one with exactly
one event that points back at it:

- the edit that shows its effect, if it had one: "Answered: Allow once, by Alice";
- a notice that says why, if it had none: not allowed, not supported, nothing to act on, already answered, or
  failed.

An app shows a press as pending until that reply arrives.

The rules for answers:

- An answer counts only if it points at the exact item and comes from someone allowed to give it.
- Ordinary text is never an answer to an approval. "Yes" in the chat is a prompt.
- An answer never reaches the model as a prompt. It goes to the harness's own answer call.
- An item is answered once. The first valid answer the agent receives wins.
- An answer names choices by the ids the item offered.
- An answer for an item that is no longer waiting has no effect.

Two exceptions: a question that the harness expects to be answered by the person's next message, and a gateway
that takes an approval as a typed command ([D-032](#d-032-every-action-needs-one-of-two-rights)).

Late and repeated events:

- One that points at an item acts only if that item is still live.
- One that points at nothing, such as "new session", is dropped if it is more than five minutes old.
- Each acts at most once.

**Why.** Two people can answer at the same moment, and only the agent knows which answer the harness took. So
the agent's own items are the one place to look for what happened. The earlier approvals design (MDK issue 858,
Marmot pull request 503) required an acknowledgement that harnesses do not give. This keeps its rules about who
may answer and drops its lifecycle guarantees.

Five minutes is a first value with no evidence behind it. It can change.

Decided 2026-11-18.

## D-029 Three shared controls, and the rest comes from the harness

**Decision.** Three controls mean the same thing for every harness: `interrupt` stops the running turn,
`new_session` ends the chat's session and starts a fresh one, and `set` changes one setting of the session.

Everything else comes from the harness. Each driver publishes, per session:

- its settings, each with an id, a label, the current value and the values it accepts. Model, mode and
  permission mode are settings;
- its own actions, such as compacting;
- its slash commands.

An app draws its controls from that list. Nothing in an app or in the core checks a harness's name.

**Why.** Models, modes and commands differ for every harness and change with every release. A shared list would
be wrong for all of them. This follows from staying true to each harness
([D-008](#d-008-pass-everything-through)) and from full control from the phone
([D-011](#d-011-full-control-from-the-phone)).

Decided 2026-11-18.

## D-030 A session is an item, the eleventh type

**Decision.** The event model gets an eleventh item type, `session`. An agent sends one when a session starts in
a chat, and edits it when a setting changes or the session ends. It carries the harness and its version, the
session's id, and the three lists of [D-029](#d-029-three-shared-controls-and-the-rest-comes-from-the-harness):
settings, actions and slash commands.

It is sent as kind 1201, like every other item ([D-023](#d-023-one-kind-carries-every-agent-item)), and it
alerts nobody.

**Why.** An app needs the list before anyone presses anything, and needs the last known one when the agent's
machine is off, so it has to be in the chat. It changes over time and an app has to find it by its type. A
notice, the alternative, is a line of information for people.

Decided 2026-11-18. The recordings were checked against the first ten types. The prototype does not produce
`session` items yet.

## D-031 A slash command is a message for the harness

**Decision.** A message that starts with a slash is passed to the harness exactly as typed. The core has no
typed commands of its own. Its controls are kind 1202 events, which an app sends from a button or a menu.

**Why.** The two can never collide, because one is text and the other is not. Three of the four recorded
harnesses already take their commands as messages.

The cost: a Marmot client with no support for agents can prompt an agent and use its slash commands, and cannot
approve, interrupt or change settings.

Decided 2026-11-18.

## D-032 Every action needs one of two rights

**Decision.** [D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve) gives the owner two rights to
hand out: who may prompt, and who may approve. Every action a person can take needs one of them. There is no
third right.

| Action | Right needed |
| --- | --- |
| Send a prompt or a slash command | Prompt |
| Answer an approval | Approve |
| Change the harness's permission mode | Approve |
| Type a command that approves, or that changes the permission mode | Approve |

A typed command is a message, so the core cannot know what it does. Each connector therefore lists the commands
that need the right to approve, and the core checks a message that starts with one of them against that right.
Hermes takes `/approve` and `/deny` as messages, and OpenClaw takes `/approve`.

**Why.** Two rights are enough to separate directing an agent from letting it act on the owner's machine. The
permission mode decides what the agent may do without asking, so changing it is a standing approval.

Decided 2026-11-18. The project owner asked for the assignment to be checked carefully. Which right the other
controls need is in [D-039](#d-039-controls-need-the-right-to-prompt).

## D-033 The socket's framing is JSON-RPC 2.0, one message per line

**Decision.** Messages on the socket are [JSON-RPC 2.0](https://www.jsonrpc.org/specification), one message per
line, over a Unix socket. Either side can send a request. A message is at most 4 MB.

**Why.** v2 already sends one JSON message per line, which is easy to write in any language and easy to record
for tests. JSON-RPC adds requests, responses and notifications without inventing them. Codex's app server and
ACP use the same framing, so libraries exist in every language we need.

Decided 2026-11-18. Windows has not been looked at.

## D-034 The core does the shared work, and a connector speaks only in agent events

**Decision.** A program on the socket sends agent events and receives prompts, answers and controls. It never
sees a key, ciphertext, a relay, group state, an event kind or a tag. The core does everything that is the same
for every harness:

- checks who may prompt and who may approve, and tells a person who was refused;
- decides whether a group message is for this agent;
- turns an agent event into a chat message, an agent item or an edit;
- numbers the items in a turn and the versions of an item;
- applies the size limit;
- decides who is alerted;
- reports the result of an answer or a control to the chat;
- closes open items when a connector goes away.

**Why.** Each of these rules then exists once. A connector written in TypeScript or Python stays small, and
cannot get the owner's rules wrong.

Decided 2026-11-18. See [`design/socket-protocol.md`](design/socket-protocol.md).

## D-035 Each harness install is its own identity, with its own sessions

**Decision.**

- Each harness install is its own Marmot identity, and so its own contact. Codex, Claude Code and Hermes on one
  machine are three contacts.
- An identity can have many sessions. Each chat with it has its own session, so a person can have several chats
  with the same harness. A new session can also be started in the same chat.
- On the socket, one connection is one agent.

**Why.** A person addresses the agent they want, so no command is needed to switch between harnesses. This is
also how today's connectors work. [Issue 8](https://github.com/marmot-protocol/marmot-agents/issues/8) proposed
the opposite, one identity answering for several harnesses.

Decided 2026-11-18. How several identities run and behave together is still open
([Q-08](open-questions.md#q-08-how-do-several-agent-identities-run-together)), and so is listing and resuming
older sessions ([Q-07](open-questions.md#q-07-how-does-a-chat-map-to-a-harness-session)).

## D-036 The ids a connector sees are the core's own

**Decision.** The id of a chat and the id of a sender, as passed to a connector, are stable values the core
makes up for this install. They are not the group's id or a person's public key. Names are passed as they are,
because the model needs them.

**Why.** A gateway keeps its own logs and transcripts. Nothing in them should identify a group or a person to
anyone who reads them.

Decided 2026-11-18.

## D-037 A prompt is delivered at most once

**Decision.** If the connection to a connector drops before a prompt was acknowledged, the core reports the
prompt as not delivered. It never sends it again.

**Why.** Running a prompt twice is worse than losing one. A person can see that a prompt was lost and send it
again.

Decided 2026-11-18. What else happens to a turn on a restart is still open
([Q-27](open-questions.md#q-27-what-happens-to-a-turn-when-the-core-or-a-driver-restarts)).

## D-038 Each connector has its own token

**Decision.** The socket is in a folder only the owner's user account can open. Each connector has its own
token, in a file only that user can read, created by the core when the owner adds the connector. The token says
which agent the connector drives, and that it is a connector and not an administrator. Administration needs a
different credential.

This is accepted as enough for now, with its limit: it stops other users of the machine, and it does not stop a
program running as the same user, which can read the token files and the core's own data. A harness with a
shell runs as that user. Real separation means running the core as a different user. That is possible later
and not planned now.

**Why.** It tells connectors apart and keeps a connector from using administration by accident, at almost no
cost in setup.

Decided 2026-11-18. How administration is done is still open
([Q-05](open-questions.md#q-05-how-is-the-agent-administered)).

## D-039 Controls need the right to prompt

**Decision.** These need the right to prompt: interrupting a turn, starting a new session, changing a setting
other than the permission mode, using a harness's own action such as compacting, and answering a question.

Answering an approval and changing the permission mode need the right to approve, as
[D-032](#d-032-every-action-needs-one-of-two-rights) says.

This replaces the words "and use controls" in
[D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve), which put every control with the right to
approve.

**Why.** A harness's slash commands are messages
([D-031](#d-031-a-slash-command-is-a-message-for-the-harness)), and they include commands that stop a turn,
start a session and switch the model. Someone who may prompt can type those. A stricter rule for the button
than for the typed command would protect nothing.

Decided 2026-11-18. Still to check when drivers are built: that each connector's list of commands needing the
right to approve is complete. Whether Hermes and OpenClaw have typed commands that change the permission mode
has not been recorded.

## D-040 A message sent during a turn goes straight to the harness

**Decision.** The core keeps no queue of its own. A message that arrives while a turn is running is passed to
the driver at once, and the harness does with it what it does natively:

| Harness | What it does |
| --- | --- |
| Codex, Hermes | Folds the message into the running turn |
| Claude Code | Ends the turn and starts another |
| OpenClaw | Holds the message until the turn ends |

Each connector states which of these its harness does, so an app can say what will happen before the message
is sent.

**Why.** A queue in the core would be a second behaviour on top of the harness's own, and the two could
disagree. People already know how their harness treats a message sent mid-turn.

The cost: in a group, with a harness that ends the turn, a second person's message cuts off the first person's
turn.

Decided 2026-11-18.

## D-041 A secret answer is accepted only in a two-person chat

**Decision.** A harness can mark a question's answer as secret, for something like a password. Such a question
can be answered from the chat only when the chat has two members: the agent and one person. In any other chat
the question is shown, cannot be answered there, and says to answer it at the terminal.

The agent's reply says that the question was answered and never repeats the answer.

**Why.** Everything sent to a group is readable by every member
([D-009](#d-009-groups-are-first-class-and-every-member-sees-agent-activity)). In a two-person chat the only
reader besides the agent is the person who typed it.

The cost: the answer stays in that chat's history on that person's devices.

Decided 2026-11-18. Codex's interface has this marking. No recording contains a secret question.

## D-042 An agent may act in its own chat, and may not change a group or the account

**Decision.** Hermes and OpenClaw give their agents message tools. Through the socket's chat section such an
agent may:

- send a message;
- edit or delete a message of its own;
- add and remove a reaction;
- read history.

It may do these only in the chat its session belongs to. They are ordinary Marmot actions, which the core
carries out.

Not in the first version: anything that changes a group or the account, such as renaming a chat, creating a
group, inviting or removing people, or changing the agent's profile. That waits for the design of
administration ([Q-05](open-questions.md#q-05-how-is-the-agent-administered)).

Codex and Claude Code have no such tools, and none are added now.

**Why.** Sending, editing, deleting and reacting are what any member of a chat can do with their own messages,
and gateways already rely on them. Keeping an agent to its own chat stops a prompt in one group from causing
messages in another. Changing a group or the account is the owner's business, and a harness that could do it
could be talked into it.

Because these are ordinary actions, a deleted message is an ordinary deletion and a reaction is an ordinary
reaction. No new item type is needed for them. The tool call itself still shows as a `tool` item.

Decided 2026-11-18. How an agent sends a file is still open
([Q-23](open-questions.md#q-23-how-does-an-agent-send-a-file-to-the-chat)).

## D-043 A setting says which right it needs

**Decision.** Each setting in a `session` item carries a mark when changing it needs the right to approve. A
setting without the mark needs the right to prompt.

**Why.** [D-032](#d-032-every-action-needs-one-of-two-rights) puts the permission mode with the right to
approve. Which settings those are differs by harness: Codex has two, the approval policy and the sandbox, and
Claude Code has one. Only the driver knows, and the core must not check a harness's name.

Decided 2026-11-18.

## D-044 A connector supplies what its harness does not say

**Decision.** A `session` item holds what the harness reports about the session. Where the harness reports
nothing, the connector fills it in from its own knowledge of that harness:

- the settings and slash commands of a gateway that reports none;
- the values a setting accepts, when the harness does not list them.

**Why.** In the recordings Hermes tells a plugin nothing about a session, and OpenClaw tells it only the model
and the thinking level. No harness lists the values a setting accepts unasked; only Codex lists any, and only
its models. Without this an app has nothing to draw controls from.

The cost: a connector's list can fall behind its harness. That is the same risk as its list of commands that
need the right to approve ([D-039](#d-039-controls-need-the-right-to-prompt)).

Decided 2026-11-18.

## D-045 A session item's commands keep their names, and lose descriptions first

**Decision.** The list of slash commands in a `session` item always carries each command's name and hint. When
the item would be too large, the descriptions are dropped first.

**Why.** Claude Code's 59 commands take 1.7 KB as names and 15 KB with descriptions, a real install has more,
and every edit sends the whole item again. The size rule
([D-026](#d-026-an-agent-item-is-at-most-64-kb)) says how to cut output, not a list. A name is enough to
complete a command as someone types.

Decided 2026-11-18.

## D-046 A setting change is an edit, and a notice only when nobody asked for it

**Decision.** When a setting changes, the agent edits the `session` item. That edit is the record. A notice is
sent as well only when nobody asked for the change, for example when the harness moves to another model by
itself or comes back from a resume with different permissions.

**Why.** A person who changed a setting sees their press resolve
([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)), so a notice would repeat
it. A change nobody asked for is different: the edit lands where the session item first appeared, which can be
far up the chat, and nothing else would mark it in the flow of the conversation.

Decided 2026-11-18. Found while running the `session` item over the recordings: a resumed Codex thread came
back with a wider sandbox than it started with.
