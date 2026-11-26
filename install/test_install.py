"""Tests for the install scripts in this folder (D-091).

    python3 -m unittest discover -s install -p 'test_*.py'

Each script runs against a throwaway HOME, with stand-ins for the programs it calls (the harnesses,
systemctl, launchctl, docker, npm and node) on PATH, so nothing on the machine running the tests is
installed, started or changed. The stand-ins for OpenClaw and Hermes keep their settings in files in
that HOME, as the real ones do, and do what the installers ask of them: add a plugin, set and unset a
setting. A move from today's connectors' plugins is also stopped at each step it takes (Interrupted), and so is
an upgrade of the core and its support files (Upgrade).
A stand-in marmot-agent plays the core; the download test serves a release from a folder, packaged by
tools/release/package.sh. The OpenClaw installer reads JSON with node, so its tests use the node on
the PATH the tests run with, and are skipped without one.
"""

import fcntl
import hashlib
import json
import os
import shlex
import shutil
import stat
import subprocess
import tempfile
import time
import tomllib
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
HARNESSES = ["codex", "claude-code", "opencode", "goose", "cursor", "pi", "openclaw", "hermes"]
# A connector of ours, as a service beside the core's.
SOCKET = {"claude-code"}
# Our plugin, in the person's own gateway.
PLUGIN = {"openclaw", "hermes"}
# What may run in a container instead, with --container.
CONTAINER = {"goose", "pi"}
# Each harness's program, as the stand-ins on PATH provide it.
PROGRAMS = ("codex", "opencode", "cursor-agent", "goose", "pi", "openclaw", "hermes")
REAL_NODE = shutil.which("node")
OWNER = "ab" * 32
AGENT_KEY = "cd" * 32
# NIP-19's own example.
NPUB = "npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg"
NPUB_HEX = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e"
SYSTEM_PATH = "/usr/bin:/bin"
# A model endpoint and a model, which a harness in a container needs and has no default for (D-100).
MODEL = ["--model-url", "https://models.example/v1", "--model", "some/model"]

FAKE_CORE = f"""#!/bin/sh
case $1 in
    key)
        case $2 in
            {NPUB}) echo {NPUB_HEX} ;;
            *) exit 1 ;;
        esac
        exit 0
        ;;
esac
echo "marmot-agent $*" >> "$FAKE_LOG"
case $1 in
    init) home=$(sed -n 's/^home = "\\(.*\\)"$/\\1/p' "$2"); mkdir -p "$home"; echo {AGENT_KEY} ;;
    run) echo "with MODEL_BASE_URL=${{MODEL_BASE_URL:-}}" >> "$FAKE_LOG" ;;
esac
"""
# An older core, which the upgrade tests install first.
OLDER_CORE = FAKE_CORE + "# an older core\n"
# A stand-in fails a command that starts as FAKE_FAIL says, such as "systemctl --user restart". One that
# starts as FAKE_PAUSE says makes the file FAKE_PAUSE_AT.reached, then waits until FAKE_PAUSE_AT is there.
FAKE_LOGGER = """#!/bin/sh
echo "$(basename "$0") $*" >> "$FAKE_LOG"
# A unit's state is FAKE_ACTIVE's, inactive unless it says; a LaunchAgent is never loaded once booted out.
case "$(basename "$0") $*" in
    "systemctl --user show"*) echo "${FAKE_ACTIVE:-inactive}"; exit 0 ;;
    "launchctl print"*) exit 113 ;;
esac
if [ -n "${FAKE_FAIL:-}" ]; then case "$(basename "$0") $*" in "$FAKE_FAIL"*) echo "failing, as asked" >&2; exit 1 ;; esac; fi
if [ -n "${FAKE_PAUSE:-}" ]; then case "$(basename "$0") $*" in "$FAKE_PAUSE"*)
    : >"$FAKE_PAUSE_AT.reached"
    n=0; while [ ! -e "$FAKE_PAUSE_AT" ] && [ "$n" -lt 1200 ]; do sleep 0.05; n=$((n + 1)); done ;;
esac; fi
"""
# A stand-in for docker that keeps its images' tags, in docker-tags beside FAKE_LOG, and its images, in
# docker-images: each build makes a new image. An image a tag no longer names is kept, as docker keeps it
# (dangling), and may be tagged again by its id; `rmi` of its last tag removes it. It says nothing of the
# containers it does not run. A command that starts as FAKE_DOCKER_FAIL says fails as one does when docker
# cannot be reached, such as "image inspect".
FAKE_DOCKER = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path

args = sys.argv[1:]
with open(os.environ["FAKE_LOG"], "a") as log:
    log.write("docker " + " ".join(args) + "\n")
if os.environ.get("FAKE_DOCKER_FAIL") and " ".join(args).startswith(os.environ["FAKE_DOCKER_FAIL"]):
    sys.exit("Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?")
store = Path(os.environ["FAKE_LOG"]).parent / "docker-tags"
kept = Path(os.environ["FAKE_LOG"]).parent / "docker-images"
tags = json.loads(store.read_text()) if store.exists() else {}
images = json.loads(kept.read_text()) if kept.exists() else sorted(set(tags.values()))


def name(tag):
    return tag if ":" in tag.rsplit("/", 1)[-1] else tag + ":latest"


if args[:1] == ["build"]:
    built = 1 + max([int(i.split(":")[1]) for i in images] + [0])
    tags[name(args[args.index("-t") + 1])] = f"image:{built}"
    images.append(f"image:{built}")
elif args[:1] == ["tag"]:
    source = tags.get(name(args[1])) or (args[1] if args[1] in images else None)
    tags[name(args[2])] = source or sys.exit(f"Error: No such image: {args[1]}")
elif args[:2] == ["image", "inspect"]:
    print(tags.get(name(args[-1])) or sys.exit(f"Error: No such image: {args[-1]}"))
elif args[:2] == ["image", "ls"]:
    # With --quiet and a reference: the image it names, if any.
    if name(args[-1]) in tags:
        print(tags[name(args[-1])])
elif args[:1] == ["rmi"]:
    for tag in args[1:]:
        image = tags.pop(name(tag), None) or sys.exit(f"Error: No such image: {tag}")
        if image not in tags.values():
            images.remove(image)
store.write_text(json.dumps(tags))
kept.write_text(json.dumps(images))
'''
# npm ci makes the packages it installs, and npm pack the archive, as the real one does.
FAKE_NPM = """#!/bin/sh
echo "npm $*" >> "$FAKE_LOG"
if [ -n "${FAKE_FAIL:-}" ]; then case "npm $*" in "$FAKE_FAIL"*) echo "failing, as asked" >&2; exit 1 ;; esac; fi
command=$1 prefix=.
while [ $# -gt 0 ]; do [ "$1" = --prefix ] && prefix=$2; shift; done
case $command in
    ci) mkdir -p "$prefix/node_modules/a-package" ;;
    pack) echo "a packed plugin" > marmot-agent-openclaw-0.0.0.tgz ;;
esac
"""
# A script node runs is run by the real one, where there is one.
FAKE_NODE = f"""#!/bin/sh
[ "$1" = -p ] && {{ echo "${{FAKE_NODE_MAJOR:-24}}"; exit 0; }}
[ "$1" = -e ] && exec "{REAL_NODE or 'false'}" "$@"
echo "node $*" >> "$FAKE_LOG"
"""
# Pi and Goose say their version, as Pi 1.6.0 and Goose 1.59.2 print it, and are not run otherwise.
FAKE_VERSIONED = """#!/bin/sh
[ "$1" = --version ] && {{ echo "${{FAKE_VERSION:-{version}}}"; exit 0; }}
echo "$(basename "$0") $*" >> "$FAKE_LOG"
"""
FAKE_OPENCLAW = r'''#!/usr/bin/env python3
"""A stand-in for `openclaw`: the commands the installer runs, on a JSON file as OpenClaw keeps it."""
import json, os, shutil, signal, sys
from pathlib import Path

args = sys.argv[1:]
profile = os.environ.get("OPENCLAW_PROFILE", "default")
state = Path(os.environ.get("OPENCLAW_STATE_DIR") or Path(os.environ.get("OPENCLAW_HOME") or os.environ["HOME"]) / (
    ".openclaw" if profile == "default" else f".openclaw-{profile}"))
config = state / "openclaw.json"
plugin = state / "extensions" / "marmot"


def load():
    return json.loads(config.read_text()) if config.exists() else {}


def save(c):
    state.mkdir(parents=True, exist_ok=True)
    config.write_text(json.dumps(c, indent=2) + "\n")


def note():
    with open(os.environ["FAKE_LOG"], "a") as log:
        log.write("openclaw " + " ".join(args) + "\n")


def walk(c, path, make=False):
    *parents, last = path.split(".")
    for p in parents:
        if p not in c:
            if not make:
                return None, last
            c[p] = {}
        c = c[p]
    return c, last


def valid():
    """OpenClaw checks the whole of its settings before it reads or changes one: the marmot channel against
    the schema of the plugin installed, ours (packages/openclaw/openclaw.plugin.json) taking only these.
    Of the commands the installer runs, only `config file` and `agents bindings` work without (as
    OpenClaw 2026.11.5 does)."""
    ours = plugin / "package.json"
    if ours.is_file() and json.loads(ours.read_text()).get("name") == "marmot-agent-openclaw":
        extra = set(load().get("channels", {}).get("marmot", {})) - {"enabled", "home"}
        if extra:
            sys.exit("OpenClaw config is invalid: channels.marmot: invalid config for plugin marmot: "
                     f"must not have additional properties: {sorted(extra)}")


def fail(command):
    """A command made to fail, as FAKE_OPENCLAW_FAIL names it."""
    if os.environ.get("FAKE_OPENCLAW_FAIL") == command:
        sys.exit(f"the openclaw stand-in fails {command}, as asked")


if args == ["--version"]:
    print(f"OpenClaw {os.environ.get('FAKE_OPENCLAW_VERSION', '2026.11.5')} (e13cf15)")
elif args[:2] == ["config", "file"]:
    print(config)
elif args[:2] == ["config", "get"]:
    valid()
    c, last = walk(load(), args[2])
    if c is None or last not in c:
        print(json.dumps({"ok": False, "error": {"message": f"Config path is valid but unset: {args[2]}"}}, indent=2))
        sys.exit(1)
    value = c[last]
    print(json.dumps(value, indent=2) if "--json" in args or not isinstance(value, str) else value)
elif args[:2] == ["config", "set"]:
    note()
    valid()
    c = load()
    parent, last = walk(c, args[2], make=True)
    parent[last] = json.loads(args[3]) if "--strict-json" in args else args[3]
    save(c)
    if os.environ.get("FAKE_OPENCLAW_KILL_AFTER") == f"config set {args[2]}":
        # The installer killed outright once this command has done its work.
        os.kill(os.getppid(), signal.SIGKILL)
elif args[:2] == ["config", "unset"]:
    note()
    valid()
    c = load()
    parent, last = walk(c, args[2])
    if parent is None or last not in parent:
        sys.exit(1)
    del parent[last]
    save(c)
elif args[:2] == ["agents", "list"] and "--json" in args:
    valid()
    # Each agent with its workspace, resolved as OpenClaw resolves it, and whether it takes what no
    # binding routes: the one agent, or the system agent.
    agents = load().get("agents", {})
    defaults = agents.get("defaults", {})
    entries = agents.get("entries") or {"main": {}}
    system = (defaults.get("systemAgent") or {}).get("agentId")
    first = system or ("main" if "main" in entries else next(iter(entries)))
    out = []
    for agent, entry in entries.items():
        workspace = (entry or {}).get("workspace")
        if not workspace and agent == first:
            workspace = defaults.get("workspace") or str(state / "workspace")
        elif not workspace:
            workspace = f"{defaults['workspace']}/{agent}" if defaults.get("workspace") else str(state / f"workspace-{agent}")
        out.append({"id": agent, "workspace": os.path.expanduser(workspace), "agentDir": str(state / "agents" / agent / "agent"),
                    "isDefault": len(entries) == 1 or agent == system})
    print(json.dumps(out, indent=2))
elif args[:2] == ["agents", "bindings"] and "--json" in args:
    print(json.dumps(load().get("bindings", []), indent=2))
elif args[:2] == ["plugins", "install"]:
    # Installing enables the plugin, unless --no-enable. A failed one may have taken the old one out.
    note()
    valid()
    shutil.rmtree(plugin, ignore_errors=True)
    fail("plugins install")
    plugin.mkdir(parents=True)
    (plugin / "package.json").write_text(json.dumps({"name": "marmot-agent-openclaw"}))
    shutil.copy(args[-1], plugin / "packed.tgz")
    c = load()
    c.setdefault("plugins", {}).setdefault("entries", {})["marmot"] = {"enabled": True, "config": {}}
    save(c)
elif args[:2] == ["plugins", "inspect"]:
    # One found in a folder of plugins.load.paths wins, and has no install record.
    valid()
    found = [f for f in load().get("plugins", {}).get("load", {}).get("paths", []) if Path(f, "package.json").is_file()]
    if found:
        print(json.dumps({"plugin": {"id": "marmot", "rootDir": found[0], "source": found[0] + "/dist/index.js"}}))
    elif plugin.is_dir():
        print(json.dumps({"plugin": {"id": "marmot", "rootDir": str(plugin)}, "install": {"installPath": str(plugin)}}))
    else:
        sys.exit(1)
elif args[:2] == ["plugins", "uninstall"]:
    note()
    shutil.rmtree(plugin, ignore_errors=True)
    c = load()
    c.get("channels", {}).pop("marmot", None)
    c.get("plugins", {}).get("entries", {}).pop("marmot", None)
    save(c)
else:
    sys.exit(f"the openclaw stand-in does not know: {args}")
'''
FAKE_HERMES = r'''#!/usr/bin/env python3
"""A stand-in for `hermes`: the commands the installer runs. Its config.yaml is JSON, which is YAML."""
import json, os, signal, sys, time
from pathlib import Path

args = sys.argv[1:]
home = Path(os.environ.get("HERMES_HOME") or Path(os.environ["HOME"]) / ".hermes")
config = home / "config.yaml"
env = home / ".env"


def load():
    return json.loads(config.read_text()) if config.exists() else {}


def save(c):
    home.mkdir(parents=True, exist_ok=True)
    config.write_text(json.dumps(c, indent=2) + "\n")


def lines():
    return env.read_text().splitlines() if env.exists() else []


def env_key(line):
    """The key of a .env line, which may start with export and have spaces around the =."""
    left = line.split("=", 1)[0].strip()
    return left[len("export "):].strip() if left.startswith("export ") else left


def note():
    with open(os.environ["FAKE_LOG"], "a") as log:
        log.write("hermes " + " ".join(args) + "\n")


def walk(c, path, make=False):
    *parents, last = path.split(".")
    for p in parents:
        if p not in c:
            if not make:
                return None, last
            c[p] = {}
        c = c[p]
    return c, last


if args == ["--version"]:
    print(f"Hermes Agent v{os.environ.get('FAKE_HERMES_VERSION', '0.23.2')} (2026.9.24) · stable")
elif args[:2] == ["config", "path"]:
    print(config)
elif args[:2] == ["config", "env-path"]:
    print(env)
elif args[:2] == ["config", "get"]:
    key = args[2]
    if key.isupper():
        found = [l.split("=", 1)[1].strip() for l in lines() if "=" in l and env_key(l) == key]
        if not found and key not in os.environ:
            sys.exit(f"Config key not set: {key}")
        print((found or [os.environ.get(key)])[-1])
    else:
        c, last = walk(load(), key)
        value = c.get(last, "") if c is not None else ""
        print(str(value).lower() if isinstance(value, bool) else value)
    if os.environ.get("FAKE_HERMES_PAUSE_AFTER") == f"config get {key}":
        # The installer held up once it has read this, as FAKE_PAUSE holds up the other stand-ins.
        sys.stdout.flush()
        at = Path(os.environ["FAKE_PAUSE_AT"])
        Path(f"{at}.reached").touch()
        for _ in range(1200):
            if at.exists():
                break
            time.sleep(0.05)
elif args[:2] == ["config", "set"]:
    note()
    key, value = args[2], args[3]
    if key.isupper():
        home.mkdir(parents=True, exist_ok=True)
        env.write_text("".join(l + "\n" for l in lines() if env_key(l) != key) + f"{key}={value}\n")
    else:
        c = load()
        parent, last = walk(c, key, make=True)
        parent[last] = {"true": True, "false": False}.get(value, value)
        save(c)
    if os.environ.get("FAKE_HERMES_KILL_AFTER") == f"config set {key}":
        # The installer killed outright once this command has done its work.
        os.kill(os.getppid(), signal.SIGKILL)
elif args[:2] == ["config", "unset"]:
    note()
    key = args[2]
    if key.isupper():
        env.write_text("".join(l + "\n" for l in lines() if env_key(l) != key))
    else:
        c = load()
        parent, last = walk(c, key)
        if parent is not None:
            parent.pop(last, None)
        save(c)
elif args[:2] in (["plugins", "enable"], ["plugins", "disable"]):
    note()
    if os.environ.get("FAKE_HERMES_FAIL") == " ".join(args[:2]):
        sys.exit(f"the hermes stand-in fails {' '.join(args[:2])}, as asked")
    if os.environ.get("FAKE_HERMES_KILL") == " ".join(args[:2]):
        # The installer killed outright as it waits on this command.
        os.kill(os.getppid(), signal.SIGKILL)
        sys.exit(1)
    if not (home / "plugins" / args[2] / "plugin.yaml").is_file():
        sys.exit(f"no plugin {args[2]}")
    c = load()
    plugins = c.setdefault("plugins", {"enabled": [], "disabled": []})
    on, off = ("enabled", "disabled") if args[1] == "enable" else ("disabled", "enabled")
    plugins[off] = [p for p in plugins.get(off, []) if p != args[2]]
    plugins[on] = sorted(set(plugins.get(on, [])) | {args[2]})
    save(c)
else:
    sys.exit(f"the hermes stand-in does not know: {args}")
