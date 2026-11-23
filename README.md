# marmot-agents

Connectors that put AI agents into [Marmot](https://github.com/marmot-protocol/marmot) group chats.

You run an agent harness such as Claude Code, Codex or Hermes on your own machine. This project connects it to an
end-to-end encrypted group, so you and the people you choose can talk to it from White Noise on your phone, see
what it is doing, approve what it asks, and control it as fully as you could at the terminal.

> **Status: building towards the swap.** The design is in [`docs/`](docs/README.md). The code so far is a Rust
> workspace under [`crates/`](crates), with the core and the Codex driver, and the Claude Code driver in
> [`packages/claude-code`](packages/claude-code). The core runs end to end with Codex
> ([how to try it](docs/try.md)).
> The connectors that ship today live in [MDK](https://github.com/marmot-protocol/mdk) under `integrations/`.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/overview-dark.svg">
  <img alt="People in a Marmot group reach agent harnesses through one core program. Codex, Pi and ACP harnesses, Cursor among them, use drivers built into the core. Claude Code, Hermes and OpenClaw connect as separate programs over a local socket." src="docs/images/overview-light.svg">
</picture>

## The idea in four sentences

1. Every harness is driven through its own richest interface, so nothing it can do is lost.
2. Each harness gets a small driver that translates its events into one shared set of Marmot events.
3. One core program handles everything that is the same for all harnesses: the Marmot account, who may talk to the
   agent, and which chat belongs to which session.
4. Harnesses that need another language connect to the core over a local socket, and so do Hermes and OpenClaw,
   messaging gateways in their own right, through a channel plugin that runs inside each.

## Installing an agent

Each harness has an install script in [`install/`](install): `codex.sh`, `claude-code.sh`, `opencode.sh`,
`cursor.sh`, `goose.sh`, `pi.sh`, `openclaw.sh` and `hermes.sh`. They work the same on Linux, on x86_64 or
arm64, and on macOS with Apple silicon. Intel Macs are not supported: the Odin compiler the core needs to build
has no pinned release for them, and there is no release of the core for them. A script never installs the
harness: it uses the one you already have, set up with your own model and sign-in
([D-102](docs/decisions.md#d-102-the-installers-connect-to-the-harness-the-person-already-has)). Codex, Claude
Code, OpenCode, Cursor, Goose and Pi must be on your `PATH`; Hermes and OpenClaw installed, as `hermes` and
`openclaw`. If one is missing, the script says how to get it and stops.

**There is no release yet**, so for now build `marmot-agent` from a checkout of this repository. You need Rust,
from [rustup](https://rustup.rs) (the pinned compiler installs itself on the first build), a C toolchain (on
macOS, Xcode's command-line tools), and the exact Odin release MDK's markdown parser is built with, which
`tools/odin/install.sh` downloads and checks. Then, from the checkout:

```sh
bash tools/odin/install.sh "$HOME/.local/share/marmot-odin"    # once; the folder must be absolute
export ODIN="$HOME/.local/share/marmot-odin/odin"
install/codex.sh --from-source --owner <your public key, hex or npub>
```

`--from-source` builds the core here, which takes a while the first time, and installs it. Once a release is
published, leave it out: the script then downloads `marmot-agent` from the latest release, for Linux on x86_64
or arm64 or macOS on Apple silicon, and checks it against the release's `SHA256SUMS`; until then that download
fails. The script makes the agent's identity and prints its public key, for you to
invite it from White Noise, and runs it as a service of your account: systemd `--user` on Linux, launchd on
macOS. Only you may prompt it or approve what it does until you change its settings. The core runs the coding
harnesses as you, in the work folder (`--cwd`), which must not hold the agents' folder. Pi asks nobody before a
tool runs, so it acts with your rights, as when you run it yourself; `pi.sh --container` runs it in a throwaway
container instead, for a boundary, and `goose.sh --container` does the same for Goose. A container runs on a
model endpoint you bring, and there is no default: give `--model-url <url>`, an OpenAI-compatible endpoint,
`--model <name>`, the model as it names it, and `--model-key <file>`, its API key in a file only you can read,
which is mounted into the container, read when the harness starts, and never copied; the work folder must not
hold it. The script stops without them, and refuses them without `--container`: your own Pi or Goose uses the
model it is set up with. Goose runs in its own default mode, which asks nothing; `--goose-mode approve` makes it
ask in the chat before each tool. Containers need Linux and Docker, and Goose's needs x86_64. Hermes and
OpenClaw keep the folder they work in; their installers let the agent send the files in it (`files_from` in its
settings), unless it is your whole home or holds the files people sent the gateway,
and then say how to name one. Whatever the
work folder, the agent never sends a file from its own account home, which holds its keys, unless you allow
it in its settings
([D-099](docs/decisions.md#d-099-files-from-the-agents-own-home-are-refused-by-default-and-the-owner-may-allow-them)). Claude
Code must be signed in on your account (run `claude`, then `/login`): an `ANTHROPIC_API_KEY` in your shell
does not reach the service, and the script refuses to rely on one.

For Hermes and OpenClaw, the script adds our channel plugin to your own Hermes or OpenClaw with their own commands, and
points it at the agent; you restart your gateway (`hermes gateway restart`, `openclaw gateway restart`) for it
to load the plugin. It changes only what the plugin needs (the `marmot` platform or channel, and for Hermes
`MARMOT_ALLOW_ALL_USERS`, so that Hermes takes whatever the agent lets through; for OpenClaw, if it forwards no
approvals, those of Marmot chats), keeps a copy of your settings in the agent's folder first, puts them back
if it fails or is stopped part way, and takes out what it added on `--uninstall`. One Hermes or OpenClaw serves one agent. If it has the Marmot plugin of today's
connectors, the script stops: move that install over first, with its settings in the agent's folder
([`docs/move.md`](docs/move.md)), then run it with `--replace-plugin`. It then uses those settings and the
identity in the old home, and makes no new one.

- `--dry-run` shows every file and command the script would write and run, and does nothing.
- Running the script again updates `marmot-agent` and the service, and keeps the agent's settings and
  identity. Until there is a release, give it `--from-source` each time. A `--cwd` or `--model-key` given
  again replaces the old one, and nothing else in the settings.
- `--uninstall` stops and removes the service, and keeps the agent's folder.
- `--no-service` sets up no service; run the agent with the `start` script in its folder (and `connector`, for
  Claude Code), which the service would run.
- The service is started again whenever the core stops, as when its harness exits, unless you stopped it.
- `--help` lists the rest: the work folder, the agent's name, its relays.

Where things go ([D-100](docs/decisions.md#d-100-the-installers-defaults)): `marmot-agent` in `~/.local/bin`, the connectors and plugins in
`~/.local/share/marmot-agent/lib`, and each agent in `~/.local/share/marmot-agent/agents/<name>`, with its
`settings.toml`, its account home and its launchers. An agent's name belongs to the harness it was installed
with. The service is `marmot-agent-<name>` (and `marmot-agent-<name>-connector`
for Claude Code), or the launchd agent `org.marmot.marmot-agent.<name>`. Releases are built
by [GitHub Actions](.github/workflows/release.yml) from a version tag, and are not signed yet
([Q-28](docs/open-questions.md#q-28-how-does-someone-know-a-release-is-genuine)).

## Where to go next

| You want | Read |
| --- | --- |
| The design in three minutes | [`docs/README.md`](docs/README.md) |
| What is true of the system now | [`docs/current-state.md`](docs/current-state.md) |
| What has been decided, and why | [`docs/decisions.md`](docs/decisions.md) |
| What is still open | [`docs/open-questions.md`](docs/open-questions.md) |
| How the parts fit together | [`docs/design/architecture.md`](docs/design/architecture.md) |
| Rules for coding agents working here | [`AGENTS.md`](AGENTS.md) |
