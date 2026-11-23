# Kinds 1201 and 1202

Status: decided on 2026-11-21, as [D-076](../decisions.md#d-076-the-tags-of-kinds-1201-and-1202) to
[D-078](../decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert), for the project to hand to MDK and
the Marmot specification. MDK took this shape on 2026-11-22 (MDK pull request 3613), renaming one tag, which
D-076 allows: a turn's message is `started_by`, not `e`
([D-112](../decisions.md#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)).
The core has sent these kinds on MDK since. How an agent event travels was decided before
([the carrier](event-carrier.md), D-022 to D-027); the exact tags were not. This page fixes them, from what the
first end-to-end test (M1 of the [plan](../plan.md)) sent and read back, under the temporary kinds of
[D-069](../decisions.md#d-069-until-mdk-takes-the-new-shape-agent-items-travel-under-a-temporary-kind). The code
that writes and reads them is `crates/marmot-agent/src/carrier.rs`. What was
[accepted with the move to MDK](#accepted-with-the-move-to-mdk) is decided
([D-120](../decisions.md#d-120-the-choices-made-in-moving-to-kinds-1201-and-1202-as-built)). The section
[provisional additions](#provisional-additions) is a proposal, built and waiting for the owner. The tags of
[an item in a subagent's thread](#an-item-in-a-subagents-thread) were decided on 2026-11-23
([D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them)) and are not built.

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
- **The agent's words as chat messages work,** but MDK's crash-safe way to send a chat message took no tags, so
  the turn tag ([D-022](../decisions.md#d-022-the-agents-words-are-ordinary-chat-messages)) and crash-safe
  sending could not both be had. MDK 0.19.0 added tagged crash-safe sends, and the core has used them since
  2026-11-22.
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
| `started_by` | On a turn: the chat message that started it. The design named it `e`; MDK keeps `e` for an edit's target (D-112) | When there is one |
| `settles` | An answer or a control whose effect this version shows; one tag each ([D-028](../decisions.md#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)) | When there are any |
| `native` | The harness's own message: its kind, then the message itself | Unless left out |
| `left_out` | `native` and the size left out, when the item was over 64 KB ([D-057](../decisions.md#d-057-the-core-applies-the-64-kb-limit)) | When it happened |
| `cut` | `output` and its full length, or `diffs` and how many were cut | When it happened |
| `E`, `K`, `P` | NIP-22's root: the top-level subagent's `task` item, `1201`, and the agent. Decided, not built ([D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them)) | In a subagent's thread |
| `e`, `k`, `p` | NIP-22's parent: the `task` item of the subagent the item belongs to, `1201`, and the agent. Decided, not built (D-121) | In a subagent's thread, on the first event |

A notice the core makes itself, to say why an answer, a control or a message was not acted on, is an item of
type `notice` with the kind `refused`, a `reason` such as `not_allowed` or `busy`, and a `settles` tag for the
request it answers.

## Accepted with the move to MDK

Built on 2026-11-22 with the move to MDK's kinds, and accepted by the owner on 2026-11-23
([D-120](../decisions.md#d-120-the-choices-made-in-moving-to-kinds-1201-and-1202-as-built)).

- **The agent's words settle at most what MDK takes in a chat message** (2026-11-22). MDK takes a chat message
  with at most 64 tags, of at most 16 KB in all, and does not export those numbers; kind 1201 and its edits are
  held only to their shape and 64 KB. So the agent's words carry as many `settles` as fit beside their `turn`
  and, for a remark, their `phase`: 62 for a remark with a short turn id, 63 without `phase`, fewer only when
  the 16 KB is reached first. Their turn's next versions settle the rest. The core keeps MDK's two numbers
  itself (`carrier.rs`), and a test against MDK checks them.
- **A place in the session for the core's own notices** (2026-11-22). MDK reads a `seq` only from 1, so a
  notice the core makes itself (`refused`, `failed`) carries the highest `seq` its chat's items have shown, or 1
  when they have shown none, not 0 as before: it sorts after what came before it.
- **A notice about a message names it with `started_by`** (2026-11-22). The core's `refused` notice about a chat
  message it did not act on, a prompt from someone without the right or one the driver refused, names that
  message with the tag a turn names its message with, now `started_by`.
- **An item whose first event has disappeared** (2026-11-22). In a chat whose messages disappear, an item's
  first event can go while the item goes on, as a session's does. MDK then refuses an edit of it, as it no
  longer has the event nor the token it went under. The core sends that version as a new kind 1201 event
  instead, the item's first from then on, with its version going on rising; the chat is told of no failure. The
  agent's words whose message has disappeared go out as a new message, as when their message was removed. An
  item's first event that someone deleted, or that never reached the group, is not sent again: that version is
  dropped with a `failed` notice, as before.
- **An item begun under the temporary kind** (2026-11-22). A state file from a core that sent items under the
  temporary kind of D-069 is taken up once: MDK applies no edit to such an event, so each of its items' next
  version goes out as a new kind 1201 event, the item's first from then on, with its version going on rising. What
  the state file kept of forgotten items is dropped. A state file that holds an item or a notice already handed
  to MDK in the old shape is refused, and the core does not start: MDK keeps that try's token bound to the old
  event and would refuse it in the new shape, though it may have gone out. No core that sent the temporary kind was released.

## An item in a subagent's thread

Decided on 2026-11-23 ([D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them)); not built in the core, and MDK does not take it yet. Each subagent
is a thread in the agent's group, rooted at its `task` item, and every item in it carries NIP-22's tags as NIP-22
defines them, the same way a comment does:

- `["E", <id>, "", <agent>]`, `["K", "1201"]`, `["P", <agent>]`: the **top-level** subagent's `task` item, the
  same at any depth;
- `["e", <id>, "", <agent>]`, `["k", "1201"]`, `["p", <agent>]`: the **immediate parent**, the `task` item of the
  subagent the item belongs to. For a top-level subagent's own items it is the same item as the root.

A top-level `task` item is in the main chat and carries none of them. A subagent's words are kind 1111 comments
with the same tags ([D-122](../decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar)). The
whole shape is in [subagents as threads](subagents.md#how-it-travels).

Left for MDK to settle: an edit already uses `e` for its target, so it cannot also carry the parent `e`. The
recommendation is that an edit carries no thread tags and takes its thread from its target, since an item never
moves. MDK also has to take a parent `e` on an item's first event, which it refuses today, and to say whether a
`p` naming the event's own author is a mention ([what MDK has to do](subagents.md#what-mdk-has-to-do)).

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
  edit of words whose message is gone goes out as a new message. A send given up without knowing whether it
  went out, because MDK no longer knows its token once the group's messages have disappeared, is dropped the
  same way, and its notice's text says it may not have been delivered.
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
- **The core's close of an item whose closing version was dropped** (2026-11-22). When MDK refuses an item's
  closing version, or the core gives it up after its tries, and nothing later of the item is queued, the core
  sends the item's next version itself, so the chat does not show it open for good: the item's ids, its final
  state and the next version number, naming its first event, as a notice of kind `warning` with the reason
  `not_delivered`, the title "Not delivered" and a one-line text, with no native message, settling nothing.
  It is not sent when the chat already shows the item closed by an earlier version that went out. When a session
  or turn id, which the driver chose, would make it over 64 KB, it leaves them out; its item id, first event,
  version and state stay. When a notice that settled requests is dropped, or a `failed` notice that told of a
  dropped send, a `failed` notice takes its place, settling what it settled. The core's `failed` notices carry no
  session. Each of these is the last: if MDK refuses it too, as in a group that is gone, nothing more is sent for
  it.

## Updating an item

A later version is an edit (kind 1009), as decided
([D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit)). It names the item's first event with its
one `e` tag, and carries the whole item again: content and every tag above, with a higher `v`. What it carries
of a subagent's thread tags is open ([above](#an-item-in-a-subagents-thread)). A client keeps the
version with the highest `v`, and holds an edit whose first event it has not received yet. Only the sender of the
first event may edit it.

The core sends the first event with MDK's `submit_custom_event`, and each later version with
`submit_tagged_edit_for_local_send`, naming the token the first event went under; MDK places the `e` tag from it.
Both are crash-safe sends ([D-109](../decisions.md#d-109-how-the-cores-sends-survive-a-crash-as-built)). MDK keeps
one row per item, at its first event, holding the latest version's content and tags and the version it read
(`TimelineMessageRecord::agent_item`): an app reads the item from there, and need not follow the edits itself.

M1 sent each later version as a new event with an `item` tag naming the first, because MDK applied edits only to
chat messages and comments then. The fields were the same.

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

Done in MDK pull request 3613, on 2026-11-22, except where a point says otherwise. What MDK's review left over is
MDK issue 3612; none of it holds up the core.

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
  Done in MDK 0.19.0 (`submit_tagged_text`, `submit_tagged_edit_for_local_send`, `submit_custom_event`); a file
  sent crash-safely (`upload_media_with_client_token`) still carries no tags: that part is not done.
- Expose items, with their latest version, to the apps.
- Let a chat message carry up to 200 `settles` tags, as an item can. MDK still caps a chat message at 64 tags and
  16 KB of them. **Not done**, and no longer needed for the core to work: the agent's words now carry only as
  many as fit, and their turn settles the rest (see what was accepted with the move to MDK, above).

## Decided

1. The tags above, for kind 1201, its edits and kind 1202
   ([D-076](../decisions.md#d-076-the-tags-of-kinds-1201-and-1202)).
2. A `refused` notice for what the core did not act on
   ([D-077](../decisions.md#d-077-a-refused-notice-says-why-something-was-not-acted-on)).
3. The agent's remarks during a turn do not alert
   ([D-078](../decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert)).
4. What was [accepted with the move to MDK](#accepted-with-the-move-to-mdk)
   ([D-120](../decisions.md#d-120-the-choices-made-in-moving-to-kinds-1201-and-1202-as-built)).
5. The tags of an item in a subagent's thread, not built ([D-121](../decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them)).
