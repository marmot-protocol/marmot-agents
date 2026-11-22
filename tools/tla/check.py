#!/usr/bin/env python3
"""Checks the CoreLifecycle model: every case must end as it says.

    python3 tools/tla/check.py [--out DIR] [--only NAME ...] [--counterexamples]

The cases (formal/tla/README.md lists them):

- the fair configuration passes every property;
- the unfair one, and the configurations that drop an assumption or let a send be refused or
  given up, fail as stated;
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
# The safety properties but S2, which the give-up and refusal configurations break by design.
BUT_S2 = "TypeOK S1 S3 S4 S10 S12"


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


def mutant(bug, prop, cfg=FAIR, **more):
    return Case(f"bug-{bug}", cfg, invariant(prop),
                {"Bug": f'"{bug}"', "INVARIANTS": prop, **SAFETY, **more})


def temporal_mutant(bug, prop, cfg, **more):
    return Case(f"bug-{bug}", cfg, TEMPORAL,
                {"Bug": f'"{bug}"', "INVARIANTS": None, "PROPERTIES": prop, **more})


def probe(name, cfg=FAIR, label=None, **more):
    return Case(f"reach-{label or name}", cfg, invariant(name),
                {"INVARIANTS": name, **SAFETY, **more})


# The cases about telling the chat of a dropped version have no prompt: it plays no part there.
NO_PROMPT = {"Prompts": "{}"}


CASES = [
    Case("fair", FAIR, PASS),
    Case("unfair", "CoreLifecycle.unfair.cfg", TEMPORAL,
         counterexample="unfair-mdk-never-answers",
         what="Without FairMdk, MDK never takes a step again once the core waits on a lookup: "
              "the notice that settles r1 is never posted (L1). The lookup stays enabled; it is "
              "never taken, so the trace ends in a stutter."),
    Case("no-crash-safe-sends", "CoreLifecycle.duplicate.cfg", invariant("S3"),
         counterexample="duplicate-first-event",
         what="The known limit until MDK has crash-safe tagged sends: a send MDK posted, a "
              "failed save, a restart, and a lookup that misses it; the send goes again and the "
              "chat has two first events for one item (S3)."),
    Case("token-dedupe", "CoreLifecycle.dedupe.cfg", PASS),
    Case("live-out-of-order", "CoreLifecycle.liveorder.cfg", invariant("S10"),
         counterexample="live-out-of-order",
         what="Without LiveInOrder: MDK hands a later message over first; the read mark passes "
              "the earlier one, which run.rs handle then skips (S10)."),
    Case("live-before-store", "CoreLifecycle.livestore.cfg", TEMPORAL,
         counterexample="live-before-store",
         what="Without LiveAfterStore: MDK hands a message over before storing it; order_of "
              "gives nothing, so run.rs handle drops it, and MDK does not hand it over again "
              "once stored. Nothing reads it until the core starts again (L3)."),
    Case("give-up-leaves-item-open", GIVEUP, invariant("S2"),
         counterexample="give-up-leaves-item-open",
         what="A finding in the code: an item's closing version is given up after its tries "
              "(the code: ten) and a lookup that does not find it; conversation.rs refused "
              "drops it, the item is closed in the core, and nothing closes it in the chat, not "
              "even a restart (S2; L2 too)."),
    Case("give-up-drops-settles", "CoreLifecycle.giveup-notice.cfg", TEMPORAL,
         counterexample="give-up-drops-settles",
         what="A finding in the code: a notice that settles r1 is given up; refused drops a "
              "notice with nothing in its place, and r1 is no longer pending, so it is settled "
              "nowhere (L1)."),
    # The chat is told of each version given up or refused.
    Case("give-up-told", GIVEUP, PASS,
         {"INVARIANTS": BUT_S2, "PROPERTIES": "S3Told", **NO_PROMPT}),
    Case("refuse-leaves-item-open", REFUSE, invariant("S2"),
         counterexample="refuse-leaves-item-open",
         what="A finding in the code, the same as the give-up one: MDK refuses an item's "
              "closing version in a live group (run.rs failed: EventTooLarge, "
              "InvalidAppMessagePayload), and nothing closes the item in the chat (S2; L2 "
              "too)."),
    Case("refuse-drops-settles", "CoreLifecycle.refuse-notice.cfg", TEMPORAL,
         counterexample="refuse-drops-settles",
         what="A finding in the code, the same as the give-up one: MDK refuses the notice that "
              "settles r1 in a live group, and r1 is settled nowhere (L1)."),
    Case("refuse-told", REFUSE, PASS,
         {"INVARIANTS": BUT_S2, "PROPERTIES": "S3Told", **NO_PROMPT}),
    Case("gone-group", "CoreLifecycle.gone.cfg", PASS),
    mutant("SaveAfterSend", "S1"),
    mutant("SettleUnpending", "S1"),
    mutant("RecoverKeepsOpen", "S2"),
    mutant("NoAttemptedFlag", "S3"),
    mutant("BlindResend", "S3"),
    temporal_mutant("NoFailedNotice", "S3Told", REFUSE, **NO_PROMPT),
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
    probe("NeverLookedUpAfterCrash"),
    probe("NeverDeliveredInCatchUp"),
    probe("NeverPromptAfterNewSession"),
    # Nothing is refused or given up in the fair configuration; MDK refuses every send here.
    probe("NeverDropped", "CoreLifecycle.gone.cfg"),
    probe("NeverTold", REFUSE, "NeverTold-refused", **NO_PROMPT),
    probe("NeverTold", GIVEUP, "NeverTold-given-up", **NO_PROMPT),
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
