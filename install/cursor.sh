#!/usr/bin/env bash
# Install marmot-agent with Cursor's agent, driven over ACP by the core (D-080). The Cursor CLI must
# already be installed and signed in. Run with --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=cursor
HARNESS_TITLE=Cursor
HARNESS_KIND=builtin
HARNESS_OPTIONS=""
HARNESS_SUPPORT=""

harness_check() {
    CURSOR=$(program_path cursor-agent) || {
        CURSOR=cursor-agent
        missing "the Cursor CLI (cursor-agent)" "Install it (https://cursor.com/cli), sign in with: cursor-agent login," \
            "then run this again."
    }
}

harness_prepare() { :; }

harness_settings() {
    say "[acp]"
    say "name = \"cursor\""
    say "command = [$(toml_str "$CURSOR"), \"acp\"]"
    say "cwd = $(toml_str "$CWD")"
}

main "$@"
