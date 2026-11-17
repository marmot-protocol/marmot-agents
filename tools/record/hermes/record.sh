#!/bin/sh
# Record one Hermes scenario in a throwaway container.
#
#   tools/record/hermes/record.sh <scenario.json> <output.jsonl> <model-key-file> [model-name]
#
# The container gets a fresh, anonymous data volume and is removed afterwards, so every
# recording starts from zero. The model key is mounted read-only and is not kept anywhere.
set -eu
[ $# -ge 3 ] || { sed -n '2,8p' "$0"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
scenario=$(realpath "$1"); out=$(realpath -m "$2"); key=$(realpath "$3")
model=${4:-${MODEL_NAME:-z-ai/glm-5.3}}
image=${RECORD_IMAGE:-marmot-agents-record-hermes}
outdir=$(mktemp -d)
chmod 0777 "$outdir"

docker image inspect "$image" >/dev/null 2>&1 || docker build -f "$here/Dockerfile" -t "$image" "$here"
docker run --rm \
    --name "record-hermes-$$" \
    -v "$scenario:/scenario.json:ro" \
    -v "$key:/run/secrets/model_key:ro" \
    -v "$outdir:/out" \
    -e MODEL_BASE_URL="${MODEL_BASE_URL:-https://api.numes.ai/api/v1}" \
    -e MODEL_NAME="$model" \
    -e RECORDER_TURN_TIMEOUT="${RECORDER_TURN_TIMEOUT:-900}" \
    "$image" || echo "container exited with status $?" >&2
mkdir -p "$(dirname "$out")"
cp "$outdir/recording.jsonl" "$out" 2>/dev/null && echo "wrote $out" || echo "no recording was produced" >&2
rm -rf "$outdir"
