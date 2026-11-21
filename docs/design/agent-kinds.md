# Kinds 1201 and 1202, as M1 used them

Status: decided on 2026-11-21, as [D-076](../decisions.md#d-076-the-tags-of-kinds-1201-and-1202) to
[D-078](../decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert), for the project to hand to MDK and
the Marmot specification; MDK may rename a tag to fit its conventions. How an agent event travels was decided
before ([the carrier](event-carrier.md), D-022 to D-027); the exact tags were not. This page fixes them, from what the first end-to-end test (M1 of the [plan](../plan.md)) sent and read back, under the
temporary kinds of [D-069](../decisions.md#d-069-until-mdk-takes-the-new-shape-agent-items-travel-under-a-temporary-kind).
The code that writes and reads them is `crates/marmot-agent/src/carrier.rs`. The section
[provisional additions](#provisional-additions) is a proposal, built and waiting for the owner.

## What M1 showed

M1 ran the core through MDK with a real Codex: a prompt, an approval, an interrupt, a new session, and a crash in
the middle of a turn ([the scenario](../../crates/marmot-agent/tests/codex.rs)). Every step worked with the shape
below. What it taught:

- **A version per item is enough to order updates.** The person's side kept the highest version of each item
  and never showed a stale state, even when two versions were sent within one second.
- **An update must carry the whole item.** A client that missed a version lost nothing.
- **Settling by a tag works.** Each answer and control was settled by the item version that named it, or by a
  notice saying why it had none. The notice needed a kind of its own, `refused`, which the event model lacked.
- **The 64 KB limit has to cover diffs, not only output.** Codex puts a file edit's whole diff in the item.
- **The agent's words as chat messages work,** but MDK's crash-safe way to send a chat message takes no tags, so
  the turn tag ([D-022](../decisions.md#d-022-the-agents-words-are-ordinary-chat-messages)) and crash-safe
  sending cannot both be had today.
- **A remark during a turn arrives as a chat message,** and alerted like the final answer. It no longer does
  ([D-078](../decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert)).

## Kind 1201: an agent item

The first version of an item is a kind 1201 event. Its content is the plain rendering: what a client that knows
nothing about agents shows.

| Tag | Holds | Always |
| --- | --- | --- |
| `v` | The version, from 1 | Yes |
| `state` | `running`, `waiting`, `done`, `failed`, `declined` or `cancelled` | Yes |
| `title` | A few words: "Running a command", "Allow this command?" | Yes |
| `harness` | The harness's name | Yes |
| `item_id` | The harness's own id for the item | Yes |
| `seq` | The event's place among its session's events, from 1 ([D-059](../decisions.md#d-059-eight-points-about-what-an-agent-event-carries)) | Yes |
| `fields` | The item type and its own fields, as JSON: `{"type": "tool", "body": {...}}` | Yes |
| `session` | The harness's session | When there is one |
| `turn` | The turn it belongs to | When there is one |
| `e` | On a turn: the chat message that started it | When there is one |
| `settles` | An answer or a control whose effect this version shows; one tag each ([D-028](../decisions.md#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)) | When there are any |
| `native` | The harness's own message: its kind, then the message itself | Unless left out |
| `left_out` | `native` and the size left out, when the item was over 64 KB ([D-057](../decisions.md#d-057-the-core-applies-the-64-kb-limit)) | When it happened |
| `cut` | `output` and its full length, or `diffs` and how many were cut | When it happened |

A notice the core makes itself, to say why an answer, a control or a message was not acted on, is an item of
type `notice` with the kind `refused`, a `reason` such as `not_allowed` or `busy`, and a `settles` tag for the
request it answers.

## Provisional additions

Built on 2026-11-21 while fixing the known gaps of [testing and verification](verification.md). These are
**provisional**: the owner has not accepted them, and they are not in `docs/decisions.md`.

- **The agent's words carry `settles`.** A chat message of the agent's words carries a `settles` tag for each
  answer or control whose effect it shows, as an item does. An edit of words cannot carry tags, so what the
  changed words settle is carried by the next version of their turn, the same as the last, sent right after the
  edit. So is what final words that come again unchanged name. If MDK refuses the edit, that version settles
  nothing, and those requests are settled as `failed`.
- **At most 200 `settles` on one event.** An item's version, or the agent's words, that would settle more goes
  out as several versions of the same item, the same and numbered on, each settling at most 200; words leave the
  rest to their turn's next versions. So every event that settles fits in 64 KB.
- **`no_effect`.** An answer the driver took whose item stopped waiting without naming it is settled at once by a
  `refused` notice with the reason `no_effect`, not left pending until the driver stops.
- **`failed` for a send MDK refuses.** A send MDK will not take, and is not an edit of words that were removed, is
  dropped. A `refused` notice with the reason `failed` tells the chat, and settles what the send settled. An
  edit of words whose message is gone goes out as a new message.
- **The `cut` tag.** One tag for each kind of field cut, with the full length of all that was cut of that kind:
  `text`, `title`, `command`, `error`, `input`, `plan`, `steps`, `choices`, `questions`, `description`. `output`
  keeps its full length and `diffs` its count, as before. A list thinned in its middle: `entries` or `changes`,
  with how many it held. A session's commands whose descriptions were dropped to fit: `descriptions`, with how
  many; every command keeps its name and hint
  ([D-045](../decisions.md#d-045-a-session-items-commands-keep-their-names-and-lose-descriptions-first)).
- **A stand-in for an item that cannot fit.** When what is never cut (ids, names, hints, values) is over 64 KB
  alone, the item's version goes out as a notice of kind `warning` with the reason `too_large`, keeping the
  item's id, state, version, first event and `settles`, and a `cut` tag `item` with the size of the event it
  stands in for. Nothing over 64 KB is sent: a send that would be, stand-in included, is refused before MDK
  sees it, and dropped with a `failed` notice.

## Updating an item

A later version is an edit (kind 1009), as decided
([D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit)). It names the item's first event with an
`e` tag, and carries the whole item again: content and every tag above, with a higher `v`. A client keeps the
version with the highest `v`, and holds an edit whose first event it has not received yet.

M1 sent each later version as a new event with an `item` tag naming the first, because MDK applies edits only to
chat messages and comments. The fields were the same.

## Kind 1202: an answer or a control

| Part | Holds |
| --- | --- |
| content | A plain rendering: "Allow once", "Stop" |
| `p` | The agent it is for. An agent ignores one that names another ([D-027](../decisions.md#d-027-answers-and-controls-are-one-kind-of-event-kind-1202)) |
| `e` | The first event of the item it acts on, when it names one |
| `details` | What it asks, as JSON, by its `action` |

| `action` | Other fields |
| --- | --- |
| `choice` | `choice`: the id of the approval's choice |
| `questions` | `questions`: one entry per question answered, each with `question`, `options` (ids) and `text` |
| `interrupt` | None. With no `e`, it acts on the chat's running turn |
| `new_session` | None |
| `set` | `setting` and `value` |
| `own` | `id`: one of the harness's own actions |

## The agent's words

A kind 9 chat message, as any person's, with one more tag: `["turn", <turn id>]`
([D-022](../decisions.md#d-022-the-agents-words-are-ordinary-chat-messages)). A remark made while the agent
works, which the harness marks as one, also carries `["phase", "commentary"]`, and does not alert
([D-078](../decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert)). When the words change after they
were sent, they are edited like any chat message.

## What MDK needs to do

- Accept kinds 1201 and 1202 in this shape from a program, in place of the old shape
  ([D-051](../decisions.md#d-051-kinds-1201-and-1202-take-the-new-shape-and-the-old-one-is-not-kept)), and refuse
  neither.
- Apply an edit to a kind 1201 event: replace its content and tags, keep the highest `v`, and hold an edit that
  arrives before its target.
- Show one row per item, at its first event, ordered within a turn by `seq`; and no row for a kind 1202 event.
- Alert for an approval or a question while it is `waiting`, and for a turn that ends `failed`; for nothing else
  of kind 1201 or 1202 ([D-024](../decisions.md#d-024-who-an-agent-event-alerts)). Do not alert for a chat
  message with `["phase", "commentary"]` (D-078).
- Let a chat message sent crash-safely carry tags, so the agent's words keep their turn and are never sent twice.
- Expose items, with their latest version, to the apps.

## Decided

1. The tags above, for kind 1201, its edits and kind 1202
   ([D-076](../decisions.md#d-076-the-tags-of-kinds-1201-and-1202)).
2. A `refused` notice for what the core did not act on
   ([D-077](../decisions.md#d-077-a-refused-notice-says-why-something-was-not-acted-on)).
3. The agent's remarks during a turn do not alert
   ([D-078](../decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert)).
