--------------------------- MODULE DriverTransport ---------------------------
(***************************************************************************)
(* One driver's side, for any harness: the core's answers and controls in  *)
(* flight, the effects that settle them, the harness's own requests        *)
(* (approvals and questions) and who answers them, a dropped wait, a       *)
(* harness that dies and starts again, and two chats on one harness.       *)
(*                                                                         *)
(* It follows the built-in drivers as of master c5c41be (#122):           *)
(* crates/marmot-agent-codex, crates/marmot-agent-acp and                  *)
(* crates/marmot-agent-pi, behind the trait in crates/marmot-agent-driver. *)
(* A configuration picks the harness's shape: one process for every chat   *)
(* (Codex, ACP), or one for each chat (Pi); whether the harness settles a  *)
(* request of its own by itself (Codex's `serverRequest/resolved`); how    *)
(* the driver names an approval's item (after the request, or after its    *)
(* session and tool call, as ACP's is); and so on. README.md maps each     *)
(* action to the code, and says what is left out.                         *)
(*                                                                         *)
(* Properties carry the ids of docs/design/verification.md: S1, S5, S11,   *)
(* S13, L1 and L2, as this model reads them (README.md).                   *)
(***************************************************************************)
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS
    Chats,          \* the chats, from {"c1", "c2"}
    Asks,           \* the harness's own requests (an approval or a question), from {"a1", "a2"}
    Calls,          \* the tool calls an approval can be for, from {"k1", "k2"}
    Answers,        \* the core's answers, from {"r1", "r5"}
    Interrupts,     \* the core's interrupts, from {"r2"}
    NewSessions,    \* the core's new-session controls, from {"r3", "r4"}
    MaxSessions,    \* how many sessions the harness opens, at most
    MaxPrompts,     \* how many prompts the driver hands the harness, in all
    MaxDeaths,      \* how many times a harness process dies
    MaxEdits,       \* how many times the harness shows a session item again unasked
    PerChat,        \* one harness process for each chat (Pi), or one for all (Codex, ACP)
    SelfResolves,   \* the harness stops waiting for a request by itself, and says so (Codex's
                    \* `serverRequest/resolved`), and, after a turn's end, for each it dropped
    Unshowable,     \* the harness can ask what the driver cannot show (a form at a web address,
                    \* params it cannot read)
    StrayAsks,      \* the harness can ask in a session no chat has (session 0)
    CallNamed,      \* an approval's item is named after its session and tool call (ACP), not
                    \* after the request (Codex, Pi)
    CancelOnInterrupt, \* the driver answers each waiting request as cancelled before it
                    \* interrupts, and what is asked while the turn is cancelled (ACP)
    ReaskAfterRestart, \* the harness may ask again about a tool call it asked about before it
                    \* died, in the session it takes up again
    FairDriver,     \* the driver eventually reads what the harness wrote, and starts again
    FairHarness,    \* the harness eventually reads what the driver wrote, and ends a turn it
                    \* was told to stop
    Bug             \* a deliberate defect, to show a property catches it; "none" for the code

Bugs == {"none",
         "AnswerAfterTurnEnd", \* S11: an answer is taken for an item closed at its turn's end,
                               \* as Pi's driver did before #118
         "ResolvedIgnored",    \* S11: the harness's word that it stopped waiting is ignored, as
                               \* Codex's driver did with `serverRequest/resolved` before #118
         "SettleOnAnySession", \* S1: a new session is settled by any session item other than the
                               \* one the core named, even an edit of the current one, as ACP's
                               \* and Pi's drivers did before #118
         "TwoNewSessions",     \* S1: a second new-session control is taken while one waits, as
                               \* Pi's driver did before #118
         "OneItemPerCall",     \* S13: two approvals for one tool call share an item, as ACP's
                               \* driver did before #118
         "UnshownUnanswered",  \* L2: a request the driver cannot show is never answered, as
                               \* ACP's driver did before #118, and Codex's until 2026-11-23
         "HeldDropped",        \* L2: a request from a session no chat has, held while a session
                               \* is picked, is dropped unanswered once none is, as ACP's driver
                               \* did before #122
         "StrayUnanswered",    \* L2: a request from a session no chat has is never answered, at
                               \* once or once held, as Codex's driver did until 2026-11-23
         "CallOnly",           \* S5: an approval's item is named after its tool call alone, so
                               \* two chats' approvals can share a name
         "TakeUnwritten",      \* S11: an answer is taken, and shown as given, when the harness
                               \* can no longer read it
         "LoseOnDrop"}         \* L1: a line read from the harness is lost when the core drops
                               \* the wait it was read in
ASSUME Bug \in Bugs

Reqs == Answers \cup Interrupts \cup NewSessions
Sess == 0..MaxSessions        \* 0: a session no chat has
Procs == IF PerChat THEN Chats ELSE {"h"}
P(c) == IF PerChat THEN c ELSE "h"
\* A fixed order for the steps that go through a set of requests one by one.
AskOrder(S) == SelectSeq(<<"a1", "a2", "a3">>, LAMBDA a : a \in S)
AppendAll(seq, S, Make(_)) ==
    LET order == AskOrder(S)
    IN seq \o [i \in 1..Len(order) |-> Make(order[i])]

NoName == [s |-> 0, k |-> "none", tag |-> "none"]
NoAsk == [st |-> "none", asked |-> FALSE, item |-> NoName, c |-> "none", s |-> 0, n |-> 0]

VARIABLES
    \* The harness.
    hTurn,      \* each session's turn: its number and whether it runs, or was told to stop
    hProc,      \* the process each session lives in
    hCreator,   \* what opened each session: a new-session control, or a prompt's pick
    nextSess,   \* the next session the harness opens
    hAsk,       \* each request of the harness: open, answered, resolved by itself, or dropped
    heard,      \* how many replies the harness read for each of its requests
    alive,      \* whether each harness process runs
    gen,        \* each process's generation: a process started again is a new one
    deaths, edits, prompts,
    \* The pipes, in order: what the driver wrote and the harness has not read, and back.
    toH, fromH,
    \* The driver.
    up,         \* the driver runs (a driver whose one harness died has ended)
    dSess,      \* each chat's session, as the driver knows it; 0 for none
    dTurn,      \* each chat's running turn: its number, -1 while a prompt is starting one, 0
    picking,    \* the request a session is being picked for, in each chat, or "none"
    dAsk,       \* each harness request as the driver holds it: its item, whether the item waits,
                \* and whether the driver still holds the request to answer (`asked`)
    names,      \* the item names each chat's translator has used (lost when the driver ends)
    effects,    \* controls taken and waiting for their effect
    early,      \* requests from a session no chat has, held while a session is picked
    cancelling, \* each chat's turn is being cancelled (ACP)
    \* The core, and what is recorded to check the properties.
    rq,         \* each request of the core: unsent, refused or taken
    nSettle,    \* how many times each request was settled
    how,        \* how it was settled: refused, effect, failed (the driver ended) or wrong
    cSess,      \* the session the core saved for each chat
    hadSess,    \* every session each chat has shown
    seenItems,  \* every item each chat has shown
    seenTurns,  \* every turn each chat has shown
    issued,     \* <<chat, item name, request>> for every item made for a harness request
    stopRead,   \* the driver has read that the harness stopped waiting for the request
    late,       \* an answer was written after the driver read that the harness stopped waiting
    misrouted,  \* an answer reached a request of another chat
    unwrittenTake, \* an answer was taken when its harness could not read it
    droppedEarly   \* requests held while a session was picked and then dropped

harnessVars == <<hTurn, hProc, hCreator, nextSess, hAsk, heard, alive, gen, deaths, edits, prompts>>
driverVars == <<up, dSess, dTurn, picking, dAsk, names, effects, early, cancelling>>
coreVars == <<rq, nSettle, how, cSess, hadSess, seenItems, seenTurns>>
histVars == <<issued, stopRead, late, misrouted, unwrittenTake, droppedEarly>>
vars == <<harnessVars, toH, fromH, driverVars, coreVars, histVars>>

-----------------------------------------------------------------------------
(* Helpers *)

\* The chat a message from process p about session s is for: Pi's records name no session and
\* go to the process's chat; Codex and ACP name a session, and go to the chat that has it.
ChatOf(p, s) ==
    IF PerChat THEN p
    ELSE IF s # 0 /\ \E c \in Chats : dSess[c] = s THEN CHOOSE c \in Chats : dSess[c] = s
    ELSE "none"

ProcOfAsk(a) == IF hAsk[a].s = 0 THEN "h" ELSE hProc[hAsk[a].s]

\* The name the translator gives the item of request a, in chat c.
NameOf(a, c) ==
    LET s == hAsk[a].s
        first == [s |-> IF Bug = "CallOnly" THEN 0 ELSE s, k |-> hAsk[a].k, tag |-> "first"]
    IN IF ~CallNamed THEN [s |-> s, k |-> "own", tag |-> a]
       ELSE IF first \notin names[c] \/ Bug = "OneItemPerCall" THEN first
       ELSE [first EXCEPT !.tag = a]

\* Settling the requests in S, each the way Way(r) says. A wrong settle stays wrong.
Counted(S) == [r \in Reqs |-> IF r \in S THEN nSettle[r] + 1 ELSE nSettle[r]]
Settled(S, Way(_)) ==
    [r \in Reqs |-> IF r \notin S THEN how[r]
                    ELSE IF how[r] = "wrong" THEN "wrong" ELSE Way(r)]

Reply(a, kind) == [m |-> "reply", a |-> a, how |-> kind]

-----------------------------------------------------------------------------
Init ==
    /\ hTurn = [s \in Sess |-> [n |-> 0,
                                st |-> IF s = 0 /\ StrayAsks THEN "running" ELSE "idle"]]
    /\ hProc = [s \in Sess |-> IF s = 0 /\ ~PerChat THEN "h" ELSE "none"]
    /\ hCreator = [s \in Sess |-> "none"]
    /\ nextSess = 1
    /\ hAsk = [a \in Asks |-> [st |-> "none", s |-> 0, n |-> 0, k |-> "none", shown |-> TRUE,
                               gen |-> 0]]
    /\ heard = [a \in Asks |-> 0]
    /\ alive = [p \in Procs |-> TRUE]
    /\ gen = [p \in Procs |-> 0]
    /\ deaths = 0 /\ edits = 0 /\ prompts = 0
    /\ toH = [p \in Procs |-> <<>>]
    /\ fromH = [p \in Procs |-> <<>>]
    /\ up = TRUE
    /\ dSess = [c \in Chats |-> 0]
    /\ dTurn = [c \in Chats |-> 0]
    /\ picking = [c \in Chats |-> "none"]
    /\ dAsk = [a \in Asks |-> NoAsk]
    /\ names = [c \in Chats |-> {}]
    /\ effects = {}
    /\ early = {}
    /\ cancelling = [c \in Chats |-> FALSE]
    /\ rq = [r \in Reqs |-> "unsent"]
    /\ nSettle = [r \in Reqs |-> 0]
    /\ how = [r \in Reqs |-> "none"]
    /\ cSess = [c \in Chats |-> 0]
    /\ hadSess = [c \in Chats |-> {}]
    /\ seenItems = [c \in Chats |-> {}]
    /\ seenTurns = [c \in Chats |-> {}]
    /\ issued = {}
    /\ stopRead = [a \in Asks |-> FALSE]
    /\ late = FALSE
    /\ misrouted = FALSE
    /\ unwrittenTake = FALSE
    /\ droppedEarly = {}

-----------------------------------------------------------------------------
(* The core's requests: `Driver::answer` and `Driver::control`. Each returns *)
(* once the driver has taken it, its message written whole to the          *)
(* harness's input, or refused it (D-066).                                 *)

Refuse(r) ==
    /\ rq' = [rq EXCEPT ![r] = "refused"]
    /\ nSettle' = Counted({r})
    /\ how' = Settled({r}, LAMBDA x : "refused")

\* `answer`: `chat_with_item` finds the chat whose translator holds the item, and the
\* translator's `reply` builds the reply to the request it waits on. The reply, fed back
\* (`Direction::Out`), makes the item's edit that shows the answer: it settles the answer. The
\* person answers an item their chat showed, which may no longer wait.
Answer(r, c, name) ==
    /\ up /\ rq[r] = "unsent" /\ r \in Answers
    /\ name \in seenItems[c]
    /\ LET held == {a \in Asks : dAsk[a].item = name /\ dAsk[a].asked
                                 /\ (dAsk[a].st = "waiting" \/ Bug = "AnswerAfterTurnEnd")}
           canWrite(a) == alive[P(dAsk[a].c)] \/ Bug = "TakeUnwritten"
       IN IF \E a \in held : canWrite(a)
          THEN \E a \in held :
                 /\ canWrite(a)
                 /\ LET p == P(dAsk[a].c)
                        shows == dAsk[a].st = "waiting"
                    IN
                    /\ toH' = [toH EXCEPT ![p] = Append(@, Reply(a, "answer"))]
                    /\ dAsk' = [dAsk EXCEPT ![a].asked = FALSE,
                                            ![a].st = IF shows THEN "answered" ELSE @]
                    /\ rq' = [rq EXCEPT ![r] = "taken"]
                    /\ nSettle' = IF shows THEN Counted({r}) ELSE nSettle
                    /\ how' = IF shows THEN Settled({r}, LAMBDA x : "effect") ELSE how
                    /\ late' = (late \/ stopRead[a])
                    /\ misrouted' = (misrouted \/ dAsk[a].c # c)
                    /\ unwrittenTake' = (unwrittenTake \/ ~alive[p])
          ELSE /\ Refuse(r)
               /\ UNCHANGED <<toH, dAsk, late, misrouted, unwrittenTake>>
    /\ UNCHANGED <<harnessVars, fromH, up, dSess, dTurn, picking, names, effects, early,
                   cancelling, cSess, hadSess, seenItems, seenTurns, issued, stopRead,
                   droppedEarly>>

\* `control` with `Action::Interrupt`: taken when the turn named is the chat's running turn;
\* its effect, `Effect::TurnEnded`, is watched for from the moment it is written. ACP's `cancel`
\* first answers every request still waiting as cancelled (`cancel_replies`).
Interrupt(r, c, t) ==
    /\ up /\ rq[r] = "unsent" /\ r \in Interrupts
    /\ t \in seenTurns[c]
    /\ IF alive[P(c)] /\ dSess[c] = t[1] /\ dTurn[c] = t[2]
       THEN LET waiting == IF CancelOnInterrupt
                           THEN {a \in Asks : dAsk[a].c = c /\ dAsk[a].asked} ELSE {}
                cancelled == AppendAll(toH[P(c)], waiting, LAMBDA a : Reply(a, "cancel"))
            IN
            /\ toH' = [toH EXCEPT ![P(c)] =
                         Append(cancelled, [m |-> "interrupt", s |-> t[1], n |-> t[2]])]
            /\ dAsk' = [a \in Asks |-> IF a \in waiting
                                       THEN [dAsk[a] EXCEPT !.asked = FALSE, !.st = "closed"]
                                       ELSE dAsk[a]]
            /\ cancelling' = [cancelling EXCEPT ![c] = CancelOnInterrupt]
            /\ effects' = effects \cup {[r |-> r, kind |-> "turn", c |-> c, s |-> t[1],
                                         n |-> t[2], old |-> 0]}
            /\ rq' = [rq EXCEPT ![r] = "taken"]
            /\ UNCHANGED <<nSettle, how>>
       ELSE /\ Refuse(r)
            /\ UNCHANGED <<toH, dAsk, cancelling, effects>>
    /\ UNCHANGED <<harnessVars, fromH, up, dSess, dTurn, picking, names, early, cSess,
                   hadSess, seenItems, seenTurns, histVars>>

\* `control` with `Action::NewSession`: taken while the chat is idle and no session is being
\* picked for it (Codex's `ask` and ACP's `picked` wait for the pick; Pi's refuses as busy). Only
\* a session other than the one the chat is in shows its effect: the driver's own first, then
\* the one the core named, which is the one it saved, or none (#118).
NewSession(r, c, named) ==
    /\ up /\ rq[r] = "unsent" /\ r \in NewSessions
    /\ named \in {0} \cup hadSess[c]
    /\ IF /\ alive[P(c)] /\ dTurn[c] = 0
          /\ (picking[c] = "none" \/ Bug = "TwoNewSessions")
       THEN LET old == IF Bug = "SettleOnAnySession" THEN named
                       ELSE IF dSess[c] # 0 THEN dSess[c] ELSE named
            IN
            /\ toH' = [toH EXCEPT ![P(c)] =
                         Append(@, [m |-> "open", c |-> c, resume |-> 0, r |-> r])]
            /\ picking' = [picking EXCEPT ![c] = r]
            /\ effects' = effects \cup {[r |-> r, kind |-> "new", c |-> c, s |-> 0, n |-> 0,
                                         old |-> old]}
            /\ rq' = [rq EXCEPT ![r] = "taken"]
            /\ UNCHANGED <<nSettle, how>>
       ELSE /\ Refuse(r)
            /\ UNCHANGED <<toH, picking, effects>>
    /\ UNCHANGED <<harnessVars, fromH, up, dSess, dTurn, dAsk, names, early, cancelling,
                   cSess, hadSess, seenItems, seenTurns, histVars>>

\* A prompt for a chat with no session first picks one (Codex's and ACP's `pick`, Pi's
\* `ready_for`): a new one, or the one the core saved, after the driver started again.
Pick(c) ==
    /\ up /\ alive[P(c)] /\ dSess[c] = 0 /\ picking[c] = "none" /\ prompts < MaxPrompts
    /\ toH' = [toH EXCEPT ![P(c)] =
                 Append(@, [m |-> "open", c |-> c, resume |-> cSess[c], r |-> "prompt"])]
    /\ picking' = [picking EXCEPT ![c] = "prompt"]
    /\ UNCHANGED <<harnessVars, fromH, up, dSess, dTurn, dAsk, names, effects, early,
                   cancelling, coreVars, histVars>>

\* `prompt`: the driver knows a turn is coming from the moment it writes the prompt (ACP's
\* `session/prompt`, Codex's `turn/start` until Codex answers it). Prompts are CoreLifecycle's;
\* here they only start turns.
Prompt(c) ==
    /\ up /\ alive[P(c)] /\ dSess[c] # 0 /\ picking[c] = "none" /\ dTurn[c] = 0
    /\ prompts < MaxPrompts
    /\ toH' = [toH EXCEPT ![P(c)] = Append(@, [m |-> "prompt", s |-> dSess[c]])]
    /\ dTurn' = [dTurn EXCEPT ![c] = -1]
    /\ prompts' = prompts + 1
    /\ UNCHANGED <<hTurn, hProc, hCreator, nextSess, hAsk, heard, alive, gen, deaths, edits,
                   fromH, up, dSess, picking, dAsk, names, effects, early, cancelling,
                   coreVars, histVars>>

-----------------------------------------------------------------------------
(* The driver reads what the harness wrote: Codex's and ACP's `route`, Pi's *)
(* `handle`, each one step, so the core dropping the wait between steps    *)
(* loses nothing (D-066).                                                  *)

\* The reply that picks a session: the chat is in it from now on, and its session item shows
\* it. That item settles each new-session control waiting in the chat whose old session it is
\* not (`ready_event`, `Effect::NewSession`). Requests held while a session was picked that name
\* a session still unknown are answered once no session is being picked (`early`; ACP's since
\* #122, Codex's since 2026-11-23).
RouteOpened(p, msg) ==
    LET c == msg.c
        s == msg.s
        matched == {e \in effects : e.kind = "new" /\ e.c = c /\ s # e.old}
        settles == {e.r : e \in matched}
        picking2 == [picking EXCEPT ![c] = "none"]
        stillPicking == \E d \in Chats : picking2[d] # "none"
        released == IF stillPicking THEN {} ELSE early
        refused == IF Bug \in {"HeldDropped", "StrayUnanswered"} THEN {} ELSE released
    IN
    /\ dSess' = [dSess EXCEPT ![c] = s]
    /\ picking' = picking2
    /\ effects' = effects \ matched
    /\ nSettle' = Counted(settles)
    /\ how' = Settled(settles, LAMBDA r : IF hCreator[s] = r THEN "effect" ELSE "wrong")
    /\ cSess' = [cSess EXCEPT ![c] = s]
    /\ hadSess' = [hadSess EXCEPT ![c] = @ \cup {s}]
    /\ early' = early \ released
    /\ toH' = [toH EXCEPT ![p] = AppendAll(toH[p], refused, LAMBDA a : Reply(a, "refuse"))]
    /\ droppedEarly' = droppedEarly \cup (released \ refused)
    /\ UNCHANGED <<harnessVars, up, dTurn, dAsk, names, cancelling, rq, seenItems,
                   seenTurns, issued, stopRead, late, misrouted, unwrittenTake>>

RouteStarted(p, msg) ==
    LET c == ChatOf(p, msg.s)
    IN
    /\ IF c # "none"
       THEN /\ dTurn' = [dTurn EXCEPT ![c] = msg.n]
            /\ seenTurns' = [seenTurns EXCEPT ![c] = @ \cup {<<msg.s, msg.n>>}]
       ELSE UNCHANGED <<dTurn, seenTurns>>
    /\ UNCHANGED <<harnessVars, toH, up, dSess, picking, dAsk, names, effects, early,
                   cancelling, rq, nSettle, how, cSess, hadSess, seenItems, histVars>>

\* A request of the harness. In a chat, and shown, it is an item that waits (and, while ACP
\* cancels the turn, it is answered as cancelled at once); in a chat and not shown, it is
\* answered at once (ACP's -32602, Codex's declined form or -32602). From a session no chat has,
\* it is held while a session is picked, and otherwise answered at once (ACP's -32002, #122;
\* Codex's -32602).
RouteAsk(p, msg) ==
    LET a == msg.a
        c == ChatOf(p, hAsk[a].s)
        name == NameOf(a, c)
        stray == c = "none"
        pickingAny == \E d \in Chats : picking[d] # "none"
        shown == ~stray /\ hAsk[a].shown
        answerNow == IF stray THEN ~pickingAny /\ Bug # "StrayUnanswered"
                     ELSE ~hAsk[a].shown /\ Bug # "UnshownUnanswered"
        cancelNow == shown /\ CancelOnInterrupt /\ cancelling[c]
    IN
    /\ toH' = IF answerNow THEN [toH EXCEPT ![p] = Append(@, Reply(a, "refuse"))]
              ELSE IF cancelNow THEN [toH EXCEPT ![p] = Append(@, Reply(a, "cancel"))]
              ELSE toH
    /\ early' = IF stray /\ pickingAny THEN early \cup {a} ELSE early
    /\ IF shown
       THEN /\ dAsk' = [dAsk EXCEPT ![a] = [st |-> IF cancelNow THEN "closed" ELSE "waiting",
                                            asked |-> ~cancelNow, item |-> name, c |-> c,
                                            s |-> hAsk[a].s, n |-> hAsk[a].n]]
            /\ names' = [names EXCEPT ![c] = @ \cup {name}]
            /\ seenItems' = [seenItems EXCEPT ![c] = @ \cup {name}]
            /\ issued' = issued \cup {<<c, name, a>>}
       ELSE UNCHANGED <<dAsk, names, seenItems, issued>>
    /\ UNCHANGED <<harnessVars, up, dSess, dTurn, picking, effects, cancelling, rq, nSettle,
                   how, cSess, hadSess, seenTurns, stopRead, late, misrouted, unwrittenTake,
                   droppedEarly>>

\* Codex's `serverRequest/resolved`: Codex waits no longer, so no answer may reach it. The item
\* is left to close as it would (#118).
RouteResolved(p, msg) ==
    LET a == msg.a
    IN
    /\ stopRead' = [stopRead EXCEPT ![a] = TRUE]
    /\ dAsk' = IF Bug = "ResolvedIgnored" THEN dAsk ELSE [dAsk EXCEPT ![a].asked = FALSE]
    /\ UNCHANGED <<harnessVars, toH, up, dSess, dTurn, picking, names, effects, early,
                   cancelling, coreVars, issued, late, misrouted, unwrittenTake, droppedEarly>>

\* A turn's end: the chat's items that wait in it close, and take no answer (Pi's fix in #118);
\* it settles an interrupt of that turn (`Effect::TurnEnded`).
RouteEnded(p, msg) ==
    LET c == ChatOf(p, msg.s)
        inTurn(a) == hAsk[a].st # "none" /\ hAsk[a].s = msg.s /\ hAsk[a].n = msg.n
                     /\ ProcOfAsk(a) = p
        closing == {a \in Asks : c # "none" /\ dAsk[a].c = c /\ dAsk[a].st = "waiting"
                                 /\ dAsk[a].s = msg.s /\ dAsk[a].n = msg.n}
        matched == {e \in effects : e.kind = "turn" /\ e.c = c /\ e.s = msg.s /\ e.n = msg.n}
        settles == {e.r : e \in matched}
    IN
    /\ stopRead' = [a \in Asks |-> stopRead[a] \/ inTurn(a)]
    /\ dAsk' = [a \in Asks |->
                  IF a \in closing
                  THEN [dAsk[a] EXCEPT !.st = "closed",
                                       !.asked = IF Bug = "AnswerAfterTurnEnd" THEN @ ELSE FALSE]
                  ELSE dAsk[a]]
    /\ dTurn' = IF c # "none" THEN [dTurn EXCEPT ![c] = 0] ELSE dTurn
    /\ cancelling' = IF c # "none" THEN [cancelling EXCEPT ![c] = FALSE] ELSE cancelling
    /\ effects' = effects \ matched
    /\ nSettle' = Counted(settles)
    /\ how' = Settled(settles, LAMBDA r : "effect")
    /\ UNCHANGED <<harnessVars, toH, up, dSess, picking, names, early, rq, cSess, hadSess,
                   seenItems, seenTurns, issued, late, misrouted, unwrittenTake, droppedEarly>>

\* The harness shows a session item again, unasked: a setting it changed, or the item shown
\* again. It settles a new-session control only if it is not the chat's old session.
RouteEdit(p, msg) ==
    LET c == ChatOf(p, msg.s)
        s == msg.s
        matched == {e \in effects : c # "none" /\ e.kind = "new" /\ e.c = c /\ s # e.old}
        settles == {e.r : e \in matched}
    IN
    /\ effects' = effects \ matched
    /\ nSettle' = Counted(settles)
    /\ how' = Settled(settles, LAMBDA r : IF hCreator[s] = r THEN "effect" ELSE "wrong")
    /\ UNCHANGED <<harnessVars, toH, up, dSess, dTurn, picking, dAsk, names, early,
                   cancelling, rq, cSess, hadSess, seenItems, seenTurns, histVars>>

\* The harness's output ends. Pi's `stopped`: the chat's items close, and an error the driver
\* makes settles each control still waiting, as failed; the next prompt starts a new Pi on the
\* chat's session. Codex's and ACP's `stopped`: the driver ends, and the core settles every
\* answer and control still open as failed (D-058).
RouteEof(p) ==
    LET mine(a) == hAsk[a].st # "none" /\ ProcOfAsk(a) = p
        gone == IF PerChat THEN {e \in effects : e.c = p} ELSE effects
        failed == IF PerChat THEN {e.r : e \in gone}
                  ELSE {r \in Reqs : rq[r] = "taken" /\ nSettle[r] = 0}
        closes(a) == PerChat => dAsk[a].c = p
    IN
    /\ stopRead' = [a \in Asks |-> stopRead[a] \/ mine(a)]
    /\ dAsk' = [a \in Asks |-> IF closes(a) /\ dAsk[a].st # "none"
                               THEN [dAsk[a] EXCEPT !.asked = FALSE,
                                       !.st = IF @ = "waiting" THEN "closed" ELSE @]
                               ELSE dAsk[a]]
    /\ effects' = effects \ gone
    /\ nSettle' = Counted(failed)
    /\ how' = Settled(failed, LAMBDA r : "failed")
    /\ up' = PerChat
    /\ dTurn' = [c \in Chats |-> IF PerChat => c = p THEN 0 ELSE dTurn[c]]
    /\ picking' = [c \in Chats |-> IF PerChat => c = p THEN "none" ELSE picking[c]]
    /\ cancelling' = [c \in Chats |-> IF PerChat => c = p THEN FALSE ELSE cancelling[c]]
    /\ UNCHANGED <<harnessVars, toH, dSess, names, early, rq, cSess, hadSess, seenItems,
                   seenTurns, issued, late, misrouted, unwrittenTake, droppedEarly>>

Route(p) ==
    /\ up /\ fromH[p] # <<>>
    /\ fromH' = [fromH EXCEPT ![p] = Tail(@)]
    /\ LET msg == Head(fromH[p])
       IN CASE msg.m = "opened" -> RouteOpened(p, msg)
            [] msg.m = "started" -> RouteStarted(p, msg)
            [] msg.m = "ask" -> RouteAsk(p, msg)
            [] msg.m = "resolved" -> RouteResolved(p, msg)
            [] msg.m = "ended" -> RouteEnded(p, msg)
            [] msg.m = "edit" -> RouteEdit(p, msg)
            [] msg.m = "eof" -> RouteEof(p)

\* The deliberate defect LoseOnDrop: a line is taken off the pipe and the wait it was read in is
\* dropped before it is handled. The code reads a whole line (`next_line` is safe to drop) and
\* handles it without waiting on anything.
DropWait(p) ==
    /\ Bug = "LoseOnDrop"
    /\ up /\ fromH[p] # <<>>
    /\ fromH' = [fromH EXCEPT ![p] = Tail(@)]
    /\ UNCHANGED <<harnessVars, toH, driverVars, coreVars, histVars>>

\* A driver whose one harness died has ended; the core starts it again (`Codex::start`,
\* `Acp::start`), and it knows nothing of what came before: each chat's next prompt takes up the
\* session the core saved (D-065).
Restart ==
    /\ ~PerChat /\ ~up /\ ~alive["h"] /\ fromH["h"] = <<>>
    /\ up' = TRUE
    /\ alive' = [alive EXCEPT !["h"] = TRUE]
    /\ gen' = [gen EXCEPT !["h"] = @ + 1]
    /\ toH' = [toH EXCEPT !["h"] = <<>>]
    /\ dSess' = [c \in Chats |-> 0]
    /\ dTurn' = [c \in Chats |-> 0]
    /\ picking' = [c \in Chats |-> "none"]
    /\ dAsk' = [a \in Asks |-> NoAsk]
    /\ names' = [c \in Chats |-> {}]
    /\ effects' = {}
    /\ early' = {}
    /\ cancelling' = [c \in Chats |-> FALSE]
    /\ UNCHANGED <<hTurn, hProc, hCreator, nextSess, hAsk, heard, deaths, edits, prompts,
                   fromH, coreVars, histVars>>

\* Pi's `ready_for` starts a new Pi for a chat whose Pi stopped, on the chat's session.
Spawn(c) ==
    /\ PerChat /\ up /\ ~alive[c] /\ fromH[c] = <<>>
    /\ alive' = [alive EXCEPT ![c] = TRUE]
    /\ gen' = [gen EXCEPT ![c] = @ + 1]
    /\ toH' = [toH EXCEPT ![c] = <<>>]
    /\ UNCHANGED <<hTurn, hProc, hCreator, nextSess, hAsk, heard, deaths, edits, prompts,
                   fromH, driverVars, coreVars, histVars>>

-----------------------------------------------------------------------------
(* The harness. *)

\* The harness reads what the driver wrote, in order.
HarnessReads(p) ==
    /\ alive[p] /\ toH[p] # <<>>
    /\ toH' = [toH EXCEPT ![p] = Tail(@)]
    /\ LET msg == Head(toH[p])
       IN CASE msg.m = "open" ->
                 LET s == IF msg.resume # 0 THEN msg.resume ELSE nextSess
                 IN /\ s <= MaxSessions
                    /\ nextSess' = IF msg.resume # 0 THEN nextSess ELSE nextSess + 1
                    /\ hProc' = [hProc EXCEPT ![s] = p]
                    /\ hCreator' = IF msg.resume # 0 THEN hCreator
                                   ELSE [hCreator EXCEPT ![s] = msg.r]
                    /\ fromH' = [fromH EXCEPT ![p] =
                                   Append(@, [m |-> "opened", c |-> msg.c, s |-> s, r |-> msg.r])]
                    /\ UNCHANGED <<hTurn, hAsk, heard>>
            [] msg.m = "prompt" ->
                 IF hTurn[msg.s].st = "idle"
                 THEN /\ hTurn' = [hTurn EXCEPT ![msg.s] = [n |-> @.n + 1, st |-> "running"]]
                      /\ fromH' = [fromH EXCEPT ![p] =
                                     Append(@, [m |-> "started", s |-> msg.s,
                                                n |-> hTurn[msg.s].n + 1])]
                      /\ UNCHANGED <<hProc, hCreator, nextSess, hAsk, heard>>
                 ELSE UNCHANGED <<hTurn, hProc, hCreator, nextSess, hAsk, heard, fromH>>
            [] msg.m = "reply" ->
                 /\ heard' = [heard EXCEPT ![msg.a] = @ + 1]
                 /\ hAsk' = IF hAsk[msg.a].st = "open"
                            THEN [hAsk EXCEPT ![msg.a].st = "answered"] ELSE hAsk
                 /\ UNCHANGED <<hTurn, hProc, hCreator, nextSess, fromH>>
            [] msg.m = "interrupt" ->
                 /\ hTurn' = IF hTurn[msg.s].n = msg.n /\ hTurn[msg.s].st = "running"
                             THEN [hTurn EXCEPT ![msg.s].st = "stopping"] ELSE hTurn
                 /\ UNCHANGED <<hProc, hCreator, nextSess, hAsk, heard, fromH>>
    /\ UNCHANGED <<alive, gen, deaths, edits, prompts, driverVars, coreVars, histVars>>

\* The harness asks: an approval or a question, in a session whose turn runs (or in a session no
\* chat has), about a tool call.
HarnessAsks(a, s, k, shown) ==
    LET p == IF s = 0 THEN "h" ELSE hProc[s]
    IN
    /\ hAsk[a].st = "none"
    /\ s = 0 => (StrayAsks /\ ~PerChat)
    /\ p \in Procs /\ alive[p]
    /\ hTurn[s].st = "running"
    /\ shown \in (IF Unshowable THEN BOOLEAN ELSE {TRUE})
    /\ ~ReaskAfterRestart =>
         ~\E b \in Asks : hAsk[b].st # "none" /\ hAsk[b].s = s /\ hAsk[b].k = k
                          /\ hAsk[b].gen # gen[p]
    /\ hAsk' = [hAsk EXCEPT ![a] = [st |-> "open", s |-> s, n |-> hTurn[s].n, k |-> k,
                                    shown |-> shown, gen |-> gen[p]]]
    /\ fromH' = [fromH EXCEPT ![p] = Append(@, [m |-> "ask", a |-> a])]
    /\ UNCHANGED <<hTurn, hProc, hCreator, nextSess, heard, alive, gen, deaths, edits, prompts,
                   toH, driverVars, coreVars, histVars>>

\* Codex resolves a request by itself, and says so (`serverRequest/resolved`).
HarnessResolves(a) ==
    /\ SelfResolves
    /\ hAsk[a].st = "open" /\ alive[ProcOfAsk(a)]
    /\ hAsk' = [hAsk EXCEPT ![a].st = "resolved"]
    /\ fromH' = [fromH EXCEPT ![ProcOfAsk(a)] = Append(@, [m |-> "resolved", a |-> a])]
    /\ UNCHANGED <<hTurn, hProc, hCreator, nextSess, heard, alive, gen, deaths, edits, prompts,
                   toH, driverVars, coreVars, histVars>>

\* A turn ends: by itself, once nothing it asked waits, or once it was told to stop. What it
\* still asked is dropped (Pi drops a late reply; Codex resolves each, and says so after the
\* turn's end, as the recording approval_unanswered shows).
HarnessEndsTurn(s) ==
    LET p == hProc[s]
        open == {a \in Asks : hAsk[a].st = "open" /\ hAsk[a].s = s /\ hAsk[a].n = hTurn[s].n
                              /\ ProcOfAsk(a) = p}
        ended == Append(fromH[p], [m |-> "ended", s |-> s, n |-> hTurn[s].n])
        told == IF SelfResolves THEN AppendAll(ended, open,
                                               LAMBDA a : [m |-> "resolved", a |-> a])
                ELSE ended
    IN
    /\ s # 0 /\ p \in Procs /\ alive[p]
    /\ \/ hTurn[s].st = "stopping"
       \/ hTurn[s].st = "running" /\ open = {}
    /\ hAsk' = [a \in Asks |-> IF a \in open THEN [hAsk[a] EXCEPT !.st = "dropped"] ELSE hAsk[a]]
    /\ hTurn' = [hTurn EXCEPT ![s].st = "idle"]
    /\ fromH' = [fromH EXCEPT ![p] = told]
    /\ UNCHANGED <<hProc, hCreator, nextSess, heard, alive, gen, deaths, edits, prompts, toH,
                   driverVars, coreVars, histVars>>

\* The harness shows a session item again, unasked: a setting changed, or the item again.
HarnessEdits(s) ==
    /\ edits < MaxEdits
    /\ s # 0 /\ hProc[s] \in Procs /\ alive[hProc[s]]
    /\ edits' = edits + 1
    /\ fromH' = [fromH EXCEPT ![hProc[s]] = Append(@, [m |-> "edit", s |-> s])]
    /\ UNCHANGED <<hTurn, hProc, hCreator, nextSess, hAsk, heard, alive, gen, deaths, prompts,
                   toH, driverVars, coreVars, histVars>>

\* A harness process dies: what it was asked and had not read is lost, what it asked is
\* dropped, and its output ends after what it wrote.
HarnessDies(p) ==
    /\ alive[p] /\ deaths < MaxDeaths
    /\ alive' = [alive EXCEPT ![p] = FALSE]
    /\ deaths' = deaths + 1
    /\ hAsk' = [a \in Asks |-> IF hAsk[a].st = "open" /\ ProcOfAsk(a) = p
                               THEN [hAsk[a] EXCEPT !.st = "dropped"] ELSE hAsk[a]]
    /\ hTurn' = [s \in Sess |-> IF (s = 0 /\ p = "h") \/ (s # 0 /\ hProc[s] = p)
                                THEN [hTurn[s] EXCEPT !.st = "idle"] ELSE hTurn[s]]
    /\ toH' = [toH EXCEPT ![p] = <<>>]
    /\ fromH' = [fromH EXCEPT ![p] = Append(@, [m |-> "eof"])]
    /\ UNCHANGED <<hProc, hCreator, nextSess, heard, gen, edits, prompts, driverVars,
                   coreVars, histVars>>

-----------------------------------------------------------------------------
DriverSteps ==
    \/ \E p \in Procs : Route(p)
    \/ Restart

HarnessProgress ==
    \/ \E p \in Procs : HarnessReads(p)
    \/ \E s \in Sess : hTurn[s].st = "stopping" /\ HarnessEndsTurn(s)

Next ==
    \/ \E p \in Procs : Route(p) \/ DropWait(p) \/ HarnessReads(p) \/ HarnessDies(p)
    \/ Restart
    \/ \E c \in Chats : Pick(c) \/ Prompt(c) \/ Spawn(c)
    \/ \E r \in Answers, c \in Chats, name \in UNION {seenItems[d] : d \in Chats} :
         Answer(r, c, name)
    \/ \E r \in Interrupts, c \in Chats, t \in UNION {seenTurns[d] : d \in Chats} :
         Interrupt(r, c, t)
    \/ \E r \in NewSessions, c \in Chats, s \in Sess : NewSession(r, c, s)
    \/ \E a \in Asks, s \in Sess, k \in Calls, shown \in BOOLEAN : HarnessAsks(a, s, k, shown)
    \/ \E a \in Asks : HarnessResolves(a)
    \/ \E s \in Sess : HarnessEndsTurn(s) \/ HarnessEdits(s)

Fairness ==
    /\ FairDriver => /\ \A p \in Procs : WF_vars(Route(p))
                     /\ WF_vars(Restart)
    /\ FairHarness => /\ \A p \in Procs : WF_vars(HarnessReads(p))
                      /\ \A s \in Sess : WF_vars(hTurn[s].st = "stopping" /\ HarnessEndsTurn(s))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Properties *)

TypeOK ==
    /\ up \in BOOLEAN
    /\ dSess \in [Chats -> Sess]
    /\ dTurn \in [Chats -> Int]
    /\ rq \in [Reqs -> {"unsent", "refused", "taken"}]
    /\ how \in [Reqs -> {"none", "refused", "effect", "failed", "wrong"}]
    /\ \A a \in Asks : hAsk[a].st \in {"none", "open", "answered", "resolved", "dropped"}
    /\ \A a \in Asks : dAsk[a].st \in {"none", "waiting", "answered", "closed"}

\* S1: every request is settled at most once, and only by an event that shows its own effect,
\* by a refusal, or as failed when the driver or its harness ends.
S1 == \A r \in Reqs : nSettle[r] <= 1 /\ how[r] # "wrong"

\* S5: an answer reaches only a request of the chat it was given in (an event is for the chat
\* that has its session by construction: `route`).
S5 == ~misrouted

\* S11: a request the driver takes was written whole to a harness that can read it, before the
\* driver returns; and an answer reaches the harness at most once, and only while, as far as the
\* driver has read, the harness still waits for it.
S11 ==
    /\ ~unwrittenTake
    /\ ~late
    /\ \A a \in Asks : heard[a] <= 1

\* S13: no item name is used for two requests of the harness in one chat, across restarts.
S13 == \A x, y \in issued : x[1] = y[1] /\ x[2] = y[2] => x[3] = y[3]

\* L1: every answer and control the driver takes is eventually settled.
L1 == \A r \in Reqs : rq[r] = "taken" ~> nSettle[r] > 0

\* L2: every request the harness makes is eventually answered, or stops waiting, unless it
\* waits on a person: a request still open for ever is one the driver holds for an answer for
\* ever. A turn waits on what it asked, so this is what keeps each turn able to end.
L2 == \A a \in Asks : <>[](hAsk[a].st = "open") => <>[](dAsk[a].asked)

-----------------------------------------------------------------------------
(* Reachability probes: each says an interesting state is never reached;  *)
(* TLC must find each false, so no pass is empty.                          *)

NeverAnswered == ~\E a \in Asks : hAsk[a].st = "answered" /\ dAsk[a].st = "answered"
NeverAnswerRefused == ~\E r \in Answers : rq[r] = "refused" /\ \E c \in Chats : seenItems[c] # {}
NeverInterruptSettled == ~\E r \in Interrupts : how[r] = "effect"
NeverNewSessionSettled == ~\E r \in NewSessions : how[r] = "effect"
NeverResolved == ~\E a \in Asks : hAsk[a].st = "resolved" /\ stopRead[a]
NeverClosedAtTurnEnd == ~\E a \in Asks : dAsk[a].st = "closed" /\ hAsk[a].st = "dropped"
NeverLateReply == ~\E a \in Asks : heard[a] = 1 /\ hAsk[a].st \in {"dropped", "resolved"}
NeverRestarted == ~\E p \in Procs : gen[p] > 0 /\ \E c \in Chats : dSess[c] # 0
NeverTwoChats == ~\A c \in Chats : dSess[c] # 0 /\ dTurn[c] > 0
NeverEditWhileNewSession ==
    ~\E e \in effects : e.kind = "new" /\ \E p \in Procs : fromH[p] # <<>>
        /\ Head(fromH[p]).m = "edit" /\ Head(fromH[p]).s = e.old
NeverRepeatAsk == ~\E x, y \in issued : x[1] = y[1] /\ x[3] # y[3]
                                       /\ x[2].k = y[2].k /\ x[2].s = y[2].s
NeverUnshownAnswered == ~\E a \in Asks : ~hAsk[a].shown /\ hAsk[a].st = "answered"
NeverStrayAnswered == ~\E a \in Asks : hAsk[a].s = 0 /\ hAsk[a].st = "answered"
NeverCancelled == ~\E a \in Asks : hAsk[a].st = "answered" /\ dAsk[a].st = "closed"
NeverFailed == ~\E r \in Reqs : how[r] = "failed"
=============================================================================
