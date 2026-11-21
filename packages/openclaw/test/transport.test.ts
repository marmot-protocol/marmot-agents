// The transport, run against a fake OpenClaw in place of the plugin SDK's runtime, and once against a fake
// core on a real Unix socket: no network and no OpenClaw. The calls the fake makes have the shapes the
// recordings in fixtures/recordings/openclaw hold. Every value here is made up.

import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { CAPABILITIES, Connector, type CoreEvent, type Port, type RunInput } from "../src/connector.ts";
import { fitEvent } from "../src/fit.ts";
import { CoreConnection, MAX_MESSAGE, notificationSize, Refusal } from "../src/socket.ts";

type Json = Record<string, unknown>;
type Callback = (...args: unknown[]) => unknown;

const KEY = "agent:main:marmot:group:c-1";

/** One run of `inbound.run`, driven by the test. */
class FakeRun {
  readonly input: RunInput;
  private finish!: (value: unknown) => void;
  private fail!: (e: Error) => void;
  readonly done: Promise<unknown>;
  constructor(input: RunInput) {
    this.input = input;
    this.done = new Promise((resolve, reject) => {
      this.finish = resolve;
      this.fail = reject;
    });
  }
  /** OpenClaw calls one of the run's progress callbacks. */
  on(name: string, ...args: unknown[]): unknown {
    return (this.input.replyOptions[name] as Callback)(...args);
  }
  deliver(payload: Json, info: Json): unknown {
    return (this.input.dispatcherOptions["deliver"] as Callback)(payload, info);
  }
  /** A turn, as OpenClaw starts one. */
  start(runId: string): void {
    this.on("onModelSelected", { provider: "p", model: "m", thinkLevel: "low" });
    this.on("onPreparedAgentRunStart", { runId, transcriptStart: { sessionKey: KEY, sessionId: "s-1" } });
  }
  end(result: Json = { admission: "dispatch", dispatched: true }): void {
    this.finish(result);
  }
  crash(): void {
    this.fail(new Error("gone"));
  }
}

class FakeOpenClaw implements Port {
  readonly runs: FakeRun[] = [];
  readonly approvals: Json[] = [];
  readonly questions: Json[] = [];
  /** What `mayApprove` said while each approval was being resolved. */
  readonly authorized: unknown[] = [];
  readonly ingresses: Json[] = [];
  readonly contexts: Json[] = [];
  connector!: Connector;
  admission = "dispatch";
  /** Thrown by the next press, as the gateway throws "approval not found". */
  pressFails: Error | null = null;
  /** Holds the next press until it resolves, as a slow gateway does. */
  pressWait: Promise<unknown> | null = null;
  async route(peer: { kind: "group"; id: string }): Promise<Json> {
    return { agentId: "main", accountId: "default", sessionKey: `agent:main:marmot:group:${peer.id}` };
  }
  async ingress(params: Json): Promise<Json> {
    this.ingresses.push(params);
    return { ingress: { admission: this.admission, decision: "allow", reasonCode: "activation_allowed" }, commandAccess: { authorized: true } };
  }
  /** Holds the next message before its context is built, until it resolves. */
  contextWait: Promise<unknown> | null = null;
  async context(params: Json): Promise<unknown> {
    this.contexts.push(params);
    const wait = this.contextWait;
    this.contextWait = null;
    if (wait !== null) await wait;
    return { Body: params["message"] };
  }
  async run(input: RunInput): Promise<unknown> {
    const run = new FakeRun(input);
    this.runs.push(run);
    return run.done;
  }
  async resolveApproval(params: Json): Promise<unknown> {
    this.approvals.push(params);
    const fails = this.pressFails;
    this.pressFails = null;
    const wait = this.pressWait;
    this.pressWait = null;
    if (wait !== null) await wait;
    if (fails !== null) throw fails;
    this.authorized.push(this.connector.mayApprove(params["senderId"]).authorized);
    return { applied: true, approval: { id: params["approvalId"], status: "resolved", decision: params["decision"] } };
  }
  async resolveQuestion(params: Json): Promise<unknown> {
    this.questions.push(params);
    return { status: "answered", questionId: params["questionId"], optionValue: params["optionValue"] };
  }
  /** The stored session entry, as the last command left it. */
  entry: Json = { sessionId: "s-1", modelOverride: "m2", providerOverride: "p" };
  async sessionEntry(): Promise<unknown> {
    return this.entry;
  }
  hasControlCommand(text: string): boolean {
    return text.startsWith("/");
  }
  async commands(): Promise<unknown> {
    return [
      { key: "think", description: "Set the thinking level.", textAliases: ["/think"], args: [{ name: "level", choices: ["off", "low", "high"] }] },
      { key: "new", description: "Start a new session.", textAliases: ["/new"] },
    ];
  }
  async models(): Promise<unknown> {
    return { byProvider: { p: ["m", "m2"] }, providers: ["p"], resolvedDefault: { provider: "p", model: "m" } };
  }
  last(): FakeRun {
    return this.runs.at(-1)!;
  }
}

