# Counterexamples

One file per configuration that must fail, each a schedule in its model's action names: `CoreLifecycle`'s,
which are the shared names of [the verification plan](../../../docs/design/verification.md); `SocketProtocol`'s
([below](#socketprotocol)); and `DriverTransport`'s, whose files start `driver-` ([below](#drivertransport)).
Nothing here is written by hand: `python3 tools/tla/check.py --counterexamples` runs TLC, reads its trace, and
writes each file with `tools/tla/schedule.py` (`tools/tla/schedule_driver.py` for `DriverTransport`). Without the
flag, `check.py` makes each schedule again and fails if it is not the committed one, so a schedule cannot drift
from the model. TLC prints a trace without the actions' arguments; `schedule.py` reads them, and the outcome of
each send, from what changed between one state and the next.

All are on the small bounds of their model's page: [`CoreLifecycle`](../README.md#configurations-and-results),
[`SocketProtocol`](../SocketProtocol.md#configurations-and-results) and
[`DriverTransport`](../DriverTransport.md#configurations-and-results). The rest of this page, up to
[`SocketProtocol`](#socketprotocol), is about `CoreLifecycle`'s.

## The format

```json
{
  "case": "...", "model": "CoreLifecycle", "config": "CoreLifecycle.<name>.cfg",
  "breaks": "S2", "what": "...", "constants": {...},
  "messages": [{"id": "r1", "chat": "c1", "kind": "control", "new_session": true}],
  "steps": [{"action": "Store", "msg": "r1"}, {"action": "Ask", "msg": "r1", "stored": true, "reads": "r1"}],
  "end": {"kind": "violation"}
}
```

- `steps` are the model's actions in order, with what each did: `Store(msg)`, `Hear(msg)` and `Ask(msg)`
  (`reads` names the message the chat's reader then handled, the oldest stored after the mark, or is `null`
  when there was none, as for a message heard before it is stored), `CatchUp(chat,
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
| [`live-out-of-order`](live-out-of-order.json) | S10 | With `MarkFromLive`, the core before MDK #3606: `p1` is stored before `r1`, but `r1` is handed over first; the core handles `r1` itself and the mark passes `p1`, which is then never read. The core now reads from the mark, which reads `p1` first; MDK does not promise storage order since #3606. Before, this was the case without the switch `LiveInOrder`, now removed |
| [`live-before-store`](live-before-store.json) | L3 | Without `LiveAfterStore` and without `TickCatchUp`: `p1` is heard before it is stored, so the reader finds nothing new; MDK does not hand it over again, and nothing reads it until the core starts again |
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

## SocketProtocol

`SocketProtocol` names each step in a variable of its own, so its schedules are its steps as they are:
`Connect`, `Hello`, `Hand` (`r`), `Take` (`r`), `Read` (`m`, the answer or event read: `k` is `resp` or `ev`),
`Run`, `EndTurn` (`hold`), `Release`, `Ask`, `Tick`, `Drop`, `GiveUp`, `Restart`, `Handoff`. The chat has one
prompt `p1`, whose turn is `t1`, and one control `r1`, an interrupt of it.

| File | Breaks | What it shows |
| --- | --- | --- |
| [`kept-connector-remembers`](kept-connector-remembers.json) | S2 | With `NoReset`, the Hermes plugin before the fix: the connection goes while the turn runs, the core closes it as cut short, and the turn's end that Hermes sends after it reconnects changes it to done |
| [`answer-on-effect`](answer-on-effect.json) | S11 | With `AnswerOnEffect`: the connector holds the interrupt's answer until the turn stops, and the core gives up a connector that was there and working |
| [`prompt-in-flight`](prompt-in-flight.json) | `UndeliveredNotRun` | The accepted limit of D-037: the connection goes before the core reads the prompt's answer, so the chat is told it may not have been delivered, and the harness runs it |

## DriverTransport

The same head (`case`, `model`, `config`, `breaks`, `what`, `constants`, `end`), with the core's requests as
`messages` (`{"id": "r1", "kind": "answer"}`, `interrupt` or `new_session`), and steps in
[the model's](../DriverTransport.md#what-is-modelled) action names, each with what it did:

- `Route(proc, reads)` and `DropWait(proc, reads)`: the driver reads the next line from a harness process
  (`h`, the one for every chat, or a chat's own Pi); `HarnessReads(proc, reads)`: the harness reads the next
  line the driver wrote. A line is `{"m": ...}` with its fields: `open` (a session to open, `resume` 0 for a new
  one, `r` the request it is for, or `prompt`), `opened`, `prompt`, `started`, `ended`, `ask`, `resolved`,
  `reply` (`how`: `answer`, `cancel` or `refuse`), `interrupt`, `edit`, `eof`.
- `HarnessAsks(ask, session, call, shown)` (session 0 is a session no chat has), `HarnessResolves(ask)`,
  `HarnessEndsTurn(session)`, `HarnessEdits(session)`, `HarnessDies(proc)`.
- `Pick(chat)`, `Prompt(chat)`, `Spawn(chat)`, `Restart`.
- `Answer(req, outcome, ask)`, `Interrupt(req, outcome)`, `NewSession(req, outcome, old_session)`, where the
  outcome is `taken` or `refused`.

A step lists the lines it wrote (`writes`) and the requests it settled (`settles`, each with how: `effect`,
`refused`, `failed`, or `wrong`, by an event that does not show its effect).

| File | Breaks | What it shows |
| --- | --- | --- |
| [`driver-unfair-harness-never-reads`](driver-unfair-harness-never-reads.json) | L1 | Without `FairHarness`: an interrupt is taken and written, and the harness never reads it, so the turn never ends and nothing settles it |
| [`driver-stray-while-picking`](driver-stray-while-picking.json) | L2 | Found in the ACP driver, and fixed in #122: a request from a session no chat has, read while a chat's session is picked, is held (`early`) and dropped unanswered once the pick is done. Now the defect `HeldDropped`, which puts the old way back |
| [`driver-codex-stray`](driver-codex-stray.json) | L2 | Found in the Codex driver, and fixed on 2026-11-23: a request for a thread no chat has is never answered. Now the defect `StrayUnanswered` |
| [`driver-codex-unshown`](driver-codex-unshown.json) | L2 | Found in the Codex driver, and fixed on 2026-11-23: a request it cannot show is never answered, and nothing in the chat waits on it. Now the defect `UnshownUnanswered` on Codex's shape |
| [`driver-reask-after-restart`](driver-reask-after-restart.json) | S13 | With `ReaskAfterRestart`: after a restart, ACP's driver names an approval for a tool call it named before, as the chat has seen |

Driving the code with them is step 5, as for `CoreLifecycle`'s: the conformance suite's fake harnesses would
play the `Harness…` steps, and the driver take the others.

## InstallUpgrade

[`InstallUpgrade`](../InstallUpgrade.md)'s files start `install-`. Each step names its action and the slot of the
run that took it (`slot`), with what the action says of itself: `Start(slot, harness)`, with the `agent` for
Hermes, `Lock`, `Begin` (`how`: `records`, `takes over`, `puts back another's`, `puts back another agent's` or
`already`), `Prepare`, `Publish`, `Core`, `Keep(part)`,
`Put(part)`, `Note` (the plugin change listed in the upgrade's record), `Connect` (`finishes`: a change a killed
run left), `Replace`, `StartServices`, `Commit`, `Tag`,
`End` (`change`: the plugin change moved into the record), `Retire`, `Done`; on a stop, `Fail(at)`, then
`PutBackPlugin` or `Trap`, `DropChange`, `Finish`, `PutBackChange(agent)` then `DropChange` for each plugin change
the record lists, then `PutBackNoChange`,
`PutBackCore`, `PutBackRemove(part)`, `PutBackMove(part)`,
`PutBackLeave(part)`, `PutBackParts`, `PutBackImage`, `PutBackRetire`, `Exit`; `Restore`; and `Kill(at)`.

| File | Breaks | What it shows |
| --- | --- | --- |
| [`install-image-put-back-apart`](install-image-put-back-apart.json) | U1 | Found in `install/lib.sh`, and fixed on 2026-11-24: Pi's install is stopped once the upgrade is done, and its exit trap puts the image tag back on the old image under the new scripts. Now the defect `ImageOutsideRecord` |
| [`install-plugin-change-ends-apart`](install-plugin-change-ends-apart.json) | U1 | Found in `install/lib.sh`, and fixed on 2026-11-24: Hermes's install is stopped between the end of the upgrade and the end of the plugin change, and puts the old plugin back over the new core. Now the defect `ChangeEndsApart` |
| [`install-plugin-put-back-apart`](install-plugin-put-back-apart.json) | U1 | Found in review, and fixed on 2026-11-24: Hermes's install is killed once our new plugin is in; Codex's install puts back the core and the support files, and leaves the plugin. Now the defect `ChangeOutsideRecord` |
| [`install-another-agents-change-left`](install-another-agents-change-left.json) | U1 | Found in review (#180), and fixed on 2026-11-25: Hermes agent a's install is killed once our new plugin is in a's Hermes; agent b's install takes the upgrade over and ends it with its own change only, leaving a's Hermes changed by a run that never ended, and a's record of the change under way. Now the defect `TakesOverAnother` |

## ActivityHoldBack

[`ActivityHoldBack`](../ActivityHoldBack.md)'s files start `hold-`. The steps: `Emit(item, shown)` (`shown`:
queued, not held back), `Flip(members)`, `ReadMembers(owners)`, `Send(tok, outcome)` with the outcome `sent`,
`pending`, `posted-lost`, `kept-lost`, `busy`, `held back`, `not kept`, `lost` (told as perhaps not delivered),
`refused` (MDK refused the token as bound to another submission), and, in the code before #194, `asked` and
`taken as new`; `Ask(tok, outcome)`, MDK asked what became of a send, with the outcome `went`, `queued`, `lost`,
`held back` or `not kept`; `MdkPosts(tok, members)`; and `Forget(tok)`, MDK letting go of a token whose send it
posted. A token is an item and a version.

| File | Breaks | What it shows |
| --- | --- | --- |
| [`hold-settled-twice`](hold-settled-twice.json) | H1 | Found in the core, and fixed on 2026-11-24: MDK posts the tool's closing version, which settles `r1`, and the answer is lost; someone joins; the core holds the version back and settles `r1` by the turn too. Now the defect `HoldAttempted` |
| [`hold-first-event-twice`](hold-first-event-twice.json) | H2 | The same defect: the tool's first version is posted and its answer lost, held back, and once the chat is the owner's again the tool's next version goes as a first event too |
| [`hold-retry-changed`](hold-retry-changed.json) | H1 | Found in review, and fixed on 2026-11-24: MDK posts the tool's closing version, handed with its native message, and the answer is lost; someone joins; the core tries it again without the native message, MDK refuses the token, and the core's notice settles `r1` again. Now the defect `RetryChanged` |
| [`hold-handed-again`](hold-handed-again.json) | H4 | Found in review, and fixed on 2026-11-25 (#194): the tool goes to MDK in the owner's chat and the answer is lost; someone joins; the core asks MDK whether it keeps the token, MDK lets go of it, and the core hands the tool again, which MDK takes as new and posts to the chat as it is now. Now the defect `HandAgain` |
| [`hold-forgot-held`](hold-forgot-held.json) | H5 | Found in review, and fixed on 2026-11-25 (#182): the tool goes to MDK in the owner's chat and the answer is lost; MDK lets go of its token; someone joins; asked, MDK has no such token, and the core holds the tool back as one MDK never took, where D-109 tells the chat it may not have been delivered. Now the defect `ForgotHeld` |
| [`hold-kept-posted-later`](hold-kept-posted-later.json) | H4 | The limit, accepted for the pilot ([D-148](../../../docs/decisions.md#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit)): MDK keeps a send of the tool handed while the chat was the owner's, someone joins, and MDK posts it to them |
