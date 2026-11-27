// Cases no recording holds, the same as the prototype's in `tools/prototype/test_translate.py`, so
// the driver and the prototype stay one translation where the recordings do not reach (D-074).

import assert from "node:assert/strict";
import { test } from "node:test";
import { Translator, type AgentEvent, type Line } from "../src/translate.ts";

const QUESTIONS = [
  {
    question: "Which files?",
    header: "Files",
    multiSelect: true,
    options: [
      { label: "a.py", description: "" },
      { label: "b.py", description: "" },
      { label: "c.py", description: "" },
    ],
  },
  {
    question: "Which greeting?",
    header: "Greeting",
    multiSelect: false,
    options: [
      { label: "Hello", description: "" },
      { label: "Hi", description: "" },
    ],
  },
  { question: "Anything else?", header: "More", multiSelect: false, options: [{ label: "No", description: "" }] },
];

function feed(translator: Translator, ...lines: Line[]): AgentEvent[] {
  return lines.flatMap((line) => translator.feed(line));
}

function answer(answers: Record<string, string>): unknown {
  const translator = new Translator();
  const events = feed(
    translator,
    {
      dir: "in",
      via: "callback",
      msg: {
        kind: "can_use_tool",
        toolName: "AskUserQuestion",
        input: { questions: QUESTIONS },
        options: { toolUseID: "toolu_1", requestId: "r1" },
      },
    },
    {
      dir: "out",
      via: "callback",
      msg: {
        kind: "permission_result",
        toolName: "AskUserQuestion",
        result: { behavior: "allow", updatedInput: { questions: QUESTIONS, answers } },
      },
    },
  );
  return (events.at(-1)?.body as Record<string, unknown>)["answer"];
}

test("labels become the ids of the options offered", () => {
  assert.deepEqual(answer({ "Which files?": "a.py, c.py", "Which greeting?": "Hi" }), [
    { question: "q0", options: ["o0", "o2"] },
    { question: "q1", options: ["o1"] },
  ]);
});

test("what matches no label is typed text", () => {
  assert.deepEqual(answer({ "Which files?": "b.py, d.py", "Which greeting?": "Howdy" }), [
    { question: "q0", options: ["o1"], text: "d.py" },
    { question: "q1", text: "Howdy" },
  ]);
});

test("nothing is sent before the session is named, and then commands keep their hints", () => {
  const translator = new Translator();
  const before = feed(
    translator,
    { dir: "out", via: "call", msg: { method: "query", options: {} } },
    {
      dir: "in",
      via: "call",
      msg: {
        method: "supportedCommands",
        response: [
          { name: "compact", description: "Compact", argumentHint: "<instructions>" },
          { name: "cost", description: "Cost" },
        ],
      },
    },
  );
  assert.deepEqual(before, []);
  const [session] = feed(translator, {
    dir: "in",
    via: "message",
    msg: {
      type: "system",
      subtype: "init",
      session_id: "s1",
      claude_code_version: "9.9.9",
      model: "m",
      permissionMode: "default",
      slash_commands: ["compact", "cost"],
    },
  });
  assert.equal(session?.type, "session");
  assert.equal(session?.session, "s1");
  const body = session?.body as Record<string, unknown>;
  assert.equal(body["harness_version"], "9.9.9");
  assert.deepEqual(body["commands"], [
    { name: "compact", description: "Compact", hint: "<instructions>" },
    { name: "cost", description: "Cost", hint: "" },
  ]);
});

// D-177. No recording holds `session_title_changed`; its shape is the one in
// `docs/reference/data/claude-sdk-message-structures.txt`. A change is an edit, a repeat none.
test("the session's title is Claude Code's, and one before the init waits for it", () => {
  const translator = new Translator();
  const title = (t: string): Line => ({
    dir: "in",
    via: "message",
    msg: { type: "system", subtype: "session_title_changed", session_id: "s1", title: t, uuid: "u" },
  });
  assert.deepEqual(feed(translator, title("Fix the build")), []);
  const events = feed(
    translator,
    {
      dir: "in",
      via: "message",
      msg: { type: "system", subtype: "init", session_id: "s1", claude_code_version: "9.9.9", slash_commands: [] },
    },
    title("Fix the build"),
    title("Fix the tests"),
  );
  const titles = events.filter((e) => e.type === "session").map((e) => (e.body as Record<string, unknown>)["title"]);
  assert.deepEqual(titles, ["Fix the build", "Fix the tests"]);
});

function ask(translator: Translator, request: string, command: string, suggestions: unknown[] = []): void {
  feed(translator, {
    dir: "in",
    via: "callback",
    msg: {
      kind: "can_use_tool",
      toolName: "Bash",
      input: { command },
      options: { toolUseID: `tool-${request}`, requestId: request, suggestions },
    },
  });
}

function reply(translator: Translator, request: string, choice: string): AgentEvent | undefined {
  return feed(translator, {
    dir: "out",
    via: "callback",
    msg: { kind: "permission_result", toolName: "Bash", requestId: request, choice, result: { behavior: "allow" } },
  }).at(-1);
}

test("two approvals waiting for one tool are answered each by its own reply", () => {
  const translator = new Translator();
  ask(translator, "r1", "ls");
  ask(translator, "r2", "pwd");
  assert.equal(reply(translator, "r2", "c0")?.item, "approval:r2");
  assert.equal(reply(translator, "r1", "c0")?.item, "approval:r1");
});

test("a standing choice is recorded as that choice", () => {
  const translator = new Translator();
  ask(translator, "r1", "ls", [{ type: "addRules" }]);
  const answered = reply(translator, "r1", "c1");
  assert.deepEqual((answered?.body as Record<string, unknown>)["answer"], { choice: "c1" });
  assert.equal(answered?.text, "Answered: Allow, and don't ask again");
});

test("a resumed session's first turn has its own id", () => {
  const first = (uuid: string) =>
    feed(new Translator(), {
      dir: "in",
      via: "message",
      msg: { type: "system", subtype: "init", session_id: "s1", uuid, claude_code_version: "9.9.9" },
    }).find((event) => event.type === "turn")?.item;
  assert.notEqual(first("u-before"), first("u-after"));
});
