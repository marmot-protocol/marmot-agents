/**
 * The translation half of the Claude Code driver: what Claude Code says through the Claude Agent SDK,
 * turned into Marmot agent events, as the core reads them over the socket (D-034).
 *
 * It is a port of the `Claude` translator in `tools/prototype/translate.py`, and is tested by replaying
 * the recordings in `fixtures/recordings/claude` against the events that prototype writes for them
 * (`fixtures/recordings/claude/expected`), as D-074 says. Where the two disagree, the recording and the
 * event model (`docs/design/event-model.md`) decide which is wrong. It does no I/O: the transport runs
 * the SDK, hands each message over as a {@link Line}, and sends the events it gets back.
 */

/** What an agent item is. */
export type ItemType =
  | "turn"
  | "message"
  | "reasoning"
  | "tool"
  | "approval"
  | "question"
  | "plan"
  | "task"
  | "notice"
  | "error"
  | "session";

/** Where an item stands. */
export type State = "running" | "waiting" | "done" | "failed" | "declined" | "cancelled";

/**
 * One message, as the recordings hold it (`fixtures/recordings/README.md`): `dir` is `out` for what the
 * client sent to Claude Code and `in` for what came back, and `via` says how it travelled: `message`,
 * `callback`, `prompt`, `call` or `note`.
 */
export interface Line {
  dir: "in" | "out";
  via?: string;
  msg: unknown;
}

/** The harness's own message: its kind, and the message itself as it was handed over (D-059). */
export interface Native {
  kind: string;
  payload: unknown;
}

/**
 * An agent event, in the shape `marmot-agent-events` reads: a field with no value is left out, and so
 * is a flag that is false or a list of changes or steps that is empty. `body` holds the item type's
 * own fields (`crates/marmot-agent-events/src/body.rs`).
 */
export interface AgentEvent {
  harness: string;
  session?: string;
  turn?: string;
  item: string;
  seq: number;
  state: State;
  title: string;
  text: string;
  type: ItemType;
  body: Record<string, unknown>;
  native?: Native;
}

type Json = Record<string, unknown>;

/** An event as the translator keeps it, before the fields with no value are left out. */
interface Kept {
  session: string | null;
  turn: string | null;
  item: string;
  seq: number;
  type: ItemType;
  state: State;
  title: string;
  text: string;
  body: Json;
  native: Native | null;
}

// The prototype is Python, and these keep its reading of a message: a missing field and null are both
// nothing, and an empty string, list or object counts as false.

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function object(value: unknown): Json {
  return isObject(value) ? value : {};
}

