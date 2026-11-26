#!/usr/bin/env python3
"""Check per-track versions and socket protocol compatibility (D-165, #222).

Validates three invariants across the four independent release tracks (`marmot-agent`,
`claude-code`, `openclaw`, `hermes`):

1. Every track declares a valid SemVer version (never `0.0.0`), and lockfiles / runtime constants
   within each track agree with its package manifest.
2. Every track's declared `socketProtocol` metadata and runtime `PROTOCOL` constant matches the
   canonical protocol string in `fixtures/socket/schema.json` (`title` and
   `$defs.HelloParams.properties.protocol.const`).
3. When `--base <git-ref>` is given, loads `fixtures/socket/schema.json` at that ref (failing
   closed if unreadable) and classifies the JSON Schema change as `none`, `additive`,
   `protocol-bump`, or `breaking` with direction-aware rules:
   - `FromConnector` (connector -> core): old connector messages must remain valid under the new
     schema (new optional properties, widened types/enums, new union branches, and docs are
     additive; removed properties/branches/enum values, newly required fields, or tightened
     constraints are breaking).
   - `FromCore` (core -> connector, including defs shared by both sides): new core messages must
     also remain readable by older connectors (only new optional properties, new unused `$defs`,
     and docs are additive; removing a required field, adding `"null"`/new types, adding or
     removing enum values, or adding/removing union variants are breaking).
   - Any unhandled JSON Schema keyword change (such as `allOf`, `not`, `pattern`, `if`/`then`)
     fails closed as breaking.

Usage:
    python3 tools/ci/check_socket_compat.py [--root DIR] [--base REF] [--json]
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any

SCHEMA_REL = "fixtures/socket/schema.json"
SEMVER_RE = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
PROTOCOL_RE = re.compile(r"^marmot\.agent-control\.v([1-9][0-9]*)$")

RUST_PROTOCOL_RE = re.compile(r'pub\s+const\s+PROTOCOL\s*:\s*&str\s*=\s*"([^"]+)"\s*;')
TS_PROTOCOL_RE = re.compile(r'export\s+const\s+PROTOCOL\s*=\s*"([^"]+)"\s*;')
PY_PROTOCOL_RE = re.compile(r'^PROTOCOL\s*=\s*"([^"]+)"\s*$', re.MULTILINE)
PY_VERSION_RE = re.compile(r'^__version__\s*=\s*"([^"]+)"\s*$', re.MULTILINE)
YAML_VERSION_RE = re.compile(r'^version:\s*["\']?([^\s"\'#]+)["\']?\s*$', re.MULTILINE)

DISCRIMINATOR_KEYS = ("method", "type", "kind", "action", "decision", "status", "state")

ANNOTATION_KEYS = frozenset({"description", "default", "examples"})

HANDLED_KEYS = frozenset(
    {
        "$schema",
        "$defs",
        "title",
        "description",
        "default",
        "examples",
        "type",
        "$ref",
        "const",
        "enum",
        "format",
        "minimum",
        "exclusiveMinimum",
        "maximum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "additionalProperties",
        "unevaluatedProperties",
        "required",
        "properties",
        "items",
        "oneOf",
        "anyOf",
    }
)


def is_valid_semver(version: Any) -> bool:
    if not isinstance(version, str):
        return False
    match = SEMVER_RE.fullmatch(version)
    if not match:
        return False
    return (match.group(1), match.group(2), match.group(3)) != ("0", "0", "0")


def protocol_major(protocol: Any) -> int | None:
    if not isinstance(protocol, str):
        return None
    match = PROTOCOL_RE.fullmatch(protocol)
    return int(match.group(1)) if match else None


def _read_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def _read_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as f:
        return tomllib.load(f)


def _extract_regex(path: Path, pattern: re.Pattern[str], label: str) -> tuple[str | None, str | None]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        return None, f"{label}: cannot read {path}: {exc}"
    match = pattern.search(text)
    if not match:
        return None, f"{label}: pattern not found in {path}"
    return match.group(1), None


def check_tracks(root: Path) -> tuple[dict[str, dict[str, str]], str | None, list[str]]:
    """Validate all four track versions and socket protocol consistency under `root`."""
    errors: list[str] = []
    tracks: dict[str, dict[str, str]] = {}

    schema_path = root / SCHEMA_REL
    try:
        schema = _read_json(schema_path)
    except (OSError, ValueError) as exc:
        return tracks, None, [f"{SCHEMA_REL}: cannot parse JSON: {exc}"]

    canonical = schema.get("title")
    if protocol_major(canonical) is None:
        errors.append(
            f"{SCHEMA_REL}: title {canonical!r} must match marmot.agent-control.v<N>"
        )
        canonical = None

    hello_const = (
        schema.get("$defs", {})
        .get("HelloParams", {})
        .get("properties", {})
        .get("protocol", {})
        .get("const")
    )
    if canonical is not None and hello_const != canonical:
        errors.append(
            f"{SCHEMA_REL}: HelloParams.properties.protocol.const ({hello_const!r}) "
            f"does not match schema title ({canonical!r})"
        )

    # 1. marmot-agent (Rust workspace)
    cargo_path = root / "Cargo.toml"
    try:
        cargo = _read_toml(cargo_path)
        core_ver = cargo.get("workspace", {}).get("package", {}).get("version")
    except (OSError, tomllib.TOMLDecodeError) as exc:
        core_ver = None
        errors.append(f"Cargo.toml: cannot parse TOML: {exc}")
    if not is_valid_semver(core_ver):
        errors.append(f"Cargo.toml: workspace.package.version {core_ver!r} is not valid non-zero SemVer")

    rust_proto, err = _extract_regex(
        root / "crates/marmot-agent/src/socket.rs", RUST_PROTOCOL_RE, "marmot-agent"
    )
    if err:
        errors.append(err)
    elif canonical is not None and rust_proto != canonical:
        errors.append(
            f"crates/marmot-agent/src/socket.rs: PROTOCOL ({rust_proto!r}) != schema ({canonical!r})"
        )
    if isinstance(core_ver, str) and isinstance(rust_proto, str):
        tracks["marmot-agent"] = {"version": core_ver, "socket_protocol": rust_proto}

    # 2 & 3. TypeScript packages (claude-code, openclaw)
    for track, pkg_dir in (
        ("claude-code", "packages/claude-code"),
        ("openclaw", "packages/openclaw"),
    ):
        pkg_path = root / pkg_dir / "package.json"
        lock_path = root / pkg_dir / "package-lock.json"
        try:
            pkg = _read_json(pkg_path)
        except (OSError, ValueError) as exc:
            errors.append(f"{pkg_dir}/package.json: cannot parse JSON: {exc}")
            continue

        ver = pkg.get("version")
        if not is_valid_semver(ver):
            errors.append(f"{pkg_dir}/package.json: version {ver!r} is not valid non-zero SemVer")

        marmot_meta = pkg.get("marmot")
        proto = marmot_meta.get("socketProtocol") if isinstance(marmot_meta, dict) else None
        if protocol_major(proto) is None:
            errors.append(
                f"{pkg_dir}/package.json: marmot.socketProtocol {proto!r} must match marmot.agent-control.v<N>"
            )
        elif canonical is not None and proto != canonical:
            errors.append(
                f"{pkg_dir}/package.json: marmot.socketProtocol ({proto!r}) != schema ({canonical!r})"
            )

        try:
            lock = _read_json(lock_path)
            lock_ver = lock.get("version")
            lock_root_ver = lock.get("packages", {}).get("", {}).get("version")
            if lock_ver != ver or lock_root_ver != ver:
                errors.append(
                    f"{pkg_dir}/package-lock.json: version ({lock_ver!r} / {lock_root_ver!r}) "
                    f"does not match package.json ({ver!r})"
                )
        except (OSError, ValueError) as exc:
            errors.append(f"{pkg_dir}/package-lock.json: cannot parse JSON: {exc}")

        ts_proto, err = _extract_regex(root / pkg_dir / "src/socket.ts", TS_PROTOCOL_RE, track)
        if err:
            errors.append(err)
        elif canonical is not None and ts_proto != canonical:
            errors.append(
                f"{pkg_dir}/src/socket.ts: PROTOCOL ({ts_proto!r}) != schema ({canonical!r})"
            )

        if isinstance(ver, str) and isinstance(proto, str):
            tracks[track] = {"version": ver, "socket_protocol": proto}

    # 4. hermes (Python plugin)
    pyproject_path = root / "python/pyproject.toml"
    try:
        pyproject = _read_toml(pyproject_path)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        pyproject = {}
        errors.append(f"python/pyproject.toml: cannot parse TOML: {exc}")

    hermes_ver = pyproject.get("project", {}).get("version")
    if not is_valid_semver(hermes_ver):
        errors.append(
            f"python/pyproject.toml: project.version {hermes_ver!r} is not valid non-zero SemVer"
        )

    hermes_proto = pyproject.get("tool", {}).get("marmot", {}).get("socket_protocol")
    if protocol_major(hermes_proto) is None:
        errors.append(
            f"python/pyproject.toml: tool.marmot.socket_protocol {hermes_proto!r} "
            f"must match marmot.agent-control.v<N>"
        )
    elif canonical is not None and hermes_proto != canonical:
        errors.append(
            f"python/pyproject.toml: tool.marmot.socket_protocol ({hermes_proto!r}) "
            f"!= schema ({canonical!r})"
        )

    yaml_ver, err = _extract_regex(
        root / "python/marmot_hermes/plugin.yaml", YAML_VERSION_RE, "hermes"
    )
    if err:
        errors.append(err)
    elif hermes_ver is not None and yaml_ver != hermes_ver:
        errors.append(
            f"python/marmot_hermes/plugin.yaml: version ({yaml_ver!r}) "
            f"!= python/pyproject.toml ({hermes_ver!r})"
        )

    init_ver, err = _extract_regex(
        root / "python/marmot_hermes/__init__.py", PY_VERSION_RE, "hermes"
    )
    if err:
        errors.append(err)
    elif hermes_ver is not None and init_ver != hermes_ver:
        errors.append(
            f"python/marmot_hermes/__init__.py: __version__ ({init_ver!r}) "
            f"!= python/pyproject.toml ({hermes_ver!r})"
        )

    py_proto, err = _extract_regex(
        root / "python/marmot_hermes/core.py", PY_PROTOCOL_RE, "hermes"
    )
    if err:
        errors.append(err)
    elif canonical is not None and py_proto != canonical:
        errors.append(
            f"python/marmot_hermes/core.py: PROTOCOL ({py_proto!r}) != schema ({canonical!r})"
        )

    if isinstance(hermes_ver, str) and isinstance(hermes_proto, str):
        tracks["hermes"] = {"version": hermes_ver, "socket_protocol": hermes_proto}

    return tracks, canonical, errors


def _type_set(node: dict[str, Any]) -> set[str] | None:
    raw = node.get("type")
    if isinstance(raw, str):
        return {raw}
    if isinstance(raw, list) and all(isinstance(item, str) for item in raw):
        return set(raw)
    return None


def _strip_descriptions(obj: Any) -> Any:
    if isinstance(obj, dict):
        return {k: _strip_descriptions(v) for k, v in obj.items() if k not in ANNOTATION_KEYS}
    if isinstance(obj, list):
        return [_strip_descriptions(v) for v in obj]
    return obj


def _branch_key(branch: Any) -> tuple[Any, ...]:
    if not isinstance(branch, dict):
        return ("literal", json.dumps(branch, sort_keys=True))
    if "$ref" in branch and isinstance(branch["$ref"], str):
        return ("ref", branch["$ref"])
    if "const" in branch:
        return ("const", json.dumps(branch["const"], sort_keys=True))
    props = branch.get("properties")
    if isinstance(props, dict):
        for key in DISCRIMINATOR_KEYS:
            prop = props.get(key)
            if isinstance(prop, dict) and "const" in prop:
                return ("tag", key, json.dumps(prop["const"], sort_keys=True))
        if len(props) == 1:
            return ("prop", next(iter(props)))
    if "type" in branch and set(branch) <= (ANNOTATION_KEYS | {"type", "format", "minimum", "maximum"}):
        return ("type", json.dumps(branch["type"], sort_keys=True))
    return ("raw", json.dumps(_strip_descriptions(branch), sort_keys=True))


def _collect_refs(node: Any, out: set[str]) -> None:
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str) and ref.startswith("#/$defs/"):
            out.add(ref[len("#/$defs/"):])
        for v in node.values():
            _collect_refs(v, out)
    elif isinstance(node, list):
        for item in node:
            _collect_refs(item, out)


def _reachable_defs(schema: dict[str, Any], root_def: str) -> set[str]:
    """Return every `$defs` name reachable from `root_def` (inclusive)."""
    defs = schema.get("$defs")
    if not isinstance(defs, dict) or root_def not in defs:
        return set()
    visited: set[str] = set()
    stack = [root_def]
    while stack:
        name = stack.pop()
        if name in visited or name not in defs:
            continue
        visited.add(name)
        refs: set[str] = set()
        _collect_refs(defs[name], refs)
        stack.extend(refs - visited)
    return visited


def _resolve_ref(node: dict[str, Any], defs: dict[str, Any]) -> tuple[dict[str, Any] | None, str | None]:
    ref = node.get("$ref")
    if not isinstance(ref, str):
        return node, None
    if not ref.startswith("#/$defs/"):
        return None, f"unsupported $ref {ref!r}"
    target = defs.get(ref[len("#/$defs/"):])
    if not isinstance(target, dict):
        return None, f"unresolved $ref {ref!r}"
    # Preserve sibling keywords (Draft 2020-12 allows siblings beside $ref, e.g. description).
    merged = dict(target)
    for k, v in node.items():
        if k != "$ref":
            merged[k] = v
    return merged, None


def _is_open_schema(node: dict[str, Any]) -> bool:
    return (
        _type_set(node) is None
        and "$ref" not in node
        and "oneOf" not in node
        and "anyOf" not in node
        and "const" not in node
        and "enum" not in node
    )


def _compare_nodes(
    old: Any,
    new: Any,
    path: str,
    violations: list[str],
    *,
    old_defs: dict[str, Any],
    new_defs: dict[str, Any],
    core_sent: bool,
    seen_refs: set[tuple[str | None, str | None]],
) -> None:
    if not isinstance(old, dict) or not isinstance(new, dict):
        if old != new:
            violations.append(f"{path}: schema node changed from {old!r} to {new!r}")
        return

    # Unhandled keywords fail closed if modified or added.
    for key in sorted((set(old) | set(new)) - HANDLED_KEYS):
        if old.get(key) != new.get(key):
            violations.append(
                f"{path}: unhandled schema keyword {key!r} changed from {old.get(key)!r} to {new.get(key)!r}"
            )

    # Resolve $ref changes (when one side introduces/removes a $ref or changes its target).
    old_ref = old.get("$ref") if isinstance(old.get("$ref"), str) else None
    new_ref = new.get("$ref") if isinstance(new.get("$ref"), str) else None
    if old_ref is not None and old_ref == new_ref:
        # Both reference the same $defs entry; that entry is compared directly under $defs.
        # Still check any non-$ref sibling keywords other than annotations/title.
        ignored = ANNOTATION_KEYS | {"$ref", "title"}
        old_sib = {k: v for k, v in old.items() if k not in ignored}
        new_sib = {k: v for k, v in new.items() if k not in ignored}
        if old_sib != new_sib:
            violations.append(f"{path}: sibling keywords beside $ref changed from {old_sib!r} to {new_sib!r}")
        return
    if old_ref is not None or new_ref is not None:
        pair = (old_ref, new_ref)
        if pair in seen_refs:
            return
        seen_refs = seen_refs | {pair}
        resolved_old, err_old = _resolve_ref(old, old_defs)
        resolved_new, err_new = _resolve_ref(new, new_defs)
        if err_old:
            violations.append(f"{path}: {err_old}")
            return
        if err_new:
            violations.append(f"{path}: {err_new}")
            return
        _compare_nodes(
            resolved_old,
            resolved_new,
            path,
            violations,
            old_defs=old_defs,
            new_defs=new_defs,
            core_sent=core_sent,
            seen_refs=seen_refs,
        )
        return

    # type narrowing / change (and widening on core-sent messages)
    old_types = _type_set(old)
    new_types = _type_set(new)
    if _is_open_schema(old) and not _is_open_schema(new):
        violations.append(f"{path}: open schema narrowed")
    elif not _is_open_schema(old) and _is_open_schema(new) and core_sent:
        violations.append(f"{path}: core-sent schema widened to open/untyped value")
    elif old_types is not None and new_types is not None:
        removed_types = sorted(old_types - new_types)
        if removed_types:
            violations.append(f"{path}: removed allowed type(s) {removed_types}")
        added_types = sorted(new_types - old_types)
        if core_sent and added_types:
            violations.append(f"{path}: core-sent schema added allowed type(s) {added_types}")

    # const constraint
    if "const" in old:
        if "const" not in new or new["const"] != old["const"]:
            violations.append(f"{path}: const changed from {old['const']!r} to {new.get('const')!r}")
    elif "const" in new:
        violations.append(f"{path}: tightened with new const {new['const']!r}")

    # enum constraint
    if "enum" in old:
        if "enum" in new:
            old_enum = old["enum"]
            new_enum = new["enum"]
            removed = [v for v in old_enum if v not in new_enum]
            if removed:
                violations.append(f"{path}: removed enum value(s) {removed!r}")
            added = [v for v in new_enum if v not in old_enum]
            if core_sent and added:
                violations.append(f"{path}: core-sent schema added enum value(s) {added!r}")
        elif core_sent:
            violations.append(f"{path}: core-sent schema removed enum restriction")
    elif "enum" in new:
        violations.append(f"{path}: tightened with new enum {new['enum']!r}")

    # format constraint
    if old.get("format") != new.get("format"):
        if new.get("format") is not None:
            violations.append(
                f"{path}: format changed from {old.get('format')!r} to {new.get('format')!r}"
            )
        elif core_sent:
            violations.append(
                f"{path}: core-sent schema removed format {old.get('format')!r}"
            )

    # Numeric and length bounds
    for key in ("minimum", "exclusiveMinimum", "minLength", "minItems"):
        if key in new:
            if key not in old or new[key] > old[key]:
                violations.append(f"{path}: tightened {key} from {old.get(key)!r} to {new[key]!r}")
            elif core_sent and new[key] < old[key]:
                violations.append(f"{path}: core-sent schema loosened {key} from {old[key]!r} to {new[key]!r}")
        elif core_sent and key in old:
            violations.append(f"{path}: core-sent schema removed {key} ({old[key]!r})")
    for key in ("maximum", "exclusiveMaximum", "maxLength", "maxItems"):
        if key in new:
            if key not in old or new[key] < old[key]:
                violations.append(f"{path}: tightened {key} from {old.get(key)!r} to {new[key]!r}")
            elif core_sent and new[key] > old[key]:
                violations.append(f"{path}: core-sent schema loosened {key} from {old[key]!r} to {new[key]!r}")
        elif core_sent and key in old:
            violations.append(f"{path}: core-sent schema removed {key} ({old[key]!r})")

    # Subschema/boolean keywords: additionalProperties, unevaluatedProperties, and items.
    for key in ("additionalProperties", "unevaluatedProperties", "items"):
        old_sub = old.get(key)
        new_sub = new.get(key)
        if old_sub == new_sub:
            continue
        old_open = old_sub is None or old_sub is True
        new_open = new_sub is None or new_sub is True
        if old_open and new_open:
            continue
        if isinstance(old_sub, dict) and isinstance(new_sub, dict):
            _compare_nodes(
                old_sub,
                new_sub,
                f"{path}.{key}",
                violations,
                old_defs=old_defs,
                new_defs=new_defs,
                core_sent=core_sent,
                seen_refs=seen_refs,
            )
        elif new_sub is False:
            violations.append(f"{path}: tightened {key} to false")
        elif old_open and isinstance(new_sub, dict):
            violations.append(f"{path}: tightened {key} with schema {new_sub!r}")
        elif old_sub is False and (new_open or isinstance(new_sub, dict)):
            if core_sent:
                violations.append(f"{path}: core-sent schema loosened {key} from false to {new_sub!r}")
        elif isinstance(old_sub, dict) and new_open:
            if core_sent:
                violations.append(f"{path}: core-sent schema removed {key} constraint")
        else:
            violations.append(f"{path}: unsupported {key} change from {old_sub!r} to {new_sub!r}")

    # Object required fields
    old_req = set(old.get("required") or [])
    new_req = set(new.get("required") or [])
    added_req = sorted(new_req - old_req)
    if added_req:
        violations.append(f"{path}: added required field(s) {added_req}")
    removed_req = sorted(old_req - new_req)
    if core_sent and removed_req:
        violations.append(f"{path}: core-sent object removed required field(s) {removed_req}")

    # Object properties (removing an existing property is breaking; adding optional property is additive)
    old_props = old.get("properties")
    new_props = new.get("properties")
    if isinstance(old_props, dict):
        if not isinstance(new_props, dict):
            violations.append(f"{path}: removed properties mapping")
        else:
            for prop_name, old_prop in old_props.items():
                if prop_name not in new_props:
                    violations.append(f"{path}.properties.{prop_name}: property removed")
                else:
                    _compare_nodes(
                        old_prop,
                        new_props[prop_name],
                        f"{path}.properties.{prop_name}",
                        violations,
                        old_defs=old_defs,
                        new_defs=new_defs,
                        core_sent=core_sent,
                        seen_refs=seen_refs,
                    )

    # Union combinators (oneOf / anyOf)
    for comb in ("oneOf", "anyOf"):
        old_branches = old.get(comb)
        new_branches = new.get(comb)
        if not isinstance(old_branches, list):
            if isinstance(new_branches, list):
                violations.append(f"{path}.{comb}: newly introduced {comb} combinator")
            continue
        if not isinstance(new_branches, list):
            violations.append(f"{path}.{comb}: combinator removed")
            continue
        by_key: dict[tuple[Any, ...], list[Any]] = {}
        for branch in new_branches:
            by_key.setdefault(_branch_key(branch), []).append(branch)
        matched_keys: set[tuple[Any, ...]] = set()
        for idx, old_branch in enumerate(old_branches):
            key = _branch_key(old_branch)
            candidates = by_key.get(key, [])
            if len(candidates) == 1:
                matched_keys.add(key)
                _compare_nodes(
                    old_branch,
                    candidates[0],
                    f"{path}.{comb}[{idx}]",
                    violations,
                    old_defs=old_defs,
                    new_defs=new_defs,
                    core_sent=core_sent,
                    seen_refs=seen_refs,
                )
            elif len(candidates) > 1:
                matched_keys.add(key)
                stripped = _strip_descriptions(old_branch)
                if not any(_strip_descriptions(c) == stripped for c in candidates):
                    violations.append(f"{path}.{comb}[{idx}]: ambiguous branch modified ({key})")
            else:
                violations.append(f"{path}.{comb}[{idx}]: union branch removed ({key})")
        if core_sent:
            for idx, new_branch in enumerate(new_branches):
                key = _branch_key(new_branch)
                if key not in matched_keys:
                    violations.append(f"{path}.{comb}[{idx}]: core-sent union added new branch ({key})")


def compare_schemas(old: dict[str, Any], new: dict[str, Any]) -> tuple[str, list[str]]:
    """Classify a change between `old` and `new` socket schemas.

    Returns `(classification, violations)` where `classification` is one of
    `none`, `additive`, `protocol-bump`, or `breaking`.
    """
    if old == new:
        return "none", []

    old_title = old.get("title")
    new_title = new.get("title")
    old_major = protocol_major(old_title)
    new_major = protocol_major(new_title)
    if old_title != new_title:
        if old_major is not None and new_major is not None and new_major > old_major:
            return "protocol-bump", []
        return "breaking", [
            f"schema title changed from {old_title!r} to {new_title!r} without incrementing "
            f"marmot.agent-control.v<N>"
        ]

    old_defs = old.get("$defs") or {}
    new_defs = new.get("$defs") or {}
    core_defs = _reachable_defs(old, "FromCore") | _reachable_defs(new, "FromCore")

    violations: list[str] = []
    # Top-level anyOf (FromCore / FromConnector)
    _compare_nodes(
        {k: v for k, v in old.items() if k != "$defs"},
        {k: v for k, v in new.items() if k != "$defs"},
        "$",
        violations,
        old_defs=old_defs,
        new_defs=new_defs,
        core_sent=False,
        seen_refs=set(),
    )

    for name, old_def in old_defs.items():
        if name not in new_defs:
            violations.append(f"#/$defs/{name}: definition removed")
        else:
            _compare_nodes(
                old_def,
                new_defs[name],
                f"#/$defs/{name}",
                violations,
                old_defs=old_defs,
                new_defs=new_defs,
                core_sent=(name in core_defs),
                seen_refs=set(),
            )

    if violations:
        return "breaking", violations
    return "additive", []


def load_git_schema(root: Path, base: str) -> tuple[dict[str, Any] | None, str | None]:
    proc = subprocess.run(
        ["git", "-C", str(root), "show", f"{base}:{SCHEMA_REL}"],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip() or f"git show exited {proc.returncode}"
        return None, f"cannot read {SCHEMA_REL} at base {base!r}: {detail}"
    try:
        return json.loads(proc.stdout), None
    except ValueError as exc:
        return None, f"{SCHEMA_REL} at base {base!r} is not valid JSON: {exc}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=".", help="repository root (default: .)")
    parser.add_argument("--base", help="base git ref to compare fixtures/socket/schema.json against")
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON output")
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    tracks, protocol, errors = check_tracks(root)

    schema_change = "none"
    if args.base and not errors:
        old_schema, base_err = load_git_schema(root, args.base)
        if base_err is not None:
            errors.append(base_err)
        elif old_schema is not None:
            new_schema = _read_json(root / SCHEMA_REL)
            schema_change, violations = compare_schemas(old_schema, new_schema)
            if schema_change == "breaking":
                next_ver = (protocol_major(protocol) or 0) + 1
                errors.extend(violations)
                errors.append(
                    f"Breaking change to {SCHEMA_REL} while keeping {protocol!r}. "
                    f"Either make the change additive or bump to marmot.agent-control.v{next_ver} "
                    f"across the core and every connector."
                )

    result = {
        "ok": not errors,
        "protocol": protocol,
        "tracks": tracks,
        "schema_change": schema_change,
        "errors": errors,
    }
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    elif errors:
        for err in errors:
            print(f"socket-compat: ERROR: {err}", file=sys.stderr)
    else:
        versions = ", ".join(f"{name}={info['version']}" for name, info in sorted(tracks.items()))
        print(f"socket-compat: OK (protocol={protocol}, schema_change={schema_change}; {versions})")
    return 0 if not errors else 1


if __name__ == "__main__":
    sys.exit(main())
