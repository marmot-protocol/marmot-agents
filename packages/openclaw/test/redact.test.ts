// What the plugin replaces before an event leaves it. Every value here is made up.

import assert from "node:assert/strict";
import { test } from "node:test";
import { redactor, SECRET, secretsFrom } from "../src/redact.ts";

test("a key that serialization escapes, or a short one, never leaves; its masked form does", () => {
  // A quote and a backslash, which JSON escapes, so a search of the serialized event would miss it.
  const key = 'sk-ab"c\\d-0123456789';
  const masked = "sk-a…6789";
  const env = { MARMOT_SECRET_ENV: "MODEL_API_KEY, SHORT, UNSET", MODEL_API_KEY: key, SHORT: "q7" };
  assert.deepEqual(secretsFrom(env), [key, "q7"]);
  const scrub = redactor(secretsFrom(env))!;
  const event = {
    chat: "c-1",
    type: "notice",
    text: `Model p/m, key ${masked}. Raw: ${key}.`,
    body: { kind: "command_output", command: "/model" },
    native: { kind: "deliver", payload: [{ text: `key=${key}`, nested: { [key]: ["q7 and q7", 3, null, true] } }] },
  };
  const out = scrub(event);
  const sent = JSON.stringify(out);
  assert.ok(!sent.includes(key) && !sent.includes(JSON.stringify(key).slice(1, -1)), "the whole key is gone");
  assert.ok(!sent.includes("q7"), "a short secret is gone too");
  assert.equal(out.text, `Model p/m, key ${masked}. Raw: ${SECRET}.`);
  assert.deepEqual(out.native.payload, [{ text: `key=${SECRET}`, nested: { [SECRET]: [`${SECRET} and ${SECRET}`, 3, null, true] } }]);
  assert.deepEqual(out.body, event.body);
  // The event given is left as it was.
  assert.ok(event.text.includes(key));
  assert.equal(redactor(secretsFrom({ MARMOT_SECRET_ENV: "UNSET" })), null);
});
