// The transport, run against a fake core on a real Unix socket and a fake `query` in place of the
// SDK's: no network and no Claude Code. Every value here is made up.

import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test } from "node:test";
import { CAPABILITIES, run, type PermissionResult, type QueryLike, type UserMessage } from "../src/connector.ts";
import type { SendFile } from "../src/files.ts";
import { problems } from "./protocol.ts";

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
  /** The settings asked for through `setModel` and `setPermissionMode`. */
  readonly sets: string[] = [];
  /** When set, `setModel` and `setPermissionMode` wait for it. */
  hold: Promise<void> | undefined;
  private error: Error | undefined;
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
  /** Claude Code stops: the SDK's iterator throws. */
  fail(error: Error): void {
    this.error = error;
    this.wake?.();
  }
  async *[Symbol.asyncIterator](): AsyncIterator<unknown> {
    for (;;) {
      while (this.out.length > 0) yield this.out.shift();
      if (this.error !== undefined) throw this.error;
      if (this.closed) return;
      await new Promise<void>((r) => (this.wake = r));
    }
  }
  /** When set, `interrupt` answers only once it settles, as Claude Code can once the turn has stopped. */
  interruptHold: Promise<unknown> | undefined;
  interrupt(): Promise<unknown> {
    this.interrupts += 1;
    return this.interruptHold ?? Promise.resolve(undefined);
  }
  setPermissionMode(mode: string): Promise<void> {
    this.sets.push(`permission_mode=${mode}`);
    return this.hold ?? Promise.resolve();
  }
  setModel(model?: string): Promise<void> {
    this.sets.push(`model=${model}`);
    return this.hold ?? Promise.resolve();
  }
  close(): void {
    this.closed = true;
    this.wake?.();
  }
}

/**
 * The core's side of the socket: it takes the hello, sends requests and keeps the events. Every line
 * either way is checked against the core's own schema (`fixtures/socket`), and what is not as the core's
 * types say is kept in `invalid`, which every test ends by finding empty.
 */
