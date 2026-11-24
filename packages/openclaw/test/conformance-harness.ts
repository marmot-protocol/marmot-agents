// The OpenClaw plugin for the driver conformance suite (`crates/marmot-agent-conformance`, "the socket
// connectors"): the plugin as OpenClaw loads it (src/index.ts), its account started as OpenClaw starts it,
// on the core's real socket, with a scripted OpenClaw behind it: the channel runtime the plugin is handed,
// and the plugin SDK's gateway calls (test/conformance-sdk.ts). No OpenClaw, no model and no network.
//
//   MARMOT_AGENT_HOME=<the agent's home> MARMOT_CONFORMANCE_STATE=<a folder> node test/conformance-harness.ts
//
// The scripted OpenClaw reads the same words in a prompt as every fake of the suite: `#ask` asks to run a
// command (an exec approval, through the plugin's approval adapter), `#quiz` asks a question (`ask_user`),
// `#sleep` runs until `/stop`; anything else is answered in words that echo the prompt.
//
// The suite drives it through a control channel on standard input and output, one JSON object a line,
// which the plugin uses for nothing else (it says what went wrong on standard error):
//
// - out `{"heard": {...}}`: what this OpenClaw understood, in the suite's shapes (`Heard`);
// - out `{"frame": <kind>, "wait": true}`: each call the plugin made on it, after what it heard in it;
//   it goes on once the suite answers `{"go": true}`, and dies there on `{"die": true}`;
// - the process ending is OpenClaw dying, as the plugin runs inside OpenClaw.

import { mkdirSync } from "node:fs";
import { registerHooks } from "node:module";
import { join } from "node:path";
import { createInterface } from "node:readline";

type Json = Record<string, unknown>;
type Callback = (...args: unknown[]) => unknown;

registerHooks({
  resolve(specifier, context, next) {
    if (specifier.startsWith("openclaw/plugin-sdk/")) return { url: new URL("./conformance-sdk.ts", import.meta.url).href, shortCircuit: true };
    return next(specifier, context);
  },
});

const home = process.env["MARMOT_AGENT_HOME"];
const state = process.env["MARMOT_CONFORMANCE_STATE"];
if (home === undefined || state === undefined) {
  console.error("conformance-harness: set MARMOT_AGENT_HOME and MARMOT_CONFORMANCE_STATE");
  process.exit(2);
}
// The plugin keeps its session counts in OpenClaw's home: here, the suite's folder.
process.env["OPENCLAW_HOME"] = state;
mkdirSync(join(state, ".openclaw"), { recursive: true });

// -- the control channel

const verdicts: ((word: Json) => void)[] = [];
const words: Json[] = [];
createInterface({ input: process.stdin }).on("line", (line) => {
  const word = JSON.parse(line) as Json;
  const waiting = verdicts.shift();
  if (waiting !== undefined) waiting(word);
  else words.push(word);
});
// The suite has gone: so does OpenClaw.
process.stdin.on("end", () => process.exit(0));

function heard(kind: string, fields: Json): void {
  process.stdout.write(JSON.stringify({ heard: { [kind]: fields } }) + "\n");
}

/** A call the plugin made, after what OpenClaw heard in it: it goes on once the suite says so. */
async function frame(kind: string): Promise<void> {
  process.stdout.write(JSON.stringify({ frame: kind, wait: true }) + "\n");
  const word = words.shift() ?? (await new Promise<Json>((r) => verdicts.push(r)));
  if (word["die"] === true) process.exit(1);
}

function object(value: unknown): Json {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Json) : {};
}

// -- the scripted OpenClaw

let ids = 0;
const next = (kind: string): string => `${kind}-${(ids += 1)}`;

/** What a run waits for: a press on its approval or its question, or `/stop`. */
const presses = new Map<string, (value: string) => void>();
/** The run each session has going until `/stop`. */
const sleeping = new Map<string, () => void>();

const entry = (await import("../src/index.ts")).default as unknown as Json;
const base = object(object(entry["plugin"])["base"]);
const startAccount = object(base["gateway"])["startAccount"] as (ctx: Json) => Promise<void>;
const exec = object(object(object(base["approvalCapability"])["render"])["exec"]);
const pending = exec["buildPendingPayload"] as (params: Json) => unknown;

(globalThis as unknown as { marmotConformance: unknown }).marmotConformance = {
  async resolveApproval(params: Json): Promise<unknown> {
    const id = String(params["approvalId"]);
    const decision = String(params["decision"]);
    const press = presses.get(id);
    if (press === undefined) throw new Error("approval not found");
    presses.delete(id);
    press(decision);
    return { applied: true, approval: { id, status: "resolved", decision } };
  },
  async resolveQuestion(params: Json): Promise<unknown> {
    const id = String(params["questionId"]);
    const value = String(params["optionValue"]);
    const press = presses.get(id);
    if (press === undefined) return { status: "not_found", questionId: id };
    presses.delete(id);
    press(value);
    return { status: "answered", questionId: id, optionValue: value };
  },
};

