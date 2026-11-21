/**
 * The Claude Code driver's transport: it runs Claude Code through the Claude Agent SDK, one session per
 * chat, and serves the core's `prompt`, `answer` and `control` requests.
 *
 * Every message the SDK yields, every permission request it makes through `canUseTool`, and everything
 * this side sends is recorded as a {@link Line}, exactly as `tools/record/claude/record_claude.ts`
 * records it, and fed to the chat's {@link Translator}. The events it makes go to the core with the chat
 * they belong to, the prompts a turn responds to, and the requests whose effect they show (D-028, D-058).
 *
 * Every query is given the connector's own tool, `send_file` (`files.ts`), when the connector can ask the
 * core to send a file: Claude Code calls it, and the connector asks the core, which decides (D-090).
 *
 * The SDK's `query` is a parameter, so tests can run the transport without Claude Code.
 */

import type { AgentEvent, Line } from "./translate.ts";
import { Translator } from "./translate.ts";
import { readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { fileServer, SEND_FILE_TOOL, type FileServer } from "./files.ts";
import { CoreConnection, MAX_MESSAGE, notificationSize, Refusal, type Capabilities, type Hello } from "./socket.ts";

type Json = Record<string, unknown>;

/** A user message, as the SDK takes it on its streaming input. */
export interface UserMessage {
  type: "user";
  message: { role: "user"; content: string };
  parent_tool_use_id: null;
  priority?: "now" | "next" | "later";
}

/** What `canUseTool` resolves to. */
export type PermissionResult =
  | { behavior: "allow"; updatedInput?: Json; updatedPermissions?: unknown[] }
  | { behavior: "deny"; message: string };

export type CanUseTool = (
  toolName: string,
  input: Json,
  options: { signal: AbortSignal } & Json,
) => Promise<PermissionResult>;

/** The part of the SDK's `Query` the transport uses. */
export interface QueryLike extends AsyncIterable<unknown> {
  interrupt(): Promise<unknown>;
  setPermissionMode(mode: string): Promise<void>;
  setModel(model?: string): Promise<void>;
  supportedModels?(): Promise<unknown>;
  close(): void;
}

/** The SDK's `query`, or a stand-in for it. */
export type QueryFn = (params: { prompt: AsyncIterable<UserMessage>; options: Json }) => QueryLike;

/** An event as it goes to the core: the translator's, with `chat`, `prompts` and `settles` added. */
export type CoreEvent = Omit<AgentEvent, "harness" | "seq"> & {
  chat: string;
  prompts?: string[];
  settles?: string[];
};

export interface ConnectorOptions {
  query: QueryFn;
  /** The folder Claude Code works in. */
  cwd: string;
  model?: string;
  permissionMode?: string;
  /** Sends one event to the core. */
  send: (event: CoreEvent) => void;
  /**
   * Asks the core to send a file to a chat (D-090): resolves once it is sent, or throws the core's
   * {@link Refusal}. Without it, Claude Code is not given the tool.
   */
  sendFile?: (chat: string, path: string) => Promise<void>;
  /** Makes the MCP server that offers the tool; the SDK's own by default. */
  fileServer?: FileServer;
  /** Says what went wrong, by kind only (D-071). */
  warn?: (kind: string) => void;
}

const PERMISSION_MODES = new Set(["default", "acceptEdits", "bypassPermissions", "plan", "dontAsk", "auto"]);
const DECLINED = "The user declined this action.";

/** The SDK's streaming input: messages pushed in, read by the SDK in order until it is closed. */
class Inbox implements AsyncIterable<UserMessage> {
  private queue: UserMessage[] = [];
  private wake: (() => void) | undefined;
  private closed = false;
  push(m: UserMessage): void {
    this.queue.push(m);
    this.wake?.();
  }
  close(): void {
    this.closed = true;
    this.wake?.();
  }
  async *[Symbol.asyncIterator](): AsyncIterator<UserMessage> {
    for (;;) {
      while (this.queue.length > 0) yield this.queue.shift()!;
      if (this.closed) return;
      await new Promise<void>((r) => (this.wake = r));
      this.wake = undefined;
    }
  }
}

/** One `query()`: a Claude Code session, or its resumption. */
interface Run {
  q: QueryLike;
  inbox: Inbox;
  /** Set when the transport closed it, so nothing it still yields is fed. */
  closed: boolean;
}

/** A permission request that waits for a person. */
interface Waiting {
  item: string;
  kind: "approval" | "question";
  toolName: string;
  input: Json;
  requestId: unknown;
  suggestions: unknown[];
  resolve: (result: PermissionResult) => void;
}

/** A `set` control on its way. */
interface PendingSet {
  request: string;
  session: string;
  setting: "model" | "permission_mode";
  value: string;
}

/** One chat: its translator, its session, and what waits for an effect. */
class Chat {
  readonly translator = new Translator();
  run: Run | null = null;
  /** The session to resume when the next prompt starts a query: one that ended without being asked to. */
  resumable: string | null = null;
  /** The prompts taken since a turn last started. */
  prompts: string[] = [];
  readonly waiting = new Map<string, Waiting>();
  readonly answered = new Set<string>();
  /** Approval and question items this chat's events have named. */
  readonly seen = new Set<string>();
  readonly turns = new Set<string>();
  /** Requests settled by the next event of an item. */
  readonly byItem = new Map<string, string[]>();
  /** Interrupts, settled by the event that ends the turn. */
  readonly byTurn = new Map<string, string[]>();
  /** Settings asked for, each settled by the first session event that shows that setting at that value. */
  sets: PendingSet[] = [];
  /** New sessions with no session to end, settled by the next session item. */
  nextSession: string[] = [];
  /** Each session's settings, as its item last showed them. */
  readonly settings = new Map<string, Map<unknown, unknown>>();
  readonly id: string;
  constructor(id: string) {
    this.id = id;
  }
}

function object(value: unknown): Json {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Json) : {};
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/** The value of each setting a session item shows. */
function settingValues(body: Json): Map<unknown, unknown> {
  const values = new Map<unknown, unknown>();
  for (const s of Array.isArray(body["settings"]) ? body["settings"] : []) values.set(object(s)["id"], object(s)["value"]);
  return values;
}

function push(map: Map<string, string[]>, key: string, value: string): void {
  map.set(key, [...(map.get(key) ?? []), value]);
}

/**
 * What Claude Code reads for a chat message, as for Codex (D-062): in a group the sender's name comes
 * first, a reply first quotes the message it answers, and a slash command goes exactly as typed (D-031).
 */
export function promptText(params: Json): string {
  const said = text(params["text"]);
  if (said.startsWith("/")) return said;
  let out = "";
  const reply = params["reply_to"];
  if (reply !== undefined && reply !== null) {
    const r = object(reply);
    const quoted = `${text(object(r["author"])["name"])}: ${text(r["text"])}`;
    for (const line of quoted.split("\n")) out += `> ${line}\n`;
    out += "\n";
  }
  if (object(params["chat"])["group"] === true) out += `${text(object(params["sender"])["name"])}: `;
  return out + said;
}

export class Connector {
  private readonly chats = new Map<string, Chat>();
  /** The chat each session belongs to: every session seen since the connector started. */
  private readonly owners = new Map<string, string>();
  private readonly options: ConnectorOptions;

  constructor(options: ConnectorOptions) {
    this.options = options;
  }

  /** Serves one request from the core. Resolves once it is taken; throws a {@link Refusal}. */
  async serve(method: string, params: Json): Promise<void> {
    if (method === "prompt") return this.prompt(params);
    if (method === "answer") return this.answer(params);
    if (method === "control") return this.control(params);
    throw new Refusal("not_supported");
  }

  /** Ends every session's query, as the connector goes. */
  close(): void {
    for (const chat of this.chats.values()) {
      if (chat.run !== null) this.stop(chat.run);
      chat.run = null;
    }
  }

  // -- prompts

  private async prompt(params: Json): Promise<void> {
    const message = text(params["message"]);
    const chatId = text(object(params["chat"])["id"]);
    if (message === "" || chatId === "") throw new Refusal("invalid");
    const chat = this.chat(chatId);
    const session = typeof params["session"] === "string" ? params["session"] : null;
    // A session another chat has had is never taken into this one, open or closed (D-065).
    if (session !== null && (this.owners.get(session) ?? chatId) !== chatId) throw new Refusal("invalid");
    // A prompt names a session only to take it up again (D-065).
    if (session !== null && chat.run !== null && chat.translator.currentSession !== session) {
      this.endRun(chat, []);
    }
    if (chat.run === null) this.start(chat, session ?? chat.resumable);
    const run = chat.run!;
    const msg: UserMessage = {
      type: "user",
      message: { role: "user", content: promptText(params) },
      parent_tool_use_id: null,
    };
    // During a turn Claude Code ends it and starts another with the new message (D-040).
    if (chat.translator.currentTurn !== null) msg.priority = "now";
    chat.prompts.push(message);
    this.feed(chat, { dir: "out", via: "prompt", msg });
    run.inbox.push(msg);
  }

  /** Starts the chat's `query()`, resuming `resume` when it is given. */
  private start(chat: Chat, resume: string | null): void {
    const inbox = new Inbox();
    // No Claude Code settings file is read: an allow rule or a hook in the owner's own settings would
    // otherwise approve or run things in a chat without the chat's approval.
    // Summarized thinking, as `tools/record/claude/record_claude.ts` recorded it: without it Claude Code
    // sends its thinking blocks empty, and a turn shows no reasoning.
    const options: Json = {
      cwd: this.options.cwd,
      settingSources: [],
      thinking: { type: "adaptive", display: "summarized" },
    };
    if (resume === null) {
      if (this.options.model !== undefined) options["model"] = this.options.model;
      if (this.options.permissionMode !== undefined) options["permissionMode"] = this.options.permissionMode;
    } else {
      // A resumed session goes on as its item last showed it. One this process never saw keeps its
      // own settings: the startup ones are for fresh sessions only.
      const shown = chat.settings.get(resume);
      const model = shown?.get("model");
      const mode = shown?.get("permission_mode");
      if (typeof model === "string" && model !== "") options["model"] = model;
      if (typeof mode === "string" && mode !== "") options["permissionMode"] = mode;
      options["resume"] = resume;
      this.own(resume, chat);
    }
    chat.resumable = null;
    this.feed(chat, { dir: "out", via: "call", msg: { method: "query", options } });
    const canUseTool: CanUseTool = (toolName, input, opts) => this.permission(chat, run, toolName, input, opts);
    const q = this.options.query({ prompt: inbox, options: { ...options, ...this.fileTool(chat), canUseTool } });
    const run: Run = { q, inbox, closed: false };
    chat.run = run;
    void this.read(chat, run);
    if (typeof q.supportedModels === "function") {
      // The models a person may switch to. The account is never asked for.
      this.feed(chat, { dir: "out", via: "call", msg: { method: "supportedModels" } });
      q.supportedModels().then(
        (response) => {
          if (chat.run === run) this.feed(chat, { dir: "in", via: "call", msg: { method: "supportedModels", response } });
        },
        () => this.warn("the models could not be listed"),
      );
    }
  }

  /**
   * The options that give a query the tool to send a file, when the core can be asked. A relative path is
   * made whole against the working folder; the core takes only an absolute one. What the model reads back
   * names no path the core did not already have from it.
   */
  private fileTool(chat: Chat): Json {
    const sendFile = this.options.sendFile;
    if (sendFile === undefined) return {};
    const send = async (path: string) => {
      const whole = isAbsolute(path) ? path : join(this.options.cwd, path);
      try {
        await sendFile(chat.id, whole);
        return { text: "Sent.", failed: false };
      } catch (e) {
        const detail = e instanceof Refusal ? e.detail : undefined;
        return { text: detail === undefined ? "Not sent." : `Not sent: ${detail}`, failed: true };
      }
    };
    const server = (this.options.fileServer ?? fileServer)(send);
    return { mcpServers: { marmot: server }, allowedTools: [SEND_FILE_TOOL] };
  }

  private async read(chat: Chat, run: Run): Promise<void> {
    try {
      for await (const m of run.q) {
        if (run.closed) break;
        this.feed(chat, { dir: "in", via: "message", msg: m });
      }
    } catch (e) {
      if (!run.closed) {
        this.warn("Claude Code stopped with an error");
        this.feed(chat, { dir: "in", via: "call", msg: { method: "query", error: String((e as Error)?.message ?? e) } });
      }
    }
    if (chat.run === run && !run.closed) this.failed(chat, run);
  }

  /**
   * Claude Code went without being asked to: it crashed, or was killed. Its running turn ends cancelled,
   * and the next prompt takes the session up again, in a turn of its own.
   */
  private failed(chat: Chat, run: Run): void {
    this.stop(run);
    chat.run = null;
    chat.resumable = chat.translator.currentSession;
    this.dropWaiting(chat);
    this.dispatch(chat, chat.translator.stopped());
    // Nothing that run would have shown can come now: what still waits for it never settles, and the
    // prompts it took belong to no turn.
    chat.prompts = [];
    chat.byItem.clear();
    chat.byTurn.clear();
    chat.sets = [];
  }

  /** Closes a query: its input first, then the process. */
  private stop(run: Run): void {
    run.closed = true;
    run.inbox.close();
    try {
      run.q.close();
    } catch {
      this.warn("a query could not be closed");
    }
  }

  /** Ends the chat's query and its session, settling `settles` on the event that ends it. */
  private endRun(chat: Chat, settles: string[]): void {
    if (chat.run !== null) this.stop(chat.run);
    chat.run = null;
    chat.resumable = null;
    this.dropWaiting(chat);
    const events = chat.translator.closeSession();
    const ends = events.find((e) => e.type === "session");
    if (ends !== undefined) for (const r of settles) push(chat.byItem, ends.item, r);
    else chat.nextSession.push(...settles);
    this.dispatch(chat, events);
  }

  // -- approvals and questions

  private permission(chat: Chat, run: Run, toolName: string, input: Json, opts: { signal: AbortSignal } & Json): Promise<PermissionResult> {
    const { signal, ...rest } = opts;
    const requestId = rest["requestId"];
    const kind = toolName === "AskUserQuestion" ? "question" : "approval";
    const item = `${kind}:${requestId === undefined || requestId === null ? "None" : String(requestId)}`;
    return new Promise((resolve) => {
      if (chat.run !== run || run.closed) {
        resolve({ behavior: "deny", message: DECLINED });
        return;
      }
      const suggestions = Array.isArray(rest["suggestions"]) ? rest["suggestions"] : [];
      chat.waiting.set(item, { item, kind, toolName, input, requestId, suggestions, resolve });
      signal?.addEventListener("abort", () => {
        // The turn was interrupted: Claude Code no longer waits, and the turn's end closes the item.
        if (chat.waiting.get(item)?.resolve === resolve) chat.waiting.delete(item);
        resolve({ behavior: "deny", message: DECLINED });
      });
      this.feed(chat, { dir: "in", via: "callback", msg: { kind: "can_use_tool", toolName, input, options: rest } });
    });
  }

  private answer(params: Json): Promise<void> {
    const request = text(params["request"]);
    const item = text(params["item"]);
    const given = object(params["given"]);
    let chat: Chat | undefined;
    let waiting: Waiting | undefined;
    for (const c of this.chats.values()) {
      waiting = c.waiting.get(item);
      if (waiting !== undefined) {
        chat = c;
        break;
      }
    }
    if (chat === undefined || waiting === undefined) {
      const chats = [...this.chats.values()];
      if (chats.some((c) => c.answered.has(item))) throw new Refusal("already_answered");
      if (chats.some((c) => c.seen.has(item))) throw new Refusal("not_waiting");
      throw new Refusal("unknown_item");
    }
    const [result, choice] = waiting.kind === "question" ? this.replies(waiting, given) : this.choice(waiting, given);
    chat.waiting.delete(item);
    chat.answered.add(item);
    if (request !== "") push(chat.byItem, item, request);
    const msg: Json = { kind: "permission_result", toolName: waiting.toolName, result, requestId: waiting.requestId };
    if (choice !== undefined) msg["choice"] = choice;
    this.feed(chat, { dir: "out", via: "callback", msg });
    waiting.resolve(result);
    return Promise.resolve();
  }

  /** An approval's answer: allow once, allow with one of Claude Code's suggestions, or deny. */
  private choice(waiting: Waiting, given: Json): [PermissionResult, string] {
    const choice = given["choice"];
    const n = typeof choice === "string" && /^c(0|[1-9][0-9]*)$/.test(choice) ? Number(choice.slice(1)) : -1;
    const deny = waiting.suggestions.length + 1;
    if (n < 0 || n > deny) throw new Refusal("invalid");
    if (n === deny) return [{ behavior: "deny", message: DECLINED }, choice as string];
    const result: PermissionResult = { behavior: "allow", updatedInput: waiting.input };
    if (n > 0) result.updatedPermissions = [waiting.suggestions[n - 1]];
    return [result, choice as string];
  }

  /**
   * A question's answer, as AskUserQuestion takes it: `answers` maps each question's text to the label
   * chosen, several joined by ", ", or to what the person typed.
   */
  private replies(waiting: Waiting, given: Json): [PermissionResult, undefined] {
    if (!Array.isArray(given["questions"])) throw new Refusal("invalid");
    const asked = Array.isArray(waiting.input["questions"]) ? waiting.input["questions"] : [];
    const answers: Record<string, string> = {};
    for (const reply of given["questions"]) {
      const r = object(reply);
      const qid = text(r["question"]);
      const q = /^q(0|[1-9][0-9]*)$/.test(qid) ? object(asked[Number(qid.slice(1))]) : {};
      const question = q["question"];
      if (typeof question !== "string") throw new Refusal("invalid");
      const offered = Array.isArray(q["options"]) ? q["options"] : [];
      const parts: string[] = [];
      for (const o of Array.isArray(r["options"]) ? r["options"] : []) {
        const oid = typeof o === "string" ? o : "";
        const option = /^o(0|[1-9][0-9]*)$/.test(oid) ? object(offered[Number(oid.slice(1))]) : {};
        if (typeof option["label"] !== "string") throw new Refusal("invalid");
        parts.push(option["label"]);
      }
      if (typeof r["text"] === "string" && r["text"] !== "") parts.push(r["text"]);
      if (parts.length === 0) throw new Refusal("invalid");
      answers[question] = parts.join(", ");
    }
    return [{ behavior: "allow", updatedInput: { ...waiting.input, answers } }, undefined];
  }

  /** Requests that wait for a person, for a query that has gone: nobody can answer them now. */
  private dropWaiting(chat: Chat): void {
    for (const w of chat.waiting.values()) w.resolve({ behavior: "deny", message: DECLINED });
    chat.waiting.clear();
  }

  // -- controls

  private async control(params: Json): Promise<void> {
    const request = text(params["request"]);
    const action = object(params["action"]);
    const type = action["type"];
    if (type === "interrupt") return this.interrupt(request, text(action["turn"]));
    if (type === "new_session") {
      const chatId = text(object(action["chat"])["id"]);
      if (chatId === "") throw new Refusal("invalid");
      const chat = this.chat(chatId);
      this.endRun(chat, request === "" ? [] : [request]);
      return;
    }
    if (type === "set") return this.set(request, text(action["session"]), text(action["setting"]), action["value"]);
    throw new Refusal("not_supported");
  }

  private async interrupt(request: string, turn: string): Promise<void> {
    const chat = [...this.chats.values()].find((c) => c.run !== null && c.translator.currentTurn === turn);
    if (chat === undefined || turn === "") throw new Refusal("nothing_to_do");
    const run = chat.run!;
    if (request !== "") push(chat.byTurn, turn, request);
    this.feed(chat, { dir: "out", via: "call", msg: { method: "interrupt" } });
    try {
      const response = await run.q.interrupt();
      if (chat.run === run) this.feed(chat, { dir: "in", via: "call", msg: { method: "interrupt", response: response ?? null } });
    } catch {
      this.unsettle(chat.byTurn, turn, request);
      throw new Refusal("failed");
    }
  }

  private async set(request: string, session: string, setting: string, value: unknown): Promise<void> {
    if (setting !== "permission_mode" && setting !== "model") throw new Refusal("not_supported");
    if (typeof value !== "string" || value === "") throw new Refusal("invalid");
    if (setting === "permission_mode" && !PERMISSION_MODES.has(value)) throw new Refusal("invalid");
    const chat = [...this.chats.values()].find((c) => c.run !== null && c.translator.currentSession === session);
    if (chat === undefined || session === "") throw new Refusal("nothing_to_do");
    // A setting that already has the value would show no change, and so nothing would settle it.
    if (chat.settings.get(session)?.get(setting) === value) throw new Refusal("nothing_to_do");
    const run = chat.run!;
    const pending: PendingSet = { request, session, setting, value };
    if (request !== "") chat.sets.push(pending);
    const msg = setting === "model" ? { method: "setModel", model: value } : { method: "setPermissionMode", mode: value };
    this.feed(chat, { dir: "out", via: "call", msg });
    try {
      if (setting === "model") await run.q.setModel(value);
      else await run.q.setPermissionMode(value);
    } catch {
      chat.sets = chat.sets.filter((s) => s !== pending);
      throw new Refusal("failed");
    }
    if (chat.run !== run) return;
    this.feed(chat, { dir: "in", via: "call", msg: { method: msg.method, response: null } });
    // Claude Code took it. Show it now: between turns nothing else would until the next one starts.
    this.dispatch(chat, chat.translator.changed(setting, value));
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
      chat = new Chat(id);
      this.chats.set(id, chat);
    }
    return chat;
  }

  /** Records that `session` is `chat`'s, unless another chat had it first. */
  private own(session: string, chat: Chat): void {
    if (!this.owners.has(session)) this.owners.set(session, chat.id);
  }

  private feed(chat: Chat, line: Line): void {
    this.dispatch(chat, chat.translator.feed(line));
  }

  /** Sends events to the core, naming the prompts a new turn responds to and the requests each settles. */
  private dispatch(chat: Chat, events: AgentEvent[]): void {
    for (const event of events) {
      if (event.session !== undefined) this.own(event.session, chat);
      const settles: string[] = [];
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
      if (event.type === "session") {
        const values = settingValues(event.body);
        const shown = (s: PendingSet) => s.session === event.session && values.get(s.setting) === s.value;
        settles.push(...chat.sets.filter(shown).map((s) => s.request));
        chat.sets = chat.sets.filter((s) => !shown(s));
        if (event.session !== undefined) chat.settings.set(event.session, values);
        settles.push(...chat.nextSession);
        chat.nextSession = [];
      }
      if (event.type === "approval" || event.type === "question") chat.seen.add(event.item);
      let prompts: string[] = [];
      if (event.type === "turn" && !chat.turns.has(event.item) && event.body["started_by"] !== "agent") {
        chat.turns.add(event.item);
        prompts = chat.prompts;
        chat.prompts = [];
      }
      const { harness: _h, seq: _s, ...rest } = event;
      const out: CoreEvent = { ...rest, chat: chat.id };
      if (prompts.length > 0) out.prompts = prompts;
      if (settles.length > 0) out.settles = settles;
      this.options.send(out);
    }
  }

  private warn(kind: string): void {
    this.options.warn?.(kind);
  }
}

