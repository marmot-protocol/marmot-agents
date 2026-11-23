# Moving an install over

Status: describes `marmot-agent import` as built
([D-089](decisions.md#d-089-an-install-moves-over-by-importing-its-account-home-in-place)), and what
[the tests](#what-is-proven) show of it. How the old
allow-lists become the new settings, who becomes the owner unasked, `files_from` (and the core refusing to
start when one overlaps the home), `--leave-running`, what the import does with a home that holds several
identities, are proposals until the project owner accepts them. That an installer killed outright may leave a
gateway with no plugin that works until it runs again, at the moments named below, is decided
([D-116](decisions.md#d-116-a-gateways-plugin-swap-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)).
Decided for a gateway's move ([D-119](decisions.md#d-119-how-a-gateway-install-moves-over-its-work-folder-a-half-done-openclaw-move-a-fresh-conversation)): the
OpenClaw installer finishes a half-done move in OpenClaw's settings file, the gateway's conversation starts
fresh, and `--replace-plugin` lists the gateway's work folder in `files_from`.

One of today's connectors (installed by MDK's `install-*-marmot.sh`) moves to the core without anyone inviting
the agent again. The core uses the old account home where it is, so the agent keeps its identity, its keys, its
encryption state and its chats.

## Steps

There is no release of `marmot-agent` yet: build it from a checkout first, as the
[README](../README.md#installing-an-agent) says, and give the installer `--from-source` in step 3.

1. See what the import would do. It reads the home, its services and its allow-lists, and changes nothing:

   ```sh
   marmot-agent import ~/.marmot-agents/codex --dry-run
   ```

   It says which connector the home belongs to, what it carries over, and, plainly, what it does not.

2. Run it:

   ```sh
   marmot-agent import ~/.marmot-agents/codex
   ```

   For Hermes and OpenClaw, put the settings where the installer looks for them, in the agent's folder
   (`hermes` here is the agent's name, the installer's `--name`, which is the harness's by default):

   ```sh
   mkdir -p ~/.local/share/marmot-agent/agents/hermes
   marmot-agent import ~/.marmot-agents/hermes --settings ~/.local/share/marmot-agent/agents/hermes/settings.toml
   ```

   In this order, it:

   - stops and disables the old services (`wn-agent-codex.service` and `wn-codex.service`, or the
     LaunchAgents on a Mac), keeping their files, and checks that systemd or launchd reports each one gone;
   - takes the home's runtime lease, the lock `wn-agent` and the core hold while they run, and stops if
     anything else holds it; it keeps the lease until the settings are written;
   - copies the whole home to a dated folder beside it, such as `codex.backup-20261121T093000Z`, that only
     you can read, and checks the copy byte for byte; the copy is made under a hidden name and renamed into
     place only once checked;
   - writes the settings to `marmot-agent.toml` in the home, readable only by you, never over a file already
     there.

   If a step fails, it says which services it has already stopped and how to start them again.

   It stops only services that run the old connector's own programs (`wn-agent` and the `wn-<harness>`
   programs), whatever the unit is called. Anything else that names the home, such as a backup job, is listed
   and the import refuses to go on until you say it may keep running with `--leave-running <name>`. The import
   finds services by their unit files, not by whether they run, so stopping such a service yourself is not
   enough: pass `--leave-running <name>` for it as well, or remove its unit file.

   Before it copies anything, with the old services stopped and the home held, it reads the install again; if
   the old allow-lists changed as the services stopped, it stops there and says how to start them again.

3. Start the core with those settings, as the import tells you: `marmot-agent run
   ~/.marmot-agents/codex/marmot-agent.toml`. For Claude Code, start its driver too. For Hermes and OpenClaw,
   run the installer instead, with `--replace-plugin`, such as `install/hermes.sh --replace-plugin`: it uses
   the imported settings and the identity in the old home, never makes a new one, runs the core as a service,
   and puts the new plugin in the gateway in place of the old one, pointed at that home. It keeps a copy of
   the old plugin and of the gateway's settings in the agent's folder, under `backups`, and takes the old
   plugin's own settings out of the gateway's (for OpenClaw, its socket, account and sender policy, which
   OpenClaw would refuse once the new plugin is in). The gateway may keep running while it does. Then
   restart the gateway. Without settings in the agent's folder, `--replace-plugin` stops before changing
   anything. The move is done once the core runs as a service. If the installer fails before that, or is
   stopped (Ctrl-C, or a `TERM`), it puts the gateway's settings and the old plugin back from those copies
   before it ends, and says so; run it again once what failed is fixed. It notes the copies in
   `plugin-change` in the agent's folder until the move is done, so if it is killed outright, run it
   again: it finishes the move from the copies the killed run kept, and takes none of its own, and if
   that run fails, it puts back what the gateway had before the killed run.

   Killed outright, the gateway has the old plugin or the new one, set up to start, but for these moments,
   in which it has no plugin that works until the installer runs again:
   - after the new plugin is in and before the core's service is started (the import stopped the old
     services, so the old plugin would have nothing to connect to either);
   - for OpenClaw, after the old plugin's settings come out and before the new plugin is in, and after it
     is in and before its settings are set: OpenClaw refuses settings its installed plugin does not take,
     so no order of its commands avoids these;
   - for Hermes, where `mv` cannot exchange two folders in one step (macOS, and Linux before GNU
     coreutils 9.5), between moving the old plugin aside and renaming the new one in. Elsewhere the two
     are exchanged at once.

   A failure as the core's service is started (`systemctl` or `launchctl` failing) also puts the gateway
   back, but leaves that service in place, and the core may have run: to go back to today's connector
   from there, follow [going back](#going-back), with the backup of the home. A run that finds the new
   plugin in and the old plugin's settings still in the channel, which OpenClaw then refuses to read or
   start with, takes those settings out of OpenClaw's settings file, after keeping a copy of it
   ([D-119](decisions.md#d-119-how-a-gateway-install-moves-over-its-work-folder-a-half-done-openclaw-move-a-fresh-conversation)).

On its first start the core reads nothing stored in a chat by the second it subscribed to MDK as a prompt, so it
never runs an old prompt again.
A message sent to the agent while neither program ran, which the old connector never read, may or may not be
answered: the core takes it as a prompt only if it reaches the agent after the second the core subscribed to MDK
in; one received through that second is history
([D-111](decisions.md#d-111-a-group-the-core-has-never-read-starts-after-what-arrived-before-the-core-did)).
Keep the gap short, and ask again afterwards if no answer comes.

Up to MDK 0.19.0, a `wn-agent` stopped in the moment after a message reached it left the home deaf to that chat
for the core ([MDK #3602](https://github.com/marmot-protocol/mdk/issues/3602)). That is fixed in the MDK
revision the core uses: the import tests stop `wn-agent` at once after the person's last message, and each
passed 10 runs of 10.

After the move, the gateway starts a new conversation in each chat ([D-119](decisions.md#d-119-how-a-gateway-install-moves-over-its-work-folder-a-half-done-openclaw-move-a-fresh-conversation)): the new
plugin names a chat by the core's own id for it, not by the group, so what Hermes or OpenClaw kept of a chat's conversation stays with
the old one (seen with OpenClaw, and read in the Hermes plugin's code). The chat itself, and everything said in
it, is where it was.

## What carries over

| Today | In the settings |
| --- | --- |
| The account home and its one identity | `home`, unchanged |
| The relays on the old service's command line | `relays`, and `allow_loopback_relays` |
| The one person the old install let both invite and prompt the agent | `owner`; otherwise give `--owner` |
| The rest of the invite allow-list | `invite` |
| Who may write to the agent: a terminal harness's `WN_*_ALLOWED_SENDERS_HEX`, Hermes's `MARMOT_ALLOWED_USERS` or the `senderPolicy` of OpenClaw's account on this home | `prompt` |
| The folders `wn-agent` could send files from (`--media-allowed-root`) | `files_from`; none when it had none |
| The harness | `[codex]`, `[pi]`, `[acp]` for OpenCode and Goose, or `[connector]` |
| The harness's program, `WN_<HARNESS>_BIN`, as the installer found it | `program` in `[codex]` and `[pi]`, the first word of `command` in `[acp]`; not for Claude Code, whose driver runs the Claude Code that comes with it |

The rule is that nobody may do more afterwards than the old install let them:

- The settings are read as the old programs ran with them: a systemd unit as systemd itself has it loaded
  (asked through `busctl --user`, so every drop-in, continued line and `UnsetEnvironment=` counts), with the
  files its `EnvironmentFile=` lines name, read as systemd reads them (quotes inside a bare value kept, a
  value running on over a line's end inside quotes or after a backslash, `#` a comment only at a line's
  start, `export A=1` setting nothing), and each list parsed as the old program parsed it. A unit whose
  command line has `$` references, which systemd fills in only as it starts the service, one that names an
  environment file by a pattern or needs one that cannot be read, and one whose files changed since systemd
  read them, stop the import: make it plain, run `systemctl --user daemon-reload`, restart it, and import
  again. A list the old program
  would have refused, such as a harness list with an npub in it or an OpenClaw policy with one bad entry,
  allowed nobody, and so does the import.
- Where a list is kept in several places, such as the harness service and `dev/wn-<harness>.env`, only the
  people on all of them may prompt.
- Where what the old install allowed cannot be read for certain, such as a Hermes `config.yaml` with its own
  allow-list, or OpenClaw's list in the gateway's environment, only the owner may prompt. Where the old
  settings may have applied, the import names each, with its value as written, under "Only the owner may
  prompt now" and in its next steps: check them, and widen `prompt` in the settings by hand to keep one
  ([D-135](decisions.md#d-135-the-import-gives-owner-only-rights-where-it-cannot-tell-the-old-sender-policy-and-names-what-to-check)).
- For OpenClaw, the import reads the sender policy of the account MDK's plugin ran on this home. With
  `channels.marmot.accounts`, each named account runs on its own settings, and a `senderPolicy` at the
  channel's root applies to none of them; without, the root is the one account. An account ran on this home
  when its `socketPath` is the old connector's socket and its `accountIdHex`, if any, is the agent's, both
  written out: a socket or identity from a `${...}` reference, no `socketPath`, a relative one, a path that
  may lead to this home's socket, or an `$include` in the settings, means that account may have run on this
  home. If no account did, several did or may have, or the account has no `senderPolicy` of its own, only
  the owner may prompt, and the import names the policies of just those accounts, or the gateway's
  environment settings for one with no policy of its own.
- In a systemd environment file, a line that systemd has read otherwise by version (one after a comment
  ending in a backslash, or a backslash in double quotes before anything but `"`, `\` or the line's end), or
  one such as `export WN_CODEX_ALLOWED_SENDERS_HEX=…` that systemd leaves out, makes that name unsure. An
  unsure sender list lets only the owner prompt; an unsure relay list or other setting the import reads stops
  the import, as a doubtful unit does. So does an environment file holding a NUL byte, which systemd refuses
  whole.
- The owner may always prompt, approve and invite. So the import makes someone the owner only if the old
  install let exactly that one person both invite and prompt the agent; otherwise it asks for `--owner`, and
  says what the named owner may now do that they could not before. The same key given once in hex and once
  as an npub is one person.
- `files_from` lists only the old media folders, each found as `wn-agent` found it (`..` taken away before
  links are followed), and never one that is, holds or lies inside the home or its backup. The working folder
  is never allowed by default. The core checks `files_from` against the home again each time it starts, and
  refuses to start if a folder has come to overlap it, unless the owner has allowed files from the home
  ([D-099](decisions.md#d-099-files-from-the-agents-own-home-are-refused-by-default-and-the-owner-may-allow-them)).
  For Hermes and OpenClaw, `--replace-plugin` then lists the gateway's work folder in `files_from` too, after
  the old media folders, which stay, as a fresh install finds it and under the same rules
  ([a gateway's files](design/drivers.md#a-gateways-files)): none when the gateway works in the whole home, in a
  sandbox, in or around its own media or state, in or around the agent's home or the import's backup of it,
  or for more than one agent. It adds the folder only to the `files_from` line the import wrote, known by the
  import's comment directly above it, while that line is still a list of strings in one line, and says so.
  The comment then names the folder added: a run again changes nothing, or lists the gateway's new folder in
  its place if it has moved. A `files_from` line without either comment above it, or written otherwise, is
  the person's: it is left as it is, and the installer says to add the folder
  ([D-119](decisions.md#d-119-how-a-gateway-install-moves-over-its-work-folder-a-half-done-openclaw-move-a-fresh-conversation)).

Who may approve is the owner alone, the default
([D-010](decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)). The harness works in one
folder, your own unless you give `--cwd`, where the old harness let each chat pick one. The old harness's
execution profile, time limits and reply limits have no setting in the core. An invite policy that let anyone
invite is narrowed to the list, one that let anyone invite into a chat of two is narrowed to the owner, and one
that refused everyone leaves only the owner. The import lists each of these as it finds them.

A home with more than one identity is refused, since the core runs every identity in its home and each
identity is its own program ([D-072](decisions.md#d-072-one-agent-identity-per-core-process-until-the-swap)).

## What is proven

A test moves an install of each coding connector over: Codex, Claude Code, OpenCode, Pi and Goose
(`crates/marmot-agent/tests/import.rs` and `import_coding.rs`). For each, it:

- runs today's `wn-agent`, MDK's `agent-connector` at the revision the core uses, on a home laid out as MDK's
  `install-terminal-harness-marmot.sh` lays it out: the harness's env file in the home, and the two systemd
  units with their settings;
- has the old connector make its identity, a person invite it into a chat of two, and the person write to it,
  which it stores;
- stops it as systemd does, makes the plan and checks it: the connector, the owner, who may prompt and
  approve, the relays, and that what does not map is said; then carries it out, and checks the backup and the
  settings written for that harness;
- starts the core on those settings, and checks that the person's next message reaches the harness and is
  answered in the same chat, from the same key, and that nothing written before the move reached the harness.

Each harness is a stand-in that needs no model or account. For OpenCode, Goose and Pi the core is
`marmot-agent run` on the imported settings, with its own driver, on a stand-in where the installer found the
harness's program: an agent over ACP, or the Pi driver's own fake Pi. For Claude Code, the core serves its
socket, and a stand-in for the Claude Code driver connects to it as the driver does. For Codex, the core runs
in the test with a stand-in driver.

A second test moves an install of each gateway plugin over: Hermes and OpenClaw
(`crates/marmot-agent/tests/import_gateways.rs`). It needs Docker, so it runs only when asked, one scenario at a
time (`cargo test -p marmot-agent --test import_gateways -- --ignored --test-threads=1`). The real gateway runs
in the repository's image (`python/hermes`, `packages/openclaw/container`) with today's plugin, built from MDK's
sources as MDK's release builds it and set up as MDK's installer sets it up, and the person has a message
answered through it. Then the import, `install/hermes.sh` or `install/openclaw.sh` with `--replace-plugin`
against that same gateway, the core started by the launcher the installer writes, and the gateway restarted with
the new plugin. The person's next message is sent once and answered, and no request the gateway makes to its
model after the move holds anything said before it. The model is a scripted one the test serves on the loopback
address, and no systemd is touched: the import's service manager notes what it stops, and the installer runs with
`--no-service`.
Each scenario passed 5 runs of 5, with `wn-agent` stopped at once after the person's last message.

The installers' own tests (`install/test_install.py`, on stand-ins for Hermes and OpenClaw that keep their
settings in files as the real ones do) stop a `--replace-plugin` run at each of its steps in turn, each call
through which the script changes something, once by `TERM` and once by `KILL`. After each, the gateway has
the old plugin or the new one set up to start and the core's service enabled and started, but for the
moments above, in each of which the move is still noted as under way; Hermes's is tested with an `mv` that
cannot exchange folders, and without it where `mv` can. After a kill that leaves the move noted, the run
again finishes it, taking no copies of its own, and a run made to fail instead puts back what the gateway
had before. Once the move is finished, the copies `backups/before-move` lists are what the gateway had. A
move killed part way and given up with `--uninstall` is not put back by a later run that fails.

Not proven yet: a move on a real machine, with systemd and the real harnesses.

## Going back

First stop the core for good. The Hermes and OpenClaw installers run it as a service that is started again
whenever it stops (`Restart=always` on Linux, `KeepAlive` on a Mac), so stopping it is not enough: remove the
service with the installer, `install/hermes.sh --uninstall --name <name>` or the same with `openclaw.sh`
(`<name>` is the agent's name, `hermes` or `openclaw` by default). That also takes our plugin and its
settings out of the gateway, and gives up a move the installer left unfinished: it removes `plugin-change`
from the agent's folder, so that no later run puts the copies back over what the gateway has by then. If you
put the gateway back by hand without `--uninstall`, delete `plugin-change` yourself. If you started the core
yourself, with `marmot-agent run`, stop it.

Then put the gateway back as it was from the copies the installer kept before the move, in the agent's folder
(`~/.local/share/marmot-agent/agents/<name>/backups`). `backups/before-move` lists them, a line for each file
or folder the move changed: `file` or `folder`, where it is, and its copy, separated by tabs. Put each copy in
place of what its line names, a folder as a whole: for Hermes, `config.yaml`, `.env` and `plugins/marmot`; for
OpenClaw, its settings file and the old plugin's folder. A line with no copy names what the move added: remove
it. If an installer said it could not put them back itself, it named the copies and where they go.

Before the core has run, delete the settings file and start the old services again, as the import said when
it finished (`systemctl --user enable --now` each one it stopped, or on a Mac `launchctl enable` and
`launchctl bootstrap`). Once it has run, MDK has upgraded the store, which only goes forward: put the backup in place of the home, and start
the old services. Do that only if the agent has done nothing since, because an older store can reuse keys or
run a prompt twice. Then restart the gateway.
