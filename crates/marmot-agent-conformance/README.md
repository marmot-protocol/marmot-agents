# marmot-agent-conformance

The driver conformance suite: layer 2 of [the verification plan](../../docs/design/verification.md#2-a-conformance-suite-for-every-driver).
One set of scenarios, written once against the `Driver` trait, that every driver must pass, each on a scriptable fake
of its harness. A checker looks at every step and names the property a failure breaks, by the ids the plan gives.

Status: phase one is built (2026-11-22): the suite, fakes for Codex, ACP and Pi, the three built-in drivers run against
it, and deliberate faults that show each check catches its bug. A first review's findings are fixed (2026-11-22).
Phase two, the socket connectors, is a design, below. Everything here is internal: it ships in no program.

## How it fits

| Part | Where | What it does |
| --- | --- | --- |
| `Subject` | `src/run.rs`, one per driver in `tests/` | Starts a driver on a fake harness; says whether one harness serves every chat |
| `Run` | `src/run.rs` | Makes the core's requests, waits for each to return (never drops one, D-066), takes events, and in `dropping` mode polls `next_event` once and drops it, again and again |
| `Checker` | `src/check.rs` | The properties, checked on every request, every output, and what the harness heard |
| Scenarios | `src/scenarios.rs` | What every driver is put through |
| Fake engine | `src/fake.rs` | Runs a fake harness on a pipe, with faults (`Knobs`), and records what it heard (`Heard`) |
| Fakes | `src/fake/codex.rs`, `acp.rs`, `pi.rs` | Each harness's own protocol, grown from the stand-ins the drivers' tests had |
| Replay | `src/fake/replay.rs` | Plays a recording's native messages back to the driver |
| Faults | `src/faults.rs` | A wrapper driver that breaks one property on purpose |

The checker knows nothing of any harness. A prompt's text carries its chat's mark (`<<c-1>>`) and its message id, and
a fake echoes the prompt in what it says, so what reaches a chat can be traced to the chat it came from. A fake reads
the same words in every harness: `#ask` (an approval; Pi, which has none, asks a question), `#quiz` (a question),
`#sleep` (runs until interrupted), `#die` (starts a command, then dies); anything else is answered in words.

## The properties

Checked here for drivers; each id is the plan's, so a search finds the property, its check and its fault.

| Id | Checked | How |
| --- | --- | --- |
| S1 | every step, and at the end | Each answer and control is settled at most once: by a refusal, or by one event of its own chat that is not a message. An event the driver made up, with no native message (D-059, point 3), such as an item it closes when the harness stops, settles nothing, unless it is an error settling it as failed. At the end every request the driver took is settled, unless the driver ended (the core then settles it). A request from before a restart is never settled after it |
| S2 | after a stop | Every item of the chats whose harness stopped is closed, but the session's own item: a session outlives its harness (Pi's driver takes it up in a new Pi), and when a driver ends the core closes what is left (D-034) |
| S4 | at each checkpoint | No prompt reaches the harness twice |
| S5 | every step | Every event names its chat; a session, a turn and an item stay with the chat they were first seen in; a chat's mark appears only in its own chat, and a turn names only its own chat's prompts; a request is settled only in its own chat; an answer reaches the harness in its own chat's session, with what was chosen |
| S11 | every request, and at each checkpoint | A request returns within the suite's patience, and when it has returned the harness has it, with the driver not polled again: an answer taken was heard, or the harness died reading what was written after it; no line the harness reads is half a message; with waits dropped at every point, nothing is lost; an answer the harness can no longer read is not taken; a request refused never reaches the harness later |
| S12 | at each checkpoint | The harness takes up a session again only when a prompt named it, or when it was the chat's current session as its last prompt was made (Pi restarting a chat's Pi), not the session the resume itself made current; a named session is taken up; a prompt naming none stays in the chat's session |
| S13 | every step | An item id, once closed, is not used again in its chat, and an id from before a restart is not used after it (a session's own item returns with its session); an item keeps its session |
| SEQ | every step | `seq` rises within a session (D-059, point 4). Not in the plan's table |

S3, S6 to S10 and the liveness properties are the core's, owned by the simulator and the TLA+ model. A wait that runs
out is reported as the liveness property it stands for (L1 a request, L2 a turn, L3 a prompt refused).

## The scenarios

Every driver runs these, but where the table under "What was run" says otherwise.

| Scenario | What happens |
| --- | --- |
| `one_turn` | A prompt answered in words |
| `two_chats` | Chat 1 runs until interrupted while chat 2's harness asks for approval, which is given; then chat 1 is interrupted |
| `answered_once` | Two chats, one approval allowed and one question answered the other way, then the same item answered again, which must be refused |
| `answer_after_death` | The harness dies while its approval waits; the driver, not yet told, is given the answer, and must refuse it; the approval must close unanswered |
| `dies_mid_turn` | The harness dies in the middle of a command; Pi's driver must start a new Pi for the next prompt |
| `dies_with_requests_pending` | The harness dies as it reads an interrupt, before acting on it, after another chat's answer was settled |
| `dropped_waits` | `two_chats` with every wait for an event dropped at the first point it would wait, and the harness writing in pieces |
| `slow_and_out_of_order` | `two_chats` with replies late, after what follows from them, and overtaking each other; for Pi, which answers a prompt before its run reports anything, everything slow instead |
| `partial_frames` | `two_chats` with every frame from the harness in three pieces |
| `never_answers` | The harness never answers the request a prompt waits on (Codex `thread/start`, ACP `session/new`, Pi `prompt`); then it answers, late, and nothing may follow from it in a chat whose prompt was refused |
| `stops_reading` | The harness stops reading its input and keeps it open while a prompt longer than any pipe is written; the prompt must return, and if refused must never reach the harness once it reads again |
| `late_replies` | Every reply comes half a second late (for Pi, everything slow): a prompt in a new chat, which waits more than once, is still taken |
| `restart` | A turn, a restart, a prompt naming the chat's session, a prompt naming none; then a restart in the middle of a turn |
| `new_session` | A new session, settled by the new session; the next prompt goes to it; another chat naming the old one is refused |
| `settings` | A setting changed, settled once |
| `dies_at_every_step` | The harness dies on its n-th frame after start, in a turn that asks for approval, for every n the turn reaches: a run with nothing failing counts them first, and each run must see the harness die before what it left is checked |
| `replay` | A recording played back through the driver, every approval allowed (the one-time choice, as recorded); every recorded message must be played, and the driver's answer to the harness's own request must be the recorded one, to the same request with the same decision, or nothing more is played. Answered the other way, a recording stops (`a_replay_stops_on_another_answer`) |

The pipe between driver and harness holds 256 bytes each way, so a dropped wait can fall in the middle of a write.

## The fakes

| Fake | From | Shapes | What differs |
| --- | --- | --- | --- |
| Codex | the stand-in in `crates/marmot-agent-codex/tests/driver.rs` | threads, turns, `commandExecution`, approvals, `requestUserInput`, steer, interrupt, settings, resume | one process for every chat |
| ACP | the agent in `crates/marmot-agent-acp/tests/driver.rs` | sessions, prompts, tool calls, `request_permission`, `elicitation/create`, cancel, config options | **OpenCode**: `session/resume`. **Goose**: `session/load` with a replay, sessions numbered by day, `activeRunId`, and after a crash a run of its own until cancelled. **Cursor**: `session/load`, no name, `cursor/update_todos` naming only its tool call |
| Pi | `crates/marmot-agent-pi/tests/fake_pi.py` | Pi's records, extension dialogs, steering, abort, a session's file written once it holds a message | one process per chat: the driver runs `src/fake/relay.py`, which connects its standard input and output to a socket the test serves, so each Pi is scripted in Rust |

The faults, set as `Knobs` and changeable between steps: frames written in pieces; replies late; replies after the
notifications that follow from them and every other reply held back so a later one overtakes it; everything slow;
requests of a kind held unanswered, and answered once taken off the list; a harness that stops reading its input,
keeping it open, until told to read again; death on the n-th frame, or when killed, in the middle of a frame when writing in
pieces. A harness's sessions outlive its processes, so a restart can take them up. A death is counted once both ends of
the pipe are closed, so a driver that writes after it is seen writing to a harness that has gone. What a driver leaves
unfinished when it closes the harness's input is not a line, and is not read as one.

## What was run, and found

Bounded result, 2026-11-22: one or two chats, up to two sessions each, one restart, a death at each frame an
approval's turn writes (3 for OpenCode and Goose, 4 for Cursor, 6 for Codex and Pi), seven recordings. Until
2026-11-22 the sweep ran a fixed 10 or 12 steps and passed a step the harness never reached, so most of it checked
nothing.

| Driver | Scenarios | Recordings | Deliberate faults caught |
| --- | --- | --- | --- |
| Codex | all, `dies_at_every_step` (6) | `simple`, `approval_allowed`, `command` | all 12 |
| ACP, OpenCode-shaped | all, `dies_at_every_step` (3) | `simple`, `approval_allowed` | |
| ACP, Goose-shaped | `two_chats`, `answered_once`, `answer_after_death`, `dies_mid_turn`, `restart`, `new_session`, `dies_at_every_step` (3) | `simple`, `approval_allowed` | |
| ACP, Cursor-shaped | `two_chats`, `answered_once`, `answer_after_death`, `restart`, `dies_at_every_step` (4) | `simple`, `approval_allowed` | |
| Pi | all but `stops_reading`, which fails, `dies_at_every_step` (6) | `gate_allowed`, `command` | 7: those that turn on a harness for each chat, and `TakeUnhanded` |

The faults (`src/faults.rs`), each caught as the property it breaks, by the scenario named. Where more than one check
names the property, the test also requires the words of the check meant for the fault (`Fault::detail`):

| Fault | Breaks | Caught by |
| --- | --- | --- |
| `DropSettle`: the first settle is left out | S1 | `two_chats` |
| `SettleTwice`: each settle repeated on the next event | S1 | `two_chats` |
| `SettleOnStop`: an interrupt settled by the turn closed when the harness stopped (Codex's driver once did this) | S1 | `dies_with_requests_pending` |
| `LeaveOpen`: what is closed for a stop is not handed over | S2 | `dies_mid_turn` |
| `PromptTwice`: each prompt handed over twice | S4 | `two_chats` |
| `WrongChat`: chat 2's events handed over as chat 1's | S5 | `two_chats` |
| `ResumeUnasked`: a prompt naming no session sent naming the chat's first one | S12 | `new_session` |
| `ReuseItem`: later turns take the first turn's item id | S13 | `new_session` |
| `SeqBack`: every third `seq` back to 1 | SEQ | `one_turn` |
| `HalfFrame`: a dropped wait leaves half a message on the harness's input | S11 | `dropped_waits` |
| `LoseOnDrop`: a dropped wait loses the event in hand | S11 | `dropped_waits` |
| `TakeUnhanded`: an answer the driver could not hand over is taken anyway, and never settled | S11 | `answer_after_death` |

Four driver bugs were found, each fixed after its scenario failed:

- **The ACP driver took an answer the agent could no longer read** (`answer_after_death`). A request's message went to
  the writer task's queue and the request returned at once, then fed the translator, so an answer to an agent that had
  gone showed as given and its event settled it. Now a request returns once its message is written, as Codex's and Pi's
  drivers did already.
- **The Codex driver waited for ever on a Codex that never answered** (`never_answers`): for its reply to
  `thread/start`, and on a request before it. The core waits for every request (D-066), so every chat stopped. Now a
  request waits at most a minute, then the connection ends, as the ACP driver and the socket's do.
- **The ACP and Codex drivers waited for ever on a harness that stopped reading** (`stops_reading`): ACP for the
  writer task to say a request's message was written, Codex while writing it. Now writing counts against the same
  minute: ACP's driver ends the connection and the writer task with it, and Codex's one minute covers everything a
  request waits for, writing included. In both, nothing the harness says after that is read.

Not covered yet, and why:

- **Pi's driver waits for a record to be written with no limit** (`stops_reading` fails, so it is an ignored test
  in `tests/pi.rs`): a Pi that stops reading its input stalls the request, and every chat with it. Found 2026-11-22;
  not fixed here.
- **Codex's greeting** (`Codex::connect`) still waits for `initialize` with no limit. It runs before the core's loop, at
  start. Not changed here.
- **Pi's start** waits up to `STARTUP_LIMIT` (60 s, fixed), so a Pi that never answers `get_state` is not run: too slow.
- **Files** the agent sends, **the harness's own actions** (compacting), **group chats** and **attachments**.
- **Goose and Cursor** run only the scenarios where they differ from OpenCode.

## Running it

```sh
cargo test -p marmot-agent-conformance            # every driver, the faults, the replays: about 30 s
cargo test -p marmot-agent-conformance --test acp # one driver
```

The Pi tests need `python3` for the relay, as Pi's own tests do. The suite is part of the workspace's tests, so CI's Rust
job runs it on every pull request.

The scenarios are listed once, in `with_scenarios!` (`src/scenarios.rs`). A driver that runs them all makes one test
for each from that list, as `tests/codex.rs` does, so a scenario added there is run at once. A driver that runs only
some (Goose, Cursor, and Pi for now) names those it runs and those it leaves out, with the reason; its
`every_scenario_is_run_or_left_out` fails unless the two name every scenario once, and a name that is no scenario does
not compile.

To add a driver: write a `Subject` for it in `tests/`, on a fake of its harness (a `Protocol`), and make its tests
from `with_scenarios!`.

## Phase two: the socket connectors (design)

The Claude Code driver (`packages/claude-code`), the OpenClaw plugin (`packages/openclaw`) and the Hermes plugin
(`python/marmot_hermes`) reach the core over the socket, and each carries its own copy of the settling logic. They
join the suite by running **the real connector over the real socket**, as the core runs it, with a scripted fake
harness inside it in place of the real one. The Rust suite drives it through `SocketDriver`, so the scenarios, the
checker and the faults stay as they are.

```text
 Rust test ──Run──▶ SocketDriver ══ the socket ══▶ connector (real) ──▶ fake harness (in the connector)
     ▲                                                                       │
     └────────────── control channel: knobs and kills in, Heard out ◀────────┘
```

**The control channel.** One small protocol, the same for all three: JSON lines on the connector process's standard
input and output, which the real connector does not use for anything else (its log goes elsewhere). In:
`{"knobs": {...}}`, the same fields as `Knobs` (`late`, `slow`, `never`, `die_after`, …), and `{"kill": true}`. Out:
`{"heard": {...}}` in the `Heard` shapes, `{"frame": "<kind>"}` for each call the connector made on the harness, and
`{"died": true}`. The fake reads the same prompt words (`#ask`, `#quiz`, `#sleep`, `#die`) and echoes the prompt.

**The Rust side.** A `Subject` named `SocketOnFake`, in a test of the core crate
(`crates/marmot-agent/tests/conformance_socket.rs`, with this crate as a dev-dependency), since `SocketDriver` lives
there and that crate's tests are already built in CI. `start` listens with `Endpoint::in_home` on a throwaway home, spawns
the connector's fake-harness entry with `MARMOT_AGENT_HOME` set, accepts, and `greet`s, with `answer_within` short. A
bridge task copies what comes on the channel into the `Fake` (which gains `record_heard`, `record_frame` and
`record_death`) and sends it the knobs. A restart kills the connector process, which also exercises what the
simulator leaves to this suite: a connector that drops mid-request, and the one that takes its place (L4).

**What each connector needs**, the smallest way:

| Connector | The seam it has | What to add |
| --- | --- | --- |
| Hermes | `Connector(hermes, send_to_core, …)` takes the Hermes object; `FakeHermes` in `python/tests/test_connector.py` | `python/tests/conformance_harness.py`: a `ScriptedHermes` grown from `FakeHermes`, run by the adapter's real socket client, reading the channel. Standard library only, as the plugin is. The smallest of the three: do it first |
| Claude Code | `connector.ts` takes the SDK's `query` as a parameter; `FakeQuery` in `test/transport.test.ts` | `test/conformance-harness.ts`: a scripted `QueryFn` grown from `FakeQuery` (an async generator of SDK messages: init, assistant text, `tool_use`, result; `canUseTool` for `#ask`, `AskUserQuestion` for `#quiz`, `interrupt()` for `#sleep`, a thrown error for `#die`), and an entry that runs `main.ts`'s transport with it. Run with the Node CI already pins |
| OpenClaw | the connector talks to the gateway through a `Port`; `FakeOpenClaw` in `test/transport.test.ts` | `test/conformance-harness.ts`: a scripted `Port` grown from `FakeOpenClaw` (runs, approvals vouched as OpenClaw vouches them), and an entry that starts the plugin's connector without a gateway |

Faults that need frames on the harness's side (`partial`, half a frame) do not apply: these harnesses are libraries
called in process. In their place the channel can make the connector's own socket writes come in pieces, through its
`socket.ts` or `core.py` writer, so the core's reader is tried the same way.

**CI.** One job, `Conformance (socket)`, beside the Rust job: Node and Python at the versions CI already pins, the Rust
build from the shared cache, and the core crate's conformance test alone. It should take as long as the simulator's job,
inside the 15 minutes (D-094).

**Owner questions, when it is built:** none expected; the entries are test-only and nothing the connectors send
changes.
