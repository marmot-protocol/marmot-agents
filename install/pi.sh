#!/usr/bin/env bash
# Install marmot-agent with Pi, built into the core. Pi must already be installed, as `pi` on your PATH,
# and set up with a model provider: the core runs it as you, in the work folder, with its own settings
# (D-102). Pi asks nobody before a tool runs, so it acts with your rights, as when you run it yourself.
# With --container it runs instead in a throwaway container for each chat, built here from tools/pi, on
# an OpenAI-compatible model endpoint (Linux only). Run with --help for the options. The shared part is
# in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=pi
HARNESS_TITLE=Pi
HARNESS_KIND=builtin
HARNESS_OPTIONS="container model-key model model-url"
HARNESS_SUPPORT="tools/pi"
# The release the driver was built and recorded against (fixtures/recordings/pi).
PI_TESTED=1.6.0
PI_HOW="Install it with: npm install -g @earendil-works/pi-coding-agent, run pi once to set up a model provider, then run this again."

harness_check() {
    if [ "$CONTAINER" = 1 ]; then
        linux_only
        need_docker
        need_model
        return 0
    fi
    container_options_only
    HARNESS_SUPPORT=""
    PI=$(harness_program pi) || {
        PI=pi
        missing "Pi" "$PI_HOW"
        return 0
    }
    check_version "$PI" "$PI_TESTED" "$PI_HOW"
}

# The image is built under a tag of its own, and given the one tools/pi/pi.sh runs once the install is
# done ("Container images" in lib.sh).
harness_prepare() {
    [ "$CONTAINER" = 1 ] || return 0
    say "building Pi's container image"
    act docker build -q -t marmot-agents-pi:staging "$MA_STAGE/tools/pi"
}

harness_commit() {
    [ "$CONTAINER" = 1 ] || return 0
    image_commit marmot-agents-pi
}

harness_put_back() {
    [ "$CONTAINER" = 1 ] || return 0
    image_put_back marmot-agents-pi
}

harness_settings() {
    say "[pi]"
    say "cwd = $(toml_str "$CWD")"
    if [ "$CONTAINER" = 1 ]; then
        say "program = $(toml_str "$MA_LIB/tools/pi/pi.sh")"
        say "args = [$(toml_str "$MODEL_KEY"), $(toml_str "$MA_AGENT_DIR/pi")]"
    else
        say "program = $(toml_str "$PI")"
    fi
}

harness_env() {
    [ "$CONTAINER" = 1 ] || return 0
    say "MODEL_BASE_URL=$MODEL_URL"
    say "MODEL_NAME=$MODEL"
}

main "$@"
