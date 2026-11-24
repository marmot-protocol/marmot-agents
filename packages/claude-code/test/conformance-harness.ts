// The Claude Code driver for the driver conformance suite (`crates/marmot-agent-conformance`, "the socket
// connectors"): the driver's own transport, `run` in src/connector.ts, on the core's real socket, with a
// scripted `query` in place of the SDK's. No Claude Code, no model and no network.
//
//   MARMOT_AGENT_HOME=<the agent's home> MARMOT_CONFORMANCE_STATE=<a folder> node test/conformance-harness.ts
//
// The scripted Claude Code reads the same words in a prompt as every fake of the suite: `#ask` asks to run
// a command (`canUseTool`), `#quiz` asks a question (AskUserQuestion), `#sleep` runs until interrupted;
// anything else is answered in words that echo the prompt. Each turn opens with `init`, as the SDK's does.
//
// It tells the suite what it does on standard output, one JSON object a line, which the driver uses for
// nothing else (it says what went wrong on standard error), and ends when its standard input closes, as
// when the suite goes:
//
// - `{"heard": {...}}`: what this Claude Code understood, in the suite's shapes (`Heard`);
// - `{"frame": <kind>}`: each call the driver made on it, after what it heard in it;
// - the process ending is Claude Code ending: the driver ends with its connection, as it does.

import { randomUUID } from "node:crypto";
import { run, type CanUseTool, type PermissionResult, type QueryFn, type QueryLike, type UserMessage } from "../src/connector.ts";

type Json = Record<string, unknown>;

// The suite has gone: so does Claude Code.
process.stdin.on("end", () => process.exit(0));
process.stdin.resume();

function out(message: Json): void {
  process.stdout.write(JSON.stringify(message) + "\n");
}

function heard(kind: string, fields: Json): void {
  out({ heard: { [kind]: fields } });
}

/** A call the driver made, after what Claude Code heard in it, counted as the suite counts what a harness reads. */
function frame(kind: string): void {
  out({ frame: kind });
}

const WORDS = ["#ask", "#quiz", "#sleep"] as const;

/** Claude Code as the SDK presents it, playing a script for each message. */
class Scripted implements QueryLike {
  private readonly out: unknown[] = [];
  private wake: (() => void) | undefined;
  private closed = false;
  private readonly session: string;
  private readonly canUseTool: CanUseTool;
  /** Ends the turn running until interrupted. */
  private interrupted: (() => void) | undefined;

  constructor(options: Json, prompt: AsyncIterable<UserMessage>) {
    this.canUseTool = options["canUseTool"] as CanUseTool;
    const resume = typeof options["resume"] === "string" ? options["resume"] : null;
    this.session = resume ?? `session-${randomUUID()}`;
    const session = this.session;
    heard("opened", { session, resumed: resume !== null, folder: options["cwd"] ?? null });
    void (async () => {
      for await (const m of prompt) {
        if (this.closed) return;
        const text = m.message.content;
        heard("prompt", { session, text, refused: false });
        frame("prompt");
        await this.turn(text);
      }
    })();
  }

  private say(m: unknown): void {
    this.out.push(m);
    this.wake?.();
  }

  private words(text: string): void {
    this.say({
      type: "assistant",
      message: { id: `msg-${randomUUID()}`, type: "message", role: "assistant", content: [{ type: "text", text: `Done: ${text}` }] },
      parent_tool_use_id: null,
      session_id: this.session,
      uuid: randomUUID(),
    });
  }

  private end(error = false): void {
    const result: Json = { type: "result", subtype: "success", is_error: false, session_id: this.session, duration_ms: 1, usage: {} };
    if (error) Object.assign(result, { subtype: "error_during_execution", is_error: true });
    this.say(result);
  }

