# Testing and verification

Status: proposal, 2026-11-21, for the project owner. Nothing on this page is decided yet. The questions for the
owner are [at the end](#for-the-owner-to-decide). It comes from reading all the code after M3 and the M4 work in
progress, and from how MDK tests itself (its simulator, its TLA+ and Tamarin models, and its testing rules).

## Why

All testing today is by example: each recording replayed against its expected events, hand-written cases,
scripted stand-in harnesses, and one ignored scenario per harness against the real thing. That caught the first
bug of each kind. Review then found the rest, and most of what it found were **interleavings**: a request and its
reply crossing, a dropped wait cutting a write in half, a restart racing a pending request, chat A's answer
reaching chat B. Reviews of four drivers did not converge, because each round found a narrower interleaving of the
same machinery. Examples do not find those. A model checker or a simulator that tries every order does.

Reading the core for this plan also found gaps that no test covers. They are listed under
[first, fix what is known](#first-fix-what-is-known).

## How we work

These rules come from MDK's own testing rules, which this project adopts:

- **One owner per contract**, at the cheapest layer that runs the real code. A property is proven or tested in
  one place, and the others point to it.
- **A check must be shown to catch its bug.** Put the defect back, and the check fails for the stated reason. A
  test that has never failed proves nothing.
- **No empty passes.** A model check also checks that the states it says are safe are reachable.
- **A bounded result is stated with its bounds.** "Checked for 3 chats, 4 requests and 2 crashes" is honest;
  "the core is verified" is not, and is never said.
- **Names match across layers**, so a property, its model, its test and its mutant can be found with one search.

## What must hold

The properties come from the decisions. Each gets an id that the models, tests and mutants use.

**Safety** (never happens):

| Id | Property | From |
| --- | --- | --- |
| S1 | Every answer and control is settled exactly once: by the event that shows its effect, by a refusal, by the core's `no_effect` net, or as failed when the driver or core stops | D-028, D-070, D-087 |
| S2 | No item is left open after the driver stops or the core restarts | D-058, D-070 |
| S3 | An item's versions rise by one, and every later version names its first event | D-025, D-069 |
| S4 | A prompt reaches the harness at most once, across any crash | D-037, D-070 |
| S5 | An event, an answer or a file reaches only the chat it belongs to | D-060 |
| S6 | Nobody prompts, approves or sets something without the right to, in any form the harness reads | D-010, D-032, D-047, D-084 |
| S7 | Nothing sent is over 64 KB, and only the core cuts | D-026, D-057 |
| S8 | No log, debug output or recording holds message text, model output, keys, identifiers or paths | D-071 |
| S9 | No file outside the allowed folders is ever read or sent | D-090 |
| S10 | Catch-up delivers every message once, in storage order, with no gaps | D-037 |
| S11 | A request is handed to the harness before the driver returns, and dropping a wait never loses or halves anything | D-066 |
| S12 | A prompt names a session only to resume it, never an ended one | D-065 |
| S13 | Item ids never repeat for a chat, across sessions and restarts | D-059 |

**Liveness** (eventually happens, if the network, MDK and the harness keep working):

| Id | Property |
| --- | --- |
| L1 | Every answer and control is eventually settled |
| L2 | Every turn eventually ends |
| L3 | Every message for the agent is eventually handled |
| L4 | A connector that goes away is eventually replaced, and the core goes on |

## First, fix what is known

Reading the core and the drivers found these, all **inferred from the code and not yet reproduced**. Each becomes
a failing test first, then a fix, and later a counterexample the model must find.

- **Crash windows in the run loop** (S1, S3). A version sent but not yet saved is sent again with the same number
  and a new first event after a restart, and the request it settled is settled again as failed. A request the
  core took but had not saved is never settled. A failed save of the read mark is ignored, so a crash can run a
  prompt twice (S4). Closing everything on a stop and then crashing sends the closes again.
- **Sends that give up** (S1, S3). The core stops trying a send after five failures, which loses settles and
  starts a new row for the next version. Part of this waits for crash-safe sends in MDK.
- **Settles carried by a message are dropped** (S1). The core reads `settles` only from items that are not
  messages. A pending answer whose approval ends without naming it stays pending until the driver stops.
- **An open item can be forgotten** (S2, S3). The cache of finished items can evict one that opened again.
- **Refusals before the rights check** (S6). Someone with no rights is told an item's state instead of
  `not_allowed`.
- **Only tool output is cut** (S7). A long message or plan goes out over 64 KB.
- **An owner key in the wrong form locks the owner out** (S6). An uppercase key or an npub in the settings never
  matches, silently. The installers now convert npubs; the settings file should refuse what it cannot read.
- **Two cores on one home** overwrite each other's state. There is no lock.
- **OpenClaw vouches a sender for every chat** (S5, S6). The core's own check is the backstop.
- **MDK's test relay limits each connection to 60 events a minute** by default. Our longer scenarios likely hit
  it, which may explain some flaky real-harness runs. The tests should lift the limit, as MDK's own do.
- **Messages missing after an abrupt stop**, seen while building the importer: the next program on the same home
  sometimes never got later messages. This looks like MDK's, and goes to MDK as an issue.

## The layers

From cheapest to most thorough. Each lists what it owns.

### 1. Unit and example tests (kept)

The replay tests, hand-written cases and stand-in harnesses stay. Each regression from review keeps its test. New
rule: every unit test of a state machine checks the properties above on every step, not only the end state.

### 2. A conformance suite for every driver

One suite, written once against the driver trait, that every driver must pass: Codex, ACP (OpenCode, Goose,
Cursor), Pi, and the socket connectors (Claude Code, OpenClaw, Hermes) through a real connector on the socket with
a fake harness behind it. It owns S1, S2, S5, S11, S12 and S13 for drivers. Three connectors carry their own copy
of the settling logic today; the suite checks all of them the same way.

Each driver gets a scriptable fake harness, grown from the stand-ins that exist (`fake_pi.py`, ACP's in-memory
agent, Codex's fake), that can: write a reply short, answer late or never, answer out of order, die at any step,
serve two chats at once, and replay a recording.

### 3. A deterministic simulator

A new crate, `marmot-agent-sim`, that runs the real core with a fake Marmot side and scripted drivers, on a virtual
clock, from a seed. It owns S1 to S4, S10, L1 to L3 for the core.

- **Faults it injects:** a crash between any two effects, a failed save, a send that fails (busy, then
  ambiguous), a live message that arrives before it is stored or out of order, a lagging event stream, a driver
  that hangs, a socket that drops mid-request, a connector that reconnects.
- **Oracles:** the safety properties checked after every step; at the end, with faults stopped, the liveness ones.
- **Reproducible:** a run is named by its seed. A failure writes the schedule to a file that replays it exactly,
  as MDK's failure capsules do, and the simulator shrinks it to a short schedule.
- **Seam:** the core uses MDK through a concrete type today. The simulator needs that behind a small trait, which
  is internal. A second lane runs the same scenarios on MDK's real test relay, with crashes, for what a fake
  cannot show.

### 4. Formal models

**TLA+**, checked with TLC, as MDK does, so both projects use one tool and one way of working. Each model has a
fair configuration that must pass, and an unfair one that must produce a counterexample, which is committed as a
simulator scenario. A test checks that the model's actions and the code's still match.

| Model | What it covers | Owns |
| --- | --- | --- |
| `CoreLifecycle` | The conversation, the run loop, saving and crashing: requests pending and settled, items and versions, read marks, sessions | S1 to S4, S10, S12, L1 to L3 |
| `DriverTransport` | One driver's side, for any harness: requests in flight, effects awaiting settlement, a dropped wait, a restart of the harness, two chats | S1, S5, S11, S13, L1, L2 |
| `SocketProtocol` | The hello, a request answered while events arrive, a connector going and another coming, one at a time | S11, L4 |
| `Gateways` | Small models of OpenClaw's approval vouching and Hermes's held turn end | S5, S6, S1 |

Model traces then drive the code: a trace the checker produces is replayed against the real conversation, and
against the simulator, to show the code takes the same steps (trace validation).

### 5. Bounded proofs with Kani

For small pure functions where every input of a bounded size can be checked: cutting an item to fit (S7: never
splits a character, always shrinks, always ends), matching typed commands (S6), the rights check (the owner always
passes, nobody else is ever the owner), token comparison, the socket's line reader, Codex's frame resumption, and
how request and item ids are formed (S13).

### 6. Property tests

proptest for Rust, fast-check for TypeScript, hypothesis for Python.

- The conversation against a small reference model, over random sequences of every input, including restarts.
- Every translator over recordings changed at random: stream chunks split, notifications repeated or reordered,
  replies dropped, restarts inserted. After each: every open item closes when the harness stops, nothing follows
  a final state, and the native message is unchanged.
- **Differential:** each translator against the prototype on the same changed recordings, so Rust, TypeScript and
  Python agree with one reference.
- The carrier: an item read back equals the item sent, and fits whenever it can.
- File confinement against the real file system on generated trees.

Failing cases are committed (`.proptest-regressions`), and a list records each property's normal and nightly case
counts.

### 7. Fuzzing

cargo-fuzz targets for everything that reads input from outside: the socket's line reader and messages, the
hello, each driver's translator, Pi's records, the carrier's tags, the settings file, and the saved state. The
recordings are the starting corpus. Run nightly.

### 8. Mutation testing

cargo-mutants on the core and the drivers, nightly, to find code no test notices changing. MDK's practice is kept
too: a few hand-made mutants per model property, to show each check catches what it claims.

### 9. Privacy checks

For S8: a scan of the source that refuses printing or logging of values, as MDK's `tracing_audit` does; and a
canary test, where the simulator and the real scenarios put unique marker strings in message text, paths and keys,
then check that no log, debug output, file or recording contains one.

### 10. Real harnesses

The scenarios against real harnesses stay, and run nightly instead of only by hand.

## In CI

| When | What |
| --- | --- |
| Every pull request | Everything that runs today, the conformance suite, property tests at normal counts, the simulator on a fixed set of seeds, TLC on small bounds, Kani on the small proofs, the privacy scan |
| Nightly | Property tests at high counts, the simulator on many new seeds, TLC on larger bounds, fuzzing, mutation testing, the real-harness scenarios |
| Weekly | Long simulator campaigns |

## In what order

1. **Fix what is known**, each with its failing test first. This also starts the "sends that survive a crash"
   item of M4.
2. **The simulator seam and the `CoreLifecycle` model**, together, since they share one picture of the core.
3. **The conformance suite and the scriptable fakes.**
4. **Property tests, Kani proofs and fuzzing.**
5. **Trace validation, mutation testing, the nightly lanes.**

## For the owner to decide

1. **TLA+ and TLC.** Recommended, to match MDK. The alternative is Quint, which has friendlier tooling and makes
   traces for testing more easily, at the cost of a second tool across the two projects.
2. **New development tools:** proptest, Kani, cargo-fuzz (which needs the nightly compiler, in its own CI job),
   cargo-mutants, fast-check and hypothesis. None ships in the program. Recommended.
3. **Running real harnesses nightly in CI** needs the numes key, and a Cursor or Claude Code sign-in, as CI
   secrets. Recommended for the numes key only, in a separate workflow that pull requests cannot reach.
4. **CI time:** about 15 more minutes on each pull request, and an hour or two each night. Recommended.
5. **The order above,** with fixing what is known first. Recommended.
