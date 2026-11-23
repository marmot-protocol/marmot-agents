--------------------------- MODULE SocketProtocol ---------------------------
(***************************************************************************)
(* The local socket between the marmot-agents core and one connector, as  *)
(* of master d98d3d4 with the fixes of branch socket-lifetime             *)
(* (2026-11-23). It models crates/marmot-agent/src/socket.rs (the hello,  *)
(* `request` and its patience, the events read while a request waits) and *)
(* run.rs `serve` (one connection at a time, each driving a Core that      *)
(* closes what was open when its driver ends), against the three kinds of *)
(* connector the repository has:                                           *)
(*   "exits"  Claude Code (packages/claude-code): the connector goes with  *)
(*            its connection, and the harness with it; something starts it *)
(*            again.                                                       *)
(*   "fresh"  OpenClaw (packages/openclaw): the gateway goes on, and makes *)
(*            a new connector for each connection; what the old run says   *)
(*            reaches the old connector, which has no socket.             *)
(*   "kept"   Hermes (python/marmot_hermes): the gateway goes on, with one *)
(*            connector across connections, which may hold a quiet turn's  *)
(*            end back for a while.                                        *)
(*                                                                         *)
(* One chat, one prompt p1 whose turn is the item t1, and one control r1, *)
(* an interrupt of that turn, whose effect is the turn's end. A           *)
(* connection goes (Drop) when the core restarts or the socket breaks;    *)
(* the core also gives a connector up (GiveUp) when it has not taken a    *)
(* request in time. CoreLifecycle.tla models the core's own saving and    *)
(* crashing; here a restart of the core is only the connection it ends.   *)
(*                                                                         *)
(* Each step names itself in the variable `step`, which                    *)
(* tools/tla/schedule.py reads to write a counterexample as a schedule.   *)
(*                                                                         *)
(* Properties carry the ids of docs/design/verification.md: S1, S2 (for   *)
(* the items a connection's end closes), S11, L1, L2 and L4. README.md    *)
(* says what is abstracted away.                                           *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Connector,      \* "exits", "fresh" or "kept", as above
    Holds,          \* whether the connector may hold a quiet turn's end back (Hermes)
    MaxDrops,       \* how many times a connection goes: Drop and GiveUp together
    TakenInTime,    \* assumption: a connector that answers a request once its harness has it
                    \* answers within the core's patience (`ANSWER_WITHIN`, 30 s)
    Supervised,     \* assumption: a connector that exits is started again
    FairConnector,  \* the connector eventually connects, reads what the core sent, and lets a
                    \* held end go
    FairHarness,    \* the harness eventually runs what it was handed, and ends its turn
    FairTime,       \* the clock eventually reaches the core's deadline for an effect (D-087)
    Bug             \* a deliberate defect, to show a property catches it; "none" for the code

Bugs == {"none",
         "NoReset",            \* S2: a connector kept across connections forgets nothing of the
                               \* old core's: the Hermes plugin before the fix
         "AnswerOnEffect",     \* S11: a control is answered only once its effect shows: Hermes's
                               \* `/new` and `/model`, Claude Code's interrupt, before the fix
         "AnswerBeforeHandoff",\* S11: a request is answered before the harness has it
         "SettleUnpending"}    \* S1: an event settles a request that is no longer pending

ASSUME Connector \in {"exits", "fresh", "kept"} /\ Bug \in Bugs /\ MaxDrops \in Nat
ASSUME Holds => Connector = "kept"

Reqs == {"p1", "r1"}
NoReq == "none"
Resp(r, ok) == [k |-> "resp", r |-> r, ok |-> ok]
Ev(st, s) == [k |-> "ev", st |-> st, s |-> s]

VARIABLES
    \* The core
    phase,      \* "accept" (no connection), "greet" (a connection, before its hello) or "serve"
    conn,       \* how many connections the core has taken
    drops,      \* how many have gone
    asked,      \* the requests people made: p1 is stored at the start; r1 once t1 shows running
    handed,     \* the requests the core has dealt with: sent to a connector, or refused at once
    inflight,   \* the request the core waits on the connector for, or "none"
    pending,    \* controls a connector took that nothing has settled yet (kept in the state file)
    \* The socket
    up,         \* what the connector wrote and the core has not read, oldest first
    down,       \* the request the core wrote and the connector has not read, or "none"
    \* The connector
    cUp,        \* its process runs
    cOn,        \* it has given its hello on the current connection
    cOpen,      \* items it has shown open (Hermes: its translator's `open`)
    cClosed,    \* items it sends nothing more of (Hermes: `Chat.closed`)
    cWait,      \* the item whose end settles r1 (`by_turn`), or "none"
    cHeld,      \* it holds t1's quiet end back (Hermes: `Chat.held`)
    cOnEffect,  \* it holds r1's answer until the effect shows (Bug "AnswerOnEffect")
    cHanding,   \* it answered p1 and has not handed it to the harness yet (Bug "AnswerBeforeHandoff")
    cDeaf,      \* the harness's run reports to a connector that is gone (OpenClaw)
    \* The harness
    hGot,       \* it has p1
    hStopped,   \* it has r1: the turn is to stop
    hT1,        \* the turn: "none", "open", "done", or "dead" (it went with the connector)
    \* Monitors: what the chat and the harness saw. Nothing takes them back.
    chatT1,     \* the turn as the chat shows it: "none", "open", "done", or "cut" (the core
                \* closed it when its connection went)
    afterCut,   \* a connector changed t1 after the core closed it
    settled,    \* how many times r1 was settled in the chat
    promptSaid, \* what the chat was told of p1: "none", "delivered", "undelivered", "refused"
    ran,        \* the harness ran p1
    unhanded,   \* the core counted p1 taken while the harness did not have it
    needless,   \* the core gave up a connector that was there and answering
    step        \* the step that led here, for the schedules

core == <<phase, conn, drops, asked, handed, inflight, pending>>
wire == <<up, down>>
connector == <<cUp, cOn, cOpen, cClosed, cWait, cHeld, cOnEffect, cHanding, cDeaf>>
harness == <<hGot, hStopped, hT1>>
monitors == <<chatT1, afterCut, settled, promptSaid, ran, unhanded, needless>>
vars == <<core, wire, connector, harness, monitors, step>>

Init ==
    /\ phase = "accept" /\ conn = 0 /\ drops = 0
    /\ asked = {"p1"} /\ handed = {} /\ inflight = NoReq /\ pending = {}
    /\ up = <<>> /\ down = NoReq
    /\ cUp = TRUE /\ cOn = FALSE /\ cOpen = {} /\ cClosed = {} /\ cWait = "none"
    /\ cHeld = FALSE /\ cOnEffect = FALSE /\ cHanding = FALSE /\ cDeaf = FALSE
    /\ hGot = FALSE /\ hStopped = FALSE /\ hT1 = "none"
    /\ chatT1 = "none" /\ afterCut = FALSE /\ settled = 0 /\ promptSaid = "none"
    /\ ran = FALSE /\ unhanded = FALSE /\ needless = FALSE
    /\ step = [a |-> "Init"]

\* What the connector writes reaches the core only on a connection it has greeted, and only
\* while it hears its harness: an event for an item it sends nothing more of is not written.
Writes == cOn /\ phase = "serve"
Emit(m) == up' = IF Writes /\ ~cDeaf /\ "t1" \notin cClosed THEN Append(up, m) ELSE up

-----------------------------------------------------------------------------
(* The connection *)

\* A connector connects; the core takes one connection at a time.
Connect ==
    /\ phase = "accept" /\ cUp /\ ~cOn
    /\ phase' = "greet" /\ conn' = conn + 1
    /\ step' = [a |-> "Connect"]
    /\ UNCHANGED <<drops, asked, handed, inflight, pending, wire, connector, harness, monitors>>

\* The hello and the core's answer: the core sends nothing before it.
Hello ==
    /\ phase = "greet"
    /\ phase' = "serve" /\ cOn' = TRUE /\ up' = <<>> /\ down' = NoReq
    /\ step' = [a |-> "Hello"]
    /\ UNCHANGED <<conn, drops, asked, handed, inflight, pending>>
    /\ UNCHANGED <<cUp, cOpen, cClosed, cWait, cHeld, cOnEffect, cHanding, cDeaf, harness, monitors>>

\* What a connector forgets when its connection goes, by kind. A connector that exits takes its
\* harness with it, and one made afresh no longer hears the run the old one started.
Forget ==
    CASE Connector = "exits" ->
            /\ cUp' = FALSE
            /\ cOpen' = {} /\ cClosed' = {} /\ cWait' = "none" /\ cHeld' = FALSE
            /\ cOnEffect' = FALSE /\ cHanding' = FALSE /\ cDeaf' = FALSE
            /\ hGot' = FALSE /\ hStopped' = FALSE
            /\ hT1' = IF hT1 = "open" THEN "dead" ELSE hT1
      [] Connector = "fresh" ->
            /\ cUp' = TRUE
            /\ cOpen' = {} /\ cClosed' = {} /\ cWait' = "none" /\ cHeld' = FALSE
            /\ cOnEffect' = FALSE /\ cHanding' = FALSE
            /\ cDeaf' = (cDeaf \/ hGot \/ hT1 = "open")
            /\ UNCHANGED harness
      [] Connector = "kept" /\ Bug = "NoReset" ->
            /\ UNCHANGED <<cUp, cOpen, cClosed, cWait, cHeld, cOnEffect, cHanding, cDeaf, harness>>
      [] Connector = "kept" ->
            \* `core_gone`: what the old core holds closed is sent no more, a held end is
            \* dropped, and nothing waits for an effect.
            /\ cClosed' = cClosed \cup cOpen \cup (IF cHeld THEN {"t1"} ELSE {})
            /\ cOpen' = {} /\ cWait' = "none" /\ cHeld' = FALSE /\ cOnEffect' = FALSE
            /\ UNCHANGED <<cUp, cHanding, cDeaf, harness>>

\* The connection ends. The core refuses the request it waited on as failed: a prompt is told
\* as not delivered and never sent again (D-037); it settles every control taken as failed and
\* closes every item open (D-058, D-070); and it waits for the next connector.
End(why) ==
    /\ phase' = "accept" /\ drops' = drops + 1
    /\ inflight' = NoReq /\ pending' = {} /\ up' = <<>> /\ down' = NoReq
    /\ promptSaid' = IF inflight = "p1" THEN "undelivered" ELSE promptSaid
    /\ settled' = settled + Cardinality(pending) + (IF inflight = "r1" THEN 1 ELSE 0)
    /\ chatT1' = IF chatT1 = "open" THEN "cut" ELSE chatT1
    /\ cOn' = FALSE
    /\ Forget
    /\ step' = [a |-> why]
    /\ UNCHANGED <<conn, asked, handed, afterCut, ran, unhanded>>

\* The core restarts, or the socket breaks.
Drop ==
    /\ phase \in {"greet", "serve"} /\ drops < MaxDrops
    /\ End("Drop")
    /\ UNCHANGED needless

\* The core's patience runs out for a request the connector has not answered, and it gives the
\* connector up. A connector that answers once its harness has a request answers in time
\* (TakenInTime); one that holds its answer until the effect shows can take any time.
Answered(r) == \E k \in DOMAIN up : up[k].k = "resp" /\ up[k].r = r
GiveUp ==
    /\ phase = "serve" /\ inflight # NoReq /\ ~Answered(inflight) /\ drops < MaxDrops
    /\ cOnEffect \/ ~TakenInTime
    /\ needless' = (needless \/ (cUp /\ cOn))
    /\ End("GiveUp")

\* A connector that exited is started again.
Restart ==
    /\ ~cUp /\ Supervised
    /\ cUp' = TRUE
    /\ step' = [a |-> "Restart"]
    /\ UNCHANGED <<core, wire, cOn, cOpen, cClosed, cWait, cHeld, cOnEffect, cHanding, cDeaf>>
    /\ UNCHANGED <<harness, monitors>>

-----------------------------------------------------------------------------
(* The core's requests *)

\* A person asks to interrupt the turn the chat shows running.
Ask ==
    /\ "r1" \notin asked /\ chatT1 = "open"
    /\ asked' = asked \cup {"r1"}
    /\ step' = [a |-> "Ask"]
    /\ UNCHANGED <<phase, conn, drops, handed, inflight, pending, wire, connector, harness>>
    /\ UNCHANGED monitors

\* The core hands a request to the connector, one at a time. An interrupt of a turn the chat no
\* longer shows running is refused at once, and reaches no connector.
Hand(r) ==
    /\ phase = "serve" /\ inflight = NoReq /\ r \in asked \ handed
    /\ handed' = handed \cup {r}
    /\ IF r = "r1" /\ chatT1 # "open"
          THEN /\ settled' = settled + 1
               /\ UNCHANGED <<inflight, down>>
          ELSE /\ inflight' = r /\ down' = r
               /\ UNCHANGED settled
    /\ step' = [a |-> "Hand", r |-> r]
    /\ UNCHANGED <<phase, conn, drops, asked, pending, up, connector, harness>>
    /\ UNCHANGED <<chatT1, afterCut, promptSaid, ran, unhanded, needless>>

\* The connector reads the request and answers it once its harness has it. A prompt is handed
\* to the harness (Hermes `inject`, OpenClaw's run started, Claude Code's input queued). An
\* interrupt is handed to the harness when it runs the turn the connector knows, or a turn whose
\* end it holds; otherwise there is nothing to do. The turn's end settles it.
Take ==
    /\ Writes /\ down # NoReq
    /\ down' = NoReq
    /\ IF down = "p1"
          THEN IF Bug = "AnswerBeforeHandoff"
                  THEN /\ cHanding' = TRUE /\ up' = Append(up, Resp("p1", TRUE))
                       /\ UNCHANGED <<hGot, cWait, cOnEffect, hStopped>>
                  ELSE /\ hGot' = TRUE /\ up' = Append(up, Resp("p1", TRUE))
                       /\ UNCHANGED <<cHanding, cWait, cOnEffect, hStopped>>
          ELSE IF "t1" \in cOpen /\ (hT1 = "open" \/ cHeld) /\ ~cDeaf
                  THEN /\ hStopped' = TRUE /\ cWait' = "t1"
                       /\ IF Bug = "AnswerOnEffect"
                             THEN cOnEffect' = TRUE /\ UNCHANGED up
                             ELSE up' = Append(up, Resp("r1", TRUE)) /\ UNCHANGED cOnEffect
                       /\ UNCHANGED <<hGot, cHanding>>
                  ELSE /\ up' = Append(up, Resp("r1", FALSE))
                       /\ UNCHANGED <<hGot, hStopped, cWait, cOnEffect, cHanding>>
    /\ step' = [a |-> "Take", r |-> down]
    /\ UNCHANGED <<core, cUp, cOn, cOpen, cClosed, cHeld, cDeaf, hT1, monitors>>

\* Only with Bug "AnswerBeforeHandoff": the harness gets the prompt the connector answered.
Handoff ==
    /\ cHanding
    /\ cHanding' = FALSE /\ hGot' = TRUE
    /\ step' = [a |-> "Handoff"]
    /\ UNCHANGED <<core, wire, cUp, cOn, cOpen, cClosed, cWait, cHeld, cOnEffect, cDeaf>>
    /\ UNCHANGED <<hStopped, hT1, monitors>>

\* The core reads what the connector wrote, in order, except that while it waits on a request
\* it holds the events it reads, and takes the answer first (socket.rs `request`, `held`).
ReadAt(k) ==
    LET m == up[k] IN
    /\ up' = [j \in 1..(Len(up) - 1) |-> IF j < k THEN up[j] ELSE up[j + 1]]
    /\ IF m.k = "resp"
          THEN /\ inflight' = NoReq
               /\ IF m.r = "p1"
                     THEN /\ promptSaid' = IF m.ok THEN "delivered" ELSE "refused"
                          /\ unhanded' = (unhanded \/ (m.ok /\ ~hGot))
                          /\ UNCHANGED <<pending, settled>>
                     ELSE /\ pending' = IF m.ok THEN pending \cup {"r1"} ELSE pending
                          /\ settled' = IF m.ok THEN settled ELSE settled + 1
                          /\ UNCHANGED <<promptSaid, unhanded>>
               /\ UNCHANGED <<chatT1, afterCut>>
          ELSE \* An event of t1 changes its row, and settles what it names that is pending.
               /\ afterCut' = (afterCut \/ chatT1 = "cut")
               /\ chatT1' = m.st
               /\ settled' = settled + Cardinality(IF Bug = "SettleUnpending"
                                                       THEN m.s ELSE m.s \cap pending)
               /\ pending' = pending \ m.s
               /\ UNCHANGED <<inflight, promptSaid, unhanded>>
Read ==
    /\ phase = "serve"
    /\ \E k \in DOMAIN up :
          /\ IF inflight # NoReq THEN up[k].k = "resp" ELSE k = 1
          /\ ReadAt(k)
          /\ step' = [a |-> "Read", m |-> up[k]]
    /\ UNCHANGED <<phase, conn, drops, asked, handed, down, connector, harness>>
    /\ UNCHANGED <<ran, needless>>

\* The core's net: a control taken whose effect has not shown by its deadline is settled as
\* having none (D-087).
Tick ==
    /\ "r1" \in pending
    /\ pending' = pending \ {"r1"} /\ settled' = settled + 1
    /\ step' = [a |-> "Tick"]
    /\ UNCHANGED <<phase, conn, drops, asked, handed, inflight, wire, connector, harness>>
    /\ UNCHANGED <<chatT1, afterCut, promptSaid, ran, unhanded, needless>>

-----------------------------------------------------------------------------
(* The harness, and what the connector makes of it *)

\* The harness runs the prompt: its turn opens.
Run ==
    /\ hGot /\ hT1 = "none"
    /\ hT1' = "open" /\ ran' = TRUE
    /\ cOpen' = cOpen \cup {"t1"}
    /\ Emit(Ev("open", {}))
    /\ step' = [a |-> "Run"]
    /\ UNCHANGED <<core, down, cUp, cOn, cClosed, cWait, cHeld, cOnEffect, cHanding, cDeaf>>
    /\ UNCHANGED <<hGot, hStopped, chatT1, afterCut, settled, promptSaid, unhanded, needless>>

\* The turn ends: stopped, when the harness has r1, or done. The end settles what waits on it.
\* A quiet end, which settles nothing, may be held back (Hermes, HOLD_SECONDS). A connector
\* that held r1's answer until its effect answers it now, after the end.
EndTurn(hold) ==
    /\ hT1 = "open"
    /\ hold => (Holds /\ cWait # "t1")
    /\ hT1' = "done"
    /\ LET s == IF cWait = "t1" THEN {"r1"} ELSE {}
           answer == IF cOnEffect /\ Writes THEN <<Resp("r1", TRUE)>> ELSE <<>>
           shown == IF Writes /\ ~cDeaf /\ "t1" \notin cClosed /\ ~hold
                       THEN <<Ev("done", s)>> ELSE <<>>
       IN up' = up \o shown \o answer
    /\ cWait' = "none" /\ cOnEffect' = FALSE
    /\ cHeld' = hold
    /\ cOpen' = IF hold THEN cOpen ELSE cOpen \ {"t1"}
    /\ step' = [a |-> "EndTurn", hold |-> hold]
    /\ UNCHANGED <<core, down, cUp, cOn, cClosed, cHanding, cDeaf, hGot, hStopped, monitors>>

\* A held end goes, once its time is up. It settles nothing: an interrupt taken while it was
\* held found the turn already over (`send_held`).
Release ==
    /\ cHeld
    /\ cHeld' = FALSE /\ cOpen' = cOpen \ {"t1"} /\ cWait' = "none"
    /\ Emit(Ev("done", {}))
    /\ step' = [a |-> "Release"]
    /\ UNCHANGED <<core, down, cUp, cOn, cClosed, cOnEffect, cHanding, cDeaf, harness, monitors>>

-----------------------------------------------------------------------------

Next ==
    \/ Connect \/ Hello \/ Drop \/ GiveUp \/ Restart
    \/ Ask \/ \E r \in Reqs : Hand(r)
    \/ Take \/ Handoff \/ Read \/ Tick
    \/ Run \/ \E hold \in BOOLEAN : EndTurn(hold) \/ Release

(***************************************************************************)
(* Fairness, one condition for each party over all its progress steps, as *)
(* in CoreLifecycle. Drops and give-ups are bounded, so they end. People   *)
(* need not ask anything.                                                  *)
(***************************************************************************)
CoreSteps == Hello \/ Read \/ \E r \in Reqs : Hand(r)
ConnectorSteps == Connect \/ Take \/ Handoff \/ Release \/ Restart
HarnessSteps == Run \/ \E hold \in BOOLEAN : EndTurn(hold)

Fairness ==
    /\ SF_vars(CoreSteps)
    /\ FairConnector => SF_vars(ConnectorSteps)
    /\ FairHarness => SF_vars(HarnessSteps)
    /\ FairTime => SF_vars(Tick)

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Safety *)

TypeOK ==
    /\ phase \in {"accept", "greet", "serve"}
    /\ inflight \in Reqs \cup {NoReq} /\ down \in Reqs \cup {NoReq}
    /\ chatT1 \in {"none", "open", "done", "cut"}
    /\ hT1 \in {"none", "open", "done", "dead"}
    /\ drops <= MaxDrops /\ conn <= MaxDrops + 1

\* S1: every answer and control is settled at most once in the chat (D-028, D-070, D-087).
S1 == settled <= 1

\* S2, for the socket: an item the core closed when its connection went is never changed
\* again: no connector, kept or new, sends a later version of it (D-058, D-070).
S2 == ~afterCut

\* S11, for the socket: a request the core counts as taken is one the harness has, and the
\* core gives up only a connector that has gone or has not taken a request in time, never one
\* that has taken it and waits for its effect (D-058, D-066; `ANSWER_WITHIN`).
S11 == ~unhanded /\ ~needless

\* The accepted limit of D-037: a prompt whose answer the drop took is told as not delivered,
\* and is never sent again; the harness may have it, and run it. Not a property the code
\* keeps: check.py shows that it fails (counterexample `prompt-in-flight`).
UndeliveredNotRun == ~(promptSaid = "undelivered" /\ ran)

-----------------------------------------------------------------------------
(* Liveness, under the fairness above *)

\* L1: every control a person makes is eventually settled in the chat, and every prompt is
\* eventually handed over or told as not delivered.
L1 == /\ ("r1" \in asked ~> settled >= 1)
      /\ <>(promptSaid # "none")

\* L2: every item the chat shows open eventually closes.
L2 == chatT1 = "open" ~> chatT1 # "open"

\* L4: a connector that goes away is eventually replaced, and the core goes on.
L4 == phase # "serve" ~> phase = "serve"

-----------------------------------------------------------------------------
(* Reachability. Each says an interesting state never happens; TLC must   *)
(* find each one false.                                                    *)

\* The core closes the turn the chat showed open, as its connection goes.
NeverCut == chatT1 # "cut"
\* A second connector, or the same one again, serves the core.
NeverReconnected == ~(conn >= 2 /\ phase = "serve")
\* The interrupt is settled by the turn's end, the effect the connector named.
NeverSettledByEffect == ~(settled = 1 /\ chatT1 = "done" /\ hStopped)
\* An event waits behind the answer to a request: the core reads the answer first.
NeverEventBeforeAnswer ==
    ~(inflight # NoReq /\ \E j, k \in DOMAIN up : j < k /\ up[j].k = "ev" /\ up[k].k = "resp")
\* A connector holds a turn's end across a connection that goes (Hermes).
NeverHeldAcross == ~(cHeld /\ drops >= 1 /\ "t1" \in cClosed)
\* The turn runs on in a gateway after its connection went, and its end is sent no more.
NeverRunOnAfterDrop == ~(drops >= 1 /\ hT1 = "done" /\ chatT1 = "cut")
=============================================================================
