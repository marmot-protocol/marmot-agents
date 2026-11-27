# shellcheck shell=bash
# The shared part of the install scripts in this folder (D-091). Each script, such as codex.sh, sets
# what its harness needs and calls `main "$@"`. It runs on Linux (a systemd user service) and macOS (a
# launchd agent), with the bash that macOS ships (3.2), so: no associative arrays, no `mapfile`, and no
# expansion of an empty array under `set -u`.
#
# What it installs, for an agent called NAME (D-100):
#
#   ~/.local/bin/marmot-agent                          the core
#   ~/.local/share/marmot-agent/lib/                   the connectors, the plugins and the container
#                                                      files; an install replaces only the parts its
#                                                      harness uses
#   ~/.local/share/marmot-agent/install.lock           locked by the install that runs, so one runs at a time
#   ~/.local/share/marmot-agent/agents/NAME/           this agent: settings.toml, its account home in home/,
#                                                      what its harness keeps, start (which runs the core
#                                                      with service.env), and the service's files
#   systemd: ~/.config/systemd/user/marmot-agent-NAME.service (and -connector.service, for a socket harness)
#   launchd: ~/Library/LaunchAgents/org.marmot.marmot-agent.NAME.plist (and .connector.plist)
#
# It never installs a harness (D-102): each must be there already, a program on PATH or, for Hermes and
# OpenClaw, an installation of the person's own, into which it adds our plugin. Containers are only for
# Goose and Pi, and only when asked for with --container.
#
# Nothing secret is written by it: a model key is named by its path, and must be readable by its owner only.

MA_REPO=${MARMOT_AGENT_REPO:-marmot-protocol/marmot-agents}
MA_RELEASES=${MARMOT_AGENT_RELEASES:-https://github.com/$MA_REPO/releases}
if [ -n "${MARMOT_AGENT_RELEASES_API+x}" ]; then
    MA_RELEASES_API=$MARMOT_AGENT_RELEASES_API
else
    case $MA_RELEASES in
        file://*) MA_RELEASES_API="" ;;
        *) MA_RELEASES_API="https://api.github.com/repos/$MA_REPO/releases" ;;
    esac
fi
# White Noise's own relays, as MDK names them, and as today's installers use.
MA_DEFAULT_RELAYS="wss://relay.eu.whitenoise.chat
wss://relay.us.whitenoise.chat"
# There is no default model endpoint or model (D-100): a person brings their own.

# What a harness script sets before calling `main`:
#   HARNESS            its id, such as codex; the default agent name
#   HARNESS_TITLE      how people know it, such as Codex; the default name the agent shows in chats
#   HARNESS_KIND       builtin (the core drives it), socket (a connector of ours drives it over the
#                      socket, as a service beside the core's) or plugin (our plugin, inside the person's
#                      own gateway, connects to the socket: the gateway is theirs to run, and has a
#                      workspace of its own, so no work folder is asked for)
#   HARNESS_OPTIONS    the options of its own it takes, of: container model-key model model-url
#                      permission-mode goose-mode replace-plugin
#   HARNESS_SUPPORT    the folders of the support files (support_paths) it uses, such as tools/pi;
#                      harness_check may change it
# and these functions:
#   harness_check      checks what the harness needs; `need` and `missing` report
#   harness_prepare    builds its images or installs its packages, from its support files in $MA_STAGE
#   harness_settings   prints its section of the settings
#   harness_connector  (socket only) prints the body of the script that starts its connector
#   harness_connect    (plugin only) adds the plugin to the gateway, once the agent is set up
#   harness_env        (optional) prints KEY=value lines for the service's environment
#   harness_summary    (optional) prints what the person does next, after the summary
#   harness_uninstall  (optional) removes what the install added outside the agent's folder

# ---------------------------------------------------------------------------------------------------
# Output

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Runs a command, or, in a dry run, says it would. The command does not get the lock on installs
# (lock_installs), so that nothing it leaves running, such as a gateway it restarts, holds it.
act() {
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would run: $*"
    else
        "$@" 9>&-
    fi
}

# Writes standard input to a file with a mode, replacing it whole; in a dry run, shows it instead.
put() {
    local file=$1 mode=$2 content
    content=$(cat)
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would write $file (mode $mode):"
        printf '%s\n' "$content" | sed 's/^/    | /'
        return 0
    fi
    mkdir -p "$(dirname "$file")"
    (
        umask 077
        printf '%s\n' "$content" >"$file.new"
    )
    chmod "$mode" "$file.new"
    mv -f "$file.new" "$file"
}

# Makes a folder that only its owner may open.
private_dir() {
    if [ "$DRY_RUN" -eq 1 ]; then
        [ -d "$1" ] || say "would make folder $1 (mode 700)"
        return 0
    fi
    mkdir -p "$1"
    chmod 700 "$1"
}

# ---------------------------------------------------------------------------------------------------
# Quoting, for each kind of file written

no_control() {
    case $2 in
        *[[:cntrl:]]*) die "$1 must not hold control characters or line breaks" ;;
    esac
}

# The escapers below use sed, not ${var//.../...}, whose replacement text bash 3.2 and bash 5.2 read
# differently. Each value is one line: no_control has checked it.

# A TOML basic string.
toml_str() {
    printf '"%s"' "$(printf '%s' "$1" | sed -e 's/[\\"]/\\&/g')"
}

# A word for sh, in single quotes.
sh_quote() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

# A word on a systemd command line, in double quotes.
systemd_quote() {
    printf '"%s"' "$(printf '%s' "$1" | sed -e 's/[\\"]/\\&/g' -e 's/%/%%/g' -e 's/\$/$$/g')"
}

# A path in a systemd setting that takes one path, such as WorkingDirectory: not quoted, % doubled.
systemd_path() {
    printf '%s' "$1" | sed -e 's/%/%%/g'
}

