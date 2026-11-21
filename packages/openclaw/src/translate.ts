/**
 * The translation half of the OpenClaw plugin: what OpenClaw hands a channel plugin, turned into Marmot
 * agent events, as the core reads them over the socket (D-034).
 *
 * It is a port of the `OpenClaw` translator in `tools/prototype/translate.py`, and is tested by replaying
 * the recordings in `fixtures/recordings/openclaw` against the events that prototype writes for them
 * (`fixtures/recordings/openclaw/expected`), as D-074 says. Where the two disagree, the recording and
 * the event model (`docs/design/event-model.md`) decide which is wrong. It does no I/O: the plugin writes
 * down each call in both directions as a {@link Line}, exactly as the recorder in `tools/record/openclaw`
 * does, and sends the events it gets back.
 *
 * OpenClaw hands a plugin structured callbacks about tools, and two formatted copies of the same
 * activity. Tool items are built from the structured callbacks. The copies are not sent, except where a
 * copy holds something the callbacks lack: a question, and the result of a tool that is not a command.
 * Where a recording does not say which run a call belongs to, the rules go by order, as the prototype's
 * comments say.
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
 * One call, as the recordings hold it (`tools/record/openclaw/plugin/index.js`): `dir` is `in` for what
 * OpenClaw handed the plugin and `out` for what the plugin handed OpenClaw, and `via` says how:
 * `inbound`, `hook`, `result`, `call`, `plugin_hook`, `press` or `note`.
 */
export interface Line {
  dir: "in" | "out";
  via?: string;
  msg: unknown;
  /**
   * The message id of the run of `inbound.run` whose callback this is, when the plugin knows it. The
   * recordings do not hold it: a replay takes a reply as the answer to the oldest command waiting.
   */
  run?: string;
}

/** The harness's own message: its kind, and the message itself as it was handed over (D-059). */
export interface Native {
  kind: string;
  payload: unknown;
}

