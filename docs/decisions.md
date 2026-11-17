# Decisions

This is the record of what has been decided and why. It exists so that settled points are not argued again.

- Add an entry when a decision is made. Give it the next number, the date, the decision and the reason.
- Do not edit an old entry to change its meaning. Add a new entry that replaces it, and mark the old one
  "Replaced by D-0NN".
- A point that is not here is not decided. Proposals live in the `design/` documents and in
  [`open-questions.md`](open-questions.md).

All entries so far were decided by the project owner in the planning discussion of 2026-11-17.

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
