// An OpenClaw channel plugin that records what OpenClaw hands a channel.
//
// It plays the part of a chat network. It feeds OpenClaw scripted messages from named senders
// in pretend chats, and writes down everything OpenClaw gives back: every reply delivery of
// every kind, every progress callback, every call on the plugin's adapters, and what each of
// the plugin's own calls into OpenClaw returned. Nothing is sent anywhere.
//
// Each output line is {"t": <ms>, "dir": "in"|"out", "via": ..., "msg": ...}:
//
//   dir "out", via "inbound"   a scripted chat message this plugin handed to OpenClaw
//   dir "in",  via "hook"      OpenClaw called the plugin: an adapter method or a callback
//   dir "out", via "result"    what that method returned to OpenClaw
//   dir "out", via "call"      the plugin called OpenClaw (route, admission, context, approval)
//   dir "in",  via "call"      what that call returned
//   dir "in",  via "plugin_hook"  OpenClaw called a plugin hook, such as session_end
//   dir "out", via "press"     a scripted press of an approval or question button
//   dir "out", via "note"      a marker written by the recorder itself
//
// Environment:
//   RECORDER_SCENARIO      path to a JSON file: {"chats": {...}, "steps": [...]}. A step is one of:
//                            {"chat", "user", "text", ...}   a chat message; waits for the turn to
//                                                            end unless "wait" is false. Optional:
//                                                            "user_name", "message_id", "reply_to",
//                                                            "quote": {"body", "sender"},
//                                                            "mentioned": true|false.
//                                                            "{approval_id}" in the text is replaced
//                                                            by the id of the last approval seen.
//                            {"wait_for": "<method>"}        wait until OpenClaw calls that method
//                            {"wait_for_text": "<words>"}    wait until OpenClaw delivers text with them
//                            {"press": "<decision>", "user"} answer the last approval: allow-once,
//                                                            allow-always or deny
//                            {"answer": "<option>", "user"}  press a button on the last question
//                            {"sdk": "<what>", "chat"}       ask OpenClaw for something as data, through
//                                                            the plugin SDK: "commands", "native_commands",
//                                                            "models", or "session" (the stored entry of
//                                                            that chat's session)
//                            {"wait_turn": true}             wait for every running turn to end
//                            {"sleep": <seconds>}
//   RECORDER_OUT           path of the .jsonl file to write
//   RECORDER_TURN_TIMEOUT  seconds to wait for OpenClaw to finish a turn (default 900); a scenario
//                          may set its own with "turn_timeout"
//   RECORDER_CALLBACKS     "all" (default) registers every progress callback OpenClaw offers a
//                          channel; "none" registers none, as the simplest plugin would
//   RECORDER_BLOCK_STREAMING   "on" (default) asks for finished blocks of a reply as they complete;
//                          "off" asks for the final reply only
//   RECORDER_QUESTION_BUTTONS  set to 1 to claim native question buttons, which is what puts the
//                          ask_user tool in the agent's tool list
//   RECORDER_APPROVAL_CARDS    set to 1 to claim an approval capability: OpenClaw then asks the
//                          plugin who may approve, and hands it the structured approval request
//   RECORDER_PLUGIN_HOOKS  comma-separated plugin hooks to register and record, for example
//                          "session_start,session_end,before_reset". None by default
//   RECORDER_QUIET         comma-separated method names to count but not write out one by one

import fs from "node:fs";
import path from "node:path";

import { createChatChannelPlugin, defineChannelPluginEntry } from "openclaw/plugin-sdk/channel-core";
import { defineChannelMessageAdapter } from "openclaw/plugin-sdk/channel-outbound";
import { hasControlCommand } from "openclaw/plugin-sdk/command-detection";
import { resolveApprovalOverGateway } from "openclaw/plugin-sdk/approval-gateway-runtime";
import { questionGatewayRuntime } from "openclaw/plugin-sdk/question-gateway-runtime";
import { listChatCommands, listNativeCommandSpecs, resolveCommandArgChoices } from "openclaw/plugin-sdk/native-command-registry";
import { buildModelsProviderData } from "openclaw/plugin-sdk/models-provider-runtime";
import { getSessionEntryAsync } from "openclaw/plugin-sdk/session-store-runtime";

