#!/bin/sh
# Run `goose acp` in a throwaway container, speaking ACP on this script's standard input and output.
#
#   tools/goose/goose-acp.sh <model-key-file> [goose-home]
#
# The current folder is the work folder: it is mounted at the same path inside, read-write, so the
# paths the client and Goose name are the same. It must not hold <goose-home> or the key file, nor be
# inside <goose-home>, nor hold or be inside MARMOT_AGENT_DATA when that is set: the script refuses to
# start if it does, links resolved. Goose keeps its configuration and sessions in <goose-home>
# (default: $HOME/.goose-acp), so a session survives the container and can be resumed by the next. The key file is mounted read-only and is never copied,
# printed or put on a command line. Docker keeps no log of the container: what Goose writes holds
# the conversation.
#
# Settings, from the environment:
#   MODEL_BASE_URL   an OpenAI-compatible endpoint, such as https://models.example/v1 (required)
#   MODEL_NAME       the model, as the endpoint names it (required)
#   GOOSE_MODE       auto or approve; unset, Goose's own default, auto, which asks nothing.
#                    RECORD_ASK=1 means approve
#   GOOSE_IMAGE      default marmot-agents-goose, built from this folder when missing
set -eu
[ $# -ge 1 ] || { sed -n '2,20p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
key=$(realpath "$1")
home=$(realpath -m "${2:-$HOME/.goose-acp}")
work=$(pwd -P)
inside() { case "$1/" in "${2%/}"/*) return 0 ;; esac; return 1; }
if inside "$home" "$work" || inside "$work" "$home"; then
    echo "error: the Goose home and the working folder must not hold one another" >&2; exit 2
fi
if inside "$key" "$work"; then
    echo "error: the model key file must not be inside the working folder" >&2; exit 2
fi
# MARMOT_AGENT_DATA, which the installer sets, names the folder that holds every agent: the work folder
# must neither hold it nor be inside it, so it cannot reach another agent's keys either.
if [ -n "${MARMOT_AGENT_DATA:-}" ]; then
    data=$(realpath -m "$MARMOT_AGENT_DATA")
    if inside "$work" "$data" || inside "$data" "$work"; then
        echo "error: the work folder and the agents' folder must not hold one another" >&2; exit 2
    fi
fi
# There is no default model endpoint or model (D-100): the installer, a test or a person names both.
[ -n "${MODEL_BASE_URL:-}" ] || { echo "error: set MODEL_BASE_URL to an OpenAI-compatible model endpoint; there is no default" >&2; exit 2; }
[ -n "${MODEL_NAME:-}" ] || { echo "error: set MODEL_NAME to the model to use; there is no default" >&2; exit 2; }
# Goose takes the endpoint as its scheme and host, and the path of its chat completions.
host=$(printf '%s' "$MODEL_BASE_URL" | sed -E 's#^(https?://[^/]+).*#\1#')
path=${MODEL_BASE_URL#"$host"}
path=${path#/}
path=${path%/}
image=${GOOSE_IMAGE:-marmot-agents-goose}
mode=${GOOSE_MODE:-}
[ "${RECORD_ASK:-}" = 1 ] && mode=approve
case $mode in "" | auto | approve) ;; *) echo "error: GOOSE_MODE takes auto or approve" >&2; exit 2 ;; esac

mkdir -p "$home"
docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" "$here" >&2
# One container per Goose home. One left from a client that was killed, which Docker keeps running,
# is removed first, so two never share a home.
name="goose-acp-$(printf '%s' "$home" | sha256sum | cut -c1-12)"
docker rm -f "$name" >/dev/null 2>&1 || true
exec docker run -i --rm --name "$name" --log-driver=none \
    --user "$(id -u):$(id -g)" \
    -v "$work:$work" -w "$work" \
    -v "$home:/goose-home" -e HOME=/goose-home \
    -v "$key:/run/secrets/model_key:ro" \
    -e MODEL_HOST="$host" \
    -e MODEL_BASE_PATH="${path:+$path/}chat/completions" \
    -e MODEL_NAME="$MODEL_NAME" \
    ${mode:+-e GOOSE_MODE=$mode} \
    "$image"
