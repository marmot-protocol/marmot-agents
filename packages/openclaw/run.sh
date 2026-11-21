#!/usr/bin/env bash
# Run OpenClaw with the marmot plugin, in a container, for the agent whose home is given.
#
#   packages/openclaw/run.sh <agent home> <openclaw state folder> <work folder> <model-key-file> [model]
#   packages/openclaw/run.sh --name <openclaw state folder>    the container's name, to stop it
#
# The script waits, for two minutes at most, until the core serves its socket in the agent's home. The
# container gets the socket folder, the token (read-only), OpenClaw's state folder and the work folder,
# and runs as the calling user, so the socket's folder, which only that user may open, can be used. The
# work folder is mounted read-write, so it must not hold the agent's home, the state folder or the key
# file, nor be inside the home or the state folder: the script refuses to start if it does, links
# resolved. The model key is mounted read-only and is not kept anywhere. The container is removed when
# it stops. It is named after the state folder, so no two run on one state folder at once, and one that
# was killed leaves nothing in the way of the next. Set MODEL_BASE_URL for another endpoint, and
# MODEL_FALLBACKS for the models OpenClaw tries when the first fails (comma-separated; empty for none).
# Nothing the chat says is written to disk or to a log (D-071): Docker keeps no log of the container, and
# of the gateway's output only the plugin's own lines, "marmot-agent-openclaw: " and the kind of what
# happened, after the gateway's timestamp, reach this script's standard error. To see every call OpenClaw makes, record a
# scripted scenario with tools/record/openclaw.
set -eu
name_of() { printf 'marmot-agent-openclaw-%s' "$(printf %s "$(realpath "$1")" | sha256sum | cut -c1-16)"; }
if [ "${1:-}" = "--name" ] && [ $# -eq 2 ]; then name_of "$2"; echo; exit 0; fi
[ $# -ge 4 ] || { sed -n '2,21p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
home=$(realpath "$1"); state=$(realpath "$2"); work=$(realpath "$3"); key=$(realpath "$4")
model=${5:-${MODEL_NAME:-z-ai/glm-5.3-flash:free}}
image=${OPENCLAW_IMAGE:-marmot-agent-openclaw}
name=$(name_of "$state")
inside() { case "$1/" in "${2%/}"/*) return 0 ;; esac; return 1; }
for folder in "$home" "$state" "$key"; do
    if inside "$folder" "$work"; then
        echo "error: the work folder must not hold the agent's home, OpenClaw's state or the model key" >&2; exit 2
    fi
done
if inside "$work" "$home" || inside "$work" "$state"; then
    echo "error: the work folder must not be inside the agent's home or OpenClaw's state" >&2; exit 2
fi
# A connector started with the core waits for it. The socket and the token are mounted with --mount,
# which fails on a missing source where -v would make an empty folder in its place.
case $home in *,*) echo "error: the agent's home must not have a comma in its path" >&2; exit 2 ;; esac
tries=0
until [ -S "$home/socket/marmot-agent.sock" ] && [ -s "$home/connector.token" ]; do
    [ $tries -lt 240 ] || { echo "error: no core socket in $home/socket" >&2; exit 2; }
    sleep 0.5; tries=$((tries + 1))
done

docker image inspect "$image" >/dev/null 2>&1 || docker build -q -f "$here/container/Dockerfile" -t "$image" "$here" >/dev/null
# A container that was killed is removed by Docker on its own; wait for that, for a few seconds. One that
# still runs keeps its name, and `docker run` refuses to start a second.
tries=0
while [ "$(docker container inspect -f '{{.State.Running}}' "$name" 2>/dev/null || echo gone)" = false ] && [ $tries -lt 40 ]; do
    sleep 0.25; tries=$((tries + 1))
done
run=(docker run --rm -i --log-driver=none \
    --name "$name" \
    --user "$(id -u):$(id -g)" \
    --mount "type=bind,source=$home/socket,target=/marmot/socket" \
    --mount "type=bind,source=$home/connector.token,target=/marmot/connector.token,readonly" \
    -v "$state:/state" \
    -v "$work:/work" \
    -v "$key:/run/secrets/model_key:ro" \
    -e MODEL_BASE_URL="${MODEL_BASE_URL:-https://api.numes.ai/api/v1}" \
    -e MODEL_NAME="$model" \
    -e MARMOT_STATE_EXCLUSIVE=1 \
    -e MODEL_FALLBACKS="${MODEL_FALLBACKS-deepseek/deepseek-v4.1-flash:free}" \
    ${EXEC_MODE:+-e EXEC_MODE="$EXEC_MODE"} \
    "$image")
# The gateway puts a timestamp before each of the plugin's lines.
plugin_line='^([0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+([+-][0-9:]+|Z) )?marmot-agent-openclaw: [a-z0-9 (),._-]+$'
"${run[@]}" 2>&1 | { grep -E --line-buffered "$plugin_line" >&2 || true; }
exit "${PIPESTATUS[0]}"
