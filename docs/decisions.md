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
