#!/usr/bin/env python3
"""Check that every crate takes the workspace's settings.

Usage:
    workspace.py

Cargo applies the workspace's lints, edition, licence and publish setting only to a crate that
asks for them. A crate that forgets is not told, and then unsafe code is allowed in it. This
check fails when a crate under crates/:

- does not inherit the workspace lints, which forbid unsafe code;
- does not inherit the edition, the Rust version, the licence or `publish = false`;
- is not named after its folder, or does not start with `marmot-agent` (D-050).

It needs Python 3.11 or newer, and nothing else.
"""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path

INHERITED = ("version", "edition", "rust-version", "license", "repository", "publish")


def check(root: Path) -> list[str]:
    problems = []
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    if workspace.get("lints", {}).get("rust", {}).get("unsafe_code") != "forbid":
        problems.append("Cargo.toml: the workspace does not forbid unsafe code")
    if workspace.get("package", {}).get("publish") is not False:
        problems.append("Cargo.toml: the workspace does not set publish = false")

    manifests = sorted((root / "crates").glob("*/Cargo.toml"))
    if not manifests:
        problems.append("crates/: no crates found")
    for manifest in manifests:
        shown = manifest.relative_to(root)
        crate = tomllib.loads(manifest.read_text(encoding="utf-8"))
        package = crate.get("package", {})
        name = package.get("name", "")
        if name != manifest.parent.name:
            problems.append(f"{shown}: the crate is named '{name}', not after its folder")
        if not name.startswith("marmot-agent"):
            problems.append(f"{shown}: the name does not start with 'marmot-agent'")
        if crate.get("lints") != {"workspace": True}:
            problems.append(f"{shown}: needs `[lints] workspace = true` and no lints of its own")
        for key in INHERITED:
            if package.get(key) != {"workspace": True}:
                problems.append(f"{shown}: needs `{key}.workspace = true`")
    return problems


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    problems = check(root)
    for problem in problems:
        print(problem)
    if problems:
        return 1
    print("every crate takes the workspace's settings")
    return 0


if __name__ == "__main__":
    sys.exit(main())
