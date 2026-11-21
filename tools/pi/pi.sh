#!/bin/sh
# Run Pi in a throwaway container, on standard input and output, in the current folder.
#
#   tools/pi/pi.sh <model-key-file> <state-folder> [pi arguments...]
#
# For example, as a driver or a recorder starts it:
#   tools/pi/pi.sh <key> <state> --mode rpc
#
# - The current folder is mounted at the same path, and Pi works there, so it must not hold the
#   state folder or the key file, nor be inside the state folder: the script refuses to start if it
#   does, symbolic links resolved, since that mount would hand Pi every chat's sessions and the key.
#   Nor may it hold or be inside MARMOT_AGENT_DATA, the folder of every agent, when that is set.
# - The state folder holds what Pi keeps between runs: its settings and its sessions. Give the
#   same one each time for sessions to be resumed. It is created if missing. When MARMOT_AGENT_CHAT
#   names a chat (the driver sets it, as lowercase hex), only that chat's own folder inside it is
#   mounted, so one chat's Pi cannot read another chat's sessions. When the last argument is
#   `--version`, as when the driver asks Pi its version, an empty folder is mounted in its place.
# - The model key is mounted read-only. It is never built into the image, never put in an
#   environment variable, and never written anywhere.
# - Docker keeps no log of the container: what Pi writes holds prompts, replies and tool output.
# - The container runs as the calling user, so files Pi writes belong to them, and is removed when
#   Pi exits. Closing standard input stops Pi; so does SIGTERM, HUP or INT to this script. A chat's
#   container is named after its state folder, and one left running by a script that was killed
#   outright is removed when that chat's Pi next starts.
#
# MODEL_BASE_URL and MODEL_NAME choose the endpoint and the model; PI_IMAGE the image.
set -eu
[ $# -ge 2 ] || { sed -n '2,26p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
key=$(realpath "$1"); state=$(realpath -m "$2"); shift 2
image=${PI_IMAGE:-marmot-agents-pi}
work=$(pwd -P)
inside() { case "$1/" in "${2%/}"/*) return 0 ;; esac; return 1; }
if inside "$state" "$work" || inside "$work" "$state"; then
    echo "error: the state folder and the working folder must not hold one another" >&2; exit 2
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
run="$$-$(date +%s%N)"
name="pi-$run"
if [ -n "${MARMOT_AGENT_CHAT:-}" ]; then
    case $MARMOT_AGENT_CHAT in
        *[!0-9a-f]*) echo "error: MARMOT_AGENT_CHAT must be lowercase hex" >&2; exit 2 ;;
    esac
    state="$state/chats/$MARMOT_AGENT_CHAT"
    name="pi-chat-$(printf '%s' "$state" | sha256sum | cut -c1-32)"
fi
# This run's own files: the container's id once Docker has created it, and for `--version`, which
# needs no state, an empty folder to mount instead.
own=$(mktemp -d)
cid="$own/cid"
last=""
for last in "$@"; do :; done
if [ "$last" = "--version" ]; then
    state="$own/state"
fi
# A container outlives the `docker run` that started it if that is killed, so remove it when this
# script ends, however it ends short of SIGKILL. It is found by a label of this run's own, not by
# name: the chat's next Pi may already have taken the name. A container Docker is still creating
# is not listed yet, so if `docker run` has started and not exited, wait (a few seconds at most)
# until it has written the container's id. `docker run` runs in the background and the script
# waits for it, because a shell runs a trap only once its foreground command has returned.
pid=""
cleanup() {
    if [ -n "$pid" ]; then
        tries=0
        while [ ! -s "$cid" ] && [ "$tries" -lt 80 ] && kill -0 "$pid" 2>/dev/null; do
            sleep 0.1; tries=$((tries + 1))
        done
    fi
    ids=$(docker ps -aq --filter "label=marmot-agents.pi.run=$run" 2>/dev/null) || true
    [ -z "$ids" ] || docker rm -f $ids >/dev/null 2>&1 || true
    rm -rf "$own"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

docker image inspect "$image" >/dev/null 2>&1 || docker build -q -t "$image" "$here" >&2
mkdir -p "$state"
# A chat's container left over from a run whose script was killed outright, as SIGKILL does.
docker rm -f "$name" >/dev/null 2>&1 || true
# A command run in the background reads /dev/null unless told otherwise: hand it this script's
# standard input explicitly.
exec 3<&0
docker run --rm -i --name "$name" --label "marmot-agents.pi.run=$run" --cidfile "$cid" \
    --log-driver=none \
    --user "$(id -u):$(id -g)" \
    -v "$key:/run/secrets/model_key:ro" \
    -v "$state:/state" \
    -v "$work:$work" -w "$work" \
    -e MODEL_BASE_URL="${MODEL_BASE_URL:-https://api.numes.ai/api/v1}" \
    -e MODEL_NAME="${MODEL_NAME:-deepseek/deepseek-v4.1-flash:free}" \
    -e THINKING="${THINKING:-low}" \
    "$image" pi "$@" <&3 3<&- &
pid=$!
exec 3<&-
status=0
wait "$pid" || status=$?
exit "$status"
