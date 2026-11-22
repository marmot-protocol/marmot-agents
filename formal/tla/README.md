# CoreLifecycle: a TLA+ model of the core

Status: built and checked with TLC on small bounds, as
[D-093](../../docs/decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top) and
[the verification page](../../docs/design/verification.md) plan. The model, its bounds and its switches are
internal and change as the work goes. Read on 2026-11-22 against master `74b56d6`, and again for the fix of
`conversation.rs` `refused` on branch `dropped-sends` the same day, and for its review round 1, and again for
the sends under MDK 0.19.0's tokens on branch `crash-safe-sends` the same day, which took the lookup's place, and
again for the reading by insertion order of MDK #3606 on branch `mdk-insertion-paging` the same day.

`CoreLifecycle.tla` models the core between MDK and one driver: requests pending and settled, items and their
versions, read marks, sessions, the outbox, saves and crashes. It owns S1, S2, S3, S4, S10, S12 and L1 to L3 for
the core.
Each result below is stated with its bounds; none of it says the core is verified.

## What is modelled

The actions are named as in the simulator (`marmot-agent-sim`, step 2 of the plan), so a trace from here can
drive it.

| Action | In the code |
| --- | --- |
| `Store(m)` | MDK stores a message for the agent (`insert_order`), and hands it to the event stream if the core runs and it was not handed over already |
| `Hear(p)`, `Ask(r)` | `run.rs` `live` for a live chat message or request, taken off the stream in any order (MDK #3606): it only wakes its chat's reader (`catch_up_group`), which handles the oldest message stored after the mark, if any (`handle`: `conversation.rs` `heard` or `asked`, and the read mark). Without `LiveAfterStore`, MDK may hand a message over before storing it, and the reader finds nothing new |
| `CatchUp(c)`, `CatchUpDone` | `run.rs` `catch_up`: what was stored after the mark, oldest first, at start; with `TickCatchUp` also at any idle moment while the core runs, as on its tick and when the event stream lags (`RecvError::Lagged`) |
| `Save`, `SaveFails` | `Store::save`, before each act and after a step; a failed save stops the core |
| `Deliver` | `carry_out_one`: a prompt or request to the driver, only once what led to it is saved |
| `Take`, `Refuse` | the driver's answer to the request it was handed (`prompted`, `requested`). A new-session control the driver takes marks the chat taken, as a prompt does, so a prompt after it names no session (D-065) |
| `Emit(i, S, fin)` | `emitted`: the driver's event for item `i`, settling `S`, closing it when `fin`; the core counts the version and queues it |
| `Tick(r)` | `overdue`: a taken control with no effect by its deadline is settled with `no_effect` |
| `Send(c, outcome)` | `flush` for the oldest send of a group, under its token (`marmot.rs` `send`), with what it carries saved first: MDK says `sent`, `busy`, `ambiguous-posted`, `ambiguous-lost`, `rejected` (it took the send and dropped it before it went out: up to `MaxRejected` tries; the next is under a new token, and after `RejectedTries` of them the send is given up), or `refused`: every send to a gone group, and up to `MaxRefusals` sends to a live one (`run.rs` `failed`: `EventTooLarge`, `InvalidAppMessagePayload`), each refused again whenever it is sent; the group stays live. With `TokenDedupe`, MDK answers a token it posted with the event it posted, and a token it dropped with its drop. A refused or given-up send goes to `refused`, below |
| (in `Send`) | `conversation.rs` `refused`, for a send MDK refused or the core gave up: it is dropped, and, unless it is itself a last resort (`last_resort`), something stands in for it (`stand_in_for`): for an item's version, the item's close when that version closed it and nothing later is queued (`close_undelivered`, a last resort), then a `failed` notice settling what the version settled; for a notice that settled requests, or a `failed` notice (`tells`), a `failed` notice settling what it settled, a last resort. A `failed` notice tells the chat of the version it stands in for, or of what the notice it replaces told of (`ToldBy`) |
| `Crash` | the process dies: memory, the event stream and the driver are lost |
| `DriverStops`, `Stop` | the loop ends: `ended` closes every open item and settles every pending request, all queued and saved before any is sent |
| `Restart` | `Core::new` and `recover`: open items closed, pending requests settled as failed, then catch-up; sends left queued go again under the tokens they were saved with |

`Queue` is not a step: the steps that put something in the outbox call it. How long the core waits before it
tries a send again is not modelled: a send may be tried again at once.

## What is not

- **Words and edits.** The agent's words, their edits, and the `settles` they hold for a turn's next version
  (`shown_by`) are not modelled. Every item is sent as versions. A model of the words would be a second model.
- **Answers.** Every request is a control (it has a deadline); in the configurations here, `r1` is a
  new-session control. An answer, settled when its item stops waiting, is not modelled; nor is a request refused
  at once (unknown item, not waiting, already answered). The new session the driver starts is not an item of its
  own: what matters to S12 is that the chat is taken once the driver took the control.
- **Rights, chats and people** (S5, S6): everyone may do everything. **Invitations**: every group is joined at
  start with mark 0.
- **Sizes and caps** (S7): `SETTLES_MAX`, the eight sends per group per turn, the cache of finished items
  (`KEPT`). The outbox is in order per group, and the core may send whenever it is idle. So the versions that
  spread what one event settles are not modelled, nor is what stands in for a dropped send leaving out a session
  id that is too large.
- **A later final version after a close that went out.** The core does not close an item again whose close went
  out (`closed_in_chat`). The model does not reach that case: a driver's item ends with the driver, and the
  versions that spread settles are not modelled (inferred from the model; the simulator reaches it, and its
  oracle allows it).
- **Time.** No clock: `Tick` may fire for any taken control, and a send may be tried again at once.
- **Files the agent sends** (D-090).

## Trusted assumptions

| Switch | When on | Shown when off |
| --- | --- | --- |
| `TokenDedupe` | MDK keeps each send under its token (MDK 0.19.0's `submit_*` and `local_send_status`): a send tried again under a token MDK posted gives back that event and posts nothing; one under a token MDK dropped is dropped again | the duplicate first event (`CoreLifecycle.duplicate.cfg`) |
| `LiveAfterStore` | MDK hands a live message over only once it has stored it | a message read only when the core starts again (`CoreLifecycle.livestore.cfg`), unless the core's tick reads it (`TickCatchUp`) |
| `LastResortTaken` | MDK never refuses a last-resort send to a live group: the core's close of an item, or a `failed` notice in place of a dropped one, is as small as the core makes anything | an item left open in the chat, the accepted limit (`last-resort-refused`, below) |

Besides the switches: a save is atomic (`Store::save` writes a new file and renames it); the driver goes when the
core goes (a built-in driver is its child, and a socket connector starts again with a new core); MDK keeps what
it stored across a crash; a refusal from MDK is for good (the group is gone, or MDK refuses those bytes again);
MDK keeps a send's token before it answers, so a token MDK posted under is one it knows when the core tries again
(with `TokenDedupe`).

`TokenDedupe` is read from MDK's code at 0.19.0 (`admit_local_message_with_draft_policy_at`: a token kept with
the same request gives back its message id, before anything else is checked; one rejected is refused), and the
core's tests against MDK show it (`crates/marmot-agent/tests/marmot.rs` and `crashes.rs`); MDK's own tests of it
were not read. `LiveAfterStore` is what MDK documents since #3606 (`MarmotAppEvent::MessageReceived`: ordinary
ingress commits a message before it announces it); MDK's tests of it were not read. MDK no longer promises that it
announces messages in storage order, or once: recovery can announce a stored message again, and an import can
store one with no announcement. The core does not rely on either: a live message only wakes its chat's reader,
which reads from the mark, so the model takes live messages in any order. A message announced again is not
modelled: the reader reads from the mark, so its notice can do no more than a `CatchUp`. Until MDK #3606 the
core moved the mark to a live message's place and relied on storage order (`LiveInOrder`, removed); the
deliberate defect `MarkFromLive` puts that back. `TickCatchUp` is what the core does (on its 10 s tick), and is
off in most configurations only to keep them small.

