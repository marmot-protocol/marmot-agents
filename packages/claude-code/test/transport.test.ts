// The transport, run against a fake core on a real Unix socket and a fake `query` in place of the
// SDK's: no network and no Claude Code. Every value here is made up.

import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { CAPABILITIES, run, type PermissionResult, type QueryLike, type UserMessage } from "../src/connector.ts";

type Json = Record<string, unknown>;

const TOKEN = "ab".repeat(32);

async function until(ready: () => boolean, what: string): Promise<void> {
  const deadline = Date.now() + 5000;
  while (!ready()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 2));
  }
}

/** Claude Code as the SDK presents it, driven by the test. */
class FakeQuery implements QueryLike {
  readonly received: UserMessage[] = [];
  interrupts = 0;
  closed = false;
  private out: unknown[] = [];
  private wake: (() => void) | undefined;
  readonly options: Json;
  constructor(options: Json, prompt: AsyncIterable<UserMessage>) {
    this.options = options;
    void (async () => {
      for await (const m of prompt) this.received.push(m);
    })();
  }
  /** Claude Code yields a message. */
  say(m: unknown): void {
    this.out.push(m);
    this.wake?.();
  }
  /** Claude Code asks permission through `canUseTool`. */
  ask(toolName: string, input: Json, options: Json): Promise<PermissionResult> {
    const canUseTool = this.options["canUseTool"] as (t: string, i: Json, o: Json) => Promise<PermissionResult>;
    return canUseTool(toolName, input, { signal: new AbortController().signal, ...options });
  }
  async *[Symbol.asyncIterator](): AsyncIterator<unknown> {
    for (;;) {
      while (this.out.length > 0) yield this.out.shift();
      if (this.closed) return;
      await new Promise<void>((r) => (this.wake = r));
    }
  }
  interrupt(): Promise<unknown> {
    this.interrupts += 1;
    return Promise.resolve(undefined);
  }
  setPermissionMode(): Promise<void> {
    return Promise.resolve();
  }
  setModel(): Promise<void> {
    return Promise.resolve();
  }
  close(): void {
    this.closed = true;
    this.wake?.();
  }
}

/** The core's side of the socket: it takes the hello, sends requests and keeps the events. */
class FakeCore {
  hello: Json | undefined;
  readonly events: Json[] = [];
  private socket: Socket | undefined;
  private readonly responses = new Map<number, (m: Json) => void>();
  private nextId = 100;
  readonly server: Server;
  constructor(server: Server) {
    this.server = server;
    server.on("connection", (socket) => {
      this.socket = socket;
      let pending = "";
      socket.on("data", (chunk) => {
        pending += chunk.toString("utf8");
        let at;
        while ((at = pending.indexOf("\n")) !== -1) {
          const m = JSON.parse(pending.slice(0, at)) as Json;
          pending = pending.slice(at + 1);
          this.read(m);
        }
      });
    });
  }
  private read(m: Json): void {
    if (m["method"] === "hello") {
      this.hello = m;
      this.socket!.write(JSON.stringify({ jsonrpc: "2.0", id: m["id"], result: { core: { version: "0" }, agent: { name: "a" } } }) + "\n");
    } else if (m["method"] === "event") {
      this.events.push(m["params"] as Json);
    } else if (typeof m["id"] === "number") {
      this.responses.get(m["id"])?.(m);
    }
  }
  request(method: string, params: Json): Promise<Json> {
    const id = (this.nextId += 1);
    return new Promise((resolve) => {
      this.responses.set(id, resolve);
      this.socket!.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    });
  }
  event(where: (e: Json) => boolean): Json | undefined {
    return this.events.find(where);
  }
  close(): void {
    this.socket?.destroy();
    this.server.close();
  }
}

