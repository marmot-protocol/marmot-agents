"""Turns a TLC error trace of a model into a schedule in the shared action names.

TLC v1.7.4 prints each state of a trace in full, headed by the name of the action that led to it,
without the action's arguments. The arguments, and the outcome of each send, are read
here from what changed between one state and the next, so a schedule is made from the trace by a
program and never by hand. The schedule is plain JSON: the messages people post, the steps, and how
the trace ends (an invariant broken at its last step, a stutter for ever, or a loop back to a step).

    from schedule import schedule_of
    schedule_of(log_text, constants)   # -> {"messages": [...], "steps": [...], "end": {...}}
    named_steps_of(log_text)           # SocketProtocol's: -> {"steps": [...], "end": {...}}

The rest of this page is about CoreLifecycle. SocketProtocol names each step in a variable of its
own, so its schedule is read from that variable (`named_steps_of`).

docs/design/verification.md names the actions. A step names what the model's action did:

- Store(msg), Hear(msg) and Ask(msg) (`reads` names the message the chat's reader then handled,
  the oldest stored after the mark, or is null when there was none), CatchUp(chat, msg),
  CatchUpDone;
- Deliver(msg, kind, session), Take(msg), Refuse(msg), Emit(item, settles, fin), Tick(req);
- Send(chat, send, outcome), where `send` names the item and version, or the notice and what it
  is about, `last_resort` when it stands in for a dropped send, and `try` the number of its tries
  MDK dropped before (each such try is under a new token); and the outcome is sent, busy,
  ambiguous (it did not go out), ambiguous_landed (it went out), refused, or rejected (MDK took it
  and dropped it before it went out; `gives_up` when the core gives it up then);
- Save, SaveFails, Crash, Restart, DriverStops, Stop.
"""

import re

CHAT_ONE = {"p1", "p2", "r1", "t1", "s1"}


def chat_of(x):
    return "c1" if x in CHAT_ONE else "c2"


# ---------------------------------------------------------------------------------------------
# TLC's values

TOKEN = re.compile(r'\s*(<<|>>|\|->|:>|@@|"(?:[^"\\]|\\.)*"|-?\d+|[A-Za-z_][A-Za-z0-9_]*|[\[\]{}(),])')


def tokens(text):
    at, out = 0, []
    text = text.rstrip()
    while at < len(text):
        found = TOKEN.match(text, at)
        if not found:
            raise ValueError(f"cannot read TLC's value at: {text[at:at + 40]!r}")
        out.append(found.group(1))
        at = found.end()
    return out


class Reader:
    def __init__(self, text):
        self.toks = tokens(text)
        self.at = 0

    def peek(self):
        return self.toks[self.at] if self.at < len(self.toks) else None

    def take(self, want=None):
        tok = self.toks[self.at]
        if want is not None and tok != want:
            raise ValueError(f"expected {want}, got {tok}")
        self.at += 1
        return tok

    def value(self):
        tok = self.peek()
        if tok == "<<":
            self.take()
            items = []
            while self.peek() != ">>":
                items.append(self.value())
                if self.peek() == ",":
                    self.take()
            self.take(">>")
            return tuple(items)
        if tok == "{":
            self.take()
            items = []
            while self.peek() != "}":
                items.append(self.value())
                if self.peek() == ",":
                    self.take()
            self.take("}")
            return frozenset(items)
        if tok == "[":
            self.take()
            fields = {}
            while self.peek() != "]":
                key = self.take()
                key = key[1:-1] if key.startswith('"') else key
                self.take("|->")
                fields[key] = self.value()
                if self.peek() == ",":
                    self.take()
            self.take("]")
            return fields
        if tok == "(":
            # A function written k1 :> v1 @@ k2 :> v2.
            self.take()
            pairs = {}
            while True:
                key = self.value()
                self.take(":>")
                pairs[key] = self.value()
                if self.peek() == "@@":
                    self.take()
                    continue
                break
            self.take(")")
            return pairs
        self.take()
        if tok.startswith('"'):
            return tok[1:-1]
        if tok in ("TRUE", "FALSE"):
            return tok == "TRUE"
        if re.fullmatch(r"-?\d+", tok):
            return int(tok)
        raise ValueError(f"unexpected {tok}")


def parse(text):
    reader = Reader(text)
    value = reader.value()
    if reader.peek() is not None:
        raise ValueError(f"left over: {reader.toks[reader.at:]}")
    return value


def as_map(value):
    """An empty function prints as << >>; anything else is a dict already."""
    return {} if value == () else value


# ---------------------------------------------------------------------------------------------
# TLC's trace

