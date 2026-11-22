#!/usr/bin/env bash
# Install marmot-agent with OpenClaw. The core serves its socket; OpenClaw runs with the marmot plugin in a
# container, built here from packages/openclaw, on an OpenAI-compatible model endpoint, and connects to it.
# Linux only, for now. Run with --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=openclaw
HARNESS_TITLE=OpenClaw
HARNESS_KIND=socket
HARNESS_OPTIONS="model-key model model-url"
HARNESS_SUPPORT="packages/openclaw"

harness_check() {
    linux_only
    need_docker
    need_model
}

harness_prepare() {
    say "building OpenClaw's container image"
    act docker build -q -f "$MA_STAGE/packages/openclaw/container/Dockerfile" -t marmot-agent-openclaw \
        "$MA_STAGE/packages/openclaw"
    private_dir "$MA_AGENT_DIR/openclaw"
}

harness_settings() {
    say "[connector]"
}

harness_env() {
    say "MODEL_BASE_URL=$MODEL_URL"
    say "MODEL_NAME=$MODEL"
}

# A container left by a connector that was killed outright still holds the name; this service owns it.
harness_connector() {
    say "run=$(sh_quote "$MA_LIB/packages/openclaw/run.sh")"
    say "state=$(sh_quote "$MA_AGENT_DIR/openclaw")"
    say "docker rm -f \"\$(\"\$run\" --name \"\$state\")\" >/dev/null 2>&1 || true"
    say "exec \"\$run\" $(sh_quote "$MA_HOME") \"\$state\" $(sh_quote "$CWD") $(sh_quote "$MODEL_KEY")"
}

harness_uninstall() {
    local run=$MA_LIB/packages/openclaw/run.sh name
    if [ -x "$run" ] && [ -d "$MA_AGENT_DIR/openclaw" ] && program_path docker >/dev/null; then
        name=$("$run" --name "$MA_AGENT_DIR/openclaw")
        act docker rm -f "$name" >/dev/null 2>&1 || true
    fi
}

main "$@"
