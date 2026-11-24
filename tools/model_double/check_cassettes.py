#!/usr/bin/env python3
"""Reports the model double's cassettes that are due to be recorded again.

Usage:
    check_cassettes.py <folder> [--max-age DAYS] [--offline] [--strict] [--today YYYY-MM-DD]

It reads the first line of every `*.jsonl` under `<folder>` (`{"cassette": 1, "harness", "version",
"recorded"}`), and reports:

- a cassette recorded more than `--max-age` days ago (14 by default);
- a harness with a release newer than the version a cassette was recorded with, looked up in the npm registry
  (unless `--offline`). A harness it has no lookup for is reported once, so the list stays complete.

It prints one line per finding. It exits 0 whatever it finds, so a pull request is told and not held up,
and with `--strict` exits 1 when a cassette is stale, so a nightly job can fail and open an issue with the output.
A newer release alone is reported and does not fail (D-127): drift shows as a request the cassette cannot
match, not as a forced recording, which needs a key, on every release.
A `<folder>` that is not there is an error, exit 2, with or without `--strict`.
It needs no key: the lookups are public. It uses the standard library only.
"""

from __future__ import annotations

import argparse
import datetime
import json
import sys
import urllib.request
from pathlib import Path

# Where each harness's releases are published: the npm package whose version is the harness's own.
NPM = {
    "claude-code": "@anthropic-ai/claude-code",
    "codex": "@openai/codex",
    "opencode": "opencode-ai",
}


def npm_latest(package: str) -> str:
    url = f"https://registry.npmjs.org/{package.replace('/', '%2F')}/latest"
    request = urllib.request.Request(url, headers={"Accept": "application/json", "User-Agent": "check-cassettes"})
    with urllib.request.urlopen(request, timeout=30) as r:
        return json.loads(r.read())["version"]


def newer(a: str, b: str) -> bool:
    """Whether version `a` is newer than `b`, by semantic versioning's precedence: the numbers of each, a release
    above its pre-releases, pre-releases by their identifiers, and build metadata (`+…`) ignored."""
    def parts(v: str) -> tuple:
        main, _, pre = v.split("+", 1)[0].partition("-")
        numbers = tuple(int(x) if x.isdigit() else 0 for x in main.split("."))
        # A numeric identifier is below any other, and a shorter list below a longer one it begins.
        ids = tuple((0, int(x), "") if x.isdigit() else (1, 0, x) for x in pre.split(".")) if pre else ()
        return numbers, pre == "", ids

    return parts(a) > parts(b)


def check(folder: Path, today: datetime.date, max_age: int, latest=None) -> list[str]:
    found = []
    versions: dict[str, set[str]] = {}
    for path in sorted(folder.rglob("*.jsonl")):
        header = json.loads(path.read_text(encoding="utf-8").split("\n", 1)[0])
        name = path.relative_to(folder)
        recorded = datetime.date.fromisoformat(header["recorded"])
        age = (today - recorded).days
        if age > max_age:
            found.append(f"stale: {name} was recorded {age} days ago, on {recorded}; record it again")
        versions.setdefault(header["harness"], set()).add(header["version"])
    if latest is None:
        return found
    for harness, recorded in sorted(versions.items()):
        if harness not in NPM:
            found.append(f"unknown: no release lookup for {harness}; check it by hand")
            continue
        try:
            current = latest(NPM[harness])
        except Exception as e:  # noqa: BLE001 - a failed lookup is reported, not fatal
            found.append(f"lookup failed: {harness} ({type(e).__name__})")
            continue
        for version in sorted(recorded):
            if newer(current, version):
                found.append(f"newer: {harness} {current} is out; cassettes were recorded with {version}")
    return found


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("folder", type=Path)
    ap.add_argument("--max-age", type=int, default=14)
    ap.add_argument("--offline", action="store_true", help="do not look up releases")
    ap.add_argument("--strict", action="store_true", help="exit 1 when a cassette is stale")
    ap.add_argument("--today", type=datetime.date.fromisoformat, default=datetime.date.today())
    args = ap.parse_args()
    if not args.folder.is_dir():
        ap.error(f"{args.folder} is not a folder")
    found = check(args.folder, args.today, args.max_age, None if args.offline else npm_latest)
    for line in found:
        print(line)
    return 1 if args.strict and failing(found) else 0


def failing(found: list[str]) -> bool:
    """Whether the findings fail a `--strict` run: only a stale cassette does (D-127)."""
    return any(line.startswith("stale:") for line in found)


if __name__ == "__main__":
    sys.exit(main())
