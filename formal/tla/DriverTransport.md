# DriverTransport: a TLA+ model of one driver's side

Status: built and checked with TLC on small bounds, as
[D-093](../../docs/decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top) and
[the verification page](../../docs/design/verification.md) plan. The model, its bounds and its switches are
internal and change as the work goes. Read on 2026-11-23 against master `d98d3d4` (#118), on branch
`tla-driver-transport`.

`DriverTransport.tla` models one driver between the core and its harness, for any harness: the core's answers
and controls in flight, the effects that settle them, the harness's own requests (approvals and questions) and
who answers them, a dropped wait, a harness that dies and starts again, and two chats on one harness. It owns
S1, S5, S11, S13, L1 and L2 for the built-in drivers (`crates/marmot-agent-codex`, `crates/marmot-agent-acp`,
`crates/marmot-agent-pi`, behind the trait in `crates/marmot-agent-driver`). Each result below is stated with its
bounds; none of it says a driver is verified.

A configuration picks the harness's shape, so one model follows three drivers:

| Shape | Configuration | What differs |
| --- | --- | --- |
| Codex | `DriverTransport.codex.cfg` | One `codex app-server` for every chat; Codex resolves a request by itself and says so (`serverRequest/resolved`), and says so after a turn's end for each request the end dropped, as the recording `approval_unanswered` shows; an approval's item is named after Codex's request |
| ACP | `DriverTransport.acp.cfg` | One agent for every chat; the driver answers at once a request it cannot show (-32602) and one from a session no chat has (-32601), and, when it interrupts, each request still waiting, as cancelled (`cancel_replies`); an approval's item is named after its session and tool call (`approval:{session}:{call}`), a repeat after its request (`approval+…`) |
| Pi | `DriverTransport.pi.cfg` | A Pi for each chat, started again on the chat's session when it dies (`ready_for`), its lines from an earlier Pi dropped; Pi drops a dialog when its turn ends and says nothing of it |
| Two chats | `DriverTransport.chats.cfg` | ACP's shape, with two chats on one agent, each asked about the same tool call |

## What is modelled

| Action | In the code |
| --- | --- |
| `Answer(r, c, item)` | `Driver::answer`: `chat_with_item` finds the chat whose translator holds the item, and the translator's `reply` builds the reply to the request it waits on; written whole (Codex's `send` and `flush`, ACP's `write_whole`, Pi's `hand`), then fed back (`Direction::Out`), which makes the item's edit that settles the answer. Refused when no translator holds the request, or the item no longer waits (`NotWaiting`, `AlreadyAnswered`, `UnknownItem`), or the harness cannot read it (`Failed`). The person answers an item their chat showed, which may no longer wait |
| `Interrupt(r, c, turn)` | `Driver::control` with `Action::Interrupt`: taken when the turn named is the chat's running turn, its effect `Effect::TurnEnded` watched for from the moment it is written. ACP's `cancel` first answers every request still waiting as cancelled (`cancel_replies`), and marks the chat `cancelling` |
| `NewSession(r, c, named)` | `Driver::control` with `Action::NewSession`: taken while the chat is idle and no session is being picked for it (Codex's `ask` waits while Codex has yet to answer, ACP's `picked`, Pi's busy refusal, #118). Its effect, `Effect::NewSession(old)`, is a session other than the chat's own as the driver knows it, or else than the one the core named (#118). The core names the session it saved, or none: the model lets it name none or any session the chat has had |
| `Pick(c)`, `Prompt(c)` | `Driver::prompt`: a chat with no session first picks one (Codex's and ACP's `pick`, Pi's `ready_for`), the one the core saved after a restart (D-065); the prompt starts a turn. The driver counts the turn from the moment it writes the prompt (ACP's `session/prompt`, Codex's `turn/start` until Codex answers it). Prompts themselves are `CoreLifecycle`'s |
| `Route(p)` | Codex's and ACP's `route`, Pi's `handle`: one line read from the harness and handled in one step. A session the harness opened becomes the chat's and settles the new-session controls whose old session it is not (`ready_event`); a turn's start; a request of the harness, made an item that waits, or answered at once (not shown, from a session no chat has, or while ACP cancels), or held while a session is picked (`early`) and dropped once no pick is left; `serverRequest/resolved` (the driver forgets the request, #118); a turn's end, which closes the items waiting in it (#118) and settles an interrupt of it; a session item shown again; the end of the harness's output (`stopped`) |
| `DropWait(p)` | Only with the defect `LoseOnDrop`: a line taken off the pipe and the wait it was read in dropped before it is handled |
| `Restart`, `Spawn(c)` | A driver whose one harness died has ended, and the core starts a new one (`Codex::start`, `Acp::start`) that knows nothing of what came before. Pi's driver starts a new Pi for the chat whose Pi stopped (`ready_for`) |
| `HarnessReads(p)` | The harness reads the next line the driver wrote: a session to open or take up, a prompt, a reply to one of its requests (counted, and taken only while it waits), an interrupt |
| `HarnessAsks(a, s, k, shown)` | An approval or question (Codex's `requestApproval` and `requestUserInput`, ACP's `session/request_permission` and `elicitation/create`, Pi's `extension_ui_request`), in a session whose turn runs, or in a session no chat has; about tool call `k`; one the driver can show or not |
| `HarnessResolves(a)` | Codex resolves a request by itself and says so (`serverRequest/resolved`) |
| `HarnessEndsTurn(s)` | A turn ends by itself once nothing it asked waits, or once told to stop; what it still asked is dropped (Codex says so of each after the turn's end) |
| `HarnessEdits(s)` | The harness shows a session item again unasked: a setting it changed, or the item again |
| `HarnessDies(p)` | A harness process dies: what it was sent and had not read is lost, what it asked is dropped, and its output ends after what it wrote |

The pipes are in order both ways. What the core sees is not queued: an item or a turn the driver shows can be
answered or interrupted from then on.

## What is not

- **Prompts and what they deliver** (S4, S12): `CoreLifecycle`'s. Here a prompt only starts a turn; one sent
  during a turn is not modelled (the driver sends none).
- **The core's side of a settle.** The core settles a request once whatever the driver says
  (`CoreLifecycle`'s S1); this model checks what the driver says, before the core.
- **Rights and people** (S6), **sizes** (S7), **files** (D-090) and **the socket** (`SocketProtocol`'s).
- **Settings and the harness's own actions** (`Action::Set`, `Action::Own`): their effects are an edit of the
  session item showing a value, and a compaction's end; neither is a session or a turn.
- **Time.** No clock: a request that waits for the harness past its patience (D-066) is not modelled; the
  harness either reads, or dies.
- **How the harness names its turns and requests**: the model gives each a fresh name, and trusts the harness to
  (`ReaskAfterRestart` aside).

## Trusted assumptions

| Switch | When on | Shown when off, or on |
| --- | --- | --- |
| `FairHarness` | The harness eventually reads what the driver wrote, and ends a turn it was told to stop | off: an interrupt never settled (`driver-unfair-harness-never-reads`) |
| `ReaskAfterRestart` (off) | The harness never asks again about a tool call it asked about before it died, in the session it takes up again | on: a reused item id in ACP's shape (`driver-reask-after-restart`) |
| `Unshowable` (off for Codex and Pi) | The harness asks only what the driver can show | on, with Codex's driver: a request never answered (`driver-codex-unshown`) |
| `StrayAsks` (off) | The harness never asks in a session no chat has | on: requests never answered in ACP's shape (`driver-stray-while-picking`) and Codex's (`driver-codex-stray`) |

Besides the switches: each `Route` is one step, as each driver reads a whole line (`next_line`, safe to drop)
and handles it without waiting on anything (Codex's `route` and Pi's `handle` are not `async`; ACP's `write`
hands a line to its writer task without waiting); the core never drops a request's future with the driver
kept (the trait's contract, D-066); Pi's dialogs are the four its driver shows (`DIALOGS`), not checked
against Pi here (so `Unshowable` is off for Pi); a request's item name, in the Codex and Pi shapes, is unique
because the harness's request ids are; in ACP's, a tool call's id is unique within its session (ACP's
schema says so).

## Properties

| Id | Property | Form | From |
| --- | --- | --- | --- |
| S1 | Every answer and control is settled at most once, and only by an event that shows its own effect, by a refusal, or as failed when the driver or its harness ends | invariant `S1` | D-028, D-058, D-087 |
| S5 | An answer reaches only a request of the chat it was given in | invariant `S5` | D-060 |
| S11 | A request the driver takes was written whole to a harness that can read it before the driver returns; an answer reaches the harness at most once, and only while, as far as the driver has read, the harness still waits for it | invariant `S11` | D-066 |
| S13 | No item name is used for two requests of the harness in one chat, across sessions and restarts | invariant `S13` | D-059 |
| L1 | Every answer and control the driver takes is eventually settled | `~>` | D-028, D-058 |
| L2 | Every request the harness makes is eventually answered or stops waiting, unless it waits on a person: one still open for ever is one the driver holds for an answer for ever. A turn waits on what it asked, so this is what lets it end | `<>[] => <>[]` | D-058, D-066 |

S5 for events holds by construction: `route` gives a line to the chat that has its session. An answer that
crosses the turn's end on the pipes (written before the driver read the end) reaches a harness that has stopped
waiting; S11 allows it, as nothing in the driver could know, and the answer shows as given.

The liveness properties hold under weak fairness for the driver (`FairDriver`: each `Route`, and `Restart`) and
for the harness (`FairHarness`: each `HarnessReads`, and the end of a turn told to stop). Deaths, prompts and
the harness's requests are bounded, so they end.

## Configurations and results

Checked on 2026-11-23 with TLC v1.7.4 (Java 21, in the pinned image) by `tools/tla/check.py`, as `CoreLifecycle`
is: the cases that must pass on 4 workers, those that must fail on one. The small bounds, every pull request:
**one harness request or two, one answer `r1`, one interrupt `r2`, one new-session control `r3`, two prompts,
one death of a harness process, one session item shown again unasked (none in Pi's shape), and up to five
sessions.** Each configuration says how it differs.

| Case | Configuration | Expected | Distinct states | Counterexample |
| --- | --- | --- | ---: | --- |
| `driver-codex` | `DriverTransport.codex.cfg` (one chat, two requests of the harness) | every property holds | 425,502 | |
| `driver-acp` | `DriverTransport.acp.cfg` (one chat, two requests for one tool call) | every property holds | 297,609 | |
| `driver-pi` | `DriverTransport.pi.cfg` (two chats, a Pi each, one request) | every property holds | 179,931 | |
| `driver-chats` | `DriverTransport.chats.cfg` (two chats on one agent, two requests, no death or new session) | every property holds | 22,203 | |
| `driver-unfair` | `DriverTransport.unfair.cfg` (Codex's, one request, no `FairHarness`) | L1 fails | 6,556 | [`driver-unfair-harness-never-reads`](counterexamples/driver-unfair-harness-never-reads.json) |
| `driver-stray-while-picking` | `DriverTransport.stray.cfg` (ACP's, one request, `StrayAsks`) | L2 fails | 13,918 | [`driver-stray-while-picking`](counterexamples/driver-stray-while-picking.json) |
| `driver-codex-stray` | Codex's, one request, `StrayAsks` | L2 fails | 14,289 | [`driver-codex-stray`](counterexamples/driver-codex-stray.json) |
| `driver-codex-unshown` | Codex's, one request, `Unshowable` | L2 fails | 13,813 | [`driver-codex-unshown`](counterexamples/driver-codex-unshown.json) |
| `driver-reask-after-restart` | ACP's, `ReaskAfterRestart`, safety only | S13 fails | 79,366 | [`driver-reask-after-restart`](counterexamples/driver-reask-after-restart.json) |

The counterexamples are schedules in this model's action names, made from TLC's traces by
`tools/tla/schedule_driver.py`; [their README](counterexamples/README.md) says how.

**Each check catches its bug.** Each defect is a value of the constant `Bug`; with it, TLC must report the
property named, on the configuration named, with only that property. The first six put back what #118 fixed:

| `Bug` | What it puts back | Configuration | Property | Distinct states to the violation |
| --- | --- | --- | --- | ---: |
| `AnswerAfterTurnEnd` | Pi: a question closed with its turn still takes an answer, written to a Pi that dropped the dialog, and never settled | Pi's | S11 | 8,985 |
| `ResolvedIgnored` | Codex: `serverRequest/resolved` ignored, so a request Codex resolved is still answered | Codex's | S11 | 2,270 |
| `SettleOnAnySession` | ACP and Pi: a new session settled by any session item other than the one the core named, so an edit of the current session settles it | ACP's | S1 | 93 |
| `TwoNewSessions` | Pi: a second new-session control taken while one waits, and both settled by the first new session | Pi's, with `r4` a second new-session control | S1 | 255 |
| `OneItemPerCall` | ACP: two permission requests for one tool call share one item | ACP's | S13 | 1,432 |
| `UnshownUnanswered` | ACP: a request the driver cannot show is never answered | ACP's | L2 | 11,304 |
| `CallOnly` | An approval named after its tool call alone, so two chats' approvals share a name and one chat's answer reaches the other's | two chats | S5 | 7,507 |
| `TakeUnwritten` | An answer taken, and shown as given, when the harness can no longer read it (the ACP driver's bug the conformance suite found) | Codex's | S11 | 1,057 |
| `LoseOnDrop` | A line read from the harness lost when the core drops the wait it was read in | Codex's | L1 | 6,555 |

**No empty passes.** Each probe says an interesting state is never reached, and TLC must find it false. On
Codex's shape: `NeverAnswered` (an answer reaches a waiting request), `NeverAnswerRefused` (an answer is
refused), `NeverInterruptSettled` and `NeverNewSessionSettled` (each by its effect), `NeverResolved` (the
driver reads that Codex resolved a request), `NeverLateReply` (a reply reaches a harness that stopped waiting:
the race the pipes allow), `NeverRestarted` (a chat has a session again after the harness died). On ACP's:
`NeverEditWhileNewSession` (an edit of the old session is next on the pipe while a new-session control waits),
`NeverRepeatAsk` (two requests for one tool call are two items), `NeverUnshownAnswered` (a request the driver
cannot show is answered), `NeverCancelled` (a waiting request is answered as cancelled). On Pi's:
`NeverClosedAtTurnEnd` (an item closed by its turn's end), `NeverRestarted` (a chat has its session in a new
Pi), `NeverFailed` (a control settled as failed). On two chats: `NeverTwoChats` (both chats' turns run at once).
With `StrayAsks`: `NeverStrayAnswered` (a request from a session no chat has is answered). All sixteen are
reached within 2,300 distinct states.

All 35 DriverTransport cases take about 5½ minutes with 4 workers here, on a machine busy with other work (the
Codex shape 2 minutes, ACP's 1, Pi's 50 s, two chats 10 s, every failing case under 10 s); CI runs them on every
pull request in a job of their own (`--model DriverTransport`), beside `CoreLifecycle`'s two, under D-094's 15
minutes.

**Larger bounds**, safety only, the same day with 8 workers: Codex's shape with two chats, one request of the
harness, and everything else as `DriverTransport.codex.cfg` (a death, a new session, a session item shown
again): every invariant holds, 3,117,919 distinct states, 46 s. Liveness at that size, and the other shapes with
two chats and a death, are for a nightly run, which is not set up yet.

## What the model found

Three requests of the harness that today's drivers never answer, each a committed counterexample and an
ignored test that fails against the driver. **None is fixed here.**

- **ACP: a request from a session no chat has, read while a session is picked, is dropped unanswered**
  (`driver-stray-while-picking`, L2). `route` answers such a request at once (-32601), unless some chat's
  session is being picked: then it holds it in `early`, in case the pick makes its session known. Once no pick
  is left, `early` is cleared, counted as unrouted, and the request is never answered, so the agent waits for
  it for ever. Test: `a_request_from_a_session_no_chat_has_is_answered_while_a_session_is_picked` in
  `crates/marmot-agent-acp/tests/driver.rs` (`--ignored`), which passes when the request comes outside a pick.
- **Codex: a request for a thread no chat has is never answered** (`driver-codex-stray`, L2). `route` refuses
  a call of a tool for no chat, and counts anything else as unrouted, approvals and questions included. Codex
  waits for a reply to each. With subagents off, a thread no chat has is one the driver left, or one Codex
  started by itself (inferred; no recording shows one asking). Test:
  `a_request_for_a_thread_no_chat_has_is_answered` in `crates/marmot-agent-codex/tests/driver.rs`.
- **Codex: a request the driver cannot show is never answered** (`driver-codex-unshown`, L2). The translator
  declines a form it cannot show, but a question with two questions of one id, or a request whose params it
  cannot read, is left unanswered (`translate.rs` `ask`, `read`), and shows nothing that waits, so the turn
  waits until someone interrupts it. Test: `a_question_the_driver_cannot_show_is_answered` in
  `crates/marmot-agent-codex/tests/driver.rs`.

One risk, resting on what an agent does after a restart, which no recording shows: **ACP's item names start
again with the driver** (`driver-reask-after-restart`, S13). A new driver has forgotten the names it gave, so an
agent that asks again about a tool call in a session taken up after a restart (Goose carries on a turn a restart
cut off) gets `approval:{session}:{call}` again, an item id the chat has had. The passing configurations assume
it does not (`ReaskAfterRestart` off).

## Running

```sh
python3 tools/tla/check.py --model DriverTransport          # every DriverTransport case
python3 tools/tla/check.py --only driver-acp                 # one case
python3 tools/tla/check.py --model DriverTransport --counterexamples   # writes its schedules again
tools/tla/run.sh formal/tla DriverTransport.tla DriverTransport.codex.cfg   # TLC on one configuration
```
