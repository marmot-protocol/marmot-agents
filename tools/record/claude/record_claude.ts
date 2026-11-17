/**
 * Record Claude Agent SDK sessions: every message the SDK yields, every permission
 * request it makes through `canUseTool`, and everything this client sends.
 *
 * Usage:
 *   bun record_claude.ts <scenario> <project-dir> <output.jsonl>
 *   bun record_claude.ts --list
 *
 * Each output line is {"t": <ms since start>, "dir": "out"|"in", "via": ..., "msg": ...}.
 *   dir "in",  via "message"   a message yielded by query()
 *   dir "in",  via "callback"  a permission request delivered to canUseTool
 *   dir "out", via "prompt"    a user message this client pushed into the session
 *   dir "out", via "callback"  this client's answer to a permission request
 *   dir "out", via "call"      a control method this client called, such as interrupt()
 *
 * Needs a signed-in Claude Code. Raw recordings contain local paths and account
 * details. Scrub them before sharing.
 */

import { query } from "@anthropic-ai/claude-agent-sdk";
import { createWriteStream } from "node:fs";
import { resolve } from "node:path";

type Msg = Record<string, any>;
type Permission = { behavior: "allow"; updatedInput?: Msg } | { behavior: "deny"; message: string };

class Inbox {
  private queue: Msg[] = [];
  private wake: (() => void) | undefined;
  private closed = false;
  push(m: Msg) {
    this.queue.push(m);
    this.wake?.();
  }
  close() {
    this.closed = true;
    this.wake?.();
  }
  async *[Symbol.asyncIterator]() {
    for (;;) {
      while (this.queue.length) yield this.queue.shift()!;
      if (this.closed) return;
      await new Promise<void>((r) => (this.wake = r));
      this.wake = undefined;
    }
  }
}

const userMessage = (text: string, priority?: "now" | "next" | "later"): Msg => ({
  type: "user",
  message: { role: "user", content: text },
  parent_tool_use_id: null,
  ...(priority ? { priority } : {}),
});

type Ctx = {
  q: any;
  inbox: Inbox;
  record: (dir: "in" | "out", via: string, msg: Msg) => void;
  state: Record<string, any>;
};

type Scenario = {
  doc: string;
  prompts: string[];
  options?: Msg;
  onPermission?: (tool: string, input: Msg) => Permission;
  onMessage?: (m: Msg, ctx: Ctx) => void | Promise<void>;
};

const allowAll = { permissionMode: "bypassPermissions", allowDangerouslySkipPermissions: true };

const usesTool = (m: Msg, name: string) =>
  m.type === "assistant" &&
  Array.isArray(m.message?.content) &&
  m.message.content.some((b: Msg) => b.type === "tool_use" && b.name === name);

const answerQuestion = (input: Msg): Permission => {
  const answers: Record<string, string> = {};
  for (const q of input.questions ?? []) answers[q.question] = q.options?.[0]?.label ?? "yes";
  return { behavior: "allow", updatedInput: { ...input, answers } };
};

