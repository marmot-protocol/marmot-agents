# marmot-agent-conformance

The driver conformance suite: layer 2 of [the verification plan](../../docs/design/verification.md#2-a-conformance-suite-for-every-driver).
One set of scenarios, written once against the `Driver` trait, that every driver must pass, each on a scriptable fake
of its harness. A checker looks at every step and names the property a failure breaks, by the ids the plan gives.

Status: phase one is built (2026-11-22): the suite, fakes for Codex, ACP and Pi, the three built-in drivers run against
it, and deliberate faults that show each check catches its bug. Two reviews' findings are fixed (2026-11-22), and so
is the Pi driver's stall on a Pi that stops reading (2026-11-22). Files the agent sends (`send_file`) are covered for
Codex, Pi and the three ACP shapes (2026-11-22).
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
| Fake tests | `tests/fake.rs` | What the checks rely on the fakes to report: refused prompt copies, deaths matched to their harness, a kill that ends a blocked harness |

The checker knows nothing of any harness. A prompt's text carries its chat's mark (`<<c-1>>`) and its message id, and
a fake echoes the prompt in what it says, so what reaches a chat can be traced to the chat it came from. A fake reads
the same words in every harness: `#ask` (an approval; Pi, which has none, asks a question), `#quiz` (a question),
`#sleep` (runs until interrupted), `#die` (starts a command, then dies), `#file` (calls the driver's own tool to
send a file, D-090, with a relative path that carries the prompt's mark, and goes on once told how it went); anything
else is answered in words.

## The properties

Checked here for drivers; each id is the plan's, so a search finds the property, its check and its fault.

| Id | Checked | How |
| --- | --- | --- |
| S1 | every step, and at the end | Each answer and control is settled at most once: by a refusal, or by one event of its own chat that is not a message. An event the driver made up, with no native message (D-059, point 3), such as an item it closes when the harness stops, settles nothing, unless it is an error settling it as failed. At the end every request the driver took is settled, unless the driver ended (the core then settles it). A request from before a restart is never settled after it |
| S2 | after a stop | Every item of the chats whose harness stopped is closed, but the session's own item: a session outlives its harness (Pi's driver takes it up in a new Pi), and when a driver ends the core closes what is left (D-034) |
| S4 | at each checkpoint | No prompt reaches the harness twice; a copy the harness refused, as when the session is busy, reached it all the same and counts |
| S5 | every step | Every event names its chat; a session, a turn and an item id stay with the chat they were first seen in; every chat mark in an event's text is its own chat's, and a turn names only its own chat's prompts; a request is settled only in its own chat; an answer reaches the harness in its own chat's session, with what was chosen (allowed or not, or the option's words); a file the agent asks to send is handed over for the chat whose harness asked, every mark in its path that chat's, and how it went reaches the harness in that chat's session, as the core gave it |
| S11 | every request, and at each checkpoint | A request returns within the suite's patience, and when it has returned the harness has it, with the driver not polled again: a prompt or an answer taken was heard, or the chat's own harness read something after it was given and then died (a death before, or of another chat's harness, shows nothing); no line the harness reads is half a message; with waits dropped at every point, nothing is lost; an answer the harness can no longer read is not taken; a request refused never reaches the harness later |
| S12 | before each prompt, and at each checkpoint | The harness takes up a session again only when the chat's last prompt named it, or when it was the chat's current session as that prompt was made (Pi restarting a chat's Pi), not a session an earlier prompt named, nor the session the resume itself made current; a named session is taken up; a prompt naming none stays in the chat's session |
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
| `answer_after_interrupt` | A turn is interrupted while its approval waits: the approval closes with the turn, and an answer given after that must be refused and never reach the harness. The Codex and Pi fakes hear a reply to what they no longer ask, as the real ones read it and drop it |
| `dies_mid_turn` | The harness dies in the middle of a command; Pi's driver must start a new Pi for the next prompt |
| `dies_with_requests_pending` | The harness dies as it reads an interrupt, before acting on it, after another chat's answer was settled; it must die, so a driver that never writes the interrupt checks nothing and fails |
| `dropped_waits` | `two_chats` with every wait for an event dropped at the first point it would wait, and the harness writing in pieces |
| `slow_and_out_of_order` | `two_chats` with replies late, after what follows from them, and overtaking each other; for Pi, which answers a prompt before its run reports anything, everything slow instead |
| `partial_frames` | `two_chats` with every frame from the harness in three pieces |
| `never_answers` | The harness never answers the request a prompt waits on (Codex `thread/start`, ACP `session/new`, Pi `prompt`); then it answers, late, and nothing may follow from it in a chat whose prompt was refused |
| `stops_reading` | The harness stops reading its input and keeps it open while a prompt longer than any pipe is written; the prompt must return, and if refused must never reach the harness once it reads again. With a harness for each chat (Pi), only chat 1's stops reading: chat 2's must serve a prompt while chat 1's is stuck, without being started again, and chat 1's next prompt must be taken by a new one |
| `late_replies` | Every reply comes half a second late (for Pi, everything slow): a prompt in a new chat, which waits more than once, is still taken |
| `restart` | A turn, a restart, a prompt naming the chat's session, a prompt naming none; then a restart in the middle of a turn |
| `new_session` | A prompt naming the chat's session; a new session, settled by the new session; the next prompt goes to it, and not to the session named before; another chat naming the old one is refused |
| `new_session_unnamed` | A setting changed and a new session asked for at once, naming no session, as the core does before it has seen the chat's (it names the one it saved), with the harness slow: the setting's edit of the current session comes after; only the new session may settle the request. A driver may refuse it |
| `folder` | A chat moves the harness to another folder (D-101): a chat that chose none opens its session in the driver's own folder; a new session naming another folder is taken, settled by the new session, which opens there, and the next prompt goes to it; a chat with no session yet moves and its first session opens there; a third chat stays in the driver's folder; after a restart the moved session is taken up in its folder. Checked against the folder the fake heard (`cwd` on Codex's and ACP's session requests, the folder Pi's relay was started in), and reported as D-101 |
| `send_file` | The agent sends a file with the driver's own tool (D-090): each request must name the chat whose harness asked, with its relative path made whole from that chat's folder (D-101), and how it went must reach that harness, and only it, as the tool's result. Two chats open sessions in the driver's folder, chat 2's last, so an agent that keeps one tool server for a folder's sessions (OpenCode) reaches the driver through chat 2's and only the call names chat 1's session; chat 1's file is sent. While it waits for the core's word, chat 1 prompts again: the prompt must return within a second, taken or refused, since the core cannot give the word until it does (S11; an agent awaiting its tool ends a cancelled turn only once the call comes back, and so does the fake). Then chat 2 moves to another folder, and two files are asked for at once, chat 2's there and chat 1's, both waiting, answered the other way round: chat 1's sent, chat 2's refused, and the core's words must reach chat 2's new session. Checked against what the fake heard as its tool's result, which must come without the driver being polled again. A driver that gives its harness no such tool (`Subject::sends_files`, false by default) must hand over no file; every built-in driver gives one |
| `settings` | A setting changed, settled once |
| `dies_at_every_step` | The harness dies on its n-th frame after start, in a turn that asks for approval, for every n the turn reaches: a run with nothing failing counts them first, and each run must see the harness die before what it left is checked |
| `replay` | A recording played back through the driver, every approval allowed (the one-time choice, as recorded); every recorded message must be played, and the driver's answer to the harness's own request must be the recorded one, to the same request with the same decision, or nothing more is played. Answered the other way, a recording stops (`a_replay_stops_on_another_answer`, through `replay_refused`): the prompt must be taken and the approval asked, the answer taken and turned away by the replay, and nothing more played, so a run that failed earlier does not pass |

The pipe between driver and harness holds 256 bytes each way, so a dropped wait can fall in the middle of a write.

## The fakes

| Fake | From | Shapes | What differs |
| --- | --- | --- | --- |
| Codex | the stand-in in `crates/marmot-agent-codex/tests/driver.rs` | threads, turns, `commandExecution`, approvals, `requestUserInput`, the driver's `send_file` dynamic tool (`item/tool/call`, only on a thread started with it), steer, interrupt, settings, resume | one process for every chat |
| ACP | the agent in `crates/marmot-agent-acp/tests/driver.rs` | sessions, prompts, tool calls, `request_permission` (with `#twice`, two at once for one tool call; with `#garbled`, one naming no tool call), `elicitation/create` (with `#garbled`, a form at a web address), a turn that goes on without what the client refused, cancel, config options, MCP servers over HTTP (`src/fake/mcp.rs` is the client: `initialize`, `tools/list`, `tools/call`) | **OpenCode**: `session/resume`; one tool server of a name for every session in a folder, the last handed over, the calling session named in `_meta["ai.opencode/sessionID"]`. **Goose**: `session/load` with a replay, sessions numbered by day, `activeRunId`, and after a crash a run of its own until cancelled; servers kept per session, the caller named in `_meta["agent-session-id"]`. **Cursor**: `session/load`, no name, `cursor/update_todos` naming only its tool call; servers kept per session, the caller not named. Each holds a cancelled turn open while its tool call is out. The driver is run with file sending switched on (`serve_files`, as `send_files = true` does) |
| Pi | `crates/marmot-agent-pi/tests/fake_pi.py` | Pi's records, extension dialogs, the driver's `send_file` tool (only with the driver's extension loaded, asking in its dialog), steering, abort, a manual `compact` (`compaction_start`, then `compaction_end`, failed or not, then the response, as Pi 1.6.0 does; a session with nothing in it fails), a session's file written once it holds a message | one process per chat: the driver runs `src/fake/relay.py`, which connects its standard input and output to a socket the test serves, so each Pi is scripted in Rust |

The faults, set as `Knobs` and changeable between steps: frames written in pieces; replies late; replies after the
notifications that follow from them and every other reply held back so a later one overtakes it; everything slow;
requests of a kind held unanswered, and answered once taken off the list; a harness that stops reading its input,
keeping it open, until told to read again, every process or only the one with a given tag; death on the n-th frame, or when killed, in the middle of a frame when writing in
pieces. A killed harness blocked writing to a driver that stopped reading is cut off after a moment, as a killed process
is; one that dies by its own script writes what it said first. Each line read and each death is logged with the process
that did it, so a death is matched to the chat whose harness it was. A harness's sessions outlive its processes, so a restart can take them up. A death is counted once both ends of
the pipe are closed, so a driver that writes after it is seen writing to a harness that has gone. What a driver leaves
unfinished when it closes the harness's input is not a line, and is not read as one. A fake can act off the pipe: the ACP
fake calls the driver's tool over HTTP in a task of its own, and the engine hands the result back to it
(`Protocol::waker`, `Protocol::woken`) as it would a frame.

## What was run, and found

Bounded result, 2026-11-22: one or two chats, up to two sessions each, one restart, a death at each frame an
approval's turn writes (3 for OpenCode and Goose, 4 for Cursor, 6 for Codex and Pi), seven recordings. Until
2026-11-22 the sweep ran a fixed 10 or 12 steps and passed a step the harness never reached, so most of it checked
nothing.

| Driver | Scenarios | Recordings | Deliberate faults caught |
| --- | --- | --- | --- |
| Codex | all, `dies_at_every_step` (6) | `simple`, `approval_allowed`, `command` | all 21 that one harness for every chat can show |
| ACP, OpenCode-shaped | all, `dies_at_every_step` (3) | `simple`, `approval_allowed` | `PromptTwice` and the five for files |
| ACP, Goose-shaped | `two_chats`, `answered_once`, `answer_after_death`, `answer_after_interrupt`, `dies_mid_turn`, `restart`, `new_session`, `new_session_unnamed`, `folder`, `send_file`, `dies_at_every_step` (3) | `simple`, `approval_allowed` | the five for files |
| ACP, Cursor-shaped | `two_chats`, `answered_once`, `answer_after_death`, `answer_after_interrupt`, `restart`, `send_file`, `dies_at_every_step` (4) | `simple`, `approval_allowed` | the five for files |
| Pi | all, `dies_at_every_step` (6) | `gate_allowed`, `command` | 13: those that turn on a harness for each chat, `TakeUnhanded`, `SwallowPrompt` and the five for files |

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
| `MixWords`: an event carrying its own chat's words carries another chat's after them | S5 | `two_chats` |
| `ShareItem`: chat 2's turns take chat 1's first turn's item id | S5 | `two_chats` |
| `SwapChoice`: a question answered with another option than the one chosen | S5 | `answered_once` |
| `ResumeUnasked`: a prompt naming no session sent naming the chat's first one, which a prompt before it named | S12 | `new_session` |
| `ReuseItem`: later turns take the first turn's item id | S13 | `new_session` |
| `SeqBack`: every third `seq` back to 1 | SEQ | `one_turn` |
| `HalfFrame`: a dropped wait leaves half a message on the harness's input | S11 | `dropped_waits` |
| `LoseOnDrop`: a dropped wait loses the event in hand | S11 | `dropped_waits` |
| `TakeUnhanded`: an answer the driver could not hand over is taken anyway, and never settled | S11 | `answer_after_death` |
| `SwallowPrompt`: a chat's later prompts taken and never handed over (Pi only: a later prompt follows a death only there) | S11 | `dies_mid_turn` |
| `MadeUpStop`: an interrupt refused without reaching the harness, the turn shown stopped all the same | L2: nothing was checked | `dies_with_requests_pending` |
| `FileWrongChat`: a file chat 2's harness asked to send handed over as chat 1's | S5 | `send_file` |
| `FileToWrongChat`: a file chat 1's agent asks to send handed over for chat 2, as a driver that routes a call by the token of the last session in a folder would | S5 | `send_file` |
| `FileOwnFolder`: a relative path made whole from the driver's own folder, not the chat's | D-101 | `send_file` |
| `FileUnanswered`: how a file went never passed on to the harness that asked | D-090 | `send_file` |
| `LoseFileResult`: a file sent is passed on, but the core's refusal of one never reaches the harness | D-090 | `send_file` |

`send_file` was also shown, 2026-11-22, to catch the ACP driver put wrong by hand three ways: a call routed by its token
alone (S5, OpenCode-shaped, where the token is the last chat's), by the session it names alone (L2, Cursor-shaped, which
names none: the file never comes), and a relative path made whole from the driver's folder instead of the chat's (D-101).

Ten driver bugs were found, each fixed after its scenario failed:

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
- **Pi's driver waited for ever on a Pi that stopped reading** (`stops_reading`), for the task that writes to the
  chat's Pi to say a record was written, so one chat's stuck Pi held up every chat. Now a request waits at most a
  minute (settable; 2 s in the suite), then that chat's Pi is stopped, as when it is let go: the task that writes
  ends, so what was queued for it never reaches it or a later Pi, the chat's open items close, and the request is
  refused as failed. Other chats' Pis run on, and the chat's next prompt takes its session up in a new Pi. Lines from
  the stopped Pi are dropped, as from any Pi the chat has let go. Shown to catch, besides the old driver: a driver that
  stops every chat's Pi, and one that refuses the request without stopping the Pi.

- **Pi's driver took an answer to a question it had closed** (`answer_after_interrupt`, 2026-11-23). At a turn's end
  the driver closes a waiting question, but it kept the dialog, so a later answer was written to Pi, which had
  resolved the dialog itself on the abort (Pi 1.6.0's `createDialogPromise`) and drops a late reply; the answer was
  never settled. Now a question closed with its turn takes no answer.
- **The ACP and Pi drivers settled a new session by an edit of the old one** (`new_session_unnamed`, 2026-11-23).
  A new session was settled by any running session item whose id differed from the one the core named; when the
  core named none, an edit of the chat's current session, such as a setting's, settled it before the new session
  came. Now the session the driver knows the chat is in comes first, as Codex's driver already checked.
- **The ACP driver gave two permission requests for one tool call one item** (`two_requests_for_one_tool_call_take_one_answer_each`
  in `tests/acp.rs`, 2026-11-23). The second edited the first's approval, and an answer went to whichever request
  the driver found first. Now each request is an approval of its own; the first for a tool call keeps its name.
- **The ACP driver never answered a permission or form it could not show** (`a_request_the_driver_cannot_show_is_answered`
  in `tests/acp.rs`, 2026-11-23): one it could not read, or a form at a web address, which it never offers. The
  agent waited for ever. Now it is refused with -32602, invalid params, as the official ACP library refuses a
  request it cannot read. Not yet covered: a request naming a session no chat took, said while a session was being
  picked, is still dropped unanswered.

Not covered yet, and why:

- **Codex's greeting** (`Codex::connect`) still waits for `initialize` with no limit. It runs before the core's loop, at
  start. Not changed here.
- **Pi's start** waits up to `STARTUP_LIMIT` (60 s, fixed), so a Pi that never answers `get_state` is not run: too slow.
- **The harness's own actions** (compacting) for Codex, **group chats** and **attachments**. Pi's compaction is
  covered by `every_compaction_ends_and_the_next_is_taken` (`tests/pi.rs`), 2026-11-23.
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
some (Goose and Cursor) names those it runs and those it leaves out, with the reason; its
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
