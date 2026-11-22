# CoreLifecycle: a TLA+ model of the core

Status: built and checked with TLC on small bounds, as
[D-093](../../docs/decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top) and
[the verification page](../../docs/design/verification.md) plan. The model, its bounds and its switches are
internal and change as the work goes. Read on 2026-11-22 against master `74b56d6`.

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
| `Hear(p)`, `Ask(r)` | `run.rs` `handle` for a live chat message or request: `conversation.rs` `heard` or `asked`, and the read mark. Without `LiveAfterStore`, MDK may hand a message over before storing it: `order_of` gives nothing and the core drops it |
| `CatchUp(c)`, `CatchUpDone` | `run.rs` `catch_up`: what was stored after the mark, oldest first, at start; with `LagCatchUp` also when the event stream lags (`RecvError::Lagged`), at any idle moment |
| `Save`, `SaveFails` | `Store::save`, before each act and after a step; a failed save stops the core |
| `Deliver` | `carry_out_one`: a prompt or request to the driver, only once what led to it is saved |
| `Take`, `Refuse` | the driver's answer to the request it was handed (`prompted`, `requested`). A new-session control the driver takes marks the chat taken, as a prompt does, so a prompt after it names no session (D-065) |
| `Emit(i, S, fin)` | `emitted`: the driver's event for item `i`, settling `S`, closing it when `fin`; the core counts the version and queues it |
| `Tick(r)` | `overdue`: a taken control with no effect by its deadline is settled with `no_effect` |
| `Send(c, outcome)` | `flush` for the oldest send of a group: saved as attempted first, then MDK says `sent`, `busy`, `ambiguous-posted`, `ambiguous-lost` or `refused`: every send to a gone group, and up to `MaxRefusals` sends to a live one (`run.rs` `failed`: `EventTooLarge`, `InvalidAppMessagePayload`), each refused again whenever it is sent; the group stays live |
| `Lookup(c, outcome, …)` | `find_sent` for a send whose outcome is unknown: `found`, or `absent` and then given up after its tries or sent again at once |
| `Crash` | the process dies: memory, the event stream and the driver are lost |
| `DriverStops`, `Stop` | the loop ends: `ended` closes every open item and settles every pending request, all queued and saved before any is sent |
| `Restart` | `Core::new` and `recover`: sends handed to MDK are looked for, open items closed, pending requests settled as failed, then catch-up |

`Queue` is not a step: the steps that put something in the outbox call it. A lookup MDK cannot answer changes
nothing, so it is a stuttering step and is left out.

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
  (`KEPT`). The outbox is in order per group, and the core may send whenever it is idle.
- **Time.** No clock: `Tick` may fire for any taken control, and a lookup may come at once.
- **Files the agent sends** (D-090).

## Trusted assumptions

| Switch | When on | Shown when off |
| --- | --- | --- |
| `ExactLookup` | A lookup always finds a send MDK posted | the duplicate first event (`CoreLifecycle.duplicate.cfg`) |
| `TokenDedupe` | MDK keeps a send's token, posts it once, and answers a lookup by it | (off in the fair configuration; on, S3 holds without `ExactLookup`) |
| `LiveInOrder` | MDK hands live messages over in storage order | a message skipped (`CoreLifecycle.liveorder.cfg`) |
| `LiveAfterStore` | MDK hands a live message over only once it has stored it | a message dropped until the core starts again (`CoreLifecycle.livestore.cfg`) |

Besides the switches: a save is atomic (`Store::save` writes a new file and renames it); the driver goes when the
core goes (a built-in driver is its child, and a socket connector starts again with a new core); MDK keeps what
it stored across a crash; a refusal from MDK is for good (the group is gone, or MDK refuses those bytes again);
a send that went out is in MDK's storage when the core looks (with `ExactLookup`).

