# CoreLifecycle counterexamples

TLC's traces for the configurations that must fail, written by `python3 tools/tla/check.py --counterexamples`.
Each becomes a simulator scenario when `marmot-agent-sim` exists (step 5 of
[the verification plan](../../../docs/design/verification.md#in-what-order)). The schedules below use the shared
action names. TLC does not print an action's arguments, so the outcome of each `Send` and `Lookup` is read from
the state that follows it, and is written here. All are on the small bounds: one chat `c1`, prompt `p1`,
control `r1`, session item `s1`, one down.

## `unfair-mdk-never-answers` (L1, `CoreLifecycle.unfair.cfg`)

MDK stops answering. Expected: L1 fails, since without `FairMdk` nothing promises a send goes out.

1. `CatchUpDone`, `Store(r1)`, `Ask(r1)`, `Save`: the control is pending, saved with its read mark.
2. `Deliver(r1)`, `Refuse(r1)`, `Save`: the driver refuses it; a notice that settles `r1` is queued and saved.
3. `Send(c1, ambiguous-lost)`: MDK's reply is lost and the notice did not go out; it is now looked for.
4. Then nothing: every `Lookup` fails (a stuttering step) and no send is accepted, for ever. `r1` is never settled
   in the chat.

## `duplicate-first-event` (S3, `CoreLifecycle.duplicate.cfg`)

The known limit until MDK has crash-safe tagged sends (`ExactLookup = FALSE`, `TokenDedupe = FALSE`).

1. `CatchUpDone`, `Emit(s1, {}, fin)`, `Save`: the item's first version is queued and saved.
2. `Send(c1, sent)`: MDK posts version 1 (marked as attempted, and saved, before MDK had it).
3. `SaveFails`: the save that records the send fails, and the core stops: the state file still holds version 1 in
   the outbox, attempted.
4. `Restart`: the send is to be looked for.
5. `Lookup(c1, absent, sent)`: the lookup does not find it (MDK published it and did not store it, or the core
   cannot tell it from an earlier one), so it is sent again, and MDK posts it again. The chat has two first
   events for one item.

With `TokenDedupe` (`CoreLifecycle.dedupe.cfg`) the second send gives back the first event, and S3 holds.

## `live-out-of-order` (S10, `CoreLifecycle.liveorder.cfg`)

Without `LiveInOrder`: MDK hands live messages over out of storage order.

1. `CatchUpDone`, `Store(r1)` (place 1), `Store(p1)` (place 2).
2. `Hear(p1)`: `p1` comes first; the read mark moves to 2 with `r1` unread. `run.rs` `handle` would then skip
   `r1`, whose place is at or before the mark.

## `give-up-leaves-item-open` (S2, `CoreLifecycle.giveup.cfg`)

A finding in the code, not a known limit: an item whose closing version is given up stays open in the chat. It
needs a send whose outcome is unknown on every try (`UnknownTries = 1` here; the code tries ten times) and that a
lookup does not find.

1. `CatchUpDone`, `Emit(s1, {}, open)`, `Save`, `Emit(s1, {}, fin)`, `Save`: versions 1 (open) and 2 (closing) are
   queued.
2. `Send(c1, sent)`, `Save`: version 1 is posted; the chat shows the item open.
3. `Send(c1, ambiguous-lost)`: version 2 did not go out, and the core does not know.
4. `Lookup(c1, absent, …)`: not found, and its tries are used up, so it is given up: `conversation.rs` `refused`
   drops it and queues a `failed` notice. `Save`.
5. The state file has the item closed and nothing queued for it, so nothing will close it in the chat, not even a
   restart (`recover` closes only open items). S2 fails here; L2 would fail too.

## `give-up-drops-settles` (L1, `CoreLifecycle.giveup-notice.cfg`)

A finding in the code: a notice that is given up takes the requests it settles with it. Same conditions as the one
above; there is no item, so that one cannot come first.

1. `CatchUpDone`, `Store(r1)`, `Ask(r1)`, `Save`, `Deliver(r1)`, `Refuse(r1)`, `Save`: a notice that settles `r1`
   is queued, and `r1` is no longer pending.
2. `Send(c1, ambiguous-lost)`: it did not go out, and the core does not know.
3. `Lookup(c1, absent, …)`: given up. `refused` drops a notice with nothing in its place, so `r1` is settled
   nowhere. `Save`.
4. Then nothing more happens: `r1` stays pending in every app for ever.