function setUp(generations: [string, number][] = []) {
  const openclaw = new FakeOpenClaw();
  const events: CoreEvent[] = [];
  const saved: [string, number][][] = [];
  const connector = new Connector({
    port: openclaw,
    channel: "marmot",
    accountId: "default",
    version: "2026.11.5",
    boot: "b0",
    generations,
    saveGenerations: (g) => saved.push(g),
    send: (e) => events.push(e),
  });
  openclaw.connector = connector;
  return { openclaw, events, connector, saved };
}

const alice = { id: "u-1", name: "Alice" };
const chat = { id: "c-1", group: true, name: "Family" };

function prompt(message: string, text: string): Json {
  return { message, text, chat, sender: alice, attachments: [] };
}

async function tick(): Promise<void> {
  await new Promise((r) => setTimeout(r, 0));
}

async function refused(p: Promise<void>, reason: string): Promise<void> {
  await assert.rejects(p, (e: unknown) => e instanceof Refusal && e.reason === reason);
}

test("a prompt reaches OpenClaw with an open policy, as a group, and its turn names it", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "hello"));
  const [ingress] = openclaw.ingresses;
  // The core decided who may prompt: OpenClaw lets in what it sends (Q-22, provisional).
  assert.equal(ingress!["dmPolicy"], "open");
  assert.equal(ingress!["groupPolicy"], "open");
  assert.deepEqual(ingress!["groupAllowFrom"], ["*"]);
  assert.deepEqual(openclaw.contexts[0]!["sender"], alice);
  assert.deepEqual(openclaw.contexts[0]!["conversation"], { kind: "group", id: "c-1", label: "Family" });
  const run = openclaw.last();
  assert.equal(run.input.text, "hello");
  run.start("r-1");
  run.deliver({ text: "Hi Alice" }, { kind: "block", assistantMessageIndex: 0 });
  run.on("onAgentRunTerminalOutcome", "completed");
  run.end();
  await tick();
  assert.deepEqual(
    events.map((e) => [e.type, e.state]),
    [["session", "running"], ["turn", "running"], ["message", "done"], ["turn", "done"]],
  );
  assert.deepEqual(events[1]!.prompts, ["m-1"]);
  for (const e of events) {
    assert.equal(e.chat, "c-1");
    assert.ok(!("harness" in e) && !("seq" in e));
  }
  // An id made up from a count carries this start's mark, so a restart never uses it again.
  assert.match(events[2]!.item, /^message:b0\.1\.\d+$/);
  // The first session item carries OpenClaw's commands, and the choices of the model and the thinking level.
  const first = events[0]!.body;
  assert.equal(first["harness_version"], "2026.11.5");
  assert.deepEqual((first["commands"] as Json[]).map((c) => c["name"]), ["/think", "/new"]);
  const options = new Map((first["settings"] as Json[]).map((s) => [s["id"], s["options"]]));
  assert.deepEqual(options.get("model"), ["p/m", "p/m2"]);
  assert.deepEqual(options.get("think"), ["off", "low", "high"]);
});

test("a message OpenClaw does not let in is refused, and no later turn claims it", async () => {
  const { openclaw, events, connector } = setUp();
  openclaw.admission = "drop";
  await refused(connector.serve("prompt", prompt("m-1", "hello")), "failed");
  assert.equal(openclaw.runs.length, 0);
  openclaw.admission = "dispatch";
  // A turn started outside the run of a person's message, here by a typed /new, responds to every
  // message not yet claimed: there are none.
  await connector.serve("control", { request: "r-7", by: alice, action: { type: "new_session", chat } });
  openclaw.last().start("r-1");
  const turn = events.find((e) => e.type === "turn")!;
  assert.equal(turn.prompts, undefined);
});