const ID = "recorder";
const env = process.env;

// ---------------------------------------------------------------------------------------------
// Turning what OpenClaw passes into JSON

const SECRET_KEY = /api[-_]?key|token|secret|password|authorization|credential/i;

function readSecret() {
  try {
    const s = fs.readFileSync("/run/secrets/model_key", "utf8").trim();
    return s.length >= 8 ? s : null;
  } catch {
    return null;
  }
}
const MODEL_KEY = readSecret();
// OpenClaw's own replies can show the key in a masked form that keeps its last characters, as
// "/model status" does. Match any such form, and the bare end of the key.
const escapeRe = (t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const KEY_TAILS = MODEL_KEY && MODEL_KEY.length >= 16 ? [8, 7, 6, 5, 4].map((n) => escapeRe(MODEL_KEY.slice(-n))) : null;
const MASKED_KEY = KEY_TAILS ? new RegExp(`[A-Za-z0-9_-]{0,24}(?:\\.{2,}|…|\\*{2,})(?:${KEY_TAILS.join("|")})`, "g") : null;

function withoutKey(line) {
  if (!MODEL_KEY) return line;
  line = line.split(MODEL_KEY).join("<model-key>");
  if (MASKED_KEY) line = line.replace(MASKED_KEY, "<model-key>").split(MODEL_KEY.slice(-8)).join("<model-key>");
  return line;
}

function looksLikeConfig(v) {
  return "gateway" in v && ("agents" in v || "models" in v || "channels" in v);
}

/** Keep structure, drop what cannot be kept, and never write the configuration or a secret. */
function jsonable(v, depth = 0, seen = new Set()) {
  if (v === null || v === undefined) return v ?? null;
  const t = typeof v;
  if (t === "string" || t === "boolean") return v;
  if (t === "number") return Number.isFinite(v) ? v : String(v);
  if (t === "bigint") return String(v);
  if (t === "function") return `<function ${v.name || "anonymous"}>`;
  if (t === "symbol") return String(v);
  if (depth > 8) return `<${v.constructor?.name ?? "object"}>`;
  if (seen.has(v)) return "<repeated>";
  if (v instanceof Error) return { _error: v.name, message: String(v.message) };
  if (v instanceof Date) return v.toISOString();
  if (Buffer.isBuffer(v) || v instanceof Uint8Array) return { _bytes: v.length };
  if (typeof AbortSignal !== "undefined" && v instanceof AbortSignal) return { _abort_signal: v.aborted };
  if (v instanceof Promise) return "<promise>";
  seen.add(v);
  try {
    if (Array.isArray(v)) return v.slice(0, 200).map((x) => jsonable(x, depth + 1, seen));
    if (v instanceof Map) return jsonable(Object.fromEntries([...v.entries()].slice(0, 200)), depth + 1, seen);
    if (v instanceof Set) return jsonable([...v].slice(0, 200), depth + 1, seen);
    if (looksLikeConfig(v)) return "<config>";
    const out = {};
    for (const [k, x] of Object.entries(v).slice(0, 200)) {
      if (x === undefined) continue;
      if (SECRET_KEY.test(k) && typeof x === "string") out[k] = "<redacted>";
      else if (k === "cfg" || k === "config") out[k] = x && typeof x === "object" ? "<config>" : jsonable(x, depth + 1, seen);
      else out[k] = jsonable(x, depth + 1, seen);
    }
    return out;
  } finally {
    seen.delete(v);
  }
}

// ---------------------------------------------------------------------------------------------
// The recording

class Recording {
  constructor(file) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    this.fd = fs.openSync(file, "w");
    this.t0 = Date.now();
    this.calls = new Map();
    this.texts = [];
    this.waiters = new Set();
    this.quiet = new Set((env.RECORDER_QUIET ?? "").split(",").map((s) => s.trim()).filter(Boolean));
    this.quietCounts = new Map();
    this.lastApprovalId = null;
    this.lastQuestionId = null;
    this.hooks = 0;
  }

  write(dir, via, msg) {
    const line = withoutKey(JSON.stringify({ t: Date.now() - this.t0, dir, via, msg: jsonable(msg) }));
    fs.writeSync(this.fd, line + "\n");
  }

  /** OpenClaw called the plugin. */
  hook(method, params) {
    this.calls.set(method, (this.calls.get(method) ?? 0) + 1);
    this.hooks += 1;
    if (this.quiet.has(method)) {
      this.quietCounts.set(method, (this.quietCounts.get(method) ?? 0) + 1);
    } else {
      this.write("in", "hook", { method, params });
    }
    this.noteText(params);
    for (const w of this.waiters) w();
  }

  result(method, value) {
    if (!this.quiet.has(method)) this.write("out", "result", { method, ...value });
  }

  /** Remember delivered text and approval ids, for the wait_for_text and press steps. */
  noteText(v, depth = 0) {
    if (v === null || v === undefined || depth > 6) return;
    if (typeof v === "string") {
      if (v.length > 0) this.texts.push(v);
      const m = /\/approve\s+([A-Za-z0-9_-]{4,})\s+(allow-once|allow-always|deny)/.exec(v);
      if (m) this.lastApprovalId = m[1];
      return;
    }
    if (typeof v !== "object" || looksLikeConfigSafe(v)) return;
    if (typeof v.approvalId === "string") this.lastApprovalId = v.approvalId;
    if (typeof v.questionId === "string") this.lastQuestionId = v.questionId;
    for (const [k, x] of Object.entries(v).slice(0, 100)) {
      if (k === "cfg" || k === "config") continue;
      this.noteText(x, depth + 1);
    }
  }

  count(method) {
    return this.calls.get(method) ?? 0;
  }

  async waitUntil(condition, timeoutSeconds) {
    const deadline = Date.now() + timeoutSeconds * 1000;
    while (!condition()) {
      const left = deadline - Date.now();
      if (left <= 0) return false;
      await new Promise((resolve) => {
        const done = () => {
          clearTimeout(timer);
          this.waiters.delete(done);
          resolve();
        };
        const timer = setTimeout(done, Math.min(left, 1000));
        this.waiters.add(done);
      });
    }
    return true;
  }
}