xml_escape() {
    printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

# ---------------------------------------------------------------------------------------------------
# Keys

# Whether a text looks like a public key, in hex or as an npub. An npub's checksum is checked by
# `marmot-agent key`, which also turns it into hex, once the core is at hand (install_core).
owner_shape() {
    local key
    key=$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')
    case $key in
        npub1*)
            key=${key#npub1}
            [ ${#key} -eq 58 ] && [ -z "${key//[qpzry9x8gf2tvdw0s3jn54khce6mua7l]/}" ]
            ;;
        *) [ ${#key} -eq 64 ] && [ -z "${key//[0-9a-f]/}" ] ;;
    esac
}

# ---------------------------------------------------------------------------------------------------
# Asking

# Whether there is a person to ask.
can_ask() {
    [ "$ASSUME_YES" -eq 0 ] && { : </dev/tty; } 2>/dev/null
}

# Asks for a value, with a default; prints the answer.
ask() {
    local question=$1 default=$2 reply=""
    if [ -n "$default" ]; then
        printf '%s [%s]: ' "$question" "$default" >/dev/tty
    else
        printf '%s: ' "$question" >/dev/tty
    fi
    IFS= read -r reply </dev/tty || reply=""
    printf '%s' "${reply:-$default}"
}

# ---------------------------------------------------------------------------------------------------
# Checks

MISSING=0

# Reports a missing prerequisite with how to get it. A real run stops after the checks; a dry run goes on.
missing() {
    printf 'missing: %s\n' "$1" >&2
    shift
    local line
    for line in "$@"; do printf '    %s\n' "$line" >&2; done
    MISSING=1
}

# Prints the absolute path of a program on PATH, or nothing.
program_path() {
    local found
    found=$(command -v "$1" 2>/dev/null) || return 1
    case $found in
        /*) printf '%s' "$found" ;;
        *) return 1 ;;
    esac
}

# A coding harness's program: for an agent moved over, the one its settings name as a whole path, as the
# import carried it over from the old service (`program`, or the first word of `command`), which need not
# be on PATH; otherwise NAME on PATH. Fails when neither is there.
harness_program() {
    local kept
    if imported; then
        kept=$(sed -n -e 's/^program = "\([^"\\]*\)"[[:space:]]*$/\1/p' \
            -e 's/^command = \["\([^"\\]*\)".*$/\1/p' "$MA_SETTINGS" | head -n 1)
        case $kept in
            /*)
                [ -x "$kept" ] || return 1
                printf '%s' "$kept"
                return 0
                ;;
        esac
    fi
    program_path "$1"
}

need_docker() {
    if ! program_path docker >/dev/null; then
        missing "Docker" "$HARNESS_TITLE runs in a container. Install Docker (https://docs.docker.com/engine/install/)."
    elif ! docker info >/dev/null 2>&1; then
        missing "access to Docker" "Docker is installed but this account cannot use it. Add it to the docker group" \
            "(sudo usermod -aG docker \"\$USER\", then log in again), or run Docker rootless."
    fi
}

# Node 24 or later, and npm, which come together; sets NODE.
need_node() {
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
}

# A harness in a container runs on a model endpoint the person brings (D-100): its address, the model,
# and its key. None has a default.
need_model() {
    if [ -z "$MODEL_URL" ]; then
        missing "a model endpoint" "$HARNESS_TITLE needs an OpenAI-compatible model endpoint of your choosing; there is" \
            "no default. Give its address with --model-url URL, such as https://models.example/v1."
    fi
    if [ -z "$MODEL" ]; then
        missing "a model" "$HARNESS_TITLE needs the name of the model to use, as the endpoint names it. Give it with" \
            "--model NAME."
    fi
    if [ -z "$MODEL_KEY" ]; then
        missing "a model key" "$HARNESS_TITLE needs the API key of an OpenAI-compatible model endpoint, in a file" \
            "only you can read (chmod 600). Give its path with --model-key FILE. It is mounted read-only" \
            "into the container and is never copied or written into a settings or service file."
        return 0
    fi
    [ -f "$MODEL_KEY" ] || { missing "the model key file" "--model-key names no file."; return 0; }
    [ -O "$MODEL_KEY" ] || { missing "a model key file of your own" "The model key file must belong to you."; return 0; }
    local mode
    mode=$(file_mode "$MODEL_KEY")
    case $mode in
        *00) ;;
        *) missing "a private model key file" "The model key file can be read by others (mode $mode): chmod 600 it." ;;
    esac
}

file_mode() {
    if [ "$OS" = Darwin ]; then stat -f %Lp "$1"; else stat -c %a "$1"; fi
}

linux_only() {
    [ "$OS" = Linux ] || die "$HARNESS_TITLE in a container (--container) is installed on Linux only for now: the container needs this folder at the same path. Leave out --container to run your own $HARNESS_TITLE."
}

# The options that only a harness in a container takes: its model is the container's to set up, while
# a harness of the person's own uses its own settings.
container_options_only() {
    if [ -n "$MODEL_KEY" ] || [ -n "$MODEL" ] || [ -n "$MODEL_URL" ]; then
        die "--model-key, --model and --model-url go with --container: your own $HARNESS_TITLE uses the model it is set up with"
    fi
}

# The first version number, such as 1.6.0, in what a program prints for --version; nothing if none.
version_of() {
    "$@" --version 2>/dev/null | sed -n 's/[^0-9]*\([0-9][0-9]*\(\.[0-9][0-9]*\)\{1,2\}\).*/\1/p' | head -n 1
}

# Whether version $1 is lower than version $2, each numbers separated by dots.
version_below() {
    local a=$1 b=$2 x y
    while [ -n "$a" ] || [ -n "$b" ]; do
        x=${a%%.*} y=${b%%.*}
        x=${x:-0} y=${y:-0}
        [ "$x" -eq "$y" ] 2>/dev/null || { [ "$x" -lt "$y" ] 2>/dev/null; return; }
        case $a in *.*) a=${a#*.} ;; *) a="" ;; esac
        case $b in *.*) b=${b#*.} ;; *) b="" ;; esac
    done
    return 1
}

# Checks a harness program found on PATH: that it runs, and its version. A version older than the one
# its driver was built and recorded against is reported, not refused: it may work.
check_version() {
    local program=$1 tested=$2 how=$3 version
    # One that fails says no version: under pipefail its status would end the run here, saying nothing.
    version=$(version_of "$program") || version=""
    if [ -z "$version" ]; then
        missing "a working $HARNESS_TITLE" "$program --version failed or printed no version. $how"
        return 0
    fi
    say "found $HARNESS_TITLE $version at $program"
    if version_below "$version" "$tested"; then
        warn "$HARNESS_TITLE $version is older than $tested, the version marmot-agent was tested with; if it misbehaves, update it. $how"
    fi
}

# ---------------------------------------------------------------------------------------------------
# The person's own files

# Keeps a copy of a file of the person's own, such as a gateway's configuration, in the agent's folder,
# readable by its owner only, before the install changes it. KEPT names the copy, or is empty when none
# was made. A copy made earlier in the same second is kept too: the new one gets a number after it. The
# copy is made under a hidden name and renamed once whole, so a run killed part way leaves none half made
# under a name that says it is a copy.
backup_file() {
    local file=$1 copy base n=1 making
    KEPT=""
    [ -e "$file" ] || return 0
    copy="$MA_AGENT_DIR/backups/$(basename "$file").$(date -u +%Y%m%dT%H%M%SZ)"
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would keep a copy of $file in $copy"
        return 0
    fi
    private_dir "$MA_AGENT_DIR/backups"
    base=$copy
    while [ -e "$copy" ]; do n=$((n + 1)) && copy="$base.$n"; done
    making="$MA_AGENT_DIR/backups/.copying"
    rm -rf "$making"
    (umask 077 && cp -R "$file" "$making")
    chmod -R go-rwx "$making"
    mv "$making" "$copy"
    KEPT=$copy
    say "kept a copy of $file in $copy"
}

# ---------------------------------------------------------------------------------------------------
# Changing a gateway's plugin
#
# Putting our plugin into a person's Hermes or OpenClaw, in place of the old connector's or not, must
# not leave it with neither plugin set up to start. Before the first change, change_begin keeps a copy
# of what is about to change and records the copies in the agent's folder. While that record is there,
# a run that fails, or is stopped by HUP, INT or TERM, puts those copies back before it ends (finish). A
# run after one killed outright finds the record, takes no copies of its own, finishes the change, and,
# should it fail, puts back what the first run found, not what the killed run left. The change is done
# once the core that our plugin connects to is set up as a service and started (main), as the old
# connector's services are stopped by then: change_end then removes the record. --uninstall removes it
# too, so a move given up is not put back over the gateway later.
#
# A change is made only during an upgrade (upgrade_begin), and goes with it: the upgrade's record lists
# the change's record before the first change (change_note), so that whatever puts the upgrade back puts
# the plugin back with the core, another harness's install too (upgrade_put_back), and the rename that
# ends the upgrade ends the change (upgrade_end). An upgrade ends with the change of the run that ends
# it, and with no other: a run takes over an upgrade only when every change it lists that is not done is
# of this agent and to this gateway (MA_GATEWAY), and puts back any other first (changes_ours).
#
# Replacing the old connector's plugin, the copies are also listed in backups/before-move, which a
# person going back to that connector puts back (docs/move.md). It lists what the gateway had before
# the latest run that found the old plugin and finished, or noted, its change.

# Starts a change. Its first argument says whether it replaces the old connector's plugin (replace) or
# not (add); then come pairs, file, folder or own and the path of one it changes, own being the folder of
# our plugin as an earlier install put it in, which an update replaces. Each is copied first, and one not
# there is taken away should the change be put back. The copy of our own plugin is removed once it is
# not needed. A record a run before left is kept as it is.
change_begin() {
    local why=$1 arg record=""
    shift
    for arg in "$@"; do no_control "a path to put back" "$arg"; done
    if [ "$DRY_RUN" -eq 1 ]; then
        while [ $# -ge 2 ]; do
            backup_file "$2"
            shift 2
        done
        say "would note in $MA_CHANGE what to put back should this fail"
        return 0
    fi
    change_note
    if [ -f "$MA_CHANGE" ]; then
        say "a run before this one stopped part way through changing your $HARNESS_TITLE's plugin; this one finishes it, and puts back what that run found if it fails"
        MA_CHANGING=1
        return 0
    fi
    while [ $# -ge 2 ]; do
        backup_file "$2"
        record="$record$1	$2	$KEPT
"
        shift 2
    done
    # The list first: should this be killed before the record, nothing has changed yet, and the next
    # run lists its own copies.
    if [ "$why" = replace ]; then printf '%s' "$record" | put "$MA_AGENT_DIR/backups/before-move" 600; fi
    printf '%s' "$record" | put "$MA_CHANGE" 600
    MA_CHANGING=1
}

# Lists the change's record in the upgrade's, unless it is there already, before the record is written:
# killed between the two, nothing has changed yet, and a record listed and not there puts nothing back.
# Written beside the list and renamed over it.
change_note() {
    local list="$MA_UPGRADE/plugin-changes"
    [ -d "$MA_UPGRADE" ] || return 0
    if [ -f "$list" ] && grep -qxF "$MA_CHANGE" "$list"; then return 0; fi
    { if [ -f "$list" ]; then cat "$list"; fi && printf '%s\n' "$MA_CHANGE"; } >"$list.new"
    mv -f "$list.new" "$list"
}

# Ends a change; one stopped before its record is gone is put back. Its record went with the upgrade's
# (upgrade_end), which every install makes; the copies of our own plugin it named go now.
change_end() {
    local own
    if [ "$DRY_RUN" -eq 0 ]; then
        if [ -f "$MA_CHANGE" ]; then own=$(own_copies); else own=${MA_OWN_COPIES:-}; fi
        act rm -f "$MA_CHANGE"
        drop_copies "$own"
    fi
    MA_CHANGING=0
}

# The copies of our own plugin the record names (that of this run's change, or the one given), one a
# line.
own_copies() {
    awk -F '\t' '$1 == "own" && $3 != "" { print $3 }' "${1:-$MA_CHANGE}"
}

# Removes the copies named, one a line, once the record that names them is gone.
drop_copies() {
    local copy
    while IFS= read -r copy; do
        [ -z "$copy" ] || rm -rf "$copy"
    done <<EOF
$1
EOF
}

# Puts back what the record names (that of this run's change, or the one given), in its order; fails if
# any of it could not be put back.
put_back_change() {
    local kind path copy ok=0
    while IFS='	' read -r kind path copy; do
        case $kind in
            file) put_back_file "$copy" "$path" || ok=1 ;;
            folder | own) replace_dir "$copy" "$path" || ok=1 ;;
        esac
    done <"${1:-$MA_CHANGE}"
    return "$ok"
}

# After a change that did not end, its status given: what it changed put back, and the run ended,
# saying what happened and what to do. Fails if it could not be put back.
change_put_back() {
    local status=$1 kind path copy what="your $HARNESS_TITLE's settings" took="" copies="" how="" why again own
    # Gone already: it ended with the upgrade (upgrade_end).
    [ -f "$MA_CHANGE" ] || return 0
    while IFS='	' read -r kind path copy; do
        if [ "$kind" = folder ] && [ -n "$copy" ]; then what="$what and its old plugin"; fi
        if [ "$kind" = folder ] && [ -z "$copy" ]; then took=", and took our plugin out of it"; fi
        if [ "$kind" = own ] && [ -n "$copy" ]; then what="$what and our plugin as it was"; fi
        [ -n "$copy" ] || continue
        how="${how:+$how, }$copy to $path"
        [ "$kind" != own ] || continue
        copies="${copies:+$copies, }$copy"
    done <"$MA_CHANGE"
    own=$(own_copies)
    case $status in
        129 | 130 | 143) why="the install was stopped" again="Run this again" ;;
        *) why="putting our plugin into your $HARNESS_TITLE failed (above)" again="Run this again once what failed is fixed" ;;
    esac
    if ! put_back_change; then
        if [ -n "$how" ]; then
            printf 'error: %s\n' "$why, and putting $what back failed too: copy $how yourself (docs/move.md). \
$again after, and it puts back the same copies should it fail" >&2
        else
            printf 'error: %s\n' "$why, and putting $what back as they were failed too (above). $again" >&2
        fi
        return 1
    fi
    # The record kept, with every copy it names, puts the same copies back again: the replay stops here
    # (upgrade_put_back_changes), and the upgrade is kept with it.
    if ! rm -f "$MA_CHANGE"; then
        printf 'error: %s\n' "$why, so this put $what back as they were$took, but removing $MA_CHANGE, which \
names the copies, failed (above). $again, and it puts back the same copies should it fail" >&2
        return 1
    fi
    drop_copies "$own"
    printf 'error: %s\n' "$why, so this put $what back as they were$took${copies:+; the copies are $copies}. \
$again, or go back to today's connector (docs/move.md)" >&2
    return 0
}

# Puts a kept copy of a file in place of the file, or takes the file away given no copy. The copy is
# written beside it and renamed into place, keeping the file's mode.
# (Each function the exit trap runs ends with a status of its own: there, a bare return gives the status
# the run ended with.)
put_back_file() {
    local copy=$1 file=$2
    if [ -z "$copy" ]; then
        act rm -f "$file" || return 1
        return 0
    fi
    act cp "$copy" "$file.marmot-agent" || return 1
    if [ -e "$file" ]; then act chmod "$(file_mode "$file")" "$file.marmot-agent" || return 1; fi
    act mv -f "$file.marmot-agent" "$file"
}

# Puts a copy of folder FROM in place of folder TO, which need not be there, or takes TO away given no
# FROM. The copy is made in the folder above the one that holds TO, where a gateway looking for its
# plugins does not look (Hermes reads every folder beside its plugins, hidden ones too), and put in
# place by renaming. Where mv can exchange two folders in one step (GNU coreutils 9.5 and later, on a
# file system that can), a kill leaves the old folder or the new one. Elsewhere, as on macOS, the old
# one is moved aside first, and a kill between the two renames leaves neither (docs/move.md).
replace_dir() {
    local from=$1 to=$2 above new old
    if [ "$DRY_RUN" -eq 1 ]; then
        if [ -n "$from" ]; then say "would copy $from to $to"; else say "would remove $to"; fi
        return 0
    fi
    above=$(dirname "$(dirname "$to")")
    new="$above/.marmot-agent-$(basename "$to").new"
    old="$above/.marmot-agent-$(basename "$to").old"
    act rm -rf "$new" "$old" || return 1
    act mkdir -p "$(dirname "$to")" || return 1
    if [ -n "$from" ]; then act cp -R "$from" "$new" || return 1; fi
    if [ -n "$from" ] && [ -e "$to" ] && act mv -T --exchange "$new" "$to" 2>/dev/null; then
        # The old folder is where the new one was.
        act rm -rf "$new"
    else
        if [ -e "$to" ]; then act mv "$to" "$old" || return 1; fi
        if [ -n "$from" ]; then act mv "$new" "$to" || return 1; fi
        act rm -rf "$old"
    fi
}

# What every run does as it ends, with its status: puts back a plugin change and an upgrade that did
# not end, and removes what it staged. A change the upgrade lists goes back with the others it lists,
# in their order (upgrade_put_back_changes): put back first, the copies of another listed after it, of
# what this change left, would go back over the older ones.
finish() {
    local status=$1
    set +e
    trap '' HUP INT TERM
    if [ "$status" -ne 0 ] && [ "${MA_UPGRADING:-0}" -eq 1 ]; then
        change_restore || warn "the plugin change in $MA_UPGRADE could not be put back in its place"
    fi
    if [ "$status" -ne 0 ] && [ "${MA_CHANGING:-0}" -eq 1 ]; then
        MA_CHANGING=0
        if [ "${MA_UPGRADING:-0}" -eq 1 ] && [ -f "$MA_UPGRADE/plugin-changes" ] &&
            grep -qxF "$MA_CHANGE" "$MA_UPGRADE/plugin-changes"; then
            MA_CHANGE_STATUS=$status
        else
            change_put_back "$status"
        fi
    fi
    if [ "$status" -ne 0 ] && [ "${MA_UPGRADING:-0}" -eq 1 ]; then
        MA_UPGRADING=0
        upgrade_put_back
    fi
    if [ "$status" -ne 0 ] && [ "${MA_PREPARED:-0}" -eq 1 ] && declare -F harness_put_back >/dev/null; then
        harness_put_back
    fi
    if [ -n "$MA_WORK" ]; then rm -rf "$MA_WORK"; fi
    if [ "$MA_STAGED" -eq 1 ]; then rm -rf "$MA_STAGE"; fi
    if [ "$status" -ne 0 ]; then unlock_installs; fi
}

# ---------------------------------------------------------------------------------------------------
# One install at a time
#
# The core, the support files and the record of an upgrade are shared by every agent in a home, and a
# gateway's plugin by the agents of one gateway, so the installs in a home run one at a time: each holds
# a lock on $MA_DATA/install.lock from before it looks at what a run before it left until it ends, having
# finished or put back what it changed. The lock is on the file open as descriptor 9, and the system lets
# it go when the run ends, however it ends. It is taken with flock, which Linux has (util-linux), or,
# where there is none, as on macOS, with perl, which macOS ships, on the same descriptor. The commands a
# run starts do not get the descriptor (act). A run that made $MA_DATA for the lock and is refused
# takes the folder away again, the lock file with it, while it holds the lock (unlock_installs).
lock_installs() {
    [ "$DRY_RUN" -eq 0 ] || return 0
    # Taken once: opened again, the descriptor would let the lock go for a moment.
    [ "${MA_LOCKED:-0}" -eq 0 ] || return 0
    local said=0
    while :; do
        [ -d "$MA_DATA" ] || MA_MADE_DATA=1
        mkdir -p "$MA_DATA"
        exec 9>>"$MA_DATA/install.lock"
        if ! lock_fd -n; then
            [ "$said" -eq 1 ] || say "another install of marmot-agent in this home is under way; waiting for it to end"
            said=1
            lock_fd || die "waiting for the other install of marmot-agent in this home failed"
        fi
        # Held on the file in its place, or on one a refused run took away while this one waited: then
        # it is opened again, where a run after this one finds it.
        if [ /dev/fd/9 -ef "$MA_DATA/install.lock" ]; then break; fi
    done
    MA_LOCKED=1
}

# As a refused run ends, still holding the lock: the data folder it made for the lock, should that be
# all it holds, goes again with the lock file, so the run leaves nothing.
unlock_installs() {
    [ "${MA_LOCKED:-0}" -eq 1 ] && [ "${MA_MADE_DATA:-0}" -eq 1 ] || return 0
    [ -z "$(find "$MA_DATA" -mindepth 1 -maxdepth 1 ! -name install.lock -print 2>/dev/null)" ] || return 0
    rm -f "$MA_DATA/install.lock" && rmdir "$MA_DATA" 2>/dev/null
}

# Locks descriptor 9; given -n, fails at once if another run holds it.
lock_fd() {
    if program_path flock >/dev/null; then
        flock "$@" 9
    elif program_path perl >/dev/null; then
        perl -MFcntl=:flock -e 'open(my $f, ">&=", 9) or exit 2; flock($f, LOCK_EX | ($ARGV[0] ? LOCK_NB : 0)) or exit 1' \
            -- "${1:-}"
    else
        die "installing runs one install at a time, which needs flock or perl: install either, then run this again"
    fi
}

# ---------------------------------------------------------------------------------------------------
# Container images
#
# Pi's and Goose's scripts in $MA_LIB run the image tagged NAME. An install builds the new image as
# NAME:staging and tags it NAME only once the upgrade is done (main), so one that fails leaves NAME on the
# image the scripts it puts back were made for, and takes NAME:staging away. The image NAME named before
# is written into the upgrade's record before the tag moves, so that whatever puts the upgrade back puts
# the tag back with the scripts: this run's exit trap, the next run after a kill, or another harness's
# install (upgrade_put_back). The tag goes with the record, never apart from it: a run stopped once the
# upgrade is done leaves the new image with the new scripts.

# Prints the id of the image the tag NAME (NAME:latest) names, or nothing once docker has said there is
# none; fails when docker cannot say, as when it cannot be reached for a moment. `docker image inspect`
# fails both ways, so an image it does not find is taken as none only once `docker image ls` lists none.
image_id() {
    local image=$1 id listed
    if id=$(docker image inspect -f '{{.Id}}' "$image" 2>/dev/null 9>&-); then
        printf '%s\n' "$id"
        return 0
    fi
    listed=$(docker image ls --quiet "$image:latest" 2>/dev/null 9>&-) || return 1
    [ -z "$listed" ]
}

# Tags NAME:staging as NAME, the image NAME was noted first in the record, unless a run before this one
# that was killed noted it: that one is the image the scripts in the record were made for. Noted as none
# only once docker says there is none: noted so by mistake, putting the upgrade back would take the tag
# off the image the old scripts run. When docker cannot say, the install stops, and puts back the
# upgrade, under which the tag has not moved.
image_commit() {
    local image=$1 before=""
    if [ "$DRY_RUN" -eq 0 ] && [ ! -f "$MA_UPGRADE/image" ]; then
        before=$(image_id "$image") || die "docker could not say which image $image names; run this again once docker answers"
        printf '%s\t%s\n' "$image" "$before" >"$MA_UPGRADE/image.new"
        mv -f "$MA_UPGRADE/image.new" "$MA_UPGRADE/image"
    fi
    act docker tag "$image:staging" "$image"
    act docker rmi "$image:staging" >/dev/null || warn "docker could not take the tag $image:staging away"
}

# After a run that did not end: NAME:staging taken away. NAME itself is put back with the upgrade.
image_put_back() {
    docker rmi "$1:staging" >/dev/null 2>&1 9>&- || true
}

# Puts the tag an upgrade's record names back on the image it named, or takes it away when it named
# none, and takes the staging tag away; fails if docker cannot, or cannot say whether the tag is there,
# so that the record is kept for a run after this one.
image_put_back_from() {
    local record=$1 image before now
    [ -f "$record" ] || return 0
    IFS='	' read -r image before <"$record" || return 1
    if [ -n "$before" ]; then
        docker tag "$before" "$image" 9>&- || return 1
    else
        now=$(image_id "$image") || return 1
        if [ -n "$now" ]; then docker rmi "$image" >/dev/null 9>&- || return 1; fi
    fi
    docker rmi "$image:staging" >/dev/null 2>&1 9>&- || true
}

# ---------------------------------------------------------------------------------------------------
# Paths

# Prints the absolute path a path names, with links, `.` and `..` resolved as the system resolves them,
# without needing all of it to exist: the part that exists is resolved link by link, the rest by its
# words, since it holds no links. Fails on a loop of links.
canonical() {
    local rest=$1 out="" part target hops=0 missing=0
    case $rest in /*) ;; *) rest="$(pwd -P)/$rest" ;; esac
    while [ -n "$rest" ]; do
        part=${rest%%/*}
        if [ "$part" = "$rest" ]; then rest=""; else rest=${rest#*/}; fi
        case $part in
            "" | .) continue ;;
            ..)
                out=${out%/*}
                [ $missing -eq 0 ] || missing=$((missing - 1))
                continue
                ;;
        esac
        if [ $missing -eq 0 ] && [ -L "$out/$part" ]; then
            hops=$((hops + 1))
            [ $hops -le 40 ] || return 1
            target=$(readlink "$out/$part") || return 1
            case $target in /*) out="" ;; esac
            rest="$target${rest:+/$rest}"
            continue
        fi
        if [ $missing -gt 0 ] || [ ! -e "$out/$part" ]; then missing=$((missing + 1)); fi
        out="$out/$part"
    done
    printf '%s' "${out:-/}"
}

# Whether a path is a folder or inside it; both canonical.
inside() {
    case "${1%/}/" in "${2%/}"/*) return 0 ;; esac
    return 1
}

# ---------------------------------------------------------------------------------------------------
# The core and the files beside it

# The files the connectors, the plugins and the containers need: the Claude Code driver, the OpenClaw
# plugin's source, from which it is built, the Hermes plugin, and Pi's and Goose's containers, for
# --container. The release archive holds these and the core; package.sh reads this list.
support_paths() {
    cat <<'EOF'
LICENSE
install
packages/claude-code/package.json
packages/claude-code/package-lock.json
packages/claude-code/src
packages/openclaw/package.json
packages/openclaw/package-lock.json
packages/openclaw/tsconfig.json
packages/openclaw/tsconfig.build.json
packages/openclaw/openclaw.plugin.json
packages/openclaw/src
python/marmot_hermes
tools/pi
tools/goose
EOF
}

# Copies the support files from a tree (a checkout or an unpacked release) to a folder: all of them, or
# those under the parts named after the two folders.
copy_support() {
    local from=$1 to=$2 paths="" path part
    shift 2
    for path in $(support_paths); do
        [ $# -gt 0 ] || { paths="$paths $path"; continue; }
        for part in "$@"; do
            case $path in "$part" | "$part"/*) paths="$paths $path"; break ;; esac
        done
    done
    # shellcheck disable=SC2086 # the list is ours, one path per word
    (cd "$from" && tar -cf - --exclude node_modules --exclude __pycache__ --exclude '*.pyc' --exclude 'test_*.py' $paths) |
        (mkdir -p "$to" && cd "$to" && tar -xf -)
}

# The parts of $MA_LIB this harness uses: the install scripts, and its own folders. An install replaces
# these and nothing else, so another harness's files, and what its install prepared beside them, such
# as the Claude Code driver's packages, stay as they are.
lib_parts() {
    # shellcheck disable=SC2086 # one folder per word
    printf '%s\n' LICENSE install $HARNESS_SUPPORT
}

# Puts the staged core in place of $MA_CORE and the staged parts into $MA_LIB, once everything they
# need is prepared, keeping what each replaces until the install is done (upgrade_begin).
publish_support() {
    local part
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would install marmot-agent to $MA_CORE, keeping the one there until the agent is started"
        say "would put $(lib_parts | tr '\n' ' ')into $MA_LIB"
        return 0
    fi
    upgrade_begin
    if store_wanted; then store_keep; fi
    mkdir -p "$MA_BIN_DIR"
    install -m 0755 "$MA_STAGE/.core/marmot-agent" "$MA_CORE.new"
    mv -f "$MA_CORE.new" "$MA_CORE"
    say "installed $MA_CORE"
    for part in $(lib_parts); do
        [ -e "$MA_STAGE/$part" ] || continue
        upgrade_keep "$part"
        mkdir -p "$(dirname "$MA_LIB/$part")"
        mv "$MA_STAGE/$part" "$MA_LIB/$part"
    done
    rm -rf "$MA_STAGE"
    MA_STAGED=0
    say "installed $(lib_parts | tr '\n' ' ')in $MA_LIB"
}

# ---------------------------------------------------------------------------------------------------
# Upgrading the core and its support files
#
# Every agent starts the one core in $MA_CORE, with the support files in $MA_LIB, so an install that
# replaced one and then failed would leave every agent, not only its own, to start on the new core with
# the old files, or the other way round. So both are prepared first, in this run's own folder, and put in
# place together (publish_support); before that, upgrade_begin keeps a copy of the core in $MA_UPGRADE,
# a folder whose being there records that an upgrade is under way, with a list of every part of $MA_LIB
# the run replaces; the folder is made whole under another name and renamed into place. Each part is
# moved there as it is replaced. While the folder is there, a run that fails, or is stopped by HUP, INT
# or TERM, puts all of it back (finish). A run after one killed outright finds it, keeps nothing more of
# what is installed, which is by then partly that run's, finishes the upgrade, and, should it fail, puts
# back what the killed run found. The upgrade is done once the agent is set up and started, as a plugin
# change is (main): upgrade_end then renames the folder away, which ends it in one step, and removes it.
#
# Only one install runs in a home at a time (lock_installs), from before it looks for an upgrade left
# under way until it ends, so no run takes over, finishes or puts back an upgrade another is making.

# Starts an upgrade, or takes over one a run before this one left under way. One that replaces a part
# this run does not, as another harness's install does, or that holds a plugin change of another agent,
# or to another gateway, this run cannot finish: it puts it back first. So too one that holds a copy of an
# account store kept for another core than this run's (stores_ours), whose store that core may have moved
# forward already, out of reach of this run's.
upgrade_begin() {
    local making
    [ "${MA_UPGRADING:-0}" -eq 0 ] || return 0
    if [ -d "$MA_UPGRADE" ]; then
        change_restore || die "a plugin change left in $MA_UPGRADE could not be put back in its place"
        if ! upgrade_ours; then
            upgrade_put_back other || die "fix what stops it, then run this again"
        elif ! stores_ours; then
            upgrade_put_back core || die "fix what stops it, then run this again"
        else
            store_restores || die "an account home a run before this one was putting back could not be put back (above); run this again"
            MA_UPGRADING=1
            say "a run before this one stopped part way through installing marmot-agent; this one finishes that, and puts back the core and support files that run found if it fails"
            upgrade_list "$MA_UPGRADE"
            return 0
        fi
    fi
    MA_UPGRADING=1
    # Made under another name and renamed once whole, so a run killed before then has changed nothing.
    making="$MA_DATA/.upgrade.new"
    rm -rf "$making"
    mkdir -p "$making/lib"
    if [ -e "$MA_CORE" ]; then
        cp -p "$MA_CORE" "$making/marmot-agent"
        printf 'core\tpresent\n' >"$making/list"
    else
        printf 'core\tabsent\n' >"$making/list"
    fi
    upgrade_list "$making"
    mv "$making" "$MA_UPGRADE"
}

# Before a gateway's script reads whom its gateway serves and which plugin it has, once MA_GATEWAY names
# the gateway: the lock taken, and an upgrade a run before this one left that this one cannot finish put
# back, the plugin changes it holds with it, as upgrade_begin would. Read before, another agent's change
# half made would be taken for the gateway's own: its home named in the settings stops this run, which
# then never puts that change back (#180). One this run can finish is left to upgrade_begin. The lock
# is taken on a first install too, with no $MA_DATA yet, so that what this run reads of the gateway
# stays as it is until it ends: another agent's install into it waits (D-154). A first install that
# then stops at a check takes $MA_DATA away again (unlock_installs), and leaves nothing.
upgrade_recover() {
    [ "$DRY_RUN" -eq 0 ] || return 0
    lock_installs
    [ -d "$MA_UPGRADE" ] || return 0
    change_restore || die "a plugin change left in $MA_UPGRADE could not be put back in its place"
    upgrade_ours || upgrade_put_back other || die "fix what stops it, then run this again"
}

# Whether this run can finish the upgrade a run before it left: it replaces no part this run does not,
# and every plugin change it holds is this run's (changes_ours).
upgrade_ours() {
    local kind part
    while IFS='	' read -r kind part _; do
        [ "$kind" = part ] || continue
        # Not grep -q: one that leaves at the first match, with pipefail, fails lib_parts still writing.
        lib_parts | grep -xF "$part" >/dev/null || return 1
    done <"$MA_UPGRADE/list"
    changes_ours
}

# Whether each plugin change the upgrade's record lists that is not done, its record still there, is
# this run's to finish: this agent's, to the gateway this run changes, whose settings file its record
# names (MA_GATEWAY, which the harness's script sets). Ended by this run, another would be left under
# way after the upgrade, to put back copies older than what its gateway has by then (#180).
changes_ours() {
    local list="$MA_UPGRADE/plugin-changes" record
    [ -f "$list" ] || return 0
    while IFS= read -r record; do
        if [ -z "$record" ] || [ ! -f "$record" ]; then continue; fi
        [ "$record" = "$MA_AGENT_DIR/plugin-change" ] && [ -n "${MA_GATEWAY:-}" ] || return 1
        # Given in the environment, not with -v, which reads backslashes in it as escapes.
        GATEWAY=$MA_GATEWAY awk -F '\t' '$2 == ENVIRON["GATEWAY"] { found = 1 } END { exit !found }' "$record" ||
            return 1
    done <"$list"
}

# Names in the list of the upgrade in a folder each part of $MA_LIB this run replaces that it does not
# name yet, as present or absent. Written beside the list and renamed over it, so the list names all
# of them before any is replaced, or none.
upgrade_list() {
    local folder=$1 part found more=""
    for part in $(lib_parts); do
        [ -z "$(upgrade_found "$part" "$folder")" ] || continue
        if [ -e "$MA_LIB/$part" ]; then found=present; else found=absent; fi
        more="${more}part	$part	$found
"
    done
    [ -n "$more" ] || return 0
    { cat "$folder/list" && printf '%s' "$more"; } >"$folder/list.new"
    mv -f "$folder/list.new" "$folder/list"
}

# Whether a part of $MA_LIB was there before the upgrade, as its list says (that of the upgrade in the
# folder given second, or $MA_UPGRADE): present, absent, or nothing when the list does not name it.
upgrade_found() {
    awk -F '\t' -v part="$1" '$1 == "part" && $2 == part { print $3; exit }' "${2:-$MA_UPGRADE}/list"
}

# Clears the way for a new copy of a part of $MA_LIB, which the upgrade's list names: the one there
# before the upgrade is moved into $MA_UPGRADE; what a run before this one put there is removed.
upgrade_keep() {
    local part=$1 kept="$MA_UPGRADE/lib/$1" found
    found=$(upgrade_found "$part")
    if [ -e "$kept" ] || [ "$found" = absent ]; then
        rm -rf "${MA_LIB:?}/${part:?}"
    elif [ -e "$MA_LIB/$part" ]; then
        mkdir -p "$(dirname "$kept")"
        mv "$MA_LIB/$part" "$kept"
    fi
}

# Removes the folders above a path, up to $MA_LIB, that are left empty.
empty_above() {
    local folder
    folder=$(dirname "$1")
    while [ "$folder" != "$MA_LIB" ] && rmdir "$folder" 2>/dev/null; do folder=$(dirname "$folder"); done
}

# Ends an upgrade: what it replaced is no longer needed. A change to the gateway's plugin this run
# makes ends with it, in the same step: its record is moved into the upgrade's first, with where it goes
# back to, so that the rename that ends the one ends the other. Ended apart, a run stopped between the
# two would put back the old plugin over the new core, or the old core under the new plugin. The copies
# of our own plugin it names are removed after (change_end). The copies of the account stores it kept go
# with the record (D-159); another agent's, kept by a run killed before it started that agent again, is
# left stopped, and how to start it is said.
upgrade_end() {
    if [ "$DRY_RUN" -eq 0 ] && [ "${MA_CHANGING:-0}" -eq 1 ] && [ -f "$MA_CHANGE" ]; then
        MA_OWN_COPIES=$(own_copies)
        printf '%s\n' "$MA_CHANGE" | put "$MA_UPGRADE/plugin-change.path" 600
        mv -f "$MA_CHANGE" "$MA_UPGRADE/plugin-change"
    fi
    if [ "$DRY_RUN" -eq 0 ]; then
        store_hints "$(store_records others)"
        upgrade_retire
    fi
    MA_UPGRADING=0
    [ "$DRY_RUN" -eq 1 ] || rm -rf "$MA_DATA/.upgrade.done.$$"
}

# A change to a gateway's plugin moved into an upgrade's record as the upgrade ended, by a run stopped
# before the record was gone, goes back to its place: the upgrade and the change are both under way
# again, as they were, for whatever finishes or puts them back.
change_restore() {
    local to
    [ -f "$MA_UPGRADE/plugin-change" ] || return 0
    to=$(cat "$MA_UPGRADE/plugin-change.path") && [ -n "$to" ] && mv -f "$MA_UPGRADE/plugin-change" "$to"
}

# Takes the record of an upgrade out of its place in one step, under a name a later run removes
# (install_core) should this one be stopped before it has: a record half removed is never read.
upgrade_retire() {
    rm -rf "$MA_DATA/.upgrade.done.$$" && mv "$MA_UPGRADE" "$MA_DATA/.upgrade.done.$$"
}

# Puts back each plugin change the upgrade's record lists whose record is still there, the last first, as
# that one's copies are of what the one before it left, and removes its record; prints the agents'
# folders of those it put back, one a line, and fails if any could not be. One that could not stops the
# rest, so that no change before it puts back its older copies while it is still to go back. One this run
# was making goes back in its turn, saying what happened as change_put_back says it (finish). The change
# moved into the upgrade's record as it ended is back in its place by now (change_restore), unless that
# failed.
upgrade_put_back_changes() {
    local list="$MA_UPGRADE/plugin-changes" record own ok=0
    [ -f "$list" ] || return 0
    while IFS= read -r record; do
        if [ -z "$record" ] || [ ! -f "$record" ]; then continue; fi
        if [ -n "${MA_CHANGE_STATUS:-}" ] && [ "$record" = "$MA_CHANGE" ]; then
            change_put_back "$MA_CHANGE_STATUS" >&2 || { ok=1 && break; }
            continue
        fi
        own=$(own_copies "$record")
        if put_back_change "$record" >&2 && rm -f "$record"; then
            drop_copies "$own"
            dirname "$record"
        else
            ok=1
            break
        fi
    done < <(awk '{ line[NR] = $0 } END { for (i = NR; i > 0; i--) print line[i] }' "$list")
    return "$ok"
}

# After an upgrade that did not end: the account stores it kept, the plugin changes it lists, the core
# and the parts of $MA_LIB there before it put back, a part that was not there removed, and what happened
# said. The agents whose stores it put back are left stopped, and how to start them is said once it is
# gone (D-159). A store that cannot go back leaves the rest as it is, and the record kept. Given "other",
# or "core" (upgrade_begin), the upgrade is one a run before this one left, which this one puts back
# before it starts, and the status says whether it could.
upgrade_put_back() {
    local why=${1:-} kind part found ok=0 plugins record records put="" hints who
    # Gone already: upgrade_end took it away, and the upgrade is done.
    [ -f "$MA_UPGRADE/list" ] || return 0
    who="a run of another installer, or for another agent, before this one"
    [ "$why" != core ] || who="a run before this one, with another marmot-agent core, which kept a copy of an account home for that core,"
    records=$(store_records)
    while IFS= read -r record; do
        [ -n "$record" ] || continue
        if store_put_back "$record"; then
            put="$put$record
"
        else
            ok=1
            break
        fi
    done <<EOF
$records
EOF
    # A store that could not go back is left as the new core made it, and so is the rest: the core and
    # support files, the plugin changes and the image, with the record, for a run once nothing holds the
    # home. Put back, the older core would be left with a store it cannot open.
    if [ "$ok" -eq 1 ]; then
        if [ -n "$why" ]; then
            printf 'error: %s\n' "$who stopped part way through installing marmot-agent, and an account home it \
kept a copy of could not be put back (above), so the core and support files are left as they are, the new ones, \
and what that run found is in $MA_UPGRADE. Run this again once nothing holds the agent's account home" >&2
            return 1
        fi
        printf 'error: %s\n' "the install did not finish, and an account home could not be put back as it was before \
it (above), so the marmot-agent core and support files are left as they are, the new ones, and those installed \
before it are in $MA_UPGRADE. Run this again once nothing holds the agent's account home: it finishes the \
install, or puts them back should it fail" >&2
        return 0
    fi
    plugins=$(upgrade_put_back_changes) || ok=1
    # A change change_restore could not put back in its place is in the record still, which is kept.
    [ ! -f "$MA_UPGRADE/plugin-change" ] || ok=1
    if [ -n "$plugins" ]; then
        say "put back the gateway plugin that upgrade changed, for the agent in $(printf '%s' "$plugins" | tr '\n' ' ')"
    fi
    while IFS='	' read -r kind part found; do
        case $kind in
            core)
                if [ "$part" = present ]; then
                    { cp -p "$MA_UPGRADE/marmot-agent" "$MA_CORE.new" && mv -f "$MA_CORE.new" "$MA_CORE"; } || ok=1
                else
                    rm -f "$MA_CORE" || ok=1
                fi
                ;;
            part)
                if [ -e "$MA_UPGRADE/lib/$part" ]; then
                    { rm -rf "${MA_LIB:?}/${part:?}" && mv "$MA_UPGRADE/lib/$part" "$MA_LIB/$part"; } || ok=1
                elif [ "$found" = absent ]; then
                    rm -rf "${MA_LIB:?}/${part:?}" || ok=1
                    empty_above "$MA_LIB/$part"
                fi
                # Otherwise the part there before was not moved yet, and is still in place.
                ;;
        esac
    done <"$MA_UPGRADE/list"
    image_put_back_from "$MA_UPGRADE/image" || ok=1
    # Read before the record goes.
    hints=$(store_hints "$put")
    if [ -n "$why" ]; then
        if [ "$ok" -eq 0 ] && upgrade_retire; then
            rm -rf "$MA_DATA/.upgrade.done.$$"
            say "$who stopped part way through installing marmot-agent; this put back the core and support files that run found"
            [ -z "$hints" ] || say "$hints"
            return 0
        fi
        printf 'error: %s\n' "$who stopped part way \
through installing marmot-agent, and putting back the core and support files that run found failed (above); they \
are in $MA_UPGRADE" >&2
        return 1
    fi
    if [ "$ok" -eq 0 ] && upgrade_retire; then
        rm -rf "$MA_DATA/.upgrade.done.$$"
        printf 'error: %s\n' "the install did not finish, so this put back the marmot-agent core and support files \
that were installed before it, which every agent on this account starts with" >&2
        [ -z "$hints" ] || say "$hints" >&2
    else
        printf 'error: %s\n' "the install did not finish, and putting back the marmot-agent core and support files \
that were installed before it failed too (above); they are in $MA_UPGRADE. Run this again: it finishes the \
install, or puts them back should it fail" >&2
    fi
    return 0
}

# ---------------------------------------------------------------------------------------------------
# The account store, put back with the core (D-159)
#
# MDK upgrades an account home's store forward only, as a newer core first opens it, and an older core
# cannot open it after. So an upgrade that puts in a core other than the one there before, for an agent
# with a home, keeps a copy of the home before the new core is in place, and puts it back with the core
# should the upgrade be put back. The agent's services are stopped first, and stay stopped until
# start_services starts them on the new core, so no core of this agent's runs in between; put back, they
# stay stopped until the owner starts them, as the installer says how to (services_hint). The copy is
# taken while this run holds MDK's own lock on the home, so none has it open: the whole home as it was at
# one moment, MDK's databases with their -wal and -shm files, its keys and records, and the core's own
# state, but for the folder of the socket, which holds only the socket the core makes again as it starts.
# It is kept in the upgrade's record, in $MA_UPGRADE/stores/NAME: `where` names the home and the agent's
# services, `core` the checksum of the core it is kept for, and home/ is the copy, made under home.new and
# renamed once whole. Each file keeps its mode, and every folder made for it is the owner's alone. It goes
# with the record, as the upgrade ends or is put back, and nothing of it is printed. Only this agent's home
# is copied: another agent whose core restarts while the new one is in place, before the upgrade ends,
# moves its store forward uncopied; and with --no-start none is, so a core of this agent's that its service
# manager starts again meanwhile moves it forward uncopied too (D-159).

# The agent's services: its systemd units, or its launchd labels.
agent_services() {
    if [ "$OS" = Linux ]; then
        say "$MA_UNIT.service"
        [ "$HARNESS_KIND" != socket ] || say "$MA_UNIT-connector.service"
    else
        say "$MA_LABEL"
        [ "$HARNESS_KIND" != socket ] || say "$MA_LABEL.connector"
    fi
}

# Whether this run, about to put the new core in place, keeps the agent's store first: an upgrade that
# starts the agent, for an agent with a home, and a core that is not the one there before, or an agent
# moved over that no install has started yet (first_start), whatever core was there. One a run before
# this one began is taken over with the upgrade.
store_wanted() {
    [ "${MA_UPGRADING:-0}" -eq 1 ] && [ "$NO_SERVICE" -eq 0 ] && [ "$NO_START" -eq 0 ] || return 1
    [ ! -d "$MA_UPGRADE/stores/$NAME" ] || return 0
    [ -d "$MA_HOME" ] || return 1
    ! first_start || return 0
    ! { [ -f "$MA_UPGRADE/marmot-agent" ] && cmp -s "$MA_UPGRADE/marmot-agent" "$MA_STAGE/.core/marmot-agent"; }
}

# Whether the agent was moved over from today's connector and no install has started it since: its home
# is as the old connector left it, and the core this run starts is the first of ours to open its store,
# even if it is the one already installed for another agent (D-160). The run that starts it notes, once
# its upgrade has ended, that it did (moved_done).
first_start() {
    imported && [ ! -f "$MA_AGENT_DIR/moved" ]
}

# Notes that an agent moved over has been started by an install whose upgrade ended: its store is the
# installed core's from now on.
moved_done() {
    [ "$DRY_RUN" -eq 0 ] && [ "$NO_SERVICE" -eq 0 ] && [ "$NO_START" -eq 0 ] && first_start || return 0
    printf '%s\n' "the agent was moved over and first started by install/$HARNESS.sh" | put "$MA_AGENT_DIR/moved" 600
}

# Before the new core is put in place: the agent's services stopped, and the copy of its home made,
# once, under MDK's lock. The record names the home and the services before they are stopped, so that
# whatever puts the upgrade back stops them again and says how to start them.
store_keep() {
    local record="$MA_UPGRADE/stores/$NAME" services
    services=$(agent_services)
    if [ ! -d "$record" ]; then
        rm -rf "$record.new"
        { (umask 077 && mkdir -p "$record.new" && printf '%s\n' "$MA_HOME" "$services" >"$record.new/where") &&
            { ! first_start || : >"$record.new/moved"; } &&
            mv "$record.new" "$record"; } || die "the record of the agent's account home could not be written in $MA_UPGRADE"
    fi
    # shellcheck disable=SC2086 # one service a line, without spaces
    services_stop $services ||
        die "the agent's services did not stop, so its account home was not copied: stop them, then run this again"
    [ ! -d "$record/home" ] || return 0
    lease_home "$MA_HOME" ||
        die "the agent's core still has its account home $MA_HOME open after its service was stopped: stop it, then run this again"
    rm -rf "$record/home.new"
    if ! { printf '%s\n' "$MA_CORE_SUM" >"$record/core" && copy_home "$MA_HOME" "$record/home.new" &&
        mv "$record/home.new" "$record/home"; }; then
        exec 8>&-
        die "copying the agent's account home into $record failed (above)"
    fi
    exec 8>&-
    say "kept a copy of the agent's account home, until it is started on the new core"
}

# Stops the agent's services named; one that is not running, or not there, is left as it is.
services_stop() {
    local service state
    [ $# -gt 0 ] || return 0
    # Each must be stopped, or not there, afterwards: a stop can fail, and a unit set to start again on
    # its own could start a core on the store as it is copied or put back (D-159). Whether the stop
    # command failed says nothing of a unit that is not there, so the state is read instead.
    if [ "$OS" = Linux ]; then
        systemctl --user stop "$@" >/dev/null 2>&1 9>&- || true
        for service in "$@"; do
            state=$(systemctl --user show -p ActiveState --value "$service" 2>/dev/null 9>&-) || return 1
            case $state in "" | inactive | failed) ;; *) return 1 ;; esac
        done
    else
        for service in "$@"; do
            launchctl bootout "gui/$(id -u)/$service" >/dev/null 2>&1 9>&- || true
            ! launchctl print "gui/$(id -u)/$service" >/dev/null 2>&1 9>&- || return 1
        done
    fi
}

# Takes the services of an agent moved over out of what starts at login, as they were before its first
# start, which is being put back (D-160): enabled, they would start a core on the home as the import left
# it, uncopied, at the next login, before its installer runs again and enables them. On macOS, its
# LaunchAgents go, which that run writes again. One that cannot be taken out is said.
services_disable() {
    local service
    for service in "$@"; do
        if [ "$OS" = Linux ]; then
            systemctl --user disable "$service" >/dev/null 2>&1 9>&- || true
            [ "$(systemctl --user is-enabled "$service" 2>/dev/null 9>&-)" != enabled ] && continue
        else
            rm -f "$MA_LAUNCHD_DIR/$service.plist" && continue
        fi
        printf 'error: %s\n' "$service could not be taken out of what starts at login: disable it yourself before you log in again" >&2
    done
}

# Says how to start the services of the agent a store record names, which are left stopped: an install
# starts no agent because of an upgrade that was put back or that it did not make (D-159).
services_hint() {
    local service command=""
    # Its home is as the import left it, and its new service was never started on it: starting that would
    # open the store, uncopied, with whatever core is in place by then.
    if [ -f "$1/moved" ]; then
        say "the agent \"${1##*/}\" is left stopped, its account home as marmot-agent import left it; run its installer again once what failed is fixed, or go back to today's connector as docs/move.md says (\"Going back\")"
        return 0
    fi
    while IFS= read -r service; do
        [ -n "$service" ] || continue
        if [ "$OS" = Linux ]; then
            command="${command:-systemctl --user start} $service"
        else
            command="${command:+$command && }launchctl bootstrap gui/$(id -u) $(sh_quote "$MA_LAUNCHD_DIR/$service.plist")"
        fi
    done <<EOF
