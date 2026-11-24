--------------------------- MODULE InstallUpgrade ---------------------------
(***************************************************************************)
(* An install script's upgrade of the core and its support files (D-134),  *)
(* with what goes with it: Pi's container image and the plugin an earlier *)
(* install put into a person's Hermes (D-116). It follows install/lib.sh  *)
(* as of master 7118005 with the fixes of its branch                    *)
(* (2026-11-24): `main`, `install_core`, `publish_support`, the           *)
(* `upgrade_*` functions, `change_begin`, `change_end`, `change_put_back`, *)
(* `image_commit`, `image_put_back`, `finish` and `lock_installs`.        *)
(*                                                                         *)
(* Every shell command that changes something shared is one step, and a   *)
(* run may be killed outright (SIGKILL: no trap runs) before any step,    *)
(* its own steps of putting back included, or stopped (TERM, or a command *)
(* that fails), after which its exit trap (`finish`) puts back what it    *)
(* changed. After a kill, the person runs an installer again, of the same *)
(* harness or another.                                                     *)
(*                                                                         *)
(* What is installed: the core, the support files in parts (S stands for  *)
(* LICENSE and install, which every harness's install replaces; "pi",     *)
(* "hermes" and "claude" for each harness's own folder; Codex has none),  *)
(* the image tagged for Pi's scripts, and the person's Hermes plugin, a   *)
(* copy of the "hermes" part. Each is "none", "old" (as an earlier        *)
(* install left it) or "new" (as this release has it). An install of any  *)
(* harness installs the new release.                                       *)
(*                                                                         *)
(* Each step names itself in the variable `step`, which tools/tla/        *)
(* schedule.py reads to write a counterexample as a schedule.             *)
(* InstallUpgrade.md maps each action to the code it follows.             *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets, TLC

CONSTANTS
    Harnesses,  \* the harnesses whose installers run, from "pi", "hermes", "claude", "codex"
    Main,       \* the harness of the first runs
    Older,      \* whether an earlier install is there to upgrade (else a first install)
    FirstRuns,  \* how many runs the person starts, besides those after a kill
    Slots,      \* how many runs may be under way at once
    MaxKills,   \* how many runs are killed outright
    MaxFails,   \* how many runs fail, or are stopped with TERM
    FairPerson, \* the person runs an installer again while one left something under way
    Bug         \* a deliberate defect, or "none" (README: "Each check catches its bug")

ASSUME Main \in Harnesses /\ Harnesses \subseteq {"pi", "hermes", "claude", "codex"}
ASSUME Bug \in {"none", "NoLock", "FreshRecord", "KeepOverwrites", "RecordAfterCore",
                "ImageOutsideRecord", "ChangeEndsApart"}

None == "none"
Old == "old"
New == "new"
Unset == "unset"
Version == {None, Old, New}

\* The parts of the support files a harness's install replaces (`lib_parts`), in order.
PartsOf(h) == IF h = "codex" THEN <<"S">> ELSE <<"S", h>>
AllParts == {"S"} \cup (Harnesses \ {"codex"})
SetOf(seq) == {seq[i] : i \in DOMAIN seq}
HasImage(h) == h = "pi"      \* `harness_prepare`, `harness_commit` and `harness_put_back` of pi.sh
HasPlugin(h) == h = "hermes" \* `harness_connect` of hermes.sh, with our own plugin there before
Images == "pi" \in Harnesses
Plugins == "hermes" \in Harnesses

\* The code before the fixes of 2026-11-24, which a Bug puts back.
OldImage == Bug = "ImageOutsideRecord" \* `image_put_back` from the run's own IMAGE_BEFORE
OldChange == Bug = "ChangeEndsApart"   \* the change ended after the upgrade, apart

VARIABLES
    core,       \* $MA_CORE
    lib,        \* each part of $MA_LIB
    image,      \* what the tag Pi's scripts run names: marmot-agents-pi
    staging,    \* marmot-agents-pi:staging
    plugin,     \* our plugin in the person's Hermes
    rec,        \* $MA_UPGRADE, the record of an upgrade under way, or [on |-> FALSE]
    chg,        \* $MA_AGENT_DIR/plugin-change: the copy of the plugin it found, or Unset
    lock,       \* the slot whose run holds install.lock, or 0
    run,        \* each slot's run
    kills, fails, firstLeft,
    step        \* the step that led here, for the schedules

vars == <<core, lib, image, staging, plugin, rec, chg, lock, run, kills, fails, firstLeft, step>>

Idle == [pc |-> "idle", h |-> None, upgrading |-> FALSE, changing |-> FALSE,
         prepared |-> FALSE, tagged |-> FALSE, before |-> None, i |-> 0, then |-> "none"]

\* A record as upgrade_begin makes it: a copy of the core, and every part this run replaces, as
\* present or absent; `kept` holds the parts moved into it; `image` and `chg` are the fixes: the
\* image the tag named before the run moved it, and the plugin change moved in as it ends.
NoRecord == [on |-> FALSE, core |-> None, list |-> <<>>, found |-> [p \in AllParts |-> "unlisted"],
             kept |-> [p \in AllParts |-> None], image |-> Unset, chg |-> Unset]
Found(v) == IF v = None THEN "absent" ELSE "present"
Fresh(h) == [NoRecord EXCEPT !.on = TRUE, !.core = core, !.list = PartsOf(h),
                             !.found = [p \in AllParts |-> IF p \in SetOf(PartsOf(h))
                                                          THEN Found(lib[p]) ELSE "unlisted"]]

Init ==
    /\ core = IF Older THEN Old ELSE None
    /\ lib = [p \in AllParts |-> IF Older THEN Old ELSE None]
    /\ image = IF Older /\ Images THEN Old ELSE None
    /\ staging = None
    /\ plugin = IF Older /\ Plugins THEN Old ELSE None
    /\ rec = NoRecord
    /\ chg = Unset
    /\ lock = 0
    /\ run = [s \in Slots |-> Idle]
    /\ kills = 0 /\ fails = 0 /\ firstLeft = FirstRuns
    /\ step = [a |-> "Init"]

(***************************************************************************)
(* The person                                                              *)
(***************************************************************************)

\* Something a run left under way: an upgrade's record, or a plugin change's.
LeftUnderWay == rec.on \/ chg # Unset

\* The person starts an installer: one of the first runs, or one after a run was killed (a run
\* of any harness: the same one finishes what was left, another puts back an upgrade it cannot).
Start(s, h) ==
    /\ run[s].pc = "idle"
    /\ \/ firstLeft > 0 /\ h = Main /\ firstLeft' = firstLeft - 1
       \/ firstLeft = 0 /\ LeftUnderWay /\ \A t \in Slots : run[t].pc = "idle"
          /\ UNCHANGED firstLeft
    /\ run' = [run EXCEPT ![s] = [Idle EXCEPT !.pc = "lock", !.h = h]]
    /\ step' = [a |-> "Start", slot |-> s, harness |-> h]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, chg, lock, kills, fails>>

(***************************************************************************)
(* A run's steps                                                           *)
(***************************************************************************)

Go(s, pc, a) ==
    /\ run' = [run EXCEPT ![s].pc = pc]
    /\ step' = [a |-> a, slot |-> s]

\* `lock_installs`: flock on install.lock, waiting while another run holds it. Then
\* `install_core` looks for an upgrade left under way.
Lock(s) ==
    /\ run[s].pc = "lock"
    /\ IF Bug = "NoLock" THEN UNCHANGED lock ELSE lock = 0 /\ lock' = s
    /\ IF rec.on
       THEN run' = [run EXCEPT ![s].pc = "begin", ![s].then = "prepare"]
       ELSE run' = [run EXCEPT ![s].pc = "prepare"]
    /\ step' = [a |-> "Lock", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, chg, kills, fails, firstLeft>>

\* With the fix, a plugin change moved into the record as it ended goes back to its place first,
\* where change_begin and change_put_back look for it.
Restore(s) ==
    /\ run[s].pc \in {"begin", "f_restore"}
    /\ rec.on /\ rec.chg # Unset
    /\ chg' = rec.chg
    /\ rec' = [rec EXCEPT !.chg = Unset]
    /\ step' = [a |-> "Restore", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, lock, run, kills, fails, firstLeft>>

\* The parts the record lists that this run does not replace.
Others(s) == SetOf(rec.list) \ SetOf(PartsOf(run[s].h))

\* `upgrade_begin`: takes over an upgrade left under way that names only parts this run
\* replaces, adding its own to the list; puts back one it cannot finish; or makes a record.
Begin(s) ==
    LET r == run[s] IN
    /\ r.pc = "begin"
    /\ ~(rec.on /\ rec.chg # Unset)
    /\ IF r.upgrading
       THEN /\ run' = [run EXCEPT ![s].pc = r.then]
            /\ step' = [a |-> "Begin", slot |-> s, how |-> "already"]
            /\ UNCHANGED rec
       ELSE IF rec.on /\ Others(s) = {} /\ Bug # "FreshRecord"
       THEN LET more == SelectSeq(PartsOf(r.h), LAMBDA p : rec.found[p] = "unlisted") IN
            /\ rec' = [rec EXCEPT !.list = rec.list \o more,
                                  !.found = [p \in AllParts |->
                                     IF p \in SetOf(more) THEN Found(lib[p]) ELSE rec.found[p]]]
            /\ run' = [run EXCEPT ![s].pc = r.then, ![s].upgrading = TRUE]
            /\ step' = [a |-> "Begin", slot |-> s, how |-> "takes over"]
       ELSE IF rec.on /\ Bug # "FreshRecord"
       THEN /\ run' = [run EXCEPT ![s].pc = "pb_core", ![s].i = 0]
            /\ step' = [a |-> "Begin", slot |-> s, how |-> "puts back another's"]
            /\ UNCHANGED rec
       ELSE /\ rec' = Fresh(r.h)
            /\ run' = [run EXCEPT ![s].pc = r.then, ![s].upgrading = TRUE]
            /\ step' = [a |-> "Begin", slot |-> s, how |-> "records"]
    /\ UNCHANGED <<core, lib, image, staging, plugin, chg, lock, kills, fails, firstLeft>>

\* `harness_prepare`: Pi's image is built under marmot-agents-pi:staging. MA_PREPARED is set.
Prepare(s) ==
    LET r == run[s] IN
    /\ r.pc = "prepare"
    /\ staging' = IF HasImage(r.h) THEN New ELSE staging
    /\ run' = [run EXCEPT ![s].pc = IF Bug = "RecordAfterCore" THEN "core" ELSE "publish",
                          ![s].prepared = TRUE]
    /\ step' = [a |-> "Prepare", slot |-> s]
    /\ UNCHANGED <<core, lib, image, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* `publish_support` calls upgrade_begin, which makes the record unless the run took one over.
Publish(s) ==
    /\ run[s].pc = "publish"
    /\ run' = [run EXCEPT ![s].pc = "begin",
                          ![s].then = IF Bug = "RecordAfterCore" THEN "keep" ELSE "core",
                          ![s].i = 1]
    /\ step' = [a |-> "Publish", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* install, then mv -f of the new core into place.
Core(s) ==
    /\ run[s].pc = "core"
    /\ core' = New
    /\ run' = [run EXCEPT ![s].pc = IF Bug = "RecordAfterCore" THEN "publish" ELSE "keep",
                          ![s].i = 1]
    /\ step' = [a |-> "Core", slot |-> s]
    /\ UNCHANGED <<lib, image, staging, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* `upgrade_keep`: the part there before the upgrade moves into the record; what a run before
\* this one put there is removed.
Keep(s) ==
    LET r == run[s]
        p == PartsOf(r.h)[r.i] IN
    /\ r.pc = "keep"
    /\ IF Bug # "KeepOverwrites" /\ (rec.kept[p] # None \/ rec.found[p] = "absent")
       THEN /\ lib' = [lib EXCEPT ![p] = None]
            /\ UNCHANGED rec
       ELSE IF lib[p] # None
       THEN /\ rec' = [rec EXCEPT !.kept[p] = lib[p]]
            /\ lib' = [lib EXCEPT ![p] = None]
       ELSE UNCHANGED <<rec, lib>>
    /\ run' = [run EXCEPT ![s].pc = "put"]
    /\ step' = [a |-> "Keep", slot |-> s, part |-> p]
    /\ UNCHANGED <<core, image, staging, plugin, chg, lock, kills, fails, firstLeft>>

\* mv of the staged part into $MA_LIB.
Put(s) ==
    LET r == run[s]
        p == PartsOf(r.h)[r.i] IN
    /\ r.pc = "put"
    /\ lib' = [lib EXCEPT ![p] = New]
    /\ run' = [run EXCEPT ![s].pc = IF r.i < Len(PartsOf(r.h)) THEN "keep" ELSE "connect",
                          ![s].i = r.i + 1]
    /\ step' = [a |-> "Put", slot |-> s, part |-> p]
    /\ UNCHANGED <<core, image, staging, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* `change_begin`: a copy of the plugin there, and the record naming it; or a record a run
\* before this one left, which this run finishes.
Connect(s) ==
    LET r == run[s] IN
    /\ r.pc = "connect"
    /\ IF HasPlugin(r.h)
       THEN /\ chg' = IF chg = Unset THEN plugin ELSE chg
            /\ run' = [run EXCEPT ![s].pc = "replace", ![s].changing = TRUE]
       ELSE /\ run' = [run EXCEPT ![s].pc = "start"]
            /\ UNCHANGED chg
    /\ step' = [a |-> "Connect", slot |-> s, finishes |-> HasPlugin(r.h) /\ chg # Unset]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, lock, kills, fails, firstLeft>>

\* `replace_dir` of $MA_LIB/python/marmot_hermes into the person's Hermes (mv --exchange).
Replace(s) ==
    /\ run[s].pc = "replace"
    /\ plugin' = New
    /\ Go(s, "start", "Replace")
    /\ UNCHANGED <<core, lib, image, staging, rec, chg, lock, kills, fails, firstLeft>>

\* `start_services`: the agent set up and started. Nothing shared changes; it is where an install
\* most often fails (systemctl).
StartServices(s) ==
    /\ run[s].pc = "start"
    /\ Go(s, "commit", "StartServices")
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* `image_commit`: with the fix, the image the tag names is written into the record first, unless
\* a run before this one wrote it; then the tag moves to the staged image, and the staging tag
\* goes.
Commit(s) ==
    LET r == run[s] IN
    /\ r.pc = "commit"
    /\ IF HasImage(r.h)
       THEN /\ rec' = IF ~OldImage /\ rec.image = Unset THEN [rec EXCEPT !.image = image] ELSE rec
            /\ run' = [run EXCEPT ![s].pc = "tag"]
       ELSE /\ run' = [run EXCEPT ![s].pc = "end"]
            /\ UNCHANGED rec
    /\ step' = [a |-> "Commit", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, chg, lock, kills, fails, firstLeft>>

Tag(s) ==
    LET r == run[s] IN
    /\ r.pc = "tag"
    /\ image' = staging
    /\ staging' = None
    /\ run' = [run EXCEPT ![s].pc = "end", ![s].tagged = TRUE, ![s].before = image]
    /\ step' = [a |-> "Tag", slot |-> s]
    /\ UNCHANGED <<core, lib, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* `upgrade_end`. With the fix, a plugin change is moved into the record first, so the one
\* rename that ends the upgrade ends the change too.
End(s) ==
    LET r == run[s] IN
    /\ r.pc = "end"
    /\ IF ~OldChange /\ r.changing /\ chg # Unset
       THEN /\ rec' = [rec EXCEPT !.chg = chg]
            /\ chg' = Unset
            /\ run' = [run EXCEPT ![s].pc = "retire"]
            /\ step' = [a |-> "End", slot |-> s, change |-> TRUE]
       ELSE /\ run' = [run EXCEPT ![s].pc = "retire"]
            /\ step' = [a |-> "End", slot |-> s, change |-> FALSE]
            /\ UNCHANGED <<rec, chg>>
    /\ UNCHANGED <<core, lib, image, staging, plugin, lock, kills, fails, firstLeft>>

\* `upgrade_retire`: mv of the record away, which ends the upgrade (and, with the fix, the change).
Retire(s) ==
    /\ run[s].pc = "retire"
    /\ rec.on
    /\ rec' = NoRecord
    /\ run' = [run EXCEPT ![s].pc = "change_end", ![s].upgrading = FALSE]
    /\ step' = [a |-> "Retire", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, chg, lock, kills, fails, firstLeft>>

\* `change_end`: before the fix, the rm of the change record. The run then ends.
ChangeEnd(s) ==
    LET r == run[s] IN
    /\ r.pc = "change_end"
    /\ chg' = IF r.changing THEN Unset ELSE chg
    /\ run' = [run EXCEPT ![s] = Idle]
    /\ lock' = IF lock = s THEN 0 ELSE lock
    /\ step' = [a |-> "Done", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, kills, fails, firstLeft>>

MainPcs == {"prepare", "publish", "begin", "core", "keep", "put", "connect", "replace", "start",
            "commit", "tag", "end", "retire", "change_end", "pb_core", "pb_mv", "pb_part",
            "pb_image", "pb_retire"}

(***************************************************************************)
(* Failing, and the exit trap                                              *)
(***************************************************************************)

\* A command fails, or the run is stopped with TERM, INT or HUP: the trap runs `finish`, with
\* those signals ignored. Stopped while it puts back another's upgrade, nothing is its own yet.
PutBackPcs == {"pb_core", "pb_part", "pb_mv", "pb_image", "pb_retire"}

Fail(s) ==
    LET r == run[s] IN
    /\ r.pc \in MainPcs
    /\ ~(r.pc \in PutBackPcs /\ r.then = "f_img")
    /\ fails < MaxFails
    /\ fails' = fails + 1
    /\ run' = [run EXCEPT ![s].pc = "f_restore", ![s].then = "none",
                          ![s].prepared = r.prepared \/ r.pc = "prepare"]
    /\ step' = [a |-> "Fail", slot |-> s, at |-> run[s].pc]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, chg, lock, kills, firstLeft>>

\* `finish`, with the fix: a change moved into the record goes back first (Restore); then
\* change_put_back.
FinishChange(s) ==
    LET r == run[s] IN
    /\ r.pc = "f_restore"
    /\ ~(rec.on /\ rec.chg # Unset)
    /\ IF r.changing /\ chg # Unset
       THEN /\ plugin' = chg
            /\ run' = [run EXCEPT ![s].pc = "f_chg_rm"]
       ELSE /\ run' = [run EXCEPT ![s].pc = "f_up"]
            /\ UNCHANGED plugin
    /\ step' = [a |-> IF r.changing /\ chg # Unset THEN "PutBackPlugin" ELSE "Trap", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, rec, chg, lock, kills, fails, firstLeft>>

FinishChangeEnd(s) ==
    /\ run[s].pc = "f_chg_rm"
    /\ chg' = Unset
    /\ run' = [run EXCEPT ![s].pc = "f_up", ![s].changing = FALSE]
    /\ step' = [a |-> "DropChange", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, lock, kills, fails, firstLeft>>

\* `upgrade_put_back`, given MA_UPGRADING (and gone already when the upgrade ended).
FinishUpgrade(s) ==
    LET r == run[s] IN
    /\ r.pc = "f_up"
    /\ IF r.upgrading /\ rec.on
       THEN run' = [run EXCEPT ![s].pc = "pb_core", ![s].upgrading = FALSE, ![s].then = "f_img"]
       ELSE run' = [run EXCEPT ![s].pc = "f_img", ![s].upgrading = FALSE]
    /\ step' = [a |-> "Finish", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* `upgrade_put_back`, for this run's upgrade (from finish) or another's (from upgrade_begin):
\* the core from its copy, then each part the list names, in order.
PutBackCore(s) ==
    /\ run[s].pc = "pb_core"
    /\ core' = rec.core
    /\ run' = [run EXCEPT ![s].pc = "pb_part", ![s].i = 1]
    /\ step' = [a |-> "PutBackCore", slot |-> s]
    /\ UNCHANGED <<lib, image, staging, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* The next part: kept ones are moved back (rm, then mv), absent ones removed; one neither was not
\* moved yet, and is still in place. Skips to the image once the list is done.
PutBackPart(s) ==
    LET r == run[s] IN
    /\ r.pc = "pb_part"
    /\ IF r.i > Len(rec.list)
       THEN /\ run' = [run EXCEPT ![s].pc = "pb_image"]
            /\ step' = [a |-> "PutBackParts", slot |-> s]
            /\ UNCHANGED lib
       ELSE LET p == rec.list[r.i] IN
            IF rec.kept[p] # None
            THEN /\ lib' = [lib EXCEPT ![p] = None]
                 /\ run' = [run EXCEPT ![s].pc = "pb_mv"]
                 /\ step' = [a |-> "PutBackRemove", slot |-> s, part |-> p]
            ELSE IF rec.found[p] = "absent"
            THEN /\ lib' = [lib EXCEPT ![p] = None]
                 /\ run' = [run EXCEPT ![s].i = r.i + 1]
                 /\ step' = [a |-> "PutBackRemove", slot |-> s, part |-> p]
            ELSE /\ run' = [run EXCEPT ![s].i = r.i + 1]
                 /\ step' = [a |-> "PutBackLeave", slot |-> s, part |-> p]
                 /\ UNCHANGED lib
    /\ UNCHANGED <<core, image, staging, plugin, rec, chg, lock, kills, fails, firstLeft>>

PutBackMove(s) ==
    LET r == run[s]
        p == rec.list[r.i] IN
    /\ r.pc = "pb_mv"
    /\ rec.on /\ r.i <= Len(rec.list)
    /\ lib' = [lib EXCEPT ![p] = rec.kept[p]]
    /\ rec' = [rec EXCEPT !.kept[p] = None]
    /\ run' = [run EXCEPT ![s].pc = "pb_part", ![s].i = r.i + 1]
    /\ step' = [a |-> "PutBackMove", slot |-> s, part |-> p]
    /\ UNCHANGED <<core, image, staging, plugin, chg, lock, kills, fails, firstLeft>>

\* With the fix: the tag back on the image the record names, and the staging tag gone.
PutBackImage(s) ==
    /\ run[s].pc = "pb_image"
    /\ IF rec.image # Unset
       THEN /\ image' = rec.image
            /\ staging' = None
       ELSE UNCHANGED <<image, staging>>
    /\ Go(s, "pb_retire", "PutBackImage")
    /\ UNCHANGED <<core, lib, plugin, rec, chg, lock, kills, fails, firstLeft>>

\* `upgrade_retire`: the record goes, in one step. From upgrade_begin, the run goes on to make
\* a record of its own.
PutBackRetire(s) ==
    LET r == run[s] IN
    /\ r.pc = "pb_retire"
    /\ rec' = NoRecord
    /\ run' = [run EXCEPT ![s].pc = IF r.then = "f_img" THEN "f_img" ELSE "begin"]
    /\ step' = [a |-> "PutBackRetire", slot |-> s]
    /\ UNCHANGED <<core, lib, image, staging, plugin, chg, lock, kills, fails, firstLeft>>

\* `harness_put_back`, given MA_PREPARED: before the fix, `image_put_back` moved the tag back to
\* the image this run found when it moved it; with it, the record does that, and this takes
\* only the staging tag away. Then the run ends.
FinishImage(s) ==
    LET r == run[s] IN
    /\ r.pc = "f_img"
    /\ IF r.prepared /\ HasImage(r.h)
       THEN /\ image' = IF OldImage /\ r.tagged THEN r.before ELSE image
            /\ staging' = None
       ELSE UNCHANGED <<image, staging>>
    /\ run' = [run EXCEPT ![s] = Idle]
    /\ lock' = IF lock = s THEN 0 ELSE lock
    /\ step' = [a |-> "Exit", slot |-> s]
    /\ UNCHANGED <<core, lib, plugin, rec, chg, kills, fails, firstLeft>>

\* SIGKILL: the run ends where it is; the system lets the lock go.
Kill(s) ==
    /\ run[s].pc # "idle"
    /\ kills < MaxKills
    /\ kills' = kills + 1
    /\ run' = [run EXCEPT ![s] = Idle]
    /\ lock' = IF lock = s THEN 0 ELSE lock
    /\ step' = [a |-> "Kill", slot |-> s, at |-> run[s].pc]
    /\ UNCHANGED <<core, lib, image, staging, plugin, rec, chg, fails, firstLeft>>

Steps(s) ==
    \/ Lock(s) \/ Restore(s) \/ Begin(s) \/ Prepare(s) \/ Publish(s) \/ Core(s) \/ Keep(s)
    \/ Put(s) \/ Connect(s) \/ Replace(s) \/ StartServices(s) \/ Commit(s) \/ Tag(s) \/ End(s)
    \/ Retire(s) \/ ChangeEnd(s) \/ FinishChange(s) \/ FinishChangeEnd(s) \/ FinishUpgrade(s)
    \/ PutBackCore(s) \/ PutBackPart(s) \/ PutBackMove(s) \/ PutBackImage(s)
    \/ PutBackRetire(s) \/ FinishImage(s)

Next ==
    \/ \E s \in Slots, h \in Harnesses : Start(s, h)
    \/ \E s \in Slots : Steps(s) \/ Fail(s) \/ Kill(s)

Fairness ==
    /\ \A s \in Slots : SF_vars(Steps(s))
    /\ FairPerson => \A h \in Harnesses : SF_vars(\E s \in Slots : Start(s, h))

Spec == Init /\ [][Next]_vars /\ Fairness

(***************************************************************************)
(* Properties                                                              *)
(***************************************************************************)

TypeOK ==
    /\ core \in Version /\ image \in Version /\ staging \in Version /\ plugin \in Version
    /\ lib \in [AllParts -> Version]
    /\ chg \in Version \cup {Unset}
    /\ lock \in Slots \cup {0}

\* No run is under way, and none left anything under way.
Quiescent == (\A s \in Slots : run[s].pc = "idle") /\ ~LeftUnderWay

\* U1, what D-134 promises: once no install is under way and none left an upgrade or a plugin
\* change under way, the core and the support files every harness's install replaces are of one
\* release, Pi's image tag names the image Pi's scripts were made for, and our plugin in the
\* person's Hermes is the one the Hermes part holds. Never the new core with the old files, or
\* the other way round, whatever was killed or stopped where.
U1 ==
    Quiescent =>
        /\ core = lib["S"]
        /\ Images => image = lib["pi"]
        /\ Plugins => plugin = lib["hermes"]

\* U2: one install at a time holds the lock, and only that one changes what is shared.
U2 == \A s, t \in Slots :
        (s # t /\ run[s].pc \notin {"idle", "lock"}) => run[t].pc \in {"idle", "lock"}

\* L1: whatever a run left under way is finished or put back, once the person runs an
\* installer again (FairPerson) and the runs are not killed or stopped for ever.
L1 == <>[]Quiescent

(***************************************************************************)
(* Reachability probes: each must be violated (no pass is empty)          *)
(***************************************************************************)

\* A run takes over an upgrade a killed run left.
NeverTakenOver == ~(step.a = "Begin" /\ step.how = "takes over")
\* A run puts back an upgrade another harness's run left.
NeverPutBackOther == ~(step.a = "Begin" /\ step.how = "puts back another's")
\* A run finishes a plugin change a killed run left.
NeverChangeFinished == ~(step.a = "Connect" /\ step.finishes)
\* A failed run puts the image tag back from the record.
NeverImagePutBack == ~(step.a = "PutBackImage" /\ rec.image = Old)
\* An upgrade ends with everything new.
NeverUpgraded == ~(Quiescent /\ core = New /\ \A p \in AllParts : lib[p] = New)
\* A run killed after it put in the new core and files has them put back by a run after it.
NeverRestored == ~(step.a = "PutBackCore" /\ kills > 0 /\ core = Old /\ lib["S"] = New)
\* A second run waits for the lock.
NeverWaited == ~(\E s, t \in Slots : s # t /\ run[s].pc = "lock" /\ run[t].pc \notin {"idle", "lock"})
=============================================================================
