#!/bin/sh
# Starts Hermes with the Marmot plugin. configure.py writes Hermes's configuration on the first start
# of a Hermes home, and keeps it on later starts, so that what a chat changed survives a restart. The
# model key is read from its mounted file at every start and written nowhere.
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
# The key goes to Hermes in its environment only. The configuration names the variable (`key_env`),
# never the key, so no file in Hermes's home holds it, a new key takes effect at the next start, and
# Hermes keeps it from the commands it runs, as it does every variable a `key_env` names.
MARMOT_MODEL_KEY=$(cat "$key_file")
export MARMOT_MODEL_KEY
python /usr/local/lib/marmot/configure.py "$HERMES_HOME/marmot/configured"

hermes plugins enable marmot || true
exec hermes gateway run