/** What is put where text was cut to fit the socket. */
const CUT = "\n[… cut to fit …]\n";
const CUT_BYTES = Buffer.byteLength(JSON.stringify(CUT)) - 2;

/** Whether the UTF-16 unit at `i` starts or ends a pair, which a cut must not split. */
function high(s: string, i: number): boolean {
  const c = s.charCodeAt(i);
  return c >= 0xd800 && c <= 0xdbff;
}
function low(s: string, i: number): boolean {
  const c = s.charCodeAt(i);
  return c >= 0xdc00 && c <= 0xdfff;
}

/** Where a string is: the object or list that holds it, and its key there. */
interface Spot {
  owner: Record<string | number, unknown>;
  key: string | number;
  length: number;
}

/** The longest string in `value`, or in `best` if that is longer. */
function longest(value: unknown, at: Spot | null = null): Spot | null {
  let best = at;
  const visit = (owner: Record<string | number, unknown>, key: string | number, v: unknown) => {
    if (typeof v === "string") {
      if (best === null || v.length > best.length) best = { owner, key, length: v.length };
    } else if (typeof v === "object" && v !== null) {
      best = longest(v, best);
    }
  };
  if (typeof value === "object" && value !== null) {
    for (const [k, v] of Object.entries(value)) visit(value as Record<string, unknown>, Array.isArray(value) ? Number(k) : k, v);
  }
  return best;
}

