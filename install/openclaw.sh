#!/usr/bin/env bash
# Install marmot-agent with OpenClaw. OpenClaw must already be installed, as `openclaw` on your PATH, and
# set up (openclaw onboard) with its model (D-102). The core runs as a service and serves its socket; the
# Marmot channel plugin, built here from packages/openclaw, is installed into your OpenClaw with its own
# plugin commands, and connects to that socket from inside your OpenClaw gateway. Of your OpenClaw's
# settings it changes only the marmot channel and, if they are off, which approvals are forwarded to a
# chat; a copy of the settings is kept first, and --uninstall takes out what it added. Run with --help
# for the options. The shared part is in lib.sh.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARNESS=openclaw
HARNESS_TITLE=OpenClaw
HARNESS_KIND=plugin
HARNESS_OPTIONS="replace-plugin"
HARNESS_SUPPORT="packages/openclaw"
# The plugin needs this release or later (packages/openclaw/package.json, openclaw.install.minHostVersion).
OPENCLAW_LEAST=2026.11.5
OPENCLAW_HOW="Install or update it (https://docs.openclaw.ai), set it up with: openclaw onboard, then run this again."
OUR_PACKAGE=marmot-agent-openclaw
# Which approvals OpenClaw forwards to the chat they came from, when it forwards none: those of a Marmot
# chat, whose session keys hold ":marmot:", and no others. Without forwarding, an approval waits unseen
# (docs/reference/openclaw-messages.md).
OUR_APPROVALS='{"enabled":true,"mode":"session","sessionFilter":[":marmot:"]}'
# The settings our plugin's channel takes (packages/openclaw/openclaw.plugin.json); OpenClaw refuses any
# other in channels.marmot while our plugin is in.
OUR_CHANNEL_KEYS="enabled home"

harness_check() {
    # What harness_connect reads, also when a dry run goes on without OpenClaw.
    OLD_PLUGIN="" OPENCLAW_FOUND=0
    choose_files_from ""
    need_node
    OPENCLAW=$(program_path openclaw) || {
        OPENCLAW=openclaw
        missing "OpenClaw" "$OPENCLAW_HOW"
        return 0
    }
    OPENCLAW_FOUND=1
    local version
    version=$(version_of "$OPENCLAW")
    if [ -z "$version" ] || version_below "$version" "$OPENCLAW_LEAST"; then
        missing "OpenClaw $OPENCLAW_LEAST or later" "This is OpenClaw ${version:-of an unknown version}. $OPENCLAW_HOW"
        return 0
    fi
    say "found OpenClaw $version at $OPENCLAW"
    finish_half_done_move
    openclaw_files_from
    # One OpenClaw serves one agent: its marmot channel names one agent's home.
    local home
    home=$(oc_get channels.marmot.home)
    if [ -n "$home" ] && [ "$home" != "$MA_HOME" ]; then
        die "your OpenClaw already connects to the agent whose home is $home. One OpenClaw serves one agent: uninstall that one first (install/openclaw.sh --uninstall --name <its name>), or install into another OpenClaw (OPENCLAW_HOME)"
    fi
    local found package
    found=$(plugin_found)
    package=$(plugin_package "${found#* }")
    if [ -z "$found" ] || [ "$package" = "$OUR_PACKAGE" ]; then
        :
    elif [ "${found%% *}" = found ]; then
        # A plugin OpenClaw loads from a folder it is pointed at wins over one it installed.
        missing "room for our Marmot plugin" "Your OpenClaw loads another plugin called marmot ($package) from" \
            "${found#* }, a folder its settings point it at (plugins.load.paths), and it would load that" \
            "one in place of ours. Take that folder out of plugins.load.paths (moving the install over" \
            "first if it is today's connector, docs/move.md), then run this again."
    else
        if [ "$REPLACE_PLUGIN" -eq 1 ]; then
            OLD_PLUGIN=${found#* }
        else
            missing "room for our Marmot plugin" "Your OpenClaw has another plugin called marmot ($package), such as the one of" \
                "today's connector. Move that install over first (docs/move.md), then run this again with" \
                "--replace-plugin to put ours in its place; a copy of the old one is kept in the agent's folder."
        fi
    fi
}

# A move stopped part way, after our plugin went in and before the old plugin's settings came out of its
# channel, leaves an OpenClaw that refuses to read or change any of its settings, or to start. With the
# old plugin in, OpenClaw takes them, and harness_connect takes them out with OpenClaw's own command; here
# they come out of the settings file itself, after a copy of it is kept. Only those OpenClaw names in its
# refusal, and only from a file that is plain JSON.
finish_half_done_move() {
    local config leftovers refusal
    config=$("$OPENCLAW" config file 2>/dev/null) || return 0
    [ -f "$config" ] || return 0
    leftovers=$(channel_leftovers "$config")
    [ -n "$leftovers" ] || return 0
    if refusal=$("$OPENCLAW" config get channels.marmot 2>&1); then return 0; fi
    case $refusal in *channels.marmot*) ;; *) return 0 ;; esac
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would take $leftovers out of channels.marmot in $config, which OpenClaw refuses with our plugin in"
        return 0
    fi
    backup_file "$config"
    # shellcheck disable=SC2016 # read by node
    "$NODE" -e '
        const fs = require("fs"), [file, ours] = process.argv.slice(1);
        const c = JSON.parse(fs.readFileSync(file, "utf8")), channel = c.channels.marmot;
        for (const k of Object.keys(channel)) if (!ours.split(" ").includes(k)) delete channel[k];
        const next = `${file}.marmot-agent`;
        fs.writeFileSync(next, JSON.stringify(c, null, 2) + "\n", { mode: fs.statSync(file).mode & 0o777 });
        fs.renameSync(next, file);' "$config" "$OUR_CHANNEL_KEYS" ||
        die "could not take $leftovers out of channels.marmot in $config: take them out yourself, then run this again"
    say "took $leftovers out of channels.marmot in $config: a move stopped part way left them, and OpenClaw refuses them with our plugin in"
}

