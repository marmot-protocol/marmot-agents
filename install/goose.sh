#!/usr/bin/env bash
# Install marmot-agent with Goose, driven over ACP by the core. Goose must already be installed, as
# `goose` on your PATH, and set up with a model provider (goose configure): the core runs `goose acp` as
# you, in the work folder, with Goose's own settings (D-102). With --container it runs instead in a
# throwaway container, built here from tools/goose, on an OpenAI-compatible model endpoint (Linux on
# x86_64 only). Run with --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=goose
HARNESS_TITLE=Goose
HARNESS_KIND=builtin
HARNESS_OPTIONS="container model-key model model-url goose-mode"
HARNESS_SUPPORT="tools/goose"
# The release the driver was built and recorded against (fixtures/recordings/acp/goose).
GOOSE_TESTED=1.59.2
GOOSE_HOW="Install it (https://github.com/aaif-goose/goose/releases), set up a model provider with: goose configure, then run this again."

harness_check() {
    if [ "$CONTAINER" = 1 ]; then
        linux_only
        # The image fetches Goose's x86_64 release (tools/goose/Dockerfile).
        case $MA_MACHINE in
            x86_64 | amd64) ;;
            *) die "Goose in a container is installed on x86_64 only for now: its container image is built for it" ;;
        esac
        need_docker
        need_model
        return 0
    fi
    container_options_only
    HARNESS_SUPPORT=""
    GOOSE=$(harness_program goose) || {
        GOOSE=goose
        missing "Goose" "$GOOSE_HOW"
        return 0
    }
    check_version "$GOOSE" "$GOOSE_TESTED" "$GOOSE_HOW"
}

# The image is built under a tag of its own, and given the one tools/goose/goose-acp.sh runs once the
# install is done ("Container images" in lib.sh).
harness_prepare() {
    [ "$CONTAINER" = 1 ] || return 0
    say "building Goose's container image"
    act docker build -q -t marmot-agents-goose:staging "$MA_STAGE/tools/goose"
}

harness_commit() {
    [ "$CONTAINER" = 1 ] || return 0
    image_commit marmot-agents-goose
}

harness_put_back() {
    [ "$CONTAINER" = 1 ] || return 0
    image_put_back marmot-agents-goose
}

harness_settings() {
    say "[acp]"
    say "name = \"goose\""
    if [ "$CONTAINER" = 1 ]; then
        say "command = [$(toml_str "$MA_LIB/tools/goose/goose-acp.sh"), $(toml_str "$MODEL_KEY"), $(toml_str "$MA_AGENT_DIR/goose")]"
    else
        say "command = [$(toml_str "$GOOSE"), \"acp\"]"
    fi
    say "cwd = $(toml_str "$CWD")"
    if [ "$CONTAINER" = 1 ]; then
        # The container cannot reach the loopback address the file tool listens on (D-110).
        say "send_files = false"
    fi
}

# Goose reads GOOSE_MODE from its environment, in its container or not, and runs in its own default
# mode, which asks nothing, unless --goose-mode chose one (D-100).
harness_env() {
    [ -z "$GOOSE_MODE" ] || say "GOOSE_MODE=$GOOSE_MODE"
    [ "$CONTAINER" = 1 ] || return 0
    say "MODEL_BASE_URL=$MODEL_URL"
    say "MODEL_NAME=$MODEL"
}

main "$@"