/**
 * An event as it fits on one line of the socket (D-033). One over the limit goes without its native
 * message; if it is still over, its longest texts are cut in the middle, keeping their start and end
 * with a mark between, until it fits. Only `title`, `text` and `body` are cut: the event's ids, state,
 * `prompts` and `settles` stay as they are. This is the socket's limit, not the item's 64 KB, which the
 * core applies (D-057). Says what it did, or returns null when nothing could make it fit.
 */
export function fitEvent(event: CoreEvent, size: (e: CoreEvent) => number, limit: number): { event: CoreEvent; did: string | null } | null {
  if (size(event) <= limit) return { event, did: null };
  const { native: _native, ...rest } = event;
  let out = rest as CoreEvent;
  if (size(out) <= limit) return { event: out, did: "sent without its native message" };
  out = structuredClone(out);
  const parts: Json = { title: out.title, text: out.text, body: out.body };
  for (;;) {
    const over = size(out) - limit;
    if (over <= 0) return { event: out, did: "cut to fit" };
    const at = longest(parts);
    if (at === null || at.length <= 2 * CUT.length) return null;
    const s = at.owner[at.key] as string;
    // Removing a UTF-16 unit removes at least a byte, so this many go, at least.
    const keep = Math.max(0, s.length - over - CUT_BYTES);
    let head = Math.ceil(keep / 2);
    let tail = s.length - (keep - head);
    if (head > 0 && high(s, head - 1)) head -= 1;
    if (tail < s.length && low(s, tail)) tail += 1;
    at.owner[at.key] = s.slice(0, head) + CUT + s.slice(tail);
    out.title = parts["title"] as string;
    out.text = parts["text"] as string;
  }
}