## Properties

| Id | Property | Form | From |
| --- | --- | --- | --- |
| S1 | Every request is settled at most once in its chat | invariant `S1` | D-028, D-070, D-087 |
| S2 | A stop or restart leaves nothing open; an item a chat shows open is open in the state file or has a later version queued there | invariant `S2` | D-058, D-070 |
| S3 | An item's versions rise by one (the only gaps are versions MDK refused or the core gave up) and every later version names the first event | invariant `S3` | D-025, D-069 |
| S3 | ...and the chat is told of each gap: every version dropped in a live group is eventually told of in a failed notice posted to the chat, unless it went out after all | `~>`, `S3Told` | D-025, D-069 |
| S4 | A prompt reaches the driver at most once, across crashes | invariant `S4` | D-037, D-070 |
| S10 | Everything up to a mark was handled, nothing twice, in memory and on disk | invariant `S10` | D-037 |
| S12 | A prompt names a session only when the driver took no prompt for the chat since it started, and never one the core saw end | invariant `S12` | D-065 |
| L1 | Every request stored is eventually settled in its chat | `~>` | D-028 |
| L2 | Every item open in the chat or in the core eventually closes in both | `~>` | D-058, D-070 |
| L3 | Every message stored is eventually handled, and that is saved | `~>` | D-037 |

