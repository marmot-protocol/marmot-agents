#!/usr/bin/env bash
# Install marmot-agent with Codex, built into the core. Codex must already be installed and signed in.
# Run with --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=codex
HARNESS_TITLE=Codex
HARNESS_KIND=builtin
HARNESS_OPTIONS=""
HARNESS_SUPPORT=""

harness_check() {
    CODEX=$(program_path codex) || {
        CODEX=codex
        missing "Codex" "Install it (https://github.com/openai/codex, for example npm install -g @openai/codex)," \
            "sign in with: codex login, then run this again."
    }
}

harness_prepare() { :; }

harness_settings() {
    say "[codex]"
    say "program = $(toml_str "$CODEX")"
    say "cwd = $(toml_str "$CWD")"
    say "# sandbox = \"workspace-write\"        # or \"read-only\", \"danger-full-access\""
    say "# approval_policy = \"on-request\"     # or \"untrusted\", \"on-failure\", \"never\""
}

main "$@"
