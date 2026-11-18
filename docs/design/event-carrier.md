# How an agent event is carried

Status: decided on 2026-11-17, as [D-022](../decisions.md#d-022-the-agents-words-are-ordinary-chat-messages)
to [D-026](../decisions.md#d-026-an-agent-item-is-at-most-64-kb). None of it is in the Marmot specification
yet, and all of it has to be agreed there
([Q-03](../open-questions.md#q-03-where-are-the-event-kinds-specified)). The exact tags are not fixed.

[The event model](event-model.md) says what an agent event means: a set of item types, each with a state. This page
says how one travels through a Marmot group: what kind of event it is, what it contains, and how an item that
changes is updated.

## What the carrier has to do

1. **Let an item change.** A tool call is sent when it starts and again when it finishes. The chat must show one
   row that updates, not two rows.
2. **Stay readable everywhere.** A client that knows nothing about agents should still show something sensible.
3. **Stay small enough.** Tool output can be large, and every event is stored on every member's devices.
4. **Alert people only when they are needed.** An approval that is waiting should reach a phone. A tool call
   should not.

## What Marmot gives us today

Read from the Marmot specification at commit `162c61b` and MDK at `a2d3486ac`, both on 2026-11-17.

| Fact | Consequence |
| --- | --- |
| An app event is an unsigned Nostr-shaped event: `kind`, `content`, `tags`, `created_at`, `pubkey`, and an `id` computed from them. The sender is proven by the group's encryption, not by a signature | An agent event is one of these; nothing new is needed underneath |
| The list of kinds is not a gate. A client must accept an event of a kind it does not know, and may choose not to show it | A new kind is allowed |
| Kinds 1201 and 1202 are reserved for agent activity, with no definition yet | They are the obvious numbers to use |
| An edit (kind 1009) points at an earlier event with one `e` tag and replaces its text. It is honoured only from the same author, never shows as a row of its own, and does not count as new activity | This is the mechanism for changing something already sent |
| An edit replaces only the text, and MDK applies one only to a chat message or a comment | To update an agent item, an edit must be allowed to target one and to replace its tags too |
| Competing edits are ordered by `created_at`, which counts whole seconds | Two updates in the same second have no defined winner |
| A client that has not yet received an edit's target may hold the edit or drop it | For agent items, dropping would leave an item stuck |
| MDK gives every 1201 and 1202 event its own row | That is right for an item's first event, and is why updates must not be sent as further 1201 events |
| Files and images travel as attachments on chat messages (kind 9) only | A large output cannot be attached to an agent event as things stand |
| Nothing is short-lived except live previews over the separate QUIC channel | Every agent event sent is kept as history |
| The spec sets no size limit on an app event | We have to choose one |

## The agent's words are ordinary chat messages

What the agent says to people is sent as a normal chat message, kind 9, exactly as a person's message is. It
carries one extra tag that names the turn it belongs to
([D-022](../decisions.md#d-022-the-agents-words-are-ordinary-chat-messages)).

Every client already shows these, and replies, reactions, search and alerts work without anyone doing anything.
An app that knows about agents can use the tag to group the message with the rest of its turn.

## Everything else is one kind: an agent item, kind 1201

Turns, tool calls, approvals, questions, plans, tasks, notices, errors and reasoning are all sent as kind 1201.
The item's type is a tag inside it, not a different kind
([D-023](../decisions.md#d-023-one-kind-carries-every-agent-item)).

One kind is right because the item types have the same shape. Every one carries the same parts: a type, a state,
a turn, a position, a harness, a plain rendering and the native message. They differ only in one part, the
type's own fields. If a later type needs a different shape, it gets its own kind then.

Kind 1202 carries what a person's app sends back, the answers and controls
([D-027](../decisions.md#d-027-answers-and-controls-are-one-kind-of-event-kind-1202)). See
[answers and controls](answers-and-controls.md).

## Who gets alerted

The agent decides, per event, whether it should alert people
([D-024](../decisions.md#d-024-who-an-agent-event-alerts)):

| Event | Alert? |
| --- | --- |
| The agent's final answer | Yes, as any chat message does |
| An approval or a question that is waiting | Yes |
| A turn that failed | Yes |
| Everything else: tool calls, reasoning, plans, tasks, notices, a turn starting or finishing normally | No |

MDK already treats 1201 and 1202 as their own class of event that triggers no alert, so "no" is today's
behaviour. The waiting approval is the case that needs adding. It matters because Codex and Claude Code wait for
an answer with no time limit.

A failed turn arrives as an edit of the turn item. Edits are otherwise silent, so this is the one edit that
alerts.

Whether the agent's remarks during a turn ("I'll run the tests now") should alert like a final answer is open.

## An item is updated with an edit

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/carrier-dark.svg">
  <img alt="An agent sends an item, then two edits that point at it. The chat shows one row, in place, holding the latest edit." src="../images/carrier-light.svg">
</picture>

The first event of an item is a kind 1201 event. To change the item, the agent sends an edit, kind 1009, that
points at that first event. This is the mechanism Marmot already has for changing a message
([D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit)).

- **The item is identified by the id of its first event.** No separate item id is needed. An answer to an
  approval points at it with an `e` tag, the way a reply points at a message.
- **Each edit carries the whole item**: the new text and the new tags. A client that missed an earlier edit loses
  nothing.
- **Only the agent that created an item can edit it.** Edits already require the same author.
- **The row stays where the item first appeared.** Edits already work this way.

Edits need three additions to do this. All three are changes to the specification.

| Addition | Why |
| --- | --- |
| An edit may target an agent item, and then its tags replace the item's tags as well as its text | An item's state and fields are in tags. Today an edit replaces text only, and only on a chat message |
| An edit of an agent item carries a version number, and the highest version wins | A tool can start and finish within one second. `created_at` cannot order those two |
| A client holds an edit whose item has not arrived yet, instead of dropping it | Otherwise an item can stay at "running" for good |

A client should also not mark an agent item as "edited". The item changing state is normal, not a correction.

### What this costs

An edit means nothing without the event it points at. Someone added to the group in the middle of a turn cannot
read the item's first event, so they see nothing of that item, however many edits follow. They see the agent's
answer, because that is a chat message, and every item that starts after they joined.

### The earlier draft, and why this replaced it

The first draft gave every item its own id and a version, and updated an item by sending a whole new 1201 event
with the same id. That is an edit under another name, with a second mechanism to specify, build and test. Edits
exist, and they behave better in apps that have not been updated: see
[what older apps show](#what-older-apps-show).

## What an agent item contains

| Part | Where | Example |
| --- | --- | --- |
| The plain rendering | `content` | `Ran a command: python3 greet.py (exit 0)` |
| Item type, and kind of tool | a tag | `tool`, `command` |
| State | a tag | `done` |
| Turn and session | tags | |
| Position within the turn | a tag | `6` |
| Harness and its version | a tag | `codex`, `0.169.0` |
| The message that started the turn | an `e` tag, as replies use | |
| The type's own fields | a tag holding JSON | the command, exit code, output |
| The harness's native message | a tag holding JSON | |

An edit of an item carries the same parts, with the `e` tag pointing at the item and a version number.

The plain rendering goes in `content` because that is the part of an event every client can read, and it is the
part an edit already replaces.

### A number gives the order within a turn

Each agent item carries its position within its turn: 1, 2, 3. Clients order a turn's items by it.

Without it, order is undefined. `created_at` counts whole seconds, and an agent often produces several items in
one second. MDK would then fall back to ordering by event id, which is effectively random.

## The size limit is 64 KB

Tool output is the part that can be large. These are sizes from four days of one developer's real use of Claude
Code; lengths only were measured, never content.

| | Typical | 9 in 10 are under | 99 in 100 are under | Largest |
| --- | --- | --- | --- | --- |
| A tool's output | 0.8 KB | 6 KB | 19 KB | 56 KB |
| A tool's input | 0.3 KB | 1.3 KB | 6 KB | 51 KB |
| A piece of the agent's text | 0.1 KB | 1 KB | 7 KB | 31 KB |
| A piece of reasoning | 0.3 KB | 0.9 KB | 3 KB | 13 KB |

How many of 16,841 tool outputs each limit would cut:

| Limit | Outputs cut |
| --- | --- |
| 16 KB | 1 in 70 |
| 32 KB | 1 in 1,000 |
| 48 KB | 1 in 10,000 |
| 64 KB | None |

Nothing in Marmot forces a small limit. The only thing that limits size is the relay:

- White Noise's own relays accept events up to 2 MB (read in MDK's deployment settings).
- Other relays accept less. 64 KB is a common default (from memory, not re-checked for this page).
- A group message grows by a third on its way to a relay, because the encrypted bytes are sent as base64.
- MDK does not send an event to a relay whose limit it would exceed. If it fits no relay of the group, the send
  fails.

Storage is not a constraint. Large outputs are rare enough that a limit of 64 KB instead of 16 KB adds
a few megabytes over days of heavy use.

**An agent item is at most 64 KB** ([D-026](../decisions.md#d-026-an-agent-item-is-at-most-64-kb)). That
carried every tool output measured.

The consequence is accepted on purpose: an item near the limit reaches a relay as about 86 KB, so a relay that
accepts only 64 KB will not take it. If a group uses only such relays, that item is not delivered. 64 KB is
not large, and relays should accept more.

When an item would be larger, the driver drops parts in this order and says so in the event:

1. the native message is left out, with its size noted;
2. the output is cut, keeping the start and the end, with the full length noted.

The native message usually repeats the output, so an item with both is about twice the size of its output. The
first step is therefore the common one: it applies to an output over about 30 KB, which is one in a thousand.

Getting the full output afterwards is a separate problem: fetch it from the agent on request, or attach it as a
file. Attachments exist only for chat messages today, so that needs work in the spec. It is the same question as
an agent sending a file ([Q-23](../open-questions.md#q-23-how-does-an-agent-send-a-file-to-the-chat)).

## The summary line

A turn is itself an item, so it can be edited. That gives the summary line from
[the detail-level proposal](../open-questions.md#a-proposal-for-q-11-and-q-12-not-decided) with no extra
mechanism: the turn item's text becomes "Working · ran 3 commands", then "Finished · ran 12 commands, edited 3
files", each an edit of the same item.

At a lower detail level, the agent would send only the turn item and not each tool call. How many edits of it to
send, and how often, is part of that product question.

## What older apps show

- **The agent's words:** shown normally, because they are chat messages.
- **Agent items:** an app built on today's MDK shows each item once, as a generic row, in its first state. It
  ignores the edits, because it applies edits only to chat messages. So a tool call stays at "running", and
  nothing appears twice. An app that hides unknown kinds shows nothing.

The White Noise apps are ours and would be updated together with this. Other Marmot clients would see the
agent's words and little else until they add support.

## Options considered

| Option | Why not |
| --- | --- |
| Send every item as a chat message and edit it | Every tool call would alert people and clutter clients that do not know about agents |
| Give each item its own id and version, and resend the whole item as a new event of the same kind | A second way to change an event, beside edits. Older apps would show every version as its own row |
| Send only what changed in each update | Smaller, but a client that misses one update shows the wrong thing from then on. Whole items are safe to miss |
| A separate kind for each item type | The types share one shape, so ten kinds would repeat the same rules ten times |
| A limit of 16 KB or 32 KB | It would fit a relay that accepts only 64 KB, but cut one output in 70, or one in 1,000 |
| Leave the agent's words in the new kind too | Clients that do not know the kind would show nothing of what the agent said |

## What it needs from the spec and from MDK

In the Marmot specification:

- kind 1201: its tags, the size limit, and what a client that does not support it should do;
- the three additions to edits, for agent items;
- the turn tag on chat messages.

In MDK:

- build and read agent items;
- apply edits to agent items, tags included, choosing the highest version;
- order a turn's items by their position number;
- decide alerts by item type and state;
- expose the result to the apps.

This is the part of the design that MDK keeps
([D-001](../decisions.md#d-001-connectors-live-in-this-repository)).

## Not covered here

**Answers and controls.** What a person's app sends back, such as "allow once" or "interrupt", has its own
page: [answers and controls](answers-and-controls.md).
