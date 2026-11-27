# Decisions

This is the record of what has been decided and why. It exists so that settled points are not argued again.

- Add an entry when a decision is made. Give it the next number, the date, the decision and the reason.
- Do not edit an old entry to change its meaning. Add a new entry that replaces it, and mark the old one
  "Replaced by D-0NN".
- When an entry is added or replaces another, update [`current-state.md`](current-state.md) in the same change
  ([D-114](#d-114-a-current-state-page-sits-beside-this-record-and-every-new-decision-updates-it)).
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
OpenAI's protocol crates nor `codex-codes`. The Cursor row is replaced by
[D-080](#d-080-cursor-is-driven-through-acp): Cursor goes through the ACP driver. What the Pi driver builds on
is replaced by [D-086](#d-086-the-pi-driver-reads-pis-rpc-messages-with-types-of-its-own): Pi's records read
with types of its own, not `pi-codes`.
The Hermes and OpenClaw rows are extended by
[D-113](#d-113-hermes-and-openclaw-are-channel-plugins-with-a-narrower-contract-than-a-driver): each plugin is a
channel plugin, with a narrower contract than a driver.

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

Narrowed for the pilot by
[D-136](#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group):
a chat that is not sent the agent's full activity is sent each item without its native payload.

## D-008 Pass everything through

**Decision.** Reasoning, tool calls, tool results, plans and other agent activity are sent to the chat. The old
rule in the MDK connectors, "never expose reasoning or tool output", does not apply here.

**Why.** The goal is to be true to what the agent is doing and give people the full picture.

Narrowed for the pilot by
[D-136](#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group):
everything is still passed through, but outside the owner's two-person chat with the agent only to the groups the
owner lists.

## D-009 Groups are first-class, and every member sees agent activity

**Decision.** An agent in a group with several people is a first-class case, not an afterthought. Everyone in the
group sees the agent's activity and its details.

**Why.** People want to share an agent, for example one agent at home that answers a whole household.

Narrowed for the pilot by
[D-136](#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group):
every member of a group sees the agent's words, its turns, what it asks and its errors; its full activity only
when the owner lists the group.

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

An exception, by [D-122](#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar): a subagent's words are kind
1111 comments in its thread. The agent's own words stay kind 9.

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

Extended by [D-122](#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar): the agent's comments in its own
subagents' threads do not alert, and an app lifts a subagent's waiting approval or question into its running bar. A
subagent's approvals and questions alert as any do.

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

Extended by
[D-124](#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners): an app
shows a held edit of an item sent before the member joined as the item itself.

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

Noted 2026-11-27: the tags were fixed by [D-076](#d-076-the-tags-of-kinds-1201-and-1202). The action is not a tag
of its own: one `details` tag holds it as JSON, with an `action` of `choice`, `questions`, `interrupt`,
`new_session`, `set` or `own`, as [kinds 1201 and 1202](design/agent-kinds.md#kind-1202-an-answer-or-a-control)
has it.

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

Extended by [D-123](#d-123-stop-and-writing-in-a-subagents-thread): a Stop that reaches several subagents is settled
once, when the last of them has ended, by the edit of whichever ended last.

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

Extended by [D-123](#d-123-stop-and-writing-in-a-subagents-thread): `interrupt` in the main chat also stops every
subagent the agent started, including ones still running after its turn has ended, and in a subagent's thread it
stops that subagent and the ones it started.

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

Refined by [D-129](#d-129-a-prompt-whose-answer-a-dropped-connection-lost-is-told-as-perhaps-not-delivered):
a prompt the connector may have had when the connection went is told as perhaps not delivered.

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

An app should not offer to answer one in a chat of more than two members
([D-181](#d-181-an-app-should-not-offer-to-answer-a-secret-question-in-a-chat-of-more-than-two-members)).

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
(Q-23, since settled as [D-090](#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder)).

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
(Q-19, since settled as [D-089](#d-089-an-install-moves-over-by-importing-its-account-home-in-place)). The name was free on crates.io when
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
(Q-22, since settled as [D-084](#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through)).

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

Noted 2026-11-27: MDK's history was rewritten on 2026-11-24, and `6f6d2017d`, cited above, is no longer on it: it
was the merge of MDK pull request 3375.

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
for the Hermes connector to design, and until a replacement is accepted D-048 is the rule. For an answer sent as an
answer, not typed, [D-146](#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id) closes it.

The socket's driver section is a draft and has not caught up. Its answer has a flat list of choices, its
events have no chat and settle nothing, its requests carry no id, and its `files_in` is yes or no. When the
socket is designed it takes the shapes above.

**Why.** A built-in driver and a connector on the socket do the same job, so the core treats them the same way.
A flag is kept only when the core or an app does something different because of it before anything has
happened. The trait needs no async runtime, so each driver picks its own.

What a debug print shows is not part of this decision. See Q-13, since settled as
[D-071](#d-071-what-may-be-logged).

Extended by [D-123](#d-123-stop-and-writing-in-a-subagents-thread) and
[D-124](#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners): a
driver states two more flags, whether a person can write to a running subagent and whether its harness tells
subagents apart. Both are named when built.

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

What the transport does with a thread that no chat asked for is open (Q-29, since settled for subagents'
threads as [D-125](#d-125-the-codex-transport-follows-subagents-threads)).

**Why.** Review of the translator went five rounds. From the third on, every finding was one case of a single
translator receiving several threads' messages: a late message from a replaced thread landing in the new one's
turn, then its settings, a resumed session, delayed replies, an MCP approval and a refused request. None of
them is in any of the 28 recordings. Handling them inside the translator meant routing between threads there,
and the transport has to route anyway, to serve several chats on one connection. Routing is now in one place,
and the translator stays a translation of one session. The four rules above are what make the reviewed cases
impossible.

Extended by [D-125](#d-125-the-codex-transport-follows-subagents-threads): the transport also gives each subagent's
thread a translator in the chat of the thread that started it, holds messages for a thread it does not know yet, and
interrupts subagents itself.

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

Decided 2026-11-21. Replaced by
[D-112](#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by): MDK took the new
shape, and the switch was made.

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

Extended by
[D-124](#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners): when a
harness stops, its session ends or the core restarts, every open `task` item is edited to `cancelled`.

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

Extended by [D-121](#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them):
an item in a subagent's thread also carries NIP-22's thread tags, `E`, `K` and `P` for the top-level subagent's
`task` item and `e`, `k` and `p` for the immediate parent's; what an edit carries of them is left open there.
Extended by [D-123](#d-123-stop-and-writing-in-a-subagents-thread): a `task` item says whether a person can write to
its subagent.

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

A rule beside this one, by [D-122](#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar): the agent's
comments in its own subagents' threads do not alert.

Decided 2026-11-21. The project owner accepted the recommendation. This settles what D-024 left open.

## D-079 The settings name the agent

**Decision.** The settings file may give the agent a name. The core publishes it as the agent's name and
display name each time it starts, keeping the rest of its profile, and publishes nothing when the name is
already right. Without a name in the settings, the profile is left as it is. A blank name is refused.

**Why.** An agent made with `marmot-agent init` otherwise shows under a name MDK makes up, such as "Neat Pigeon".
Publishing at every start makes a changed name take effect on the next start, with one path for the first run
and for a rename.

Decided 2026-11-21. The project owner accepted the recommendation.

## D-080 Cursor is driven through ACP

**Decision.** Cursor uses the ACP driver built into the core, speaking `cursor-agent acp` from Cursor's official
command-line tool, in place of a TypeScript driver on `@cursor/sdk`. What Cursor adds beyond standard ACP is
handled as a small flavour of the ACP driver. This replaces the Cursor row of
[D-005](#d-005-which-driver-each-harness-gets).

**Why.** When M3 started, the SDK (`@cursor/sdk` 1.4.2) could not ask for an approval in a local run: every
tool call ran unasked, or the sandbox refused it with nobody asked. It also needed an API key even for local
runs. `cursor-agent acp` runs on the command-line tool's own sign-in and sends real permission requests (allow
once, allow always, reject once), and it reports models, modes and settings. It is still Cursor's own
interface, as [D-006](#d-006-prefer-official-interfaces-and-libraries) asks, and it reuses a driver that is
being built anyway.

Decided 2026-11-21. The project owner accepted the recommendation.

## D-081 The socket messages as built are accepted

**Decision.** The messages the core and a connector exchange over the socket are those in
[the messages, as built](design/socket-protocol.md#the-messages-as-built): the hello with its token and the
seven capability flags, the prompt, answer and control requests, and the event notification with its item,
chat, prompts and settles fields. They follow the driver trait
([D-058](#d-058-the-driver-trait-and-the-capability-flags)).

**Why.** M2 drove a real Claude Code through them end to end, with an approval, an interrupt, a new session
and a crash, and they needed nothing the trait lacks. Fixing them lets the other connectors be written against
a stable boundary.

Decided 2026-11-21. The project owner accepted the recommendation from M2.

## D-082 The Claude Code driver loads no Claude Code settings files

**Decision.** The Claude Code driver starts Claude Code with no settings files (the SDK's `settingSources` is
empty). No allow rule, hook or `CLAUDE.md` from the owner's own Claude Code setup applies in a chat.

**Why.** Rules written for one person at a terminal would otherwise approve or run things in a group chat
without anyone in the chat being asked, which [D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)
puts in the owner's settings in the core. The cost is that the agent does not read the owner's `CLAUDE.md`.

Decided 2026-11-21. The project owner accepted the recommendation from M2.

## D-083 The core passes no files to a harness yet

**Decision.** Until files are designed (Q-23, since settled as [D-090](#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder)),
the core passes no attachments to a harness, whatever the driver's `files_in` says. The Claude Code driver
reports `files_in` as `none`. The Codex driver reports `images`, which is what Codex accepts; it receives none
yet.

**Why.** Passing files on raises what may be read and from where, which is not designed. Text covers the
scenarios that prove the model.

Decided 2026-11-21. The project owner accepted the recommendation from M2.

## D-084 The core decides first, and a gateway allows what the core lets through

**Decision.** For Hermes and OpenClaw, as for every harness, the core decides who may prompt and who may approve
from the owner's settings ([D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)), and tells a
refused person why. Each gateway's plugin sets the gateway to allow whatever reaches it from the core: Hermes's
allow-list and administrator list admit what the plugin delivers, and OpenClaw is handed an open policy, with
its question whether a sender may approve answered from the core's verdict. This settles Q-22 for who may speak
and approve. How a gateway's own sessions map to chats stays open.

**Why.** One set of rules, in one place, for every harness. Hermes drops a refused sender without a word; the
core can say why.

Decided 2026-11-21. The project owner accepted the recommendation from M2.

## D-085 Cursor recordings keep only Cursor's built-in commands

**Decision.** When Cursor's sessions are recorded, the scrubber keeps only Cursor's built-in slash commands in
the command list `cursor-agent acp` sends, and drops the owner's personal commands and skills and the entries
Cursor syncs from the account (named `synced-` and ids). Recording uses the existing sign-in on the machine, and
no Cursor credential is copied anywhere.

**Why.** The command list carries the names of the owner's own commands and identifiers from the account, which
must not go into fixtures. A separate sign-in in a container would still get the synced entries, and would put a
credential in the container.

Decided 2026-11-21. The project owner accepted the recommendation.


## D-086 The Pi driver reads Pi's RPC messages with types of its own

**Decision.** The Pi driver keeps each record from `pi --mode rpc` as the text it arrived as, and reads it as
JSON in this repository's own code, with no library between. It does not link `pi-codes`. The interface is
still Pi's own RPC mode. This replaces what the Pi row of [D-005](#d-005-which-driver-each-harness-gets) says
the driver builds on.

**Why.** `pi-codes` 1.6.0 keeps up with Pi, but its client runs only a `pi` found on the path, where the driver
runs Pi through a script that starts a container, one per chat. It sends one command at a time, cannot answer
Pi's extension UI requests, and never reads Pi's standard error. Its typed records lose the exact text that each
event's native message must carry ([D-059](#d-059-eight-points-about-what-an-agent-event-carries)). The
reasoning is the same as for Codex ([D-055](#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own)),
and so is the cost: nothing fails to compile when Pi changes a record, and a new recording, replayed in CI, is
what shows it. See [the comparison](reference/harness-interfaces.md#pi-which-library-to-build-on).

Decided 2026-11-21. The project owner accepted the recommendation.

## D-087 The core settles a request whose effect never shows

**Decision.** When a driver takes a control and no event that shows its effect arrives within two minutes of
the driver taking it, the core settles it with a `refused` notice whose reason is `no_effect`. An effect that
arrives later is still shown; it just settles nothing. An answer is not settled this way: it keeps its place
until its item stops waiting, so a second answer is still refused. Each driver still settles what it can itself
([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)).

**Why.** In M3 the same gap turned up in Pi, ACP and OpenClaw: a setting set to the value it already had, or a
control the harness quietly dropped, produced no event, so the person's press stayed pending for good. Each
driver can close the cases it knows, but a net in the core keeps a new driver from leaving one open. Two minutes
is long enough for a slow harness to stop a turn, and short enough that a press does not look stuck.

Replaced in part by
[D-156](#d-156-the-no_effect-net-counts-when-output-reached-the-core-not-when-the-core-read-it): the two minutes
count when the effect reached the core, not when the core read it, and an effect the core reads before it settles
the control still settles it, even after the two minutes. The two minutes and the `no_effect` reason stand.

Decided 2026-11-21. The project owner accepted the recommendation.

## D-088 OpenClaw's `/model status` may show the masked model key

**Decision.** The OpenClaw plugin lets `/model status` through, and the chat sees the model key in the masked form
OpenClaw prints, which keeps a few of its characters. The plugin still replaces the whole key with a marker in
anything it sends. Recordings still never send `/model status`, so no masked form goes into the fixtures.

**Why.** The owner needs to check from the chat which model and key an agent uses. The masked form does not give
the key away, and refusing the command would hide what OpenClaw is set to.

Decided 2026-11-21. The project owner decided, against the recommendation to refuse the command.

## D-089 An install moves over by importing its account home in place

**Decision.** `marmot-agent import <old home>` takes over one of today's connector installs. It uses the
existing account home where it is, so the agent keeps its identity, its keys, its encryption state and every
group it is in, and nobody has to invite it again. Before the core first opens the store it takes a backup,
because MDK's storage upgrades run forward only and the backup is the only way back. It turns the old
allow-lists into the new settings, and stops and disables the old service without removing it. Each identity
is its own program ([D-072](#d-072-one-agent-identity-per-core-process-until-the-swap)). This settles Q-19.

**Why.** It meets the requirement from issue 10, with nothing to merge or split. The cost: going back after the
first open needs that backup, and restoring an older copy of a store that has since processed messages could
reuse keys or run a prompt twice, so a restore is only for before the new core did anything.

Decided 2026-11-21. The project owner accepted the recommendation.

## D-090 An agent sends a file as an encrypted attachment from an allowed folder

**Decision.** A file the agent produces goes to the chat as an ordinary encrypted chat attachment, through MDK's
media sending, tagged with its turn like the agent's words
([D-022](#d-022-the-agents-words-are-ordinary-chat-messages)). There is no new item type. The core reads a file
only from folders the settings list, by default the working folder, and never follows a link out of them. This
settles Q-23 and ends what [D-083](#d-083-the-core-passes-no-files-to-a-harness-yet) says only for files the
agent sends; files passed to a harness are still not designed.

**Why.** Today's Codex connector already sends files, and an attachment is what every app already shows. The
cost: an app cannot yet show a file as part of the agent's work, which a later item type could add.

Decided 2026-11-21. The project owner accepted the recommendation.

## D-091 One install script per harness, and release binaries from CI

**Decision.** Each harness gets an install script that installs `marmot-agent` and what that harness needs, and
sets the agent up as a service of the person's own account. Release binaries are built by GitHub Actions. Signed
releases and automatic updates wait ([Q-28](open-questions.md#q-28-how-does-someone-know-a-release-is-genuine)).

**Why.** It matches how today's connectors are installed, and is the smallest step that lets an install move
over. Signing is wanted, not needed to replace what runs today.

Decided 2026-11-21. The project owner accepted the recommendation.

## D-092 The choices made while building M3 are accepted

**Decision.** The choices the drivers were built on, marked provisional until now, stand:

- **Pi.** An extension's question is a `question` item and `notify` a notice, since Pi has no approvals of its
  own. A control Pi fails, or that a stopped Pi cut off, is settled by an error item the driver makes, and a
  prompt Pi refuses shows as an error item. Pi runs in a container by default, each chat's state in a folder of
  its own named from a hash of the chat id (`MARMOT_AGENT_CHAT`). After a restart a session that never held a
  message, and one whose file was deleted, are both made again empty (issue #63). (Replaced by
  [D-174](#d-174-the-pi-driver-remembers-across-a-restart-which-sessions-held-a-message): one that held a
  message is not made again, after a restart too.) A question asked while Pi
  starts fails the start. A second Compact while one runs is refused as busy. The default model is
  `deepseek/deepseek-v4.1-flash:free`. (Replaced by
  [D-100](#d-100-the-installers-defaults): there is no default model.)
- **ACP.** A streamed message's native message is the list of its chunks. An ACP agent reads a chat message as
  Codex does ([D-062](#d-062-what-codex-reads-for-a-chat-message)). A message during a turn cancels it and
  prompts (`restart`). A setting in ACP's `mode` category needs the right to approve. OpenCode's question tool
  and Cursor's to-do tool make no tool item, so each shows once, as a question or a plan.
- **OpenClaw.** Every chat is shown to OpenClaw as a group, so each has its own session. Its list of commands that
  need the right to approve stands. It queues a message sent during a turn, picks its own sessions, resumes them
  and forwards approvals. Its replies to typed commands, and a notice when it drops one, are `command_output`
  notices. The container's defaults: commands need approval, approval forwarding is on, `/bash` is off, nobody is
  OpenClaw's owner, and the agent's message tool has no actions. The entrypoint clears a dead gateway's state
  lock. After the core goes, the plugin connects again only once its cancelled runs have ended.
- **Hermes.** Hermes's approval mode is `manual`, so a person is asked about every command it flags. Its tool
  lines and heartbeats are commentary messages, its own notices `warning`, its replies to typed commands
  `command_output`. After a crash the plugin sends `/discard`. The model endpoint's address is removed from
  Hermes's model answer. `/approve`, `/deny`, `/yolo`, `/approvals`, `/memory`, `/skills` and `/codex-runtime` at
  the start of a message need the right to approve. A refused `/new` shows Hermes's reply. An answer to a
  question skips Hermes's own check of who may answer, as approvals do
  ([D-084](#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through)). The kept log holds
  only the plugin's own lines, so it needs no filter for the key.

**Why.** Each was built, reviewed and run end to end against the real harness. Recording them settles what other
programs and people see from these drivers.

Decided 2026-11-21. The project owner accepted the recommendations.

## D-093 The core and the drivers are verified in layers, with TLA+ models at the top

**Decision.** The core and the drivers are tested and verified as [the verification page](design/verification.md)
lays out: the properties it lists, each owned by one layer; a conformance suite every driver passes; a
deterministic simulator of the core with fault injection; TLA+ models checked with TLC, each with a fair
configuration that must pass and an unfair one whose counterexample is committed as a simulator scenario; Kani
proofs of small pure functions; property and differential tests (proptest, fast-check, hypothesis); fuzzing with
cargo-fuzz; mutation testing with cargo-mutants; and privacy checks. None of these tools ships in the program.
Known gaps are fixed first, each with a failing test.

**Why.** Review kept finding interleavings that example tests miss. TLA+ and TLC match what MDK uses, so both
projects share one tool and one way of working. Each check is shown to catch its bug, and a bounded result is
stated with its bounds.

Decided 2026-11-21. The project owner accepted the recommendations.

## D-094 CI on a pull request stays under 15 minutes, and the rest runs nightly

**Decision.** CI on a pull request stays under 15 minutes, using build caching and independent checks as parallel
jobs. Long checks run nightly or weekly: property tests at high counts, many simulator seeds, larger model bounds,
fuzzing, mutation testing, and the scenarios against real harnesses. Those run on numes.ai's free models, with its
key as a CI secret, in a workflow that pull requests cannot reach.

Replaced in part by [D-127](#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key): the
real-harness scenarios run in CI on the model double, and no model key is in CI at all. The 15-minute ceiling and
the nightly and weekly checks stand.

**Why.** A fast pull request keeps the work moving; the expensive checks still run every day. Keeping the key out
of anything a pull request can trigger keeps it from code nobody reviewed.

Decided 2026-11-21. The project owner accepted the recommendation, with the 15-minute ceiling.

## D-095 The import prints the paths it needs to undo a move

**Decision.** `marmot-agent import` prints to the terminal the paths of the old service files it stopped, the backup
it made and the settings it wrote. Logs still follow [D-071](#d-071-what-may-be-logged).

**Why.** The person running the import needs them to go back. The terminal is theirs; a log is not.

Decided 2026-11-22. The project owner accepted the recommendation.

## D-096 Four limits of the core that the simulator and the model accept

**Decision.** These are accepted as limits, not bugs, and the simulator and the `CoreLifecycle` model check around
them:

- A request with no target, read in time but undone by a crash and read again more than five minutes later, is
  dropped with no notice ([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)).
- If a crash loses the driver's "session ended" event before it is saved, the next prompt after the restart takes up
  that session again. Nothing can replay the event.
- When a version of an item is refused or given up, its versions have a gap; that is allowed once the chat has been
  told with a `failed` notice.
- When MDK also refuses the one replacement for a dropped send (D-097), what it would have closed or settled stays as
  it is. In practice that means the group is gone.

**Why.** Each needs MDK to refuse or lose things it rarely does, or would need state no program can recover.

Decided 2026-11-22. The project owner accepted the recommendations.

## D-097 What stands in for a send that is dropped

**Decision.** When MDK refuses a send for good, or the core gives one up after its tries:

- a closing version the chat never got is replaced by the core's own close: the item's next version, in its final
  state, with a `warning` notice body of reason `not_delivered` ("Not delivered"), and no native message;
- a `failed` notice that settled requests, or told the chat of a dropped version, is replaced by a fresh `failed`
  notice doing the same;
- each is replaced once, whatever MDK's reason; a replacement that is itself dropped is not replaced.

**Why.** Without it an item stayed open, or requests stayed pending, for good (found by the model). One small
replacement fits and cannot loop.

Decided 2026-11-22. The project owner accepted the recommendations.

## D-098 A harness that stops reading ends its connection

**Decision.** A driver that cannot hand a request to its harness within its patience (one minute) gives up on that
harness: it stops it, closes its open items and refuses the request, and later requests, as failed. An answer the
harness can no longer read is refused as failed, never shown as given. This applies to every built-in driver
(Codex, ACP and Pi).

**Why.** The core waits for each request ([D-066](#d-066-the-core-finishes-every-request-it-starts)), so a harness
that keeps its input open but stops reading would otherwise stall every chat. The conformance suite found it.

Decided 2026-11-22. The project owner accepted the recommendations.

Extended to the socket by [D-138](#d-138-a-connector-that-stops-reading-is-given-up-in-a-requests-time): a
connector that stops reading is given up after 30 seconds.
## D-099 Files from the agent's own home are refused by default, and the owner may allow them

**Decision.** By default the core refuses to send any file inside the agent's account home, which holds its keys
and encryption state, wherever the allowed folders are ([D-090](#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder)).
The owner may turn this off with one explicit line in the agent's settings. The installers never choose `~` itself
as the work folder.

**Why.** Safe defaults, and the owner keeps control of how their agent is set up. A work folder of `~` would
otherwise let the agent send its own keys.

Decided 2026-11-22. The project owner decided.

## D-100 The installers' defaults

**Decision.** The installers (D-091) put the program in `~/.local/bin/marmot-agent` and each agent in
`~/.local/share/marmot-agent/agents/<name>/`; name services `marmot-agent-<name>` (and `-connector` for a socket
harness), or `org.marmot.marmot-agent.<name>` on macOS; ask only the owner's key and the work folder; use White
Noise's relays for the agent's own account (inside a group, the group's relays apply); print the
`loginctl enable-linger` command rather than run it; and are published as one archive per platform from a version
tag. Goose runs in its own default mode, which asks nothing. There is no default model endpoint: a person brings
their own endpoint and key. Tests read the models they use from a local configuration file, and in CI from secrets,
and most tests use recorded sessions rather than live models.

**Why.** The project owner chose these from the proposed set. A default endpoint would send people's work to a
service they did not choose.

Decided 2026-11-22. The project owner decided; this replaces the installers' provisional defaults.

## D-101 A chat can move a coding harness to another folder

**Decision.** For the coding harnesses (Codex, Claude Code, OpenCode, Goose, Cursor and Pi), each chat has a folder
setting. A chat that has chosen none uses the agent's install folder. Someone with the right to approve sets it from
the chat, by path; it applies from the chat's next session, so setting it also starts one. The owner lists in the
settings the top-level folders agents may work in (by default the install folder), and a chat may pick any folder
inside them; listing `~` lifts the limit ([replaced by D-104](#d-104-how-a-chats-folder-works-as-built):
`~` allows the home's folders, and `/` lifts the limit). Hermes and OpenClaw keep the workspace they manage themselves.

**Why.** Working in a different folder or project is a baseline need for a coding agent. The harness picks its folder
when a session starts. The owner's list keeps a chat member from pointing the agent anywhere on the machine, and the
right to approve matches what a folder changes: what the agent can touch.

Decided 2026-11-22. The project owner decided.

## D-102 The installers connect to the harness the person already has

**Decision.** The installers never install a harness. They assume it is already there: Codex, Pi, Goose, OpenCode and
Cursor on the person's `PATH`, and Hermes and OpenClaw already installed, into which the installer adds our plugin.
They work the same on Linux and macOS. Containers are used only by this project's own tests and CI. Pi runs as the
person's own `pi` by default, with the container kept as an option; this replaces what
[D-092](#d-092-the-choices-made-while-building-m3-are-accepted) says about Pi running in a container by default.

**Why.** People install and update their harness themselves, as with today's connectors. Running it directly removes
Docker as a requirement and the macOS limit. Pi asks nobody before a tool runs, so natively it acts with the person's
own rights, as it does when they run it themselves; the container stays for those who want a boundary.

Decided 2026-11-22. The project owner decided.


## D-103 A bind mount into the agent's home is an accepted limit of D-099

**Decision.** The core finds the agent's home by walking up `..` from the folders it holds open
([D-099](#d-099-files-from-the-agents-own-home-are-refused-by-default-and-the-owner-may-allow-them)). A bind mount can show a folder or file of
the home somewhere outside it, and that walk does not see the home, so a file reached that way is not refused. This
is accepted as a known limit; the core does not read the mount table.

**Why.** Only someone allowed to create bind mounts can set one up, which the agent cannot do. Reading the mount
table adds code that differs between systems for a case the person creates on purpose.

Decided 2026-11-22. The project owner decided.

## D-104 How a chat's folder works, as built

**Decision.** For [D-101](#d-101-a-chat-can-move-a-coding-harness-to-another-folder):

- The owner's `folders` setting lists the folders a chat may work in, by default the harness's install folder.
  Listing `~` allows any folder inside the home of the account the agent runs as. Listing `/` lifts the limit. This
  replaces D-101's "listing `~` lifts the limit".
- The agent's own account home, and any folder inside it, is never a chat's folder, whatever is listed.
- Setting the folder always starts a new session, even when it names the folder the chat already has.
- For Pi, which keeps one process per chat, a move stops that chat's Pi and starts a new one in the new folder.

**Why.** The owner accepted these from the built proposal. Keeping `~` to mean the home's folders, not the whole
machine, makes the setting say what it reads as. The agent's home holds its keys
([D-099](#d-099-files-from-the-agents-own-home-are-refused-by-default-and-the-owner-may-allow-them)).

Decided 2026-11-22. The project owner decided.

## D-105 Which gateway settings the installers change

**Decision.** When the installers add our plugin to a person's own OpenClaw or Hermes
([D-102](#d-102-the-installers-connect-to-the-harness-the-person-already-has)):

- If OpenClaw forwards no approvals (`approvals.exec.enabled` is missing or `false`), the installer turns forwarding
  on for Marmot chats only. When it is already `true` or `"auto"`, it is left alone. `--uninstall` puts back what was
  there.
- The installers change no other gateway setting: not OpenClaw's verbosity, reasoning, streaming or exec mode, and
  not Hermes's approval mode or who may answer approvals.
- The installers print the command that restarts the gateway rather than restart it.

**Why.** Without forwarding, an approval in a Marmot chat waits unseen. The other settings apply to the person's
whole agent, so changing them would change their other channels, and a restart would interrupt those channels.
MDK's installers print the restart too.

Decided 2026-11-22. The project owner decided.

## D-106 The core keeps reading again a group MDK cannot read for a moment

**Decision.** When MDK fails for a moment to say a message's group, its place in storage, what a catch-up reads, or
which groups there are, the core does not move its read mark. It holds that group and reads it again from its mark,
after 100 ms and then doubling up to 10 s, with no limit on how often. Other groups carry on.

**Why.** A message is never skipped. A group that stays unreadable costs one read every 10 s, and the log line it
writes names no group ([D-071](#d-071-what-may-be-logged)).

Decided 2026-11-22. The project owner decided.

## D-107 Every harness that lets an agent send a file gets file sending, before the swap

**Decision.** File sending ([D-090](#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder))
is a hard requirement for every harness whose interface lets an agent send a file, or lets a client give the agent
a tool that can. This applies to the coding harnesses (Codex, Claude Code, Pi, OpenCode, Goose, Cursor) and the
gateways (Hermes, OpenClaw). A harness that offers no way is named, with the evidence, and is the only exception.
The swap waits for it.

**Why.** Sending a file to the chat is part of what an agent is for. Today's connectors for Hermes and OpenClaw
already send media, and the new system must not do less than they do on any harness that can.

Decided 2026-11-22. The project owner decided.

## D-108 How an agent's file reaches the core, as built

**Decision.** For [D-090](#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder) and
[D-107](#d-107-every-harness-that-lets-an-agent-send-a-file-gets-file-sending-before-the-swap):

- **The tool.** Every harness the core gives a tool to gets the same one: `send_file`, taking one argument, `path`,
  and answering "Sent." or "Not sent: …". Codex gets it as a dynamic tool and Claude Code from an in-process MCP
  server. Pi gets it from a small extension the driver ships and loads with `--extension`, since Pi's RPC mode
  has no way for a client to add a tool.
- **A gateway's own way.** The Hermes and OpenClaw plugins turn the files their gateway hands the platform into
  the core's `send_file`, one request for each file. A caption goes as the agent's words, before the files.
- **OpenClaw's original path.** OpenClaw copies a reply's file into its own outbound media store and hands the
  channel only the copy. The plugin sends the path the agent named, which OpenClaw keeps in a record its plugin
  SDK does not export. This is a stretch of [D-006](#d-006-prefer-official-interfaces-and-libraries), accepted
  until OpenClaw exposes that path, which the project asks it to do.
- **A gateway's folder.** The installer lists the gateway's work folder in `files_from`: Hermes's `terminal.cwd`,
  or the workspace of the OpenClaw agent that takes the Marmot chats. It lists none when that folder is the
  whole home, holds or lies in the agents' data, or holds or lies in a gateway's home or media cache, or when
  Hermes runs its commands in a sandbox, or OpenClaw's Marmot chats may go to more than one agent. A folder that
  lies inside OpenClaw's state folder, as its default workspace does, is allowed. A `files_from` already in the
  settings is never changed by a re-run. (Replaced in part by
  [D-119](#d-119-how-a-gateway-install-moves-over-its-work-folder-a-half-done-openclaw-move-a-fresh-conversation):
  `--replace-plugin` adds the gateway's folder to the line the import wrote.)
- **Media caches stay out.** Hermes's and OpenClaw's media caches are never allowed, since they also hold files
  people sent the gateway in any of its chats, on any platform.

**Why.** The project owner accepted the recommendations. One tool with one wording reads the same on every
harness. Each way is the harness's own, or the only one it offers. Leaving the caches out keeps a file sent in
one chat or platform from being posted into another.

Decided 2026-11-22. The project owner decided.

## D-109 How the core's sends survive a crash, as built

**Decision.** On MDK 0.19.0 the core hands every send in its outbox to MDK under a token saved with it (MDK's
token-based local sends):

- **Retries.** A send whose outcome is not known is tried again under the same token, with no limit, since that
  cannot send it twice. A try MDK took and then dropped gets a new token, and the send is given up after ten such
  drops, with what stands in for it
  ([D-097](#d-097-what-stands-in-for-a-send-that-is-dropped)).
- **The state file** keeps, besides what it kept before: a random salt the tokens are made from, how many tries of
  each send MDK dropped, the token of each first message, and when each send was first tried under its token.
  None of these identifies anyone.
- **A send MDK may have forgotten.** Once a group's disappearing-messages period has passed since a send's first
  try, and MDK no longer knows its token, the send is not tried again. It is given up with a `failed` notice saying
  "Something the agent sent may not have been delivered to the chat."
- **An accepted limit,** as in [D-096](#d-096-four-limits-of-the-core-that-the-simulator-and-the-model-accept): the
  core reads the group's period as it is now, so a period made longer after a send went out, while its outcome was
  not known, can let that send go out twice.
- **Timings.** On a stop, the core waits up to 60 seconds in all for sends MDK still holds. It asks again after a
  held send at most every 500 ms.
- **A state file from before tokens,** whose outbox holds sends already tried, stops the core at start.
- **Files** keep no token until MDK's token form for an upload carries a message's tags, which the project asks MDK
  for.

**Why.** The project owner accepted the recommendations. A token makes trying again safe, so the duplicate first
event that D-096 used to accept no longer happens. The notice for a forgotten send says what is known, not more.

Decided 2026-11-22. The project owner decided.

## D-110 An ACP agent gets the file tool from a loopback MCP server, on by default

**Decision.** This settles what was Q-31. OpenCode, Goose and Cursor take MCP servers from their client only by an
HTTP address, so the ACP driver serves its own MCP server, with one tool, `send_file`, on `127.0.0.1`, and hands
it to every session it opens or takes up.

- **On by default.** `send_files = false` in `[acp]` switches it off.
- **Tokens.** Each chat gets a random 128-bit token, new each time the core starts, sent in a header and never in
  the address or an event. A token reaches only its own chat. For OpenCode, which keeps one server for every
  session in a folder, it reaches the chats working in that folder.
- **Refusals.** A call with no session or a malformed one is refused from agents that always name their session
  (OpenCode, Goose), and so is a call from a session no chat has, such as an OpenCode subagent, until
  Q-30 is settled (since settled as
  [D-121](#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them) to D-126).
- **Approvals.** An agent's own approval of the tool, as Cursor asks for every MCP tool, goes to the chat like any
  approval. The driver answers none itself.
- **Goose's test container** cannot reach the host's loopback address, so Goose there has no tool. Goose on the
  host, the default, has it.

**Why.** The project owner accepted the recommendations. It is the only way these agents can have the tool, which
[D-107](#d-107-every-harness-that-lets-an-agent-send-a-file-gets-file-sending-before-the-swap) requires. A
listener on the loopback address can be reached by any program on the machine, but a call does nothing without a
chat's token, and even then only what the core allows that chat. A driver that answered an approval would decide
who may approve, which only the core does
([D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)).

Decided 2026-11-22. The project owner decided.

## D-111 A group the core has never read starts after what arrived before the core did

**Decision.** For a group the core has no read mark for, as on an imported account's first run or after a lost state
file, the core goes by when each stored message reached this device (MDK's `received_at`), compared with the second
the core subscribed to MDK. A message that arrived at or before that second is history: it is marked read and is
never a prompt. One that arrived later is new and is read once, in storage order. A message that arrived in that
very second counts as history.

**Why.** MDK announces a sync's messages only after storing them all, so a core that has just subscribed can hear
messages that were stored before it started, and it must not act on old messages as prompts. MDK gives arrival times
in whole seconds, so a message in the start second cannot be told from one just before it. Missing one new message
in a rare case (no mark, at start) is better than acting on history. The project owner accepted the
recommendation.

Decided 2026-11-22. The project owner decided.

## D-112 The core sends kinds 1201, 1202 and edits, and a turn's message is `started_by`

**Decision.** MDK takes the new shape of kinds 1201 and 1202 (MDK pull request 3613), so the core uses it, in
place of the temporary kinds of
[D-069](#d-069-until-mdk-takes-the-new-shape-agent-items-travel-under-a-temporary-kind):

- **An item's first version** is a kind 1201 event, and **each later version** is MDK's edit of it, kind 1009,
  carrying the whole item with a higher `v` ([D-025](#d-025-an-agent-item-is-updated-with-an-edit)). Both go
  through MDK's token-based sends ([D-109](#d-109-how-the-cores-sends-survive-a-crash-as-built)); an edit names
  the token the first event went under, and MDK places its `e` tag from it.
- **One tag is renamed, by MDK:** on kind 1201 and its edits, the chat message that started the turn is
  `["started_by", <id>]`, not `e`. An edit of an item carries every tag of the item and exactly one `e`, its
  target, which a turn's `e` would have repeated. Kind 1202 keeps its `e`, the first event of the item it acts on.
  What each tag carries is as [D-076](#d-076-the-tags-of-kinds-1201-and-1202) says.
- **Answers and controls** are kind 1202 events.

**Why.** D-069 said the switch is made in one place when MDK is ready, and MDK is. D-076 lets MDK rename a tag to
fit its conventions: an edit's `e` is MDK's own, and a second one on the same event would be ambiguous.

Decided 2026-11-22. MDK made the rename, which D-076 allows.

## D-113 Hermes and OpenClaw are channel plugins, with a narrower contract than a driver

**Decision.** The Hermes and OpenClaw connectors are a kind of connector of their own: **channel plugins**. A
channel plugin runs inside a messaging gateway as one of its channels and connects to the core over the socket,
as the Claude Code driver does. Its contract is narrower than a driver's. It promises what the core needs from
every connector, and leaves to the gateway what the gateway owns: the conversation's sessions, its media, its own
allow-lists, and its own setting for how much tool activity it sends. The contract is written in
[channel plugins](design/drivers.md#channel-plugins).

This extends the Hermes and OpenClaw rows of [D-005](#d-005-which-driver-each-harness-gets), which stand: each
is its own plugin, over the socket. The layers of [D-004](#d-004-four-layers-with-drivers-behind-one-trait)
stand, and so does [D-084](#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through): the
core decides who may prompt and who may approve.

**Why.** Hermes and OpenClaw are messaging gateways in their own right, and the Marmot plugin is one channel
among many inside them. Holding them to the shape of a driver led to coupling to their internals, such as
OpenClaw's record of the path an agent named for a reply's file (`replyMediaSourceUrls`), which the plugin reads
to send that file ([D-108](#d-108-how-an-agents-file-reaches-the-core-as-built)) and which OpenClaw's plugin SDK
does not export.

Decided 2026-11-23. The project owner decided.

## D-114 A current-state page sits beside this record, and every new decision updates it

**Decision.** [`current-state.md`](current-state.md) says, in one short page grouped by area, what is true of the
system now, each line linking to the decisions behind it, and leaving out what a later decision replaced. It
says which `master` commit it was last brought up to date at. This file stays the record: it says why, and where
the two disagree, it wins. A change that adds a decision, or replaces one, updates `current-state.md` in the same
change.

**Why.** There are more than a hundred entries, many of them replaced in part, and a new agent or person should
not have to read them all to learn what holds today. A page kept in step with each decision stays true without a
separate effort to catch up.

Decided 2026-11-23. The project owner decided.

## D-115 The socket's schema is made from the core's own types, and checks this repository's connectors

**Decision.** Every socket message is written and read through types in the core
(`crates/marmot-agent/src/socket/wire.rs`). A JSON Schema and a set of example messages are generated from those
types and committed (`fixtures/socket`), and the Claude Code driver, the OpenClaw plugin and the Hermes plugin each
check against them that what they send is what the core reads, and that they read only what the core sends. The
schema admits no field the types do not name: it is strict for checking this repository's connectors. The core
itself stays as lenient as the socket was before ([D-081](#d-081-the-socket-messages-as-built-are-accepted)): it
passes over fields it does not know and reads each field it needs on its own. The OpenClaw plugin stops sending
the 13 body fields the core passes over (listed in `fixtures/socket/passed-over.json`), rather than the event
model gaining them; until it does, the checks allow exactly those. (It stopped on 2026-11-23: the list is removed, and the checks allow no field the core does not read.)

**Why.** Each capability was added by hand in four places, and each client was tested only against its own fake
of the core, so the four could drift apart without a test failing. Making the description from the core's types
means it cannot fall behind them. Keeping the core lenient changes nothing for a connector it took before; the
strictness belongs in the check, not in what the core refuses. The 13 fields reach no chat today, and the model
they mostly carry is already in the session item.

Decided 2026-11-23. The project owner decided.

## D-116 A gateway's plugin swap is put back on failure, and finished by a re-run after a kill

**Decision.** When an install script changes a gateway's plugin (`--replace-plugin`, a fresh install or an
update of Hermes or OpenClaw), it first writes down the copies it took of what it changes. If the run fails, or is
stopped with TERM, INT or HUP, it puts those copies back. After a SIGKILL, which no script can catch, running the
installer again always either finishes the change or puts it back; the record stays until the core's service is
running. At a few named moments a SIGKILL can leave the gateway with neither plugin working until the installer
runs again: they are listed in [the move](move.md), and the install tests stop each swap at every step to check
that this is all. Two of them are OpenClaw's own: its `plugins install` refuses settings for a plugin it does not
have, so no order of its commands avoids them. The record is given up by `--uninstall`.

**Why.** A swap that fails or is stopped must not leave a person's gateway without a working plugin, while the
import has already stopped the old services. Doing the swap any other way for OpenClaw, such as loading our
plugin from `plugins.load.paths`, would go around OpenClaw's own install command, against
[D-006](#d-006-prefer-official-interfaces-and-libraries). Putting settings back on a failed fresh install or update
is the same protection, and costs nothing more.

Decided 2026-11-23. The project owner decided.

## D-117 Codex's subagents are switched on, and shown flat in the chat that started them

**Decision.** The Codex driver no longer switches Codex's subagents off. Until a subagent has a thread of its own
(Q-30, since settled as [D-121](#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them)), its work is shown flat in the chat of
the turn that started it, as Claude Code's subagents already are. Nothing is added to the event vocabulary:

- The item that starts a subagent, or gives it more work, is a `tool` item of kind `subagent`. Each turn of the
  subagent's is a `task` item of kind `subagent`, whose `subject` is that tool item. Its other items (commands,
  edits, approvals, questions, plans) are shown as the agent's own are, and its words are messages whose
  `parent` is the tool item. All of them are in the chat's session, under the turn the person started.
- An approval or a question from a subagent waits in the chat, and is answered there as the agent's own are.
- Stop in the chat stops the agent's turn and every subagent under it, including one still working after the
  agent's turn has ended, and is settled once, when the last of them has ended
  ([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it),
  [D-029](#d-029-three-shared-controls-and-the-rest-comes-from-the-harness)). A subagent whose turn starts while
  the Stop waits is stopped too. If Codex refuses to stop one that then runs on, the Stop is settled as failed, by
  that refusal. A Stop that names no item, once the agent's turn has ended, stops what is still working beside it:
  the turn its newest open `task` names.
- When the chat's session is replaced, its subagents still working are stopped, since Codex does not stop them
  with it. Every open subagent item closes as cancelled when Codex stops, when the chat's session ends, and when
  the core restarts ([D-070](#d-070-a-restart-closes-the-turn-as-interrupted)).

How the transport does it is internal, and is point 8 of [subagents as threads](design/subagents.md):
each subagent's thread has a translator of its own, in the chat of the thread that started it; a message for a
thread not known yet is held until the item that names that thread arrives, since a subagent can speak first;
and the transport interrupts the subagents itself, since Codex does not, following `rootTurnId` and the links
Codex gives between threads (`agentThreadId`, `receiverThreadIds`, and `parentThreadId` from Codex 0.171.0).
This stands beside [D-060](#d-060-a-codex-translator-follows-one-thread-and-the-transport-routes): a translator
still follows one thread. It answers Q-29, since settled as [D-125](#d-125-the-codex-transport-follows-subagents-threads),
for a subagent's thread.

Replaced in part by [D-131](#d-131-the-codex-driver-refuses-a-request-of-codexs-that-no-chat-takes): a request
of Codex's for a thread not known yet is held only while a session is being picked or a subagent started, and
refused after. Replaced in part by [D-142](#d-142-how-subagent-threads-are-built): Codex's subagents are shown in
threads, each as one `task` item. Replaced in part by
[D-132](#d-132-a-stop-waiting-for-a-subagent-is-settled-by-codex-after-a-new-session): a subagent a Stop waits
for, when the chat's session ends, closes when Codex says what came of the Stop.

**Why.** Codex's subagents were switched off only because nothing showed them, and an approval from one would
have had nowhere to go. Shown flat, they need nothing new from the apps, the event model or MDK, and the
transport this needs is the one threads will build on. Codex does not stop a subagent when its parent is
interrupted (recorded), so Stop would otherwise leave work running that the chat can no longer stop.

Decided 2026-11-23. The project owner decided.

## D-118 The channel-plugin contract is accepted as written

**Decision.** The contract in [channel plugins](design/drivers.md#channel-plugins), drawn on 2026-11-23 from
what the Hermes and OpenClaw plugins do at that day's `master`, is accepted as written. It holds every
channel plugin to what the core needs from every connector on the socket, and leaves to the gateway what
[D-113](#d-113-hermes-and-openclaw-are-channel-plugins-with-a-narrower-contract-than-a-driver) says it owns.
In particular:

- **Sessions are always the gateway's own.** A channel plugin states `sessions` as `own`; the core never names
  a session in a prompt to it, and `new_session` asks the gateway for one in its own way.
- **No folder and no tool from the core.** A channel plugin states no `folder` in its hello, so the core offers
  no `folder` setting for its chats, and the gateway keeps the workspace it manages
  ([D-101](#d-101-a-chat-can-move-a-coding-harness-to-another-folder)). The core gives its agent no `send_file`
  tool: the agent sends a file in the gateway's own way, and the plugin turns that into the core's `send_file`
  ([D-108](#d-108-how-an-agents-file-reaches-the-core-as-built)).
- **Passing everything through means what the gateway hands its channel.** For a channel plugin,
  [D-008](#d-008-pass-everything-through) is met by passing through everything the gateway hands the channel,
  at the gateway's own setting for how much tool activity it sends, which the installers leave alone
  ([D-105](#d-105-which-gateway-settings-the-installers-change)). Hermes hands formatted lines, so its tool
  activity shows as commentary messages and never as `tool` items.
- **Controls go only as the gateway's typed commands**, from the person who sent them. A setting the gateway has
  no command for is refused. What the gateway does not report as data, the plugin fills in from its own
  knowledge of the gateway ([D-044](#d-044-a-connector-supplies-what-its-harness-does-not-say)).
- **The gateway's own check is opened, so the core's counts**
  ([D-084](#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through)). For Hermes,
  `install/hermes.sh` sets `MARMOT_ALLOW_ALL_USERS=true` in Hermes's settings and puts the earlier value back on
  `--uninstall`, and the plugin answers an approval card or a question as the person whose answer the core
  sent. For OpenClaw, the installer sets no allow-list; the plugin hands OpenClaw an open policy with every
  message it delivers (`dmPolicy` and `groupPolicy` `open`, `allowFrom` `*`), and answers OpenClaw's question
  whether a sender may approve (`authorizeActorAction`) with yes only for a sender the core has vouched for. A
  plugin's list of typed commands that need the right to approve is its own, complete for the gateway version
  it was read in.
- **A gateway's internals.** Where a plugin uses something the gateway does not publish, the gateway's
  reference page lists it with the version it was read in, and a test fails when it changes. That test may be
  the end-to-end scenario against the real gateway, which runs on demand and must pass before a milestone
  closes. Two Hermes internals have no such test yet, `COMMAND_REGISTRY` with `_is_gateway_available` and
  `resolve_gateway_clarify` ([what the plugin relies on](reference/hermes-messages.md)); they are a known gap,
  closed by having the Hermes scenario check that its session item lists commands and ask one question.

Not decided: the socket's chat section, which the gateways' own message tools need
([Q-32](open-questions.md#q-32-what-does-the-sockets-chat-section-carry)). Until it is designed, OpenClaw's
message tool is offered no actions.

**Why.** The contract was written from what both plugins do, and both pass their tests and their end-to-end
scenarios on it, so accepting it as written settles what is built. Each point follows from the gateway owning
the conversation: its sessions, its workspace and its way of sending a file are already there, and a second
of each from the core would compete with the gateway's. Hermes gives a platform only formatted lines, so `tool`
items would have to be guessed back out of text. The gateways' typed commands are the one way in that they
publish for controls. A test of the plugin's own package runs against a fake gateway, so only a run inside the
real one can see an internal change.

Decided 2026-11-23. The project owner decided.

## D-119 How a gateway install moves over: its work folder, a half-done OpenClaw move, a fresh conversation

**Decision.** Three points about moving a Hermes or OpenClaw install of today's connectors over
([D-089](#d-089-an-install-moves-over-by-importing-its-account-home-in-place),
[move](move.md)):

- **`--replace-plugin` lists the gateway's work folder in `files_from`**, as a fresh install does
  ([a gateway's files](design/drivers.md#a-gateways-files)): the folder the installer finds for that gateway,
  under the same rules, and none in the same cases. It is added beside the old connector's folders the import
  wrote, not in their place, and only to the line the import wrote, marked by the import's comment and then by the installer's, which
  names the folder it added; a re-run changes nothing while the gateway works there, and puts the new folder
  in its place if the gateway has moved; a `files_from` the person wrote is left alone, and the installer says what to add. A folder
  that holds or lies inside the moved account's home, or the import's backup of it, is never added. (These
  details were decided 2026-11-23 by the builder, under the project owner's instruction to decide minor
  points.)
- **The installer edits `openclaw.json` itself to finish a half-done move.** A move stopped after our plugin went
  into OpenClaw and before the old plugin's settings came out of its `marmot` channel leaves an OpenClaw that
  refuses to read or change any of its settings, or to start, so its own commands cannot finish the move. When
  a run of `install/openclaw.sh` finds settings in `channels.marmot` that our plugin does not take, and OpenClaw
  refuses `config get channels.marmot` naming that channel, it keeps a copy of OpenClaw's settings file (the one
  `openclaw config file` names) in the agent's `backups` folder, and takes those settings out of the file itself,
  leaving `enabled` and `home`, which our plugin takes. It does so only for a file that is plain JSON, and
  writes the new file beside it, then moves it into place. Everywhere else the installers change a gateway's
  settings only through the gateway's own commands.
- **The gateway's conversation starts fresh after a move.** The new plugin names a chat by the core's own id
  for it ([D-036](#d-036-the-ids-a-connector-sees-are-the-cores-own)), not by the group, so what Hermes or
  OpenClaw kept of a chat's conversation stays with the old name, and nothing carries it over. The chat itself,
  and everything said in it, stays where it was.

At `master` commit `e923d4e` the first point is not built yet: the import always writes a `files_from` line, the
old connector's media folders or none, and the installer leaves a `files_from` line it finds as it is. The other
two are built.

**Why.** A moved agent should be able to send the files a freshly installed one can, or the move takes something
away; the folder is found by the same rules, which keep out the home and the gateway's media cache. Finishing a
half-done move is the only way out of a state OpenClaw cannot leave on its own commands, and the copy kept first
makes it undoable; this is the exception to building on the gateway's official interface
([D-006](#d-006-prefer-official-interfaces-and-libraries)). Carrying the old conversation over would mean giving
the plugin the group's id, or a map from it, which D-036 keeps from every connector.

Decided 2026-11-23. The project owner decided.

## D-120 The choices made in moving to kinds 1201 and 1202, as built

**Decision.** Five points built with [D-112](#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)
on 2026-11-22 are accepted, as [kinds 1201 and 1202](design/agent-kinds.md#accepted-with-the-move-to-mdk) and
`crates/marmot-agent/src/conversation.rs` and `carrier.rs` have them:

- **A notice the core makes carries `seq` from 1.** A `refused` or `failed` notice of the core's own carries as
  its `seq` the highest its chat's items have shown, or 1 when they have shown none, since MDK reads a `seq`
  only from 1. It sorts after what came before it.
- **An item begun under the temporary kinds carries on as a new kind 1201 event.** A state file from a core that
  sent items under the temporary kind of
  [D-069](#d-069-until-mdk-takes-the-new-shape-agent-items-travel-under-a-temporary-kind) is taken up once: MDK
  applies no edit to such an event, so each item's next version goes out as a new kind 1201 event, the item's
  first from then on, with its version going on rising. The agent's words, chat messages then too, are edited as
  before. What the state file kept of forgotten items is dropped. A state file that holds an item or a notice
  already handed to MDK in the old shape is refused, and the core does not start, since MDK keeps that try's
  token bound to the old event and would refuse it in the new shape, though it may have gone out.
- **A first event that disappeared is sent again; one that was deleted is not.** In a chat whose messages
  disappear, MDK forgets an item's first event and its token, and refuses an edit of it. The core then sends
  that version as a new kind 1201 event, the item's first from then on, under the same item id and with its
  version going on rising; the chat is told of no failure. The agent's words whose message disappeared go out as
  a new message. A first event that someone deleted, or that never reached the group, is not sent again: that
  version is dropped with a `failed` notice.
- **The agent's words settle at most what MDK takes in a chat message.** MDK takes a chat message with at most
  64 tags, of at most 16 KB in all, and does not export those numbers; the core keeps them itself, and a test
  against MDK checks them. So the agent's words carry as many `settles` as fit beside their `turn` and, for a
  remark, their `phase`: 62 for a remark with a short turn id, 63 for words without `phase`, fewer only when the
  16 KB is reached first. Their turn's next versions settle the rest. An item is held only to its shape and
  64 KB.
- **A refused notice about a message names it with `started_by`.** The core's `refused` notice about a chat
  message it did not act on, a prompt from someone without the right or one the driver refused, names that
  message with the tag a turn names its message with, `started_by`.

The other additions on that page, built on 2026-11-21 and 2026-11-22, stay proposals.

**Why.** Each follows from what MDK does now: it reads `seq` from 1, it cannot edit an event in the old shape or
one it has forgotten, and it caps a chat message's tags. A disappearing chat is working as meant, so a gone first
event is no failure to report, while a deleted one is someone's choice, which the core does not undo. A notice
about a message uses the name MDK gave that tag.

Decided 2026-11-23. The project owner decided.

## D-121 A subagent is a thread in the agent's own group, with NIP-22's tags as NIP-22 defines them

**Decision.** This settles what was Q-30, with
[D-122](#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar) to
[D-126](#d-126-no-mls-group-for-subagents). Where a harness tells its subagents apart
([D-124](#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners)),
each subagent is a thread in the agent's own group, as a thread is in Slack or Discord. The design is
[subagents as threads](design/subagents.md).

- **A subagent's row is its `task` item**, kind 1201, sent in the turn that started the subagent and edited as
  it works ([D-025](#d-025-an-agent-item-is-updated-with-an-edit)). What the agent does that names the subagent
  updates the row; what does not stays the agent's own activity. A subagent's session names its `task` item.
- **Everything the subagent does is in its thread**: its items, as kind 1201 events as in the main chat, its
  words ([D-122](#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar)), and what people write there. A
  subagent that starts a subagent has a `task` item in its thread, and that item opens the nested subagent's
  view.
- **The thread tags are NIP-22's, as NIP-22 defines them**, not a Marmot tag of our own. On every event in a
  top-level subagent's tree, the uppercase `E`, `K` and `P` name the **top-level** subagent's `task` item: its
  first event, kind `1201`, and the agent. The lowercase `e`, `k` and `p` name the **immediate parent**: the
  `task` item of the subagent the event belongs to. For a top-level subagent's own events, root and parent are
  the same item, as for a first comment. A nested subagent's `task` item names, as its
  parent, the `task` item of the subagent that started it. A top-level `task` item carries no thread tags: it is in the main chat.
- **Nesting is rebuilt from the lowercase parent.** An app opens a nested subagent's view by filtering on its
  `task` item as the parent. A reply to a message in a view names that message as its parent, and belongs to
  the view of the first `task` item up its chain of parents (inferred from NIP-22; not checked in an app).
- **MDK places kind 1201 events in threads** as it does a poll's, and takes a thread message's root from its
  parent's thread, as it already does (`crates/marmot-app/src/client/thread_placement.rs` at MDK `6f6b427`).

For a nested subagent, started by a top-level one, each event in its view carries:

```
["E", <top-level task item>, "", <agent>], ["K", "1201"], ["P", <agent>],
["e", <its own task item>, "", <agent>], ["k", "1201"], ["p", <agent>]
```

Its own `task` item, in the top-level subagent's thread, carries the same `E`, `K` and `P`, and the top-level
`task` item as `e`, `k` and `p`.

This extends the tags of kind 1201 ([D-076](#d-076-the-tags-of-kinds-1201-and-1202)) for an item in a
subagent's thread.

**Left open, for MDK to settle.** An edit, kind 1009, already uses `e` for its target
([D-112](#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)), so it cannot also
carry NIP-22's parent `e`. Recommended: an edit carries no thread tags and takes its thread from its target, since
an item never moves. That would be an exception to D-112's "an edit of an item carries every tag of the item",
for the thread tags alone. (Settled by [D-133](#d-133-an-edit-of-an-item-in-a-subagents-thread-repeats-the-threads-root-tags): an edit repeats the root tags.)

**What MDK has to do**, as read at `6f6b427`:

- Place a kind 1201 event in a thread. Today only a comment, a chat message and a poll can be in one
  (`own_thread`, `crates/storage-sqlite/src/app/timeline/threads.rs`).
- Take the parent `e` on an item's first event. Today MDK refuses any `e` on a kind 1201 event a program sends
  (`validate_agent_item_tags`, `crates/traits/src/agent_events.rs`), because an edit's target is MDK's to place.
- Settle what a parent `p` means here. MDK leaves NIP-22's parent `p` off the comments it sends, because on
  Marmot a `p` is a mention. On these events the parent's author is always the agent that sends them, so the
  `p` names the sender. Recommended: MDK takes a `p` that names the event's own author as no mention.

**Why.** Marmot already has threads, as NIP-22 comments, so a subagent needs no new kind and no new group, and an
app that shows threads already knows the shape. Using NIP-22's tags as NIP-22 defines them means MDK's own
placement and any NIP-22 reader see the same tree. The proposal had each thread flat, with `E` naming the
nearest `task` item; that would have given the standard tag a meaning of its own, and MDK's placement passes a
parent's root on, which this matches.

None of it is built in the core, the drivers, MDK or the apps yet. Until the Codex part is built
([D-125](#d-125-the-codex-transport-follows-subagents-threads)), Codex's subagents are shown flat, which another
pull request records as D-117.

Decided 2026-11-23. The project owner decided.

Noted 2026-11-27: MDK's history was rewritten on 2026-11-24, and `6f6b427`, cited above, is no longer on it: it was the merge of MDK pull request 3613. Its content is in `bea7b400`, the revision the
core uses.

## D-122 A subagent's words, what alerts, and the app's running bar

**Decision.**

- **A subagent's words are kind 1111 comments**, with the thread tags of
  [D-121](#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them). This
  is an exception to [D-022](#d-022-the-agents-words-are-ordinary-chat-messages) for subagents; the agent's own
  words stay kind 9.
- **The agent's comments in its own threads do not alert.** MDK keeps silent a comment whose root is a kind 1201
  item by the comment's own author. A subagent's approvals and questions alert as any do
  ([D-024](#d-024-who-an-agent-event-alerts)). This is a rule beside D-024 and
  [D-078](#d-078-the-agents-remarks-during-a-turn-do-not-alert), and MDK has to follow it.
- **The app shows a running bar above the composer** while any subagent runs, drawn from the `task` items still
  `running` or `waiting`, at any depth. Its list shows each one's path ("Update the tests › Find the fixtures"),
  and groups by agent when several agents in the group have subagents. Tapping one opens its thread.
- **The app lifts every waiting approval and question into the bar**, with the subagent's name and the choices,
  so it can be answered there without opening the thread. The answer is the same kind 1202 event as anywhere,
  and the item's edit shows who answered.
- **The app folds a finished thread into its row.** The row keeps its replies count and a one-line result where
  the harness gives one, such as Claude Code's `summary`, never the whole report. The thread stays readable for
  as long as the chat keeps history; it has no shorter life of its own.

Nothing new is sent for the bar or the folding. These are rules for apps, beside D-024.

**Why.** A thread view draws comments as words, people reply to them the same way, and an app without threads
still shows them, inline. The subagent's result reaches the parent, whose answer alerts, so the subagent's words
need not. Every tool that shows subagents keeps a list of the running ones near the composer and shows a
subagent's approval where the person already is; without that, threads are hard to use while they run. A shorter
life for thread events would be a new kind of expiry in Marmot, and would take away what an owner may later need
to check.

MDK does not keep any comment silent today: a thread message alerts like chat.

Decided 2026-11-23. The project owner decided.

## D-123 Stop, and writing in a subagent's thread

**Decision.**

- **Stop in the main chat stops every subagent the agent started**, with the agent's turn, including subagents
  still running after its turn has ended. It names the agent's last turn.
- **Stop in a thread stops that subagent and the ones it started.** It names the subagent's `task` item.
- **Either is taken while any of them works, and settled once**, when the last of them has ended, by the edit of
  whichever ended last. This extends
  [D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it) and the `interrupt`
  control of [D-029](#d-029-three-shared-controls-and-the-rest-comes-from-the-harness). If the parent is still
  working when a subagent is stopped, the harness tells it, and it may start another: that is the parent's
  choice, shown as a new row.
- **A comment in a thread is a prompt for that subagent only where its driver says a person can write to a
  running subagent, and the writer has the right to prompt**
  ([D-032](#d-032-every-action-needs-one-of-two-rights)). The subagent is the first `task` item up the
  comment's chain of parents. The `task` item says whether it takes messages, so the app knows before anything is
  sent; where it does not, the composer says so ("Codex's subagents don't take messages; write in the main
  chat"), and people can still talk to each other there. Such a comment is for people, and the agent never sees
  it. **It is never passed to the parent.** The agent's own comments are never prompts, and each agent's core
  reads only comments in threads whose root it sent. Where a comment does go through, the subagent's next turn
  shows it, as any prompt's turn does.
- **A driver states one more capability flag**: whether a person can write to a running subagent. This extends
  [D-058](#d-058-the-driver-trait-and-the-capability-flags), and adds a field to the `task` item
  ([D-076](#d-076-the-tags-of-kinds-1201-and-1202)); both are named when built. Neither the Claude Code driver
  nor the Codex driver says yes until it has been recorded, since neither was tested.

**Why.** Stop in the main chat is what a person uses when they want it all to end. Subagents can outlive the turn
in Codex, Claude Code, Hermes and OpenClaw, and interrupting a Codex turn leaves its subagents running, so
stopping only what is under the turn would leave work going that the person meant to stop. T3 Code, OpenClaw's
`/stop` and Hermes's `/stop` stop the whole tree. Few harnesses let a person write to a subagent, and a reply
rerouted to the parent would be answered in the main chat, to a message that was not sent there.

Decided 2026-11-23. The project owner decided.

## D-124 What keeps subagents reliable: closes, harnesses that do not tell them apart, and late joiners

**Decision.**

- **A driver closes what it can no longer follow.** When its harness stops, its session ends or the core
  restarts, every open `task` item is edited to `cancelled`, as an open turn already is. This extends
  [D-070](#d-070-a-restart-closes-the-turn-as-interrupted).
- **The app keeps counting a `running` row that has had no edit for 30 minutes**, and shows it as not heard from
  since then ("no news since 14:02"). Nothing is assumed: the agent's machine may simply be off. 30 minutes is a
  first value with no evidence behind it.
- **A driver says whether its harness tells subagents apart.** Where it does not, a subagent stays a `tool` item
  of kind `subagent` in the agent's own activity, with no thread, and anything it asks shows as the agent's own
  approval. This is one more capability flag, extending
  [D-058](#d-058-the-driver-trait-and-the-capability-flags), so nothing above the driver checks the harness's
  name. As read on 2026-11-23, Goose, Hermes, Pi and Cursor over ACP do not tell them apart, and Codex, Claude
  Code and OpenCode do ([subagents, threads and what others do](reference/subagents-prior-art.md#harnesses)).
- **An app shows a held edit of an item sent before the member joined as the item itself**, since an edit
  carries the whole item. This holds for every item, not only a subagent's, and extends
  [D-025](#d-025-an-agent-item-is-updated-with-an-edit). A thread then shows what arrived after the member
  joined, under a row that says it started earlier.

**Why.** A subagent can fail to report its end: Codex reports one only from the parent's thread, and a harness
that is killed reports nothing. A driver that closes what it can no longer follow keeps the bar from staying
full of rows that will never end. A row the driver has not closed may still be working, so the app keeps it and
says when it was last heard from, rather than guessing. Making up a thread from a harness's text would be a
guess too. A member added in the middle of a subagent's work otherwise sees no row for anything running when
they arrived.

Decided 2026-11-23. The project owner decided.

## D-125 The Codex transport follows subagents' threads

**Decision.** Codex starts a thread of its own for each subagent. The Codex driver's transport:

1. gives each subagent's thread a translator of its own, in the chat of the thread that started it;
2. holds a message for a thread it does not know yet until the item that names that thread arrives, since a
   subagent can speak first ([recorded](reference/harness-messages.md#subagents));
3. interrupts subagents itself, since interrupting the parent does not stop them, following `agentThreadId`,
   and `parentThreadId` from Codex 0.171.0 or `receiverThreadIds` on the older runtime.

This extends [D-060](#d-060-a-codex-translator-follows-one-thread-and-the-transport-routes), whose four rules
stand for each chat's own thread, and settles what was Q-29 for the threads Codex starts for subagents. No other
thread is known to arrive unasked; one that does is a new question.

Point 2 is replaced in part by [D-131](#d-131-the-codex-driver-refuses-a-request-of-codexs-that-no-chat-takes):
a request of Codex's is held only while a session is being picked or a subagent started, and refused after.

**Why.** D-060 puts routing between threads in the transport, and a subagent's thread is one more thread to
route. Given to the parent's translator, its messages would be dropped, and an approval from a subagent must
reach someone or the subagent waits.

Until this is built, the Codex driver shows subagents flat, which another pull request records as D-117.

Decided 2026-11-23. The project owner decided.

## D-126 No MLS group for subagents

**Decision.** A subagent's thread is in the agent's own group. No separate MLS group is made for a subagent, by
RFC 9420 branching or otherwise. Nothing changes; this is recorded so the question is not raised again for this
purpose. A branch for output only the owner sees
([Q-11](open-questions.md#q-11-what-keeps-sensitive-output-from-spreading)) is a different question, and is not
settled here.

**Why.** A group per subagent would have the same members, so it hides nothing and saves nothing: an MLS message
is encrypted once for the whole group, and every device downloads it either way. It would cost a KeyPackage and
a Welcome per member per subagent, and scatter one chat across groups. MDK does not do it. The detail is in
[MLS subgroup branching](reference/subagents-prior-art.md#mls-subgroup-branching).

Decided 2026-11-23. The project owner decided.

## D-127 Real harnesses run in CI on the model double, with no model key

**Decision.** The real harness programs are tested in CI against the model double
([`tools/model_double`](../tools/model_double/README.md)), a local server that speaks the providers' wire formats,
so only the model is fake:

- **No model key is in CI at all**, on a pull request or nightly. A scenario that runs in CI uses the double,
  scripted or replaying a committed cassette. This replaces the item of
  [D-094](#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly) that ran the real-harness
  scenarios nightly on numes.ai's free models with its key as a CI secret; the rest of D-094 stands.
- **Codex's sandbox runs on the CI runner**: the job lifts Ubuntu's limit on unprivileged user namespaces, which
  bubblewrap needs.
- **Cassettes are recorded through numes, on a non-vendor model, for now**, on the owner's machine, with the double
  in a container that has only the key file mounted, read-only. The double translates a harness's Anthropic
  Messages or Responses request to Chat Completions, which is all numes serves.
- **Replay reports a harness version newer than a cassette's, and does not fail on it.** A request the cassette
  has no exchange for fails, whatever the version.
- **A cassette older than 14 days is reported on a pull request**, as a warning that does not fail it, **and fails
  the nightly run**, which opens an issue.
- **The gateways (Hermes and OpenClaw) stay on scripted mode** until the matching rule ignores the timestamps and
  session ids they put in a prompt; until then a cassette of theirs would never match.

**Why.** Claude Code and Codex have no free model, and a key in CI can reach code nobody reviewed. The double runs
the real harness on every pull request, with nothing to keep secret. Recording through a non-vendor model proves
the wire formats and the harness's own loop, not the vendor model's choices; that is what these tests need. A
newer harness should show its drift as a failing match, not as a forced re-record on every release; the age limit
keeps a cassette from drifting unnoticed.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-128 A folder move whose session shows late still moves the chat

**Decision.** A new session the core settled with `no_effect` because nothing showed within two minutes
([D-087](#d-087-the-core-settles-a-request-whose-effect-never-shows)) still counts when its session item shows
later: the chat takes that session, and for a folder move ([D-101](#d-101-a-chat-can-move-a-coding-harness-to-another-folder),
[D-104](#d-104-how-a-chats-folder-works-as-built)) the folder it was asked in, so files come from where the
harness works. Every new session records the folder it was asked in, a plain one the chat's folder at the time,
and the chat takes each in the order the driver shows their sessions. The core keeps such late ones only while
the driver runs. The person is still told `no_effect` at two minutes, and the session item shows the new folder
when it comes. The request is settled once, by the `no_effect` notice; the late session settles nothing more.

**Why.** Otherwise the harness works in one folder while the chat's files come from another, which is what the
folder rules exist to prevent. Only a running driver can still show a late session, so nothing is kept across a
restart.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-129 A prompt whose answer a dropped connection lost is told as perhaps not delivered

**Decision.** When a connector's connection goes, or the core gives it up, after the core wrote a prompt to it
and before its answer came, the chat is told: "Your message may not have been passed to the agent: the
connection to it was lost before it answered. It is not sent again." A prompt the connector never got, or
refused, is still told it was not passed to the agent. Either way it is never sent again
([D-037](#d-037-a-prompt-is-delivered-at-most-once)).

Replaced in part by [D-180](#d-180-a-prompt-a-driver-refuses-as-having-ended-waits-for-the-next-driver-whenever-the-core-handed-it-on): a prompt the connector never got because the driver had ended waits for the next
driver, and nobody is told.

**Why.** The connector may have handed the prompt to the harness before the connection went, and the harness
may run it (the `prompt-in-flight` counterexample of `formal/tla/SocketProtocol.md`). The core cannot tell the
two apart, so "not delivered" would be wrong when the agent then answers.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-130 CI runs in three tiers, with one required check

*(Replaced by [D-169](#d-169-team-pull-requests-use-a-fast-static-gate-backed-by-hourly-canary-and-the-bisect-agent).)*

**Decision.** CI runs in three tiers, and adds to
[D-094](#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly):

- **Pull request**, aimed at under 10 minutes: formatting, Clippy, the workspace's tests with the conformance
  suite and the real Claude Code and Codex on the model double
  ([D-127](#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key)), the simulator at its normal count, and the TypeScript, Python, install-script, recording and document
  checks. The TLA+ models run on small bounds, every model's cases, only when the models or the code one of them
  follows changed, which a step of CI's own finds with `git diff`.
- **Merge queue** (GitHub's, on `master`), aimed at under 30 minutes: all of the pull-request tier whatever
  changed, the simulator at 200 runs, and the tests that need Docker and no key or sign-in. A push to `master`
  runs the same, so `master` stays checked. *(Amended by [D-161](#d-161-ci-no-longer-runs-on-pushes-to-master): pushes to `master` no longer trigger CI.)*
- **Nightly**: the simulator on new seeds, TLC on `CoreLifecycle`'s larger bounds, a coverage report, a 64th of the core's mutants
  (by the day of the year), and the model double's cassettes against their age and the harnesses' releases, which
  fails the night as D-127 says. A failed job opens an issue, or adds to the one it opened, with what the job
  found when it found something: the mutants missed, the cassettes due.

One job, `CI passed`, needs every job of the first two tiers and is the only check the `master` ruleset requires:
it fails when a job failed or was cancelled, and it lists the run's jobs and fails unless every job the tier
expects ran and passed, since GitHub can report a matrix job as passed when one of its legs never ran. A job left
out by its tier or the path rule counts as passed. CI uses no third-party action and no secret; the gate's token
may read the run's jobs, and nothing more. Build caches are written by a push to `master` and by a run started by
hand on its own branch; a pull request and the merge queue read `master`'s and write none. *(Amended by [D-161](#d-161-ci-no-longer-runs-on-pushes-to-master) and [D-163](#d-163-hourly-canary-runs-the-full-gate-on-master-and-refreshes-build-caches): hourly `canary` refreshes `master`'s build caches and runs the full gate instead of `merge_group` and `push: master`.)*

**Why.** A pull request gets its answer fast, and the slower checks still run before anything reaches `master`,
on the code as it will be merged. One required check means the ruleset need not change when a job is added.
Detecting paths with a step of our own, and no third-party actions, keeps what CI runs to what the repository
holds, for the supply chain.

Decided 2026-11-23. The project owner agreed the tiers; the settings that switch on the queue and the required
check are the owner's.

## D-131 The Codex driver refuses a request of Codex's that no chat takes

**Decision.** Codex waits for a reply to each of its requests, so the Codex driver answers with an error every
request that nobody in a chat is asked about and that it does not answer itself. It uses the codes Codex's own
server uses, and Codex takes the error as a refusal: an approval denied, a question with no answers.

- A request the driver cannot show, such as a question with two questions of one id or params it cannot read,
  is refused at once with -32602, Invalid params; one of a kind it has no rule for with -32601, Method not found.
- A request for a thread a chat has left, or for no thread, is refused at once with -32602, naming the thread.
- A request for a thread never seen is held while a chat's session is being picked or a subagent is being
  started (an open `spawnAgent` item), since it may be a subagent's whose thread is not named yet. Once neither
  is under way it is refused with -32602, naming the thread. One that Codex says is resolved while it is held
  gets no reply.

This replaces in part [D-117](#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them)
and point 2 of [D-125](#d-125-the-codex-transport-follows-subagents-threads), which hold a message for a thread
not known yet until the item that names it arrives: a request is held only as long as something may still name
its thread. A notification is still held so.

**Why.** Left unanswered, such a request keeps Codex, and the turn it belongs to, waiting until someone
interrupts it. The `DriverTransport` model found both cases (`driver-codex-stray`, `driver-codex-unshown`). An
error is how Codex's own server refuses what it cannot take, and Codex reads it as a refusal. Holding while a
pick or a spawn is under way keeps an early request from a subagent able to reach its chat.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-132 A Stop waiting for a subagent is settled by Codex after a new session

**Decision.** When a chat's session is replaced while a Stop waits for one of its Codex subagents, the driver
does not settle the Stop itself, and does not interrupt that subagent a second time. It follows the subagent
only for what Codex says next of the Stop's interrupt and of the subagent's turn: the turn's end settles the
Stop, and Codex's refusal to stop it settles the Stop as failed. The subagent's open items close then, rather
than when the session is replaced, and when Codex stops. Nothing else the subagent says is shown, and a request
from it is refused ([D-131](#d-131-the-codex-driver-refuses-a-request-of-codexs-that-no-chat-takes)).

This replaces in part [D-117](#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them),
under which every open subagent item closes when the chat's session ends.

**Why.** Settling the Stop when the driver let the subagent go reported a success Codex had not given: if Codex
then refused to stop it, the subagent ran on while the chat said it had stopped. Closing its items once Codex
has spoken keeps what the chat shows true.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-133 An edit of an item in a subagent's thread repeats the thread's root tags

**Decision.** An edit (kind 1009) of an item in a subagent's thread carries the thread's root tags again, `E`, `K`
and `P`, with the same values as the item's first event, beside the `e` MDK places for its target. It carries no
lowercase parent tags, since an edit's `e` is its target
([D-112](#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)). This settles the
point [D-121](#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them) left
open for MDK, in place of the recommendation there that an edit carry no thread tags.

**Why.** Someone added to the group after an item's first event cannot read that event, so an edit that carries
no thread tags cannot be placed in its thread; repeating the root tags lets a late joiner place it
([D-124](#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners)) and
changes nothing for anyone else. MDK, which places items in threads, asked for this.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points, on MDK's
request.

## D-134 An upgrade of the core and its support files is put back on failure, and finished by a re-run after a kill

**Decision.** Every agent on an account starts the one `marmot-agent` in `~/.local/bin` with the one set of
support files in `~/.local/share/marmot-agent/lib`, so an install script upgrades the two together. It prepares
the new core and the new files, with the harness's packages, in a folder of the run's own, and only then puts
them in place, one after the other. Before the first of these changes it makes a record,
`~/.local/share/marmot-agent/upgrade`: a copy of the core it replaces, and a list of every part of the files it
replaces, written whole before the record takes its name; it moves each part there as it replaces it. If the
run fails, or is stopped with TERM, INT or HUP, before its agent is set up and started, it puts all of them back,
and says so. Once the agent is started, the record is renamed away in one step, which ends the upgrade, and then
deleted. After a SIGKILL, running the installer again finishes the upgrade or, should it fail, puts back what
the killed run found; another harness's installer, which cannot finish it, puts it back before it starts. A
first install that fails takes away the core and files it put in place.

Three things go with the core and are put back with it. Pi's and Goose's container image is built under a tag
of its own, `marmot-agents-pi:staging` or `marmot-agents-goose:staging`, and given the tag their scripts run only
once the upgrade is done; a failed run leaves the old tag on the old image and takes the staging tag away. The
Hermes and OpenClaw plugin an earlier install put into the person's gateway is copied before it is replaced, as
the old connector's is ([D-116](#d-116-a-gateways-plugin-swap-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)),
and put back with the core; the copy is deleted once the update is done.

Only one install runs in a home at a time. Each holds a lock on `~/.local/share/marmot-agent/install.lock`
from before it looks for an upgrade left under way until it ends, so no install takes over, finishes or puts
back an upgrade another is still making; one that finds the lock held says so and waits. The lock is taken
with `flock`, and where there is none, as on macOS, with `perl`, which macOS ships; the system lets it go
when the install ends, however it ends.

The install tests stop an upgrade at every step to check this, run two installs at once, with and without
`flock`, and fail an upgrade after Pi's and Goose's image is built and after the gateway's plugin is replaced.
It is the same protection
[D-116](#d-116-a-gateways-plugin-swap-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill) gives a
gateway's plugin.

**Why.** The installer used to replace the core first, and the support files later. One that failed in between,
as when the Claude Code driver's packages or a container image could not be made, left every agent of every
harness to start, at its next restart, on the new core with the old files. Agents other than the one being
installed were broken by an install that said it had failed. Without the lock, a second install could take an
upgrade still under way for one left by a killed run, fail, and put the old files back under the first, which
then said it had installed the new ones. Without the whole list first, another harness's install could take a
killed upgrade for its own before the killed run had named its harness's files.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-135 The import gives owner-only rights where it cannot tell the old sender policy, and names what to check

**Decision.** Wherever `marmot-agent import` cannot tell exactly which sender policy (who may prompt, who may
approve) the old connector applied to the account being moved, it lets only the owner prompt and approve, the
default of [D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve). It says so plainly in its
summary, and names each old setting that may have applied, with its old value as written, for the person to
check and widen `prompt` by hand. This extends the rule of
[D-089](#d-089-an-install-moves-over-by-importing-its-account-home-in-place) that nobody may do more after the
move than before. Two places it applies, found in issues #138 and #147:

- **OpenClaw's accounts.** The import reads the sender policy of the one account MDK's OpenClaw plugin ran on
  this home, as the plugin picks it: with `channels.marmot.accounts`, each named account runs on its own
  settings and a `senderPolicy` at the channel's root applies to none of them; without, the root is the one
  account. An account ran on this home when its `socketPath` is the old connector's socket and its
  `accountIdHex`, if it has one, is the agent's. Only literal settings tell: an account may have run on this
  home when its `socketPath` or `accountIdHex` uses a `${...}` reference, which OpenClaw fills in from the
  gateway's environment, when it names no `socketPath`, when its `socketPath` is relative or starts with a
  `~` other than `~/`, which the plugin leaves as written, or when its socket is not certainly another place
  than this home's, and every account may have when the settings include another file with `$include`. Two
  paths are the same place when they are written alike, or when the file system resolves them alike; a
  `~/` is joined to the user's home as a string, as the plugin does, and `..` is never taken off by hand.
  Where no account ran on this home, where several did or may have, or where the account has no policy of
  its own (so the gateway's environment gave it one), only the owner may prompt. Only the policies of the
  accounts that ran or may have run on this home are named to check, never the root's when `accounts` is
  set, and an account with no policy of its own names the gateway's environment settings instead.
- **systemd environment files.** A unit's `EnvironmentFile=` is read as systemd reads it, not as a shell
  would: quotes inside a bare value are kept, a value runs on over a line's end inside quotes or after a
  backslash, `#` and `;` start a comment only at a line's start, and a line whose name is not a name, such
  as `export A=1`, sets nothing. Where systemd has read a line otherwise by version (a line after a comment
  ending in a backslash, which systemd before 254 took as comment too; a backslash in double quotes before
  anything but `"`, `\` or the line's end), or a line such as `export A=1` meant a name, that name is unsure.
  An unsure sender list lets only the owner prompt; any other setting the import reads, such as the relays,
  makes the service doubtful, which stops the import as before. So does a file holding a NUL byte, which
  systemd refuses whole.

The old values are shown only to the person running the import, in its summary and its next steps, and never
logged ([D-071](#d-071-what-may-be-logged)).

**Why.** A move that guessed could give more people the right to prompt than the old connector did: the
import read OpenClaw's root policy for every install, where the plugin gives a named account only its own, and
read an environment file a line at a time as a shell would, so a quoted value running over two lines could hide
a list from systemd and show it to the import. Failing closed keeps the agent safe; naming the setting and its
value keeps the person from losing access they meant to give. Matching OpenClaw's accounts only on literal
settings, rather than reproducing how OpenClaw and the plugin fill paths in, keeps the import from widening
rights where it would have reproduced them wrong.

Decided 2026-11-23. The project owner decided.

## D-136 For the pilot, the agent's full activity goes only to the owner's two-person chat, unless the owner lists the group

**Decision.** For the pilot, the agent's full activity, everything beyond its words, goes only to the owner's
two-person chat with the agent. Every other chat, a group or someone else's two-person chat with the agent, is sent
what the people in it need to follow the agent and to answer it:

| Item | Sent to every chat? | Why |
| --- | --- | --- |
| `message`, the agent's own | Yes | Its words to the people there ([D-022](#d-022-the-agents-words-are-ordinary-chat-messages)) |
| `turn` | Yes | Its start and end show the agent working, and how the turn ended |
| `approval` and `question`, a subagent's too | Yes | Someone in the chat answers them ([D-010](#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)) |
| `error` | Yes | It says why the agent stopped, or did not do what was asked |
| `session` | Yes | It says what can be set and done, which the controls people send need ([D-029](#d-029-three-shared-controls-and-the-rest-comes-from-the-harness)) |
| `reasoning`, `tool`, `plan`, `task` | Only with full activity | How the agent works |
| `notice` from the harness that settles a request | Yes | It says what came of an answer or a control, such as a refused new session or a failed compaction ([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)) |
| Any other `notice` from the harness | Only with full activity | About that work: compaction, a retry, a warning, a model change, a command's output |
| `message` of a subagent, with a `parent` | Only with full activity | A subagent's words are part of its work, not the agent's words to the chat ([D-117](#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them)) |

Hermes hands its channel its tool activity as formatted lines, which the plugin sends as the agent's commentary
messages, never as `tool` items
([D-118](#d-118-the-channel-plugin-contract-is-accepted-as-written)), so a Hermes agent's tool narration still
reaches every chat.

A chat that is not sent the agent's full activity is sent each item it is sent without its native payload: its
shown fields only, the type's own fields and the plain rendering. A native payload can hold what such a chat is
not sent: Claude Code's turn end after a denied approval carries the denied tool's input. The owner's two-person
chat and the groups listed are sent the native payload, and the core keeps the whole event either way. This is an
exception to [D-007](#d-007-one-shared-event-vocabulary) for those chats.

The core's own notices, which say why something a person sent was not acted on or was not delivered, go to the
chat they concern, as do the files the agent sends
([D-077](#d-077-a-refused-notice-says-why-something-was-not-acted-on),
[D-097](#d-097-what-stands-in-for-a-send-that-is-dropped),
[D-090](#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder)).

The owner turns full activity on for a group in the settings file, as `full_activity = ["<group id>", ...]`,
MDK's group id in hex, or for every chat as `full_activity = "all"`. None is listed by default. As with every
setting, a change takes effect when the core starts again, and there is no turning it on from the chat
([D-073](#d-073-standing-settings-live-in-a-configuration-file)). The setting's name and shape are provisional.

The core applies the rule, the same for every harness and gateway; a driver or a channel plugin never decides
what reaches whom ([D-004](#d-004-four-layers-with-drivers-behind-one-trait)). A chat is the owner's two-person
chat while MDK's members of it are exactly the agent and the owner: MDK marks it direct, no name and two
accounts, and those two accounts are theirs. The core reads this from MDK before each send, each version of an
item and each try again included, and after a restart before anything queued goes; it saves none of it. So a
person added to the owner's chat, by the owner or anyone, is sent none of the agent's full activity from then
on, even with no message since: not what was queued before they came, nor later versions of an item the chat was
sent before. Such an item stays in the chat as it was last sent. An item the chat is not sent is still kept by
the core, so a Stop there still stops a subagent the chat is not shown
([D-117](#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them)), and what it
settles is settled by the next version of its turn, or else of the chat's session, so the person who asked sees
the request settled by what came of it
([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)). With neither in the chat,
a notice of the core's own settles it, "Done": "The agent acted on this. What it did is shown only where the owner
sees all its activity." When MDK cannot say who is in a chat, the core waits and reads it again before it sends.

This narrows [D-008](#d-008-pass-everything-through) and
[D-009](#d-009-groups-are-first-class-and-every-member-sees-agent-activity) for the pilot, settles
[Q-11](open-questions.md#q-11-what-keeps-sensitive-output-from-spreading) for the pilot and narrows
[Q-12](open-questions.md#q-12-how-many-events-is-too-many). It answers issue 152.

**Why.** Reasoning and tool output can hold secrets from the owner's machine, and everything sent to a group
stays in every member's history. Only the owner prompts by default, and should see what the agent does before
anyone else does. The people in a group need the agent's answers and what they can answer, not its working. A
client that hides tool rows does not withhold them, so the core holds them back before anything reaches a relay.

What the builder chose, and the owner has not yet seen: the exact list above, `session` and `error` among what
every chat is sent, a harness's notice that settles a request among them, the setting's name and shape, and the
words of the core's notice that settles in place of an item held back. The review of the change found that a
chat's membership was read only when someone wrote in it; the builder settled the open point of a person added to
the owner's chat with the strict answer above, and decided, following the owner's decision for the pilot, that a
chat without full activity is sent no native payload. That MDK keeps a direct chat's two accounts current, as the
core reads them, is read from MDK's source and not tested against MDK.

Decided 2026-11-23. The project owner decided the rule; the builder decided the points above, on 2026-11-23.

"Not what was queued before they came" is narrowed by
[D-148](#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit):
a send of the full activity that the core handed to MDK while the chat was the owner's, and that MDK keeps and
posts later, can reach someone who joined in between.

## D-137 A secret answer's membership check is not bound to the epoch it is sent under

**Decision.** Secret answers stay as they are
([D-041](#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat)): the chat client offers one only in a
two-person chat, and the core accepts one only from a chat with two members. The check of who is in the chat is
not bound to the MLS epoch the answer is encrypted under, so a person added between the check and the send could
read the answer. This is an accepted risk. Issue 149 proposed binding the two, and sending people to the
harness's terminal until then; neither is done.

**Why.** People are added to a chat to see what happens there.

Decided 2026-11-23. The project owner decided.

## D-138 A connector that stops reading is given up in a request's time

**Decision.** The core's 30 seconds for a connector to take a request cover writing the request too, and every
other line the core writes to a connector, such as its answer to a `send_file`, has the same 30 seconds. A
connector that stops reading, so that a line no longer fits in the socket, is given up when they run out, as one
that does not answer is, and the core goes on. A prompt of which part was written is told as perhaps not
delivered ([D-129](#d-129-a-prompt-whose-answer-a-dropped-connection-lost-is-told-as-perhaps-not-delivered)), and
one of which nothing was written as not delivered; neither is sent again
([D-037](#d-037-a-prompt-is-delivered-at-most-once)).

**Why.** A write waits for room in the socket, and nothing bounded that wait. A connector that kept its connection
open and stopped reading held the core in the write: no chat went on, and a stop waited for it for good. It is
what [D-098](#d-098-a-harness-that-stops-reading-ends-its-connection) does for the built-in drivers, with the
socket's own time. Part of a line may reach the connector, perhaps all of it but its end, which a reader may take
as whole when the connection closes, so a part counts as the prompt perhaps delivered. Issue #134.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-139 Prompts a driver that ended was never given wait for the next driver

**Decision.** Once a driver has ended, as when a connector's connection goes or the core gives it up, the core
reads no more messages for it: it does not move a read mark past a message it did not try to hand on, and ends,
as for any driver that ends ([D-058](#d-058-the-driver-trait-and-the-capability-flags)). The next driver, such as
the next connector on the socket, is given those messages from the marks, each once. Only a prompt the core
handed to the driver that ended is told as not delivered or perhaps not delivered, and it is never sent again
([D-037](#d-037-a-prompt-is-delivered-at-most-once),
[D-129](#d-129-a-prompt-whose-answer-a-dropped-connection-lost-is-told-as-perhaps-not-delivered)).

**Why.** The core went on reading what was stored while it handed it on, and handed every later message to a
driver that had ended. Each was refused at once, the chat was told it was not passed to the agent, and its mark
was saved past it, so no later connector got it, though none had been tried. What arrives while no connector is
there already waits for the next one; this is the same. The core sees that a driver has ended by asking for its
next event without waiting, before each message it reads. A connection that goes after that look and before the
prompt is written still has that one prompt told as not passed. Issue #135.

Replaced in part by [D-180](#d-180-a-prompt-a-driver-refuses-as-having-ended-waits-for-the-next-driver-whenever-the-core-handed-it-on): such a prompt, none of which was written, waits for the next driver too.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-140 An answer or a control the agent cannot read is refused

**Decision.** A kind 1202 event that names the agent and that the core cannot read is settled with a `refused`
notice ([D-077](#d-077-a-refused-notice-says-why-something-was-not-acted-on)), as any other request is
([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)): with the reason
`not_supported` when its `action` is one the core does not know, and `invalid` when it has no details, or details
that are not an action, or an action whose fields are missing or of the wrong type. The right still comes first. An
event that cannot be read cannot say which right it needs, and every action needs one of the two
([D-032](#d-032-every-action-needs-one-of-two-rights)), so someone who holds neither is told `not_allowed` and
nothing else; someone who holds either is told why it was not read. As for any request, each event is settled
once, and one that names no item and is more than five minutes old is dropped. An event that names no agent, or
another, is not for this one, and the agent says nothing ([D-027](#d-027-answers-and-controls-are-one-kind-of-event-kind-1202)).

**Why.** The core used to drop such an event with no reply, so the sender's app showed the press as pending for
good. An app newer than the core will send actions the core does not know, and the reason lets it say so.

Decided 2026-11-23 by the builder, under the project owner's instruction to decide minor points.

## D-141 Subagent threads are built before the swap

**Decision.** Subagents as threads
([D-121](#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them) to
[D-126](#d-126-no-mls-group-for-subagents), and
[D-133](#d-133-an-edit-of-an-item-in-a-subagents-thread-repeats-the-threads-root-tags)) move out of the plan's
"not before the swap" and into M4: they are built before the release. The Codex driver moves from showing its
subagents flat ([D-117](#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them)) to
threads; a driver whose harness tells no subagents apart is unchanged.

**Why.** MDK places agent items in subagent threads, keeps the agent's own words there quiet and shows a late
joiner an item from its edits (MDK pull requests 3622, 3630 and 3640, at the revision the core uses), so what is
left is this repository's part. Shown flat, a busy subagent fills the main chat, and the first release is what
people will judge.

Decided 2026-11-24 by the project owner.

## D-142 How subagent threads are built

**Decision.** The choices made in building
[D-121](#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them) to
[D-124](#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners)
and [D-133](#d-133-an-edit-of-an-item-in-a-subagents-thread-repeats-the-threads-root-tags), where they left
something open:

1. **A driver names the subagent an event belongs to** in a new field of the event, `subagent`: the item id of
   that subagent's `task` item, in the same chat. A nested subagent's `task` item names the subagent that started
   it; a top-level one names none, and is in the main chat. The events stay in the chat's session, numbered as
   before, and under the turn the person started. This is D-121's "a subagent's session names its `task` item",
   as built: a subagent has no session of its own, since a session is what a chat resumes and sets.
2. **One more capability flag, `subagents`**: whether the harness tells its subagents apart
   ([D-124](#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners)).
   The core puts events in threads only for a driver that says yes. Codex's says yes; every other driver says no,
   and a connector on the socket that leaves it out says no. Claude Code's driver says no until it is moved to
   threads; its subagents stay flat.
3. **The core places an item when its first event goes out**, under the first event of its subagent's `task`
   item, in the tree that item is in or is the root of, and keeps that place with the item for every later
   version, across a restart. A group's sends go out in order, and a driver makes a `task` item before the work
   it names, so that item has gone out first. An item whose subagent's `task` item has not gone out to that chat,
   as in a chat not sent the agent's full activity
   ([D-136](#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group))
   or after a dropped send, goes in the main chat with no thread tags, as any item does. Nothing is lost.
4. **A subagent's words are MDK's thread message** under the subagent's `task` item
   ([D-122](#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar)). MDK writes their `E`, `K`, `P`, `e`
   and `k` from that item's place and leaves NIP-22's parent `p` off, as on every comment it sends. An item
   carries the parent `p`, naming the agent; MDK reads a `p` that names its sender as no mention (MDK pull
   request 3622), which settles what D-121 left open about it. The core sends words once they are complete, as
   it sends the agent's own. MDK keeps no edit of a thread message under a token, as it does a chat message's
   or an item's (`submit_tagged_edit_for_local_send`, at MDK `029e057`, takes only those), so a later version of
   a subagent's words in its thread is MDK's plain edit of the thread message (`edit_message`), which MDK takes
   for a comment. Tried again after an answer that was lost, it shows the same words again. Two later versions
   in one second may show in either order, as MDK orders a message's edits by their second; a harness sends a
   subagent's complete words once, so this is not expected to be seen. And when MDK cannot place the thread
   message, because the `task` item, or the thread's root, disappeared under the chat's retention, MDK does not
   take it, and the words go in the main chat as the agent's words, and stay there in every later version.
5. **Every item of a subagent's carries a `subagent` tag** with that field, as `turn` and `session` are carried.
   An edit carries only the root's tags (D-133), so a member who joined late can place it in the tree but not
   under its nested subagent; the tag lets an app find that subagent's view by its `task` item's id.
6. **A Codex subagent is one `task` item**, `task:` and its thread's id, which each of its turns opens again, as
   D-121's row "edited as it works"; under D-117 each turn was a `task` item of its own. The parent's `subagent`
   tool items, which start a subagent or give it more work, stay in the main chat as the agent's own tool calls,
   and the row names the newest as its `subject`. The row shows the state of the subagent's newest turn: the end
   of an earlier turn, read late, leaves it as it is, and only adds, on the row's text, that the earlier turn
   failed, with its error, when it did.
7. **Not built yet.** The flag and the `task` field for writing to a running subagent
   ([D-123](#d-123-stop-and-writing-in-a-subagents-thread)) come with the first driver that says yes; until
   then the core reads no comment in a thread, as it reads only chat messages and kind 1202. And Stop in a
   thread: a Stop that names a subagent's item names its turn, the person's, so it stops the agent's turn and
   every subagent under it, which is more than D-123 asks; stopping only that subagent and those it started needs
   a Stop that names the subagent, which a later change adds. (Replaced for Stop by
   [D-149](#d-149-stop-in-a-subagents-thread-stops-that-subagent-alone), which adds it.)

This replaces in part
[D-117](#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them): Codex's subagents
are shown in threads, each as one `task` item.

**Why.** The core, not a driver, decides where an event goes (D-004), so a driver gives only which subagent's
work it is, in terms it already has: its own item ids. A session of its own for each subagent would have changed
how every chat's session is resumed and set. Placing an item when it goes out, from what has gone out, needs no
event to be held, and falls back to the main chat whenever there is no thread to put it in. One row per subagent
is what D-121 describes, and a subagent given more work is the same subagent.

Decided 2026-11-24 by the builder, under the project owner's instruction to decide minor points. Points 5 and 6,
and what point 4 says of a later version and of words whose thread is gone, are visible to apps and provisional
until the owner accepts them.

Noted 2026-11-27: MDK's history was rewritten on 2026-11-24, and `029e057`, cited above, is no longer on it: it was the merge of MDK pull request 3640. Its content is in `bea7b400`, the revision the
core uses.

## D-143 One issue tracks the CI jobs that flake

**Decision.** One open issue, "Flaky job tracker", ranks the CI jobs that failed and then passed when they were run
again in the same run, on the same commit: on pull requests, in the merge queue, on `master` and in the nightly
runs. A workflow of its own, `.github/workflows/flake-report.yml`, brings it up to date after each run of CI or
Nightly that was a rerun, once a day, and by hand. Its script, `tools/ci/flake_report.py`, lists the runs of the
last 14 days that were run again and the jobs of each attempt, and counts each job that failed or timed out in one
attempt and passed in a later one, by its workflow and its name, a matrix leg's name with its leg. The `CI passed`
gate, which fails whenever another job does, counts only when every other job's latest result up to its attempt
passed: a job a partial rerun did not run again keeps its earlier result, which the gate read. A job that failed in
every attempt is not counted, since it may be a real failure; it is counted if a later rerun passes. The nightly
mutation shard is chosen by the day the run was created, so a rerun tests the same mutants.

It reads no log, and names no test. A job's log mixes what the runner printed with what the tests printed, so a
test could put its own text into the public issue. Which test flaked is for machine-readable results that CI does
not write yet, and is issue [#184](https://github.com/marmot-protocol/marmot-agents/issues/184). For each job the
issue shows its workflow and name, its flakes in the last 30 days and in all, where they happened (pull request,
merge queue, `master`, nightly), when it was first and last seen, and a link to the last job that flaked; each name
is shown as code, as written. A name is shown only if the default branch's `ci.yml` or `nightly.yml` names a job
so, a matrix leg's name with its leg filled in: a pull request's CI runs the pull request's own workflow, which can
name a job anything, so a job named otherwise is counted under one label, "(a job not on the default branch)", with
its link, and its own name is never kept. It is ranked by the flakes of the last 30 days, then by all. What the
script keeps between runs (each job's count, its flakes by day over the last 30 days, its latest flake, and the
jobs it has counted, so that none is counted twice) is in the HTML comment that ends the issue's body, on one line;
the issue is its own store. If that comment cannot be read, the body is empty, or what it keeps no longer fits an
issue, the run fails and leaves the issue as it was: no count is dropped to make room, and the table is shortened
first. Only when no tracker is open does a count start afresh. A listing of runs or jobs longer than the script
reads fails the run too, rather than count from part of it. The workflow runs the default branch's code only, also
when started by hand, and its token may list the runs and their jobs and write issues, nothing more. It edits only
an open tracker it opened itself and labelled `flaky-tracker`, found by that label and not by its body, so a
tracker whose comment was mangled fails the run rather than lead to a second one. If there is none, it opens one,
so closing it starts a new count.

**Why.** Flaky tests were found one at a time, in builders' reports and in #86 and #96, and a rerun that passed hid
each one. A job that fails and then passes on the same commit is a flake without anyone having to judge it, and a
count over time shows which to fix first and whether a fix held. MDK keeps a leaderboard the same way
(its `flake-report.yml`), from test reports; CI here writes none yet, and reading the test names from logs let a
test's own output into the issue, which both reviews of the first version showed. The owner chose to track jobs
now and name the tests from machine-readable results later. The state is in the issue, not in artifacts that
expire.

Decided 2026-11-24 by the project owner: a tracker like MDK's, jobs only for now, the tests later from
machine-readable results. The details are the builder's, under the instruction to decide minor points.

## D-144 Property tests run on every pull request, Kani in the full gate, and fuzzing nightly on a pinned nightly compiler

**Decision.** The fourth step of the verification plan runs in CI's tiers
([D-130](#d-130-ci-runs-in-three-tiers-with-one-required-check)) like this:

- **Pull request**: the property tests, at proptest's normal 256 cases each, in the workspace's tests: the core's
  (`crates/marmot-agent/tests/properties`) and the built-in translators' over changed recordings (the conformance
  suite's `tests/translators.rs`). So does every fuzz target's check over its seeds, on the workspace's compiler
  (`crates/marmot-agent-fuzz/tests/seeds.rs`). A failing case found anywhere is committed in the test's
  `proptest-regressions` file.
- **Merge queue and `master`**: the Kani proofs, `cargo kani -p marmot-agent`, in a job of their own. Kani builds
  the core with MDK under its own compiler before it proves anything, which is more than a pull request's time. *(Amended by [D-161](#d-161-ci-no-longer-runs-on-pushes-to-master): pushes to `master` no longer trigger CI.)*
- **Nightly**: the property tests at 10,000 cases each, and every fuzz target for five minutes, from its seeds and
  from what the nights before found, which a cache carries from night to night. An input that makes a target fail
  is kept as the run's artifact for 30 days; the issue the night opens names the target and the input's file name,
  nothing it holds.

The tools are pinned: proptest by `Cargo.lock`; cargo-fuzz 0.13.2 and Kani 0.68.0 by version, and by the
checksums of their release files; Kani installs the compiler its release names. Fuzzing uses that same nightly
compiler, `nightly-2026-08-21`, the one that builds MDK; a later nightly tried (2026-11-01) failed on one of MDK's
dependencies. Everything else builds with MDK's compiler, as
[D-053](#d-053-the-rust-workspace-builds-with-mdks-compiler-and-forbids-unsafe-code) says. cargo-fuzz sets the
compiler's flags for its own build, which replaces the ban on unsafe code there; the fuzz targets hold none, and
every other build keeps the ban. The fuzz targets' inputs are made by the fuzzer from the repository's seeds,
never from real use.

**Why.** The property tests take about a minute in all and catch what example tests do not, as the first one did
(a request MDK refuses that the core acted on), so they belong where a change is made. Kani's proofs are bounded
and change rarely, but its build is long. Fuzzing finds more the longer it runs, so it runs where time is cheap,
and a corpus kept between nights lets each night start where the last stopped.

Decided 2026-11-24 by the builder, under the project owner's instruction to decide minor points.

## D-145 The core bounds what it holds for a connector, and the files it sends at once

**Decision.** While the core waits for a connector's answer to a request, it holds what the connector sends
meanwhile for later, and it holds the refusals it owes for a `send_file` that reused an unanswered id until its
next write. All of that together is at most 32 MB, counted by the lines it came in, eight messages of the largest
size ([D-033](#d-033-the-sockets-framing-is-json-rpc-20-one-message-per-line)). A connector that sends more is
given up at once, and the line that would pass the budget is not kept, as one that does not answer in time is
([D-138](#d-138-a-connector-that-stops-reading-is-given-up-in-a-requests-time)): the request waited on is told as
perhaps taken ([D-129](#d-129-a-prompt-whose-answer-a-dropped-connection-lost-is-told-as-perhaps-not-delivered)),
what was held still reaches the chat, and the messages it was never given wait for the next connector
([D-139](#d-139-prompts-a-driver-that-ended-was-never-given-wait-for-the-next-driver)). No socket message changes.

Of the files an agent asks to send ([D-090](#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder)),
whatever its driver, the core reads and uploads two at once, and holds at most 16, those uploading included. The
others wait their turn, in order, as a path only. One more is refused as `busy`, at once, with a refused notice in
the chat. Each upload is a task the core owns: when the core stops, or its driver ends as when a connector goes,
the uploads under way are stopped and the driver is told they failed, with nothing in the chat, since MDK may
already have taken the file; the files waiting were never offered to MDK, and the chat is told they were not sent.
An upload starts only while the driver is waiting for more, so a file a driver hands over once it has ended, as
one a connector asked for before the core gave it up, or one still waiting when telling the connector how a file
went gives it up, is never offered to MDK, and is told in the chat as not sent. A read cannot be stopped, so a file
being read when its upload is stopped goes on being read until the read returns, holding one of the two places for
reads, and then reaches no MDK, since the upload that would have sent it is gone. Every read has returned before
the program ends. A connector's core does not wait for one as the connector goes, so a read stalled on a slow or
network folder holds up no next connector, which reads in one place fewer until it returns. A save that fails ends
the files the same way, before the core stops. MDK may still finish a send it was already handed after the core
stops waiting for it, which is why the chat is told nothing of it. No upload goes on after the core.

**Why.** The core answered only for single lines. A connector that went on writing while a request waited, as
fast as a local socket carries, made the core hold all of it for up to 30 seconds, without bound; and each file
the agent asked for started an upload of its own, holding up to 64 MB, with no limit on how many, which went on
uploading after a lost connector, into a chat whose turn the core had already closed. The budget is far above what
a connector sends while it hands a request to its harness, and giving up is what the protocol already does for a
connector it cannot keep up with. The files past the two uploading wait, rather than being refused as busy,
because an agent often sends a few in one turn and no harness tries a refused one again by itself; a waiting file
costs only its path. Issue #150.

Decided 2026-11-24 by the builder, under the project owner's instruction to decide minor points.

## D-146 The Hermes plugin answers a request to confirm a command by its id

**Decision.** An answer to Hermes's request to confirm a command, the `question` item of
[D-048](#d-048-a-request-to-confirm-a-command-is-a-question), goes to Hermes through Hermes's own call for it,
`tools.slash_confirm.resolve`, which names the session and the confirmation's id. The plugin never types `/approve`,
`/always` or `/cancel` for it. It answers only while that confirmation still waits in Hermes; when it does not,
because it was answered, a newer one replaced it or it waited longer than Hermes waits, the answer is refused as
`not_waiting` and nothing else is answered: not an approval that started meanwhile, and not another command's
confirmation. The plugin stops five seconds before Hermes's own limit, so that an answer it hands Hermes is one
Hermes runs. Before it answers, the plugin asks Hermes's own check of who may answer, `slash_confirm_refusal`, with the
person who answered and the chat's Hermes settings, as Hermes's own platforms do: where Hermes's settings name
admins for the chat, only someone Hermes lets run the confirmed command may confirm it, whatever the core allows.
A person Hermes refuses is refused as `not_supported`, with Hermes's words, and the confirmation still waits.

The plugin answers the core's request as soon as Hermes has taken the confirmation, which Hermes does before it
runs the command: the question shows the answer at once, and the command runs on by itself, however long it takes
(a `/new` first closes the old session's tools). A request is taken once the harness has it
([D-058](#d-058-the-driver-trait-and-the-capability-flags)), and the core gives a connector up when one is not taken
within 30 seconds ([socket protocol](design/socket-protocol.md#when-things-go-wrong)); the core going away
meanwhile does not cancel the command.
When the command finishes, Hermes hands back its reply to the answer instead of sending it, and the plugin sends it
as the command's output, in the session Hermes asked in, outside any turn. That reply settles the command the confirmation is for, and no
other: in a group, where each sender has a session and each can have a confirmation waiting, an answer to one
settles nothing of another's. When Hermes gives no reply, a notice says so (`Hermes gave no reply to /new.`), and
settles the command. The notice never says the command ran, as the remaining limit below explains.

Nothing else changes. Answering the confirmation still needs the right to prompt, and answering an approval the
right to approve. The confirmation and its answer show as before; the one difference people see is that the
answer no longer starts a turn of its own, which the typed `/approve` did. The plugin does not claim Hermes's
confirmation buttons, so Hermes still sends the question as text, and a person who types `/approve` in the chat is
still read as [D-048](#d-048-a-request-to-confirm-a-command-is-a-question) says, by the core and then by Hermes,
which runs the same check of who may answer on typed text.

**Why.** A structured answer names the item it answers, but typed text names nothing. Hermes reads a typed
`/approve` as the answer to an approval when one waits in the session, and otherwise to the session's current
confirmation, whatever its id. So between the core's check and Hermes reading the text, an approval that started
or a confirmation that replaced the first would take the answer, and someone who may only prompt could approve
what the agent does ([D-049](#d-049-a-standing-approval-is-an-ordinary-approval-answer) records this gap). Hermes's
call checks the id, and its own platforms with buttons answer through it, after asking its check of who may
answer, which the call itself does not ask: without that check, an answer through the call could do what the
same person typing `/approve` could not. The call is not on Hermes's list of what a plugin may use, so it is
listed in [what the plugin relies on](reference/hermes-messages.md#what-the-plugin-relies-on-outside-the-published-plugin-interface),
and the Hermes move-over test fails when it changes. Issue #153.

Not closed: a `/approve` that a person who may only prompt types while a confirmation waits still has the gap
D-049 describes, as typed text always will. Answers to an approval card and to a question ask Hermes's check with
the chat's settings too, as [D-151](#d-151-the-hermes-plugin-asks-hermess-own-check-of-who-may-answer-an-approval-card-or-a-question)
records.

A limit for the pilot: the plugin keeps its own record of which confirmation waits, and for whom, beside
Hermes's. When two people confirm commands at once in one group chat, that record and Hermes's can disagree, so one
person's answer may settle the other's command, or leave a confirmation that still waits in Hermes unanswerable
from the chat. Neither lets anyone answer what Hermes would not let them answer. Issue #183 rebuilds the record
from Hermes's own.

A remaining limit: Hermes's call says `None` both when it ran nothing and when the command it ran gave no reply, and
at the pinned commit Hermes offers no other way to learn that it ran the command. Hermes checks the confirmation's
age again when it runs it, so if the gateway stalls longer than the plugin's five seconds between the plugin's check
and Hermes's, a confirmation the plugin handed over can be one Hermes drops without running it. The answer then
shows as given, and the notice says only that Hermes gave no reply, which is true either way; a person who sees the
command did not happen asks again. The plugin does not pass Hermes's call a longer `timeout` to close this gap:
that would override Hermes's own 300-second rule for confirmations, and Hermes's rule stands.

Replaced in part by
[D-155](#d-155-the-hermes-plugin-keeps-each-request-to-confirm-a-command-as-hermes-keeps-it): the plugin keeps
each confirmation by Hermes's session and id rather than its own record, which closes the pilot limit above, and a
command Hermes confirmed is settled by its effect, or by the answer or close that says it will not run, not by
Hermes's reply or the no-reply notice, except a reply that says the command was refused. Answering by id through
Hermes's call, Hermes's own check of who may answer, and Hermes's 300-second rule stand.

Decided 2026-11-24 by the project owner.

## D-147 A request whose effect a connector showed before its connection dropped was taken

**Decision.** While the core waits for a connector's response to a request, it holds the events it reads
([D-058](#d-058-the-driver-trait-and-the-capability-flags)). When the connection then drops, or the connector is
given up, before the response, a request that one of those events already settles, or a prompt that a turn of
the prompt's own chat among them names, counts as taken: the event settles it, once, and the chat is not told it failed or was perhaps
not delivered ([D-129](#d-129-a-prompt-whose-answer-a-dropped-connection-lost-is-told-as-perhaps-not-delivered)).
A request no event showed is refused as failed, as before. Taken or not, nothing is sent again
([D-037](#d-037-a-prompt-is-delivered-at-most-once)).

**Why.** A connector may send the event that shows an answer's effect before its response to the answer; the
OpenClaw plugin does. The core refused the answer as failed when the connection dropped between the two, then
handed on the event that settled it: the request was settled twice, and the chat was told the answer failed
though it was taken. The socket connectors' run of the conformance suite (`dropped_after_delivery`) found it.
Issue #154.

Decided 2026-11-24 by the builder, under the project owner's instruction to decide minor points.

## D-148 For the pilot, a send of the full activity that MDK keeps and posts after someone joins is an accepted limit

**Decision.** A send of the agent's full activity that the core handed to MDK while a chat was the owner's
two-person chat, and that MDK keeps and posts later, can reach someone who joined the chat in between. This goes
against [D-136](#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)'s
"none of what was queued before they came", and is accepted for the pilot. The core cannot take back a send MDK
holds; it holds back everything it has not yet handed to MDK, and a send MDK was handed goes on only as MDK was
handed it. The owner will ask MDK for a way out, either a call that withdraws a queued send or MDK refusing to
post a send queued before a member joined, and the core will use whichever MDK offers. The model
`ActivityHoldBack` shows the limit as the counterexample
[`hold-kept-posted-later`](../formal/tla/counterexamples/hold-kept-posted-later.json)
([its page](../formal/tla/ActivityHoldBack.md#what-the-model-found)).

The limit also covers a narrower path to the same outcome. To learn what became of a send MDK was handed, the
core asks MDK whether it still holds the token, and if so hands the same send again under it; it treats a token
MDK may forget within 60 seconds as gone. If the core stalls for longer than that between the two calls, MDK may
have forgotten the token and take the send as new, posting it to the chat as it is then. That too is accepted for
the pilot, and the request to MDK covers it. Issue #194 is the core's own fix, to make before the release: never
hand MDK such a send again, and learn its outcome by asking only.

**Why.** MDK keeps a send under its token from the moment it takes it, and posts it to whoever is in the group
when its engine sends it. Nothing the core can call at MDK's pinned revision withdraws it. The moment is short,
and so is the one between the core's read of a chat's members and MDK taking a send, which is the same limit.

Decided 2026-11-24 by the project owner.

The narrower path is narrowed further by
[D-152](#d-152-a-send-mdk-was-handed-in-a-form-its-chat-is-no-longer-sent-is-asked-after-never-handed-again): the core
no longer hands such a send to MDK again, but for one case, a send whose first answer from MDK the core lost.
It is closed by [D-157](#d-157-the-core-goes-on-with-the-event-mdk-names-for-a-send-it-holds-and-never-hands-such-a-send-to-mdk-again): the core hands no such send to MDK again on any path. The limit itself stands.

[D-158](#d-158-the-core-does-not-limit-a-send-to-the-chats-members-a-send-in-flight-when-the-owner-adds-someone-may-reach-them) accepts the limit for good: the core does not limit sends to the chat's members, and asks MDK
for nothing more.

## D-149 Stop in a subagent's thread stops that subagent alone

**Decision.** This builds the Stop in a thread of
[D-123](#d-123-stop-and-writing-in-a-subagents-thread), which
[D-142](#d-142-how-subagent-threads-are-built), point 7, left unbuilt.

1. **Kind 1202 is unchanged.** An `interrupt` whose `e` names a subagent's `task` item (one of kind
   `subagent`), or an item in a subagent's thread, stops that subagent and the subagents it started: the
   subagent of the item, which is the `task` item itself, or else the subagent the item belongs to. The person's
   turn and the other subagents go on. One whose `e` names any other item, or none, stops the turn and every
   subagent under it, as before. So does one that names a subagent's item shown in the main chat, as its
   approval in a chat not sent the agent's full activity, which sees only the turn. The `e` already says which
   subagent is meant, so no tag is added. A Stop of one subagent is passed on even when the item it names has
   ended, since a subagent that finished can have started one that still works, and the driver, which knows,
   refuses it as `nothing_to_do` when nothing of it does ([D-123](#d-123-stop-and-writing-in-a-subagents-thread)).
2. **The driver's `interrupt` control, and the socket's `interrupt` message, take an optional `subagent`**: the
   `task` item's id, as the driver named it. Left out, it is today's Stop.
3. **One more capability flag, `stop_subagent`**: whether a Stop can stop one subagent and those it started
   while the turn goes on. This extends [D-058](#d-058-the-driver-trait-and-the-capability-flags). The core names
   a subagent only to a driver that says yes, and that says `subagents` too. To any other, a Stop in a thread
   stops the whole turn, as one in the main chat does, and the flag is how that driver says it cannot stop one
   alone. Codex's driver says yes; every other driver says no, and a connector on the socket that leaves it out
   says no.
4. **Codex** interrupts the running turn of that subagent's thread and of every thread it started, at any
   depth, each parent before those it started, and a turn of theirs whose start is read while the Stop waits.
   It does not interrupt the person's turn, nor any other subagent. The Stop is settled once, by the end of the
   last of them, as a Stop of the turn is. A Stop naming a subagent the driver does not follow is refused as
   `nothing_to_do`, not widened to the turn.

**Why.** D-123 asks for this, and D-142 left it out until the Stop could name the subagent. The `e` tag already
names the item a press was on, so an app needs to send nothing new. A flag rather than a notice: the core knows
before it passes the Stop on, and a driver that cannot stop one subagent need do nothing new.

Decided 2026-11-24 by the builder, under the project owner's instruction to decide minor points. It is visible to
apps and connectors, and provisional until the owner accepts it.


## D-150 The core matches a typed command as the harness reads it, failing towards asking for the right to approve

**Decision.** When the core checks a message against the commands a connector lists as needing the right to
approve ([D-047](#d-047-a-listed-command-says-where-in-a-message-it-counts)), it reads the message as every
harness may: it skips before a command whatever any of the languages the harnesses are written in skips as space,
it takes a line as starting after whatever any of them breaks a line at, and it compares a command's name in each
way any of them lowercases text, ASCII only, by Unicode's rules, and folded by Unicode's case folding. A command
whose name ends in a mark that cannot continue a name counts whatever follows it, as a harness runs the rest of
the line after such a shorthand. Where the harnesses read a message differently, the core takes every reading, so
it asks for the right to approve whenever any harness could read the message as such a command. It may ask more
often than a harness would, never less. A connector lists a command by every name its harness runs it under.

Two tests hold it there, beside the gateway move-over tests and in the same images. Each names the commands that
need the right to approve by their identifiers in the harness's own command registry, not by the plugin's list,
and fails when the registry no longer has one. It takes every name the registry gives those commands, in the form
the harness runs it, and requires the core to ask for the right to approve for each, alone and followed by more;
for OpenClaw, also for each directive of those it applies anywhere in ordinary text
([D-047](#d-047-a-listed-command-says-where-in-a-message-it-counts)), within such text. Then it makes thousands of
messages from all the harness's command names and asks the harness's own code, Hermes's command parser and
registry and OpenClaw's command parser, shorthand and directive parser, which it would run as such a command, and
the core must ask for that right for each. Each also checks every character the harness's language skips as space,
or lowercases or folds to plain letters. A command that one of OpenClaw's own plugins adds is not in its registry,
and these tests do not check it.

**Why.** The core trimmed and lowercased with Rust's ASCII rules, while Hermes, in Python, and OpenClaw, in
JavaScript, skip more characters as space and lowercase by Unicode's rules, and each runs a command by names the
connector did not list. So a message a harness read as a command that needs the right to approve could pass the
core as plain text, from someone who may only prompt.

Decided 2026-11-24 by the project owner.

## D-151 The Hermes plugin asks Hermes's own check of who may answer an approval card or a question

**Decision.** Before it answers an approval card, the plugin asks Hermes's own check of who may answer it,
`approval_refusal`, with the person who answered, the card's request, the choice and the chat's Hermes settings, and
it hands Hermes's call for the card the same settings. Before it answers a question the agent asked, it asks
`clarify_refusal` the same way. Hermes's own platforms with buttons ask both. A person Hermes refuses is refused as
`not_supported`, with Hermes's words, as for a confirmation
([D-146](#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id)), and the card or the question
still waits for someone Hermes lets answer it. Where Hermes's settings name no admins for the chat and narrow no
approvers, Hermes refuses no one, and nothing changes.

The core's own rights are checked first, as before: answering an approval still needs the right to approve, and
a question the right to prompt ([D-032](#d-032-every-action-needs-one-of-two-rights)). Hermes's check can only
refuse more.

**Why.** The plugin answered an approval card without the chat's settings, and a question without asking Hermes
at all, so a Hermes whose owner limited who may approve, or who may answer for someone else, let through someone
it would refuse on its own platforms. Both calls are on Hermes's list of what a platform plugin may use.

Decided 2026-11-24 by the project owner.


## D-152 A send MDK was handed in a form its chat is no longer sent is asked after, never handed again

**Decision.** A send the core handed to MDK in a form its chat is no longer sent, the agent's full activity once
the chat stopped being the owner's two-person chat, or an item with its native message or without, is not handed
to MDK again. The core learns what became of it by asking MDK (`local_send_status`), as built:

- **Its event.** MDK names the event a send will be when it takes it, and the core now keeps that with the send in
  its state file. MDK's status names the event again once its engine has sent the send. The send goes on to the
  core's record with that event, as MDK was handed it.
- **Queued.** While MDK keeps the send queued, the core asks again later, as it does for a send MDK holds, and the
  chat's later sends wait for it.
- **Never taken, or dropped.** A send MDK never took is held back, if it is the full activity, or goes as the chat
  is sent now, as before. One MDK took and dropped goes the same way, under a new token, as any dropped try does.
- **No longer known.** A send whose token MDK no longer knows, once the group's messages have disappeared since its
  first try, may have gone out. The chat is told so with the `failed` notice of
  [D-109](#d-109-how-the-cores-sends-survive-a-crash-as-built), "Something the agent sent may not have been
  delivered to the chat.", which settles what the send settled; it is no longer held back as a send MDK never took
  (issue 182). So is one whose event the core kept, whose token MDK no longer knows, however long the group's
  messages last now: MDK took it, so it let go of it since, as when the group's messages were made to last longer
  after it went (found in review, 2026-11-25).
- **A send an older core handed to MDK**, whose state file noted no form for it, is asked after the same way
  whoever is in the chat now, and is never handed to MDK again: the older core may have left its native message
  out, and MDK would refuse it in another form, settling what it settled a second time (issue 181). Where MDK has
  nothing under its token, it goes as the chat is sent now.
- **Its engine has it, naming no event.** When the core lost MDK's answer to its first hand-over, which takes a
  crash between MDK taking the send and the core saving its answer, MDK's status names no event while its engine
  has the send. The core never tells the chat such a send may not have been delivered while MDK says its engine
  has it, since MDK still posts it, and the notice would settle what it settled a second time. It asks again, a
  little later each time up to ten seconds, until MDK names the event, or lets go of the token with the group's
  messages, when the chat is told as above. The chat's later sends wait for it meanwhile; a stop does not.
- **The one path left.** Such a send, in the form the core noted it was handed, is handed to MDK again under its
  token, as it was handed, while MDK will keep the token for at least another minute: that gives its event at
  once. This is the narrower path of
  [D-148](#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit),
  left only there, and still accepted under it. The project owner chose to keep it, and to ask MDK for its status
  to name the event while its engine holds a send, which would make it unneeded.
- **A send MDK's engine keeps for good**, as one whose publish failed after its engine took it, keeps a status
  that names no event until MDK lets go of its token. Asked after, it holds up its chat's later sends until then:
  in a group whose messages never disappear, for good, unless the one path left hands it again. Other chats go
  on.
- **A known limit: a stall for good in a chat whose messages disappear within a minute.** A current core's send
  can also hold up its chat for good. It takes all of these: the core crashed after MDK took a send of the full
  activity and before it saved MDK's answer, so it kept no event; MDK's publish of it then failed for good, so
  its engine keeps it and names no event; someone joined the chat since; and the group's messages disappear
  within a minute, so the one path left is not allowed (`Side::lasts` is false), while MDK keeps the token of
  the failed send. That chat's later sends wait, after a restart too; other chats go on. The project owner
  accepted this on 2026-11-25 until MDK's status names the event while its engine holds a send, which has been
  asked of MDK. When MDK's pin moves to a release that does, this stall and the one path left both go.

The model `ActivityHoldBack` lets MDK forget a token, and catches a second hand-over after someone joins
([`hold-handed-again`](../formal/tla/counterexamples/hold-handed-again.json)), a forgotten send held back
([`hold-forgot-held`](../formal/tla/counterexamples/hold-forgot-held.json)), and a send MDK's engine has told as
lost ([`hold-settled-owned`](../formal/tla/counterexamples/hold-settled-owned.json)) as defects.

**Why.** MDK takes a token it has let go of as a new send. Handed again, a send of the owner's chat could reach
someone who joined since, if the core stalled for longer than its minute's margin between asking and handing
(issue 194). Asking alone cannot post anything. MDK's status gives no event while it holds a send, but MDK names
the event when it first takes it, so keeping that answer is enough on every path but the one above, which MDK's
status naming its event while its engine holds a send would close. A send MDK no longer knows may have gone, and
D-109 already says how the chat is told of one; one MDK's engine has has not been lost, and telling the chat it
might be settled its requests twice once MDK posted it (found in review, 2026-11-25).

Decided 2026-11-25 by the builder, under the project owner's instruction to decide minor points. What the chat is
told follows D-109. That the one path left stays, and the known limit above, were decided 2026-11-25 by the
project owner.

Replaced in part by [D-157](#d-157-the-core-goes-on-with-the-event-mdk-names-for-a-send-it-holds-and-never-hands-such-a-send-to-mdk-again): MDK's status names the event of a send it holds, so a send its engine has goes
on with that event, the one path left and the known limit are gone, and so is the stall of a send MDK's engine
keeps for good.

## D-154 An install takes over a killed upgrade only for the same agent and the same gateway

**Decision.** An upgrade a killed install left under way may hold a change to a gateway's plugin
([D-134](#d-134-an-upgrade-of-the-core-and-its-support-files-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)).
An install takes such an upgrade over, and finishes it, only when every change it holds that is not done is of
the same agent and to the same gateway, the one whose settings file the install changes (for Hermes, its
`config.yaml`; for OpenClaw, the file `openclaw config file` names). Otherwise, as for another harness's install,
it puts the whole upgrade back first, each plugin change with it, says so, and then installs as if none had run.
It does so before it reads whom the gateway serves or which plugin it has, as what it would read there may be
another agent's change half made.
So an install for a second Hermes or OpenClaw agent, or for the same agent pointed at another gateway, never ends
another agent's change half made. An install that fails puts its own plugin change back in the same order as the
others its upgrade holds, the last made first, and stops at the first that cannot go back.
A gateway's install takes the install lock before it reads anything of the gateway, on every run, a first
install too, so what it reads stays as it is until it ends; a run that is then refused takes away the data
folder it made for the lock, the lock file with it, so it still leaves nothing. The project owner decided
this rule on 2026-11-25, after review found that a first install, with no data folder yet, read whom the
gateway serves outside the lock, so that a second agent's install could finish meanwhile and the first
then put its own agent's home over the second's.

**Why.** Two agents of one gateway harness replace the same support files, so a second agent's install took over
the first's killed upgrade and ended it with only its own change. The first agent's gateway was left changed by a
run that never ended, and its record of the change stayed, to put back copies older than what that gateway had
by then over a later install, settings included. Putting the install's own change back before the others let a
later change's copies, taken of what the earlier one left, go back over the older ones: our plugin beside the
older core. Found by review of the model of the installer (issue #180); the install tests and the model's
two-agent configuration now check both.

Decided 2026-11-25 by the builder, under the project owner's instruction to decide minor points.

## D-155 The Hermes plugin keeps each request to confirm a command as Hermes keeps it

**Decision.** The Hermes plugin keeps each request to confirm a command, the `question` item of
[D-048](#d-048-a-request-to-confirm-a-command-is-a-question), by Hermes's own session key and the
confirmation's id. Hermes holds one confirmation in a session, and a new one replaces it (`tools.slash_confirm`).
The question closes only on evidence of what became of it:

- answered, by Hermes taking an answer given on it
  ([D-146](#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id)), or by Hermes's reply to
  a typed answer, matched by the message it replies to. As that reply comes, the plugin asks Hermes
  (`get_pending`) whether it still holds that confirmation: when it does, or cannot be asked, Hermes refused
  the sender or an approval took the word, and the question still waits;
- replaced, by a newer one in the same session, and it shows as cancelled (provisional);
- expired, by Hermes's own limit passing, 300 seconds by `tools.slash_confirm`, after which Hermes never runs
  it: the plugin counts from when Hermes offers it, and a second more, and it shows as cancelled.

That Hermes no longer holds a confirmation says nothing of why: approved and still running, expired, replaced
or answered. On its own it closes nothing and settles nothing. The end of a turn, its sender's or another's,
interrupted or not, closes none.

While a typed answer to a confirmation still waits for Hermes's reply, Hermes may have taken it and be running
the command, so nothing else closes that question: not a later answer's reply, not a newer confirmation in its
session; only that answer's own reply, as above, or Hermes's limit. A newer confirmation still replaces an older
one that no answer waits on. The project owner decided this on 2026-11-25.

A typed answer answers only the confirmation in its sender's own session, as Hermes reads it, in any of the
words Hermes reads as one in English (`/approve`, `/cancel`, `yes`, `no` and the rest).

Every reply of Hermes's goes by the message it replies to: to that message's turn while it runs, or else to its
sender's session outside any turn, never to the chat's latest turn. Only a turn that a slash command started
makes its replies that command's output.

The command a confirmation is for settles as other effects do
([D-087](#d-087-the-core-settles-a-request-whose-effect-never-shows)), not from Hermes's reply to the answer.
When it will not run, because the answer was Cancel, or the confirmation was replaced or expired, the event that
closes the question settles it. When it runs, its own effect settles it, as for a command Hermes did not ask
about: the end of the old session for a new session, the model read for a change of model. Hermes's words then
settle it only when they say it was refused, as for any typed command; when Hermes gives no reply, the notice
that says so settles nothing, and the core settles the command as having no effect after two minutes, unless
its effect shows first (provisional; before, that notice settled it at once).

The cost accepted: a question can show waiting until Hermes's limit passes after Hermes has dropped it without a
word, as it does at a session boundary, when a connector declines its send, or for one too old when the
session's next message comes.

Nothing changes in who may answer: the core's rights and Hermes's own check, as D-146 and
[D-151](#d-151-the-hermes-plugin-asks-hermess-own-check-of-who-may-answer-an-approval-card-or-a-question) say.

**Why.** The plugin kept its own record beside Hermes's: one confirmation for the whole chat, closed by any
turn's end, and a typed answer read as answering the latest one. With two people confirming at once in one
group, a typed `/cancel` from one closed the other's question, a confirmation Hermes replaced stayed open, and
another sender's interrupted turn closed one that still waited in Hermes, which then could not be answered from
the chat. Hermes's session and id name the confirmation as Hermes does. A first fix also read a confirmation's
absence from `get_pending` as its outcome, and two rounds of review found the same fault each time: an expired
confirmation shown as approved, and an approved `/new` shown as cancelled while Hermes ran the reset, because
Hermes takes a confirmation before it runs the command and a message sent meanwhile found none waiting. Replies
that fell back to the chat's latest turn put one person's answer and its settlement on another's turn, and made
an ordinary reply the output of someone else's command (#201). A reply says less than an effect: Hermes says
nothing both when it ran nothing and when the command gave no reply. This closes D-146's limit for the pilot,
that two people confirming at once in one group may settle each other's commands or leave one unanswerable
from the chat; D-146's remaining limit stands, except that a command Hermes dropped after a stall now ends as
having no effect rather than as settled. Issues #183 and #201.

Not closed: Hermes reads a typed answer in the language it is set to as well as in English, and a word only
that language has leaves the question waiting until Hermes's limit passes, and then cancelled, not answered. A
typed answer Hermes takes in the last moments before its limit, whose reply comes after the plugin's count
ends, shows the question cancelled and settles its command as not run, though Hermes ran it; its effect still
shows. A typed answer word that Hermes read as an ordinary message, because it had dropped the confirmation
without a word just before, shows the question answered by Hermes's reply (inferred from Hermes's source, not
seen). A typed `/approve` while an approval waits in the same session goes to the approval, as D-146 says.

The question closing only on evidence, every reply going by the message it replies to, and the cost they
accept were decided 2026-11-25 by the project owner. The rest was decided 2026-11-25 by the builder, under the
project owner's instruction to decide minor points.

## D-156 The no_effect net counts when output reached the core, not when the core read it

**Decision.** The core settles a control as `no_effect`
([D-087](#d-087-the-core-settles-a-request-whose-effect-never-shows)) only once it has read every output of the
driver that reached it by the control's deadline. A driver says when the output it hands over reached it, through a
method of the driver trait with a default ([D-058](#d-058-the-driver-trait-and-the-capability-flags)): a built-in
driver that reads its harness while a request waits, and holds what it read, says when it read each output; a driver
that does not say is taken to have read it when it hands it over. For a connector the core notes the time as it
reads each line from the socket, so the socket has no field for it and no connector changes. Output that reaches the
core after a deadline does not hold the net, so a driver that is never idle cannot keep a control pending. The core
reads that output one at a time in its loop, as it always did, with nothing read ahead, so a stop loses nothing the
driver handed over. A driver hands its outputs over in the order they reached it. An effect the core reads before
the net settles the control settles it, even after the deadline, as before this decision.

Not closed: a Codex message held until its thread is named is noted when the driver releases it, not when it read it,
so an effect inside one released after the deadline does not count as in time. And a driver that reads without end
lines that give no output, as a connector that floods its socket, holds the net, though not the rest of the core:
the core cannot tell it has read everything that reached the driver.

**Why.** The nightly simulator found a control settled `no_effect` though its effect reached the core in time: the
core was blocked on a slow request of the driver, and when it went on, the clock's tick came before the event
(issue #198). Two fixes that guessed where to stop reading, waiting for the driver to be idle and reading at most a
set number of outputs, each failed review: the first let a busy driver hold the net off for good, the second still
missed an effect behind a deep queue and could drop file requests on a stop. When each output reached the core is
the question D-087 asks, and the set of outputs that reached it by a deadline is finite.

Decided 2026-11-25 by the project owner.

## D-157 The core goes on with the event MDK names for a send it holds, and never hands such a send to MDK again

**Decision.** MDK's `local_send_status` names the event of a send it holds, whether it keeps it queued or its
engine has it (MDK pull request 3712, in MDK `master` at `bea7b40`, the revision the core now uses). For a send
handed to MDK in a form its chat is no longer sent
([D-152](#d-152-a-send-mdk-was-handed-in-a-form-its-chat-is-no-longer-sent-is-asked-after-never-handed-again)), the
core uses it so:

- **Its engine has it.** The send goes on to the core's record with that event, as MDK was handed it, as one MDK's
  engine has sent does. So does one whose first answer the core lost to a crash.
- **Queued.** The core keeps the event with the send in its state file, if it had not, and asks again later; the
  chat's later sends wait for it, as before. Should MDK let go of the token later, the core knows MDK took the send,
  and the chat is told it may not have been delivered
  ([D-109](#d-109-how-the-cores-sends-survive-a-crash-as-built)), even if the group's messages were made to last
  longer since.
- **Done with.** MDK names the event in the status of every send under a token that its engine is done with, sent
  or kept to post later, as before. A status that named none would be no answer, and the core would ask again.
- **Refused for a member it does not list.** MDK can now refuse a send that would reach a member account its list
  leaves out. The core does not give such a list yet; such a refusal is taken as MDK's drop of a try.

So the core hands no such send to MDK again, on any path: the one path left of D-152 (`Core::hand_again`,
`Side::lasts`) is gone. So is the stall D-152 accepts: a send MDK's engine keeps for good no longer holds up its
chat, whatever the group's disappearing time.

**Why.** D-152 kept one second hand-over, and accepted a stall, only because MDK's status named no event while its
engine held a send. The project owner asked MDK to name it, and decided on 2026-11-25 that both would go once MDK's
pin moved to a release that did. Without the second hand-over, the narrower path of
[D-148](#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit),
a send handed again after MDK let go of its token, is closed too.

That the one path left and the stall go was decided 2026-11-25 by the project owner, in D-152. Taking a refusal for
a member as a drop was decided 2026-11-25 by the builder, under the project owner's instruction to decide minor
points.

## D-158 The core does not limit a send to the chat's members: a send in flight when the owner adds someone may reach them

**Decision.** The core keeps the rule of
[D-136](#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group):
it reads who is in a chat before each send, and holds back the agent's full activity from a chat that is not the
owner's two-person chat. It does not use MDK's sends limited to named members (`submit_custom_event_to_members`,
MDK pull request 3712). A send the core handed to MDK while the chat was the owner's, which MDK posts after someone
was added, can reach the person added. That is fine, not a limit: only the owner can add someone to their
two-person chat with the agent, and the person they add is meant to read the chat. D-148's limit is accepted for
good, not for the pilot only, and nothing more is asked of MDK for it.

**Why.** Limiting those sends took more bookkeeping than the gap is worth: MDK can refuse such a send after its status
already says its engine has it, so the core could not tell when a send was done, and review found one edge after
another. The gap is a few seconds wide, and opens only when the owner adds someone on purpose.

Decided 2026-11-26 by the project owner. The change that limited sends (branch `member-limited-sends`) was built,
reviewed and not merged.

## D-159 An upgrade keeps a copy of the agent's account store, and puts it back with the old core

**Decision.** MDK upgrades an account home's store forward only, the first time a newer core opens it, and an
older core cannot open it after (MDK's migration 0226 at `bea7b40`, as 0220 and 0221 before it). The new core
first opens the store as an upgrade starts the agent, before the upgrade is done
([D-134](#d-134-an-upgrade-of-the-core-and-its-support-files-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)),
so an upgrade put back after that left the old core beside a store it could not open (issue #210). Now:

- **The copy.** Before an install puts in place a core other than the one there before, for an agent that has an
  account home, it stops the agent's services. It waits up to a minute for MDK's own lock on the home
  (`.marmot-runtime.lock`, which a core holds for as long as it has the home open) and, holding it, copies the home:
  all of it, MDK's databases with their `-wal` and `-shm` files, its keys, its records and the core's own state, as
  they were at one moment. It leaves out MDK's lock and the folder of the socket, which the core makes again as it
  starts. A core that still has the home open after that minute stops the install, and so does a service that is not
  stopped once it was asked to stop, as one waiting to start again on its own: the installer reads each unit's
  state, not whether the stop command failed. Putting a copy back checks the same, and keeps everything as it is if
  a service did not stop. The core put in place is checked against the checksum the copy is kept for. The services stay stopped until
  the install starts them on the new core, as before, so no core of the agent's opens the store in between. A new
  agent, a run with the same core as before, and one with `--no-start` or `--no-service` copy nothing.
- **No `init` on a home in use.** The installer runs `marmot-agent init` only on an account home that is missing
  or empty. A home that holds anything but MDK's lock and the folder of the socket, with the agent's
  `public-key` file lost, stops the install before any core opens it: the error says to write the agent's public
  key into that file, or to move the home away to give the agent a new identity. So no new core opens the store
  uncopied, as `init` would in a run that copies nothing.
- **Where it lives.** In the upgrade's record only,
  `~/.local/share/marmot-agent/upgrade/stores/<name>/home`, beside a file that names the home and the agent's
  services and one that holds the checksum of the core it was kept for. It is made under another name and renamed once whole. Each file keeps its owner and mode, and every
  folder made for it is the owner's alone (mode 700). Nothing of it is printed or logged: the installer names
  only its path.
- **Put back.** Whatever puts the upgrade back puts the copy back with the core: the run's own exit trap, the next
  run after a kill, or another harness's or another agent's install that cannot finish it
  ([D-154](#d-154-an-install-takes-over-a-killed-upgrade-only-for-the-same-agent-and-the-same-gateway)). It stops
  the agent's services and, under MDK's lock, replaces what the home holds with the copy, in place, so that the home
  and its lock never move. A put back under way is marked in the record, and whatever reads the record next
  finishes it before anything else: a run that takes the upgrade over, `--no-start` too, or one that puts it back.
  The agent's services are left stopped, on the old core: the installer prints the command that starts them,
  `systemctl --user start` with the agent's units, or `launchctl bootstrap` with its agents on macOS, and starts
  nothing itself. A copy that could not go back, as when something still holds MDK's lock on the home, leaves
  the rest as it is too: the new core and support files stay in place, so the new core is never left beside an
  old store or the old core beside a new one, and the record is kept. The installer says to run it again once
  nothing holds the agent's home.
- **Ended.** The copy goes with the record, in the one step that ends the upgrade. A run that takes over a killed
  upgrade keeps the copy that run took, which is of the store before any new core opened it, but only when it
  installs the same core the copy was kept for. A run with any other core, an older release too, puts that
  upgrade back first, the store with it: the killed run's core may have moved the store forward already, and
  taking it over would end the upgrade with the copy gone and another core on that store. Another agent's
  services that the killed run stopped are left stopped, and the run that ends the upgrade prints how to start
  them. No install starts an agent because of an earlier one that failed or was killed.

Three limits are accepted. Only the agent being installed has its home copied: another agent on the account whose
core restarts while the new core is in place, before the upgrade ends, opens its store with the new core, and an
upgrade then put back leaves it as #210 did, until the new release is installed again. An upgrade with
`--no-start` copies no home either, so should the agent's service manager start its core again while the new one
is in place (its units restart it whenever its harness exits), the store moves forward uncopied in the same way;
the builder accepted this on 2026-11-26, under the project owner's instruction to decide minor points. What the new core did
between its start and the put back is lost with it: the agent is back where it was as the copy was taken. That
is mostly the seconds until a later step fails, but an older store can reuse keys or run a prompt twice, as going
back from a move can ([`docs/move.md`](move.md#going-back)). It is the price of an agent that starts again at all.
Once an upgrade that started the agent has ended, there is no way back to an older release: the old core cannot
open the store, and the installer keeps no copy of it.

**Why.** Of the three ways forward issue #210 named, putting the store back with the core keeps D-134's promise
that a failed upgrade leaves the agent as it found it, and costs a copy of one agent's home on each upgrade. The
lock is MDK's own, taken as MDK takes it, so the copy is never of a store a core is writing, and no core opens the
home while it is copied or put back. Taking the copy before the new core is in place, not as the agent starts,
leaves no moment in which the agent's core can run the new release on an uncopied store. Review of the first
version found that moment, and that swapping the home for a copy moved MDK's lock out from under a core waiting
for it.

Leaving the services stopped after a put back, as the put back of the core alone always did, keeps the installer
from starting an agent the owner had stopped, or one a later install had no part in. Starting them again took a
list of services to start that outlived the run, and review found it starting agents despite `--no-start` and
dropping a start that failed.

The project owner decided on 2026-11-26 that the store is put back with the core, the first of the three, and,
the same day, that a put back leaves the agent's services stopped and prints how to start them. How it is copied,
where it is kept, when the services stop, and the `init` guard were decided 2026-11-26 by the builder, under the
project owner's instruction to decide minor points.

The `init` guard ("No `init` on a home in use") is replaced by
[D-173](#d-173-marmot-agent-init-refuses-a-home-with-an-identity-before-it-opens-its-store-and-no-core-starts-on-a-home-being-put-back): the core's
`init` refuses a home that holds an identity before it opens the store, and the installer no longer refuses a home
for holding other files. D-173 also says what a put back under way, or one that fails part way, keeps from
starting. The rest stands.

## D-160 The coding installers take an install moved over, and run it as a service

**Decision.** An install of today's connector for Codex, Claude Code, OpenCode, Pi or Goose moves over the way a
Hermes or OpenClaw install does ([`docs/move.md`](move.md#steps)): first `marmot-agent import <old home> --settings
<the agent's settings>`, which writes the settings where the installer looks for them,
`~/.local/share/marmot-agent/agents/<name>/settings.toml`, and then the harness's installer, such as
`install/codex.sh`. After that, `marmot-agent` runs as the agent's service, as it does after a fresh install, and
for Claude Code so does its connector, beside it.

- **No flag.** The installer knows settings the import wrote by their first line, which names the connector they
  came from (`# Imported by marmot-agent import from a Codex install, …`). It takes those of its own harness, and
  refuses, before it changes anything, settings imported from another harness's connector. Every installer does,
  Hermes's and OpenClaw's too. The gateways keep `--replace-plugin`, which changes the gateway's plugin; a coding
  harness has no plugin to change.
- **The import's defaults stay.** Without `--settings` the import still writes `marmot-agent.toml` in the old home,
  for every harness. The installer's folder depends on the `--name` it is given, which the import does not know,
  and the gateways already work this way. Settings written there can be moved into the agent's folder before the
  installer runs.
- **What it uses, and adds.** It uses the settings as the import wrote them and the identity in the old home. It
  never makes an identity or runs `marmot-agent init` there. It adds nothing to the settings: the owner, the
  rights, the allow-lists, the relays, `files_from` and the work folder stay as written. Until the move is done, a
  `--cwd` that differs from the settings' `cwd` stops it, before anything changes, and says to change the folder
  in the settings; after that, `--cwd` replaces it, as on any run again. It checks the harness's program the
  settings name as a whole path, as the import carried it over from the old service, not the one on `PATH`. In the agent's folder it writes what it writes for
  every agent: the record of its harness, its choices, the service's environment and launchers. It also writes the
  service's units. The Claude Code connector takes its work folder on its command line, and the import's settings
  for Claude Code do not name one, so `claude-code.sh` needs `--cwd`.
- **The work folder is checked.** Unless the import was given `--cwd`, its work folder is the person's home
  folder. The installer checks the folder the settings name as it checks `--cwd`. It refuses, before it changes
  anything, a folder that holds or lies inside its data folder, which holds the agents' keys. For an agent moved
  over it also refuses one that holds or lies inside the old home, or the import's backup of it, which hold that
  agent's keys outside the data folder: the harness reads anything in its work folder.
- **The first start is copied.** [D-159](#d-159-an-upgrade-keeps-a-copy-of-the-agents-account-store-and-puts-it-back-with-the-old-core)
  copies an agent's home only when the install puts in a core other than the one there before. The first core
  of ours to open a home moved over moves its store forward from the old connector's, whichever core it is. So an
  agent moved over, whose home no install has started yet, is copied whatever core was installed before. A move
  made after another agent's install has put the same core in place is copied too, and so is a move of Hermes or
  OpenClaw. An install whose upgrade ended after it started such an agent notes that in the agent's folder
  (`moved`), and from then on D-159's rule holds. Put back, the agent is left stopped, with its home as the import
  left it, and its new services are taken out of what starts at login (disabled on Linux; on macOS its
  LaunchAgents are removed, which a run again writes back). The installer then says to run it again once what
  failed is fixed, or to go back to today's connector. It does not print the command that starts the service,
  which would open the home uncopied.
- **Accepted limit.** The move counts as done once the service manager has started the core (`Type=exec` on
  Linux), not once the core says it is ready. A core that then fails, as when the harness cannot start, has
  moved the store forward; the new core stays installed and opens it, so the agent runs once the harness is
  fixed, and going back to today's connector takes the import's backup, as after any move.

**Why.** Run by hand, an agent stops when the person logs out or the machine restarts. Each piece takes the smallest
step that is safe. The settings already say which connector they came from, so a flag would add nothing. A
`--replace-plugin` for the coding installers would be a flag that replaces nothing. Without D-159's copy on the first
start, a failure just after it would leave the old home moved forward and the old core unable to open it. Recovery
would then need the import's backup, which is for going back, not for an install that failed.

The project owner decided the outcome on 2026-11-26: the five coding harnesses end a move with `marmot-agent` running
as a service, as Hermes and OpenClaw do. The details above were decided 2026-11-26 by the builder, under the project
owner's instruction to decide minor points: no flag, the import's defaults kept, the work-folder check, the copy of
the first start and its note, and what a put back says.

The accepted limit ("Accepted limit.") is closed by
[D-178](#d-178-an-install-counts-a-start-as-done-once-the-core-and-claude-codes-connector-say-they-are-ready): a
move, and an upgrade, count as done only once the core, and Claude Code's connector, say they are ready. The rest
stands.

## D-161 CI no longer runs on pushes to `master`

**Decision.** The `push` to `master` trigger is removed from `.github/workflows/ci.yml`, amending
[D-130](#d-130-ci-runs-in-three-tiers-with-one-required-check) and
[D-144](#d-144-property-tests-run-on-every-pull-request-kani-in-the-full-gate-and-fuzzing-nightly-on-a-pinned-nightly-compiler).
The full gate (including the simulator at 200 runs, the Docker move-over tests, and the Kani proofs) continues to run
on manual `workflow_dispatch` runs with `tier=full` (and in `merge_group` whenever a merge queue is active). Rust
build caches are saved only by `workflow_dispatch` runs (until hourly `canary` takes over cache saving in Phase 1
of #222); if `master`'s caches are stale or evicted, run `ci.yml` by hand on `master` with `tier=full`.

**Why.** Every merge to `master` was running the full 21-job CI workflow a third time (`pull_request` ~47 min,
`merge_group` ~53 min, `push: master` ~51 min) on the exact tree just verified, burning ~51 runner-minutes per merge
even when every cache already hit. Dropping the `push: master` trigger saves ~51 runner-minutes per merge immediately
as Phase 0 of #222.

Decided 2026-11-26 by the project owner (as Phase 0 of #222).

## D-162 Pull requests record verification, review, and release-impact trailers on the tip commit

**Decision.** Every pull request targeting `master` records three Git trailers in the final trailer block of its tip
commit (preserved on `master` under GitHub's `COMMIT_MESSAGES` squash setting), matching `marmot-protocol/mdk`:

- `Local-Checks: <commands run locally>`
- `Local-Review: <seat-a> + <seat-b> merge @ <short-sha>` (or `human`, or `exempt (<reason>)`)
- `Release-Impact: none | patch | minor` (audited by both local review seats)
- Optional `Changelog: <one-line summary>` for user-facing changes.

`tools/ci/check_pr_trailers.py` validates the tip commit's trailers and runs in advisory mode in
`.github/workflows/ci.yml` during Phases 0–2 of #222 (emitting warnings without failing CI), with hard enforcement
starting in Phase 3 when the fast Team PR gate replaces full per-PR CI. Repo-local `.agents/skills/land-change` and
`.agents/skills/review-local` (adapted from `agent-tools` to this repository's `D-068` rules) carry the workflow for
coding agents.

**Why.** Moving to a fast Team PR gate and automated per-track SemVer releases (#222) requires durable, machine-readable
verification, cross-model review, and release-impact metadata in `master`'s git history rather than only in ephemeral PR
comments, while keeping the trailer schema identical across `marmot-protocol/mdk` and `marmot-protocol/marmot-agents`.

Decided 2026-11-26 by the project owner (as Phase 0.3 of #222).

## D-163 Hourly canary runs the full gate on `master` and refreshes build caches

**Decision.** Amending [D-130](#d-130-ci-runs-in-three-tiers-with-one-required-check),
[D-144](#d-144-property-tests-run-on-every-pull-request-kani-in-the-full-gate-and-fuzzing-nightly-on-a-pinned-nightly-compiler),
and [D-161](#d-161-ci-no-longer-runs-on-pushes-to-master):

- The GitHub merge queue is removed from `master`'s ruleset (`#27632519`) and `allow_auto_merge` is enabled; pull
  requests squash-merge directly once `CI passed` is green.
- A protected fast-forward branch `canary` (ruleset `#28171730`: no deletion, no force-push, required signatures)
  tracks the latest commit on `master` verified by the full CI gate.
- `.github/workflows/canary.yml` runs hourly (`schedule`) and on `workflow_dispatch` from `refs/heads/master`,
  serialized by `concurrency: { group: canary, cancel-in-progress: false }`. It skips immediately when `github.sha`
  is already at or behind `canary`, or when the same SHA has already failed canary twice (unless `force=true`,
  preventing unbounded hourly retries on a deterministic regression while keeping one automatic retry for transient
  flakes). Otherwise it invokes `.github/workflows/ci.yml` via `workflow_call` with `tier: full` and
  `save_caches: true` (running all TLA+ models, simulator at 200 runs, Docker move-over tests, and Kani proofs, and
  saving `master`'s Rust build caches once per batch), fast-forwards `canary` to the exact verified SHA via the Git
  Refs API (`force: false`) on green, or opens/updates a `Canary: full gate failed` issue on failure.
- `tools/ci/passed.py` accepts `--prefix` so `CI passed` verifies every expected job of `ci.yml` when invoked through
  `workflow_call` (where GitHub prefixes called job names with the caller job name).

**Why.** Running the full 21-job gate on every PR in a merge queue and again on every `push: master` burned ~150+
runner-minutes per merged PR and serialized merges behind slow queue runs. Batching full-gate verification and
cache warming into an hourly `canary` run keeps `master`'s caches fresh and guarantees every heavy check (Docker
move-over, Kani, simulator-200) runs against `master` while cutting runner usage dramatically (Phase 1 of #222).

Decided 2026-11-26 by the project owner (as Phase 1 of #222).

## D-164 Nightly verification runs against the verified `canary` branch

**Decision.** Amending [D-130](#d-130-ci-runs-in-three-tiers-with-one-required-check) and
[D-143](#d-143-one-issue-tracks-the-ci-jobs-that-flake), building on
[D-163](#d-163-hourly-canary-runs-the-full-gate-on-master-and-refreshes-build-caches):

- `.github/workflows/nightly.yml` resolves `refs/heads/canary` once in a first `resolve` job on scheduled runs,
  persists that resolved SHA in a run-scoped cache (`nightly-sha-<run_id>`) so "Re-run all jobs" always tests the
  exact same commit even if `canary` advances between attempts (preserving D-143's same-commit flake guarantee),
  checks out that pinned commit across all nightly jobs (`simulator`, `properties`, `fuzzing`, `models`,
  `coverage`, `mutants`, `cassettes`), and reports that SHA in failure issues. Scheduled runs execute `master`'s
  `nightly.yml` against `canary`'s checkout (so changes to `nightly.yml` that depend on new repository code take
  effect once `canary` advances). Manual `workflow_dispatch` runs always test their dispatched ref (`github.sha`).
- `.github/workflows/flake-report.yml` and `tools/ci/flake_report.py` also watch `canary.yml` (expanding its local
  `workflow_call` to `ci.yml` with the `CI / ` job prefix and treating `CI / CI passed` as a gate), so flakes in
  the hourly full-gate tier continue to surface in the `Flaky job tracker`.

**Why.** The nightly soak burns ~173 runner-minutes per night (5,000 simulator seeds, 10,000-case property tests,
fuzzing, larger TLA+ bounds, coverage, mutation testing). Pinning scheduled runs to `canary` ensures nightly never
wastes a full soak on an unverified or broken `master` commit and never drifts mid-run if `canary` advances, while
tracking `canary.yml` in the flaky job tracker keeps flake visibility across all three tiers (Phase 1.3 of #222).

Decided 2026-11-26 by the project owner (as Phase 1.3 of #222).

## D-165 Four independent release tracks decoupled by the socket protocol version

**Decision.** Amending [D-091](#d-091-one-install-script-per-harness-and-release-binaries-from-ci) and building on
[D-020](#d-020-the-socket-speaks-agent-control-v3-with-a-clean-cutover) and
[D-115](#d-115-the-sockets-schema-is-made-from-the-cores-own-types-and-checks-this-repositorys-connectors), the
repository versions four independent release tracks decoupled by the socket protocol version
(`marmot.agent-control.v3`), laying the versioning and compatibility contract for per-track releases in Phase 4 of
#222:

- **`marmot-agent`** (planned tag `vX.Y.Z`, source of truth `Cargo.toml` `[workspace.package].version`): the Rust
  core, built-in drivers (`codex`, `pi`, `acp`), container files (`tools/pi`, `tools/goose`), and installers
  (`install/`).
- **`claude-code`** (planned tag `claude-code-vX.Y.Z`, source of truth `packages/claude-code/package.json`): the
  TypeScript Claude Code driver, declaring `marmot.socketProtocol`.
- **`openclaw`** (planned tag `openclaw-vX.Y.Z`, source of truth `packages/openclaw/package.json`): the TypeScript
  OpenClaw channel plugin, declaring `marmot.socketProtocol` and reading its runtime version from `package.json`.
- **`hermes`** (planned tag `hermes-vX.Y.Z`, source of truth `python/pyproject.toml` `[project].version`, kept in
  sync with `python/marmot_hermes/plugin.yaml` and `python/marmot_hermes/__init__.py::__version__`): the Python
  Hermes channel plugin, declaring `[tool.marmot].socket_protocol`.

Every track starts at a real SemVer version (`0.1.0`, never `0.0.0`). `tools/ci/check_socket_compat.py` runs in
CI's `Python` job: on every run it verifies that all four tracks have valid non-zero SemVer versions, matching
lockfiles/manifests, and matching socket protocol metadata; on pull requests (`--base HEAD^1`) it also runs a
direction-aware structural check on `fixtures/socket/schema.json` so changes on the same protocol version cannot
break either older connectors sending `FromConnector` messages or older connectors reading `FromCore` messages
(breaking changes require bumping `marmot.agent-control.vN+1` across the core and every connector). Per-track
packaging and release tags follow in later Phase 4 PRs.

**Why.** Bundling TypeScript and Python connector fixes behind a 3-platform Rust release build couples unrelated
ship cadences and turns a 30-second plugin fix into an hour-long cross-platform binary release. Real per-track
SemVer manifests and a mechanical schema compatibility guard let each track version and ship independently on top
of a stable socket protocol boundary (Phase 4.1 of #222).

Decided 2026-11-26 by the project owner (as Phase 4.1 of #222).

## D-166 Per-track releases, compatibility manifests, and protected release tags

**Decision.** Building on [D-091](#d-091-one-install-script-per-harness-and-release-binaries-from-ci) and
[D-165](#d-165-four-independent-release-tracks-decoupled-by-the-socket-protocol-version):

- `.github/workflows/release.yml` ships all four tracks independently:
  - `vX.Y.Z*` (`marmot-agent`) runs the 3-platform cold Rust core build (`tools/release/package.sh`).
  - `claude-code-vX.Y.Z*`, `openclaw-vX.Y.Z*`, and `hermes-vX.Y.Z*` run only that connector's verification suite
    and `tools/release/package_connector.sh`, publishing in ~30–60 seconds with no Rust build.
  - Manual `workflow_dispatch` on `master` plans from the verified `canary` branch via
    `tools/release/plan_release.py` (`dry_run: true` by default) and can cut `-rc.N` prereleases directly from
    `canary` once the track's manifest version matches the planned base version (`tools/release/resolve_release.py`).
- Every release publishes `SHA256SUMS` and a `latest.json` compatibility manifest (`tools/release/write_manifest.py`)
  recording `schema_version`, `track`, `version`, `tag`, `prerelease`, `commit`, `socket_protocol`, and each asset's
  SHA-256 digest and byte size. GitHub's repository-wide "Latest" release always tracks the latest stable
  `marmot-agent` core release (`--latest=false` on connector tracks) so installers fetching `/releases/latest` keep
  working. Only the final `publish` job holds `contents: write`, and it runs no repository code.
- Repository tag ruleset `#28179915` (`Release tags`) protects `v*`, `claude-code-v*`, `openclaw-v*`, and
  `hermes-v*` (excluding `*-rc.*`) against deletion, updates, and unsigned tags.

**Why.** Connector fixes ship in under a minute without rebuilding Rust across three platforms, every release
advertises its exact socket protocol and checksums for split-artifact installers (Phase 4.4), and stable release
tags stay immutable and cryptographically signed (Phase 4.3 of #222).

Decided 2026-11-26 by the project owner (as Phase 4.3 of #222).

## D-167 Socket and plugin installers fetch compatible standalone connector releases atomically

**Decision.** Building on [D-134](#d-134-an-upgrade-of-the-core-and-its-support-files-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill),
[D-165](#d-165-four-independent-release-tracks-decoupled-by-the-socket-protocol-version), and
[D-166](#d-166-per-track-releases-compatibility-manifests-and-protected-release-tags):

- `install/claude-code.sh`, `install/openclaw.sh`, and `install/hermes.sh` declare `CONNECTOR_TRACK` and accept
  `--connector-version <track>-v<version>` (refused on built-in harnesses).
- When downloading a core release, socket/plugin installers either fetch the explicit `--connector-version` tag or
  query `MARMOT_AGENT_RELEASES_API` (following same-origin `Link: rel="next"` pagination, 100 releases per page) for
  stable `<track>-vX.Y.Z` releases strictly newer than the connector bundled in the core archive (sorted highest
  SemVer first) and select the newest one whose `latest.json` `socket_protocol` matches the core's `latest.json`.
  They verify its `SHA256SUMS` and archive paths, and overlay its files into `$MA_STAGE` before `harness_prepare`
  and `publish_support` (falling back to the connector bundled in the core tarball if no newer compatible standalone
  release is available or reachable).
- Protocol verification fails closed: when the selected core's `socket_protocol` cannot be determined (for example,
  a core release missing `latest.json` or `--binary` with no adjacent `latest.json`), auto-discovery preserves the
  bundled connector and explicit `--connector-version` is refused before modifying `$MA_LIB` or `$MA_CORE`. When
  the core declares a `socket_protocol`, an explicit `--connector-version` whose `latest.json` disagrees or declares
  no `socket_protocol` fails before modifying `$MA_LIB` or `$MA_CORE`; auto-discovery skips incompatible, prerelease,
  or undeclared releases and picks the newest compatible release.

**Why.** Independent connector releases (`claude-code-v*`, `openclaw-v*`, `hermes-v*`) reach users directly
through the existing installers without losing single-tarball offline fallback, checksum verification, protocol
safety, or atomic upgrade/rollback semantics (Phase 4.4 of #222).

Decided 2026-11-26 by the project owner (as Phase 4.4 of #222).

## D-168 Always-on revert-only bisect agent watches hourly canary and activates Meltdown Mode

**Decision.** Building on [D-143](#d-143-one-issue-tracks-the-ci-jobs-that-flake),
[D-162](#d-162-pull-requests-record-verification-review-and-release-impact-trailers-on-the-tip-commit), and
[D-163](#d-163-hourly-canary-runs-the-full-gate-on-master-and-refreshes-build-caches), an isolated systemd
timer on the CI host (`marmot-agents-bot`, separate from MDK's `mdk-bot`) runs `tools/ci/bisect_agent.py`
to watch completed `.github/workflows/canary.yml` runs on `master`:

- Failures are classified into four buckets (stopping early on the first confirmed deterministic regression):
  1. `infra` (runner/network/registry/cache outages, checked only after real test/compiler failure markers) —
     retries `canary.yml` once; escalates to Meltdown on a second consecutive failure on the same SHA.
  2. `known_flake` (jobs recorded in the open `Flaky job tracker` issue, read via `flake_report.read_state`) —
     verified locally on `tip` when a local command exists (promoting to `deterministic_regression` if it
     reproduces deterministically on `tip` and passes on `canary`), otherwise retries `canary.yml` once without
     reverting and escalates to Meltdown on a second consecutive failure on the same SHA.
  3. `suspected_race` (non-infra, unlisted job that passes or flakes locally on `tip`) — retries `canary.yml`
     once and escalates to Meltdown on a second consecutive failure on the same SHA.
  4. `deterministic_regression` (fails locally on `tip` and passes on `canary`) — confirms current `master`
     still fails, bisects `canary..tip` by exporting each candidate SHA via `git archive` and running checks
     inside a `bwrap` (bubblewrap) read-only-root + PID/IPC/mount-namespace sandbox (masking bot credentials,
     host container sockets such as `/run/docker.sock`, `/run/user/$UID`, and the control checkout, using
     dedicated credential-free writable `CARGO_HOME`/`RUSTUP_HOME` caches and a fresh per-check `HOME`, and
     failing closed if unavailable), verifies a clean `git revert` on `master` passes that command, and opens
     a signed `revert` PR (`Local-Review: exempt (automated bisect revert)`) with squash auto-merge enabled.
     Docker-dependent jobs (`Move-over tests (Docker)`) are treated as locally unbisectable so the bot never
     exposes a host Docker socket.
- Whenever automated revert cannot safely recover `master` (revert conflict, reverted tree still fails,
  broken `canary` baseline, unbisectable repeated failure, or persistent flake/race/infra), the agent enters
  **Meltdown Mode**: pushes a signed commit to the protected `tree-state` branch setting `state.json` to
  `"red"` (retrying on subsequent ticks if the remote update fails), posts recovery instructions on the
  canary failure issue, and dispatches a structured White Noise / Marmot alert. Once a subsequent completed
  `canary.yml` run genuinely advances `canary` to cover `master` (ignoring skipped runs), the agent
  automatically resets `tree-state` back to `"green"`.

**Why.** Moving team pull requests onto a fast gate (Phase 3) requires an always-on automated safety net
that isolates deterministic regressions by revert-only bisection, avoids reverting on flakes or infra blips,
and freezes `master` loudly rather than silently piling commits onto a broken tree (Phase 2 of #222).

Decided 2026-11-26 by the project owner (as Phase 2 of #222).

## D-169 Team pull requests use a fast static gate backed by hourly canary and the bisect agent

**Decision.** Replaces [D-130](#d-130-ci-runs-in-three-tiers-with-one-required-check). `.github/workflows/ci.yml` keeps `CI passed` as the single required check on `master` (checked by `tools/ci/passed.py`), and routes each run into one of three tiers:

- **Fast Team PR gate (~45–60 seconds, zero Rust compilation):** applies **only** to same-repo pull requests (`head.repo.full_name == github.repository` and `head.repo.fork != true`) authored by a collaborator whose live repository permission is `write`, `maintain`, or `admin`, unless the PR carries the `ci:full` label. Runs:
  - Verified commit-signature check on every commit in the PR before any PR code is checked out;
  - `tree-state` check reading `origin/tree-state:state.json` (fails closed on every PR run started or re-run while `state != "green"`, i.e. Meltdown Mode under [D-168](#d-168-always-on-revert-only-bisect-agent-watches-hourly-canary-and-activates-meltdown-mode), unless labeled `tree-fix` or `revert`; a PR whose `CI passed` check had already turned green before Meltdown is not retroactively invalidated by this workflow alone);
  - Hard-enforced tip commit trailers (`tools/ci/check_pr_trailers.py`, [D-162](#d-162-pull-requests-record-verification-review-and-release-impact-trailers-on-the-tip-commit));
  - `Rust format` (`cargo fmt --all --check`, installing only `rustfmt` with zero compilation), `ShellCheck` (lint only), `Recordings`, `TypeScript (claude-code)`, `TypeScript (openclaw)`, `Python` (including socket compatibility and release planner tests), `Documents` (`lychee`), and `CI passed` (which also runs `tools/ci` unit tests).
- **External / fork PR gate (and `ci:full` Team PRs):** any pull request from a fork, a non-collaborator, a read-only member, or when the live permission lookup fails fails closed into the full PR suite (`Rust format and Clippy`, `Rust tests (core)`, `Rust tests (drivers and the rest)`, `Simulator`, `Conformance (socket)`, `Install scripts`, path-scoped `TLA+ models`, plus all fast static checks), with commit trailers kept advisory so outside contributors are not blocked by internal agent conventions. Merging an external/fork PR requires a repository collaborator with write access to review the head commit and merge it (GitHub already prevents non-write contributors from merging or arming auto-merge, and clears auto-merge if a non-write user pushes). Adding the `ci:full` label to any PR and re-running CI runs the complete `full` tier (including `Move-over tests (Docker)`, `Kani proofs`, `Simulator` at 200 runs, and all `TLA+ models`).
- **Hourly `canary` and Nightly:** unchanged from [D-163](#d-163-hourly-canary-runs-the-full-gate-on-master-and-refreshes-build-caches) and [D-164](#d-164-nightly-verification-runs-against-the-verified-canary-branch). Hourly `canary` runs the full gate on `master` via `workflow_call` (with `pull-requests: read` granted to the called workflow), refreshes Rust build caches once per batch, and fast-forwards `canary` on green; the always-on bisect agent ([D-168](#d-168-always-on-revert-only-bisect-agent-watches-hourly-canary-and-activates-meltdown-mode)) isolates deterministic regressions or activates Meltdown Mode.

**Why.** Team changes are already verified locally and cross-model reviewed before opening a PR ([D-068](#d-068-work-follows-the-plan-and-the-builder-decides-what-is-internal), [D-162](#d-162-pull-requests-record-verification-review-and-release-impact-trailers-on-the-tip-commit)). Removing redundant per-PR Rust compilation cuts team PR turnaround to ~60 seconds and eliminates ~85–90% of Actions compute while keeping external/fork PRs fully tested and maintainer-merged, and keeping `master` protected by hourly full-gate `canary` + revert-only bisection + Meltdown Mode (Phase 3 of #222).

Decided 2026-11-26 by the project owner (as Phase 3 of #222).

## D-170 Google Antigravity CLI is driven through ACP

**Decision.** Google Antigravity CLI (`agy acp`, tested on 1.5.18) is supported through the shared ACP driver built
into the core, with `install/antigravity.sh` connecting to the person's own signed-in `agy` on `PATH` (extending
[D-005](#d-005-which-driver-each-harness-gets) and
[D-102](#d-102-the-installers-connect-to-the-harness-the-person-already-has)). Antigravity-specific handling in the
ACP driver is limited to three wire differences confirmed in real sessions:

- Commands read `rawInput.CommandLine` when `rawInput.command` is absent.
- `ask_question` tool items are suppressed and `<name>_other` free-text fields in `elicitation/create` are merged
  into `<name>` for `antigravity-acp-server` only (rule 4 of the event model).
- Because `agy acp` 1.5.18 omits `mcpCapabilities` in `initialize` while still connecting to client-given streamable
  HTTP MCP servers per session, the driver offers the loopback `send_file` server to `Antigravity ACP Server` when
  `mcpCapabilities.http` is omitted (still honoring an explicit `false`) and requires callers to name their session
  in `_meta["antigravity.google/conversation_id"]` (extending
  [D-110](#d-110-an-acp-agent-gets-the-file-tool-from-a-loopback-mcp-server-on-by-default)).

For privacy in committed fixtures, `tools/record/scrub.py` trims Antigravity's `available_commands_update` to its
built-in slash commands and skills, neutralises `_meta.antigravity.tier`, and scrubs mapped paths split across
streamed ACP chunks (extending
[D-085](#d-085-cursor-recordings-keep-only-cursors-built-in-commands)).

**Why.** `agy acp` already speaks standard ACP v1 over stdio with `session/resume`, `session/load`,
`session/request_permission`, `elicitation/create`, `configOptions`, and per-session HTTP MCP servers. Reusing the
ACP driver keeps maintenance small while fixing the three real-session differences and keeping personal skills,
account tier, and local paths out of committed recordings (#235).

Decided 2026-11-26 (requested in #235).

## D-171 The flaky tracker names individual tests from machine-readable JUnit results

**Decision.** Extending [D-143](#d-143-one-issue-tracks-the-ci-jobs-that-flake), CI writes machine-readable JUnit
XML results for its Rust, TypeScript and Python test jobs and uploads them on failure (`if: failure()`) under
deterministic attempt-scoped artifact names (`junit-<job-slug>-attempt-<N>`):

- **Rust** (`ci.yml` and `nightly.yml` test jobs): `cargo-nextest` (pinned by version and SHA-256 checksum in
  `.github/actions/nextest`, configured in `.config/nextest.toml` under `[profile.ci]` with `fail-fast = false`,
  `store-success-output = false`, `store-failure-output = false`; `cargo test --doc` still runs beside it so
  doctests remain covered).
- **TypeScript** (`TypeScript (claude-code)` and `TypeScript (openclaw)`): Node 24's built-in
  `--test-reporter=junit` alongside `spec` to stdout.
- **Python** (`Recordings`, `Python`, `Install scripts`, `CI passed`): a standard-library runner
  (`tools/ci/unittest_junit.py`) writing only `<testcase>` identifiers (`classname`, `name`, `time`) and empty
  `<failure/>` / `<error/>` markers, with zero extra dependencies.

Uploading only `if: failure()` adds zero upload overhead to passing runs, including the ~60s Fast Team PR gate
([D-169](#d-169-team-pull-requests-use-a-fast-static-gate-backed-by-hourly-canary-and-the-bisect-agent)).

`tools/ci/flake_report.py` reads those artifacts for newly counted flaky jobs and adds a `## Flaky tests` table
above `## Flaky jobs` in the same "Flaky job tracker" issue (keeping the issue title and `state["jobs"]` layout
unchanged for `tools/ci/bisect_agent.py`, and upgrading persisted `version: 1` state to `version: 2` on read):

- **Privacy and log isolation:** it still reads no job logs and never reads or publishes test stdout, stderr,
  stack traces, or `<failure>` / `<error>` messages — only structured `<testcase>` `classname` and `name` values
  for cases with a `<failure>` or `<error>` child element.
- **Trust boundary:** JUnit artifacts are downloaded and parsed only for runs whose `head_repository` is this
  repository itself (never fork pull requests, because TypeScript tests use prose names) and whose job name is
  trusted on the default branch. Fork PR runs still count in `## Flaky jobs` as before.
- **Defense in depth and graceful fallback:** artifact archives are bounded by download size, XML entry count,
  and actual streaming decompressed byte count; XML containing `<!DOCTYPE` or `<!ENTITY` is rejected before
  parsing; test names with control characters or over 240 characters are ignored; attempts with more than 25
  distinct failing tests skip individual test attribution (whole-suite/infra outage); and missing, expired,
  oversized, unreadable, or malformed artifacts fall back to counting the job without test names.

**Why.** Follow-up requested in [#184](https://github.com/marmot-protocol/marmot-agents/issues/184) when D-143 was
decided: naming the exact failing test from machine-readable JUnit results tells builders which test to stress and
fix without ever pulling test stdout/stderr or untrusted fork-PR text into the public issue.

Decided 2026-11-27 (follow-up #184 to D-143).

## D-172 An uninstall does not put back the plugin a `--replace-plugin` move replaced

**Decision.** `--uninstall` of a Hermes or OpenClaw agent takes our plugin and its settings out of the gateway,
and gives up a move left unfinished ([D-116](#d-116-a-gateways-plugin-swap-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)),
but it does not put back the old connector's plugin and settings that a finished `--replace-plugin` move
replaced. Going back stays by hand, from the copies listed in `backups/before-move`, as [the move](move.md)
("Going back") says.

**Why.** Putting them back would change what an uninstall does to a person's gateway, and copying the old
settings file back would wipe any edits made since the move, unless it merged them. Raised in
[#89](https://github.com/marmot-protocol/marmot-agents/issues/89) and left out of its fix.

Decided 2026-11-27. The project owner decided.

## D-173 `marmot-agent init` refuses a home with an identity before it opens its store, and no core starts on a home being put back

**Decision.** Two points of
[D-159](#d-159-an-upgrade-keeps-a-copy-of-the-agents-account-store-and-puts-it-back-with-the-old-core), the first
replaced, the second added:

- **The `init` guard.** `marmot-agent init` reads MDK's account records in the home first, as it always decided
  whether the home has an identity (an account that signs here), and refuses such a home before it opens the store.
  The installer runs `init` whenever the agent has no `public-key` file, on a home that holds files too; `init`'s
  refusal stops the install, which says to write the agent's public key into that file, or to move the home away
  to give the agent a new identity. A home that a first install's `init` was stopped in before it made an identity
  is used. So, as before, `init` never opens an identity's store with the new core uncopied.
- **A home being put back.** While a put back of the agent's home is under way, marked in the upgrade's record
  (`stores/<name>/restoring`), the agent's `start` launcher, which its service runs, refuses to run the core. A
  service manager that starts the agent after a kill and a reboot, before the installer runs again, so finds the
  home part removed or part copied and starts no core on it. An upgrade writes the launchers again before it copies
  the home, so that one an older release wrote, without this refusal, is never beside a put back of that copy. A
  put back whose copy fails part way keeps the mark, and the installer says that the home is incomplete, and to
  fix what failed and run the installer again before starting the agent. The run that finishes the put back removes the mark, as D-159 has it.

**Why.** Issue #215: a first install whose `init` was stopped before it made an identity left files in the home, and
the next run refused it as a home that held one, asking for a public key that did not exist. Asking whether a home
holds an identity is the core's, through MDK; read in the shell it would be MDK's layout copied. Issue #217, from
CodeRabbit's review of #216: a kill during a put back, then a reboot, let the enabled service start the new core on
the home half put back, and a put back that failed part way said only to run the installer again once nothing held
the home.

Decided 2026-11-27 by the builder, under the project owner's instruction to decide minor points. The `init` guard
it replaces was the builder's too. The project owner accepted it on 2026-11-27.

## D-174 The Pi driver remembers, across a restart, which sessions held a message

**Decision.** The Pi driver keeps the sessions it has seen hold a message in a file of its own, one id a line,
`pi-held-sessions` in the agent's account home, and reads it when it starts. After a restart, a session whose file
has gone and which that file names is not made again empty: the prompt is refused, and the chat's notice says
"Pi could not open this session's history, which may have been deleted. Start a new session to go on." The
words say "may": Pi also exits this way when it fails as it starts, which the driver cannot tell apart, since Pi's
diagnostics are not read ([D-071](#d-071-what-may-be-logged)). A session the file does not name, such
as a new one the core restarted after before anyone wrote, is made again with its id as before. This replaces
what [D-092](#d-092-the-choices-made-while-building-m3-are-accepted) says of a deleted session after a restart
(issue #63).

**Why.** Pi writes a session's file only once it holds a message, so a missing file alone cannot tell a session
that never held one from one whose history was deleted; the core records nothing that does either. Making the
deleted one again empty lost its history without a word. The record stays in the driver, so nothing above it
depends on Pi ([D-004](#d-004-four-layers-with-drivers-behind-one-trait)); it holds only Pi's session ids, which
the core already keeps. A refusal, as the driver already gave before a restart, rather than going on empty,
because a file that has gone can also be a moved or unmounted session folder, which a new empty session would
hide.

Decided 2026-11-27 by the builder. The notice's words and the refusal are visible to people; the project owner
accepted both on 2026-11-27.

## D-175 An agent sends its session item when it joins a chat, and an agent's messages are not prompts

**Decision.** Three points, for issue #42:

- **The session item comes first.** When the agent joins a chat, the core sends that chat a `session` item at
  once, before anyone prompts: the harness and its version, the settings the core keeps itself (a chat's folder,
  [D-101](#d-101-a-chat-can-move-a-coding-harness-to-another-folder)) and the rights of
  [D-176](#d-176-the-session-item-says-who-in-the-chat-may-prompt-and-who-may-approve). It names no session,
  since none has started. The first session's item takes its place, as
  [D-030](#d-030-a-session-is-an-item-the-eleventh-type) has it. A chat the agent is already in when the core
  first runs this way gets one too.
- **An agent is a member that has sent a session item.** A member that has sent a `session` item in a chat is an
  agent, to the core and to apps. The core never takes its messages as prompts, whatever `prompt` says, unless the
  owner lists that agent by its key in `prompt`, so that an agent the owner points at another one can still give
  it work. Answers and controls from it are refused the same way. A member running one of today's connectors
  sends no session item and is not recognised; the rights still apply to it as to anyone.
- **Two optional fields, set by the owner, off by default.** A `label` that tells two installs of one harness
  apart ("laptop", "server"), and the owner, as a `p` tag ([replaced by D-179](#d-179-the-owner-shown-in-a-session-item-is-a-field-of-the-item-not-a-p-tag): a field of the item), for an owner who wants the chat to see whose agent it
  is. Both are sent only inside the chat, in the session item. Nothing about the agent goes in its public profile:
  the profile flag [Q-08](open-questions.md#q-08-how-do-several-agent-identities-run-together) proposed is
  dropped.

**Why.** Today an app learns that a member is an agent only at its first turn, so it cannot mark the agent or
offer its commands before then. A public profile flag would publish which accounts are agents, and often whose,
to anyone, and proves nothing, since anyone can set it. The session item is already inside the encrypted chat and
sent by the agent itself. Today only the default rights keep two agents from answering each other: with
`prompt = "everyone"` in a group, two agents that reply to each other could go on without end (inferred from
[D-075](#d-075-in-a-group-the-agent-answers-when-mentioned-or-replied-to-in-a-direct-chat-always), not tested).

Decided 2026-11-27. The project owner accepted the recommendation, from issue #42.

## D-176 The session item says who in the chat may prompt and who may approve

**Decision.** The `session` item carries the chat's two rights
([D-032](#d-032-every-action-needs-one-of-two-rights)): for each, `everyone`, or the keys of the chat's members
who hold it, the owner among them. The rights are set once for the agent
([D-073](#d-073-standing-settings-live-in-a-configuration-file)), so the core cuts each list down to the chat's
members before it sends it, and edits the item when the chat's members change or the settings do. Someone the
owner trusts elsewhere is never named in a chat they are not in. Approval items stay as they are: an app reads
who may answer one from the chat's session item. Changing the rights from the chat stays open
([Q-10](open-questions.md#q-10-how-are-permissions-changed-from-the-chat)).

**Why.** An app learns who may prompt or approve only when someone tries and is refused. With the rights in the
chat, it can grey out the agent for someone who may not prompt it, and show who an approval waits for where its
buttons would only be refused (issue #43). With the default, the owner alone, every member of the chat can now
read which key is the owner's. They learn it anyway the first time they are refused; the owner accepted that it
is now stated.

Decided 2026-11-27. The project owner accepted the recommendation, from issue #43, including what it shows the
chat about the owner.

## D-177 The session item carries the session's title; its project comes later, to full-activity chats only

**Decision.** The `session` item gets a `title`, the harness's own title for the session, filled by the driver
when the harness gives one, and edited when it changes, as a setting is
([D-046](#d-046-a-setting-change-is-an-edit-and-a-notice-only-when-nobody-asked-for-it)). Claude Code
(`session_title_changed`), Goose, Cursor and Antigravity (ACP's `session_info_update`) give one; OpenCode does
not; Codex's thread name and the gateways' session titles are to be checked against each harness when this is
built. A title goes to every chat: it sums up what was said in that chat.

A `project` (a label the owner maps from a folder, the branch, and a pull request's address) is built later, as a
second step, and goes only to chats sent the agent's full activity
([D-136](#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)),
since a branch name can say more than a group should see. Only Claude Code and Codex report a branch.

**Why.** A long-lived chat holds many sessions, and only the session knows what the current work is. With a
title an app can show a chat's topic without anyone renaming the group, which
[D-042](#d-042-an-agent-may-act-in-its-own-chat-and-may-not-change-a-group-or-the-account) rules out for agents
anyway (issue #41). The project is wanted but not needed first, and its branch belongs with the rest of the
agent's work. Issue #41 says a local path is never sent; a chat's folder, a path, already is, as a setting
(D-101), and that is unchanged.

Decided 2026-11-27. The project owner accepted the recommendation, from issue #41.

## D-178 An install counts a start as done once the core, and Claude Code's connector, say they are ready

**Decision.** The installers wait for the agent to say it is ready before they end an upgrade, drop its copy of the
store ([D-159](#d-159-an-upgrade-keeps-a-copy-of-the-agents-account-store-and-puts-it-back-with-the-old-core)) or
note a move done (`moved`,
[D-160](#d-160-the-coding-installers-take-an-install-moved-over-and-run-it-as-a-service)). This closes the limit
D-160 accepted, that a move counted as done once the service manager had started the core.

- **Ready.** The core is ready once it has opened the agent's account store and its driver has started: Codex,
  Pi or an ACP agent has answered its start, or, for a connector or a gateway's plugin, the socket listens. Claude
  Code's connector is ready once the core has taken its hello. A gateway's plugin is not waited for: the gateway is
  the person's, and the installer only prints its restart.
- **How it is said.** On Linux the core's unit is `Type=notify`, with `NotifyAccess=main`: the core sends
  `READY=1`, systemd's notify protocol, to the socket `NOTIFY_SOCKET` names, a path or an abstract name (`@`), with
  a few lines over the standard library's `UnixDatagram` and no new dependency. `systemctl --user restart` then
  returns once the core is ready, and fails if it stops first or is not ready within `TimeoutStartSec`. launchd has
  no such protocol, so on macOS the core's launcher names a file in the agent's folder, `core.ready`, in
  `MARMOT_AGENT_READY_FILE`, and the core makes it. The connector's launcher names `connector.ready` the same way,
  on both systems, and the connector makes it once its hello is taken. The installer removes both files before it
  starts the services, so only this start counts. A core or connector started by hand, with neither variable set,
  does as before.
- **Bounded.** The installer waits up to 120 seconds for each (systemd's start timeout on Linux). A core or a
  connector that stops, or is not ready in that time, is a failed start: the installer says which one, and where its
  logs are, and puts back as D-159 and D-160 say: the old core and the copy of the store, the agent's services left
  stopped, with the command that starts them, or, for a move, disabled and the home as the import left it.
- **The units go back too.** An older core never says it is ready, so on Linux the agent's units as they were
  before the upgrade are kept with the copy of its store and put back with it, and systemd reads them again before
  the installer prints the start command. On macOS the connector is started once the core is ready, as `After=`
  orders it on Linux.

**Why.** Issue #219, from the review of D-160. The units were `Type=exec`, so `systemctl restart` returned once the
launcher ran. A core that failed after that, as when an ACP agent's handshake fails, had moved the store forward
while the installer had already ended the upgrade, dropped the copy and noted the move done; nothing was
unrecoverable, but the old core could no longer open the store. The core is the one that knows when its store is
open and its harness started, so it says so, through systemd's own protocol where there is one. Claude Code's
connector runs as a service of its own, after the core's, and only the hello shows it reached the core. Node cannot
send systemd's datagram without a native module, and a file serves launchd anyway, so it makes a file on both
systems. No release of the connector was published before this, so the installer never meets one that does not
make the file.

Decided 2026-11-27 by the builder; the project owner had agreed to build it. What ready means, the files and the
two-minute bound were decided by the builder, under the project owner's instruction to decide minor points.

## D-179 The owner shown in a session item is a field of the item, not a `p` tag

**Decision.** When the owner sets `show_owner`, the owner's key travels as an `owner` field of the `session`
item's own fields. This replaces the `p` tag that
[D-175](#d-175-an-agent-sends-its-session-item-when-it-joins-a-chat-and-an-agents-messages-are-not-prompts)
names for it; the rest of D-175 stands.

**Why.** MDK does not pass an item's other tags on to apps, while an app reads every field of a session item.
And `p` already names the agent, on a kind 1202 request and on an item in a subagent's thread
([D-142](#d-142-how-subagent-threads-are-built)), so a second meaning on one kind would be read wrongly.

Decided 2026-11-27. The builder found it while building D-175, and the project owner accepted the
recommendation.

Corrected 2026-11-27, in reading MDK for the specification draft of issue #156: the first reason is wrong. MDK at
`bea7b400`, the revision the core uses, does pass an item's other tags on to apps: an app's row holds the latest
version's tags, the ones MDK does not read (`native`, `left_out`, `cut`) among them
(`crates/marmot-uniffi/src/conversions/timeline.rs`). The decision stands on its second reason, that `p` already
names the agent.

## D-180 A prompt a driver refuses as having ended waits for the next driver, whenever the core handed it on

**Decision.** When a driver refuses a prompt saying it has ended and none of the prompt reached it
(`Refusal::ended`: a connector whose connection is gone, or to which nothing of the prompt could be written; Codex
or an ACP agent whose harness is gone), the prompt was never given, whatever the core saw when it last looked
whether the driver had anything for it. Nobody is told, its read mark stays before it, the core reads nothing more
for that driver and ends as for any driver that ends, and the next driver is given it once: the next connector
on the socket, or the core started again with a new harness. This replaces the sentence of
[D-139](#d-139-prompts-a-driver-that-ended-was-never-given-wait-for-the-next-driver) that a connection that goes
after the core's look and before the prompt is written still has that prompt told as not passed, and, for such a
prompt, the part of [D-129](#d-129-a-prompt-whose-answer-a-dropped-connection-lost-is-told-as-perhaps-not-delivered)
that says a prompt the connector never got is told it was not passed.

- **What stays.** A prompt of which any part was written, or that the connector or the harness had, is told as
  D-129 says and never sent again ([D-037](#d-037-a-prompt-is-delivered-at-most-once)). A prompt no connection
  could carry, as one over the size a message may have, is refused as failed and told, never as ended, so it does
  not pass from one connector to the next without end.
- **Answers and controls are still settled.** A driver that refuses an answer or a control as having ended has its
  refusal shown, once, as before ([D-028](#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)):
  what an answer answers, and the turn a Stop stops, belong to the driver that ended and are closed with it
  ([D-058](#d-058-the-driver-trait-and-the-capability-flags),
  [D-070](#d-070-a-restart-closes-the-turn-as-interrupted)), so the next driver could only refuse them; and a
  request that names no item and is more than five minutes old is dropped with no reply
  ([D-140](#d-140-an-answer-or-a-control-the-agent-cannot-read-is-refused)), so one kept for a connector that
  comes later might never be settled at all. The person can press again.

**Why.** Issue #257 and pull request #265 made a prompt refused so wait only when the driver had output ready,
which hid its end from the core's look; when the look was clear, a connection that dropped in the moment after it
still had the prompt told as not passed, and the person had to send it again. The two cases differ only in when
the core looked, which the person cannot see. A prompt none of which reached the driver is in the same place as
one that arrived while no connector was there, which already waits for the next.

Decided 2026-11-27 by the project owner; how answers and controls are treated was decided by the builder, under
the project owner's instruction to decide minor points. The project owner accepted both on 2026-11-27, and that a prompt which
waits through repeated restarts of a failing harness tells nobody, for now: issue #44, which lets the owner see
whether the agent is running, is the fix for that.

## D-181 An app should not offer to answer a secret question in a chat of more than two members

**Decision.** The specification of agents in chats
([D-184](#d-184-the-specification-of-agents-in-chats-is-proposed-to-the-marmot-repository)) makes
[D-041](#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat) a rule for apps as well: an app SHOULD NOT
offer to answer a secret question in a chat of more than two members, and says to answer it at the agent instead.
The agent still refuses such an answer there. D-041 stays the agent's rule, and what the agent does is unchanged.

**Why.** An answer sent to a group is readable by every member, whatever the agent then does with it, so refusing
it only at the agent is too late to keep the secret. An app is the one place that can stop it being sent. It is a
SHOULD, not a MUST, since the agent cannot check an app, and an app that offers it anyway is still refused.

Decided 2026-11-27. The project owner decided, from the specification draft for issue #156.

## D-182 Nothing in a chat proves that a member is an agent, or whose it is

**Decision.** This is stated as a known limit, in the specification and here. A `session` item proves nothing: any
member can send one, so a member that has sent one is an agent only in what it says of itself
([D-175](#d-175-an-agent-sends-its-session-item-when-it-joins-a-chat-and-an-agents-messages-are-not-prompts)).
`rights` in a session item is the agent's statement, for display
([D-176](#d-176-the-session-item-says-who-in-the-chat-may-prompt-and-who-may-approve)): an app MAY grey out what
the viewer may not do and show whom an approval waits for, and MUST NOT grant or deny anything on its strength.
Only the agent enforces its rights, on the owner's machine. How an owner's app could know an agent is the owner's
own is a new open question,
[Q-33](open-questions.md#q-33-how-does-an-owners-app-know-an-agent-is-theirs).

**Why.** MLS proves which account sent an event and nothing more about it. The session item is sent by the agent
about itself, and a profile mark, which D-175 dropped, would prove no more. Saying so keeps an app, or the
attention contract of the Marmot specification's issue 412, from treating the mark as authority. Proof needs
something the owner holds, such as the pairing proposed on issue #4, which is product design of its own.

Decided 2026-11-27. The project owner decided, from the specification draft for issue #156.

## D-183 The agent's words carry `settles`

**Decision.** The provisional addition built on 2026-11-21 is accepted, as
[kinds 1201 and 1202](design/agent-kinds.md#accepted-for-the-specification) has it: a chat message of the
agent's words carries a `settles` tag for each answer or control whose effect it shows, as an item does. An edit of
words cannot carry tags, so what the changed words settle is carried by the next version of their turn, the same as
the last, sent right after the edit; so is what final words that come again unchanged name. If MDK refuses the
edit, that version settles nothing, and those requests are settled as `failed`. How many `settles` the words carry
stays as [D-120](#d-120-the-choices-made-in-moving-to-kinds-1201-and-1202-as-built) has it. The specification
makes the tag normative on kind 9.

**Why.** A final answer is often what shows that an interrupt or an answer took effect, and D-120, already
accepted, depends on the words carrying it. The addition has been built and sent since 2026-11-21.

Decided 2026-11-27. The project owner accepted the provisional addition, from the specification draft for issue
#156.

## D-184 The specification of agents in chats is proposed to the Marmot repository

**Decision.** What the core and MDK send and read for agents in chats is proposed to the Marmot specification
(`marmot-protocol/marmot`) as one feature document, `features/agents-in-chats.md`, with the edits it needs to
`foundation/application-messages.md` (the exception to kind 1009 for an agent item, and the tags on kind 9) and
`foundation/registries.md` (kinds 1201 and 1202), as a draft pull request, from issue #156. It carries no MIP
number: the Marmot repository no longer takes numbered MIPs. With D-181 to D-183, the choices the draft left open
are taken as it recommends:

- **Size.** The limit of [D-026](#d-026-an-agent-item-is-at-most-64-kb) is 65,536 bytes, measured as MDK
  measures it, on the inner event without a signature; the core's own measure is a little stricter, and stays
  within it. A receiver does not refuse an event for its size; an app may decline to show it.
- **Subagent threads** are a separable section, not normative until the specification defines threads.
- **What a reader does with what it does not know.** A stray `e` on a kind 1201 is ignored for placement, replies
  and edits, and senders must not send one. A body-less item, or one with a value a reader does not know, is shown
  generically. An unknown `state` leaves an item unreadable, so a new state needs new kind numbers, and there is no
  version marker for the shape. Senders must not repeat a JSON member, and a reader may treat such a value as
  invalid.
- **Optional parts.** The provisional parts the core sends (the other `cut` kinds, the `too_large` stand-in, the
  `not_delivered` close, the `subagent` tag) are optional, and an app must not depend on them. Senders should not
  send a kind 1201 of type `message`.
- **Order.** An agent's current session item in a chat is its readable one whose first event is latest. Two
  versions with one `v` are ordered by `created_at`, then event id, the greater winning.
- **Attention.** The draft defines no alert policy, only the facts the specification's attention contract (its
  issue 412) needs, with MDK's behaviour as an interim SHOULD.
- **Rules that stay the agent's.** A request that names no item is taken only within five minutes of its
  `created_at`. An app without threads should quote a turn's `started_by` message.

**Why.** The kinds are Marmot events, and a second client needs one place that says what they are
([Q-03](open-questions.md#q-03-where-are-the-event-kinds-specified)). The Marmot repository is organised by
surface since 2026-07-02, and its own idea page for agents names a feature document as its path into the
specification. Each choice describes what MDK does today. Where the core reads more strictly, it is the core's
to change: it rejects a whole item that holds a value it does not know (issue #271).

Decided 2026-11-27. The project owner decided. The draft is opened in the Marmot repository after this is
recorded; until the Marmot repository accepts it, the definition there is a proposal.

## D-185 A reply's quote is what the chat shows now, and a message no longer shown is quoted without its text

**Decision.** When a prompt replies to a message, the core reads the quote from MDK's timeline, which holds what
the chat shows now, when it takes the prompt from MDK:

- a message that was edited is quoted as last edited;
- a message its author deleted is quoted by its author alone, and its text is `[this message was deleted]`;
- a message that MDK invalidated, or that the timeline no longer holds, is quoted the same way, with the text
  `[this message is not available]`.

The harness is never handed what such a message said. A reply to such a message of the agent's is still a reply
to the agent
([D-075](#d-075-in-a-group-the-agent-answers-when-mentioned-or-replied-to-in-a-direct-chat-always)). The
socket's `reply_to` keeps its shape, and no driver or connector changes.

**Why.** The core read the quote from the stored event, which keeps a message's first text for good. A person
who deleted a message, and anyone replying to it later, had its text handed to the harness and its model again
([#266](https://github.com/marmot-protocol/marmot-agents/issues/266), shown by a test). The connectors this
project replaces already left the text out, a behaviour to keep. Keeping the author tells the agent who is
being answered, which the chat still shows. Fixed words in the text's place, and not a new field, keep every
harness's reading of a quote as it is.

**Not covered.** A quote is read once. A prompt the core holds after that, for a connector that is away, keeps
the quote it was read with. And what a harness was handed before the deletion stays in its session.

Decided 2026-11-27. The builder recommended it, and the project owner accepted keeping the author and chose
the wording for a deleted message.

## D-186 The ACP driver sends a message during a turn with the agent's steering extension, when it announces one

**Decision.** An ACP agent that announces steering in `initialize` (`_meta.steering.supported`, as OpenCode
does) is sent a message that arrives during a turn with `_session/steering`, and the driver reports `mid_turn:
fold` for it. The turn goes on, and names that message among the prompts it responds to once the agent has said
it took it. When the agent says it did not, because its turn had just ended, or refuses, the message goes as a
prompt of its own, as before. So do a slash command, which OpenCode reads as plain text when it is steered, and
a message that comes while the turn is being stopped. An agent that announces nothing keeps `restart`: the
driver cancels the turn, then prompts.

**Why.** ACP v1 has no message during a turn, so the driver cancelled the running turn for every agent. In a
group that lets a second person's message cut off the first person's turn
([D-040](#d-040-a-message-sent-during-a-turn-goes-straight-to-the-harness)). OpenCode offers the fold at its
terminal, and the aim is as much from the chat as the terminal gives
([D-011](#d-011-full-control-from-the-phone)). The extension is on the interface the driver already
speaks, so no second interface to OpenCode is needed
([D-006](#d-006-prefer-official-interfaces-and-libraries)). It was seen running in the `steer` recording
(OpenCode 2.3.2, 2026-11-27), and the request came from
[#269](https://github.com/marmot-protocol/marmot-agents/issues/269).

**The cost.** The extension is OpenCode's own and its source calls it interim, until ACP has steering. The
driver uses it only when it is announced, so an OpenCode that drops it goes back to `restart` with no change
here. ACP v2 lets a prompt be sent in any state; when the driver moves to it, this entry is replaced.

Decided 2026-11-27. The builder recommended it, as what apps see, and the project owner accepted it that day.

## D-187 A quote holds the first 2,000 characters of a message

**Decision.** The text of a reply's quote
([D-185](#d-185-a-replys-quote-is-what-the-chat-shows-now-and-a-message-no-longer-shown-is-quoted-without-its-text))
is the quoted message's first 2,000 characters. A longer message is cut there, between characters, and the
quote ends in `…`. The core cuts it, so every harness is handed the same.

**Why.** The whole message was handed over, so each short reply to a long message, such as an agent's own long
answer, sent all of it to the harness and its model again. The reply needs enough of the message to say which
one is meant, and the harness that wrote a message usually still holds it. 2,000 characters is the limit of the
connectors this project replaces, a behaviour to keep
([#267](https://github.com/marmot-protocol/marmot-agents/issues/267)). A mark in the text, and not a new field,
keeps the socket's `reply_to` as it is.

**Not covered.** A quote says nothing of the files the quoted message had, so a reply to a message that was
only a file quotes no text. Naming them would add to a socket message, and is left to its own decision (#267).

Decided 2026-11-27. The builder recommended it, as what a harness is handed, and the project owner accepted it
that day.

## D-188 A quote names the files of the message it quotes

**Decision.** After the text of a reply's quote
([D-187](#d-187-a-quote-holds-the-first-2000-characters-of-a-message)) comes a line for each file the quoted
message carried, with its name and type: `[file: plan.txt (text/plain)]`. A message that was only a file is
quoted by those lines alone. At most 16 files are named, and `[and more files]` follows when there were more;
a name or a type over 200 characters is cut there, and ends in `…`.
The core reads them from the message's own tags and fetches nothing. A message the chat no longer shows names
none ([D-185](#d-185-a-replys-quote-is-what-the-chat-shows-now-and-a-message-no-longer-shown-is-quoted-without-its-text)).

**Why.** A reply to a message that was only a file, "change this one", gave the harness an empty quote, so it
could not tell what was meant. The name and type are what the chat shows of a file, and what the connectors
this project replaces passed on, with the same limit of 16
([#267](https://github.com/marmot-protocol/marmot-agents/issues/267)). The file itself is not passed: the core
passes no files to a harness yet ([D-083](#d-083-the-core-passes-no-files-to-a-harness-yet)).

**In the text, not a field.** The recommendation the project owner accepted was a list of files on the socket's
`reply_to`. Built, it is lines in the quote's text instead, as the two entries before this one did it: every
driver and connector already hands the text to its harness, so none of them changes, and the socket's messages
stay as they are. The cost is that a connector cannot tell a file's line from the message's own words. None
reads a quote for anything but showing it to the model. If one comes to need the list, a field can be added
beside the text then.

Decided 2026-11-27. The project owner accepted the builder's recommendation to name the files; the builder
chose the form.
