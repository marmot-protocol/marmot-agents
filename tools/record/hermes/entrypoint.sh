#!/bin/sh
# Configure a fresh Hermes home for one recording, then run the gateway until the scenario ends.
#
# Needs:
#   /run/secrets/model_key   the model provider's API key (a mounted file, never an image layer)
#   MODEL_BASE_URL           an OpenAI-compatible endpoint
#   MODEL_NAME               the model to use
set -eu

: "${MODEL_BASE_URL:?set MODEL_BASE_URL}"
: "${MODEL_NAME:?set MODEL_NAME}"
key_file=/run/secrets/model_key
[ -r "$key_file" ] || { echo "error: mount the model key at $key_file" >&2; exit 2; }

mkdir -p "$HERMES_HOME/plugins"
rm -rf "$HERMES_HOME/plugins/recorder"
cp -r /opt/recorder-plugin "$HERMES_HOME/plugins/recorder"

umask 077
# A fresh config for this recording. JSON strings are valid YAML, which avoids needing a YAML library.
python - <<PY
import json, os, pathlib
key = pathlib.Path("$key_file").read_text().strip()
q = json.dumps
config = f"""model:
  default: {q(os.environ["MODEL_NAME"])}
  provider: custom
  base_url: {q(os.environ["MODEL_BASE_URL"])}
  api_key: {q(key)}
platforms:
  recorder:
    enabled: true
"""
pathlib.Path(os.environ["HERMES_HOME"], "config.yaml").write_text(config)
PY

hermes plugins enable recorder || true
exec hermes gateway run