test("an approval is answered through OpenClaw's gateway, for the core's approver only", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "run it"));
  const run = openclaw.last();
  run.start("r-1");
  run.on("onItemEvent", { toolCallId: "t-1", phase: "start", name: "exec", status: "running", commandBearing: true });
  run.on("onToolStart", { toolCallId: "t-1", name: "exec", phase: "start", args: { command: "touch x" } });
  const request = { id: "ap-1", request: { command: "touch x", toolCallId: "t-1", allowedDecisions: ["allow-once", "allow-always", "deny"] } };
  connector.adapterCall("approvalCapability.render.exec.buildPendingPayload", { request, target: { channel: "marmot", to: "c-1" } }, null);
  const asked = events.find((e) => e.type === "approval")!;
  assert.equal(asked.state, "waiting");
  assert.equal(asked.text, "touch x");
  // Nobody may approve through OpenClaw on their own.
  assert.equal(connector.mayApprove("u-1").authorized, false);
  await connector.serve("answer", { request: "r-9", item: asked.item, by: alice, given: { choice: "c0" } });
  assert.deepEqual(openclaw.approvals, [
    { approvalId: "ap-1", decision: "allow-once", approvalKind: "exec", channel: "marmot", accountId: "default", senderId: "u-1" },
  ]);
  assert.deepEqual(openclaw.authorized, [true]);
  assert.equal(connector.mayApprove("u-1").authorized, false);
  const done = events.filter((e) => e.item === asked.item).at(-1)!;
  assert.equal(done.state, "done");
  assert.deepEqual(done.settles, ["r-9"]);
  await refused(connector.serve("answer", { request: "r-10", item: asked.item, by: alice, given: { choice: "c0" } }), "already_answered");
  await refused(connector.serve("answer", { request: "r-11", item: "approval:nope", by: alice, given: { choice: "c0" } }), "unknown_item");
});

test("an approval whose press fails waits for another answer", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "run it"));
  const run = openclaw.last();
  run.start("r-1");
  run.on("onItemEvent", { toolCallId: "t-1", phase: "start", name: "exec", status: "running", commandBearing: true });
  run.on("onToolStart", { toolCallId: "t-1", name: "exec", phase: "start", args: { command: "touch x" } });
  const request = { id: "ap-1", request: { command: "touch x", toolCallId: "t-1", allowedDecisions: ["allow-once", "allow-always", "deny"] } };
  connector.adapterCall("approvalCapability.render.exec.buildPendingPayload", { request, target: { channel: "marmot", to: "c-1" } }, null);
  const asked = events.find((e) => e.type === "approval")!;
  // The gateway throws, as it does in the recording approval_press_unclaimed.
  openclaw.pressFails = new Error("approval not found");
  await refused(connector.serve("answer", { request: "r-9", item: asked.item, by: alice, given: { choice: "c0" } }), "failed");
  assert.equal(events.filter((e) => e.item === asked.item).at(-1)!.state, "waiting");
  await connector.serve("answer", { request: "r-10", item: asked.item, by: alice, given: { choice: "c2" } });
  assert.equal(openclaw.approvals.length, 2);
  const done = events.filter((e) => e.item === asked.item).at(-1)!;
  assert.deepEqual([done.state, done.settles], ["declined", ["r-10"]]);
  assert.ok(!events.some((e) => e.settles?.includes("r-9")), "the core settles a refused request");
  await refused(connector.serve("answer", { request: "r-11", item: asked.item, by: alice, given: { choice: "c0" } }), "already_answered");
});