function looksLikeConfigSafe(v) {
  try {
    return looksLikeConfig(v);
  } catch {
    return true;
  }
}

const rec = new Recording(env.RECORDER_OUT ?? "/out/recording.jsonl");

/** Wrap a function OpenClaw calls: record the call, run it, record what it returned. */
function tap(method, fn) {
  return function tapped(...args) {
    rec.hook(method, args.length === 1 ? args[0] : args);
    let out;
    try {
      out = fn.apply(this, args);
    } catch (err) {
      rec.result(method, { error: jsonable(err) });
      throw err;
    }
    if (out && typeof out.then === "function") {
      return out.then(
        (value) => {
          rec.result(method, { result: value });
          return value;
        },
        (err) => {
          rec.result(method, { error: jsonable(err) });
          throw err;
        },
      );
    }
    rec.result(method, { result: out });
    return out;
  };
}

/** Wrap every function in an adapter object, to any depth. */
function tapAll(prefix, obj, depth = 0) {
  if (!obj || typeof obj !== "object" || depth > 4 || Array.isArray(obj)) return obj;
  const out = {};
  for (const [k, v] of Object.entries(obj)) {
    const name = prefix ? `${prefix}.${k}` : k;
    if (typeof v === "function") out[k] = tap(name, v);
    else if (v && typeof v === "object" && !Array.isArray(v)) out[k] = tapAll(name, v, depth + 1);
    else out[k] = v;
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// The channel's own settings

function section(cfg) {
  return cfg?.channels?.[ID] ?? {};
}

function resolveAccount(cfg, accountId) {
  const s = section(cfg);
  return {
    accountId: accountId ?? "default",
    dmPolicy: s.dmPolicy ?? "pairing",
    groupPolicy: s.groupPolicy ?? "allowlist",
    allowFrom: s.allowFrom ?? [],
    groupAllowFrom: s.groupAllowFrom ?? [],
    requireMention: s.requireMention ?? true,
  };
}

let outCount = 0;
function receipt(kind) {
  outCount += 1;
  const id = `out-${outCount}`;
  return {
    receipt: {
      primaryPlatformMessageId: id,
      platformMessageIds: [id],
      parts: [{ platformMessageId: id, kind, index: 0 }],
      sentAt: Date.now(),
    },
  };
}

// ---------------------------------------------------------------------------------------------
// The scripted chat

const scenario = JSON.parse(fs.readFileSync(env.RECORDER_SCENARIO ?? "/scenario.json", "utf8"));
const turnTimeout = Number(scenario.turn_timeout ?? env.RECORDER_TURN_TIMEOUT ?? "900");
const callbackMode = env.RECORDER_CALLBACKS ?? "all";

// Every progress callback OpenClaw's reply options offer a channel. Each is recorded and
// returns nothing, which leaves OpenClaw's own handling as it was.
const REPLY_CALLBACKS = [
  "onPreparedAgentRunStart", "onAgentRunStart", "onAgentRunTerminalOutcome", "onFollowupQueueDisposition",
  "onVisibleWorkSessions", "onReplyStart", "onTypingCleanup", "onPartialReply", "onReasoningStream",
  "onReasoningProgress", "onReasoningEnd", "onAssistantMessageStart", "onBlockReplyQueued", "onToolResult",
  "onToolStart", "onItemEvent", "onNarrationUpdate", "onPlanUpdate", "onApprovalEvent", "onCommandOutput",
  "onPatchSummary", "onCompactionStart", "onCompactionEnd", "onModelSelected", "onQueuedFollowupAdmitted",
  "onQueuedFollowupSettled", "onObservedReplyDelivery",
];

let inCount = 0;
const running = new Set();
let gateway = null;

async function callOpenClaw(name, params, fn) {
  rec.write("out", "call", { method: name, params });
  try {
    const value = await fn();
    rec.write("in", "call", { method: name, result: value });
    return value;
  } catch (err) {
    rec.write("in", "call", { method: name, error: jsonable(err) });
    throw err;
  }
}

async function handleInbound(step) {
  const { cfg, channelRuntime: rt, accountId } = gateway;
  const account = resolveAccount(cfg, accountId);
  const chat = scenario.chats?.[step.chat] ?? {};
  const kind = (step.chat_type ?? chat.type ?? "group") === "dm" ? "direct" : "group";
  inCount += 1;
  const messageId = step.message_id ?? `in-${inCount}`;
  const text = String(step.text).replaceAll("{approval_id}", rec.lastApprovalId ?? "unknown");
  rec.write("out", "inbound", { message_id: messageId, ...step, text });

  // 1. Which agent and which session does this chat belong to? OpenClaw decides.
  const peer = { kind, id: kind === "direct" ? step.user : step.chat };
  const route = await callOpenClaw("routing.resolveAgentRoute", { channel: ID, accountId, peer }, () =>
    rt.routing.resolveAgentRoute({ cfg, channel: ID, accountId, peer }),
  );

  // 2. May this sender run the agent here? OpenClaw decides, from the channel's settings.
  const controlCommand = hasControlCommand(text, cfg);
  const mentioned = step.mentioned ?? true;
  const ingressParams = {
    channelId: ID,
    accountId,
    subject: { stableId: step.user },
    conversation: { kind, id: step.chat },
    contextBinding: { agentId: route.agentId, sessionKey: route.sessionKey, messageId, inboundEventKind: "user_request" },
    event: { kind: "message", authMode: "inbound", mayPair: kind === "direct" },
    dmPolicy: account.dmPolicy,
    groupPolicy: account.groupPolicy,
    policy: {
      groupAllowFromFallbackToAllowFrom: true,
      activation: { requireMention: kind === "group" && account.requireMention, allowTextCommands: true },
    },
    allowFrom: account.allowFrom,
    groupAllowFrom: account.groupAllowFrom,
    mentionFacts: { canDetectMention: true, wasMentioned: mentioned },
    command: { allowTextCommands: true, hasControlCommand: controlCommand },
  };
  const ingress = await callOpenClaw("inbound.ingress.resolveStable", ingressParams, () =>
    rt.inbound.ingress.resolveStable({ ...ingressParams, cfg, accessGroups: cfg.accessGroups }),
  );
  if (ingress.ingress.admission !== "dispatch") {
    // The decision is OpenClaw's. Acting on it is the plugin's job, and this plugin does nothing.
    rec.write("out", "note", {
      not_dispatched: messageId,
      admission: ingress.ingress.admission,
      reason: ingress.ingress.reasonCode,
    });
    return;
  }

  // 3. Hand over the message, with who sent it and where.
  const contextParams = {
    channel: ID,
    accountId,
    messageId,
    timestamp: Date.now(),
    // For a group, OpenClaw reads the group's identity out of "from"; the person is in "sender".
    from: kind === "group" ? `${ID}:group:${step.chat}` : `${ID}:${step.user}`,
    sender: { id: step.user, name: step.user_name ?? step.user },
    conversation: { kind, id: step.chat, label: chat.name ?? step.chat },
    route: { agentId: route.agentId, accountId: route.accountId, routeSessionKey: route.sessionKey },
    reply: { to: step.chat, ...(step.reply_to ? { replyToId: step.reply_to } : {}) },
    message: { rawBody: text, bodyForAgent: text },
    access: {
      commands: { authorized: ingress.commandAccess.authorized },
      mentions: { canDetectMention: true, wasMentioned: mentioned, requireMention: account.requireMention },
    },
    ...(step.quote
      ? { supplemental: { quote: { id: step.reply_to, body: step.quote.body, sender: step.quote.sender, isQuote: true } } }
      : {}),
  };
  rec.write("out", "call", { method: "inbound.buildContext", params: contextParams });
  const ctxPayload = await rt.inbound.buildContext({
    ...contextParams,
    channelIngress: ingress,
    resolveSupplementalMedia: true,
    suppressSelfQuoteBody: false,
  });
  rec.write("in", "call", { method: "inbound.buildContext", result: ctxPayload });

  const storePath = rt.session.resolveStorePath(cfg.session?.store, { agentId: route.agentId });

  // 4. Run the turn. Everything OpenClaw hands back arrives through these callbacks.
  const replyOptions = {
    sourceReplyDeliveryMode: "automatic",
    disableBlockStreaming: (env.RECORDER_BLOCK_STREAMING ?? "on") === "off",
  };
  if (callbackMode === "all") {
    for (const name of REPLY_CALLBACKS) replyOptions[name] = tap(name, () => undefined);
    replyOptions.onTypingController = (typing) => rec.hook("onTypingController", { methods: Object.keys(typing ?? {}) });
  }
  const dispatcherOptions = {
    deliver: tap("deliver", async (payload, info) => {
      outCount += 1;
      return { messageId: `out-${outCount}` };
    }),
    onError: tap("dispatcher.onError", () => undefined),
    onSkip: tap("dispatcher.onSkip", () => undefined),
    onIdle: tap("dispatcher.onIdle", () => undefined),
    onReplyStart: tap("dispatcher.onReplyStart", () => undefined),
    onSettled: tap("dispatcher.onSettled", () => undefined),
    onCleanup: tap("dispatcher.onCleanup", () => undefined),
    onDeliverySettled: tap("dispatcher.onDeliverySettled", () => undefined),
    typingCallbacks: {
      onReplyStart: tap("typing.onReplyStart", async () => undefined),
      onIdle: tap("typing.onIdle", () => undefined),
      onCleanup: tap("typing.onCleanup", () => undefined),
    },
  };

  rec.write("out", "call", { method: "inbound.run", params: { messageId } });
  const result = await rt.inbound.run({
    channel: ID,
    accountId,
    raw: step,
    log: tap("inbound.log", () => undefined),
    adapter: {
      ingest: () => ({ id: messageId, rawText: text, textForAgent: text }),
      resolveTurn: () => ({
        channel: ID,
        accountId,
        routeSessionKey: route.sessionKey,
        storePath,
        ctxPayload,
        recordInboundSession: rt.session.recordInboundSession,
        record: { groupResolution: kind === "group" ? { key: `${ID}:group:${step.chat}`, channel: ID, id: step.chat, chatType: "group" } : undefined },
        runDispatchLifecycle: { turnAdoptionLifecycle: undefined, onDispatchSkipped: async () => undefined },
        runDispatch: () =>
          rt.reply.dispatchReplyWithBufferedBlockDispatcher({ ctx: ctxPayload, cfg, dispatcherOptions, replyOptions }),
      }),
    },
  });
  rec.write("in", "call", {
    method: "inbound.run",
    result: { admission: result?.admission, dispatched: result?.dispatched, dispatchResult: result?.dispatchResult },
  });
}

/** Ask OpenClaw for something as data, through the plugin SDK, and record the answer. */
async function sdkStep(step) {
  const { cfg, channelRuntime: rt, accountId } = gateway;
  const [provider, ...rest] = String(cfg.agents?.defaults?.model?.primary ?? "").split("/");
  const model = rest.join("/");
  let method;
  let params = {};
  let fn;
  if (step.sdk === "commands") {
    method = "native-command-registry.listChatCommands";
    // Some commands compute their choices from the model. Those are resolved for the configured one.
    params = { resolveChoicesFor: { provider, model } };
    fn = () =>
      listChatCommands().map((command) => ({
        ...command,
        args: command.args?.map((arg) =>
          typeof arg.choices === "function"
            ? { ...arg, choices: "<function>", resolvedChoices: resolveCommandArgChoices({ cfg, provider, model, command, arg }) }
            : arg,
        ),
      }));
  } else if (step.sdk === "native_commands") {
    method = "native-command-registry.listNativeCommandSpecs";
    params = { provider: ID };
    fn = () => listNativeCommandSpecs({ provider: ID });
  } else if (step.sdk === "models") {
    method = "models-provider-runtime.buildModelsProviderData";
    params = { agentId: "main" };
    fn = () => buildModelsProviderData(cfg, "main");
  } else if (step.sdk === "session") {
    const chat = scenario.chats?.[step.chat] ?? {};
    const kind = (chat.type ?? "group") === "dm" ? "direct" : "group";
    const peer = { kind, id: kind === "direct" ? step.user : step.chat };
    const route = rt.routing.resolveAgentRoute({ cfg, channel: ID, accountId, peer });
    method = "session-store-runtime.getSessionEntryAsync";
    params = { agentId: route.agentId, sessionKey: route.sessionKey };
    fn = async () => {
      const entry = await getSessionEntryAsync(params);
      if (!entry) return entry ?? null;
      // Keep the settings and ids. Leave out the large parts: the skills and the prompt report.
      const out = {};
      for (const [k, v] of Object.entries(entry)) {
        const size = JSON.stringify(jsonable(v) ?? null).length;
        out[k] = size > 1500 ? { _omitted_bytes: size } : v;
      }
      return out;
    };
  } else {
    throw new Error(`unknown sdk step: ${step.sdk}`);
  }
  rec.write("out", "call", { method, params });
  try {
    rec.write("in", "call", { method, result: await fn() });
  } catch (err) {
    rec.write("in", "call", { method, error: jsonable(err) });
  }
}

function startInbound(step) {
  const p = handleInbound(step).catch((err) => {
    rec.write("out", "note", { inbound_failed: jsonable(err), stack: String(err?.stack ?? "").split("\n").slice(0, 6) });
  });
  running.add(p);
  p.finally(() => running.delete(p));
  return p;
}

async function waitTurns(settleSeconds) {
  const deadline = Date.now() + turnTimeout * 1000;
  for (;;) {
    while (running.size > 0) {
      const left = deadline - Date.now();
      if (left <= 0) return false;
      await Promise.race([Promise.allSettled([...running]), new Promise((r) => setTimeout(r, left))]);
    }
    // A queued follow-up can start after the turn that queued it has ended, and a stopped turn
    // never reports an outcome. So wait until OpenClaw has been quiet for a while.
    const before = rec.hooks;
    await new Promise((r) => setTimeout(r, settleSeconds * 1000));
    if (running.size === 0 && rec.hooks === before) return true;
    if (Date.now() >= deadline) return false;
  }
}

async function runScenario() {
  await new Promise((r) => setTimeout(r, Number(scenario.start_delay ?? 3) * 1000));
  rec.write("out", "note", { scenario: scenario.name, steps: scenario.steps.length });
  let status = "ok";
  try {
    for (const step of scenario.steps) {
      if ("sleep" in step) {
        await new Promise((r) => setTimeout(r, Number(step.sleep) * 1000));
      } else if ("wait_for_text" in step) {
        const words = step.wait_for_text;
        const seen = await rec.waitUntil(() => rec.texts.some((t) => t.includes(words)), Number(step.timeout ?? turnTimeout));
        rec.write("out", "note", { wait_for_text: words, seen });
        if (!seen && (step.required ?? true)) {
          status = `OpenClaw never delivered text containing ${JSON.stringify(words)}`;
          break;
        }
        await new Promise((r) => setTimeout(r, Number(step.settle ?? 1) * 1000));
      } else if ("wait_for" in step) {
        const method = step.wait_for;
        const after = Number(step.after ?? 0);
        const seen = await rec.waitUntil(() => rec.count(method) > after, Number(step.timeout ?? turnTimeout));
        rec.write("out", "note", { wait_for: method, seen });
        if (!seen && (step.required ?? true)) {
          status = `OpenClaw never called ${method}`;
          break;
        }
        await new Promise((r) => setTimeout(r, Number(step.settle ?? 1) * 1000));
      } else if ("press" in step) {
        const params = {
          approvalId: rec.lastApprovalId, decision: step.press, approvalKind: "exec",
          channel: ID, accountId: gateway.accountId, senderId: step.user,
        };
        rec.write("out", "press", params);
        try {
          const resolved = await resolveApprovalOverGateway({ ...params, cfg: gateway.cfg });
          rec.write("in", "call", { method: "resolveApprovalOverGateway", result: resolved });
        } catch (err) {
          rec.write("in", "call", { method: "resolveApprovalOverGateway", error: jsonable(err) });
        }
      } else if ("sdk" in step) {
        await sdkStep(step);
      } else if ("answer" in step) {
        const params = { questionId: rec.lastQuestionId, optionValue: step.answer, senderId: step.user };
        rec.write("out", "press", params);
        try {
          const resolved = await questionGatewayRuntime.resolveOption({ ...params, cfg: gateway.cfg });
          rec.write("in", "call", { method: "questionGatewayRuntime.resolveOption", result: resolved });
        } catch (err) {
          rec.write("in", "call", { method: "questionGatewayRuntime.resolveOption", error: jsonable(err) });
        }
      } else if (step.wait_turn) {
        if (!(await waitTurns(Number(step.settle ?? 4)))) {
          status = "timed out waiting for a turn to finish";
          break;
        }
      } else {
        startInbound(step);
        if (step.wait ?? true) {
          if (!(await waitTurns(Number(step.settle ?? 4)))) {
            status = "timed out waiting for a turn to finish";
            break;
          }
        } else {
          await new Promise((r) => setTimeout(r, 200));
        }
      }
    }
  } catch (err) {
    status = `failed: ${err?.name}: ${err?.message}`;
  }
  if (rec.quietCounts.size > 0) rec.write("out", "note", { counted_not_written: Object.fromEntries(rec.quietCounts) });
  rec.write("out", "note", { finished: status });
  // Ask the gateway to shut down cleanly, and leave even if something keeps the process alive.
  process.kill(process.pid, "SIGTERM");
  setTimeout(() => process.exit(0), 15000);
}

// ---------------------------------------------------------------------------------------------
// The plugin

let runner = null;

async function startAccount(ctx) {
  rec.hook("gateway.startAccount", { accountId: ctx.accountId, account: ctx.account, has: Object.keys(ctx) });
  gateway = ctx;
  ctx.setStatus?.({ ...ctx.getStatus?.(), accountId: ctx.accountId, running: true, connected: true, lastStartAt: Date.now() });
  if (!ctx.channelRuntime) throw new Error("recorder: no channel runtime was handed to the plugin");
  if (!runner) runner = runScenario();
  // OpenClaw keeps a channel account alive for as long as this promise is pending.
  await new Promise((resolve) => {
    if (ctx.abortSignal.aborted) resolve();
    else ctx.abortSignal.addEventListener("abort", resolve, { once: true });
  });
  rec.hook("gateway.abort", { accountId: ctx.accountId });
}

function buildPlugin() {
  const message = defineChannelMessageAdapter({
    id: ID,
    durableFinal: { capabilities: { text: true, media: true, payload: true, replyTo: true } },
    send: {
      text: tap("message.send.text", async () => receipt("text")),
      media: tap("message.send.media", async () => receipt("media")),
      payload: tap("message.send.payload", async () => receipt("text")),
    },
    receive: { defaultAckPolicy: "after_agent_dispatch", supportedAckPolicies: ["after_agent_dispatch", "manual"] },
  });

  const base = {
    id: ID,
    meta: {
      id: ID, label: "Recorder", selectionLabel: "Recorder", docsPath: "channels/recorder",
      blurb: "Plays scripted chats into OpenClaw and records everything it hands back.", markdownCapable: true,
    },
    capabilities: {
      chatTypes: ["direct", "group"], reply: true, media: true, reactions: true, edit: true, unsend: true,
      blockStreaming: true,
    },
    config: {
      listAccountIds: () => ["default"],
      resolveAccount,
      inspectAccount: () => ({ enabled: true, configured: true }),
    },
    message,
    messaging: tapAll("messaging", {
      targetPrefixes: [ID],
      normalizeTarget: (raw) => String(raw).trim().replace(/^recorder:/, "") || undefined,
      inferTargetChatType: ({ to } = {}) => (scenario.chats?.[String(to ?? "").replace(/^recorder:/, "")]?.type === "dm" ? "direct" : "group"),
      targetResolver: {
        hint: "<chat name, for example family>",
        looksLikeId: () => true,
        resolveTarget: async ({ input }) => {
          const to = String(input).trim().replace(/^recorder:/, "");
          return to ? { to, kind: scenario.chats?.[to]?.type === "dm" ? "user" : "group", source: "normalized" } : null;
        },
      },
    }),
    agentPrompt: tapAll("agentPrompt", {
      ...(env.RECORDER_QUESTION_BUTTONS === "1" ? { questionInputMode: "buttons" } : {}),
      messageToolHints: () => [
        "- This is a test chat network. A chat is addressed by its name, and a message by its id.",
      ],
    }),
    actions: tapAll("actions", {
      describeMessageTool: () => ({ actions: ["send", "react", "edit", "delete"] }),
      supportsAction: ({ action }) => ["send", "react", "edit", "delete"].includes(action),
      handleAction: async (ctx) => {
        outCount += 1;
        const details = { ok: true, action: ctx.action, messageId: `out-${outCount}` };
        return { content: [{ type: "text", text: JSON.stringify(details) }], details };
      },
    }),
    groups: tapAll("groups", {
      resolveRequireMention: ({ cfg }) => section(cfg).requireMention ?? true,
    }),
    heartbeat: tapAll("heartbeat", {
      sendTyping: async () => undefined,
      clearTyping: async () => undefined,
    }),
    gateway: { startAccount },
  };

  if (env.RECORDER_APPROVAL_CARDS === "1") {
    base.approvalCapability = tapAll("approvalCapability", {
      // OpenClaw asks the channel whether this sender may approve. This plugin says yes to everyone.
      authorizeActorAction: () => ({ authorized: true }),
      getActionAvailabilityState: () => ({ kind: "enabled" }),
      render: {
        exec: { buildPendingPayload: () => null, buildResolvedPayload: () => null },
        plugin: { buildPendingPayload: () => null, buildResolvedPayload: () => null },
      },
    });
  }

  return createChatChannelPlugin({
    base,
    security: tapAll("security", {
      dm: {
        channelKey: ID,
        resolvePolicy: (account) => account.dmPolicy,
        resolveAllowFrom: (account) => account.allowFrom,
        defaultPolicy: "pairing",
      },
    }),
    pairing: {
      text: {
        idLabel: "Recorder user name",
        message: "Send this code to verify your identity:",
        notify: tap("pairing.notify", async () => undefined),
      },
    },
    threading: { topLevelReplyToMode: "reply" },
  });
}

export default defineChannelPluginEntry({
  id: ID,
  name: "Recorder",
  description: "Plays scripted chats into OpenClaw and records everything it hands back.",
  plugin: buildPlugin(),
  registerFull(api) {
    // Typed plugin hooks, which any plugin may register beside its channel. Each is recorded and
    // returns nothing, so it changes nothing.
    for (const name of (env.RECORDER_PLUGIN_HOOKS ?? "").split(",").map((s) => s.trim()).filter(Boolean)) {
      api.on(name, (event, ctx) => {
        rec.write("in", "plugin_hook", { hook: name, event, ctx });
      });
    }
  },
});
