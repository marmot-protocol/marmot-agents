#!/usr/bin/env bash
# Install marmot-agent with OpenCode, driven over ACP by the core. OpenCode must already be installed, with
# a model provider set up. Run with --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=opencode
HARNESS_TITLE=OpenCode
HARNESS_KIND=builtin
HARNESS_OPTIONS=""
HARNESS_SUPPORT=""

harness_check() {
    OPENCODE=$(program_path opencode) || {
        OPENCODE=opencode
        missing "OpenCode" "Install it (https://opencode.ai), set up a model provider with: opencode auth login," \
            "then run this again."
    }
}

harness_prepare() { :; }

harness_settings() {
    say "[acp]"
    say "name = \"opencode\""
    say "command = [$(toml_str "$OPENCODE"), \"acp\"]"
    say "cwd = $(toml_str "$CWD")"
}

main "$@"