/** One run of a message, as OpenClaw's dispatcher makes it. */
async function dispatch(params: Json): Promise<unknown> {
  const ctx = object(params["ctx"]);
  const replyOptions = object(params["replyOptions"]);
  const dispatcherOptions = object(params["dispatcherOptions"]);
  const on = (name: string, ...args: unknown[]): unknown => (replyOptions[name] as Callback)(...args);
  const deliver = (payload: Json, info: Json): unknown => (dispatcherOptions["deliver"] as Callback)(payload, info);
  const text = String(ctx["Body"] ?? "");
  const session = String(ctx["SessionKey"] ?? "");
  const chat = String(ctx["Chat"] ?? "");
  const abort = replyOptions["abortSignal"] as AbortSignal | undefined;
  // The connection went: the plugin ends its runs before it connects again, and OpenClaw ends them.
  const aborted = new Promise<string>((resolve) => abort?.addEventListener("abort", () => resolve("aborted"), { once: true }));

  if (text.startsWith("/")) {
    heard("control", { session, what: text === "/stop" ? "interrupt" : text.slice(1) });
    await frame("inbound.run");
    const stop = sleeping.get(session);
    if (text === "/stop" && stop !== undefined) {
      sleeping.delete(session);
      await deliver({ text: "Agent was aborted." }, { kind: "final" });
      stop();
    } else {
      await deliver({ text: `Unknown command ${text}.` }, { kind: "final" });
    }
    return { admission: "dispatch", dispatched: true };
  }

  heard("prompt", { session, text, refused: false });
  await frame("inbound.run");
  const runId = next("run");
  on("onModelSelected", { provider: "p", model: "m", thinkLevel: "low" });
  on("onPreparedAgentRunStart", { runId, transcriptStart: { sessionKey: session, sessionId: "s-1" } });
  if (text.includes("#sleep")) {
    const stopped = await Promise.race([new Promise<string>((r) => sleeping.set(session, () => r("stopped"))), aborted]);
    if (stopped === "aborted") sleeping.delete(session);
    on("onAgentRunTerminalOutcome", "aborted");
    return { admission: "dispatch", dispatched: true };
  }
  if (text.includes("#ask") || text.includes("#quiz")) {
    const asking = text.includes("#ask");
    const tool = next("tool");
    const id = next(asking ? "ap" : "q");
    const pressed = new Promise<string>((r) => presses.set(id, r));
    if (asking) {
      on("onItemEvent", { toolCallId: tool, phase: "start", name: "exec", status: "running", commandBearing: true });
      on("onToolStart", { toolCallId: tool, name: "exec", phase: "start", args: { command: text } });
      const request = { id, request: { command: text, toolCallId: tool, allowedDecisions: ["allow-once", "deny"] } };
      pending({ request, target: { channel: "marmot", to: chat } });
    } else {
      const question = { id: "q0", question: `${text}?`, options: [{ label: "yes", value: "yes" }, { label: "no", value: "no" }] };
      on("onItemEvent", { toolCallId: tool, phase: "start", name: "ask_user", status: "running" });
      on("onToolStart", { toolCallId: tool, name: "ask_user", phase: "start", args: { questions: [question] } });
      await deliver({ text: question.question, channelData: { askUser: { questionId: id, optionValues: ["yes", "no"] } } }, { kind: "tool" });
    }
    const value = await Promise.race([pressed, aborted]);
    if (value === "aborted") {
      presses.delete(id);
      on("onAgentRunTerminalOutcome", "aborted");
      return { admission: "dispatch", dispatched: true };
    }
    if (asking) heard("answer", { session, allow: value !== "deny" });
    else heard("answer", { session, text: value });
    await frame(asking ? "resolveApproval" : "resolveQuestion");
  }
  await deliver({ text: `Done: ${text}` }, { kind: "final", assistantMessageIndex: 0 });
  on("onAgentRunTerminalOutcome", "completed");
  return { admission: "dispatch", dispatched: true };
}

/** The channel runtime OpenClaw hands a channel's account, as far as the plugin calls it. */
const channelRuntime: Json = {
  routing: {
    resolveAgentRoute: (p: Json) => ({ agentId: "main", accountId: "default", sessionKey: `agent:main:marmot:group:${object(p["peer"])["id"]}` }),
  },
  inbound: {
    ingress: {
      resolveStable: () => ({ ingress: { admission: "dispatch", decision: "allow", reasonCode: "activation_allowed" }, commandAccess: { authorized: true } }),
    },
    buildContext: (p: Json) => ({
      Body: object(p["message"])["rawBody"],
      Chat: object(p["conversation"])["id"],
      SessionKey: object(p["route"])["routeSessionKey"],
    }),
    run: async (p: Json) => {
      const adapter = object(p["adapter"]);
      const turn = object((adapter["resolveTurn"] as () => unknown)());
      return (turn["runDispatch"] as () => Promise<unknown>)();
    },
  },
  session: {
    resolveStorePath: () => join(state, "sessions.json"),
    recordInboundSession: async () => undefined,
  },
  reply: { dispatchReplyWithBufferedBlockDispatcher: dispatch },
};

await startAccount({
  cfg: { channels: { marmot: { home } } },
  accountId: "default",
  channelRuntime,
  abortSignal: new AbortController().signal,
});