'''

# What a move to our plugin changes in each gateway's folder in HOME, which going back puts back.
CHANGED = {"hermes": (".hermes", ("config.yaml", ".env", "plugins/marmot")),
           "openclaw": (".openclaw", ("openclaw.json", "extensions/marmot"))}
# An mv that cannot exchange two folders, as on macOS or before GNU coreutils 9.5.
MV_WITHOUT_EXCHANGE = f"""#!/bin/sh
for a; do [ "$a" = --exchange ] && {{ echo "mv: unrecognized option '--exchange'" >&2; exit 1; }}; done
exec {shutil.which("mv", path=SYSTEM_PATH)} "$@"
"""
MV_EXCHANGES = "--exchange" in subprocess.run(["mv", "--help"], capture_output=True, text=True,
                                              env={"PATH": SYSTEM_PATH}).stdout

# A systemctl that, as it starts or restarts an agent's unit, plays the core opening the agent's store, as
# MDK does: the store, in home/accounts/agent, is made the core's own generation (session.sqlite and its -wal
# file say which), forward only. An older core finds a newer store refused: noted in FAKE_LOG.refused, and
# the store left as it is. Otherwise it is FAKE_LOGGER, paused before the core starts and failed after, as
# when a connector's unit fails beside it.
FAKE_SYSTEMCTL_STORE = """#!/bin/sh
echo "$(basename "$0") $*" >> "$FAKE_LOG"
case "$*" in "--user show"*) echo "${FAKE_ACTIVE:-inactive}"; exit 0 ;; esac
if [ -n "${FAKE_PAUSE:-}" ]; then case "$(basename "$0") $*" in "$FAKE_PAUSE"*)
    : >"$FAKE_PAUSE_AT.reached"
    n=0; while [ ! -e "$FAKE_PAUSE_AT" ] && [ "$n" -lt 1200 ]; do sleep 0.05; n=$((n + 1)); done ;;
esac; fi
case "$2" in restart | start)
    core="$HOME/.local/bin/marmot-agent"
    if grep -q '^# an older core' "$core"; then made=older; else made=new; fi
    for unit; do
        case $unit in marmot-agent-*-connector.service | -*) continue ;; marmot-agent-*.service) ;; *) continue ;; esac
        name=${unit#marmot-agent-}
        store="$HOME/.local/share/marmot-agent/agents/${name%.service}/home/accounts/agent"
        [ -d "$store" ] || continue
        if [ "$made" = older ] && [ "$(cat "$store/session.sqlite" 2>/dev/null)" = new ]; then
            echo "the older core cannot open the store of ${name%.service}" >>"$FAKE_LOG.refused"
            continue
        fi
        echo "$made" >"$store/session.sqlite"
        echo "$made" >"$store/session.sqlite-wal"
    done ;;
esac
if [ -n "${FAKE_FAIL:-}" ]; then case "$(basename "$0") $*" in "$FAKE_FAIL"*) echo "failing, as asked" >&2; exit 1 ;; esac; fi
"""

# Runs the install script named after it, as `bash script` does, and sends STOP_SIGNAL to the script's
# process group as it comes to step STOP_AT. A step is a call of act or put, through which the scripts
# change things, or an rm, cp or mv they run outside one: counted as each is entered, so once, in the
# subshell of a pipeline that runs it. The count is kept in the file STOP_COUNT, as a subshell counts
# too; with STOP_AT 0 it only counts. No hook in the scripts themselves: bash's DEBUG trap, which -T
# hands to functions and subshells.
STOPPED = r"""set -T
trap 'case ${FUNCNAME[0]:-}:$BASH_COMMAND in
    act:act\ * | put:put\ *) step=1 ;;
    *:rm\ * | *:cp\ * | *:mv\ *) case " ${FUNCNAME[*]:-} " in *" act "* | *" put "*) step=0 ;; *) step=1 ;; esac ;;
    *) step=0 ;;
esac
if [ "$step" = 1 ]; then
    read -r n <"$STOP_COUNT"; n=$((n + 1)); echo "$n" >"$STOP_COUNT"
    if [ "$n" = "$STOP_AT" ]; then kill -s "$STOP_SIGNAL" 0; fi
fi' DEBUG
. "$0" "$@"
"""


def mode(path: Path) -> int:
    return stat.S_IMODE(path.stat().st_mode)


class Machine:
    """A throwaway HOME and a PATH of stand-ins."""

    def __init__(self, test: unittest.TestCase, programs=PROGRAMS):
        self.dir = Path(tempfile.mkdtemp(prefix="marmot-install-"))
        test.addCleanup(shutil.rmtree, self.dir, True)
        self.home = self.dir / "home"
        self.home.mkdir()
        self.bin = self.dir / "bin"
        self.bin.mkdir()
        self.log = self.dir / "calls.log"
        self.log.touch()
        self.steps = self.dir / "steps"
        self.core = self.dir / "marmot-agent"
        self.write(self.core, FAKE_CORE)
        for name in ("systemctl", "launchctl", "docker", *programs):
            self.write(self.bin / name, FAKE_LOGGER)
        stand_ins = {"goose": FAKE_VERSIONED.format(version=" 1.59.2"), "pi": FAKE_VERSIONED.format(version="1.6.0"),
                     "openclaw": FAKE_OPENCLAW, "hermes": FAKE_HERMES}
        for name in programs:
            if name in stand_ins:
                self.write(self.bin / name, stand_ins[name])
        self.write(self.bin / "npm", FAKE_NPM)
        self.write(self.bin / "node", FAKE_NODE)
        self.key = self.dir / "model.key"
        self.key.write_text("not a real key\n")
        self.key.chmod(0o600)
        # Claude Code's stored sign-in, as `claude` keeps it on Linux after /login.
        self.claude = self.home / ".claude"
        self.claude.mkdir()
        (self.claude / ".credentials.json").write_text("{}")
        self.data = self.home / ".local/share/marmot-agent"
        self.units = self.home / ".config/systemd/user"

    @staticmethod
    def write(path: Path, text: str) -> None:
        path.write_text(text)
        path.chmod(0o755)

    def run(self, harness: str, *args: str, env=None, check=True, stop=None) -> subprocess.CompletedProcess:
        """Runs an install script; with stop, a signal and a step (see STOPPED), stops it there."""
        environment = {
            "HOME": str(self.home),
            "PATH": f"{self.bin}:{SYSTEM_PATH}",
            "FAKE_LOG": str(self.log),
            "LANG": "C",
        }
        environment.update(env or {})
        script = ["bash", str(HERE / f"{harness}.sh")]
        if stop is not None:
            signal, step = stop
            self.steps.write_text("0\n")
            environment.update({"STOP_SIGNAL": signal, "STOP_AT": str(step), "STOP_COUNT": str(self.steps)})
            script = ["bash", "-c", STOPPED, str(HERE / f"{harness}.sh")]
        result = subprocess.run(
            [*script, *args],
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=120,
            # Its own process group, which the signal goes to, as a terminal's or a service manager's does.
            start_new_session=stop is not None,
        )
        if check and result.returncode != 0:
            raise AssertionError(f"{harness}.sh failed ({result.returncode}):\n{result.stdout}\n{result.stderr}")
        return result

    def start(self, harness: str, *args: str, env=None) -> subprocess.Popen:
        """Starts an install script and leaves it running, its output going to files beside the log."""
        environment = {"HOME": str(self.home), "PATH": f"{self.bin}:{SYSTEM_PATH}", "FAKE_LOG": str(self.log),
                       "LANG": "C", **(env or {})}
        out = tempfile.NamedTemporaryFile("w+", dir=self.dir, delete=False)
        process = subprocess.Popen(["bash", str(HERE / f"{harness}.sh"), *args], env=environment,
                                   stdin=subprocess.DEVNULL, stdout=out, stderr=subprocess.STDOUT, text=True)
        out.close()
        process.output_path = Path(out.name)
        return process

    @staticmethod
    def output(process: subprocess.Popen) -> str:
        return process.output_path.read_text()

    @staticmethod
    def until(*conditions) -> None:
        """Waits, for up to a minute, until one of the conditions holds."""
        for _ in range(1200):
            if any(condition() for condition in conditions):
                return
            time.sleep(0.05)

    def args(self, harness: str, *more: str, container=False) -> list:
        args = ["--yes", "--owner", OWNER]
        if harness not in PLUGIN:
            args += ["--cwd", str(self.home / "work")]
        if container:
            args += ["--container", "--model-key", str(self.key), *MODEL]
        return args + list(more)

    def calls(self) -> list:
        return self.log.read_text().splitlines()

    def files(self) -> list:
        """What is in HOME, but for Claude Code's sign-in."""
        return sorted(str(p.relative_to(self.home)) for p in self.home.rglob("*")
                      if not p.is_relative_to(self.claude))


class DryRun(unittest.TestCase):
    """--dry-run writes nothing, and shows every file and command a real run would."""

    def test_every_harness(self):
        """Each harness, as the person has it, the same on Linux and on macOS (D-102)."""
        for system in ("Linux", "Darwin"):
            for harness in HARNESSES:
                with self.subTest(harness=harness, system=system):
                    self.dry_run(harness, system)

    def dry_run(self, harness: str, system: str) -> None:
        m = Machine(self)
        out = m.run(harness, *m.args(harness, "--binary", str(m.core), "--dry-run"),
                    env={"MARMOT_AGENT_OS": system}).stdout
        self.assertEqual(m.files(), [], "a dry run writes nothing")
        self.assertEqual(m.calls(), [], "a dry run only looks")
        agent = m.data / "agents" / harness
        self.assertIn(f"would write {agent}/settings.toml (mode 600):", out)
        self.assertIn(f'    | owner = "{OWNER}"', out)
        self.assertIn(f'    | home = "{agent}/home"', out)
        self.assertIn(f"would run: {m.home}/.local/bin/marmot-agent init {agent}/settings.toml", out)
        self.assertIn(f"would write {agent}/start (mode 700):", out)
        self.assertIn(f"    | exec '{m.home}/.local/bin/marmot-agent' run '{agent}/settings.toml'", out)
        self.assertNotIn("docker", out, "nothing runs in a container unless asked")
        if harness in PLUGIN:
            self.assertIn("    | [connector]", out)
            self.assertNotIn("cwd = ", out, "the gateway keeps the workspace it manages itself")
            self.assertNotIn(f"{agent}/connector", out, "the person's gateway is no service of ours")
        elif harness in SOCKET:
            self.assertIn("    | [connector]", out)
            self.assertIn(f"would write {agent}/connector (mode 700):", out)
        else:
            self.assertNotIn("[connector]", out)
            self.assertIn(f'    | cwd = "{m.home}/work"', out)
        if system == "Linux":
            unit = m.units / f"marmot-agent-{harness}.service"
            self.assertIn(f"would write {unit} (mode 644):", out)
            self.assertIn(f'    | ExecStart="{agent}/start"', out)
            self.assertIn("    | Restart=always", out, "the core is started again when its harness stops")
            connector = m.units / f"marmot-agent-{harness}-connector.service"
            self.assertEqual(str(connector) in out, harness in SOCKET)
            if harness in SOCKET:
                self.assertIn(f"    | BindsTo=marmot-agent-{harness}.service", out)
                self.assertIn(f"    | ExecStart=\"{agent}/connector\"", out)
            self.assertIn("would run: systemctl --user daemon-reload", out)
            self.assertIn(f"would run: systemctl --user restart marmot-agent-{harness}.service", out)
            self.assertNotIn("launchctl", out)
        else:
            plist = m.home / f"Library/LaunchAgents/org.marmot.marmot-agent.{harness}.plist"
            self.assertIn(f"would write {plist} (mode 644):", out)
            self.assertIn(f"    |         <string>{agent}/start</string>", out)
            self.assertIn("    |     <key>KeepAlive</key><true/>", out)
            self.assertIn(f"would run: launchctl bootstrap gui/{os.getuid()} {plist}", out)
            self.assertNotIn("systemctl", out)
            connector = m.home / f"Library/LaunchAgents/org.marmot.marmot-agent.{harness}.connector.plist"
            self.assertEqual(str(connector) in out, harness in SOCKET)

    def test_harness_sections(self):
        m = Machine(self)
        expected = {
            "codex": ["[codex]", f'program = "{m.bin}/codex"'],
            "opencode": ["[acp]", 'name = "opencode"', f'command = ["{m.bin}/opencode", "acp"]'],
            "cursor": ["[acp]", 'name = "cursor"', f'command = ["{m.bin}/cursor-agent", "acp"]'],
            "goose": ["[acp]", 'name = "goose"', f'command = ["{m.bin}/goose", "acp"]'],
            "pi": ["[pi]", f'program = "{m.bin}/pi"'],
        }
        for harness, lines in expected.items():
            with self.subTest(harness=harness):
                out = m.run(harness, *m.args(harness, "--binary", str(m.core), "--dry-run")).stdout
                for line in lines:
                    self.assertIn(f"    | {line}", out)
                self.assertIn(f'    | cwd = "{m.home}/work"', out)
                self.assertNotIn("    | args = ", out)
                for variable in ("MODEL_BASE_URL", "MODEL_HOST", "MODEL_NAME"):
                    self.assertNotIn(variable, out, "the harness's own settings choose its model")

    def test_plugin_harnesses(self):
        """What a dry run says it would do to the person's own OpenClaw and Hermes."""
        m = Machine(self)
        agent = m.data / "agents/openclaw"
        out = m.run("openclaw", *m.args("openclaw", "--binary", str(m.core), "--dry-run")).stdout
        self.assertIn("would run: sh -c cd ", out)
        self.assertIn(f"would run: {m.bin}/openclaw plugins install --force --accept-capabilities "
                      f"{m.data}/lib/packages/openclaw/marmot-agent-openclaw.tgz", out)
        self.assertNotIn("plugins enable", out, "installing a plugin enables it")
        if REAL_NODE:
            self.assertIn(f'would run: {m.bin}/openclaw config set channels.marmot '
                          f'{{"enabled":true,"home":"{agent}/home"}} --strict-json', out)
            self.assertIn(f"would run: {m.bin}/openclaw config set approvals.exec "
                          '{"enabled":true,"mode":"session","sessionFilter":[":marmot:"]} --strict-json', out)
        self.assertIn("openclaw gateway restart", out)
        agent = m.data / "agents/hermes"
        out = m.run("hermes", *m.args("hermes", "--binary", str(m.core), "--dry-run")).stdout
        self.assertIn(f"would copy {m.data}/lib/python/marmot_hermes to {m.home}/.hermes/plugins/marmot", out)
        self.assertIn(f"would run: {m.bin}/hermes plugins enable marmot", out)
        self.assertIn(f"would run: {m.bin}/hermes config set platforms.marmot.enabled true", out)
        self.assertIn(f"would run: {m.bin}/hermes config set platforms.marmot.home {agent}/home", out)
        self.assertIn(f"would run: {m.bin}/hermes config set MARMOT_ALLOW_ALL_USERS true", out)
        self.assertIn("hermes gateway restart", out)

    def test_plugin_harnesses_without_the_gateway(self):
        """A dry run goes on without the gateway, and says how to get it."""
        m = Machine(self, programs=())
        for harness, title in (("openclaw", "OpenClaw"), ("hermes", "Hermes")):
            with self.subTest(harness=harness):
                result = m.run(harness, *m.args(harness, "--binary", str(m.core), "--dry-run"))
                self.assertIn(f"missing: {title}", result.stderr)
                self.assertIn(f"would add the Marmot plugin to your {title}", result.stdout)

    def test_containers_on_request(self):
        """Pi and Goose run in a container only with --container, on the model it is given."""
        m = Machine(self)
        out = m.run("pi", *m.args("pi", "--binary", str(m.core), "--dry-run", container=True)).stdout
        self.assertIn("would run: docker build -q -t marmot-agents-pi", out)
        self.assertIn(f'    | program = "{m.data}/lib/tools/pi/pi.sh"', out)
        self.assertIn(f'    | args = ["{m.key}", "{m.data}/agents/pi/pi"]', out)
        self.assertIn(str(m.key), out, "the key is named by its path")
        self.assertNotIn("not a real key", out, "the key is never read into a file")
        out = m.run("goose", *m.args("goose", "--binary", str(m.core), "--dry-run", "--model-url",
                                     "https://models.example/v1/", container=True)).stdout
        self.assertIn("would run: docker build -q -t marmot-agents-goose", out)
        self.assertIn(f'    | command = ["{m.data}/lib/tools/goose/goose-acp.sh", "{m.key}", '
                      f'"{m.data}/agents/goose/goose"]', out)
        self.assertIn("    | MODEL_BASE_URL='https://models.example/v1/'", out)
        self.assertIn("    | MODEL_NAME='some/model'", out)
        self.assertNotIn("| GOOSE_MODE='", out, "Goose runs in its own default mode, which asks nothing (D-100)")
        out = m.run("goose", *m.args("goose", "--binary", str(m.core), "--dry-run", "--goose-mode", "approve",
                                     container=True)).stdout
        self.assertIn("    | GOOSE_MODE='approve'", out)
        self.assertEqual([c for c in m.calls() if c != "docker info"], [])

    def test_goose_mode_reaches_the_persons_goose(self):
        m = Machine(self)
        out = m.run("goose", *m.args("goose", "--binary", str(m.core), "--dry-run", "--goose-mode", "auto")).stdout
        self.assertIn("    | GOOSE_MODE='auto'", out)

    def test_containers_are_linux_only(self):
        m = Machine(self)
        result = m.run("pi", *m.args("pi", "--binary", str(m.core), "--dry-run", container=True),
                       env={"MARMOT_AGENT_OS": "Darwin"}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Linux only", result.stderr)

    def test_download_is_announced(self):
        m = Machine(self)
        out = m.run("codex", *m.args("codex", "--dry-run", "--version", "v9.9.9")).stdout
        self.assertIn("would download marmot-agent-", out)
        self.assertIn("v9.9.9", out)
        self.assertIn("SHA256SUMS", out)


class Install(unittest.TestCase):
    """A real run, into the throwaway HOME, against the stand-ins."""

    def test_install_again_and_uninstall(self):
        m = Machine(self)
        out = m.run("codex", *m.args("codex", "--binary", str(m.core))).stdout
        agent = m.data / "agents/codex"
        self.assertIn(AGENT_KEY, out, "the agent's key is shown, to invite it")
        self.assertEqual((agent / "public-key").read_text().strip(), AGENT_KEY)
        self.assertEqual(mode(m.data), 0o700)
        self.assertEqual(mode(agent), 0o700)
        for name in ("settings.toml", "service.env", "choices", "public-key"):
            self.assertEqual(mode(agent / name), 0o600, name)
        self.assertEqual(mode(m.home / ".local/bin/marmot-agent"), 0o755)
        self.assertTrue((m.data / "lib/install/codex.sh").is_file())
        self.assertFalse((m.data / "lib/tools").exists(), "Codex needs no container files")
        self.assertEqual(sorted(p.name for p in m.data.iterdir()), ["agents", "install.lock", "lib"],
                         "nothing staged is left")
        unit = m.units / "marmot-agent-codex.service"
        self.assertEqual(mode(unit), 0o644)
        self.assertIn(f'ExecStart="{agent}/start"', unit.read_text())
        self.assertEqual(mode(agent / "start"), 0o700)
        self.assertEqual((agent / "harness").read_text(), "codex\n")
        self.assertTrue((m.home / "work").is_dir())
        settings = (agent / "settings.toml").read_text()
        self.assertIn(f'owner = "{OWNER}"', settings)
        self.assertIn('relays = ["wss://relay.eu.whitenoise.chat", "wss://relay.us.whitenoise.chat"]', settings)
        self.assertEqual(
            m.calls(),
            [
                f"marmot-agent init {agent}/settings.toml",
                "systemctl --user daemon-reload",
                "systemctl --user enable marmot-agent-codex.service",
                "systemctl --user restart marmot-agent-codex.service",
            ],
        )
        verify_units(self, [unit])

        # Again: the settings and the identity are kept; the core is not made again.
        (agent / "settings.toml").write_text(settings + "# edited\n")
        m.log.write_text("")
        m.run("codex", "--yes", "--binary", str(m.core))
        self.assertTrue((agent / "settings.toml").read_text().endswith("# edited\n"))
        self.assertNotIn("marmot-agent init", "\n".join(m.calls()))
        self.assertIn("systemctl --user restart marmot-agent-codex.service", m.calls())

        # Uninstall: the service goes, the agent's folder stays.
        m.log.write_text("")
        out = m.run("codex", "--uninstall").stdout
        self.assertFalse(unit.exists())
        self.assertEqual(
            m.calls(),
            ["systemctl --user disable --now marmot-agent-codex.service", "systemctl --user daemon-reload"],
        )
        self.assertTrue((agent / "settings.toml").is_file())
        self.assertTrue((agent / "home").is_dir())
        self.assertIn(f"kept the agent's folder, with its settings and identity: {agent}", out)

    def test_socket_harness(self):
        m = Machine(self)
        m.run("claude-code", *m.args("claude-code", "--binary", str(m.core), "--name", "helper",
                                     "--display-name", 'My "helper"', "--permission-mode", "plan"))
        agent = m.data / "agents/helper"
        connector = agent / "connector"
        self.assertEqual(mode(connector), 0o700)
        text = connector.read_text()
        self.assertIn(f". '{agent}/service.env'", text)
        self.assertIn(f"exec '{m.bin}/node' '{m.data}/lib/packages/claude-code/src/main.ts'", text)
        self.assertIn(f"--home '{agent}/home' --cwd '{m.home}/work' --permission-mode 'plan'", text)
        self.assertIn('name = "My \\"helper\\""', (agent / "settings.toml").read_text())
        units = [m.units / "marmot-agent-helper.service", m.units / "marmot-agent-helper-connector.service"]
        for unit in units:
            self.assertTrue(unit.is_file())
        self.assertTrue(any(c.startswith("npm ci --omit=dev --no-fund --no-audit --prefix") for c in m.calls()))
        self.assertTrue((m.data / "lib/packages/claude-code/node_modules/a-package").is_dir())
        self.assertIn(
            "systemctl --user restart marmot-agent-helper.service marmot-agent-helper-connector.service", m.calls()
        )
        verify_units(self, units)
        # The choices not in the settings are remembered.
        m.log.write_text("")
        m.run("claude-code", "--yes", "--binary", str(m.core), "--name", "helper")
        self.assertIn("--permission-mode 'plan'", connector.read_text())
        m.run("claude-code", "--uninstall", "--name", "helper")
        for unit in units:
            self.assertFalse(unit.exists())

    def test_hermes(self):
        """Our plugin goes into the person's own Hermes, and comes out again; the rest of it is theirs."""
        m = Machine(self)
        hermes = m.home / ".hermes"
        hermes.mkdir()
        mine = {"model": {"default": "their-model"}, "platforms": {"telegram": {"enabled": True}}}
        (hermes / "config.yaml").write_text(json.dumps(mine))
        (hermes / ".env").write_text("THEIR_KEY=x\n")
        out = m.run("hermes", *m.args("hermes", "--binary", str(m.core))).stdout
        agent = m.data / "agents/hermes"
        config = json.loads((hermes / "config.yaml").read_text())
        self.assertEqual(config["platforms"]["marmot"], {"enabled": True, "home": f"{agent}/home"})
        self.assertEqual(config["platforms"]["telegram"], {"enabled": True})
        self.assertEqual(config["model"], mine["model"])
        self.assertIn("marmot", config["plugins"]["enabled"])
        self.assertEqual((hermes / ".env").read_text(), "THEIR_KEY=x\nMARMOT_ALLOW_ALL_USERS=true\n")
        plugin = hermes / "plugins/marmot"
        self.assertEqual(sorted(p.name for p in plugin.iterdir()),
                         sorted(p.name for p in (ROOT / "python/marmot_hermes").iterdir() if p.name != "__pycache__"))
        backups = sorted(p.name.split(".2")[0] for p in (agent / "backups").iterdir())
        self.assertEqual(backups, [".env", "config.yaml"], "their settings are copied before they change")
        self.assertEqual(json.loads(next((agent / "backups").glob("config.yaml.*")).read_text()), mine)
        self.assertEqual(mode(agent / "backups"), 0o700)
        self.assertIn("hermes gateway restart", out)
        self.assertEqual([u.name for u in m.units.iterdir()], ["marmot-agent-hermes.service"],
                         "the person's gateway is no service of ours")
        self.assertFalse((agent / "connector").exists())
        self.assertNotIn("cwd", (agent / "settings.toml").read_text())
        # Again: nothing new is recorded, and the plugin is replaced by the one installed now.
        (plugin / "stale.py").write_text("")
        m.run("hermes", "--yes", "--binary", str(m.core))
        self.assertFalse((plugin / "stale.py").exists())
        # Uninstall: what was added goes, and what was theirs stays.
        m.log.write_text("")
        m.run("hermes", "--uninstall")
        config = json.loads((hermes / "config.yaml").read_text())
        self.assertNotIn("marmot", config["platforms"])
        self.assertEqual(config["platforms"]["telegram"], {"enabled": True})
        self.assertNotIn("marmot", config["plugins"]["enabled"])
        self.assertEqual((hermes / ".env").read_text(), "THEIR_KEY=x\n")
        self.assertFalse(plugin.exists())
        self.assertIn("hermes config unset platforms.marmot", m.calls())

    def test_hermes_keeps_an_allow_all_it_had(self):
        # In each shape Hermes reads in its .env: put back with its value.
        for line in ("MARMOT_ALLOW_ALL_USERS=1", "export MARMOT_ALLOW_ALL_USERS=yes", "MARMOT_ALLOW_ALL_USERS = on"):
            with self.subTest(line=line):
                m = Machine(self)
                (m.home / ".hermes").mkdir()
                (m.home / ".hermes/.env").write_text(line + "\n")
                m.run("hermes", *m.args("hermes", "--binary", str(m.core), "--no-service"))
                self.assertEqual((m.home / ".hermes/.env").read_text(), "MARMOT_ALLOW_ALL_USERS=true\n")
                m.run("hermes", "--uninstall")
                value = line.split("=", 1)[1].strip()
                self.assertEqual((m.home / ".hermes/.env").read_text(), f"MARMOT_ALLOW_ALL_USERS={value}\n")

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_openclaw(self):
        """Our plugin goes into the person's own OpenClaw, and comes out again; the rest of it is theirs."""
        m = Machine(self)
        state = m.home / ".openclaw"
        state.mkdir()
        mine = {"channels": {"telegram": {"enabled": True}}, "agents": {"defaults": {"workspace": "/theirs"}}}
        (state / "openclaw.json").write_text(json.dumps(mine))
        out = m.run("openclaw", *m.args("openclaw", "--binary", str(m.core))).stdout
        agent = m.data / "agents/openclaw"
        config = json.loads((state / "openclaw.json").read_text())
        self.assertEqual(config["channels"]["marmot"], {"enabled": True, "home": f"{agent}/home"})
        self.assertEqual(config["channels"]["telegram"], {"enabled": True})
        self.assertEqual(config["agents"], mine["agents"], "their agent's settings are theirs")
        self.assertEqual(config["approvals"]["exec"],
                         {"enabled": True, "mode": "session", "sessionFilter": [":marmot:"]})
        self.assertEqual((state / "extensions/marmot/packed.tgz").read_text(), "a packed plugin\n")
        self.assertTrue(config["plugins"]["entries"]["marmot"]["enabled"])
        self.assertFalse([c for c in m.calls() if c.startswith("openclaw plugins enable")])
        lib = m.data / "lib/packages/openclaw"
        self.assertFalse((lib / "node_modules").exists(), "what only builds the plugin is removed")
        self.assertTrue(any(c.startswith("npm ci") for c in m.calls()))
        self.assertIn("npm pack --silent", m.calls())
        self.assertEqual(json.loads(next((agent / "backups").glob("openclaw.json.*")).read_text()), mine)
        self.assertIn("openclaw gateway restart", out)
        self.assertEqual([u.name for u in m.units.iterdir()], ["marmot-agent-openclaw.service"])
        # Uninstall: the plugin, its channel and the approvals the install switched on go.
        m.run("openclaw", "--uninstall")
        config = json.loads((state / "openclaw.json").read_text())
        self.assertEqual(config["channels"], mine["channels"])
        self.assertNotIn("exec", config.get("approvals", {}))
        self.assertFalse((state / "extensions/marmot").exists())

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_openclaw_keeps_the_approvals_it_was_given(self):
        m = Machine(self)
        state = m.home / ".openclaw"
        state.mkdir()
        # Forwarding that is on is the person's: kept, and kept again by the uninstall.
        on = {"enabled": True, "mode": "targets", "targets": [{"channel": "telegram", "to": "1"}]}
        (state / "openclaw.json").write_text(json.dumps({"approvals": {"exec": on}}))
        out = m.run("openclaw", *m.args("openclaw", "--binary", str(m.core), "--no-service")).stdout
        self.assertIn("forwards approvals already", out)
        self.assertEqual(json.loads((state / "openclaw.json").read_text())["approvals"]["exec"], on)
        m.run("openclaw", "--uninstall")
        self.assertEqual(json.loads((state / "openclaw.json").read_text())["approvals"]["exec"], on)
        # Forwarding that is off is switched on, and put back as it was.
        m = Machine(self)
        state = m.home / ".openclaw"
        state.mkdir()
        off = {"enabled": False, "mode": "targets"}
        (state / "openclaw.json").write_text(json.dumps({"approvals": {"exec": off}}))
        m.run("openclaw", *m.args("openclaw", "--binary", str(m.core), "--no-service"))
        self.assertTrue(json.loads((state / "openclaw.json").read_text())["approvals"]["exec"]["enabled"])
        m.run("openclaw", "--uninstall")
        self.assertEqual(json.loads((state / "openclaw.json").read_text())["approvals"]["exec"], off)

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_openclaw_keeps_approvals_forwarded_automatically(self):
        m = Machine(self)
        state = m.home / ".openclaw"
        state.mkdir()
        auto = {"enabled": "auto", "mode": "targets"}
        (state / "openclaw.json").write_text(json.dumps({"approvals": {"exec": auto}}))
        out = m.run("openclaw", *m.args("openclaw", "--binary", str(m.core), "--no-service")).stdout
        self.assertIn("forwards approvals already", out)
        self.assertEqual(json.loads((state / "openclaw.json").read_text())["approvals"]["exec"], auto)

    def test_npub_owner(self):
        m = Machine(self)
        m.run("opencode", "--yes", "--owner", NPUB, "--binary", str(m.core), "--no-service",
              "--cwd", str(m.home / "work"))
        self.assertIn(f'owner = "{NPUB_HEX}"', (m.data / "agents/opencode/settings.toml").read_text())
        self.assertFalse(m.units.exists(), "--no-service sets up no service")

    def test_bad_owner(self):
        m = Machine(self)
        bad = NPUB[:-1] + ("q" if NPUB[-1] != "q" else "p")
        for owner in ("abc", bad, "zz" * 32):
            with self.subTest(owner=owner):
                result = m.run("codex", "--yes", "--owner", owner, "--binary", str(m.core), check=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("--owner must be a public key", result.stderr)
        self.assertEqual(m.files(), [])

    def test_owner_is_required_without_a_person(self):
        m = Machine(self)
        result = m.run("codex", "--yes", "--binary", str(m.core), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("give your public key with --owner", result.stderr)

    def test_missing_harness_stops_before_anything(self):
        m = Machine(self, programs=())
        if shutil.which("codex", path=SYSTEM_PATH):
            self.skipTest("this machine has codex in /usr/bin")
        result = m.run("codex", *m.args("codex", "--binary", str(m.core)), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing: Codex", result.stderr)
        self.assertIn("codex login", result.stderr)
        self.assertEqual(m.files(), [])

    def test_each_missing_harness_says_how_to_get_it(self):
        hints = {"goose": ("missing: Goose", "goose configure"),
                 "pi": ("missing: Pi", "npm install -g @earendil-works/pi-coding-agent"),
                 "openclaw": ("missing: OpenClaw", "openclaw onboard"),
                 "hermes": ("missing: Hermes", "hermes setup")}
        for harness, (missing, how) in hints.items():
            with self.subTest(harness=harness):
                m = Machine(self, programs=())
                if shutil.which(harness, path=SYSTEM_PATH):
                    self.skipTest(f"this machine has {harness} in /usr/bin")
                result = m.run(harness, *m.args(harness, "--binary", str(m.core)), check=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(missing, result.stderr)
                self.assertIn(how, result.stderr)
                self.assertEqual(m.files(), [])
                self.assertEqual(m.calls(), [])

    def test_an_older_harness_is_named(self):
        m = Machine(self)
        result = m.run("pi", *m.args("pi", "--binary", str(m.core), "--dry-run"), env={"FAKE_VERSION": "1.5.9"})
        self.assertIn("Pi 1.5.9 is older than 1.6.0", result.stderr)
        result = m.run("goose", *m.args("goose", "--binary", str(m.core), "--dry-run"), env={"FAKE_VERSION": "1.60.0"})
        self.assertNotIn("older", result.stderr)
        # OpenClaw refuses a plugin for a newer release than itself, so an older one is missing.
        result = m.run("openclaw", *m.args("openclaw", "--binary", str(m.core)),
                       env={"FAKE_OPENCLAW_VERSION": "2026.10.10"}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing: OpenClaw 2026.11.5 or later", result.stderr)

    def test_model_options_go_with_a_container(self):
        m = Machine(self)
        for harness in CONTAINER:
            with self.subTest(harness=harness):
                result = m.run(harness, *m.args(harness, "--binary", str(m.core), "--model", "x"), check=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("go with --container", result.stderr)
        for harness in PLUGIN:
            with self.subTest(harness=harness):
                result = m.run(harness, *m.args(harness, "--binary", str(m.core), "--container"), check=False)
                self.assertIn(f"takes no --container", result.stderr)
                result = m.run(harness, *m.args(harness, "--binary", str(m.core), "--cwd", "/x"), check=False)
                self.assertIn("takes no --cwd", result.stderr)
        self.assertEqual(m.files(), [])

    def test_a_native_agent_stays_native(self):
        m = Machine(self)
        m.run("pi", *m.args("pi", "--binary", str(m.core), "--no-service"))
        result = m.run("pi", "--yes", "--binary", str(m.core), "--no-service", "--container", "--model-key",
                       str(m.key), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("runs the Pi on your PATH", result.stderr)
        # And one in a container stays there, run again without --container.
        m.run("goose", *m.args("goose", "--binary", str(m.core), "--no-service", container=True))
        m.run("goose", "--yes", "--binary", str(m.core), "--no-service")
        self.assertIn("goose-acp.sh", (m.data / "agents/goose/settings.toml").read_text())
        self.assertIn("MODEL_BASE_URL=", (m.data / "agents/goose/service.env").read_text())

    def test_goose_in_a_container_is_given_no_file_tool_it_cannot_reach(self):
        # Goose in its container cannot reach the loopback address the ACP driver's file tool
        # listens on, so the installer switches the tool off there; on the host it stays on (D-110).
        m = Machine(self)
        m.run("goose", *m.args("goose", "--binary", str(m.core), "--no-service", container=True))
        settings = (m.data / "agents/goose/settings.toml").read_text()
        self.assertIn("send_files = false", settings)
        m.run("opencode", *m.args("opencode", "--binary", str(m.core), "--no-service"))
        self.assertNotIn("send_files", (m.data / "agents/opencode/settings.toml").read_text())

    def test_claude_code_needs_a_stored_sign_in(self):
        m = Machine(self)
        shutil.rmtree(m.claude)
        result = m.run("claude-code", *m.args("claude-code", "--binary", str(m.core)),
                       env={"ANTHROPIC_API_KEY": "not a real key"}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing: a stored Claude Code sign-in", result.stderr)
        self.assertIn("the agent's service does not get it", result.stderr)
        self.assertIn("/login", result.stderr)
        result = m.run("claude-code", *m.args("claude-code", "--binary", str(m.core)), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing: a Claude Code sign-in", result.stderr)
        self.assertEqual(m.files(), [])
        self.assertNotIn("not a real key", "".join(p.read_text() for p in m.dir.rglob("*") if p.is_file()
                                                    and p != m.key))

    def test_old_node(self):
        m = Machine(self)
        result = m.run("claude-code", *m.args("claude-code", "--binary", str(m.core)),
                       env={"FAKE_NODE_MAJOR": "22"}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing: Node 24", result.stderr)

    def test_a_container_harness_needs_an_endpoint_and_a_model(self):
        # There is no default endpoint or model: one would send people's work to a service they did
        # not choose (D-100). Each is asked for by name, and nothing is installed without them.
        for harness in sorted(CONTAINER):
            for left_out, message in (("--model-url", "missing: a model endpoint"), ("--model", "missing: a model\n")):
                with self.subTest(harness=harness, left_out=left_out):
                    m = Machine(self)
                    args = m.args(harness, "--binary", str(m.core), container=True)
                    at = args.index(left_out)
                    del args[at:at + 2]
                    result = m.run(harness, *args, check=False)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(message, result.stderr)
                    self.assertIn(left_out, result.stderr, "says which option gives it")
                    self.assertEqual(m.files(), [])
                    self.assertNotIn("numes", result.stdout + result.stderr)

    def test_model_key_must_be_private(self):
        m = Machine(self)
        m.key.chmod(0o644)
        result = m.run("pi", *m.args("pi", "--binary", str(m.core), container=True), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("chmod 600", result.stderr)
        self.assertEqual(m.files(), [])

    def test_model_key_outside_the_work_folder(self):
        m = Machine(self)
        (m.home / "work").mkdir()
        key = m.home / "work/key"
        key.write_text("x")
        key.chmod(0o600)
        result = m.run("pi", "--yes", "--owner", OWNER, "--cwd", str(m.home / "work"), "--container",
                       "--model-key", str(key), "--binary", str(m.core), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not be inside the work folder", result.stderr)

    def test_unknown_option(self):
        m = Machine(self)
        result = m.run("codex", "--model-key", str(m.key), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("takes no --model-key", result.stderr)


class Boundaries(unittest.TestCase):
    """What keeps an install from handing a harness the keys, or another agent's service."""

    def refused(self, m: Machine, harness: str, *args: str, message: str) -> subprocess.CompletedProcess:
        result = m.run(harness, *args, check=False)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)
        return result

    def test_work_folder_is_compared_with_links_and_dots_resolved(self):
        m = Machine(self)
        project = m.dir / "project"
        (project / "sub").mkdir(parents=True)
        key = project / "model.key"
        key.write_text("x")
        key.chmod(0o600)
        (m.dir / "link").symlink_to(project)
        common = ["--yes", "--owner", OWNER, "--binary", str(m.core), "--container", "--model-key", str(key), *MODEL]
        inside = "must not be inside the work folder"
        # The key's folder, named through `..`, through a link, and through a link to a folder below it.
        self.refused(m, "goose", *common, "--cwd", f"{project}/sub/..", message=inside)
        self.refused(m, "goose", *common, "--cwd", str(m.dir / "link"), message=inside)
        self.refused(m, "pi", *common, "--cwd", f"{m.dir}/link/sub/../", message=inside)
        # The key named through a link from outside the work folder.
        outside = m.dir / "outside.key"
        outside.symlink_to(key)
        self.refused(m, "pi", "--yes", "--owner", OWNER, "--binary", str(m.core), "--container", "--model-key",
                     str(outside), "--cwd", str(project), message=inside)
        # The folder that holds the agents, named through a link, and a work folder inside it.
        (m.dir / "home-link").symlink_to(m.home)
        self.refused(m, "codex", "--yes", "--owner", OWNER, "--binary", str(m.core),
                     "--cwd", str(m.dir / "home-link"), message="must not hold")
        self.refused(m, "codex", "--yes", "--owner", OWNER, "--binary", str(m.core),
                     "--cwd", f"{m.home}/x/../.local/share/marmot-agent/agents/codex/work", message="must not be inside")
        self.assertEqual(m.files(), [])
        # A link to a folder that holds neither is fine, and is written resolved.
        (m.dir / "fine").mkdir()
        (m.dir / "fine-link").symlink_to(m.dir / "fine")
        m.run("pi", *common, "--cwd", f"{m.dir}/fine-link/.", "--no-service")
        self.assertIn(f'cwd = "{m.dir}/fine"', (m.data / "agents/pi/settings.toml").read_text())

    def test_the_account_home_is_written_without_links(self):
        m = Machine(self)
        link = m.dir / "home-link"
        link.symlink_to(m.home)
        m.run("codex", *m.args("codex", "--binary", str(m.core), "--no-service"), env={"HOME": str(link)})
        self.assertIn(f'home = "{m.data}/agents/codex/home"', (m.data / "agents/codex/settings.toml").read_text())

    def test_another_harness_keeps_claude_codes_packages(self):
        m = Machine(self)
        m.run("claude-code", *m.args("claude-code", "--binary", str(m.core)))
        packages = m.data / "lib/packages/claude-code/node_modules/a-package"
        self.assertTrue(packages.is_dir())
        m.run("pi", *m.args("pi", "--binary", str(m.core), container=True))
        self.assertTrue(packages.is_dir(), "installing Pi leaves the Claude Code driver's packages")
        self.assertTrue((m.data / "lib/packages/claude-code/src/main.ts").is_file())
        self.assertTrue((m.data / "lib/tools/pi/pi.sh").is_file())

    def test_a_name_belongs_to_one_harness(self):
        m = Machine(self)
        m.run("codex", *m.args("codex", "--binary", str(m.core), "--name", "helper"))
        before = m.files()
        m.log.write_text("")
        for args in (m.args("goose", "--binary", str(m.core), "--name", "helper"), ["--uninstall", "--name", "helper"]):
            with self.subTest(args=args[0]):
                self.refused(m, "goose", *args, message='the agent "helper" runs codex')
        self.assertEqual(m.files(), before)
        self.assertEqual(m.calls(), [])

    def test_a_saved_choice_is_only_data(self):
        m = Machine(self)
        work = m.home / "$(touch pwned)`touch pwned2`$HOME"
        m.run("claude-code", "--yes", "--owner", OWNER, "--binary", str(m.core), "--cwd", str(work))
        m.run("claude-code", "--yes", "--binary", str(m.core))
        self.assertTrue(work.is_dir())
        self.assertIn(f"--cwd '{work}'", (m.data / "agents/claude-code/connector").read_text())
        self.assertEqual(list(m.home.glob("pwned*")) + list(Path.cwd().glob("pwned*")), [])

    def test_a_bad_model_url_stops_before_anything(self):
        m = Machine(self)
        result = m.run("goose", *m.args("goose", "--binary", str(m.core), "--model-url", "ftp://models.example",
                                        container=True), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--model-url must start with http:// or https://", result.stderr)
        self.assertEqual(m.files(), [])
        self.assertEqual([c for c in m.calls() if c != "docker info"], [])

    def test_a_dry_run_leaves_a_data_folder_whose_name_has_new_in_it(self):
        m = Machine(self)
        data = m.dir / "data.new.1"
        installed = data / "marmot-agent/lib/install/codex.sh"
        installed.parent.mkdir(parents=True)
        installed.write_text("installed\n")
        m.run("codex", *m.args("codex", "--binary", str(m.core), "--dry-run"), env={"XDG_DATA_HOME": str(data)})
        self.assertEqual(installed.read_text(), "installed\n")

    def test_a_work_folder_or_key_given_again_is_written_into_the_kept_settings(self):
        m = Machine(self)
        m.run("pi", *m.args("pi", "--binary", str(m.core), "--no-service", container=True))
        settings = m.data / "agents/pi/settings.toml"
        edited = settings.read_text() + "# edited\n"
        settings.write_text(edited)
        # Run again with nothing new: the settings are kept as they are.
        m.run("pi", "--yes", "--binary", str(m.core), "--no-service")
        self.assertEqual(settings.read_text(), edited)
        # A new work folder and key file: those, and nothing else, change.
        key = m.dir / "other.key"
        key.write_text("another\n")
        key.chmod(0o600)
        out = m.run("pi", "--yes", "--binary", str(m.core), "--no-service", "--cwd", str(m.dir / "elsewhere"),
                    "--model-key", str(key)).stdout
        self.assertIn("with the work folder or the model key file given now", out)
        expected = edited.replace(f'cwd = "{m.home}/work"', f'cwd = "{m.dir}/elsewhere"').replace(
            f'"{m.key}"', f'"{key}"')
        self.assertNotEqual(expected, edited)
        self.assertEqual(settings.read_text(), expected)
        self.assertIn(f"MODEL_KEY={key}", (m.data / "agents/pi/choices").read_text())
        # Once more with nothing new: kept.
        m.run("pi", "--yes", "--binary", str(m.core), "--no-service")
        self.assertEqual(settings.read_text(), expected)

    def test_a_key_the_settings_no_longer_name_is_refused(self):
        m = Machine(self)
        m.run("goose", *m.args("goose", "--binary", str(m.core), "--no-service", container=True))
        agent = m.data / "agents/goose"
        settings = agent / "settings.toml"
        settings.write_text(settings.read_text().replace(f'"{m.key}"', '"/somewhere/else.key"'))
        before = settings.read_text(), (agent / "choices").read_text()
        key = m.dir / "other.key"
        key.write_text("another\n")
        key.chmod(0o600)
        result = m.run("goose", "--yes", "--binary", str(m.core), "--no-service", "--model-key", str(key),
                       check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not name the model key file the agent was installed with", result.stderr)
        self.assertEqual((settings.read_text(), (agent / "choices").read_text()), before)

    @staticmethod
    def imported(m: Machine, harness: str, title: str) -> tuple:
        """An install of today's connector moved over as docs/move.md says: its home where it was, and
        the settings `marmot-agent import --settings` wrote in the agent's folder."""
        old_home = m.home / f".marmot-agents/{harness}"
        (old_home / "accounts").mkdir(parents=True)
        settings = m.data / f"agents/{harness}/settings.toml"
        settings.parent.mkdir(parents=True)
        settings.write_text(f"# Imported by `marmot-agent import` from a {title} install, at 20261122T000000Z.\n"
                            f"# The home before the import is backed up in {old_home}.backup-20261122T000000Z.\n"
                            f'home = "{old_home}"\nrelays = ["wss://relay.example"]\nowner = "{OWNER}"\n'
                            '\n[connector]\n')
        return old_home, settings

    def test_another_plugin_called_marmot_is_not_replaced_unasked(self):
        """Today's connectors put a plugin called marmot into Hermes and OpenClaw (docs/move.md)."""
        m = Machine(self)
        old = m.home / ".hermes/plugins/marmot"
        old.mkdir(parents=True)
        (old / "plugin.yaml").write_text("name: marmot\ndescription: the old connector's\n")
        self.refused(m, "hermes", *m.args("hermes", "--binary", str(m.core)), message="--replace-plugin")
        self.assertEqual(m.calls(), [])
        self.assertFalse(m.data.exists())
        # Replacing it without moving the install over would make a new identity, in none of its chats.
        self.refused(m, "hermes", *m.args("hermes", "--binary", str(m.core), "--replace-plugin"),
                     message="marmot-agent import")
        self.assertEqual(m.calls(), [])
        self.assertFalse(m.data.exists())
        old_home, settings = self.imported(m, "hermes", "Hermes")
        before = settings.read_text()
        m.run("hermes", *m.args("hermes", "--binary", str(m.core), "--no-service", "--replace-plugin"))
        kept = next((m.data / "agents/hermes/backups").glob("marmot.*"))
        self.assertEqual((kept / "plugin.yaml").read_text(), "name: marmot\ndescription: the old connector's\n")
        self.assertIn("marmot-agent's local socket", (old / "plugin.yaml").read_text())
        # The imported identity and home are kept, and the gateway points at that home.
        config = m.home / ".hermes/config.yaml"
        self.assertEqual(json.loads(config.read_text())["platforms"]["marmot"]["home"], str(old_home))
        self.assertEqual(settings.read_text(), before)
        self.assertFalse([c for c in m.calls() if c.startswith("marmot-agent init")])
        self.assertFalse((m.data / "agents/hermes/home").exists())
        self.assertEqual((m.data / "agents/hermes/harness").read_text(), "hermes\n")
        # Again, and then uninstalled: the same home, and no identity made.
        m.run("hermes", "--yes", "--binary", str(m.core), "--no-service")
        self.assertEqual(json.loads(config.read_text())["platforms"]["marmot"]["home"], str(old_home))
        self.assertFalse([c for c in m.calls() if c.startswith("marmot-agent init")])
        m.run("hermes", "--uninstall")
        self.assertNotIn("marmot", json.loads(config.read_text())["platforms"])

    def test_a_failed_hermes_replacement_puts_the_old_plugin_back(self):
        """Our plugin goes in place of the old one first; if a step after fails, both the old plugin and
        Hermes's settings come back, and run again, it finishes the move."""
        m = Machine(self)
        old = m.home / ".hermes/plugins/marmot"
        old.mkdir(parents=True)
        (old / "plugin.yaml").write_text("name: marmot\ndescription: the old connector's\n")
        self.imported(m, "hermes", "Hermes")
        config, env = m.home / ".hermes/config.yaml", m.home / ".hermes/.env"
        config.write_text(json.dumps({"plugins": {"enabled": ["marmot"]}, "platforms": {"marmot": {"enabled": True}}}))
        env.write_text(f"MARMOT_ALLOWED_USERS={OWNER}\n")
        before = config.read_text(), env.read_text()
        args = m.args("hermes", "--binary", str(m.core), "--no-service", "--replace-plugin")
        result = m.run("hermes", *args, env={"FAKE_HERMES_FAIL": "plugins enable"}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("hermes plugins enable marmot", m.calls())
        self.assertEqual((old / "plugin.yaml").read_text(), "name: marmot\ndescription: the old connector's\n")
        self.assertEqual((config.read_text(), env.read_text()), before)
        backups = m.data / "agents/hermes/backups"
        self.assertIn("put your Hermes's settings and its old plugin back", result.stderr)
        self.assertIn(str(next(backups.glob("marmot.*"))), result.stderr)
        self.assertFalse((m.data / "agents/hermes/plugin-change").exists())
        m.run("hermes", *args)
        self.assertIn("marmot-agent's local socket", (old / "plugin.yaml").read_text())
        self.assertIn("MARMOT_ALLOW_ALL_USERS=true", env.read_text())

    def test_a_move_given_up_is_not_put_back_later(self):
        """A move killed part way, then given up as docs/move.md says (--uninstall, and the copies listed in
        before-move put back), is forgotten: a later move that fails puts back what the gateway had then,
        not what it had before the first."""
        m = Machine(self)
        hermes = m.home / ".hermes"
        old = hermes / "plugins/marmot"
        old.mkdir(parents=True)
        (old / "plugin.yaml").write_text("name: marmot\ndescription: the old connector's\n")
        self.imported(m, "hermes", "Hermes")
        config = hermes / "config.yaml"
        config.write_text(json.dumps({"plugins": {"enabled": ["marmot"]}, "platforms": {"marmot": {"enabled": True}}}))
        (hermes / ".env").write_text(f"MARMOT_ALLOWED_USERS={OWNER}\n")
        args = m.args("hermes", "--binary", str(m.core), "--no-service", "--replace-plugin")
        killed = m.run("hermes", *args, env={"FAKE_HERMES_KILL": "plugins enable"}, check=False)
        self.assertEqual(killed.returncode, -9, killed.stderr)
        agent = m.data / "agents/hermes"
        self.assertTrue((agent / "plugin-change").exists())
        uninstalled = m.run("hermes", "--uninstall")
        self.assertFalse((agent / "plugin-change").exists())
        self.assertIn("plugin-change", uninstalled.stdout)
        for line in (agent / "backups/before-move").read_text().splitlines():
            kind, path, copy = line.split("\t")
            if kind == "folder":
                shutil.rmtree(path, ignore_errors=True)
                shutil.copytree(copy, path)
            else:
                shutil.copy(copy, path)
        self.assertEqual((old / "plugin.yaml").read_text(), "name: marmot\ndescription: the old connector's\n")
        # The person goes on with the old connector, and adds a platform.
        now = {"plugins": {"enabled": ["marmot"]}, "platforms": {"marmot": {"enabled": True}, "telegram": {"enabled": True}}}
        config.write_text(json.dumps(now))
        failed = m.run("hermes", *args, env={"FAKE_HERMES_FAIL": "plugins enable"}, check=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(json.loads(config.read_text()), now)
        self.assertEqual((old / "plugin.yaml").read_text(), "name: marmot\ndescription: the old connector's\n")

    def test_settings_imported_for_another_harness_are_refused(self):
        m = Machine(self)
        self.imported(m, "hermes", "OpenClaw")
        self.refused(m, "hermes", *m.args("hermes", "--binary", str(m.core)), message="no record of its harness")

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_another_openclaw_plugin_called_marmot_is_not_replaced_unasked(self):
        m = Machine(self)
        old = m.home / ".openclaw/extensions/marmot"
        old.mkdir(parents=True)
        (old / "package.json").write_text(json.dumps({"name": "@marmot-protocol/openclaw-marmot"}))
        self.refused(m, "openclaw", *m.args("openclaw", "--binary", str(m.core)), message="--replace-plugin")
        self.assertEqual(m.calls(), [])
        old_home, settings = self.imported(m, "openclaw", "OpenClaw")
        # Today's connector points the channel at its home, which the import keeps, with its socket, its
        # account and who may write, as its installer and the person set them.
        old_channel = {"enabled": True, "home": str(old_home), "socketPath": f"{old_home}/dev/wn-agent.sock",
                       "accountIdHex": "ab" * 32, "dm": {"allowFrom": [OWNER]},
                       "senderPolicy": {"allowedUsers": [OWNER]}}
        (m.home / ".openclaw/openclaw.json").write_text(json.dumps({"channels": {"marmot": old_channel}}))
        m.run("openclaw", *m.args("openclaw", "--binary", str(m.core), "--no-service", "--replace-plugin"))
        kept = next((m.data / "agents/openclaw/backups").glob("marmot.*"))
        self.assertEqual(json.loads((kept / "package.json").read_text())["name"], "@marmot-protocol/openclaw-marmot")
        config = json.loads((m.home / ".openclaw/openclaw.json").read_text())
        self.assertEqual(config["channels"]["marmot"], {"enabled": True, "home": str(old_home)})
        self.assertFalse([c for c in m.calls() if c.startswith("marmot-agent init")])
        m.run("openclaw", "--uninstall")
        self.assertNotIn("marmot", json.loads((m.home / ".openclaw/openclaw.json").read_text())["channels"])

    @staticmethod
    def old_openclaw(m: Machine):
        """An OpenClaw with today's connector's plugin in it, its channel as that plugin's installer and the
        person set it, and the install imported. Gives the old home, the channel and the plugin's folder."""
        old = m.home / ".openclaw/extensions/marmot"
        old.mkdir(parents=True)
        (old / "package.json").write_text(json.dumps({"name": "@marmot-protocol/openclaw-marmot"}))
        old_home, _ = Boundaries.imported(m, "openclaw", "OpenClaw")
        old_channel = {"enabled": True, "home": str(old_home), "socketPath": f"{old_home}/dev/wn-agent.sock",
                       "accountIdHex": "ab" * 32, "dm": {"allowFrom": [OWNER]},
                       "senderPolicy": {"allowedUsers": [OWNER]}}
        (m.home / ".openclaw/openclaw.json").write_text(json.dumps({"channels": {"marmot": old_channel}}))
        return old_home, old_channel, old

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_a_failed_openclaw_replacement_puts_the_old_plugin_back(self):
        """The old channel comes out before our plugin goes in; if that fails, both come back."""
        m = Machine(self)
        _, old_channel, old = self.old_openclaw(m)
        result = m.run("openclaw", *m.args("openclaw", "--binary", str(m.core), "--no-service", "--replace-plugin"),
                       env={"FAKE_OPENCLAW_FAIL": "plugins install"}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("openclaw config unset channels.marmot", m.calls())
        config = json.loads((m.home / ".openclaw/openclaw.json").read_text())
        self.assertEqual(config["channels"]["marmot"], old_channel)
        self.assertEqual(json.loads((old / "package.json").read_text())["name"], "@marmot-protocol/openclaw-marmot")
        # It says so, and where the copies are, should putting them back fail.
        backups = m.data / "agents/openclaw/backups"
        self.assertIn("put your OpenClaw's settings and its old plugin back", result.stderr)
        self.assertIn(str(next(backups.glob("openclaw.json.*"))), result.stderr)
        self.assertIn(str(next(backups.glob("marmot.*"))), result.stderr)
        # Run again, it finishes the move.
        m.run("openclaw", *m.args("openclaw", "--binary", str(m.core), "--no-service", "--replace-plugin"))
        config = json.loads((m.home / ".openclaw/openclaw.json").read_text())
        self.assertEqual(set(config["channels"]["marmot"]), {"enabled", "home"})

    def test_the_openclaw_channel_keys_are_our_plugins(self):
        """What a half-done move leaves is what our plugin's channel schema does not take."""
        schema = json.loads((ROOT / "packages/openclaw/openclaw.plugin.json").read_text())
        keys = set(schema["channelConfigs"]["marmot"]["schema"]["properties"])
        line = next(l for l in (ROOT / "install/openclaw.sh").read_text().splitlines() if l.startswith("OUR_CHANNEL_KEYS="))
        self.assertEqual(set(line.split("=", 1)[1].strip('"').split()), keys)

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_a_half_done_openclaw_move_is_finished(self):
        """Our plugin in, and the old plugin's settings left in its channel, which OpenClaw then refuses to
        read or change at all: a run takes out what our plugin does not take, from the file itself."""
        m = Machine(self)
        old_home, old_channel, old = self.old_openclaw(m)
        (old / "package.json").write_text(json.dumps({"name": "marmot-agent-openclaw"}))
        config_file = m.home / ".openclaw/openclaw.json"
        mine = {"channels": {"marmot": old_channel, "other": {"token": "theirs"}}, "gateway": {"port": 18789}}
        config_file.write_text(json.dumps(mine))
        out = m.run("openclaw", *m.args("openclaw", "--binary", str(m.core), "--no-service", "--replace-plugin")).stdout
        config = json.loads(config_file.read_text())
        self.assertEqual(config["channels"]["marmot"], {"enabled": True, "home": str(old_home)})
        self.assertEqual(config["channels"]["other"], {"token": "theirs"})
        self.assertEqual(config["gateway"], {"port": 18789})
        self.assertIn("accountIdHex, dm, senderPolicy, socketPath", out)
        # The copy kept first has them.
        kept = sorted((m.data / "agents/openclaw/backups").glob("openclaw.json.*"))
        self.assertEqual(len(kept), 2)
        self.assertEqual(json.loads(kept[0].read_text()), mine)
        self.assertNotIn("openclaw config unset channels.marmot", m.calls())

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_an_openclaw_plugin_found_in_a_load_path_counts(self):
        """OpenClaw loads a plugin from plugins.load.paths with no install record, in place of one it installed."""
        m = Machine(self)
        found = m.home / "plugins/marmot"
        found.mkdir(parents=True)
        (found / "package.json").write_text(json.dumps({"name": "@marmot-protocol/openclaw-marmot"}))
        (m.home / ".openclaw").mkdir()
        (m.home / ".openclaw/openclaw.json").write_text(json.dumps({"plugins": {"load": {"paths": [str(found)]}}}))
        self.imported(m, "openclaw", "OpenClaw")
        result = self.refused(m, "openclaw", *m.args("openclaw", "--binary", str(m.core), "--replace-plugin"),
                              message="plugins.load.paths")
        self.assertIn(str(found), result.stderr)
        self.assertEqual(m.calls(), [])

    def test_one_gateway_serves_one_agent(self):
        m = Machine(self)
        m.run("hermes", *m.args("hermes", "--binary", str(m.core), "--no-service"))
        before = (m.home / ".hermes/config.yaml").read_text()
        self.refused(m, "hermes", *m.args("hermes", "--binary", str(m.core), "--name", "second"),
                     message="already connects to the agent whose home is")
        self.assertEqual((m.home / ".hermes/config.yaml").read_text(), before)
        # Uninstalling the second leaves the first in place.
        m.run("hermes", "--uninstall", "--name", "second")
        self.assertEqual((m.home / ".hermes/config.yaml").read_text(), before)

    def test_goose_needs_x86_64(self):
        m = Machine(self)
        result = m.run("goose", *m.args("goose", "--binary", str(m.core), container=True),
                       env={"MARMOT_AGENT_MACHINE": "aarch64"}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("x86_64 only", result.stderr)
        self.assertEqual(m.files(), [])

    def test_without_a_service_the_launcher_has_the_environment(self):
        m = Machine(self)
        out = m.run("goose", *m.args("goose", "--binary", str(m.core), "--no-service",
                                     "--model-url", "https://models.example/v1", container=True)).stdout
        start = m.data / "agents/goose/start"
        self.assertIn(f"Run it with: {start}", out)
        m.log.write_text("")
        subprocess.run([str(start)], check=True, env={"PATH": SYSTEM_PATH, "FAKE_LOG": str(m.log)})
        self.assertIn("with MODEL_BASE_URL=https://models.example/v1", m.calls())


class Interrupted(unittest.TestCase):
    """A move from today's connector's plugin (--replace-plugin) stopped at any step leaves the gateway
    with the old plugin or ours, set up to start, and the same command run again finishes the move. Each
    run is stopped at one step, for every step the script takes: by SIGTERM, after which it puts back
    what it changed, and by SIGKILL, after which the next run finishes the change or, should that fail,
    puts back what the killed run found. Ours is the move finished: our plugin set up to start, and the
    core's service enabled and started. A run killed after ours is in and before the core is started
    leaves the change noted as under way, and the next run finishes it."""

    # Our plugin is in, set up to start, and the core it points at is not yet: a SIGKILL may leave it,
    # with the change noted as under way, so that the next run finishes it.
    STARTING = "ours, its core not started"

    def every_step(self, harness: str, make, state, done, fail: dict, windows=(), mv=None) -> None:
        """make(m) sets up the old install and gives what state(m, made) and done(m, made) need. state
        says "old", "ours" (our plugin in and set up to start; the core's service is checked here) or
        what is wrong; done lists what else is wrong once the move is finished. fail is the environment
        that makes the stand-in fail a step that comes after the old plugin is out. windows are states a
        SIGKILL may leave that no order of the gateway's own commands avoids, each left by exactly one
        step, and finished by the next run. mv, given, stands in for mv."""

        def machine() -> Machine:
            m = Machine(self)
            if mv:
                m.write(m.bin / "mv", mv)
            return m

        m = machine()
        made = make(m)
        args = m.args(harness, "--binary", str(m.core), "--replace-plugin")
        m.run(harness, *args, stop=("KILL", 0))
        steps = int(m.steps.read_text())
        self.assertGreater(steps, 10)
        cases = [(signal, step) for signal in ("TERM", "KILL") for step in range(1, steps + 1)]
        agent = f"agents/{harness}"

        def started(m: Machine) -> bool:
            unit = f"marmot-agent-{harness}.service"
            calls = m.calls()
            return f"systemctl --user enable {unit}" in calls and f"systemctl --user restart {unit}" in calls

        def full(m: Machine, made) -> str:
            after = state(m, made)
            return self.STARTING if after == "ours" and not started(m) else after

        def snapshot(path: Path):
            if path.is_dir():
                return {str(f.relative_to(path)): f.read_text() for f in sorted(path.rglob("*")) if f.is_file()}
            return path.read_text() if path.is_file() else None

        def found(m: Machine) -> dict:
            """What the move changes, as the gateway has it."""
            gateway, changed = CHANGED[harness]
            return {str(m.home / gateway / name): snapshot(m.home / gateway / name) for name in changed}

        def finished(m: Machine, made, before: dict) -> list:
            """What is wrong once a run said it finished: the move, and the copies going back names."""
            wrong = [] if full(m, made) == "ours" else [full(m, made)]
            wrong += done(m, made)
            if (m.data / agent / "plugin-change").exists():
                wrong.append("the change is still marked as under way")
            if (m.data / agent / "backups/.copying").exists():
                wrong.append("a copy half made is left")
            # Going back puts each copy before-move lists in its place (docs/move.md): what the gateway had.
            listed = m.data / agent / "backups/before-move"
            lines = [line.split("\t") for line in listed.read_text().splitlines()] if listed.exists() else []
            if not lines:
                wrong.append("no list of the copies kept before the move")
            for kind, path, copy in lines:
                if not copy or snapshot(Path(copy)) != before.get(path):
                    wrong.append(f"before-move does not keep {path} as it was")
            if sorted(path for _, path, _ in lines) != sorted(before):
                wrong.append(f"before-move lists {[path for _, path, _ in lines]}, not {sorted(before)}")
            return wrong

        def one(case):
            signal, step = case
            m = machine()
            made = make(m)
            before = found(m)
            stopped = m.run(harness, *args, stop=(signal, step), check=False)
            after = full(m, made)
            under_way = (m.data / agent / "plugin-change").exists()
            plain = failed = after_failure = None
            if signal == "KILL" and (under_way or after not in ("old", "ours")):
                # Killed the same way and run again as it was, it finishes, from the copies the killed run
                # took: it takes no more of the old plugin.
                other = machine()
                other_made = make(other)
                other_before = found(other)
                other.run(harness, *args, stop=(signal, step), check=False)
                noted = (other.data / agent / "plugin-change").exists()
                kept = sorted(p.name for p in (other.data / agent / "backups").glob("marmot.*"))
                result = other.run(harness, *args, check=False)
                plain = (result, finished(other, other_made, other_before),
                         not noted or kept == sorted(p.name for p in (other.data / agent / "backups").glob("marmot.*")))
                failed = m.run(harness, *args, env=fail, check=False)
                after_failure = full(m, made)
            again = m.run(harness, *args, check=False)
            wrong = finished(m, made, before) if again.returncode == 0 else ["the run again failed"]
            return case, stopped, after, under_way, plain, failed, after_failure, again, wrong

        in_windows = []
        with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            for (signal, step), stopped, after, under_way, plain, failed, after_failure, again, wrong in pool.map(one, cases):
                with self.subTest(signal=signal, step=step):
                    said = f"after SIG{signal} at step {step}:\n{stopped.stdout}\n{stopped.stderr}"
                    self.assertNotEqual(stopped.returncode, 0, "the run was stopped")
                    if signal == "KILL" and after in windows:
                        in_windows.append(after)
                    elif signal == "KILL" and after == self.STARTING:
                        self.assertTrue(under_way, f"the change is noted as under way until the core is started {said}")
                    else:
                        self.assertIn(after, ("old", "ours"), said)
                    if signal == "TERM":
                        self.assertFalse(under_way, f"a change put back or finished {said}")
                    if plain is not None:
                        result, plain_wrong, same_copies = plain
                        self.assertEqual(result.returncode, 0, f"{said}\nthen again:\n{result.stdout}\n{result.stderr}")
                        self.assertEqual(plain_wrong, [], f"{said}\nthen again:\n{result.stdout}")
                        self.assertTrue(same_copies, f"the next run took more copies of the plugin, {said}")
                    if failed is not None:
                        # What the killed run found is put back, not what it left.
                        self.assertNotEqual(failed.returncode, 0)
                        self.assertEqual(after_failure, "old", f"{said}\nthen failing:\n{failed.stdout}\n{failed.stderr}")
                    self.assertEqual(again.returncode, 0, again.stderr)
                    self.assertEqual(wrong, [], again.stdout)
        self.assertEqual(sorted(in_windows), sorted(windows), "each window is left by one step")

    def hermes(self, mv=None, windows=()):
        ours = sorted(str(f.relative_to(ROOT / "python/marmot_hermes")) for f in (ROOT / "python/marmot_hermes").rglob("*")
                      if f.is_file() and "__pycache__" not in f.parts and not f.name.startswith("test_"))
        old_plugin = {"plugin.yaml": "name: marmot\ndescription: the old connector's\n", "adapter.py": "# the old one\n"}

        def make(m: Machine):
            # As MDK's install-hermes-marmot.sh leaves it: the plugin enabled, its platform's settings in
            # extra, and who may write to it in .env (docs/reference/current-connectors.md).
            old_home, _ = Boundaries.imported(m, "hermes", "Hermes")
            hermes = m.home / ".hermes"
            (hermes / "plugins/marmot").mkdir(parents=True)
            for name, text in old_plugin.items():
                (hermes / "plugins/marmot" / name).write_text(text)
            extra = {"home": str(old_home), "socket_path": f"{old_home}/dev/wn-agent.sock", "account_id_hex": "cd" * 32}
            (hermes / "config.yaml").write_text(json.dumps({
                "plugins": {"enabled": ["marmot"]},
                "platforms": {"marmot": {"enabled": True, "extra": extra}, "telegram": {"enabled": True}}}))
            (hermes / ".env").write_text(f"THEIR_KEY=x\nMARMOT_ALLOWED_USERS={OWNER}\n")
            return old_home, extra

        def plugin(m: Machine):
            folder = m.home / ".hermes/plugins/marmot"
            return {str(f.relative_to(folder)): f.read_text() for f in folder.rglob("*") if f.is_file()} if folder.is_dir() else {}

        def state(m: Machine, made):
            old_home, extra = made
            hermes = m.home / ".hermes"
            config = json.loads((hermes / "config.yaml").read_text())
            env = (hermes / ".env").read_text().splitlines()
            platform = config.get("platforms", {}).get("marmot", {})
            files = plugin(m)
            if not ("marmot" in config.get("plugins", {}).get("enabled", []) and platform.get("enabled") is True):
                return f"the plugin or its platform is not enabled: {config}"
            if config["platforms"].get("telegram") != {"enabled": True}:
                return "their other settings changed"
            if files == old_plugin:
                # The old one, as it was set up: never with everyone let through.
                if platform.get("extra") != extra or env != ["THEIR_KEY=x", f"MARMOT_ALLOWED_USERS={OWNER}"]:
                    return f"the old plugin, with settings changed: {platform}, {env}"
                return "old"
            if sorted(files) == ours:
                # Hermes reads a platform's other keys and its extra into one, extra winning (gateway/config.py).
                home = {**platform, **platform.get("extra", {})}.get("home")
                return "ours" if home == str(old_home) else f"our plugin, pointed at {home}"
            return f"neither plugin: {sorted(files)}"

        def done(m: Machine, made):
            wrong = []
            hermes = m.home / ".hermes"
            if "MARMOT_ALLOW_ALL_USERS=true" not in (hermes / ".env").read_text().splitlines():
                wrong.append("not everyone the core lets through is let through")
            leftovers = sorted(p.name for p in hermes.iterdir() if p.name.startswith(".marmot-agent"))
            leftovers += [p.name for p in (hermes / "plugins").iterdir() if p.name != "marmot"]
            if leftovers:
                wrong.append(f"left over: {leftovers}")
            return wrong

        self.every_step("hermes", make, state, done, fail={"FAKE_HERMES_FAIL": "plugins enable"}, windows=windows, mv=mv)

    def test_hermes(self):
        """Where mv cannot exchange two folders, ours goes in by two renames, with neither between."""
        self.hermes(mv=MV_WITHOUT_EXCHANGE, windows=("neither plugin: []",))

    @unittest.skipUnless(MV_EXCHANGES, "mv cannot exchange two folders here (GNU coreutils 9.5 and later can)")
    def test_hermes_exchanging(self):
        """Where mv can exchange two folders, ours goes in in one step, and Hermes always has a plugin."""
        self.hermes()

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_openclaw(self):
        def make(m: Machine):
            old_home, old_channel, _ = Boundaries.old_openclaw(m)
            return old_home, old_channel

        def state(m: Machine, made):
            old_home, old_channel = made
            state = m.home / ".openclaw"
            config = json.loads((state / "openclaw.json").read_text())
            channel = config.get("channels", {}).get("marmot")
            package = state / "extensions/marmot/package.json"
            name = json.loads(package.read_text()).get("name") if package.is_file() else None
            if name == "@marmot-protocol/openclaw-marmot":
                return "old" if channel == old_channel else f"the old plugin, with its channel {channel}"
            if name == "marmot-agent-openclaw":
                entry = config.get("plugins", {}).get("entries", {}).get("marmot", {})
                if channel == {"enabled": True, "home": str(old_home)} and entry.get("enabled") is True:
                    return "ours"
                return f"our plugin, with its channel {channel}"
            return "neither plugin"

        # OpenClaw refuses settings in the channel that the plugin installed does not take, so the old
        # plugin's channel comes out before ours goes in, and ours is set only once ours is in.
        self.every_step("openclaw", make, state, lambda m, made: [], fail={"FAKE_OPENCLAW_FAIL": "plugins install"},
                        windows=("the old plugin, with its channel None", "our plugin, with its channel None"))


class Upgrade(unittest.TestCase):
    """Every agent runs the one core, with the one set of support files. An install over an older one,
    here of another agent of the same harness, that fails or is stopped at any step leaves both as they
    were, or, once it is finished, both new: never the new core with the old files, or the other way
    round. Stopped by SIGTERM, it puts back what it changed; killed by SIGKILL, the same command run
    again finishes the upgrade, or, should that fail, puts back what the killed run found."""

    HARNESS = "claude-code"
    # The parts of the support files Claude Code's install replaces.
    PARTS = ("LICENSE", "install", "packages/claude-code")

    def older(self, m: Machine) -> None:
        """An agent installed with an older core and older support files."""
        older = m.dir / "older-core"
        m.write(older, OLDER_CORE)
        m.run(self.HARNESS, *m.args(self.HARNESS, "--binary", str(older), "--name", "first"))
        lib = m.data / "lib"
        (lib / "LICENSE").write_text((lib / "LICENSE").read_text() + "an older licence\n")
        for part in self.PARTS[1:]:
            (lib / part / "OLDER").write_text("an older file\n")

    def args(self, m: Machine) -> list:
        return m.args(self.HARNESS, "--binary", str(m.core), "--name", "second")

    @staticmethod
    def snapshot(m: Machine) -> dict:
        """The core and the support files every agent starts with."""
        core = m.home / ".local/bin/marmot-agent"
        files = {"core": core.read_text() if core.exists() else None}
        lib = m.data / "lib"
        for path in sorted(lib.rglob("*")) if lib.exists() else []:
            files[str(path.relative_to(lib))] = path.read_text() if path.is_file() else "(a folder)"
        return files

    def setUp(self):
        m = Machine(self)
        self.older(m)
        self.before = self.snapshot(m)
        m.run(self.HARNESS, *self.args(m))
        self.after = self.snapshot(m)
        self.assertNotEqual(self.before, self.after)
        self.assertEqual(self.after["core"], FAKE_CORE)
        self.assertNotIn("install/OLDER", self.after)

    def state(self, m: Machine) -> str:
        now = self.snapshot(m)
        if now == self.before:
            return "older"
        if now == self.after:
            return "new"
        changed = sorted(k for k in set(now) | set(self.before) if now.get(k) != self.before.get(k))
        return f"mixed: {changed}"

    def leftovers(self, m: Machine) -> list:
        return sorted(p.name for p in m.data.iterdir() if p.name not in ("agents", "lib", "install.lock"))

    def failed(self, fail: str) -> None:
        m = Machine(self)
        self.older(m)
        result = m.run(self.HARNESS, *self.args(m), env={"FAKE_FAIL": fail}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.state(m), "older", result.stderr)
        self.assertEqual(self.leftovers(m), [], "nothing staged or kept is left")
        m.run(self.HARNESS, *self.args(m))
        self.assertEqual(self.state(m), "new")
        self.assertEqual(self.leftovers(m), [])

    def test_a_failure_preparing_the_new_files_keeps_the_older_core(self):
        self.failed("npm ci")

    def test_a_failure_once_the_new_core_is_in_puts_back_the_older_one(self):
        self.failed("systemctl --user restart")

    def test_a_first_install_that_fails_leaves_no_core(self):
        m = Machine(self)
        result = m.run(self.HARNESS, *self.args(m), env={"FAKE_FAIL": "systemctl --user restart"}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.snapshot(m), {"core": None})

    def test_another_harness_puts_back_an_upgrade_it_cannot_finish(self):
        """A run of another harness, which has none of Claude Code's files to put in, finds an upgrade a
        killed run left under way, at any step, even before Claude Code's own files were touched: it puts
        back what that run found, then installs as if none had run."""
        reference = Machine(self)
        self.older(reference)
        reference.run("codex", *reference.args("codex", "--binary", str(reference.core)))
        expected = self.snapshot(reference)
        steps = Machine(self)
        self.older(steps)
        steps.run(self.HARNESS, *self.args(steps), stop=("KILL", 0))
        tried = untouched = 0
        for step in range(1, int(steps.steps.read_text()) + 1):
            m = Machine(self)
            self.older(m)
            m.run(self.HARNESS, *self.args(m), stop=("KILL", step), check=False)
            if not (m.data / "upgrade").exists():
                continue
            tried += 1
            untouched += not (m.data / "upgrade/lib/packages").exists()
            out = m.run("codex", *m.args("codex", "--binary", str(m.core))).stdout
            self.assertIn("a run of another installer, or for another agent, before this one stopped part way", out, f"killed at step {step}")
            self.assertEqual(self.snapshot(m), expected, f"killed at step {step}")
            self.assertEqual(self.leftovers(m), [])
        self.assertGreater(untouched, 0, "some step leaves the upgrade under way before Claude Code's own files")
        self.assertGreater(tried, untouched)

    def test_a_second_install_waits_for_one_under_way(self):
        """Two installs in one home at once: the second waits until the first is done, and so never takes
        over, finishes or puts back an upgrade the first is still making. The lock is taken with flock,
        or, without it, as on macOS, with perl."""
        self.one_at_a_time(None)
        without = Path(tempfile.mkdtemp(prefix="marmot-install-"))
        self.addCleanup(shutil.rmtree, without, True)
        for folder in SYSTEM_PATH.split(":"):
            for program in Path(folder).iterdir():
                if program.name != "flock" and not os.path.lexists(without / program.name):
                    (without / program.name).symlink_to(program)
        self.one_at_a_time(str(without))

    def one_at_a_time(self, system_path):
        m = Machine(self)
        self.older(m)
        path = {"PATH": f"{m.bin}:{system_path}"} if system_path else {}
        pause = m.dir / "pause"
        first = m.start(self.HARNESS, *self.args(m),
                        env={"FAKE_PAUSE": "systemctl --user restart", "FAKE_PAUSE_AT": str(pause), **path})
        self.addCleanup(first.kill)
        reached = pause.with_name("pause.reached")
        for _ in range(1200):
            if reached.exists() or first.poll() is not None:
                break
            time.sleep(0.05)
        self.assertTrue(reached.exists(), m.output(first))
        # Another agent of the same harness, whose install fails once it has prepared its files.
        second = m.start(self.HARNESS, *m.args(self.HARNESS, "--binary", str(m.core), "--name", "third"),
                         env={"FAKE_FAIL": "npm ci", **path})
        self.addCleanup(second.kill)
        for _ in range(1200):
            if "waiting for it" in m.output(second) or second.poll() is not None:
                break
            time.sleep(0.05)
        pause.touch()
        self.assertEqual(first.wait(timeout=120), 0, m.output(first))
        self.assertNotEqual(second.wait(timeout=120), 0, m.output(second))
        self.assertIn("waiting for it", m.output(second))
        self.assertEqual(self.state(m), "new", m.output(second))
        self.assertEqual(self.leftovers(m), [])

    def test_a_stop_while_the_record_is_removed_puts_back_nothing(self):
        """The upgrade is done once its record leaves its place, whole: a TERM as the record is being
        removed, some of it gone, leaves the new core and files, never the old core with new files."""
        m = Machine(self)
        self.older(m)
        rm = shutil.which("rm", path=SYSTEM_PATH)
        # An rm that, given the record, removes the copies of the old files and is then stopped with the
        # installer, as a TERM to both would.
        m.write(m.bin / "rm", f"""#!/bin/sh
for a; do
    if [ -f "$a/list" ] && [ ! -e "$FAKE_LOG.removing" ]; then
        : >"$FAKE_LOG.removing"
        {rm} -rf "$a/lib"
        kill -s TERM "$PPID"
        exit 143
    fi
done
exec {rm} "$@"
""")
        result = m.run(self.HARNESS, *self.args(m), check=False)
        self.assertTrue((m.dir / "calls.log.removing").exists(), "the record was removed")
        self.assertEqual(result.returncode, 143, result.stderr)
        self.assertEqual(self.state(m), "new", result.stderr)
        (m.bin / "rm").unlink()
        m.run(self.HARNESS, *self.args(m))
        self.assertEqual(self.state(m), "new")
        self.assertEqual(self.leftovers(m), [])

    def test_a_failed_upgrade_keeps_the_image_its_scripts_were_made_for(self):
        """Pi and Goose in a container: an upgrade that fails after building its image leaves the image
        tagged for the scripts it puts back, and the next one tags its own."""
        for harness in ("pi", "goose"):
            with self.subTest(harness=harness):
                m = Machine(self)
                m.write(m.bin / "docker", FAKE_DOCKER)
                args = m.args(harness, "--binary", str(m.core), container=True)
                m.run(harness, *args)
                tags = m.dir / "docker-tags"
                before = json.loads(tags.read_text())
                self.assertEqual(list(before), [f"marmot-agents-{harness}:latest"])
                failed = m.run(harness, *args, env={"FAKE_FAIL": "systemctl --user restart"}, check=False)
                self.assertNotEqual(failed.returncode, 0)
                self.assertEqual(json.loads(tags.read_text()), before, failed.stderr)
                m.run(harness, *args)
                after = json.loads(tags.read_text())
                self.assertEqual(list(after), [f"marmot-agents-{harness}:latest"])
                self.assertNotEqual(after, before)

    def test_a_failed_upgrade_puts_back_the_gateway_plugin(self):
        """Hermes and OpenClaw: an update of our plugin in the person's gateway that fails puts back the
        plugin it found, with the core it found, and the next one puts in its own."""
        for harness, plugin in (("hermes", ".hermes/plugins/marmot"), ("openclaw", ".openclaw/extensions/marmot")):
            if harness == "openclaw" and not REAL_NODE:
                continue
            with self.subTest(harness=harness):
                m = Machine(self)
                older = m.dir / "older-core"
                m.write(older, OLDER_CORE)
                args = m.args(harness, "--binary", str(older))
                m.run(harness, *args)
                (m.home / plugin / "OLDER").write_text("an older file\n")
                args = m.args(harness, "--binary", str(m.core))
                failed = m.run(harness, *args, env={"FAKE_FAIL": "systemctl --user restart"}, check=False)
                self.assertNotEqual(failed.returncode, 0)
                self.assertTrue((m.home / plugin / "OLDER").exists(), failed.stderr)
                self.assertEqual((m.home / ".local/bin/marmot-agent").read_text(), OLDER_CORE, failed.stderr)
                m.run(harness, *args)
                self.assertFalse((m.home / plugin / "OLDER").exists())
                self.assertEqual((m.home / ".local/bin/marmot-agent").read_text(), FAKE_CORE)
                self.assertEqual(list((m.data / f"agents/{harness}/backups").glob("marmot.*")), [],
                                 "no copy of our own plugin is kept once the update is done")

    @staticmethod
    def judged(found: dict, expected: tuple):
        """None if what together(m) found is all of one of the installs expected: "older", "new", or
        "codex", the core and the shared files new beside this harness's older ones; else what is wrong."""
        shared = ("core", "install files")
        wanted = {"older": lambda part: "older", "new": lambda part: "new",
                  "codex": lambda part: "new" if part in shared else "older"}
        for install in expected:
            if all(found[part] == wanted[install](part) for part in found):
                return None
        return f"expected the {' or the '.join(expected)} install, found " + \
            ", ".join(f"the {part} {state}" for part, state in found.items())

    def every_step_together(self, harness: str, args, older, together, stopped_env=None) -> None:
        """What goes with the core, Pi's image or the person's Hermes plugin, stays with the files it
        was made for, whatever step an upgrade is stopped at (InstallUpgrade.tla, U1): stopped by TERM;
        killed, then run again, or run again and failing, or followed by Codex's install. older(m) puts
        in an older install, marking each part of it; together(m) says which install each part is of,
        older or new (generations). Checked once nothing is left under way. A run stopped by TERM puts
        back what it changed as it ends, so it leaves nothing under way, and the whole of one install:
        the older or the new; only a kill leaves something under way. After a kill, the next run of the
        same harness finishes the install, all of it new; a run that fails puts back all of it, the core
        and a plugin change too, all of it older; and Codex's install, which cannot finish another
        harness's upgrade, puts it all back, then installs the core and the files every install
        replaces, new, beside the older ones of this harness. stopped_env is the stopped run's, not the
        runs' after it (#197)."""
        m = Machine(self)
        m.write(m.bin / "docker", FAKE_DOCKER)
        older(m)
        m.run(harness, *args(m), stop=("KILL", 0), env=stopped_env, check=stopped_env is None)
        steps = int(m.steps.read_text())
        self.assertGreater(steps, 10)
        cases = [(signal, step) for signal in ("TERM", "KILL") for step in range(1, steps + 1)]

        def under_way(m: Machine) -> bool:
            return (m.data / "upgrade").exists() or (m.data / f"agents/{harness}/plugin-change").exists()

        def one(case):
            signal, step = case
            found = []
            for then in (None, "again", "failing", "codex"):
                m = Machine(self)
                m.write(m.bin / "docker", FAKE_DOCKER)
                older(m)
                stopped = m.run(harness, *args(m), stop=(signal, step), env=stopped_env, check=False)
                said = f"after SIG{signal} at step {step}"
                if then is None:
                    if not under_way(m):
                        found.append((said, self.judged(together(m), ("older", "new")), stopped.stderr))
                    elif signal == "TERM":
                        found.append((said, "something is left under way", stopped.stderr))
                    continue
                if signal == "TERM" or not under_way(m):
                    break
                if then == "codex":
                    ran = m.run("codex", *m.args("codex", "--binary", str(m.core), "--name", "other"), check=False)
                else:
                    env = {"FAKE_FAIL": "systemctl --user restart"} if then == "failing" else {}
                    ran = m.run(harness, *args(m), env=env, check=False)
                if then == "again" and ran.returncode != 0:
                    found.append((f"{said}, then {then}", "the run failed", ran.stderr))
                elif then == "failing" and (ran.returncode == 0 or "failing, as asked" not in ran.stderr):
                    found.append((f"{said}, then {then}", "the run did not fail as asked", ran.stderr))
                elif under_way(m):
                    found.append((f"{said}, then {then}", "something is left under way", ran.stderr))
                else:
                    expected = {"again": ("new",), "failing": ("older",), "codex": ("codex",)}[then]
                    found.append((f"{said}, then {then}", self.judged(together(m), expected), ran.stderr))
            return found

        with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            for found in pool.map(one, cases):
                for said, wrong, stderr in found:
                    with self.subTest(said):
                        self.assertIsNone(wrong, f"{said}:\n{stderr}")

    @staticmethod
    def shared_generations(m: Machine) -> dict:
        """Which install the core and the files every install replaces are of: older, new, or what is
        wrong with them. older(m) marks the older install's."""
        core = m.home / ".local/bin/marmot-agent"
        if not core.is_file():
            core_is = "not there"
        else:
            text = core.read_text()
            core_is = "older" if text == OLDER_CORE else "new" if text == FAKE_CORE else "another core"
        install = m.data / "lib/install"
        files_are = "not there" if not (install / "lib.sh").is_file() else \
            "older" if (install / "OLDER").exists() else "new"
        return {"core": core_is, "install files": files_are}

    @staticmethod
    def mark_older(m: Machine) -> None:
        """Marks the core and the shared files of the install there as older."""
        (m.data / "lib/install/OLDER").write_text("an older file\n")

    @classmethod
    def pi_together(cls, m: Machine) -> dict:
        """Which install Pi's scripts, the image tagged for them, the core and the shared files are of."""
        tag = json.loads((m.dir / "docker-tags").read_text()).get("marmot-agents-pi:latest")
        scripts = m.data / "lib/tools/pi"
        return {**cls.shared_generations(m),
                "Pi's scripts": "not there" if not scripts.is_dir() else
                "older" if (scripts / "OLDER").exists() else "new",
                "image": "not tagged" if not tag else "older" if tag == m.older_image else "new"}

    def test_every_step_keeps_the_image_with_its_scripts(self):
        def args(m: Machine) -> list:
            return m.args("pi", "--binary", str(m.core), container=True)

        def older(m: Machine) -> None:
            first = m.dir / "older-core"
            m.write(first, OLDER_CORE)
            m.run("pi", *m.args("pi", "--binary", str(first), container=True))
            (m.data / "lib/tools/pi/OLDER").write_text("an older file\n")
            self.mark_older(m)
            m.older_image = json.loads((m.dir / "docker-tags").read_text())["marmot-agents-pi:latest"]

        self.every_step_together("pi", args, older, self.pi_together)

    def test_a_lookup_docker_cannot_answer_never_takes_the_tag_away(self):
        """Pi: when docker cannot say which image the tag names, as when it cannot be reached for a
        moment, the image is not noted as none, which would have a run that puts the upgrade back take the
        tag off the image the old scripts run: the install stops, whatever step it is stopped at too."""
        def args(m: Machine) -> list:
            return m.args("pi", "--binary", str(m.core), container=True)

        def older(m: Machine) -> None:
            first = m.dir / "older-core"
            m.write(first, OLDER_CORE)
            m.run("pi", *m.args("pi", "--binary", str(first), container=True))
            (m.data / "lib/tools/pi/OLDER").write_text("an older file\n")
            self.mark_older(m)
            m.older_image = json.loads((m.dir / "docker-tags").read_text())["marmot-agents-pi:latest"]

        self.every_step_together("pi", args, older, self.pi_together, stopped_env={"FAKE_DOCKER_FAIL": "image inspect"})

    def test_a_put_back_docker_cannot_answer_keeps_the_record(self):
        """Pi: a first install killed once its tag moved, and put back by Codex's install while docker
        cannot say whether the tag is there: the upgrade's record is kept, not retired with the tag left on,
        and the next install, with docker back, takes the tag away."""
        m = Machine(self)
        m.write(m.bin / "docker", FAKE_DOCKER)
        args = m.args("pi", "--binary", str(m.core), container=True)
        m.run("pi", *args, stop=("KILL", 0))
        steps = int(m.steps.read_text())
        for step in range(1, steps + 1):
            m = Machine(self)
            m.write(m.bin / "docker", FAKE_DOCKER)
            m.run("pi", *m.args("pi", "--binary", str(m.core), container=True), stop=("KILL", step), check=False)
            tags = m.dir / "docker-tags"
            tagged = tags.exists() and "marmot-agents-pi:latest" in json.loads(tags.read_text())
            if tagged and (m.data / "upgrade").exists():
                break
        else:
            self.fail("no step leaves the tag moved and the upgrade under way")
        codex = m.args("codex", "--binary", str(m.core), "--name", "other")
        ran = m.run("codex", *codex, env={"FAKE_DOCKER_FAIL": "image inspect"}, check=False)
        self.assertNotEqual(ran.returncode, 0, ran.stdout)
        self.assertTrue((m.data / "upgrade").exists(), ran.stderr)
        m.run("codex", *codex)
        self.assertFalse((m.data / "upgrade").exists())
        self.assertNotIn("marmot-agents-pi:latest", json.loads(tags.read_text()))

    def test_every_step_keeps_the_hermes_plugin_with_its_core(self):
        plugin = ".hermes/plugins/marmot"

        def args(m: Machine) -> list:
            return m.args("hermes", "--binary", str(m.core))

        def older(m: Machine) -> None:
            first = m.dir / "older-core"
            m.write(first, OLDER_CORE)
            m.run("hermes", *m.args("hermes", "--binary", str(first)))
            (m.data / "lib/python/marmot_hermes/OLDER").write_text("an older file\n")
            (m.home / plugin / "OLDER").write_text("an older file\n")
            self.mark_older(m)

        def together(m: Machine) -> dict:
            found = self.shared_generations(m)
            for name, folder in (("Hermes files", m.data / "lib/python/marmot_hermes"), ("plugin", m.home / plugin)):
                found[name] = "not there" if not (folder / "plugin.yaml").is_file() else \
                    "older" if (folder / "OLDER").exists() else "new"
            return found

        self.every_step_together("hermes", args, older, together)

    def test_every_step(self):
        m = Machine(self)
        self.older(m)
        m.run(self.HARNESS, *self.args(m), stop=("KILL", 0))
        steps = int(m.steps.read_text())
        self.assertGreater(steps, 10)
        cases = [(signal, step) for signal in ("TERM", "KILL") for step in range(1, steps + 1)]

        def one(case):
            signal, step = case
            m = Machine(self)
            self.older(m)
            stopped = m.run(self.HARNESS, *self.args(m), stop=(signal, step), check=False)
            after = self.state(m)
            under_way = (m.data / "upgrade").exists()
            failed = after_failure = None
            if signal == "KILL" and under_way:
                # On a copy killed the same way, a run that fails puts back what the killed run found.
                other = Machine(self)
                self.older(other)
                other.run(self.HARNESS, *self.args(other), stop=(signal, step), check=False)
                failed = other.run(self.HARNESS, *self.args(other), env={"FAKE_FAIL": "systemctl --user restart"},
                                   check=False)
                after_failure = (self.state(other), self.leftovers(other))
            again = m.run(self.HARNESS, *self.args(m), check=False)
            return case, stopped, after, under_way, failed, after_failure, again, self.state(m), self.leftovers(m)

        taken_over = 0
        with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            for (signal, step), stopped, after, under_way, failed, after_failure, again, finished, left in pool.map(one, cases):
                taken_over += failed is not None
                with self.subTest(signal=signal, step=step):
                    said = f"after SIG{signal} at step {step}:\n{stopped.stdout}\n{stopped.stderr}"
                    self.assertNotEqual(stopped.returncode, 0, "the run was stopped")
                    if signal == "TERM" or not under_way:
                        self.assertIn(after, ("older", "new"), said)
                    if signal == "TERM":
                        self.assertFalse(under_way, f"an upgrade put back or finished {said}")
                    if failed is not None:
                        self.assertNotEqual(failed.returncode, 0)
                        self.assertEqual(after_failure, ("older", []), f"{said}\nthen failing:\n{failed.stderr}")
                    self.assertEqual(again.returncode, 0, f"{said}\nthen again:\n{again.stderr}")
                    self.assertEqual(finished, "new", again.stdout)
                    self.assertFalse(any(name.startswith((".upgrade", "upgrade")) for name in left), left)
        self.assertGreater(taken_over, 0, "some step leaves an upgrade under way for the next run")


    # The account store (D-159): an older agent, "first", upgraded in place, whose core opens its store as it
    # starts (FAKE_SYSTEMCTL_STORE).
    STORE = "agents/first/home"

    def older_store(self, m: Machine, harness=None) -> None:
        """Agent "first" of the harness, installed with an older core that has opened its store: a database
        with its -wal file, a secret and the folder of the socket, each the owner's alone."""
        harness = harness or self.HARNESS
        m.write(m.bin / "systemctl", FAKE_SYSTEMCTL_STORE)
        older = m.dir / "older-core"
        m.write(older, OLDER_CORE)
        m.run(harness, *m.args(harness, "--binary", str(older), "--name", "first"))
        self.mark_older(m)
        home = m.data / self.STORE
        store = home / "accounts/agent"
        store.mkdir(parents=True)
        for name, text in (("session.sqlite", "older\n"), ("session.sqlite-wal", "older\n"),
                           ("secret.json", "not a real secret\n")):
            (store / name).write_text(text)
            (store / name).chmod(0o600)
        for folder in (home, home / "accounts", store):
            folder.chmod(0o700)
        (home / "socket").mkdir(mode=0o700)
        (home / "socket/marmot-agent.sock").write_text("")

    def store_args(self, m: Machine, harness=None) -> list:
        harness = harness or self.HARNESS
        return m.args(harness, "--binary", str(m.core), "--name", "first")

    def store_of(self, m: Machine) -> str:
        """Which core's store agent "first" has, as its database and its -wal file say, or what is wrong."""
        store = m.data / self.STORE / "accounts/agent"
        refused = m.dir / "calls.log.refused"
        if refused.exists():
            return refused.read_text().strip()
        found = {(store / name).read_text().strip() if (store / name).exists() else "none"
                 for name in ("session.sqlite", "session.sqlite-wal")}
        secret = store / "secret.json"
        if not secret.is_file() or secret.read_text() != "not a real secret\n":
            return "its secret is gone or changed"
        return found.pop() if len(found) == 1 else f"mixed: {sorted(found)}"

    def no_copy(self, m: Machine) -> None:
        """No copy of the home is left anywhere: in the upgrade's record, or beside the home."""
        self.assertFalse((m.data / "upgrade").exists())
        self.assertEqual([p.name for p in (m.data / "agents/first").iterdir() if "marmot-agent-" in p.name], [])
        self.assertEqual(self.leftovers(m), [])

    START = "systemctl --user start marmot-agent-first.service marmot-agent-first-connector.service"

    def test_a_failure_once_the_new_core_started_puts_back_the_store(self):
        """An upgrade that fails after the new core opened the agent's store puts the store back as it was
        with the older core, the agent's services stopped first and left stopped, and prints the command
        that starts them; the next upgrade puts the new core in, and its store goes forward. With mv that
        can exchange two folders and with one that cannot."""
        for exchanges in (True, False):
            if exchanges and not MV_EXCHANGES:
                continue
            with self.subTest(exchanges=exchanges):
                m = Machine(self)
                if not exchanges:
                    m.write(m.bin / "mv", MV_WITHOUT_EXCHANGE)
                self.older_store(m)
                m.log.write_text("")
                failed = m.run(self.HARNESS, *self.store_args(m), env={"FAKE_FAIL": "systemctl --user restart"},
                               check=False)
                self.assertNotEqual(failed.returncode, 0)
                self.assertIn("failing, as asked", failed.stderr)
                units = "marmot-agent-first.service marmot-agent-first-connector.service"
                calls = [c for c in m.calls() if c.split(" ")[:3] in (["systemctl", "--user", verb] for verb in
                                                                       ("stop", "restart", "start"))]
                self.assertEqual(calls, [f"systemctl --user stop {units}", f"systemctl --user restart {units}",
                                         f"systemctl --user stop {units}"], failed.stderr)
                self.assertIn(f'the agent "first" is left stopped; start it again with: {self.START}\n',
                              failed.stderr)
                self.assertEqual(self.shared_generations(m), {"core": "older", "install files": "older"})
                self.assertEqual(self.store_of(m), "older", failed.stderr)
                self.assertIn("put back the account home", failed.stdout)
                home = m.data / self.STORE
                self.assertEqual(mode(home / "accounts/agent/secret.json"), 0o600)
                self.assertEqual(mode(home), 0o700)
                self.no_copy(m)
                m.run(self.HARNESS, *self.store_args(m))
                self.assertEqual(self.shared_generations(m), {"core": "new", "install files": "new"})
                self.assertEqual(self.store_of(m), "new")
                self.no_copy(m)

    def test_a_service_that_does_not_stop_stops_the_upgrade_before_the_copy(self):
        """An upgrade whose agent's services are still running, or waiting to start again, once they were
        stopped copies nothing and puts nothing new in: the older core and its store stay as they were."""
        m = Machine(self)
        self.older_store(m)
        failed = m.run(self.HARNESS, *self.store_args(m), env={"FAKE_ACTIVE": "activating"}, check=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("the agent's services did not stop", failed.stderr)
        self.assertEqual(self.shared_generations(m), {"core": "older", "install files": "older"})
        self.assertEqual(self.store_of(m), "older")
        self.assertFalse((m.data / "upgrade/stores/first/home").exists(), "nothing was copied")
        # Once the services stop, the next run finishes the upgrade, and the store goes forward.
        m.run(self.HARNESS, *self.store_args(m))
        self.assertEqual(self.shared_generations(m), {"core": "new", "install files": "new"})
        self.assertEqual(self.store_of(m), "new")
        self.no_copy(m)

    def test_a_successful_upgrade_removes_the_copy(self):
        """While the upgrade is under way, the copy of the store is in the upgrade's record alone, the
        owner's alone, each file with its mode, and nothing of it is printed; once the upgrade is done it is
        gone. Installed again with the same core, nothing is copied."""
        m = Machine(self)
        self.older_store(m)
        home = m.data / self.STORE
        pause = m.dir / "pause"
        run = m.start(self.HARNESS, *self.store_args(m),
                      env={"FAKE_PAUSE": "systemctl --user restart", "FAKE_PAUSE_AT": str(pause)})
        self.addCleanup(run.kill)
        m.until(lambda: pause.with_name("pause.reached").exists(), lambda: run.poll() is not None)
        self.assertTrue(pause.with_name("pause.reached").exists(), m.output(run))
        record = m.data / "upgrade/stores/first"
        copy = record / "home"
        self.assertEqual(mode(record), 0o700)
        self.assertEqual(mode(record.parent), 0o700)
        self.assertEqual((record / "where").read_text().splitlines()[0], str(home))
        expected = {str(p.relative_to(home)): (mode(p), p.read_text() if p.is_file() else None)
                    for p in home.rglob("*")
                    if not p.is_relative_to(home / "socket") and p.name != ".marmot-runtime.lock"}
        found = {str(p.relative_to(copy)): (mode(p), p.read_text() if p.is_file() else None) for p in copy.rglob("*")}
        self.assertEqual(found, expected, "the whole home but MDK's lock and the socket's folder, each file with its mode")
        self.assertEqual(mode(copy), mode(home))
        self.assertEqual(found["accounts/agent/session.sqlite"][1], "older\n", "taken before the new core started")
        pause.touch()
        self.assertEqual(run.wait(timeout=120), 0, m.output(run))
        self.assertNotIn("not a real secret", m.output(run))
        self.assertEqual(self.store_of(m), "new")
        self.no_copy(m)
        m.log.write_text("")
        m.run(self.HARNESS, *self.store_args(m))
        self.assertNotIn("systemctl --user stop marmot-agent-first.service marmot-agent-first-connector.service",
                         m.calls(), "the same core again opens no store anew, so none is copied")

    def test_init_is_refused_on_a_home_in_use(self):
        """An agent whose public-key file was lost, with a home a core has opened: the install stops before
        `marmot-agent init`, which would open the store with the new core, uncopied with --no-start, and
        says what to do; the older core and its store are as they were."""
        for no_start in (False, True):
            with self.subTest(no_start=no_start):
                m = Machine(self)
                self.older_store(m)
                (m.data / "agents/first/public-key").unlink()
                store = m.data / self.STORE / "accounts/agent"
                m.write(m.core, FAKE_CORE.replace(
                    "    init) home=",
                    f'    init) echo new >"{store}/session.sqlite"; echo new >"{store}/session.sqlite-wal"\n'
                    '        echo "the home already has an identity" >&2; exit 1 ;;\n    unused) home='))
                m.log.write_text("")
                failed = m.run(self.HARNESS, *self.store_args(m), *(["--no-start"] if no_start else []), check=False)
                self.assertNotEqual(failed.returncode, 0)
                self.assertNotIn("marmot-agent init", "\n".join(m.calls()))
                self.assertIn(f"the agent's account home {m.data / self.STORE} already holds an identity", failed.stderr)
                self.assertIn("Write the agent's public key into that file", failed.stderr)
                self.assertEqual(self.shared_generations(m), {"core": "older", "install files": "older"})
                self.assertEqual(self.store_of(m), "older", failed.stderr)
                self.no_copy(m)

    def test_a_kill_while_the_store_goes_back_is_finished_by_the_next_run(self):
        """An upgrade that fails once the new core has opened the store, killed at any step as it puts the
        store back: the next run of the same agent, with --no-start, finishes putting the store back before
        it ends the upgrade, so the agent's home is never left half put back or missing. Neither it nor an
        install for another agent starts this one's services, which the killed run had stopped; one that
        puts its store back says how to start them."""
        fail = {"FAKE_FAIL": "systemctl --user restart"}
        m = Machine(self)
        self.older_store(m)
        m.run(self.HARNESS, *self.store_args(m), env=fail, stop=("KILL", 0), check=False)
        steps = int(m.steps.read_text())

        def one(case):
            step, then = case
            m = Machine(self)
            self.older_store(m)
            m.run(self.HARNESS, *self.store_args(m), env=fail, stop=("KILL", step), check=False)
            m.log.write_text("")
            if then == "no-start":
                ran = m.run(self.HARNESS, *self.store_args(m), "--no-start", check=False)
            else:
                ran = m.run("codex", *m.args("codex", "--binary", str(m.core), "--name", "other"), check=False)
            calls = m.calls()
            starts = [c for c in calls if c.startswith(("systemctl --user start", "systemctl --user restart"))
                      and "marmot-agent-first" in c]
            wrong = None
            if ran.returncode != 0:
                wrong = "the run failed"
            elif (m.data / "upgrade").exists():
                wrong = "something is left under way"
            elif starts:
                wrong = f"the agent's services are started: {starts}"
            elif self.store_of(m) not in ("older", "new"):
                wrong = f"the store is {self.store_of(m)}"
            elif then == "codex" and self.store_of(m) != "older":
                wrong = f"the store is {self.store_of(m)}"
            elif then == "codex" and "put back the account home" in ran.stdout and self.START not in ran.stdout:
                wrong = "how to start the agent is not said"
            return f"killed at step {step}, then {then}", wrong, ran.stderr

        cases = [(step, then) for step in range(1, steps + 1) for then in ("no-start", "codex")]
        with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            for said, wrong, stderr in pool.map(one, cases):
                with self.subTest(said):
                    self.assertIsNone(wrong, f"{said}:\n{stderr}")

    def test_every_step_keeps_the_store_with_its_core(self):
        """The store goes with the core, whatever step an upgrade of the agent is stopped at: never the older
        core with the store the new one made (U1, as every_step_together checks it). A killed upgrade that the
        same agent's install finishes leaves the new store and removes the copy; one that fails puts it back,
        and so does Codex's install, which cannot finish it (D-154)."""
        def together(m: Machine) -> dict:
            return {**self.shared_generations(m), "store": self.store_of(m)}

        self.every_step_together(self.HARNESS, self.store_args, self.older_store, together)

    def test_another_core_puts_back_an_upgrade_whose_store_moved(self):
        """An upgrade killed after the new core opened the agent's store, then the same agent installed with
        the older core, with --no-start or without: that run does not take the upgrade over, which would end it
        with the copy gone and the older core on the new store. It puts it back first, the store with it, then
        installs the older core on the older store."""
        def killed(step: int) -> Machine:
            m = Machine(self)
            self.older_store(m)
            m.run(self.HARNESS, *self.store_args(m), stop=("KILL", step), check=False)
            return m

        def moved(step: int) -> bool:
            m = killed(step)
            return (m.data / "upgrade/stores/first/home").is_dir() and self.store_of(m) == "new"

        counted = Machine(self)
        self.older_store(counted)
        counted.run(self.HARNESS, *self.store_args(counted), stop=("KILL", 0))
        steps = int(counted.steps.read_text())
        step = next((s for s in range(steps, 0, -1) if moved(s)), None)
        self.assertIsNotNone(step, "some step leaves the upgrade under way once the new core opened the store")
        for no_start in (True, False):
            with self.subTest(no_start=no_start):
                m = killed(step)
                older = m.dir / "older-core"
                ran = m.run(self.HARNESS, *m.args(self.HARNESS, "--binary", str(older), "--name", "first"),
                            *(["--no-start"] if no_start else []), check=False)
                self.assertEqual(ran.returncode, 0, ran.stderr)
                self.assertEqual(self.shared_generations(m)["core"], "older")
                self.assertEqual(self.store_of(m), "older", ran.stderr)
                self.no_copy(m)
                self.assertIn("with another marmot-agent core", ran.stdout)

    @unittest.skipUnless(shutil.which("flock", path=SYSTEM_PATH), "holds MDK's lock with flock")
    def test_a_store_that_cannot_go_back_keeps_the_new_core(self):
        """An upgrade that fails after the new core opened the store, while something else holds MDK's lock on
        the agent's home: the store cannot go back, so neither do the core and the support files, which are
        left new beside the new store, never the older core beside it. The record is kept, and the error says
        to run the installer again once nothing holds the home; that run, the lock let go, finishes the
        upgrade. The wait for the lock is cut from a minute to a second."""
        m = Machine(self)
        self.older_store(m)
        m.write(m.bin / "flock", '#!/bin/sh\nif [ "$1" = -w ] && [ "$2" = 60 ]; then shift 2; set -- -w 1 "$@"; fi\n'
                f'exec {shutil.which("flock", path=SYSTEM_PATH)} "$@"\n')
        pause = m.dir / "pause"
        run = m.start(self.HARNESS, *self.store_args(m),
                      env={"FAKE_PAUSE": "systemctl --user restart", "FAKE_PAUSE_AT": str(pause),
                           "FAKE_FAIL": "systemctl --user restart"})
        self.addCleanup(run.kill)
        m.until(lambda: pause.with_name("pause.reached").exists(), lambda: run.poll() is not None)
        self.assertTrue(pause.with_name("pause.reached").exists(), m.output(run))
        held = os.open(m.data / self.STORE / ".marmot-runtime.lock", os.O_WRONLY | os.O_APPEND)
        try:
            fcntl.flock(held, fcntl.LOCK_EX)
            pause.touch()
            self.assertNotEqual(run.wait(timeout=120), 0)
            out = m.output(run)
            self.assertIn("failing, as asked", out)
            self.assertEqual(self.shared_generations(m), {"core": "new", "install files": "new"}, out)
            self.assertEqual(self.store_of(m), "new", out)
            self.assertTrue((m.data / "upgrade/stores/first/home").is_dir(), "the record and its copy are kept")
            self.assertIn("Run this again once nothing holds the agent's account home", out)
            self.assertNotIn("put back the account home", out)
        finally:
            os.close(held)
        m.run(self.HARNESS, *self.store_args(m))
        self.assertEqual(self.shared_generations(m), {"core": "new", "install files": "new"})
        self.assertEqual(self.store_of(m), "new")
        self.no_copy(m)

    def test_the_start_command_on_macos_is_quoted(self):
        """How to start an agent left stopped, on macOS: each plist's path one word, a home with spaces too."""
        record = Path(tempfile.mkdtemp(prefix="marmot-install-")) / "first"
        self.addCleanup(shutil.rmtree, record.parent, True)
        record.mkdir()
        (record / "where").write_text("/the home\norg.marmot.marmot-agent.first\norg.marmot.marmot-agent.first.connector\n")
        launchd = "/Users/some one/Library/LaunchAgents"
        said = subprocess.run(["bash", "-c", f'. "{HERE}/lib.sh"; OS=Darwin; MA_LAUNCHD_DIR="$1"; services_hint "$2"',
                               "bash", launchd, str(record)], capture_output=True, text=True, check=True).stdout
        command = said.split("start it again with: ", 1)[1].strip()
        uid = os.getuid()
        self.assertEqual(shlex.split(command), [
            "launchctl", "bootstrap", f"gui/{uid}", f"{launchd}/org.marmot.marmot-agent.first.plist", "&&",
            "launchctl", "bootstrap", f"gui/{uid}", f"{launchd}/org.marmot.marmot-agent.first.connector.plist"])

class TwoAgents(unittest.TestCase):
    """Two agents of one gateway harness in one home share the core and the support files, and each
    has its own change to a gateway's plugin. A run for one agent never finishes an upgrade that holds
    another agent's plugin change, or one to another gateway: it puts that upgrade back first, the
    other's change with it, so no change is left to put stale copies back later (#180)."""

    @staticmethod
    def args(m: Machine, name: str, core=None) -> list:
        return m.args("hermes", "--binary", str(core or m.core), "--name", name)

    @staticmethod
    def own_hermes(m: Machine) -> dict:
        """The second agent's Hermes, beside the first's in ~/.hermes."""
        return {"HERMES_HOME": str(m.home / ".hermes-b")}

    @staticmethod
    def gateway(m: Machine, folder: str = ".hermes") -> dict:
        """The files of a Hermes. A folder put back empty may stay: replace_dir makes the one above."""
        root = m.home / folder
        return {str(p.relative_to(root)): p.read_text() for p in sorted(root.rglob("*")) if p.is_file()}

    @staticmethod
    def under_way(m: Machine) -> list:
        return sorted(str(p.relative_to(m.data)) for p in [*m.data.glob("agents/*/plugin-change"), m.data / "upgrade"]
                      if p.exists())

    def older(self, m: Machine) -> None:
        """Two Hermes agents, a and b, each in a Hermes of its own, installed with an older core."""
        first = m.dir / "older-core"
        m.write(first, OLDER_CORE)
        m.run("hermes", *self.args(m, "a", first))
        m.run("hermes", *self.args(m, "b", first), env=self.own_hermes(m))
        for marked in (m.data / "lib/python/marmot_hermes", m.data / "lib/install",
                       m.home / ".hermes/plugins/marmot", m.home / ".hermes-b/plugins/marmot"):
            (marked / "OLDER").write_text("an older file\n")

    def test_another_agent_puts_back_an_upgrade_it_cannot_finish(self):
        """Agent a's upgrade is killed at any step; then agent b's install, into its own Hermes, puts a's
        upgrade back, a's Hermes as it was with it, and installs its own. Before, b took the upgrade over
        and ended it with only its own change, leaving a's Hermes changed and a's record of the change
        under way, to put back stale copies over a later install."""
        counting = Machine(self)
        self.older(counting)
        counting.run("hermes", *self.args(counting, "a"), stop=("KILL", 0))
        steps = int(counting.steps.read_text())

        def one(step):
            m = Machine(self)
            self.older(m)
            before = self.gateway(m)
            m.run("hermes", *self.args(m, "a"), stop=("KILL", step), check=False)
            left = (m.data / "upgrade").exists()
            killed = self.gateway(m)
            ran = m.run("hermes", *self.args(m, "b"), env=self.own_hermes(m), check=False)
            core = (m.home / ".local/bin/marmot-agent").read_text()
            return (step, left, before, killed, ran, self.gateway(m), self.gateway(m, ".hermes-b"), core,
                    (m.data / "lib/install/OLDER").exists(), self.under_way(m))

        put_back = 0
        with ThreadPoolExecutor(max_workers=os.cpu_count() or 4) as pool:
            for step, left, before, killed, ran, a, b, core, files_older, under_way in pool.map(one, range(1, steps + 1)):
                put_back += left and killed != before
                with self.subTest(step=step):
                    said = f"a killed at step {step}, then b:\n{ran.stdout}\n{ran.stderr}"
                    self.assertEqual(ran.returncode, 0, said)
                    self.assertEqual(under_way, [], said)
                    # A run of a stopped part way through its upgrade is put back with it; one stopped
                    # after, its upgrade done, is left as it is.
                    self.assertEqual(a, before if left else killed, said)
                    self.assertNotIn("plugins/marmot/OLDER", b, said)
                    self.assertEqual((core, files_older), (FAKE_CORE, False), said)
        self.assertGreater(put_back, 0, "some step leaves a's Hermes changed and its upgrade under way")

    def test_the_same_agent_and_gateway_finish_the_upgrade(self):
        """The agent whose run was killed part way through changing its gateway's plugin, run again with
        the same gateway, takes the upgrade over and finishes the change: what it finds names its own
        gateway's settings file (MA_GATEWAY)."""
        for harness in ("hermes", "openclaw"):
            if harness == "openclaw" and not REAL_NODE:
                continue
            with self.subTest(harness=harness):
                counting = Machine(self)
                args = counting.args(harness, "--binary", str(counting.core))
                counting.run(harness, *args, stop=("KILL", 0))
                for step in range(1, int(counting.steps.read_text()) + 1):
                    m = Machine(self)
                    args = m.args(harness, "--binary", str(m.core))
                    m.run(harness, *args, stop=("KILL", step), check=False)
                    if (m.data / f"agents/{harness}/plugin-change").exists() and (m.data / "upgrade").exists():
                        break
                else:
                    self.fail("no step leaves a plugin change under way")
                out = m.run(harness, *args).stdout
                self.assertIn("this one finishes that", out)
                self.assertIn("plugin; this one finishes it", out)
                self.assertEqual(self.under_way(m), [])

    def test_the_same_agent_into_another_hermes_puts_back_the_first(self):
        """An agent's run killed once our plugin is in its Hermes, then its run again into another Hermes:
        the first Hermes is put back as it was, not left with half a change no run finishes, and the
        second gets the plugin, with copies of its own settings."""
        m = Machine(self)
        first = m.dir / "older-core"
        m.write(first, OLDER_CORE)
        m.run("codex", *m.args("codex", "--binary", str(first), "--name", "c"))
        before = self.gateway(m)
        killed = m.run("hermes", *self.args(m, "a"), env={"FAKE_HERMES_KILL": "plugins enable"}, check=False)
        self.assertNotEqual(killed.returncode, 0, killed.stdout)
        ran = m.run("hermes", *self.args(m, "a"), env=self.own_hermes(m), check=False)
        self.assertEqual(ran.returncode, 0, ran.stderr)
        self.assertEqual(self.gateway(m), before, ran.stdout)
        self.assertIn("plugins/marmot/plugin.yaml", self.gateway(m, ".hermes-b"))
        self.assertEqual(self.under_way(m), [], ran.stdout)

    def test_runs_for_two_agents_into_one_hermes_then_a_failure_put_back_all_of_it(self):
        """Agents a and b put into one Hermes, a's run killed once our plugin is in and before the
        Hermes names a's home, then b's the same way, then a's again, failing: the core, the support
        files and the Hermes are all as they were before a's first run. Before, a's last run put back
        its own change first, and then b's, whose copies are of what a's first run left: our plugin,
        beside the older core."""
        m = Machine(self)
        first = m.dir / "older-core"
        m.write(first, OLDER_CORE)
        m.run("codex", *m.args("codex", "--binary", str(first), "--name", "c"))
        before = (Upgrade.snapshot(m), self.gateway(m))
        kill = {"FAKE_HERMES_KILL": "plugins enable"}
        for name in ("a", "b"):
            killed = m.run("hermes", *self.args(m, name), env=kill, check=False)
            self.assertNotEqual(killed.returncode, 0, killed.stdout)
            self.assertTrue((m.home / ".hermes/plugins/marmot/plugin.yaml").exists(), killed.stdout)
        failed = m.run("hermes", *self.args(m, "a"), env={"FAKE_FAIL": "systemctl --user restart"}, check=False)
        self.assertNotEqual(failed.returncode, 0, failed.stdout)
        self.assertEqual((Upgrade.snapshot(m), self.gateway(m)), before, failed.stderr)
        self.assertEqual(self.under_way(m), [], failed.stderr)

    def test_another_agent_into_a_gateway_a_killed_run_named_puts_that_run_back(self):
        """Agent a's run into a gateway no agent had, killed just after the gateway names a's home, then
        agent b's into the same gateway: b's puts a's upgrade back, the gateway's settings with it, before
        it reads whom the gateway serves, and installs. Before, b read a's half-made settings first and
        stopped, saying the gateway was a's, and the recovery that would take them out never ran."""
        for harness, kill in (("hermes", {"FAKE_HERMES_KILL_AFTER": "config set platforms.marmot.home"}),
                              ("openclaw", {"FAKE_OPENCLAW_KILL_AFTER": "config set channels.marmot"})):
            if harness == "openclaw" and not REAL_NODE:
                continue
            with self.subTest(harness=harness):
                m = Machine(self)
                first = m.dir / "older-core"
                m.write(first, OLDER_CORE)
                m.run("codex", *m.args("codex", "--binary", str(first), "--name", "c"))
                args = lambda name: m.args(harness, "--binary", str(m.core), "--name", name)
                killed = m.run(harness, *args("a"), env=kill, check=False)
                self.assertNotEqual(killed.returncode, 0, killed.stdout)
                self.assertIn(str(m.data / "agents/a/home"), (m.home / CHANGED[harness][0] / CHANGED[harness][1][0]).read_text())
                ran = m.run(harness, *args("b"), check=False)
                said = f"{ran.stdout}\n{ran.stderr}"
                self.assertEqual(ran.returncode, 0, said)
                self.assertIn("or for another agent, before this one stopped part way", ran.stdout)
                settings = (m.home / CHANGED[harness][0] / CHANGED[harness][1][0]).read_text()
                self.assertIn(str(m.data / "agents/b/home"), settings, said)
                self.assertNotIn(str(m.data / "agents/a/home"), settings, said)
                self.assertEqual(self.under_way(m), [], said)

    def test_a_first_install_reads_the_gateway_only_under_the_lock(self):
        """Agents a and b installed at once into one Hermes, in a home with no agent yet: b's run held
        up just after it reads whom the Hermes serves, then a's run. a's waits for b's to end, then
        finds b's home in the Hermes and stops; b's installs. Before, a first install took the lock
        only once it put the core in, so a's ran to its end while b's was held up, and b's then put
        its home over a's in the Hermes, which a's agent was left cut off from."""
        m = Machine(self)
        pause = m.dir / "pause"
        b = m.start("hermes", *self.args(m, "b"), env={"FAKE_HERMES_PAUSE_AFTER": "config get platforms.marmot.home",
                                                      "FAKE_PAUSE_AT": str(pause)})
        self.addCleanup(b.kill)
        reached = pause.with_name("pause.reached")
        m.until(reached.exists, lambda: b.poll() is not None)
        self.assertTrue(reached.exists(), m.output(b))
        a = m.start("hermes", *self.args(m, "a"))
        self.addCleanup(a.kill)
        m.until(lambda: "waiting for it" in m.output(a), lambda: a.poll() is not None)
        pause.touch()
        self.assertEqual(b.wait(timeout=120), 0, m.output(b))
        self.assertNotEqual(a.wait(timeout=120), 0, m.output(a))
        self.assertIn("waiting for it", m.output(a))
        self.assertIn(f"already connects to the agent whose home is {m.data / 'agents/b/home'}", m.output(a))
        config = json.loads((m.home / ".hermes/config.yaml").read_text())
        self.assertEqual(config["platforms"]["marmot"]["home"], str(m.data / "agents/b/home"))

    def test_a_refused_first_install_hands_the_lock_on(self):
        """A first install into a Hermes that serves another agent is refused, and takes away the data
        folder it made for the lock, the lock file with it. A run that waited for that lock opens the
        file again, so a run after it waits for it in turn, not taking a lock of its own on a new file
        while it holds one on a file that is gone."""
        m = Machine(self)
        (m.home / ".hermes").mkdir()
        (m.home / ".hermes/config.yaml").write_text(json.dumps({"platforms": {"marmot": {"home": "/elsewhere"}}}))
        held, holds = m.dir / "held", m.dir / "holds"
        refused = m.start("hermes", *self.args(m, "a"), env={"FAKE_HERMES_PAUSE_AFTER": "config get platforms.marmot.home",
                                                            "FAKE_PAUSE_AT": str(held)})
        self.addCleanup(refused.kill)
        m.until(held.with_name("held.reached").exists, lambda: refused.poll() is not None)
        self.assertTrue(m.data.exists(), m.output(refused))
        codex = lambda name, env: m.start("codex", *m.args("codex", "--binary", str(m.core), "--name", name), env=env)
        waited = codex("c", {"FAKE_PAUSE": "systemctl --user restart", "FAKE_PAUSE_AT": str(holds)})
        self.addCleanup(waited.kill)
        reached = holds.with_name("holds.reached")
        m.until(lambda: "waiting for it" in m.output(waited), reached.exists, lambda: waited.poll() is not None)
        self.assertIn("waiting for it", m.output(waited))
        held.touch()
        self.assertNotEqual(refused.wait(timeout=120), 0, m.output(refused))
        self.assertIn("already connects to the agent whose home is /elsewhere", m.output(refused))
        m.until(reached.exists, lambda: waited.poll() is not None)
        self.assertTrue(reached.exists(), m.output(waited))
        after = codex("d", {})
        self.addCleanup(after.kill)
        m.until(lambda: "waiting for it" in m.output(after), lambda: after.poll() is not None)
        self.assertIn("waiting for it", m.output(after))
        holds.touch()
        self.assertEqual(waited.wait(timeout=120), 0, m.output(waited))
        self.assertEqual(after.wait(timeout=120), 0, m.output(after))


class FilesFrom(unittest.TestCase):
    """A gateway's agent may send the files in the folder its gateway works in (D-090): the installer
    reads that folder from the gateway's own settings and lists it in `files_from` (provisional). It never
    lists the home itself (D-099), nor a folder that holds or lies in the agents' data, which would stop
    the core, nor one that holds or lies in the gateway's media cache, which holds the files people sent
    it on any platform, nor one that holds the gateway's own folder; then it lists none, and says how to.
    The folder it lists is there when the core starts, which opens it then or never."""

    def install(self, m: Machine, harness: str, env=None) -> str:
        return m.run(harness, *m.args(harness, "--binary", str(m.core), "--no-service"), env=env).stdout

    def settings(self, m: Machine, harness: str, env=None) -> tuple:
        out = self.install(m, harness, env)
        text = (m.data / f"agents/{harness}/settings.toml").read_text()
        return tomllib.loads(text).get("files_from"), text, out

    def hermes(self, m: Machine, terminal: dict, home=None) -> None:
        home = home or m.home / ".hermes"
        home.mkdir(parents=True)
        (home / "config.yaml").write_text(json.dumps({"terminal": terminal}))

    def openclaw(self, m: Machine, config: dict, state=None) -> None:
        state = state or m.home / ".openclaw"
        state.mkdir(parents=True, exist_ok=True)
        (state / "openclaw.json").write_text(json.dumps(config))

    def assertNone(self, files, text: str, out: str) -> None:
        self.assertIsNone(files, text)
        self.assertIn("# files_from = [", text)
        self.assertIn("The agent sends no files until you list a folder with files_from", out)

    def test_hermes_lists_the_folder_its_commands_run_in(self):
        for cwd, listed in (("{dir}/projects", "{dir}/projects"), ("~/projects", "{home}/projects")):
            with self.subTest(cwd=cwd):
                m = Machine(self)
                where = {"dir": m.dir, "home": m.home}
                self.hermes(m, {"cwd": cwd.format(**where), "backend": "local"})
                files, _, out = self.settings(m, "hermes")
                self.assertEqual(files, [listed.format(**where)])
                self.assertTrue(Path(files[0]).is_dir(), "the core opens the folder when it starts")
                self.assertIn(f"The agent may send the files in {files[0]} to a chat", out)

    def test_hermes_in_the_home_or_a_sandbox_lists_none(self):
        for terminal in ({"cwd": "."}, {}, {"cwd": "~"}, {"cwd": "/srv/projects", "backend": "docker"},
                         {"cwd": "~/.local/share"}, {"cwd": "~/.local/share/marmot-agent/agents"}):
            with self.subTest(terminal=terminal):
                m = Machine(self)
                self.hermes(m, terminal)
                self.assertNone(*self.settings(m, "hermes"))

    def test_hermes_in_a_folder_that_holds_or_lies_in_its_own_lists_none(self):
        """Hermes keeps its media cache, the files people sent it and the ones it made, in its home."""
        for cwd, hermes_home in (("~/.hermes/cache/documents", None), ("~/.hermes", None),
                                 ("{dir}/gateway", "{dir}/gateway/hermes"), ("{dir}/gateway/work", "{dir}/gateway")):
            with self.subTest(cwd=cwd, hermes_home=hermes_home):
                m = Machine(self)
                home = Path(hermes_home.format(dir=m.dir)) if hermes_home else None
                self.hermes(m, {"cwd": cwd.format(dir=m.dir)}, home)
                env = {"HERMES_HOME": str(home)} if home else None
                self.assertNone(*self.settings(m, "hermes", env))

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_openclaw_lists_the_workspace_of_the_agent_its_channel_goes_to(self):
        two = {"ownership": "explicit", "entries": {"main": {"workspace": "~/main"}, "coding": {"workspace": "~/coding"}}}
        for config, env, listed in (({"agents": {"defaults": {"workspace": "{dir}/theirs"}}}, {}, "{dir}/theirs"),
                                    ({"agents": {"defaults": {"workspace": "{dir}/theirs"}, "entries": {"main": {
                                        "workspace": "~/main"}}}}, {}, "{home}/main"),
                                    ({}, {}, "{home}/.openclaw/workspace"),
                                    ({}, {"OPENCLAW_PROFILE": "work"}, "{home}/.openclaw-work/workspace"),
                                    ({"agents": two, "bindings": [{"agentId": "coding", "match": {"channel": "marmot"}}]},
                                     {}, "{home}/coding"),
                                    # A binding for another account of the channel, which the plugin has not.
                                    ({"agents": {**two, "defaults": {"systemAgent": {"agentId": "main"}}}, "bindings": [
                                        {"agentId": "coding", "match": {"channel": "marmot", "accountId": "other"}}]},
                                     {}, "{home}/main")):
            with self.subTest(config=config, env=env):
                m = Machine(self)
                where = {"dir": m.dir, "home": m.home}
                config = json.loads(json.dumps(config).replace("{dir}", str(m.dir)))
                profile = env.get("OPENCLAW_PROFILE")
                self.openclaw(m, config, m.home / f".openclaw-{profile}" if profile else None)
                files, _, out = self.settings(m, "openclaw", env)
                self.assertEqual(files, [listed.format(**where)], out)
                self.assertTrue(Path(files[0]).is_dir(), "the core opens the folder when it starts")

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_openclaw_whose_chats_may_go_to_more_than_one_agent_lists_none(self):
        two = {"ownership": "explicit", "entries": {"main": {"workspace": "~/main"}, "coding": {"workspace": "~/coding"}}}
        for config in ({"agents": two},
                       {"agents": two, "bindings": [{"agentId": "main", "match": {"channel": "marmot"}},
                                                    {"agentId": "coding", "match": {"channel": "marmot", "peer": {
                                                        "kind": "group", "id": "c-1"}}}]},
                       {"agents": two, "bindings": [{"agentId": "coding", "match": {"channel": "marmot", "peer": {
                           "kind": "group", "id": "c-1"}}}]}):
            with self.subTest(config=config):
                m = Machine(self)
                self.openclaw(m, config)
                files, text, out = self.settings(m, "openclaw")
                self.assertNone(files, text, out)
                self.assertIn("list the workspace of the agent that takes them", text)

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_openclaw_in_a_folder_that_holds_its_state_or_its_media_lists_none(self):
        for workspace, state in (("~/.openclaw/media/outbound", None), ("~/.openclaw/media", None),
                                 ("{dir}/gateway", "{dir}/gateway/state")):
            with self.subTest(workspace=workspace, state=state):
                m = Machine(self)
                state = Path(state.format(dir=m.dir)) if state else None
                self.openclaw(m, {"agents": {"defaults": {"workspace": workspace.format(dir=m.dir)}}}, state)
                env = {"OPENCLAW_STATE_DIR": str(state)} if state else None
                self.assertNone(*self.settings(m, "openclaw", env))

    def test_a_run_again_adds_files_from_to_kept_settings_without_one_and_keeps_one_that_is_there(self):
        m = Machine(self)
        self.hermes(m, {"cwd": str(m.dir / "projects")})
        files, text, _ = self.settings(m, "hermes")
        self.assertEqual(files, [str(m.dir / "projects")])
        settings = m.data / "agents/hermes/settings.toml"
        # Settings from before the installer listed a folder.
        older = "\n".join(line for line in text.splitlines() if "files_from" not in line and "D-090" not in line)
        settings.write_text(older + "\n")
        files, again, out = self.settings(m, "hermes")
        self.assertEqual(files, [str(m.dir / "projects")])
        self.assertEqual(tomllib.loads(again)["connector"], tomllib.loads(text)["connector"])
        self.assertIn(f"The agent may send the files in {m.dir / 'projects'} to a chat", out)
        # One the person changed is theirs, even when Hermes now works elsewhere.
        settings.write_text(again.replace(str(m.dir / "projects"), str(m.dir / "mine")))
        (m.home / ".hermes/config.yaml").write_text(json.dumps({"terminal": {"cwd": str(m.dir / "elsewhere")}}))
        files, kept, out = self.settings(m, "hermes")
        self.assertEqual(files, [str(m.dir / "mine")])
        self.assertEqual(kept.count("files_from"), again.count("files_from"))
        self.assertFalse((m.dir / "elsewhere").exists())
        self.assertIn("files_from in its settings lists the folders", out)
        self.assertNotIn(str(m.dir / "elsewhere"), out)
        # None to list, and a run again adds no second comment.
        m = Machine(self)
        self.hermes(m, {})
        _, first, _ = self.settings(m, "hermes")
        _, second, _ = self.settings(m, "hermes")
        self.assertEqual(first, second)

    # The line `marmot-agent import` writes above the old connector's media folders (import/mod.rs).
    IMPORTED = "# Only the folders the old connector let files be sent from; never the working folder."

    def moved(self, m: Machine, harness: str, title: str, listed: list) -> Path:
        """An install of today's connector moved over, its settings as the import wrote them, with the
        old connector's media folders in `files_from`; and that connector's plugin in the gateway."""
        if harness == "hermes":
            old = m.home / ".hermes/plugins/marmot"
            old.mkdir(parents=True)
            (old / "plugin.yaml").write_text("name: marmot\ndescription: the old connector's\n")
        else:
            old = m.home / ".openclaw/extensions/marmot"
            old.mkdir(parents=True)
            (old / "package.json").write_text(json.dumps({"name": "@marmot-protocol/openclaw-marmot"}))
        old_home, settings = Boundaries.imported(m, harness, title)
        text = settings.read_text().replace("\n[connector]\n", "")
        line = "files_from = [" + ", ".join(json.dumps(str(f)) for f in listed) + "]"
        settings.write_text(f"{text}{self.IMPORTED}\n{line}\n\n[connector]\n")
        if harness == "openclaw":
            channel = {"enabled": True, "home": str(old_home), "socketPath": f"{old_home}/dev/wn-agent.sock"}
            config = json.loads((m.home / ".openclaw/openclaw.json").read_text())
            (m.home / ".openclaw/openclaw.json").write_text(json.dumps({**config, "channels": {"marmot": channel}}))
        return settings

    def replace(self, m: Machine, harness: str) -> tuple:
        out = m.run(harness, *m.args(harness, "--binary", str(m.core), "--no-service", "--replace-plugin")).stdout
        text = (m.data / f"agents/{harness}/settings.toml").read_text()
        return tomllib.loads(text).get("files_from"), text, out

    def test_a_moved_gateway_lists_its_folder_beside_the_old_connectors(self):
        """`--replace-plugin` lists the gateway's folder as a fresh install does (D-119), after the old
        connector's media folders, which stay; a run again lists it once."""
        for listed in ([], ["{dir}/media"], ["{dir}/media", "{dir}/more"]):
            with self.subTest(listed=listed):
                m = Machine(self)
                old = [f.format(dir=m.dir) for f in listed]
                self.hermes(m, {"cwd": str(m.dir / "projects"), "backend": "local"})
                self.moved(m, "hermes", "Hermes", old)
                files, text, out = self.replace(m, "hermes")
                self.assertEqual(files, [*old, str(m.dir / "projects")], text)
                self.assertTrue((m.dir / "projects").is_dir(), "the core opens the folder when it starts")
                self.assertNotIn(self.IMPORTED, text, "the import's comment says what is listed no longer")
                self.assertIn(f"and {m.dir / 'projects'}, where Hermes works", out)
                again, text_again, _ = self.replace(m, "hermes")
                self.assertEqual(again, files)
                self.assertEqual(text_again, text)

    def test_a_moved_gateway_in_its_home_or_a_sandbox_lists_only_the_old_folders(self):
        """The same exclusions as a fresh install: then the old connector's folders are all there is. The
        agent's home stays where the old connector had it, out of the agents' data, and a folder that holds
        or lies in it is not listed either, or the core would not start; nor one that holds or lies in the
        backup of it the import took, which the import leaves out of files_from too."""
        backup = "~/.marmot-agents/hermes.backup-20261122T000000Z"  # as Boundaries.imported names it
        for terminal in ({"cwd": "."}, {"cwd": "/srv/projects", "backend": "docker"}, {"cwd": "~/.hermes/cache"},
                         {"cwd": "~/.marmot-agents/hermes/work"}, {"cwd": "~/.marmot-agents"},
                         {"cwd": backup}, {"cwd": f"{backup}/work"}):
            with self.subTest(terminal=terminal):
                m = Machine(self)
                self.hermes(m, terminal)
                settings = self.moved(m, "hermes", "Hermes", [str(m.dir / "media")])
                before = settings.read_text()
                files, text, out = self.replace(m, "hermes")
                self.assertEqual(files, [str(m.dir / "media")])
                self.assertEqual(text, before)
                self.assertIn("files_from in its settings lists the folders", out)

    def test_a_moved_gateway_leaves_a_files_from_the_person_wrote_otherwise(self):
        """Only the line the import wrote, with its comment directly above it, is changed. A line the
        person wrote, or changed so it is no longer one the import writes, is theirs, even one in a single
        line: it is left as it is, and the installer says how to add the folder."""
        mine = {
            "several lines": 'files_from = [\n  "/srv/mine",\n]',
            "a trailing comma": 'files_from = ["/srv/mine",]',
            "a comment after it": 'files_from = ["/srv/mine"]  # mine',
            "no comment above, empty": "files_from = []",
            "no comment above": 'files_from = ["/srv/mine"]',
            "another comment above": '# Mine.\nfiles_from = ["/srv/mine"]',
        }
        for case, line in mine.items():
            with self.subTest(case=case):
                m = Machine(self)
                self.hermes(m, {"cwd": str(m.dir / "projects"), "backend": "local"})
                settings = self.moved(m, "hermes", "Hermes", [])
                text = settings.read_text()
                if case.startswith("no comment") or case.startswith("another"):
                    text = text.replace(f"{self.IMPORTED}\n", "")
                text = text.replace("files_from = []", line)
                settings.write_text(text)
                files, after, out = self.replace(m, "hermes")
                self.assertEqual(files, tomllib.loads(text).get("files_from"))
                self.assertEqual(after, text)
                self.assertIn(f"add {m.dir / 'projects'}", out)

    def test_a_moved_gateway_already_listing_its_folder_changes_nothing(self):
        """The old connector may have let files be sent from the folder the gateway works in: it is
        listed once, and the settings are left as the import wrote them."""
        m = Machine(self)
        listed = [str(m.dir / "projects")]
        self.hermes(m, {"cwd": listed[0], "backend": "local"})
        before = self.moved(m, "hermes", "Hermes", listed).read_text()
        files, text, out = self.replace(m, "hermes")
        self.assertEqual((files, text), (listed, before))
        self.assertIn(f"and {listed[0]}, where Hermes works", out)

    def test_a_moved_gateway_that_works_elsewhere_now_lists_the_new_folder(self):
        """The folder the installer added is its own while its comment stays above the line: run again
        after the gateway has moved to another folder, it lists that one in its place."""
        m = Machine(self)
        old = [str(m.dir / "media")]
        self.hermes(m, {"cwd": str(m.dir / "projects"), "backend": "local"})
        self.moved(m, "hermes", "Hermes", old)
        files, _, _ = self.replace(m, "hermes")
        self.assertEqual(files, [*old, str(m.dir / "projects")])
        (m.home / ".hermes/config.yaml").write_text(
            json.dumps({"terminal": {"cwd": str(m.dir / "elsewhere"), "backend": "local"}}))
        files, text, out = self.replace(m, "hermes")
        self.assertEqual(files, [*old, str(m.dir / "elsewhere")], text)
        self.assertTrue((m.dir / "elsewhere").is_dir())
        self.assertIn(f"and {m.dir / 'elsewhere'}, where Hermes works", out)
        again, text_again, _ = self.replace(m, "hermes")
        self.assertEqual((again, text_again), (files, text))

    def test_a_moved_gateway_folder_is_written_as_it_is_named(self):
        """A folder's name is copied into the line as it is, whatever characters it holds."""
        for name in ("R&D", "a\\b", "R&D\\&1"):
            with self.subTest(name=name):
                m = Machine(self)
                old = [str(m.dir / "media")]
                self.hermes(m, {"cwd": str(m.dir / name), "backend": "local"})
                self.moved(m, "hermes", "Hermes", old)
                files, text, _ = self.replace(m, "hermes")
                self.assertEqual(files, [*old, str(m.dir / name)], text)
                again, text_again, _ = self.replace(m, "hermes")
                self.assertEqual((again, text_again), (files, text))

    @unittest.skipUnless(REAL_NODE, "the OpenClaw installer reads JSON with node")
    def test_a_moved_openclaw_lists_its_workspace_beside_the_old_connectors(self):
        m = Machine(self)
        self.openclaw(m, {"agents": {"defaults": {"workspace": str(m.dir / "theirs")}}})
        self.moved(m, "openclaw", "OpenClaw", [str(m.dir / "media")])
        files, text, out = self.replace(m, "openclaw")
        self.assertEqual(files, [str(m.dir / "media"), str(m.dir / "theirs")], out)
        self.assertTrue((m.dir / "theirs").is_dir())


class Wrappers(unittest.TestCase):
    """The scripts that start a harness's container check its mounts again, and wait for the core."""

    def setUp(self):
        self.m = Machine(self)
        self.agent = self.m.dir / "agent"
        (self.agent / "home/socket").mkdir(parents=True)
        (self.agent / "state/x").mkdir(parents=True)
        self.work = self.m.dir / "work"
        self.work.mkdir()

    def run_script(self, script: str, *args: str, timeout=30, cwd=None, env=None) -> subprocess.CompletedProcess:
        environment = {"PATH": f"{self.m.bin}:{SYSTEM_PATH}", "FAKE_LOG": str(self.m.log), "HOME": str(self.m.home),
                       "MODEL_BASE_URL": "https://models.example/v1", "MODEL_NAME": "some/model"}
        environment.update(env or {})
        for name in [k for k, v in environment.items() if v is None]:
            del environment[name]
        return subprocess.run([str(ROOT / script), *args], env=environment, cwd=cwd or self.work,
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=timeout)

    def serve_core(self, after=0.0):
        """Plays the core: its token and its socket, in the agent's home, after a while."""
        home = self.agent / "home"

        def core():
            import socket
            import time
            time.sleep(after)
            (home / "connector.token").write_text("ab" * 32)
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(str(home / "socket/marmot-agent.sock"))
            self.addCleanup(listener.close)

        import threading
        thread = threading.Thread(target=core)
        thread.start()
        return thread

    def wrappers(self, work: Path) -> list:
        """Each container script, as its harness's install starts it, on a work folder."""
        home, state = self.agent / "home", self.agent / "state"
        return [
            ("tools/goose/goose-acp.sh", [str(self.m.key), str(self.agent / "goose")], work),
            ("tools/pi/pi.sh", [str(self.m.key), str(self.agent / "pi"), "--version"], work),
            ("packages/openclaw/run.sh", [str(home), str(state), str(work), str(self.m.key)], self.work),
            ("python/hermes/run.sh", [str(home), str(work), "v", str(self.m.key)], self.work),
        ]

    def test_the_folder_of_every_agent_is_kept_out_of_the_work_folder(self):
        data = self.m.dir / "around/data"
        (data / "agents/other/home").mkdir(parents=True)
        (self.m.dir / "into-data").symlink_to(data / "agents/other")
        env = {"MARMOT_AGENT_DATA": str(data)}
        for work in (data / "agents/other/home", self.m.dir / "into-data/home", self.m.dir / "around"):
            for script, args, cwd in self.wrappers(work):
                with self.subTest(script=script, work=str(work)):
                    result = self.run_script(script, *args, cwd=cwd, env=env)
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn("the work folder and the agents' folder must not hold one another", result.stderr)
        self.assertEqual([c for c in self.m.calls() if c.startswith("docker run")], [])
        # A work folder beside it is fine.
        self.serve_core().join()
        for script, args, cwd in self.wrappers(self.work):
            with self.subTest(script=script, work="beside"):
                result = self.run_script(script, *args, cwd=cwd, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len([c for c in self.m.calls() if c.startswith("docker run")]), 4)

    def test_no_container_script_has_a_default_endpoint_or_model(self):
        # Each needs both from its environment, as the installer writes them (D-100).
        self.serve_core().join()
        for name in ("MODEL_BASE_URL", "MODEL_NAME"):
            for script, args, cwd in self.wrappers(self.work):
                with self.subTest(script=script, without=name):
                    result = self.run_script(script, *args, cwd=cwd, env={name: None})
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn(f"set {name}", result.stderr)
        self.assertEqual([c for c in self.m.calls() if c.startswith("docker run")], [])
        # Goose takes the endpoint as one address, as the others do, and Goose's own mode unless told.
        result = self.run_script("tools/goose/goose-acp.sh", str(self.m.key), str(self.agent / "goose"),
                                 env={"MODEL_BASE_URL": "https://models.example/api/v1/"})
        self.assertEqual(result.returncode, 0, result.stderr)
        run = [c for c in self.m.calls() if c.startswith("docker run")][-1]
        self.assertIn("-e MODEL_HOST=https://models.example ", run)
        self.assertIn("-e MODEL_BASE_PATH=api/v1/chat/completions ", run)
        self.assertIn("-e MODEL_NAME=some/model ", run)
        self.assertNotIn("GOOSE_MODE", run)

    def test_openclaw_passes_on_the_plugins_lines_only(self):
        import re
        source = "".join(p.read_text() for p in sorted((ROOT / "packages/openclaw/src").glob("*.ts")))
        calls = re.findall(r'(?:\bsay|this\.warn)\((["`])((?:(?!\1).)*)\1\)', source)
        self.assertGreaterEqual(len(calls), 11, "the plugin's messages are found in its source")
        lines = []
        for _, message in calls:
            for did in ("sent without its native message", "cut to fit"):
                lines.append(re.sub(r"\$\{[^}]*\}", lambda m: did if "did" in m.group(0) else "a_kind/of-it.1", message))
        plugin = sorted({f"marmot-agent-openclaw: {line}" for line in lines})
        plugin.append("2026-11-21T10:00:00.123+00:00 marmot-agent-openclaw: OpenClaw's run of a message failed")
        gateway = [
            "2026-11-21T10:00:00.123Z [gateway] what someone said in the chat",
            "marmot-agent-openclaw: what someone said in the chat",
            "marmot-agent-openclaw: OpenClaw's run of a message failed: what someone said",
            "marmot-agent-openclaw: OpenClaw refused a call (what someone said, in full)",
        ]
        output = self.m.dir / "gateway.out"
        output.write_text("\n".join(plugin + gateway) + "\n")
        self.m.write(self.m.bin / "docker", 'echo "docker $*" >> "$FAKE_LOG"\n[ "$1" = run ] && cat "$FAKE_OUTPUT"\nexit 0\n')
        self.serve_core().join()
        result = self.run_script("packages/openclaw/run.sh", str(self.agent / "home"), str(self.agent / "state"),
                                 str(self.work), str(self.m.key), env={"FAKE_OUTPUT": str(output)})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr.splitlines(), plugin)

    def test_a_gateway_sees_its_work_folder_where_the_core_does(self):
        """A gateway's plugin hands the core the path its agent names (D-090): the container mounts the work
        folder at the same path, and its gateway works there."""
        self.serve_core().join()
        for script, args, _ in self.wrappers(self.work)[2:]:
            with self.subTest(script=script):
                result = self.run_script(script, *args)
                self.assertEqual(result.returncode, 0, result.stderr)
                run = [c for c in self.m.calls() if c.startswith("docker run")][-1]
                self.assertIn(f" --mount type=bind,source={self.work},target={self.work} ", run)
                self.assertIn(f" -w {self.work} ", run)
                self.assertIn(f" -e MARMOT_WORK={self.work} ", run)
                self.assertNotIn(":/work", run)
        comma = self.m.dir / "a,b"
        comma.mkdir()
        for script, args, _ in self.wrappers(comma)[2:]:
            with self.subTest(script=script, work="a,b"):
                result = self.run_script(script, *args)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("must not have a comma", result.stderr)

    def test_a_work_folder_holding_the_state_is_refused(self):
        home, state = self.agent / "home", self.agent / "state"
        cases = [
            ("tools/goose/goose-acp.sh", [str(self.m.key), str(self.m.dir)], "must not hold one another"),
            ("packages/openclaw/run.sh", [str(home), str(state), str(self.m.dir), str(self.m.key)], "must not hold"),
            ("packages/openclaw/run.sh", [str(home), str(state), str(state / "x/../x"), str(self.m.key)], "must not be inside"),
            ("python/hermes/run.sh", [str(home), str(self.agent), "v", str(self.m.key)], "must not hold"),
        ]
        (self.m.dir / "link").symlink_to(self.m.dir)
        cases.append(("python/hermes/run.sh", [str(home), str(self.m.dir / "link"), "v", str(self.m.key)], "must not hold"))
        for script, args, message in cases:
            with self.subTest(script=script, work=args[-3] if len(args) > 2 else args[-1]):
                result = self.run_script(script, *args)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn(message, result.stderr)
        self.assertEqual([c for c in self.m.calls() if c.startswith("docker run")], [])

    def test_openclaw_waits_for_the_core(self):
        home = self.agent / "home"
        self.serve_core(after=2)
        result = self.run_script("packages/openclaw/run.sh", str(home), str(self.agent / "state"), str(self.work),
                                 str(self.m.key))
        self.assertEqual(result.returncode, 0, result.stderr)
        run = [c for c in self.m.calls() if c.startswith("docker run")]
        self.assertEqual(len(run), 1)
        self.assertIn("--log-driver=none", run[0])
        self.assertIn(f"--mount type=bind,source={home}/connector.token,target=/marmot/connector.token,readonly",
                      run[0])
        self.assertFalse((home / "connector.token").is_dir())


class PutBack(unittest.TestCase):
    """How lib.sh puts back an upgrade that did not end, with a step of it failing, which no stand-in
    makes fail: run on lib.sh's own functions."""

    def lib(self, script: str, **env) -> subprocess.CompletedProcess:
        return subprocess.run(["bash", "-c", f'. "{HERE}/lib.sh"; DRY_RUN=0\n{script}'],
                              capture_output=True, text=True, env={**os.environ, **env})

    def test_a_plugin_change_that_fails_to_go_back_keeps_the_ones_before_it(self):
        """The plugin changes an upgrade lists go back the last first, as each one's copies are of what
        the one before it left. One that fails stops the rest, so an earlier one never puts back its
        older copies while a later one is still to go back."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            upgrade = tmp / "upgrade"
            upgrade.mkdir()
            records = [tmp / name / "plugin-change" for name in ("first", "second")]
            for record in records:
                record.parent.mkdir()
                record.write_text("")
            (upgrade / "plugin-changes").write_text("".join(f"{r}\n" for r in records))
            log = tmp / "put-back"
            ran = self.lib(
                'put_back_change() { case $1 in */second/*) return 1 ;; esac; printf "%s\\n" "$1" >>"$LOG"; }\n'
                'upgrade_put_back_changes; echo "status $?"',
                MA_UPGRADE=str(upgrade), LOG=str(log))
            self.assertIn("status 1", ran.stdout, ran.stderr)
            self.assertFalse(log.exists(), "nothing put back before the one that failed")
            self.assertTrue(all(r.exists() for r in records))

    def test_the_change_of_a_run_that_fails_goes_back_in_its_turn(self):
        """A run that fails puts its own plugin change back in the same replay as the others its upgrade
        lists, the last first: listed before another, it goes back after it, as the other's copies may
        be of what this one left. Before, it went back first, and the other's copies over it (#180). One
        that fails to go back still stops those before it."""
        for failing, expected in ((None, ["second", "first"]), ("second", [])):
            with self.subTest(failing=failing), tempfile.TemporaryDirectory() as tmp:
                tmp = Path(tmp)
                data = tmp / "data"
                upgrade = data / "upgrade"
                upgrade.mkdir(parents=True)
                (upgrade / "list").write_text("")
                records = [tmp / name / "plugin-change" for name in ("first", "second")]
                for record in records:
                    record.parent.mkdir()
                    record.write_text("")
                (upgrade / "plugin-changes").write_text("".join(f"{r}\n" for r in records))
                log = tmp / "put-back"
                ran = self.lib(
                    'put_back_change() { r=${1:-$MA_CHANGE}; case $r in */$FAILING/*) return 1 ;; esac\n'
                    '    basename "$(dirname "$r")" >>"$LOG"; }\n'
                    'image_put_back_from() { return 0; }\n'
                    'HARNESS_TITLE=Hermes MA_UPGRADING=1 MA_CHANGING=1 MA_PREPARED=0 MA_WORK="" MA_STAGED=0\n'
                    'finish 1',
                    MA_DATA=str(data), MA_UPGRADE=str(upgrade), MA_CORE=str(tmp / "core"), MA_LIB=str(tmp / "lib"),
                    MA_CHANGE=str(records[0]), LOG=str(log), FAILING=failing or "none")
                self.assertEqual(log.read_text().split() if log.exists() else [], expected, ran.stderr)
                self.assertEqual([r.exists() for r in records], [bool(failing)] * 2, ran.stderr)

    def test_a_record_of_this_runs_change_that_cannot_be_removed_keeps_the_upgrade(self):
        """A run that fails puts its own plugin change back, and then cannot remove the record of it: the
        record stays, with the copy of our plugin it names, the replay stops before the changes listed
        before it, and the upgrade's record is kept, for the next run to put the same copies back. Before,
        the copy went and the replay went on, retiring the upgrade with the record naming a copy gone."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            data = tmp / "data"
            upgrade = data / "upgrade"
            upgrade.mkdir(parents=True)
            (upgrade / "list").write_text("")
            records = [tmp / name / "plugin-change" for name in ("first", "second")]
            copy = tmp / "second" / "backups" / "marmot.copy"
            copy.mkdir(parents=True)
            records[0].parent.mkdir()
            records[0].write_text("")
            records[1].write_text(f"own\t{tmp / 'plugin'}\t{copy}\n")
            (upgrade / "plugin-changes").write_text("".join(f"{r}\n" for r in records))
            log = tmp / "put-back"
            ran = self.lib(
                'put_back_change() { basename "$(dirname "${1:-$MA_CHANGE}")" >>"$LOG"; }\n'
                'image_put_back_from() { return 0; }\n'
                'rm() { if [ "$#" -eq 2 ] && [ "$2" = "$MA_CHANGE" ]; then echo "rm: cannot remove" >&2; return 1; fi\n'
                '    command rm "$@"; }\n'
                'HARNESS_TITLE=Hermes MA_UPGRADING=1 MA_CHANGING=1 MA_PREPARED=0 MA_WORK="" MA_STAGED=0\n'
                'finish 1',
                MA_DATA=str(data), MA_UPGRADE=str(upgrade), MA_CORE=str(tmp / "core"), MA_LIB=str(tmp / "lib"),
                MA_CHANGE=str(records[1]), LOG=str(log))
            self.assertEqual(log.read_text().split(), ["second"], ran.stderr)
            self.assertEqual([r.exists() for r in records], [True, True], ran.stderr)
            self.assertTrue(copy.exists(), ran.stderr)
            self.assertTrue((upgrade / "plugin-changes").exists(), ran.stderr)

    def test_a_change_names_this_gateway_only_when_its_path_is_the_same(self):
        """changes_ours compares the settings file a plugin change names with this run's gateway as they
        are written: a backslash in a path is a backslash, not an escape. Before, awk -v read the path
        given, so /profiles/\\141/config.yaml was taken for the gateway /profiles/a/config.yaml."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            upgrade = tmp / "upgrade"
            upgrade.mkdir()
            record = tmp / "agents" / "a" / "plugin-change"
            record.parent.mkdir(parents=True)
            record.write_text(f"file\t{tmp}/profiles/a/config.yaml\t\n")
            (upgrade / "plugin-changes").write_text(f"{record}\n")
            for gateway, ours in ((f"{tmp}/profiles/a/config.yaml", True), (f"{tmp}/profiles/\\141/config.yaml", False)):
                with self.subTest(gateway=gateway):
                    ran = self.lib('if changes_ours; then echo ours; else echo other; fi',
                                   MA_UPGRADE=str(upgrade), MA_AGENT_DIR=str(record.parent), MA_GATEWAY=gateway)
                    self.assertEqual(ran.stdout.strip(), "ours" if ours else "other", ran.stderr)

    def test_an_upgrade_of_this_harness_is_ours_however_its_parts_are_written(self):
        """upgrade_ours looks each part of the upgrade up in lib_parts, through a pipe, with pipefail on as
        the scripts run: grep reads all of it, so lib_parts never writes into a pipe grep has left. Before,
        grep -q left at the first match, and lib_parts, writing after it, failed with SIGPIPE: the part
        was taken for another harness's, and the run put back an upgrade it was to finish, now and then."""
        with tempfile.TemporaryDirectory() as tmp:
            upgrade = Path(tmp) / "upgrade"
            upgrade.mkdir()
            (upgrade / "list").write_text("core\tpresent\npart\tLICENSE\tpresent\n")
            ran = self.lib("set -o pipefail\n"
                           "lib_parts() { printf 'LICENSE\\n'; sleep 0.2; printf 'install\\n'; }\n"
                           "if upgrade_ours; then echo ours; else echo other; fi",
                           MA_UPGRADE=str(upgrade))
            self.assertEqual(ran.stdout.strip(), "ours", ran.stderr)

    def test_a_plugin_change_that_cannot_go_back_in_place_keeps_the_upgrade(self):
        """A run stopped once it had moved its plugin change into the upgrade's record puts the change
        back in its place as it ends. When it cannot, the upgrade's record is kept, the change in it,
        and not retired with the core put back beside the changed plugin."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            data = tmp / "data"
            upgrade = data / "upgrade"
            upgrade.mkdir(parents=True)
            (upgrade / "list").write_text("")
            (upgrade / "plugin-change").write_text("")
            # Its place is in a folder that is gone, so it cannot be put there.
            (upgrade / "plugin-change.path").write_text(f"{tmp / 'gone/plugin-change'}\n")
            ran = self.lib(
                'image_put_back_from() { return 0; }\n'
                'MA_UPGRADING=1 MA_CHANGING=0 MA_PREPARED=0 MA_WORK="" MA_STAGED=0\nfinish 1',
                MA_DATA=str(data), MA_UPGRADE=str(upgrade), MA_CORE=str(tmp / "core"), MA_LIB=str(tmp / "lib"))
            self.assertTrue((upgrade / "plugin-change").exists(), ran.stderr)
            self.assertTrue((upgrade / "list").exists(), "the upgrade is still under way")
            self.assertIn("Run this again", ran.stderr)


class Download(unittest.TestCase):
    """The default: a release download, checked against its SHA256SUMS."""

    def release(self, m: Machine, tamper=False) -> str:
        target = subprocess.run(
            ["bash", "-c", f'. "{HERE}/lib.sh"; OS=$(uname -s); MA_MACHINE=$(uname -m); release_target'],
            capture_output=True, text=True, check=True,
        ).stdout
        folder = m.dir / "releases/download/v1.2.3"
        folder.mkdir(parents=True)
        subprocess.run(
            [str(ROOT / "tools/release/package.sh"), str(m.core), target, str(folder)],
            check=True, capture_output=True,
        )
        asset = folder / f"marmot-agent-{target}.tar.gz"
        digest = hashlib.sha256(asset.read_bytes()).hexdigest()
        if tamper:
            digest = ("0" if digest[0] != "0" else "1") + digest[1:]
        (folder / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
        return f"file://{m.dir}/releases"

    def test_download(self):
        m = Machine(self)
        url = self.release(m)
        m.run("codex", *m.args("codex", "--version", "v1.2.3", "--no-service"), env={"MARMOT_AGENT_RELEASES": url})
        core = m.home / ".local/bin/marmot-agent"
        self.assertEqual(core.read_text(), FAKE_CORE)
        self.assertTrue((m.data / "lib/install/codex.sh").is_file())
        self.assertIn("marmot-agent init", "\n".join(m.calls()))

    def test_checksum_mismatch(self):
        m = Machine(self)
        url = self.release(m, tamper=True)
        result = m.run("codex", *m.args("codex", "--version", "v1.2.3", "--no-service"),
                       env={"MARMOT_AGENT_RELEASES": url}, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match its checksum", result.stderr)
        self.assertFalse((m.home / ".local/bin/marmot-agent").exists())


def verify_units(test: unittest.TestCase, units: list) -> None:
    """Has systemd read the units, where this machine can."""
    analyze = shutil.which("systemd-analyze")
    if not analyze:
        return
    result = subprocess.run([analyze, "verify", *map(str, units)], capture_output=True, text=True)
    # A unit another one names, such as the core for its connector, is found beside it; nothing else
    # systemd says about the machine itself counts.
    problems = [line for line in result.stderr.splitlines() if any(str(u) in line for u in units)]
    test.assertEqual(problems, [], result.stderr)


if __name__ == "__main__":
    unittest.main()
