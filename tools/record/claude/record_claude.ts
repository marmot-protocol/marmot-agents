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
import { createWriteStream, mkdtempSync, openSync, readFileSync, renameSync, rmSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

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

const userMessage = (content: string | Msg[], priority?: "now" | "next" | "later"): Msg => ({
  type: "user",
  message: { role: "user", content },
  parent_tool_use_id: null,
  ...(priority ? { priority } : {}),
});

type Ctx = {
  q: any;
  abort: AbortController;
  inbox: Inbox;
  record: (dir: "in" | "out", via: string, msg: Msg) => void;
  state: Record<string, any>;
};

type Prompt = string | ((project: string) => Msg[]);

type Scenario = {
  doc: string;
  prompts: Prompt[];
  options?: Msg;
  onPermission?: (tool: string, input: Msg) => Permission;
  onMessage?: (m: Msg, ctx: Ctx) => void | Promise<void>;
  /** A second query() that resumes the session of the first one. */
  resumeWith?: Prompt[];
  /** A second query() that does not resume: a new session for the same project. */
  thenNew?: Prompt[];
  /** Runs after query() is called and before the first prompt is sent. */
  beforePrompt?: (ctx: Ctx) => Promise<void>;
  /** Leave permission requests unanswered for this long, then interrupt the turn. */
  holdPermissionMs?: number;
  /** How long one query() may run before the recorder gives up. */
  timeoutMs?: number;
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const HERE = dirname(fileURLToPath(import.meta.url));
const DEMO_MCP_SERVER = join(HERE, "..", "demo_mcp_server.py");

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
  resume: {
    doc: "A session is resumed by a second query(). Leaves one session in the Claude Code history.",
    prompts: ["Remember the word 'marmot'. Reply with: Noted."],
    resumeWith: ["What word did I ask you to remember? Reply with just the word."],
    options: allowAll,
  },
  plan_mode: {
    doc: "Plan mode: the agent proposes a plan and asks to leave plan mode.",
    prompts: ["I want a farewell() function added to greet.py that returns 'Goodbye, ' plus the name. Give me your plan."],
    options: { permissionMode: "plan" },
    onPermission: (_tool, input) => ({ behavior: "allow", updatedInput: input }),
  },
  compact: {
    doc: "The user runs /compact between two turns.",
    prompts: ["Remember the word 'marmot'. Reply with: Noted.", "/compact", "What word did I ask you to remember? Reply with just the word."],
    options: allowAll,
  },
  slash_command: {
    doc: "The client lists the slash commands, then the user runs one.",
    prompts: ["/context"],
    options: allowAll,
    onMessage: async (m, ctx) => {
      if (m.type === "system" && m.subtype === "init" && !ctx.state.listed) {
        ctx.state.listed = true;
        ctx.record("out", "call", { method: "supportedCommands" });
        const commands = await ctx.q.supportedCommands();
        ctx.record("in", "call", { method: "supportedCommands", response: commands });
      }
    },
  },
  model_switch: {
    doc: "The client switches model between two turns.",
    prompts: ["Reply with exactly: first.", "Reply with exactly: second."],
    options: allowAll,
    onMessage: async (m, ctx) => {
      if (m.type === "result" && !ctx.state.switched) {
        ctx.state.switched = true;
        ctx.record("out", "call", { method: "setModel", model: "sonnet" });
        await ctx.q.setModel("sonnet");
        ctx.record("in", "call", { method: "setModel", response: null });
      }
    },
  },
  web_search: {
    doc: "The agent searches the web.",
    prompts: ["Search the web for the latest stable release of the Rust programming language and tell me its version number."],
    options: allowAll,
  },
  image: {
    doc: "The user attaches an image.",
    prompts: [
      (project) => [
        { type: "image", source: { type: "base64", media_type: "image/png", data: readFileSync(join(project, "red.png")).toString("base64") } },
        { type: "text", text: "What colour is the attached image? Answer with one word." },
      ],
    ],
    options: allowAll,
  },
  mcp_tool: {
    doc: "The agent calls a tool on an MCP server.",
    prompts: ["Use the define_word tool from the demo MCP server to define 'marmot', then tell me the definition."],
    options: { ...allowAll, mcpServers: { demo: { type: "stdio", command: "python3", args: [DEMO_MCP_SERVER] } } },
  },
  reasoning: {
    doc: "A turn with summarised thinking switched on. Without the option, thinking blocks arrive empty.",
    prompts: [
      "Without running any commands: how many times does the letter r appear in 'strawberry marmot burrow'? " +
        "Work it out carefully, then give the number.",
    ],
    options: { ...allowAll, thinking: { type: "adaptive", display: "summarized" } },
  },
  approval_unanswered: {
    doc: "A command needs approval and nobody answers. After six minutes the client interrupts.",
    prompts: ["Run `python3 greet.py` and tell me what it printed."],
    options: { permissionMode: "default" },
    holdPermissionMs: 360_000,
    timeoutMs: 480_000,
  },
  restart_mid_turn: {
    doc: "The harness is stopped while a command is running, then the session is resumed. Leaves one session in the Claude Code history.",
    prompts: ["Run `sleep 45 && echo finished` and then tell me what it printed."],
    resumeWith: ["You were stopped and restarted. What happened to the command you were running? Answer in one sentence."],
    options: allowAll,
    onMessage: async (m, ctx) => {
      if (usesTool(m, "Bash") && !ctx.state.stopped) {
        ctx.state.stopped = true;
        await sleep(3000);
        ctx.record("out", "call", { method: "abort", note: "the harness process is stopped in the middle of the turn" });
        ctx.abort.abort();
      }
    },
  },
  bad_model: {
    doc: "A session is started with a model that does not exist.",
    prompts: ["Reply with exactly: hello."],
    options: { ...allowAll, model: "no-such-model" },
  },
  early_info: {
    doc: "Before any prompt is sent, the client asks what the session offers and waits five seconds. Then one turn.",
    prompts: ["Reply with exactly: Hello from the recording."],
    options: allowAll,
    beforePrompt: async (ctx) => {
      for (const method of ["initializationResult", "supportedModels", "supportedCommands"]) {
        ctx.record("out", "call", { method });
        const r = await ctx.q[method]();
        // The initialization result names the signed-in account. It is never written down.
        const response = method === "initializationResult" ? { ...r, account: "(left out by the recorder)" } : r;
        ctx.record("in", "call", { method, response });
      }
      await sleep(5000);
      ctx.record("out", "note", { note: "five seconds have passed since query() and no prompt has been sent yet" });
    },
  },
  permission_mode_switch: {
    doc: "The client changes the permission mode between two turns. The first edit needs approval; after the switch to acceptEdits the second should not.",
    prompts: [
      "Add the comment line `# one` at the top of greet.py. Do nothing else.",
      "Add the comment line `# two` at the top of greet.py. Do nothing else.",
    ],
    options: { permissionMode: "default" },
    onPermission: (_tool, input) => ({ behavior: "allow", updatedInput: input }),
    onMessage: async (m, ctx) => {
      if (m.type === "result" && !ctx.state.switched) {
        ctx.state.switched = true;
        ctx.record("out", "call", { method: "setPermissionMode", mode: "acceptEdits" });
        await ctx.q.setPermissionMode("acceptEdits");
        ctx.record("in", "call", { method: "setPermissionMode", response: null });
      }
    },
  },
  new_session: {
    doc: "A second session is started for the same project without resuming the first.",
    prompts: ["Remember the word 'marmot'. Reply with: Noted."],
    thenNew: ["What word did I ask you to remember? If you do not know, say so in one sentence."],
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

  // Written owner-only in an owner-only folder beside the destination, and renamed over it only once the
  // scenario has finished, so a run that fails or is interrupted leaves the destination as it was.
  const work = mkdtempSync(join(dirname(out), `.${basename(out)}.`));
  const raw = join(work, basename(out));
  const discard = () => rmSync(work, { recursive: true, force: true });
  process.once("SIGINT", () => {
    discard();
    process.exit(130);
  });
  const file = createWriteStream(raw, { fd: openSync(raw, "wx", 0o600), encoding: "utf-8" });
  const t0 = performance.now();
  const record = (dir: "in" | "out", via: string, msg: Msg) =>
    file.write(JSON.stringify({ t: Math.round(performance.now() - t0), dir, via, msg }) + "\n");

  let sessionId: string | undefined;
  let status = "ok";
  let timedOut = false;

  const runQuery = async (promptList: Prompt[], extra: Msg) => {
    const inbox = new Inbox();
    const prompts = [...promptList];
    const pushNext = () => {
      const next = prompts.shift();
      if (next === undefined) return false;
      const msg = userMessage(typeof next === "string" ? next : next(resolve(project)));
      record("out", "prompt", msg);
      inbox.push(msg);
      return true;
    };

    const options: Msg = {
      cwd: resolve(project),
      model: "haiku",
      settingSources: [],
      ...scenario.options,
      ...extra,
      canUseTool: async (toolName: string, input: Msg, opts: Msg) => {
        const { signal, ...rest } = opts ?? {};
        record("in", "callback", { kind: "can_use_tool", toolName, input, options: rest });
        if (scenario.holdPermissionMs) {
          // Nobody answers. See whether the harness gives up by itself, then interrupt.
          const gaveUp = await Promise.race([
            sleep(scenario.holdPermissionMs).then(() => false),
            new Promise<boolean>((r) => signal?.addEventListener("abort", () => r(true))),
          ]);
          record("out", "note", gaveUp
            ? { note: "the harness withdrew the permission request before the hold ended" }
            : { note: `${scenario.holdPermissionMs / 1000} seconds passed with the request unanswered; the client interrupts` });
          if (!gaveUp) {
            record("out", "call", { method: "interrupt" });
            q.interrupt?.().then((r: any) => record("in", "call", { method: "interrupt", response: r ?? null })).catch(() => {});
          }
          return { behavior: "deny", message: "Nobody answered this request." };
        }
        const answer = scenario.onPermission?.(toolName, input) ?? { behavior: "allow", updatedInput: input };
        record("out", "callback", { kind: "permission_result", toolName, result: answer });
        return answer;
      },
    };
    const abort = new AbortController();
    const { canUseTool: _c, ...loggedOptions } = options;
    record("out", "call", { method: "query", options: loggedOptions });

    const q: any = query({ prompt: inbox as any, options: { ...options, abortController: abort } as any });
    const ctx: Ctx = { q, abort, inbox, record, state: {} };

    const timer = setTimeout(() => {
      timedOut = true;
      record("out", "call", { method: "timeout" });
      inbox.close();
      q.interrupt?.().catch(() => {});
    }, scenario.timeoutMs ?? 240_000);

    // Reading starts before the first prompt, so that anything the harness says unasked is recorded
    // at the moment it arrives.
    // An error of the scenario's own step, kept apart from one the harness ends the query with.
    let failed: { error: unknown } | undefined;
    const reading = (async () => {
      try {
        for await (const m of q) {
          record("in", "message", m as Msg);
          if ((m as Msg).session_id) sessionId = (m as Msg).session_id;
          try {
            if (scenario.onMessage) await scenario.onMessage(m as Msg, ctx);
          } catch (error) {
            failed = { error };
            break;
          }
          if ((m as Msg).type === "result" && !pushNext()) inbox.close();
        }
      } catch (e: any) {
        status = `ended with an error: ${e?.message}`;
        record("in", "call", { method: "query", error: String(e?.message ?? e) });
      }
    })();
    if (scenario.beforePrompt && !extra.resume) await scenario.beforePrompt(ctx);
    pushNext();
    await reading;
    clearTimeout(timer);
    if (failed) throw failed.error;
  };

  // An error the harness ends a query with is part of what some scenarios record; the recorder giving up
  // on one, or an error of the scenario's own step or of the recorder, is not.
  try {
    await runQuery(scenario.prompts, {});
    if (scenario.resumeWith && !timedOut) await runQuery(scenario.resumeWith, { resume: sessionId });
    if (scenario.thenNew && !timedOut) await runQuery(scenario.thenNew, {});
    await new Promise((r) => file.end(r));
    if (timedOut) {
      console.error(`${name}: FAILED: timed out; nothing was written`);
      process.exitCode = 1;
      return;
    }
    renameSync(raw, out);
  } finally {
    discard();
  }
  console.log(`${name}: ${status}`);
}

main();
