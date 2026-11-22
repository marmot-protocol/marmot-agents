#!/bin/sh
# Set up OpenClaw with the marmot channel, the first time, and run its gateway.
#
# Needs:
#   /run/secrets/model_key   the model provider's API key (a mounted file, never an image layer)
#   MODEL_BASE_URL           an OpenAI-compatible endpoint
#   MODEL_NAME               the model to use
#   MODEL_FALLBACKS          comma-separated models of the same endpoint, tried in turn when it fails
#   /marmot                  the agent's home: socket/marmot-agent.sock and connector.token
#   /state                   OpenClaw's state, kept from one run to the next
#   MARMOT_WORK              the folder the agent works in, mounted at the path the core knows it by
#                            (default: /work)
# Optional:
#   EXEC_MODE                OpenClaw's tools.exec.mode (default: ask, which asks in the chat before any
#                            command not on OpenClaw's allow-list)
#   THINKING                 the thinking level OpenClaw asks the model for (default: low)
#   OPENCLAW_CONFIG_PATCH    a JSON object merged into the configuration last
#   MARMOT_STATE_EXCLUSIVE   set to 1 by run.sh, which names the container after /state's folder, so Docker
#                            lets no second container use it: OpenClaw's lock on /state, left by a container
#                            that was killed, is then recovered. OpenClaw cannot tell by itself that a gateway
#                            in another container has stopped, and waits for an operator otherwise
set -eu

: "${MODEL_BASE_URL:?set MODEL_BASE_URL}"
: "${MODEL_NAME:?set MODEL_NAME}"
key_file=/run/secrets/model_key
[ -r "$key_file" ] || { echo "error: mount the model key at $key_file" >&2; exit 2; }

umask 077
mkdir -p "$OPENCLAW_HOME"
config="$OPENCLAW_HOME/.openclaw/openclaw.json"

if [ ! -f "$config" ]; then
    # OpenClaw refuses to start a gateway that has not been set up. Set it up once, with no model.
    openclaw onboard --non-interactive --accept-risk --mode local --flow quickstart \
        --suppress-gateway-token-output --no-install-daemon --skip-channels --skip-skills --skip-ui \
        --skip-search --skip-hooks --skip-health --skip-bootstrap --auth-choice skip >/dev/null
fi

if [ "${MARMOT_STATE_EXCLUSIVE:-0}" = "1" ]; then
    confirmation=$(openclaw gateway recover-lock --json 2>/dev/null | node -e '
        let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
          try { const j = JSON.parse(s); if (j.present && j.confirmation) process.stdout.write(j.confirmation); } catch {}
        });') || true
    if [ -n "$confirmation" ]; then
        openclaw gateway recover-lock --confirm "$confirmation" >/dev/null
        echo "marmot-agent-openclaw: recovered the state lock of a gateway that had stopped" >&2
    fi
fi

# Install the plugin the way a release is installed: from a packed archive. Again on every start, so a
# rebuilt image brings its plugin.
openclaw plugins install --force --accept-capabilities /opt/marmot-plugin.tgz >/dev/null
openclaw plugins enable marmot >/dev/null

# The key stays in this process's environment. The configuration file only names the variable, and the
# plugin never lets its whole value into an event. The masked form `/model status` shows, which keeps a
# few of its characters, may reach the chat (D-088).
MODEL_API_KEY=$(cat "$key_file")
MARMOT_SECRET_ENV=MODEL_API_KEY
export MODEL_API_KEY MARMOT_SECRET_ENV

node - "$config" <<'JS'
const fs = require("fs");
const file = process.argv[2];
const cfg = JSON.parse(fs.readFileSync(file, "utf8"));
const env = process.env;

const fallbacks = (env.MODEL_FALLBACKS ?? "").split(",").map((s) => s.trim()).filter(Boolean);
const models = [env.MODEL_NAME, ...fallbacks.filter((m) => m !== env.MODEL_NAME)];
cfg.models = cfg.models ?? {};
cfg.models.providers = {
  ...(cfg.models.providers ?? {}),
  marmot: {
    baseUrl: env.MODEL_BASE_URL,
    apiKey: "${MODEL_API_KEY}",
    api: "openai-completions",
    timeoutSeconds: 600,
    models: models.map((id) => ({
      id, name: id, reasoning: true, input: ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000, maxTokens: 8192,
    })),
  },
};
cfg.agents = cfg.agents ?? {};
cfg.agents.defaults = {
  ...(cfg.agents.defaults ?? {}),
  model: { primary: `marmot/${env.MODEL_NAME}`, fallbacks: models.slice(1).map((m) => `marmot/${m}`) },
  workspace: env.MARMOT_WORK || "/work",
  thinkingDefault: env.THINKING ?? "low",
  // Everything the agent does reaches the plugin, which passes it on (D-008). The recordings were made
  // with these.
  verboseDefault: "full",
  reasoningDefault: "on",
  blockStreamingDefault: "on",
  timeoutSeconds: 1800,
};
// Onboarding names the workspace for the main agent too, and that comes first.
for (const entry of Object.values(cfg.agents.entries ?? {})) entry.workspace = env.MARMOT_WORK || "/work";
// Who may talk to the agent is the core's to decide (D-010): the channel lets in what the core sends.
cfg.channels = { ...(cfg.channels ?? {}), marmot: { enabled: true } };
// Approvals reach the chat: the plugin claims OpenClaw's approval capability, and forwarding is on.
cfg.tools = { ...(cfg.tools ?? {}), exec: { ...(cfg.tools?.exec ?? {}), mode: env.EXEC_MODE || "ask" } };
cfg.approvals = { ...(cfg.approvals ?? {}), exec: { enabled: true, mode: "session" } };
// No command from the chat runs on the host without the agent, and nobody in a chat is OpenClaw's owner.
cfg.commands = { ...(cfg.commands ?? {}), bash: false };
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

exec openclaw gateway run
