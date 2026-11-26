#!/usr/bin/env bash
# Package one external socket connector track (`claude-code`, `openclaw`, or `hermes`) as a
# deterministic release tarball without building the Rust core (D-165, D-166, #222).
#
#   tools/release/package_connector.sh <claude-code|openclaw|hermes> <out folder>
#
# Reuses install/lib.sh's support_paths / copy_support so connector tarballs stay in sync with
# what the installers expect, and tools/release/archive.sh for deterministic tar+gzip output.
set -euo pipefail
[ $# -eq 2 ] || { sed -n '2,8p' "$0"; exit 2; }
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
# shellcheck source=../../install/lib.sh
. "$root/install/lib.sh"
# shellcheck source=archive.sh
. "$root/tools/release/archive.sh"
track=$1 out=$2

case $track in
    claude-code) part="packages/claude-code" ;;
    openclaw) part="packages/openclaw" ;;
    hermes) part="python/marmot_hermes" ;;
    *)
        echo "error: unknown connector track '$track' (expected claude-code, openclaw, or hermes)" >&2
        exit 2
        ;;
esac

name="marmot-agent-$track"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/$name" "$out"
copy_support "$root" "$work/$name" LICENSE "$part"
if [ "$track" = "hermes" ]; then
    install -m 0644 "$root/python/pyproject.toml" "$work/$name/python/pyproject.toml"
fi
write_release_tarball "$root" "$work" "$name" "$out"
echo "$out/$name.tar.gz"
