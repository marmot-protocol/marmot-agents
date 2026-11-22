#!/usr/bin/env python3
"""Checks the CoreLifecycle model: every case must end as it says.

    python3 tools/tla/check.py [--out DIR] [--only NAME ...] [--counterexamples]

The cases (formal/tla/README.md lists them):

- the fair configuration passes every property;
- the unfair one, and the configurations that drop an assumption, fail as stated;
- each deliberate defect (`Bug`) fails the property it is named for: each check is shown to
  catch its bug;
- each reachability probe fails: the states the properties speak of are reached, so no pass is
  empty.

Each case runs TLC through tools/tla/run.sh in a folder of its own under --out, which keeps its
configuration and its log. A line per case gives the result and the state counts TLC reported.
With --counterexamples, the traces of the cases that are committed as counterexamples are
written again into formal/tla/counterexamples/.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MODEL_DIR = ROOT / "formal" / "tla"
MODEL = "CoreLifecycle"
RUN = ROOT / "tools" / "tla" / "run.sh"

PASS = "pass"
TEMPORAL = "temporal"


def invariant(name):
    return "invariant " + name


# (case, configuration file, changes to it, expected result, counterexample file or None).
# A change replaces the line that starts with its key, or the INVARIANTS / PROPERTIES lines.
FAIR = "CoreLifecycle.fair.cfg"
SAFETY = {"PROPERTIES": None}  # drops the liveness properties: safety only


def mutant(bug, prop, **more):
    return (f"bug-{bug}", FAIR, {"Bug": f'"{bug}"', "INVARIANTS": prop, **SAFETY, **more},
            invariant(prop), None)


def probe(name, cfg=FAIR):
    return (f"reach-{name}", cfg, {"INVARIANTS": name, **SAFETY}, invariant(name), None)


CASES = [
    ("fair", FAIR, {}, PASS, None),
    ("unfair", "CoreLifecycle.unfair.cfg", {}, TEMPORAL, "unfair-mdk-never-answers"),
    ("no-crash-safe-sends", "CoreLifecycle.duplicate.cfg", {}, invariant("S3"),
     "duplicate-first-event"),
    ("token-dedupe", "CoreLifecycle.dedupe.cfg", {}, PASS, None),
    ("live-out-of-order", "CoreLifecycle.liveorder.cfg", {}, invariant("S10"),
     "live-out-of-order"),
    ("give-up-leaves-item-open", "CoreLifecycle.giveup.cfg", {}, invariant("S2"),
     "give-up-leaves-item-open"),
    ("give-up-drops-settles", "CoreLifecycle.giveup-notice.cfg", {}, TEMPORAL,
     "give-up-drops-settles"),
    ("gone-group", "CoreLifecycle.gone.cfg", {}, PASS, None),
    mutant("SaveAfterSend", "S1"),
    mutant("SettleUnpending", "S1"),
    mutant("RecoverKeepsOpen", "S2"),
    mutant("NoAttemptedFlag", "S3"),
    mutant("BlindResend", "S3"),
    mutant("DeliverBeforeSave", "S4"),
    mutant("MarkNotLoaded", "S10"),
    mutant("CatchUpAnyOrder", "S10"),
    # A second prompt in the chat, in the same run of the driver, is what it names a session for.
    mutant("ResumeAlways", "S12", Prompts='{"p1", "p2"}'),
    mutant("KeepEndedSession", "S12"),
    probe("NeverSettled"),
    probe("NeverSettledByEvent"),
    probe("NeverResumed"),
    probe("NeverClosedByCore"),
    probe("NeverLookedUpAfterCrash"),
    probe("NeverDeliveredInCatchUp"),
    # Nothing is refused or given up in the fair configuration; MDK refuses every send here.
    probe("NeverDropped", "CoreLifecycle.gone.cfg"),
]


def configure(text, changes):
    lines = text.splitlines()
    out = []
    for line in lines:
        key = line.strip().split(" ")[0] if line.strip() else ""
        if key in changes:
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


def trace(log):
    """The error trace TLC printed, from its first state to the end of its last."""
    start = log.find("Error: ")
    if start < 0:
        return log
    end = log.find("\nFinished in", start)
    kept = log[start:end if end > 0 else len(log)]
    # TLC's progress and timing reports name times, which change on every run.
    timed = r"^(\d+ states generated|Progress\(|Checking |Finished checking )"
    lines = [l for l in kept.splitlines() if not re.match(timed, l)]
    return "\n".join(lines).rstrip() + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, default=ROOT / "target" / "tla")
    parser.add_argument("--only", nargs="*", help="run only these cases")
    parser.add_argument("--counterexamples", action="store_true",
                        help="write the committed counterexample traces again")
    args = parser.parse_args()

    model = (MODEL_DIR / f"{MODEL}.tla").read_text()
    failed = []
    print(f"{'case':32} {'expected':16} {'result':16} {'distinct':>10} {'generated':>11} {'secs':>6}")
    for name, cfg, changes, expected, counterexample in CASES:
        if args.only and name not in args.only:
            continue
        folder = args.out / name
        if folder.exists():
            shutil.rmtree(folder)
        folder.mkdir(parents=True)
        (folder / f"{MODEL}.tla").write_text(model)
        (folder / f"{MODEL}.cfg").write_text(configure((MODEL_DIR / cfg).read_text(), changes))
        began = time.monotonic()
        # A case that must fail runs on one worker, so its trace and its counts are the same on
        # every run; one that must pass may use more ($TLC_WORKERS).
        env = dict(os.environ)
        if expected != PASS:
            env["TLC_WORKERS"] = "1"
        ran = subprocess.run([str(RUN), str(folder), f"{MODEL}.tla", f"{MODEL}.cfg"],
                             capture_output=True, text=True, env=env)
        secs = time.monotonic() - began
        log = ran.stdout + ran.stderr
        (folder / "tlc.log").write_text(log)
        got = result(log)
        distinct, generated = counts(log)[1], counts(log)[0]
        ok = got == expected
        mark = "" if ok else "   <-- not as expected"
        print(f"{name:32} {expected:16} {got:16} {distinct:>10} {generated:>11} {secs:6.0f}{mark}",
              flush=True)
        if not ok:
            failed.append(name)
        elif counterexample and args.counterexamples:
            path = MODEL_DIR / "counterexamples" / f"{counterexample}.trace.txt"
            path.write_text(trace(log))
    if failed:
        print(f"not as expected: {', '.join(failed)} (logs in {args.out})", file=sys.stderr)
        sys.exit(1)
    print("every case ended as expected")


if __name__ == "__main__":
    main()
