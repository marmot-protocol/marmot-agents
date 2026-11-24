# ActivityHoldBack: a TLA+ model of holding back the full activity

Status: built and checked with TLC on small bounds, as the verification page plans for the parts of the system
that face crashes and reordering (see [the inventory](inventory.md)). The model, its bounds and its switches are
internal and change as the work goes. Read on 2026-11-24 against master `7118005`, on branch
`t3/survey-and-model-state-machines`, which also fixes the two findings below. The limit it shows is accepted for
the pilot ([D-148](../../docs/decisions.md#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit),
[below](#what-the-model-found)).

`ActivityHoldBack.tla` models the core holding the agent's full activity back from a chat that is no longer the
owner's two-person chat with it
([D-136](../../docs/decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)),
while MDK may already hold what the core holds back: a send it keeps to post later (`Pending`), or one it posted
or kept whose answer the core lost. [`CoreLifecycle`](README.md) leaves this out: every chat there is the owner's,
and so is every chat of the simulator. It follows `crates/marmot-agent/src/conversation.rs` and `run.rs` `flush`.
Each result below is stated with its bounds; none of it says the core is verified.

## What is modelled

One chat in one group. The turn `T`, which every chat is sent, has its first version in the chat already. The
tool `K` is the agent's full activity. Both carry a native message, which a chat not sent the full activity is
sent without (`as_sent`). One request, `r1`, waits for what `K`'s closing version shows (an answer or a control
the driver took). The driver emits `K` running, `K` done settling `r1`, and `T` done. A send's token is its item
and version (`{key}@{version}`). `N` names the core's notices of a send it gave up, each a new item in the code.

| Action | In the code |
| --- | --- |
| `Emit` | `Conversation::emitted`: an event of the full activity in a chat the core last read as not the owner's is not queued, and what it settles is settled by the turn's next version (`settle_for_hidden`, `carrier_of`); anything else is queued as the item's next version (`queue_versions`), settling what it names (`take_pending`) |
| `Flip` | Someone is added to the chat, or leaves it: MDK no longer marks it, or marks it again, a two-person chat of the agent and the owner (`Marmot::two_people`) |
| `ReadMembers` | `members_now`, which `flush` calls before each send: the core reads who is in the chat (`owners_chat`); in a chat no longer sent everything, `hold_back` takes out of the outbox each send of the full activity (`full_activity_only`), but, since the fix, one MDK was handed (`tried_at`), and settles what it settled by the turn's next version |
| `Send` | `flush`: the oldest send, its first event filled in (`next_send`), noted as handed to MDK and saved (`attempting`), and handed to MDK under its token (`Marmot::send`); MDK posts it and answers (`sent`), keeps it to post later (`Pending`), posts or keeps it and the answer is lost (`Unknown`), or does not take it (`Busy`); tried again under the same token, MDK answers from what it keeps if it is handed what it was handed first, and refuses anything else as another submission, which the core gives up with a notice settling what the send settled (`give_up`, `refused`). Since the fixes, a send MDK was handed otherwise than the chat is sent now (`handed_otherwise`: the full activity, or an item with its native message or without) goes on only as far as MDK has it (`Side::keeps`, MDK's `local_send_status`), as MDK was first handed it (`Queued::bare`); one MDK does not have is held back then if it is the full activity, and else goes as the chat is sent now (`not_kept`) |
| `MdkPosts(t)` | MDK posts a send it kept, as it was handed, to whoever is in the chat then |

The members are read just before each send, as `flush` does: the model's `Send` needs the last read to be fresh.
Between that read and MDK taking the send, someone could still join; the model leaves that race out (the core
cannot close it).

## What is not

- **Crashes.** The outbox, the items and what is pending are saved; who is in a chat is not, and is read again
  before the first send after a restart, so a restart is a read like any other here. `CoreLifecycle` checks the
  saves and the tokens across crashes. Nor an outbox saved by an older core, whose sends tried before carry no
  record of how they went (`Queued::bare`): after someone joins, such a send is tried again changed, which MDK
  refuses, and its stand-in can settle its request a second time. Review found it, not the model; it is issue
  [#181](https://github.com/marmot-protocol/marmot-agents/issues/181).
- **MDK forgetting a token**, its record of a send expiring before the core asks again: the model's MDK keeps
  every token it took. The hold-back check (`not_kept`) then takes a send MDK posted and forgot for one it never
  took, and settles it with a `not_shown` notice, or by the turn, where
  [D-109](../../docs/decisions.md#d-109-how-the-cores-sends-survive-a-crash-as-built) tells it as failed,
  delivery uncertain. Review found it, not the model; it is issue [#182](https://github.com/marmot-protocol/marmot-agents/issues/182).
  MDK takes a token it has forgotten as a new send, so one forgotten between `Side::keeps` and the send would
  go out again, as it was handed, to the chat as it is now. `Side::keeps` therefore counts a token as not kept
  once MDK may forget it within a minute, by the group's disappearing time and the send's first try
  (`a_token_mdk_may_let_go_of_before_it_is_handed_again_is_not_kept`, `tests/marmot.rs`); such a send is then
  taken for one MDK never took, the case of #182.
- **Refusals and give-ups**, and what stands in for them, but for MDK refusing a token handed another submission:
  `CoreLifecycle`'s. That one is modelled as a notice settling what the send settled; the version that closes an
  item the core then sends in its place is left out.
- **The session as carrier** (when the chat has no turn) and the core's `not_shown` notice (when it has neither):
  the same move of what is settled, to another item.
- **The agent's words**, which every chat is sent, and a subagent's words, which are its full activity: their
  settles move as an item's do (`settle_by_turn`).
- **Gaps in an item's versions**: a version held back is never sent, so a chat that is the owner's again sees the
  item's next version after a gap. Apps keep the highest version
  ([D-025](../../docs/decisions.md#d-025-an-agent-item-is-updated-with-an-edit)), so a gap shows nothing wrong.

## Trusted assumptions

Besides the switches: MDK keeps each send under its token and answers a send tried again under it from what it
keeps, if it is handed what it was handed first, and refuses anything else (`local_submissions.rs`, which binds a
token to a hash of the request); MDK's `local_send_status` says whether it keeps a token; MDK posts what it keeps
as it was handed.

| Switch | When on | Shown when off, or on |
| --- | --- | --- |
| `MdkKeeps` | MDK may keep a send to post later, and answer `Pending` | on: MDK posts the full activity to someone who joined after the core handed it over (`hold-kept-posted-later`), the limit below, accepted for the pilot (D-148) |
| `FairMdk` | MDK eventually posts what it keeps | off: `r1` may never be settled in the chat (`hold-unfair`) |

## Properties

| Id | Property | Form | From |
| --- | --- | --- | --- |
| H1 | Every request is settled at most once in the chat (S1, for D-136) | invariant `H1` | D-028, D-136 |
| H2 | An item has one first event in the chat, and each later version names it (S3, for D-136) | invariant `H2` | D-025, D-136 |
| H4 | The full activity is posted only while the chat is the owner's two-person chat | invariant `H4` | D-136 |
| L1 | Every request is eventually settled in the chat | `<>` | D-028 |

## Configurations and results

Checked on 2026-11-24 with TLC v1.7.4 (Java 21, in the pinned image) by `tools/tla/check.py`, as the other models
are. The bounds: **one chat, `T` and `K`, one request, who is in the chat changes 3 times, and 2 of MDK's answers
are lost (or MDK is busy).**

| Case | Configuration | Expected | Distinct states | Counterexample |
| --- | --- | --- | ---: | --- |
| `hold-fair` | `ActivityHoldBack.fair.cfg` (MDK may keep a send; H1, H2, L1) | every property holds | 10,557 | |
| `hold-posted` | `ActivityHoldBack.posted.cfg` (MDK never keeps a send; H1, H2, H4, L1) | every property holds | 3,197 | |
| `hold-unfair` | fair, without `FairMdk` | L1 fails | 10,557 | |
| `hold-kept-posted-later` | fair, H4 | H4 fails | 88 | [`hold-kept-posted-later`](counterexamples/hold-kept-posted-later.json) |
| `hold-bug-HoldAttempted-H1` | posted, the code before the fix, H1 | H1 fails | 479 | [`hold-settled-twice`](counterexamples/hold-settled-twice.json) |
| `hold-bug-HoldAttempted-H2` | fair, the code before the fix, H2 | H2 fails | 2,621 | [`hold-first-event-twice`](counterexamples/hold-first-event-twice.json) |
| `hold-bug-RetryChanged-H1` | posted, the code before the second fix, H1 | H1 fails | 921 | [`hold-retry-changed`](counterexamples/hold-retry-changed.json) |

Every case of this model together takes under half a minute here.

**Each check catches its bug.** `HoldAttempted` and `RetryChanged`, the code before each fix (above); `NoMove`, a
send held back whose settles are dropped, not moved to the turn, fails L1 (7,969 distinct states); `HoldNothing`, no hold-back at all,
fails H4 even where MDK never keeps a send (59).

**No empty passes.** On the fair configuration: `NeverHeldBack` (a send of the tool is held back once the tool
went out), `NeverSettledByTurn` and `NeverSettledByTool` (`r1` is settled by each), `NeverOwnersAgain` (the tool
goes again once the chat is the owner's again), `NeverHeldAtSend` (a send MDK was handed and does not have is held
back at its send, the fix's path), `NeverKeptPosted` (MDK posts a tool it kept) and `NeverRetriedOtherwise` (the
turn is tried again as MDK was handed it, in a chat sent otherwise now, the second fix's path). All are reached
within 1,600 distinct states.

## What the model found

**A send MDK was already handed was held back as if it had never gone** (H1, H2). `hold_back` took every send of
the full activity out of the outbox, also one the core had handed to MDK, whose answer it did not have yet: MDK
kept it to post later (`Pending`), or posted it and the answer was lost. So, when someone joined the owner's chat
in that moment:

- what the send settled was settled again by the turn's next version, and the chat saw the request settled twice
  (`hold-settled-twice`, with MDK never keeping a send: one lost answer is enough);
- the item's first event was never known to the core, so once the chat was the owner's again, the item's next
  version went out as a first event too, and the chat showed the item twice (`hold-first-event-twice`).

**Fixed** (2026-11-24, this branch), first shown by the conversation's test
`a_send_mdk_was_handed_is_not_held_back_with_what_it_settles`: `hold_back` leaves a send MDK was handed
(`tried_at`) in the outbox; `flush`, coming to it in a chat that is no longer sent it, asks MDK whether it keeps
its token (`Side::keeps`) and tries it again under it only if MDK does, which posts nothing new; one MDK does not
have is held back then, as before (now `not_kept`). So the core never hands MDK the full activity for a chat
that is no longer sent it, as before the fix, and what MDK has goes on to the core's record of it.

**A send tried again was not the send MDK was handed** (H1, found in review, and then modelled). `next_send`
leaves an item's native message out for a chat that is not sent the full activity, by who is in the chat at each
try. MDK binds a token to a hash of what it was first handed, and refuses the token for anything else ("client
token is already bound to another submission"). So a send handed with its native message in the owner's chat,
whose answer was lost or which MDK kept to post, was tried again without it once someone joined: MDK refused it,
the core gave it up with a notice that settled its requests, and MDK still posted the original, which settled
them too (`hold-retry-changed`, the defect `RetryChanged`). The same held for an item every chat is sent, such as
a turn, before the first fix. **Fixed** (2026-11-24, this branch), first shown through the real MDK by
`a_send_mdk_keeps_is_tried_again_as_it_was_handed_after_someone_joins` (`tests/crashes.rs`) and in the
conversation by `a_send_is_tried_again_as_mdk_was_first_handed_it`: each send notes, at its first try under a
token, whether it went without its native message (`Queued::bare`), and goes so under that token from then on.
`flush` asks MDK whether it keeps a send handed otherwise than the chat is sent now (`handed_otherwise`); if not,
nothing binds the token, and it goes as the chat is sent now (`not_kept`). MDK offers no read of a kept send's
event before its engine takes it, so trying it again as it was handed is the only way to learn its event.

**The limit, accepted for the pilot** ([`hold-kept-posted-later`](counterexamples/hold-kept-posted-later.json),
H4; [D-148](../../docs/decisions.md#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit)):
a send MDK keeps to post later, which the core handed it while the chat was the owner's, MDK posts to whoever is
in the chat when it does, someone who joined since too. D-136 says a person added is sent none of what was queued
before they came. The core cannot take back what MDK keeps; it holds back everything it has not handed over. Only
MDK could do more, by withdrawing a queued send or refusing one queued before a member joined; the owner will ask
MDK for one of these, and the core will use whichever MDK offers. The same is true, more briefly, of the moment
between the core's read of the members and MDK taking a send. The fixes change neither.

## Running

```sh
python3 tools/tla/check.py --model ActivityHoldBack
tools/tla/run.sh formal/tla ActivityHoldBack.tla ActivityHoldBack.fair.cfg
```

CI runs this model's cases with `InstallUpgrade`'s, in the `TLA+ models (InstallUpgrade and ActivityHoldBack)`
job: in the merge queue and on `master` always, and on a pull request when it changes the core, or any other code
a model follows, `formal/`, `tools/tla/` or `.github/workflows/ci.yml`.
