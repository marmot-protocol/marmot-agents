#!/bin/sh
# Run `goose acp` in a throwaway container, speaking ACP on this script's standard input and output.
#
#   tools/goose/goose-acp.sh <model-key-file> [goose-home]
#
# The current folder is the work folder: it is mounted at the same path inside, so the paths the
# client and Goose name are the same. Goose keeps its configuration and sessions in <goose-home>
# (default: $HOME/.goose-acp), so a session survives the container and can be resumed by the next.
# The key file is mounted read-only and is never copied, printed or put on a command line.
#
# Settings, from the environment:
#   MODEL_HOST       default https://api.numes.ai
#   MODEL_BASE_PATH  default api/v1/chat/completions
#   MODEL_NAME       default z-ai/glm-5.3-flash:free
#   GOOSE_MODE       auto (default) or approve; RECORD_ASK=1 means approve
#   GOOSE_IMAGE      default marmot-agents-goose, built from this folder when missing
set -eu
[ $# -ge 1 ] || { sed -n '2,17p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
key=$(realpath "$1")
home=$(realpath -m "${2:-$HOME/.goose-acp}")
work=$(pwd)
image=${GOOSE_IMAGE:-marmot-agents-goose}
mode=${GOOSE_MODE:-auto}
[ "${RECORD_ASK:-}" = 1 ] && mode=approve

mkdir -p "$home"
docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" "$here" >&2
# One container per Goose home. One left from a client that was killed, which Docker keeps running,
# is removed first, so two never share a home.
name="goose-acp-$(printf '%s' "$home" | sha256sum | cut -c1-12)"
docker rm -f "$name" >/dev/null 2>&1 || true
exec docker run -i --rm --name "$name" \
    --user "$(id -u):$(id -g)" \
    -v "$work:$work" -w "$work" \
    -v "$home:/goose-home" -e HOME=/goose-home \
    -v "$key:/run/secrets/model_key:ro" \
    -e MODEL_HOST="${MODEL_HOST:-https://api.numes.ai}" \
    -e MODEL_BASE_PATH="${MODEL_BASE_PATH:-api/v1/chat/completions}" \
    -e MODEL_NAME="${MODEL_NAME:-z-ai/glm-5.3-flash:free}" \
    -e GOOSE_MODE="$mode" \
    "$image"
