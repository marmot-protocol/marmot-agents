#!/bin/sh
# Configure a fresh Hermes home for one recording, then run the gateway until the scenario ends.
#
# Needs:
#   /run/secrets/model_key   the model provider's API key (a mounted file, never an image layer)
#   MODEL_BASE_URL           an OpenAI-compatible endpoint
#   MODEL_NAME               the model to use
#   REASONING_EFFORT         optional; the reasoning effort Hermes asks for (default: high)
#   HERMES_CONFIG_PATCH      optional; a JSON object merged into the configuration last, for example to
#                            name who may use every slash command
set -eu

: "${MODEL_BASE_URL:?set MODEL_BASE_URL}"
: "${MODEL_NAME:?set MODEL_NAME}"
key_file=/run/secrets/model_key
[ -r "$key_file" ] || { echo "error: mount the model key at $key_file" >&2; exit 2; }

mkdir -p "$HERMES_HOME/plugins"
rm -rf "$HERMES_HOME/plugins/recorder"
cp -r /opt/recorder-plugin "$HERMES_HOME/plugins/recorder"

umask 077
# A fresh config for this recording. JSON is valid YAML, which avoids needing a YAML library.
python - <<PY
import json, os, pathlib
key = pathlib.Path("$key_file").read_text().strip()
config = {
    "model": {"default": os.environ["MODEL_NAME"], "provider": "custom",
              "base_url": os.environ["MODEL_BASE_URL"], "api_key": key},
    "agent": {"reasoning_effort": os.environ.get("REASONING_EFFORT", "high")},
    "platforms": {"recorder": {"enabled": True}},
}

def merge(a, b):
    for k, v in b.items():
        if isinstance(v, dict) and isinstance(a.get(k), dict):
            merge(a[k], v)
        else:
            a[k] = v
    return a

if os.environ.get("HERMES_CONFIG_PATCH"):
    merge(config, json.loads(os.environ["HERMES_CONFIG_PATCH"]))
pathlib.Path(os.environ["HERMES_HOME"], "config.yaml").write_text(json.dumps(config, indent=2) + "\n")
PY

hermes plugins enable recorder || true
exec hermes gateway run