class FakeCore {
  hello: Json | undefined;
  readonly invalid: string[] = [];
  readonly events: Json[] = [];
  /** The size of every line the connector sent, in bytes, without its newline. */
  readonly lines: number[] = [];
  private socket: Socket | undefined;
  private readonly responses = new Map<number, (m: Json) => void>();
  private nextId = 100;
  /** The files the connector asked to send, and what the core answers each. */
  readonly files: Json[] = [];
  answerFile: (params: Json) => Json = () => ({ result: {} });
  readonly server: Server;
  constructor(server: Server) {
    this.server = server;
    server.on("connection", (socket) => {
      this.socket = socket;
      let pending: Buffer[] = [];
      socket.on("data", (data: Buffer) => {
        let chunk = data;
        let at;
        while ((at = chunk.indexOf(0x0a)) !== -1) {
          const line = Buffer.concat([...pending, chunk.subarray(0, at)]);
          pending = [];
          chunk = chunk.subarray(at + 1);
          this.lines.push(line.length);
          const m = JSON.parse(line.toString("utf8")) as Json;
          for (const p of problems("FromConnector", m)) this.invalid.push(`from the connector: ${p}`);
          this.read(m);
        }
        if (chunk.length > 0) pending.push(chunk);
      });
    });
  }
  /** Writes a line as the core, checked as the core's. */
  private write(m: Json): void {
    for (const p of problems("FromCore", m)) this.invalid.push(`from the core: ${p}`);
    this.socket!.write(JSON.stringify(m) + "\n");
  }
  private read(m: Json): void {
    if (m["method"] === "hello") {
      this.hello = m;
      this.write({ jsonrpc: "2.0", id: m["id"], result: { core: { version: "0" }, agent: { name: "a" } } });
    } else if (m["method"] === "event") {
      this.events.push(m["params"] as Json);
    } else if (m["method"] === "send_file") {
      this.files.push(m["params"] as Json);
      const answer = this.answerFile(m["params"] as Json);
      this.write({ jsonrpc: "2.0", id: m["id"], ...answer });
    } else if (typeof m["id"] === "number") {
      this.responses.get(m["id"])?.(m);
    }
  }
  request(method: string, params: Json): Promise<Json> {
    const id = (this.nextId += 1);
    return new Promise((resolve) => {
      this.responses.set(id, resolve);
      this.write({ jsonrpc: "2.0", id, method, params });
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

async function start(cwd = "/nowhere"): Promise<{
  core: FakeCore;
  queries: FakeQuery[];
  running: Promise<void>;
  warnings: string[];
  done: () => Promise<void>;
}> {
  const home = mkdtempSync(join(tmpdir(), "marmot-agent-claude-test-"));
  mkdirSync(join(home, "socket"));
  writeFileSync(join(home, "connector.token"), `${TOKEN}\n`);
  const server = createServer();
  await new Promise<void>((r) => server.listen(join(home, "socket", "marmot-agent.sock"), r));
  const core = new FakeCore(server);
  const queries: FakeQuery[] = [];
  const running = run({
    home,
    cwd,
    model: "model-x",
    permissionMode: "default",
    query: ({ prompt, options }) => {
      const q = new FakeQuery(options, prompt);
      queries.push(q);
      return q;
    },
    version: "0.0.0",
    harnessVersion: "2.1.0",
    warn: (kind) => warnings.push(kind),
    // A stand-in for the SDK's MCP server: the test calls the tool's handler itself.
    fileServer: (send) => ({ send }),
  });
  const warnings: string[] = [];
  const done = async () => {
    stopping.delete(done);
    core.close();
    await running;
    rmSync(home, { recursive: true, force: true });
    assert.deepEqual(core.invalid, [], "every line either way is as the core's types say");
  };
  // A test that fails before it stops the connector must not keep the others waiting.
  stopping.add(done);
  await until(() => core.hello !== undefined, "the hello");
  return { core, queries, running, warnings, done };
}

const stopping = new Set<() => Promise<void>>();
afterEach(async () => {
  for (const done of [...stopping]) await done();
});

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
  // The folder it works in when a chat chose none, so the core can let a chat move it (D-101).
  assert.equal(params["folder"], "/nowhere");
  await done();
});

test("a chat's folder starts its next query there, and a file it names is taken from there", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  assert.equal(q.options["cwd"], "/nowhere", "a chat that chose none works in the connector's folder");
  // A running query stays where it is: the folder applies from the next session (D-101).
  await core.request("prompt", { ...PROMPT, message: "m2", chat: { ...PROMPT.chat, folder: "/elsewhere/a" } });
  assert.equal(queries.length, 1);
  q.say(DONE);
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "done") !== undefined, "the turn's end");
  const moved = { type: "new_session", chat: { ...PROMPT.chat, folder: "/elsewhere/a" }, session: "s1" };
  assert.deepEqual((await core.request("control", { request: "n1", by: { id: "p1", name: "Alice" }, action: moved }))["result"], {});
  await core.request("prompt", { ...PROMPT, message: "m3", chat: { ...PROMPT.chat, folder: "/elsewhere/a" } });
  assert.equal(queries.length, 2);
  const fresh = queries[1]!;
  assert.equal(fresh.options["cwd"], "/elsewhere/a");
  assert.equal("resume" in fresh.options, false);
  const { send } = (fresh.options["mcpServers"] as Record<string, { send: SendFile }>)["marmot"]!;
  await send("out.txt");
  assert.deepEqual(core.files[0], { chat: "c1", path: "/elsewhere/a/out.txt" });
  // A folder that is not a whole path is refused, on a prompt and on a new session.
  const relative = { ...PROMPT.chat, id: "c2", folder: "a" };
  const refused = await core.request("prompt", { ...PROMPT, message: "m4", chat: relative });
  assert.equal(((refused["error"] as Json)["data"] as Json)["reason"], "invalid");
  const bad = { type: "new_session", chat: relative, session: null };
  const control = await core.request("control", { request: "n2", by: { id: "p1", name: "Alice" }, action: bad });
  assert.equal(((control["error"] as Json)["data"] as Json)["reason"], "invalid");
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

/** What every query must be given, fresh or resumed. */
function assertQueryOptions(q: FakeQuery): void {
  // No allow rule or hook from the owner's own settings may approve or run anything in a chat.
  assert.deepEqual(q.options["settingSources"], []);
  // As the reasoning recording was made: without it, thinking blocks arrive empty.
  assert.deepEqual(q.options["thinking"], { type: "adaptive", display: "summarized" });
}

test("a query loads no Claude Code settings files and asks for summarized reasoning", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  assertQueryOptions(q);
  await done();
});

