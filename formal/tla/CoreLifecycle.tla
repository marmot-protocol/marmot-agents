--------------------------- MODULE CoreLifecycle ---------------------------
(***************************************************************************)
(* The marmot-agents core between MDK and one driver: requests pending and *)
(* settled, items and their versions, read marks, the outbox, saves and    *)
(* crashes. It models crates/marmot-agent/src/run.rs (the loop, the state  *)
(* file, the outbox and its sends), conversation.rs (what each message and *)
(* event becomes) and marmot.rs (sends), as of master 74b56d6, with       *)
(* conversation.rs `refused` as fixed on 2026-11-22: what stands in for a  *)
(* dropped send, `last_resort` and `tells`; and with the sends MDK 0.19.0  *)
(* keeps under a token (branch crash-safe-sends, 2026-11-22), which took   *)
(* the place of the lookup of a send whose outcome was not known; and     *)
(* with the reading by insertion order of MDK #3606 (branch                *)
(* mdk-insertion-paging, 2026-11-22): a live message only wakes its       *)
(* chat's reader, which reads from the mark, and the core's tick reads    *)
(* every chat from its mark.                                               *)
(*                                                                         *)
(* Action names are the ones the simulator uses, so a trace from here can  *)
(* drive it (docs/design/verification.md, step 5): Hear, Store, CatchUp,   *)
(* Ask, Deliver, Take, Refuse, Emit, Send, Save, SaveFails, Tick, Crash,   *)
(* Restart, DriverStops, Stop. Queue is not a step of its own: the steps   *)
(* that put something in the outbox call it. CatchUpDone is the end of a   *)
(* CatchUp at start. TLC prints a trace without                            *)
(* the actions' arguments; tools/tla/schedule.py reads them from the       *)
(* states, and writes each counterexample as a schedule in these names.    *)
(*                                                                         *)
(* Properties carry the ids of docs/design/verification.md: S1, S2, S3     *)
(* (with S3Told, its liveness half), S4, S10, S12 and L1 to L3. README.md  *)
(* says what is abstracted away.                                           *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Prompts,        \* chat messages that prompt the agent, from {"p1", "p2", "p3"}
    Requests,       \* answers and controls (kind 1202), from {"r1", "r2"}
    NewSession,     \* the requests that are new-session controls: a subset of Requests
    Items,          \* the driver's items, from {"t1", "s1", "t2"}; "s1" is a session item
    MaxDowns,       \* how many times the core goes down: Crash, SaveFails, DriverStops, Stop
    MaxEmits,       \* how many events the driver emits for one item; the last one closes it
    MaxAmbiguous,   \* how many sends MDK answers with an unknown outcome
    MaxRefusals,    \* how many sends to a live group MDK refuses, each for good (too large, or
                    \* a payload it will not take)
    MaxRejected,    \* how many tries MDK takes and drops before they go out
    RejectedTries,  \* how many tries of one send MDK may drop before the core gives it up
    GoneChats,      \* groups MDK refuses every send to, as a removed group
    TokenDedupe,    \* assumption: MDK keeps each send under its token (MDK 0.19.0's `submit_*`):
                    \* a send tried again under a token it posted gives back the earlier event and
                    \* posts nothing, and one under a token it dropped is dropped again
    LiveAfterStore, \* assumption: MDK hands a live message over only once it has stored it
    LastResortTaken,\* assumption: MDK never refuses a last-resort send to a live group: it is as
                    \* small as the core makes anything (the accepted limit when off)
    TickCatchUp,    \* whether the core reads every chat from its mark while it runs, as on its
                    \* tick and when the event stream lags
    FairMdk,        \* MDK eventually accepts sends and answers lookups
    FairDriver,     \* the driver eventually answers each request and closes each open item
    FairTime,       \* the clock eventually reaches every deadline
    Bug             \* a deliberate defect, to show a property catches it; "none" for the code

\* The universe the constants pick from, and the chat each belongs to.
ChatOf(x) == IF x \in {"p1", "p2", "r1", "t1", "s1"} THEN "c1" ELSE "c2"
SessionItems == Items \cap {"s1"}
Msgs == Prompts \cup Requests
Chats == {ChatOf(x) : x \in Msgs \cup Items}
\* Fixed orders, for the steps that go through a set one by one, as the code goes through a map.
ReqOrder == SelectSeq(<<"r1", "r2">>, LAMBDA r : r \in Requests)
ItemOrder == SelectSeq(<<"t1", "s1", "t2">>, LAMBDA i : i \in Items)

Bugs == {"none",
         "SaveAfterSend",      \* S1, S3: a send goes out before what queued it is saved
         "SettleUnpending",    \* S1: an event settles a request that is no longer pending
         "RecoverKeepsOpen",   \* S2: a restart leaves open items open
         "FreshTokenOnRestart",\* S3: a restart tries each queued send under a new token
         "NewTokenResend",     \* S3: a send whose outcome is unknown goes again under a new token
         "DeliverBeforeSave",  \* S4: a prompt reaches the driver before its read mark is saved
         "MarkNotLoaded",      \* S10: a restart reads every group from its start
         "CatchUpAnyOrder",    \* S10: catch-up takes any stored message after the mark
         "MarkFromLive",       \* S10: a live message is handled itself and moves the mark to
                               \* its place, as before MDK #3606
         "ResumeAlways",       \* S12: every prompt names the chat's saved session
         "KeepEndedSession",   \* S12: a session that ended stays the chat's
         "NewSessionUntaken",  \* S12: a new-session control the driver took leaves the chat untaken
         "NoFailedNotice",     \* S3Told: a version MDK refuses is dropped with no failed notice
         "NoReplacement",      \* S2, L1: a dropped close or settling notice has nothing in its place
         "NoRetell"}           \* S3Told: a dropped failed notice that settled nothing is not replaced

ASSUME Bug \in Bugs
ASSUME Prompts \subseteq {"p1", "p2", "p3"} /\ Requests \subseteq {"r1", "r2"}
ASSUME NewSession \subseteq Requests
ASSUME Items \subseteq {"t1", "s1", "t2"} /\ MaxEmits >= 1 /\ RejectedTries >= 1

NoAct == [k |-> "none", x |-> "none", sess |-> "none"]
NoSession == "none"

(***************************************************************************)
(* What the core keeps across a restart (run.rs `Kept`): for each group    *)
(* its read mark, the requests pending, each item's version, first event  *)
(* and whether it is open, each chat's session, and the outbox. Two      *)
(* monitors are kept with them, so a crash takes them back as it takes    *)
(* back the rest; the code has no such fields: `h`, how many times each   *)
(* message was handled, and `ends`, the sessions the core saw end.        *)
(*                                                                         *)
(* An outbox entry: its token (an item's id and version, or what a notice *)
(* is for), group, item ("notice" for a notice the core makes up), version,*)
(* the requests it settles, whether it closes the item, who made it        *)
(* ("drv" or "core"), how many of its tries MDK dropped (`rejected`: the   *)
(* token MDK keeps it under is the pair of the two, as the code makes it   *)
(* from both), and whether it stands in for a dropped send                 *)
(* (`last_resort`), so that nothing stands in for it in turn.              *)
(***************************************************************************)
Empty == [mark    |-> [c \in Chats |-> 0],
          h       |-> [m \in Msgs |-> 0],
          pending |-> {},
          items   |-> [i \in Items |-> [ver |-> 0, first |-> 0, open |-> FALSE]],
          sess    |-> [c \in Chats |-> NoSession],
          ends    |-> {},
          outbox  |-> <<>>]

VARIABLES
    \* MDK
    order,      \* each message's place in storage order (insert_order); 0 until stored
    nextOrder,  \* the next place
    live,       \* messages MDK has handed to the core's event stream, not yet taken
    early,      \* messages MDK handed over before storing them (only without LiveAfterStore)
    mdkTok,     \* the tokens <<send, rejected>> MDK posted under
    mdkRefused, \* the sends MDK refused in a live group: it refuses them again
    mdkDropped, \* the tokens MDK took and dropped before they went out
    \* The core
    phase,      \* "start" (recovering and catching up), "run", "stopping" or "down"
    cv,         \* the core's state in memory
    sv,         \* the state file
    taken,      \* chats the driver took a prompt for since it started (conversation.rs `taken`)
    deadline,   \* requests the driver took, whose no_effect deadline runs (volatile)
    acts,       \* acts still to carry out for the step in hand
    inflight,   \* the request the core is waiting on the driver for
    \* The driver
    drvItems,   \* each item: "new", "open", "done", or "dead" (open when the driver went)
    drvEmits,   \* how many events the driver emitted for each item
    drvReqs,    \* requests the driver took and has not named yet
    drvChats,   \* chats the driver took a prompt for since it started
    \* Monitors: what people and the harness saw. A crash takes none of it back.
    delivered,  \* how many times each prompt reached the driver
    shown,      \* how many events posted to a chat settle each request
    iposts,     \* for each item, the versions posted, in order: [v, f (first event named), fin, by,
                \* s, lr]
    dropped,    \* for each item, the versions MDK refused or the core gave up
    told,       \* the versions <<item, version>> a posted failed notice tells the chat of, itself
                \* or by replacing a failed notice that did (ToldBy)
    deduped,    \* whether a send tried again after the core went down got back the event its
                \* token was posted as
    downs,      \* how many times the core went down
    ambig,      \* how many sends had an unknown outcome
    rejects     \* how many tries MDK dropped

mdk == <<order, nextOrder, live, early, mdkTok, mdkRefused, mdkDropped>>
core == <<phase, cv, sv, taken, deadline, acts, inflight>>
drv == <<drvItems, drvEmits, drvReqs, drvChats>>
ghost == <<delivered, shown, iposts, dropped, told, deduped, downs, ambig, rejects>>
vars == <<mdk, core, drv, ghost>>

Init ==
    /\ order = [m \in Msgs |-> 0]
    /\ nextOrder = 1
    /\ live = <<>>
    /\ early = {}
    /\ mdkTok = {}
    /\ mdkRefused = {}
    /\ mdkDropped = {}
    /\ phase = "start"
    /\ cv = Empty
    /\ sv = Empty
    /\ taken = {}
    /\ deadline = {}
    /\ acts = <<>>
    /\ inflight = NoAct
    /\ drvItems = [i \in Items |-> "new"]
    /\ drvEmits = [i \in Items |-> 0]
    /\ drvReqs = {}
    /\ drvChats = {}
    /\ delivered = [p \in Prompts |-> 0]
    /\ shown = [r \in Requests |-> 0]
    /\ iposts = [i \in Items |-> <<>>]
    /\ dropped = [i \in Items |-> {}]
    /\ told = {}
    /\ deduped = FALSE
    /\ downs = 0
    /\ ambig = 0
    /\ rejects = 0

-----------------------------------------------------------------------------
(* Helpers *)

Dirty == cv # sv
Up == phase # "down"
\* Nothing in hand: the loop waits for its next input (run.rs `run_until`).
Idle == acts = <<>> /\ inflight = NoAct /\ ~Dirty

RemoveAt(s, k) == [j \in 1..(Len(s) - 1) |-> IF j < k THEN s[j] ELSE s[j + 1]]
Drop(f, x) == [y \in DOMAIN f \ {x} |-> f[y]]
Put(f, x, v) == [y \in DOMAIN f \cup {x} |-> IF y = x THEN v ELSE f[y]]
Last(s) == s[Len(s)]

\* conversation.rs `queue`.
Queue(s, e) == [s EXCEPT !.outbox = Append(@, e)]
Notice(tok, c, settles) ==
    [tok |-> tok, c |-> c, i |-> "notice", v |-> 1, s |-> settles,
     fin |-> TRUE, by |-> "core", rej |-> 0, lr |-> FALSE]
Version(i, v, settles, fin, by) ==
    [tok |-> <<i, v>>, c |-> ChatOf(i), i |-> i, v |-> v, s |-> settles,
     fin |-> fin, by |-> by, rej |-> 0, lr |-> FALSE]
LastResort(e) == [e EXCEPT !.lr = TRUE]

\* conversation.rs `settle_pending`: a notice that settles a pending request.
SettleNotice(s, r) ==
    Queue([s EXCEPT !.pending = @ \ {r}], Notice(<<"settle", r>>, ChatOf(r), {r}))

RECURSIVE SettleAll(_, _)
SettleAll(s, rs) ==
    IF rs = <<>> THEN s
    ELSE SettleAll(IF Head(rs) \in s.pending THEN SettleNotice(s, Head(rs)) ELSE s, Tail(rs))

\* conversation.rs `close_open`: an open item gets a version that closes it, as cancelled. A
\* session closed this way stays the chat's (D-065, D-070).
Close(s, i) ==
    LET v == s.items[i].ver + 1 IN
    [Queue(s, Version(i, v, {}, TRUE, "core")) EXCEPT
        !.items[i] = [ver |-> v, first |-> s.items[i].first, open |-> FALSE],
        !.sess[ChatOf(i)] = IF i \in SessionItems THEN i ELSE @]

RECURSIVE CloseAll(_, _)
CloseAll(s, is) ==
    IF is = <<>> THEN s
    ELSE CloseAll(IF s.items[Head(is)].open THEN Close(s, Head(is)) ELSE s, Tail(is))

\* conversation.rs `ended` (and `recover`): requests first, then items (D-058, D-070).
Ended(s) == CloseAll(SettleAll(s, ReqOrder), ItemOrder)

\* What a restart makes of the state file.
Recover(s) ==
    CASE Bug = "RecoverKeepsOpen" -> SettleAll(s, ReqOrder)
      [] Bug = "MarkNotLoaded"    -> [Ended(s) EXCEPT !.mark = [c \in Chats |-> 0]]
      [] OTHER                    -> Ended(s)

\* The core's process goes: what was not saved is lost, and the driver goes with it.
DriverGone ==
    /\ drvItems' = [i \in Items |-> IF drvItems[i] = "open" THEN "dead" ELSE drvItems[i]]
    /\ drvReqs' = {}
    /\ drvChats' = {}
    /\ UNCHANGED drvEmits

GoDown ==
    /\ phase' = "down"
    /\ cv' = sv
    /\ taken' = {} /\ deadline' = {}
    /\ acts' = <<>> /\ inflight' = NoAct
    /\ live' = <<>>
    /\ DriverGone
    /\ downs' = downs + 1

-----------------------------------------------------------------------------
(* MDK side *)

\* MDK stores a message for the agent, and hands it to the event stream if the core runs and
\* it did not hand it over already, before storing it.
Store(m) ==
    /\ order[m] = 0
    /\ order' = [order EXCEPT ![m] = nextOrder]
    /\ nextOrder' = nextOrder + 1
    /\ live' = IF phase \in {"start", "run"} /\ m \notin early THEN Append(live, m) ELSE live
    /\ UNCHANGED <<early, mdkTok, mdkRefused, mdkDropped, core, drv, ghost>>

-----------------------------------------------------------------------------
(* The core takes a message: run.rs `handle`, then conversation.rs `heard` *)
(* or `asked`. The read mark and what the message became are saved in one *)
(* write before anything reaches the driver (D-037, D-070).               *)

Handle(m) ==
    LET c == ChatOf(m) IN
    IF order[m] = 0 \/ order[m] <= cv.mark[c]
    THEN UNCHANGED <<cv, acts>>   \* already read; or, with MarkFromLive, not stored: dropped
    ELSE
        /\ cv' = [cv EXCEPT !.mark[c] = order[m],
                            !.h[m] = @ + 1,
                            !.pending = IF m \in Requests THEN @ \cup {m} ELSE @]
        /\ acts' = <<IF m \in Prompts
                     THEN [k |-> "prompt", x |-> m,
                           \* D-065: a prompt names the chat's session only to resume it.
                           sess |-> IF c \in taken /\ Bug # "ResumeAlways"
                                    THEN NoSession ELSE cv.sess[c]]
                     ELSE [k |-> "request", x |-> m, sess |-> NoSession]>>

\* The oldest of the messages stored in chat c after its mark, `after`. With CatchUpAnyOrder, any.
Oldest(m, after) == Bug # "CatchUpAnyOrder" => \A n \in after : order[m] <= order[n]
After(c) == {m \in Msgs : ChatOf(m) = c /\ order[m] > cv.mark[c]}

\* run.rs `catch_up_group`, one message of it: the oldest stored after the chat's mark, if any.
ReadFrom(c) ==
    IF After(c) = {} THEN UNCHANGED <<cv, acts>>
    ELSE \E m \in After(c) : Oldest(m, After(c)) /\ Handle(m)

\* run.rs `live`: takes m off the live stream, in any order, since MDK hands messages over in
\* any order (MDK #3606), and wakes the chat's reader, which reads from the mark: m itself, or
\* the message stored before it there, or nothing when it was read already. Without
\* LiveAfterStore, MDK may also hand m over before it stores it; the reader finds nothing new,
\* and MDK does not hand m over again once stored. With MarkFromLive, the core before MDK #3606:
\* m itself is handled, and the mark moves to its place.
LiveTake(m) ==
    /\ phase = "run" /\ Idle
    /\ \/ \E j \in DOMAIN live :
            /\ live[j] = m
            /\ live' = RemoveAt(live, j)
            /\ UNCHANGED early
       \/ /\ ~LiveAfterStore
          /\ order[m] = 0 /\ m \notin early
          /\ early' = early \cup {m}
          /\ UNCHANGED live
    /\ IF Bug = "MarkFromLive" THEN Handle(m) ELSE ReadFrom(ChatOf(m))
    /\ UNCHANGED <<order, nextOrder, mdkTok, mdkRefused, mdkDropped, phase, sv, taken, deadline,
                   inflight, drv, ghost>>

\* A chat message arrives live.
Hear(m) ==
    /\ m \in Prompts
    /\ LiveTake(m)

\* An answer or a control arrives live.
Ask(r) ==
    /\ r \in Requests
    /\ LiveTake(r)

\* run.rs `catch_up`: what was stored after the mark, oldest first. At start; with TickCatchUp
\* also at any idle moment while the core runs, as on its tick (every 10 s) and whenever the
\* event stream lags (RecvError::Lagged).
CatchUp(c) ==
    /\ phase = "start" \/ (phase = "run" /\ TickCatchUp)
    /\ Idle
    /\ After(c) # {}
    /\ \E m \in After(c) : Oldest(m, After(c)) /\ Handle(m)
    /\ UNCHANGED <<mdk, phase, sv, taken, deadline, inflight, drv, ghost>>

\* Nothing is left to catch up: the loop starts.
CatchUpDone ==
    /\ phase = "start" /\ Idle
    /\ \A m \in Msgs : order[m] <= cv.mark[ChatOf(m)]
    /\ phase' = "run"
    /\ UNCHANGED <<mdk, cv, sv, taken, deadline, acts, inflight, drv, ghost>>

-----------------------------------------------------------------------------
(* Carrying out acts: run.rs `carry_out`. The conversation is saved before *)
(* each act.                                                               *)

Save ==
    /\ Up /\ Dirty
    /\ sv' = cv
    /\ UNCHANGED <<mdk, phase, cv, taken, deadline, acts, inflight, drv, ghost>>

\* The state file cannot be written: the core stops where it is, as a crash would.
SaveFails ==
    /\ Up /\ Dirty /\ downs < MaxDowns
    /\ GoDown
    /\ UNCHANGED <<order, nextOrder, early, mdkTok, mdkRefused, mdkDropped, sv, drvEmits, delivered,
                   shown,
                   iposts, dropped, told, deduped, ambig, rejects>>

\* The core hands a prompt or a request to the driver, once what led to it is saved.
Deliver ==
    /\ Up /\ acts # <<>> /\ inflight = NoAct
    /\ ~Dirty \/ Bug = "DeliverBeforeSave"
    /\ inflight' = Head(acts)
    /\ acts' = Tail(acts)
    /\ delivered' = IF Head(acts).k = "prompt"
                    THEN [delivered EXCEPT ![Head(acts).x] = @ + 1] ELSE delivered
    /\ UNCHANGED <<mdk, phase, cv, sv, taken, deadline, drv, shown, iposts, dropped, told,
                   deduped, downs, ambig, rejects>>

\* The driver takes what it was handed (D-058, D-066): conversation.rs `prompted` or `requested`.
\* A new-session control it takes ends the chat's session in the driver, before the driver
\* emits the new one, so the chat is taken then, as for a prompt (D-065).
Take ==
    /\ Up /\ inflight # NoAct
    /\ LET a == inflight
           c == ChatOf(a.x)
       IN
       IF a.k = "prompt"
       THEN /\ taken' = taken \cup {c}
            /\ drvChats' = drvChats \cup {c}
            /\ UNCHANGED <<deadline, drvReqs>>
       ELSE /\ drvReqs' = drvReqs \cup {a.x}
            /\ deadline' = IF a.x \in cv.pending THEN deadline \cup {a.x} ELSE deadline
            /\ taken' = IF a.x \in NewSession /\ a.x \in cv.pending /\ Bug # "NewSessionUntaken"
                        THEN taken \cup {c} ELSE taken
            /\ drvChats' = IF a.x \in NewSession THEN drvChats \cup {c} ELSE drvChats
    /\ inflight' = NoAct
    /\ UNCHANGED <<mdk, phase, cv, sv, acts, drvItems, drvEmits, ghost>>

\* The driver refuses it: a refused prompt is not sent again and the person is told (D-037); a
\* refused request is settled by a notice that says why (D-028).
Refuse ==
    /\ Up /\ inflight # NoAct
    /\ LET a == inflight IN
       cv' = IF a.k = "prompt"
             THEN Queue(cv, Notice(<<"refused", a.x>>, ChatOf(a.x), {}))
             ELSE IF a.x \in cv.pending THEN SettleNotice(cv, a.x) ELSE cv
    /\ inflight' = NoAct
    /\ UNCHANGED <<mdk, phase, sv, taken, deadline, acts, drv, ghost>>

\* The driver emits an event for item i, closing it when fin, naming the requests S it settles
\* (D-028). The core counts the version and queues it with what it settles: conversation.rs
\* `emitted` and `queue_versions`. Only a pending request of the item's own chat is settled.
Emit(i, S, fin) ==
    /\ phase = "run" /\ Idle
    /\ drvItems[i] \in {"new", "open"}
    /\ drvEmits[i] < MaxEmits
    /\ drvEmits[i] + 1 = MaxEmits => fin
    /\ S \subseteq drvReqs
    /\ LET c == ChatOf(i)
           v == cv.items[i].ver + 1
           settles == IF Bug = "SettleUnpending"
                      THEN {r \in S : ChatOf(r) = c}
                      ELSE {r \in S \cap cv.pending : ChatOf(r) = c}
           sess == IF i \notin SessionItems THEN cv.sess[c]
                   ELSE IF ~fin THEN i
                   \* A session that ended is no longer the chat's to resume (D-065).
                   ELSE IF cv.sess[c] = i /\ Bug # "KeepEndedSession" THEN NoSession
                   ELSE cv.sess[c]
       IN /\ cv' = [Queue(cv, Version(i, v, settles, fin, "drv")) EXCEPT
                       !.pending = @ \ settles,
                       !.items[i] = [ver |-> v, first |-> cv.items[i].first, open |-> ~fin],
                       !.sess[c] = sess,
                       !.ends = IF i \in SessionItems /\ fin THEN @ \cup {i} ELSE @]
          /\ deadline' = deadline \ settles
    /\ drvItems' = [drvItems EXCEPT ![i] = IF fin THEN "done" ELSE "open"]
    /\ drvEmits' = [drvEmits EXCEPT ![i] = @ + 1]
    /\ drvReqs' = drvReqs \ S
    /\ UNCHANGED <<mdk, phase, sv, taken, acts, inflight, drvChats, delivered, shown,
                   iposts, dropped, told, deduped, downs, ambig, rejects>>

\* The clock reaches a taken control's deadline with no effect shown: conversation.rs `overdue`
\* settles it with no_effect (D-087).
Tick(r) ==
    /\ phase = "run" /\ Idle
    /\ r \in deadline /\ r \in cv.pending
    /\ cv' = SettleNotice(cv, r)
    /\ deadline' = deadline \ {r}
    /\ UNCHANGED <<mdk, phase, sv, taken, acts, inflight, drv, ghost>>

-----------------------------------------------------------------------------
(* Sends: run.rs `flush`. Each group's sends go oldest first, each under   *)
(* the token it was saved with, which MDK keeps (marmot.rs `send`): a send *)
(* whose outcome is unknown, or that a crash left queued, is tried again   *)
(* under the same token, and a try MDK dropped under a new one (S1, S3).   *)

\* The oldest entry for chat c, by its place in the outbox.
HeadFor(s, c) == CHOOSE k \in DOMAIN s.outbox :
                    /\ s.outbox[k].c = c
                    /\ \A j \in DOMAIN s.outbox : s.outbox[j].c = c => k <= j
HasFor(s, c) == \E k \in DOMAIN s.outbox : s.outbox[k].c = c

\* The token MDK keeps a send under: its name, and how many of its tries MDK dropped
\* (conversation.rs `mdk_token`, from the state file's salt, the name and that count).
MdkToken(e) == <<e.tok, e.rej>>
\* The chat sees a send. With token dedupe, MDK posts a token once.
Posts(e) == ~(TokenDedupe /\ MdkToken(e) \in mdkTok)
\* The id of the event a send became: for an item, its place among the item's posts.
PostId(e) ==
    IF e.i = "notice" THEN 0
    ELSE IF Posts(e) THEN Len(iposts[e.i]) + 1
    ELSE CHOOSE k \in DOMAIN iposts[e.i] : iposts[e.i][k].v = e.v

\* The versions a notice with token t tells the chat of: a failed notice tells of the version it
\* stands in for, or of what the failed notice it replaces told of; any other notice, of nothing.
RECURSIVE ToldBy(_)
ToldBy(t) == IF t[1] # "failed" THEN {}
             ELSE IF t[2][1] \in Items THEN {t[2]}
             ELSE ToldBy(t[2])

\* MDK posts e, filled in as conversation.rs `next_send` fills it: a later version names the
\* item's first event.
Post(s, e) ==
    IF Posts(e)
    THEN /\ mdkTok' = mdkTok \cup {MdkToken(e)}
         /\ shown' = [r \in Requests |-> IF r \in e.s THEN shown[r] + 1 ELSE shown[r]]
         /\ iposts' = IF e.i = "notice" THEN iposts
                      ELSE [iposts EXCEPT ![e.i] = Append(@,
                             [v |-> e.v, f |-> s.items[e.i].first, fin |-> e.fin, by |-> e.by,
                              s |-> e.s, lr |-> e.lr])]
         /\ told' = IF e.i = "notice" THEN told \cup ToldBy(e.tok) ELSE told
    ELSE UNCHANGED <<mdkTok, shown, iposts, told>>

\* conversation.rs `sent`: the entry goes, and the item's first event is kept.
Sent(s, k, id) ==
    LET e == s.outbox[k]
        s1 == [s EXCEPT !.outbox = RemoveAt(@, k)]
    IN IF e.i # "notice" /\ s.items[e.i].first = 0
       THEN [s1 EXCEPT !.items[e.i].first = id] ELSE s1

\* conversation.rs `close_undelivered`: an item the core holds closed, with nothing later of it
\* queued, gets its next version, closing it, from the core, settling nothing, as a last resort.
CloseUndelivered(s, i) ==
    IF s.items[i].open \/ \E j \in DOMAIN s.outbox : s.outbox[j].i = i THEN s
    ELSE LET v == s.items[i].ver + 1 IN
         [Queue(s, LastResort(Version(i, v, {}, TRUE, "core"))) EXCEPT !.items[i].ver = v]

\* conversation.rs `refused` (MDK refused the send in a live group, or the core gave it up): the
\* send is dropped, and something stands in for it (`stand_in_for`), unless it is itself a last
\* resort, so nothing loops. In place of an item's version: the item's close, when that version
\* closed it and nothing later is queued, then a failed notice that settles what the version
\* settled. In place of a notice that settled requests, or of a failed notice (which tells of a
\* dropped send, and is marked `tells` in the code): a failed notice that settles what it settled,
\* as a last resort. Any other notice that settled nothing is not replaced.
Refused(s, k) ==
    LET e == s.outbox[k]
        s1 == [s EXCEPT !.outbox = RemoveAt(@, k)]
        failed == Notice(<<"failed", e.tok>>, e.c, e.s)
    IN CASE e.lr -> s1
         \* The code before the fix: a version gets its failed notice only; a notice, nothing.
         [] Bug = "NoReplacement" -> IF e.i = "notice" THEN s1 ELSE Queue(s1, failed)
         [] e.i = "notice" ->
                IF e.s = {} /\ (e.tok[1] # "failed" \/ Bug = "NoRetell") THEN s1
                ELSE Queue(s1, LastResort(failed))
         [] Bug = "NoFailedNotice" -> CloseUndelivered(s1, e.i)
         [] OTHER -> Queue(CloseUndelivered(s1, e.i), failed)
DropGhost(e) ==
    dropped' = IF e.i = "notice" THEN dropped ELSE [dropped EXCEPT ![e.i] = @ \cup {e.v}]

\* Whether MDK refuses every send of e to chat c: a gone group, or a send it refused before
\* (too large, or a payload it will not take: the same bytes are refused again).
ForGood(c, e) == c \in GoneChats \/ e.tok \in mdkRefused

\* The entry at k with one more of its tries dropped by MDK: its next try is under a new token.
Bumped(s, k) == [s EXCEPT !.outbox[k].rej = @ + 1]

\* MDK's answer to a send of the entry at k for chat c, under its token: Sent, Busy, Ambiguous
\* (it went out or it did not, and the core cannot tell), Rejected (MDK took it and dropped it
\* before it went out: nothing went), or Refused: every send to a gone group, and up to
\* MaxRefusals sends to a live one, each refused again whenever it is sent (run.rs `failed`:
\* EventTooLarge, InvalidAppMessagePayload). The group stays live; the core drops the send. With
\* LastResortTaken, MDK refuses no last-resort send to a live group. With TokenDedupe, MDK
\* answers a token it posted with the event it posted, posting nothing, and a token it dropped
\* with its drop. A send MDK dropped RejectedTries tries of is given up, as refused.
MdkSend(c, k, outcome) ==
    LET e == cv.outbox[k]
        kept == TokenDedupe /\ MdkToken(e) \in mdkTok
        lost == TokenDedupe /\ MdkToken(e) \in mdkDropped
    IN
    IF ForGood(c, e) \/ outcome = "refused"
    THEN /\ outcome = "refused"
         /\ ~kept /\ ~lost
         /\ \/ ForGood(c, e)
            \/ Cardinality(mdkRefused) < MaxRefusals /\ ~(LastResortTaken /\ e.lr)
         /\ mdkRefused' = IF c \in GoneChats THEN mdkRefused ELSE mdkRefused \cup {e.tok}
         /\ cv' = Refused(cv, k) /\ DropGhost(e)
         /\ UNCHANGED <<mdkTok, mdkDropped, shown, iposts, told, deduped, ambig, rejects>>
    ELSE /\ UNCHANGED mdkRefused
         /\ CASE outcome = "sent" ->
                /\ ~lost
                /\ Post(cv, e)
                /\ cv' = Sent(cv, k, PostId(e))
                /\ deduped' = (deduped \/ (kept /\ downs >= 1))
                /\ UNCHANGED <<mdkDropped, dropped, ambig, rejects>>
           [] outcome = "busy" ->
                /\ UNCHANGED <<cv, mdkTok, mdkDropped, shown, iposts, told, dropped, deduped,
                               ambig, rejects>>
           [] outcome \in {"ambiguous-posted", "ambiguous-lost"} ->
                /\ ~lost
                /\ ambig < MaxAmbiguous
                /\ ambig' = ambig + 1
                /\ IF outcome = "ambiguous-posted" THEN Post(cv, e)
                   ELSE UNCHANGED <<mdkTok, shown, iposts, told>>
                /\ cv' = IF Bug = "NewTokenResend" THEN Bumped(cv, k) ELSE cv
                /\ UNCHANGED <<mdkDropped, dropped, deduped, rejects>>
           [] outcome = "rejected" ->
                /\ ~kept
                /\ \/ lost /\ UNCHANGED <<mdkDropped, rejects>>
                   \/ ~lost /\ rejects < MaxRejected
                     /\ rejects' = rejects + 1
                     /\ mdkDropped' = mdkDropped \cup {MdkToken(e)}
                /\ IF e.rej + 1 >= RejectedTries
                   THEN cv' = Refused(Bumped(cv, k), k) /\ DropGhost(e)
                   ELSE cv' = Bumped(cv, k) /\ UNCHANGED dropped
                /\ UNCHANGED <<mdkTok, shown, iposts, told, deduped, ambig>>
           [] OTHER -> FALSE

SendOutcomes == {"sent", "busy", "ambiguous-posted", "ambiguous-lost", "refused", "rejected"}

\* The core hands the oldest send for chat c to MDK. It sends between steps: right after the
\* step that queued it (each step queues after its driver call returns), on its retry timer, at
\* start and while stopping; never in the middle of a driver call. What it sends is saved
\* first, with its token.
Send(c, outcome) ==
    /\ Up /\ acts = <<>> /\ inflight = NoAct
    /\ ~Dirty \/ Bug = "SaveAfterSend"
    /\ HasFor(cv, c)
    /\ MdkSend(c, HeadFor(cv, c), outcome)
    /\ UNCHANGED <<order, nextOrder, live, early, phase, sv, taken, deadline, acts, inflight, drv,
                   delivered, downs>>

-----------------------------------------------------------------------------
(* Going down and coming back *)

\* The process dies: everything not saved is lost.
Crash ==
    /\ Up /\ downs < MaxDowns
    /\ GoDown
    /\ UNCHANGED <<order, nextOrder, early, mdkTok, mdkRefused, mdkDropped, sv, drvEmits, delivered,
                   shown,
                   iposts, dropped, told, deduped, ambig, rejects>>

\* The driver ends, or the core is told to stop: everything open is closed and every pending
\* request settled as failed, all queued and saved before any of it is sent (run.rs, after the
\* loop). The sends go out while the core stops; what is left goes when it starts again.
StopWith ==
    /\ cv' = Ended(cv)
    /\ phase' = "stopping"
    /\ taken' = {} /\ deadline' = {} /\ live' = <<>>
    /\ DriverGone
    /\ downs' = downs + 1
    /\ UNCHANGED <<order, nextOrder, early, mdkTok, mdkRefused, mdkDropped, sv, acts, inflight,
                   delivered, shown, iposts, dropped, told, deduped, ambig, rejects>>
\* The driver ends.
DriverStops ==
    /\ phase = "run" /\ Idle /\ downs < MaxDowns
    /\ StopWith
\* The core is told to stop.
Stop ==
    /\ phase = "run" /\ Idle /\ downs < MaxDowns
    /\ StopWith

\* The core starts again from the state file: what was handed to MDK may have gone out, and is
\* tried again under the token it was saved with; what was left open is closed and what was
\* pending settled (D-070); then catch-up.
Restart ==
    /\ \/ phase = "down"
       \/ phase = "stopping" /\ ~Dirty
    /\ cv' = IF Bug = "FreshTokenOnRestart"
             THEN LET r == Recover(sv) IN
                  [r EXCEPT !.outbox =
                      [k \in DOMAIN r.outbox |-> [r.outbox[k] EXCEPT !.rej = @ + 1]]]
             ELSE Recover(sv)
    /\ phase' = "start"
    /\ taken' = {} /\ deadline' = {} /\ acts' = <<>> /\ inflight' = NoAct /\ live' = <<>>
    /\ DriverGone
    /\ UNCHANGED <<order, nextOrder, early, mdkTok, mdkRefused, mdkDropped, sv, ghost>>

-----------------------------------------------------------------------------

Next ==
    \/ \E m \in Msgs : Store(m)
    \/ \E p \in Prompts : Hear(p)
    \/ \E r \in Requests : Ask(r)
    \/ \E c \in Chats : CatchUp(c)
    \/ CatchUpDone
    \/ Save \/ SaveFails \/ Deliver \/ Take \/ Refuse
    \/ \E i \in Items, S \in SUBSET Requests, fin \in BOOLEAN : Emit(i, S, fin)
    \/ \E r \in Requests : Tick(r)
    \/ \E c \in Chats, o \in SendOutcomes : Send(c, o)
    \/ Crash \/ DriverStops \/ Stop \/ Restart

(***************************************************************************)
(* Fairness: one condition for each party, each over all its progress     *)
(* steps. Bounded faults (crashes, failed saves, stops, unknown outcomes,  *)
(* dropped tries)                                                          *)
(* end, since each has a bound.                                            *)
(*   The core always: it saves, hands over, restarts, catches up at start  *)
(*               and, with TickCatchUp, on its tick, and takes live        *)
(*               messages.                                                 *)
(*   FairDriver: the driver answers each request and closes each open item.*)
(*   FairMdk:    a send MDK keeps being offered is eventually accepted,    *)
(*               or, for good, refused, or, under a token it dropped,      *)
(*               dropped again.                                            *)
(*   FairTime:   the clock reaches each deadline.                          *)
(* Strong fairness, since most steps wait for the core to be idle, which  *)
(* comes and goes. A condition over a party's steps together is weaker    *)
(* than one for each step, so properties that hold under it hold under    *)
(* the stronger one too; it also keeps TLC's liveness check small.        *)
(***************************************************************************)
CoreSteps ==
    \/ Save \/ Deliver \/ Restart \/ CatchUpDone
    \/ \E c \in Chats : CatchUp(c)
    \/ \E m \in Msgs : Hear(m) \/ Ask(m)
DriverSteps ==
    \/ Take \/ Refuse
    \/ \E i \in Items, S \in SUBSET Requests : drvItems[i] = "open" /\ Emit(i, S, TRUE)
\* A send MDK refuses for good is answered with its refusal, and one under a token MDK dropped
\* with its drop; that is MDK answering too.
HeadForGood(c) == HasFor(cv, c) /\ ForGood(c, cv.outbox[HeadFor(cv, c)])
HeadLost(c) ==
    HasFor(cv, c) /\ TokenDedupe /\ MdkToken(cv.outbox[HeadFor(cv, c)]) \in mdkDropped
MdkSteps ==
    \E c \in Chats :
        \/ Send(c, "sent")
        \/ HeadForGood(c) /\ Send(c, "refused")
        \/ HeadLost(c) /\ Send(c, "rejected")
TimeSteps == \E r \in Requests : Tick(r)

Fairness ==
    /\ SF_vars(CoreSteps)
    /\ FairDriver => SF_vars(DriverSteps)
    /\ FairMdk => SF_vars(MdkSteps)
    /\ FairTime => SF_vars(TimeSteps)

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Safety *)

TypeOK ==
    /\ phase \in {"start", "run", "stopping", "down"}
    /\ order \in [Msgs -> 0..Cardinality(Msgs)]
    /\ inflight.k \in {"none", "prompt", "request"}
    /\ downs <= MaxDowns /\ ambig <= MaxAmbiguous /\ rejects <= MaxRejected
    /\ \A k \in DOMAIN cv.outbox : cv.outbox[k].c \in Chats

\* S1: every answer and control is settled at most once in the chat; L1 says at least once.
\* (D-028, D-070, D-087)
S1 == \A r \in Requests : shown[r] <= 1

\* S2: no item is left open after the driver stops or the core restarts (D-058, D-070).
\* (a) A stop or a restart closes everything the core holds open.
\* (b) An item a chat shows as open is open in the state file, or the file holds a later
\*     version of it still to send: the core never forgets one.
OpenInChat(i) == iposts[i] # <<>> /\ ~Last(iposts[i]).fin
S2 ==
    /\ phase \in {"start", "stopping"} => \A i \in Items : ~cv.items[i].open
    /\ \A i \in Items : OpenInChat(i) =>
          \/ sv.items[i].open
          \/ \E k \in DOMAIN sv.outbox :
                sv.outbox[k].i = i /\ sv.outbox[k].v > Last(iposts[i]).v

\* S3: an item's versions rise by one, and every later version names its first event (D-025,
\* D-112). A version MDK refused, or that the core gave up after MDK dropped its tries, is a
\* gap; nothing else is (S3Told below checks that the chat is told of each gap). The first event
\* is the first one posted.
S3 ==
    \A i \in Items :
        LET P == iposts[i] IN
        /\ \A k \in 2..Len(P) : P[k].v > P[k - 1].v
        /\ \A k \in DOMAIN P :
              \A w \in 1..(P[k].v - 1) : w \in dropped[i] \/ \E j \in 1..(k - 1) : P[j].v = w
        /\ Len(P) >= 1 => P[1].f = 0
        /\ \A k \in 2..Len(P) : P[k].f = 1

\* S4: a prompt reaches the driver at most once, across any crash (D-037, D-070).
S4 == \A p \in Prompts : delivered[p] <= 1

\* S10: the reader hands each message over once, in storage order, with no
\* gaps (D-037): everything up to a group's mark was handled, and nothing twice, in memory and
\* in the state file. With the mark only rising, that is storage order.
Covered(s) ==
    \A m \in Msgs : (order[m] > 0 /\ order[m] <= s.mark[ChatOf(m)]) => s.h[m] >= 1
S10 ==
    /\ Covered(cv) /\ Covered(sv)
    /\ \A m \in Msgs : cv.h[m] <= 1 /\ sv.h[m] <= 1

\* S12: a prompt names a session only to resume it, never an ended one (D-065): only when the
\* driver has taken no prompt and no new-session control for that chat since it started, and
\* never one the core saw end.
\* A driver's event that ends a session and is lost in a crash before it is saved is not seen:
\* the driver went with it, and the core resumes the session it kept (README.md).
S12 ==
    (inflight.k = "prompt" /\ inflight.sess # NoSession) =>
        /\ ChatOf(inflight.x) \notin drvChats
        /\ inflight.sess \notin cv.ends

Safety == TypeOK /\ S1 /\ S2 /\ S3 /\ S4 /\ S10 /\ S12

-----------------------------------------------------------------------------
(* Liveness, under the fairness above. A gone group sees nothing, so it is *)
(* left out.                                                               *)

\* L1: every answer and control stored for the agent is eventually settled in its chat.
L1 == \A r \in Requests : (ChatOf(r) \notin GoneChats /\ order[r] > 0) ~> shown[r] >= 1

\* L2: every turn, and every item, eventually ends: one a chat shows as open, or the core holds
\* open, is eventually closed in both.
L2 == \A i \in Items :
        (ChatOf(i) \notin GoneChats /\ (OpenInChat(i) \/ sv.items[i].open))
            ~> (~OpenInChat(i) /\ ~sv.items[i].open)

\* L3: every message stored for the agent is eventually handled, and that is saved.
L3 == \A m \in Msgs : order[m] > 0 ~> sv.h[m] >= 1

\* S3, its second half: a gap in an item's versions is told. Every version dropped in a live
\* group (refused by MDK, or given up) is eventually told of in a failed notice posted to the
\* chat, unless the version went out after all (a crash took back the drop, and the send was
\* made again).
Versions == 1..(MaxEmits + 1)
S3Told == \A i \in Items, v \in Versions :
            (ChatOf(i) \notin GoneChats /\ v \in dropped[i])
                ~> (<<i, v>> \in told \/ \E k \in DOMAIN iposts[i] : iposts[i][k].v = v)

-----------------------------------------------------------------------------
(* Reachability. Each says an interesting state never happens; TLC must   *)
(* find each one false, or the properties above could pass on a model     *)
(* that never gets there.                                                  *)

\* A request is settled in its chat.
NeverSettled == \A r \in Requests : shown[r] = 0
\* A request is settled by the driver's own event naming it, in an item's version.
NeverSettledByEvent == \A i \in Items : \A k \in DOMAIN iposts[i] : iposts[i][k].s = {}
\* A prompt resumes the chat's session after the core went down and started again.
NeverResumed == ~(downs >= 1 /\ inflight.k = "prompt" /\ inflight.sess # NoSession)
\* A restart or a stop closes an item the chat showed open.
NeverClosedByCore == \A i \in Items : \A k \in DOMAIN iposts[i] : iposts[i][k].by # "core"
\* After the core went down, a send that went out is tried again under its token, and MDK gives
\* back the event it posted.
NeverDedupedAfterCrash == ~deduped
\* A send MDK dropped a try of goes out, under a new token.
NeverSentAfterDrop == \A t \in mdkTok : t[2] = 0
\* A message is handed to the driver during a catch-up after a restart.
NeverDeliveredInCatchUp == ~(downs >= 1 /\ phase = "start" /\ inflight # NoAct)
\* A version is refused or given up.
NeverDropped == \A i \in Items : dropped[i] = {}
\* A failed notice that tells of a dropped version is posted.
NeverTold == told = {}
\* The core's close of an item whose closing version was dropped is posted.
NeverClosedUndelivered == \A i \in Items : \A k \in DOMAIN iposts[i] : ~iposts[i][k].lr
\* A failed notice in place of a dropped notice that settled requests is posted.
NeverResettled == ~\E t \in mdkTok : t[1][1] = "failed" /\ t[1][2][1] \notin Items
\* A message MDK handed over before storing it is dropped, and stored after.
NeverDroppedEarly == \A m \in early : order[m] = 0
\* A prompt reaches the driver after it took a new-session control for the chat.
NeverPromptAfterNewSession ==
    ~(inflight.k = "prompt" /\ \E r \in NewSession \cap drvReqs : ChatOf(r) = ChatOf(inflight.x))
=============================================================================