HEAD = re.compile(r"^State (\d+): (?:<(\w+)[ (].*>|<Initial predicate>|(Stuttering))$")
BACK = re.compile(r"^(?:State \d+: )?Back to state (\d+)")


def states(log):
    """The states of the trace, each (action, {variable: value}), and how it ends."""
    lines = log.splitlines()
    out, end = [], {"kind": "violation"}
    current, name, body = None, None, []

    def close():
        if current is not None:
            out.append((name, variables(body)))

    for line in lines:
        back = BACK.match(line)
        if back:
            close()
            current = None
            end = {"kind": "loop", "to_state": int(back.group(1))}
            break
        head = HEAD.match(line)
        if head:
            close()
            if head.group(3):
                current = None
                end = {"kind": "stutter"}
                break
            current, name, body = int(head.group(1)), head.group(2) or "Init", []
            continue
        if current is not None:
            if line.strip() == "" or line.startswith("Error:") or line.startswith("Finished"):
                close()
                current = None
                continue
            body.append(line)
    close()
    return out, end


def variables(body):
    found, name, text = {}, None, []
    for line in body:
        head = re.match(r"^/\\ (\w+) = (.*)$", line)
        if head:
            if name:
                found[name] = parse(" ".join(text))
            name, text = head.group(1), [head.group(2)]
        else:
            text.append(line.strip())
    if name:
        found[name] = parse(" ".join(text))
    return found


# ---------------------------------------------------------------------------------------------
# What each step did

ITEMS = {"t1", "s1", "t2"}


def about_of(tok):
    """What a token names: an item's version, or a notice and what it is about, in turn."""
    if tok[0] in ITEMS:
        return {"item": tok[0], "version": tok[1]}
    kind, about = tok
    return {"notice": kind, "about": about_of(about) if isinstance(about, tuple) else about}


def send_of(entry):
    """The send an outbox entry is, marked when it stands in for a dropped send (a last resort),
    and with the number of its tries MDK dropped, when there were any."""
    if entry["i"] != "notice":
        out = {"item": entry["i"], "version": entry["v"]}
    else:
        out = about_of(entry["tok"])
    if entry.get("lr"):
        out["last_resort"] = True
    if entry.get("rej"):
        out["try"] = entry["rej"]
    return out


def head_for(cv, chat):
    for entry in cv["outbox"]:
        if entry["c"] == chat:
            return entry
    return None


def toks(cv):
    return {entry["tok"] for entry in cv["outbox"]}


def mdk_token(entry):
    return (entry["tok"], entry["rej"])


def posted(s, t):
    return s["iposts"] != t["iposts"] or s["shown"] != t["shown"] or s["mdkTok"] != t["mdkTok"]


def send_outcome(s, t, entry):
    """What MDK answered, and, for a try it dropped, whether the core gave the send up."""
    tok = entry["tok"]
    if t["ambig"] > s["ambig"]:
        landed = (mdk_token(entry) in t["mdkTok"] and mdk_token(entry) not in s["mdkTok"]) \
            or posted(s, t)
        return "ambiguous_landed" if landed else "ambiguous", False
    after = [e for e in t["cv"]["outbox"] if e["tok"] == tok]
    if after and after[0]["rej"] > entry["rej"]:
        return "rejected", False
    if tok in toks(t["cv"]):
        return "busy", False
    if t["rejects"] > s["rejects"] or (t["mdkDropped"] == s["mdkDropped"]
                                       and mdk_token(entry) in s["mdkDropped"]):
        return "rejected", True
    # Gone from the outbox: refused (MDK took note of it, or the version was dropped, or nothing
    # was posted for it), or else sent (posted now, or, with TokenDedupe, posted before).
    if t["mdkRefused"] != s["mdkRefused"] or t["dropped"] != s["dropped"] \
            or mdk_token(entry) not in t["mdkTok"]:
        return "refused", False
    return "sent", False


def chats_with(cv):
    seen = []
    for entry in cv["outbox"]:
        if entry["c"] not in seen:
            seen.append(entry["c"])
    return seen


def changed_head(s, t):
    """The chat whose oldest send the step acted on."""
    picks = []
    for chat in chats_with(s["cv"]):
        entry = head_for(s["cv"], chat)
        after = [e for e in t["cv"]["outbox"] if e["tok"] == entry["tok"]]
        if after != [entry] or t["mdkTok"] != s["mdkTok"] or t["ambig"] != s["ambig"] \
                or t["mdkDropped"] != s["mdkDropped"]:
            picks.append((chat, entry))
    heads = [(chat, head_for(s["cv"], chat)) for chat in chats_with(s["cv"])]
    if not picks and len(heads) == 1:
        # Busy changes nothing: the one send there is was the one tried.
        return heads[0]
    if len(picks) != 1:
        raise ValueError(f"cannot tell which send the step acted on: {picks}")
    return picks[0]