$(sed 1d "$1/where")
EOF
    [ -z "$command" ] ||
        say "the agent \"${1##*/}\" is left stopped; start it again with: $command"
}

# services_hint for each store record given, one a line.
store_hints() {
    local record
    while IFS= read -r record; do
        [ -z "$record" ] || services_hint "$record"
    done <<EOF
$1
EOF
}

# Waits, for up to a minute, for MDK's own lock on an account home, which a core holds for as long as it
# has the home open, and holds it on descriptor 8 until this run closes that: then no core has the home
# open, and none can open it meanwhile. Taken as MDK takes it, with flock on the file (fs-private), which
# is made, private, should it not be there. Fails if a core still holds it.
lease_home() {
    local lock="$1/.marmot-runtime.lock"
    [ ! -L "$lock" ] || return 1
    if [ ! -e "$lock" ]; then (umask 077 && : >>"$lock") || return 1; fi
    exec 8>>"$lock" || return 1
    if program_path flock >/dev/null; then
        flock -w 60 8 9>&- && return 0
    elif program_path perl >/dev/null; then
        perl -MFcntl=:flock -e 'open(my $f, ">&=", 8) or exit 2;
            for (1 .. 600) { exit 0 if flock($f, LOCK_EX | LOCK_NB); select(undef, undef, undef, 0.1) } exit 1' \
            9>&- && return 0
    fi
    exec 8>&-
    return 1
}

