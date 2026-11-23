"""Turns a TLC error trace of the DriverTransport model into a schedule in its action names.

As tools/tla/schedule.py does for CoreLifecycle: TLC v1.7.4 prints each state of a trace in full,
headed by the name of the action that led to it, without the action's arguments. The arguments are
read here from what changed between one state and the next, so a schedule is made from the trace
by a program and never by hand.

    from schedule_driver import schedule_of
    schedule_of(log_text, constants)   # -> {"messages": [...], "steps": [...], "end": {...}}

formal/tla/DriverTransport.md names the actions. A step names what the action did:

- Route(proc, reads) and DropWait(proc, reads): the driver reads the next line from a harness
  process; `settles` lists the core's requests the events it made settled, and how; `writes` what
  the driver wrote to the harness in the same step;
- HarnessReads(proc, reads), HarnessAsks(ask, session, call, shown), HarnessResolves(ask),
  HarnessEndsTurn(session), HarnessEdits(session), HarnessDies(proc);
- Pick(chat), Prompt(chat), Spawn(chat), Restart;
- Answer(req, outcome, ask), Interrupt(req, outcome), NewSession(req, outcome, old): the core's
  requests, taken or refused, with what the driver wrote and what it settled.
"""

from schedule import states


def describe(msg):
    """A line on a pipe, as plain JSON."""
    return {k: v for k, v in sorted(msg.items())}


def appended(s, t, var):
    """What each process's pipe gained in this step."""
    out = []
    for p in t[var]:
        before, after = list(s[var][p]), list(t[var][p])
        # A pipe that lost its head and gained lines: compare from the right length.
        if after[: len(before)] == before:
            out += [describe(m) for m in after[len(before):]]
        elif after[: len(before) - 1] == before[1:]:
            out += [describe(m) for m in after[len(before) - 1:]]
    return out


def consumed(s, t, var):
    """The process whose pipe lost its head, and the line."""
    for p in s[var]:
        before, after = list(s[var][p]), list(t[var][p])
        if before and after == before[1:]:
            return p, describe(before[0])
    raise ValueError(f"no line was read from {var}")


def settled(s, t):
    return [{"req": r, "how": t["how"][r]}
            for r in sorted(t["nSettle"]) if t["nSettle"][r] != s["nSettle"][r]]


def with_effects(step, s, t):
    done = settled(s, t)
    if done:
        step["settles"] = done
    wrote = appended(s, t, "toH")
    if wrote:
        step["writes"] = wrote
    return step


def changed_key(s, t, var):
    keys = [k for k in t[var] if t[var][k] != s[var][k]]
    if len(keys) != 1:
        raise ValueError(f"{var}: expected one change, got {keys}")
    return keys[0]


def step(action, s, t, constants):
    if action == "Next":
        # TLC names the core's requests after Next, as they choose a request in its \E.
        r = changed_key(s, t, "rq")
        kinds = (("Answers", "Answer"), ("Interrupts", "Interrupt"), ("NewSessions", "NewSession"))
        (action,) = [name for key, name in kinds if r in constants.get(key, [])]
    if action in ("Route", "DropWait"):
        p, msg = consumed(s, t, "fromH")
        return with_effects({"action": action, "proc": p, "reads": msg}, s, t)
    if action == "HarnessReads":
        p, msg = consumed(s, t, "toH")
        made = {"action": action, "proc": p, "reads": msg}
        wrote = appended(s, t, "fromH")
        if wrote:
            made["writes"] = wrote
        return made
    if action == "HarnessDies":
        return {"action": action, "proc": changed_key(s, t, "alive")}
    if action == "Spawn":
        return {"action": action, "chat": changed_key(s, t, "alive")}
    if action == "Restart":
        return {"action": action}
    if action == "Pick":
        c = changed_key(s, t, "picking")
        return with_effects({"action": action, "chat": c}, s, t)
    if action == "Prompt":
        c = changed_key(s, t, "dTurn")
        return with_effects({"action": action, "chat": c}, s, t)
    if action in ("Answer", "Interrupt", "NewSession"):
        r = changed_key(s, t, "rq")
        made = {"action": action, "req": r, "outcome": t["rq"][r]}
        if action == "Answer" and t["rq"][r] == "taken":
            (a,) = [a for a in t["dAsk"] if t["dAsk"][a] != s["dAsk"][a]]
            made["ask"] = a
        if action == "NewSession" and t["rq"][r] == "taken":
            (effect,) = [e for e in t["effects"] if e not in list(s["effects"])]
            made["old_session"] = effect["old"]
        return with_effects(made, s, t)
    if action == "HarnessAsks":
        a = changed_key(s, t, "hAsk")
        ask = t["hAsk"][a]
        return {"action": action, "ask": a, "session": ask["s"], "call": ask["k"],
                "shown": ask["shown"]}
    if action == "HarnessResolves":
        return {"action": action, "ask": changed_key(s, t, "hAsk")}
    if action == "HarnessEndsTurn":
        (sess,) = [x for x in t["hTurn"] if t["hTurn"][x] != s["hTurn"][x]]
        return {"action": action, "session": sess, "writes": appended(s, t, "fromH")}
    if action == "HarnessEdits":
        (msg,) = appended(s, t, "fromH")
        return {"action": action, "session": msg["s"]}
    raise ValueError(f"no rule for the action {action}")


def schedule_of(log, constants):
    trace, end = states(log)
    if not trace:
        raise ValueError("no trace in the log")
    steps, first_step_of = [], {1: 0}
    for number, ((_, s), (action, t)) in enumerate(zip(trace, trace[1:]), start=2):
        first_step_of[number] = len(steps)
        steps.append(step(action, s, t, constants))
    if end["kind"] == "loop":
        end = {"kind": "loop", "from_step": first_step_of.get(end["to_state"] + 1, len(steps))}
    messages = [{"id": r, "kind": kind}
                for kind, key in (("answer", "Answers"), ("interrupt", "Interrupts"),
                                  ("new_session", "NewSessions"))
                for r in sorted(constants.get(key, []))]
    return {"messages": messages, "steps": steps, "end": end}