test("the agent sends a file: the core is asked with the chat and a whole path, and the model hears how it went", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  // The tool needs nobody's approval in the chat: the core decides what may be sent (D-090).
  const input = { path: "out/notes.txt" };
  assert.deepEqual(await q.ask("mcp__marmot__send_file", input, { requestId: "r9" }), { behavior: "allow", updatedInput: input });
  assert.equal(core.event((e) => e["type"] === "approval"), undefined, "no approval is asked in the chat");
  const { send } = (q.options["mcpServers"] as Record<string, { send: SendFile }>)["marmot"]!;
  assert.deepEqual(await send("out/notes.txt"), { text: "Sent.", failed: false });
  assert.deepEqual(core.files[0], { chat: "c1", path: "/nowhere/out/notes.txt" });
  const detail = "The file is outside the folders the agent may send files from.";
  core.answerFile = () => ({ error: { code: -32000, message: "not_allowed", data: { reason: "not_allowed", detail } } });
  assert.deepEqual(await send("/elsewhere/secret.txt"), { text: `Not sent: ${detail}`, failed: true });
  assert.deepEqual(core.files[1], { chat: "c1", path: "/elsewhere/secret.txt" });
  await done();
});

test("a file named relative to a working folder given as `.` reaches the core as a whole path, its `..` kept", async () => {
  const { core, queries, done } = await start(".");
  const { q } = await turn(core, queries);
  assert.equal(q.options["cwd"], process.cwd(), "Claude Code is given the folder whole");
  const { send } = (q.options["mcpServers"] as Record<string, { send: SendFile }>)["marmot"]!;
  await send("link/../notes.txt");
  // Not shortened to `notes.txt`: if `link` leads elsewhere, `..` leads elsewhere too, and the core follows it.
  assert.deepEqual(core.files[0], { chat: "c1", path: `${process.cwd()}/link/../notes.txt` });
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

test("an interrupt is taken once Claude Code has it, not once Claude Code answers", async () => {
  const { core, queries, done } = await start();
  const { q, turn: id } = await turn(core, queries);
  let answer!: (value: unknown) => void;
  q.interruptHold = new Promise((r) => (answer = r));
  const taken = core.request("control", { request: "i1", by: { id: "p1", name: "Alice" }, action: { type: "interrupt", turn: id } });
  const first = await Promise.race([taken, new Promise((r) => setTimeout(() => r("still waiting"), 3000))]);
  assert.deepEqual((first as Json)["result"], {});
  assert.equal(q.interrupts, 1);
  answer(undefined);
  q.say({ ...DONE, subtype: "error_during_execution", is_error: true });
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "cancelled") !== undefined, "the turn's end");
  assert.deepEqual(core.event((e) => e["type"] === "turn" && e["state"] === "cancelled")!["settles"], ["i1"]);
  await done();
});