# The entries of a folder, one a line, but for MDK's lock and the folder of the socket.
home_entries() {
    local entry
    for entry in "$1"/* "$1"/.[!.]* "$1"/..?*; do
        [ -e "$entry" ] || [ -L "$entry" ] || continue
        case ${entry##*/} in .marmot-runtime.lock | socket) continue ;; esac
        printf '%s\n' "$entry"
    done
}

# Copies an account home into a folder, the owner's alone, each file with its mode and times, but for
# MDK's lock and the folder of the socket; the folder is made unless it is there.
copy_home() {
    local from=$1 to=$2 entry
    (
        umask 077
        [ -d "$to" ] || mkdir "$to" || exit 1
        while IFS= read -r entry; do
            [ -z "$entry" ] || cp -pR "$entry" "$to/" || exit 1
        done <<EOF
$(home_entries "$from")
EOF
        chmod "$(file_mode "$from")" "$to"
    )
}

# Puts a copy of an account home back into the home, in place, under MDK's lock: what the home holds is
# removed, but for that lock, and the copy put in. The home and its lock stay where they are, so a core
# that waits for the lock finds the home whole. A restore under way is marked in the record, and finished
# by whatever reads the record next (upgrade_begin, upgrade_put_back), before anything else.
store_put_back() {
    local record=$1 home entry ok=0
    home=$(sed -n 1p "$record/where") && [ -n "$home" ] || return 1
    # shellcheck disable=SC2046 # one service a line, without spaces
    if ! services_stop $(sed 1d "$record/where"); then
        printf 'error: %s\n' "the services of the agent whose account home is $home did not stop, so the home could not be put back as it was before the upgrade" >&2
        return 1
    fi
    # shellcheck disable=SC2046 # one service a line, without spaces
    [ ! -f "$record/moved" ] || services_disable $(sed 1d "$record/where")
    [ -d "$record/home" ] || return 0
    (umask 077 && mkdir -p "$home") || return 1
    if ! lease_home "$home"; then
        printf 'error: %s\n' "something still holds the account home $home open, so it could not be put back as it was before the upgrade" >&2
        return 1
    fi
    if : >"$record/restoring"; then
        while IFS= read -r entry; do
            [ -z "$entry" ] || rm -rf "$entry" || ok=1
        done <<EOF
$(home_entries "$home")
EOF
        [ "$ok" -eq 1 ] || copy_home "$record/home" "$home" || ok=1
        [ "$ok" -eq 1 ] || rm -f "$record/restoring" || ok=1
    else
        ok=1
    fi
    exec 8>&-
    [ "$ok" -eq 1 ] || say "put back the account home $home as it was before the upgrade"
    return "$ok"
}

# Finishes each restore of a home a run before this one was killed making, before this run uses the
# home; fails if one could not be finished.
store_restores() {
    local record
    while IFS= read -r record; do
        [ -z "$record" ] || [ ! -f "$record/restoring" ] || store_put_back "$record" || return 1
    done <<EOF
$(store_records)
EOF
}

# Whether each copy of a store the upgrade's record holds was kept for the core this run installs
# (MA_CORE_SUM, which install_core reads), which may have moved that store forward since: taken over by
# another core, the upgrade would end with the copy gone, and that core on a store it cannot open.
stores_ours() {
    local record
    [ -n "${MA_CORE_SUM:-}" ] || return 1
    while IFS= read -r record; do
        if [ -z "$record" ] || [ ! -d "$record/home" ]; then continue; fi
        [ -f "$record/core" ] && [ "$(cat "$record/core")" = "$MA_CORE_SUM" ] || return 1
    done <<EOF
$(store_records)
EOF
}

# The records of the stores the upgrade kept, one a line; given "others", only other agents'.
store_records() {
    local record
    for record in "$MA_UPGRADE"/stores/*; do
        case $record in *.new) continue ;; esac
        [ -f "$record/where" ] || continue
        if [ "${1:-}" = others ] && [ "$record" = "$MA_UPGRADE/stores/$NAME" ]; then continue; fi
        printf '%s\n' "$record"
    done
}

release_target() {
    case "$OS:$MA_MACHINE" in
        Linux:x86_64 | Linux:amd64) printf x86_64-unknown-linux-gnu ;;
        Linux:aarch64 | Linux:arm64) printf aarch64-unknown-linux-gnu ;;
        Darwin:arm64 | Darwin:aarch64) printf aarch64-apple-darwin ;;
        *) die "there is no release of marmot-agent for $OS on $MA_MACHINE; build it with --from-source" ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

download() {
    curl --proto '=https,file' --tlsv1.2 --fail --silent --show-error --location \
        --connect-timeout 15 --max-time 600 --retry 3 --output "$2" "$1"
}

# Reads the first matching string field from a JSON file (such as latest.json, package.json, or schema.json).
json_file_field() {
    [ -f "$1" ] || return 0
    awk -v k="$2" '
        {
            pat = "\"" k "\"[ \t\r\n]*:[ \t\r\n]*\"[^\"]*\""
            if (match($0, pat)) {
                s = substr($0, RSTART, RLENGTH)
                sub(/^"[^"]*"[ \t\r\n]*:[ \t\r\n]*"/, "", s)
                sub(/"$/, "", s)
                print s
                exit
            }
        }
    ' "$1"
}

# Reads the connector version bundled inside a core tree, if present.
bundled_connector_version() {
    local tree=$1
    case ${CONNECTOR_TRACK:-} in
        claude-code) json_file_field "$tree/packages/claude-code/package.json" version ;;
        openclaw) json_file_field "$tree/packages/openclaw/package.json" version ;;
        hermes)
            [ -f "$tree/python/marmot_hermes/__init__.py" ] || return 0
            sed -n 's/^__version__[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' \
                "$tree/python/marmot_hermes/__init__.py" | head -n 1
            ;;
    esac
}

# Fetches up to 10 pages of GitHub Releases API JSON (100 releases/page) into $work/releases-*.json,
# following same-origin https Link: rel="next" headers, or one local file:// fixture.
fetch_release_pages() {
    local work=$1 api_url=$2 page=1 hdr next url origin
    case $api_url in
        https://*)
            case $api_url in *\?*) url="$api_url&per_page=100" ;; *) url="$api_url?per_page=100" ;; esac
            origin=${api_url#https://}
            origin="https://${origin%%/*}/"
            while [ -n "$url" ] && [ "$page" -le 10 ]; do
                hdr="$work/releases-$page.hdr"
                curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
                    --connect-timeout 15 --max-time 60 --retry 3 \
                    --dump-header "$hdr" --output "$work/releases-$page.json" "$url" 2>/dev/null || return 1
                next=$(awk '
                    /^[Ll][Ii][Nn][Kk]:/ {
                        s = $0
                        while (match(s, /<[^>]*>[ \t]*;[ \t]*rel="next"/)) {
                            u = substr(s, RSTART + 1, RLENGTH - 1)
                            sub(/>.*/, "", u)
                            print u
                            exit
                        }
                    }
                ' "$hdr" | tr -d '\r')
                case $next in
                    "$origin"*) url=$next ;;
                    *) url="" ;;
                esac
                page=$((page + 1))
            done
            ;;
        *)
            download "$api_url" "$work/releases-1.json" 2>/dev/null || return 1
            ;;
    esac
}

