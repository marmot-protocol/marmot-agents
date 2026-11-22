"""Tests for the install scripts in this folder (D-091).

    python3 -m unittest discover -s install -p 'test_*.py'

Each script runs against a throwaway HOME, with stand-ins for the programs it calls (the harnesses,
systemctl, launchctl, docker, npm and node) on PATH, so nothing on the machine running the tests is
installed, started or changed. A stand-in marmot-agent plays the core; the download test serves a
release from a folder, packaged by tools/release/package.sh.
"""

import hashlib
import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
HARNESSES = ["codex", "claude-code", "opencode", "goose", "cursor", "pi", "openclaw", "hermes"]
SOCKET = {"claude-code", "openclaw", "hermes"}
CONTAINER = {"goose", "pi", "openclaw", "hermes"}
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
FAKE_LOGGER = """#!/bin/sh
echo "$(basename "$0") $*" >> "$FAKE_LOG"
"""
# npm ci makes the packages it installs, as the real one does.
FAKE_NPM = """#!/bin/sh
echo "npm $*" >> "$FAKE_LOG"
for last in "$@"; do :; done
mkdir -p "$last/node_modules/a-package"
"""
FAKE_NODE = """#!/bin/sh
[ "$1" = -p ] && { echo "${FAKE_NODE_MAJOR:-24}"; exit 0; }
echo "node $*" >> "$FAKE_LOG"
"""


def mode(path: Path) -> int:
    return stat.S_IMODE(path.stat().st_mode)


