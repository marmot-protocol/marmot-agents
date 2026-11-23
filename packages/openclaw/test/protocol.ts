// The core's socket messages, as the core's own types describe them: the JSON Schema and the examples in
// `fixtures/socket`, which a test in `crates/marmot-agent` writes and keeps true. The tests here check
// against them that what this connector sends is what the core reads, and that what it reads from the
// core is what the core sends. Copied from `packages/claude-code/test/protocol.ts`; keep the two the
// same.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Ajv2020, type ValidateFunction } from "ajv/dist/2020.js";

type Json = Record<string, unknown>;
type Schema = Json | boolean;

const folder = new URL("../../../fixtures/socket/", import.meta.url);

/** The schema, which admits no field the core's types do not name. */
export const schema = JSON.parse(readFileSync(new URL("schema.json", folder), "utf8")) as Json;

/** One example: a name, which side sends it, and the message. */
export interface Example {
  name: string;
  from: "core" | "connector";
  message: Json;
}

export const examples = (JSON.parse(readFileSync(new URL("examples.json", folder), "utf8")) as { examples: Example[] }).examples;

const ajv = new Ajv2020({ allErrors: true, strict: false, validateFormats: false });
ajv.addSchema(schema, "socket");

function validator(def: string): ValidateFunction {
  const validate = ajv.getSchema(`socket#/$defs/${def}`);
  if (validate === undefined) throw new Error(`no ${def} in the schema`);
  return validate;
}

/** What is wrong with `value` as a `def` of the schema, or nothing. */
export function problems(def: "FromCore" | "FromConnector" | "EventParams", value: unknown): string[] {
  const validate = validator(def);
  if (validate(value)) return [];
  return focused(def, value) ?? said(validate);
}

function said(validate: ValidateFunction, at = ""): string[] {
  return (validate.errors ?? []).map((e) => `${at}${e.instancePath || "/"} ${e.message ?? ""} ${JSON.stringify(e.params)}`);
}

/**
 * A message or an item is one branch of a `oneOf`, picked by its `method` or its `type`, so ajv's errors
 * for it are those of every branch. These are the errors of the branch it picked: its params' or its
 * body's, when those are what is wrong.
 */
function focused(def: string, value: unknown): string[] | undefined {
  const m = value !== null && typeof value === "object" ? (value as Json) : {};
  const camel = (word: string) => word.split("_").map((w) => `${w[0]!.toUpperCase()}${w.slice(1)}`).join("");
  const event = def === "EventParams" ? m : m["method"] === "event" ? (m["params"] as Json) : undefined;
  const defs = schema["$defs"] as Json;
  if (event !== undefined && typeof event["type"] === "string" && camel(event["type"]) in defs) {
    const body = validator(camel(event["type"]));
    if (!body(event["body"])) return said(body, def === "EventParams" ? "/body" : "/params/body");
  }
  if (typeof m["method"] === "string" && `${camel(m["method"])}Params` in defs) {
    const params = validator(`${camel(m["method"])}Params`);
    if (!params(m["params"])) return said(params, "/params");
  }
  return undefined;
}

/**
 * Fields a connector puts in an item's body that the core's types do not hold yet, each an open question
 * (Q-01): `fixtures/socket/passed-over.json`. The core passes over them; the checks allow them and no
 * others.
 */
const passedOver = (JSON.parse(readFileSync(new URL("passed-over.json", folder), "utf8")) as { fields: { type: string; field: string }[] }).fields;

/** The event without the fields passed over for its type. */
export function withoutPassedOver(event: Json): Json {
  const copy = structuredClone(event);
  const drop = (value: unknown, path: string[]): void => {
    const [first, ...rest] = path;
    if (first === undefined) return;
    if (first === "[]") {
      if (Array.isArray(value)) for (const v of value) drop(v, rest);
    } else if (typeof value === "object" && value !== null && first in value) {
      if (rest.length > 0) drop((value as Json)[first], rest);
      else delete (value as Json)[first];
    }
  };
  for (const { type, field } of passedOver) if (copy["type"] === type) drop(copy["body"], field.replace(/\[\]/g, ".[]").split("."));
  return copy;
}

/** What is wrong with an event the connector sends, as the core reads it, beyond what is passed over. */
export function eventProblems(event: unknown): string[] {
  return problems("EventParams", withoutPassedOver(JSON.parse(JSON.stringify(event)) as Json));
}

/** Fails unless `value` is a `def` of the schema. */
export function assertValid(def: "FromCore" | "FromConnector" | "EventParams", value: unknown, what: string): void {
  assert.deepEqual(problems(def, value), [], `${what} is not what the core's types say: ${JSON.stringify(value)}`);
}

// -- what a connector reads

const IGNORED = new Set(["toJSON", "constructor", "then"]);

/**
 * `value`, with every field read from it, at any depth, recorded in `reads` as a path such as
 * `params.chat.id` or `params.attachments[].path`.
 */
export function tracked<T>(value: T, reads: Set<string>, path = ""): T {
  if (typeof value !== "object" || value === null) return value;
  const array = Array.isArray(value);
  return new Proxy(value as object, {
    get(target, key, receiver) {
      const got: unknown = Reflect.get(target, key, receiver);
      if (typeof key !== "string" || IGNORED.has(key) || key in Object.prototype) return got;
      if (array) return /^[0-9]+$/.test(key) ? tracked(got, reads, `${path}[]`) : got;
      const at = path === "" ? key : `${path}.${key}`;
      reads.add(at);
      return tracked(got, reads, at);
    },
    has(target, key) {
      if (typeof key === "string" && !array) reads.add(path === "" ? key : `${path}.${key}`);
      return Reflect.has(target, key);
    },
  }) as T;
}

function resolve(node: Schema): Schema {
  let at = node;
  while (typeof at === "object" && typeof at["$ref"] === "string") {
    const ref = at["$ref"];
    const name = ref.replace("#/$defs/", "");
    at = (schema["$defs"] as Record<string, Schema>)[name]!;
  }
  return at;
}

/** Whether the schema names the field at `path` under `node`, in any of its branches. */
function names(node: Schema, path: string[]): boolean {
  const at = resolve(node);
  if (path.length === 0) return true;
  if (at === true) return true;
  if (at === false) return false;
  const branches = ["anyOf", "oneOf", "allOf"].flatMap((k) => (Array.isArray(at[k]) ? (at[k] as Schema[]) : []));
  if (branches.some((b) => names(b, path))) return true;
  const [first, ...rest] = path as [string, ...string[]];
  if (first === "[]") return at["items"] !== undefined && names(at["items"] as Schema, rest);
  const properties = at["properties"] as Record<string, Schema> | undefined;
  if (properties !== undefined && first in properties) return names(properties[first]!, rest);
  // A value of any shape, such as a tool's input or a harness's own message: anything under it.
  const shaped = ["type", "properties", "items", "anyOf", "oneOf", "allOf", "enum", "const", "$ref"].some((k) => k in at);
  return !shaped;
}

/** The paths in `reads` that no message of `def` has, so the core never sends them. */
export function unknownReads(def: string, reads: Set<string>): string[] {
  const root = { $ref: `#/$defs/${def}` };
  return [...reads].filter((path) => !names(root, path.replace(/\[\]/g, ".[]").split(".").filter((p) => p !== ""))).sort();
}

/** The examples of requests from the core, each as its method and params. */
export function coreRequests(): { name: string; method: string; params: Json }[] {
  return examples
    .filter((e) => e.from === "core" && typeof e.message["method"] === "string")
    .map((e) => ({ name: e.name, method: e.message["method"] as string, params: e.message["params"] as Json }));
}