test("a turn that ends while a press is in flight does not settle it; the press's refusal does", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "run it"));
  const run = openclaw.last();
  run.start("r-1");
  run.on("onItemEvent", { toolCallId: "t-1", phase: "start", name: "exec", status: "running", commandBearing: true });
  run.on("onToolStart", { toolCallId: "t-1", name: "exec", phase: "start", args: { command: "touch x" } });
  const request = { id: "ap-1", request: { command: "touch x", toolCallId: "t-1", allowedDecisions: ["allow-once", "allow-always", "deny"] } };
  connector.adapterCall("approvalCapability.render.exec.buildPendingPayload", { request, target: { channel: "marmot", to: "c-1" } }, null);
  const asked = events.find((e) => e.type === "approval")!;
  let release!: () => void;
  openclaw.pressWait = new Promise<void>((r) => (release = r));
  openclaw.pressFails = new Error("approval not found");
  const pressed = connector.serve("answer", { request: "r-9", item: asked.item, by: alice, given: { choice: "c0" } });
  await tick();
  // The turn is stopped while the press is in flight: the approval ends, and not by this answer.
  run.crash();
  await settle();
  const ended = events.filter((e) => e.item === asked.item).at(-1)!;
  assert.notEqual(ended.state, "waiting");
  release();
  await refused(pressed, "failed");
  assert.ok(!events.some((e) => e.settles?.includes("r-9")), "only the refusal settles the press");
});

test("a typed /approve, which the core checked, lets its sender approve while it runs", async () => {
  const { openclaw, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "/approve ap-1 allow-once"));
  assert.equal(connector.mayApprove("u-1").authorized, true);
  assert.equal(connector.mayApprove("u-2").authorized, false);
  openclaw.last().end();
  await tick();
  assert.equal(connector.mayApprove("u-1").authorized, false);
});

test("an interrupt is /stop from the person who sent it, settled by the end of the turn", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "sleep"));
  const run = openclaw.last();
  run.start("r-1");
  const turn = events.find((e) => e.type === "turn")!.turn!;
  await connector.serve("control", { request: "r-5", by: alice, action: { type: "interrupt", turn } });
  const stop = openclaw.last();
  assert.equal(stop.input.text, "/stop");
  assert.deepEqual(openclaw.contexts.at(-1)!["sender"], alice);
  stop.deliver({ text: "Agent was aborted." }, { kind: "final" });
  const end = events.at(-1)!;
  assert.deepEqual([end.type, end.state, end.settles], ["turn", "cancelled", ["r-5"]]);
  await refused(connector.serve("control", { request: "r-6", by: alice, action: { type: "interrupt", turn } }), "nothing_to_do");
});

test("a new session is /new, settled by the session's end, and its count outlives a restart", async () => {
  const { openclaw, events, connector, saved } = setUp();
  await connector.serve("prompt", prompt("m-1", "hello"));
  const run = openclaw.last();
  run.start("r-1");
  run.on("onAgentRunTerminalOutcome", "completed");
  run.end();
  await tick();
  await connector.serve("control", { request: "r-7", by: alice, action: { type: "new_session", chat } });
  const typed = openclaw.last();
  assert.equal(typed.input.text, "/new");
  typed.deliver({ text: "New session started.", isStatusNotice: true }, { kind: "final" });
  const ended = events.find((e) => e.type === "session" && e.state === "done")!;
  assert.equal(ended.session, KEY);
  assert.deepEqual(ended.settles, ["r-7"]);
  assert.deepEqual(saved.at(-1), [[KEY, 2]]);

  // The plugin starts again, with the count it saved: the next turn is in the new session.
  const again = setUp(saved.at(-1));
  await again.connector.serve("prompt", prompt("m-2", "hello again"));
  again.openclaw.last().start("r-2");
  assert.equal(again.events.find((e) => e.type === "turn")!.session, `${KEY}#2`);
});

test("a run that fails ends its turn as cancelled", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "hello"));
  const run = openclaw.last();
  run.start("r-1");
  run.crash();
  await tick();
  await tick();
  const end = events.at(-1)!;
  assert.deepEqual([end.type, end.state], ["turn", "cancelled"]);
});