  private async turn(text: string): Promise<void> {
    this.say({ type: "system", subtype: "init", session_id: this.session, uuid: randomUUID(), model: "scripted", permissionMode: "default", claude_code_version: "2.1.0", slash_commands: [] });
    const script = WORDS.find((w) => text.includes(w));
    if (script === "#sleep") {
      await new Promise<void>((r) => (this.interrupted = r));
      return this.end(true);
    }
    if (script === "#ask" || script === "#quiz") {
      const tool = randomUUID();
      const asking = script === "#ask";
      const name = asking ? "Bash" : "AskUserQuestion";
      const input: Json = asking
        ? { command: text, description: "a command" }
        : { questions: [{ question: `${text}?`, header: "Which", multiSelect: false, options: [{ label: "yes", description: "" }, { label: "no", description: "" }] }] };
      this.say({
        type: "assistant",
        message: { id: `msg-${randomUUID()}`, type: "message", role: "assistant", content: [{ type: "tool_use", id: tool, name, input }] },
        parent_tool_use_id: null,
        session_id: this.session,
        uuid: randomUUID(),
      });
      // As the SDK's: the request to use a tool comes after the messages before it have been read.
      await this.read();
      const result: PermissionResult = await this.canUseTool(name, input, { signal: new AbortController().signal, toolUseID: tool, requestId: randomUUID() });
      if (this.closed) return;
      if (asking) heard("answer", { session: this.session, allow: result.behavior === "allow" });
      else {
        const answers = result.behavior === "allow" ? (result.updatedInput?.["answers"] as Record<string, string> | undefined) : undefined;
        heard("answer", { session: this.session, text: Object.values(answers ?? {})[0] ?? null });
      }
      frame("permission_result");
      this.say({
        type: "user",
        message: { role: "user", content: [{ tool_use_id: tool, type: "tool_result", content: "ok", is_error: result.behavior !== "allow" }] },
        parent_tool_use_id: null,
        session_id: this.session,
        uuid: randomUUID(),
      });
    }
    this.words(text);
    this.end();
  }

  /** Waits until the driver has read every message said so far. */
  private async read(): Promise<void> {
    while (this.out.length > 0 && !this.closed) await new Promise((r) => setTimeout(r, 1));
    await new Promise((r) => setImmediate(r));
  }

  async *[Symbol.asyncIterator](): AsyncIterator<unknown> {
    for (;;) {
      while (this.out.length > 0) yield this.out.shift();
      if (this.closed) return;
      await new Promise<void>((r) => (this.wake = r));
      this.wake = undefined;
    }
  }

  async interrupt(): Promise<unknown> {
    heard("control", { session: this.session, what: "interrupt" });
    frame("interrupt");
    const ends = this.interrupted;
    this.interrupted = undefined;
    ends?.();
    return undefined;
  }

  async setPermissionMode(mode: string): Promise<void> {
    heard("control", { session: this.session, what: `set permission_mode=${mode}` });
    frame("setPermissionMode");
  }

  async setModel(model?: string): Promise<void> {
    heard("control", { session: this.session, what: `set model=${model}` });
    frame("setModel");
  }

  close(): void {
    this.closed = true;
    this.interrupted?.();
    this.wake?.();
  }
}

const query: QueryFn = ({ prompt, options }) => new Scripted(options, prompt);

const home = process.env["MARMOT_AGENT_HOME"];
if (home === undefined) {
  console.error("conformance-harness: set MARMOT_AGENT_HOME");
  process.exit(2);
}
try {
  await run({
    home,
    cwd: "/work/demo",
    query,
    version: "0.0.0",
    harnessVersion: "2.1.0",
    warn: (kind) => console.error(`marmot-agent-claude-code: ${kind}`),
    // A stand-in for the SDK's MCP server: the scripted Claude Code sends no file.
    fileServer: (send) => ({ send }),
  });
} catch (e) {
  console.error(`marmot-agent-claude-code: ${e instanceof Error ? e.message : "it stopped"}`);
  process.exit(1);
}
process.exit(0);
