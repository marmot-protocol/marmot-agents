# The state machines, and which a model would pay for

Status: an inventory, internal, kept as the code changes. Read on 2026-11-24 against master `7118005`, by
reading the code named in each row; the drivers' and connectors' rows were read more briefly than the core's and
the installer's, and say so. What a model would find is a judgement, marked as such. The models it led to are
[`InstallUpgrade`](InstallUpgrade.md) and [`ActivityHoldBack`](ActivityHoldBack.md); the others are
[`CoreLifecycle`](README.md), [`SocketProtocol`](SocketProtocol.md) and [`DriverTransport`](DriverTransport.md).
[The verification plan](../../docs/design/verification.md#4-formal-models) says what each model owns.

Each row names a machine that is in the code, said or not: its states and steps, what it faces (concurrency, a
crash, messages out of order), what checks it today, and what a model would pay. "Pays" weighs the bugs a model
could find against what it costs to write and to keep in step with the code: a machine with few states that a
test already walks step by step pays little; one whose steps interleave with another party's, or that a crash can
cut anywhere, pays most.

## Ranked: what to model next

1. **The installer's upgrade** (D-134, D-116): modelled, [`InstallUpgrade`](InstallUpgrade.md). It found two
   windows in which a TERM or a kill left Pi's image tag, or the person's Hermes plugin, apart from the files they
   go with; both fixed. Review found a third, which U1 as first stated let through: another harness's install
   put back the core and left the Hermes plugin; fixed, and U1 restated so the model catches it.
2. **The core's hold-back of the full activity** (D-136): modelled, [`ActivityHoldBack`](ActivityHoldBack.md). It
   found that a send MDK was already handed, held back when someone joins the chat, settles its request twice,
   and can start its item twice; fixed. In review, a send tried again without the native message it was first
   handed with, which MDK refuses as another submission, settled its request twice too; fixed, and modelled
   since. It also shows the privacy limit that MDK posts what it keeps, to whoever
   is in the chat then: a limit accepted for the pilot (D-148).
3. **The agent's words and the settles they carry** (`conversation.rs` `emitted` for `Body::Message`,
   `settle_by_turn`, `shown_by`, `refused` for words): a second model of the core, which `CoreLifecycle` leaves
   out on purpose. The requests words name move between the words' next new message, the turn's next version and
   the stop's net, and a refused edit goes as a new message. The simulator's oracle checks S1 over words (read in
   `marmot-agent-sim/src/oracle.rs`), but only in the owner's chat and without a version of a turn that is held
   back. Pays well: several hands-off of one request, across MDK's refusals (judgement).
4. **Answers** (D-087: settled when their item stops waiting, or `no_effect` on a turn's end): not in any model;
   checked by the conversation's tests and the simulator. Smaller than the words; would fold into the words model.
5. **Hermes's held turn end and `/model` and `/new` settlement** (`python/marmot_hermes/connector.py` `reserve`,
   `handed`, `generation`), and **OpenClaw's approval vouching**: the plan's `Gateways` model. `SocketProtocol`
   checks the connection's side of both (`AnswerOnEffect`, `NoReset`); what is left is in the gateway. Pays some:
   asyncio tasks and the gateway's own callbacks interleave (judgement, from a brief reading).
6. **Codex's subagents** (D-117, D-132): being built on another branch; not modelled until it is on master. Then
   `DriverTransport` would need a subagent's thread whose requests are held while it starts, which it now stands
   in for with a pick.

The rest pay little now: their states are few, a test walks each step, or a model already holds them.

## The core (`crates/marmot-agent`)

