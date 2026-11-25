-------------------------- MODULE ActivityHoldBack --------------------------
(***************************************************************************)
(* The core holding back the agent's full activity from a chat that is no *)
(* longer the owner's two-person chat (D-136), while MDK may already hold  *)
(* what it holds back. It follows crates/marmot-agent/src/conversation.rs *)
(* (`emitted`, `settle_for_hidden`, `members_now`, `hold_back`,            *)
(* `next_send`, `attempting`, `sent`, `lost`) and run.rs `flush`, as of    *)
(* master 4d7baac with the fixes of branch held-sends (2026-11-25),       *)
(* against MDK's sends under tokens (marmot.rs `send`, `outcome`).        *)
(*                                                                         *)
(* One chat, one group. The turn T is sent to every chat; its first       *)
(* version went out before the model starts. The tool K is the agent's    *)
(* full activity. One request, r1, waits for what K's closing version     *)
(* shows. The driver emits K running, K done settling r1, and T done.     *)
(* Someone may be added to the chat, and leave again (Flip): MDK no longer *)
(* marks it a two-person chat of the agent and the owner.                 *)
(*                                                                         *)
(* MDK takes a send under its token and posts it at once, keeps it to post *)
(* later (the core is told `Pending`), or posts it or keeps it and the     *)
(* answer is lost (`Unknown`). Tried again under the same token, MDK       *)
(* answers from what it keeps, posting nothing again, if what it is handed *)
(* is what it was handed first; anything else under the token it refuses  *)
(* as another submission, and the core gives the send up with a notice    *)
(* that settles what it settled. T and K carry a native message, which a  *)
(* chat not sent the full activity is sent without (`as_sent`).           *)
(*                                                                         *)
(* MDK may let go of a token it posted (MdkForgets), as it does once the   *)
(* group's messages disappear: the post leaves the chat, and MDK takes the *)
(* token as new if it is handed it again. A send the core handed to MDK in *)
(* a form the chat is no longer sent, the core never hands it again: it    *)
(* asks MDK what became of it (`Side::outcome`), and tells the chat of one *)
(* MDK no longer knows that it may not have been delivered (D-109).        *)
(* MDK's status may name no event for a send its engine has (`named`), as  *)
(* for one whose answer the core lost; the core then goes on with the     *)
(* event MDK named in a Pending answer, if it kept one (`Queued::event`), *)
(* else hands it again as it was handed while MDK keeps the token (D-152, *)
(* `Core::hand_again`), else waits until MDK lets go of the token.        *)
(*                                                                         *)
(* Each step names itself in the variable `step`, which tools/tla/        *)
(* schedule.py reads to write a counterexample as a schedule.             *)
(* ActivityHoldBack.md maps each action to the code it follows.           *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    MaxFlips,   \* how many times who is in the chat changes
    MaxLost,    \* how many of MDK's answers are lost
    MdkKeeps,   \* MDK may keep a send to post later, and answer Pending
    FairMdk,    \* MDK eventually posts a send it keeps
    MdkForgets, \* MDK may let go of a token whose send it posted, the post disappearing with it
    Bug         \* a deliberate defect, or "none"

ASSUME Bug \in {"none", "HoldAttempted", "NoMove", "HoldNothing", "RetryChanged", "HandAgain",
                "ForgotHeld", "SettleOwned"}

\* The code before the fix of 2026-11-24: hold_back took every send of the full activity out of
\* the outbox, a send MDK had already been handed too.
HoldAttempted == Bug = "HoldAttempted"

\* The code before the second fix of 2026-11-24: a send tried again under its token left out its
\* native message by who was in the chat at that try, not as MDK was first handed it.
RetryChanged == Bug = "RetryChanged"

\* The code before the fix of 2026-11-25 (#194): a send handed to MDK otherwise than the chat is
\* sent now was handed to MDK again under its token, once MDK said it kept the token
\* (`Side::keeps`), to learn its event. MDK could let go of the token in between.
HandAgain == Bug = "HandAgain"

\* The code before the fix of 2026-11-25 (#182): a send MDK no longer knows was taken for one it
\* never took, and held back, or sent as the chat is sent now.
ForgotHeld == Bug = "ForgotHeld"

\* The code before the review of 2026-11-25: a send MDK's engine has, whose status names no
\* event and whose event the core did not keep, was told as one that may not have been delivered
\* when MDK might let go of its token within the minute (`lasts` false), while MDK still had it.
SettleOwned == Bug = "SettleOwned"

\* Whether a send handed otherwise than the chat is sent now is handed to MDK again: the code
\* before the fixes. `RetryChanged` is older still, and hands it as the chat is sent now.
OldPath == HandAgain \/ RetryChanged

\* The turn, the tool, and the core's notices of a send given up (`stand_in_for`), each a new
\* item in the code, here one name: only what they settle is checked.
Items == {"T", "K", "N"}
Kind(i) == IF i = "K" THEN "full" ELSE "every"
\* The items that carry a native message: the driver's.
Native(i) == i # "N"
NoEvent == <<>>

\* What the driver emits, in order: an item, whether it closes, the requests it settles.
Script == <<[item |-> "K", fin |-> FALSE, settles |-> {}],
            [item |-> "K", fin |-> TRUE, settles |-> {"r1"}],
            [item |-> "T", fin |-> TRUE, settles |-> {}]>>

VARIABLES
    members,    \* who MDK says is in the chat: "owners" (the agent and the owner) or "more"
    owners,     \* what the core last read of it (conversation.rs `owners`)
    pending,    \* the requests not yet settled by anything queued (`saved.pending`)
    version,    \* each item's last version queued (`Record::version`)
    first,      \* each item's first event as the core knows it (`Record::first`), or NoEvent
    outbox,     \* what is queued, oldest first (`saved.outbox`)
    mdk,        \* each token MDK was handed: what it keeps, whether it posted it, whether its
                \* status names its event once posted (`named`), and whether the core, asking,
                \* read that MDK may let go of it within the minute (`soon`)
    forgot,     \* the tokens MDK let go of
    chat,       \* what the chat was sent, in order, and whether each post disappeared since
    emitted,    \* how much of the script the driver emitted
    flips, lost,
    step

vars == <<members, owners, pending, version, first, outbox, mdk, forgot, chat, emitted, flips,
          lost, step>>

Tok(i, v) == <<i, v>>
\* `bare`: whether its last try went without its native message (`Queued::bare`). `asked`: the
\* code before #194 asked MDK whether it kept the token, before it handed the send again.
\* `event`: whether the core kept the event MDK named in a Pending answer (`Queued::event`).
Queued(i, v, s) == [tok |-> Tok(i, v), item |-> i, settles |-> s, attempted |-> FALSE,
                    bare |-> FALSE, asked |-> FALSE, event |-> FALSE]

Init ==
    /\ members = "owners"
    /\ owners = TRUE
    /\ pending = {"r1"}
    /\ version = [i \in Items |-> IF i = "T" THEN 1 ELSE 0]
    /\ first = [i \in Items |-> IF i = "T" THEN Tok("T", 1) ELSE NoEvent]
    /\ outbox = <<>>
    /\ mdk = [t \in {} |-> 0]
    /\ forgot = {}
    /\ chat = <<[tok |-> Tok("T", 1), item |-> "T", names |-> NoEvent, settles |-> {}, to |-> "owners",
                 gone |-> FALSE]>>
    /\ emitted = 0
    /\ flips = 0 /\ lost = 0
    /\ step = [a |-> "Init"]

\* The next version of the turn, the same as its last, carrying `s` (`queue_versions` on the
\* item `carrier_of` gives: the chat's turn).
ByTurn(box, s) ==
    IF s = {} THEN <<box, version>>
    ELSE <<Append(box, Queued("T", version["T"] + 1, s)), [version EXCEPT !["T"] = @ + 1]>>

(***************************************************************************)
(* The driver, and the core's `emitted`                                    *)
(***************************************************************************)

\* `emitted`: a full-activity event in a chat the core last read as not the owner's is not
\* queued, and what it settles is settled by the turn's next version (`settle_for_hidden`);
\* anything else is queued as the item's next version, settling what it names.
Emit ==
    /\ emitted < Len(Script)
    /\ LET e == Script[emitted + 1]
           s == e.settles \cap pending IN
       IF owners = FALSE /\ Kind(e.item) = "full" /\ Bug # "HoldNothing"
       THEN LET by == ByTurn(outbox, s) IN
            /\ outbox' = by[1]
            /\ version' = by[2]
            /\ pending' = pending \ s
            /\ step' = [a |-> "Emit", item |-> e.item, shown |-> FALSE]
       ELSE /\ outbox' = Append(outbox, Queued(e.item, version[e.item] + 1, s))
            /\ version' = [version EXCEPT ![e.item] = @ + 1]
            /\ pending' = pending \ s
            /\ step' = [a |-> "Emit", item |-> e.item, shown |-> TRUE]
    /\ emitted' = emitted + 1
    /\ UNCHANGED <<members, owners, first, mdk, forgot, chat, flips, lost>>

\* Someone is added to the chat, or leaves it.
Flip ==
    /\ flips < MaxFlips
    /\ flips' = flips + 1
    /\ members' = IF members = "owners" THEN "more" ELSE "owners"
    /\ step' = [a |-> "Flip", members |-> members']
    /\ UNCHANGED <<owners, pending, version, first, outbox, mdk, forgot, chat, emitted, lost>>

(***************************************************************************)
(* The core's `flush`                                                      *)
(***************************************************************************)

Full(q) == Kind(q.item) = "full"

\* `hold_back`: the sends of the full activity MDK was never handed taken out of the outbox (before
\* the fix, every one), and what each settled settled by the turn instead.
Held(q) == Full(q) /\ (HoldAttempted \/ ~q.attempted)

HoldBack(box) ==
    LET kept == SelectSeq(box, LAMBDA q : ~Held(q))
        gone == SelectSeq(box, LAMBDA q : Held(q))
        s == UNION {gone[k].settles : k \in DOMAIN gone} IN
    IF Bug = "NoMove" THEN <<kept, version>> ELSE ByTurn(kept, s)

\* `members_now` before each send: the core reads who is in the chat, and holds back what the
\* chat is no longer sent. `held` counts the sends it held back.
ReadMembers ==
    /\ owners' = (members = "owners")
    /\ owners' # owners
    /\ LET holds == ~owners' /\ Bug # "HoldNothing" IN
       /\ IF holds
          THEN LET by == HoldBack(outbox) IN outbox' = by[1] /\ version' = by[2]
          ELSE UNCHANGED <<outbox, version>>
       /\ step' = [a |-> "ReadMembers", owners |-> owners',
                    held |-> IF holds THEN Len(SelectSeq(outbox, Held)) ELSE 0]
    /\ UNCHANGED <<members, pending, first, mdk, forgot, chat, emitted, flips, lost>>

Posted(t, names, settles) ==
    [tok |-> t, item |-> t[1], names |-> names, settles |-> settles, to |-> members, gone |-> FALSE]

\* `sent`: the event MDK gave back for the send at the head; the item's first event, if it had
\* none.
Took(q, rest) ==
    /\ outbox' = rest
    /\ first' = IF first[q.item] = NoEvent THEN [first EXCEPT ![q.item] = q.tok] ELSE first

\* Whether a send goes without its native message, handed to MDK now: a try again under a token
\* MDK keeps, as MDK was first handed it (`Conversation::bare`); a first try, or one MDK has
\* nothing under the token for (`not_kept`), as the chat is sent now. Before the fix, every try
\* as the chat is sent now.
Bare(q) == IF q.tok \in DOMAIN mdk /\ ~RetryChanged THEN q.bare ELSE ~owners

\* MDK answers a send under a token it keeps from what it keeps: the event it posted, or Pending.
\* Handed anything else under the token, it refuses it as another submission, and the core gives
\* the send up (`give_up`, `refused`): a notice settles in its place what it settled.
Answered(q, rest) ==
    IF Native(q.item) /\ Bare(q) # mdk[q.tok].bare
    THEN /\ outbox' = Append(rest, Queued("N", version["N"] + 1, q.settles))
         /\ version' = [version EXCEPT !["N"] = @ + 1]
         /\ step' = [a |-> "Send", tok |-> q.tok, outcome |-> "refused"]
         /\ UNCHANGED <<first, mdk, chat, lost>>
    ELSE IF mdk[q.tok].posted
    THEN /\ Took(q, rest)
         /\ step' = [a |-> "Send", tok |-> q.tok, outcome |-> "sent"]
         /\ UNCHANGED <<version, mdk, chat, lost>>
    ELSE /\ outbox' = <<[q EXCEPT !.event = TRUE]>> \o rest
         /\ step' = [a |-> "Send", tok |-> q.tok, outcome |-> "pending"]
         /\ UNCHANGED <<version, first, mdk, chat, lost>>

\* `lost`: a send MDK no longer knows may have gone out; the chat is told it may not have been
\* delivered, by a notice that settles what it settled (D-109; the version that closes the item
\* in its place is left out, as for a refusal). `a` names the step.
Lost(q, rest, a) ==
    /\ outbox' = Append(rest, Queued("N", version["N"] + 1, q.settles))
    /\ version' = [version EXCEPT !["N"] = @ + 1]
    /\ step' = [a |-> a, tok |-> q.tok, outcome |-> "lost"]
    /\ UNCHANGED <<first, mdk, chat, lost>>

\* `not_kept`: a send MDK never took, of a form the chat is no longer sent. The full activity is
\* held back now, what it settled settled by the turn; anything else goes as the chat is sent
\* now, under the same token, which nothing binds. `a` names the step.
NotKept(q, rest, a) ==
    IF Full(q) /\ ~owners
    THEN LET by == ByTurn(rest, q.settles) IN
         /\ outbox' = by[1] /\ version' = by[2]
         /\ step' = [a |-> a, tok |-> q.tok, outcome |-> "held back"]
         /\ UNCHANGED <<first, mdk, chat, lost>>
    ELSE /\ outbox' = <<[q EXCEPT !.bare = ~owners]>> \o rest
         /\ step' = [a |-> a, tok |-> q.tok, outcome |-> "not kept"]
         /\ UNCHANGED <<version, first, mdk, chat, lost>>

\* Whether MDK was handed the send `q` otherwise than the chat is sent now (`handed_otherwise`):
\* the full activity, which the chat is no longer sent, or an item with its native message or
\* without.
HandedOtherwise(q) ==
    /\ q.attempted
    /\ \/ Full(q) /\ ~owners
       \/ Native(q.item) /\ q.bare = owners

\* MDK's engine has the send `q` and its status names no event, and the core kept none
\* (`Outcome::Taken`). While MDK will keep the token another minute (`Side::lasts`), the core hands
\* it again as it was handed, which MDK answers from what it keeps: the one second hand-over left
\* (D-152, `Core::hand_again`), here with no stall between the ask and the hand-over, which is
\* D-148's accepted limit (`HandAgain` shows it). Else MDK may let go of the token within the minute,
\* and the core asks again until it does (`soon`, which `Spec` makes MDK do); `SettleOwned` tells
\* the chat at once instead.
Owned(q, rest) ==
    IF mdk[q.tok].soon
    THEN /\ step' = [a |-> "Ask", tok |-> q.tok, outcome |-> "owned"]
         /\ UNCHANGED <<outbox, version, first, mdk, chat, lost>>
    ELSE \/ /\ Took(q, rest)
            /\ step' = [a |-> "Ask", tok |-> q.tok, outcome |-> "handed again"]
            /\ UNCHANGED <<version, mdk, chat, lost>>
         \/ /\ MdkForgets
            /\ IF SettleOwned
               THEN Lost(q, rest, "Ask")
               ELSE /\ mdk' = [mdk EXCEPT ![q.tok].soon = TRUE]
                    /\ step' = [a |-> "Ask", tok |-> q.tok, outcome |-> "owned"]
                    /\ UNCHANGED <<outbox, version, first, chat, lost>>

\* Since #194, such a send is never handed to MDK again: the core asks MDK what became of it
\* (`Side::outcome`, MDK's `local_send_status`), and MDK gives its event once it posted it, says
\* it keeps it queued, or says it has no such token. Since #182, one MDK let go of is told as
\* D-109 says (`lost`), and only one MDK never took is held back or sent otherwise.
Ask(q, rest) ==
    IF q.tok \in forgot /\ ~ForgotHeld
    THEN Lost(q, rest, "Ask")
    ELSE IF q.tok \in DOMAIN mdk
    THEN IF mdk[q.tok].posted /\ (mdk[q.tok].named \/ q.event)
         THEN /\ Took(q, rest)
              /\ step' = [a |-> "Ask", tok |-> q.tok,
                           outcome |-> IF mdk[q.tok].named THEN "went" ELSE "kept event"]
              /\ UNCHANGED <<version, mdk, chat, lost>>
         ELSE IF mdk[q.tok].posted
         THEN Owned(q, rest)
         ELSE /\ step' = [a |-> "Ask", tok |-> q.tok, outcome |-> "queued"]
              /\ UNCHANGED <<outbox, version, first, mdk, chat, lost>>
    ELSE NotKept(q, rest, "Ask")

\* The code before #194: MDK is asked whether it keeps the token (`Side::keeps`; before #182, a
\* token it let go of is not kept), and the send is handed to it again under the token, as it
\* was first handed (`RetryChanged`: as the chat is sent now). With `HandAgain`, the two are
\* steps apart, as they are in the code: MDK may let go of the token in between, and then takes
\* the send as new, posting it to the chat as it is now.
HandedAgain(q, rest) ==
    IF HandAgain /\ ~q.asked
    THEN IF q.tok \in DOMAIN mdk
         THEN /\ outbox' = <<[q EXCEPT !.asked = TRUE]>> \o rest
              /\ step' = [a |-> "Send", tok |-> q.tok, outcome |-> "asked"]
              /\ UNCHANGED <<version, first, mdk, chat, lost>>
         ELSE NotKept(q, rest, "Send")
    ELSE IF q.tok \in DOMAIN mdk
    THEN Answered(q, rest)
    ELSE IF q.asked
    THEN /\ mdk' = q.tok :> [names |-> first[q.item], settles |-> q.settles, bare |-> q.bare,
                             posted |-> TRUE, named |-> TRUE, soon |-> FALSE] @@ mdk
         /\ chat' = Append(chat, Posted(q.tok, first[q.item], q.settles))
         /\ Took(q, rest)
         /\ step' = [a |-> "Send", tok |-> q.tok, outcome |-> "taken as new"]
         /\ UNCHANGED <<version, lost>>
    ELSE NotKept(q, rest, "Send")

\* The send at the head of the outbox, the members read just before (`owners` is fresh). One
\* MDK was handed otherwise than the chat is sent now goes on only as MDK says (`Ask`). Any
\* other is handed to MDK under its token: `next_send` fills in the item's first event,
\* `attempting` notes that MDK may have it, and MDK answers; one MDK let go of is not handed
\* again, and the chat is told it may not have been delivered (D-109, marmot.rs `send`).
Send ==
    /\ outbox # <<>>
    /\ owners = (members = "owners")
    /\ LET q == Head(outbox)
           rest == Tail(outbox)
           names == first[q.item] IN
       IF HandedOtherwise(q) /\ Bug # "HoldNothing"
       THEN IF OldPath THEN HandedAgain(q, rest) ELSE Ask(q, rest)
       ELSE IF Full(q) /\ ~owners /\ Bug # "HoldNothing"
       THEN FALSE \* held back by `ReadMembers`, never handed in this chat
       ELSE IF q.tok \in forgot
       THEN Lost(q, rest, "Send")
       ELSE IF q.tok \in DOMAIN mdk
       THEN Answered(q, rest)
       ELSE \E outcome \in {"sent", "pending", "posted-lost", "kept-lost", "busy"},
                named \in BOOLEAN :
            \* MDK's status may name no event for a send whose answer was not "sent".
            /\ ~named => outcome \in {"pending", "posted-lost", "kept-lost"}
            /\ outcome \in {"pending", "kept-lost"} => MdkKeeps
            /\ outcome \in {"posted-lost", "kept-lost", "busy"} => lost < MaxLost
            /\ lost' = IF outcome \in {"posted-lost", "kept-lost", "busy"} THEN lost + 1 ELSE lost
            /\ mdk' = IF outcome = "busy" THEN mdk
                      ELSE q.tok :> [names |-> names, settles |-> q.settles, bare |-> Bare(q),
                                     posted |-> outcome \in {"sent", "posted-lost"},
                                     named |-> named, soon |-> FALSE] @@ mdk
            /\ chat' = IF outcome \in {"sent", "posted-lost"}
                       THEN Append(chat, Posted(q.tok, names, q.settles)) ELSE chat
            /\ IF outcome = "sent"
               THEN Took(q, rest)
               ELSE /\ outbox' = <<[q EXCEPT !.attempted = TRUE, !.bare = Bare(q),
                                               !.event = (outcome = "pending")]>> \o rest
                    /\ UNCHANGED first
            /\ step' = [a |-> "Send", tok |-> q.tok, outcome |-> outcome]
            /\ UNCHANGED version
    /\ UNCHANGED <<members, owners, pending, forgot, emitted, flips>>

\* MDK posts a send it kept, as it was handed: to whoever is in the chat then.
MdkPosts(t) ==
    /\ t \in DOMAIN mdk
    /\ ~mdk[t].posted
    /\ mdk' = [mdk EXCEPT ![t].posted = TRUE]
    /\ chat' = Append(chat, Posted(t, mdk[t].names, mdk[t].settles))
    /\ step' = [a |-> "MdkPosts", tok |-> t, members |-> members]
    /\ UNCHANGED <<members, owners, pending, version, first, outbox, forgot, emitted, flips, lost>>

\* MDK lets go of a token whose send it posted, before the core learnt what became of it: the
\* group's messages disappeared, the post with them (D-109). The driver's items only: what stands
\* in for a send given up is the core's last word on it.
Forget(t) ==
    /\ MdkForgets
    /\ t[1] # "N"
    /\ t \in DOMAIN mdk
    /\ mdk[t].posted
    /\ \E k \in DOMAIN outbox : outbox[k].tok = t
    /\ mdk' = [u \in DOMAIN mdk \ {t} |-> mdk[u]]
    /\ forgot' = forgot \cup {t}
    /\ chat' = [k \in DOMAIN chat |-> IF chat[k].tok = t THEN [chat[k] EXCEPT !.gone = TRUE]
                                                         ELSE chat[k]]
    /\ step' = [a |-> "Forget", tok |-> t]
    /\ UNCHANGED <<members, owners, pending, version, first, outbox, emitted, flips, lost>>

Core == ReadMembers \/ Send
Mdk == \E t \in DOMAIN mdk : MdkPosts(t)

Next == Emit \/ Flip \/ Core \/ Mdk \/ (\E t \in DOMAIN mdk : Forget(t))

\* MDK lets go of a token the core read it may let go of within the minute (`soon`).
LetsGo == \E t \in DOMAIN mdk : mdk[t].soon /\ Forget(t)

Spec == Init /\ [][Next]_vars /\ WF_vars(Emit) /\ WF_vars(Core) /\ (FairMdk => WF_vars(Mdk))
        /\ WF_vars(LetsGo)

(***************************************************************************)
(* Properties                                                              *)
(***************************************************************************)

\* The posts of the item `i` still in the chat: a post that disappeared is no longer seen.
Posts(i) == {k \in DOMAIN chat : chat[k].item = i /\ ~chat[k].gone}

TypeOK ==
    /\ members \in {"owners", "more"} /\ owners \in BOOLEAN
    /\ pending \subseteq {"r1"}
    /\ \A k \in DOMAIN outbox : outbox[k].item \in Items

\* H1 (S1 of the verification plan, for D-136): every request is settled at most once in the chat.
\* A post that disappeared is no longer seen.
H1 == \A r \in {"r1"} :
        Cardinality({k \in DOMAIN chat : r \in chat[k].settles /\ ~chat[k].gone}) <= 1

\* H2 (S3, for D-136): an item has one first event in the chat, and each later version names it.
H2 == \A i \in Items \ {"N"} :
        \A k \in Posts(i) :
            IF \A j \in Posts(i) : j >= k
            THEN chat[k].names = NoEvent
            ELSE chat[k].names = chat[CHOOSE f \in Posts(i) : \A j \in Posts(i) : j >= f].tok

\* H4 (D-136's privacy): the full activity is posted only while the chat is the owner's
\* two-person chat. The core reads who is in it just before it hands a send to MDK; a send MDK
\* keeps, MDK posts on its own, later, to whoever is in the chat then.
H4 == \A k \in DOMAIN chat : Kind(chat[k].item) = "full" => chat[k].to = "owners"

\* H5 (D-109, #182): the core takes a send for one MDK never took, holding it back or sending it
\* as the chat is sent now, only if MDK never posted it.
H5 == step.a \in {"Send", "Ask"} /\ step.outcome \in {"held back", "not kept"}
        => \A k \in DOMAIN chat : chat[k].tok # step.tok

\* L1: every request is eventually settled in the chat.
L1 == <>(\A r \in {"r1"} : \E k \in DOMAIN chat : r \in chat[k].settles)

\* Reachability probes: each must be violated. A send of the tool still queued is held back
\* once an earlier version of it went out;
\* r1 is settled by the turn, and by the tool; the tool goes again once the chat is the owner's
\* again; MDK holds back what it was never handed, at its send; MDK posts a tool it kept.
NeverHeldBack == ~(step.a = "ReadMembers" /\ step.held > 0 /\ \E k \in DOMAIN chat : chat[k].item = "K")
NeverSettledByTurn == ~(\E k \in DOMAIN chat : chat[k].item = "T" /\ "r1" \in chat[k].settles)
NeverSettledByTool == ~(\E k \in DOMAIN chat : chat[k].item = "K" /\ "r1" \in chat[k].settles)
NeverOwnersAgain == ~(step.a = "Send" /\ step.tok[1] = "K" /\ step.outcome = "sent"
                      /\ flips = 2 /\ members = "owners")
NeverHeldAtSend == ~(step.a = "Ask" /\ step.outcome = "held back")
NeverKeptPosted == ~(step.a = "MdkPosts" /\ step.tok[1] = "K")
\* The turn's outcome learnt by asking MDK, in a chat sent otherwise than MDK was handed it.
NeverAskedOtherwise == ~(step.a = "Ask" /\ step.tok[1] = "T" /\ step.outcome = "went")
\* A send of the tool MDK let go of is told as not known, after someone joined.
NeverToldLost == ~(step.a = "Ask" /\ step.tok[1] = "K" /\ step.outcome = "lost")
\* A send MDK's engine has, its status naming no event, goes on with the event the core kept; is
\* handed again while its token lasts; is waited on while MDK may let go of the token.
NeverKeptEvent == ~(step.a = "Ask" /\ step.outcome = "kept event")
NeverHandedAgainOwned == ~(step.a = "Ask" /\ step.outcome = "handed again")
NeverOwnedWaited == ~(step.a = "Ask" /\ step.outcome = "owned")
=============================================================================
