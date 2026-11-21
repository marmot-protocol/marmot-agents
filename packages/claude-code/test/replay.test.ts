// Every Claude Code recording, replayed through the translator, must give the events the prototype
// gives for it (`fixtures/recordings/claude/expected`, written by
// `tools/prototype/translate.py --expected`). The two were written apart, from the same design;
// where they disagree, the recording and the event model decide which is wrong (D-074).

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { Translator, type AgentEvent, type Line } from "../src/translate.ts";

const folder = new URL("../../../fixtures/recordings/claude/", import.meta.url);

function lines(url: URL): unknown[] {
  return readFileSync(url, "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line) as unknown);
}

function replay(name: string): { events: AgentEvent[]; translator: Translator } {
  const translator = new Translator();
  const events: AgentEvent[] = [];
  for (const line of lines(new URL(name, folder)) as Line[]) {
    const made = translator.feed(line);
    for (const event of made) {
      if (event.native !== undefined) {
        // The native message is the harness's whole message, as it was handed over (D-059).
        assert.equal(event.native.payload, line.msg, `${name}: the native message of ${event.item}`);
      }
    }
    events.push(...made);
  }
  return { events, translator };
}

const recordings = readdirSync(folder)
  .filter((name) => name.endsWith(".jsonl"))
  .sort();

test("every Claude Code recording is here", () => {
  assert.equal(recordings.length, 29);
});

for (const name of recordings) {
  test(`${name} gives the prototype's events`, () => {
    const { events, translator } = replay(name);
    assert.deepEqual([...translator.unknown], [], "every message has a rule");
    // An event goes to the core as JSON, so it is compared as JSON.
    const made = JSON.parse(JSON.stringify(events)) as unknown[];
    const want = lines(new URL(`expected/${name}`, folder));
    assert.deepEqual(made, want);
  });
}
