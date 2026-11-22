# marmot-agent-sim

A deterministic simulator of the core: layer 3 of [the verification plan](../../docs/design/verification.md#3-a-deterministic-simulator).
It runs the real core (`marmot_agent::run::Core`) with a simulated MDK and a scripted harness, on tokio's paused clock,
from a seed, and checks the properties after every step.

## What is real and what is simulated

| Real | Simulated |
| --- | --- |
| The run loop, the conversation, the outbox, the save points, the state file on disk (in `/dev/shm` when there is one), the carrier that writes and reads items | MDK, behind the core's `marmot::Side` trait: storage with insert order, read a page at a time after a place as MDK's `messages_after_insert_order` gives it, live events in any order and sometimes again (MDK #3606), invitations, sends and their outcomes, the token MDK keeps each send under (a send tried again under it gets the same event, or the same drop, and posts nothing), reads of what arrived |
| The `no_effect` net, retries, how many dropped tries give a send up: production values, on a virtual clock | The harness, behind a `Driver`: sessions, turns, approvals, words, the effects of controls |

Not simulated: MDK's own failures beyond what the core can tell apart (busy, outcome unknown, refused, dropped
before it went out, a read that fails for a moment),
files the agent sends, a person without rights, group chats with mentions, and the socket connector. A connector that
drops mid-request needs real Unix sockets, whose readiness tokio's paused clock does not order, so it belongs to the
conformance suite (layer 2), which owns the socket.

## The properties

Checked from outside the core only: what the simulated MDK stored and refused, and what the harness was handed and
emitted (`src/oracle.rs`).

| Id | Checked | How |
| --- | --- | --- |
| S1 | every step | Each request is settled at most once, only in its own chat, and never by the `no_effect` net when the harness showed its effect before the net could first fire: 119 s after it took the request, which is when the driver answered the core (after a `Slow` move's time) and the core set its deadline, since the core waits `EFFECT_LIMIT_SECS` (120 s) on a clock of whole seconds and looks on a 10 s tick |
| S3 | every step | Each item's stored versions rise by one from v1, skipping only versions MDK refused (before the first stored one too); every later version names the first; the agent's words go out as a new message once |
| S4 | every step | No prompt reaches the harness twice, across every crash |
| S5 | every step | A chat id stands for one chat; an item, the agent's words and a settle reach only their own chat, and an edit of the agent's words edits their own message there |
| S10 | every step | What reaches the harness from a chat comes in storage order, each message once: a prompt by its text, an answer or a control as the request message the core read last, which it hands on before reading another |
| S12 | every step | A prompt names a session only on a driver's first prompt for the chat, only one the chat had, never one whose end was stored before the prompt reached the harness (by storage order) |
| L1, L2, L3 | at the end | Every chat joined and everything posted stored; every request settled once; every turn ended, the latest emission of each item of the running driver stored or refused, known by its `seq` (nothing waits in the outbox for good), no harness work left and no send still to land; every prompt handled; within 900 s of virtual time from the start of the fair part |
| S2 | at the end | Before the last stop, nothing an earlier driver left is open; after it, nothing at all |

The end is fair: faults stop, everything posted is stored and heard, a harness that hangs is restarted once, the
harness finishes its turns, a send that lands late lands, and the clock runs. By design, and so not counted as failures:

- A prompt whose delivery a crash cut off is lost (D-037: at most once, not at least once).
- A request with no target read more than five minutes late is dropped (D-028), also when it was read in time and a
  crash undid that before it was saved.
- A send MDK refuses, or that the core gives up after its tries, is answered once, and what answers it is the last: a
  request whose settling notice was dropped must be settled by the `failed` notice in its place, and an item whose
  closing version was dropped must be closed by the core's own close (`not_delivered`), unless the chat already showed
  it closed by the version before; only when that one is dropped too is the request or the item let off, since the core
  then takes the group as one MDK refuses everything for, as a gone one, and stops so nothing loops. The request is
  let off only by two different notices of its own chat, and the item only while the dropped close is the newest of
  its versions. These two were let off whenever the first send was refused, until the TLA+ model showed the core left
  them for good; with that limit removed, 80 of 1,200 gate runs failed (seed 0, 200 runs per family) until the core
  was fixed.
- The world sees a give-up from outside: a send whose last try came back unknown and stored nothing, when the core
  tries another send to the same chat instead (sends to a chat go one at a time, oldest first). It is recorded with
  the sends MDK refused.

A run also fails, with its schedule and trace, when the core's task panics, ends when no stop, driver end or failed
save explains it, or does not end cleanly after the last stop.

## Families

A run is named `family/vVERSION/SEED/INDEX`. The name makes the schedule (`src/schedule.rs`); a change to what a name
makes raises the family's version.

| Family | Faults | In the gate |
| --- | --- | --- |
| `calm` | none | yes |
| `crash` | `Crash` before any effect (a save, a send, a request to the driver, an accept), `SaveFails`, `Stop`, a send that goes out just before a crash | yes |
| `sends` | `Send` outcomes `Busy`, `Ambiguous` (gone out or not), `Refused`, `Rejected` (taken and dropped before it went out), crashes | yes |
| `live` | `Lag`, live messages that come late and in any order, `Reannounce` (MDK announces a stored message again, as its recovery may), `ReadFails`: a read of what arrived that fails for a moment (where a message's group is, the groups a catch-up lists, a page of what it reads) | yes |
| `driver` | `Refuse`, `Slow` (taken, and acted on, only after its time), `Hang` (the very next request, then a crash), `Ignore` (taken, never shown), `DriverStops` | yes |
| `mixed` | all of the above | yes |
| `disorder` | beyond MDK's contract: a message heard before it is stored | no: shows what the core does. Since the core reads every group from its mark on its tick, it loses nothing here (200 of 200 runs pass, seed 0, 2026-11-22) |
| `late` | a send of unknown outcome that lands 20 to 90 s later, while the core tries it again under its token; it found the accepted duplicate while the core looked sends up instead | yes |

The steps are named as the shared vocabulary names them (`Hear`, `Store`, `Ask`, `Reannounce`, `Emit`, `Send`, `ReadFails`,
`SaveFails`, `Crash`, `Restart`, `DriverStops`, `Stop`, `Tick`), and so is the trace, which adds what the core did:
`CatchUp`, `Deliver` with `Take` or `Refuse`, `Save`, the outcome of each `Send` (and, for a send tried again under a
token MDK keeps, `Sent before, same token`, `still to land, same token` or `Rejected again`), and each read that failed
(`Place`, `Groups` or `Stored`: `Error`). A read that finds nothing new, as most of the tick's do, is not traced.

## Running it

The crate needs Odin like the core (`AGENTS.md`, "Verification").

```sh
cargo test -p marmot-agent-sim --all-features          # the gate, the bug switches, replay
MARMOT_AGENT_SIM_RUNS=200 cargo test --locked --release -p marmot-agent-sim --all-features   # as CI
MARMOT_AGENT_SIM_SEED=42 MARMOT_AGENT_SIM_RUNS=2000 cargo test --release -p marmot-agent-sim --test seeds  # nightly

cargo run --release -p marmot-agent-sim -- run --family gate --seed 0 --runs 100 --out /tmp/sim
cargo run --release -p marmot-agent-sim -- show crash 0 7         # the schedule a name makes
cargo run --release -p marmot-agent-sim -- schedule FILE          # run a schedule, print its trace
```

`run` writes each schedule to `--out` before it runs, and removes it when the run passes; a run that hangs or panics
leaves it behind. A schedule that cannot be written stops `run` before that run, and a capsule that cannot be written
is reported and fails it. A failure leaves `NAME.capsule.json`: the schedule shrunk to a short one that fails the same
property, the original, the property and the trace. The seed tests write theirs under the target folder
(`target/tmp/marmot-agent-sim`), and CI keeps that folder as the `simulator-capsules` artifact of a failed run.

## Replaying a capsule

```sh
cargo run --release -p marmot-agent-sim -- replay path/to/NAME.capsule.json
```

It runs the capsule's schedule and says whether it fails the same property at the same step with the same trace, then
prints the trace. A capsule that puts a bug back needs `--features bugs`.

## Bug switches

With the `bugs` feature (which turns on `marmot-agent`'s `sim-bugs`), a schedule's `bugs` puts back a bug fixed before:
`save_after_send`, `retry_ambiguous_with_new_token` (a send whose outcome is not known is tried again under a new
token, which MDK cannot know for the send it has), `settles_only_from_items`, `mark_from_live` (a live message moves
the read mark to its place, as before MDK #3606, so one announced before a message stored earlier passes it). `tests/bugs.rs` shows the simulator finds
each. No release has the feature: the release builds `-p marmot-agent` alone, and nothing outside this crate calls
`Core::with_bugs`.

## Adding a family

1. Add the variant to `Family` in `src/schedule.rs`, with its name, and its weights in `Family::mix`; a new kind of
   fault needs a step in `Action`, a weight in `Mix`, and its handling in `src/sim.rs` (`apply`) and `src/world.rs`.
2. Add it to `Family::GATE` if it must pass on every pull request, and a test in `tests/seeds.rs`.
3. A change to what an existing name generates raises that family's `version`.
