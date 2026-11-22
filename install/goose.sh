#!/usr/bin/env bash
# Install marmot-agent with Goose, driven over ACP by the core. Goose runs in a throwaway container,
# built here from tools/goose, on an OpenAI-compatible model endpoint. Linux only, for now. Run with
# --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=goose
HARNESS_TITLE=Goose
HARNESS_KIND=builtin
HARNESS_OPTIONS="model-key model model-url goose-mode"
HARNESS_SUPPORT="tools/goose"

harness_check() {
    linux_only
    # The image fetches Goose's x86_64 release (tools/goose/Dockerfile).
    case $MA_MACHINE in
        x86_64 | amd64) ;;
        *) die "Goose is installed on x86_64 only for now: its container image is built for it" ;;
    esac
    need_docker
    need_model
}

harness_prepare() {
    say "building Goose's container image"
    act docker build -q -t marmot-agents-goose "$MA_STAGE/tools/goose"
}

harness_settings() {
    say "[acp]"
    say "name = \"goose\""
    say "command = [$(toml_str "$MA_LIB/tools/goose/goose-acp.sh"), $(toml_str "$MODEL_KEY"), $(toml_str "$MA_AGENT_DIR/goose")]"
    say "cwd = $(toml_str "$CWD")"
}

# Goose runs in its own default mode, which asks nothing, unless --goose-mode chose one (D-100).
harness_env() {
    say "MODEL_BASE_URL=$MODEL_URL"
    say "MODEL_NAME=$MODEL"
    [ -z "$GOOSE_MODE" ] || say "GOOSE_MODE=$GOOSE_MODE"
}

main "$@"