test("a question is answered by one option, as a press", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "ask me"));
  const run = openclaw.last();
  run.start("r-1");
  run.on("onItemEvent", { toolCallId: "t-2", phase: "start", name: "ask_user", status: "running" });
  run.on("onToolStart", {
    toolCallId: "t-2",
    name: "ask_user",
    phase: "start",
    args: { questions: [{ id: "g", question: "Which?", options: [{ label: "Hello", value: "Hello" }, { label: "Hi", value: "Hi" }] }] },
  });
  run.deliver({ text: "Which?", channelData: { askUser: { questionId: "q-1", optionValues: ["Hello", "Hi"] } } }, { kind: "tool" });
  const asked = events.find((e) => e.type === "question")!;
  await refused(
    connector.serve("answer", { request: "r-3", item: asked.item, by: alice, given: { questions: [{ question: "g", options: [], text: "Hey" }] } }),
    "not_supported",
  );
  await connector.serve("answer", { request: "r-4", item: asked.item, by: alice, given: { questions: [{ question: "g", options: ["o1"] }] } });
  assert.deepEqual(openclaw.questions, [{ questionId: "q-1", optionValue: "Hi", senderId: "u-1" }]);
  const done = events.filter((e) => e.item === asked.item).at(-1)!;
  assert.deepEqual([done.state, done.settles], ["done", ["r-4"]]);
});

test("a model is set by /model, and settled once the stored entry shows it", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "hello"));
  const run = openclaw.last();
  run.start("r-1");
  run.on("onAgentRunTerminalOutcome", "completed");
  run.end();
  await tick();
  await refused(connector.serve("control", { request: "r-8", by: alice, action: { type: "set", session: KEY, setting: "fast", value: "on" } }), "not_supported");
  await connector.serve("control", { request: "r-8", by: alice, action: { type: "set", session: KEY, setting: "model", value: "p/m2" } });
  const typed = openclaw.last();
  assert.equal(typed.input.text, "/model p/m2");
  typed.deliver({ text: "Model set." }, { kind: "final" });
  typed.end();
  await tick();
  await tick();
  const shown = events.filter((e) => e.type === "session").at(-1)!;
  assert.deepEqual(shown.settles, ["r-8"]);
});

/** A chat with one finished turn, on model p/m at thinking level low. */
async function idle() {
  const s = setUp();
  await s.connector.serve("prompt", prompt("m-1", "hello"));
  const run = s.openclaw.last();
  run.start("r-1");
  run.on("onAgentRunTerminalOutcome", "completed");
  run.end();
  await tick();
  return s;
}

async function settle(): Promise<void> {
  for (let i = 0; i < 4; i += 1) await tick();
}

/** Every request settled so far, in order, and the event that settled each. */
function settled(events: CoreEvent[]): [string, string][] {
  return events.flatMap((e) => (e.settles ?? []).map((r): [string, string] => [r, e.type]));
}

test("a set that picks the value already in use is settled by OpenClaw's answer, at once", async () => {
  const { openclaw, events, connector } = await idle();
  openclaw.entry = { sessionId: "s-1", modelOverride: "m", providerOverride: "p" };
  await connector.serve("control", { request: "r-8", by: alice, action: { type: "set", session: KEY, setting: "model", value: "p/m" } });
  const typed = openclaw.last();
  typed.deliver({ text: "Model already p/m." }, { kind: "final" });
  typed.end();
  await settle();
  assert.deepEqual(settled(events), [["r-8", "notice"]]);
  assert.equal(events.at(-1)!.text, "Model already p/m.");
});

test("a refused set is settled by OpenClaw's answer, and a later change never settles it", async () => {
  const { openclaw, events, connector } = await idle();
  openclaw.entry = { sessionId: "s-1", thinkingLevel: "low" };
  await connector.serve("control", { request: "r-8", by: alice, action: { type: "set", session: KEY, setting: "think", value: "lots" } });
  const refusedRun = openclaw.last();
  refusedRun.deliver({ text: "Unrecognized thinking level." }, { kind: "final" });
  refusedRun.end();
  await settle();
  assert.deepEqual(settled(events), [["r-8", "notice"]]);

  openclaw.entry = { sessionId: "s-1", thinkingLevel: "high" };
  await connector.serve("control", { request: "r-9", by: alice, action: { type: "set", session: KEY, setting: "think", value: "high" } });
  const changed = openclaw.last();
  changed.deliver({ text: "Thinking level set to high." }, { kind: "final" });
  changed.end();
  await settle();
  // Each request is settled once, by its own command: the change by the session's edit.
  assert.deepEqual(settled(events), [["r-8", "notice"], ["r-9", "session"]]);
});

