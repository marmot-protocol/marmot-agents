# marmot-agent-sim

A deterministic simulator of the core: layer 3 of [the verification plan](../../docs/design/verification.md#3-a-deterministic-simulator).
It runs the real core (`marmot_agent::run::Core`) with a simulated MDK and a scripted harness, on tokio's paused clock,
from a seed, and checks the properties after every step.

## What is real and what is simulated

| Real | Simulated |
| --- | --- |
| The run loop, the conversation, the outbox, the save points, the state file on disk (in `/dev/shm` when there is one), the carrier that writes and reads items | MDK, behind the core's `marmot::Side` trait: storage with insert order, live events, invitations, sends and their outcomes, lookups of a send |
| The `no_effect` net, retries, the wait before looking for a send: production values, on a virtual clock | The harness, behind a `Driver`: sessions, turns, approvals, words, the effects of controls |

Not simulated: MDK's own failures beyond what the core can tell apart (busy, outcome unknown, refused, lookup failed),
files the agent sends, a person without rights, group chats with mentions, and the socket connector. A connector that
drops mid-request needs real Unix sockets, whose readiness tokio's paused clock does not order, so it belongs to the
conformance suite (layer 2), which owns the socket.

## The properties

Checked from outside the core only: what the simulated MDK stored and refused, and what the harness was handed and
emitted (`src/oracle.rs`).

| Id | Checked | How |
| --- | --- | --- |
| S1 | every step | Each request is settled at most once, only in its own chat, and never by the `no_effect` net when the harness showed its effect within 30 s of taking it |
| S3 | every step | Each item's stored versions rise by one, skipping only versions MDK refused; every later version names the first; the agent's words go out as a new message once |
| S4 | every step | No prompt reaches the harness twice, across every crash |
| S5 | every step | A chat id stands for one chat; an item and a settle reach only their own chat |
| S10 | every step | What reaches the harness from a chat comes in storage order |
| S12 | every step | A prompt names a session only on a driver's first prompt for the chat, only one the chat had, never one that ended |
| L1, L2, L3 | at the end | Every request settled once; every turn ended, and everything the running driver emitted seen by the chat in its last state (nothing waits in the outbox for good); every prompt handled; within 900 s of virtual time |
| S2 | at the end | Before the last stop, nothing an earlier driver left is open; after it, nothing at all |

The end is fair: faults stop, everything posted is stored and heard, a harness that hangs is restarted once, the
harness finishes its turns, and the clock runs. By design, and so not counted as failures:

- A prompt whose delivery a crash cut off is lost (D-037: at most once, not at least once).
- A request with no target read more than five minutes late is dropped (D-028), also when it was read in time and a
  crash undid that before it was saved.
- A request settled only by a core notice that MDK refused for good stays unsettled: the core drops a refused notice so
  nothing loops.
- An item MDK refused a version of may stay as the chat last saw it.

## Families

A run is named `family/vVERSION/SEED/INDEX`. The name makes the schedule (`src/schedule.rs`); a change to what a name
makes raises the family's version.

| Family | Faults | In the gate |
| --- | --- | --- |
| `calm` | none | yes |
| `crash` | `Crash` before any effect (a save, a send, a request to the driver, an accept), `SaveFails`, `Stop`, a send that goes out just before a crash | yes |
| `sends` | `Send` outcomes `Busy`, `Ambiguous` (gone out or not), `Refused`, `Lookup` errors, crashes | yes |
| `live` | `Lag`, live messages that come late | yes |
| `driver` | `Refuse`, `Slow`, `Hang` (then a crash), `Ignore` (taken, never shown), `DriverStops` | yes |
| `mixed` | all of the above | yes |
| `disorder` | beyond MDK's contract: a message heard before it is stored, or out of order | no: shows what the core does |
| `late` | beyond the core's assumption: a send of unknown outcome lands after the core looked for it | no: must find the accepted duplicate |

The steps are named as the shared vocabulary names them (`Hear`, `Store`, `Ask`, `Emit`, `Send`, `Lookup`,
`SaveFails`, `Crash`, `Restart`, `DriverStops`, `Stop`, `Tick`), and so is the trace, which adds what the core did:
`CatchUp`, `Deliver` with `Take` or `Refuse`, `Save`, the outcome of each `Send` and `Lookup`.

## Running it

The crate needs Odin like the core (`AGENTS.md`, "Verification").

```sh
cargo test -p marmot-agent-sim --all-features          # the gate, the bug switches, replay
MARMOT_AGENT_SIM_RUNS=200 cargo test --release -p marmot-agent-sim --all-features --test seeds   # as CI
MARMOT_AGENT_SIM_SEED=42 MARMOT_AGENT_SIM_RUNS=2000 cargo test --release -p marmot-agent-sim --test seeds  # nightly

cargo run --release -p marmot-agent-sim -- run --family gate --seed 0 --runs 100 --out /tmp/sim
cargo run --release -p marmot-agent-sim -- show crash 0 7         # the schedule a name makes
cargo run --release -p marmot-agent-sim -- schedule FILE          # run a schedule, print its trace
```

`run` writes each schedule to `--out` before it runs, and removes it when the run passes; a run that hangs or panics
leaves it behind. A failure leaves `NAME.capsule.json`: the schedule shrunk to a short one that fails the same
property, the original, the property and the trace. The seed tests write theirs under the target folder
(`target/tmp/marmot-agent-sim`).

## Replaying a capsule

```sh
cargo run --release -p marmot-agent-sim -- replay path/to/NAME.capsule.json
```

It runs the capsule's schedule and says whether it fails the same property at the same step with the same trace, then
prints the trace. A capsule that puts a bug back needs `--features bugs`.

## Bug switches

With the `bugs` feature (which turns on `marmot-agent`'s `sim-bugs`), a schedule's `bugs` puts back a bug fixed before:
`save_after_send`, `retry_ambiguous_blindly`, `settles_only_from_items`. `tests/bugs.rs` shows the simulator finds
each. No release has the feature: the release builds `-p marmot-agent` alone, and nothing outside this crate calls
`Core::with_bugs`.

## Adding a family

1. Add the variant to `Family` in `src/schedule.rs`, with its name, and its weights in `Family::mix`; a new kind of
   fault needs a step in `Action`, a weight in `Mix`, and its handling in `src/sim.rs` (`apply`) and `src/world.rs`.
2. Add it to `Family::GATE` if it must pass on every pull request, and a test in `tests/seeds.rs`.
3. A change to what an existing name generates raises that family's `version`.
