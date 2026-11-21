#!/bin/sh
# Point Pi at one OpenAI-compatible endpoint, then run the command given (normally `pi ...`).
#
# Needs:
#   /run/secrets/model_key   the endpoint's API key (a mounted file, never an image layer)
#   MODEL_BASE_URL           the endpoint
#   MODEL_NAME               the model to use
#
# Pi reads the key from the file each time it needs it (`!cat ...` in models.json), so the key is
# in no file Pi writes and in no environment variable.
set -eu

: "${MODEL_BASE_URL:?set MODEL_BASE_URL}"
: "${MODEL_NAME:?set MODEL_NAME}"
key_file=/run/secrets/model_key
[ -r "$key_file" ] || { echo "error: mount the model key at $key_file" >&2; exit 2; }

umask 077
mkdir -p "$PI_CODING_AGENT_DIR" "$HOME"

node - <<'JS'
const fs = require("fs");
const dir = process.env.PI_CODING_AGENT_DIR;
const env = process.env;
const models = {
  providers: {
    numes: {
      name: "Numes",
      baseUrl: env.MODEL_BASE_URL,
      api: "openai-completions",
      apiKey: "!cat /run/secrets/model_key",
      models: [{
        id: env.MODEL_NAME,
        name: env.MODEL_NAME,
        reasoning: true,
        input: ["text"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: 128000,
        maxTokens: 16384,
      }],
    },
  },
};
fs.writeFileSync(`${dir}/models.json`, JSON.stringify(models, null, 2) + "\n");
const file = `${dir}/settings.json`;
let settings = {};
try { settings = JSON.parse(fs.readFileSync(file, "utf8")); } catch {}
Object.assign(settings, {
  defaultProvider: "numes",
  defaultModel: env.MODEL_NAME,
  defaultThinkingLevel: env.THINKING ?? "low",
  enableInstallTelemetry: false,
});
fs.writeFileSync(file, JSON.stringify(settings, null, 2) + "\n");
JS

exec "$@"
