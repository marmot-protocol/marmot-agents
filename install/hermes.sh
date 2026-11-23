#!/usr/bin/env bash
# Install marmot-agent with Hermes. Hermes must already be installed, as `hermes` on your PATH, and set up
# with its model (D-102). The core runs as a service and serves its socket; the Marmot platform plugin
# (python/marmot_hermes) is put into your Hermes's plugins folder, enabled with Hermes's own commands, and
# connects to that socket from inside your Hermes gateway. Of your Hermes's settings it changes only the
# marmot platform and MARMOT_ALLOW_ALL_USERS, which lets Hermes take whatever the core lets through
# (D-084); a copy of config.yaml and .env is kept first, and --uninstall takes out what it added. Run with
# --help for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=hermes
HARNESS_TITLE=Hermes
HARNESS_KIND=plugin
HARNESS_OPTIONS="replace-plugin"
HARNESS_SUPPORT="python/marmot_hermes"
# The release the plugin was built and recorded against (python/hermes/Dockerfile).
HERMES_TESTED=0.23.2
HERMES_HOW="Install or update it (https://github.com/NousResearch/hermes-agent), set up its model with: hermes setup, then run this again."
# What our plugin's plugin.yaml says, and the old connector's does not.
OUR_PLUGIN="marmot-agent's local socket"