/** What Claude Code can do, as the core and the apps read it (D-058). */
export const CAPABILITIES: Capabilities = {
  interrupt: true,
  // A message sent during a turn ends it, and Claude Code starts another (D-040).
  mid_turn: "restart",
  sessions: "core",
  resume: true,
  approvals: true,
  files_in: "none",
  approval_commands: [],
};

export interface RunOptions {
  /** The agent's home: the socket and the token are found there. */
  home: string;
  cwd: string;
  model?: string;
  permissionMode?: string;
  query: QueryFn;
  /** The connector's own version, and Claude Code's, for the hello. */
  version: string;
  harnessVersion?: string;
  /** Says what went wrong, by kind only (D-071). */
  warn: (kind: string) => void;
  /** Makes the MCP server that offers the tool to send a file; the SDK's own by default. */
  fileServer?: FileServer;
}

/**
 * Connects to the core of the agent whose home is given, gives the hello, and serves the core until the
 * socket closes. Throws, naming a kind only, when the token cannot be read, the socket cannot be opened
 * or the hello is refused.
 */
export async function run(options: RunOptions): Promise<void> {
  let token: string;
  try {
    token = readFileSync(join(options.home, "connector.token"), "utf8").trim();
  } catch {
    throw new Error("the token file could not be read");
  }
  let core: CoreConnection;
  try {
    core = await CoreConnection.open(join(options.home, "socket", "marmot-agent.sock"));
  } catch {
    throw new Error("the core's socket could not be opened");
  }
  const connector = new Connector({
    query: options.query,
    cwd: options.cwd,
    model: options.model,
    permissionMode: options.permissionMode,
    send: (event) => {
      const fitted = fitEvent(event, (e) => notificationSize("event", e), MAX_MESSAGE);
      if (fitted?.did) options.warn(`an event over the size limit was ${fitted.did} (${event.type})`);
      if (fitted === null || !core.notify("event", fitted.event)) options.warn(`an event could not be sent (${event.type})`);
    },
    sendFile: (chat, path) => core.request("send_file", { chat, path }),
    fileServer: options.fileServer,
    warn: options.warn,
  });
  const harness: Hello["harness"] = { name: "claude-code" };
  if (options.harnessVersion !== undefined) harness.version = options.harnessVersion;
  try {
    await core.hello(
      {
        connector: { name: "marmot-agent-claude-code", version: options.version },
        harness,
        token,
        capabilities: CAPABILITIES,
      },
      (method, params) => connector.serve(method, params),
    );
  } catch {
    core.close();
    throw new Error("the core refused the hello");
  }
  await core.done;
  connector.close();
}
