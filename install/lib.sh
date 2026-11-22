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

# Runs a command, or, in a dry run, says it would.
act() {
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would run: $*"
    else
        "$@"
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
    version=$(version_of "$program")
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
# readable by its owner only, before the install changes it.
backup_file() {
    local file=$1 copy
    [ -e "$file" ] || return 0
    copy="$MA_AGENT_DIR/backups/$(basename "$file").$(date -u +%Y%m%dT%H%M%SZ)"
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would keep a copy of $file in $copy"
        return 0
    fi
    private_dir "$MA_AGENT_DIR/backups"
    rm -rf "$copy"
    (umask 077 && cp -R "$file" "$copy")
    chmod -R go-rwx "$copy"
    say "kept a copy of $file in $copy"
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

# Moves the staged parts into $MA_LIB, each in place of its old copy.
publish_support() {
    local part
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would put $(lib_parts | tr '\n' ' ')into $MA_LIB"
        return 0
    fi
    for part in $(lib_parts); do
        [ -e "$MA_STAGE/$part" ] || continue
        mkdir -p "$(dirname "$MA_LIB/$part")"
        rm -rf "$MA_LIB/$part.old"
        if [ -e "$MA_LIB/$part" ]; then mv "$MA_LIB/$part" "$MA_LIB/$part.old"; fi
        mv "$MA_STAGE/$part" "$MA_LIB/$part"
        rm -rf "$MA_LIB/$part.old"
    done
    rm -rf "$MA_STAGE"
    MA_STAGED=0
    say "installed $(lib_parts | tr '\n' ' ')in $MA_LIB"
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

# Downloads the release for this machine, checks it against the release's SHA256SUMS, and unpacks it into
# a folder; prints the unpacked tree. The checksum catches a broken download, not a replaced one (Q-28).
fetch_release() {
    local work=$1 target asset base expected actual member
    target=$(release_target)
    asset="marmot-agent-$target.tar.gz"
    if [ -n "$VERSION" ]; then base="$MA_RELEASES/download/$VERSION"; else base="$MA_RELEASES/latest/download"; fi
    download "$base/$asset" "$work/$asset" || die "could not download $base/$asset"
    download "$base/SHA256SUMS" "$work/SHA256SUMS" || die "could not download $base/SHA256SUMS"
    expected=$(awk -v a="$asset" '$2 == a || $2 == "*" a { print tolower($1); exit }' "$work/SHA256SUMS")
    [ ${#expected} -eq 64 ] || die "SHA256SUMS has no line for $asset"
    actual=$(sha256_of "$work/$asset")
    [ "$actual" = "$expected" ] || die "$asset does not match its checksum in SHA256SUMS"
    while IFS= read -r member; do
        case $member in
            /* | .. | ../* | */../* | */..) die "the archive holds an unsafe path" ;;
            "marmot-agent-$target" | "marmot-agent-$target/"*) ;;
            *) die "the archive holds a path outside marmot-agent-$target/" ;;
        esac
    done < <(tar -tzf "$work/$asset")
    mkdir -p "$work/unpacked"
    tar -xzf "$work/$asset" -C "$work/unpacked"
    [ -x "$work/unpacked/marmot-agent-$target/bin/marmot-agent" ] || die "the archive holds no bin/marmot-agent"
    printf '%s' "$work/unpacked/marmot-agent-$target"
}

# Installs the core and the support files, from wherever they come from.
install_core() {
    local tree binary work=""
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
        binary="(downloaded)"
        tree=$MA_ROOT
    else
        program_path curl >/dev/null || die "downloading the release needs curl"
        MA_WORK=$(mktemp -d)
        say "downloading marmot-agent ${VERSION:-(latest release)}"
        tree=$(fetch_release "$MA_WORK")
        binary="$tree/bin/marmot-agent"
    fi
    owner_hex "$binary"
    if [ "$DRY_RUN" -eq 1 ]; then
        say "would install marmot-agent to $MA_CORE"
        say "would copy the support files of $HARNESS_TITLE to $MA_LIB"
        MA_STAGE=$MA_LIB
        return 0
    fi
    mkdir -p "$MA_BIN_DIR" "$MA_DATA"
    install -m 0755 "$binary" "$MA_CORE.new"
    mv -f "$MA_CORE.new" "$MA_CORE"
    say "installed $MA_CORE"
    # The support files are prepared in a folder of this run's own and published after; the exit trap
    # removes it if the run stops before then.
    MA_STAGE=$MA_DATA/lib.new.$$
    rm -rf "$MA_STAGE"
    MA_STAGED=1
    # shellcheck disable=SC2046 # one part per word
    copy_support "$tree" "$MA_STAGE" $(lib_parts)
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
service and keeps the agent's settings and identity.

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
    cat <<EOF
  --version TAG        install this release (default: the latest)
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
    BINARY_FROM="" VERSION="" NAME=$HARNESS DISPLAY=$HARNESS_TITLE OWNER="" RELAYS="" LOCAL_RELAY="" CWD=""
    MODEL_KEY="" MODEL="" MODEL_URL="" PERMISSION_MODE="" GOOSE_MODE="" SET_CWD=0 SET_KEY=""
    CONTAINER="" REPLACE_PLUGIN=0
    while [ $# -gt 0 ]; do
        case $1 in
            --owner | --cwd | --name | --display-name | --relay | --local-relay | --version | --binary | \
                --model-key | --model | --model-url | --permission-mode | --goose-mode)
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
    MA_AGENT_DIR=$MA_DATA/agents/$NAME
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
    elif [ "$HARNESS_KIND" = plugin ] && imported "$HARNESS_TITLE install,"; then
        # Moved over from today's connector (docs/move.md): the settings name its home and identity.
        say "using the settings marmot-agent import wrote in $MA_SETTINGS"
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
        # A harness the core drives works where its settings say, which may have been edited since.
        if [ "$HARNESS_KIND" = builtin ] && [ -z "$CWD_GIVEN" ] && [ -n "$settings_cwd" ]; then
            CWD=$settings_cwd
        fi
        [ -n "$CWD" ] || CWD=$settings_cwd
        [ -n "$CWD" ] || die "the agent has settings but no work folder is known: give --cwd"
        CWD=$(absolute "$CWD")
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
    no_control "--cwd" "$CWD"
    local key
    CWD=$(canonical "$CWD") || die "--cwd names a loop of links"
    if inside "$CWD" "$MA_DATA"; then die "--cwd must not be inside $MA_DATA, which holds the agents' keys"; fi
    if inside "$MA_DATA" "$CWD"; then die "--cwd must not hold $MA_DATA, which holds the agents' keys"; fi
    [ -n "$MODEL_KEY" ] || return 0
    key=$(canonical "$MODEL_KEY") || die "--model-key names a loop of links"
    if inside "$key" "$CWD"; then
        die "the model key file must not be inside the work folder, where $HARNESS_TITLE could read it"
    fi
    if inside "$key" "$MA_DATA"; then die "the model key file must not be inside $MA_DATA"; fi
}

main() {
    MA_HERE=$(cd "$(dirname "${BASH_SOURCE[1]}")" && pwd -P)
    parse_args "$@"
    set_paths
    check_harness
    # MA_STAGED says whether this run made the folder in MA_STAGE: a dry run names the installed one.
    MA_WORK="" MA_STAGE="" MA_STAGED=0
    trap 'if [ -n "$MA_WORK" ]; then rm -rf "$MA_WORK"; fi; if [ "$MA_STAGED" -eq 1 ]; then rm -rf "$MA_STAGE"; fi' EXIT
    if [ "$UNINSTALL" -eq 1 ]; then
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
    if [ "$NO_SERVICE" -eq 0 ]; then
        if [ "$OS" = Linux ]; then systemd_units; else launchd_agents; fi
        start_services
    fi
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
