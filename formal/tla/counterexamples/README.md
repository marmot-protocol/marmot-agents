# CoreLifecycle counterexamples

One file per configuration that must fail, each a schedule in the shared action names of
[the verification plan](../../../docs/design/verification.md). Nothing here is written by hand:
`python3 tools/tla/check.py --counterexamples` runs TLC, reads its trace, and writes each file with
`tools/tla/schedule.py`. Without the flag, `check.py` makes each schedule again and fails if it is not the
committed one, so a schedule cannot drift from the model. TLC prints a trace without the actions' arguments;
`schedule.py` reads them, and the outcome of each send, from what changed between one state and the next.

All are on the small bounds of [the model's README](../README.md#configurations-and-results).

## The format

```json
{
  "case": "...", "model": "CoreLifecycle", "config": "CoreLifecycle.<name>.cfg",
  "breaks": "S2", "what": "...", "constants": {...},
  "messages": [{"id": "r1", "chat": "c1", "kind": "control", "new_session": true}],
  "steps": [{"action": "Store", "msg": "r1"}, {"action": "Ask", "msg": "r1", "stored": true, "handled": true}],
  "end": {"kind": "violation"}
}
```

- `steps` are the model's actions in order, with what each did: `Store(msg)`, `Hear(msg)` and `Ask(msg)`
  (`handled` says whether the core read it: a message heard before it is stored is dropped), `CatchUp(chat,
  msg)`, `CatchUpDone`, `Deliver(msg, kind, session)`, `Take(msg)`, `Refuse(msg)`, `Emit(item, settles, fin)`,
  `Tick(req)`, `Send(chat, send, outcome)` with the outcome `sent`, `busy`, `ambiguous` (it did not go out),
  `ambiguous_landed` (it did), `refused`, or `rejected` (MDK took it and dropped it before it went out, and the
  send `gives_up` when that was its last try), `Save`, `SaveFails`, `Crash`, `Restart`, `DriverStops`, `Stop`. A
  `send` names an item's version, or a notice and what it is about, carries `"last_resort": true` when it stands
  in for a dropped send, and `"try": n` when MDK dropped `n` of its tries before, so it goes under a new token.
- `end` says how the trace ends: `violation` (the invariant `breaks` names is false after the last step),
  `stutter` (nothing happens again, for ever) or `loop` (from step `from_step` on, the steps repeat for ever).

## The cases

| File | Breaks | What it shows |
| --- | --- | --- |
| [`unfair-mdk-never-answers`](unfair-mdk-never-answers.json) | L1 | Without `FairMdk`: a send is queued, and MDK never takes another step. The send stays enabled and is never taken (it does not fail), so the notice that settles `r1` is never posted |
| [`duplicate-first-event`](duplicate-first-event.json) | S3 | Without `TokenDedupe`, as before MDK 0.19.0's token-based sends: MDK posts a first version, the save after it fails, and after the restart the version is tried again, which MDK does not know for the one it posted, so it goes again |
| [`live-out-of-order`](live-out-of-order.json) | S10 | Without `LiveInOrder`: `p1` is stored before `r1`, but `r1` is handed over first; the mark passes `p1`, which `run.rs` `handle` then skips |
| [`live-before-store`](live-before-store.json) | L3 | Without `LiveAfterStore`: `p1` is heard before it is stored, so `order_of` gives nothing and it is dropped; MDK does not hand it over again, and nothing reads it until the core starts again |
| [`last-resort-refused`](last-resort-refused.json) | S2 | The accepted limit, without `LastResortTaken`: MDK refuses an item's closing version in a live group, and then the core's close that stands in for it (`"last_resort": true`). Nothing stands in for a last resort, so the item stays open in the chat |
| [`last-resort-given-up`](last-resort-given-up.json) | S2 | The same limit reached another way, with `LastResortTaken`, which speaks only of refusals: MDK refuses an item's closing version, and MDK takes the core's close that stands in for it and drops it before it goes out, which gives it up after its tries. The item stays open in the chat |

The four schedules of the gaps the model found in dropping a send (`give-up-leaves-item-open`,
`give-up-drops-settles`, `refuse-leaves-item-open`, `refuse-drops-settles`) are gone: the code was fixed on
2026-11-22, and those configurations now pass. The deliberate defect `NoReplacement`, the code before the fix,
shows the check still catches them. So is `refuse-twice-untold`, a gap left untold when MDK refused the `failed`
notice that told of it: since review round 1 such a notice is replaced too, and `NoRetell` puts the old way back.

## Driving the simulator with them (step 5)

The simulator's schedule (`crates/marmot-agent-sim/src/schedule.rs` on the `core-simulator` branch) uses the
same names, but most of its steps arm what happens next rather than say what happened, and the core takes its
own steps. These files cannot be fed to it as they are. Trace validation, step 5, has to add:

- **Messages by number.** Model ids (`p1`, `r1`) become indices into its `messages`; a new-session control is
  `What::NewSession`, a prompt `What::Prompt { approval: false }`.
- **The core's own steps as expectations.** `CatchUp`, `CatchUpDone`, `Save` and `Deliver` are things the core
  does, not things a schedule makes happen. Step 5 has to record them from the
  run and check them against the model's steps, in order.
- **An `Emit` that names its item, what it settles and whether it closes it**, where the simulator's `Emit`
  picks from the driver's script by number.
- **A `Send` that names the send it meets.** The simulator's `Send { outcome }` arms the next send, whatever it is.
  `sent` is the default (no step); `busy`, `ambiguous`, `ambiguous_landed`, `refused` and `rejected` map to its
  outcomes. A send tried again under a token the simulated MDK keeps meets no armed outcome, as in the model.
- **The number of tries.** The model gives up after one or two dropped tries; the code's `rejected_sends` knob, and
  the simulator's use of `REJECTED_TRIES`, must match.
- **`Tick(req)`** as a `Tick { ms }` past the 120-second no-effect limit.
- **An unfair end.** A `stutter` or `loop` end means MDK, or another party, never takes the step that would end
  it. The simulator always runs a fair tail; step 5 needs a schedule that stops MDK answering, and an oracle that
  then reports the property `breaks` names.