const SCENARIOS: Record<string, Scenario> = {
  simple: {
    doc: "One turn with a short text answer and no tools.",
    prompts: ["Reply with exactly: Hello from the recording."],
    options: allowAll,
  },
  streaming: {
    doc: "The simple turn again, with partial messages switched on.",
    prompts: ["Reply with exactly: Hello from the recording."],
    options: { ...allowAll, includePartialMessages: true },
  },
  command: {
    doc: "One turn that runs a shell command and reports its output.",
    prompts: ["Run `python3 greet.py` and tell me what it printed."],
    options: allowAll,
  },
  file_edit: {
    doc: "One turn that edits a file, with no approval needed.",
    prompts: ["In greet.py, make greet() end its result with an exclamation mark. Then run `python3 greet.py`."],
    options: allowAll,
  },
  plan: {
    doc: "One turn where the agent keeps a to-do list while it works.",
    prompts: [
      "Use your to-do list tool to write a three-step list, then carry it out, updating the list as you go: " +
        "1) read greet.py, 2) read test_greet.py, 3) tell me in one sentence whether the test would pass.",
    ],
    options: allowAll,
  },
  approval_allowed: {
    doc: "A command needs approval and the user allows it.",
    prompts: ["Run `python3 greet.py` and tell me what it printed."],
    options: { permissionMode: "default" },
    onPermission: (_tool, input) => ({ behavior: "allow", updatedInput: input }),
  },
  approval_denied: {
    doc: "A command needs approval and the user declines it.",
    prompts: ["Run `python3 greet.py` and tell me what it printed. If you are not allowed, say so and stop."],
    options: { permissionMode: "default" },
    onPermission: () => ({ behavior: "deny", message: "The user declined this action." }),
  },
  file_approval: {
    doc: "A file change needs approval and the user allows it.",
    prompts: ["Add a one-line comment at the top of greet.py that says `# demo`. Do nothing else."],
    options: { permissionMode: "default" },
    onPermission: (_tool, input) => ({ behavior: "allow", updatedInput: input }),
  },
  question: {
    doc: "The agent asks the user a multiple-choice question.",
    prompts: [
      "Before doing anything, ask me one multiple-choice question with your AskUserQuestion tool: " +
        "which greeting should greet.py use, 'Hello' or 'Hi'? Then tell me which I chose. Do not edit files.",
    ],
    options: { permissionMode: "default" },
    onPermission: (tool, input) =>
      tool === "AskUserQuestion" ? answerQuestion(input) : { behavior: "allow", updatedInput: input },
  },
  interrupt: {
    doc: "The user interrupts a turn while a command is running.",
    prompts: ["Run `sleep 60` and then tell me it finished."],
    options: allowAll,
    onMessage: async (m, ctx) => {
      if (usesTool(m, "Bash") && !ctx.state.interrupted) {
        ctx.state.interrupted = true;
        await new Promise((r) => setTimeout(r, 2000));
        ctx.record("out", "call", { method: "interrupt" });
        const response = await ctx.q.interrupt();
        ctx.record("in", "call", { method: "interrupt", response: response ?? null });
      }
    },
  },
  steer: {
    doc: "The user sends a new message while a turn is running.",
    prompts: ["Run `sleep 8`, then tell me the name of the function defined in greet.py."],
    options: allowAll,
    onMessage: (m, ctx) => {
      if (usesTool(m, "Bash") && !ctx.state.steered) {
        ctx.state.steered = true;
        const msg = userMessage("Also tell me how many lines greet.py has.", "now");
        ctx.record("out", "prompt", msg);
        ctx.inbox.push(msg);
      }
    },
  },
  failing_command: {
    doc: "A command exits with an error.",
    prompts: ["Run `python3 -m pytest -q test_greet.py` once and report the result in one sentence. Do not fix anything."],
    options: allowAll,
  },
  two_turns: {
    doc: "Two turns in one session.",
    prompts: ["Remember the word 'marmot'. Reply with: Noted.", "What word did I ask you to remember? Reply with just the word."],
    options: allowAll,
  },
  subagent: {
    doc: "The agent hands a task to a subagent.",
    prompts: ["Use your Agent tool to have a subagent read greet.py and report the name of the function in it. Then tell me the name."],
    options: allowAll,
  },
};

async function main() {
  const [name, project, out] = process.argv.slice(2);
  if (name === "--list") {
    for (const [k, s] of Object.entries(SCENARIOS)) console.log(`${k}: ${s.doc}`);
    return;
  }
  const scenario = SCENARIOS[name ?? ""];
  if (!scenario || !project || !out) {
    console.error("usage: bun record_claude.ts <scenario> <project-dir> <output.jsonl> | --list");
    process.exit(2);
  }

  const file = createWriteStream(out, { encoding: "utf-8" });
  const t0 = performance.now();
  const record = (dir: "in" | "out", via: string, msg: Msg) =>
    file.write(JSON.stringify({ t: Math.round(performance.now() - t0), dir, via, msg }) + "\n");

  const inbox = new Inbox();
  const prompts = [...scenario.prompts];
  const pushNext = () => {
    const text = prompts.shift();
    if (text === undefined) return false;
    const msg = userMessage(text);
    record("out", "prompt", msg);
    inbox.push(msg);
    return true;
  };

  const options: Msg = {
    cwd: resolve(project),
    model: "haiku",
    settingSources: [],
    ...scenario.options,
    canUseTool: async (toolName: string, input: Msg, opts: Msg) => {
      const { signal: _signal, ...rest } = opts ?? {};
      record("in", "callback", { kind: "can_use_tool", toolName, input, options: rest });
      const answer = scenario.onPermission?.(toolName, input) ?? { behavior: "allow", updatedInput: input };
      record("out", "callback", { kind: "permission_result", toolName, result: answer });
      return answer;
    },
  };
  const { canUseTool: _c, ...loggedOptions } = options;
  record("out", "call", { method: "query", options: loggedOptions });

  const q: any = query({ prompt: inbox as any, options: options as any });
  const ctx: Ctx = { q, inbox, record, state: {} };
  pushNext();

  const timer = setTimeout(() => {
    record("out", "call", { method: "timeout" });
    inbox.close();
    q.interrupt?.().catch(() => {});
  }, 240_000);

  let status = "ok";
  try {
    for await (const m of q) {
      record("in", "message", m as Msg);
      if (scenario.onMessage) await scenario.onMessage(m as Msg, ctx);
      if ((m as Msg).type === "result" && !pushNext()) inbox.close();
    }
  } catch (e: any) {
    status = `FAILED: ${e?.name}: ${e?.message}`;
    record("in", "call", { method: "query", error: String(e?.message ?? e) });
  }
  clearTimeout(timer);
  await new Promise((r) => file.end(r));
  console.log(`${name}: ${status}`);
}

main();
