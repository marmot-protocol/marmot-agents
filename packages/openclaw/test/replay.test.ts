// Every OpenClaw recording, replayed through the translator, must give the events the prototype gives
// for it (`fixtures/recordings/openclaw/expected`, written by `tools/prototype/translate.py
// --expected`). The two were written apart, from the same design; where they disagree, the recording
// and the event model decide which is wrong (D-074).

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { Translator, type AgentEvent, type Line } from "../src/translate.ts";

const folder = new URL("../../../fixtures/recordings/openclaw/", import.meta.url);

function lines(url: URL): unknown[] {
  return readFileSync(url, "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line) as unknown);
}

const recordings = readdirSync(folder)
  .filter((name) => name.endsWith(".jsonl"))
  .sort();

test("the OpenClaw recordings are here", () => {
  assert.ok(recordings.length > 0, "no recordings found");
});

for (const name of recordings) {
  test(`${name} gives the prototype's events`, () => {
    const translator = new Translator();
    const events: AgentEvent[] = [];
    for (const line of lines(new URL(name, folder)) as Line[]) events.push(...translator.feed(line));
    assert.deepEqual([...translator.unknown], [], "every call has a rule");
    // An event goes to the core as JSON, so it is compared as JSON.
    const made = JSON.parse(JSON.stringify(events)) as unknown[];
    const want = lines(new URL(`expected/${name}`, folder));
    assert.deepEqual(made, want);
  });
}
