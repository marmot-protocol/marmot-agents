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
    need_model_key
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

# goose-acp.sh takes the endpoint as its host and the path of its chat completions. settle_values has
# checked the URL.
harness_env() {
    local url=${MODEL_URL:-$MA_DEFAULT_MODEL_URL} host path
    host=$(printf '%s' "$url" | sed -E 's#^(https?://[^/]+).*#\1#')
    path=${url#"$host"}
    path=${path#/}
    path=${path%/}
    say "MODEL_HOST=$host"
    say "MODEL_BASE_PATH=${path:+$path/}chat/completions"
    [ -z "$MODEL" ] || say "MODEL_NAME=$MODEL"
    say "GOOSE_MODE=$GOOSE_MODE"
}

main "$@"
