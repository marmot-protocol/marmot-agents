/**
 * The OpenClaw plugin's transport: it hands the core's prompts, answers and controls to OpenClaw as a
 * chat channel does, and sends what OpenClaw hands back to the core as agent events.
 *
 * Every call in both directions is written down as a {@link Line}, exactly as the recorder in
 * `tools/record/openclaw` writes it, and fed to the chat's {@link Translator}. The events it makes go to
 * the core with the chat they belong to, the prompts a turn responds to, and the requests whose effect
 * they show (D-028, D-058).
 *
 * Whose rules govern (Q-22, provisional): the core decides who may prompt and who may approve, first, as
 * for every harness (D-010). OpenClaw is configured to let in whatever the core sends, and when it asks
 * the plugin whether a sender may approve, the plugin says yes only for a sender the core has just
 * vouched for: one whose answer to that approval the core sent, or whose typed `/approve` the core
 * checked against the right to approve (D-032).
 *
 * A file OpenClaw delivers with a reply (a `MEDIA:` line in the agent's answer, or a payload's `mediaUrl`
 * and `mediaUrls`) goes to the core as a `send_file` request, one for each local file, after the reply's
 * text, which reaches the chat as the agent's words (D-090). The core decides whether it may be sent, from
 * the folders its settings allow, and shows a refusal in the chat itself.
 *
 * OpenClaw itself is reached through a {@link Port}, so tests can run the transport without it.
 */

