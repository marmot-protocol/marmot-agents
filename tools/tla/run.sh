#!/usr/bin/env bash
# Runs TLC, the TLA+ model checker, on one model and one configuration.
#
#   tools/tla/run.sh <folder> <Model.tla> <Model.cfg> [more TLC options]
#
# <folder> holds the model and the configuration; TLC runs there, so its paths are relative to
# it. The TLC release jar is downloaded once, pinned by version and checksum, into a cache
# folder ($TLA_CACHE, or ~/.cache/marmot-agents/tla), and is never committed. It runs with the
# local Java when there is one, and otherwise in a pinned Java image with Docker. TLA2TOOLS_JAR
# names a jar already on disk instead, which must match the checksum.
#
# The exit status is TLC's: 0 when the model checks, not 0 when it finds a violation.
set -euo pipefail

# To move to another release, change the version and the checksum together: the checksum is
# the jar's SHA-256, as MDK pins the same release (its Justfile, `_tla-tools`).
TLA_VERSION=1.7.4
TLA_SHA256=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88
# Used only when there is no local Java: Eclipse Temurin 21, pinned by digest.
JAVA_IMAGE=eclipse-temurin:21.0.9_10-jre@sha256:d45e4719da1774b3abde398867f78a6d9b9afe4265f574f3d54062f6daeb00b3

if [[ $# -lt 3 ]]; then
    echo "usage: $0 <folder> <Model.tla> <Model.cfg> [TLC options]" >&2
    exit 2
fi
folder="$(cd "$1" && pwd)"
model="$2"
config="$3"
shift 3

checksum() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# Fails unless the file is the pinned release.
verify() {
    local actual
    actual="$(checksum "$1")"
    if [[ "$actual" != "$TLA_SHA256" ]]; then
        echo "error: $1 is not TLC v${TLA_VERSION}: expected $TLA_SHA256, got $actual" >&2
        return 1
    fi
}

if [[ -n "${TLA2TOOLS_JAR:-}" ]]; then
    jar="$TLA2TOOLS_JAR"
else
    cache="${TLA_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/marmot-agents/tla}"
    jar="$cache/tla2tools-v${TLA_VERSION}.jar"
    if [[ ! -f "$jar" ]]; then
        mkdir -p "$cache"
        # Downloaded beside the cache's jar and checked there; only a jar that matches is moved
        # into place, and anything else is deleted.
        tmp="$(mktemp "${jar}.tmp.XXXXXX")"
        trap 'rm -f "$tmp"' EXIT
        curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
            --connect-timeout 20 --max-time 300 --retry 5 --retry-all-errors \
            --output "$tmp" \
            "https://github.com/tlaplus/tlaplus/releases/download/v${TLA_VERSION}/tla2tools.jar"
        verify "$tmp"
        mv "$tmp" "$jar"
        trap - EXIT
    fi
fi
# Checked on every run, so a jar changed in the cache, or named by TLA2TOOLS_JAR, is refused.
verify "$jar"

# One worker keeps runs reproducible; the in-memory fingerprint set suits these small models.
options=(-workers "${TLC_WORKERS:-1}" -deadlock -cleanup -metadir "states" -config "$config")
if command -v java >/dev/null; then
    cd "$folder"
    exec java -XX:+UseParallelGC -Xmx"${TLC_HEAP:-2g}" -jar "$jar" "${options[@]}" "$@" "$model"
fi
if ! command -v docker >/dev/null; then
    echo "error: TLC needs Java, or Docker to run it in a Java image" >&2
    exit 1
fi
exec docker run --rm --network none --user "$(id -u):$(id -g)" \
    -v "$folder:/model" -v "$jar:/tla/tla2tools.jar:ro" -w /model "$JAVA_IMAGE" \
    java -XX:+UseParallelGC -Xmx"${TLC_HEAP:-2g}" -jar /tla/tla2tools.jar \
    "${options[@]}" "$@" "$model"