# Lists stable SemVer release tags for one connector track from one or more Releases API JSON files,
# sorted highest SemVer first. When min_version is a valid X.Y.Z, returns only strictly newer tags.
list_connector_tags() {
    local track=$1 min_version=$2
    shift 2
    [ $# -gt 0 ] || return 0
    awk -v prefix="${track}-v" -v min="$min_version" '
        function parse_semver(s, a) {
            if (split(s, a, ".") != 3) return 0
            return a[1] ~ /^(0|[1-9][0-9]*)$/ &&
                   a[2] ~ /^(0|[1-9][0-9]*)$/ &&
                   a[3] ~ /^(0|[1-9][0-9]*)$/
        }
        BEGIN {
            plen = length(prefix)
            has_min = parse_semver(min, m)
        }
        {
            line = $0
            while (match(line, /"tag_name"[ \t\r\n]*:[ \t\r\n]*"[^"]*"/)) {
                tok = substr(line, RSTART, RLENGTH)
                line = substr(line, RSTART + RLENGTH)
                sub(/^"tag_name"[ \t\r\n]*:[ \t\r\n]*"/, "", tok)
                sub(/"$/, "", tok)
                if (substr(tok, 1, plen) != prefix) continue
                ver = substr(tok, plen + 1)
                if (!parse_semver(ver, v)) continue
                if (has_min) {
                    if (v[1] + 0 < m[1] + 0) continue
                    if (v[1] + 0 == m[1] + 0 && v[2] + 0 < m[2] + 0) continue
                    if (v[1] + 0 == m[1] + 0 && v[2] + 0 == m[2] + 0 && v[3] + 0 <= m[3] + 0) continue
                }
                print ver
            }
        }
    ' "$@" 2>/dev/null |
        sort -u -t. -k1,1nr -k2,2nr -k3,3nr |
        sed "s/^/${track}-v/"
}

# Downloads one tarball and its SHA256SUMS, checks the checksum and member paths, and unpacks it.
# When soft_download=1 (discovery mode), a failed download warns and returns 2 instead of aborting;
# checksum mismatches and unsafe paths always fail hard.
fetch_tarball() {
    local base=$1 asset=$2 prefix=$3 work=$4 soft_download=${5:-0} expected actual member
    mkdir -p "$work"
    if ! download "$base/$asset" "$work/$asset"; then
        [ "$soft_download" -eq 1 ] && { warn "could not download $base/$asset"; return 2; }
        die "could not download $base/$asset"
    fi
    if ! download "$base/SHA256SUMS" "$work/SHA256SUMS"; then
        [ "$soft_download" -eq 1 ] && { warn "could not download $base/SHA256SUMS"; return 2; }
        die "could not download $base/SHA256SUMS"
    fi
    expected=$(awk -v a="$asset" '$2 == a || $2 == "*" a { print tolower($1); exit }' "$work/SHA256SUMS")
    [ ${#expected} -eq 64 ] || die "SHA256SUMS has no line for $asset"
    actual=$(sha256_of "$work/$asset")
    [ "$actual" = "$expected" ] || die "$asset does not match its checksum in SHA256SUMS"
    while IFS= read -r member; do
        case $member in
            /* | .. | ../* | */../* | */..) die "the archive holds an unsafe path" ;;
            "$prefix" | "$prefix/"*) ;;
            *) die "the archive holds a path outside $prefix/" ;;
        esac
    done < <(tar -tzf "$work/$asset")
    mkdir -p "$work/unpacked"
    tar -xzf "$work/$asset" -C "$work/unpacked"
}

# Downloads the core release for this machine, checks it against SHA256SUMS, and unpacks it into
# a folder; prints the unpacked tree. Also saves latest.json when present for socket protocol checks.
# The checksum catches a broken download, not a replaced one (Q-28).
fetch_release() {
    local work=$1 target asset base
    target=$(release_target)
    asset="marmot-agent-$target.tar.gz"
    if [ -n "$VERSION" ]; then base="$MA_RELEASES/download/$VERSION"; else base="$MA_RELEASES/latest/download"; fi
    fetch_tarball "$base" "$asset" "marmot-agent-$target" "$work/core" 0
    download "$base/latest.json" "$work/core-latest.json" 2>/dev/null || rm -f "$work/core-latest.json"
    [ -x "$work/core/unpacked/marmot-agent-$target/bin/marmot-agent" ] || die "the archive holds no bin/marmot-agent"
    printf '%s' "$work/core/unpacked/marmot-agent-$target"
}

# Downloads one standalone connector release (D-166, D-167), checks its socket protocol against the
# core's declared protocol, verifies SHA256SUMS and archive paths, and prints its unpacked tree.
fetch_connector() {
    local work=$1 tag=$2 core_protocol=$3 strict=$4
    local base="$MA_RELEASES/download/$tag" asset="marmot-agent-$CONNECTOR_TRACK.tar.gz"
    local cwork="$work/connector" connector_protocol="" rc=0
    [ -n "$core_protocol" ] || die "cannot verify $tag compatibility: marmot-agent declares no socket_protocol"
    rm -rf "$cwork"
    mkdir -p "$cwork"
    if download "$base/latest.json" "$cwork/latest.json" 2>/dev/null; then
        connector_protocol=$(json_file_field "$cwork/latest.json" socket_protocol)
    else
        rm -f "$cwork/latest.json"
    fi
    if [ -z "$connector_protocol" ]; then
        if [ "$strict" -eq 1 ]; then
            die "$tag declares no socket_protocol in latest.json (expected $core_protocol)"
        fi
        warn "skipping $tag (no socket_protocol in latest.json; marmot-agent speaks $core_protocol)"
        return 2
    fi
    if [ "$strict" -eq 0 ] && grep -Eq '"prerelease"[[:space:]]*:[[:space:]]*true([^A-Za-z0-9_]|$)' "$cwork/latest.json"; then
        warn "skipping prerelease $tag"
        return 2
    fi
    if [ "$core_protocol" != "$connector_protocol" ]; then
        if [ "$strict" -eq 1 ]; then
            die "$tag speaks $connector_protocol, but marmot-agent speaks $core_protocol"
        fi
        warn "skipping $tag ($connector_protocol; marmot-agent speaks $core_protocol)"
        return 2
    fi
    fetch_tarball "$base" "$asset" "marmot-agent-$CONNECTOR_TRACK" "$cwork" $((1 - strict)) || rc=$?
    [ "$rc" -eq 0 ] || return "$rc"
    [ -d "$cwork/unpacked/marmot-agent-$CONNECTOR_TRACK/$HARNESS_SUPPORT" ] ||
        die "the archive holds no $HARNESS_SUPPORT"
    printf '%s' "$cwork/unpacked/marmot-agent-$CONNECTOR_TRACK"
}

# Resolves an explicit --connector-version or discovers the newest compatible standalone connector
# release strictly newer than the bundled connector. Prints the unpacked connector tree, or nothing
# to keep the connector bundled in the core tree.
resolve_connector_tree() {
    local work=$1 core_protocol=$2 downloaded_core=$3 bundled_version=${4:-}
    [ -n "${CONNECTOR_TRACK:-}" ] || return 0
    if [ -n "$CONNECTOR_VERSION" ]; then
        program_path curl >/dev/null || die "downloading the connector release needs curl"
        [ -n "$core_protocol" ] || die "cannot verify $CONNECTOR_VERSION compatibility: marmot-agent declares no socket_protocol"
        say "downloading $CONNECTOR_TRACK connector $CONNECTOR_VERSION" >&2
        fetch_connector "$work" "$CONNECTOR_VERSION" "$core_protocol" 1
        return $?
    fi
    [ "$downloaded_core" -eq 1 ] && [ -n "$MA_RELEASES_API" ] && [ -n "$core_protocol" ] || return 0
    local tag rc=0 tree=""
    fetch_release_pages "$work" "$MA_RELEASES_API" || {
        warn "could not list $CONNECTOR_TRACK connector releases; keeping the bundled connector"
        return 0
    }
    while IFS= read -r tag; do
        [ -n "$tag" ] || continue
        case $tag in *[!A-Za-z0-9._+-]* | *..*) continue ;; esac
        rc=0
        tree=$(fetch_connector "$work" "$tag" "$core_protocol" 0) || rc=$?
        if [ "$rc" -eq 0 ] && [ -n "$tree" ]; then
            say "using standalone $CONNECTOR_TRACK connector release $tag" >&2
            printf '%s' "$tree"
            return 0
        fi
        if [ "$rc" -ne 2 ]; then
            return 1
        fi
    done <<EOF
$(list_connector_tags "$CONNECTOR_TRACK" "$bundled_version" "$work"/releases-*.json)
EOF
}

# Installs the core and the support files, from wherever they come from.
install_core() {
    local tree binary stage downloaded_core=0 core_protocol="" connector_tree=""
    if [ -n "$BINARY_FROM" ]; then
        [ -x "$BINARY_FROM" ] || die "--binary names no program"
        binary=$BINARY_FROM
        tree=$MA_ROOT
        say "installing marmot-agent from $BINARY_FROM"
    elif [ "$FROM_SOURCE" -eq 1 ]; then
        [ -f "$MA_ROOT/Cargo.toml" ] || die "--from-source needs a checkout of marmot-agents; this script is not in one"
        program_path cargo >/dev/null || die "--from-source needs cargo: install Rust from https://rustup.rs"
        [ -n "${ODIN:-}" ] && [ -x "${ODIN:-}" ] ||
            die "--from-source needs the pinned Odin: run bash tools/odin/install.sh <absolute folder> and set ODIN to the odin program there"
        say "building marmot-agent from source (this takes a while)"
        # shellcheck disable=SC2016 # expanded by that shell
        act sh -c 'cd "$1" && cargo build --locked --release -p marmot-agent' sh "$MA_ROOT"
        binary="$MA_ROOT/target/release/marmot-agent"
        tree=$MA_ROOT
    elif [ -z "$VERSION" ] && [ -x "$MA_ROOT/bin/marmot-agent" ]; then
        # This script is in an unpacked release: use what is beside it.
        binary="$MA_ROOT/bin/marmot-agent"
        tree=$MA_ROOT
        say "installing the marmot-agent beside this script"
    elif [ "$DRY_RUN" -eq 1 ]; then
        say "would download marmot-agent-$(release_target).tar.gz ${VERSION:-(latest)} from $MA_RELEASES and check it against SHA256SUMS"
        if [ -z "$CONNECTOR_VERSION" ] && [ -n "${CONNECTOR_TRACK:-}" ] && [ -n "$MA_RELEASES_API" ]; then
            say "would check $MA_RELEASES_API for a newer compatible $CONNECTOR_TRACK connector release"
        fi
        binary="(downloaded)"
        tree=$MA_ROOT
    else
        program_path curl >/dev/null || die "downloading the release needs curl"
        MA_WORK=$(mktemp -d)
        say "downloading marmot-agent ${VERSION:-(latest release)}"
        tree=$(fetch_release "$MA_WORK")
        binary="$tree/bin/marmot-agent"
        downloaded_core=1
    fi
    if [ "$DRY_RUN" -eq 1 ] && [ -n "$CONNECTOR_VERSION" ]; then
        say "would download marmot-agent-$CONNECTOR_TRACK.tar.gz ($CONNECTOR_VERSION) from $MA_RELEASES and check it against SHA256SUMS"
    fi
    owner_hex "$binary"
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would copy the support files of $HARNESS_TITLE to $MA_LIB"
        MA_STAGE=$MA_LIB
        return 0
    fi
    if [ -n "$CONNECTOR_VERSION" ] || { [ "$downloaded_core" -eq 1 ] && [ -n "${CONNECTOR_TRACK:-}" ]; }; then
        [ -n "${MA_WORK:-}" ] || MA_WORK=$(mktemp -d)
        if [ "$downloaded_core" -eq 1 ]; then
            [ ! -f "$MA_WORK/core-latest.json" ] || core_protocol=$(json_file_field "$MA_WORK/core-latest.json" socket_protocol)
        elif [ -n "$BINARY_FROM" ]; then
            local bin_dir
            bin_dir=$(dirname "$BINARY_FROM")
            if [ -f "$bin_dir/latest.json" ]; then
                core_protocol=$(json_file_field "$bin_dir/latest.json" socket_protocol)
            elif [ -f "$bin_dir/../latest.json" ]; then
                core_protocol=$(json_file_field "$bin_dir/../latest.json" socket_protocol)
            fi
        elif [ "$FROM_SOURCE" -eq 1 ]; then
            [ ! -f "$MA_ROOT/fixtures/socket/schema.json" ] || core_protocol=$(json_file_field "$MA_ROOT/fixtures/socket/schema.json" title)
        elif [ -f "$MA_ROOT/latest.json" ]; then
            core_protocol=$(json_file_field "$MA_ROOT/latest.json" socket_protocol)
        fi
        connector_tree=$(resolve_connector_tree "$MA_WORK" "$core_protocol" "$downloaded_core" "$(bundled_connector_version "$tree")")
    fi
    # Nothing is written before the checks above, and nothing is read that another install changes
    # before the lock is held: a gateway's script takes it sooner, before it reads the gateway
    # (upgrade_recover).
    lock_installs
    # The checksum of the core this run installs, which a copy of a store it keeps is kept for, and which
    # a copy an upgrade left under way holds must be kept for, for this run to take that upgrade over.
    MA_CORE_SUM=$(sha256_of "$binary")
    [ -n "$MA_CORE_SUM" ] || die "the checksum of $binary could not be read"
    # An upgrade a run before this one left under way is finished by this one, or put back should this
    # one fail before it is.
    if [ -d "$MA_UPGRADE" ]; then upgrade_begin; fi
    mkdir -p "$MA_DATA"
    # What a run killed outright staged, or left of an upgrade's record it was removing, is removed: a
    # folder named after a process that is gone.
    for stage in "$MA_DATA"/lib.new.* "$MA_DATA"/.upgrade.done.*; do
        [ -d "$stage" ] || continue
        kill -0 "${stage##*.}" 2>/dev/null || rm -rf "$stage"
    done
    # The core and the support files are prepared in a folder of this run's own, and put in place
    # together once the harness's are ready (publish_support); the exit trap removes the folder if the
    # run stops before then.
    MA_STAGE=$MA_DATA/lib.new.$$
    rm -rf "$MA_STAGE"
    MA_STAGED=1
    mkdir -p "$MA_STAGE/.core"
    install -m 0755 "$binary" "$MA_STAGE/.core/marmot-agent"
    # The core staged is the one the checksum names, which a copy of a store is kept for.
    [ "$(sha256_of "$MA_STAGE/.core/marmot-agent")" = "$MA_CORE_SUM" ] ||
        die "$binary changed while it was being installed: run this again"
    # shellcheck disable=SC2046 # one part per word
    copy_support "$tree" "$MA_STAGE" $(lib_parts)
    if [ -n "$connector_tree" ]; then
        rm -rf "${MA_STAGE:?}/$HARNESS_SUPPORT"
        (cd "$connector_tree" && tar -cf - "$HARNESS_SUPPORT") | (cd "$MA_STAGE" && tar -xf -)
    fi
}

# A new agent's owner, in hex: an npub is read by the core about to be installed, which checks it.
owner_hex() {
    case $OWNER in npub1* | NPUB1*) ;; *) return 0 ;; esac
    [ ! -f "$MA_SETTINGS" ] || return 0
    if [ -x "$1" ]; then
        OWNER=$("$1" key "$OWNER" 2>/dev/null) || die "--owner must be a public key in hex (64 characters) or an npub"
    else
        say "would read the owner's npub with marmot-agent key"
    fi
}