/**
 * An agent event, in the shape `marmot-agent-events` reads: a field with no value is left out, and so
 * is a flag that is false. `body` holds the item type's own fields
 * (`crates/marmot-agent-events/src/body.rs`).
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

/** Python's `str.split()`: on runs of white space, with none at either end. */
function words(text: string): string[] {
  const t = text.trim();
  return t === "" ? [] : t.split(/\s+/);
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

const COMMAND_TITLES: Partial<Record<State, string>> = {
  running: "Running a command",
  done: "Ran a command",
  failed: "Command failed",
  declined: "Command not allowed",
};

function toolTitle(kind: unknown, state: State): string {
  if (kind === "command") return COMMAND_TITLES[state] ?? `Running a command: ${state}`;
  const [doing, done] = VERBS.get(kind) ?? ["Using a tool", "Used a tool"];
  return state === "running" ? doing : state === "done" ? done : `${doing}: ${state}`;
}

const NOT_SENT = new Map<string, string>([
  ["dispatcher.onReplyStart", "typing signal"],
  ["onTypingController", "typing signal"],
  ["inbound.log", "receive-path log"],
  ["dispatcher.onDeliverySettled", "delivery bookkeeping"],
  ["dispatcher.onIdle", "delivery bookkeeping"],
  ["dispatcher.onSettled", "delivery bookkeeping"],
  ["dispatcher.onCleanup", "delivery bookkeeping"],
  ["onObservedReplyDelivery", "delivery bookkeeping"],
  ["onBlockReplyQueued", "delivery bookkeeping"],
  ["messaging.normalizeTarget", "address tidying"],
  ["messaging.targetResolver.looksLikeId", "address tidying"],
  ["messaging.targetResolver.resolveTarget", "address tidying"],
  ["messaging.inferTargetChatType", "address tidying"],
  ["groups.resolveRequireMention", "question to the plugin about the channel"],
  ["actions.describeMessageTool", "question to the plugin about the channel"],
  ["actions.supportsAction", "question to the plugin about the channel"],
  ["agentPrompt.messageToolHints", "question to the plugin about the channel"],
  ["approvalCapability.getActionAvailabilityState", "question to the plugin about approvals"],
  ["approvalCapability.authorizeActorAction", "question to the plugin about approvals"],
  ["gateway.startAccount", "the account started"],
  ["onAssistantMessageStart", "start of a message, before it has text"],
  ["onPartialReply", "partial text"],
  ["onToolResult", "formatted line about a tool, a copy of the tool delivery"],
]);

/** Only the tool names seen in the recordings. A command is known by `commandBearing`, not by name. */
const TOOL_KINDS = new Map<unknown, string>([
  ["write", "edit"],
  ["edit", "edit"],
]);

const DECISIONS = new Map<unknown, string>([
  ["allow-once", "Allow once"],
  ["allow-always", "Always allow"],
  ["deny", "Deny"],
]);

const BY_DRIVER = "(none: closed by the driver)";

/** Typed commands that change one setting of the session, and the setting each one changes. */
const SETTING_COMMANDS = new Map<string, string>([
  ["/think", "think"],
  ["/thinking", "think"],
  ["/t", "think"],
  ["/verbose", "verbose"],
  ["/v", "verbose"],
  ["/reasoning", "reasoning"],
  ["/reason", "reasoning"],
  ["/fast", "fast"],
  ["/trace", "trace"],
  ["/usage", "usage"],
  ["/queue", "queue"],
  ["/activation", "activation"],
  ["/send", "send"],
]);

/** Fields of a stored session entry that are settings: field -> id, label, needs the right to approve. */
const ENTRY_SETTINGS: [string, string, string, boolean][] = [
  ["thinkingLevel", "think", "Thinking", false],
  ["verboseLevel", "verbose", "Verbose", false],
  ["reasoningLevel", "reasoning", "Reasoning", false],
  ["fastMode", "fast", "Fast mode", false],
  ["traceLevel", "trace", "Trace", false],
  ["sendPolicy", "send", "Send policy", false],
  ["elevatedLevel", "elevated", "Elevated", true],
  ["permissionMode", "permission_mode", "Permission mode", true],
  ["execHost", "exec_host", "Exec host", true],
  ["execNode", "exec_node", "Exec node", true],
];
const ENTRY_FIELDS = new Set(ENTRY_SETTINGS.map(([field]) => field));

const HOOKS_NOT_SENT = new Map<unknown, string>([
  ["session_start", "a session started, which the next run also says"],
  ["before_reset", "the conversation that is about to be dropped"],
]);

/** The release every OpenClaw recording was made with. OpenClaw names its version to no channel. */
export const RECORDED_VERSION = "2026.11.5";

/** A setting's value as an event carries it: text. OpenClaw stores some as true or false. */
function asText(value: unknown): unknown {
  return value === null || value === undefined || typeof value === "string" ? (value ?? null) : JSON.stringify(value);
}

interface ToolCall extends Json {
  name: unknown;
  title: unknown;
  command: boolean;
  mode: "tool" | "question" | "send";
}

interface Ending {
  tid: unknown;
  state: State;
  native: unknown;
}

export interface TranslatorOptions {
  /** The OpenClaw release, for the session item (D-044). */
  version?: string;
  /**
   * Put in every id the translator makes up from a count: a message's, a notice's, an error's. A
   * plugin that restarts counts from one again, and an item id is never used for another item, also
   * after the connector reconnects (`docs/design/socket-protocol.md`). Replays leave it empty.
   */
  ids?: string;
  /** How many sessions each session key has named, as a plugin that restarted last knew it. */
  generations?: Iterable<[string, number]>;
}

/**
 * Translates what OpenClaw hands a plugin for one chat. Feed it every call in order; each call returns
 * the events that call made. A call it has no rule for makes no event and is counted in
 * {@link Translator.unknown}; a call left out on purpose is counted in {@link Translator.notSent}.
 */
export class Translator {
  /** Calls with no rule, by kind. */
  readonly unknown = new Map<string, number>();
  /** Calls left out on purpose, by kind. */
  readonly notSent = new Map<string, number>();

  private made: Kept[] = [];
  private readonly version: string;
  private readonly ids: string;
  private session: string | null = null;
  private turn: string | null = null;
  /** Every event this translator made; some made-up item ids use it. */
  private seq = 0;
  private readonly seqIn = new Map<string | null, number>();
  private readonly open = new Map<string, Kept>();
  /** Session item -> its body as last sent. */
  private readonly sessions = new Map<string, string>();
  /** Each item's last event. */
  private readonly lastOf = new Map<string, Kept>();

  private turnN = 0;
  private turnBody: Json = {};
  /** The open turn was opened by the driver, not reported by OpenClaw. */
  private driverTurn = false;
  /** The last turn, if it failed: its error text arrives after it. */
  private failedTurn: string | null = null;
  private routeSession: string | null = null;
  private model: string | null = null;
  private think: unknown = null;
  private modelNative: unknown = null;
  private defaultModel: string | null = null;
  /** The model's thinking so far. */
  private thinking = "";
  private readonly tools = new Map<unknown, ToolCall>();
  /** A tool that has ended and is held for one call, for its result line. */
  private ending: Ending | null = null;
  /** The ask_user call in progress. */
  private asking: unknown = null;
  /** Message id -> what the person sent. */
  private readonly inboundText = new Map<unknown, string>();
  /** Slash commands handed to OpenClaw and not yet answered. */
  private readonly commands: string[] = [];
  /** Approval id -> the decision the client sent. */
  private readonly decisions = new Map<unknown, unknown>();
  private lastPress: unknown = null;
  /** The plugin's id for a posted message -> the event it became. */
  private readonly sent = new Map<unknown, Kept>();
  /** The call whose result will name the posted message, and the event it became. */
  private posting: [string, Kept] | null = null;
  /** Tool calls whose approval was denied. */
  private readonly denied = new Set<unknown>();
  /** The session key of the last route OpenClaw gave. */
  private routeKey: string | null = null;
  /** Session key -> how many sessions it has named. */
  private readonly generation = new Map<string, number>();
  /** Session name -> setting id -> the setting. */
  private readonly known = new Map<string, Map<string, Json>>();
  /** Sessions OpenClaw has started afresh. */
  private readonly ended = new Set<string>();
  /** Setting id -> the values OpenClaw accepts, if the plugin asked. */
  private readonly options = new Map<unknown, unknown[]>();
  /** OpenClaw's command list, if the plugin asked. */
  private commandList: Json[] = [];
  /** The session key the plugin last asked about. */
  private askedKey: string | null = null;
  /** The text of the message last handed to inbound.run, until that call returns. */
  private lastRun: string | null = null;
  /** The run the line being fed came from, if the plugin named it. */
  private lineRun: string | null = null;
  private resetByHook = false;

  constructor(options: TranslatorOptions = {}) {
    this.version = options.version ?? RECORDED_VERSION;
    this.ids = options.ids ?? "";
    for (const [key, n] of options.generations ?? []) this.generation.set(key, n);
  }

  /** The turn that is running, as the events name it, or null between turns. */
  get currentTurn(): string | null {
    return this.turn;
  }

  /** The session the events name. */
  get currentSession(): string | null {
    return this.session;
  }

  /** How many sessions each session key has named: what a plugin keeps across a restart. */
  get generations(): [string, number][] {
    return [...this.generation];
  }

  feed(line: Line): AgentEvent[] {
    this.made = [];
    this.lineRun = line.run ?? null;
    this.handle(line);
    return this.shaped();
  }

  /**
   * The plugin knows the turn will report nothing more: OpenClaw's run for it threw, or the plugin is
   * stopping. The driver closes the open items and the turn as cancelled. Replays never call this.
   */
  stopped(reason: string): AgentEvent[] {
    this.made = [];
    if (this.ending !== null) this.finishTool();
    if (this.turn !== null) this.closeTurn("cancelled", BY_DRIVER, { reason });
    else this.closeOpenItems(reason);
    return this.shaped();
  }

  /**
   * A notice the plugin makes, outside any turn: OpenClaw answered a command with nothing. Replays never
   * call this.
   */
  notice(title: string, text: string, body: Json): AgentEvent[] {
    this.made = [];
    this.emit(this.next("notice"), "notice", "done", title, text, body, BY_DRIVER, null, [this.session, null]);
    return this.shaped();
  }

  private shaped(): AgentEvent[] {
    const events = this.made.map(
      (e) =>
        crateShape({
          harness: "openclaw",
          session: e.session,
          turn: e.turn,
          item: e.item,
          seq: e.seq,
          type: e.type,
          state: e.state,
          title: e.title,
          text: e.text,
          body: e.body,
          native: e.native,
        }) as AgentEvent,
    );
    this.made = [];
    return events;
  }

  /** An id made up from the count of events, as the prototype makes it. */
  private next(kind: string): string {
    return `${kind}:${this.ids}${this.seq + 1}`;
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
  ): Kept {
    const [session, turn] = owner;
    this.seq += 1;
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
      native: nativeKind.startsWith("(none") ? null : { kind: nativeKind, payload: native ?? null },
    };
    this.made.push(event);
    this.lastOf.set(item, event);
    if (state === "running" || state === "waiting") this.open.set(item, event);
    else this.open.delete(item);
    return event;
  }

  /** A turn ended. Anything still running in it will never report again, so close it. */
  private closeOpenItems(_reason: string): void {
    for (const [item, e] of [...this.open]) {
      if (e.type === "turn" || e.type === "task" || e.type === "session") continue;
      this.emit(item, e.type, "cancelled", e.title, e.text, e.body, BY_DRIVER, null, [e.session, e.turn]);
    }
  }

  /** A session started, or something about it changed. Sent only when the body differs. */
  private sessionItem(sid: string, settings: Json[], nativeKind: string, native: unknown): void {
    const item = `session:${sid}`;
    const body: Json = { harness_version: this.version, settings, actions: [], commands: this.commandList };
    const key = canonical(body);
    if (this.sessions.get(item) === key) return;
    this.sessions.set(item, key);
    const text = settings
      .filter((x) => x["value"] !== null && x["value"] !== undefined)
      .map((x) => `${word(x["label"])}: ${word(x["value"])}`)
      .join(", ");
    // A session belongs to no turn.
    this.emit(item, "session", "running", "Session", text, body, nativeKind, native, [this.session, null]);
  }

  private skip(kind: string): void {
    this.notSent.set(kind, (this.notSent.get(kind) ?? 0) + 1);
  }

  private noRule(kind: string): void {
    this.unknown.set(kind, (this.unknown.get(kind) ?? 0) + 1);
  }

  private handle(line: Line): void {
    const m = object(line.msg);
    const via = line.via;
    const out = line.dir === "out";
    if (via === "note") return this.skip("recorder note");
    const method = m["method"];
    if (via === "result") return this.result(method, m);
    // A tool that is not a command reports its end, and its result arrives as text in the tool
    // delivery that follows. Any other call means no result line is coming.
    if (this.ending !== null && !(via === "hook" && (method === "onToolResult" || method === "deliver"))) {
      this.finishTool();
    }
    if (via === "inbound") return this.inbound(m);
    if (via === "press") return this.press(m);
    if (via === "plugin_hook") return this.pluginHook(m);
    if (via === "call") return out ? this.callOut(word(method), m) : this.callIn(word(method), m);
    const p = m["params"];
    const name = word(method);
    const reason = NOT_SENT.get(name);
    if (reason !== undefined) return this.skip(`${reason} (${name})`);
    switch (name) {
      case "onModelSelected":
        return this.modelSelected(object(p));
      case "onPreparedAgentRunStart":
        return this.runStart(name, object(p));
      case "onAgentRunTerminalOutcome":
        return this.runEnd(name, p);
      case "gateway.abort":
        return this.accountStopped(name, p);
      case "onItemEvent":
        return this.itemEvent(object(p));
      case "onToolStart":
        return this.toolStart(name, object(p));
      case "onCommandOutput":
        return this.commandOutput(name, object(p));
      case "onApprovalEvent":
        return this.approvalEvent(name, object(p));
      case "onReasoningStream":
        return this.reasoningStream(object(p));
      case "onReasoningEnd":
        return this.reasoningEnd(name);
      case "deliver":
        return this.deliver(name, p);
      case "message.send.payload":
        return this.sendPayload(name, object(p));
      case "approvalCapability.render.exec.buildPendingPayload":
        return this.approvalCard(name, object(p));
      case "approvalCapability.render.exec.buildResolvedPayload":
        return this.approvalCardResolved(name, object(p));
      case "actions.handleAction":
        return this.handleAction(name, object(p));
    }
    this.noRule(name);
  }

  // -- what the plugin sends, and what its own calls return

  /** Which control a typed command is: counted, as the prototype counts it, and not sent. */
  private inbound(m: Json): void {
    const text = plain(or(m["text"], "")).trim();
    this.inboundText.set(m["message_id"] ?? null, text);
    if (text.split(" ")[0] === "/approve") return this.skip("client: answer to an approval, as a slash command");
    this.skip(
      text.startsWith("/") ? "client: slash command" : text.startsWith("!") ? "client: host command" : "client: chat message",
    );
  }

  private press(m: Json): void {
    if ("approvalId" in m) {
      this.lastPress = m["approvalId"];
      return this.skip("client: button press on an approval");
    }
    this.skip("client: button press on a question");
  }

  private callOut(method: string, m: Json): void {
    if (method === "inbound.run") {
      // Only a message that is run can be answered. A command is answered by OpenClaw itself.
      const text = this.inboundText.get(object(m["params"])["messageId"] ?? null) ?? "";
      this.lastRun = text;
      if (text.startsWith("/") || text.startsWith("!")) {
        this.commands.push(text);
        const parts = words(text);
        if (parts[0] === "/approve" && parts.length >= 3) this.decisions.set(parts[1], parts[2]);
      }
    } else if (method === "session-store-runtime.getSessionEntryAsync") {
      const key = object(m["params"])["sessionKey"];
      this.askedKey = typeof key === "string" ? key : null;
    }
    this.skip(`client: ${method}`);
  }

  private callIn(method: string, m: Json): void {
    const result = object(or(m["result"], {}));
    if (method === "routing.resolveAgentRoute") {
      const key = result["sessionKey"];
      this.routeKey = typeof key === "string" ? key : null;
      this.routeSession = this.name(this.routeKey);
      if (this.turn === null) this.session = this.routeSession;
    } else if (method === "inbound.run") {
      const dropped = truthy(object(or(result["dispatchResult"], {}))["beforeAgentRunBlocked"]);
      const ran = this.ran();
      if (dropped && ran !== null) {
        // OpenClaw took the message as a command from someone who may not use commands. It ran
        // nothing and sent nothing, so no reply will answer it.
        this.removeCommand(ran);
      }
      this.lastRun = null;
      if (dropped) return this.skip("result of inbound.run: OpenClaw dropped the message and sent nothing");
      if (this.driverTurn) {
        // With no run callbacks registered, the return of inbound.run is the only end there is.
        return this.closeTurn("done", "result of inbound.run", result);
      }
    } else if (
      method.startsWith("native-command-registry.") ||
      method.startsWith("models-provider-runtime.") ||
      method.startsWith("session-store-runtime.")
    ) {
      return this.asked(method, m["result"]);
    } else if (method === "resolveApprovalOverGateway") {
      if ("error" in m) {
        const err = object(m["error"]);
        this.emit(
          this.next("error"),
          "error",
          "failed",
          "OpenClaw refused an answer",
          or(err["message"], ""),
          { request: method, subject: `approval:${word(this.lastPress)}` },
          `error result of ${method}`,
          or(m["error"], {}),
        );
        return;
      }
      const approval = object(result["approval"]);
      if (!truthy(result["applied"])) return this.noRule(`${method} result that was not applied`);
      return this.approvalResolved(approval["id"], `result of ${method}`, result, approval["decision"]);
    } else if (method === "questionGatewayRuntime.resolveOption") {
      return this.questionAnswered(method, result);
    }
    this.skip(`result of ${method}`);
  }

  /** What the plugin returned to OpenClaw. For a posted message that is the plugin's id for it. */
  private result(method: unknown, m: Json): void {
    const res = m["result"];
    if (this.posting !== null && this.posting[0] === method && isObject(res)) {
      const mid = or(res["messageId"], object(res["details"])["messageId"], object(res["receipt"])["primaryPlatformMessageId"]);
      if (truthy(mid)) this.sent.set(mid, this.posting[1]);
      this.posting = null;
    }
    this.skip(`result of ${word(method)}`);
  }

  /**
   * The text of the message whose run this line belongs to: the run the plugin named, or in a replay,
   * the message last handed to `inbound.run` while that call has not returned.
   */
  private ran(): string | null {
    return this.lineRun !== null ? (this.inboundText.get(this.lineRun) ?? null) : this.lastRun;
  }

  private removeCommand(text: string): void {
    const at = this.commands.indexOf(text);
    if (at !== -1) this.commands.splice(at, 1);
  }

  // -- sessions

  /**
   * OpenClaw keeps a session's key and id when it starts the session afresh. So the driver counts: the
   * second session under a key is named key#2.
   */
  private name(key: string | null): string | null {
    if (key === null) return null;
    const n = this.generation.get(key) ?? 1;
    return n === 1 ? key : `${key}#${n}`;
  }

  private put(name: string, id: string, label: string, value: unknown, approve = false): void {
    let settings = this.known.get(name);
    if (settings === undefined) {
      settings = new Map();
      this.known.set(name, settings);
    }
    settings.set(id, { id, label, value: asText(value), options: this.options.get(id) ?? null, approve });
  }

  /** Send the session item, if anything in it is new. */
  private sendSession(name: string, nativeKind: string, native: unknown): void {
    const settings = this.known.get(name);
    if (settings === undefined || this.ended.has(name)) return;
    for (const [k, x] of settings) settings.set(k, { ...x, options: this.options.get(x["id"]) ?? x["options"] ?? null });
    const session = this.session;
    this.session = name;
    this.sessionItem(name, [...settings.values()], nativeKind, native);
    this.session = session;
  }

  /** OpenClaw started this chat's session afresh. Its key and its id stay the same. */
  private sessionReplaced(key: string | null, nativeKind: string, native: unknown): void {
    if (key === null) return this.noRule("a session replaced with no session key");
    let name = this.name(key)!;
    const prev = this.last(`session:${name}`);
    if (prev !== undefined && prev.state === "running") {
      this.emit(prev.item, "session", "done", "Session ended", prev.text, prev.body, nativeKind, native, [name, null]);
    }
    this.ended.add(name);
    this.generation.set(key, (this.generation.get(key) ?? 1) + 1);
    name = this.name(key)!;
    this.known.set(name, new Map());
    this.sendSession(name, nativeKind, native);
    if (key === this.routeKey) {
      this.routeSession = name;
      if (this.turn === null) this.session = name;
    }
  }

  private pluginHook(m: Json): void {
    const hook = m["hook"];
    const event = object(m["event"]);
    if (hook === "session_end") {
      const reason = event["reason"];
      if (reason === "new" || reason === "reset") {
        this.resetByHook = true;
        const key = event["sessionKey"];
        return this.sessionReplaced(typeof key === "string" ? key : null, `${hook} [${reason}]`, m);
      }
      return this.skip(`plugin hook session_end [${word(reason)}]: the gateway stopped, and the session is kept`);
    }
    const reason = HOOKS_NOT_SENT.get(hook);
    if (reason !== undefined) return this.skip(`plugin hook ${word(hook)}: ${reason}`);
    this.noRule(`plugin hook ${word(hook)}`);
  }

  /** What the plugin SDK returned when the plugin asked for something as data. */
  private asked(method: string, result: unknown): void {
    if (method.endsWith(".listChatCommands") && Array.isArray(result)) {
      this.commandList = result.map((c) => {
        const x = object(c);
        const aliases = list(x["textAliases"]);
        const name = aliases.length > 0 ? aliases[0] : `/${word(x["key"])}`;
        const hint = list(x["args"])
          .map((a) => `<${word(object(a)["name"])}>`)
          .join(" ");
        return command(name, x["description"], hint);
      });
      for (const c of result) {
        const x = object(c);
        const args = list(x["args"]);
        const arg = object(args.length > 0 ? args[0] : {});
        const choices = or(arg["resolvedChoices"], arg["choices"]);
        if (Array.isArray(choices) && args.length === 1) {
          this.options.set(
            x["key"] ?? null,
            choices.map((v) => asText(isObject(v) ? (v["value"] ?? null) : v)),
          );
        }
      }
    } else if (method.endsWith(".buildModelsProviderData") && isObject(result)) {
      const models: string[] = [];
      for (const [provider, ids] of Object.entries(object(or(result["byProvider"], {})))) {
        for (const id of list(ids)) models.push(`${provider}/${word(id)}`);
      }
      this.options.set("model", models);
      const d = object(or(result["resolvedDefault"], {}));
      this.defaultModel = truthy(d) ? `${word(d["provider"])}/${word(d["model"])}` : null;
    } else if (method.endsWith(".getSessionEntryAsync") && isObject(result)) {
      const name = this.name(this.askedKey);
      if (name === null) return this.noRule("a stored session entry for no session");
      // The store has an entry, so there is a session to describe.
      if (!this.known.has(name)) this.known.set(name, new Map());
      if (truthy(result["providerOverride"]) && truthy(result["modelOverride"])) {
        this.put(name, "model", "Model", `${word(result["providerOverride"])}/${word(result["modelOverride"])}`);
      } else if (result["modelOverrideSource"] === "default" && truthy(this.defaultModel)) {
        // The session's own choice was cleared. The entry does not name the default it falls back
        // to; the model list does, if the plugin asked for it.
        this.put(name, "model", "Model", this.defaultModel);
      }
      for (const [field, id, label, approve] of ENTRY_SETTINGS) {
        if (field in result) this.put(name, id, label, result[field], approve);
      }
      this.skip(`result of ${method}`);
      const native: Json = {};
      for (const [k, v] of Object.entries(result)) if (ENTRY_FIELDS.has(k) || k.includes("verride")) native[k] = v;
      return this.sendSession(name, method, native);
    }
    // The lists belong in the item of every session still running.
    for (const name of [...this.known.keys()]) this.sendSession(name, method, { asked: method });
    this.skip(`result of ${method}`);
  }

  // -- turns

  private modelSelected(p: Json): void {
    this.model = `${word(p["provider"])}/${word(p["model"])}`;
    this.think = or(p["thinkLevel"], this.think);
    this.modelNative = p;
    this.skip("onModelSelected, kept for the turn and the session item");
  }

  private openTurn(turn: unknown = null): void {
    // OpenClaw names the model and the thinking level before each run. It says nothing else about a
    // session unless the plugin asks.
    if (truthy(this.session)) {
      this.put(this.session!, "model", "Model", this.model);
      this.put(this.session!, "think", "Thinking", this.think);
      this.sendSession(this.session!, "onModelSelected", this.modelNative);
    }
    this.turnN += 1;
    this.turn = truthy(turn) ? String(turn) : `${word(this.session)}:${this.ids}${this.turnN}`;
    this.failedTurn = null;
    this.turnBody = { started_by: "user" };
    if (truthy(this.model)) this.turnBody["model"] = this.model;
  }

  private runStart(method: string, p: Json): void {
    if (this.turn !== null) return this.noRule("onPreparedAgentRunStart while a turn is open");
    const key = object(p["transcriptStart"])["sessionKey"];
    if (truthy(key)) this.session = this.name(String(key));
    const ran = this.ran();
    if (ran !== null && this.commands.includes(ran)) {
      // The message started a run, so it was a directive with a task, or a command the model answers.
      // OpenClaw sends no reply of its own for it.
      this.removeCommand(ran);
    }
    this.openTurn(p["runId"]);
    this.emit(`turn:${this.turn}`, "turn", "running", "Working", "", this.turnBody, method, p);
  }

  /** The agent spoke and no run was reported: no run callbacks were registered. */
  private ensureTurn(): void {
    if (this.turn !== null) return;
    this.openTurn();
    this.driverTurn = true;
    this.emit(`turn:${this.turn}`, "turn", "running", "Working", "", this.turnBody, "(none: opened by the driver)", null);
  }

  private runEnd(method: string, p: unknown): void {
    const state: State | undefined = p === "completed" ? "done" : p === "failed" ? "failed" : undefined;
    if (state === undefined) return this.noRule(`onAgentRunTerminalOutcome [${word(p)}]`);
    if (this.turn === null) {
      if (state !== "failed") return this.noRule("onAgentRunTerminalOutcome with no turn open");
      this.openTurn(); // the run failed before OpenClaw reported it as started
    }
    this.closeTurn(state, method, p);
  }

  private closeTurn(state: "done" | "failed" | "cancelled", nativeKind: string, native: unknown, text = ""): void {
    if (this.thinking.trim() !== "") {
      // Thinking that never reported its end.
      this.emit(this.next("reasoning"), "reasoning", "done", "Thinking", this.thinking, {}, BY_DRIVER, null);
    }
    this.thinking = "";
    this.closeOpenItems(`the turn ended: ${state}`);
    const title = state === "done" ? "Finished" : state === "failed" ? "Failed" : "Interrupted";
    this.emit(`turn:${this.turn}`, "turn", state, title, text, this.turnBody, nativeKind, native);
    this.failedTurn = state === "failed" ? this.turn : null;
    this.turn = null;
    this.driverTurn = false;
    this.asking = null;
    this.session = this.routeSession ?? this.session;
  }

  /** OpenClaw asked the account to stop. Nothing in progress will report again. */
  private accountStopped(method: string, p: unknown): void {
    if (![...this.open.values()].some((e) => e.type !== "task")) return this.skip(method);
    if (this.turn !== null) {
      return this.closeTurn("cancelled", method, p, "The account stopped before the turn finished.");
    }
    this.closeOpenItems("the account stopped");
  }

  // -- tools

  /**
   * A call made through dispatch_action arrives twice: under the model's id, and again under an id built
   * from it. The second is the copy.
   */
  private isInner(tid: unknown): boolean {
    const parts = word(tid).split(":");
    return parts.length > 1 && parts[0] === "tool_call" && this.tools.has(parts[1]);
  }

  private itemEvent(p: Json): void {
    const tid = p["toolCallId"] ?? null;
    const phase = p["phase"];
    const status = p["status"];
    if (this.isInner(tid)) return this.skip("inner copy of a call made through dispatch_action");
    if (phase === "start") {
      this.tools.set(tid, { name: p["name"] ?? null, title: p["title"] ?? null, command: truthy(p["commandBearing"]), mode: "tool" });
      return this.skip("onItemEvent start, kept until onToolStart gives the arguments");
    }
    const t = this.tools.get(tid);
    if (t === undefined || !("body" in t)) return this.noRule("onItemEvent for a tool call that never started");
    if (phase === "update") return this.skip("onItemEvent update with nothing new");
    if (phase !== "end") return this.noRule(`onItemEvent [${word(phase)}]`);
    if (truthy(t["ended"])) return this.skip("onItemEvent end, repeated");
    t["ended"] = true;
    const state: State | undefined =
      status === "completed" ? "done" : status === "failed" || status === "blocked" ? "failed" : undefined;
    if (state === undefined) return this.noRule(`onItemEvent end [${word(status)}]`);
    if (t.mode === "question" && truthy(t["question"])) {
      const prev = this.open.get(String(t["question"]));
      if (prev === undefined) return this.skip("end of a tool call that is really a question (ask_user)");
      // The tool ended and no answer went through the plugin. The result line says why, in words; the
      // callback does not.
      this.emit(prev.item, "question", "cancelled", prev.title, "Closed with no answer from the chat", prev.body, "onItemEvent [end of ask_user]", p);
      return;
    }
    if (t.mode === "send" && truthy(t["message"])) {
      if (state === "done") return this.skip("end of a tool call that is really a message (message send)");
      const prev = t["message"] as Kept;
      this.emit(prev.item, "message", "failed", prev.title, prev.text, prev.body, "onItemEvent [end of message send]", p);
      return;
    }
    if (t.command) return this.skip("onItemEvent end of a command, which closes on onCommandOutput");
    this.ending = { tid, state, native: p }; // sent by finishTool, with the next call
  }

  private toolStart(method: string, p: Json): void {
    const tid = p["toolCallId"] ?? null;
    const phase = p["phase"];
    if (this.isInner(tid)) return this.skip("inner copy of a call made through dispatch_action");
    if (phase === "update") return this.skip("onToolStart update with no arguments");
    if (phase !== "start") return this.noRule(`onToolStart [${word(phase)}]`);
    let t = this.tools.get(tid);
    if (t === undefined) {
      t = { name: p["name"] ?? null, title: null, command: false, mode: "tool" };
      this.tools.set(tid, t);
    }
    let name: unknown = or(t.name, p["name"] ?? null);
    let args = object(or(p["args"], {}));
    let wrapper: string | null = null;
    if (p["name"] === "dispatch_action") {
      // A wrapper: the real tool and its arguments are inside.
      name = or(args["id"], name);
      args = object(or(args["args"], {}));
      wrapper = "dispatch_action";
    }
    const kind = t.command ? "command" : (TOOL_KINDS.get(name) ?? "other");
    const body: Json = { kind, name };
    if (wrapper !== null) body["called_through"] = wrapper;
    if (kind === "command") {
      body["command"] = args["command"] ?? null;
    } else {
      if (kind === "edit") body["path"] = args["path"] ?? null;
      body["input"] = args;
    }
    t.name = name;
    t["args"] = args;
    t["body"] = body;
    t["text"] = or(body["command"], body["path"], t.title, name);
    if (name === "ask_user") {
      // A question dressed as a tool call. It becomes a question when it reaches the chat. If OpenClaw
      // rejects the call first, it never does, and is sent as a failed tool.
      t.mode = "question";
      this.asking = tid;
      return this.skip("tool call that is really a question (ask_user)");
    }
    if (name === "message" && args["action"] === "send") {
      t.mode = "send";
      return this.skip("tool call that is really a message (message send)");
    }
    this.emit(word(tid), "tool", "running", toolTitle(kind, "running"), t["text"], body, `${method} [${word(name)}]`, p);
  }

  private commandOutput(method: string, p: Json): void {
    const tid = p["toolCallId"] ?? null;
    const phase = p["phase"];
    if (phase === "delta") return this.skip("partial output (onCommandOutput delta)");
    if (phase !== "end") return this.noRule(`onCommandOutput [${word(phase)}]`);
    if (!("itemId" in p)) return this.skip("onCommandOutput end with no fields; a second call carries them");
    const t = this.tools.get(tid);
    let state: State | undefined = p["status"] === "completed" ? "done" : p["status"] === "failed" ? "failed" : undefined;
    if (t === undefined || !this.open.has(word(tid)) || state === undefined) {
      return this.noRule("onCommandOutput end for a command that is not open");
    }
    if (state === "failed" && this.denied.has(tid)) state = "declined";
    t["body"] = {
      ...object(t["body"]),
      cwd: p["cwd"] ?? null,
      exit_code: p["exitCode"] ?? null,
      duration_ms: p["durationMs"] ?? null,
      output: plain(p["output"]),
    };
    this.emit(word(tid), "tool", state, toolTitle("command", state), t["text"], object(t["body"]), `${method} [end]`, p);
  }

  /** Close a tool that is not a command. Its result exists only as a formatted line. */
  private finishTool(output: unknown = null, delivery: unknown = undefined): void {
    const e = this.ending!;
    this.ending = null;
    const t = this.tools.get(e.tid)!;
    if (output !== null && output !== undefined) t["body"] = { ...object(t["body"]), output: plain(output) };
    const native = delivery !== undefined ? { onItemEvent: e.native, deliver: delivery } : e.native;
    const body = object(t["body"]);
    this.emit(
      word(e.tid),
      "tool",
      e.state,
      toolTitle(body["kind"], e.state),
      t["text"],
      body,
      "onItemEvent [end]" + (delivery !== undefined ? " and its tool delivery" : ""),
      native,
    );
  }

  private approvalEvent(method: string, p: Json): void {
    const tid = p["toolCallId"] ?? null;
    const status = p["status"];
    const t = this.tools.get(tid);
    if (status === "failed") return this.skip("onApprovalEvent: the approval request failed, and the tool item already failed");
    if (status !== "denied" || t === undefined || !("body" in t)) return this.noRule(`onApprovalEvent [${word(status)}]`);
    const prev = this.last(word(tid));
    if (prev !== undefined && prev.state === "declined") {
      return this.skip("onApprovalEvent: the denial is already known from the approval");
    }
    // The command was reported as failed one call ago. Only this call says a person said no.
    const body = object(t["body"]);
    this.emit(word(tid), "tool", "declined", toolTitle(body["kind"], "declined"), t["text"], body, `${method} [denied]`, p);
  }

  /** The last event of an item. */
  private last(item: string): Kept | undefined {
    return this.lastOf.get(item);
  }

  // -- the agent's message tool

  /** OpenClaw asks the plugin to carry out an action of the agent's message tool. */
  private handleAction(method: string, p: Json): void {
    const action = p["action"];
    const params = object(p["params"]);
    const key = plain(or(params["idempotencyKey"], ""));
    const tid = [...this.tools.keys()].find((i) => key.endsWith(`:${word(i)}`));
    const t = tid === undefined ? undefined : this.tools.get(tid);
    if (t === undefined || !("body" in t)) return this.noRule("actions.handleAction with no tool call");
    if (action === "send" && t.mode === "send") {
      const body: Json = { sent_with: "message tool" };
      const args = object(t["args"]);
      if (args["final"] !== null && args["final"] !== undefined) body["phase"] = truthy(args["final"]) ? "final" : "commentary";
      const ev = this.emit(`message:${word(tid)}`, "message", "done", "Message", params["message"], body, `${method} [send]`, p);
      t["message"] = ev;
      this.posting = [method, ev];
      return;
    }
    // react and delete. No item type says "the agent acted on the chat", so the call stays a tool
    // item. What it acted on is only here: the tool arguments name no message for a react.
    const target = or(params["messageId"], object(p["toolContext"])["currentMessageId"]) ?? null;
    const body: Json = { ...object(t["body"]), action, target: { chat: params["to"] ?? null, message: target } };
    const sent = this.sent.get(target);
    if (sent !== undefined) body["subject"] = sent.item;
    t["body"] = body;
    this.skip("actions.handleAction: the plugin carries out an action that the tool item shows");
  }

  // -- thinking

  private reasoningStream(p: Json): void {
    this.thinking = plain(or(p["text"], ""));
    this.skip("partial thinking (onReasoningStream)");
  }

  private reasoningEnd(method: string): void {
    const text = this.thinking;
    this.thinking = "";
    if (text.trim() === "") return this.skip("onReasoningEnd with no thinking text");
    this.emit(this.next("reasoning"), "reasoning", "done", "Thinking", text, {}, `${method}, with the last onReasoningStream`, { text });
  }

  // -- everything that arrives through deliver

  private deliver(method: string, p: unknown): void {
    const pair = list(p);
    const payload = object(or(pair[0] ?? null, {}));
    const info = object(or(pair[1] ?? null, {}));
    const kind = info["kind"];
    const text = payload["text"];
    this.posting = null;
    if (kind === "tool") {
      if (truthy(object(payload["channelData"])["askUser"])) return this.questionAsked(payload, p);
      if (this.ending !== null) return this.finishTool(text ?? null, p);
      return this.skip("deliver [tool]: formatted line about a tool that is already an item");
    }
    if (this.ending !== null) this.finishTool();
    if (truthy(payload["isError"])) {
      // A failed run reports its outcome first and its error text after.
      const turn = this.turn ?? this.failedTurn;
      this.emit(this.next("error"), "error", "failed", "Error", text, {}, `${method} [${word(kind)}, isError]`, p, [this.session, turn]);
      return;
    }
    if (truthy(payload["isStatusNotice"]) && kind !== "final" && !("assistantMessageIndex" in info)) {
      // OpenClaw's own line about a directive it took out of a message, sent before the run that the
      // rest of the message starts. "/exec security=full ask=off <task>" is answered this way.
      const ran = this.ran();
      const command = ran !== null && ran.startsWith("/") ? words(ran)[0] : null;
      if (ran !== null) this.removeCommand(ran);
      const ev = this.emit(
        this.next("notice"),
        "notice",
        "done",
        "Directive taken",
        text,
        { kind: "command_output", command: command ?? null },
        `${method} [${word(kind)}, isStatusNotice]`,
        p,
      );
      this.posting = [method, ev];
      return;
    }
    if ("assistantMessageIndex" in info) {
      // Only the agent's own words carry a message index.
      this.ensureTurn();
      const ev = this.emit(this.next("message"), "message", "done", "Message", text, kind === "final" ? { phase: "final" } : {}, `${method} [${word(kind)}]`, p);
      this.posting = [method, ev];
      return;
    }
    if (kind === "final") return this.ownReply(method, text, p);
    this.noRule(`deliver [${word(kind)}] with no message index`);
  }

  /**
   * OpenClaw answers a slash command itself. The reply carries no mark of which command it answers. The
   * plugin names the run it came from, which is the command's own; a replay has no run, and takes it as
   * the answer to the oldest command still waiting.
   */
  private ownReply(method: string, text: unknown, p: unknown): void {
    if (this.commands.length === 0) return this.noRule("deliver [final] that is not the agent's, with no command waiting");
    const own = this.lineRun !== null ? this.commands.indexOf(this.inboundText.get(this.lineRun) ?? "") : -1;
    const command = words(own !== -1 ? this.commands.splice(own, 1)[0]! : this.commands.shift()!)[0] ?? "";
    if (command !== "/stop") {
      const body: Json = { kind: "command_output", command };
      let title = "Command output";
      if ((command === "/new" || command === "/reset") && truthy(object(list(p)[0])["isStatusNotice"])) {
        // The session was started afresh. Without the plugin hook this reply is the only sign.
        title = "New session";
        if (!this.resetByHook) this.sessionReplaced(this.routeKey, `${method} [final, isStatusNotice, reply to ${command}]`, p);
        this.resetByHook = false;
      }
      const ev = this.emit(this.next("notice"), "notice", "done", title, text, body, `${method} [final, reply to ${command}]`, p);
      this.posting = [method, ev];
      return;
    }
    const ev = this.emit(this.next("notice"), "notice", "done", "Stopped", text, { kind: "command_output", command }, `${method} [final, reply to ${command}]`, p);
    this.posting = [method, ev];
    if (this.turn !== null) {
      // A stopped run reports no outcome.
      this.closeTurn("cancelled", BY_DRIVER, { reason: "the run was stopped and reports no outcome" });
    }
  }

  // -- questions

  private questionAsked(payload: Json, p: unknown): void {
    const ask = object(object(payload["channelData"])["askUser"]);
    // The delivery names the question, not the tool call. It is taken to belong to the one ask_user
    // call in progress, whose arguments hold the question in full.
    const t: Json = this.tools.get(this.asking) ?? {};
    const buttons: Json[] = [];
    for (const blk of list(object(payload["presentation"])["blocks"])) {
      if (object(blk)["type"] === "buttons") for (const b of list(object(blk)["buttons"])) buttons.push(object(b));
    }
    const allowText = buttons.some((b) => object(b["action"])["intent"] === "custom-input");
    const qs: Json[] = [];
    list(object(t["args"])["questions"]).forEach((asked, n) => {
      const q = object(asked);
      const options = list(q["options"]).map((o) => (isObject(o) ? o : { label: o }));
      qs.push({
        id: or(q["id"], `q${n}`),
        header: q["header"] ?? null,
        text: q["question"] ?? null,
        multiple: truthy(q["multiSelect"]),
        allow_text: allowText,
        options: options.map((o, k) => ({
          id: `o${k}`,
          label: o["label"] ?? null,
          description: o["description"] ?? null,
          native: "value" in o ? o["value"] : (o["label"] ?? null),
        })),
      });
    });
    if (qs.length === 0) {
      qs.push({
        id: "q0",
        text: payload["text"] ?? null,
        multiple: false,
        allow_text: allowText,
        options: list(ask["optionValues"]).map((v, k) => ({ id: `o${k}`, label: v, native: v })),
      });
    }
    const item = `question:${word(ask["questionId"])}`;
    t["question"] = item;
    this.emit(item, "question", "waiting", "Question", qs[0]!["text"], { answer_via: "request", subject: this.asking, questions: qs }, "deliver [tool, channelData.askUser]", p);
  }

  private questionAnswered(method: string, result: Json): void {
    const item = this.tools.get(this.asking)?.["question"];
    const prev = typeof item === "string" ? this.open.get(item) : undefined;
    if (prev === undefined || result["status"] !== "answered") return this.noRule(`${method} result [${word(result["status"])}]`);
    // The press names the question as OpenClaw does, which is not the id of one of its questions: a
    // press answers the first, as a button does.
    const q = object(list(prev.body["questions"])[0]);
    const chosen = list(q["options"])
      .filter((o) => object(o)["native"] === result["optionValue"])
      .map((o) => object(o)["id"]);
    const answer = [{ question: q["id"], options: chosen, text: chosen.length > 0 ? null : (result["optionValue"] ?? null) }];
    this.emit(prev.item, "question", "done", prev.title, "Answered", { ...prev.body, answer }, `result of ${method}`, result);
  }

  // -- approvals

  private choices(allowed: unknown): Json[] {
    return list(allowed).map((d, n) => ({
      id: `c${n}`,
      label: DECISIONS.get(d) ?? d,
      effect: d === "deny" ? "deny" : "allow",
      native: d,
    }));
  }

  /** The plugin claims the approval capability, so the request arrives with its fields. */
  private approvalCard(method: string, p: Json): void {
    const req = object(p["request"]);
    const inner = object(req["request"]);
    this.emit(
      `approval:${word(req["id"])}`,
      "approval",
      "waiting",
      "Allow this command?",
      inner["command"],
      {
        answer_via: "request",
        subject: inner["toolCallId"] ?? null,
        choices: this.choices(inner["allowedDecisions"]),
        expires_at_ms: req["expiresAtMs"] ?? null,
      },
      method,
      p,
    );
  }

  private approvalCardResolved(method: string, p: Json): void {
    const resolved = object(p["resolved"]);
    this.approvalResolved(resolved["id"], method, p, resolved["decision"]);
  }

  /** A message OpenClaw sends outside a reply. In the recordings every one is an approval. */
  private sendPayload(method: string, p: Json): void {
    const payload = object(p["payload"]);
    const approval = object(object(payload["channelData"])["execApproval"]);
    if (!truthy(approval)) return this.noRule("message.send.payload that is not an approval");
    const aid = approval["approvalId"];
    const state = approval["state"];
    if (state === "resolved") return this.approvalResolved(aid, `${method} [execApproval resolved]`, p);
    if (state !== "pending") return this.noRule(`message.send.payload [execApproval ${word(state)}]`);
    if (this.open.has(`approval:${word(aid)}`)) return this.skip("message.send.payload: an approval prompt as text, already an item");
    // Without the approval capability the prompt names no tool call. If exactly one command is running,
    // the approval is taken to be about it.
    const running = [...this.open].filter(([, e]) => e.type === "tool" && e.body["kind"] === "command").map(([i]) => i);
    const body: Json = {
      answer_via: "message",
      subject: running.length === 1 ? running[0] : null,
      choices: this.choices(approval["allowedDecisions"]),
    };
    if (truthy(body["subject"])) body["subject_inferred"] = true;
    const ev = this.emit(`approval:${word(aid)}`, "approval", "waiting", "Allow this command?", payload["text"], body, `${method} [execApproval pending]`, p);
    this.posting = [method, ev];
  }

  /** An approval can report its outcome three times. The first closes the item. */
  private approvalResolved(aid: unknown, nativeKind: string, native: unknown, decision: unknown = null): void {
    const prev = this.open.get(`approval:${word(aid)}`);
    if (prev === undefined) return this.skip(`${nativeKind.split(" [")[0]}: an approval outcome that is already known`);
    // The resolved message does not say which way. The decision is the one the client sent.
    const chosenDecision = or(decision, this.decisions.get(aid) ?? null);
    const chosen = list(prev.body["choices"]).map(object).find((c) => c["native"] === chosenDecision);
    if (chosen === undefined) return this.noRule("an approval resolved, and which way is only in the wording");
    const allow = chosen["effect"] === "allow";
    this.emit(
      prev.item,
      "approval",
      allow ? "done" : "declined",
      prev.title,
      `Answered: ${word(chosen["label"])}`,
      { ...prev.body, answer: { choice: chosen["id"] } },
      nativeKind,
      native,
    );
    if (!allow && truthy(prev.body["subject"])) this.denied.add(prev.body["subject"]);
  }
}