import { homedir } from "node:os";
import { isAbsolute, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

import type { AgentEvent, Line } from "./translate.ts";
import { Translator } from "./translate.ts";
import { Refusal, type Capabilities } from "./socket.ts";

type Json = Record<string, unknown>;

/** What the plugin calls in OpenClaw. `src/index.ts` gives the real one. */
export interface Port {
  /** Which agent and which session a chat belongs to. */
  route(peer: { kind: "group"; id: string }): Promise<Json>;
  /** Whether this sender may run the agent here, by the channel's own settings. */
  ingress(params: Json): Promise<Json>;
  /** The message as OpenClaw will see it. */
  context(params: Json, ingress: Json): Promise<unknown>;
  /** Runs the message. It resolves when the turn it starts ends. */
  run(turn: RunInput): Promise<unknown>;
  /** Answers an approval, as a press does. */
  resolveApproval(params: Json): Promise<unknown>;
  /** Answers a question, as a press does. */
  resolveQuestion(params: Json): Promise<unknown>;
  /** The stored entry of a session. */
  sessionEntry(params: { agentId: unknown; sessionKey: unknown }): Promise<unknown>;
  /** Whether a message is a command OpenClaw answers itself. */
  hasControlCommand(text: string): boolean;
  /** OpenClaw's typed commands, with the choices each one offers. */
  commands(): Promise<unknown>;
  /** The models an agent may use, and its default. */
  models(agentId: string): Promise<unknown>;
}

/** What `inbound.run` is handed, beside what OpenClaw's adapter needs from the runtime. */
export interface RunInput {
  messageId: string;
  text: string;
  chat: string;
  route: Json;
  ctx: unknown;
  dispatcherOptions: Json;
  replyOptions: Json;
  log: (...args: unknown[]) => void;
}

/** An event as it goes to the core: the translator's, with `chat`, `prompts` and `settles` added. */
export type CoreEvent = Omit<AgentEvent, "harness" | "seq"> & {
  chat: string;
  prompts?: string[];
  settles?: string[];
};

export interface ConnectorOptions {
  port: Port;
  /** The channel's id in OpenClaw, and the account. */
  channel: string;
  accountId: string;
  /** OpenClaw's release, for the session item (D-044). */
  version: string;
  /** Put in every id made up from a count, so no id is used twice across restarts. */
  boot: string;
  /** How many sessions each session key has named, as last saved. */
  generations?: Iterable<[string, number]>;
  /** Called when that count changes, to save it. */
  saveGenerations?: (generations: [string, number][]) => void;
  /** Sends one event to the core. */
  send: (event: CoreEvent) => void;
  /** Says what went wrong, by kind only (D-071). */
  warn?: (kind: string) => void;
  /**
   * Asks the core to send a file to a chat (D-090): resolves once it is sent, or throws the core's
   * {@link Refusal}. Without it, no file is sent.
   */
  sendFile?: (chat: string, path: string) => Promise<void>;
  /** The workspace of an OpenClaw agent, by its id, from which a relative path in a reply is taken. */
  workspace?: (agentId: string | undefined) => string | undefined;
}

/**
 * Every progress callback OpenClaw's reply options offer a channel. Each is written down and returns
 * nothing, which leaves OpenClaw's own handling as it was. The list is the recorder's: what the
 * recordings hold is what the translator was written against.
 */
export const REPLY_CALLBACKS = [
  "onPreparedAgentRunStart",
  "onAgentRunStart",
  "onAgentRunTerminalOutcome",
  "onFollowupQueueDisposition",
  "onVisibleWorkSessions",
  "onReplyStart",
  "onTypingCleanup",
  "onPartialReply",
  "onReasoningStream",
  "onReasoningProgress",
  "onReasoningEnd",
  "onAssistantMessageStart",
  "onBlockReplyQueued",
  "onToolResult",
  "onToolStart",
  "onItemEvent",
  "onNarrationUpdate",
  "onPlanUpdate",
  "onApprovalEvent",
  "onCommandOutput",
  "onPatchSummary",
  "onCompactionStart",
  "onCompactionEnd",
  "onModelSelected",
  "onQueuedFollowupAdmitted",
  "onQueuedFollowupSettled",
  "onObservedReplyDelivery",
];

// ------------------------------------------------------------------------------------------------
// What a call holds, as JSON. This is the recorder's `jsonable`, with its limits taken out: the recorder
// keeps at most 200 entries of a collection and 8 levels of nesting, which a live payload must not lose
// (D-059). The core alone cuts an event that is too large (D-057).

const SECRET_KEY = /api[-_]?key|token|secret|password|authorization|credential/i;

function looksLikeConfig(v: Json): boolean {
  try {
    return "gateway" in v && ("agents" in v || "models" in v || "channels" in v);
  } catch {
    return true;
  }
}

/**
 * Keeps everything JSON can hold, and gives what it cannot a stand-in: a number that is not finite as
 * text, a function or a promise as a marker, bytes as base64, a value met again inside itself as
 * `<repeated>`. Never keeps the configuration, or a string under a key that names a credential.
 */
export function jsonable(v: unknown, seen = new Set<unknown>()): unknown {
  if (v === null || v === undefined) return null;
  const t = typeof v;
  if (t === "string" || t === "boolean") return v;
  if (t === "number") return Number.isFinite(v) ? v : String(v);
  if (t === "bigint") return String(v);
  if (t === "function") return `<function ${(v as { name?: string }).name || "anonymous"}>`;
  if (t === "symbol") return String(v);
  const o = v as object;
  if (seen.has(o)) return "<repeated>";
  if (o instanceof Error) return { _error: o.name, message: String(o.message) };
  if (o instanceof Date) return Number.isNaN(o.getTime()) ? String(o) : o.toISOString();
  if (o instanceof Uint8Array) return { _bytes: o.length, base64: Buffer.from(o.buffer, o.byteOffset, o.byteLength).toString("base64") };
  if (typeof AbortSignal !== "undefined" && o instanceof AbortSignal) return { _abort_signal: o.aborted };
  if (o instanceof Promise) return "<promise>";
  seen.add(o);
  try {
    if (Array.isArray(o)) return o.map((x) => jsonable(x, seen));
    if (o instanceof Map) return jsonable(Object.fromEntries(o.entries()), seen);
    if (o instanceof Set) return jsonable([...o], seen);
    if (looksLikeConfig(o as Json)) return "<config>";
    const out: Json = {};
    for (const [k, x] of Object.entries(o)) {
      if (x === undefined) continue;
      if (SECRET_KEY.test(k) && typeof x === "string") out[k] = "<redacted>";
      else if (k === "cfg" || k === "config") out[k] = x && typeof x === "object" ? "<config>" : jsonable(x, seen);
      else out[k] = jsonable(x, seen);
    }
    return out;
  } finally {
    seen.delete(o);
  }
}

function object(value: unknown): Json {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Json) : {};
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/** The media a delivery's payload carries, each once, in order: `mediaUrl`, then `mediaUrls`. */
function mediaOf(payload: unknown): string[] {
  const p = object(payload);
  const all = [p["mediaUrl"], ...(Array.isArray(p["mediaUrls"]) ? p["mediaUrls"] : [])];
  return [...new Set(all.filter((u): u is string => typeof u === "string" && u.trim() !== ""))];
}

/**
 * The media of a reply as the agent named it. Before it hands a reply to the channel, OpenClaw copies each
 * local file into its outbound media store and names the copy, `<state>/media/outbound/<name>---<id>`, in
 * the payload; the core sends only from the folders it allows, which never hold that store. OpenClaw keeps
 * the path the agent named in its record of the payload (`replyMediaSourceUrls`, keyed by the copy), which
 * the plugin SDK does not export: read on 2026-11-22 in release 2026.11.5's bundled code
 * (`src/auto-reply/reply/reply-media-paths.ts`). A copy with no source recorded goes as it is named, and the
 * core refuses it unless its folder is allowed.
 */
function sourcesOf(payload: unknown, media: string[]): string[] {
  const record = (globalThis as Record<symbol, unknown>)[Symbol.for("openclaw.replyPayloadMetadata")];
  if (!(record instanceof WeakMap) || typeof payload !== "object" || payload === null) return media;
  const sources = object(record.get(payload))["replyMediaSourceUrls"];
  if (!(sources instanceof Map)) return media;
  const named = media.map((url) => {
    const from: unknown = sources.get(url) ?? (isAbsolute(url) ? sources.get(normalize(url)) : undefined);
    const first = Array.isArray(from) ? from.find((u): u is string => typeof u === "string" && u.trim() !== "") : undefined;
    return first ?? url;
  });
  return [...new Set(named)];
}

function push(map: Map<string, string[]>, key: string, value: string): void {
  map.set(key, [...(map.get(key) ?? []), value]);
}

/** A request that the first event passing `test` settles. */
interface Settle {
  request: string;
  test: (event: AgentEvent) => boolean;
  done: boolean;
}

/** An approval or a question that waits, as its event showed it. */
interface Asking {
  chat: string;
  kind: "approval" | "question";
  /** OpenClaw's id for it. */
  id: string;
  /** The native value of each choice or option, by its id. */
  natives: Map<string, unknown>;
}

/** The setting commands a `set` control is typed as. */
const SET_COMMANDS = new Map([
  ["model", "/model"],
  ["think", "/think"],
]);

/** One chat: its translator, and what waits for an effect. */
class Chat {
  readonly translator: Translator;
  readonly id: string;
  /** The chat's name, as the last prompt gave it. */
  name = "";
  /** OpenClaw's route for it, once asked. */
  route: Json | null = null;
  /** The command and model lists have been asked for. */
  catalogued = false;
  /** Runs of `inbound.run` still going. */
  running = 0;
  /** People's messages handed over and not yet claimed by a turn. */
  unclaimed: string[] = [];
  readonly turns = new Set<string>();
  /** Requests settled by the next event of an item. */
  readonly byItem = new Map<string, string[]>();
  /** Interrupts, settled by the event that ends the turn. */
  readonly byTurn = new Map<string, string[]>();
  /** New sessions, settled by the next session event. */
  nextSession: string[] = [];
  constructor(id: string, translator: Translator) {
    this.id = id;
    this.translator = translator;
  }
}

export class Connector {
  private readonly chats = new Map<string, Chat>();
  private readonly options: ConnectorOptions;
  private readonly generations: Map<string, number>;
  /** Approvals and questions that wait, by item. */
  private readonly asking = new Map<string, Asking>();
  /** Items answered through this plugin. */
  private readonly answered = new Set<string>();
  /** Approvals and questions whose answer OpenClaw has not yet confirmed. */
  private readonly pressing = new Map<string, Asking>();
  /** Approval and question items an event has named. */
  private readonly seen = new Set<string>();
  /** Senders the core has vouched for as approvers, with how many reasons are still open. */
  private readonly vouched = new Map<string, number>();
  private count = 0;
  /** Cancels every run this connection started, once it closes. */
  private readonly abort = new AbortController();
  /** Runs of `inbound.run` still going. */
  private readonly runs = new Set<Promise<unknown>>();
  /** Requests from the core still being served: a prompt or a control may not have started its run yet. */
  private readonly serving = new Set<Promise<void>>();
  /** The connection has closed: nothing more goes to the core. */
  private closed = false;

  constructor(options: ConnectorOptions) {
    this.options = options;
    this.generations = new Map(options.generations ?? []);
  }

  /** Serves one request from the core. Resolves once it is taken; throws a {@link Refusal}. */
  async serve(method: string, params: Json): Promise<void> {
    if (this.closed) throw new Refusal("failed", "The connection to the core closed.");
    const served =
      method === "prompt" ? this.prompt(params)
      : method === "answer" ? this.answer(params)
      : method === "control" ? this.control(params)
      : Promise.reject(new Refusal("not_supported"));
    this.serving.add(served);
    try {
      return await served;
    } finally {
      this.serving.delete(served);
    }
  }

  /**
   * OpenClaw asks whether a sender may approve. Only one the core vouched for may: the core alone
   * decides who may approve (D-010), and it has checked the sender by the time it vouches.
   */
  mayApprove(senderId: unknown): { authorized: boolean; reason?: string } {
    if (typeof senderId === "string" && (this.vouched.get(senderId) ?? 0) > 0) return { authorized: true };
    return { authorized: false, reason: "Approvals are answered in the Marmot chat, by those its owner allows." };
  }

  /**
   * A call OpenClaw made on one of the plugin's adapters, outside any run: an approval to show, or a
   * message to post. It is fed to the chat it names.
   */
  adapterCall(method: string, params: unknown, result: unknown): void {
    const p = object(params);
    const to = text(p["to"]) || text(object(p["target"])["to"]) || text(object(p["params"])["to"]);
    const chat = to === "" ? undefined : this.chats.get(to);
    if (chat === undefined) {
      this.warn(`a call for no chat this plugin knows (${method})`);
      return;
    }
    this.feed(chat, { dir: "in", via: "hook", msg: { method, params: jsonable(params) } });
    this.feed(chat, { dir: "out", via: "result", msg: { method, result: jsonable(result) } });
  }

  /**
   * The connection to the core has closed, and the core has ended every turn on it. Cancels the runs
   * this connection started, lets no request still being served start one, and resolves once every
   * request and run is over: a run of a later connection must not share OpenClaw's session with one of
   * these, so the plugin does not connect again until then. A run still going after `wait` ms is
   * reported, and waited for, unless `untilOver` is false, as when OpenClaw is stopping the plugin.
   * Nothing more is sent.
   */
  async stopped(wait = 10_000, untilOver = true): Promise<void> {
    this.closed = true;
    this.abort.abort(new Error("the connection to the core closed"));
    for (const chat of this.chats.values()) {
      this.feed(chat, { dir: "in", via: "hook", msg: { method: "gateway.abort", params: { accountId: this.options.accountId } } });
    }
    // A request being served may have been about to start a run; it starts none now, but is waited for.
    const over = (async () => {
      while (this.serving.size > 0 || this.runs.size > 0) await Promise.allSettled([...this.serving, ...this.runs]);
      return false;
    })();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const late = new Promise<boolean>((resolve) => (timer = setTimeout(() => resolve(true), wait)));
    if (await Promise.race([over, late])) {
      this.warn("a run went on after it was cancelled");
      if (untilOver) await over;
    }
    clearTimeout(timer);
  }

  // -- prompts

  private async prompt(params: Json): Promise<void> {
    const message = text(params["message"]);
    const said = text(params["text"]);
    const c = object(params["chat"]);
    const chatId = text(c["id"]);
    if (message === "" || said === "" || chatId === "") throw new Refusal("invalid");
    const chat = this.chat(chatId);
    chat.name = text(c["name"]) || chat.name;
    const sender = object(params["sender"]);
    // The core sends a typed approval only from someone with the right to approve (D-032), and OpenClaw
    // will ask whether the sender may approve while the message runs.
    const approves = /^\s*\/approve(\s|$)/.test(said);
    chat.unclaimed.push(message);
    try {
      await this.inbound(chat, {
        messageId: message,
        text: said,
        sender: { id: text(sender["id"]), name: text(sender["name"]) },
        replyTo: params["reply_to"] === undefined || params["reply_to"] === null ? null : object(params["reply_to"]),
        prompt: true,
        vouch: approves ? text(sender["id"]) : null,
      });
    } catch (e) {
      // The run never started, so no turn will respond to the message.
      chat.unclaimed = chat.unclaimed.filter((m) => m !== message);
      throw e;
    }
  }

  /**
   * Hands one message to OpenClaw, as a channel does: the route, the verdict on the sender, the
   * context, and the run. Resolves once the run has started; the run goes on.
   */
  private async inbound(
    chat: Chat,
    m: {
      messageId: string;
      text: string;
      sender: { id: string; name: string };
      replyTo: Json | null;
      /** A person's message, which a turn responds to; not a control typed by the plugin. */
      prompt: boolean;
      vouch: string | null;
      after?: () => void;
      /** Where the run's notices are kept, instead of being sent, until `after` sends them. */
      hold?: AgentEvent[];
    },
  ): Promise<void> {
    const { channel, accountId, port } = this.options;
    const kind = "group";
    this.feed(chat, {
      dir: "out",
      via: "inbound",
      msg: { message_id: m.messageId, chat: chat.id, user: m.sender.id, user_name: m.sender.name, text: m.text },
    });
    // Every chat is a group to OpenClaw, so each has a session of its own: OpenClaw puts every direct
    // chat in one session otherwise.
    const peer = { kind, id: chat.id } as const;
    const route = await this.call(chat, "routing.resolveAgentRoute", { channel, accountId, peer }, () => port.route(peer));
    chat.route = route;
    if (!chat.catalogued) {
      // The session item lists the commands and the models (D-029), as the recorder asks for them.
      chat.catalogued = true;
      const agentId = text(route["agentId"]) || "main";
      await this.call(chat, "native-command-registry.listChatCommands", {}, () => port.commands()).catch(() => undefined);
      await this.call(chat, "models-provider-runtime.buildModelsProviderData", { agentId }, () => port.models(agentId)).catch(() => undefined);
    }
    const controlCommand = port.hasControlCommand(m.text);
    const ingressParams: Json = {
      channelId: channel,
      accountId,
      subject: { stableId: m.sender.id },
      conversation: { kind, id: chat.id },
      contextBinding: { agentId: route["agentId"], sessionKey: route["sessionKey"], messageId: m.messageId, inboundEventKind: "user_request" },
      event: { kind: "message", authMode: "inbound", mayPair: false },
      // The core has decided already: OpenClaw lets in whatever it sends.
      dmPolicy: "open",
      groupPolicy: "open",
      policy: {
        groupAllowFromFallbackToAllowFrom: true,
        activation: { requireMention: false, allowTextCommands: true },
      },
      allowFrom: ["*"],
      groupAllowFrom: ["*"],
      mentionFacts: { canDetectMention: true, wasMentioned: true },
      command: { allowTextCommands: true, hasControlCommand: controlCommand },
    };
    const ingress = await this.call(chat, "inbound.ingress.resolveStable", ingressParams, () => port.ingress(ingressParams));
    const verdict = object(ingress["ingress"]);
    if (verdict["admission"] !== "dispatch") {
      this.warn(`OpenClaw did not let a message in (${text(verdict["reasonCode"]) || "no reason"})`);
      throw new Refusal("failed", "OpenClaw did not take the message.");
    }
    const contextParams: Json = {
      channel,
      accountId,
      messageId: m.messageId,
      timestamp: Date.now(),
      // For a group, OpenClaw reads the group's identity out of "from"; the person is in "sender".
      from: `${channel}:group:${chat.id}`,
      sender: { id: m.sender.id, name: m.sender.name },
      conversation: { kind, id: chat.id, label: chat.name || chat.id },
      route: { agentId: route["agentId"], accountId: route["accountId"], routeSessionKey: route["sessionKey"] },
      reply: { to: chat.id, ...(m.replyTo !== null ? { replyToId: text(m.replyTo["message"]) } : {}) },
      message: { rawBody: m.text, bodyForAgent: m.text },
      access: {
        commands: { authorized: object(ingress["commandAccess"])["authorized"] === true },
        mentions: { canDetectMention: true, wasMentioned: true, requireMention: false },
      },
      ...(m.replyTo !== null
        ? {
            supplemental: {
              quote: {
                id: text(m.replyTo["message"]),
                body: text(m.replyTo["text"]),
                sender: text(object(m.replyTo["author"])["name"]),
                isQuote: true,
              },
            },
          }
        : {}),
    };
    const ctx = await this.call(chat, "inbound.buildContext", contextParams, () => port.context(contextParams, ingress));

    const from = m.prompt ? m.messageId : null;
    const hold = m.hold ?? null;
    // Every line a callback of this run makes names the run, so a reply and its effect are this
    // message's, however the runs of a chat overlap.
    const run = m.messageId;
    const tap =
      (method: string, returns: (...args: unknown[]) => unknown = () => undefined) =>
      (...args: unknown[]): unknown => {
        this.feed(chat, { dir: "in", via: "hook", msg: { method, params: jsonable(args.length === 1 ? args[0] : args) }, run }, from, hold);
        const out = returns(...args);
        if (out instanceof Promise) {
          return out.then((value) => {
            this.feed(chat, { dir: "out", via: "result", msg: { method, result: jsonable(value) }, run }, from, hold);
            return value;
          });
        }
        this.feed(chat, { dir: "out", via: "result", msg: { method, result: jsonable(out) }, run }, from, hold);
        return out;
      };
    const replyOptions: Json = { sourceReplyDeliveryMode: "automatic", disableBlockStreaming: false, abortSignal: this.abort.signal };
    for (const name of REPLY_CALLBACKS) replyOptions[name] = tap(name);
    replyOptions["onTypingController"] = (typing: unknown) =>
      this.feed(chat, { dir: "in", via: "hook", msg: { method: "onTypingController", params: { methods: Object.keys(object(typing)) } } }, from);
    const dispatcherOptions: Json = {
      // Nothing is posted here: what OpenClaw delivers reaches the chat as the events it becomes.
      // The files a reply carries go to the core once its text is on its way (D-090).
      deliver: tap("deliver", async (payload: unknown, info: unknown) => {
        if (text(object(info)["kind"]) !== "tool") await this.sendFiles(chat.id, chat.route, sourcesOf(payload, mediaOf(payload)));
        return { messageId: this.receipt() };
      }),
      onError: tap("dispatcher.onError"),
      onSkip: tap("dispatcher.onSkip"),
      onIdle: tap("dispatcher.onIdle"),
      onReplyStart: tap("dispatcher.onReplyStart"),
      onSettled: tap("dispatcher.onSettled"),
      onCleanup: tap("dispatcher.onCleanup"),
      onDeliverySettled: tap("dispatcher.onDeliverySettled"),
      typingCallbacks: {
        onReplyStart: tap("typing.onReplyStart", async () => undefined),
        onIdle: tap("typing.onIdle"),
        onCleanup: tap("typing.onCleanup"),
      },
    };

    // The connection closed while the message was on its way: it does not run.
    if (this.closed) throw new Refusal("failed", "The connection to the core closed.");
    this.feed(chat, { dir: "out", via: "call", msg: { method: "inbound.run", params: { messageId: m.messageId } } }, from);
    if (m.vouch) this.vouch(m.vouch, +1);
    chat.running += 1;
    const running = port.run({
      messageId: m.messageId,
      text: m.text,
      chat: chat.id,
      route,
      ctx,
      dispatcherOptions,
      replyOptions,
      log: tap("inbound.log"),
    });
    this.runs.add(running);
    void running.finally(() => this.runs.delete(running)).catch(() => undefined);
    void running.then(
      (result) => {
        const r = object(result);
        const msg = { method: "inbound.run", result: jsonable({ admission: r["admission"], dispatched: r["dispatched"], dispatchResult: r["dispatchResult"] }) };
        this.ran(chat, m, () => this.feed(chat, { dir: "in", via: "call", msg, run }, from, hold));
      },
      () => {
        this.warn("OpenClaw's run of a message failed");
        this.ran(chat, m, () => {
          this.feed(chat, { dir: "in", via: "call", msg: { method: "inbound.run", error: { _error: "Error" } }, run }, from, hold);
          // Nothing more will come for a turn this run held, unless another run in the chat holds it.
          if (chat.running === 0 && chat.translator.currentTurn !== null) this.dispatch(chat, chat.translator.stopped("the run failed"));
        });
      },
    );
  }

  private ran(chat: Chat, m: { vouch: string | null; after?: () => void }, then: () => void): void {
    chat.running -= 1;
    if (m.vouch) this.vouch(m.vouch, -1);
    then();
    m.after?.();
  }

  /** Calls OpenClaw, writing down the call and what it returned. */
  private async call(chat: Chat, method: string, params: Json, fn: () => Promise<unknown>): Promise<Json> {
    this.feed(chat, { dir: "out", via: "call", msg: { method, params: jsonable(params) } });
    let value: unknown;
    try {
      value = await fn();
    } catch (e) {
      this.feed(chat, { dir: "in", via: "call", msg: { method, error: jsonable(e) } });
      this.warn(`OpenClaw refused a call (${method})`);
      throw new Refusal("failed");
    }
    this.feed(chat, { dir: "in", via: "call", msg: { method, result: jsonable(value) } });
    return object(value);
  }

  private vouch(sender: string, by: number): void {
    const n = (this.vouched.get(sender) ?? 0) + by;
    if (n > 0) this.vouched.set(sender, n);
    else this.vouched.delete(sender);
  }

  private receipt(): string {
    this.count += 1;
    return `${this.options.boot}-out-${this.count}`;
  }

  // -- approvals and questions

  private async answer(params: Json): Promise<void> {
    const request = text(params["request"]);
    const item = text(params["item"]);
    const given = object(params["given"]);
    const by = object(params["by"]);
    const senderId = text(by["id"]);
    const asking = this.asking.get(item);
    if (asking === undefined) {
      if (this.answered.has(item) || this.pressing.has(item)) throw new Refusal("already_answered");
      if (this.seen.has(item)) throw new Refusal("not_waiting");
      throw new Refusal("unknown_item");
    }
    const chat = this.chats.get(asking.chat)!;
    if (asking.kind === "approval") {
      const decision = asking.natives.get(text(given["choice"]));
      if (decision === undefined) throw new Refusal("invalid");
      const press = {
        approvalId: asking.id,
        decision,
        approvalKind: "exec",
        channel: this.options.channel,
        accountId: this.options.accountId,
        senderId,
      };
      await this.press(chat, asking, item, request, press, "resolveApprovalOverGateway", senderId, () =>
        this.options.port.resolveApproval(press),
      );
      return;
    }
    // A press answers one option, as a button does.
    const replies = Array.isArray(given["questions"]) ? given["questions"].map(object) : [];
    const options = replies.length === 1 && Array.isArray(replies[0]!["options"]) ? (replies[0]!["options"] as unknown[]) : [];
    if (options.length !== 1 || text(replies[0]!["text"]) !== "") {
      throw new Refusal("not_supported", "Pick one of the options.");
    }
    const value = asking.natives.get(text(options[0]));
    if (value === undefined) throw new Refusal("invalid");
    const press = { questionId: asking.id, optionValue: value, senderId };
    await this.press(chat, asking, item, request, press, "questionGatewayRuntime.resolveOption", null, () =>
      this.options.port.resolveQuestion(press),
    );
  }

  /**
   * Presses a button on an approval or a question, and writes down the press and its outcome. The item
   * counts as answered once OpenClaw confirms, and only then does its next event settle the request: an
   * event that ends it meanwhile, such as the turn's end, is not this answer's effect. If the press fails,
   * the refusal settles the request, and the item waits again for another answer.
   */
  private async press(
    chat: Chat,
    asking: Asking,
    item: string,
    request: string,
    press: Json,
    method: string,
    vouch: string | null,
    fn: () => Promise<unknown>,
  ): Promise<void> {
    this.asking.delete(item);
    this.pressing.set(item, asking);
    this.feed(chat, { dir: "out", via: "press", msg: press });
    if (vouch !== null) this.vouch(vouch, +1);
    try {
      const result = await fn();
      const r = object(result);
      if (r["applied"] === false || (r["status"] !== undefined && r["status"] !== "answered")) {
        // OpenClaw no longer waits for it.
        this.feed(chat, { dir: "in", via: "call", msg: { method, result: jsonable(result) } });
        throw new Refusal("not_waiting");
      }
      this.answered.add(item);
      // Confirmed: the event this result makes shows the answer, and settles the request.
      if (request !== "") push(chat.byItem, item, request);
      this.feed(chat, { dir: "in", via: "call", msg: { method, result: jsonable(result) } });
    } catch (e) {
      if (e instanceof Refusal) throw e;
      // Still waiting, unless an event ended it meanwhile: another answer may yet be given.
      if (this.pressing.has(item)) this.asking.set(item, asking);
      this.feed(chat, { dir: "in", via: "call", msg: { method, error: jsonable(e) } });
      throw new Refusal("failed");
    } finally {
      this.pressing.delete(item);
      if (vouch !== null) this.vouch(vouch, -1);
    }
  }

  // -- controls

  private async control(params: Json): Promise<void> {
    const request = text(params["request"]);
    const by = object(params["by"]);
    const sender = { id: text(by["id"]), name: text(by["name"]) };
    const action = object(params["action"]);
    const type = action["type"];
    if (type === "interrupt") {
      const turn = text(action["turn"]);
      const chat = [...this.chats.values()].find((c) => c.translator.currentTurn === turn);
      if (chat === undefined || turn === "") throw new Refusal("nothing_to_do");
      if (request !== "") push(chat.byTurn, turn, request);
      return this.typed(chat, request, "/stop", sender, () => this.unsettle(chat.byTurn, turn, request));
    }
    if (type === "new_session") {
      const chatId = text(object(action["chat"])["id"]);
      if (chatId === "") throw new Refusal("invalid");
      const chat = this.chat(chatId);
      if (request !== "") chat.nextSession.push(request);
      return this.typed(chat, request, "/new", sender, () => {
        chat.nextSession = chat.nextSession.filter((r) => r !== request);
      });
    }
    if (type === "set") {
      const setting = text(action["setting"]);
      const value = action["value"];
      const session = text(action["session"]);
      const typed = SET_COMMANDS.get(setting);
      if (typed === undefined) throw new Refusal("not_supported");
      if (typeof value !== "string" || value === "" || /\s/.test(value)) throw new Refusal("invalid");
      const chat = [...this.chats.values()].find((c) => c.translator.currentSession === session);
      if (chat === undefined || session === "") throw new Refusal("nothing_to_do");
      // OpenClaw answers a setting command in words, and what it stored says whether it took it. The
      // command's own run settles the request, once it is over; nothing later does.
      const held: AgentEvent[] = [];
      return this.typed(chat, request, `${typed} ${value}`, sender, () => undefined, () => void this.setDone(chat, request, typed, setting, value, held), held);
    }
    throw new Refusal("not_supported");
  }

  /** A control, typed into the chat as OpenClaw takes it: a command from the person who sent it. */
  private async typed(
    chat: Chat,
    request: string,
    command: string,
    sender: { id: string; name: string },
    undo: () => void,
    after?: () => void,
    hold?: AgentEvent[],
  ): Promise<void> {
    try {
      await this.inbound(chat, {
        messageId: `${request || this.receipt()}`,
        text: command,
        sender,
        replyTo: null,
        prompt: false,
        vouch: null,
        after,
        hold,
      });
    } catch (e) {
      undo();
      throw e;
    }
  }

  /**
   * A setting command's run is over. If the stored entry now shows the value, the session's edit settles
   * the request. Otherwise the command's own answer does: a refusal, a value that was already set, or
   * words; and if OpenClaw sent none, as when it dropped the command, a notice the plugin makes.
   */
  private async setDone(chat: Chat, request: string, command: string, setting: string, value: string, held: AgentEvent[]): Promise<void> {
    const settle: Settle = {
      request,
      done: request === "",
      test: (e) => {
        if (e.type !== "session") return false;
        const shown = (Array.isArray(e.body["settings"]) ? e.body["settings"] : []).map(object).find((s) => s["id"] === setting);
        const now = text(shown?.["value"]);
        return now === value || (setting === "model" && now.endsWith(`/${value}`));
      },
    };
    await this.readEntry(chat, settle);
    settle.test = (e) => e.type === "notice";
    this.dispatch(chat, held, null, null, settle);
    if (!settle.done) {
      const body = { kind: "command_output" };
      this.dispatch(chat, chat.translator.notice("Command output", `OpenClaw did not answer ${command}.`, body), null, null, settle);
    }
  }

  /** Reads a chat's stored session entry, as the recorder's `sdk` step does. */
  private async readEntry(chat: Chat, settle: Settle | null = null): Promise<void> {
    const route = chat.route;
    if (route === null) return;
    const params = { agentId: route["agentId"], sessionKey: route["sessionKey"] };
    const method = "session-store-runtime.getSessionEntryAsync";
    this.feed(chat, { dir: "out", via: "call", msg: { method, params } });
    try {
      const entry = await this.options.port.sessionEntry(params);
      const result = jsonable(entry);
      this.feed(chat, { dir: "in", via: "call", msg: { method, result } }, null, null, settle);
    } catch (e) {
      this.feed(chat, { dir: "in", via: "call", msg: { method, error: jsonable(e) } });
    }
  }

  private unsettle(map: Map<string, string[]>, key: string, request: string): void {
    const left = (map.get(key) ?? []).filter((r) => r !== request);
    if (left.length > 0) map.set(key, left);
    else map.delete(key);
  }

  // -- events

  private chat(id: string): Chat {
    let chat = this.chats.get(id);
    if (chat === undefined) {
      const translator = new Translator({
        version: this.options.version,
        ids: `${this.options.boot}.${this.chats.size + 1}.`,
        generations: this.generations,
      });
      chat = new Chat(id, translator);
      this.chats.set(id, chat);
    }
    return chat;
  }

  /**
   * Feeds one line to a chat's translator. `from` is the person's message whose run made it; `hold`
   * keeps the notices it makes, and `settle` names a request the first fitting event settles.
   */
  private feed(chat: Chat, line: Line, from: string | null = null, hold: AgentEvent[] | null = null, settle: Settle | null = null): void {
    this.dispatch(chat, chat.translator.feed(line), from, hold, settle, line.run ?? null);
    let changed = false;
    for (const [key, n] of chat.translator.generations) {
      if (this.generations.get(key) !== n) {
        this.generations.set(key, n);
        changed = true;
      }
    }
    if (changed) this.options.saveGenerations?.([...this.generations]);
  }

  /**
   * Sends events to the core, naming the prompt a new turn responds to and the requests each settles.
   * `run` is the message whose run made them: a new session settles only the `/new` whose run it is.
   */
  private dispatch(
    chat: Chat,
    events: AgentEvent[],
    from: string | null = null,
    hold: AgentEvent[] | null = null,
    settle: Settle | null = null,
    run: string | null = null,
  ): void {
    if (this.closed) return;
    for (const event of events) {
      if (hold !== null && event.type === "notice") {
        hold.push(event);
        continue;
      }
      const settles: string[] = [];
      if (settle !== null && !settle.done && settle.test(event)) {
        settles.push(settle.request);
        settle.done = true;
      }
      const take = (map: Map<string, string[]>, key: string | undefined) => {
        if (key === undefined) return;
        const found = map.get(key);
        if (found !== undefined) {
          settles.push(...found);
          map.delete(key);
        }
      };
      take(chat.byItem, event.item);
      const open = event.state === "running" || event.state === "waiting";
      if (event.type === "turn" && !open) take(chat.byTurn, event.turn);
      const own = run !== null && chat.nextSession.includes(run);
      const answersNew = event.type === "notice" && ["/new", "/reset"].includes(chat.translator.commandAnswered(event.item) ?? "");
      // The run of a /new starts the session, or answers the command and starts none: either is all
      // there is of its effect.
      if (own && (event.type === "session" || answersNew)) {
        settles.push(run);
        chat.nextSession = chat.nextSession.filter((r) => r !== run);
      }
      if (event.type === "approval" || event.type === "question") {
        this.seen.add(event.item);
        if (event.state === "waiting") this.remember(chat, event);
        else {
          this.asking.delete(event.item);
          this.pressing.delete(event.item);
        }
      }
      let prompts: string[] = [];
      if (event.type === "turn" && event.turn !== undefined && !chat.turns.has(event.turn)) {
        chat.turns.add(event.turn);
        // The run that started the turn says which message it was. A turn OpenClaw started from no
        // run of a message responds to every message not yet claimed.
        prompts = from !== null ? [from] : chat.unclaimed;
        chat.unclaimed = chat.unclaimed.filter((m) => !prompts.includes(m));
      }
      const { harness: _h, seq: _s, ...rest } = event;
      const out: CoreEvent = { ...rest, chat: chat.id };
      if (prompts.length > 0) out.prompts = prompts;
      if (settles.length > 0) out.settles = settles;
      this.options.send(out);
    }
  }

  /** Keeps what an answer to a waiting approval or question needs. */
  private remember(chat: Chat, event: AgentEvent): void {
    const natives = chat.translator.valuesOf(event.item);
    const id = event.item.slice(event.item.indexOf(":") + 1);
    this.asking.set(event.item, { chat: chat.id, kind: event.type as "approval" | "question", id, natives });
  }

  /**
   * Media OpenClaw sends to a chat through the channel's outbound adapter, outside a run. Its caption goes
   * first, as the agent's words, then its file to the core. Resolves with the receipt only once the core
   * has sent the file; throws when the core refused it or cannot be asked, or the file is not on this
   * machine, so OpenClaw counts the send as failed.
   */
  async sendMedia(params: unknown, receipt: () => Json): Promise<Json> {
    const method = "message.send.media";
    const p = object(params);
    const to = text(p["to"]).replace(/^marmot:/, "");
    if (to === "") throw new Error("marmot: media for no chat");
    const chat = this.chat(to);
    this.feed(chat, { dir: "in", via: "hook", msg: { method, params: jsonable(params) } });
    const failure = await this.sendFiles(chat.id, chat.route, mediaOf(p));
    if (failure !== null) throw failure;
    const result = receipt();
    this.feed(chat, { dir: "out", via: "result", msg: { method, result: jsonable(result) } });
    return result;
  }

  /**
   * Asks the core to send each file, in turn, and gives back the first failure, or null when every file
   * went. A refusal or a failure is said by kind only: the core shows its own notice in the chat, naming
   * no path, and the next file still goes.
   */
  private async sendFiles(chat: string, route: Json | null, media: string[]): Promise<Error | null> {
    if (media.length === 0) return null;
    const send = this.options.sendFile;
    if (send === undefined || this.closed) {
      this.warn("a file was not sent: the core cannot be asked");
      return new Refusal("failed", "The agent is not connected to its core.");
    }
    let failure: Error | null = null;
    for (const url of media) {
      const path = this.localPath(url, route);
      if (path === null) {
        this.warn("a file not on this machine was not sent");
        failure ??= new Refusal("invalid", "The file is not on this machine.");
        continue;
      }
      try {
        await send(chat, path);
      } catch (e) {
        this.warn(`a file was not sent (${e instanceof Refusal ? e.reason : "failed"})`);
        failure ??= e instanceof Error ? e : new Refusal("failed");
      }
    }
    return failure;
  }

  /**
   * A local file's path, whole, as the core takes it: a `file://` address, a whole path, one from the home
   * (`~/`), or one relative to the agent's workspace, as OpenClaw reads them. A `..` is left for the core to
   * follow. Anything else, such as an address on the web or a stored inbound attachment, is not a file here.
   */
  private localPath(url: string, route: Json | null): string | null {
    let path: string;
    if (url.startsWith("file://")) {
      try {
        path = fileURLToPath(url);
      } catch {
        return null;
      }
    } else if (/^[a-z][a-z0-9+.-]*:/i.test(url)) {
      return null;
    } else {
      path = url;
    }
    if (path === "~" || path.startsWith("~/")) return homedir() + path.slice(1);
    if (isAbsolute(path)) return path;
    const agentId = route === null ? undefined : text(route["agentId"]) || undefined;
    const workspace = this.options.workspace?.(agentId);
    if (workspace === undefined || !isAbsolute(workspace)) return null;
    return workspace.endsWith(sep) ? workspace + path : workspace + sep + path;
  }

  private warn(kind: string): void {
    this.options.warn?.(kind);
  }
}

/**
 * What OpenClaw can do, as the core and the apps read it (D-058). Provisional, for the owner's review.
 */
export const CAPABILITIES: Capabilities = {
  // `/stop` ends a turn.
  interrupt: true,
  // A message sent during a turn waits, with no notice, and runs as a turn of its own (D-040).
  mid_turn: "queue",
  // OpenClaw routes each chat to its own session.
  sessions: "own",
  // OpenClaw keeps its sessions in its own store, which outlives it.
  resume: true,
  // The plugin claims OpenClaw's approval capability, and the owner switches forwarding on.
  approvals: true,
  files_in: "none",
  // Typed commands that answer an approval or widen what the agent may do without asking
  // (`docs/design/answers-and-controls.md`). OpenClaw honours a directive anywhere in a message, so each
  // of these counts anywhere, except the commands only an owner may use and those that start a message.
  approval_commands: [
    { command: "/approve", counts: "anywhere" },
    { command: "/exec", counts: "anywhere" },
    { command: "/elevated", counts: "anywhere" },
    { command: "/elev", counts: "anywhere" },
    { command: "/config", counts: "start" },
    { command: "/debug", counts: "start" },
    { command: "/bash", counts: "start" },
    { command: "!", counts: "start" },
    { command: "/allowlist", counts: "start" },
    { command: "/acp", counts: "start" },
    { command: "/codex", counts: "start" },
    { command: "/mcp", counts: "start" },
    { command: "/plugins", counts: "start" },
    // Each by every name OpenClaw's command registry gives it, since OpenClaw runs a command by any of
    // them (D-150).
    { command: "/plugin", counts: "start" },
  ],
};
