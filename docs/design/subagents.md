# Subagents as threads

Status: decided on 2026-11-23, as
[D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them)
to [D-126](../decisions.md#d-126-no-mls-group-for-subagents), which settle what was Q-30, with what an edit carries
settled by [D-133](../decisions.md#d-133-an-edit-of-an-item-in-a-subagents-thread-repeats-the-threads-root-tags).
Built on 2026-11-24 in the core and the Codex driver
([D-141](../decisions.md#d-141-subagent-threads-are-built-before-the-swap),
[as built](#as-built), [D-142](../decisions.md#d-142-how-subagent-threads-are-built)), and in MDK at
`029e0579f`; the White Noise apps have their part still to do. Still open: two proposals from the cases, which were
not among the points decided ([still open](#still-open)).
The direction was agreed with the project owner on 2026-11-20 and sharpened on 2026-11-23, after a research round
written up in [subagents, threads and what others do](../reference/subagents-prior-art.md).

An agent can start subagents: agents of its own that work on part of a task, often in parallel, and can start
subagents in turn. Codex, Claude Code, OpenCode, Cursor, Goose, Hermes and OpenClaw all do, and four of them
say in their events which subagent did what. This page says how a chat shows them.

## The core interaction

```
Chat with Codex
  Alice: refactor the parser and update the tests
  Codex: I'll split this into three parts.
     ▸ Read the grammar        done     0:41  ·  3 replies  ← tap: opens its thread
     ▸ Update the tests        working  1:12
     ▸ Check other callers  ⚠ needs approval
  ┌──────────────────────────────────────────────────────┐
  │ 2 subagents running · 1 needs you                  › │  ← stays while any runs
  └──────────────────────────────────────────────────────┘
  [ Message Codex…                                       ]
```

1. **Each subagent is one row in the chat**, under what the agent was doing when it started it. The row says
   what the subagent is for, whether it is working, done, stopped or waiting for someone, and how long it has
   run. It changes in place; it never becomes a stream of lines.
2. **Tapping the row opens its thread**, as a thread opens in Slack or Discord. The thread holds everything the
   subagent did, drawn as the main chat draws the agent's own work: its commands, edits, plans, approvals,
   questions and words. Anything people wrote there is in it too. A subagent that starts a subagent has a row
   in its thread, which opens a thread of its own.
3. **While any subagent runs, a bar sits above the composer**: "2 subagents running · 1 needs you". Tapping it
   lists them; tapping one opens its thread. When the last one ends, the bar goes away.
4. **When a subagent needs a person, they hear about it where they are.** Its approval or question alerts, as
   any does, and shows in the bar and on its row. It can be answered from the bar without opening the thread,
   and the thread shows who answered.
5. **Writing in a thread reaches that subagent when its harness lets a person talk to it.** When it does not,
   the composer says so before anything is sent ("Codex's subagents don't take messages; write in the main
   chat"), and people can still talk to each other there.
6. **Stop in the main chat stops the agent and every subagent it started**, even one still running after its
   turn has ended. **Stop in a thread stops that subagent and the ones it started.**
7. **A finished thread folds into its row.** The row keeps its result and its replies count; the thread stays
   readable for as long as the chat keeps history.

Everything else on this page follows from these seven. The rest is how it travels, then twelve harder cases.

## How it travels

Marmot needs no new kind. A thread in a Marmot group is already NIP-22's: a root event, and comments that name it
([MDK's contract](../reference/subagents-prior-art.md#mdk-at-6f6b427)). A subagent's thread uses NIP-22's tags as
NIP-22 defines them
([D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them)):
the uppercase ones name the root of the whole tree, the lowercase ones the immediate parent.

| What | Sent as |
| --- | --- |
| The subagent's row | A `task` item, kind 1201, sent in the turn that started it and edited as the subagent works ([D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit)). A top-level subagent's is in the main chat, carries no thread tags, and is the root of its tree |
| Everything the subagent does except its words | Kind 1201 items as in the main chat, each also carrying the thread tags; an edit repeats the root tags ([an edit](#an-edit)) |
| The subagent's words | Kind 1111 comments with the thread tags |
| A person writing in the thread | A kind 1111 comment, as any thread message MDK sends today. Its parent is the message it answers, or the subagent's `task` item |
| A subagent's subagent | A `task` item in its parent's thread. Its own events name it as their parent |

The tags on every event in a top-level subagent's tree:

- **`E`, `K`, `P`**: the top-level subagent's `task` item, kind `1201`, and the agent. The same at any depth.
- **`e`, `k`, `p`**: the immediate parent, the `task` item of the subagent the event belongs to. For a top-level
  subagent's own events it is the same item as the root, as for a first comment.

```
Main chat
  task item A   (a subagent of the agent; no thread tags)
    A's command, A's words         E = A   e = A
    task item B (A's subagent)     E = A   e = A
      B's command, B's words       E = A   e = B
```

B's command, in full:

```
["E", <A's first event>, "", <agent's key>], ["K", "1201"], ["P", <agent's key>],
["e", <B's first event>, "", <agent's key>], ["k", "1201"], ["p", <agent's key>]
```

**Nesting is rebuilt from the lowercase parent.** A's tree is everything whose root is A. An app opens a
subagent's view by filtering on its `task` item as the parent: A's view holds what names A, B's what names B. A
reply to a message names that message as its parent, and belongs to the
view of the first `task` item up its chain of parents (inferred from NIP-22; not checked in an app). MDK already
places a thread message this way: it takes the root from its parent's thread
(`crates/marmot-app/src/client/thread_placement.rs` at MDK `6f6b427`), so a person's reply anywhere in the tree
keeps A as its root. An app that reads only the root shows B's events inside A's thread: busy, and nothing lost.
White Noise for Linux already shows a thread inside a thread as a view of its own (inferred to fit; not checked
with agent items).

The core finds the subagent a person's comment is for by the same walk: the first `task` item up its parents.

### An edit

An edit, kind 1009, already uses `e` for its target
([D-112](../decisions.md#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)), so it
cannot also carry NIP-22's parent `e`. It repeats the thread's root tags, `E`, `K` and `P`, with the same values
as the item's first event, and carries no lowercase parent tags, so a member who joined after the first event can
still place it in its thread ([D-133](../decisions.md#d-133-an-edit-of-an-item-in-a-subagents-thread-repeats-the-threads-root-tags)).

### What MDK has to do

Done in MDK by pull requests 3622, 3630 and 3640 (in master at `029e0579f`, and in `bea7b40`, which the core uses now): a kind
1201 with `E` joins its thread under its `e`, an edit keeps its first event's place and may repeat `E`, `K` and
`P`, the agent's own comments in its threads neither alert nor count as unread, a `p` naming the sender is no
mention, and a late joiner is shown an item from its edits. What follows was the list as read at MDK `6f6b427`:

- **Place a kind 1201 event in a thread.** Today only a comment, a chat message and a poll can be in one.
- **Take the parent `e` on an item's first event.** Today MDK refuses any `e` on a kind 1201 event a program
  sends, because an edit's target is MDK's to place.
- **Settle what a parent `p` means here.** MDK leaves NIP-22's parent `p` off the comments it sends, because on
  Marmot a `p` is a mention. On these events it always names the agent that sends them. The recommendation is
  that MDK takes a `p` naming the event's own author as no mention.
- **Keep the agent's comments in its own threads silent**
  ([D-122](../decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar)). Today a thread message
  alerts like chat.

## What each part does

| Part | What it does |
| --- | --- |
| A driver | Says whether its harness tells subagents apart, and whether a person can write to a running one. Finds each subagent, makes its `task` item and keeps it current, gives its activity a session of its own that names the `task` item, closes it when the harness stops, and stops a subagent and those it started when asked |
| The Codex transport | Gives each subagent's thread a translator of its own, in the chat of the thread that started it. Holds a message for a thread it does not know yet until the item that names it arrives, since a subagent can speak first ([recorded](../reference/harness-messages.md#subagents)). Interrupts subagents itself, since interrupting the parent does not stop them, following `agentThreadId` (and `parentThreadId` from 0.171.0, or `receiverThreadIds` on the older runtime) |
| The core | Puts a subagent's events in its thread once its `task` item has been sent, adding the thread tags, and in the main chat when it has not. Takes a person's comment as a prompt for the subagent whose `task` item is first up its parents, when the driver allows it and the writer may prompt. Takes Stop as below |
| MDK | Places a kind 1201 event and its edits in the thread its tags name, as it does a poll's. Takes the parent `e` on an item's first event. Keeps the agent's own comments in its threads silent |
| White Noise | Draws the `task` item as the row and the thread under it, a nested subagent's view from the lowercase parent, the bar from the `task` items still open, and a waiting approval in the bar |

## As built

What [D-142](../decisions.md#d-142-how-subagent-threads-are-built) chose where the decisions left it open:

- A driver names the subagent each event belongs to in the event's `subagent` field, its `task` item's id; a
  nested subagent's `task` item names the one that started it. The events stay in the chat's session. A driver
  says, in the capability flag `subagents`, whether its harness tells its subagents apart; only Codex's does.
- The core places an item when its first event goes out, and keeps that place for its later versions. It holds
  nothing back: a group's sends go out in order, after the `task` item. An item whose `task` item has not gone
  out to that chat, as in a chat not sent the agent's full activity, goes in the main chat.
- A subagent's words are MDK's thread message under its `task` item, sent once complete; a later version is MDK's
  plain edit of it, as MDK keeps no edit of a thread message under a token. Words whose `task` item has
  disappeared under the chat's retention go in the main chat. Each item carries a `subagent` tag with the
  `task` item's id, so an app can find a nested subagent's view from an edit, which names only the root.
- A Codex subagent's row shows its newest turn's state; an earlier turn's end read late only adds to its text
  that the turn failed, with its error, when it did.
- Each Codex subagent is one `task` item, which each of its turns opens again.
- A Stop in a subagent's thread stops that subagent and those it started, for a driver that says it can
  (`stop_subagent`, Codex's); for any other it stops the whole turn
  ([D-149](../decisions.md#d-149-stop-in-a-subagents-thread-stops-that-subagent-alone)).
- Not built yet: writing to a running subagent, which no driver allows.

## The cases

Each says what others do, from [the research](../reference/subagents-prior-art.md), and what was proposed. What
is marked **Proposed** was decided as proposed, in the entry named, except where it says otherwise.

### 1. A subagent asks for approval or asks a question

Claude Code, Codex and T3 Code all show a subagent's approval where the person already is, naming the
subagent; none makes them go and find it. Claude Code takes `AskUserQuestion` away from subagents, and Hermes
blocks its children's `clarify`.

**Proposed:** the `approval` or `question` item lives in the subagent's thread, like everything it does, and
alerts as any does ([D-024](../decisions.md#d-024-who-an-agent-event-alerts)). The app lifts every waiting one
into the bar, with the subagent's name and the choices, so it can be answered there. The answer is the same kind
1202 event as anywhere, pointing at the item, and the item's edit shows who answered.

### 2. Subagents start subagents

Claude Code allows three levels below the main conversation, OpenClaw five, Codex has a limit of its own, Cursor
two; Goose and Hermes (by default) one. Nobody draws more than a list or a tree of names.

**Proposed:** a nested subagent is a row in its parent's thread, and its thread opens from there. The bar counts
every running subagent at any depth, and its list shows the path ("Update the tests › Find the fixtures"), so
no one has to open threads one by one to find a waiting approval.

### 3. Who a reply in a thread goes to

Only Claude Code's terminal and OpenClaw let a person type to a running subagent. T3 Code makes subagents an
agent started read-only on purpose, and Codex and Cursor route through the parent.

| Harness | Can a person write to a subagent | From |
| --- | --- | --- |
| Claude Code | In the terminal, yes. Through the SDK, not found | Documentation, SDK types |
| Codex | Not tested: `turn/start` accepts any thread, and nothing forbids a subagent's | Schema |
| OpenClaw | "Visible spawned sessions can be typed into" | Documentation |
| OpenCode, Goose, Cursor, Pi, Hermes | No way found | Documentation, source |

**Proposed:** a person's comment in a thread goes to that subagent only where its driver says it can, and the
writer has the right to prompt ([D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights)). The `task`
item says whether it can, so the app knows before anything is sent. Elsewhere the comment is for people, and
the agent never sees it. It is never passed to the parent instead: the parent would answer in the main chat, to
a message it was not sent. The agent's own comments are never prompts. Where it does go through, the
subagent's next turn shows it, as any prompt's turn does.

Whether Claude Code and Codex can take one is to be recorded before their drivers say yes.

### 4. The running bar, and a subagent whose end never arrives

Claude Code keeps a panel of running subagents below the prompt; T3 Code a **Working** section and a
"Subagent waiting" line; the Codex IDE a list above the composer.

**Proposed:** the bar is drawn by the app from the `task` items still `running` or `waiting`. Nothing new is sent
for it. A subagent can still fail to report its end: Codex reports one only from the parent's thread, and a
harness that is killed reports nothing. Two rules cover it:

- **The driver closes what it can no longer follow.** When the harness stops, the session ends or the core
  restarts, every open `task` item is edited to `cancelled`, as an open turn already is
  ([D-070](../decisions.md#d-070-a-restart-closes-the-turn-as-interrupted)).
- **The app keeps counting a quiet row.** A `running` row with no edit for 30 minutes stays in the bar and reads
  "no news since 14:02". The agent's machine may simply be off, and nothing is assumed. 30 minutes is a first
  value with no evidence behind it. (The proposal had the row leave the bar; the project owner chose to keep it,
  [D-124](../decisions.md#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners).)

### 5. Volume

Every subagent's events go to every member of a chat that is sent the agent's full activity, as everything an
agent does does
([D-009](../decisions.md#d-009-groups-are-first-class-and-every-member-sees-agent-activity),
[Q-12](../open-questions.md#q-12-how-many-events-is-too-many)). For the pilot, any other chat is sent only a
subagent's approvals and questions
([D-136](../decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)). An MLS application message is encrypted once
for the whole group, not once per member, so the cost is each device downloading and storing it, not the agent
encrypting it many times. No subagent volume has been measured; in one developer's Claude Code use,
background-task messages were about 7 per turn.

**Proposed:** nothing new to reduce it now. Still a proposal: a thread is the natural first thing to thin when the detail setting
of [the Q-11 and Q-12 proposal](../open-questions.md#a-proposal-for-q-11-and-q-12-not-decided) is decided: at
"Summary", a subagent would send only its `task` item, plus its approvals and questions. A separate MLS group per
subagent would not cut it either, since it has the same members (see
[MLS subgroup branching](../reference/subagents-prior-art.md#mls-subgroup-branching)).

### 6. Collapsing finished threads, or keeping them for less time

Claude Code clears a finished row after 30 seconds and keeps it in `/tasks`; Discord archives an idle thread
and keeps it; T3 Code moves a finished thread out of **Working**. Nobody deletes subagent work sooner than the
rest.

**Proposed:** fold, in the app; keep, in the protocol. A finished row keeps its result and replies count and
leaves the bar. A shorter life for thread events would be a new kind of expiry in Marmot, and it would also take
away what an owner may later need to check.

### 7. The report shown twice

Codex's parent restates what its subagent said (recorded: the subagent says "It printed: Hello, world", then the
parent "The subagent reported that … printed"). Claude Code also gives a `summary` when a subagent ends.

**Proposed:** show both. The subagent's words stay in its thread; the parent's restating is the parent's own
answer, in the main chat. The row adds a one-line result only where the harness gives one, such as Claude
Code's `summary`, and never the whole report. Folding the thread keeps the duplicate out of sight.

### 8. Stop in a thread, Stop everything, and a parent that starts again

T3 Code, OpenClaw's `/stop` and Hermes's `/stop` stop the whole tree. Claude Code has no stop-all and stops one
subagent from its row. Interrupting a Codex turn leaves its subagents running.

**Proposed**, as before: Stop in the main chat names the agent's last turn and stops it and every subagent the
agent started, including subagents still running after the turn ended
([D-123](../decisions.md#d-123-stop-and-writing-in-a-subagents-thread)). Stop in a thread names that subagent and
stops it and those it started. Either is taken while any of them works, and settled once, when the last has
ended, by the edit of whichever ended last ([D-028](../decisions.md#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it),
[D-029](../decisions.md#d-029-three-shared-controls-and-the-rest-comes-from-the-harness)). If the parent is still
working when a subagent is stopped, the harness tells it, and it may start another to do the same work. That is
the parent's choice, shown as a new row; the person who wants it all to end uses Stop in the main chat.

### 9. A thread that began before someone joined

A member added to the group cannot read anything sent before they joined. So they miss a running subagent's
`task` item and the start of its thread, and get later edits of an item they never had. MDK at `6f6b427` held
such an edit, waiting for an item that would never come
([what this costs](event-carrier.md#what-this-costs)); at `029e0579f` it shows the item from its edits.

**Proposed:** an edit carries the whole item, so an app that holds an edit of an item sent before the member
joined shows the edit as the item. This helps every item, not only subagents. The thread then shows what
arrived after the member joined, under a row that says it started earlier.

### 10. Several agents with subagents in one group

Each agent is its own account ([D-035](../decisions.md#d-035-each-harness-install-is-its-own-identity-with-its-own-sessions)),
so its `task` items, and the `P` in its threads' root tags, are its own.

**Proposed:** the bar groups by agent ("Codex: 2 running · Claude: 1 needs you"). Each agent's core reads only
comments in threads whose root it sent, and ignores the rest, as it ignores an answer that names another agent.

### 11. Harnesses that do not tell subagents apart

Goose names a child only in a `delegate` call's result, and its lines arrive as the parent's. Hermes tells a
plugin nothing about its children except their approvals and failures. Pi has none of its own. Cursor's showed
nothing over ACP. OpenCode tells them apart, with a parent and a depth.

**Proposed:** no thread. The subagent is a `tool` item of kind `subagent` in the agent's own activity, as today,
and anything it asks shows as the agent's own approval. The driver says which it does, so nothing above it
checks the harness's name ([D-058](../decisions.md#d-058-the-driver-trait-and-the-capability-flags)).

### 12. Apps without threads

The White Noise phone apps have no thread view yet. Android plans one, off by default and switched on per chat
([its issue](https://github.com/marmot-protocol/whitenoise-android/issues/2841)). MDK's contract has an app
without one show a thread message inline in the main timeline, quoting its parent.

**Proposed:** nothing is lost and nothing is hidden: an app without threads shows the subagent's items as the
generic rows it shows any agent item, and its words inline, all in the main chat. It is busy, and it is what
those apps do with any thread today. Still a proposal, not among the decided points: an app with threads always shows an agent's subagents in threads, whatever
its setting for people's replies, since the setting is about how people talk and this is about what an agent
does.

## Considered and not chosen

- **A separate MLS group per subagent, by RFC 9420 branching.** Same members, so nothing gained; a KeyPackage
  and a Welcome per member per subagent, and a chat scattered across groups. MDK does not do it; OpenMLS can.
  Detail [here](../reference/subagents-prior-art.md#mls-subgroup-branching). A branch for owner-only output
  ([Q-11](../open-questions.md#q-11-what-keeps-sensitive-output-from-spreading)) is a different question
  ([D-126](../decisions.md#d-126-no-mls-group-for-subagents)).
- **A Marmot tag of our own for the thread**, as the first draft proposed. NIP-22's tags already do the job, and
  MDK already reads them on other kinds.
- **A flat thread per subagent, with `E` naming the nearest `task` item**, as the revised proposal had it. It
  would have given NIP-22's root tag a meaning of its own, and MDK's placement passes a parent's root on
  ([D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them)).
- **Passing a reply in a thread to the parent** when the subagent cannot take it. The parent's answer would
  appear in the main chat, to a message it was not sent there.
- **Only a summary in the main chat**, with the subagent's activity left out. It hides what the subagents do,
  against [D-008](../decisions.md#d-008-pass-everything-through), and its approvals still have to be shown
  somewhere.
- **Keeping a subagent's detail on the agent's machine, fetched on demand.** It needs a protocol for fetching,
  and the detail is gone with the machine.
- **A subagent's words as kind 9.** Every app shows them, but in the main chat, mixed with the agent's own, and
  an app with threads could not place them without the same thread tags.
- **A shorter retention for threads.** See case 6.

## Decided

The project owner decided the twelve points of the proposal on 2026-11-23, with three changes: the thread tags
are NIP-22's as NIP-22 defines them, not flat threads (point 2); Stop in the main chat reaches every subagent the
agent started, not only those under its turn (point 6); and the app keeps counting a quiet row (point 8).

| Point | Entry |
| --- | --- |
| 1. A subagent's row is its `task` item, and its thread is in the same group | [D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them) |
| 2. The thread tags are NIP-22's `E`/`K`/`P` for the top-level `task` item and `e`/`k`/`p` for the immediate parent's | [D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them) |
| 3. A subagent's words are kind 1111 comments | [D-122](../decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar) |
| 4. The agent's comments in its own threads do not alert | [D-122](../decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar) |
| 5. The app's running bar, waiting approvals lifted into it, and finished threads folded | [D-122](../decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar) |
| 6. Stop in the main chat stops every subagent the agent started; Stop in a thread stops that subagent and those it started; settled once | [D-123](../decisions.md#d-123-stop-and-writing-in-a-subagents-thread) |
| 7. A comment in a thread is a prompt only where the driver allows it and the writer may prompt; never passed to the parent | [D-123](../decisions.md#d-123-stop-and-writing-in-a-subagents-thread) |
| 8. A driver closes open `task` items when it can no longer follow them; the app keeps counting a quiet row and says so | [D-124](../decisions.md#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners) |
| 9. A driver says whether its harness tells subagents apart; where not, a subagent stays a `tool` item | [D-124](../decisions.md#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners) |
| 10. An app shows a held edit of an item sent before the member joined as the item | [D-124](../decisions.md#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners) |
| 11. The Codex transport's translators, held messages and interrupts for subagents | [D-125](../decisions.md#d-125-the-codex-transport-follows-subagents-threads) |
| 12. No separate MLS group for subagents | [D-126](../decisions.md#d-126-no-mls-group-for-subagents) |

Before Claude Code's or Codex's driver says a person can write to a subagent (point 7), that has to be recorded:
neither was tested.

## Still open

- **A thread's detail at a lower detail setting** (case 5), with
  [the Q-11 and Q-12 proposal](../open-questions.md#a-proposal-for-q-11-and-q-12-not-decided).
- **An app's setting for people's replies** and subagents' threads (case 12), for the apps.