The liveness properties hold under fairness, one strong-fairness condition per party over all its progress
steps: the core always; the driver (`FairDriver`) answers and closes open items; MDK (`FairMdk`) eventually
accepts a send, refuses one for good, or drops again one under a token it dropped; the clock (`FairTime`)
reaches deadlines. Faults (crashes, failed saves, stops, unknown outcomes, dropped tries) are bounded, so they
end. A condition over a party's steps together is weaker than one
per step, so a pass under it holds under the stronger assumption too.

## Configurations and results

Checked on 2026-11-22 with TLC v1.7.4 (Java 21, in the pinned image) by `tools/tla/check.py`: the cases that
must pass on 4 workers, those that must fail on one, so their traces and counts repeat (the unfair one's count
moves, since TLC checks liveness on a timer during the search; its trace does not). The small bounds, every pull
request: **one chat `c1`, one prompt `p1`, one control `r1` (a new-session control), the session item `s1` (2
events from the driver, so 3 versions with the core's close), 1 down (a crash, a failed save, a driver stop or a
stop), 1 send with an unknown outcome, no try dropped by MDK (in the fair configuration, one, and two dropped
tries give a send up), no refusal in a live group, MDK keeping each send under its token (`TokenDedupe`) and
taking every last-resort send (`LastResortTaken`), live messages taken in any order, and, in the fair
configuration, the core's tick, which reads every chat from its mark at any idle moment (`TickCatchUp`).** Each configuration says how it differs. A second chat multiplies
the states by about 60, since its activity interleaves freely with the first's, so it is left to the larger
bounds below.

| Case | Configuration | Expected | Distinct states | Counterexample |
| --- | --- | --- | ---: | --- |
| fair | `CoreLifecycle.fair.cfg` | every property holds, S3Told too | 278,139 | |
| unfair | `CoreLifecycle.unfair.cfg` (no `FairMdk`) | L1 fails | 25,338 and 26,531 in two runs | [`unfair-mdk-never-answers`](counterexamples/unfair-mdk-never-answers.json) |
| no token dedupe | `CoreLifecycle.duplicate.cfg` (no `TokenDedupe`, no unknown outcome) | S3 fails | 490 | [`duplicate-first-event`](counterexamples/duplicate-first-event.json) |
| live out of order | `CoreLifecycle.fair.cfg` with `MarkFromLive`, safety only | S10 fails | 58 | [`live-out-of-order`](counterexamples/live-out-of-order.json) |
| live before store | `CoreLifecycle.livestore.cfg` (no `LiveAfterStore`, no `TickCatchUp`; only `p1`) | L3 fails | 102 | [`live-before-store`](counterexamples/live-before-store.json) |
| live before store, tick | the same, with `TickCatchUp` | every property holds | 141 | |
| give up, item | `CoreLifecycle.giveup.cfg` (1 dropped try gives a send up) | every property holds | 325,927 | |
| give up, notice | `CoreLifecycle.giveup-notice.cfg` (the same, no item) | every property holds | 6,211 | |
| refused, item | `CoreLifecycle.refuse.cfg` (1 refusal in the live group) | every property holds | 126,865 | |
| refused, notice | `CoreLifecycle.refuse-notice.cfg` (1 refusal, no item) | every property holds | 2,779 | |
| refused twice | `CoreLifecycle.refuse.cfg`, 2 refusals, no prompt, `LastResortTaken` | every property holds, S3Told too | 16,018 | |
| last resort refused | the same, without `LastResortTaken`, safety only | S2 fails | 1,581 | [`last-resort-refused`](counterexamples/last-resort-refused.json) |
| last resort given up | `CoreLifecycle.refuse.cfg`, 1 refusal, 1 dropped try that gives a send up, no prompt, safety only | S2 fails | 2,314 | [`last-resort-given-up`](counterexamples/last-resort-given-up.json) |
| gone group | `CoreLifecycle.gone.cfg` (MDK refuses every send) | every property holds | 119,272 | |

