# Testing and verification

Status: the approach is decided
([D-093](../decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top)), and
so is the CI budget ([D-094](../decisions.md#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly)),
on 2026-11-21. The property list, the models and the layers below are the plan; their details are internal and
change as the work goes. It comes from reading all the code after M3 and the M4 work in progress, and from how MDK
tests itself (its simulator, its TLA+ and Tamarin models, and its testing rules). Under
[first, fix what is known](#first-fix-what-is-known), the behaviour the fixes made visible is provisional: built,
and waiting for the owner. Of the models, `CoreLifecycle` is built (2026-11-22); the others are planned. The
conformance suite's first phase, for the built-in drivers, is built (2026-11-22); its second, for the socket
connectors, is designed.

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
a failing test first, then a fix, and later a counterexample the model must find. Those marked **fixed** were
fixed on 2026-11-21, each with a test that failed first.

- **Crash windows in the run loop** (S1, S3). **Fixed.** A version sent but not yet saved was sent again with the
  same number and a new first event after a restart, and the request it settled was settled again as failed. A
  request the core took but had not saved was never settled, including one whose read mark was saved before the
  request was. A failed save of the read mark was ignored, so a crash could run a prompt twice (S4). Closing
  everything on a stop and then crashing sent the closes again. A group accepted just before a failed save
  started, after the restart, at its latest message, skipping what had arrived.
- **Sends that give up** (S1, S3). **Fixed, as far as the core can.** The core stopped trying a send after five
  failures, which lost settles and started a new row for the next version. A send whose outcome was not known,
  as when the wait for MDK ran out, may have gone out, and was sent again blindly. Now a send is kept until it
  goes. One the core handed to MDK, and whose outcome is not known, is looked for in what MDK stored before it is
  sent again: at once after a restart, and after 15 seconds while running; when MDK cannot be asked, the core
  asks again later. After its tenth try it is looked for once more, and only then given up. One MDK refuses is
  dropped with a notice.

  **What a restart still cannot recover.** A crash between MDK storing a send and the core saving that it went
  leaves the core to find it by looking. An item's version is found by its id, its version and its first
  event. The agent's words are found by their text and tags; when an earlier message of the agent's is the same
  in both, the core cannot tell the two apart. When the lookup cannot identify the earlier send, the send goes
  out again, and the chat shows a duplicate first event. That ends when MDK's crash-safe sends take tagged
  events, and the token each send keeps is handed to MDK.
- **A message MDK cannot read for a moment is skipped for good** (S10, L3). **Fixed** on 2026-11-22
  ([#83](https://github.com/marmot-protocol/marmot-agents/issues/83)). The core took a message it could not place,
  as when its group's state could not be read for a moment, for one not meant for the agent, and moved its read
  mark past it. A failed lookup of a live message's place in storage, or of what a catch-up reads, did the same
  once the next message came. Now such a failure leaves the mark where it is; the group waits, and is read again
  from its mark, a little later each time up to ten seconds, while other groups go on. Found while looking into a
  flaky test, not by reading; the simulator's `ReadFails` fault failed 114 of 200 `live` runs before the fix.
  Review found three more ways in, each fixed with a test that failed first: a catch-up whose list of groups
  failed read nothing and was not tried again (7 of 200 `live` runs failed); a group the core had never seen, as
  on an imported account, ignored what arrived while MDK could not say where its history ends, then started
  after it; and two groups read again at the same moment were read in no fixed order, so a capsule could not
  replay. Now a failed list is tried again before any live message moves a mark, a new group starts at the first
  message heard live, and waiting groups are read again in order.
- **Settles carried by a message are dropped** (S1). **Fixed.** The core read `settles` only from items that are
  not messages. A pending answer whose approval ends without naming it stayed pending until the driver stopped.
  A driver's event could settle another chat's request.
- **An open item can be forgotten** (S2, S3). **Fixed.** The cache of finished items could evict one that opened
  again.
- **Refusals before the rights check** (S6). **Fixed.** Someone with no rights was told an item's state instead
  of `not_allowed`.
- **Only tool output is cut** (S7). **Fixed.** A long message or plan went out over 64 KB, and the general cutter
  could still return an oversized item or drop a session's commands.
- **An owner key in the wrong form locks the owner out** (S6). **Fixed.** An uppercase key or an npub in the
  settings never matched, silently. The installers now convert npubs; the settings file should refuse what it
  cannot read.
- **Two cores on one home** overwrite each other's state. **Fixed:** MDK already refuses the second; a test shows
  it.
- **OpenClaw vouches a sender for every chat** (S5, S6). The core's own check is the backstop.
- **MDK's test relay limits each connection to 60 events a minute** by default. **Fixed:** the tests lift the
  limit, as MDK's own do.
- **A dropped send leaves an item open, or a request unsettled** (S2, L1, L2). **Fixed** on 2026-11-22, found by
  the TLA+ model (`CoreLifecycle`, the give-up and refused configurations) and then by the simulator once its
  oracle stopped letting these cases off. When the core gave up a send after ten unknown outcomes, or MDK refused
  one in a group that stays live, an item's closing version was dropped with the item already closed in the
  core, so nothing closed it in the chat, not even a restart; and a notice that settled requests was dropped
  with nothing in its place, so those requests were settled nowhere. Now the core closes such an item with a
  small version of its own, and settles such requests with a `failed` notice in place of the dropped one. What
  stands in for a dropped send is the last: if it is dropped too, as in a group that is gone, nothing takes its
  place, so nothing loops. The model checks the fix: with one send given up, or one or two refused in a live
  group, every item closes, every request is settled and the chat is told of every gap. Review found three more
  cases, fixed the same way, each with a test that failed first: an item whose close had gone out was closed
  again, as "not delivered", when a later final version was dropped; what stands in for a dropped send carried
  a session id over 64 KB that the driver chose, and was refused for it too; and a dropped `failed` notice that
  settled nothing was not replaced, so the gap went untold. **The accepted limit:** if what stands in for a
  dropped send is dropped too, in a group that stays live, the item stays open in the chat; the model assumes
  MDK does not refuse it (`LastResortTaken`), since it is as small as the core makes anything, and shows what
  happens when it does ([`last-resort-refused`](../../formal/tla/counterexamples/last-resort-refused.json)) or
  when it is given up after its tries
  ([`last-resort-given-up`](../../formal/tla/counterexamples/last-resort-given-up.json)).
- **Messages missing after an abrupt stop**, seen while building the importer: the next program on the same home
  sometimes never got later messages. This looks like MDK's, and goes to MDK as an issue. A related case was seen
  in the tests: right after a person makes a second group with the agent, messages between them sometimes do not
  arrive.

### Provisional, from these fixes

Built, and not yet accepted by the owner, so not in `docs/decisions.md`. What other programs see is in
[kinds 1201 and 1202](agent-kinds.md#provisional-additions): `settles` on the agent's words, `no_effect`, `failed`
for a refused send, the `cut` tags, the stand-in, and the core's close of an item whose closing version was
dropped. Besides those:

- **The settings refuse a key they cannot read.** `owner`, `invite`, and the people in `prompt` and `approve` are
  read as hex in either case or as an npub; anything else stops the core at start, naming the setting.
- **A failed save stops the core**, as a crash would, and the next start takes up what is on disk.
- **The state file** (`marmot-agent-state.json` in the home) keeps, besides what it kept before: the sends not
  yet made, oldest first, each with a token that stays the same across tries, whether it was handed to MDK,
  whether it stands in for a dropped send (and so is the last), whether it is a `failed` notice that tells of
  a dropped send, for each item whether the latest version of it that went out closed it, and,
  for a turn's version that settles what the agent's words showed, the send of those words; where recently
  forgotten items stood; for a turn, the message that started it; and for the agent's words, the requests they
  settle, held until a message carries them.
- **A group's read mark is saved before the agent accepts the group.**
- **One chat sends at most eight events in each turn of the core's loop**, then the others go; a stop sends all
  it can.

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

**Built**, phase one (2026-11-22): [`crates/marmot-agent-conformance`](../../crates/marmot-agent-conformance/README.md).
Codex, ACP (OpenCode-, Goose- and Cursor-shaped) and Pi run 15 scenarios on fakes of their harnesses (Goose and
Cursor the ones where they differ), a death at each frame an approval's turn writes, and seven recordings replayed
through the transport, which play on only after the recorded answer. The checker names each failure by its
property: S1, S2, S5, S11, S12, S13, and S4 and `seq` for drivers besides. Seventeen deliberate faults, put in by a
wrapper driver, are each caught as the property they break, by the check meant for them. It found five driver
bugs, each fixed after its scenario failed: the ACP driver took, and showed as given, an
answer the agent could no longer read; the Codex driver waited for ever on a Codex that never answered; and the ACP,
Codex and Pi drivers waited for ever on a harness that stopped reading its input. Each of the last four held up
every chat. Pi's driver now stops only the stuck chat's Pi, and the suite checks that another chat's Pi serves on
meanwhile. Bounded result: one or two chats, up to two sessions each, one restart. Phase two, the socket connectors over the real socket with a fake harness
inside each, is designed in the suite's README.

### 3. A deterministic simulator

A new crate, `marmot-agent-sim`, that runs the real core with a fake Marmot side and scripted drivers, on a virtual
clock, from a seed. It owns S1 to S4, S10, L1 to L3 for the core.

- **Faults it injects:** a crash between any two effects, a failed save, a send that fails (busy, then
  ambiguous), a read of what arrived that fails for a moment, a live message that arrives before it is stored or
  out of order, a lagging event stream, a driver that hangs, a socket that drops mid-request, a connector that
  reconnects.
- **Oracles:** the safety properties checked after every step; at the end, with faults stopped, the liveness ones.
- **Reproducible:** a run is named by its seed. A failure writes the schedule to a file that replays it exactly,
  as MDK's failure capsules do, and the simulator shrinks it to a short schedule.
- **Seam:** the core uses MDK through a concrete type today. The simulator needs that behind a small trait, which
  is internal. A second lane runs the same scenarios on MDK's real test relay, with crashes, for what a fake
  cannot show.

**Built** (2026-11-22): [`crates/marmot-agent-sim`](../../crates/marmot-agent-sim/README.md). The seam is
`marmot::Side`, which the real `Marmot` implements; the clock is tokio's, paused in a run; the run loop looks at
what it waits for in turn rather than at random, so a schedule replays exactly. It checks S1, S3, S4, S5, S10 and
S12 after every step, and L1, L2, L3 and S2 at the end within 900 s of virtual time, from what the simulated MDK
stored and what the harness saw. Runs are named `family/vVERSION/SEED/INDEX`; a failure leaves a capsule that
replays exactly, shrunk greedily. Six families form the gate and run on every pull request. Two more show what
lies beyond an assumption: `disorder` (a message heard before MDK stores it, or out of order, which MDK does not
do) loses messages, and `late` (a send that lands after the core looked for it) finds the accepted duplicate.
Three bugs fixed before are put back behind a test-only feature, and each is found. Bounded result, 2026-11-22:
36,000 runs of the gate (6 families, 1,000 runs each, seeds 0 to 5), one or two chats, up to about 120 steps
each, all passing. After `ReadFails` joined `live` and `mixed` (version 3 of each), the same day: 18,000 runs
(seeds 1 to 3), all passing; after it gained a failed list of groups, 6,000 runs of `live` and `mixed` (seeds 1 to
3), all passing. Not yet: the connector that
drops mid-request and the one that reconnects (left to the conformance suite, which owns the socket), files, and
the second lane on MDK's real relay.

### 4. Formal models

**TLA+**, checked with TLC, as MDK does, so both projects use one tool and one way of working. Each model has a
fair configuration that must pass, and an unfair one that must produce a counterexample, which is committed as a
simulator scenario. A test checks that the model's actions and the code's still match.

| Model | What it covers | Owns | Status |
| --- | --- | --- | --- |
| `CoreLifecycle` | The conversation, the run loop, saving and crashing: requests pending and settled, items and versions, read marks, sessions | S1 to S4, S10, S12, L1 to L3 | Built, in [`formal/tla`](../../formal/tla/README.md): checked on small bounds on every pull request, with its counterexamples, defects and reachability probes. It found two gaps when a send is given up or refused in a live group (S2, L1, L2), since fixed and checked; see its README |
| `DriverTransport` | One driver's side, for any harness: requests in flight, effects awaiting settlement, a dropped wait, a restart of the harness, two chats | S1, S5, S11, S13, L1, L2 | Planned |
| `SocketProtocol` | The hello, a request answered while events arrive, a connector going and another coming, one at a time | S11, L4 | Planned |
| `Gateways` | Small models of OpenClaw's approval vouching and Hermes's held turn end | S5, S6, S1 | Planned |

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

The scenarios against real harnesses stay, and run nightly instead of only by hand. Most tests use recorded
sessions instead of live models. The scenarios that need a model (OpenCode, Goose, Pi, OpenClaw and Hermes), and the
recorders in `tools/record`, read it from these names, with no default anywhere in the code
([D-100](../decisions.md#d-100-the-installers-defaults)):

| Name | What |
| --- | --- |
| `MODEL_BASE_URL` | An OpenAI-compatible endpoint |
| `MODEL_NAME` | The model, as the endpoint names it |
| `MODEL_KEY_FILE` | The file that holds the endpoint's key, readable by its owner alone |
| `OPENCODE_MODEL` | For OpenCode, which reaches models through its own providers: the model as OpenCode names it |

Each comes from the environment, or else from `tests.env` at the top of the repository, which git ignores and
which may give each name once;
[`tests.env.example`](../../tests.env.example) shows it. The nightly workflow, once it lands, sets
`MODEL_BASE_URL`, `MODEL_NAME` and `OPENCODE_MODEL` from CI secrets of the same names, and writes the secret `MODEL_KEY`, the key
itself, to a file only the job can read, which it names in `MODEL_KEY_FILE`.

## In CI

Pull requests keep CI under 15 minutes, by caching builds and running independent checks as parallel jobs
([D-094](../decisions.md#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly)). Today it
takes about 8 to 10.

| When | What |
| --- | --- |
| Every pull request | Everything that runs today, the conformance suite (built: in the Rust job, about 30 s), property tests at normal counts, the simulator on a fixed set of seeds (built: 200 runs of each gate family from seed 0, in release, in its own job), TLC on small bounds, Kani on the small proofs, the privacy scan, each in its own job |
| Nightly | Property tests at high counts, the simulator on many new seeds, TLC on larger bounds, fuzzing, mutation testing, and the real-harness scenarios on numes.ai's free models, named by CI secrets ([real harnesses](#10-real-harnesses)), in a workflow that pull requests cannot reach |
| Weekly | Long simulator campaigns |

## In what order

1. **Fix what is known**, each with its failing test first. This also starts the "sends that survive a crash"
   item of M4.
2. **The simulator seam and the `CoreLifecycle` model**, together, since they share one picture of the core.
3. **The conformance suite and the scriptable fakes.**
4. **Property tests, Kani proofs and fuzzing.**
5. **Trace validation, mutation testing, the nightly lanes.**

## Decided

1. TLA+ with TLC for the models, as MDK uses
   ([D-093](../decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top)).
2. The development tools: proptest, Kani, cargo-fuzz (on the nightly compiler, in its own job), cargo-mutants,
   fast-check and hypothesis. None ships in the program.
3. The real-harness scenarios run nightly on numes.ai, with its key as a CI secret in a workflow pull requests
   cannot reach ([D-094](../decisions.md#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly)).
4. Pull-request CI under 15 minutes, with caching and parallel jobs.
5. The order above, fixing what is known first.