`LiveInOrder` and `LiveAfterStore` are inferred from MDK's code (`publish_app_runtime_summary` emits a sync's
stored messages, in order) and not checked against MDK's tests. The core drops a live message that is not stored
yet (`order_of` gives nothing), and skips one whose place is at or before the mark, so it relies on both.

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
accepts a send and answers a lookup; the clock (`FairTime`) reaches deadlines. Faults (crashes, failed saves,
stops, unknown outcomes) are bounded, so they end. A condition over a party's steps together is weaker than one
per step, so a pass under it holds under the stronger assumption too.

## Configurations and results

Checked on 2026-11-22 with TLC v1.7.4 (Java 21, in the pinned image) by `tools/tla/check.py`: the cases that
must pass on 4 workers, those that must fail on one, so their traces and counts repeat (the unfair one's count
moves, since TLC checks liveness on a timer during the search; its trace does not). The small bounds, every pull
request: **one chat `c1`, one prompt `p1`, one control `r1` (a new-session control), the session item `s1` (2
events from the driver, so 3 versions with the core's close), 1 down (a crash, a failed save, a driver stop or a
stop), 1 send with an unknown outcome, 2 tries, no refusal in a live group, and, in the fair configuration, a
live stream that may lag at any idle moment.** Each configuration says how it differs. A second chat multiplies
the states by about 60, since its activity interleaves freely with the first's, so it is left to the larger
bounds below.

| Case | Configuration | Expected | Distinct states | Counterexample |
| --- | --- | --- | ---: | --- |
| fair | `CoreLifecycle.fair.cfg` | every property holds, S3Told too | 125,644 | |
| unfair | `CoreLifecycle.unfair.cfg` (no `FairMdk`) | L1 fails | 15,000 to 25,000 | [`unfair-mdk-never-answers`](counterexamples/unfair-mdk-never-answers.json) |
| no crash-safe sends | `CoreLifecycle.duplicate.cfg` (no `ExactLookup`, no unknown outcome) | S3 fails | 548 | [`duplicate-first-event`](counterexamples/duplicate-first-event.json) |
| token dedupe | `CoreLifecycle.dedupe.cfg` (no `ExactLookup`, `TokenDedupe`, no lag) | every property holds | 102,702 | |
| live out of order | `CoreLifecycle.liveorder.cfg` (no `LiveInOrder`) | S10 fails | 56 | [`live-out-of-order`](counterexamples/live-out-of-order.json) |
| live before store | `CoreLifecycle.livestore.cfg` (no `LiveAfterStore`; only `p1`) | L3 fails | 115 | [`live-before-store`](counterexamples/live-before-store.json) |
| give up, item | `CoreLifecycle.giveup.cfg` (1 try) | S2 fails | 4,115 | [`give-up-leaves-item-open`](counterexamples/give-up-leaves-item-open.json) |
| give up, notice | `CoreLifecycle.giveup-notice.cfg` (1 try, no item) | L1 fails | 3,472 | [`give-up-drops-settles`](counterexamples/give-up-drops-settles.json) |
| give up, told | `CoreLifecycle.giveup.cfg`, no prompt, without S2 | S3Told holds | 14,089 | |
| refused, item | `CoreLifecycle.refuse.cfg` (1 refusal in the live group) | S2 fails | 1,905 | [`refuse-leaves-item-open`](counterexamples/refuse-leaves-item-open.json) |
| refused, notice | `CoreLifecycle.refuse-notice.cfg` (1 refusal, no item) | L1 fails | 2,239 | [`refuse-drops-settles`](counterexamples/refuse-drops-settles.json) |
| refused, told | `CoreLifecycle.refuse.cfg`, no prompt, without S2 | S3Told holds | 9,534 | |
| gone group | `CoreLifecycle.gone.cfg` (MDK refuses every send) | every property holds | 38,767 | |

The counterexamples are schedules in the shared action names, made from TLC's traces by a program;
[their README](counterexamples/README.md) says how, and what step 5 must add to drive the simulator with them.
The token-dedupe case runs without the lag, to keep the run short.

**Each check catches its bug.** Each defect is a value of the constant `Bug`; with it, TLC must report the
property named, on the fair configuration with only that property, unless the table says otherwise:

