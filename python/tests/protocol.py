"""The core's socket messages, as the core's own types describe them: the JSON Schema and the examples
in `fixtures/socket`, which a test in `crates/marmot-agent` writes and keeps true. The tests check
against them that what the plugin sends is what the core reads, and that what it reads from the core
is what the core sends.

The plugin uses the standard library and what Hermes provides, and so do its tests: this is a check of
the part of JSON Schema the core's schema uses, written here, not a validator of all of it. A keyword
it does not know fails the check, so the schema cannot grow past it unseen."""

from __future__ import annotations

import copy
import json
from pathlib import Path

FOLDER = Path(__file__).resolve().parents[2] / "fixtures" / "socket"
SCHEMA = json.loads((FOLDER / "schema.json").read_text(encoding="utf-8"))
EXAMPLES = json.loads((FOLDER / "examples.json").read_text(encoding="utf-8"))["examples"]
DEFS = SCHEMA["$defs"]

# What a keyword is for, when it says nothing about a value.
NOTES = {"description", "default", "format", "title", "$schema", "examples"}
KNOWN = NOTES | {"$ref", "type", "properties", "required", "additionalProperties", "unevaluatedProperties",
                 "items", "enum", "const", "oneOf", "anyOf", "allOf", "minimum", "maximum", "$defs"}


def kind_of(value) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, int):
        return "integer"
    if isinstance(value, float):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "array"
    if isinstance(value, dict):
        return "object"
    return type(value).__name__


def is_kind(value, kind: str) -> bool:
    found = kind_of(value)
    return found == kind or (kind == "number" and found == "integer")


def check(schema, value, at: str = "") -> tuple[list[str], set[str]]:
    """What is wrong with `value` as `schema`, and the fields of it the schema looked at, which
    `unevaluatedProperties` needs."""
    if schema is True:
        return [], set(value) if isinstance(value, dict) else set()
    if schema is False:
        return [f"{at or '/'} is not allowed"], set()
    unknown = set(schema) - KNOWN
    if unknown:
        raise ValueError(f"the schema uses {sorted(unknown)}, which this check does not know")
    errors: list[str] = []
    seen: set[str] = set()
    where = at or "/"
    if "$ref" in schema:
        e, s = check(DEFS[schema["$ref"].removeprefix("#/$defs/")], value, at)
        errors += e
        seen |= s
    if "type" in schema:
        kinds = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        if not any(is_kind(value, k) for k in kinds):
            return errors + [f"{where} is {kind_of(value)}, not {' or '.join(kinds)}"], seen
    if "const" in schema and value != schema["const"]:
        errors.append(f"{where} is not {json.dumps(schema['const'])}")
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{where} is not one of {schema['enum']}")
    if "minimum" in schema and is_kind(value, "number") and value < schema["minimum"]:
        errors.append(f"{where} is under {schema['minimum']}")
    if "maximum" in schema and is_kind(value, "number") and value > schema["maximum"]:
        errors.append(f"{where} is over {schema['maximum']}")
    if isinstance(value, list) and "items" in schema:
        for i, item in enumerate(value):
            errors += check(schema["items"], item, f"{at}/{i}")[0]
    if isinstance(value, dict):
        for name in schema.get("required", []):
            if name not in value:
                errors.append(f"{where} has no {name}")
        for name, sub in schema.get("properties", {}).items():
            if name in value:
                seen.add(name)
                errors += check(sub, value[name], f"{at}/{name}")[0]
    for sub in schema.get("allOf", []):
        e, s = check(sub, value, at)
        errors += e
        seen |= s
    if "anyOf" in schema:
        results = [check(sub, value, at) for sub in schema["anyOf"]]
        passed = [s for e, s in results if not e]
        if passed:
            for s in passed:
                seen |= s
        else:
            errors += best(results, where, "any")
    if "oneOf" in schema:
        results = [check(sub, value, at) for sub in schema["oneOf"]]
        passed = [s for e, s in results if not e]
        if len(passed) == 1:
            seen |= passed[0]
        elif passed:
            errors.append(f"{where} matches {len(passed)} of its kinds, not one")
        else:
            errors += best(results, where, "one")
    if isinstance(value, dict):
        extra = schema.get("additionalProperties")
        if extra is not None:
            for name in set(value) - seen - set(schema.get("properties", {})):
                errors += check(extra, value[name], f"{at}/{name}")[0]
                seen.add(name)
        if schema.get("unevaluatedProperties") is False:
            for name in sorted(set(value) - seen):
                errors.append(f"{at}/{name} is not a field the core's types have")
            seen = set(value)
    return errors, seen


def best(results, where: str, how: str) -> list[str]:
    """The errors of the branch that came closest: a message or an item is one branch, picked by its
    `method` or its `type`, so that branch's errors are the ones that say what is wrong."""
    picked = [e for e, _ in results if not any(" is not \"" in x and x.split(" is not ")[0].endswith(("/type", "/method"))
                                                for x in e)]
    if len(picked) == 1:
        return picked[0]
    return [f"{where} matches none of its kinds"]


