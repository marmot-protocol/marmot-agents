# Subagents as threads

Status: a proposal, for [Q-30](../open-questions.md#q-30-how-does-a-chat-show-an-agents-subagents). Nothing on
this page is decided. The direction was agreed with the project owner on 2026-11-20; the points to decide are
listed [at the end](#to-decide). Some would be exceptions to, or new entries beside, decisions already made,
and say so. Meanwhile Codex's subagents are shown flat in the chat that started them
([D-117](../decisions.md#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them)),
and the Codex transport of point 8 is built for that.

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
  working, done or waiting for an approval, and how long it has run. What the agent does that names that
  subagent (starting it, giving it more work) updates that row. What does not name one, such as Codex's
  waiting for its subagents, is shown as the agent's own activity, as it is today.
- **Opening it** shows its thread: its commands, edits, plans, approvals and words, as the main chat shows the
  agent's own. A subagent that starts a subagent has a row in its thread, which opens a thread in turn.
- **Stop in the main chat** stops the agent and every subagent, including subagents still running after the
  agent's own turn has ended. **Stop in a thread** stops that subagent and every subagent it started.
- **A waiting approval or question alerts**, wherever it is, as any does
  ([D-024](../decisions.md#d-024-who-an-agent-event-alerts)). The row in the main chat shows that one is waiting.
  A subagent's words do not alert: its result reaches the agent, whose final answer alerts as it does today.
- **Writing in a subagent's thread** is a message for that subagent, when the harness lets a person talk to it
  and the writer has the right to prompt. Otherwise the person is told it cannot.

## How it travels

Marmot needs nothing new for the thread itself. Nostr's threaded comments
([NIP-22](https://github.com/nostr-protocol/nips/blob/master/22.md)) already work inside Marmot groups: White
Noise for Linux shows them as threads, and MDK stores them and works out each one's parent from its `e` tag.

| What | Sent as |
| --- | --- |
| The subagent's row in the main chat | A `task` item, kind 1201, in the turn that started it, edited as the subagent works, like any item ([D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit)). The thread hangs from its first event |
| Everything else the subagent does, of every item type the main chat shows: turns, reasoning, commands, edits, approvals, questions, plans, notices, errors | Kind 1201 items as in the main chat, with one more tag saying which thread they are in. It is a Marmot tag, not `e`, which a kind 1201 already uses for the message that started its turn and an edit uses for the item it replaces. Its name is chosen with the kind 1201 specification. An edit carries the whole item ([D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit)), so it repeats the tag, with the value the item was first sent with: an item never moves to another thread |
| The subagent's words, the one kind sent differently | Kind 1111 comments, NIP-22's own kind, so an app that shows threads shows them |
| A subagent's subagent | A `task` item inside the first thread, from which a thread of its own hangs |

A comment carries NIP-22's tags: uppercase for the root of the whole tree (the first `task` item, in the main
chat) and lowercase for its parent (the `task` item whose thread it is in). For a subagent started directly by
the agent, the two are the same:

```
["E", <task item's first event>, "", <agent's key>], ["K", "1201"], ["P", <agent's key>],
["e", <task item's first event>, "", <agent's key>], ["k", "1201"], ["p", <agent's key>]
```

In a nested thread, `E` still names the first `task` item, and `e` names the inner one. The thread of a `task`
item is the comments whose `e` names it; `E` is there so that one subscription can follow the whole tree. A
person's reply inside a thread names, as `e`, the comment it answers, so a reply belongs to the thread of the
comment it answers, found by following `e` back to a `task` item.

This is an exception to [D-022](../decisions.md#d-022-the-agents-words-are-ordinary-chat-messages), which sends
the agent's words as kind 9 so every app shows them. A comment is shown only by an app that shows threads, and
only if it also shows the `task` item it hangs from: White Noise for Linux shows threads, but whether it shows a
thread under a kind 1201 has not been checked, and the phone apps do not show threads yet. Until they do, a
subagent's words are visible only to apps that do both.

Comments alert today, as chat messages do: MDK keeps silent only kinds 1201 and 1202
([who gets alerted](event-carrier.md#who-gets-alerted)). So "a subagent's words do not alert" is a new rule beside
D-024, and MDK has to treat comments in an agent's thread that way.

## What each part does

| Part | What it does |
| --- | --- |
| The event model | A subagent's session names the `task` item that started it, which answers the thread part of [Q-07](../open-questions.md#q-07-how-does-a-chat-map-to-a-harness-session). The `task` item says what the subagent was asked, where the harness says (Codex's newer runtime does not), and how it ended. The `tool` kind `subagent` is not used for a subagent that has a `task` item, so it has one row |
| A driver | Finds each subagent, makes its `task` item and keeps it current, gives the subagent's activity a session of its own, and stops a subagent and those it started when asked |
| The Codex transport | Gives each subagent's thread a translator of its own, attached to the chat of the thread that started it; the parent's translator never sees those messages. Holds a message for a thread it does not know yet until the item that names it arrives, since a subagent can speak first ([recorded](../reference/harness-messages.md#subagents)). Interrupts subagents itself, because interrupting the parent does not stop them: for Stop in the main chat, every live turn whose `rootTurnId` is the agent's turn; for Stop in a thread, that subagent's live turn and, through the links Codex gives when one starts another (`agentThreadId`, or `receiverThreadIds` on the older runtime), those of the subagents it started, and theirs. `rootTurnId` is the same for every subagent under one turn, so it cannot tell them apart. This is a new decision beside [D-060](../decisions.md#d-060-a-codex-translator-follows-one-thread-and-the-transport-routes), not a change to it |
| The core | Holds a subagent's events until its `task` item has been sent, then adds the thread tags. Treats a person's comment in a subagent's thread as a prompt for that subagent's session, when the harness allows it and the writer has the right to prompt ([D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights)); the agent's own comments are never prompts. Takes a Stop as described below |
| MDK | Works out a kind 1201 event's thread from its tag, as it does a kind 1111 comment's parent |
| White Noise | Shows kind 1201 items and kind 1111 comments inside a thread under a `task` item, and the `task` item as the subagent's row. The Linux app has threads; the phone apps would need them |

## Stop, when the agent's turn has ended

The recordings show a subagent still working after the agent's turn has ended. Today a control acts on a running
turn ([D-029](../decisions.md#d-029-three-shared-controls-and-the-rest-comes-from-the-harness)), and its effect
is shown by one reply ([D-028](../decisions.md#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)).
The proposal extends both:

- **Stop in the main chat** names the agent's last turn. It is taken while that turn, or any subagent under it,
  is still working, not only while the turn itself is.
- **Stop in a thread** names that subagent's turn, and is taken while it or a subagent it started is working.
- **It is settled once, when the last of them has ended**, by the edit of the `task` item or turn that ended last,
  which points back at the Stop. If nothing was working, it is settled by the notice "nothing to act on", as
  today.

## What it costs

- **Volume.** Every member's device receives every subagent's items. In a chat with one person and their agent
  that is what they asked to see. In a large group it is more traffic, and nothing here reduces it.
- **The specification.** The agent event kinds are not specified yet
  ([Q-03](../open-questions.md#q-03-where-are-the-event-kinds-specified)); the thread tag on kind 1201 goes into
  that specification when it is written.

## Considered and not proposed

- **A separate group per subagent.** Every subagent would need a group created and its members added, and the
  chat would scatter across groups.
- **Only a summary in the main chat**, with the subagent's own activity left out. Smaller, but it hides what the
  subagents do, against [D-008](../decisions.md#d-008-pass-everything-through), and its approvals still have to be
  shown somewhere.
- **Keeping a subagent's detail on the agent's machine, fetched on demand.** It would need a protocol for
  fetching, and the detail would be gone when that machine is.
- **A subagent's words as kind 9.** Every app would show them, but in the main chat, mixed with the agent's own.

## To decide

1. A subagent's row is its `task` item, and its thread hangs from that item's first event, in the same group.
   What names the subagent updates the row; what does not stays the agent's own activity.
2. Every item a subagent makes carries a Marmot thread tag, named with the kind 1201 specification, and every
   edit repeats it with the first value.
3. A subagent's words are kind 1111 comments with NIP-22's tags, parented by `e`. This is an exception to D-022.
4. Those comments do not alert. This is a new rule beside D-024, and MDK has to follow it.
5. A subagent's session names its `task` item, and the driver keeps that item current.
6. Stop in the main chat stops every subagent under the agent's turn; Stop in a thread stops that subagent and
   those it started. Either is taken while any of them works, and settled once, when the last has ended. This
   extends D-028 and D-029.
7. A person's comment in a subagent's thread is a prompt for it when the harness allows it and the writer has
   the right to prompt; otherwise the person is told. The agent's own comments never are.
8. The Codex transport gives each subagent's thread its own translator in the parent's chat, holds messages for
   a thread it does not know yet, and interrupts subagents itself, following the links between them. This is a
   new entry beside D-060.
