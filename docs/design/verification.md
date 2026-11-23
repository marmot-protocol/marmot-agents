# Testing and verification

Status: the approach is decided
([D-093](../decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top)), and
so is the CI budget ([D-094](../decisions.md#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly)),
on 2026-11-21, and CI's three tiers ([D-130](../decisions.md#d-130-ci-runs-in-three-tiers-with-one-required-check)),
on 2026-11-23, built with the nightly workflow the same day. The property list, the models and the layers below are the plan; their details are internal and
change as the work goes. It comes from reading all the code after M3 and the M4 work in progress, and from how MDK
tests itself (its simulator, its TLA+ and Tamarin models, and its testing rules). Under
[first, fix what is known](#first-fix-what-is-known), the behaviour the fixes made visible is provisional: built,
and waiting for the owner. Of the models, `CoreLifecycle` is built (2026-11-22), and so are `SocketProtocol` and
`DriverTransport` (2026-11-23); the others are planned. The
conformance suite's first phase, for the built-in drivers, is built (2026-11-22); its second, for the socket
connectors, is designed. The core's sends went over to MDK 0.19.0's token-based sends on 2026-11-22, which ends
the duplicate a restart could make; how often a send is tried is decided
([D-109](../decisions.md#d-109-how-the-cores-sends-survive-a-crash-as-built)). The same day the core moved to MDK
master after 0.19.0 (`7f7dcc6ca`) and reads each chat from its mark in MDK's insertion order. The
[model double](#11-a-model-double) is decided ([D-127](../decisions.md#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key), 2026-11-23): real harnesses run in CI on it,
with no model key in CI.

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
| S3 | An item's versions rise by one, and every later version is an edit of its first event; MDK reads each one | D-025, D-112 |
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
- **Sends that give up** (S1, S3). **Fixed.** The core stopped trying a send after five failures, which lost
  settles and started a new row for the next version. A send whose outcome was not known, as when the wait for
  MDK ran out, may have gone out, and was sent again blindly. Now a send is kept until it goes, and is handed to
  MDK under a token saved with it before MDK has it (MDK 0.19.0's `submit_tagged_text`,
  `submit_tagged_edit_for_local_send` and `submit_custom_event`). MDK keeps the send under that token before it
  answers: tried again under it, after an answer that was lost or a restart, the send gives back the event MDK
  made of it and sends nothing. So a send whose outcome is not known is simply tried again, under the same token,
  a little later each time, with no limit. A try MDK took and dropped before it went out (its status
  `Rejected`) sent nothing; MDK refuses its token from then on, so the next try is under a new one, and after
  ten such tries the send is given up. One MDK refuses is dropped with a notice. Review of the move found four
  more cases, each fixed with a test that failed first, against MDK itself where it is MDK's behaviour. MDK makes
  an event's id from its kind, tags, content and second, and keeps the id of every send it took, even one it
  dropped: a dropped send tried again in the same second under its new token, or a second send of the same
  words in one second, met that id and was given up as refused; now that is MDK busy for the second, and the
  send goes in the next (the tests make MDK drop sends for real, while the agent's own disband of a group is
  under way). A send to a group being disbanded, or one MDK does not have, was tried for ever, and nothing
  after it in the chat went; now, when MDK holds nothing under the token, it is refused for good. The core
  waited in its one loop, up to a minute, for MDK to take each send into its engine, so a send MDK kept queued
  held up every chat and a stop; now MDK is asked once per try, and a group whose send MDK has queued asks again
  through its retry timer while the rest goes on. And the tenth drop of a send was saved before its give-up, in
  a second write; now the two are saved together. A state file that has sends handed to MDK and no salt, which
  only an unreleased core from before tokens could write, stops the core at start rather than risk sending one
  twice.

  **The limit MDK's message retention sets.** MDK forgets a token with the message it made: when a group's
  messages disappear (a period any admin of the group may set, with no lower bound), the token goes with them,
  and a send tried again under it after that would be a new one. So the core keeps, with each send, when it was
  first tried under its token, and once the group's period has passed since then it asks MDK whether it still
  knows the token before it tries again. If MDK does not, the send may have gone out: it is not tried again, and
  is given up as not known, with a `failed` notice that says it may not have been delivered. The period read is
  the group's now; a period made longer after a send went out, while its outcome was not known, is not seen,
  and such a send could go out twice. That needs a lost answer or a crash, a restart later than the old period,
  and the period changed in between; it is accepted as a limit
  ([D-109](../decisions.md#d-109-how-the-cores-sends-survive-a-crash-as-built)).

  **What a restart could not recover, until MDK 0.19.0.** Before, the core looked a send whose outcome was not
  known for in what MDK stored, by its id, version and first event for an item, and by its text and tags for the
  agent's words. A crash between MDK storing a send and the core saving that it went, with a lookup that could
  not tell the send from an earlier one, or that came before MDK had stored it, sent it again, and the chat
  showed a duplicate first event. The token ends that: nothing is looked for any more. The model now assumes
  what MDK's tokens give (`TokenDedupe`) and shows the duplicate without it
  ([`duplicate-first-event`](../../formal/tla/counterexamples/duplicate-first-event.json)); the simulator's
  `late` family, a send that lands long after it was handed over, found the duplicate before and finds none now,
  and is part of the gate. Two things keep no token: an edit of words an older core sent without one, which goes
  as before and, sent again, shows the same text; and a file the agent sends (D-090), whose token form in MDK
  carries no tags, so the file would lose its turn. A file is not tried again after a failure that may have come
  after MDK took it, as before.
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
  message heard live (since replaced: see the next entry), and waiting groups are read again in order.
- **A new group can start at its history** (L3). **Fixed** on 2026-11-22. The test for the new group above failed
  now and then (about 1 run in 6): a message stored while the core was down reached the driver as a prompt. MDK
  stores a message some time after it arrived, and announces a sync's messages live only once it has stored them
  all, so neither where MDK says a group ends nor which message the core hears first tells history from what is new.
  Now that is told by MDK's own time of arrival, in whole seconds. For a group with no read mark, every message that
  arrived by the second the core subscribed to MDK is history, marked read and never a prompt; every one that
  arrived after is read, once, in storage order, whatever order MDK stored or announced them in. The core reads such
  a group from its start, and starts it just before the first new message. The second is taken where the program
  subscribes, before MDK starts and before the harness starts or a connector comes, so a prompt that arrives while a
  slow harness starts is read. A message that arrived in that very second is taken as history
  ([D-111](../decisions.md#d-111-a-group-the-core-has-never-read-starts-after-what-arrived-before-the-core-did)). Review found three ways the first fix still let one through, each with a test that failed first:
  history stored after a new prompt reached the driver; the second was taken only once the harness was up, so a
  prompt that came while it started was dropped; and a read that succeeded after failing jumped the mark past a
  prompt it had found, whose announcement then came too late. A later review found a fourth: the second was kept
  only for the run, so a core that stopped after saving where a new group starts and before it could read the group
  left the next run to read, as a prompt, history stored after a new prompt. The state file now keeps the second
  with the group's first mark, and every run after goes by it (`history_is_history_after_a_restart`, which failed
  first). The simulator does not reach this: its groups always
  start from an invitation, with a mark. The import test's check that the agent answers after the move waited for
  MDK: on an imported home, MDK up to 0.19.0 delivered nothing sent once the core had started, even after the pause
  meant to avoid [MDK #3602](https://github.com/marmot-protocol/mdk/issues/3602), and the check passed before only
  because its message arrived in the second the core started, which is now history. MDK's fix for #3602 is in the
  revision the core uses since 2026-11-22 (`7f7dcc6ca`): the check passed 15 runs of 15 there and runs again, as do
  the two tests of a core stopped just after, or five seconds after, a message (`tests/stop.rs`, each 15 of 15).
  The import tests no longer pause before they stop `wn-agent`: stopped at once after the person's last message,
  each of the six that run it, in `tests/import.rs` and `tests/import_coding.rs`, passed 10 runs of 10.
- **A live notice could move the mark past a message** (S10, L3). **Fixed** on 2026-11-22, on MDK master after
  0.19.0 (`7f7dcc6ca`), which can page a chat forward from a saved place in the order messages were stored here
  ([MDK #3603](https://github.com/marmot-protocol/mdk/issues/3603)). The core moved a chat's read mark to the place of
  each message MDK announced live. MDK does not promise to announce messages in the order it stored them, can
  announce one again in its recovery, and stores some with no announcement, as an import does; so a message
  announced before one stored earlier passed it, and the earlier one was never read. A catch-up read all of a chat
  each time. Now a notice only wakes the chat's reader, which reads from the mark, a page of 100 at a time, handling
  each message once and moving the mark past it and past the agent's own messages; a notice of a message already
  read reads nothing. A chat hands on at most 8 messages in each turn of the core's loop and is read on from its
  mark in the next, so a long catch-up for a harness slow to take prompts holds up no stop, no driver output and no
  other chat (`a_long_catch_up_lets_a_stop_through`, which failed first: the stop waited 59 s). The core's 10 s tick reads every chat from its mark too, for a message stored with no notice.
  The simulator's MDK now keeps only that contract: a message heard live brings no earlier one with it, those still
  to be heard come in any order, and `Reannounce` announces a stored one again (version 4 of `live`, 5 of `mixed`).
  On the core before the fix, 163 of 200 `live` runs and 122 of 200 `mixed` runs (seed 0) failed L1 or L3; after,
  every family passes 200 of 200, `disorder` too. The bug switch `mark_from_live` puts the old way back, and the
  simulator finds it. In `CoreLifecycle` the switch `LiveInOrder` is gone, live messages are taken in any order, and
  the deliberate defect `MarkFromLive` fails S10. Three tests on a stand-in MDK
  (`crates/marmot-agent/tests/new_groups.rs`) failed on the old way first.
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
- **Requests of the harness that are never answered** (L2). Found on 2026-11-23 by the TLA+ model
  (`DriverTransport`), and each shown by a test that fails against the driver. The ACP driver held a request
  from a session no chat has while a chat's session was picked, and dropped it unanswered once the pick was
  done: **fixed in #122**, which refuses it with -32002 (resource not found); the model's ACP configuration with
  such requests now passes, and the defect `HeldDropped` puts the old way back. The Codex driver never answered
  a request for a thread no chat has, nor one it could not show (a question with two questions of one id, or
  params it cannot read), and Codex waits for a reply to each: **fixed on 2026-11-23**, by refusing each with an
  error, which Codex takes as a refusal; the defects `StrayUnanswered` and `UnshownUnanswered` put the old way
  back. See
  [the model's README](../../formal/tla/DriverTransport.md#what-the-model-found).
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
- **The state file** (`marmot-agent-state.json` in the home) keeps, besides what it kept before: a random salt,
  made once, that is part of every token handed to MDK, so a core whose state file was lost never reuses a token
  MDK holds; the sends not yet made, oldest first, each with a name that stays the same across tries, how many
  of its tries MDK dropped (the token MDK keeps it under is a digest of the salt, the name and that count),
  whether it was handed to MDK, when it was first handed to MDK under its token now,
  whether it stands in for a dropped send (and so is the last), whether it is a `failed` notice that tells of
  a dropped send, for each item whether the latest version of it that went out closed it, and,
  for a turn's version that settles what the agent's words showed, the send of those words; where recently
  forgotten items stood; for a turn, the message that started it; and for the agent's words, the requests they
  settle, held until a message carries them, and the token their first message went under, which an edit names.
- **How often a send is tried.** One whose outcome is not known is tried again under the same token with no
  limit, a little later each time up to ten seconds; one MDK took and dropped is tried again under a new token,
  and given up, with a `failed` notice, after ten such tries. MDK is asked once per try whether its engine has
  taken a send; while MDK has it queued the group asks again a little later each time, up to half a second, and
  a stop waits up to a minute in all for what MDK has queued. A send MDK no longer knows the token of, once the
  group's messages have disappeared since its first try, is given up with a `failed` notice that says
  "Something the agent sent may not have been delivered to the chat."
- **A state file with sends handed to MDK and no salt stops the core at start**, saying to move it away.
- **A group's read mark is saved before the agent accepts the group.**
- **One chat sends at most eight events in each turn of the core's loop**, then the others go; a stop sends all
  it can.

## The layers

From cheapest to most thorough. Each lists what it owns.

### 1. Unit and example tests (kept)

The replay tests, hand-written cases and stand-in harnesses stay. Each regression from review keeps its test. New
rule: every unit test of a state machine checks the properties above on every step, not only the end state.

**The socket's messages are checked in every language** (built 2026-11-23). The core writes and reads them through
types, and the JSON Schema made from those types is committed with an example of each message,
[`fixtures/socket`](../../fixtures/socket/schema.json); the core's tests fail when either falls behind. The Claude
Code and OpenClaw packages and the Hermes plugin check, in their own tests, that every message they send and every
event their translators make is one the core reads, that their fake cores send only what the core sends, and that
they read only fields the core sends. So a field changed on one side only fails a test.

### 2. A conformance suite for every driver

One suite, written once against the driver trait, that every driver must pass: Codex, ACP (OpenCode, Goose,
Cursor), Pi, and the socket connectors (Claude Code, OpenClaw, Hermes) through a real connector on the socket with
a fake harness behind it. It owns S1, S2, S5, S11, S12 and S13 for drivers. Three connectors carry their own copy
of the settling logic today; the suite checks all of them the same way.

Each driver gets a scriptable fake harness, grown from the stand-ins that exist (`fake_pi.py`, ACP's in-memory
agent, Codex's fake), that can: write a reply short, answer late or never, answer out of order, die at any step,
serve two chats at once, and replay a recording.

**Built**, phase one (2026-11-22): [`crates/marmot-agent-conformance`](../../crates/marmot-agent-conformance/README.md).
Codex, ACP (OpenCode-, Goose- and Cursor-shaped) and Pi run 17 scenarios on fakes of their harnesses (Goose and
Cursor the ones where they differ), a death at each frame an approval's turn writes, and seven recordings replayed
through the transport, which play on only after the recorded answer. The checker names each failure by its
property: S1, S2, S5, S11, S12, S13, and S4 and `seq` for drivers besides. Twenty-two deliberate faults, put in by a
wrapper driver, are each caught as the property they break, by the check meant for them. One scenario, `send_file`,
covers the file tool of every built-in driver (Codex, Pi and the three ACP shapes, with the ACP driver's listener on, as it is by default,
[D-110](../decisions.md#d-110-an-acp-agent-gets-the-file-tool-from-a-loopback-mcp-server-on-by-default)), with five faults of its own. It found five driver
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
stored and what the harness saw. Its MDK refuses an item that MDK's own reading of an item refuses
(`cgka_traits::agent_events`, at the revision the core depends on), and S3 fails on any such send. Runs are named `family/vVERSION/SEED/INDEX`; a failure leaves a capsule that
replays exactly, shrunk greedily. Seven families form the gate and run on every pull request. One more shows
what lies beyond an assumption: `disorder` (a message heard before MDK stores it, which MDK does not do) lost
messages until the core read every chat from its mark on its tick, and loses none since. `late` (a send that lands long after it was handed over) found the accepted duplicate
until the sends took MDK's tokens, and joined the gate then.
Four bugs fixed before are put back behind a test-only feature, and each is found. Bounded result, 2026-11-22:
36,000 runs of the gate (6 families, 1,000 runs each, seeds 0 to 5), one or two chats, up to about 120 steps
each, all passing. After `ReadFails` joined `live` and `mixed` (version 3 of each), the same day: 18,000 runs
(seeds 1 to 3), all passing; after it gained a failed list of groups, 6,000 runs of `live` and `mixed` (seeds 1 to
3), all passing. After the sends took MDK's tokens (version 2 of `sends`, 4 of `mixed`, and `late` in the
gate): 21,000 runs (7 families, 1,000 runs each, seeds 0 to 2), all passing. After the core read each chat from its mark in
MDK's insertion order (version 4 of `live`, 5 of `mixed`, and `Reannounce`): 17,600 runs (8 families with
`disorder`, 200 runs each at seed 0 and 1,000 each at seeds 1 and 2), all passing. Not yet: the connector that
drops mid-request and the one that reconnects (left to the conformance suite, which owns the socket), files, and
the second lane on MDK's real relay.

### 4. Formal models

**TLA+**, checked with TLC, as MDK does, so both projects use one tool and one way of working. Each model has a
fair configuration that must pass, and an unfair one that must produce a counterexample, which is committed as a
simulator scenario. A test checks that the model's actions and the code's still match.

| Model | What it covers | Owns | Status |
| --- | --- | --- | --- |
| `CoreLifecycle` | The conversation, the run loop, saving and crashing: requests pending and settled, items and versions, read marks, sessions | S1 to S4, S10, S12, L1 to L3 | Built, in [`formal/tla`](../../formal/tla/README.md): checked on small bounds on every pull request, with its counterexamples, defects and reachability probes. It found two gaps when a send is given up or refused in a live group (S2, L1, L2), since fixed and checked; see its README |
| `DriverTransport` | One driver's side, for any harness: requests in flight, effects awaiting settlement, a dropped wait, a restart of the harness, two chats | S1, S5, S11, S13, L1, L2 | Built, in [`formal/tla`](../../formal/tla/DriverTransport.md): Codex's, ACP's and Pi's shapes and two chats on one harness, checked on small bounds on every pull request, with a deliberate defect for each bug #118 and #122 fixed in what it models. It found three requests of the harness that were never answered, all fixed: ACP's, while a session is picked, in #122; two in Codex, on 2026-11-23; see its README |
| `SocketProtocol` | The hello, a request answered while events arrive, a connector going and another coming, one at a time; for each kind of connector, what survives a connection | S11, L4 | Built, in [`formal/tla`](../../formal/tla/SocketProtocol.md): checked on small bounds on every pull request. It showed that a connector kept across connections must forget what the old core knew, and that a connector must answer a request once its harness has it, both fixed for Hermes and Claude Code; see its page |
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
- The carrier: an item read back equals the item sent, fits whenever it can, and is one MDK reads.
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

The scenarios against real harnesses stay. Claude Code's and Codex's run in CI on the
[model double](#11-a-model-double); the gateway move tests use it too, and run by hand, since they need Docker;
the rest run by hand. Most tests use recorded
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
[`tests.env.example`](../../tests.env.example) shows it. No model key is in CI ([D-127](../decisions.md#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key)): in CI, a
scenario runs on the [model double](#11-a-model-double), and one that needs a real model runs by hand, with
these names.

### 11. A model double

Status: decided ([D-127](../decisions.md#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key), 2026-11-23): real harnesses run in CI on the double, with no model key in CI;
cassettes are recorded through numes on a non-vendor model; replay reports a newer harness version and does not
fail on it; a cassette older than 14 days is a warning on a pull request and fails the nightly run; the gateways
stay scripted. Claude Code and Codex run on it, and the nightly run fails on age; the matching rule and the
formats' details are internal.

The scenarios against real harnesses need a signed-in harness or a model key, so they never run on a pull request,
and Codex and Claude Code have no free model. The model double runs the **real harness program** against a
small local server of ours that speaks the providers' wire formats, so only the model is fake. OpenAI tests
Codex the same way, with a mock server streaming scripted Responses events.
[`tools/model_double`](../../tools/model_double/README.md) has the server, its tests and how to use it.

```text
 scenario ──► core ──► driver ──► real harness ──HTTP──► model double ──(record only)──► real model
                                  (Claude Code,          scripted | replay | record
                                   Codex, Hermes …)
```

**What the spike showed** (each on 2026-11-23):

| Unknown | Result |
| --- | --- |
| Claude Code, as our driver runs it (Agent SDK 0.3.326, Claude Code 2.1.326) | Works. `ANTHROPIC_BASE_URL` and a dummy `ANTHROPIC_API_KEY`, a cleared environment, a new `HOME` and `CLAUDE_CONFIG_DIR`: no sign-in is read. The whole M2 scenario passes through the core: thinking, text, a `Bash` call needing approval, an interrupt while it waits, a new session, a crash mid-command, `send_file`, a move to another folder. About 17 s. |
| Codex (`codex app-server` 0.171.0, as `marmot-agent-codex` runs it) | Works. A new `CODEX_HOME` whose `config.toml` names a `[model_providers.double]` with `wire_api = "responses"` and an `env_key` holding a dummy: no ChatGPT sign-in. The whole M1 scenario passes through the core, including approvals of `exec_command`. About 9 s. |
| Gateways on Chat Completions | Works. The gateway move tests (`tests/import_gateways.rs`) now run on the double, scripted, in place of the stub model they had; Hermes's and OpenClaw's pass, with the old plugin and ours (about 4 min each, most of it Docker). OpenClaw's first run failed: it puts its runtime context in a user message of its own after the person's words, so the key's `prompt` now takes every user message since the model last spoke, as the stub did. OpenCode, Goose and Pi take the same OpenAI-compatible address. |
| Cursor | Not possible. `--endpoint` points at Cursor's own backend (its first call is `POST /auth/exchange_user_api_key`), and the agent's loop runs on Cursor's servers. Left out. |
| Recording through numes | numes serves Chat Completions only (`/api/v1/chat/completions`; `/responses` and `/messages` are 404). The double translates an Anthropic Messages or Responses request to Chat Completions and writes the reply back in the harness's format. A Claude Code turn was recorded this way on a free model and is replayed on every run. A Codex turn recorded the same way, by hand; it is not committed. |

**Modes.** *Scripted*: the test states the model's replies, by words of the prompt and step. *Replay*: from a
committed cassette, refusing (with a 400, which no harness retries) any request it has no exchange for.
*Record*: forwards to a real model and writes the cassette; it runs in a container that has the key file mounted
read-only and nothing else of it. A cassette holds a header (harness, version, date, format) and, per exchange,
only the request's key and the reply: no header, key, system prompt or request body, so it holds little to scrub.

**The matching rule.** A request's key is `kind` (`main` if it offers tools, `side` if not), `prompt` (the last
user message that is a person's words, with every tagged block such as `<system-reminder>` or
`<environment_context>` removed and spaces collapsed), `after` (how many model replies follow that prompt) and
`last` (`prompt`, or the names of the tools whose results came). Replay matches on all four, and when only
`last` differs it says so: the harness renamed a tool, or answers in a different order. A replay test also fails
when an exchange of the cassette was never asked for, and checks the chat got the recorded reply. The system prompt, the
tool list, ids, model names and headers are left out. Evidence: the cassette recorded with Claude Code 2.1.326
replays against 2.1.330 with every request matched, and against 2.1.300 with both main requests matched, though
the three versions' system prompts and tool lists all differ (2.1.300 offers 20 tools, the others 12). 2.1.300
also sent two side requests (an auto-mode safety classifier, in `-p` mode) the cassette does not hold, which
strict replay reported. That is the rule working: a new kind of request is a change to look at.
Until that changes, the gateways stay scripted ({D127}): they put a timestamp, a session id and the host in the prompt itself (OpenClaw writes `[Mon
2026-11-23 08:21 UTC] Conversation info: …`), so a gateway cassette would never match as it is. Before
recording one, the rule needs to drop those, or match the recorded prompt by the scenario's own words, as
scripted mode does.

**Dependencies.** None. The double is Python's standard library (about 700 lines with the translation), as the
Pi fakes already are; the Rust side starts it with `python3`. A mocking crate would not help: the work is the
three wire formats, and no crate has those. Recording uses the `python` image the repository already pins. The stub model the gateway tests had, about 130 lines of Rust, is gone.

**What CI needs.** For scripted and replay: `python3`, Node 24 with `npm ci` in `packages/claude-code`, and a
pinned `codex` binary, which the core's tests job installs with its checksum (`.github/actions/harnesses`). Codex runs commands in bubblewrap, so the
job lifts Ubuntu 24.04's limit on unprivileged user namespaces. No Docker, key or network to a model. Recording needs Docker and a key, on the owner's
machine.

**Maintenance.** The formats drift. Each harness release can need a new event or field: Claude Code 2.1.326 put
its environment in a `system` message inside `messages`, and retries any 5xx ten times. A drift shows up as a
failing scenario, and the fix is in one file. Scripts drift with the scenarios they serve, in the same file.

**Staleness.** `tools/model_double/check_cassettes.py` reports each cassette older than 14 days, and each harness
whose latest release (npm) is newer than the version a cassette was recorded with. It needs no key. A pull
request runs it offline, in the Recordings job, and each finding is a warning that does not fail it. The nightly
run adds `--strict`, which exits 1 on a finding, and opens an issue with the output: a cassette due for its age,
or for a newer release of its harness. The replay test prints the
Claude Code version when it differs from the cassette's, and does not fail on it: failing would make every pull
request that bumps a harness re-record, which needs a key. A request the cassette has no exchange for still fails.

## In CI

Pull requests keep CI under 15 minutes, by caching builds and running independent checks as parallel jobs
([D-094](../decisions.md#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly)), and CI
runs in three tiers ([D-130](../decisions.md#d-130-ci-runs-in-three-tiers-with-one-required-check)). Rows marked
"planned" are not built yet.

| When | What |
| --- | --- |
| Every pull request, aimed at under 10 minutes | Formatting and Clippy; the workspace's tests in two jobs side by side, the core's and every other, with the conformance suite (about 30 s) and real Claude Code and Codex on the [model double](#11-a-model-double) in the core's; the simulator on a fixed set of seeds at its normal count (20 runs of each gate family from seed 0, in release, in its own job); TLC on small bounds, in three jobs, only when the core or the models changed; the TypeScript, Python, install-script, recording and document checks, and the cassettes' age as a warning. Planned: property tests at normal counts, Kani on the small proofs, the privacy scan |
| The merge queue, and each push to `master`, aimed at under 30 minutes | All of the above whatever changed; the simulator at 200 runs of each family; the move-over tests that need Docker and no key (Hermes and OpenClaw installs moved over, on the model double, scripted) |
| Nightly (`.github/workflows/nightly.yml`) | The simulator on a new seed each night, 5,000 runs of each family, its capsules kept; TLC on larger bounds, safety only (liveness there takes hours and is run by hand); a coverage report (`cargo llvm-cov`, a report only); mutation testing on a 64th of the core's 1,639 mutants (the simulator's hooks left out, since only it exercises them), by the day of the year, so each shard runs about every two months; the cassettes' age and harness releases, failing (`check_cassettes.py --strict`, [the model double](#11-a-model-double)). Planned: property tests at high counts, fuzzing. A failed job opens an issue, with the mutants missed or the cassettes due |
| Weekly (planned) | Long simulator campaigns |

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
3. The real-harness scenarios run in CI on the model double, with no model key in CI at all
   ([D-127](../decisions.md#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key), which replaces
   the nightly runs with a key of
   [D-094](../decisions.md#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly)).
4. Pull-request CI under 15 minutes, with caching and parallel jobs.
5. The order above, fixing what is known first.
