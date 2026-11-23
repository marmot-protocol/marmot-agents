// The plugin against the core's own description of the socket (`fixtures/socket`, from
// `crates/marmot-agent`): every example is what the schema says, and every event the translator makes for
// any recording is one the core reads. The transport's tests check every message both ways too, and that
// the plugin reads only what the core sends (`transport.test.ts`, `plugin.test.ts`).

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { Translator, type Line } from "../src/translate.ts";
import { assertValid, eventProblems, examples } from "./protocol.ts";

test("this package's checker is the Claude Code package's, past its opening comment", () => {
  const body = (file: URL) => {
    const text = readFileSync(file, "utf8");
    return text.slice(text.indexOf("\nimport "));
  };
  assert.equal(
    body(new URL("./protocol.ts", import.meta.url)),
    body(new URL("../../claude-code/test/protocol.ts", import.meta.url)),
    "packages/openclaw/test/protocol.ts and packages/claude-code/test/protocol.ts differ: change both",
  );
});

test("every example of the socket's messages is what the schema says", () => {
  assert.ok(examples.length > 0);
  for (const e of examples) assertValid(e.from === "core" ? "FromCore" : "FromConnector", e.message, e.name);
});

const recordings = new URL("../../../fixtures/recordings/openclaw/", import.meta.url);

test("every event made from a recording is one the core reads", () => {
  const names = readdirSync(recordings).filter((n) => n.endsWith(".jsonl"));
  assert.ok(names.length > 0);
  for (const name of names) {
    const translator = new Translator();
    for (const text of readFileSync(new URL(name, recordings), "utf8").split("\n")) {
      if (text.trim() === "") continue;
      for (const event of translator.feed(JSON.parse(text) as Line)) {
        // As the plugin sends it: the core fills in `harness` and `seq`, and the chat is named.
        const { harness: _h, seq: _s, ...rest } = event;
        assert.deepEqual(eventProblems({ ...rest, chat: "c-1" }), [], `${name}: ${event.type} ${event.item} is not what the core reads`);
      }
    }
  }
});
