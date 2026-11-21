#!/usr/bin/env bash
# Run Hermes with the Marmot plugin in a container, connected to a running core.
#
#   python/hermes/run.sh <agent-home> <work-folder> <hermes-volume> <model-key-file> [model-name]
#
# <agent-home> holds the core's socket folder and the plugin's token (`socket/`, `connector.token`).
# Only those two are mounted, never the rest of the agent's home. <work-folder> is Hermes's working
# folder. The plugin is mounted from this checkout, so a change to it needs no new image.
# <hermes-volume> is a Docker volume for Hermes's own home, so that its sessions survive a
# restart; create it with `docker volume create`, and remove it when done. The model key is mounted
# read-only and kept nowhere else but in Hermes's configuration in that volume.
#
# Set HERMES_CONTAINER to name the container (to stop it: `docker rm -f <name>`), and HERMES_LOG to
# a file to keep the plugin's own log lines. Hermes's own output is never kept, by Docker either.
set -euo pipefail
[ $# -ge 4 ] || { sed -n '2,14p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
home=$(realpath "$1"); work=$(realpath "$2"); volume=$3; key=$(realpath "$4")
model=${5:-${MODEL_NAME:-z-ai/glm-5.3-flash:free}}
image=${HERMES_IMAGE:-marmot-agents-hermes}
name=${HERMES_CONTAINER:-marmot-hermes-$$}
[ -S "$home/socket/marmot-agent.sock" ] || { echo "error: no core socket in $home/socket" >&2; exit 2; }

docker image inspect "$image" >/dev/null 2>&1 \
    || docker build --build-arg UID="$(id -u)" --build-arg GID="$(id -g)" \
        -f "$here/Dockerfile" -t "$image" "$here/.." >&2
# Hermes's own output holds what people and the model say, so none of it is kept: Docker logs
# nothing (`--log-driver=none`, or its default would keep it all under its own folder), and what
# reaches this script goes nowhere. The plugin's log lines name what went wrong by kind only (D-071):
# with HERMES_LOG the plugin appends them to that file, mounted for it alone, and nothing from
# Hermes's output is copied there.
run=(docker run --rm --log-driver=none --name "$name"
    -v "$home/socket:/run/marmot-agent/socket"
    -v "$home/connector.token:/run/marmot-agent/connector.token:ro"
    -v "$work:/work"
    -v "$here/../marmot_hermes:/opt/marmot-plugin:ro"
    -v "$volume:/data"
    -v "$key:/run/secrets/model_key:ro"
    -e MODEL_BASE_URL="${MODEL_BASE_URL:-https://api.numes.ai/api/v1}"
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
