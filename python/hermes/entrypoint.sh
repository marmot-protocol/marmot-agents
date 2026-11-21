#!/bin/sh
# Starts Hermes with the Marmot plugin. On the first start of a Hermes home it writes Hermes's
# configuration, replacing the one the image's build left; later starts keep it, so that what a chat
# changed survives a restart.
#
# Needs:
#   /run/secrets/model_key           the model provider's API key (a mounted file, never an image layer)
#   /run/marmot-agent/socket         the core's socket folder, and /run/marmot-agent/connector.token
#   MODEL_BASE_URL, MODEL_NAME       an OpenAI-compatible endpoint, and the model to use
#   REASONING_EFFORT                 optional; the reasoning effort Hermes asks for (default: high)
#   HERMES_CONFIG_PATCH              optional; a JSON object merged into the configuration last
set -eu

: "${MODEL_BASE_URL:?set MODEL_BASE_URL}"
: "${MODEL_NAME:?set MODEL_NAME}"
key_file=/run/secrets/model_key
[ -r "$key_file" ] || { echo "error: mount the model key at $key_file" >&2; exit 2; }

mkdir -p "$HERMES_HOME/plugins"
rm -rf "$HERMES_HOME/plugins/marmot"
cp -r /opt/marmot-plugin "$HERMES_HOME/plugins/marmot"

umask 077
configured="$HERMES_HOME/marmot/configured"
if [ ! -f "$configured" ]; then
# JSON is valid YAML, which avoids needing a YAML library.
python - <<PY
import json, os, pathlib
key = pathlib.Path("$key_file").read_text().strip()
config = {
    "model": {"default": os.environ["MODEL_NAME"], "provider": "custom",
              "base_url": os.environ["MODEL_BASE_URL"], "api_key": key},
    "agent": {"reasoning_effort": os.environ.get("REASONING_EFFORT", "high")},
    "platforms": {"marmot": {"enabled": True}},
    # The core decides who may prompt and who may approve (D-010), so Hermes takes whatever reaches
    # it (provisional, Q-22): every user is allowed (MARMOT_ALLOW_ALL_USERS), anyone in the chat may
    # answer an approval, and a person who may start a new session is not asked to confirm it.
    # Hermes's own approval mode is "manual": a command it flags is always put to a person, never
    # decided by a second model.
    "approvals": {"mode": "manual", "approvers": ["*"], "destructive_slash_confirm": False},
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
mkdir -p "$(dirname "$configured")" && touch "$configured"
fi

hermes plugins enable marmot || true
exec hermes gateway run
