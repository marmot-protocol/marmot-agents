#!/bin/sh
# Run OpenClaw with the marmot plugin, in a container, for the agent whose home is given.
#
#   packages/openclaw/run.sh <agent home> <openclaw state folder> <work folder> <model-key-file> [model]
#   packages/openclaw/run.sh --name <openclaw state folder>    the container's name, to stop it
#
# The core must already serve its socket in the agent's home. The container gets the socket folder, the
# token (read-only), OpenClaw's state folder and the work folder, and runs as the calling user, so the
# socket's folder, which only that user may open, can be used. The model key is mounted read-only and is
# not kept anywhere. The container is removed when it stops. It is named after the state folder, so no
# two run on one state folder at once, and one that was killed leaves nothing in the way of the next.
# Set MODEL_BASE_URL for another endpoint, and MODEL_FALLBACKS for the models OpenClaw tries when the
# first fails (comma-separated; empty for none). Nothing the chat says is written to disk (D-071): to see
# every call OpenClaw makes, record a scripted scenario with tools/record/openclaw.
set -eu
name_of() { printf 'marmot-agent-openclaw-%s' "$(printf %s "$(realpath "$1")" | sha256sum | cut -c1-16)"; }
if [ "${1:-}" = "--name" ] && [ $# -eq 2 ]; then name_of "$2"; echo; exit 0; fi
[ $# -ge 4 ] || { sed -n '2,16p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
home=$(realpath "$1"); state=$(realpath "$2"); work=$(realpath "$3"); key=$(realpath "$4")
model=${5:-${MODEL_NAME:-z-ai/glm-5.3-flash:free}}
image=${OPENCLAW_IMAGE:-marmot-agent-openclaw}
name=$(name_of "$state")

docker image inspect "$image" >/dev/null 2>&1 || docker build -q -f "$here/container/Dockerfile" -t "$image" "$here" >/dev/null
# A container that was killed is removed by Docker on its own; wait for that, for a few seconds. One that
# still runs keeps its name, and `docker run` refuses to start a second.
tries=0
while [ "$(docker container inspect -f '{{.State.Running}}' "$name" 2>/dev/null || echo gone)" = false ] && [ $tries -lt 40 ]; do
    sleep 0.25; tries=$((tries + 1))
done
exec docker run --rm -i \
    --name "$name" \
    --user "$(id -u):$(id -g)" \
    -v "$home/socket:/marmot/socket" \
    -v "$home/connector.token:/marmot/connector.token:ro" \
    -v "$state:/state" \
    -v "$work:/work" \
    -v "$key:/run/secrets/model_key:ro" \
    -e MODEL_BASE_URL="${MODEL_BASE_URL:-https://api.numes.ai/api/v1}" \
    -e MODEL_NAME="$model" \
    -e MARMOT_STATE_EXCLUSIVE=1 \
    -e MODEL_FALLBACKS="${MODEL_FALLBACKS-deepseek/deepseek-v4.1-flash:free}" \
    ${EXEC_MODE:+-e EXEC_MODE="$EXEC_MODE"} \
    "$image"
