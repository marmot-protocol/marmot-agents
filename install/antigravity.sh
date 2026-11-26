#!/usr/bin/env bash
# Install marmot-agent with Google Antigravity, driven over ACP by the core. The Antigravity CLI
# (`agy`) must already be installed and signed in. Run with --help for the options. The shared part
# is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=antigravity
HARNESS_TITLE=Antigravity
HARNESS_KIND=builtin
HARNESS_OPTIONS=""
HARNESS_SUPPORT=""
# The release the driver was built and recorded against (fixtures/recordings/acp/antigravity).
AGY_TESTED=1.5.18
AGY_HOW="Install it (https://antigravity.google/docs/cli/acp/), sign in with: agy acp --login (or run agy), then run this again."

harness_check() {
    AGY=$(harness_program agy) || {
        AGY=agy
        missing "the Antigravity CLI (agy)" "$AGY_HOW"
        return 0
    }
    check_version "$AGY" "$AGY_TESTED" "$AGY_HOW"
}

harness_prepare() { :; }

harness_settings() {
    say "[acp]"
    say "name = \"antigravity\""
    say "command = [$(toml_str "$AGY"), \"acp\"]"
    say "cwd = $(toml_str "$CWD")"
}

main "$@"
