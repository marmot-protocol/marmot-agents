#!/usr/bin/env python3
"""Path-aware SemVer release planner for the four release tracks (D-165, #222).

Maps landed commits on a target ref (default `HEAD`, e.g. `origin/canary`) since each track's last
reachable stable tag to the tracks they affect, combining `Release-Impact:` commit trailers (D-162)
with a mechanical floor on `fixtures/socket/schema.json` changes (D-165):

- `marmot-agent` (tag `vX.Y.Z`): `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/`,
  `crates/`, `install/`, `tools/pi/`, `tools/goose/`, `fixtures/socket/`
- `claude-code` (tag `claude-code-vX.Y.Z`): `packages/claude-code/`
- `openclaw` (tag `openclaw-vX.Y.Z`): `packages/openclaw/`
- `hermes` (tag `hermes-vX.Y.Z`): `python/marmot_hermes/`, `python/pyproject.toml`

Usage:
    python3 tools/release/plan_release.py [--root DIR] [--ref REF]
        [--track all|marmot-agent|claude-code|openclaw|hermes]
        [--channel stable|rc] [--json]
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
CI_TOOLS = ROOT / "tools" / "ci"
if str(CI_TOOLS) not in sys.path:
    sys.path.insert(0, str(CI_TOOLS))

import check_pr_trailers as pr_trailers  # noqa: E402
import check_socket_compat as socket_compat  # noqa: E402

IMPACT_ORDER = {"none": 0, "patch": 1, "minor": 2}
STABLE_SEMVER_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")


@dataclass(frozen=True)
class TrackSpec:
    name: str
    tag_prefix: str
    exact_paths: tuple[str, ...]
    dir_prefixes: tuple[str, ...]

    def owns_path(self, rel_path: str) -> bool:
        if rel_path in self.exact_paths:
            return True
        return any(rel_path.startswith(prefix) for prefix in self.dir_prefixes)


TRACKS: tuple[TrackSpec, ...] = (
    TrackSpec(
        name="marmot-agent",
        tag_prefix="v",
        exact_paths=("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"),
        dir_prefixes=(".cargo/", "crates/", "install/", "tools/pi/", "tools/goose/", "fixtures/socket/"),
    ),
    TrackSpec(
        name="claude-code",
        tag_prefix="claude-code-v",
        exact_paths=(),
        dir_prefixes=("packages/claude-code/",),
    ),
    TrackSpec(
        name="openclaw",
        tag_prefix="openclaw-v",
        exact_paths=(),
        dir_prefixes=("packages/openclaw/",),
    ),
    TrackSpec(
        name="hermes",
        tag_prefix="hermes-v",
        exact_paths=("python/pyproject.toml",),
        dir_prefixes=("python/marmot_hermes/",),
    ),
)
TRACKS_BY_NAME = {spec.name: spec for spec in TRACKS}


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    ).stdout


def parse_stable_version(version: str) -> tuple[int, int, int] | None:
    match = STABLE_SEMVER_RE.fullmatch(version)
    if not match:
        return None
    triple = (int(match.group(1)), int(match.group(2)), int(match.group(3)))
    return None if triple == (0, 0, 0) else triple


def format_version(triple: tuple[int, int, int]) -> str:
    return f"{triple[0]}.{triple[1]}.{triple[2]}"


def bump_version(triple: tuple[int, int, int], impact: str) -> tuple[int, int, int]:
    major, minor, patch = triple
    if impact == "minor":
        return (major, minor + 1, 0)
    if impact == "patch":
        return (major, minor, patch + 1)
    return triple


def max_impact(a: str, b: str) -> str:
    return a if IMPACT_ORDER[a] >= IMPACT_ORDER[b] else b


def find_latest_stable_tag(root: Path, ref: str, spec: TrackSpec) -> tuple[str | None, tuple[int, int, int] | None]:
    """Return `(tag_name, (major, minor, patch))` for the highest stable tag reachable from `ref`."""
    tags = git(root, "tag", "--merged", ref, "--list", f"{spec.tag_prefix}*").splitlines()
    best_tag: str | None = None
    best_ver: tuple[int, int, int] | None = None
    for tag in tags:
        ver = parse_stable_version(tag[len(spec.tag_prefix):])
        if ver is None:
            continue
        if best_ver is None or ver > best_ver:
            best_ver = ver
            best_tag = tag
    return best_tag, best_ver


def next_rc_number(root: Path, spec: TrackSpec, base_version: str) -> int:
    """Return the next unused `-rc.N` integer across all repository tags for `base_version`."""
    prefix = f"{spec.tag_prefix}{base_version}-rc."
    tags = git(root, "tag", "--list", f"{prefix}*").splitlines()
    highest = 0
    for tag in tags:
        suffix = tag[len(prefix):]
        if suffix.isdigit():
            highest = max(highest, int(suffix))
    return highest + 1


def list_commits(root: Path, rev_range: str) -> list[dict[str, Any]]:
    """Return first-parent commits in `rev_range` (oldest first) with changed files and trailers."""
    shas = git(root, "rev-list", "--first-parent", "--reverse", rev_range).splitlines()
    commits: list[dict[str, Any]] = []
    for sha in shas:
        msg = git(root, "show", "-s", "--format=%B", sha)
        subject = msg.splitlines()[0].strip() if msg.splitlines() else ""
        files = [
            line
            for line in git(
                root,
                "diff-tree",
                "--no-commit-id",
                "--name-only",
                "-r",
                "--no-renames",
                "--root",
                "-m",
                "--first-parent",
                sha,
            ).splitlines()
            if line
        ]
        trailers, trailer_errors = pr_trailers.extract_landed_trailers(msg)
        raw_impact = trailers.get("release-impact")
        duplicate_impact = any("release-impact" in err.lower() for err in trailer_errors)
        normalized_impact: str | None = None
        missing_or_invalid = False
        if not duplicate_impact and raw_impact is not None and raw_impact.strip().lower() in IMPACT_ORDER:
            normalized_impact = raw_impact.strip().lower()
        else:
            missing_or_invalid = True

        commits.append(
            {
                "sha": sha,
                "short_sha": sha[:7],
                "subject": subject,
                "files": files,
                "impact": normalized_impact,
                "missing_or_invalid_impact": missing_or_invalid,
                "changelog": trailers.get("changelog"),
            }
        )
    return commits


def check_socket_floor(
    root: Path, base_tag: str, target_ref: str, track_name: str
) -> tuple[str, str, list[str]]:
    """Return `(floor_impact, schema_change, errors)` for `base_tag..target_ref`."""
    old_schema, old_err = socket_compat.load_git_schema(root, base_tag)
    if old_err is not None:
        return "none", "error", [old_err]
    new_schema, new_err = socket_compat.load_git_schema(root, target_ref)
    if new_err is not None:
        return "none", "error", [new_err]
    assert old_schema is not None and new_schema is not None
    change, violations = socket_compat.compare_schemas(old_schema, new_schema)
    if change == "breaking":
        return "none", change, violations + [
            f"breaking change to {socket_compat.SCHEMA_REL} between {base_tag} and {target_ref}"
        ]
    if change == "protocol-bump":
        return "minor", change, []
    if change == "additive" and track_name == "marmot-agent":
        return "minor", change, []
    return "none", change, []


def read_manifest_versions_at_ref(root: Path, ref: str) -> tuple[dict[str, str], str | None, list[str]]:
    """Read track manifest versions and canonical protocol at `ref` via `git show`."""
    import tomllib

    errors: list[str] = []
    versions: dict[str, str] = {}

    def read_text(rel: str) -> str | None:
        try:
            return git(root, "show", f"{ref}:{rel}")
        except subprocess.CalledProcessError as exc:
            errors.append(f"cannot read {rel} at {ref}: {exc}")
            return None

    schema_text = read_text(socket_compat.SCHEMA_REL)
    protocol: str | None = None
    if schema_text is not None:
        try:
            protocol = json.loads(schema_text).get("title")
        except ValueError as exc:
            errors.append(f"invalid JSON in {socket_compat.SCHEMA_REL} at {ref}: {exc}")

    cargo_text = read_text("Cargo.toml")
    if cargo_text is not None:
        try:
            ver = tomllib.loads(cargo_text).get("workspace", {}).get("package", {}).get("version")
            if isinstance(ver, str):
                versions["marmot-agent"] = ver
        except tomllib.TOMLDecodeError as exc:
            errors.append(f"invalid Cargo.toml at {ref}: {exc}")

    for name, rel in (
        ("claude-code", "packages/claude-code/package.json"),
        ("openclaw", "packages/openclaw/package.json"),
    ):
        text = read_text(rel)
        if text is not None:
            try:
                ver = json.loads(text).get("version")
                if isinstance(ver, str):
                    versions[name] = ver
            except ValueError as exc:
                errors.append(f"invalid {rel} at {ref}: {exc}")

    py_text = read_text("python/pyproject.toml")
    if py_text is not None:
        try:
            ver = tomllib.loads(py_text).get("project", {}).get("version")
            if isinstance(ver, str):
                versions["hermes"] = ver
        except tomllib.TOMLDecodeError as exc:
            errors.append(f"invalid python/pyproject.toml at {ref}: {exc}")

    for name in TRACKS_BY_NAME:
        if not socket_compat.is_valid_semver(versions.get(name)):
            errors.append(f"track {name}: manifest version {versions.get(name)!r} at {ref} is invalid")

    return versions, protocol, errors


def plan_track(
    root: Path,
    ref: str,
    spec: TrackSpec,
    manifest_version: str,
    channel: str,
) -> tuple[dict[str, Any], list[str]]:
    errors: list[str] = []
    manifest_base = manifest_version.split("+", 1)[0].split("-", 1)[0]
    manifest_triple = parse_stable_version(manifest_base)
    assert manifest_triple is not None, f"validated manifest version {manifest_version!r}"

    last_tag, last_ver = find_latest_stable_tag(root, ref, spec)
    if last_tag is None or last_ver is None:
        # Initial release for this track: ship the manifest version (e.g. 0.1.0).
        next_stable = format_version(manifest_triple)
        target_version = (
            f"{next_stable}-rc.{next_rc_number(root, spec, next_stable)}"
            if channel == "rc"
            else next_stable
        )
        return {
            "track": spec.name,
            "tag_prefix": spec.tag_prefix,
            "manifest_version": manifest_version,
            "last_tag": None,
            "last_version": None,
            "needs_release": True,
            "reason": f"initial release (no prior {spec.tag_prefix}* stable tag)",
            "trailer_impact": "none",
            "mechanical_floor": "none",
            "effective_impact": "initial",
            "schema_change": "none",
            "next_stable_version": next_stable,
            "next_version": target_version,
            "next_tag": f"{spec.tag_prefix}{target_version}",
            "commits": [],
            "changelog": [],
        }, []

    all_commits = list_commits(root, f"{last_tag}..{ref}")
    relevant = [c for c in all_commits if any(spec.owns_path(p) for p in c["files"])]

    trailer_impact = "none"
    changelog_entries: list[str] = []
    commit_summaries: list[dict[str, Any]] = []
    for commit in relevant:
        # Commits touching track paths without a valid Release-Impact trailer conservatively
        # default to patch so no real change is silently dropped.
        effective_commit_impact = "patch" if commit["missing_or_invalid_impact"] else commit["impact"]
        trailer_impact = max_impact(trailer_impact, effective_commit_impact)
        if commit["changelog"]:
            changelog_entries.append(f"- {commit['changelog']} ({commit['short_sha']})")
        elif effective_commit_impact != "none":
            changelog_entries.append(f"- {commit['subject']} ({commit['short_sha']})")
        commit_summaries.append(
            {
                "sha": commit["short_sha"],
                "subject": commit["subject"],
                "impact": effective_commit_impact,
                "missing_trailer": commit["missing_or_invalid_impact"],
                "changelog": commit["changelog"],
            }
        )

    floor_impact, schema_change, floor_errors = check_socket_floor(root, last_tag, ref, spec.name)
    errors.extend(floor_errors)

    effective_impact = max_impact(trailer_impact, floor_impact)
    needs_release = effective_impact != "none"

    if needs_release:
        bumped = bump_version(last_ver, effective_impact)
        # Never regress below a manifest version already bumped ahead of last_tag.
        next_triple = max(bumped, manifest_triple)
        next_stable = format_version(next_triple)
        target_version = (
            f"{next_stable}-rc.{next_rc_number(root, spec, next_stable)}"
            if channel == "rc"
            else next_stable
        )
        reasons = []
        if trailer_impact != "none":
            reasons.append(f"trailers={trailer_impact}")
        if floor_impact != "none":
            reasons.append(f"socket_floor={floor_impact} ({schema_change})")
        reason = ", ".join(reasons)
    else:
        next_stable = format_version(last_ver)
        target_version = next_stable
        reason = f"no release-impacting changes since {last_tag}"

    return {
        "track": spec.name,
        "tag_prefix": spec.tag_prefix,
        "manifest_version": manifest_version,
        "last_tag": last_tag,
        "last_version": format_version(last_ver),
        "needs_release": needs_release,
        "reason": reason,
        "trailer_impact": trailer_impact,
        "mechanical_floor": floor_impact,
        "effective_impact": effective_impact,
        "schema_change": schema_change,
        "next_stable_version": next_stable,
        "next_version": target_version,
        "next_tag": f"{spec.tag_prefix}{target_version}" if needs_release else None,
        "commits": commit_summaries,
        "changelog": changelog_entries,
    }, errors


def plan_releases(
    root: Path,
    *,
    ref: str = "HEAD",
    track: str = "all",
    channel: str = "stable",
) -> tuple[dict[str, Any], list[str]]:
    resolved_sha = git(root, "rev-parse", f"{ref}^{{commit}}").strip()
    versions, protocol, errors = read_manifest_versions_at_ref(root, resolved_sha)
    if errors:
        return {"ok": False, "ref": ref, "sha": resolved_sha, "errors": errors}, errors

    selected = list(TRACKS) if track == "all" else [TRACKS_BY_NAME[track]]
    plans: dict[str, Any] = {}
    for spec in selected:
        track_plan, track_errors = plan_track(root, resolved_sha, spec, versions[spec.name], channel)
        errors.extend(track_errors)
        if track_plan:
            plans[spec.name] = track_plan

    result = {
        "ok": not errors,
        "ref": ref,
        "sha": resolved_sha,
        "channel": channel,
        "socket_protocol": protocol,
        "tracks": plans,
        "errors": errors,
    }
    return result, errors


def format_markdown(plan: dict[str, Any]) -> str:
    lines = [
        f"# Release plan (`{plan['ref']}` @ `{plan['sha'][:7]}`, channel=`{plan['channel']}`, "
        f"protocol=`{plan['socket_protocol']}`)",
        "",
        "| Track | Last tag | Impact | Next version | Next tag | Status |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for name, info in plan["tracks"].items():
        last_tag = info["last_tag"] or "(none)"
        next_tag = f"`{info['next_tag']}`" if info["next_tag"] else "-"
        status = "release" if info["needs_release"] else "up to date"
        lines.append(
            f"| `{name}` | `{last_tag}` | `{info['effective_impact']}` | "
            f"`{info['next_version']}` | {next_tag} | {status} ({info['reason']}) |"
        )
    for name, info in plan["tracks"].items():
        if info["needs_release"] and info["changelog"]:
            lines.extend(["", f"## {name} (`{info['next_tag']}`)", ""])
            lines.extend(info["changelog"])
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT, help="repository root")
    parser.add_argument("--ref", default="HEAD", help="target git ref to plan from (default: HEAD)")
    parser.add_argument(
        "--track",
        choices=("all", *TRACKS_BY_NAME.keys()),
        default="all",
        help="track to plan (default: all)",
    )
    parser.add_argument(
        "--channel",
        choices=("stable", "rc"),
        default="stable",
        help="release channel (default: stable)",
    )
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON output")
    args = parser.parse_args(argv)

    try:
        result, errors = plan_releases(args.root.resolve(), ref=args.ref, track=args.track, channel=args.channel)
    except subprocess.CalledProcessError as exc:
        err = f"plan-release: git error: {exc}"
        if args.json:
            print(json.dumps({"ok": False, "errors": [err]}, indent=2))
        else:
            print(err, file=sys.stderr)
        return 1

    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    elif errors:
        for err in errors:
            print(f"plan-release: ERROR: {err}", file=sys.stderr)
    else:
        print(format_markdown(result))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