def step(action, s, t, constants):
    if action in ("Save", "SaveFails", "Crash", "Restart", "DriverStops", "Stop", "CatchUpDone"):
        return [{"action": action}]
    if action == "Store":
        (msg,) = [m for m in t["order"] if t["order"][m] != s["order"][m]]
        return [{"action": "Store", "msg": msg}]
    if action in ("Hear", "Ask"):
        early = t["early"] - s["early"]
        if early:
            (msg,) = early
        else:
            live = list(s["live"])
            for m in t["live"]:
                live.remove(m)
            (msg,) = live
        read = [m for m in t["cv"]["h"] if t["cv"]["h"][m] != s["cv"]["h"][m]]
        return [{"action": action, "msg": msg, "stored": s["order"][msg] > 0,
                 "reads": read[0] if read else None}]
    if action == "CatchUp":
        (msg,) = [m for m in t["cv"]["h"] if t["cv"]["h"][m] != s["cv"]["h"][m]]
        return [{"action": "CatchUp", "chat": chat_of(msg), "msg": msg}]
    if action == "Deliver":
        act = t["inflight"]
        session = None if act["sess"] == "none" else act["sess"]
        return [{"action": "Deliver", "msg": act["x"], "kind": act["k"], "session": session}]
    if action in ("Take", "Refuse"):
        return [{"action": action, "msg": s["inflight"]["x"]}]
    if action == "Emit":
        (item,) = [i for i in t["drvEmits"] if t["drvEmits"][i] != s["drvEmits"][i]]
        settles = sorted(s["drvReqs"] - t["drvReqs"])
        return [{"action": "Emit", "item": item, "settles": settles,
                 "fin": t["drvItems"][item] == "done"}]
    if action == "Tick":
        (req,) = s["deadline"] - t["deadline"]
        return [{"action": "Tick", "req": req}]
    if action == "Send":
        chat, entry = changed_head(s, t)
        outcome, gives_up = send_outcome(s, t, entry)
        made = {"action": "Send", "chat": chat, "send": send_of(entry), "outcome": outcome}
        if gives_up:
            made["gives_up"] = True
        return [made]
    raise ValueError(f"no rule for the action {action}")


def schedule_of(log, constants):
    trace, end = states(log)
    if not trace:
        raise ValueError("no trace in the log")
    steps, first_step_of = [], {1: 0}
    for number, ((_, s), (action, t)) in enumerate(zip(trace, trace[1:]), start=2):
        first_step_of[number] = len(steps)
        steps.extend(step(action, s, t, constants))
    if end["kind"] == "loop":
        # From the step after that state on, the behaviour repeats for ever.
        end = {"kind": "loop", "from_step": first_step_of.get(end["to_state"] + 1, len(steps))}
    messages = [{"id": m, "chat": chat_of(m), "kind": "prompt"} for m in sorted(constants["Prompts"])]
    messages += [{"id": r, "chat": chat_of(r), "kind": "control",
                  "new_session": r in constants["NewSession"]}
                 for r in sorted(constants["Requests"])]
    return {"messages": messages, "steps": steps, "end": end}


def plain(value):
    """A TLC value as JSON takes it: a set as a sorted list."""
    if isinstance(value, frozenset):
        return sorted(plain(v) for v in value)
    if isinstance(value, dict):
        return {k: plain(v) for k, v in value.items()}
    if isinstance(value, tuple):
        return [plain(v) for v in value]
    return value


def named_steps_of(log):
    """The schedule of a model whose every state names the step that led to it in its variable
    `step`, as SocketProtocol's do: the steps in order, and how the trace ends."""
    trace, end = states(log)
    if not trace:
        raise ValueError("no trace in the log")
    steps = []
    for _, state in trace[1:]:
        named = plain(state["step"])
        steps.append({"action": named.pop("a"), **named})
    if end["kind"] == "loop":
        # State n is reached by the step at index n - 2; from the step after state `to_state` on,
        # the behaviour repeats for ever.
        end = {"kind": "loop", "from_step": end["to_state"] - 1}
    return {"steps": steps, "end": end}


def constants_of(cfg_text):
    found = {}
    for line in cfg_text.splitlines():
        line = line.split("\\*")[0].rstrip()
        got = re.match(r"^\s+(\w+)\s*=\s*(.+)$", line)
        if got:
            value = parse(got.group(2))
            found[got.group(1)] = sorted(value) if isinstance(value, frozenset) else value
    return found