function list(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function truthy(value: unknown): boolean {
  if (Array.isArray(value)) return value.length > 0;
  if (isObject(value)) return Object.keys(value).length > 0;
  return Boolean(value);
}

/** The first value that counts as true, or else the last. */
function or(...values: unknown[]): unknown {
  for (const value of values.slice(0, -1)) {
    if (truthy(value)) return value;
  }
  return values.at(-1);
}

/** A value inside a made-up id or a word, as the prototype writes it. */
function word(value: unknown): string {
  return value === null || value === undefined ? "None" : String(value);
}

/** Text as an event carries it. Nothing is cut: the core applies the size limit (D-057). */
function plain(value: unknown): string {
  return value === null || value === undefined ? "" : String(value);
}

function idOrNull(value: unknown): string | null {
  return value === null || value === undefined ? null : String(value);
}

function own(record: Json, key: unknown): unknown {
  return typeof key === "string" && Object.hasOwn(record, key) ? record[key] : undefined;
}

/** The same JSON text for the same value, whatever order its keys were set in. */
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (isObject(value)) {
    const keys = Object.keys(value).sort();
    return `{${keys.map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`).join(",")}}`;
  }
  return JSON.stringify(value ?? null);
}

const LEFT_OUT_WHEN_FALSE = new Set(["secret", "approve"]);
const LEFT_OUT_WHEN_EMPTY = new Set(["changes", "entries"]);

/** An event as `marmot-agent-events` writes it. The harness's own JSON is kept as it is. */
function crateShape(value: unknown, key?: string): unknown {
  if (Array.isArray(value)) return value.map((v) => crateShape(v, key));
  if (!isObject(value)) return value;
  const out: Json = {};
  for (const [k, v] of Object.entries(value)) {
    if ((key === "native" && k === "payload") || (key === "body" && k === "input")) {
      out[k] = v;
      continue;
    }
    const empty = Array.isArray(v) && v.length === 0;
    if (v === null || v === undefined) continue;
    if (LEFT_OUT_WHEN_FALSE.has(k) && v === false) continue;
    if (LEFT_OUT_WHEN_EMPTY.has(k) && empty) continue;
    if (key === "answer" && k === "options" && empty) continue;
    out[k] = crateShape(v, k);
  }
  return out;
}

/** A slash command: always a name and a hint, the hint empty when it takes nothing (D-045). */
function command(name: unknown, description: unknown = null, hint: unknown = null): Json {
  return { name: name ?? null, description: description ?? null, hint: truthy(hint) ? hint : "" };
}

/**
 * The answer a question item records, from Claude Code's `answers`, which maps each question's text to
 * the label chosen, or to several labels joined by ", " when several may be chosen (D-059). A label
 * becomes the id of the option the item offered; anything that matches no label is text.
 */
function answered(questions: unknown, answers: Json): Json[] {
  const out: Json[] = [];
  for (const asked of list(questions)) {
    const q = object(asked);
    const given = own(answers, q["text"]);
    if (given === null || given === undefined) continue;
    const ids = new Map<unknown, unknown>(list(q["options"]).map((o) => [object(o)["label"], object(o)["id"]]));
    let parts: string[];
    if (Array.isArray(given)) parts = given.map(word);
    else if (ids.has(word(given)) || !truthy(q["multiple"])) parts = [word(given)];
    else parts = word(given).split(", ");
    const rest = parts.filter((x) => !ids.has(x));
    out.push({
      question: q["id"],
      options: parts.filter((x) => ids.has(x)).map((x) => ids.get(x)),
      text: rest.length > 0 ? rest.join(", ") : null,
    });
  }
  return out;
}

/**
 * One file an edit changed, as a unified diff built from Claude Code's `structuredPatch`. A new file
 * comes with no hunks and its whole content, which becomes one hunk of added lines.
 */
function fileChange(result: Json, path: unknown): Json {
  let hunks = list(or(result["structuredPatch"], []));
  if (hunks.length === 0 && result["type"] === "create" && truthy(result["content"])) {
    const lines = word(result["content"]).split("\n");
    if (lines.at(-1) === "") lines.pop();
    hunks = [{ oldStart: 0, oldLines: 0, newStart: 1, newLines: lines.length, lines: lines.map((x) => `+${x}`) }];
  }
  const diff = hunks
    .map((hunk) => {
      const h = object(hunk);
      const head = `@@ -${word(h["oldStart"])},${word(h["oldLines"])} +${word(h["newStart"])},${word(h["newLines"])} @@\n`;
      return head + list(h["lines"]).map((x) => `${word(x)}\n`).join("");
    })
    .join("");
  const change = result["type"] === "create" ? "add" : result["type"] === "update" ? "update" : null;
  return { path: or(result["filePath"], path) ?? null, diff, change };
}

const TOOL_KINDS = new Map<unknown, string>([
  ["Bash", "command"],
  ["Edit", "edit"],
  ["Write", "edit"],
  ["NotebookEdit", "edit"],
  ["Read", "read"],
  ["Grep", "search"],
  ["Glob", "search"],
  ["ToolSearch", "search"],
  ["WebSearch", "search"],
  ["WebFetch", "fetch"],
  ["Agent", "subagent"],
  ["Task", "subagent"],
]);

/** Tool calls that are really a question or a plan. Each becomes one item, when its request arrives. */
const NOT_TOOLS = new Set<unknown>(["AskUserQuestion", "ExitPlanMode"]);

const NOT_SENT_SYSTEM = new Set<unknown>([
  "thinking_tokens",
  "background_tasks_changed",
  "session_title_changed",
  "vcs_state_changed",
  "code_change_published",
  "commands_changed",
  "hook_started",
  "hook_response",
]);

const VERBS = new Map<unknown, [string, string]>([
  ["command", ["Running a command", "Ran a command"]],
  ["edit", ["Editing a file", "Edited a file"]],
  ["read", ["Reading a file", "Read a file"]],
  ["search", ["Searching", "Searched"]],
  ["fetch", ["Fetching", "Fetched"]],
  ["mcp", ["Using a tool", "Used a tool"]],
  ["subagent", ["Starting a subagent", "Started a subagent"]],
  ["other", ["Using a tool", "Used a tool"]],
]);

function toolTitle(kind: unknown, state: State): string {
  const [doing, done] = VERBS.get(kind) ?? ["Using a tool", "Used a tool"];
  return state === "running" ? doing : state === "done" ? done : `${doing}: ${state}`;
}

const TASK_ENDS = new Map<unknown, State>([
  ["completed", "done"],
  ["failed", "failed"],
  ["stopped", "cancelled"],
  ["killed", "cancelled"],
]);

const TASK_TITLES: Record<State, string> = {
  running: "Background task",
  waiting: "Background task",
  done: "Background task finished",
  failed: "Background task failed",
  declined: "Background task stopped",
  cancelled: "Background task stopped",
};

/**
 * Translates one chat's Claude Code session. Feed it every message in order; each call returns the
 * events that message made. A message it has no rule for makes no event and is counted in
 * {@link Translator.unknown}; a message left out on purpose is counted in {@link Translator.notSent}.
 *
 * When Claude Code stops, the SDK's `query()` throws, and that makes no event, as in the prototype.
 * The turn's result has usually closed what was open in it, and a background task stays open until
 * Claude Code reports it when the session is resumed (`restart_mid_turn.jsonl`). When it stops in the
 * middle of a turn, the transport calls {@link Translator.stopped}.
 */
export class Translator {
  /** Messages with no rule, by kind. */
  readonly unknown = new Map<string, number>();
  /** Messages left out on purpose, by kind. */
  readonly notSent = new Map<string, number>();

  private made: Kept[] = [];
  private session: string | null = null;
  private turn: string | null = null;
  private readonly seqIn = new Map<string | null, number>();
  private readonly open = new Map<string, Kept>();
  private readonly sessions = new Map<string, string>();
  private prompted = false;
  private turnN = 0;
  private readonly tools = new Map<string, Json>();
  /** Tool name -> open approval item, for answers with no request id. */
  private readonly approvals = new Map<unknown, string>();
  /** The callback's request id -> open approval item. */
  private readonly approvalIds = new Map<unknown, string>();
  private settings: Json = {};
  private commands: Json[] = [];
  private models: unknown[] | null = null;
  private version: unknown = null;
  private sessionKey: string | null = null;

  /** The turn that is running, as the events name it, or null between turns. */
  get currentTurn(): string | null {
    return this.turn;
  }

  /** The session the events name, once Claude Code has named it. */
  get currentSession(): string | null {
    return this.sessionKey ?? this.session;
  }

  feed(line: Line): AgentEvent[] {
    this.made = [];
    this.handle(line);
    return this.shaped();
  }

  /**
   * The transport ended the session itself, by closing its `query()`, as a "new session" control
   * does. Claude Code says nothing when that happens, so the driver closes what was open: the items
   * of a running turn and the turn as cancelled, then the session item as ended. A `query()` that
   * starts later without `resume` then has no session left to end. Replays never call this.
   */
  closeSession(): AgentEvent[] {
    this.made = [];
    this.cancelTurn();
    if (this.sessionKey) this.endSession(this.sessionKey, "the session was closed by the driver");
    this.session = this.sessionKey = this.version = null;
    this.settings = {};
    this.commands = [];
    return this.shaped();
  }

  /**
   * Claude Code stopped without ending its turn: the SDK's `query()` threw, or ended, with no result.
   * Nothing more will come for that turn, so the driver closes the turn's open items and the turn as
   * cancelled. The session stays, to be resumed by the next `query()`. Replays never call this.
   */
  stopped(): AgentEvent[] {
    this.made = [];
    this.cancelTurn();
    this.prompted = false;
    return this.shaped();
  }

  /** Closes a running turn, and what is open in it, as cancelled. */
  private cancelTurn(): void {
    if (this.turn === null) return;
    const turn = this.open.get(`turn:${this.turn}`);
    this.closeOpenItems();
    if (turn !== undefined) {
      this.emit(turn.item, "turn", "cancelled", "Interrupted", "", turn.body, "(none: closed by the driver)", null, [
        turn.session,
        turn.turn,
      ]);
    }
    this.turn = null;
  }

  private shaped(): AgentEvent[] {
    const events = this.made.map(
      (e) =>
        crateShape({
          harness: "claude",
          session: e.session,
          turn: e.turn,
          item: e.item,
          seq: e.seq,
          state: e.state,
          title: e.title,
          text: e.text,
          type: e.type,
          body: e.body,
          native: e.native,
        }) as AgentEvent,
    );
    this.made = [];
    return events;
  }

  private handle(line: Line): void {
    const m = object(line.msg);
    const via = line.via;
    const out = line.dir === "out";
    if (via === "note") return this.skip("recorder note");
    if (via === "prompt") {
      this.prompted = true;
      return this.skip("client: user message" + (truthy(m["priority"]) ? " (steer)" : ""));
    }
    if (via === "call") {
      if ("error" in m) return this.skip("client library: query() threw at the end of a failed turn");
      const method = m["method"];
      if (out) {
        if (method === "query" && !("resume" in object(m["options"]))) {
          // Claude Code sends nothing to say the earlier session is over.
          if (this.sessionKey) this.endSession(this.sessionKey, "a new session was started");
          this.session = this.sessionKey = this.version = null;
          this.settings = {};
          this.commands = [];
        }
      } else if (
        (method === "supportedCommands" || method === "supportedModels") &&
        "response" in m
      ) {
        // The client asked, which it can do before any prompt is sent.
        const response = m["response"];
        const r = object(
          method === "supportedCommands"
            ? { commands: response }
            : method === "supportedModels"
              ? { models: response }
              : response,
        );
        if (Array.isArray(r["commands"])) {
          this.commands = r["commands"].map((c) =>
            command(object(c)["name"], object(c)["description"], object(c)["argumentHint"]),
          );
        }
        if (Array.isArray(r["models"])) this.models = r["models"].map((x) => object(x)["value"] ?? null);
        if (truthy(r["current_permission_mode"])) this.settings["permission_mode"] = r["current_permission_mode"];
        return this.announce(`result of ${word(method)}()`, m);
      }
      return this.skip(out ? `client: ${word(method)}()` : `result of ${word(method)}()`);
    }
    if (via === "callback") return out ? this.permissionAnswer(m) : this.permissionRequest(m);
    const t = m["type"];
    const st = m["subtype"];
    if (t === "system") return this.system(st, m);
    if (t === "assistant") return this.assistant(m);
    if (t === "user") return this.user(m);
    if (t === "result") return this.result(st, m);
    if (t === "stream_event" || t === "rate_limit_event" || t === "tool_progress" || t === "command_lifecycle") {
      return this.skip(t === "stream_event" ? "partial text (stream_event)" : t);
    }
    this.noRule(truthy(st) ? `${word(t)}/${word(st)}` : word(t));
  }

  // -- events

  private emit(
    item: string,
    type: ItemType,
    state: State,
    title: string,
    text: unknown,
    body: Json,
    nativeKind: string,
    native: unknown,
    owner: [string | null, string | null] = [this.session, this.turn],
  ): void {
    const [session, turn] = owner;
    const seq = (this.seqIn.get(session) ?? 0) + 1;
    this.seqIn.set(session, seq);
    const event: Kept = {
      session,
      turn,
      item,
      seq,
      type,
      state,
      title,
      text: plain(text),
      body,
      // An event the driver made up carries no native message (D-059).
      native: nativeKind.startsWith("(none") ? null : { kind: nativeKind, payload: native },
    };
    this.made.push(event);
    if (state === "running" || state === "waiting") this.open.set(item, event);
    else this.open.delete(item);
  }

  /** A turn ended. Anything still running in it will never report again, so close it. */
  private closeOpenItems(): void {
    for (const [item, e] of [...this.open]) {
      if (e.type === "turn" || e.type === "task" || e.type === "session") continue;
      this.emit(item, e.type, "cancelled", e.title, e.text, e.body, "(none: closed by the driver)", null, [
        e.session,
        e.turn,
      ]);
    }
  }

  /**
   * Send the session item, or an edit of it. A session item carries the session's id and Claude Code's
   * version (D-030), and Claude Code states both only in its init message, at the start of the first
   * turn. What the client learns before then is kept, and goes out with that message. An edit is sent
   * only when something in it changed.
   */
  private announce(nativeKind: string, native: Json): void {
    if (this.session === null || this.version === null) {
      return this.skip(`${nativeKind}, before Claude Code has named the session`);
    }
    if (this.sessionKey === null) this.sessionKey = this.session;
    const settings: Json[] = [
      { id: "model", label: "Model", value: this.settings["model"] ?? null, options: this.models, approve: false },
      {
        id: "permission_mode",
        label: "Permission mode",
        value: this.settings["permission_mode"] ?? null,
        options: null,
        approve: true,
      },
    ];
    const item = `session:${this.sessionKey}`;
    const body: Json = { harness_version: this.version, settings, actions: [], commands: [...this.commands] };
    const key = canonical(body);
    if (this.sessions.get(item) === key) return this.skip(`${nativeKind}, nothing new for the session item`);
    this.sessions.set(item, key);
    const text = settings
      .filter((x) => x["value"] !== null && x["value"] !== undefined)
      .map((x) => `${word(x["label"])}: ${word(x["value"])}`)
      .join(", ");
    // A session belongs to no turn.
    this.emit(item, "session", "running", "Session", text, body, nativeKind, native, [this.session, null]);
  }

  /** Nothing Claude Code sends says a session is over. The driver ends the item when a new one starts. */
  private endSession(sid: string, _reason: string): void {
    const item = `session:${sid}`;
    const prev = this.open.get(item);
    if (prev === undefined) return;
    this.emit(item, "session", "done", "Session ended", prev.text, prev.body, "(none: closed by the driver)", null, [
      this.session,
      null,
    ]);
  }

  // -- turns

  /**
   * Claude Code repeats this at the start of every turn. It is the only place the model, the
   * permission mode and the names of the slash commands are stated.
   */
  private init(m: Json): void {
    this.session = idOrNull(m["session_id"]);
    this.version = or(m["claude_code_version"], this.version) ?? null;
    this.settings["model"] = m["model"] ?? null;
    this.settings["permission_mode"] = m["permissionMode"] ?? null;
    // The init message has names only; keep fuller entries the client asked for.
    if (this.commands.length === 0) this.commands = list(m["slash_commands"]).map((n) => command(n));
    this.announce("system / init", m);
  }

  private startTurn(m: Json, nativeKind = "system / init"): void {
    this.turnN += 1;
    this.session = idOrNull(m["session_id"]);
    // Named after the message that opens it, so a turn of a resumed session never takes the id of
    // a turn from before the driver restarted.
    this.turn = `${word(this.session)}:${truthy(m["uuid"]) ? word(m["uuid"]) : this.turnN}`;
    const by = this.prompted ? "user" : "agent";
    this.prompted = false;
    this.emit(`turn:${this.turn}`, "turn", "running", "Working", "", { started_by: by }, nativeKind, m);
  }

  private result(st: unknown, m: Json): void {
    const ok = st === "success" && !truthy(m["is_error"]);
    const interrupted = st === "error_during_execution";
    this.closeOpenItems();
    if (this.turn === null) {
      // A failure before any turn began.
      this.turnN += 1;
      this.session = idOrNull(m["session_id"]);
      this.turn = `${word(this.session)}:${truthy(m["uuid"]) ? word(m["uuid"]) : this.turnN}`;
    }
    const state: State = ok ? "done" : interrupted ? "cancelled" : "failed";
    const title = ok ? "Finished" : interrupted ? "Interrupted" : "Failed";
    const usage = object(m["usage"]);
    const started = this.open.get(`turn:${this.turn}`)?.body["started_by"] ?? "user";
    const body: Json = {
      started_by: started,
      duration_ms: m["duration_ms"] ?? null,
      usage: { input_tokens: usage["input_tokens"] ?? null, output_tokens: usage["output_tokens"] ?? null },
    };
    const errors = list(m["errors"]).map(word).join("; ");
    const text = ok ? "" : errors || word(or(m["result"], ""));
    this.emit(`turn:${this.turn}`, "turn", state, title, text, body, `result / ${word(st)}`, m);
    this.turn = null;
  }

  /** Messages that arrive after a result, from background work, start a turn the driver makes up. */
  private ensureTurn(m: Json): void {
    if (this.turn === null) {
      this.startTurn({ session_id: m["session_id"], uuid: m["uuid"] }, "(none: a message arrived outside a turn)");
    }
  }

  // -- content

  private assistant(m: Json): void {
    this.ensureTurn(m);
    const parent = m["parent_tool_use_id"];
    const content = list(object(m["message"])["content"]);
    for (const [n, block] of content.entries()) {
      const b = object(block);
      const bt = b["type"];
      if (bt === "text") {
        // A subagent's message names the item it belongs to.
        const body: Json = truthy(parent) ? { parent } : {};
        this.emit(`${word(m["uuid"])}:${n}`, "message", "done", "Message", b["text"], body, "assistant [text]", m);
      } else if (bt === "thinking") {
        if (plain(or(b["thinking"], "")).trim() === "") {
          this.skip("thinking block with no text");
          continue;
        }
        this.emit(`${word(m["uuid"])}:${n}`, "reasoning", "done", "Thinking", b["thinking"], {}, "assistant [thinking]", m);
      } else if (bt === "tool_use") {
        const name = b["name"];
        const id = word(b["id"]);
        const inp = object(b["input"]);
        if (NOT_TOOLS.has(name)) {
          // A question or a plan, dressed as a tool call. It becomes one item, when the request
          // arrives through the permission callback, not a tool call as well.
          this.tools.set(id, { kind: "internal", name });
          this.skip(`tool call that is really a question or a plan (${word(name)})`);
          continue;
        }
        const kind = word(name).startsWith("mcp__") ? "mcp" : (TOOL_KINDS.get(name) ?? "other");
        const body: Json = { kind, name: name ?? null };
        if (kind === "command") body["command"] = inp["command"] ?? null;
        else if (kind === "edit" || kind === "read") body["path"] = inp["file_path"] ?? null;
        else if (kind === "search") body["query"] = or(inp["query"], inp["pattern"] ?? null);
        else body["input"] = inp;
        const text = or(body["command"], body["path"], body["query"], inp["description"], name);
        this.tools.set(id, body);
        this.emit(id, "tool", "running", toolTitle(kind, "running"), text, body, `assistant [tool_use: ${word(name)}]`, m);
      } else {
        this.noRule(`assistant block [${word(bt)}]`);
      }
    }
  }

  private user(m: Json): void {
    const content = object(m["message"])["content"];
    if (typeof content === "string" && content.startsWith("<local-command-stdout>")) {
      this.ensureTurn(m);
      const text = content.replaceAll("<local-command-stdout>", "").replaceAll("</local-command-stdout>", "").trim();
      return this.emit(
        `command-output:${word(m["uuid"])}`,
        "notice",
        "done",
        "Command output",
        text,
        { kind: "command_output" },
        "user [local command output]",
        m,
      );
    }
    if (typeof content === "string" && content.startsWith("This session is being continued from a previous conversation")) {
      return this.skip("the compaction summary, which is for the model");
    }
    if (!Array.isArray(content)) return this.skip("echo of the user's message");
    for (const block of content) {
      const b = object(block);
      if (b["type"] !== "tool_result") {
        this.skip("echo of the user's message");
        continue;
      }
      const tid = word(b["tool_use_id"]);
      const body: Json = { ...(this.tools.get(tid) ?? { kind: "other", name: "?" }) };
      if (body["kind"] === "internal") {
        this.skip(`result of a tool call that is really a question or a plan (${word(body["name"])})`);
        continue;
      }
      const c = b["content"];
      const out =
        typeof c === "string"
          ? c
          : list(c)
              .filter(isObject)
              .map((x) => plain(x["text"]))
              .join("\n");
      const prev = this.open.get(tid);
      const state: State = truthy(b["is_error"]) ? "failed" : "done";
      if (prev === undefined && this.tools.has(tid)) continue; // already closed, for example declined
      body["output"] = out;
      const result = m["tool_use_result"];
      if (isObject(result) && body["kind"] === "edit" && result["structuredPatch"] != null) {
        body["changes"] = [fileChange(result, body["path"])];
      }
      this.emit(tid, "tool", state, toolTitle(body["kind"], state), prev?.text ?? "", body, "user [tool_result]", m);
    }
  }

  // -- approvals and questions

  private permissionRequest(m: Json): void {
    const tool = m["toolName"];
    const inp = object(m["input"]);
    const opts = object(m["options"]);
    const subject = opts["toolUseID"] ?? null;
    if (tool === "AskUserQuestion") {
      const qs = list(inp["questions"]).map((asked, n) => {
        const q = object(asked);
        return {
          id: `q${n}`,
          header: q["header"] ?? null,
          text: q["question"] ?? null,
          multiple: truthy(q["multiSelect"]),
          allow_text: true,
          options: list(q["options"]).map((o, k) => ({
            id: `o${k}`,
            label: object(o)["label"] ?? null,
            description: object(o)["description"] ?? null,
          })),
        };
      });
      const item = `question:${word(opts["requestId"])}`;
      this.approvals.set(tool, item);
      this.approvalIds.set(opts["requestId"], item);
      return this.emit(
        item,
        "question",
        "waiting",
        "Question",
        qs[0]?.text ?? "",
        { answer_via: "request", subject, questions: qs },
        "canUseTool [AskUserQuestion]",
        m,
      );
    }
    if (tool === "ExitPlanMode" && truthy(inp["plan"])) {
      this.emit(`plan:${word(subject)}`, "plan", "done", "Plan", inp["plan"], { text: inp["plan"] }, "canUseTool [ExitPlanMode]", m);
    }
    const choices: Json[] = [{ id: "c0", label: "Allow once", effect: "allow" }];
    for (const [n, suggestion] of list(opts["suggestions"]).entries()) {
      const s = object(suggestion);
      const label =
        s["type"] === "addRules"
          ? "Allow, and don't ask again"
          : s["type"] === "setMode"
            ? `Allow, and switch to ${word(s["mode"])} mode`
            : "Allow, with a rule";
      choices.push({ id: `c${n + 1}`, label, effect: "allow" });
    }
    choices.push({ id: `c${choices.length}`, label: "Deny", effect: "deny" });
    const item = `approval:${word(opts["requestId"])}`;
    this.approvals.set(tool, item);
    this.approvalIds.set(opts["requestId"], item);
    const text = or(inp["command"], inp["file_path"], opts["description"], tool);
    const what = tool === "ExitPlanMode" ? "plan" : "action";
    this.emit(item, "approval", "waiting", `Allow this ${what}?`, text, { subject, choices }, `canUseTool [${word(tool)}]`, m);
  }

  private permissionAnswer(m: Json): void {
    // A reply names its request when the driver recorded it; recordings name only the tool.
    let item = truthy(m["requestId"]) ? this.approvalIds.get(m["requestId"]) : undefined;
    if (item === undefined) {
      item = this.approvals.get(m["toolName"]);
      this.approvals.delete(m["toolName"]);
    } else {
      this.approvalIds.delete(m["requestId"]);
      if (this.approvals.get(m["toolName"]) === item) this.approvals.delete(m["toolName"]);
    }
    const prev = item ? this.open.get(item) : undefined;
    if (item === undefined || prev === undefined) return this.skip("client: permission result");
    const res = object(m["result"]);
    if (prev.type === "question") {
      const answers = object(object(res["updatedInput"])["answers"]);
      return this.emit(
        item,
        "question",
        "done",
        prev.title,
        "Answered",
        { ...prev.body, answer: answered(prev.body["questions"], answers) },
        "permission result for AskUserQuestion",
        m,
      );
    }
    const allow = res["behavior"] === "allow";
    const choices = list(prev.body["choices"]);
    // The choice the person made, when the driver recorded it, so a standing approval is not shown
    // as a one-off; otherwise allow once, or deny.
    const chosen = truthy(m["choice"]) ? m["choice"] : allow ? "c0" : object(choices.at(-1))["id"];
    const picked = choices.map(object).find((c) => c["id"] === chosen);
    const label = picked ? word(picked["label"]) : allow ? "Allow once" : "Deny";
    this.emit(
      item,
      "approval",
      allow ? "done" : "declined",
      prev.title,
      "Answered: " + label,
      { ...prev.body, answer: { choice: chosen } },
      "permission result",
      m,
    );
    const subject = prev.body["subject"];
    const tool = typeof subject === "string" ? this.open.get(subject) : undefined;
    if (!allow && tool !== undefined) {
      this.emit(
        tool.item,
        "tool",
        "declined",
        toolTitle(tool.body["kind"], "declined"),
        tool.text,
        tool.body,
        "(none: closed by the driver)",
        null,
      );
    }
  }

  // -- system messages

  private system(st: unknown, m: Json): void {
    if (st === "init") {
      this.init(m);
      if (this.turn === null) this.startTurn(m);
      return;
    }
    if (NOT_SENT_SYSTEM.has(st)) return this.skip(`system / ${word(st)}`);
    if (st === "status") {
      if (m["status"] === "compacting") {
        this.ensureTurn(m);
        return this.emit(
          `compaction:${word(m["uuid"])}`,
          "notice",
          "running",
          "Compacting the conversation",
          "",
          { kind: "compaction" },
          "system / status",
          m,
        );
      }
      if (truthy(m["permissionMode"]) && truthy(this.session)) {
        this.settings["permission_mode"] = m["permissionMode"];
        return this.announce("system / status", m);
      }
      return this.skip("system / status");
    }
    if (st === "compact_boundary") {
      this.ensureTurn(m);
      for (const [item, e] of [...this.open]) {
        if (e.type === "notice" && e.body["kind"] === "compaction") {
          return this.emit(item, "notice", "done", "Conversation compacted", "", e.body, "system / compact_boundary", m);
        }
      }
      return this.emit(
        `compaction:${word(m["uuid"])}`,
        "notice",
        "done",
        "Conversation compacted",
        "",
        { kind: "compaction" },
        "system / compact_boundary",
        m,
      );
    }
    if (st === "api_retry") {
      this.ensureTurn(m);
      return this.emit(`retry:${word(m["uuid"])}`, "notice", "done", "Retrying", "", { kind: "retry" }, "system / api_retry", m);
    }
    if (st === "task_started" || st === "task_progress" || st === "task_updated" || st === "task_notification") {
      this.ensureTurn(m);
      const item = `task:${word(m["task_id"])}`;
      const body: Json = { ...this.open.get(item)?.body };
      const update: Json = {
        kind: truthy(m["subagent_type"]) ? "subagent" : or(body["kind"], m["task_type"], "background"),
        subject: or(m["tool_use_id"], body["subject"]),
        description: or(m["description"], body["description"]),
      };
      for (const [k, v] of Object.entries(update)) {
        if (v !== null && v !== undefined) body[k] = v;
      }
      const state: State = st === "task_notification" ? (TASK_ENDS.get(m["status"]) ?? "done") : "running";
      const text = or(m["summary"], body["description"], "");
      return this.emit(item, "task", state, TASK_TITLES[state], text, body, `system / ${st}`, m);
    }
    this.noRule(`system / ${word(st)}`);
  }

  // -- counts

  private skip(kind: string): void {
    this.notSent.set(kind, (this.notSent.get(kind) ?? 0) + 1);
  }

  private noRule(kind: string): void {
    this.unknown.set(kind, (this.unknown.get(kind) ?? 0) + 1);
  }
}
