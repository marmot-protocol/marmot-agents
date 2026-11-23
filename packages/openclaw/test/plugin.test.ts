// The plugin as OpenClaw loads it (src/index.ts), with the plugin SDK's modules replaced by
// test/sdk-stub.ts and a fake core on a real Unix socket: OpenClaw's calls on the channel's own
// adapters, end to end to the core. Every value here is made up.

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { createServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { eventMessageProblems, problems } from "./protocol.ts";

registerHooks({
  resolve(specifier, context, next) {
    if (specifier.startsWith("openclaw/plugin-sdk/")) return { url: new URL("./sdk-stub.ts", import.meta.url).href, shortCircuit: true };
    return next(specifier, context);
  },
});

type Json = Record<string, unknown>;
const entry = (await import("../src/index.ts")).default as unknown as Json;
const base = object(object(entry["plugin"])["base"]);
const send = object(object(base["message"])["send"]);
const media = send["media"] as (params: Json) => Promise<Json>;
const startAccount = object(base["gateway"])["startAccount"] as (ctx: Json) => Promise<void>;

function object(value: unknown): Json {
  return typeof value === "object" && value !== null ? (value as Json) : {};
}

/** How the fake core answers a `send_file`: done, refused, or by closing the socket. */
type Answer = "sent" | "refused" | "close";

/**
 * A fake core: it welcomes the plugin, keeps every line it is sent, and answers each `send_file`. Every
 * line either way is checked against the core's own schema (`fixtures/socket`); what is not as the core's
 * types say is kept in `invalid`.
 */
async function fakeCore(dir: string) {
  const lines: Json[] = [];
  const invalid: string[] = [];
  const state = { answer: "sent" as Answer, welcomed: false };
  const sockets: Socket[] = [];
  const server = createServer((socket) => {
    sockets.push(socket);
    let buffered = "";
    socket.on("data", (data) => {
      buffered += data.toString("utf8");
      let at;
      while ((at = buffered.indexOf("\n")) !== -1) {
        const m = JSON.parse(buffered.slice(0, at)) as Json;
        buffered = buffered.slice(at + 1);
        lines.push(m);
        const found = m["method"] === "event" ? eventMessageProblems(m) : problems("FromConnector", m);
        for (const p of found) invalid.push(`from the plugin: ${p}`);
        const reply = (body: Json) => {
          const answer = { jsonrpc: "2.0", id: m["id"], ...body };
          for (const p of problems("FromCore", answer)) invalid.push(`from the core: ${p}`);
          socket.write(JSON.stringify(answer) + "\n");
        };
        if (m["method"] === "hello") {
          reply({ result: { core: { version: "0" }, agent: { name: "a" } } });
          state.welcomed = true;
        } else if (m["method"] === "send_file") {
          if (state.answer === "sent") reply({ result: {} });
          else if (state.answer === "refused") {
            reply({ error: { code: -32000, message: "not_allowed", data: { reason: "not_allowed", detail: "The file is outside the folders the agent may send files from." } } });
          } else socket.destroy();
        }
      }
    });
  });
  mkdirSync(join(dir, "socket"));
  writeFileSync(join(dir, "connector.token"), "a-token\n");
  await new Promise<void>((r) => server.listen(join(dir, "socket", "marmot-agent.sock"), r));
  return { lines, invalid, state, close: () => (sockets.forEach((s) => s.destroy()), server.close()) };
}

async function until(done: () => boolean): Promise<void> {
  const deadline = Date.now() + 5000;
  while (!done()) {
    if (Date.now() > deadline) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 5));
  }
}

test("media OpenClaw sends through the channel goes as its caption, then its file, and fails when the core cannot send it", async () => {
  delete process.env["MARMOT_AGENT_HOME"];
  const dir = mkdtempSync(join(tmpdir(), "marmot-openclaw-"));
  const core = await fakeCore(dir);
  const abort = new AbortController();
  try {
    // Not connected yet: the send fails, and OpenClaw says so.
    await assert.rejects(media({ to: "marmot:c-1", text: "A chart", mediaUrl: "/work/chart.png" }));

    const running = startAccount({ cfg: { channels: { marmot: { home: dir } } }, accountId: "default", channelRuntime: {}, abortSignal: abort.signal });
    await until(() => core.state.welcomed);
    await until(() => core.lines.length > 0 && core.lines.at(-1)!["method"] === "hello");
    await new Promise((r) => setTimeout(r, 20));

    // The core refuses the file: the caption went first, as the agent's words, and the send fails.
    core.state.answer = "refused";
    await assert.rejects(media({ to: "marmot:c-1", text: "The chart you asked for", mediaUrl: "/work/chart.png" }));
    const asked = core.lines.findIndex((m) => m["method"] === "send_file");
    const said = core.lines.findIndex((m) => m["method"] === "event" && object(m["params"])["type"] === "message");
    assert.ok(said !== -1 && asked !== -1 && said < asked, "the caption goes before the file");
    assert.equal(object(core.lines[said]!["params"])["text"], "The chart you asked for");
    assert.deepEqual(core.lines[asked]!["params"], { chat: "c-1", path: "/work/chart.png" });

    // The core sends it: only then is there a receipt.
    core.state.answer = "sent";
    const receipt = await media({ to: "marmot:c-1", text: "", mediaUrl: "report.pdf" });
    assert.ok(typeof object(receipt["receipt"])["primaryPlatformMessageId"] === "string");
    assert.deepEqual(core.lines.filter((m) => m["method"] === "send_file").at(-1)!["params"], { chat: "c-1", path: "/workspaces/main/report.pdf" });
    assert.equal(core.lines.filter((m) => m["method"] === "event").length, 1, "no caption, no words");

    // The core goes while the file is asked for: the send fails.
    core.state.answer = "close";
    await assert.rejects(media({ to: "marmot:c-1", mediaUrl: "/work/late.png" }));
    abort.abort();
    await running;
    assert.deepEqual(core.invalid, [], "every line either way is as the core's types say");
  } finally {
    abort.abort();
    core.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