| Machine | States and steps | Faces | Checked by | A model pays |
| --- | --- | --- | --- | --- |
| A request (`conversation.rs` `pending`, `settle_pending`, `overdue`) | heard → refused at once, or handed to the driver → taken or refused → settled by an event, a tick (`no_effect`), the stop's net (`failed`), or a dropped send's stand-in | a crash between any two; the driver ending; MDK refusing what settles it | `CoreLifecycle` (controls: S1, L1); the conversation's tests; the simulator | Covered for controls. Answers: rank 4 |
| An item's versions (`Record`: `version`, `first`, `closed_in_chat`, `forgotten`) | queued versions → sent (first event known) → edits; closed by the driver, the stop, a restart, or a stand-in; forgotten after `KEPT` finished items | crash, refusal, give-up, MDK forgetting a token (`lost`), a first event that disappeared (`first_gone`) | `CoreLifecycle` (S2, S3, L2); the simulator | Covered, but for the cache of finished items and disappearing messages, which `CoreLifecycle` leaves out (no clock). Little |
| The agent's words (`held`, `words`, `settle_by_turn`, `shown_by`) | words new → edits; the settles they name held for the next new message, or moved to the turn; a refused edit sent as a new message | MDK refusing either; a turn closed meanwhile | the conversation's tests; the simulator | Rank 3 |
| The outbox and its tokens (`flush`, `attempting`, `rejected`, `give_up`, `refused`) | queued → attempted (saved first) → sent, pending, busy, unknown, rejected (new token), refused or given up (stand-in, last resort) | crash at any point; MDK's answers lost; MDK dropping a try | `CoreLifecycle` (S1, S3, S3Told); `crashes.rs`; the simulator | Covered |
| The hold-back of the full activity (`members_now`, `hold_back`, `settle_for_hidden`, `full_activity_only`) | per group: the owner's chat or not, read before each send; queued sends of the full activity taken out, their settles moved to the turn or the session or a `not_shown` notice | people joining and leaving at any time; MDK holding a send already handed; a restart | the conversation's tests (no send in flight); not the simulator (every chat is the owner's) | Rank 2, modelled |
| Read marks and readers (`read`, `catch_up`, `catch_up_group`, `history`, `unread`) | a group's mark → each message after it handled once, oldest first, the mark saved with what it led to | live messages in any order or before they are stored (MDK #3606); a lagging stream; crash | `CoreLifecycle` (S10, L3); `new_groups.rs`; the simulator | Covered |
| Invitations (`consider_invitations`) | invited → mark 0 saved → accepted or declined → read from its start | a crash between saving the mark and accepting; a decline that fails | `marmot.rs` (against MDK); the simulator (its `Invite` step, with crashes) | Little: two steps, and a crash on either side is safe by the order (judgement) |
| Sessions (`sessions`, `taken`, `ended_sessions`) | a chat's saved session → named by the next prompt after a restart → taken, ended, or new by control | crash; a driver taking a new-session control | `CoreLifecycle` (S12) | Covered |
| Files the agent sends (`send_file`, `upload`, `file_done`) | path read from an allowed folder → uploaded in a task, retried while MDK is busy → sent or refused (notice) → the driver told | a crash loses the upload and the driver with it; a slow upload | `files.rs` (the path rules), `run.rs` tests | Little: no state survives a crash, and the path rules are not a machine |
| The run loop (`run`, `driver_ready`, `serve`, `ended`, `recover`) | starting → catch-up → live; driver ends → closes and settles → waits or restarts; stop | a driver ending mid catch-up (D-139); a connector going | `CoreLifecycle`, `SocketProtocol` | Covered |
| A socket connection (`socket.rs`: hello, `request`, patience, give-up, D-138) | connecting → hello → requests in flight, events between → dropped or given up | a connector that stops reading; reconnects | `SocketProtocol` (S11, L4); the conformance suite's second phase is designed, not built | Covered |
| The import (`import/`: plan, stop services, lease, backup, settings) | read → planned → services stopped → lease → backup checked → settings published, never over anything | a kill at any step; a doubtful service (fails closed, D-135) | `import/` tests, `import_gateways` with Docker | Little: each step either leaves nothing under a final name or is checked; D-135's rules are parsing, not states |

## The built-in drivers (read briefly)

| Machine | States and steps | Faces | Checked by | A model pays |
| --- | --- | --- | --- | --- |
| Codex: turns, interrupts, held approvals (`crates/marmot-agent-codex`) | thread picked → turn started → requests waiting, resolved by Codex, held while a session is picked → turn ends | one app-server for every chat; Codex resolving by itself; restart | `DriverTransport` (Codex's shape); the conformance suite | Covered, but settings and compaction (`Action::Set`, `Own`) and patience (no clock) |
| Codex: subagents (D-117, D-132) | a subagent's thread started → its requests held until known → shown flat in the starting chat | threads appearing before their start is known | the conformance suite (`subagents.rs`) | Rank 6, once on master |
| ACP: sessions and permission requests (`crates/marmot-agent-acp`) | session picked or loaded → prompt → permission requests and questions → cancel answers them → turn ends | a request in a session no chat has (#122); cancelling | `DriverTransport` (ACP's shape, stray, two chats) | Covered |
| Pi: sessions, compaction, resume (`crates/marmot-agent-pi`) | a Pi per chat → started on its session (`ready_for`) → dialogs → dies and starts again | a Pi dying mid-turn; lines from an earlier Pi | `DriverTransport` (Pi's shape); the conformance suite | Covered, but compaction (`Own`), whose effect is a notice: little |

## The connectors (read briefly)

| Machine | States and steps | Faces | Checked by | A model pays |
| --- | --- | --- | --- | --- |
| Claude Code (`packages/claude-code/src/connector.ts`) | connected → prompt → `canUseTool` waits for an answer → interrupt → result | the connection going with the connector (it exits) | `SocketProtocol` ("exits"); its tests against a fake SDK | Covered |
| OpenClaw (`packages/openclaw/src/connector.ts`, `index.ts`) | a connector per connection → runs → approvals forwarded or vouched → cancelled at the connection's end | the gateway outliving connections | `SocketProtocol` ("fresh"); its tests | The approval vouching: rank 5 |
| Hermes (`python/marmot_hermes/connector.py`, `adapter.py`) | one connector across connections; a quiet turn's end held back; `/model`, `/new` reserved until they settle (`reserve`, `handed`); approval cards (`cards`, `resolve_card`) | asyncio tasks; the gateway's own callbacks; connections going | `SocketProtocol` ("kept"); its tests against a fake core and Hermes | Rank 5 |

## The installers (`install/`)

| Machine | States and steps | Faces | Checked by | A model pays |
| --- | --- | --- | --- | --- |
| The upgrade, with its lock, staging, record and putting back (D-134) | lock → take over, or put back another's, or record → stage → core → each part kept and put → plugin change → start → image tag → end; on a stop, put back | SIGKILL anywhere, TERM anywhere, another harness's install next, two installs at once | `install/test_install.py` `Upgrade` (Claude Code at every step); now `InstallUpgrade` | Rank 1, modelled |
| A gateway's plugin change (D-116) | copies kept → record → plugin replaced → started → record removed; on a stop, put back | the same; macOS's two renames | `Interrupted` (a move at every step); now `InstallUpgrade`, with the upgrade it ends with | Modelled with the upgrade |
| Pi's and Goose's image (`image_commit`, `image_put_back`) | built as `:staging` → tagged once started → staging tag gone; put back on a stop | the same | `test_a_failed_upgrade_keeps_the_image_its_scripts_were_made_for`; now `InstallUpgrade` | Modelled with the upgrade |