test("a turn that ends on its own before Claude Code refuses a taken interrupt settles nothing", async () => {
  const { core, queries, done } = await start();
  const { q, turn: id } = await turn(core, queries);
  let refuse!: (error: Error) => void;
  q.interruptHold = new Promise((_, reject) => (refuse = reject));
  const taken = await core.request("control", { request: "i1", by: ALICE, action: { type: "interrupt", turn: id } });
  assert.deepEqual(taken["result"], {});
  // The turn finished as it would have: the interrupt had no effect, and the core settles it (D-087).
  q.say(DONE);
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "done") !== undefined, "the turn's end");
  assert.equal("settles" in core.event((e) => e["type"] === "turn" && e["state"] === "done")!, false);
  refuse(new Error("no"));
  await done();
});

test("an interrupt Claude Code refuses at once is refused, and its turn's end settles nothing", async () => {
  const { core, queries, done } = await start();
  const { q, turn: id } = await turn(core, queries);
  const no = Promise.reject(new Error("no"));
  no.catch(() => undefined);
  q.interruptHold = no;
  const refused = await core.request("control", { request: "i1", by: { id: "p1", name: "Alice" }, action: { type: "interrupt", turn: id } });
  assert.equal(((refused["error"] as Json)["data"] as Json)["reason"], "failed");
  q.say(DONE);
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "done") !== undefined, "the turn's end");
  assert.equal("settles" in core.event((e) => e["type"] === "turn" && e["state"] === "done")!, false);
  await done();
});

test("two approvals Claude Code names no request for, or an empty one, are two items, each answered on its own", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  const first = q.ask("Bash", BASH, { toolUseID: "t1" });
  const second = q.ask("Bash", { command: "touch y" }, { toolUseID: "t2", requestId: "" });
  const approvals = () => [...new Set(core.events.filter((e) => e["type"] === "approval").map((e) => e["item"] as string))];
  await until(() => approvals().length === 2, "two approvals");
  const [one, two] = approvals();
  const answers = [
    await core.request("answer", { request: "a2", item: two, by: { id: "p1", name: "Alice" }, given: { choice: "c1" } }),
    await core.request("answer", { request: "a1", item: one, by: { id: "p1", name: "Alice" }, given: { choice: "c0" } }),
  ];
  assert.deepEqual(answers.map((a) => a["result"]), [{}, {}]);
  assert.deepEqual(await first, { behavior: "allow", updatedInput: BASH });
  assert.equal((await second).behavior, "deny");
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

test("a run that fails ends its turn cancelled, and the next prompt resumes the session in a turn of its own", async () => {
  const { core, queries, done } = await start();
  const { q, turn: id } = await turn(core, queries);
  const asked = q.ask("Bash", BASH, { suggestions: [SUGGESTION], toolUseID: "t1", requestId: "r1" });
  await until(() => core.event((e) => e["item"] === "approval:r1") !== undefined, "the approval");
  // Claude Code crashes, or is killed, in the middle of the turn. The connector stays.
  q.fail(new Error("exited"));
  assert.equal((await asked).behavior, "deny");
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "cancelled") !== undefined, "the turn's end");
  assert.equal(core.event((e) => e["type"] === "turn" && e["state"] === "cancelled")!["turn"], id);
  assert.ok(core.event((e) => e["item"] === "approval:r1" && e["state"] === "cancelled"));
  // The session is not over: it is taken up again.
  assert.equal(core.event((e) => e["type"] === "session" && e["state"] === "done"), undefined);
  assert.deepEqual((await core.request("prompt", { ...PROMPT, message: "m2" }))["result"], {});
  assert.equal(queries.length, 2);
  const resumed = queries[1]!;
  assert.equal(resumed.options["resume"], "s1");
  assertQueryOptions(resumed);
  await until(() => resumed.received.length === 1, "the user message");
  // No turn is running, so the message does not try to end one.
  assert.equal(resumed.received[0]!.priority, undefined);
  resumed.say(init("u2"));
  await until(() => core.event((e) => e["type"] === "turn" && e["turn"] === "s1:u2") !== undefined, "the new turn");
  const next = core.event((e) => e["type"] === "turn" && e["turn"] === "s1:u2")!;
  assert.equal(next["state"], "running");
  assert.deepEqual(next["prompts"], ["m2"]);
  await done();
});

