#!/usr/bin/env bash
# Install marmot-agent with Pi, built into the core. Pi runs in a throwaway container for each chat,
# built here from tools/pi, on an OpenAI-compatible model endpoint (D-092). Linux only, for now. Run with
# --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=pi
HARNESS_TITLE=Pi
HARNESS_KIND=builtin
HARNESS_OPTIONS="model-key model model-url"
HARNESS_SUPPORT="tools/pi"

harness_check() {
    linux_only
    need_docker
    need_model
}

harness_prepare() {
    say "building Pi's container image"
    act docker build -q -t marmot-agents-pi "$MA_STAGE/tools/pi"
}

harness_settings() {
    say "[pi]"
    say "cwd = $(toml_str "$CWD")"
    say "program = $(toml_str "$MA_LIB/tools/pi/pi.sh")"
    say "args = [$(toml_str "$MODEL_KEY"), $(toml_str "$MA_AGENT_DIR/pi")]"
}

harness_env() {
    say "MODEL_BASE_URL=$MODEL_URL"
    say "MODEL_NAME=$MODEL"
}

main "$@"
