/**
 * The Claude Code connector: drives Claude Code for one agent through the core's local socket.
 *
 *   node src/main.ts --home <agent home> --cwd <project folder> [--model <model>] [--permission-mode <mode>]
 *
 * The socket is `socket/marmot-agent.sock` in the agent's home and the token is `connector.token` there
 * (`Endpoint::in_home` in `crates/marmot-agent/src/socket.rs`). It exits when the socket closes.
 * It prints errors by kind only: never a message, a prompt, a key or a path (D-071).
 *
 * Once the core has taken its hello, it makes the file `MARMOT_AGENT_READY_FILE` names, if set: the
 * installer waits for it before it counts the agent as started (D-178).
 */

import { query } from "@anthropic-ai/claude-agent-sdk";
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { run, type QueryFn } from "./connector.ts";

const USAGE = "usage: node src/main.ts --home <agent home> --cwd <project folder> [--model <m>] [--permission-mode <mode>]";

function say(kind: string): void {
  console.error(`marmot-agent-claude-code: ${kind}`);
}

function field(url: URL, name: string): string | undefined {
  try {
    const value = (JSON.parse(readFileSync(url, "utf8")) as Record<string, unknown>)[name];
    return typeof value === "string" ? value : undefined;
  } catch {
    return undefined;
  }
}

let args;
try {
  args = parseArgs({
    options: {
      home: { type: "string" },
      cwd: { type: "string" },
      model: { type: "string" },
      "permission-mode": { type: "string" },
    },
  }).values;
} catch {
  say(USAGE);
  process.exit(2);
}
if (args.home === undefined || args.cwd === undefined) {
  say(USAGE);
  process.exit(2);
}

// The SDK runs the Claude Code it ships with, and names its version.
const sdk = new URL("package.json", import.meta.resolve("@anthropic-ai/claude-agent-sdk"));
try {
  await run({
    home: args.home,
    cwd: args.cwd,
    model: args.model,
    permissionMode: args["permission-mode"],
    query: query as unknown as QueryFn,
    version: field(new URL("../package.json", import.meta.url), "version") ?? "0.0.0",
    harnessVersion: field(sdk, "claudeCodeVersion"),
    warn: say,
    ready: () => {
      const file = process.env["MARMOT_AGENT_READY_FILE"];
      if (!file) return;
      try {
        writeFileSync(file, "", { mode: 0o600 });
      } catch {
        say("the file that says the connector is ready could not be made");
      }
    },
  });
} catch (e) {
  // What `run` throws names a kind and nothing else.
  say(e instanceof Error ? e.message : "it stopped");
  process.exit(1);
}
say("the core closed the socket");
process.exit(0);
