#!/bin/sh
# Record one Hermes scenario in a throwaway container.
#
#   tools/record/hermes/record.sh <scenario.json> <output.jsonl> <model-key-file> [model-name]
#
# The container gets a fresh, anonymous data volume and is removed afterwards, so every
# recording starts from zero. The model key is mounted read-only and is not kept anywhere.
# Set RECORD_LOG to a file to keep the gateway's own log, for finding out why a run failed.
set -eu
[ $# -ge 3 ] || { sed -n '2,9p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
scenario=$(realpath "$1"); out=$(realpath -m "$2"); key=$(realpath "$3")
model=${4:-${MODEL_NAME:-z-ai/glm-5.3}}
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
    -e MODEL_BASE_URL="${MODEL_BASE_URL:-https://api.numes.ai/api/v1}" \
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