class Machine:
    """A throwaway HOME and a PATH of stand-ins."""

    def __init__(self, test: unittest.TestCase, programs=("codex", "opencode", "cursor-agent")):
        self.dir = Path(tempfile.mkdtemp(prefix="marmot-install-"))
        test.addCleanup(shutil.rmtree, self.dir, True)
        self.home = self.dir / "home"
        self.home.mkdir()
        self.bin = self.dir / "bin"
        self.bin.mkdir()
        self.log = self.dir / "calls.log"
        self.log.touch()
        self.core = self.dir / "marmot-agent"
        self.write(self.core, FAKE_CORE)
        for name in ("systemctl", "launchctl", "docker", *programs):
            self.write(self.bin / name, FAKE_LOGGER)
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

    def run(self, harness: str, *args: str, env=None, check=True) -> subprocess.CompletedProcess:
        environment = {
            "HOME": str(self.home),
            "PATH": f"{self.bin}:{SYSTEM_PATH}",
            "FAKE_LOG": str(self.log),
            "LANG": "C",
        }
        environment.update(env or {})
        result = subprocess.run(
            ["bash", str(HERE / f"{harness}.sh"), *args],
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=120,
        )
        if check and result.returncode != 0:
            raise AssertionError(f"{harness}.sh failed ({result.returncode}):\n{result.stdout}\n{result.stderr}")
        return result

    def args(self, harness: str, *more: str) -> list:
        args = ["--yes", "--owner", OWNER, "--cwd", str(self.home / "work")]
        if harness in CONTAINER:
            args += ["--model-key", str(self.key), *MODEL]
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
        for harness in HARNESSES:
            with self.subTest(harness=harness):
                m = Machine(self)
                out = m.run(harness, *m.args(harness, "--binary", str(m.core), "--dry-run")).stdout
                self.assertEqual(m.files(), [], "a dry run writes nothing")
                self.assertEqual([c for c in m.calls() if c != "docker info"], [], "a dry run only looks")
                agent = m.data / "agents" / harness
                self.assertIn(f"would write {agent}/settings.toml (mode 600):", out)
                self.assertIn(f'    | owner = "{OWNER}"', out)
                self.assertIn(f'    | home = "{agent}/home"', out)
                self.assertIn(f"would run: {m.home}/.local/bin/marmot-agent init {agent}/settings.toml", out)
                unit = m.units / f"marmot-agent-{harness}.service"
                self.assertIn(f"would write {unit} (mode 644):", out)
                self.assertIn(f'    | ExecStart="{agent}/start"', out)
                self.assertIn("    | Restart=always", out, "the core is started again when its harness stops")
                self.assertIn(f"would write {agent}/start (mode 700):", out)
                self.assertIn(f"    | exec '{m.home}/.local/bin/marmot-agent' run '{agent}/settings.toml'", out)
                connector = m.units / f"marmot-agent-{harness}-connector.service"
                if harness in SOCKET:
                    self.assertIn("    | [connector]", out)
                    self.assertIn(f"would write {agent}/connector (mode 700):", out)
                    self.assertIn(f"would write {connector} (mode 644):", out)
                    self.assertIn(f"    | BindsTo=marmot-agent-{harness}.service", out)
                    self.assertIn(f"    | ExecStart=\"{agent}/connector\"", out)
                else:
                    self.assertNotIn(str(connector), out)
                    self.assertNotIn("[connector]", out)
                self.assertIn("would run: systemctl --user daemon-reload", out)
                self.assertIn(f"would run: systemctl --user restart marmot-agent-{harness}.service", out)
                if harness in CONTAINER:
                    self.assertIn("would run: docker build", out)
                    self.assertIn(str(m.key), out, "the key is named by its path")
                self.assertNotIn("not a real key", out, "the key is never read into a file")

    def test_harness_sections(self):
        m = Machine(self)
        expected = {
            "codex": ["[codex]", f'program = "{m.bin}/codex"'],
            "opencode": ["[acp]", 'name = "opencode"', f'command = ["{m.bin}/opencode", "acp"]'],
            "cursor": ["[acp]", 'name = "cursor"', f'command = ["{m.bin}/cursor-agent", "acp"]'],
            "goose": [
                "[acp]",
                f'command = ["{m.data}/lib/tools/goose/goose-acp.sh", "{m.key}", "{m.data}/agents/goose/goose"]',
            ],
            "pi": [
                "[pi]",
                f'program = "{m.data}/lib/tools/pi/pi.sh"',
                f'args = ["{m.key}", "{m.data}/agents/pi/pi"]',
            ],
        }
        for harness, lines in expected.items():
            with self.subTest(harness=harness):
                out = m.run(harness, *m.args(harness, "--binary", str(m.core), "--dry-run")).stdout
                for line in lines:
                    self.assertIn(f"    | {line}", out)
                self.assertIn(f'    | cwd = "{m.home}/work"', out)

    def test_goose_endpoint_and_mode(self):
        m = Machine(self)
        out = m.run(
            "goose",
            *m.args("goose", "--binary", str(m.core), "--dry-run", "--model-url", "https://models.example/v1/"),
        ).stdout
        self.assertIn("    | MODEL_BASE_URL='https://models.example/v1/'", out)
        self.assertIn("    | MODEL_NAME='some/model'", out)
        self.assertNotIn("| GOOSE_MODE='", out, "Goose runs in its own default mode, which asks nothing (D-100)")
        out = m.run("goose", *m.args("goose", "--binary", str(m.core), "--dry-run", "--goose-mode", "approve")).stdout
        self.assertIn("    | GOOSE_MODE='approve'", out)

    def test_macos_launch_agents(self):
        for harness in ("codex", "claude-code"):
            with self.subTest(harness=harness):
                m = Machine(self)
                out = m.run(
                    harness, *m.args(harness, "--binary", str(m.core), "--dry-run"), env={"MARMOT_AGENT_OS": "Darwin"}
                ).stdout
                plist = m.home / f"Library/LaunchAgents/org.marmot.marmot-agent.{harness}.plist"
                self.assertIn(f"would write {plist} (mode 644):", out)
                self.assertIn(f"    |         <string>{m.data}/agents/{harness}/start</string>", out)
                self.assertIn("    |     <key>KeepAlive</key><true/>", out)
                self.assertIn(f"would run: launchctl bootstrap gui/{os.getuid()} {plist}", out)
                self.assertNotIn("systemctl", out)
                connector = m.home / f"Library/LaunchAgents/org.marmot.marmot-agent.{harness}.connector.plist"
                self.assertEqual(str(connector) in out, harness in SOCKET)

    def test_containers_are_linux_only(self):
        m = Machine(self)
        result = m.run("pi", *m.args("pi", "--binary", str(m.core), "--dry-run"),
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
        self.assertEqual(sorted(p.name for p in m.data.iterdir()), ["agents", "lib"], "nothing staged is left")
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

    def test_container_harness(self):
        m = Machine(self)
        m.run("hermes", *m.args("hermes", "--binary", str(m.core), "--model", "some/model:free", "--no-start"))
        agent = m.data / "agents/hermes"
        env = (agent / "service.env").read_text()
        self.assertIn("MODEL_NAME='some/model:free'", env)
        # Docker's names are shared by the machine's accounts: the volume and the container are named
        # after the agent's folder too, and the image, built for this account's user, after that user.
        volume = "marmot-agent-hermes-hermes-" + hashlib.sha256(os.path.realpath(agent).encode()).hexdigest()[:12]
        image = f"marmot-agents-hermes-{os.getuid()}-{os.getgid()}"
        self.assertIn(f"HERMES_CONTAINER='{volume}'", env)
        self.assertIn(f"HERMES_IMAGE='{image}'", env)
        self.assertIn(f"MARMOT_AGENT_DATA='{m.data}'", env)
        self.assertNotIn("not a real key", env)
        text = (agent / "connector").read_text()
        self.assertIn(f"'{m.data}/lib/python/hermes/run.sh' '{agent}/home' '{m.home}/work' "
                      f"'{volume}' '{m.key}'", text)
        calls = m.calls()
        self.assertIn(f"docker volume create {volume}", calls)
        self.assertTrue(any(c.startswith("docker build -q --build-arg UID=") and f" -t {image} " in c for c in calls))
        self.assertNotIn("systemctl --user restart", "\n".join(calls), "--no-start starts nothing")
        # Installed again, it keeps the same names, and so Hermes's home.
        m.run("hermes", "--yes", "--binary", str(m.core), "--no-start")
        self.assertIn(f"HERMES_CONTAINER='{volume}'", (agent / "service.env").read_text())

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
                    args = m.args(harness, "--binary", str(m.core))
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
        result = m.run("pi", *m.args("pi", "--binary", str(m.core)), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("chmod 600", result.stderr)
        self.assertEqual(m.files(), [])

    def test_model_key_outside_the_work_folder(self):
        m = Machine(self)
        (m.home / "work").mkdir()
        key = m.home / "work/key"
        key.write_text("x")
        key.chmod(0o600)
        result = m.run("pi", "--yes", "--owner", OWNER, "--cwd", str(m.home / "work"), "--model-key", str(key),
                       "--binary", str(m.core), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not be inside the work folder", result.stderr)

    def test_unknown_option(self):
        m = Machine(self)
        result = m.run("codex", "--model-key", str(m.key), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("takes no --model-key", result.stderr)


class Boundaries(unittest.TestCase):
    """What keeps an install from handing a harness the keys, or another agent's service."""

    def refused(self, m: Machine, harness: str, *args: str, message: str) -> None:
        result = m.run(harness, *args, check=False)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)

    def test_work_folder_is_compared_with_links_and_dots_resolved(self):
        m = Machine(self)
        project = m.dir / "project"
        (project / "sub").mkdir(parents=True)
        key = project / "model.key"
        key.write_text("x")
        key.chmod(0o600)
        (m.dir / "link").symlink_to(project)
        common = ["--yes", "--owner", OWNER, "--binary", str(m.core), "--model-key", str(key), *MODEL]
        inside = "must not be inside the work folder"
        # The key's folder, named through `..`, through a link, and through a link to a folder below it.
        self.refused(m, "goose", *common, "--cwd", f"{project}/sub/..", message=inside)
        self.refused(m, "goose", *common, "--cwd", str(m.dir / "link"), message=inside)
        self.refused(m, "pi", *common, "--cwd", f"{m.dir}/link/sub/../", message=inside)
        # The key named through a link from outside the work folder.
        outside = m.dir / "outside.key"
        outside.symlink_to(key)
        self.refused(m, "pi", "--yes", "--owner", OWNER, "--binary", str(m.core), "--model-key", str(outside),
                     "--cwd", str(project), message=inside)
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
        m.run("pi", *m.args("pi", "--binary", str(m.core)))
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
        result = m.run("goose", *m.args("goose", "--binary", str(m.core), "--model-url", "ftp://models.example"),
                       check=False)
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
        m.run("pi", *m.args("pi", "--binary", str(m.core), "--no-service"))
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
        m.run("goose", *m.args("goose", "--binary", str(m.core), "--no-service"))
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

    def test_goose_needs_x86_64(self):
        m = Machine(self)
        result = m.run("goose", *m.args("goose", "--binary", str(m.core)), env={"MARMOT_AGENT_MACHINE": "aarch64"},
                       check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("x86_64 only", result.stderr)
        self.assertEqual(m.files(), [])

    def test_without_a_service_the_launcher_has_the_environment(self):
        m = Machine(self)
        out = m.run("goose", *m.args("goose", "--binary", str(m.core), "--no-service",
                                     "--model-url", "https://models.example/v1")).stdout
        start = m.data / "agents/goose/start"
        self.assertIn(f"Run it with: {start}", out)
        m.log.write_text("")
        subprocess.run([str(start)], check=True, env={"PATH": SYSTEM_PATH, "FAKE_LOG": str(m.log)})
        self.assertIn("with MODEL_BASE_URL=https://models.example/v1", m.calls())


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
