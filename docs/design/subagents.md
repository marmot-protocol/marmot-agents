# Subagents as threads

Status: a proposal, for [Q-30](../open-questions.md#q-30-how-does-a-chat-show-an-agents-subagents). Nothing on
this page is decided. The direction was agreed with the project owner on 2026-11-20; the points to decide are
listed [at the end](#to-decide).

An agent can start subagents: agents of its own that work on part of a task, in parallel, and can start
subagents in turn. Codex and Claude Code both do. This page proposes how a chat shows them: each subagent as a
thread inside the conversation, which a person can open to see everything it does.

## What a person sees

```
Chat with Codex
  Alice: refactor the parser and update the tests
  ▸ Working
     ┌ Subagents ────────────────────────────┐
     │ Read the grammar        done     0:41 │  ← tap: opens its thread
     │ Update the tests        working  1:12 │
     │ Check other callers  ⚠ needs approval │
     └───────────────────────────────────────┘
  Codex: Done. The parser now…
```

- **In the main chat**, each subagent is one row that changes as it works: what it is for, whether it is
  working, done or waiting for an approval, and how long it has run.
- **Opening it** shows its thread: its commands, edits, plans, approvals and words, as the main chat shows the
  agent's own. A subagent that starts a subagent has a row in its thread, which opens a thread in turn.
- **Stop** in the main chat stops the agent and every subagent. Stop inside a thread stops that subagent only.
- **A waiting approval alerts**, wherever it is, as any waiting approval does
  ([D-024](../decisions.md#d-024-who-an-agent-event-alerts)). The row in the main chat shows that one is waiting.
- **Writing in a subagent's thread** reaches the subagent only if the harness lets a person talk to it.
  Otherwise the person is told it cannot.

## How it travels

Marmot needs nothing new. Nostr's threaded comments ([NIP-22](https://github.com/nostr-protocol/nips/blob/master/22.md))
already work inside Marmot groups: White Noise for Linux shows them as threads, and MDK stores them and works out
each one's parent. A thread message is a kind 1111 comment with an `e` tag naming the event it belongs under
and a `k` tag giving that event's kind.

| What | Sent as |
| --- | --- |
| The subagent's row in the main chat | A `task` item, kind 1201, in the turn that started it. Edited as the subagent works, like any item ([D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit)) |
| The subagent's own items: its turns, commands, approvals, plans | Kind 1201 items, as in the main chat, with the thread's tags: `e` naming the task item's first event, `k` 1201 |
| The subagent's words | Kind 1111 comments with the same tags, as the agent's words in the main chat are ordinary chat messages ([D-022](../decisions.md#d-022-the-agents-words-are-ordinary-chat-messages)) |
| A subagent's subagent | A `task` item inside the first thread, which roots a thread of its own |

So an app that shows threads but knows nothing of agents still shows what each subagent says, in its thread.

## What each part does

| Part | What it does |
| --- | --- |
| The event model | A subagent's session names the `task` item that started it. A `task` item for a subagent says what it was asked, where the harness says (Codex's newer runtime does not), and how it ended |
| A driver | Finds each subagent, makes its `task` item and keeps it current, gives the subagent's activity its own session, and stops every subagent when the agent is stopped |
| The Codex driver | Routes a subagent's thread to the chat of the thread that started it ([D-060](../decisions.md#d-060-a-codex-translator-follows-one-thread-and-the-transport-routes) grows to say so). Holds a message for a thread it does not know yet, since a subagent can speak before the item that names it arrives ([recorded](../reference/harness-messages.md#subagents)). Interrupts each subagent's turn on Stop, because Codex does not |
| The core | Turns a session's link to its `task` item into the thread tags, and lets an answer or a Stop in a thread act on that subagent |
| MDK | Works out a kind 1201 event's thread from its tags, as it does for kind 1111 |
| White Noise | Shows kind 1201 items inside a thread, and the `task` item as the subagent's row. The Linux app has threads already; the phone apps would need them |

## What it costs

- **Volume.** Every member's device receives every subagent's items. In a chat with one person and their agent
  that is what they asked to see. In a large group it is more traffic, and nothing here reduces it.
- **The specification.** The agent event kinds are not specified yet
  ([Q-03](../open-questions.md#q-03-where-are-the-event-kinds-specified)); the thread tags on kind 1201 go into
  that specification when it is written.

## Considered and not proposed

- **A separate group per subagent.** Every subagent would need a group created and its members added, and the
  chat would scatter across groups.
- **Only a summary in the main chat**, with the subagent's own activity left out. Smaller, but it hides what the
  subagents do, against [D-008](../decisions.md#d-008-pass-everything-through), and its approvals still have to be
  shown somewhere.
- **Keeping a subagent's detail on the agent's machine, fetched on demand.** It would need a protocol for
  fetching, and the detail would be gone when that machine is.

## To decide

1. A subagent is a NIP-22 thread in the same group, rooted at its `task` item.
2. A subagent's items carry the thread's `e` and `k` tags; its words are kind 1111 comments.
3. A subagent's session names the `task` item that started it, and the driver keeps that item current.
4. Stop in the main chat stops every subagent; Stop in a thread stops that subagent.
5. A person can write to a subagent only when the harness allows it.
6. The Codex transport routes a subagent's thread to its parent's chat, holds messages for a thread it does not
   know yet, and interrupts each subagent on Stop.