async function start(): Promise<{ core: FakeCore; queries: FakeQuery[]; running: Promise<void>; done: () => Promise<void> }> {
  const home = mkdtempSync(join(tmpdir(), "marmot-agent-claude-test-"));
  mkdirSync(join(home, "socket"));
  writeFileSync(join(home, "connector.token"), `${TOKEN}\n`);
  const server = createServer();
  await new Promise<void>((r) => server.listen(join(home, "socket", "marmot-agent.sock"), r));
  const core = new FakeCore(server);
  const queries: FakeQuery[] = [];
  const running = run({
    home,
    cwd: "/nowhere",
    permissionMode: "default",
    query: ({ prompt, options }) => {
      const q = new FakeQuery(options, prompt);
      queries.push(q);
      return q;
    },
    version: "0.0.0",
    harnessVersion: "2.1.0",
    warn: () => {},
  });
  await until(() => core.hello !== undefined, "the hello");
  return {
    core,
    queries,
    running,
    done: async () => {
      core.close();
      await running;
      rmSync(home, { recursive: true, force: true });
    },
  };
}

const PROMPT = { message: "m1", text: "list the files", chat: { id: "c1", group: true, name: null }, sender: { id: "p1", name: "Alice" }, attachments: [] };

function init(uuid: string): Json {
  return { type: "system", subtype: "init", session_id: "s1", uuid, model: "model-a", permissionMode: "default", claude_code_version: "2.1.0", slash_commands: [] };
}

const DONE = { type: "result", subtype: "success", is_error: false, session_id: "s1", duration_ms: 1, usage: {} };

/** A running turn, started by a prompt. */
async function turn(core: FakeCore, queries: FakeQuery[]): Promise<{ q: FakeQuery; turn: string }> {
  assert.deepEqual(await core.request("prompt", PROMPT), { jsonrpc: "2.0", id: 101, result: {} });
  const q = queries[0]!;
  q.say(init("u1"));
  await until(() => core.event((e) => e["type"] === "turn") !== undefined, "the turn");
  return { q, turn: core.event((e) => e["type"] === "turn")!["turn"] as string };
}

const BASH = { command: "touch x", description: "make a file" };
const SUGGESTION = { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "touch x" }], behavior: "allow", destination: "localSettings" };

test("the hello names the protocol, the token, Claude Code and its seven capabilities", async () => {
  const { core, done } = await start();
  const params = core.hello!["params"] as Json;
  assert.equal(params["protocol"], "marmot.agent-control.v3");
  assert.equal(params["token"], TOKEN);
  assert.deepEqual(params["harness"], { name: "claude-code", version: "2.1.0" });
  assert.deepEqual(params["connector"], { name: "marmot-agent-claude-code", version: "0.0.0" });
  assert.deepEqual(params["capabilities"], {
    interrupt: true,
    mid_turn: "restart",
    sessions: "core",
    resume: true,
    approvals: true,
    files_in: "none",
    approval_commands: [],
  });
  assert.deepEqual(params["capabilities"], CAPABILITIES);
  await done();
});

test("a prompt becomes a user message, and the turn it starts names it", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  assert.equal(q.options["permissionMode"], "default");
  assert.equal("resume" in q.options, false);
  await until(() => q.received.length === 1, "the user message");
  // In a group the model reads who wrote it (D-062).
  assert.deepEqual(q.received[0]!.message, { role: "user", content: "Alice: list the files" });
  const opened = core.event((e) => e["type"] === "turn")!;
  assert.deepEqual(opened["prompts"], ["m1"]);
  assert.equal(opened["chat"], "c1");
  assert.equal("harness" in opened, false);
  assert.equal("seq" in opened, false);
  q.say(DONE);
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "done") !== undefined, "the turn's end");
  assert.equal("prompts" in core.event((e) => e["type"] === "turn" && e["state"] === "done")!, false);
  await done();
});

test("an approval answered with c0 allows once, and its next event settles the answer", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  const asked = q.ask("Bash", BASH, { suggestions: [SUGGESTION], toolUseID: "t1", requestId: "r1" });
  await until(() => core.event((e) => e["item"] === "approval:r1") !== undefined, "the approval");
  const answered = await core.request("answer", { request: "a1", item: "approval:r1", by: { id: "p1", name: "Alice" }, given: { choice: "c0" } });
  assert.deepEqual(answered["result"], {});
  assert.deepEqual(await asked, { behavior: "allow", updatedInput: BASH });
  await until(() => core.event((e) => e["item"] === "approval:r1" && e["state"] === "done") !== undefined, "the answer");
  const shown = core.event((e) => e["item"] === "approval:r1" && e["state"] === "done")!;
  assert.deepEqual(shown["settles"], ["a1"]);
  assert.deepEqual((shown["body"] as Json)["answer"], { choice: "c0" });
  // Answered once: the second answer is refused.
  const again = await core.request("answer", { request: "a2", item: "approval:r1", by: { id: "p1", name: "Alice" }, given: { choice: "c0" } });
  assert.equal(((again["error"] as Json)["data"] as Json)["reason"], "already_answered");
  await done();
});