# The settings in the marmot channel of OpenClaw's settings file that our plugin does not take, sorted
# and joined by commas; nothing if there are none, or the file is not plain JSON.
channel_leftovers() {
    # shellcheck disable=SC2016 # read by node
    "$NODE" -e '
        try {
          const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))?.channels?.marmot;
          if (c && typeof c === "object" && !Array.isArray(c)) {
            const ours = process.argv[2].split(" ");
            process.stdout.write(Object.keys(c).filter((k) => !ours.includes(k)).sort().join(", "));
          }
        } catch {}' "$1" "$OUR_CHANNEL_KEYS" 2>/dev/null || true
}

# A value of OpenClaw's settings, as JSON with --json; nothing if it is not set.
oc_get() {
    local out
    if out=$("$OPENCLAW" config get "$@" 2>/dev/null); then printf '%s' "$out"; fi
}

# The plugin called marmot that OpenClaw has, if any, as "installed FOLDER" for one an install of its
# own put there, or "found FOLDER" for one it finds in a folder it is pointed at, with no install record.
plugin_found() {
    "$OPENCLAW" plugins inspect marmot --json 2>/dev/null | "$NODE" -e '
        let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
          try {
            const j = JSON.parse(s), p = j.plugin || {}, installed = j.install?.installPath;
            const folder = installed || p.rootDir || (p.source ? require("path").dirname(p.source) : "");
            if (folder) process.stdout.write((installed ? "installed " : "found ") + folder);
          } catch {}
        });' || true
}
# The package name of the plugin in a folder; nothing for no folder.
plugin_package() {
    local folder=$1
    [ -n "$folder" ] || return 0
    [ -f "$folder/package.json" ] || { printf 'unknown'; return 0; }
    "$NODE" -e 'try { process.stdout.write(require(process.argv[1]).name || "unknown"); } catch { process.stdout.write("unknown"); }' \
        "$folder/package.json"
}

# Builds the plugin as JavaScript and packs it, as a release of it is installed. The packages that only
# build it are removed after.
harness_prepare() {
    say "building the OpenClaw plugin"
    # shellcheck disable=SC2016 # expanded by that shell
    act sh -c 'cd "$1" && npm ci --no-fund --no-audit --loglevel=error && npm run --silent build &&
        npm pack --silent >/dev/null && mv -f marmot-agent-openclaw-*.tgz marmot-agent-openclaw.tgz && rm -rf node_modules' \
        sh "$(canonical "$MA_STAGE/packages/openclaw")"
}