# ---------------------------------------------------------------------------------------------------
# The agent

write_settings() {
    if [ -f "$MA_SETTINGS" ]; then
        if [ "$SET_CWD" -eq 1 ] || [ -n "$SET_KEY" ]; then
            update_settings
        else
            say "keeping the settings in $MA_SETTINGS (edit them there, or remove the agent's folder to start again)"
        fi
        add_files_from
        return 0
    fi
    local relay first=1 relays=""
    while IFS= read -r relay; do
        [ -n "$relay" ] || continue
        if [ $first -eq 1 ]; then first=0; else relays="$relays, "; fi
        relays="$relays$(toml_str "$relay")"
    done <<EOF
$RELAYS
EOF
    {
        say "# The settings of the agent \"$NAME\", written by install/$HARNESS.sh. The installer keeps this"
        say "# file when it runs again, so edit it freely. What each setting means: crates/marmot-agent/src/config.rs."
        say "home = $(toml_str "$MA_HOME")"
        say "name = $(toml_str "$DISPLAY")"
        say "relays = [$relays]"
        say "owner = $(toml_str "$OWNER")"
        say "# Only the owner may prompt or approve until you say otherwise (D-010):"
        say "# prompt = { people = [\"<hex>\"] }   or \"everyone\""
        say "# approve = { people = [\"<hex>\"] }"
        if [ -n "$LOCAL_RELAY" ]; then
            say "white_noise = false"
            say "allow_loopback_relays = true"
        fi
        say ""
        harness_settings
    } | put "$MA_SETTINGS" 600
}

# Writes a work folder or a model key file given again into the settings that are kept, in place of
# the one they name, and changes nothing else in them. plan_settings_update has checked that it can.
update_settings() {
    local cwd_line="" old="" new=""
    [ "$SET_CWD" -eq 0 ] || cwd_line="cwd = $(toml_str "$CWD")"
    if [ -n "$SET_KEY" ]; then
        old=$(toml_str "$SET_KEY")
        new=$(toml_str "$MODEL_KEY")
    fi
    say "keeping the settings in $MA_SETTINGS, with the work folder or the model key file given now"
    CWD_LINE=$cwd_line OLD=$old NEW=$new awk '
        ENVIRON["CWD_LINE"] != "" && /^cwd = / { print ENVIRON["CWD_LINE"]; next }
        ENVIRON["OLD"] == "" { print; next }
        {
            out = ""; rest = $0; old = ENVIRON["OLD"]
            while ((i = index(rest, old)) > 0) {
                out = out substr(rest, 1, i - 1) ENVIRON["NEW"]
                rest = substr(rest, i + length(old))
            }
            print out rest
        }' "$MA_SETTINGS" | put "$MA_SETTINGS" 600
}

# Whether `marmot-agent import` wrote the agent's settings, from today's connector of the harness that
# follows, such as "Hermes install,", or of any (docs/move.md): its identity came with its home.
imported() {
    local line
    [ -f "$MA_SETTINGS" ] || return 1
    line=$(head -n 1 "$MA_SETTINGS")
    case $line in "# Imported by \`marmot-agent import\` from a ${1:-}"*) return 0 ;; esac
    return 1
}

