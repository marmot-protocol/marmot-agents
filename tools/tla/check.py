#!/usr/bin/env python3
"""Checks the CoreLifecycle model: every case must end as it says.

    python3 tools/tla/check.py [--out DIR] [--only NAME ...] [--counterexamples]

The cases (formal/tla/README.md lists them):

- the fair configuration, and those that let a send be refused or given up, pass every property;
- the unfair one, and the configurations that drop an assumption, fail as stated;
- each deliberate defect (`Bug`) fails the property it is named for: each check is shown to
  catch its bug;
- each reachability probe fails: the states the properties speak of are reached, so no pass is
  empty.

Each case runs TLC through tools/tla/run.sh in a folder of its own under --out, which keeps its
configuration and its log. A line per case gives the result and the state counts TLC reported.

A case that is a committed counterexample has its schedule made from TLC's trace by
tools/tla/schedule.py, in the shared action names, and compared with the committed
formal/tla/counterexamples/<name>.json: a schedule that changed fails the check. With
--counterexamples, the schedules are written again instead.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from schedule import constants_of, schedule_of  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
MODEL_DIR = ROOT / "formal" / "tla"
COUNTEREXAMPLES = MODEL_DIR / "counterexamples"
MODEL = "CoreLifecycle"
RUN = ROOT / "tools" / "tla" / "run.sh"

PASS = "pass"
TEMPORAL = "temporal"


def invariant(name):
    return "invariant " + name


FAIR = "CoreLifecycle.fair.cfg"
GIVEUP = "CoreLifecycle.giveup.cfg"
REFUSE = "CoreLifecycle.refuse.cfg"
SAFETY = {"PROPERTIES": None}  # drops the liveness properties: safety only


class Case:
    """A configuration, the changes to it, what TLC must say, and the counterexample it is."""

    def __init__(self, name, cfg, expected, changes=None, counterexample=None, what=None):
        self.name = name
        self.cfg = cfg
        self.expected = expected
        # A change replaces the line that starts with its key; INVARIANTS and PROPERTIES lines
        # are added when the configuration has none, and removed when the value is None.
        self.changes = changes or {}
        self.counterexample = counterexample
        self.what = what


def mutant(bug, prop, cfg=FAIR, name=None, **more):
    return Case(name or f"bug-{bug}", cfg, invariant(prop),
                {"Bug": f'"{bug}"', "INVARIANTS": prop, **SAFETY, **more})


def temporal_mutant(bug, prop, cfg, name=None, **more):
    return Case(name or f"bug-{bug}", cfg, TEMPORAL,
                {"Bug": f'"{bug}"', "INVARIANTS": None, "PROPERTIES": prop, **more})


def probe(name, cfg=FAIR, label=None, **more):
    return Case(f"reach-{label or name}", cfg, invariant(name),
                {"INVARIANTS": name, **SAFETY, **more})


# The cases about telling the chat of a dropped version have no prompt: it plays no part there.
NO_PROMPT = {"Prompts": "{}"}
# MDK refuses two sends to the live group: the last resort is reached again, as it is not with one.
TWICE = {"MaxRefusals": "2", **NO_PROMPT}


CASES = [
    Case("fair", FAIR, PASS),
    Case("unfair", "CoreLifecycle.unfair.cfg", TEMPORAL,
         counterexample="unfair-mdk-never-answers",
         what="Without FairMdk, MDK never takes a step again once a send is queued: the notice "
              "that settles r1 is never posted (L1). The send stays enabled; it is never taken, "
              "so the trace ends in a stutter."),
    Case("no-token-dedupe", "CoreLifecycle.duplicate.cfg", invariant("S3"),
         counterexample="duplicate-first-event",
         what="Without TokenDedupe, as before MDK 0.19.0's token-based sends: a send MDK posted, "
              "a failed save, a restart, and the send tried again, which MDK does not know for "
              "the one it posted; the chat has two first events for one item (S3)."),
    Case("live-out-of-order", FAIR, invariant("S10"),
         {"Bug": '"MarkFromLive"', "INVARIANTS": "S10", **SAFETY},
         counterexample="live-out-of-order",
         what="MarkFromLive, the core before MDK #3606: MDK hands a later message over first, "
              "the core handles it and moves the read mark to its place, past the earlier one, "
              "which is then never read (S10). The core now reads from the mark instead."),
    Case("live-before-store", "CoreLifecycle.livestore.cfg", TEMPORAL,
         counterexample="live-before-store",
         what="Without LiveAfterStore and without TickCatchUp: MDK hands a message over before "
              "storing it; the chat's reader finds nothing new, and MDK does not hand it over "
              "again once stored. Nothing reads it until the core starts again (L3)."),
    # With the core's tick, which reads every chat from its mark, such a message is read (L3).
    Case("live-before-store-tick", "CoreLifecycle.livestore.cfg", PASS, {"TickCatchUp": "TRUE"}),
    # A dropped send has something in its place (conversation.rs refused): the item is closed,
    # the requests settled, and the chat told of the gap.
    Case("give-up-closes-item", GIVEUP, PASS),
    Case("give-up-settles", "CoreLifecycle.giveup-notice.cfg", PASS),
    Case("refuse-closes-item", REFUSE, PASS),
    Case("refuse-settles", "CoreLifecycle.refuse-notice.cfg", PASS),
    # Two refusals, so a failed notice in place of a version is refused too, and what stands in
    # for it settles and tells of the gap; MDK never refuses a last resort (LastResortTaken).
    Case("refuse-twice", REFUSE, PASS, {"PROPERTIES": "L1 L2 L3 S3Told", **TWICE}),
    Case("last-resort-refused", REFUSE, invariant("S2"),
         {"LastResortTaken": "FALSE", **SAFETY, **TWICE},
         counterexample="last-resort-refused",
         what="The accepted limit: MDK refuses an item's closing version in a live group, then "
              "the core's close that stands in for it, though it is as small as the core makes "
              "anything. Nothing stands in for a last resort, so nothing loops, and the item "
              "stays open in the chat (S2; L2 too)."),
    Case("last-resort-given-up", REFUSE, invariant("S2"),
         {"MaxRejected": "1", "RejectedTries": "1", **SAFETY, **NO_PROMPT},
         counterexample="last-resort-given-up",
         what="The same limit, reached by a give-up: MDK refuses an item's closing version in a "
              "live group, and MDK takes the core's close that stands in for it and drops it "
              "before it goes out, which gives it up after its tries (one here; the code: ten). "
              "LastResortTaken speaks only of refusals. Nothing stands in for a last resort, so "
              "the item stays open in the chat (S2)."),
    Case("gone-group", "CoreLifecycle.gone.cfg", PASS),
    mutant("SaveAfterSend", "S1"),
    mutant("SettleUnpending", "S1"),
    mutant("RecoverKeepsOpen", "S2"),
    mutant("FreshTokenOnRestart", "S3"),
    mutant("NewTokenResend", "S3"),
    temporal_mutant("NoFailedNotice", "S3Told", REFUSE, **NO_PROMPT),
    temporal_mutant("NoRetell", "S3Told", REFUSE, **TWICE),
    mutant("NoReplacement", "S2", GIVEUP),
    temporal_mutant("NoReplacement", "L1", "CoreLifecycle.refuse-notice.cfg",
                    name="bug-NoReplacement-L1"),
    mutant("DeliverBeforeSave", "S4"),
    mutant("MarkNotLoaded", "S10"),
    mutant("CatchUpAnyOrder", "S10"),
    # A second prompt in the chat, in the same run of the driver, is what it names a session for.
    mutant("ResumeAlways", "S12", Prompts='{"p1", "p2"}'),
    mutant("KeepEndedSession", "S12"),
    mutant("NewSessionUntaken", "S12"),
    probe("NeverSettled"),
    probe("NeverSettledByEvent"),
    probe("NeverResumed"),
    probe("NeverClosedByCore"),
    probe("NeverDedupedAfterCrash"),
    probe("NeverSentAfterDrop"),
    probe("NeverDeliveredInCatchUp"),
    probe("NeverPromptAfterNewSession"),
    # Nothing is refused or given up in the fair configuration; MDK refuses every send here.
    probe("NeverDropped", "CoreLifecycle.gone.cfg"),
    probe("NeverTold", REFUSE, "NeverTold-refused", **NO_PROMPT),
    probe("NeverTold", GIVEUP, "NeverTold-given-up", **NO_PROMPT),
    probe("NeverClosedUndelivered", REFUSE, "NeverClosedUndelivered-refused", **NO_PROMPT),
    probe("NeverClosedUndelivered", GIVEUP, "NeverClosedUndelivered-given-up", **NO_PROMPT),
    probe("NeverResettled", "CoreLifecycle.refuse-notice.cfg", "NeverResettled-refused"),
    probe("NeverResettled", "CoreLifecycle.giveup-notice.cfg", "NeverResettled-given-up"),
    probe("NeverDroppedEarly", "CoreLifecycle.livestore.cfg"),
]
NAMES = [case.name for case in CASES]
assert len(set(NAMES)) == len(NAMES), "case names must differ"


def configure(text, changes):
    out, done = [], set()
    for line in text.splitlines():
        key = line.strip().split(" ")[0] if line.strip() else ""
        if key in changes:
            done.add(key)
            value = changes[key]
            if value is None:
                continue
            if key in ("INVARIANTS", "PROPERTIES"):
                out.append(f"{key} {value}")
            else:
                indent = line[: len(line) - len(line.lstrip())]
                out.append(f"{indent}{key} = {value}")
        else:
            out.append(line)
    for key in ("INVARIANTS", "PROPERTIES"):
        if key in changes and key not in done and changes[key] is not None:
            out.append(f"{key} {changes[key]}")
            done.add(key)
    missing = set(changes) - done - {"INVARIANTS", "PROPERTIES"}
    if missing:
        raise SystemExit(f"the configuration has no line for {sorted(missing)}")
    return "\n".join(out) + "\n"


def result(log):
    if "Model checking completed. No error has been found." in log:
        return PASS
    if "Temporal properties were violated." in log:
        return TEMPORAL
    found = re.search(r"Invariant (\w+) is violated", log)
    if found:
        return invariant(found.group(1))
    return "error"


def counts(log):
    found = re.findall(r"([\d,]+) states generated, ([\d,]+) distinct states found", log)
    if not found:
        return "-", "-"
    generated, distinct = found[-1]
    return generated.replace(",", ""), distinct.replace(",", "")


def breaks(case, cfg_text):
    """The property a counterexample breaks: the invariant TLC named, or the one temporal
    property its configuration checks."""
    if case.expected.startswith("invariant "):
        return case.expected.removeprefix("invariant ")
    (props,) = [l.split()[1:] for l in cfg_text.splitlines() if l.startswith("PROPERTIES")]
    (prop,) = props
    return prop


def schedule_text(case, cfg_text, log):
    """The committed form of a counterexample: one JSON object, one step to a line."""
    made = schedule_of(log, constants_of(cfg_text))
    head = {
        "case": case.counterexample,
        "model": MODEL,
        "config": case.cfg,
        "breaks": breaks(case, cfg_text),
        "what": case.what,
        "generated_by": "python3 tools/tla/check.py --counterexamples",
        "constants": constants_of(cfg_text),
        "messages": made["messages"],
    }
    lines = ["{"]
    for key, value in head.items():
        lines.append(f"  {json.dumps(key)}: {json.dumps(value)},")
    lines.append('  "steps": [')
    steps = [f"    {json.dumps(s)}" for s in made["steps"]]
    lines.append(",\n".join(steps))
    lines.append("  ],")
    lines.append(f'  "end": {json.dumps(made["end"])}')
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, default=ROOT / "target" / "tla")
    parser.add_argument("--only", nargs="+", choices=NAMES, metavar="NAME",
                        help=f"run only these cases, from: {', '.join(NAMES)}")
    parser.add_argument("--counterexamples", action="store_true",
                        help="write the committed counterexample schedules again")
    args = parser.parse_args()

    model = (MODEL_DIR / f"{MODEL}.tla").read_text()
    failed = []
    print(f"{'case':32} {'expected':18} {'result':18} {'distinct':>10} {'generated':>11} {'secs':>6}")
    for case in CASES:
        if args.only and case.name not in args.only:
            continue
        folder = args.out / case.name
        if folder.exists():
            shutil.rmtree(folder)
        folder.mkdir(parents=True)
        (folder / f"{MODEL}.tla").write_text(model)
        cfg_text = configure((MODEL_DIR / case.cfg).read_text(), case.changes)
        (folder / f"{MODEL}.cfg").write_text(cfg_text)
        began = time.monotonic()
        # A case that must fail runs on one worker, so its trace and its counts are the same on
        # every run; one that must pass may use more ($TLC_WORKERS).
        env = dict(os.environ)
        if case.expected != PASS:
            env["TLC_WORKERS"] = "1"
        ran = subprocess.run([str(RUN), str(folder), f"{MODEL}.tla", f"{MODEL}.cfg"],
                             capture_output=True, text=True, env=env)
        secs = time.monotonic() - began
        log = ran.stdout + ran.stderr
        (folder / "tlc.log").write_text(log)
        got = result(log)
        generated, distinct = counts(log)
        ok = got == case.expected
        note = ""
        if ok and case.counterexample:
            path = COUNTEREXAMPLES / f"{case.counterexample}.json"
            text = schedule_text(case, cfg_text, log)
            if args.counterexamples:
                path.write_text(text)
                note = f"   wrote {path.relative_to(ROOT)}"
            elif not path.exists() or path.read_text() != text:
                (folder / path.name).write_text(text)
                ok = False
                note = (f"   <-- its schedule is not the committed {path.relative_to(ROOT)} "
                        f"(the new one is in {folder})")
        elif not ok:
            note = "   <-- not as expected"
        print(f"{case.name:32} {case.expected:18} {got:18} {distinct:>10} {generated:>11} "
              f"{secs:6.0f}{note}", flush=True)
        if not ok:
            failed.append(case.name)
    if failed:
        print(f"not as expected: {', '.join(failed)} (logs in {args.out})", file=sys.stderr)
        sys.exit(1)
    print("every case ended as expected")


if __name__ == "__main__":
    main()
