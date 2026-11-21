#!/usr/bin/env bash
# Install marmot-agent with Claude Code. The core serves its socket and the Claude Code driver
# (packages/claude-code, on Node 24) connects to it, running the Claude Code its SDK ships. Claude Code
# must be signed in on this account. Run with --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=claude-code
HARNESS_TITLE="Claude Code"
HARNESS_KIND=socket
HARNESS_OPTIONS="permission-mode"
HARNESS_SUPPORT="packages/claude-code"

harness_check() {
    local major
    NODE=$(program_path node) || NODE=""
    if [ -z "$NODE" ]; then
        NODE=node
        missing "Node 24" "Install Node 24 or later (https://nodejs.org), then run this again."
    else
        major=$("$NODE" -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)
        [ "$major" -ge 24 ] 2>/dev/null ||
            missing "Node 24" "This machine has Node $major. Install Node 24 or later (https://nodejs.org)."
    fi
    program_path npm >/dev/null || missing "npm" "It comes with Node (https://nodejs.org)."
    if [ ! -d "$HOME/.claude" ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
        warn "Claude Code does not look signed in on this account: run claude once and sign in before the agent's first prompt"
    fi
}

harness_prepare() {
    say "installing the Claude Code driver's packages"
    # The folder with links resolved: npm ci misreads the lock file through a linked folder, such as a
    # home that is a link.
    act npm ci --omit=dev --no-fund --no-audit --prefix "$(canonical "$MA_STAGE/packages/claude-code")"
}

harness_settings() {
    say "[connector]"
}

harness_connector() {
    say "exec $(sh_quote "$NODE") $(sh_quote "$MA_LIB/packages/claude-code/src/main.ts") \\"
    say "    --home $(sh_quote "$MA_HOME") --cwd $(sh_quote "$CWD") --permission-mode $(sh_quote "$PERMISSION_MODE")"
}

main "$@"