harness_check() {
    # What harness_connect reads, also when a dry run goes on without a Hermes to add the plugin to.
    HERMES_CONFIG="" HERMES_ENV="" PLUGIN="" OLD_PLUGIN=0
    choose_files_from ""
    HERMES=$(program_path hermes) || {
        HERMES=hermes
        missing "Hermes" "$HERMES_HOW"
        return 0
    }
    local version
    # Hermes prints its release after a v, and a date beside it; a checkout without tags says vunknown.
    version=$("$HERMES" --version 2>/dev/null | sed -n 's/.*[[:space:]]v\([0-9][0-9.]*[0-9]\).*/\1/p' | head -n 1)
    if ! "$HERMES" --version >/dev/null 2>&1; then
        missing "a working Hermes" "hermes --version failed. $HERMES_HOW"
        return 0
    fi
    say "found Hermes ${version:-of an unknown release} at $HERMES"
    if [ -n "$version" ] && version_below "$version" "$HERMES_TESTED"; then
        warn "Hermes $version is older than $HERMES_TESTED, the release marmot-agent was tested with; if it misbehaves, update it. $HERMES_HOW"
    fi
    HERMES_CONFIG=$("$HERMES" config path 2>/dev/null) || HERMES_CONFIG=""
    HERMES_ENV=$("$HERMES" config env-path 2>/dev/null) || HERMES_ENV=""
    case $HERMES_CONFIG in
        /*/config.yaml) ;;
        *) missing "Hermes's configuration" "hermes config path named no config.yaml. $HERMES_HOW"; return 0 ;;
    esac
    hermes_files_from
    PLUGIN=$(dirname "$HERMES_CONFIG")/plugins/marmot
    no_control "Hermes's plugins folder" "$PLUGIN"
    # One Hermes serves one agent: its marmot platform names one agent's home.
    local home
    home=$("$HERMES" config get platforms.marmot.home 2>/dev/null) || home=""
    if [ -n "$home" ] && [ "$home" != "$MA_HOME" ]; then
        die "your Hermes already connects to the agent whose home is $home. One Hermes serves one agent: uninstall that one first (install/hermes.sh --uninstall --name <its name>), or install into another Hermes (HERMES_HOME)"
    fi
    if [ -e "$PLUGIN" ] && ! ours "$PLUGIN"; then
        if [ "$REPLACE_PLUGIN" -eq 1 ]; then
            OLD_PLUGIN=1
        else
            missing "room for our Marmot plugin" "Your Hermes has another plugin in $PLUGIN, such as the one of today's" \
                "connector. Move that install over first (docs/move.md), then run this again with" \
                "--replace-plugin to put ours in its place; a copy of the old one is kept in the agent's folder."
        fi
    fi
}

ours() {
    [ -f "$1/plugin.yaml" ] && grep -qF "$OUR_PLUGIN" "$1/plugin.yaml"
}

# The folder Hermes's commands run in, which its agent writes the files it sends in (D-090):
# terminal.cwd. Unset, or ".", "auto" or "cwd", it is the home of the account Hermes runs as, which is not
# listed. With a terminal backend other than local, Hermes's commands run in a sandbox, whose files are
# not on this machine where Hermes names them. Hermes keeps its media cache, the files people sent it on
# any platform and the ones it made, in its home, beside config.yaml: a folder that holds or lies in it
# is not listed either.
hermes_files_from() {
    GATEWAY_CACHE=("$(dirname "$HERMES_CONFIG")")
    local cwd backend
    cwd=$("$HERMES" config get terminal.cwd 2>/dev/null) || cwd=""
    backend=$("$HERMES" config get terminal.backend 2>/dev/null) || backend=""
    case $backend in
        "" | local) ;;
        *) FILES_FROM="" FILES_WHY="Hermes runs its commands elsewhere (terminal.backend is $backend)"; return 0 ;;
    esac
    case $cwd in
        "" | . | auto | cwd) cwd="~" ;;
    esac
    choose_files_from "$cwd"
}

harness_prepare() { :; }

harness_settings() {
    files_from_settings
    say ""
    say "[connector]"
}

# Adds the plugin to the person's Hermes and points its platform at this agent's home. What it changes is
# put back if the install fails or is stopped before the core is started (change_begin in lib.sh):
# Hermes's settings, and the plugin it replaces, the old connector's or an older one of ours, or, where
# there was none, ours.
# Nothing changes before ours is in, and ours starts on the old plugin's settings too: Hermes reads a
# platform's extra over its other settings (gateway/config.py), and the old one's names this home.
# Everyone is let through only once ours is in, so the old plugin is never left with that.
harness_connect() {
    if [ -z "$PLUGIN" ]; then
        say "would add the Marmot plugin to your Hermes, once it is installed and set up"
        return 0
    fi
    local why=add
    [ "$OLD_PLUGIN" -eq 0 ] || why=replace
    set -- "$why" file "$HERMES_CONFIG"
    [ -z "$HERMES_ENV" ] || set -- "$@" file "$HERMES_ENV"
    # The old connector's plugin, none, or ours as an earlier install put it in, which goes with the
    # core that install put in, and is put back with it should this fail.
    if [ "$OLD_PLUGIN" -eq 1 ] || [ ! -e "$PLUGIN" ]; then
        set -- "$@" folder "$PLUGIN"
    else
        set -- "$@" own "$PLUGIN"
    fi
    change_begin "$@"
    say "adding the Marmot plugin to your Hermes"
    replace_dir "$MA_LIB/python/marmot_hermes" "$PLUGIN"
    # Enabling it the first time makes Hermes install what it needs, which takes a while.
    act "$HERMES" plugins enable marmot </dev/null
    act "$HERMES" config set platforms.marmot.enabled true
    act "$HERMES" config set platforms.marmot.home "$MA_HOME"
    # What was there before, to put back on --uninstall: the value, as Hermes reads it in any of the
    # shapes its .env takes, or nothing.
    local before
    before=$("$HERMES" config get MARMOT_ALLOW_ALL_USERS 2>/dev/null) || before=""
    if [ "$before" != true ]; then
        if [ "$DRY_RUN" -eq 1 ]; then
            say "would record MARMOT_ALLOW_ALL_USERS as it is in $MA_AGENT_DIR/hermes-allow-all"
        elif [ ! -f "$MA_AGENT_DIR/hermes-allow-all" ]; then
            printf '%s\n' "$before" | put "$MA_AGENT_DIR/hermes-allow-all" 600
        fi
        act "$HERMES" config set MARMOT_ALLOW_ALL_USERS true
    fi
}

harness_summary() {
    say ""
    files_from_summary
    say "Your Hermes loads the plugin when its gateway starts. Restart it: hermes gateway restart"
    say "(or start it: hermes gateway run). The plugin connects to the agent once both run."
}

# Takes out of the person's Hermes what the install added: the plugin, its platform, and the
# MARMOT_ALLOW_ALL_USERS it set. Only if the platform names this agent.
harness_uninstall() {
    HERMES=$(program_path hermes) || {
        warn "hermes is not on PATH: remove the marmot plugin and platform from your Hermes yourself"
        return 0
    }
    local home config
    home=$("$HERMES" config get platforms.marmot.home 2>/dev/null) || home=""
    if [ "$home" != "$MA_HOME" ]; then
        say "your Hermes does not connect to this agent; it is left as it is"
        return 0
    fi
    say "removing the Marmot plugin and its platform from your Hermes"
    act "$HERMES" config unset platforms.marmot
    if [ -f "$MA_AGENT_DIR/hermes-allow-all" ]; then
        local before
        before=$(cat "$MA_AGENT_DIR/hermes-allow-all")
        if [ -n "$before" ]; then
            act "$HERMES" config set MARMOT_ALLOW_ALL_USERS "$before"
        else
            act "$HERMES" config unset MARMOT_ALLOW_ALL_USERS
        fi
        act rm -f "$MA_AGENT_DIR/hermes-allow-all"
    fi
    act "$HERMES" plugins disable marmot
    config=$("$HERMES" config path 2>/dev/null) || config=""
    case $config in
        /*/config.yaml)
            PLUGIN=$(dirname "$config")/plugins/marmot
            if ours "$PLUGIN"; then act rm -rf "$PLUGIN"; fi
            ;;
    esac
    say "Restart your Hermes gateway for it to drop the plugin: hermes gateway restart"
}

main "$@"
