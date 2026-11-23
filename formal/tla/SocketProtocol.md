# SocketProtocol: a TLA+ model of the socket

Status: built and checked with TLC on small bounds, as
[D-093](../../docs/decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top) and
[the verification page](../../docs/design/verification.md) plan. The model, its bounds and its switches are
internal and change as the work goes. Read on 2026-11-23 against master `d98d3d4`, with the fixes of branch
`socket-lifetime`.

`SocketProtocol.tla` models the local socket between the core and one connector: the hello, a request answered
while events arrive, the core's patience for an answer, and a connection that goes and another that comes, one at
a time. It owns S11 and L4 for the socket, and checks S1, S2, L1 and L2 where a connection's end touches them.
[`CoreLifecycle`](README.md) models the core itself; until this model it took as given that the driver goes when
the core goes. A connector on the socket need not: this model checks what each kind does instead.

## The three kinds of connector

| `Connector` | Which | What survives a connection that goes |
| --- | --- | --- |
| `exits` | Claude Code (`packages/claude-code`) | Nothing: the connector exits, and its harness with it. Something starts it again (`Supervised`) |
| `fresh` | OpenClaw (`packages/openclaw`) | The gateway, whose run goes on; a new connector is made for the next connection, and what the old run says reaches the old one, which has no socket |
| `kept` | Hermes (`python/marmot_hermes`) | The gateway and its one connector, which may hold a quiet turn's end back (`Holds`, `HOLD_SECONDS`) |

## What is modelled

One chat, one prompt `p1` whose turn is the item `t1`, and one control `r1`, an interrupt of that turn, whose
effect is the turn's end. Each step names itself in the variable `step`, from which `tools/tla/schedule.py`
writes a counterexample.

| Action | In the code |
| --- | --- |
| `Connect`, `Hello` | `run.rs` `serve` takes one connection at a time; `socket.rs` `greet` answers the hello, and nothing is sent before it |
| `Hand(r)` | The core sends a request (`SocketDriver::request`), one at a time. An interrupt of a turn the chat no longer shows running is refused at once (`conversation.rs` `open_turn`) |
| `Take` | The connector reads a request and answers once its harness has it: Hermes `inject`, OpenClaw's run started, Claude Code's input queued or `interrupt()` sent. An interrupt of a turn it does not know is refused (`nothing_to_do`) |
| `Read` | The core reads what the connector wrote, in order; while it waits on a request it holds the events it reads and takes the answer first (`request`, `held`). An event changes its item's row and settles what it names that is pending |
| `Tick` | The core's net for a control whose effect never shows (D-087) |
| `Run`, `EndTurn(hold)`, `Release` | The harness runs the prompt and ends its turn, stopped when it has `r1`; the end settles what waits on it. Hermes may hold a quiet end and let it go later (`send_held`) |
| `Drop` | The connection goes: the core restarts, or the socket breaks |
| `GiveUp` | `ANSWER_WITHIN`: the core's 30 s for a request to be taken runs out, and it gives the connector up |
| (in `Drop`, `GiveUp`) | `ended`: the request waited on is refused as failed (a prompt is told as not delivered and never sent again, D-037), every control taken is settled as failed, every open item is closed (D-058, D-070). The connector forgets by its kind; Hermes's `core_gone` closes what was open on its side, drops a held end, and forgets what waited |
| `Restart` | A connector that exited is started again |

## What is not

- **The core's own state.** Saving, crashing, the outbox and MDK are `CoreLifecycle`'s. A restart of the core
  here is only the connection it ends; what the core keeps across it (`pending`) is kept.
- **Answers, other controls, files, several chats.** An answer is settled as a control is; `send_file` is the
  connector's request and the core answers it whenever the upload ends.
- **The hello's refusals.** A wrong token or protocol is refused and the connector tries again; with either for
  good, no connector is ever served, which needs no model.
- **Time.** No clock: `GiveUp` may fire whenever the connector holds an answer back for an effect, and `Tick`
  for any control taken. `TakenInTime` stands for the 30 s.
- **What a connector does with the session's numbers** (`seq`): the core numbers each session's events on
  from the last it kept (`socket.rs` `numbered_after`), tested in `crates/marmot-agent/tests/socket.rs`.

## Trusted assumptions