| `Bug` | What it breaks | Property | Distinct states to the violation |
| --- | --- | --- | ---: |
| `SaveAfterSend` | a send goes out before what queued it is saved | S1 | 2,969 |
| `SettleUnpending` | an event settles a request no longer pending | S1 | 12,385 |
| `RecoverKeepsOpen` | a restart leaves open items open | S2 | 187 |
| `NoAttemptedFlag` | a send handed to MDK is not saved as such; a restart sends it blindly | S3 | 778 |
| `BlindResend` | a send whose outcome is unknown is sent again without a lookup | S3 | 181 |
| `NoFailedNotice` | a version MDK refuses is dropped with no failed notice (on the refused-told configuration) | S3Told | 5,653 |
| `DeliverBeforeSave` | a prompt reaches the driver before its read mark is saved | S4 | 580 |
| `MarkNotLoaded` | a restart reads every group from its start | S10 | 485 |
| `CatchUpAnyOrder` | catch-up takes any stored message after the mark | S10 | 19 |
| `ResumeAlways` | every prompt names the saved session (with a second prompt, `p2`) | S12 | 22,616 |
| `KeepEndedSession` | a session that ended stays the chat's | S12 | 2,007 |
| `NewSessionUntaken` | a new-session control the driver took leaves the chat untaken, so a prompt before the new session names the old one | S12 | 7,897 |

**No empty passes.** Each probe says an interesting state is never reached, and TLC must find it false. On the
fair configuration: `NeverSettled` (a request is settled), `NeverSettledByEvent` (by the driver's own event),
`NeverResumed` (a prompt names the session after the core went down and started again), `NeverClosedByCore` (a
stop or restart closes an item in the chat), `NeverLookedUpAfterCrash` (after the core went down, a lookup takes
its found branch), `NeverDeliveredInCatchUp` (a request reaches the driver during the catch-up after a restart),
`NeverPromptAfterNewSession` (a prompt reaches the driver while the driver has taken a new-session control and not
yet named it). On the gone configuration, `NeverDropped` (a version is refused). On the refused-told and
give-up-told configurations, `NeverTold` (a failed notice for a dropped version is posted). On the live-before-store
configuration, `NeverDroppedEarly` (a message handed over before it was stored is dropped, then stored). All
eleven are reached within 3,000 distinct states.

**Larger bounds**, safety only, on the same day with 8 workers and a 6 GB heap, the other constants as in the fair
configuration. Liveness at these sizes did not finish in 15 minutes with a 2 GB heap, so it is for a nightly run
with more memory, which is not set up yet:

| Bounds | Distinct states | Time |
| --- | ---: | ---: |
| one chat, `p1`, `r1`, items `t1` and `s1`, 2 downs | 8,366,684 | 2 min 3 s |
| two chats, `p1` and `r1` in `c1`, `r2` in `c2`, item `t1`, 1 down | 9,544,632 | 2 min 5 s |

All of `check.py` takes about 3 minutes here; CI runs it on every pull request with 4 workers.

## What the model found

Two gaps in dropping a send, each reached two ways: when the core gives a send up after ten tries whose outcome
stays unknown and a lookup that does not find it (`run.rs` `flush`, then `give_up`), and when MDK refuses one
send to a group that stays live (`run.rs` `failed`: `EventTooLarge`, `InvalidAppMessagePayload`, then
`give_up`). Both end in `conversation.rs` `refused`. Neither is fixed here; both are for the owner:

- **An item whose closing version is dropped stays open in the chat** (S2, L2). `refused` drops the version and
  queues a `failed` notice, but the item is already closed in the core, so nothing sends a close again, not even
  a restart: `recover` closes only items that are open.
- **A notice that is dropped takes the requests it settles with it** (L1). `refused` drops a notice with nothing
  in its place, and its requests are no longer pending, so they stay pending in every app.

The `failed` notice for a dropped item version is posted (S3Told holds, with one refusal or one give-up).

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
