#!/usr/bin/env python3
"""Write SHA256SUMS and a per-track `latest.json` compatibility manifest for release assets (D-165, #222).

Usage:
    python3 tools/release/write_manifest.py \
        --dir dist --track <marmot-agent|claude-code|openclaw|hermes> \
        --version X.Y.Z --tag TAG --commit SHA --socket-protocol PROTO
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
CI_TOOLS = ROOT / "tools" / "ci"
if str(CI_TOOLS) not in sys.path:
    sys.path.insert(0, str(CI_TOOLS))

import check_socket_compat as socket_compat  # noqa: E402

VALID_TRACKS = ("marmot-agent", "claude-code", "openclaw", "hermes")
SCHEMA_VERSION = 1


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build_manifest(
    dist_dir: Path,
    *,
    track: str,
    version: str,
    tag: str,
    commit: str,
    socket_protocol: str,
) -> tuple[dict[str, Any], str]:
    if track not in VALID_TRACKS:
        raise ValueError(f"unknown track {track!r}")
    if not socket_compat.is_valid_semver(version):
        raise ValueError(f"invalid SemVer version {version!r}")
    if socket_compat.protocol_major(socket_protocol) is None:
        raise ValueError(f"invalid socket protocol {socket_protocol!r}")
    if not tag:
        raise ValueError("tag must be non-empty")
    if not commit:
        raise ValueError("commit must be non-empty")

    archives = sorted(p for p in dist_dir.iterdir() if p.is_file() and p.name.endswith(".tar.gz"))
    if not archives:
        raise ValueError(f"no .tar.gz release assets found in {dist_dir}")

    assets: list[dict[str, Any]] = []
    sum_lines: list[str] = []
    for archive in archives:
        digest = sha256_file(archive)
        assets.append({"name": archive.name, "sha256": digest, "size": archive.stat().st_size})
        sum_lines.append(f"{digest}  {archive.name}")

    sums_text = "\n".join(sum_lines) + "\n"
    manifest: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "track": track,
        "version": version,
        "tag": tag,
        "prerelease": "-" in version.split("+", 1)[0],
        "commit": commit,
        "socket_protocol": socket_protocol,
        "assets": assets,
    }
    return manifest, sums_text


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dir", type=Path, required=True, help="directory containing .tar.gz assets")
    parser.add_argument("--track", choices=VALID_TRACKS, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--socket-protocol", required=True)
    args = parser.parse_args(argv)

    try:
        manifest, sums_text = build_manifest(
            args.dir,
            track=args.track,
            version=args.version,
            tag=args.tag,
            commit=args.commit,
            socket_protocol=args.socket_protocol,
        )
    except ValueError as exc:
        print(f"write-manifest: ERROR: {exc}", file=sys.stderr)
        return 1

    (args.dir / "SHA256SUMS").write_text(sums_text, encoding="utf-8")
    (args.dir / "latest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"write-manifest: wrote SHA256SUMS and latest.json for {args.tag} ({len(manifest['assets'])} asset(s))")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
