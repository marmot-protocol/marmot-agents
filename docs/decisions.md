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

What the Codex driver builds on is replaced by
[D-055](#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own): types of its own, and neither
OpenAI's protocol crates nor `codex-codes`.

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

A secret question that is no longer waiting goes without its native payload
([D-056](#d-056-a-secret-question-carries-a-native-message-only-while-it-is-waiting)).

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

Replaced by [`plan.md`](plan.md) ([D-068](#d-068-work-follows-the-plan-and-the-builder-decides-what-is-internal)),
in which Codex runs end to end before the socket is built.

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
  (Q-14, since settled as [D-067](#d-067-the-quic-preview-stack-is-removed)).

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

Decided 2026-11-17. Whether the agent's remarks during a turn alert was left open, and is settled by
[D-078](#d-078-the-agents-remarks-during-a-turn-do-not-alert).

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

Replaced in part by [D-057](#d-057-the-core-applies-the-64-kb-limit): the core applies the limit, not the
driver. The limit and the order stand.

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
controls need is in [D-039](#d-039-controls-need-the-right-to-prompt). The check on typed commands was
widened by [D-047](#d-047-a-listed-command-says-where-in-a-message-it-counts).

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
(Q-27, since settled as [D-070](#d-070-a-restart-closes-the-turn-as-interrupted)).

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

A secret question carries a native message only while it is waiting
([D-056](#d-056-a-secret-question-carries-a-native-message-only-while-it-is-waiting)).

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

## D-047 A listed command says where in a message it counts

**Decision.** Each connector lists the typed commands that need the right to approve
([D-032](#d-032-every-action-needs-one-of-two-rights)). For each one it also says whether the command counts
only at the start of a message or anywhere in it. The core checks every message accordingly. A message with
such a command from someone without the right to approve is not passed on, and the person is told.

This replaces the words "a message that starts with one of them" in D-032.

**Why.** OpenClaw honoured `/exec security=full ask=off` at the start, in the middle and at the end of a
message, from an ordinary member of a group, and the command that followed ran with no approval. A check of
how a message starts misses two of those three.

The cost: someone who only mentions such a command in a sentence is refused too.

Decided 2026-11-18. The lists as found are in
[`design/answers-and-controls.md`](design/answers-and-controls.md#typed-commands-that-widen-what-an-agent-may-do).
Neither gateway marks these commands, so each list is the connector's own and can fall behind.

## D-048 A request to confirm a command is a question

**Decision.** When a gateway asks a person to confirm a command they typed, the connector sends that as a
`question` item. Answering it needs the right to prompt, like any question
([D-039](#d-039-controls-need-the-right-to-prompt)).

A typed `/approve` or `/deny` counts as an answer to an approval, and needs the right to approve, only while
an approval is waiting. While a confirmation is waiting and no approval is, it answers the confirmation.

**Why.** In Hermes `/approve` answers an approval and also confirms `/new`. Without this, someone who may
prompt and may not approve could ask for a new session and never confirm it.

Decided 2026-11-18.

## D-049 A standing approval is an ordinary approval answer

**Decision.** "Always" and "for this session" are answers to an approval like any other. They need the right
to approve and nothing more. What a standing approval then covers is the gateway's behaviour
([D-021](#d-021-approval-behaviour-belongs-to-the-harness)), and the approval item says what it covers when
the gateway tells the connector.

**Why.** In both gateways, after one person answered "always", the same command from another person ran with
no prompt. In Hermes the standing approval covers the reason a command was flagged, such as "delete in root
path", and not the one command. The core cannot narrow that without a second layer of permissions, which
D-021 rules out. It can make sure the person who gave it had the right to, and show the group what was given.

Decided 2026-11-18.

## D-050 The core program is called `marmot-agent`

**Decision.** The core program is `marmot-agent`. Its crates take the name as a prefix: for example
`marmot-agent-events` for the item types, `marmot-agent-driver` for the driver trait and `marmot-agent-codex`
for the Codex driver.

**Why.** The name says what the program is and matches the repository and the socket
(`marmot.agent-control`). It is not tied to one app, so it suits anyone who uses a Marmot client. It differs
from today's `wn-agent`, so both can be installed while an install moves over
([Q-19](open-questions.md#q-19-how-do-existing-installs-move-over)). The name was free on crates.io when
checked on 2026-11-19.

Decided 2026-11-19.

## D-051 Kinds 1201 and 1202 take the new shape, and the old one is not kept

**Decision.** Agent items use kind 1201 and answers and controls use kind 1202, in the shapes designed here.
The shapes that MDK builds for those kinds today are replaced, not kept beside the new ones. Nothing is added
to tell the two apart, and no new kind numbers are taken.

**Why.** The old shapes are hardly used. Today's connectors send a kind 1201 in two cases only, and a kind 1202
in one ([what they carry today](design/event-carrier.md#what-kinds-1201-and-1202-carry-today)). Keeping both
shapes would put a second set of rules into MDK and every app for almost no traffic.

An old connector that is still running will send the old shape for a while. A client shows such an event as
it shows any agent item it cannot read.

Decided 2026-11-19.

## D-052 The Codex driver is built first

**Decision.** The first code is the item types, the driver trait and the Codex driver. It is tested by
replaying the Codex recordings and requiring the events the prototype produces
([`tools/prototype`](../tools/prototype/README.md)).

The plan after that, which is not fixed, and is replaced by [`plan.md`](plan.md)
([D-068](#d-068-work-follows-the-plan-and-the-builder-decides-what-is-internal)):

1. MDK learns the new shapes: agent items, edits of them, and answers and controls. This can run beside the
   first step.
2. The core: the account, the socket, rights, sessions and delivery, with the Codex driver built in.
3. The Claude Code driver, over the socket.
4. The Hermes and OpenClaw plugins.

**Why.** The Codex driver needs nothing from MDK, so it can start at once, and it is the cheapest test of the
design in real code. It also settles three open questions by doing them: which Codex library to build on
(Q-15, since settled as [D-055](#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own)), the capability flags
(Q-16, since settled as [D-058](#d-058-the-driver-trait-and-the-capability-flags)) and how drivers are tested
(Q-17, since settled as [D-074](#d-074-every-driver-is-tested-by-replay-and-by-one-end-to-end-scenario)). The core cannot send an agent item until MDK changes (in 1201 and 1202; the first test sends items under a
temporary kind before then, [D-069](#d-069-until-mdk-takes-the-new-shape-agent-items-travel-under-a-temporary-kind)).
The gateways come last because whose rules govern them is still open
([Q-22](open-questions.md#q-22-whose-rules-govern-a-gateway)).

Decided 2026-11-19.

## D-053 The Rust workspace builds with MDK's compiler and forbids unsafe code

**Decision.**

- The Rust code is one workspace, with its crates under `crates/`.
- It is pinned to the compiler MDK is pinned to, which is Rust 1.97.1 today, and uses edition 2024. When MDK
  moves to another compiler, this repository moves with it.
- Unsafe code is forbidden in every crate. Cargo's own configuration sets this for the whole workspace, so a
  new crate cannot leave it out.
- `Cargo.lock` is committed, and CI builds with `--locked`.
- The crates are not published to crates.io.

**Why.** The core will link MDK as a library
([D-002](#d-002-one-rust-core-program-with-mdk-as-a-library)), so both must build with one compiler. Nothing
planned here needs unsafe code, and the program will hold keys. A committed lock file means CI and every
machine build the same dependencies.

Decided 2026-11-19.

## D-054 Changes arrive as small signed pull requests that CI checks

**Decision.**

- Each step of the work is one small pull request, on its own branch from `master`. Only the project owner
  merges.
- Every commit is signed.
- CI runs on every pull request and on every push to `master`: formatting, Clippy with warnings as errors, the
  tests, the prototype over all the recordings, and the links between the Markdown files.
- The link check is [lychee](https://github.com/lycheeverse/lychee), run without network access. It is a tool
  that CI runs, not a dependency of the code.
- The CI workflow pins each action to a commit and each downloaded tool to a version and a checksum, can only
  read the repository, and uses no secrets.
- Work here does not change the MDK repository.

**Why.** Small changes are easy to review, and the owner decides what goes in. The documents and the recordings
are as much the project as the code, so they are checked the same way. A workflow that can only read, pinned to
commits, has nothing to leak and cannot be changed from outside.

A link check of our own was written first. Three reviews each found Markdown it read wrongly. Lychee read
the cases that were tried on it correctly.

Decided 2026-11-19. That only the project owner merges is replaced by
[D-068](#d-068-work-follows-the-plan-and-the-builder-decides-what-is-internal).

## D-055 The Codex driver reads Codex's messages with types of its own

**Decision.**

- The Codex driver keeps each message from `codex app-server` as the JSON it arrived as. It reads a message
  through small types written in this repository, which hold only the fields the translation uses.
- It links neither OpenAI's protocol crates nor `codex-codes`.
- The interface is still OpenAI's own, `codex app-server`. Codex's own schema, from
  `codex app-server generate-json-schema --experimental` at the version that was recorded, is the reference
  for what the driver reads and sends.

This replaces what the Codex row of [D-005](#d-005-which-driver-each-harness-gets) says the driver builds on.

**Why.** All three candidates were built and run against the 28 Codex recordings
([the trial](reference/harness-interfaces.md#codex-which-library-to-build-on)).

- OpenAI's protocol crate, at `rust-v0.169.0`, could not be resolved into one program with MDK at
  `6f6d2017d`: the two need different versions of SQLite's Rust binding, and the core must link MDK
  ([D-002](#d-002-one-rust-core-program-with-mdk-as-a-library)). It also built only with OpenAI's lock file
  and four of their patches, and it is 721 crates.
- `codex-codes` is light and well kept, but it covers only the stable half of the protocol. It drops the
  choices a command approval offers and has no types for the settings requests.
- Neither keeps a message unchanged, so the driver has to keep the JSON itself whichever it uses.

This sets aside the official library that
[D-006](#d-006-prefer-official-interfaces-and-libraries) prefers, for Codex only. D-006 stands for the other
drivers.

The cost: the types are ours to keep right, and nothing fails to compile when Codex changes a message. A new
recording, replayed in CI, is what shows a change.

One limit of the evidence: the trial showed reading. The requests and replies the driver sends are built by
hand too, and that was not tried. The trial page proposes checks for it. They are proposals, not part of
this decision.

Decided 2026-11-19.

## D-056 A secret question carries a native message only while it is waiting

**Decision.** A question item that holds a secret question carries a native message only while it is
waiting for an answer. Once it is answered, cancelled or failed, its events carry none. If it was answered,
the item says so, and never what the answer was
([D-041](#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat)).

This is an exception to "never drop the native payload" ([D-007](#d-007-one-shared-event-vocabulary)). The
only other one is the size limit, which leaves the native message out of an item that would be over 64 KB
([D-026](#d-026-an-agent-item-is-at-most-64-kb)). The core applies it, not the driver
([D-057](#d-057-the-core-applies-the-64-kb-limit)). While the question waits, its native message is the
harness's request, which holds no answer, and it is kept.

**Why.** After a question is answered, the message that records it holds the answer. For Codex that message
is the reply the client sends to Codex's request. Passing it on would put a password into the history of every
member's device, where it cannot be taken back. A few events without a native message cost little.

The rule does not depend on an answer having been filled in, so that a driver cannot miss it by leaving the
answer out: an item counts as waiting only when its state says so and it records no answer. The item types
hold it: an event that breaks it cannot be built, and one that arrives as JSON cannot be read. For the same
reason the questions in one item must have ids that differ. The types cannot read a native message, so a
driver's own tests still have to show that it never hands over that reply as if it were the request.

Decided 2026-11-19. The project owner first decided that the answer to a secret question carries no native
message, and then accepted this wider form when review showed the narrower one could be stepped round. No
recording contains a secret question, so this has been tested with a hand-written one only.

## D-057 The core applies the 64 KB limit

**Decision.** The core applies the size limit of [D-026](#d-026-an-agent-item-is-at-most-64-kb) to every agent
item, whichever driver or connector it came from. A driver never leaves anything out or cuts anything to keep
an item under the limit: it hands over the native message as it arrived, and text and output at full size. When
an item would be over 64 KB, the core leaves out the native message first, then cuts the output, keeping the
start and the end, and says in the event what it did.

This is about size only. A secret question that is no longer waiting still carries no native message
([D-056](#d-056-a-secret-question-carries-a-native-message-only-while-it-is-waiting)), and its driver leaves
that out.

This replaces "the driver" in D-026. The limit, and the order in which parts are left out, stand.

**Why.** D-026 says the driver applies the limit.
[D-034](#d-034-the-core-does-the-shared-work-and-a-connector-speaks-only-in-agent-events), a day later, lists
it among the shared work the core does, and the socket page follows D-034. Only the core knows an item's final
size, because the core turns the event into the item. In one place, the rule exists once, and a connector
written in another language cannot get it wrong.

Decided 2026-11-19.

## D-058 The driver trait and the capability flags

**Decision.** Every Rust driver implements the trait in
[`crates/marmot-agent-driver`](../crates/marmot-agent-driver). It has the operations of
[the socket's driver section](design/socket-protocol.md#the-driver-section), and no others:

| On the socket | In the trait |
| --- | --- |
| The hello: the harness, and what it can do | `harness`, `capabilities` |
| The requests `prompt`, `answer`, `control` | `prompt`, `answer`, `control`. Each returns once the driver has taken or refused it, with one of the eight reasons |
| The notification `event` | `next_event`: the next agent event, with the chat it belongs to, the prompts a turn responds to and the requests it settles |

- **A request is taken once the driver has handed it to the harness.** It does not wait for the harness to
  confirm, because a harness may never say.
- **Each answer and control carries an id the core makes up, and the event that shows its effect names it**,
  including when the effect is that it failed
  ([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it),
  [D-046](#d-046-a-setting-change-is-an-edit-and-a-notice-only-when-nobody-asked-for-it)).
- **When the harness stops, the driver closes what it can first.**
- **When a driver ends, for any reason, the core closes the items still open**, as it does when a connector
  goes away ([D-034](#d-034-the-core-does-the-shared-work-and-a-connector-speaks-only-in-agent-events)), and
  settles every answer and control still open with a notice that it failed. A prompt is not settled this way:
  one that was not taken is not delivered, and is never sent again
  ([D-037](#d-037-a-prompt-is-delivered-at-most-once)).
- **An event says which chat it belongs to**, whenever the driver knows. "New session" names a chat, and a
  session only if there is one to end.
- **An answer says which kind it is.** To an approval it gives one choice. To a question item it gives, for
  each question, the question's id, the ids of the options chosen and any typed text. What is delivered to the
  harness is a different type from what the question item records
  ([D-041](#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat)).
- **A refusal's detail is shown in the chat**, to every member. A driver puts the harness's own words there
  only when it can be sure they do not repeat an answer to a secret question, and otherwise leaves them out.
- **Starting and stopping a driver are not in the trait.** Each driver has its own settings.

A driver states seven capability flags, every one, every time:

| Flag | Values | Codex | Who reads it |
| --- | --- | --- | --- |
| `interrupt` | yes or no | yes | An app, to offer the button |
| `mid_turn` | `fold`, `restart`, `queue` | `fold` | An app, to say what a message sent during a turn will do |
| `sessions` | `core`, `own` | `core` | The core, to know whether it names the session |
| `resume` | yes or no | yes | The core, after a restart |
| `approvals` | yes or no | yes | An app, to know whether approval requests will arrive in the chat |
| `files_in` | none, images, any | images | An app, to allow an attachment |
| `approval_commands` | a list; each entry has the command and whether it counts at the start of a message or anywhere | none | The core, on every message |

The socket's draft flags `tool_items`, `reasoning` and `questions` are left out: the items already say them.

An approval command does not say when it needs the right to approve.
[D-048](#d-048-a-request-to-confirm-a-command-is-a-question) stands: a typed `/approve` or `/deny` needs it
only while an approval is waiting. A flag with that test was tried and taken out, because an approval can
start after the core's check and before the harness reads the message, and the same word then approves the
agent's action. Closing that gap needs a way to tie a confirmation to the exact request it confirms. That is
for the Hermes connector to design, and until a replacement is accepted D-048 is the rule.

The socket's driver section is a draft and has not caught up. Its answer has a flat list of choices, its
events have no chat and settle nothing, its requests carry no id, and its `files_in` is yes or no. When the
socket is designed it takes the shapes above.

**Why.** A built-in driver and a connector on the socket do the same job, so the core treats them the same way.
A flag is kept only when the core or an app does something different because of it before anything has
happened. The trait needs no async runtime, so each driver picks its own.

What a debug print shows is not part of this decision. See Q-13, since settled as
[D-071](#d-071-what-may-be-logged).

Decided 2026-11-19. The project owner accepted the trait, and then the seven flags.

## D-059 Eight points about what an agent event carries

**Decision.**

1. **An event has no version number of its own.** The socket and the Marmot event kind carry versions.
2. **The native message is the harness's whole payload**: for Codex, the whole `params` of its message, or the
   whole `result` of a reply, as the exact text that arrived.
3. **An event the driver makes up carries no native message**, such as an item it closes because the harness
   never did. The harness said nothing. The event's state says what happened.
4. **`seq` counts the events of one session**, from 1, in the order the driver produced them.
5. **The answer a question item records gives, for each question, its id, the ids of the options chosen and
   any typed text.** A driver turns the harness's labels back into ids. Anything that matches no label is
   typed text. A secret question records its id only
   ([D-041](#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat)).
6. **A plan's explanation is its `text`**, and its steps are its `entries`.
7. **A tool's `error` is text.** When a harness sends an object, the driver takes its message, and the object
   stays in the native message.
8. **A message's `phase` is `commentary` or `final`.** Any other phase, such as Codex's `partial_answer`, is
   left out, and the native message keeps the harness's word.

**Why.** The event page, the prototype and the item types disagreed on each of these
([Q-01](open-questions.md#q-01-what-exactly-is-in-an-agent-event)), and the Codex driver could not be tested
against the prototype until they agreed. In each case the answer is the one that keeps the event model's own
promises: the native message is the harness's own and unchanged, the item types carry ids rather than a
harness's words, and nothing is added that one harness alone would need.

Decided 2026-11-19. The project owner accepted the recommendation on each point.

## D-060 A Codex translator follows one thread, and the transport routes

**Decision.** The Codex driver's translator follows one Codex thread at a time: the chat's session. The reply
to the client's `thread/start` or `thread/resume` picks it. A chat can replace its session, and the translator
then ends the old session item. A message that names another thread makes no event in that translator, and is
counted.

One Codex connection can carry several threads, so the transport, the half of the driver that runs Codex,
gives each translator only its own thread's messages. It:

1. gives each chat its own translator, and sends it the messages that name that chat's thread;
2. sends Codex's reply to a request to the translator that made the request;
3. has at most one request to start or resume a session waiting for each chat;
4. replaces a chat's session only when its thread is idle: no turn running, or an interrupt that Codex has
   confirmed.

What the transport does with a thread that no chat asked for is open
([Q-29](open-questions.md#q-29-what-does-the-codex-driver-do-with-a-thread-no-chat-asked-for)).

**Why.** Review of the translator went five rounds. From the third on, every finding was one case of a single
translator receiving several threads' messages: a late message from a replaced thread landing in the new one's
turn, then its settings, a resumed session, delayed replies, an MCP approval and a refused request. None of
them is in any of the 28 recordings. Handling them inside the translator meant routing between threads there,
and the transport has to route anyway, to serve several chats on one connection. Routing is now in one place,
and the translator stays a translation of one session. The four rules above are what make the reviewed cases
impossible.

Decided 2026-11-20. The project owner chose this over keeping each thread's state inside the translator.

## D-061 A Codex session starts in a working folder, with a safe sandbox unless configured

**Decision.** The core gives the Codex driver a working folder, and may give a sandbox and an approval policy.
Without them, a session's sandbox is Codex's `workspace-write`: commands may write inside the working folder and
the machine's temporary folders (`/tmp` and the folder `$TMPDIR` names), and nowhere else. Codex asks before
anything outside that (`on-request`). Nothing wider is used unless it is configured. The driver
sends all three whenever it starts or resumes a session.

**Why.** A Codex thread resumed without its settings came back with full access, the default on the machine
that recorded it ([rule 10 for drivers](design/event-model.md#what-a-driver-has-to-do)). Sending them every
time means a session does what it was set up to do, whatever Codex's own configuration says. The defaults are
the narrowest that still let an agent work on a project. The temporary folders are writable because many tools,
such as package installers, test runners and compilers, write there; closing them would make an agent ask for
approval for ordinary work. This is what Codex's `workspace-write` does for any Codex user.

Decided 2026-11-20. The project owner accepted the recommendation, and that the temporary folders stay
writable.

## D-062 What Codex reads for a chat message

**Decision.** In a two-person chat Codex reads the message's text alone. In a group it reads the sender's name
and the text, as `Alice: run the tests`. A reply first quotes the message it answers, as
`> Bob: which file?`, followed by a blank line. An image goes as an image Codex reads from the file the core
wrote. A message that starts with a slash goes exactly as typed, with no name and no quote
([D-031](#d-031-a-slash-command-is-a-message-for-the-harness)).

**Why.** Codex has one user per thread and cannot tell people apart, so in a group the name is the only way it
knows who asked. In a two-person chat there is only one person, and a name would add nothing. A slash command
has to reach Codex as the command it is.

Decided 2026-11-20. The project owner accepted the recommendation. The slash-command exception follows from
D-031.

## D-063 Codex keeps a session so it can be resumed

**Decision.** The Codex driver starts each session as one Codex keeps on disk (`ephemeral: false`), so it can
be resumed after Codex or the core restarts.

**Why.** The capability flags say Codex resumes sessions
([D-058](#d-058-the-driver-trait-and-the-capability-flags)), and a session Codex threw away could not be. Codex
keeps the conversation in its own home folder on the machine that runs it, as it does for any Codex user.

Decided 2026-11-20. The project owner accepted the recommendation.

## D-064 The Codex driver declines an MCP server's form until forms are designed

**Decision.** When an MCP server asks, through Codex, for a form to be filled in, the Codex driver declines it
at once and shows a notice in the chat that a form it cannot show was declined. A request to approve an MCP
tool is not a form, and is shown and answered as an approval.

**Why.** No recording holds a form, so everything the driver would do with one came from Codex's schema, and
review of that kept finding cases it got wrong: answers recorded as text, not as the options chosen, fields of
kinds it could not show, forms that ask the person to open a link. Declining at once means Codex never waits
for an answer that cannot come, and the chat says what happened. Forms become a step of their own, built from a
recording of one.

Decided 2026-11-20. The project owner accepted the recommendation.

## D-065 A prompt names a session only to resume one

**Decision.** For a driver whose `sessions` flag is `core`, the core keeps each chat's session, and a prompt
names it only to take it up again: after the driver has started, when that chat had a session, on each prompt
until the driver has taken one for the chat or taken a "new session" for it. Every other prompt names no
session, and the driver sends it to the chat's current session, or starts one if the chat has none. A driver
that gets a prompt naming a session the chat does not have resumes that session.

The core names only a session of that chat's own. The driver refuses a session it knows belongs to another
chat, but it knows only the sessions it has seen since it started, so the core's record is what keeps one
chat's history out of another after a restart.

**Why.** While a prompt named the session it was for on every turn, it could name one the chat had just left.
A "new session" control returns once it is handed to the harness, before the new session is reported, so the
core's next prompt still named the old one, and the driver could not tell that from a wish to go back to it.
Review of the Codex transport found this in three rounds running; each fix closed one timing and the next
round found another. Naming a session only to resume it leaves nothing to tell apart: the driver always
knows the chat's current session better than the core does.

Decided 2026-11-20. The project owner accepted the recommendation, after four rounds of review on the Codex
transport.

## D-066 The core finishes every request it starts

**Decision.** The core waits for each call of `prompt`, `answer` and `control` to return, and never drops
one before it does. Only the future from `next_event` may be dropped, as
[D-058](#d-058-the-driver-trait-and-the-capability-flags) says. To stop waiting on a driver, the core drops
the request and the driver together, and never uses that driver again; a driver that runs a harness stops it
when it is dropped. A request is never dropped with the driver kept.

**Why.** A request can be partway through when it is dropped: written in part, or waiting for the harness to
answer a request it has sent. The driver then cannot know whether the harness has it, and the same request
sent again can arrive twice, or leave a session marked as being taken that nobody takes. Review of the Codex
transport found such cases in each of its four rounds, and each fix added more state to the transport. A
request is short: it returns once the driver has handed it to the harness, so there is no reason to give up
on one. A harness that never answers is a driver that has stopped
working, and ending it is the remedy.

Decided 2026-11-20. The project owner accepted the recommendation.

## D-067 The QUIC preview stack is removed

**Decision.** Word-by-word previews over QUIC are removed: the broker, the stream crates, kind 1200, and the
group component that sets a stream's policy. Nothing here sends previews, and the socket has no preview
stream. An agent's activity reaches the chat as items, each sent when it starts and edited as it changes
([D-025](#d-025-an-agent-item-is-updated-with-an-edit)), and its words as messages. If word-by-word text is
wanted later, the agent's message item can be edited every second or two while it is written, with no side
channel. This settles Q-14.

**Why.** The stack never worked reliably, only Hermes could use it, and only when configured. The new design
does not need it: drivers drop partial text, and a busy turn already sends a few dozen items as it works. It
costs a broker service, crates, storage, an MLS group component and API surface in the apps. Removing it from
MDK, the Marmot specification and the apps is the project owner's to arrange. A group that already carries the
component must keep working when it is gone.

Decided 2026-11-21. The project owner chose removal, and deprecates it in the specification.

## D-068 Work follows the plan, and the builder decides what is internal

**Decision.** [`plan.md`](plan.md) is the go-ahead for the work it lays out. Work moves through its milestones
without asking for each step.

- **The builder decides** whatever is internal and can be changed later, inside the decisions already made:
  how the code is laid out, defaults, which cases to handle, how to test. A choice that others can see is
  recorded here; the rest is not.
- **The project owner decides** what other programs, apps or people see (event shapes, socket messages,
  anything MDK or the apps read), privacy and security, who may do what, any change to a decision, and the
  plan's scope and order.
- **Questions for the owner are gathered**, normally once per milestone, each with a recommendation and the
  reason. Meanwhile work carries on with the recommendation, marked as provisional, except on privacy or
  security, which wait.
- **A pull request is one piece of work that makes sense alone**, on its own branch from `master`, with every
  commit signed and CI passing. A milestone takes several.
- **A code change gets one round of review** by two other models. A blocker is fixed, and checked again only
  if the fix is large. Lesser findings are fixed when cheap, or kept as issues. Decisions and documents are not
  reviewed this way.
- **The builder merges** a pull request once CI passes and its review left no blocker unfixed. The owner reads
  the work at the end of each milestone, and can revert anything.

The first point of [D-054](#d-054-changes-arrive-as-small-signed-pull-requests-that-ci-checks), that only the
owner merges, is replaced by the last point here. The plan after the Codex driver in
[D-052](#d-052-the-codex-driver-is-built-first) is replaced by [`plan.md`](plan.md).

**Why.** Step by step, each step waited on the owner, and choices that follow from the design came back as
questions. Review went round after round on cases nothing had yet shown to happen. The goal now is to replace
MDK's connectors quickly, and that needs a runway: a plan agreed once, the owner's attention kept for what only
the owner can decide, and real use deciding which edge cases matter.

Decided 2026-11-21.

## D-069 Until MDK takes the new shape, agent items travel under a temporary kind

**Decision.** Until MDK supports the new shape of kinds 1201 and 1202
([D-051](#d-051-kinds-1201-and-1202-take-the-new-shape-and-the-old-one-is-not-kept)), the core sends an agent
item, and the test client sends an answer or a control, as an app event of a kind MDK does not reserve, chosen
in the code. Each update of an item is a new event carrying the item's id and a higher version, in place of an
edit; a reader keeps the highest version of each item. The agent's words are chat messages, as
[D-022](#d-022-the-agents-words-are-ordinary-chat-messages) says. Only test groups are used this way. The
switch to 1201, 1202 and edits is made in one place, when MDK is ready.

**Why.** MDK refuses kinds 1201 and 1202 from programs and builds them only in the old shape, and it applies an
edit only to a chat message or a comment. Waiting for MDK would hold up the first end-to-end test, and the shape
is better specified for MDK after that test than before it. A version per item is what ordered edits need
anyway ([the carrier](design/event-carrier.md)), so only the carrying is temporary.

Decided 2026-11-21.

## D-070 A restart closes the turn as interrupted

**Decision.** When the core or a driver stops during a turn, the turn and every item still open are closed as
interrupted, and every answer and control still open is settled as failed
([D-058](#d-058-the-driver-trait-and-the-capability-flags)). A core that stopped without doing so does it when
it starts again, from the items it kept as open. Nothing is sent again. The chat's session is kept, and the next
prompt resumes it ([D-065](#d-065-a-prompt-names-a-session-only-to-resume-one)). The core never sends the model a prompt that nobody wrote. This settles Q-27.

**Why.** The recordings show Codex and Claude Code both keep the session when stopped mid-turn, rerun nothing,
and send nothing to say the turn ended, so someone has to close it. The simple version keeps all three things a
restart must not break: the saved session, no prompt run twice, no turn left running forever. A continuation
turn ([issue 9](https://github.com/marmot-protocol/marmot-agents/issues/9)) would put words in the agent's mouth,
and can be added later.

Decided 2026-11-21.

## D-071 What may be logged

**Decision.** A log, at any level, never holds message text, prompts, model output, a native message, a file's
contents, keys, Nostr public keys or event ids, group ids, relay addresses, file paths, or a harness's session or
turn ids. It may hold item types and states, kinds, counts, sizes, durations, error kinds, and the ids the core
makes up for its own requests. What a type prints for debugging follows the same rule, so the item types in
`marmot-agent-events` stop printing their contents. This settles Q-13.

**Why.** This is a private messenger, and logs get pasted into bug reports and kept on disk. The allowed list is
what is needed to see what the program did without seeing what anyone said. MDK's own logs work the same way,
with a privacy-safe kind for each error.

Decided 2026-11-21.

## D-072 One agent identity per core process, until the swap

**Decision.** Until the swap, each run of the core is one agent identity
([D-035](#d-035-each-harness-install-is-its-own-identity-with-its-own-sessions)) with one harness, one account
home and one configuration, as each connector is today. Several identities in one program
([Q-08](open-questions.md#q-08-how-do-several-agent-identities-run-together)) waits until after the swap.

**Why.** It is how today's installs are laid out, so moving one over maps one home to one process. Each
identity's keys stay in their own process. Several in one program is easier to run but needs product work, and
it is not needed to replace what exists.

Decided 2026-11-21.

## D-073 Standing settings live in a configuration file

**Decision.** The core reads one configuration file at start: the account home, the relays, who owns the agent,
who may prompt and who may approve ([D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve), the owner
alone by default), who may invite it, and the harness with its settings, such as Codex's working folder and
sandbox ([D-061](#d-061-a-codex-session-starts-in-a-working-folder-with-a-safe-sandbox-unless-configured)). A
change takes effect when the core starts again. There is no administration over the socket and no changing of
permissions from the chat before the swap. This settles the first part of
[Q-05](open-questions.md#q-05-how-is-the-agent-administered), and defers
[Q-10](open-questions.md#q-10-how-are-permissions-changed-from-the-chat).

**Why.** It is the smallest way to set what the core needs, it is what today's connectors do with their
environment and allow-lists, and nothing that can talk to the core can widen its own access through it.

Decided 2026-11-21.

## D-074 Every driver is tested by replay, and by one end-to-end scenario

**Decision.** Each driver is tested three ways. Its translation replays its recordings and must produce the
events the prototype produces for them, as the Codex driver does. The part that runs the harness is tested
against a stand-in. And one scripted scenario runs the core, the driver and the real harness on a local relay,
with the test client as the person. CI runs the first two. The scenario needs the harness installed and signed
in, so it runs on demand, and must pass before a milestone closes. This settles Q-17.

**Why.** Replay catches what a harness really sends, a stand-in catches the order things can arrive in, and only
the scenario shows the pieces work together. The Codex driver was tested the first two ways, and review kept
finding cases that only a running system could confirm or rule out.

Decided 2026-11-21.

## D-075 In a group the agent answers when mentioned or replied to; in a direct chat, always

**Decision.** In a chat with one person, every message is for the agent. In a group, a message is for the agent
only when it mentions the agent or replies to one of the agent's messages; any other message is not a prompt,
and the agent says nothing about it. A message that is for the agent is then checked as any prompt is: from
someone without the right to prompt, the core says so
([D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)). Answers to approvals and questions, and
controls, are not affected: each names the item or the agent it is for. The rest of how several agents share a
group ([Q-08](open-questions.md#q-08-how-do-several-agent-identities-run-together)) is still open.

**Why.** In a group people talk to each other more than to the agent, and an agent that took every message as
a prompt would answer conversations that were not for it. A mention or a reply is how a person addresses
anyone in a chat. In a direct chat there is nobody else to address.

Decided 2026-11-21. The project owner gave this rule.

## D-076 The tags of kinds 1201 and 1202

**Decision.** Agent items, their edits, and answers and controls carry the tags in
[kinds 1201 and 1202](design/agent-kinds.md): an item's plain rendering in its content; its version, state,
title, harness, the harness's id for it, its place in the session, and its type with its own fields as JSON, in
tags; its session, turn, starting message, the requests it settles, the native message and what the 64 KB limit
left out, when there are any. A later version is an edit that carries the whole item and a higher version. An
answer or a control names its agent and its item, and says what it asks as JSON. This is what the project
hands to MDK and the Marmot specification. MDK may rename a tag to fit its conventions; what each carries stands.

**Why.** The first end-to-end test (M1) ran every step of its scenario with this shape, under a temporary kind
([D-069](#d-069-until-mdk-takes-the-new-shape-agent-items-travel-under-a-temporary-kind)), and nothing it needed
was missing. One JSON tag for a type's own fields keeps the specification to one set of tags for eleven types.

Decided 2026-11-21. The project owner accepted the proposal from M1.

## D-077 A refused notice says why something was not acted on

**Decision.** The event model's notice gains a kind, `refused`: a message, an answer or a control that was not
acted on, with a reason in a word (`not_allowed`, `busy`, `unknown_item`, `not_waiting`, `already_answered`,
`nothing_to_do`, `not_supported`, `invalid`, `failed`) and a sentence in its text. The core sends it, settling the
request it answers ([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)), or
about the message it answers ([D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)). A driver
does not.

**Why.** D-028 needs a notice that says why an answer or a control had no effect, and none of the notice kinds
said it. With its own kind and a reason an app can read, an app can clear the pending press and say why in its
own words.

Decided 2026-11-21. The project owner accepted the proposal from M1.

## D-078 The agent's remarks during a turn do not alert

**Decision.** When the harness marks the agent's words as a remark made while it works, not its answer, the
core sends them as a chat message with the tag `["phase", "commentary"]`, and MDK does not alert for a chat
message that has it. The final answer alerts, as
[D-024](#d-024-who-an-agent-event-alerts) says. Words the harness does not mark alert as any chat message does.

**Why.** In M1, Codex said what it was about to do before doing it, and each remark alerted like the answer: a
long task would buzz many times. A phone should buzz when a person is needed or the work is finished. The
remarks stay chat messages ([D-022](#d-022-the-agents-words-are-ordinary-chat-messages)), so every app still
shows them.

Decided 2026-11-21. The project owner accepted the recommendation. This settles what D-024 left open.

## D-079 The settings name the agent

**Decision.** The settings file may give the agent a name. The core publishes it as the agent's name and
display name each time it starts, keeping the rest of its profile, and publishes nothing when the name is
already right. Without a name in the settings, the profile is left as it is. A blank name is refused.

**Why.** An agent made with `marmot-agent init` otherwise shows under a name MDK makes up, such as "Neat Pigeon".
Publishing at every start makes a changed name take effect on the next start, with one path for the first run
and for a rename.

Decided 2026-11-21. The project owner accepted the recommendation.