test("a standing choice allows with Claude Code's suggestion, and the last choice denies", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  const first = q.ask("Bash", BASH, { suggestions: [SUGGESTION], toolUseID: "t1", requestId: "r1" });
  const second = q.ask("Bash", BASH, { suggestions: [SUGGESTION], toolUseID: "t2", requestId: "r2" });
  await until(() => core.event((e) => e["item"] === "approval:r2") !== undefined, "the approvals");
  await core.request("answer", { request: "a2", item: "approval:r2", by: { id: "p1", name: "Alice" }, given: { choice: "c2" } });
  await core.request("answer", { request: "a1", item: "approval:r1", by: { id: "p1", name: "Alice" }, given: { choice: "c1" } });
  assert.deepEqual(await first, { behavior: "allow", updatedInput: BASH, updatedPermissions: [SUGGESTION] });
  assert.equal((await second).behavior, "deny");
  const denied = core.event((e) => e["item"] === "approval:r2" && e["state"] === "declined")!;
  assert.deepEqual(denied["settles"], ["a2"]);
  const allowed = core.event((e) => e["item"] === "approval:r1" && e["state"] === "done")!;
  assert.deepEqual(allowed["settles"], ["a1"]);
  assert.deepEqual((allowed["body"] as Json)["answer"], { choice: "c1" });
  await done();
});

test("an interrupt reaches Claude Code and settles on the event that ends the turn", async () => {
  const { core, queries, done } = await start();
  const { q, turn: id } = await turn(core, queries);
  const taken = await core.request("control", { request: "i1", by: { id: "p1", name: "Alice" }, action: { type: "interrupt", turn: id } });
  assert.deepEqual(taken["result"], {});
  assert.equal(q.interrupts, 1);
  q.say({ ...DONE, subtype: "error_during_execution", is_error: true });
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "cancelled") !== undefined, "the turn's end");
  assert.deepEqual(core.event((e) => e["type"] === "turn" && e["state"] === "cancelled")!["settles"], ["i1"]);
  // With no turn running there is nothing to interrupt.
  const late = await core.request("control", { request: "i2", by: { id: "p1", name: "Alice" }, action: { type: "interrupt", turn: id } });
  assert.equal(((late["error"] as Json)["data"] as Json)["reason"], "nothing_to_do");
  await done();
});

test("a new session ends the query, settles on the end of the session, and the next prompt starts afresh", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  q.say(DONE);
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "done") !== undefined, "the turn's end");
  const action = { type: "new_session", chat: { id: "c1", group: true, name: null }, session: "s1" };
  assert.deepEqual((await core.request("control", { request: "n1", by: { id: "p1", name: "Alice" }, action }))["result"], {});
  assert.equal(q.closed, true);
  const ended = core.event((e) => e["type"] === "session" && e["state"] === "done")!;
  assert.deepEqual(ended["settles"], ["n1"]);
  await core.request("prompt", { ...PROMPT, message: "m2" });
  assert.equal(queries.length, 2);
  assert.equal("resume" in queries[1]!.options, false);
  await done();
});

test("an answer to an item the connector does not know is refused, as is an unknown control", async () => {
  const { core, done } = await start();
  const answered = await core.request("answer", { request: "a1", item: "approval:nope", by: { id: "p1", name: "Alice" }, given: { choice: "c0" } });
  assert.equal(((answered["error"] as Json)["data"] as Json)["reason"], "unknown_item");
  const own = await core.request("control", { request: "o1", by: { id: "p1", name: "Alice" }, action: { type: "own", session: "s1", id: "x" } });
  assert.equal(((own["error"] as Json)["data"] as Json)["reason"], "not_supported");
  await done();
});