The counterexamples are schedules in the shared action names, made from TLC's traces by a program;
[their README](counterexamples/README.md) says how, and what step 5 must add to drive the simulator with them.
Until the sends took MDK's tokens, the fair configuration assumed instead that a lookup always found a send MDK
posted (`ExactLookup`), and the duplicate first event was the case without it.

**Each check catches its bug.** Each defect is a value of the constant `Bug`; with it, TLC must report the
property named, on the fair configuration with only that property, unless the table says otherwise:

| `Bug` | What it breaks | Property | Distinct states to the violation |
| --- | --- | --- | ---: |
| `SaveAfterSend` | a send goes out before what queued it is saved | S1 | 14,779 |
| `SettleUnpending` | an event settles a request no longer pending | S1 | 15,915 |
| `RecoverKeepsOpen` | a restart leaves open items open | S2 | 191 |
| `FreshTokenOnRestart` | a restart tries each queued send under a new token, as if the tokens were not kept | S3 | 1,621 |
| `NewTokenResend` | a send whose outcome is unknown is tried again under a new token | S3 | 378 |
| `NoFailedNotice` | a version MDK refuses is dropped with no failed notice (on the refused configuration, no prompt, S3Told only) | S3Told | 5,594 |
| `NoRetell` | a dropped failed notice that settled nothing is not replaced: the code before review round 1 (on the refused configuration, 2 refusals, no prompt, S3Told only) | S3Told | 8,600 |
| `NoReplacement` | nothing stands in for a dropped send: the code before the fix (on the give-up configuration) | S2 | 2,601 |
| `NoReplacement` | the same (on the refused-notice configuration, L1 only) | L1 | 2,265 |
| `DeliverBeforeSave` | a prompt reaches the driver before its read mark is saved | S4 | 607 |
| `MarkNotLoaded` | a restart reads every group from its start | S10 | 506 |
| `CatchUpAnyOrder` | catch-up takes any stored message after the mark | S10 | 19 |
| `ResumeAlways` | every prompt names the saved session (with a second prompt, `p2`) | S12 | 28,545 |
| `KeepEndedSession` | a session that ended stays the chat's | S12 | 2,292 |
| `NewSessionUntaken` | a new-session control the driver took leaves the chat untaken, so a prompt before the new session names the old one | S12 | 9,723 |

