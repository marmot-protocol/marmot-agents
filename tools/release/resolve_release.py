#!/usr/bin/env python3
"""Resolve and validate release metadata for .github/workflows/release.yml (D-166, #222).

Handles both tag pushes (`--tag <tag>`) and on-demand `workflow_dispatch` planning/RC cuts from
`canary` (`--dispatch --ref <sha> --track <track> --channel <rc|stable> [--dry-run]`), checking
that manifest versions match the release version so published artifacts never disagree with their
`latest.json` metadata.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

RELEASE_DIR = Path(__file__).resolve().parent
ROOT = RELEASE_DIR.parents[1]
CI_DIR = ROOT / "tools" / "ci"
for p in (str(RELEASE_DIR), str(CI_DIR)):
    if p not in sys.path:
        sys.path.insert(0, p)

import check_socket_compat as socket_compat  # noqa: E402
import plan_release as planner  # noqa: E402

TAG_PREFIXES: tuple[tuple[str, str], ...] = (
    ("claude-code-v", "claude-code"),
    ("openclaw-v", "openclaw"),
    ("hermes-v", "hermes"),
    ("v", "marmot-agent"),
)


def parse_release_tag(tag: str) -> tuple[str, str]:
    for prefix, track in TAG_PREFIXES:
        if tag.startswith(prefix):
            version = tag[len(prefix) :]
            if not socket_compat.is_valid_semver(version):
                raise ValueError(f"tag {tag!r} carries invalid SemVer version {version!r}")
            return track, version
    raise ValueError(f"unsupported release tag {tag!r}")


def _manifest_matches_version(manifest_version: str, target_version: str, prerelease: bool) -> bool:
    target_base = target_version.split("+", 1)[0].split("-", 1)[0]
    if prerelease:
        return manifest_version in (target_version, target_base)
    return manifest_version == target_version


def _release_notes(track: str, tag: str, protocol: str, summary: str | None = None) -> str:
    if track == "marmot-agent":
        install_hint = "Install with a script from `install/` (see README.md)."
    else:
        install_hint = f"Standalone `{track}` connector archive for `marmot-agent` (`{protocol}`)."
    header = summary if summary else f"{track} {tag} (socket protocol `{protocol}`)."
    return (
        f"{header}\n\n"
        f"{install_hint} Each archive's checksum is in `SHA256SUMS` and `latest.json`. "
        "Releases are not signed yet.\n"
    )


def resolve_tag(root: Path, tag: str, commit: str) -> tuple[dict[str, Any], str]:
    track, version = parse_release_tag(tag)
    tracks, protocol, errors = socket_compat.check_tracks(root)
    if errors or protocol is None:
        raise ValueError("; ".join(errors) or "socket compatibility check failed")

    manifest_version = tracks[track]["version"]
    prerelease = "-" in version.split("+", 1)[0]
    if not _manifest_matches_version(manifest_version, version, prerelease):
        kind = "Prerelease" if prerelease else "Stable"
        raise ValueError(
            f"{kind} tag {tag} ({version}) does not match {track} manifest version {manifest_version}."
        )

    meta = {
        "track": track,
        "version": version,
        "tag": tag,
        "commit": commit,
        "socket_protocol": protocol,
        "prerelease": prerelease,
        "should_build": True,
        "verify_existing_tag": True,
    }
    return meta, _release_notes(track, tag, protocol)


def resolve_dispatch(
    root: Path,
    *,
    ref: str,
    track: str,
    channel: str,
    dry_run: bool,
) -> tuple[dict[str, Any], str, str]:
    tracks, protocol, compat_errors = socket_compat.check_tracks(root)
    if compat_errors or protocol is None:
        raise ValueError("; ".join(compat_errors) or "socket compatibility check failed")

    plan, plan_errors = planner.plan_releases(root, ref=ref, track=track, channel=channel)
    if plan_errors:
        raise ValueError("; ".join(plan_errors))

    summary_md = planner.format_markdown(plan)
    info = plan["tracks"][track]
    needs_release = bool(info["needs_release"])
    next_version = info["next_version"]
    next_stable = info["next_stable_version"]
    next_tag = info["next_tag"]
    commit = plan["sha"]

    if dry_run or not needs_release or not next_tag:
        return (
            {
                "track": track,
                "version": next_version,
                "tag": next_tag or "",
                "commit": commit,
                "socket_protocol": protocol,
                "prerelease": channel == "rc",
                "should_build": False,
                "verify_existing_tag": False,
            },
            summary_md,
            "",
        )

    if channel == "stable":
        raise ValueError(
            f"Stable tags require a signed git tag under the Release tags ruleset. "
            f"Bump {track} to {next_version} if needed, then run: "
            f"git tag -s {next_tag} {commit} && git push origin {next_tag}"
        )

    manifest_version = tracks[track]["version"]
    if not _manifest_matches_version(manifest_version, next_version, prerelease=True):
        raise ValueError(
            f"Planned RC {next_tag} ({next_version}) targets base version {next_stable}, "
            f"but {track} manifest is still at {manifest_version}. "
            f"Bump {track} to {next_stable} first so packaged artifacts match latest.json."
        )

    meta = {
        "track": track,
        "version": next_version,
        "tag": next_tag,
        "commit": commit,
        "socket_protocol": protocol,
        "prerelease": True,
        "should_build": True,
        "verify_existing_tag": False,
    }
    return meta, summary_md, _release_notes(track, next_tag, protocol, summary=summary_md)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--tag", help="pushed release tag name")
    mode.add_argument("--dispatch", action="store_true", help="workflow_dispatch mode from canary")
    parser.add_argument("--commit", help="commit SHA (for --tag)")
    parser.add_argument("--ref", default="HEAD", help="target ref/SHA (for --dispatch)")
    parser.add_argument("--track", choices=("marmot-agent", "claude-code", "openclaw", "hermes"))
    parser.add_argument("--channel", choices=("rc", "stable"), default="rc")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--notes-out", type=Path, help="write release notes Markdown here when building")
    parser.add_argument("--summary-out", type=Path, help="write dispatch plan summary Markdown here")
    parser.add_argument("--github-output", type=Path, help="append key=value lines for GitHub Actions")
    args = parser.parse_args(argv)

    root = args.root.resolve()
    try:
        if args.tag:
            if not args.commit:
                raise ValueError("--commit is required with --tag")
            meta, notes = resolve_tag(root, args.tag, args.commit)
            summary = ""
        else:
            if not args.track:
                raise ValueError("--track is required with --dispatch")
            meta, summary, notes = resolve_dispatch(
                root,
                ref=args.ref,
                track=args.track,
                channel=args.channel,
                dry_run=args.dry_run,
            )
    except ValueError as exc:
        print(f"resolve-release: ERROR: {exc}", file=sys.stderr)
        return 1

    if summary and args.summary_out:
        args.summary_out.parent.mkdir(parents=True, exist_ok=True)
        with args.summary_out.open("a", encoding="utf-8") as f:
            f.write(summary + "\n")
    if summary:
        print(summary)
    if notes and args.notes_out and meta["should_build"]:
        args.notes_out.parent.mkdir(parents=True, exist_ok=True)
        args.notes_out.write_text(notes, encoding="utf-8")
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as f:
            for key, value in meta.items():
                rendered = str(value).lower() if isinstance(value, bool) else str(value)
                f.write(f"{key}={rendered}\n")
    print(json.dumps(meta, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
