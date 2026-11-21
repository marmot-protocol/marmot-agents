#!/usr/bin/env bash
# Install marmot-agent with Hermes. The core serves its socket; Hermes runs with the Marmot plugin in a
# container, built here from python/hermes, on an OpenAI-compatible model endpoint, and connects to it.
# Hermes's own home is a Docker volume. Linux only, for now. Run with --help for the options. The shared
# part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=hermes
HARNESS_TITLE=Hermes
HARNESS_KIND=socket
HARNESS_OPTIONS="model-key model model-url"
HARNESS_SUPPORT="python/hermes python/marmot_hermes"

volume() { printf 'marmot-agent-hermes-%s' "$NAME"; }

harness_check() {
    linux_only
    need_docker
    need_model_key
}

harness_prepare() {
    say "building Hermes's container image"
    act docker build -q --build-arg UID="$(id -u)" --build-arg GID="$(id -g)" \
        -f "$MA_STAGE/python/hermes/Dockerfile" -t marmot-agents-hermes "$MA_STAGE/python"
    act docker volume create "$(volume)"
}

harness_settings() {
    say "[connector]"
}

harness_env() {
    say "MODEL_BASE_URL=${MODEL_URL:-$MA_DEFAULT_MODEL_URL}"
    [ -z "$MODEL" ] || say "MODEL_NAME=$MODEL"
    say "HERMES_CONTAINER=$(volume)"
}

# A container left by a connector that was killed outright still holds the name; this service owns it.
harness_connector() {
    say "docker rm -f \"\$HERMES_CONTAINER\" >/dev/null 2>&1 || true"
    say "exec $(sh_quote "$MA_LIB/python/hermes/run.sh") $(sh_quote "$MA_HOME") $(sh_quote "$CWD") $(sh_quote "$(volume)") $(sh_quote "$MODEL_KEY")"
}

harness_uninstall() {
    if program_path docker >/dev/null; then act docker rm -f "$(volume)" >/dev/null 2>&1 || true; fi
    say "kept Hermes's home, the Docker volume $(volume)"
}

main "$@"