test("a prompt that asks to resume another chat's session is refused, even once that session is closed", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  q.say(DONE);
  const other = { ...PROMPT, message: "m2", chat: { id: "c2", group: true, name: null }, session: "s1" };
  const open = await core.request("prompt", other);
  assert.equal(((open["error"] as Json)["data"] as Json)["reason"], "invalid");
  const action = { type: "new_session", chat: { id: "c1", group: true, name: null }, session: "s1" };
  await core.request("control", { request: "n1", by: { id: "p1", name: "Alice" }, action });
  const closed = await core.request("prompt", { ...other, message: "m3" });
  assert.equal(((closed["error"] as Json)["data"] as Json)["reason"], "invalid");
  // No query was started for either.
  assert.equal(queries.length, 1);
  await done();
});

test("a resumed session gets the model and permission mode its item last showed", async () => {
  const { core, queries, done } = await start();
  await core.request("prompt", PROMPT);
  const q = queries[0]!;
  q.say({ ...init("u1"), model: "model-b", permissionMode: "acceptEdits" });
  await until(() => core.event((e) => e["type"] === "turn") !== undefined, "the turn");
  q.fail(new Error("exited"));
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "cancelled") !== undefined, "the turn's end");
  await core.request("prompt", { ...PROMPT, message: "m2" });
  const resumed = queries[1]!;
  assert.equal(resumed.options["resume"], "s1");
  assert.equal(resumed.options["model"], "model-b");
  assert.equal(resumed.options["permissionMode"], "acceptEdits");
  await done();
});

test("a session this connector has not seen is resumed with its own settings, and a fresh one gets the startup ones", async () => {
  const { core, queries, done } = await start();
  await core.request("prompt", { ...PROMPT, session: "s9" });
  const resumed = queries[0]!;
  assert.equal(resumed.options["resume"], "s9");
  assert.equal("model" in resumed.options, false);
  assert.equal("permissionMode" in resumed.options, false);
  await core.request("prompt", { ...PROMPT, message: "m2", chat: { id: "c2", group: true, name: null } });
  const fresh = queries[1]!;
  assert.equal("resume" in fresh.options, false);
  assert.equal(fresh.options["model"], "model-x");
  assert.equal(fresh.options["permissionMode"], "default");
  await done();
});

/** The value a session event shows for one setting. */
function shows(e: Json, setting: string): unknown {
  const settings = (e["body"] as Json)["settings"] as Json[];
  return settings.find((s) => s["id"] === setting)?.["value"];
}

const ALICE = { id: "p1", name: "Alice" };

test("a set settles on the session edit that shows its value, and not on an unrelated edit", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  let release!: () => void;
  q.hold = new Promise((r) => (release = r));
  const action = { type: "set", session: "s1", setting: "model", value: "model-b" };
  const taken = core.request("control", { request: "x1", by: ALICE, action });
  await until(() => q.sets.length === 1, "the switch");
  // While the switch is under way the permission mode changes: the session item is edited.
  q.say({ type: "system", subtype: "status", status: null, permissionMode: "acceptEdits", session_id: "s1", uuid: "u9" });
  const unrelated = (e: Json) => e["type"] === "session" && shows(e, "permission_mode") === "acceptEdits";
  await until(() => core.event(unrelated) !== undefined, "the unrelated edit");
  assert.equal("settles" in core.event(unrelated)!, false);
  release();
  assert.deepEqual((await taken)["result"], {});
  const switched = (e: Json) => e["type"] === "session" && shows(e, "model") === "model-b";
  await until(() => core.event(switched) !== undefined, "the switch shown");
  assert.deepEqual(core.event(switched)!["settles"], ["x1"]);
  await done();
});

