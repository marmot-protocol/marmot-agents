# marmot-agent-conformance

The driver conformance suite: layer 2 of [the verification plan](../../docs/design/verification.md#2-a-conformance-suite-for-every-driver).
One set of scenarios, written once against the `Driver` trait, that every driver must pass, each on a scriptable fake
of its harness. A checker looks at every step and names the property a failure breaks, by the ids the plan gives.

Status: phase one is built (2026-11-22): the suite, fakes for Codex, ACP and Pi, the three built-in drivers run against
it, and deliberate faults that show each check catches its bug. Two reviews' findings are fixed (2026-11-22), and so
is the Pi driver's stall on a Pi that stops reading (2026-11-22). Files the agent sends (`send_file`) are covered for
Codex, Pi and the three ACP shapes (2026-11-22). Six suspicions from a read of the drivers' state machines were
tested (2026-11-23): five were real and are fixed, below; Pi's compaction was not (under "Not covered yet").
A review of those fixes found four more, fixed the same day: an ACP approval repeated for one tool call could take
the name of another's first, an ACP request id spelled with escapes was not found, two Pi new sessions asked for at
once were both settled by the first, and Codex did not count a resolved request of another thread.
Phase two, the socket connectors, is built (2026-11-24), below, for the four contracts issue #154 named: chat
isolation, a request settled once, what the next connection is handed, and a connection that drops after
delivery. Everything here is internal: it ships in no program.

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

### Subagents

A driver whose harness starts subagents also runs the scenarios in `src/subagents.rs` (Codex's, today; D-117).
The fake reads four more words: `#spawn` starts one subagent on Codex's newer runtime and waits for it, `#spawn1`
the same on the older runtime, whose subagent's turn starts before the item that names it is complete, `#nest` a
subagent that starts one of its own, and `#outlive` a subagent the turn does not wait for. The prompt's other
words say what the subagent does, as for any turn. The checks are that the chat shows each subagent flat: a
`subagent` tool item, a `subagent` task naming it, the subagent's words naming it as `parent`, all in the chat's
one session.

| Scenario | What happens |
| --- | --- |
| `subagent_in_its_parents_chat` | A subagent's work reaches the chat of the turn that started it, and not another chat served meanwhile |
| `subagent_speaks_first` | The older runtime: what the subagent says before the item that names it is held, and shown |
| `nested_subagents` | A subagent's subagent is shown too, named by the item of the subagent that started it |
| `subagent_asks_for_approval` | A subagent's approval waits in the chat, and the answer reaches the subagent |
| `stop_stops_every_subagent` | Stop while the agent waits for a subagent: both stop, and the Stop is settled once, by the last to end |
| `stop_after_the_turn_has_ended` | Stop naming the agent's ended turn stops the subagent still working under it |
| `stop_stops_nested_subagents` | Stop stops the agent, its subagent and that subagent's subagent, each parent first |
| `dies_with_subagents_running` | The harness dies while two subagents work: every item closes (S2) |
| `new_session_with_subagents_running` | A new session while a subagent of the old one works: it is stopped, and its items close (D-070, D-117) |
| `restart_with_subagents_running` | A restart while a subagent works: the next prompt takes the session up, and nothing of the old subagent is shown |

`replays_subagent_recordings` plays four of `fixtures/recordings/codex/subagents` back through the Codex driver,
with the same checks.

## The fakes

| Fake | From | Shapes | What differs |
| --- | --- | --- | --- |
| Codex | the stand-in in `crates/marmot-agent-codex/tests/driver.rs` | threads, turns, `commandExecution`, approvals, `requestUserInput`, the driver's `send_file` dynamic tool (`item/tool/call`, only on a thread started with it), steer, interrupt, settings, resume, subagents (`subAgentActivity`, `collabAgentToolCall`, `rootTurnId`; an interrupt leaves a turn's subagents running) | one process for every chat |
| ACP | the agent in `crates/marmot-agent-acp/tests/driver.rs` | sessions, prompts, tool calls, `request_permission` (with `#twice`, two at once for one tool call; with `#garbled`, one naming no tool call), `elicitation/create` (with `#garbled`, a form at a web address), a turn that goes on without what the client refused, cancel, config options, MCP servers over HTTP (`src/fake/mcp.rs` is the client: `initialize`, `tools/list`, `tools/call`) | **OpenCode**: `session/resume`; one tool server of a name for every session in a folder, the last handed over, the calling session named in `_meta["ai.opencode/sessionID"]`. **Goose**: `session/load` with a replay, sessions numbered by day, `activeRunId`, and after a crash a run of its own until cancelled; servers kept per session, the caller named in `_meta["agent-session-id"]`. **Cursor**: `session/load`, no name, `cursor/update_todos` naming only its tool call; servers kept per session, the caller not named. Each holds a cancelled turn open while its tool call is out. The driver is run with file sending switched on (`serve_files`, as `send_files = true` does) |
| Pi | `crates/marmot-agent-pi/tests/fake_pi.py` | Pi's records, extension dialogs, the driver's `send_file` tool (only with the driver's extension loaded, asking in its dialog), steering, abort, a session's file written once it holds a message | one process per chat: the driver runs `src/fake/relay.py`, which connects its standard input and output to a socket the test serves, so each Pi is scripted in Rust |

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
  A form (a question) asked again for one tool call was named the same way and is now an item of its own too
  (`a_repeated_request_takes_no_name_a_first_could_have` in the driver's own `tests/cases.rs`, 2026-11-23).
- **The ACP driver never answered a permission or form it could not show** (`a_request_the_driver_cannot_show_is_answered`
  in `tests/acp.rs`, 2026-11-23): one it could not read, or a form at a web address, which it never offers. The
  agent waited for ever. Now it is refused with -32602, invalid params, as the official ACP library refuses a
  request it cannot read.
- **The ACP driver never answered a request naming a session no chat took, said while a session was being picked**
  (`a_request_from_a_session_no_chat_has_is_refused_as_not_found` in the driver's own `tests/driver.rs`,
  2026-11-23). It was held, since the session being picked can speak before the reply that picks it, and dropped
  when the reply named another. Now it is refused with -32002, resource not found, naming the session, as the
  official ACP library refuses a request for a session it does not have (`Error::session_not_found`); so is one
  that comes when no session is being picked, which was refused as an unknown method.

Not covered yet, and why:

- **Codex's greeting** (`Codex::connect`) still waits for `initialize` with no limit. It runs before the core's loop, at
  start. Not changed here.
- **Pi's start** waits up to `STARTUP_LIMIT` (60 s, fixed), so a Pi that never answers `get_state` is not run: too slow.
- **The harness's own actions** (compacting), **group chats** and **attachments**. That Pi ends every compaction it
  starts with `compaction_end`, failed or not, before it responds, is read in `fixtures/recordings/pi/compact.jsonl`
  and Pi 1.6.0's `AgentSession.compact`, not shown by a fake; the Pi driver's own tests
  (`one_compaction_at_a_time_settled_by_its_own_end`) check what it does with both ends.
- **Goose and Cursor** run only the scenarios where they differ from OpenCode.

## Running it

```sh
cargo test -p marmot-agent-conformance            # every driver, the faults, the replays: about 30 s
cargo test -p marmot-agent-conformance --test acp # one driver
```

The Pi tests need `python3` for the relay, as Pi's own tests do. The suite is part of the workspace's tests, so CI's Rust
tests run it on every pull request.

The scenarios are listed once, in `with_scenarios!` (`src/scenarios.rs`). A driver that runs them all makes one test
for each from that list, as `tests/codex.rs` does, so a scenario added there is run at once. A driver that runs only
some (Goose and Cursor) names those it runs and those it leaves out, with the reason; its
`every_scenario_is_run_or_left_out` fails unless the two name every scenario once, and a name that is no scenario does
not compile.

To add a driver: write a `Subject` for it in `tests/`, on a fake of its harness (a `Protocol`), and make its tests
from `with_scenarios!`.

## Phase two: the socket connectors

Built on 2026-11-24, as designed on 2026-11-22 but for what is said here. The Claude Code driver
(`packages/claude-code`), the OpenClaw plugin (`packages/openclaw`) and the Hermes plugin (`python/marmot_hermes`)
reach the core over the socket, and each carries its own copy of the settling logic. They run the suite as **the
shipped connector over the real socket**, as the core runs it, with a scripted harness inside it in place of the real
one, driven through the core's own `SocketDriver`, so the scenarios and the checker stay as they are. No model, no
key and no network.

```text
 Rust test ──Run──▶ SocketDriver ══ relay ══ the socket ══▶ connector (shipped) ──▶ scripted harness (in the connector)
     ▲                                                                                 │
     └──────────────────────── control channel: frames and Heard out, the suite's word in ◀──┘
```

| Part | Where | What it does |
| --- | --- | --- |
| `OnSocket` | `crates/marmot-agent/tests/conformance_socket.rs` | The `Subject`: listens with `Endpoint::in_home` on a throwaway home, starts the connector's entry, takes its connection and `greet`s it through a relay of its own (below); a restart drops the connection, as the core's restart does |
| Hermes's entry | `python/tests/conformance_harness.py` | The plugin's own `MarmotAdapter` and its reconnecting socket client, with Hermes's `gateway` modules stood in for and what Hermes does played by a script |
| Claude Code's entry | `packages/claude-code/test/conformance-harness.ts` | The driver's own `run`, with a scripted `query` in place of the SDK's: `init` at each turn, `canUseTool` for an approval, AskUserQuestion for a question, `interrupt()` |
| OpenClaw's entry | `packages/openclaw/test/conformance-harness.ts`, `test/conformance-sdk.ts` | The plugin as OpenClaw loads it, its account started by `startAccount`, with a scripted channel runtime and plugin SDK: runs, an exec approval through the plugin's approval adapter, `ask_user`, `/stop` |
| Socket scenarios | `src/socket.rs` | `reconnect` and `dropped_after_delivery`, below |

**The control channel.** JSON lines on the connector process's standard input and output, which no connector uses
for anything else (their logs go to standard error). Out: `{"heard": {...}}` in the `Heard` shapes, and
`{"frame": <kind>, "wait": true}` for each call the connector made on its harness, written after what the harness
heard in it. The harness then waits for the suite's word: `{"go": true}`, or `{"die": true}`, when it dies there.
The process ending is the harness dying, unless the suite ended it. A frame is counted as `Fake::frames` counts
them (`Fake::record_frame`), so `Knobs::die_after` applies.

**The relay.** The connection runs through a relay in the test, line by line, which keeps each request of the
core's until its response passes, and notes each event that shows a request's effect: a turn of the prompt's chat
that names the prompt, or an event that settles the answer or the control. While a new knob,
`Knobs::withhold_responses`, is set, it holds the connector's responses back, so they never reach the core; its
events go on. `Fake::drop_connection` drops the connection at both ends, and keeps what was in flight
(`Fake::dropped`). So a drop after delivery and before the response is one, whenever the connector responds: the
scenario drops the connection once the harness has had the request and the connector has responded, and checks
that the request was the one in flight, its response held back.

**Each connector, as it lives.** Hermes and OpenClaw run inside their gateway, which goes on when the connection
drops: they connect again, and the next `start` takes that connection. Claude Code's connector ends with its
connection, and the next is started, as the core starts it; its scripted Claude Code takes up a session it is
asked to resume.

**What each runs** (`conformance!` in the test):

| Connector | Scenarios | Left out |
| --- | --- | --- |
| Hermes | `one_turn`, `two_chats`, `answered_once`, `reconnect`, `dropped_after_delivery` | `restart`: Hermes keeps its own sessions (`sessions: own`), so the core never names one, and the scenario checks that a named one is taken up |
| Claude Code | the same, and `restart` | |
| OpenClaw | as Hermes, with `dropped_after_delivery` run as `openclaw_dropped_after_delivery`, which also requires that the event settling the answer reached the core before the drop | as Hermes |

The rest of `with_scenarios!` is not run yet: the faults on the harness's pipe (pieces, half frames, a harness that
stops reading) do not apply, as these harnesses are libraries called in process, and deaths at each step, files,
settings and new sessions are not scripted yet.

| Scenario | What happens |
| --- | --- |
| `reconnect` | A turn, then an approval waits in another chat, and the connection drops: nothing of the first connection is settled on the next (S1) or shown there again (S13), an answer to the approval asked before is refused (S11), and the first chat's next prompt is answered |
| `dropped_after_delivery` | The connection drops once the harness has a prompt and the connector has responded, its response held back by the relay; a drop that does not happen there, or finds anything else in flight, fails the scenario (L3). The prompt is taken exactly when an event the core read before the drop showed it (S1, D-147). The connector connects again, and the chat's next prompt is answered, with the first heard once (S4). Then the same for an answer to an approval: whatever the core was told, it reached the harness once (S11), and nothing the old connection was handed is settled on the next (S1) |

`Run` names a turn's prompts on its later events as its first event named them, as the core keeps them
(`started_by`): the socket connectors name a turn's prompts only on its first event, and the suite's scenarios
wait for a turn's end by its prompt.

Found, each fixed after its scenario failed:

- **The core counted a request as failed when its connection dropped after the event that showed its effect**
  (`dropped_after_delivery`, OpenClaw). The plugin sends the event that settles an answer before its response; a
  connection that dropped between them had the core refuse the answer as failed, then hand on the event that
  settled it: settled twice (S1), and the chat told the answer failed though it was taken. The same held for a
  prompt whose turn had started. Now `SocketDriver` counts a request as taken once an event it read while waiting
  for the response settles it, or, for a prompt, is a turn of the prompt's chat that names it
  ([D-147](../../docs/decisions.md#d-147-a-request-whose-effect-a-connector-showed-before-its-connection-dropped-was-taken)).
- **OpenClaw's plugin settled an interrupt with a turn end it made up** (`two_chats`). A stopped OpenClaw run
  reports no outcome, so the plugin closes the turn on OpenClaw's reply to `/stop`, but marked the end as its own,
  with no native message, and an event a driver makes up shows no effect (S1, D-059). The end now carries that
  reply, the line it follows from, in the plugin's translator and the prototype; `stop.jsonl`'s expected events
  changed by that one native message.

Shown to catch a connector put wrong by hand: Hermes's plugin with its `core_gone` keeping the items the old core
closed open, which `reconnect` reports as S11. And shown to catch a relay put wrong by hand: with the drop left
out, `dropped_after_delivery` fails for each of the three connectors (L3); before the relay, it passed for each.

Not a bug, and left as it is: when its connection drops, Hermes's plugin cancels a request it is still handing to
Hermes (`core.py`), so a prompt then perhaps never reaches Hermes. The core already counts such a prompt as perhaps
not delivered (D-129). The scenario drops the connection only once the harness has heard the request.

**CI.** One job, `Conformance (socket)`, beside the Rust tests: Node and Python at the versions CI pins, both
packages' `npm ci`, the core crate's build from the core's cache, and this test alone, which is ignored in the
core's tests. About a minute once built.

```sh
cargo test --locked --all-features -p marmot-agent --test conformance_socket -- --ignored
```