test("a set OpenClaw drops with no answer is settled by a notice the plugin makes", async () => {
  const { openclaw, events, connector } = await idle();
  openclaw.entry = { sessionId: "s-1", modelOverride: "m", providerOverride: "p" };
  await connector.serve("control", { request: "r-8", by: alice, action: { type: "set", session: KEY, setting: "model", value: "p/m3" } });
  openclaw.last().end({ admission: "dispatch", dispatched: false, dispatchResult: { beforeAgentRunBlocked: true } });
  await settle();
  assert.deepEqual(settled(events), [["r-8", "notice"]]);
  const notice = events.at(-1)!;
  assert.deepEqual([notice.title, notice.text, notice.body, notice.native ?? null], [
    "Command output",
    "OpenClaw did not answer /model.",
    { kind: "command_output", command: "/model" },
    null,
  ]);
});

test("the hello states every capability, and the core's requests are served over the socket", async () => {
  const dir = mkdtempSync(join(tmpdir(), "marmot-openclaw-"));
  const path = join(dir, "s.sock");
  const lines: Json[] = [];
  const server = createServer((socket) => {
    let buffered = "";
    socket.on("data", (data) => {
      buffered += data.toString("utf8");
      let at;
      while ((at = buffered.indexOf("\n")) !== -1) {
        const m = JSON.parse(buffered.slice(0, at)) as Json;
        buffered = buffered.slice(at + 1);
        lines.push(m);
        if (m["method"] === "hello") {
          socket.write(JSON.stringify({ jsonrpc: "2.0", id: m["id"], result: { core: { version: "0" }, agent: { name: "a" } } }) + "\n");
          socket.write(JSON.stringify({ jsonrpc: "2.0", id: 7, method: "prompt", params: prompt("m-1", "hello") }) + "\n");
        }
      }
    });
  });
  await new Promise<void>((r) => server.listen(path, r));
  try {
    const { openclaw, connector } = setUp();
    const core = await CoreConnection.open(path);
    await core.hello({ connector: { name: "c", version: "0" }, harness: { name: "openclaw" }, token: "t", capabilities: CAPABILITIES }, (method, params) =>
      connector.serve(method, params),
    );
    const deadline = Date.now() + 5000;
    while (!lines.some((m) => m["id"] === 7) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5));
    assert.deepEqual(lines.find((m) => m["id"] === 7), { jsonrpc: "2.0", id: 7, result: {} });
    assert.equal(openclaw.runs.length, 1);
    const hello = lines[0]!["params"] as Json;
    assert.deepEqual(Object.keys(hello["capabilities"] as Json).sort(), [
      "approval_commands",
      "approvals",
      "files_in",
      "interrupt",
      "mid_turn",
      "resume",
      "sessions",
    ]);
    core.close();
    await core.done;
  } finally {
    server.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("when the core restarts mid-turn, the connection's runs are cancelled and over before it reconnects", async () => {
  const dir = mkdtempSync(join(tmpdir(), "marmot-openclaw-"));
  const path = join(dir, "s.sock");
  let drop!: () => void;
  const server = createServer((socket) => {
    drop = () => socket.destroy();
    let buffered = "";
    socket.on("data", (data) => {
      buffered += data.toString("utf8");
      let at;
      while ((at = buffered.indexOf("\n")) !== -1) {
        const m = JSON.parse(buffered.slice(0, at)) as Json;
        buffered = buffered.slice(at + 1);
        if (m["method"] === "hello") {
          socket.write(JSON.stringify({ jsonrpc: "2.0", id: m["id"], result: { core: { version: "0" }, agent: { name: "a" } } }) + "\n");
          socket.write(JSON.stringify({ jsonrpc: "2.0", id: 7, method: "prompt", params: prompt("m-1", "sleep") }) + "\n");
        }
      }
    });
  });
  await new Promise<void>((r) => server.listen(path, r));
  let core: CoreConnection | undefined;
  try {
    const { openclaw, events, connector } = setUp();
    core = await CoreConnection.open(path);
    await core.hello({ connector: { name: "c", version: "0" }, harness: { name: "openclaw" }, token: "t", capabilities: CAPABILITIES }, (method, params) =>
      connector.serve(method, params),
    );
    const deadline = Date.now() + 5000;
    while (openclaw.runs.length === 0 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5));
    const run = openclaw.last();
    run.start("r-1");
    // OpenClaw goes on with the run until it is told to stop; then it reports and ends, a little later.
    const signal = run.input.replyOptions["abortSignal"];
    assert.ok(signal instanceof AbortSignal, "the run can be cancelled");
    let over = false;
    signal.addEventListener("abort", () =>
      setTimeout(() => {
        run.deliver({ text: "half an answer" }, { kind: "block", assistantMessageIndex: 0 });
        run.on("onAgentRunTerminalOutcome", "failed");
        run.end();
        over = true;
      }, 20),
    );
    const sent = events.length;
    // The core goes, and OpenClaw stays: the plugin's socket closes, as `connectOnce` sees it.
    drop();
    await core.done;
    await connector.stopped();
    assert.ok(signal.aborted, "the run was cancelled");
    assert.ok(over, "the run was over before another connection could take requests");
    assert.equal(events.length, sent, "nothing was sent after the socket closed");
  } finally {
    core?.close();
    server.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a message on its way when the core goes never runs, and the plugin waits for it and for every run", async () => {
  const dir = mkdtempSync(join(tmpdir(), "marmot-openclaw-"));
  const path = join(dir, "s.sock");
  let drop!: () => void;
  const server = createServer((socket) => {
    drop = () => socket.destroy();
    let buffered = "";
    socket.on("data", (data) => {
      buffered += data.toString("utf8");
      let at;
      while ((at = buffered.indexOf("\n")) !== -1) {
        const m = JSON.parse(buffered.slice(0, at)) as Json;
        buffered = buffered.slice(at + 1);
        if (m["method"] === "hello") {
          socket.write(JSON.stringify({ jsonrpc: "2.0", id: m["id"], result: { core: { version: "0" }, agent: { name: "a" } } }) + "\n");
          socket.write(JSON.stringify({ jsonrpc: "2.0", id: 7, method: "prompt", params: prompt("m-1", "hello") }) + "\n");
        }
      }
    });
  });
  await new Promise<void>((r) => server.listen(path, r));
  let core: CoreConnection | undefined;
  try {
    const { openclaw, events, connector } = setUp();
    let release!: () => void;
    openclaw.contextWait = new Promise<void>((r) => (release = r));
    core = await CoreConnection.open(path);
    await core.hello({ connector: { name: "c", version: "0" }, harness: { name: "openclaw" }, token: "t", capabilities: CAPABILITIES }, (method, params) =>
      connector.serve(method, params),
    );
    const deadline = Date.now() + 5000;
    while (openclaw.contexts.length === 0 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5));
    // The core goes while OpenClaw is still building the message's context.
    drop();
    await core.done;
    let stopped = false;
    const stopping = connector.stopped().then(() => (stopped = true));
    await settle();
    assert.equal(stopped, false, "the plugin waits for the message on its way");
    const sent = events.length;
    release();
    await stopping;
    assert.equal(openclaw.runs.length, 0, "the message never ran");
    assert.equal(events.length, sent, "nothing was sent after the socket closed");
    await refused(connector.serve("prompt", prompt("m-2", "hello")), "failed");
  } finally {
    core?.close();
    server.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a run that goes on past the wait is still waited for before the plugin connects again", async () => {
  const { openclaw, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "sleep"));
  const run = openclaw.last();
  run.start("r-1");
  let over = false;
  (run.input.replyOptions["abortSignal"] as AbortSignal).addEventListener("abort", () =>
    setTimeout(() => {
      run.end();
      over = true;
    }, 60),
  );
  await connector.stopped(10);
  assert.ok(over, "the run was over before stopped() returned");
});

test("an event still too large without its native message is cut to fit, keeping its prompts and settles", () => {
  const huge = `${"a".repeat(3 * 1024 * 1024)}${"z".repeat(3 * 1024 * 1024)}`;
  const event: CoreEvent = {
    chat: "c-1",
    session: KEY,
    item: "notice:b0.1.9",
    type: "notice",
    state: "done",
    title: "Command output",
    text: huge,
    body: { kind: "command_output", command: "/status" },
    native: { kind: "deliver", payload: huge },
    prompts: ["m-1"],
    settles: ["r-8"],
  };
  const size = (e: CoreEvent) => notificationSize("event", e);
  const fitted = fitEvent(event, size, MAX_MESSAGE)!;
  assert.equal(fitted.did, "cut to fit");
  assert.ok(size(fitted.event) <= MAX_MESSAGE);
  assert.ok(!("native" in fitted.event));
  assert.deepEqual([fitted.event.item, fitted.event.state, fitted.event.prompts, fitted.event.settles], ["notice:b0.1.9", "done", ["m-1"], ["r-8"]]);
  assert.ok(fitted.event.text.startsWith("aaa") && fitted.event.text.endsWith("zzz") && fitted.event.text.includes("cut to fit"));
  // The event given is left as it was.
  assert.equal(event.text, huge);
});

test("the socket client is the Claude Code driver's, with its opening comment and the Claude Code driver's own requests left out", () => {
  // Each package stands alone, so the client is copied, not shared (packages/openclaw/src/socket.ts). Only the
  // Claude Code driver sends the core requests of its own, to send a file (D-090); those parts of its copy are
  // marked, and this plugin's copy leaves them out.
  const own = /[ \t]*\/\/ Only the Claude Code driver sends the core requests of its own \(D-090\): begin\n[\s\S]*?[ \t]*\/\/ end\n/g;
  const code = (path: string) => {
    const file = readFileSync(new URL(path, import.meta.url), "utf8");
    return file.slice(file.indexOf("*/")).replace(own, "").replace(/\n{3,}/g, "\n\n");
  };
  const claude = readFileSync(new URL("../../claude-code/src/socket.ts", import.meta.url), "utf8");
  assert.equal(claude.match(own)?.length, 5, "the Claude Code driver's own parts are marked");
  assert.equal(code("../src/socket.ts"), code("../../claude-code/src/socket.ts"));
  assert.ok(!readFileSync(new URL("../src/socket.ts", import.meta.url), "utf8").includes("request(method"));
});

test("a live payload reaches the core whole: no entry and no level is left out", async () => {
  const { openclaw, events, connector } = setUp();
  await connector.serve("prompt", prompt("m-1", "hello"));
  const run = openclaw.last();
  run.start("r-1");
  let deep: Json = { leaf: "bottom" };
  for (let i = 0; i < 12; i += 1) deep = { down: deep };
  const many = Array.from({ length: 250 }, (_, i) => i);
  const looped: Json = { name: "loop" };
  looped["self"] = looped;
  run.deliver(
    { text: "Hi", many, deep, bytes: new Uint8Array([1, 2, 3]), odd: Number.NaN, big: 12n, looped, apiKey: "made-up" },
    { kind: "block", assistantMessageIndex: 0 },
  );
  const message = events.find((e) => e.type === "message")!;
  const [payload] = (message.native as { payload: Json[] }).payload;
  assert.deepEqual(payload!["many"], many);
  let at = payload!["deep"] as Json;
  for (let i = 0; i < 12; i += 1) at = at["down"] as Json;
  assert.deepEqual(at, { leaf: "bottom" });
  // What JSON cannot hold gets a stand-in, and a credential is still left out.
  assert.deepEqual(payload!["bytes"], { _bytes: 3, base64: "AQID" });
  assert.deepEqual([payload!["odd"], payload!["big"], payload!["looped"], payload!["apiKey"]], ["NaN", "12", { name: "loop", self: "<repeated>" }, "<redacted>"]);
});

test("overlapping commands are each answered and settled by their own run, whatever order the replies come in", async () => {
  const { openclaw, events, connector } = await idle();
  openclaw.entry = { sessionId: "s-1", thinkingLevel: "low" };
  await connector.serve("control", { request: "r-8", by: alice, action: { type: "set", session: KEY, setting: "think", value: "lots" } });
  const think = openclaw.last();
  await connector.serve("control", { request: "r-7", by: alice, action: { type: "new_session", chat } });
  const fresh = openclaw.last();
  // The later command is answered first.
  fresh.deliver({ text: "New session started.", isStatusNotice: true }, { kind: "final" });
  fresh.end();
  await settle();
  think.deliver({ text: "Unrecognized thinking level." }, { kind: "final" });
  think.end();
  await settle();
  assert.deepEqual(settled(events), [["r-7", "session"], ["r-8", "notice"]]);
  assert.deepEqual(
    events.filter((e) => e.type === "notice").map((e) => [e.body["command"], e.text]),
    [["/new", "New session started."], ["/think", "Unrecognized thinking level."]],
  );
  assert.ok(events.some((e) => e.type === "session" && e.session === KEY && e.state === "done"), "the session was replaced");
});
