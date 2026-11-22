#!/usr/bin/env bash
# Install marmot-agent with Claude Code. The core serves its socket and the Claude Code driver
# (packages/claude-code, on Node 24) connects to it, running the Claude Code its SDK ships. Claude Code
# must be signed in on this account, with /login: an ANTHROPIC_API_KEY in the environment does not reach
# the service. Run with --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=claude-code
HARNESS_TITLE="Claude Code"
HARNESS_KIND=socket
HARNESS_OPTIONS="permission-mode"
HARNESS_SUPPORT="packages/claude-code"

harness_check() {
    need_node
    claude_signed_in
}

# The service has none of this shell's environment, and the installer writes no key into a file, so
# Claude Code must have a sign-in of its own stored: in ~/.claude/.credentials.json on Linux, in the
# Keychain on macOS (under "Claude Code-credentials": inferred from Claude Code's documentation, not
# checked on a Mac). An ANTHROPIC_API_KEY in this shell is not one.
claude_signed_in() {
    local how="Run claude, sign in with /login (a Claude account, or an Anthropic Console account for API use), then run this again."
    [ ! -f "$HOME/.claude/.credentials.json" ] || return 0
    if [ "$OS" = Darwin ]; then
        security find-generic-password -s "Claude Code-credentials" >/dev/null 2>&1 && return 0
    fi
    if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
        missing "a stored Claude Code sign-in" \
            "ANTHROPIC_API_KEY is set here, but the agent's service does not get it, and the installer does" \
            "not write a key into a file." "$how"
    elif [ "$OS" = Darwin ]; then
        warn "Claude Code does not look signed in on this account: run claude and sign in with /login before the agent's first prompt"
    else
        missing "a Claude Code sign-in" "$how"
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
