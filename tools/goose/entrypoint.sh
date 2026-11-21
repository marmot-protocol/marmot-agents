#!/bin/sh
# Start `goose acp` on standard input and output, configured for an OpenAI-compatible model.
#
# Needs:
#   /run/secrets/model_key   the model provider's API key (a mounted file, never an image layer)
#   MODEL_HOST               the endpoint's scheme and host, for example https://api.numes.ai
#   MODEL_BASE_PATH          the path of its chat completions, for example api/v1/chat/completions
#   MODEL_NAME               the model to use
#   HOME                     a folder Goose may keep its configuration and sessions in
# Optional:
#   GOOSE_MODE               auto (the default here): run tools without asking; approve: ask before each
set -eu

: "${MODEL_HOST:?set MODEL_HOST}"
: "${MODEL_BASE_PATH:?set MODEL_BASE_PATH}"
: "${MODEL_NAME:?set MODEL_NAME}"
key_file=/run/secrets/model_key
[ -r "$key_file" ] || { echo "error: mount the model key at $key_file" >&2; exit 2; }

umask 077
config="$HOME/.config/goose/config.yaml"
mkdir -p "$(dirname "$config")"
# The developer extension gives Goose its shell and its file editor. Nothing else is switched on.
cat > "$config" <<EOF
GOOSE_PROVIDER: openai
GOOSE_MODEL: "$MODEL_NAME"
GOOSE_MODE: "${GOOSE_MODE:-auto}"
GOOSE_TELEMETRY_ENABLED: false
extensions:
  developer:
    enabled: true
    type: builtin
    name: developer
    display_name: Developer
    timeout: 300
    bundled: true
EOF

# The key reaches Goose through its own environment only, never a command line or a file of its own.
OPENAI_API_KEY=$(cat "$key_file")
export OPENAI_API_KEY
export OPENAI_HOST="$MODEL_HOST" OPENAI_BASE_PATH="$MODEL_BASE_PATH"
export GOOSE_DISABLE_KEYRING=1
exec goose acp