# The workspace of the OpenClaw agent that takes the Marmot channel's chats, which it writes the files it
# sends in (D-090), as OpenClaw itself resolves it (docs/concepts/multi-agent.md): the one agent its
# bindings route the channel's default account to, or, with no binding, its one agent or its system
# agent. None when the chats may go to more than one agent, or to none OpenClaw names. OpenClaw's state
# folder, beside its settings and its agents' own folders, and its media store in it are kept out: they
# hold the files people sent it on any channel.
openclaw_files_from() {
    local agents bindings config line state found=""
    agents=$("$OPENCLAW" agents list --json 2>/dev/null) || agents=""
    bindings=$("$OPENCLAW" agents bindings --json 2>/dev/null) || bindings=""
    config=$("$OPENCLAW" config file 2>/dev/null) || config=""
    case $config in /*) GATEWAY_STATE+=("$(dirname "$config")") ;; esac
    while IFS= read -r line; do
        case $line in
            "workspace "*) found=${line#workspace } ;;
            "why "*) FILES_WHY=${line#why } ;;
            "state "*) GATEWAY_STATE+=("${line#state }") ;;
        esac
    done <<END
$(printf '%s' "$agents" | channel_agent "$bindings")
END
    for state in ${GATEWAY_STATE[@]+"${GATEWAY_STATE[@]}"}; do GATEWAY_CACHE+=("$state/media"); done
    if [ -z "$found" ]; then
        state=${FILES_WHY:-OpenClaw did not say which of its agents takes the Marmot chats}
        choose_files_from ""
        FILES_WHY="$state; list the workspace of the agent that takes them"
        return 0
    fi
    choose_files_from "$found"
}

# Reads OpenClaw's agents (`agents list --json`) on standard input and its bindings (`agents bindings
# --json`) as an argument, and says "workspace FOLDER" for the agent the Marmot channel's chats go to,
# or "why WORDS", and "state FOLDER" for the state folder each agent's own folder lies in.
channel_agent() {
    # shellcheck disable=SC2016 # read by node
    "$NODE" -e '
        let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
          const read = (t) => { try { return JSON.parse(t); } catch { return null; } };
          const agents = read(s), bindings = read(process.argv[1]) ?? [];
          if (!Array.isArray(agents) || !Array.isArray(bindings)) return console.log("why OpenClaw did not list its agents");
          for (const a of agents) {
            const m = /^(.*)\/agents\/[^/]+\/agent\/?$/.exec(String(a?.agentDir ?? ""));
            if (m && m[1]) console.log(`state ${m[1]}`);
          }
          // The plugin has one account, "default": a binding names it, or every account, or none.
          const ours = bindings.filter((b) => b?.match?.channel === "marmot" && [undefined, "", "default", "*"].includes(b.match.accountId));
          const narrow = (b) => Object.keys(b.match).some((k) => k !== "channel" && k !== "accountId");
          const ids = [...new Set(ours.map((b) => b.agentId))];
          const fallback = agents.length === 1 ? agents[0]?.id : agents.find((a) => a?.isDefault === true)?.id;
          let id;
          if (ids.length === 0) id = fallback;
          else if (ids.length === 1 && (ours.some((b) => !narrow(b)) || ids[0] === fallback)) id = ids[0];
          else return console.log("why OpenClaw may give its Marmot chats to more than one of its agents");
          const agent = agents.find((a) => a?.id === id);
          if (id === undefined || agent === undefined) return console.log("why OpenClaw gives its Marmot chats to no agent it names");
          if (typeof agent.workspace !== "string" || agent.workspace === "") return console.log("why OpenClaw did not say where its agent works");
          console.log(`workspace ${agent.workspace}`);
        });' "$1" 2>/dev/null || true
}

harness_settings() {
    files_from_settings
    say ""
    say "[connector]"
}

# Adds the plugin to the person's OpenClaw and points its channel at this agent's home.
harness_connect() {
    local config channel approvals enabled
    if [ "$OPENCLAW_FOUND" -eq 0 ]; then
        say "would add the Marmot plugin to your OpenClaw, once it is installed"
        return 0
    fi
    # What it changes is put back if it fails or is stopped before the channel is set (change_begin in
    # lib.sh): OpenClaw's settings, and the old connector's plugin, which it replaces.
    config=$("$OPENCLAW" config file 2>/dev/null) || config=""
    set --
    if [ -n "$config" ]; then
        backup_file "$config"
        set -- file "$config" "$KEPT"
    fi
    if [ -n "$OLD_PLUGIN" ]; then
        backup_file "$OLD_PLUGIN"
        set -- "$@" folder "$OLD_PLUGIN" "$KEPT"
    fi
    change_begin "$@"
    channel=$("$NODE" -e 'process.stdout.write(JSON.stringify({ enabled: true, home: process.argv[1] }))' "$MA_HOME")
    put_plugin_in "$channel"
    change_end
    approvals=$(oc_get approvals.exec --json)
    # On is true or "auto"; only one that is off or not set is switched on.
    enabled=$(printf '%s' "$approvals" | json_field enabled)
    if [ -n "$enabled" ] && [ "$enabled" != false ]; then
        if [ ! -f "$MA_AGENT_DIR/openclaw-approvals" ]; then
            say "your OpenClaw forwards approvals already (approvals.exec): it must forward those of a Marmot chat's"
            say "sessions to their own chat, or an approval waits unseen"
        fi
        return 0
    fi
    # What was there before, to put back on --uninstall: the setting as JSON, or nothing.
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would record approvals.exec as it is in $MA_AGENT_DIR/openclaw-approvals"
    else
        printf '%s\n' "$approvals" | put "$MA_AGENT_DIR/openclaw-approvals" 600
    fi
    act "$OPENCLAW" config set approvals.exec "$OUR_APPROVALS" --strict-json
}

# Takes the old plugin's channel out, puts our plugin in, and points its channel at this agent's home,
# given as JSON. Fails at the first of these that fails.
put_plugin_in() {
    # The old plugin's channel has settings ours does not take (its socket, account and who may write).
    # OpenClaw checks the whole of its settings against the installed plugin's schema before it reads or
    # changes any, so once ours is in, it would refuse to do anything: take them out first, while the old
    # plugin still accepts what is left. The copy kept before has them.
    if [ -n "$OLD_PLUGIN" ] && [ -n "$(oc_get channels.marmot)" ]; then
        act "$OPENCLAW" config unset channels.marmot || return 1
    fi
    # Installing a plugin enables it too.
    say "adding the Marmot plugin to your OpenClaw"
    act "$OPENCLAW" plugins install --force --accept-capabilities "$MA_LIB/packages/openclaw/marmot-agent-openclaw.tgz" ||
        return 1
    act "$OPENCLAW" config set channels.marmot "$1" --strict-json
}

# A top-level field of a JSON object on standard input, as JSON; nothing if it cannot be read.
json_field() {
    "$NODE" -e '
        let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
          try { const v = JSON.parse(s)[process.argv[1]]; if (v !== undefined) process.stdout.write(JSON.stringify(v)); } catch {}
        });' "$1" 2>/dev/null || true
}

harness_summary() {
    say ""
    files_from_summary
    say "Your OpenClaw loads the plugin when its gateway starts. Restart it: openclaw gateway restart"
    say "(or start it: openclaw gateway run). The plugin connects to the agent once both run."
}

# Takes out of the person's OpenClaw what the install added: the plugin and its channel, and the
# approvals setting, if it is still the one the install wrote. Only if the channel names this agent.
harness_uninstall() {
    OPENCLAW=$(program_path openclaw) || {
        warn "openclaw is not on PATH: remove the marmot plugin from your OpenClaw yourself (openclaw plugins uninstall marmot)"
        return 0
    }
    NODE=$(program_path node) || NODE=node
    if [ "$(oc_get channels.marmot.home)" != "$MA_HOME" ]; then
        say "your OpenClaw does not connect to this agent; it is left as it is"
        return 0
    fi
    say "removing the Marmot plugin and its channel from your OpenClaw"
    act "$OPENCLAW" plugins uninstall --force marmot
    if [ -f "$MA_AGENT_DIR/openclaw-approvals" ]; then
        local before now
        before=$(cat "$MA_AGENT_DIR/openclaw-approvals")
        now=$(oc_get approvals.exec --json | "$NODE" -e '
            let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => {
              try { process.stdout.write(JSON.stringify(JSON.parse(s))); } catch {}
            });' 2>/dev/null || true)
        if [ "$now" = "$OUR_APPROVALS" ]; then
            if [ -n "$before" ]; then
                act "$OPENCLAW" config set approvals.exec "$before" --strict-json
            else
                act "$OPENCLAW" config unset approvals.exec
            fi
        fi
        act rm -f "$MA_AGENT_DIR/openclaw-approvals"
    fi
    say "Restart your OpenClaw gateway for it to drop the plugin: openclaw gateway restart"
}

main "$@"
