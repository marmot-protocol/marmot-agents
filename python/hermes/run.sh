#!/usr/bin/env bash
# Run Hermes with the Marmot plugin in a container, connected to a running core.
#
#   python/hermes/run.sh <agent-home> <work-folder> <hermes-volume> <model-key-file> [model-name]
#
# <agent-home> holds the core's socket folder and the plugin's token (`socket/`, `connector.token`).
# Only those two are mounted, never the rest of the agent's home; the script waits, for two minutes at
# most, until the core serves the socket. <work-folder> is Hermes's working folder, mounted read-write at
# its own path, where Hermes's commands run, so it must not hold the agent's home or the key file, nor be inside the home: the script refuses to
# start if it does, links resolved, and likewise if it holds or is inside MARMOT_AGENT_DATA, when that
# is set. The plugin is mounted from this checkout, so a change to it needs no new image.
# <hermes-volume> is a Docker volume for Hermes's own home, so that its sessions survive a restart;
# create it with `docker volume create`, and remove it when done. The model key is mounted read-only
# and read at each start; it is kept nowhere, the volume included.
#
# Set HERMES_CONTAINER to name the container (to stop it: `docker rm -f <name>`), and HERMES_LOG to
# a file to keep the plugin's own log lines. Hermes's own output is never kept, by Docker either.
set -euo pipefail
[ $# -ge 4 ] || { sed -n '2,17p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
home=$(realpath "$1"); work=$(realpath "$2"); volume=$3; key=$(realpath "$4")
model=${5:-${MODEL_NAME:-}}
# The image is built for the calling user and group, so it is named after them: another account's
# build of it would hand Hermes a user that cannot open this one's socket.
image=${HERMES_IMAGE:-marmot-agents-hermes-$(id -u)-$(id -g)}
name=${HERMES_CONTAINER:-marmot-hermes-$$}
inside() { case "$1/" in "${2%/}"/*) return 0 ;; esac; return 1; }
if inside "$home" "$work" || inside "$key" "$work" || inside "$work" "$home"; then
    echo "error: the work folder must not hold the agent's home or the model key, nor be inside the home" >&2; exit 2
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
[ -n "$model" ] || { echo "error: set MODEL_NAME, or give the model's name, to choose the model; there is no default" >&2; exit 2; }
# The socket and the token are mounted with --mount, which fails on a missing source where -v would
# make an empty folder in its place; a connector started with the core waits for it.
case $home in *,*) echo "error: the agent's home must not have a comma in its path" >&2; exit 2 ;; esac
# The work folder is mounted at its own path, so a file the agent names there is the one the core sends
# (D-090), and with --mount too.
case $work in *,*) echo "error: the work folder must not have a comma in its path" >&2; exit 2 ;; esac
tries=0
until [ -S "$home/socket/marmot-agent.sock" ] && [ -s "$home/connector.token" ]; do
    [ $tries -lt 240 ] || { echo "error: no core socket in $home/socket" >&2; exit 2; }
    sleep 0.5; tries=$((tries + 1))
done

docker image inspect "$image" >/dev/null 2>&1 \
    || docker build --build-arg UID="$(id -u)" --build-arg GID="$(id -g)" \
        -f "$here/Dockerfile" -t "$image" "$here/.." >&2
# Hermes's own output holds what people and the model say, so none of it is kept: Docker logs
# nothing (`--log-driver=none`, or its default would keep it all under its own folder), and what
# reaches this script goes nowhere. The plugin's log lines name what went wrong by kind only (D-071):
# with HERMES_LOG the plugin appends them to that file, mounted for it alone, and nothing from
# Hermes's output is copied there.
run=(docker run --rm --log-driver=none --name "$name"
    --mount "type=bind,source=$home/socket,target=/run/marmot-agent/socket"
    --mount "type=bind,source=$home/connector.token,target=/run/marmot-agent/connector.token,readonly"
    --mount "type=bind,source=$work,target=$work"
    -w "$work"
    -e MARMOT_WORK="$work"
    -v "$here/../marmot_hermes:/opt/marmot-plugin:ro"
    -v "$volume:/data"
    -v "$key:/run/secrets/model_key:ro"
    -e MODEL_BASE_URL="$MODEL_BASE_URL"
    -e MODEL_NAME="$model"
    -e REASONING_EFFORT="${REASONING_EFFORT:-high}")
[ -z "${HERMES_CONFIG_PATCH:-}" ] || run+=(-e HERMES_CONFIG_PATCH="$HERMES_CONFIG_PATCH")
if [ -n "${HERMES_LOG:-}" ]; then
    touch "$HERMES_LOG"
    run+=(-v "$(realpath "$HERMES_LOG"):/run/marmot-log/plugin.log" -e MARMOT_LOG=/run/marmot-log/plugin.log)
fi
run+=("$image")
status=0
"${run[@]}" > /dev/null 2>&1 || status=$?
exit "$status"
