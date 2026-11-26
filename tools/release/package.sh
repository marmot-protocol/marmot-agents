#!/usr/bin/env bash
# Package a built marmot-agent with the files its connectors, plugins and containers need, as a release
# asset.
#
#   tools/release/package.sh <marmot-agent program> <target> <out folder>
#
# For example: tools/release/package.sh target/release/marmot-agent x86_64-unknown-linux-gnu dist
# writes dist/marmot-agent-x86_64-unknown-linux-gnu.tar.gz, which unpacks to one folder holding
# bin/marmot-agent, the install scripts and the support files that install/lib.sh lists. The release
# workflow (.github/workflows/release.yml) runs it, and the install scripts' tests do.
set -euo pipefail
[ $# -eq 3 ] || { sed -n '2,9p' "$0"; exit 2; }
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
# shellcheck source=../../install/lib.sh
. "$root/install/lib.sh"
# shellcheck source=archive.sh
. "$root/tools/release/archive.sh"
binary=$1 target=$2 out=$3
case $target in *[!a-z0-9_-]*) echo "error: the target is a name such as x86_64-unknown-linux-gnu" >&2; exit 2 ;; esac
[ -x "$binary" ] || { echo "error: $binary is not a program" >&2; exit 2; }
name="marmot-agent-$target"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/$name/bin" "$out"
install -m 0755 "$binary" "$work/$name/bin/marmot-agent"
copy_support "$root" "$work/$name"
write_release_tarball "$root" "$work" "$name" "$out"
echo "$out/$name.tar.gz"
