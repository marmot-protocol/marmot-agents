#!/bin/sh
# Configure a fresh OpenClaw home for one recording, then run the gateway until the scenario ends.
#
# Needs:
#   /run/secrets/model_key   the model provider's API key (a mounted file, never an image layer)
#   MODEL_BASE_URL           an OpenAI-compatible endpoint
#   MODEL_NAME               the model to use
# Optional:
#   THINKING                 the thinking level OpenClaw asks the model for (default: low)
#   VERBOSE                  off, on or full: how much tool activity OpenClaw sends to a chat (default: full)
#   REASONING                off, on or stream: whether OpenClaw sends the model's reasoning to a chat (default: on)
#   BLOCK_STREAMING          on or off: whether finished blocks of a reply are sent as they complete (default: on)
#   EXEC_MODE                OpenClaw's tools.exec.mode, for example "ask" to ask before commands that are
#                            not on its allow-list (default: left as OpenClaw sets it)
#   RECORDER_DM_POLICY, RECORDER_GROUP_POLICY, RECORDER_ALLOW_FROM, RECORDER_GROUP_ALLOW_FROM,
#   RECORDER_REQUIRE_MENTION the channel's own access settings; the lists are comma-separated
#   OPENCLAW_CONFIG_PATCH    a JSON object merged into the configuration last
#   RECORDER_DEBUG_SHELL     set to 1 to set everything up and then idle, for looking around with
#                            docker exec; the gateway is not started
set -eu

: "${MODEL_BASE_URL:?set MODEL_BASE_URL}"
: "${MODEL_NAME:?set MODEL_NAME}"
key_file=/run/secrets/model_key
[ -r "$key_file" ] || { echo "error: mount the model key at $key_file" >&2; exit 2; }

umask 077
mkdir -p "$OPENCLAW_HOME"
config="$OPENCLAW_HOME/.openclaw/openclaw.json"

# OpenClaw refuses to start a gateway that has not been set up. Set it up once, with no model.
openclaw onboard --non-interactive --accept-risk --mode local --flow quickstart \
    --suppress-gateway-token-output --no-install-daemon --skip-channels --skip-skills --skip-ui \
    --skip-search --skip-hooks --skip-health --skip-bootstrap --auth-choice skip >/dev/null

# Install the plugin the way a release is installed: from a packed archive.
( cd /tmp && rm -rf recorder-pack && cp -r /opt/recorder-plugin recorder-pack && cd recorder-pack && npm pack --silent >/dev/null )
openclaw plugins install --force --accept-capabilities /tmp/recorder-pack/openclaw-recorder-0.0.1.tgz >/dev/null
openclaw plugins enable recorder >/dev/null

# The key stays in this process's environment. The configuration file only names the variable.
MODEL_API_KEY=$(cat "$key_file")
export MODEL_API_KEY

node - "$config" <<'JS'
const fs = require("fs");
const file = process.argv[2];
const cfg = JSON.parse(fs.readFileSync(file, "utf8"));
const env = process.env;
const list = (v) => (v ?? "").split(",").map((s) => s.trim()).filter(Boolean);

cfg.models = cfg.models ?? {};
cfg.models.providers = {
  ...(cfg.models.providers ?? {}),
  recorded: {
    baseUrl: env.MODEL_BASE_URL,
    apiKey: "${MODEL_API_KEY}",
    api: "openai-completions",
    timeoutSeconds: 600,
    models: [{
      id: env.MODEL_NAME, name: env.MODEL_NAME, reasoning: true, input: ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000, maxTokens: 8192,
    }],
  },
};
cfg.agents = cfg.agents ?? {};
cfg.agents.defaults = {
  ...(cfg.agents.defaults ?? {}),
  model: { primary: `recorded/${env.MODEL_NAME}` },
  thinkingDefault: env.THINKING ?? "low",
  verboseDefault: env.VERBOSE ?? "full",
  reasoningDefault: env.REASONING ?? "on",
  blockStreamingDefault: env.BLOCK_STREAMING ?? "on",
  timeoutSeconds: 1800,
};
cfg.channels = cfg.channels ?? {};
cfg.channels.recorder = {
  enabled: true,
  dmPolicy: env.RECORDER_DM_POLICY ?? "open",
  groupPolicy: env.RECORDER_GROUP_POLICY ?? "open",
  allowFrom: list(env.RECORDER_ALLOW_FROM ?? "*"),
  groupAllowFrom: list(env.RECORDER_GROUP_ALLOW_FROM ?? "*"),
  requireMention: (env.RECORDER_REQUIRE_MENTION ?? "false") === "true",
};
if (env.EXEC_MODE) {
  cfg.tools = { ...(cfg.tools ?? {}), exec: { ...(cfg.tools?.exec ?? {}), mode: env.EXEC_MODE } };
}
const merge = (a, b) => {
  for (const [k, v] of Object.entries(b)) {
    if (v && typeof v === "object" && !Array.isArray(v) && a[k] && typeof a[k] === "object") merge(a[k], v);
    else a[k] = v;
  }
  return a;
};
if (env.OPENCLAW_CONFIG_PATCH) merge(cfg, JSON.parse(env.OPENCLAW_CONFIG_PATCH));
fs.writeFileSync(file, JSON.stringify(cfg, null, 2) + "\n");
JS

if [ "${RECORDER_DEBUG_SHELL:-0}" = "1" ]; then exec sleep infinity; fi
exec openclaw gateway run
