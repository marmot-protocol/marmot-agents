// The connector against the core's own description of the socket (`fixtures/socket`, from
// `crates/marmot-agent`): every example is what the schema says, every field the connector reads from a
// request of the core's is one the core sends, and every event the translator makes for any recording is
// one the core reads. The transport's tests check every line both ways too (`transport.test.ts`).

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { Connector, type CanUseTool, type QueryLike, type UserMessage } from "../src/connector.ts";
import { Refusal } from "../src/socket.ts";
import { Translator, type Line } from "../src/translate.ts";
import { assertValid, coreRequests, examples, tracked, unknownReads } from "./protocol.ts";

type Json = Record<string, unknown>;

test("every example of the socket's messages is what the schema says", () => {
  assert.ok(examples.length > 0);
  for (const e of examples) assertValid(e.from === "core" ? "FromCore" : "FromConnector", e.message, e.name);
});

/** Claude Code, as far as serving the core's requests needs: it takes messages and asks permission. */
class Quiet implements QueryLike {
  readonly canUseTool: CanUseTool;
  constructor(options: Json, prompt: AsyncIterable<UserMessage>) {
    this.canUseTool = options["canUseTool"] as CanUseTool;
    void (async () => {
      for await (const _ of prompt);
    })();
  }
  async *[Symbol.asyncIterator](): AsyncIterator<unknown> {
    yield { type: "system", subtype: "init", session_id: "s-1", uuid: "u1", model: "model-a", permissionMode: "default", claude_code_version: "2.1.0", slash_commands: [] };
    await new Promise(() => {});
  }
  interrupt(): Promise<unknown> {
    return Promise.resolve(undefined);
  }
  setPermissionMode(): Promise<void> {
    return Promise.resolve();
  }
  setModel(): Promise<void> {
    return Promise.resolve();
  }
  close(): void {}
}

test("the connector reads nothing from the core's requests that the core does not send", async () => {
  const queries: Quiet[] = [];
  const connector = new Connector({
    query: ({ prompt, options }) => {
      const q = new Quiet(options, prompt);
      queries.push(q);
      return q;
    },
    cwd: "/work/demo",
    send: () => {},
  });
  const requests = coreRequests();
  assert.ok(requests.some((r) => r.method === "prompt") && requests.some((r) => r.method === "answer") && requests.some((r) => r.method === "control"));
  // Prompts first, so a session runs, and the answers find what they answer waiting.
  const order = ["prompt", "answer", "control"];
  requests.sort((a, b) => order.indexOf(a.method) - order.indexOf(b.method));
  let asked = 0;
  for (const { name, method, params } of requests) {
    const given = method === "answer" ? (params["given"] as Json) : undefined;
    if (given !== undefined) {
      // Claude Code asks, and the example answers what it asked: the item is the connector's own id.
      const q = queries[0]!;
      const question = "questions" in given;
      const tool = question ? "AskUserQuestion" : "Bash";
      const input = question ? { questions: [{ question: "Which?", options: [{ label: "Red" }, { label: "Blue" }] }, { question: "Why?", options: [] }] } : { command: "ls" };
      asked += 1;
      void q.canUseTool(tool, input, { signal: new AbortController().signal, requestId: `x${asked}`, suggestions: [] });
      params["item"] = `${question ? "question" : "approval"}:x${asked}`;
    }
    const reads = new Set<string>();
    try {
      await connector.serve(method, tracked(params, reads));
    } catch (e) {
      if (!(e instanceof Refusal)) throw e;
    }
    const def = `${method[0]!.toUpperCase()}${method.slice(1)}Params`;
    assert.ok(reads.size > 0, `${name}: nothing was read`);
    assert.deepEqual(unknownReads(def, reads), [], `${name}: read, and never sent by the core`);
  }
  connector.close();
});

const recordings = new URL("../../../fixtures/recordings/claude/", import.meta.url);

test("every event made from a recording is one the core reads", () => {
  const names = readdirSync(recordings).filter((n) => n.endsWith(".jsonl"));
  assert.ok(names.length > 0);
  for (const name of names) {
    const translator = new Translator();
    for (const text of readFileSync(new URL(name, recordings), "utf8").split("\n")) {
      if (text.trim() === "") continue;
      for (const event of translator.feed(JSON.parse(text) as Line)) {
        // As the transport sends it: the core fills in `harness` and `seq`, and the chat is named.
        const { harness: _h, seq: _s, ...rest } = event;
        assertValid("EventParams", JSON.parse(JSON.stringify({ ...rest, chat: "c-1" })), `${name}: ${event.type} ${event.item}`);
      }
    }
  }
});