| Switch | When on | Shown when off |
| --- | --- | --- |
| `TakenInTime` | A connector that answers a request once its harness has it answers within the core's 30 s | the core gives up a connector that was there and working (`socket-not-taken-in-time`, S11) |
| `Supervised` | A connector that exits is started again (Claude Code's service) | no connector serves the core again (`socket-unsupervised`, L4) |

## Properties

| Id | Property | Form | From |
| --- | --- | --- | --- |
| S1 | The control is settled at most once in the chat | invariant `S1` | D-028, D-070, D-087 |
| S2 | An item the core closed when its connection went is never changed again | invariant `S2` | D-058, D-070 |
| S11 | A request the core counts as taken is one the harness has, and the core gives up only a connector that has not taken a request in time | invariant `S11` | D-058, D-066 |
| L1 | The control is eventually settled; the prompt is eventually handed over or told as not delivered | `~>`, `<>` | D-028, D-037 |
| L2 | An item the chat shows open eventually closes | `~>` | D-058, D-070 |
| L4 | A connector that goes away is eventually replaced, and the core goes on | `~>` | |

The liveness properties hold under one strong-fairness condition per party, as in `CoreLifecycle`: the core;
the connector (`FairConnector`) connects, reads, lets a held end go and starts again; the harness
(`FairHarness`) runs and ends its turn; the clock (`FairTime`) reaches the net's deadline. Drops and give-ups are
bounded, so they end.

## Configurations and results

Checked on 2026-11-23 with TLC v1.7.4 by `tools/tla/check.py`, on 4 workers for the cases that pass and one for
those that fail. The small bounds, every pull request: **one prompt and its turn, one interrupt, two
connections that go (a drop or a give-up)**; each fair configuration is one kind of connector.

| Case | Configuration | Expected | Distinct states | Counterexample |
| --- | --- | --- | ---: | --- |
| socket-exits | `SocketProtocol.exits.cfg` | every property holds | 236 | |
| socket-fresh | `SocketProtocol.fresh.cfg` | every property holds | 284 | |
| socket-kept | `SocketProtocol.kept.cfg` | every property holds | 725 | |
| socket-not-taken-in-time | kept, without `TakenInTime` | S11 fails | 8 | |
| socket-unsupervised | exits, without `Supervised` | L4 fails | 52 | |
| socket-prompt-in-flight | kept, `UndeliveredNotRun` | fails: the accepted limit | 20 | [`prompt-in-flight`](counterexamples/prompt-in-flight.json) |

At four connections that go, once on the same day: exits 436, fresh 532 and kept 1,485 distinct states, every
property holding, each in a second.

**Each check catches its bug** (`Bug`, on the configuration named, with only that property):

| `Bug` | What it breaks | Property | Distinct states | Counterexample |
| --- | --- | --- | ---: | --- |
| `NoReset` | a connector kept across connections forgets nothing of the old core's: the Hermes plugin before the fix (kept) | S2 | 291 | [`kept-connector-remembers`](counterexamples/kept-connector-remembers.json) |
| `AnswerOnEffect` | a control is answered only once its effect shows: Claude Code's interrupt, Hermes's `/new` and `/model`, before the fix (exits) | S11 | 94 | [`answer-on-effect`](counterexamples/answer-on-effect.json) |
| `AnswerBeforeHandoff` | a prompt is answered before the harness has it (kept) | S11 | 14 | |
| `SettleUnpending` | an event settles a request no longer pending (kept) | S1 | 563 | |

**No empty passes.** Each probe must be found false: on the kept configuration, `NeverCut` (the core closes the
turn as its connection goes), `NeverReconnected` (a connector serves the core again), `NeverSettledByEffect`
(the interrupt is settled by the turn's end), `NeverEventBeforeAnswer` (an event waits behind the answer to a
request), `NeverHeldAcross` (a held end at a connection that goes) and `NeverRunOnAfterDrop` (the turn runs on in
the gateway after its connection went); on the fresh one, `NeverRunOnAfterDrop` again. All within 480 distinct
states.

## What the model found

- **A connector kept across connections must forget what the old core knew** (S2). Without it, as the Hermes
  plugin was, the turn the core closed as cut short is changed to done by the turn's end that Hermes sends after
  it reconnects, and a held end does the same. The Hermes plugin's tests found it first
  (`test_a_new_connection_starts_from_what_the_new_core_knows`); `NoReset` puts it back. Fixed on this branch:
  `Connector.core_gone`.
- **A connector must answer once its harness has a request, never once the effect shows** (S11). Otherwise the
  core's patience runs out on a healthy connector, and everything open is cut short. Fixed on this branch for
  Claude Code's interrupt and Hermes's `/new` and `/model`; `AnswerOnEffect` puts it back.
- **The accepted limit of D-037** (`prompt-in-flight`): a prompt whose answer a dropped connection took is told
  as not delivered, and never sent again, but the harness may have it and run it; with a gateway, its turn
  reaches the next core. The core cannot tell the two apart. The notice it gives says the prompt was not
  delivered, which is then not always so (inferred from the model and the code, not seen in a recording).

## Running

```sh
python3 tools/tla/check.py --model SocketProtocol   # its cases only
tools/tla/run.sh formal/tla SocketProtocol.tla SocketProtocol.kept.cfg   # TLC on one configuration
```

CI runs them with every other case but `CoreLifecycle`'s fair one, in the second `TLA+ models` job.