**No empty passes.** Each probe says an interesting state is never reached, and TLC must find it false. On the
fair configuration: `NeverSettled` (a request is settled), `NeverSettledByEvent` (by the driver's own event),
`NeverResumed` (a prompt names the session after the core went down and started again), `NeverClosedByCore` (a
stop or restart closes an item in the chat), `NeverDedupedAfterCrash` (after the core went down, a send tried
again under its token gets back the event MDK posted), `NeverSentAfterDrop` (a send goes out under a new token
after MDK dropped a try of it), `NeverDeliveredInCatchUp` (a request reaches the driver during the catch-up after a restart),
`NeverPromptAfterNewSession` (a prompt reaches the driver while the driver has taken a new-session control and not
yet named it). On the gone configuration, `NeverDropped` (a version is refused). On the refused and give-up
configurations, with no prompt, `NeverTold` (a failed notice for a dropped version is posted) and
`NeverClosedUndelivered` (the core's close of an item whose closing version was dropped is posted). On the
refused-notice and give-up-notice configurations, `NeverResettled` (a failed notice in place of a dropped notice
that settled requests is posted). On the live-before-store configuration, `NeverDroppedEarly` (a message handed
over before it was stored is dropped, then stored). All seventeen are reached within 3,200 distinct states.

**Larger bounds**, safety only, on the same day with 8 workers and a 6 GB heap, the other constants as in the fair
configuration, before the sends took MDK's tokens (not run again since). Liveness at these sizes did not finish in 15 minutes with a 2 GB heap, so it is for a nightly run
with more memory, which is not set up yet:

| Bounds | Distinct states | Time |
| --- | ---: | ---: |
| one chat, `p1`, `r1`, items `t1` and `s1`, 2 downs | 8,366,684 | 2 min 3 s |
| two chats, `p1` and `r1` in `c1`, `r2` in `c2`, item `t1`, 1 down | 9,544,632 | 2 min 5 s |

All of `check.py` takes about 10 minutes here with 4 workers (the fair and give-up configurations take about 3½
each, since live messages are taken in any order); CI runs it on every pull request with 4 workers. For
review round 1 it took 24 minutes, on a machine busy with the simulator and the Rust tests at the same time. The
larger bounds were not run again for the fix: no send is refused or given up in them.

## What the model found

Two gaps in dropping a send, each reached two ways: when the core gives a send up after ten tries whose outcome
stays unknown and a lookup that does not find it (`run.rs` `flush`, then `give_up`), and when MDK refuses one
send to a group that stays live (`run.rs` `failed`: `EventTooLarge`, `InvalidAppMessagePayload`, then
`give_up`). Both end in `conversation.rs` `refused`. **Both are fixed** (2026-11-22, branch `dropped-sends`),
and the model follows the fix:

- **An item whose closing version is dropped stayed open in the chat** (S2, L2). `refused` dropped the version
  and queued a `failed` notice, but the item was already closed in the core, so nothing sent a close again, not
  even a restart: `recover` closes only items that are open. Now the core queues the item's next version, a
  close of its own, as a last resort.
- **A notice that is dropped took the requests it settles with it** (L1). `refused` dropped a notice with
  nothing in its place, and its requests were no longer pending, so they stayed pending in every app. Now a
  `failed` notice settles them in its place, as a last resort.

The give-up and refused configurations, which failed before, now pass every property, and the deliberate
defect `NoReplacement`, the code before the fix, fails S2 and L1 again. What stands in for a dropped send is the
last: nothing stands in for it in turn, so nothing loops in a group that refuses everything (the gone
configuration). Two limits follow, each a committed counterexample:

- **The accepted limit** (`last-resort-refused`): if MDK refuses the last resort too in a group that stays live,
  the item stays open in the chat (S2, L2), or the requests unsettled (L1). The passing configurations assume
  MDK does not (`LastResortTaken`), since what stands in is as small as the core makes anything. The same
  follows when the last resort is given up after its tries (`last-resort-given-up`: one refusal, then a give-up;
  `LastResortTaken` speaks only of refusals).
- **Untold gaps**, closed in review round 1: if MDK refused an item's version and then the `failed` notice that
  told of it, and that notice settled nothing, nothing told the chat again (S3Told failed). Now such a notice is
  replaced once too, as a last resort, and a failed notice that replaces one tells of what that one told of
  (`ToldBy`; before, the model counted only a failed notice naming a version, so a replacement did not count).
  `refuse-twice` now passes S3Told; `NoRetell` puts the old way back and fails it.

One case that is not a bug in the code: an event from the driver that ends a session, lost in a crash before it
is saved, is never seen, so the next prompt after the restart resumes that session (S12 speaks of the sessions the
core saw end). The driver went with the crash, and nothing could replay the event.

The review of the first version of this model found that it left out the new-session control: a driver that takes
one has ended the chat's session before it emits the new one, and `conversation.rs` `requested` marks the chat
taken then, so a prompt in between names no session. The code was right; the model now follows it, and
`NewSessionUntaken` shows S12 catches the omission.

## Running

```sh
python3 tools/tla/check.py                 # every case; logs and configurations in target/tla
python3 tools/tla/check.py --only fair     # one case
python3 tools/tla/check.py --counterexamples   # writes the committed schedules again, from TLC's traces
tools/tla/run.sh formal/tla CoreLifecycle.tla CoreLifecycle.fair.cfg   # TLC on one configuration
```

`tools/tla/run.sh` downloads TLC v1.7.4 (the release MDK pins, with the same SHA-256) into
`~/.cache/marmot-agents/tla` once, and runs it with the local Java, or, when there is none, in a pinned Eclipse
Temurin 21 image with Docker. `TLC_WORKERS` sets TLC's workers (default 1); `TLC_HEAP` its heap (default 2g).
CI runs `check.py` in the `TLA+ models` job on every pull request.