test("a model switch between turns shows and settles at once", async () => {
  const { core, queries, done } = await start();
  const { q } = await turn(core, queries);
  q.say(DONE);
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "done") !== undefined, "the turn's end");
  const action = { type: "set", session: "s1", setting: "model", value: "model-b" };
  assert.deepEqual((await core.request("control", { request: "x2", by: ALICE, action }))["result"], {});
  assert.deepEqual(q.sets, ["model=model-b"]);
  const switched = (e: Json) => e["type"] === "session" && shows(e, "model") === "model-b";
  await until(() => core.event(switched) !== undefined, "the switch shown");
  const shown = core.event(switched)!;
  assert.deepEqual(shown["settles"], ["x2"]);
  assert.equal(shown["state"], "running");
  await done();
});

const LIMIT = 4 * 1024 * 1024;

test("an approval too large for the socket goes without its native message, cut to fit, and can still be answered", async () => {
  const { core, queries, warnings, done } = await start();
  const { q } = await turn(core, queries);
  const huge = { command: `${"a".repeat(3 * 1024 * 1024)}${"z".repeat(3 * 1024 * 1024)}`, description: "make a file" };
  const asked = q.ask("Bash", huge, { suggestions: [SUGGESTION], toolUseID: "t1", requestId: "r1" });
  await until(() => core.event((e) => e["item"] === "approval:r1") !== undefined, "the approval");
  const waiting = core.event((e) => e["item"] === "approval:r1")!;
  assert.equal(waiting["state"], "waiting");
  assert.equal("native" in waiting, false);
  // The command is the approval's text: its start and its end are kept, with a mark where it was cut.
  const text = waiting["text"] as string;
  assert.ok(text.startsWith("aaaa") && text.endsWith("zzzz") && text.includes("cut"), "cut in the middle");
  assert.ok(text.length < huge.command.length);
  assert.equal(((waiting["body"] as Json)["choices"] as Json[]).length, 3);
  assert.ok(Math.max(...core.lines) <= LIMIT, "every line within the limit");
  assert.ok(warnings.length > 0);
  // Claude Code still gets the whole command back when it is allowed.
  await core.request("answer", { request: "a1", item: "approval:r1", by: ALICE, given: { choice: "c0" } });
  assert.deepEqual(await asked, { behavior: "allow", updatedInput: huge });
  await until(() => core.event((e) => e["item"] === "approval:r1" && e["state"] === "done") !== undefined, "the answer");
  const answered = core.event((e) => e["item"] === "approval:r1" && e["state"] === "done")!;
  assert.deepEqual(answered["settles"], ["a1"]);
  assert.equal("native" in answered, false);
  assert.ok(Math.max(...core.lines) <= LIMIT, "every line within the limit");
  await done();
});

test("a turn too large for the socket keeps its prompts and what it settles", async () => {
  const { core, queries, done } = await start();
  await core.request("prompt", PROMPT);
  const q = queries[0]!;
  // A field the translator does not read makes the native message, and nothing else, too large.
  q.say({ ...init("u1"), padding: "p".repeat(LIMIT) });
  await until(() => core.event((e) => e["type"] === "turn") !== undefined, "the turn");
  const opened = core.event((e) => e["type"] === "turn")!;
  assert.deepEqual(opened["prompts"], ["m1"]);
  assert.equal("native" in opened, false);
  await core.request("control", { request: "i1", by: ALICE, action: { type: "interrupt", turn: opened["turn"] } });
  q.say({ ...DONE, subtype: "error_during_execution", is_error: true, errors: ["e".repeat(2 * LIMIT)] });
  await until(() => core.event((e) => e["type"] === "turn" && e["state"] === "cancelled") !== undefined, "the turn's end");
  const ended = core.event((e) => e["type"] === "turn" && e["state"] === "cancelled")!;
  assert.deepEqual(ended["settles"], ["i1"]);
  assert.ok((ended["text"] as string).includes("cut"));
  assert.ok(Math.max(...core.lines) <= LIMIT, "every line within the limit");
  await done();
});
