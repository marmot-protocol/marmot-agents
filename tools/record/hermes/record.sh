#!/bin/sh
# Record one Hermes scenario in a throwaway container.
#
#   tools/record/hermes/record.sh <scenario.json> <output.jsonl> [model-key-file] [model-name]
#
# The model is MODEL_BASE_URL, MODEL_NAME (or [model-name]) and MODEL_KEY_FILE (or [model-key-file]),
# from the environment or from tests.env at the top of the repository (tests.env.example says how);
# there is no default.
#
# The container gets a fresh, anonymous data volume and is removed afterwards, so every
# recording starts from zero. The model key is mounted read-only and is not kept anywhere.
# Set RECORD_LOG to a file to keep the gateway's own log, for finding out why a run failed.
set -eu
[ $# -ge 2 ] || { sed -n '2,12p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
TESTS_ENV="$here/../../../tests.env"
# shellcheck source=../tests-env.sh
. "$here/../tests-env.sh"
scenario=$(realpath "$1"); out=$(realpath -m "$2")
key=${3:-$(tests_env MODEL_KEY_FILE)}
key=$(realpath "$key")
model=${4:-$(tests_env MODEL_NAME)}
url=$(tests_env MODEL_BASE_URL)
image=${RECORD_IMAGE:-marmot-agents-record-hermes}
outdir=$(mktemp -d)
chmod 0777 "$outdir"

docker image inspect "$image" >/dev/null 2>&1 || docker build -f "$here/Dockerfile" -t "$image" "$here"
# A scenario may set container environment variables, for example to restrict who is allowed.
# They go through a file, so that a value may hold spaces, as a JSON object does.
envfile="$outdir/env"
python3 -c 'import json,sys; [print(f"{k}={v}") for k, v in (json.load(open(sys.argv[1])).get("env") or {}).items()]' "$scenario" > "$envfile"

docker run --rm \
    --name "record-hermes-$(basename "$scenario" .json)-$$" \
    --env-file "$envfile" \
    -v "$scenario:/scenario.json:ro" \
    -v "$key:/run/secrets/model_key:ro" \
    -v "$outdir:/out" \
    -e MODEL_BASE_URL="$url" \
    -e MODEL_NAME="$model" \
    -e REASONING_EFFORT="${REASONING_EFFORT:-high}" \
    -e RECORDER_TURN_TIMEOUT="${RECORDER_TURN_TIMEOUT:-900}" \
    "$image" > "$outdir/gateway.log" 2>&1 || echo "container exited with status $?" >&2
if [ -n "${RECORD_LOG:-}" ]; then
    # Never keep a line that holds the key, whatever put it there.
    grep -v -F -f "$key" "$outdir/gateway.log" > "$RECORD_LOG" || true
fi
mkdir -p "$(dirname "$out")"
cp "$outdir/recording.jsonl" "$out" 2>/dev/null && echo "wrote $out" || echo "no recording was produced" >&2
rm -rf "$outdir"