def problems(definition: str, value) -> list[str]:
    """What is wrong with `value` as `definition` of the schema, or nothing."""
    errors = check({"$ref": f"#/$defs/{definition}"}, value)[0]
    return (focused(definition, value) or errors) if errors else []


def camel(word: str) -> str:
    return "".join(w[:1].upper() + w[1:] for w in word.split("_"))


def focused(definition: str, value) -> list[str]:
    """A message or an item is one branch of a `oneOf`, picked by its `method` or its `type`. These are
    the errors of the branch it picked, when its params or its body are what is wrong."""
    m = value if isinstance(value, dict) else {}
    event = m if definition == "EventParams" else m.get("params") if m.get("method") == "event" else None
    if isinstance(event, dict) and isinstance(event.get("type"), str) and camel(event["type"]) in DEFS:
        at = "/body" if definition == "EventParams" else "/params/body"
        errors = check({"$ref": f"#/$defs/{camel(event['type'])}"}, event.get("body"), at)[0]
        if errors:
            return errors
    if isinstance(m.get("method"), str) and f"{camel(m['method'])}Params" in DEFS:
        return check({"$ref": f"#/$defs/{camel(m['method'])}Params"}, m.get("params"), "/params")[0]
    return []


# -- what the plugin sends: an event, as it goes to the core

PASSED_OVER = json.loads((FOLDER / "passed-over.json").read_text(encoding="utf-8"))["fields"]


def without_passed_over(event: dict) -> dict:
    """The event without the fields `fixtures/socket/passed-over.json` lists for its type: fields a
    connector sends that the core's types do not hold yet, each an open question (Q-01)."""
    event = copy.deepcopy(event)
    for entry in PASSED_OVER:
        if entry["type"] == event.get("type"):
            drop(event.get("body"), entry["field"].replace("[]", ".[]").split("."))
    return event


def drop(value, path: list[str]) -> None:
    if not path:
        return
    first, rest = path[0], path[1:]
    if first == "[]":
        for item in value if isinstance(value, list) else []:
            drop(item, rest)
    elif isinstance(value, dict) and first in value:
        if rest:
            drop(value[first], rest)
        else:
            del value[first]


def event_problems(event: dict) -> list[str]:
    """What is wrong with an event the plugin sends, as the core reads it, beyond what is passed over."""
    return problems("EventParams", without_passed_over(event))


# -- what the plugin reads


class Reads:
    """`value`, with every field read from it, at any depth, recorded as a path such as `chat.id` or
    `attachments[].path`."""

    def __init__(self):
        self.paths: set[str] = set()

    def track(self, value, path: str = ""):
        if isinstance(value, dict):
            return TrackedDict(value, self, path)
        if isinstance(value, list):
            return TrackedList(value, self, path)
        return value


class TrackedDict(dict):
    def __init__(self, value: dict, reads: Reads, path: str):
        super().__init__(value)
        self._reads, self._path = reads, path

    def _at(self, key) -> str:
        at = f"{self._path}.{key}" if self._path else str(key)
        self._reads.paths.add(at)
        return at

    def __getitem__(self, key):
        return self._reads.track(super().__getitem__(key), self._at(key))

    def get(self, key, default=None):
        at = self._at(key)
        return self._reads.track(super().get(key, default), at) if key in self.keys() else default

    def __contains__(self, key):
        self._at(key)
        return super().__contains__(key)


class TrackedList(list):
    def __init__(self, value: list, reads: Reads, path: str):
        super().__init__(value)
        self._reads, self._path = reads, f"{path}[]"

    def __getitem__(self, index):
        got = super().__getitem__(index)
        if isinstance(index, slice):
            return [self._reads.track(v, self._path) for v in got]
        return self._reads.track(got, self._path)

    def __iter__(self):
        return (self._reads.track(v, self._path) for v in super().__iter__())


def names(schema, path: list[str]) -> bool:
    """Whether the schema names the field at `path` under `schema`, in any of its branches."""
    while isinstance(schema, dict) and "$ref" in schema:
        schema = DEFS[schema["$ref"].removeprefix("#/$defs/")]
    if not path or schema is True:
        return True
    if schema is False:
        return False
    if any(names(b, path) for k in ("anyOf", "oneOf", "allOf") for b in schema.get(k, [])):
        return True
    first, rest = path[0], path[1:]
    if first == "[]":
        return "items" in schema and names(schema["items"], rest)
    if first in schema.get("properties", {}):
        return names(schema["properties"][first], rest)
    # A value of any shape, such as a tool's input: anything under it.
    return not (set(schema) - NOTES)


def unknown_reads(definition: str, reads: Reads) -> list[str]:
    """The paths read that no message of `definition` has, so the core never sends them."""
    root = {"$ref": f"#/$defs/{definition}"}
    return sorted(p for p in reads.paths if not names(root, [s for s in p.replace("[]", ".[]").split(".") if s]))


def core_requests() -> list[tuple[str, str, dict]]:
    """The examples of requests from the core: each one's name, method and params."""
    return [(e["name"], e["message"]["method"], copy.deepcopy(e["message"]["params"]))
            for e in EXAMPLES if e["from"] == "core" and "method" in e["message"]]