# The install the import names in the first line of the settings it wrote, such as "a Codex install".
imported_from() {
    local line
    line=$(head -n 1 "$MA_SETTINGS")
    line=${line#"# Imported by \`marmot-agent import\` from "}
    printf '%s' "${line%%,*}"
}

# The account home the agent's settings name: the one in the agent's folder, unless they were imported
# or edited since. Only a plain path in quotes is read.
settings_home() {
    local value
    value=$(awk '/^[[:space:]]*\[/ { exit } /^home[[:space:]]*=/ { sub(/^home[[:space:]]*=[[:space:]]*/, ""); print; exit }' "$MA_SETTINGS")
    value=$(printf '%s' "$value" | sed -n -e 's/^"\([^"\\]*\)"[[:space:]]*\(#.*\)\{0,1\}$/\1/p' \
        -e "s/^'\([^']*\)'[[:space:]]*\(#.*\)\{0,1\}\$/\1/p")
    case $value in /*) printf '%s' "$value" ;; *) return 1 ;; esac
}

# Makes the agent's identity once; its public key is kept beside the settings to show again. An
# imported agent has one in its home already, and is never given another.
init_agent() {
    if [ -s "$MA_AGENT_DIR/public-key" ]; then
        say "the agent already has an identity"
        return 0
    fi
    if imported; then
        say "the agent keeps the identity it was imported with, in $MA_HOME"
        return 0
    fi
    # A home a core has opened holds an identity already, and init would open its store with the new core,
    # in an upgrade that keeps no copy of it, as with --no-start, or before the copy is kept (D-159).
    if [ -d "$MA_HOME" ] && [ -n "$(home_entries "$MA_HOME")" ]; then
        local why="the agent's account home $MA_HOME already holds an identity, but $MA_AGENT_DIR/public-key, which names it, is missing. Write the agent's public key into that file and run this again; or, to give the agent a new identity, move $MA_HOME away first"
        [ "$DRY_RUN" -eq 1 ] || die "$why"
        warn "a real run would stop here: $why"
        return 0
    fi
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would run: $MA_CORE init $MA_SETTINGS"
        return 0
    fi
    say "making the agent's identity and publishing it to its relays"
    local key
    key=$("$MA_CORE" init "$MA_SETTINGS") || die "marmot-agent init failed (see above). If it says the home already has an identity, write its public key into $MA_AGENT_DIR/public-key, or move $MA_HOME away to make a new one"
    printf '%s\n' "$key" | put "$MA_AGENT_DIR/public-key" 600
}

# The agent's environment, as KEY=value lines: the PATH this script runs with, so the harness finds
# the programs it finds now; the folder that holds every agent, which the container scripts refuse to
# mount, as check_cwd does; and what the harness adds.
service_env() {
    no_control "PATH" "$PATH"
    say "PATH=$PATH"
    say "MARMOT_AGENT_DATA=$MA_DATA"
    if declare -F harness_env >/dev/null; then harness_env; fi
}

# The environment file, which the launchers read, and the launchers: start, which runs the core, and
# connector, for a socket harness. The service runs them, and a person can run them the same way.
write_launchers() {
    local lines line content=""
    lines=$(service_env) || die "the agent's environment could not be made"
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        no_control "the environment" "$line"
        content="$content${line%%=*}=$(sh_quote "${line#*=}")
"
    done <<EOF
$lines
EOF
    printf '%s' "$content" | put "$MA_AGENT_DIR/service.env" 600
    {
        launcher_head "the core"
        say "exec $(sh_quote "$MA_CORE") run $(sh_quote "$MA_SETTINGS")"
    } | put "$MA_AGENT_DIR/start" 700
    [ "$HARNESS_KIND" = socket ] || return 0
    {
        launcher_head "the $HARNESS_TITLE connector"
        harness_connector
    } | put "$MA_AGENT_DIR/connector" 700
}

launcher_head() {
    say "#!/bin/sh"
    say "# Starts $1 of the agent \"$NAME\", with its environment. Written by install/$HARNESS.sh."
    say "set -eu"
    say "set -a"
    say ". $(sh_quote "$MA_AGENT_DIR/service.env")"
    say "set +a"
}

# ---------------------------------------------------------------------------------------------------
# Services

systemd_units() {
    {
        say "[Unit]"
        say "Description=marmot-agent $NAME ($HARNESS_TITLE)"
        say "Documentation=https://github.com/$MA_REPO"
        say ""
        say "[Service]"
        say "Type=exec"
        say "ExecStart=$(systemd_quote "$MA_AGENT_DIR/start")"
        say "WorkingDirectory=$(systemd_path "$MA_AGENT_DIR")"
        say "# Started again however it ends, as when its harness stops, unless systemd stopped it."
        say "Restart=always"
        say "RestartSec=5"
        say "TimeoutStopSec=30"
        say ""
        say "[Install]"
        say "WantedBy=default.target"
    } | put "$MA_SYSTEMD_DIR/$MA_UNIT.service" 644
    if [ "$HARNESS_KIND" = socket ]; then
        {
            say "[Unit]"
            say "Description=marmot-agent $NAME: the $HARNESS_TITLE connector"
            say "Documentation=https://github.com/$MA_REPO"
            say "BindsTo=$MA_UNIT.service"
            say "After=$MA_UNIT.service"
            say ""
            say "[Service]"
            say "Type=exec"
            say "ExecStart=$(systemd_quote "$MA_AGENT_DIR/connector")"
            say "WorkingDirectory=$(systemd_path "$MA_AGENT_DIR")"
            say "Restart=always"
            say "RestartSec=5"
            say "TimeoutStopSec=30"
            say ""
            say "[Install]"
            say "WantedBy=$MA_UNIT.service"
        } | put "$MA_SYSTEMD_DIR/$MA_UNIT-connector.service" 644
    fi
}

launchd_plist() {
    local label=$1 log=$2 arg
    shift 2
    say '<?xml version="1.0" encoding="UTF-8"?>'
    say '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
    say '<plist version="1.0">'
    say '<dict>'
    say "    <key>Label</key><string>$(xml_escape "$label")</string>"
    say '    <key>ProgramArguments</key>'
    say '    <array>'
    for arg in "$@"; do say "        <string>$(xml_escape "$arg")</string>"; done
    say '    </array>'
    say "    <key>WorkingDirectory</key><string>$(xml_escape "$MA_AGENT_DIR")</string>"
    say '    <key>RunAtLoad</key><true/>'
    say '    <key>KeepAlive</key><true/>'
    say '    <key>ThrottleInterval</key><integer>10</integer>'
    say "    <key>StandardOutPath</key><string>$(xml_escape "$log")</string>"
    say "    <key>StandardErrorPath</key><string>$(xml_escape "$log")</string>"
    say '</dict>'
    say '</plist>'
}

launchd_agents() {
    private_dir "$MA_AGENT_DIR/logs"
    launchd_plist "$MA_LABEL" "$MA_AGENT_DIR/logs/core.log" "$MA_AGENT_DIR/start" |
        put "$MA_LAUNCHD_DIR/$MA_LABEL.plist" 644
    if [ "$HARNESS_KIND" = socket ]; then
        launchd_plist "$MA_LABEL.connector" "$MA_AGENT_DIR/logs/connector.log" "$MA_AGENT_DIR/connector" |
            put "$MA_LAUNCHD_DIR/$MA_LABEL.connector.plist" 644
    fi
}

start_services() {
    local units="$MA_UNIT.service" label labels="$MA_LABEL"
    if [ "$HARNESS_KIND" = socket ]; then
        units="$units $MA_UNIT-connector.service"
        labels="$labels $MA_LABEL.connector"
    fi
    if [ "$OS" = Linux ]; then
        act systemctl --user daemon-reload
        # shellcheck disable=SC2086 # two unit names at most, without spaces
        act systemctl --user enable $units
        if [ "$NO_START" -eq 1 ]; then
            say "the service is set up and enabled, not started: systemctl --user start $MA_UNIT"
            return 0
        fi
        # shellcheck disable=SC2086
        act systemctl --user restart $units
    else
        if [ "$NO_START" -eq 1 ]; then
            say "the service is set up, not loaded: launchctl bootstrap gui/$(id -u) $MA_LAUNCHD_DIR/$MA_LABEL.plist"
            return 0
        fi
        for label in $labels; do
            if [ "$DRY_RUN" -eq 0 ]; then launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true; fi
            act launchctl bootstrap "gui/$(id -u)" "$MA_LAUNCHD_DIR/$label.plist"
        done
    fi
}

uninstall() {
    say "removing the service of the agent \"$NAME\""
    if [ "$OS" = Linux ]; then
        local unit
        for unit in "$MA_UNIT-connector.service" "$MA_UNIT.service"; do
            if [ -f "$MA_SYSTEMD_DIR/$unit" ]; then
                if program_path systemctl >/dev/null; then
                    act systemctl --user disable --now "$unit" || warn "systemctl could not stop $unit"
                fi
                act rm -f "$MA_SYSTEMD_DIR/$unit"
            fi
        done
        if program_path systemctl >/dev/null; then act systemctl --user daemon-reload || true; fi
    else
        local label
        for label in "$MA_LABEL.connector" "$MA_LABEL"; do
            if [ -f "$MA_LAUNCHD_DIR/$label.plist" ]; then
                act launchctl bootout "gui/$(id -u)/$label" || true
                act rm -f "$MA_LAUNCHD_DIR/$label.plist"
            fi
        done
    fi
    if declare -F harness_uninstall >/dev/null; then harness_uninstall; fi
    # A change a run killed outright left unfinished is given up, so that no later run puts back what
    # that run found over what the gateway has by then: also one it had moved into the upgrade's record.
    if [ "$DRY_RUN" -eq 0 ] && [ "$(cat "$MA_UPGRADE/plugin-change.path" 2>/dev/null)" = "$MA_AGENT_DIR/plugin-change" ]; then
        change_restore || warn "the plugin change in $MA_UPGRADE could not be put back in its place"
    fi
    if [ -f "$MA_AGENT_DIR/plugin-change" ]; then
        say "giving up the change to your $HARNESS_TITLE's plugin that a run before this one left unfinished, noted in $MA_AGENT_DIR/plugin-change"
        act rm -f "$MA_AGENT_DIR/plugin-change"
    fi
    say "kept the agent's folder, with its settings and identity: $MA_AGENT_DIR"
    say "kept $MA_CORE and $MA_LIB, which other agents may use"
}

# ---------------------------------------------------------------------------------------------------
# The command line

usage() {
    cat <<EOF
Usage: install/$HARNESS.sh [options]

Installs marmot-agent with $HARNESS_TITLE, makes the agent's identity, and runs it as a service of your
account (systemd --user on Linux, launchd on macOS). Running it again updates marmot-agent and the
service and keeps the agent's settings and identity. An install of today's connector moved over with
marmot-agent import, its settings written to the agent's folder, keeps those settings and the identity
in its old home, and is given none (docs/move.md).

Options:
  --owner KEY          your public key, in hex or as an npub: only you may prompt and approve (asked
                       when not given)
EOF
    [ "$HARNESS_KIND" = plugin ] || cat <<EOF
  --cwd FOLDER         the folder $HARNESS_TITLE works in (asked; default ~/marmot-agent/NAME)
EOF
    cat <<EOF
  --name NAME          the agent's name on this machine, for its folder and its service (default: $HARNESS)
  --display-name TEXT  the name people see in chats (default: $HARNESS_TITLE)
  --relay URL          a relay for the agent's account; may repeat (default: White Noise's relays)
  --local-relay URL    use only this relay on this machine, for a try (no White Noise relays)
EOF
    case " $HARNESS_OPTIONS " in *" container "*) cat <<EOF
  --container          run $HARNESS_TITLE in a throwaway container instead of the one on your PATH
                       (Linux only; needs Docker), on an OpenAI-compatible model endpoint
EOF
    esac
    case " $HARNESS_OPTIONS " in *" model-key "*) cat <<EOF
  --model-key FILE     with --container: the model endpoint's API key, in a file only you can read
                       (required)
EOF
    esac
    case " $HARNESS_OPTIONS " in *" model "*) cat <<EOF
  --model NAME         with --container: the model, as the endpoint names it (required)
EOF
    esac
    case " $HARNESS_OPTIONS " in *" model-url "*) cat <<EOF
  --model-url URL      with --container: an OpenAI-compatible endpoint, such as
                       https://models.example/v1 (required)
EOF
    esac
    case " $HARNESS_OPTIONS " in *" replace-plugin "*) cat <<EOF
  --replace-plugin     put our plugin in place of the Marmot plugin of today's connectors, if
                       $HARNESS_TITLE has that one, for an install moved over with its settings in
                       the agent's folder (see docs/move.md); the old one is kept in that folder
EOF
    esac
    case " $HARNESS_OPTIONS " in *" permission-mode "*) cat <<EOF
  --permission-mode M  Claude Code's permission mode (default: default, which asks in the chat)
EOF
    esac
    case " $HARNESS_OPTIONS " in *" goose-mode "*) cat <<EOF
  --goose-mode MODE    auto or approve (ask in the chat before each tool); by default Goose's own mode,
                       auto, which asks nothing
EOF
    esac
    if [ -n "${CONNECTOR_TRACK:-}" ]; then cat <<EOF
  --connector-version TAG
                       install this $CONNECTOR_TRACK connector release (default: latest compatible)
EOF
    fi
    cat <<EOF
  --version TAG        install this core release (default: the latest)
  --from-source        build marmot-agent from this checkout instead (needs cargo and ODIN)
  --binary PATH        install this marmot-agent program instead
  --no-service         set up no service
  --no-start           set up the service without starting it
  --uninstall          stop and remove the service, and what the install added to $HARNESS_TITLE;
                       keep the agent's folder and identity
  --dry-run            say what would be done, and show each file that would be written
  --yes                ask nothing; stop if a required value is missing
  -h, --help           show this

Environment: MARMOT_AGENT_RELEASES (where releases are, default $MA_RELEASES),
MARMOT_AGENT_RELEASES_API (releases index for standalone connectors),
XDG_DATA_HOME and XDG_CONFIG_HOME (where files go).
EOF
}

takes() {
    case " $HARNESS_OPTIONS " in *" $1 "*) return 0 ;; esac
    die "install/$HARNESS.sh takes no --$1"
}

# shellcheck disable=SC2034 # MODEL and MODEL_URL are read by the harness scripts
parse_args() {
    DRY_RUN=0 ASSUME_YES=0 NO_SERVICE=0 NO_START=0 UNINSTALL=0 FROM_SOURCE=0
    BINARY_FROM="" VERSION="" CONNECTOR_VERSION="" NAME=$HARNESS DISPLAY=$HARNESS_TITLE OWNER="" RELAYS="" LOCAL_RELAY="" CWD=""
    MODEL_KEY="" MODEL="" MODEL_URL="" PERMISSION_MODE="" GOOSE_MODE="" SET_CWD=0 SET_KEY=""
    CONTAINER="" REPLACE_PLUGIN=0 CWD_WHAT=""
    while [ $# -gt 0 ]; do
        case $1 in
            --owner | --cwd | --name | --display-name | --relay | --local-relay | --version | \
                --connector-version | --binary | --model-key | --model | --model-url | \
                --permission-mode | --goose-mode)
                [ $# -ge 2 ] || die "$1 needs a value"
                no_control "$1" "$2"
                case $1 in
                    --owner) OWNER=$2 ;;
                    --cwd) CWD=$2 ;;
                    --name) NAME=$2 ;;
                    --display-name) DISPLAY=$2 ;;
                    --relay) RELAYS="$RELAYS$2
" ;;
                    --local-relay) LOCAL_RELAY=$2 ;;
                    --version) VERSION=$2 ;;
                    --connector-version)
                        [ -n "${CONNECTOR_TRACK:-}" ] || die "install/$HARNESS.sh takes no --connector-version"
                        CONNECTOR_VERSION=$2
                        ;;
                    --binary) BINARY_FROM=$2 ;;
                    --model-key) takes model-key; MODEL_KEY=$2 ;;
                    --model) takes model; MODEL=$2 ;;
                    --model-url) takes model-url; MODEL_URL=$2 ;;
                    --permission-mode) takes permission-mode; PERMISSION_MODE=$2 ;;
                    --goose-mode) takes goose-mode; GOOSE_MODE=$2 ;;
                esac
                shift 2
                ;;
            --container) takes container; CONTAINER=1; shift ;;
            --replace-plugin) takes replace-plugin; REPLACE_PLUGIN=1; shift ;;
            --from-source) FROM_SOURCE=1; shift ;;
            --no-service) NO_SERVICE=1; shift ;;
            --no-start) NO_START=1; shift ;;
            --uninstall) UNINSTALL=1; shift ;;
            --dry-run) DRY_RUN=1; shift ;;
            --yes | --non-interactive) ASSUME_YES=1; shift ;;
            -h | --help) usage; exit 0 ;;
            *) usage >&2; die "unknown option: $1" ;;
        esac
    done
    case $NAME in
        "" | -* | *[!a-z0-9-]*) die "--name takes lowercase letters, digits and hyphens" ;;
    esac
    [ ${#NAME} -le 40 ] || die "--name is too long"
    [ -n "$DISPLAY" ] || die "--display-name must not be empty"
    if [ -n "$LOCAL_RELAY" ]; then
        [ -z "$RELAYS" ] || die "give --relay or --local-relay, not both"
        RELAYS=$LOCAL_RELAY
    fi
    [ -n "$RELAYS" ] || RELAYS=$MA_DEFAULT_RELAYS
    local count=0
    [ -z "$BINARY_FROM" ] || count=$((count + 1))
    [ "$FROM_SOURCE" -eq 0 ] || count=$((count + 1))
    [ -z "$VERSION" ] || count=$((count + 1))
    [ $count -le 1 ] || die "give one of --version, --from-source and --binary"
    if [ -n "$CONNECTOR_VERSION" ]; then
        case $CONNECTOR_VERSION in
            "${CONNECTOR_TRACK}-v"[0-9]*.*) ;;
            *) die "--connector-version must be a ${CONNECTOR_TRACK}-v<version> release tag" ;;
        esac
        case $CONNECTOR_VERSION in
            *[!A-Za-z0-9._+-]* | *..*) die "--connector-version holds an unsafe character" ;;
        esac
    fi
    if [ -n "$BINARY_FROM" ]; then BINARY_FROM=$(absolute "$BINARY_FROM"); fi
    if [ -n "$MODEL_KEY" ]; then MODEL_KEY=$(absolute "$MODEL_KEY"); fi
    if [ "$HARNESS_KIND" = plugin ] && [ -n "$CWD" ]; then
        die "$HARNESS_TITLE works in the workspace it manages itself, so it takes no --cwd"
    fi
    # What this run was given, before the saved choices fill in the rest.
    CWD_GIVEN=$CWD MODEL_KEY_GIVEN=$MODEL_KEY CONTAINER_GIVEN=$CONTAINER
}

# An absolute path, without needing the file to exist.
absolute() {
    case $1 in
        /*) printf '%s' "$1" ;;
        \~/*) printf '%s/%s' "$HOME" "${1#\~/}" ;;
        *) printf '%s/%s' "$(pwd -P)" "$1" ;;
    esac
}

set_paths() {
    OS=${MARMOT_AGENT_OS:-$(uname -s)}
    MA_MACHINE=${MARMOT_AGENT_MACHINE:-$(uname -m)}
    case $OS in Linux | Darwin) ;; *) die "marmot-agent installs on Linux and macOS only" ;; esac
    [ -n "${HOME:-}" ] || die "HOME is not set"
    MA_ROOT=$(cd "$MA_HERE/.." && pwd -P)
    MA_BIN_DIR=$HOME/.local/bin
    MA_CORE=$MA_BIN_DIR/marmot-agent
    # With links resolved: MDK refuses an account home whose path goes through a link.
    MA_DATA=$(canonical "${XDG_DATA_HOME:-$HOME/.local/share}/marmot-agent") || die "the data folder names a loop of links"
    MA_LIB=$MA_DATA/lib
    MA_UPGRADE=$MA_DATA/upgrade
    MA_AGENT_DIR=$MA_DATA/agents/$NAME
    MA_CHANGE=$MA_AGENT_DIR/plugin-change
    MA_SETTINGS=$MA_AGENT_DIR/settings.toml
    MA_HOME=$MA_AGENT_DIR/home
    MA_UNIT=marmot-agent-$NAME
    MA_LABEL=org.marmot.marmot-agent.$NAME
    MA_SYSTEMD_DIR=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
    MA_LAUNCHD_DIR=$HOME/Library/LaunchAgents
    no_control "the data folder" "$MA_DATA"
}

# What was chosen when the agent was installed, and is not in its settings: kept beside them, so that
# running the script again changes only what it is given.
CHOICES="CWD MODEL_KEY MODEL MODEL_URL PERMISSION_MODE GOOSE_MODE CONTAINER"

load_choices() {
    [ -f "$MA_AGENT_DIR/choices" ] || return 0
    local key value
    while IFS='=' read -r key value; do
        case " $CHOICES " in
            *" $key "*)
                # A value given on the command line wins. The value is only ever data.
                [ -n "${!key}" ] || printf -v "$key" '%s' "$value"
                ;;
        esac
    done <"$MA_AGENT_DIR/choices"
}

# A choice as saved by the last run, or nothing.
saved_choice() {
    [ -f "$MA_AGENT_DIR/choices" ] || return 0
    sed -n "s/^$1=//p" "$MA_AGENT_DIR/choices" | head -n 1
}

save_choices() {
    local key
    for key in $CHOICES; do
        say "$key=${!key}"
    done | put "$MA_AGENT_DIR/choices" 600
}

# Each agent is one harness's: the same --name from another harness's script would take over its
# identity and its service. Checked before anything is changed.
check_harness() {
    local owner=""
    if [ -f "$MA_AGENT_DIR/harness" ]; then
        owner=$(cat "$MA_AGENT_DIR/harness")
    elif imported "$HARNESS_TITLE install,"; then
        # Moved over from today's connector (docs/move.md): the settings name its home and identity (D-160).
        say "using the settings marmot-agent import wrote in $MA_SETTINGS"
    elif imported; then
        die "the settings in $MA_SETTINGS were imported from $(imported_from), not from a $HARNESS_TITLE install: run that harness's installer, or give another --name"
    elif [ -f "$MA_SETTINGS" ]; then
        die "the agent \"$NAME\" has settings but no record of its harness in $MA_AGENT_DIR/harness: give another --name, or write the harness's name, such as $HARNESS, there"
    fi
    [ -z "$owner" ] || [ "$owner" = "$HARNESS" ] ||
        die "the agent \"$NAME\" runs $owner; use install/$owner.sh for it, or give another --name"
    # The home is the one the settings name, which an imported agent keeps where it was.
    if [ -f "$MA_SETTINGS" ]; then
        MA_HOME=$(settings_home) ||
            die "$MA_SETTINGS names no account home this script can read: write it as home = \"/its/path\""
        no_control "the account home" "$MA_HOME"
    fi
}

# Asks for, or checks, what a new agent needs: its owner and its work folder.
settle_values() {
    load_choices
    [ -n "$PERMISSION_MODE" ] || PERMISSION_MODE=default
    case $GOOSE_MODE in "" | approve | auto) ;; *) die "--goose-mode takes approve or auto" ;; esac
    case $MODEL_URL in "" | http://?* | https://?*) ;; *) die "--model-url must start with http:// or https://" ;; esac
    if [ -f "$MA_SETTINGS" ]; then
        [ -z "$OWNER" ] || warn "the agent already has settings; --owner is ignored (edit $MA_SETTINGS)"
        # The settings that are kept say where the harness runs; a run again does not change that.
        if [ -n "$CONTAINER_GIVEN" ] && [ "$(saved_choice CONTAINER)" != 1 ]; then
            die "the agent \"$NAME\" runs the $HARNESS_TITLE on your PATH; to run it in a container, install another agent with --name, or remove $MA_AGENT_DIR to start again"
        fi
        [ "$HARNESS_KIND" != plugin ] || return 0
        local settings_cwd
        settings_cwd=$(sed -n 's/^cwd = "\(.*\)"$/\1/p' "$MA_SETTINGS" | head -n 1)
        # Read as written, so a TOML escape would name another folder to the core than the one checked here.
        case $settings_cwd in *\\*) die "the cwd in $MA_SETTINGS holds a backslash: write the folder without escapes" ;; esac
        # A harness the core drives works where its settings say, which may have been edited since.
        if [ "$HARNESS_KIND" = builtin ] && [ -z "$CWD_GIVEN" ] && [ -n "$settings_cwd" ]; then
            CWD=$settings_cwd
        fi
        [ -n "$CWD" ] || CWD=$settings_cwd
        if [ -z "$CWD" ]; then
            # The Claude Code connector's work folder is on its command line, not in the settings the import wrote.
            ! imported || die "give --cwd, the folder $HARNESS_TITLE works in (the import's --cwd): the settings marmot-agent import wrote do not say it"
            die "the agent has settings but no work folder is known: give --cwd"
        fi
        CWD=$(absolute "$CWD")
        # One kept from before, as one the import wrote (by default your home folder), is checked as --cwd is.
        if [ -z "$CWD_GIVEN" ] && [ "$HARNESS_KIND" = builtin ] && first_start; then
            CWD_WHAT="the work folder the import wrote in $MA_SETTINGS, $CWD (write another there as cwd),"
        elif [ -z "$CWD_GIVEN" ]; then
            CWD_WHAT="the agent's work folder, $CWD (give --cwd for another),"
        fi
        check_cwd
        plan_settings_update
        return 0
    fi
    if [ -z "$OWNER" ] && can_ask; then
        say "The owner is the only one who may prompt the agent and approve what it does."
        OWNER=$(ask "Your public key (hex or npub)" "")
    fi
    [ -n "$OWNER" ] || die "give your public key with --owner"
    owner_shape "$OWNER" || die "--owner must be a public key in hex (64 characters) or an npub"
    case $OWNER in npub1* | NPUB1*) ;; *) OWNER=$(printf '%s' "$OWNER" | tr '[:upper:]' '[:lower:]') ;; esac
    [ "$HARNESS_KIND" != plugin ] || return 0
    if [ -z "$CWD" ] && can_ask; then
        CWD=$(ask "The folder $HARNESS_TITLE works in" "$HOME/marmot-agent/$NAME")
    fi
    [ -n "$CWD" ] || CWD=$HOME/marmot-agent/$NAME
    CWD=$(absolute "$CWD")
    check_cwd
}

# A harness the core drives reads its work folder and its model key file from the settings, which a
# run again keeps: one given now in place of the one they name is written into them (update_settings),
# or, when the settings no longer say it where the installer wrote it, the run stops here, before
# anything is changed. A harness with a connector reads both from its launcher, written each time.
plan_settings_update() {
    [ "$HARNESS_KIND" = builtin ] || return 0
    local old
    if [ -n "$CWD_GIVEN" ] && ! grep -qxF -- "cwd = $(toml_str "$CWD")" "$MA_SETTINGS"; then
        # The settings of a move not yet done stay as the import wrote them (D-160).
        ! first_start ||
            die "$MA_SETTINGS names the work folder the import wrote; to change it, write it there as cwd, then run this again without --cwd"
        [ "$(grep -c '^cwd = ' "$MA_SETTINGS" || true)" = 1 ] ||
            die "$MA_SETTINGS has no single cwd line to change: write the work folder there yourself"
        SET_CWD=1
    fi
    if [ -n "$MODEL_KEY_GIVEN" ]; then
        old=$(saved_choice MODEL_KEY)
        if [ "$old" != "$MODEL_KEY" ]; then
            if [ -z "$old" ] || ! grep -qF -- "$(toml_str "$old")" "$MA_SETTINGS"; then
                die "$MA_SETTINGS does not name the model key file the agent was installed with: write $MODEL_KEY there yourself"
            fi
            SET_KEY=$old
        fi
    fi
}

# The work folder is mounted read-write into a harness's container, and a harness may read anything in
# it: so it must not hold the agents' keys or the model key, nor be inside the folder that holds the
# agents. Paths are compared with links, `.` and `..` resolved, and the work folder is kept that way.
check_cwd() {
    local key kept what=${CWD_WHAT:---cwd}
    no_control "$what" "$CWD"
    CWD=$(canonical "$CWD") || die "$what names a loop of links"
    if inside "$CWD" "$MA_DATA"; then die "$what must not be inside $MA_DATA, which holds the agents' keys"; fi
    if inside "$MA_DATA" "$CWD"; then die "$what must not hold $MA_DATA, which holds the agents' keys"; fi
    # An agent moved over keeps its home, and the import's backup of it, where they were, out of MA_DATA
    # (D-160): the harness reads anything in its work folder, so that holds neither, nor lies in one.
    if imported; then
        for kept in "$MA_HOME" "$(settings_backup || true)"; do
            [ -n "$kept" ] || continue
            kept=$(canonical "$kept") || die "$kept names a loop of links"
            if inside "$CWD" "$kept" || inside "$kept" "$CWD"; then
                die "$what must not hold or be inside $kept, which holds the agent's keys"
            fi
        done
    fi
    [ -n "$MODEL_KEY" ] || return 0
    key=$(canonical "$MODEL_KEY") || die "--model-key names a loop of links"
    if inside "$key" "$CWD"; then
        die "the model key file must not be inside the work folder, where $HARNESS_TITLE could read it"
    fi
    if inside "$key" "$MA_DATA"; then die "the model key file must not be inside $MA_DATA"; fi
}

# The folder a gateway's agent works in, from which the core may send the files it names (D-090), for
# a harness whose plugin states no folder: FILES_FROM, or none and FILES_WHY saying why. Never the home
# itself (D-099), nor a folder that holds or lies in the agents' data or the agent's home, which would
# stop the core, or in the backup of an imported agent's home, which the import leaves out too. Nor one that holds or lies in a folder of GATEWAY_CACHE, where the gateway keeps the
# files people sent it on any platform, or that holds one of GATEWAY_STATE, its own folder, which holds
# that cache. Provisional, for the owner's review.
choose_files_from() {
    local folder=$1 kept
    FILES_FROM="" FILES_WHY="" FILES_CHOSEN=1
    case $folder in
        "") FILES_WHY="its work folder is not known"; return 0 ;;
        \~ | \~/*) folder=$HOME${folder#\~} ;;
        /*) ;;
        *) FILES_WHY="its work folder is not a whole path"; return 0 ;;
    esac
    no_control "$HARNESS_TITLE's work folder" "$folder"
    folder=$(canonical "$folder") || { FILES_WHY="its work folder names a loop of links"; return 0; }
    if [ "$folder" = "$(canonical "$HOME")" ] || [ "$folder" = / ]; then
        FILES_WHY="it works in your whole home folder, which holds the agent's keys and more"
        return 0
    elif inside "$folder" "$MA_DATA" || inside "$MA_DATA" "$folder"; then
        FILES_WHY="its work folder holds or lies in $MA_DATA, which holds the agents' keys"
        return 0
    fi
    # An imported agent's home stays where the old connector had it, out of MA_DATA.
    kept=$(canonical "$MA_HOME") || kept=$MA_HOME
    if inside "$folder" "$kept" || inside "$kept" "$folder"; then
        FILES_WHY="its work folder holds or lies in $kept, the agent's home, which holds its keys"
        return 0
    fi
    if kept=$(settings_backup) && kept=$(canonical "$kept") &&
        { inside "$folder" "$kept" || inside "$kept" "$folder"; }; then
        FILES_WHY="its work folder holds or lies in $kept, the backup of the agent's home, which holds its keys"
        return 0
    fi
    for kept in ${GATEWAY_CACHE[@]+"${GATEWAY_CACHE[@]}"}; do
        kept=$(gateway_folder "$kept") || continue
        if inside "$folder" "$kept" || inside "$kept" "$folder"; then
            FILES_WHY="its work folder holds or lies in $kept, where $HARNESS_TITLE keeps the files people send it"
            return 0
        fi
    done
    for kept in ${GATEWAY_STATE[@]+"${GATEWAY_STATE[@]}"}; do
        kept=$(gateway_folder "$kept") || continue
        if inside "$kept" "$folder"; then
            FILES_WHY="its work folder holds $kept, where $HARNESS_TITLE keeps its own files and the files people send it"
            return 0
        fi
    done
    FILES_FROM=$folder
}

# The backup of the account home `marmot-agent import` took, as the settings it wrote name it in the
# comment at their top; fails for settings that name none, or not as a whole path.
settings_backup() {
    local value
    [ -f "$MA_SETTINGS" ] || return 1
    value=$(PREFIX="# The home before the import is backed up in " awk '
        /^[[:space:]]*\[/ { exit }
        index($0, ENVIRON["PREFIX"]) == 1 { print substr($0, length(ENVIRON["PREFIX"]) + 1); exit }' "$MA_SETTINGS")
    value=${value%.}
    case $value in
        *[[:cntrl:]]*) return 1 ;;
        /*) printf '%s' "$value" ;;
        *) return 1 ;;
    esac
}

# A folder the gateway named for itself, whole and canonical; fails for one that is not.
gateway_folder() {
    case $1 in
        *[[:cntrl:]]* | "") return 1 ;;
        \~ | \~/*) canonical "$HOME${1#\~}" ;;
        /*) canonical "$1" ;;
        *) return 1 ;;
    esac
}

# The settings' line for choose_files_from: the folder, or a comment saying how to name one.
files_from_settings() {
    if [ -n "$FILES_FROM" ]; then
        say "# The folder $HARNESS_TITLE works in: the agent may send the files in it to a chat (D-090)."
        say "files_from = [$(toml_str "$FILES_FROM")]"
    else
        say "# The agent sends no files until a folder is listed here (D-090): $FILES_WHY."
        say "# files_from = [\"/the/folder/$HARNESS_TITLE/writes/its/files/in\"]"
    fi
}

# Settings kept from an earlier run that list no folder, as those from before the installer chose one
# do, get the folder chosen now, as files_from_settings writes it, before their first table, where a
# top-level setting must be; nothing else in them changes. A files_from line there is the person's,
# and is left as it is (FILES_KEPT), except in an install moved over (add_gateway_folder). With no
# folder to list, the summary says how to name one.
add_files_from() {
    [ "$FILES_CHOSEN" -eq 1 ] || return 0
    if grep -Eq '^[[:space:]]*files_from[[:space:]]*=' "$MA_SETTINGS"; then
        FILES_KEPT=1
        if [ "$REPLACE_PLUGIN" -eq 1 ] && [ -n "$FILES_FROM" ]; then add_gateway_folder; fi
        return 0
    fi
    [ -n "$FILES_FROM" ] || return 0
    say "adding files_from to the settings in $MA_SETTINGS"
    FILES_LINES=$(files_from_settings) awk '
        !done && /^[[:space:]]*\[/ { print ENVIRON["FILES_LINES"]; print ""; done = 1 }
        { print }
        END { if (!done) { print ""; print ENVIRON["FILES_LINES"] } }' "$MA_SETTINGS" | put "$MA_SETTINGS" 600
}

# An install moved over from today's connector lists the gateway's folder, chosen as for a fresh
# install, after the old connector's media folders, which stay (D-119). Only the files_from line the
# import wrote is changed, known by the comment the import puts directly above it, and only while it is
# still one the import writes: one line, its strings separated by ", ". The import's comment is then
# replaced by one that names the folder added, by which a run again knows the line as the installer's:
# it changes nothing if the gateway works in that folder still (FILES_ALSO), and lists the new one in its
# place if not. A files_from line the installer does not know as the import's or its own is the
# person's: it is left as it is, and the summary says to add the folder (FILES_ADD). Every value is
# copied as it is, never through a pattern.
add_gateway_folder() {
    local folder found status number base
    folder=$(toml_str "$FILES_FROM")
    found=$(FOLDER=$folder MINE=$(gateway_folder_comment "") \
        IMPORTED="# Only the folders the old connector let files be sent from; never the working folder." awk '
        # How many TOML strings s is, basic or literal, each in one line, with ", " between them; -1 if
        # it is not that. listed says whether one of them is FOLDER as it is written.
        function strings(s,   i, n, c, q, from, count) {
            n = length(s)
            if (n == 0) return 0
            for (i = 1; ; i += 2) {
                q = substr(s, i, 1)
                if (q != "\"" && q != "\047") return -1
                for (from = i++; i <= n && (c = substr(s, i, 1)) != q; i++) if (q == "\"" && c == "\\") i++
                if (i > n) return -1
                count++
                if (substr(s, from, i - from + 1) == ENVIRON["FOLDER"]) listed = 1
                if (++i > n) return count
                if (substr(s, i, 2) != ", ") return -1
            }
        }
        /^[[:space:]]*\[/ { table = 1 }
        /^[[:space:]]*files_from[[:space:]]*=/ { lines++; if (!table) { line = $0; above = prev; at = NR } }
        { prev = $0 }
        END {
            open = "files_from = ["
            if (lines != 1 || at == 0 || index(line, open) != 1 || substr(line, length(line)) != "]") {
                print "other"; exit
            }
            list = substr(line, length(open) + 1, length(line) - length(open) - 1)
            # The line the import wrote: the folder goes after its own.
            if (above == ENVIRON["IMPORTED"]) {
                if (strings(list) < 0) print "other"
                else if (listed) print "same"
                else { print "import"; print at; print open list }
                exit
            }
            # The line this wrote before: the folder named in its comment is the last.
            mine = ENVIRON["MINE"]
            added = substr(above, length(mine) + 1)
            if (index(above, mine) != 1 || strings(added) != 1) { print "other"; exit }
            if (list == added) rest = ""
            else if (substr(list, length(list) - length(added) - 1) == ", " added) {
                rest = substr(list, 1, length(list) - length(added) - 2)
                if (strings(rest) < 1) { print "other"; exit }
            } else { print "other"; exit }
            if (added == ENVIRON["FOLDER"]) { print "same"; exit }
            print "mine"; print at; print open rest
        }' "$MA_SETTINGS")
    status=$(printf '%s\n' "$found" | sed -n 1p)
    number=$(printf '%s\n' "$found" | sed -n 2p)
    base=$(printf '%s\n' "$found" | sed -n 3p)
    case $status in
        import | mine) ;;
        same) FILES_ALSO=1; return 0 ;;
        *) FILES_ADD=1; return 0 ;;
    esac
    FILES_ALSO=1
    case $base in *\[) ;; *) base="$base, " ;; esac
    say "listing $FILES_FROM in files_from in $MA_SETTINGS"
    AT=$number COMMENT=$(gateway_folder_comment "$folder") LINE="$base$folder]" awk '
        NR == ENVIRON["AT"] - 1 { print ENVIRON["COMMENT"]; next }
        NR == ENVIRON["AT"] + 0 { print ENVIRON["LINE"]; next }
        { print }' "$MA_SETTINGS" | put "$MA_SETTINGS" 600
}

# The comment add_gateway_folder puts above the files_from line it changes, naming the folder it added,
# as a TOML string, at its end.
gateway_folder_comment() {
    printf '%s' "# The folders the old connector let files be sent from, then the one $HARNESS_TITLE works in, which the installer keeps (D-119): $1"
}

# The core opens the folders files_from lists when it starts, and one not there then allows nothing
# until it starts again: the folder listed is made first.
make_files_from() {
    if [ -z "$FILES_FROM" ] || [ -d "$FILES_FROM" ]; then return 0; fi
    if [ "$FILES_KEPT" -eq 1 ] && [ "$FILES_ALSO" -eq 0 ]; then return 0; fi
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would make folder $FILES_FROM"
        return 0
    fi
    mkdir -p "$FILES_FROM" ||
        warn "could not make $FILES_FROM: the agent sends no files from it until it is there and the agent starts again"
}

# What the summary says about the files the agent may send.
files_from_summary() {
    if [ "$FILES_ALSO" -eq 1 ]; then
        say "files_from in its settings lists the folders the agent sent files from before, and $FILES_FROM, where $HARNESS_TITLE works."
    elif [ "$FILES_ADD" -eq 1 ]; then
        say "files_from in its settings lists the folders the agent may send files from, as it did before;"
        say "add $FILES_FROM, where $HARNESS_TITLE works, for the agent to send the files it makes there."
    elif [ "$FILES_KEPT" -eq 1 ]; then
        say "files_from in its settings lists the folders the agent may send files from, as it did before."
    elif [ -n "$FILES_FROM" ]; then
        say "The agent may send the files in $FILES_FROM to a chat; change that with files_from in its settings."
    else
        say "The agent sends no files until you list a folder with files_from in its settings ($FILES_WHY)."
    fi
}

main() {
    MA_HERE=$(cd "$(dirname "${BASH_SOURCE[1]}")" && pwd -P)
    parse_args "$@"
    set_paths
    check_harness
    # MA_STAGED says whether this run made the folder in MA_STAGE: a dry run names the installed one.
    MA_WORK="" MA_STAGE="" MA_STAGED=0 MA_CHANGING=0 MA_UPGRADING=0 MA_PREPARED=0 MA_CHANGE_STATUS="" MA_LOCKED=0
    MA_MADE_DATA=0
    trap 'finish $?' EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
    if [ "$UNINSTALL" -eq 1 ]; then
        if [ -d "$MA_DATA" ]; then lock_installs; fi
        uninstall
        return 0
    fi
    [ "$DRY_RUN" -eq 0 ] || say "dry run: nothing is installed, written or started"
    # The plugin of today's connector serves an identity in its old home: an agent made here instead
    # would be a new one, in none of its chats.
    if [ "$REPLACE_PLUGIN" -eq 1 ] && [ ! -f "$MA_SETTINGS" ]; then
        die "--replace-plugin is for an install moved over from today's connector: first run marmot-agent import <its home> --settings $MA_SETTINGS (docs/move.md), then this again"
    fi

    settle_values
    # What choose_files_from finds, for a harness whose plugin states no folder for the agent's files.
    FILES_CHOSEN=0 FILES_KEPT=0 FILES_ALSO=0 FILES_ADD=0 FILES_FROM="" FILES_WHY=""
    GATEWAY_STATE=()
    GATEWAY_CACHE=()
    # The settings file of the gateway this run changes the plugin of, which harness_check sets.
    MA_GATEWAY=""
    harness_check
    if [ "$NO_SERVICE" -eq 0 ]; then
        if [ "$OS" = Linux ]; then
            program_path systemctl >/dev/null ||
                missing "systemd" "This machine has no systemctl. Use --no-service and start marmot-agent yourself."
        else
            program_path launchctl >/dev/null || missing "launchctl" "Use --no-service and start marmot-agent yourself."
        fi
    fi
    if [ "$MISSING" -eq 1 ]; then
        [ "$DRY_RUN" -eq 1 ] || die "get what is missing above, then run this again"
        warn "a real run would stop here, for what is missing above"
    fi

    install_core
    MA_PREPARED=1
    harness_prepare
    publish_support
    private_dir "$MA_DATA"
    private_dir "$MA_DATA/agents"
    private_dir "$MA_AGENT_DIR"
    printf '%s\n' "$HARNESS" | put "$MA_AGENT_DIR/harness" 600
    if [ -z "$CWD" ]; then
        :
    elif [ "$DRY_RUN" -eq 1 ]; then
        [ -d "$CWD" ] || say "would make folder $CWD"
    else
        mkdir -p "$CWD"
    fi
    write_settings
    save_choices
    init_agent
    write_launchers
    if [ "$HARNESS_KIND" = plugin ]; then harness_connect; fi
    make_files_from
    if [ "$NO_SERVICE" -eq 0 ]; then
        if [ "$OS" = Linux ]; then systemd_units; else launchd_agents; fi
        start_services
    fi
    # An upgrade, with a container image of its harness's, and a change to the gateway's plugin, are
    # done once the core is set up to start, and started unless --no-start or --no-service said not to.
    if declare -F harness_commit >/dev/null; then harness_commit; fi
    if [ "$MA_UPGRADING" -eq 1 ]; then upgrade_end; fi
    if [ "$MA_CHANGING" -eq 1 ]; then change_end; fi
    moved_done
    summary
}

summary() {
    say ""
    if [ "$DRY_RUN" -eq 1 ]; then
        say "The agent \"$NAME\" ($HARNESS_TITLE) would be installed."
    else
        say "The agent \"$NAME\" ($HARNESS_TITLE) is installed."
    fi
    if [ -s "$MA_AGENT_DIR/public-key" ]; then
        say "Its public key, to invite it to a chat: $(cat "$MA_AGENT_DIR/public-key")"
    fi
    say "Its settings: $MA_SETTINGS"
    if [ "$NO_SERVICE" -eq 1 ]; then
        say "Run it with: $MA_AGENT_DIR/start"
        [ "$HARNESS_KIND" != socket ] || say "and its connector with: $MA_AGENT_DIR/connector"
    elif [ "$OS" = Linux ]; then
        say "Its logs: journalctl --user -u $MA_UNIT"
        say "To keep it running when you are logged out: loginctl enable-linger \"\$USER\""
    else
        say "Its logs: $MA_AGENT_DIR/logs"
    fi
    say "To remove the service (the agent's folder is kept): $MA_LIB/install/$HARNESS.sh --uninstall --name $NAME"
    if declare -F harness_summary >/dev/null; then harness_summary; fi
}
