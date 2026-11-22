# Moving an install over

Status: describes `marmot-agent import` as built
([D-089](decisions.md#d-089-an-install-moves-over-by-importing-its-account-home-in-place)). How the old
allow-lists become the new settings, who becomes the owner unasked, `files_from` (and the core refusing to
start when one overlaps the home), `--leave-running`, and what the import does with a home that holds several
identities, are proposals until the project owner accepts them.

One of today's connectors (installed by MDK's `install-*-marmot.sh`) moves to the core without anyone inviting
the agent again. The core uses the old account home where it is, so the agent keeps its identity, its keys, its
encryption state and its chats.

## Steps

1. See what the import would do. It reads the home, its services and its allow-lists, and changes nothing:

   ```sh
   marmot-agent import ~/.marmot-agents/codex --dry-run
   ```

   It says which connector the home belongs to, what it carries over, and, plainly, what it does not.

2. Run it:

   ```sh
   marmot-agent import ~/.marmot-agents/codex
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
   ~/.marmot-agents/codex/marmot-agent.toml`. For Claude Code, start its driver too; for Hermes and OpenClaw,
   put the new plugin in the gateway in place of the old one and restart the gateway.

On its first start the core reads nothing already in a chat as a prompt, so it never runs an old prompt again.
That also means a message sent to the agent between the import and that first start is not answered: keep the
gap short, or ask again afterwards.

Known problem, in MDK at the revision the core uses: when `wn-agent` is stopped in the moment after a message
reaches it, the core afterwards does not hear that chat. The import test stops `wn-agent` five seconds after
the last message for that reason. Do not import while people are writing to the agent.

## What carries over

| Today | In the settings |
| --- | --- |
| The account home and its one identity | `home`, unchanged |
| The relays on the old service's command line | `relays`, and `allow_loopback_relays` |
| The one person the old install let both invite and prompt the agent | `owner`; otherwise give `--owner` |
| The rest of the invite allow-list | `invite` |
| Who may write to the agent: a terminal harness's `WN_*_ALLOWED_SENDERS_HEX`, Hermes's `MARMOT_ALLOWED_USERS` or OpenClaw's `senderPolicy` | `prompt` |
| The folders `wn-agent` could send files from (`--media-allowed-root`) | `files_from`; none when it had none |
| The harness | `[codex]`, `[pi]`, `[acp]` for OpenCode and Goose, or `[connector]` |

The rule is that nobody may do more afterwards than the old install let them:

- The settings are read as the old programs ran with them: a systemd unit as systemd itself has it loaded
  (asked through `busctl --user`, so every drop-in, continued line and `UnsetEnvironment=` counts), with the
  files its `EnvironmentFile=` lines name, and each list parsed as the old program parsed it. A unit whose
  command line has `$` references, which systemd fills in only as it starts the service, one that names an
  environment file by a pattern or needs one that cannot be read, and one whose files changed since systemd
  read them, stop the import: make it plain, run `systemctl --user daemon-reload`, restart it, and import
  again. A list the old program
  would have refused, such as a harness list with an npub in it or an OpenClaw policy with one bad entry,
  allowed nobody, and so does the import.
- Where a list is kept in several places, such as the harness service and `dev/wn-<harness>.env`, only the
  people on all of them may prompt.
- Where what the old install allowed cannot be read for certain, such as a Hermes `config.yaml` with its own
  allow-list, or OpenClaw's list in the gateway's environment, only the owner may prompt.
- The owner may always prompt, approve and invite. So the import makes someone the owner only if the old
  install let exactly that one person both invite and prompt the agent; otherwise it asks for `--owner`, and
  says what the named owner may now do that they could not before. The same key given once in hex and once
  as an npub is one person.
- `files_from` lists only the old media folders, each found as `wn-agent` found it (`..` taken away before
  links are followed), and never one that is, holds or lies inside the home or its backup. The working folder
  is never allowed by default. The core checks `files_from` against the home again each time it starts, and
  refuses to start if a folder has come to overlap it, unless the owner has allowed files from the home
  ([D-099](decisions.md#d-099-files-from-the-agents-own-home-are-refused-by-default-and-the-owner-may-allow-them)).

Who may approve is the owner alone, the default
([D-010](decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)). The harness works in one
folder, your own unless you give `--cwd`, where the old harness let each chat pick one. The old harness's
execution profile, time limits and reply limits have no setting in the core. An invite policy that let anyone
invite is narrowed to the list, one that let anyone invite into a chat of two is narrowed to the owner, and one
that refused everyone leaves only the owner. The import lists each of these as it finds them.

A home with more than one identity is refused, since the core runs every identity in its home and each
identity is its own program ([D-072](decisions.md#d-072-one-agent-identity-per-core-process-until-the-swap)).

## Going back

Before the core has run, delete the settings file and start the old services again. Once it has run, MDK has
upgraded the store, which only goes forward: stop the core, put the backup in place of the home, and start the
old services. Do that only if the agent has done nothing since, because an older store can reuse keys or run a
prompt twice.
