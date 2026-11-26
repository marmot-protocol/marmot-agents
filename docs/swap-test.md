Status: a plan written 2026-11-26, not yet run. Steps marked **[D-158]** or **[D-159]** depend on changes still
being built and describe how those changes will behave; steps marked *(inferred)* were worked out, not read in
a source or seen to run.

# The swap test

The project owner runs this by hand, on a fresh Linux virtual machine. It installs today's MDK connector for every
harness, makes history with each through real White Noise chats, moves each over to `marmot-agent`, and checks
that nothing was lost. It is the last check of M4 in the [plan](plan.md#m4-ready-to-swap): an install of each of
today's connectors moved over, on a test machine, without being invited again.

How the move works is in [`move.md`](move.md). This page says what to do, in what order, and what passing looks
like.

## What to know first

Read these before anything else. The first three are also questions in [Before you start](#before-you-start).

- **Only Hermes and OpenClaw move over through an installer.** For Codex, Claude Code, OpenCode, Pi and Goose
  the move is `marmot-agent import`, then `marmot-agent run` by hand ([`move.md`](move.md#steps), step 3). The
  coding installers (`install/codex.sh` and the others) refuse an agent whose settings they did not write
  (`check_harness` in `install/lib.sh`). So after the move those five run in a terminal, not as a service.
- **The import has only been tested on homes made by `wn-agent` at the MDK revision the core uses** (`bea7b40`,
  of 2026-11-25). MDK's published connectors are `wn-agent-v0.19.0`, of 2026-11-22. A home made by 0.19.0 has
  never been moved over. This test is the first time.
- **White Noise may not show agent items.** The core sends reasoning, tools, plans and threads as kinds 1201 and
  1202 ([D-112](decisions.md#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)).
  An app shows them only once it is built on an MDK that has them ([`try.md`](try.md)). Several checks below
  need a build that does.
- **Today's terminal harnesses cannot ask.** They run one process per turn and close its input, so there is no
  approval or question before the move for Claude Code, Codex, OpenCode, Pi or Goose. Of today's connectors only
  Hermes asks for approval, as plain text answered with `/approve`
  ([`current-connectors.md`](reference/current-connectors.md#hermes-plugin)).
- **Today's coding connectors send no files**, except Codex with an opt-in export that needs a group id in its
  settings. The history step leaves that out.

## Before you start

The owner answers these. Each has a recommendation; the plan below follows it.

1. **The coding harnesses after the move: by hand, or an installer first?** Recommended: build a
   `--replace`-style path for the coding installers before the test, so all seven move as a person would move
   them and run as services. Until then, this plan runs them by hand in `tmux`, which tests the import and the
   core but not the service.
2. **Which `wn-agent` release?** Recommended: `wn-agent-v0.19.0`, the release MDK's integration guide names
   at the pinned commit, and what real installs run. A nightly (`wn-agent-v0.20.0-nightly.*`) is closer to what
   the import was tested on, and so proves less.
3. **Which White Noise build, and does it show agent items and threads?** Recommended: one built on MDK at or
   after 2026-11-22. Without it, the checks marked *(needs items)* can only be partly seen, and are reported
   as not seen.
4. **Who are the second and third people?** Two more White Noise accounts, on other devices. Called *B* and *C*
   below. Use test accounts, not real people.
5. **Can White Noise add a member to a direct chat?** If not, make the owner's two-person chat as a group of two,
   so [F4](#f4-a-third-person-joins-the-owners-chat) can add *C* to it. Decide before making any history.
6. **[D-159] How to make an upgrade fail after the new core opened the store.** Recommended: ask the D-159 change
   for a way. Until then, [F3](#f3-an-upgrade-that-fails-part-way) gives one *(inferred)*.
7. **Is the D-159 backup taken on a gateway's first move**, as well as on an upgrade of `marmot-agent`? The plan
   assumes yes.
8. **[D-159] Does a failed service start stop the core before the store is put back?** Such a failure leaves the
   core's service in place, and it may be running ([`move.md`](move.md#steps)); a store put back under a running
   core is not safe *(inferred)*. Recommended: the D-159 change stops and removes that service first.

## Setup

### The machine

- **Ubuntu 24.04 LTS server, x86_64.** CI runs its tests on 24.04 (`.github/workflows/ci.yml`), and a release,
  built on 22.04 (`.github/workflows/release.yml`), runs on it. Two things follow from it:
  - Codex runs commands in bubblewrap, which needs user namespaces that 24.04 keeps from programs. Run once:
    `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` (as CI does, `.github/actions/harnesses`).
  - Its GNU coreutils is 9.4, so Hermes's plugin swap goes the two-step way, with a moment of no plugin
    ([`move.md`](move.md#steps)). That is fine; it tests that path.
- **8 vCPUs, 16 GB of memory, 100 GB of disk**, to build the core on the machine and run seven harnesses at once
  *(inferred: MDK's build is large, and Rust's target folder grows past 20 GB)*.
- **Snapshots.** Take one after [setup](#setup), one after [making history](#making-history), and one before each
  move. A snapshot is the only way back after a move that worked ([Going back](#going-back)).
- One user account, the owner's, with `sudo`. Every service runs as that user. Keep services running when logged
  out: `loginctl enable-linger "$USER"`.
- A console to the machine that is not over its network (the hypervisor's), for [F2](#f2-the-network-stops).

Install:

```sh
sudo apt-get update
sudo apt-get install -y build-essential git curl tmux python3 python3-venv jq qrencode
# Node 24.22.1, the release this repository's containers use, from nodejs.org or nvm
# Rust, from https://rustup.rs (the pinned compiler installs itself on the first build)
```

### The harnesses

Install each as its own project says, at the release below, and sign it in. Check each works on its own in a
terminal before going on: a plain question, answered.

| Harness | Release | Why that one | Sign-in or key |
| --- | --- | --- | --- |
| Claude Code | the current release, at least 2.1.0; note which | MDK's connector needs 2.1.0 or newer. After the move the driver runs the Claude Code its SDK brings (`@anthropic-ai/claude-agent-sdk` 0.3.331), not this one | `claude`, then `/login`. An `ANTHROPIC_API_KEY` in the shell does not reach a service |
| Codex | 0.171.0 | What CI runs (`.github/actions/harnesses`) | `codex login` |
| OpenCode | 2.3.2 | What the ACP driver was checked on ([`harness-interfaces.md`](reference/harness-interfaces.md)) | a model provider, with `opencode auth login` |
| Pi | 1.6.0 | `PI_TESTED` in `install/pi.sh` | a model provider key, as Pi asks |
| Goose | 1.59.2 | `GOOSE_TESTED` in `install/goose.sh` | a provider, with `goose configure` |
| Hermes | 0.23.2 | `HERMES_TESTED` in `install/hermes.sh`; MDK's plugin needs 0.19.0 or newer | a model, with `hermes setup`; its gateway set up as a service, so `hermes gateway restart` works |
| OpenClaw | 2026.11.5 | the oldest our plugin takes (`packages/openclaw/package.json`); MDK's needs 2026.10.10 or newer | a model, with its own onboarding; its gateway set up as a service, so `openclaw gateway restart` works |
| Cursor | 2026.11.22 | what the ACP driver was checked on | `cursor-agent login` |

Each harness's model key is the owner's. None goes in this repository, the report or a chat.

Make a work folder for each coding harness, under the home, so that today's `/<path>` picker can reach it:

```sh
mkdir -p ~/swap-test/work/{claude,codex,opencode,pi,goose,cursor}
for h in claude codex opencode pi goose cursor; do mkdir -p ~/swap-test/work/$h/out; done
echo "a file the agent may send" > ~/swap-test/work/codex/out/report.txt   # and the same in each
echo "a file the agent must not send" > ~/swap-test/outside.txt
```

### The relays

White Noise's own: `wss://relay.eu.whitenoise.chat` and `wss://relay.us.whitenoise.chat`. They are the default
of MDK's installers and of ours ([D-100](decisions.md#d-100-the-installers-defaults)), so no `--relay` is given
anywhere. Check that the owner's, *B*'s and *C*'s White Noise list both.

### `marmot-agent`

There is no release yet: nothing has been tagged, and `.github/workflows/release.yml` runs only on a tag that is
the workspace's version (`v0.1.0` today). So build it on the machine from a checkout, at the `master` commit
that has both D-158 and D-159:

```sh
git clone https://github.com/marmot-protocol/marmot-agents.git ~/marmot-agents
cd ~/marmot-agents && git checkout <commit>
bash tools/odin/install.sh "$HOME/.local/share/marmot-odin"
export ODIN="$HOME/.local/share/marmot-odin/odin"
cargo build --locked --release -p marmot-agent
npm ci --prefix packages/claude-code
export CORE="$HOME/marmot-agents/target/release/marmot-agent"
```

Put the two `export` lines in `~/.bashrc`. The installers below take `--binary "$CORE"`, so nothing is built
twice.

Build it on the machine, not elsewhere: a build made on a newer distribution, such as Debian 13, will not run on
24.04's older C library *(inferred)*. That is why releases are built on 22.04.

To try the release's own layout too, package the build and use its install scripts for Cursor's fresh install
([M8](#m8-cursor-a-fresh-install)):

```sh
tools/release/package.sh "$CORE" x86_64-unknown-linux-gnu ~/swap-test/dist
mkdir -p ~/swap-test/release && tar -xzf ~/swap-test/dist/marmot-agent-x86_64-unknown-linux-gnu.tar.gz -C ~/swap-test/release
```

A real pre-release (a tag such as `v0.1.0-rc.1`) would first need the workspace's version changed to match. That
is the owner's call, and not needed for this test.

## Today's connectors

Install them the way MDK's guide says (`integrations/README.md` in MDK): each installer downloaded beside its
checksum, checked, then run. Define the guide's `install_verified` helper once (copy it from that page), then:

```sh
base_url="https://github.com/marmot-protocol/mdk/releases/download/wn-agent-v0.19.0"
OWNER_NPUB=<owner npub>
OWNER_HEX=$("$CORE" key "$OWNER_NPUB")
```

Install one at a time, and invite each from White Noise before the next.

| Harness | Command | Then |
| --- | --- | --- |
| Claude Code | `install_verified "$base_url/install-claude-marmot.sh" "$base_url/install-claude-marmot.sh.sha256" --yes --allow-welcomer "$OWNER_NPUB"` | |
| Codex | the same, with `codex` in place of `claude` in both addresses, and `--allow-welcomer <B npub>` added | *B* may invite and prompt Codex; see [why](#why-b-on-codex) |
| OpenCode | the same with `opencode` | its home is `~/.marmot-agents/harnesses` |
| Pi | the same with `pi` | |
| Goose | the same with `goose` | |
| Hermes | `install_verified "$base_url/install-hermes-marmot.sh" "$base_url/install-hermes-marmot.sh.sha256" --yes --allow-welcomer "$OWNER_NPUB" --allow-user "$OWNER_NPUB"` | `hermes gateway restart` |
| OpenClaw | `install_verified "$base_url/install-openclaw-marmot.sh" "$base_url/install-openclaw-marmot.sh.sha256" --yes --allow-welcomer "$OWNER_NPUB"` | set `channels.marmot.senderPolicy` to `{"allowedUsers": ["<owner hex>"], "allowAll": false}` in OpenClaw's settings, with the owner's key as `$OWNER_HEX` prints it, keeping the rest; then `openclaw gateway restart` |

What each one sets up (read in MDK at `bea7b40`, `scripts/install-*-marmot.sh`): a `wn-agent` binary and, for the
terminal harnesses, a `wn-<harness>` binary in `~/.local/bin`; an account home in `~/.marmot-agents/<name>`; a
systemd user service `wn-agent-<name>`; for a terminal harness a second service, `wn-<harness>`; for Hermes and
OpenClaw, MDK's plugin inside the gateway, sending files from `dev/outbound-media` in the home. Each prints the
agent's npub and keeps it in `bootstrap.json` in the home. [`current-connectors.md`](reference/current-connectors.md#where-an-install-keeps-what)
has the full table.

For each:

1. Compare the whole npub the installer printed with the contact you add in White Noise.
2. Check both services run: `systemctl --user status wn-agent-<name> wn-<harness>`.
3. Note the result in the [report](#reporting-results), row T.

#### Why *B* on Codex

On Codex only, *B* may invite and prompt, as two `--allow-welcomer` give. So the import finds two people who may do
both, cannot tell the owner, and asks for `--owner` ([`move.md`](move.md#what-carries-over)). After the move *B*
may still prompt Codex, and may not approve. Everywhere else only the owner may do anything, and *B* is only a
member of the group.

## Making history

For each harness, in its two chats: **the owner's two-person chat** with the agent, and **a group** of the owner,
*B* and the agent. Make the group in White Noise with *B* and the agent in it; on Codex, *B* could have made it.

Pick eight words nobody would type: `<W1>` to `<W8>`, the same for every harness. A reply that holds one of them
is an answer to that step's prompt. After the move, any new reply holding one means an old prompt was read again.

The terminal harnesses first need a folder: send `/swap-test/work/<harness>` as the first message in each chat.

| Step | Where | Send | Passing looks like |
| --- | --- | --- | --- |
| H1 | two-person chat | "Reply with the word `<W1>` and nothing else." | one reply with `<W1>` |
| H2 | two-person chat | "Make a file called `before.txt` in your folder holding one line, then say done." | the file is there; the agent says so |
| H3 | group | mentioning the agent: "Reply with the word `<W2>`." | one reply with `<W2>` |
| H4 | group | *B*, mentioning the agent: "Reply with the word `<W3>`." | Codex answers (*B* may prompt it); the others do not |
| H5 | two-person chat | Hermes only: "Run the shell command `rm -rf ~/swap-test/work/scratch` and tell me what happened." | Hermes asks first, as plain text; answer `/approve` |
| H6 | two-person chat | Hermes and OpenClaw only: "Send me a small text file holding the word done." | a file arrives in the chat |
| H7 | two-person chat | Codex only: "Use two subagents: one counts the files in `~/swap-test/work`, the other lists their names. Then report." | a final answer; today only Codex's final words reach the chat *(inferred)* |
| H8 | two-person chat | with the connector stopped, below | one reply with `<W4>`, once |

H8, catching up today. Stop the connector, write while it is down, start it:

```sh
systemctl --user stop wn-agent-<name> wn-<harness>    # Hermes and OpenClaw: wn-agent-<name> only
# from White Noise, in the two-person chat: "Reply with the word <W4>."
sleep 120
systemctl --user start wn-agent-<name> wn-<harness>
```

Questions: none of today's connectors can ask one (above). Record "not possible" for H-questions; questions are
tried after the move ([C11](#checks-after-each-move)).

Then take a snapshot of the machine.

## The move

### The order

Move one at a time, check it ([Checks after each move](#checks-after-each-move)), and leave it running before
moving the next.

1. **Codex** (M1). Built into the core, the most tested, and the one with subagents.
2. **OpenCode** (M2), **Goose** (M3), **Pi** (M4). Built in, simple.
3. **Claude Code** (M5). Over the socket, with its driver.
4. **Hermes** (M6). An installer and a plugin swap; the failure check [F3](#f3-an-upgrade-that-fails-part-way)
   runs here first.
5. **OpenClaw** (M7). The same, with OpenClaw's settings file.
6. **Cursor** (M8). A fresh install: it has no connector today.

Before each, take a snapshot, and send "Reply with the word `<W5>`." in the two-person chat *while the move is
under way* (after the import stopped the old services, before the core starts). That is the gap message, checked
in C4.

### M1: Codex

```sh
cd ~/marmot-agents
"$CORE" import ~/.marmot-agents/codex --owner "$OWNER_NPUB" --cwd ~/swap-test/work/codex --dry-run
```

Read the plan it prints. Passing: it says a Codex install; the owner is the one you gave; *B* may invite and
prompt, and only the owner may approve; two relays; the folder each chat chose with `/<path>` is not carried over;
`files_from` is empty. Then run it without `--dry-run`.

The import writes `~/.marmot-agents/codex/marmot-agent.toml`. Edit two lines in it for the test:

- `files_from = []` becomes `files_from = ["/home/<you>/swap-test/work/codex/out"]`, so C8 can send a file;
- under `[codex]`, add `approval_policy = "untrusted"`, so Codex asks before most commands, as
  [`try.md`](try.md) does.

Check the groups, with the agent stopped. This prints ids to the terminal only; do not copy them anywhere:

```sh
"$CORE" groups ~/.marmot-agents/codex/marmot-agent.toml
```

Passing: the two-person chat, marked as the owner's, and the group, and nothing else. Then start it in its own
`tmux` window:

```sh
tmux new-session -d -s codex "$CORE run ~/.marmot-agents/codex/marmot-agent.toml"
```

### M2 to M4: OpenCode, Goose and Pi

The same as Codex, without `--owner` and without *B*, each with its home and folder:

| Harness | Home | Before starting, in `marmot-agent.toml` | Start |
| --- | --- | --- | --- |
| OpenCode | `~/.marmot-agents/harnesses` | `files_from` as for Codex | `tmux new-session -d -s opencode "$CORE run ~/.marmot-agents/harnesses/marmot-agent.toml"` |
| Goose | `~/.marmot-agents/goose` | `files_from` as for Codex | `tmux new-session -d -s goose "GOOSE_MODE=approve $CORE run ~/.marmot-agents/goose/marmot-agent.toml"`, so Goose asks before each tool *(inferred: the core passes its environment to Goose, as the installer's `GOOSE_MODE` does)* |
| Pi | `~/.marmot-agents/pi` | `files_from` as for Codex | `tmux new-session -d -s pi "$CORE run ~/.marmot-agents/pi/marmot-agent.toml"` |

Pi asks nobody before a tool runs, so it has no approval to check.

### M5: Claude Code

Import as for OpenCode, with `~/.marmot-agents/claude` and `--cwd ~/swap-test/work/claude`. Then start the core
and, beside it, its driver, as the import says:

```sh
tmux new-session -d -s claude "$CORE run ~/.marmot-agents/claude/marmot-agent.toml"
tmux new-window -t claude "node ~/marmot-agents/packages/claude-code/src/main.ts --home ~/.marmot-agents/claude --cwd ~/swap-test/work/claude"
```

The driver's permission mode is `default`, which asks in the chat.

### M6: Hermes

Run [F3](#f3-an-upgrade-that-fails-part-way) first. Then:

```sh
mkdir -p ~/.local/share/marmot-agent/agents/hermes
"$CORE" import ~/.marmot-agents/hermes --settings ~/.local/share/marmot-agent/agents/hermes/settings.toml --dry-run
"$CORE" import ~/.marmot-agents/hermes --settings ~/.local/share/marmot-agent/agents/hermes/settings.toml
install/hermes.sh --binary "$CORE" --replace-plugin
hermes gateway restart
```

Passing: the import finds the owner by itself; the installer says it keeps the identity it was imported with,
keeps a copy of the old plugin and of Hermes's settings under `backups`, lists Hermes's work folder in
`files_from`, and starts `marmot-agent-hermes`. **[D-159]** It also says it backed up the account store before the
new core first opened it.

### M7: OpenClaw

The same, with `openclaw` for `hermes`, and `openclaw gateway restart`. Passing also: the installer takes the old
plugin's settings out of OpenClaw's (its socket, account and sender policy), and OpenClaw starts with the new
plugin.

### M8: Cursor, a fresh install

From the packaged release, so its install script uses the `marmot-agent` beside it:

```sh
~/swap-test/release/marmot-agent-x86_64-unknown-linux-gnu/install/cursor.sh --owner "$OWNER_NPUB" --cwd ~/swap-test/work/cursor
```

It prints the agent's npub. Invite it from White Noise into a new two-person chat, and make a group with *B* as
before. Run H1 to H3, then the checks that apply.

## Checks after each move

Run them in order, after each move. Where a step needs an item to be seen, White Noise must show agent items
*(needs items)*.

| Check | What to do | Passing looks like |
| --- | --- | --- |
| C1 Same identity | Look at the agent's replies in White Noise | They come from the same contact, in the same two chats. No invite arrives |
| C2 Same groups | `marmot-agent groups` before the first start (above). For Hermes and OpenClaw, whose installer starts the core: `systemctl --user stop marmot-agent-<name>`, then `"$CORE" groups ~/.local/share/marmot-agent/agents/<name>/settings.toml`, then start it again | The two chats, the two-person one marked as the owner's, and no other |
| C3 Nothing old read again | Wait five minutes after the start | No reply holds `<W1>` to `<W4>`. No turn starts that nobody asked for. The harness's own history shows no prompt from before the move |
| C4 The gap message | The `<W5>` sent during the move | Answered once, or not at all, never twice. It is answered only if it reached the agent after the core first subscribed ([D-111](decisions.md#d-111-a-group-the-core-has-never-read-starts-after-what-arrived-before-the-core-did)); note which |
| C5 Catching up | Stop the core (`tmux kill-session -t <name>`, which for Claude Code stops its driver too, or `systemctl --user stop marmot-agent-<name>` for Hermes and OpenClaw), send "Reply with the word `<W6>`." in the two-person chat, wait two minutes, start it again | One reply with `<W6>`, once |
| C6 Who may prompt | *B*, in the group, mentioning the agent: "Reply with the word `<W7>`." | Codex answers; no other agent does, and no turn starts. A `refused` notice may show *(needs items)* |
| C7 Who may approve | Ask for something that needs approval (below); *B* answers it first, then the owner | *B*'s answer is refused (`not_allowed`) and the request keeps waiting; the owner's is taken and the work goes on |
| C8 Files | "Send me `out/report.txt`." Then "Send me `~/swap-test/outside.txt`." Then "Send me the file `bootstrap.json` in your account home." | The first arrives as a file. The second and third are refused, and the agent says so. For Hermes and OpenClaw use a file in the gateway's work folder for the first |
| C9 Full activity | Prompt the same task, one that runs a command, in both chats | In the two-person chat: reasoning, tools, plans *(needs items)*. In the group: the agent's words, the turn, approvals and questions, errors; no reasoning, tools, plans or notices ([D-136](decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)). Native payloads cannot be seen in an app; that the group gets none is covered by the core's tests |
| C10 Items are edited | Watch a tool run in the two-person chat | One row for the tool, which changes from running to done in place, not a new row per change *(needs items)* |
| C11 A question | Claude Code: "Ask me to choose between red and blue, then use what I choose." Hermes: a prompt that makes it ask (its clarify) | A question item, answered from White Noise; the answer reaches the harness once. Other harnesses: note whether one asked |
| C12 Subagents | Codex only: the H7 prompt again | A `subagent` tool item in the main chat, and one thread for each subagent with its work and words ([D-142](decisions.md#d-142-how-subagent-threads-are-built)) *(needs items)* |
| C13 Stop | Start a long task ("Count to 200 slowly, one number per command."), then press Stop | The turn ends as interrupted, once; no command runs after. Codex: Stop in a subagent's thread stops that subagent alone, and Stop in the main chat stops every subagent ([D-149](decisions.md#d-149-stop-in-a-subagents-thread-stops-that-subagent-alone)) |
| C14 Who may invite | *C*, who is on no list, makes a new group with the agent in it. On Codex, *B* does the same | The agent never joins *C*'s group, and does not answer in it. Codex joins *B*'s, as the invite list carried over. Leave *B*'s group afterwards |

What needs approval: Codex, any command (with `approval_policy = "untrusted"`); Claude Code, a shell command;
Goose, any tool (with `GOOSE_MODE=approve`); OpenCode and Cursor, whatever they ask about *(inferred: they ask
for some tools, not all)*; Hermes, a command it confirms, such as H5's; OpenClaw, a command its own approval
settings ask about. Pi never asks.

## Failure checks

### F1: Kill the core mid-turn

On Codex and on Hermes. Start a long task in the two-person chat, and once a command is running:

```sh
pkill -KILL -f 'marmot-agent run .*/codex/'      # Codex, run by hand; then start it again as in M1
systemctl --user kill -s KILL marmot-agent-hermes  # Hermes; systemd starts it again
```

Passing: once it is back, the turn shows as interrupted, its open items are closed, and an open approval is settled
as failed ([D-070](decisions.md#d-070-a-restart-closes-the-turn-as-interrupted)). Nothing the agent sent before is
sent again, no command runs twice, and the next prompt is answered.

### F2: The network stops

Once all eight agents run. From the hypervisor's console, disconnect the machine's network for five minutes. While
it is down, send one prompt to each agent's two-person chat. Then connect it again.

Passing: no core needs restarting. Each prompt is answered once after the network is back. A turn that was running
fails with an error shown, or finishes; nothing shows twice. A send that could not go out is either sent once
afterwards or shown as perhaps not delivered.

### F3: An upgrade that fails part-way

On Hermes, before its real move. Take a snapshot first.

**F3a: a failure before the core starts.** Run the import as in M6, then the installer, and press Ctrl-C while
it says it is preparing, before it says it is starting the service.

Passing: the installer says it put back Hermes's settings and the old plugin. Then go back as
[`move.md`](move.md#going-back) says for a core that never ran: delete the
settings the import wrote, `~/.local/share/marmot-agent/agents/hermes/settings.toml`, start the old service (`systemctl --user enable --now wn-agent-hermes`), and restart Hermes. The old connector answers
"Reply with the word `<W8>`." in the two-person chat, from the same contact.

**F3b: a failure after the new core opened the store. [D-159]** Make the installer's service start fail after the
service really started *(inferred: this fakes `systemctl restart` failing; use the D-159 change's own way if it
has one)*:

```sh
mkdir -p ~/swap-test/failing-bin
cat > ~/swap-test/failing-bin/systemctl <<'EOF'
#!/bin/sh
/usr/bin/systemctl "$@"; status=$?
case " $* " in *" restart "*) sleep 30; exit 1 ;; esac
exit $status
EOF
chmod +x ~/swap-test/failing-bin/systemctl
# the import as in M6, then:
PATH="$HOME/swap-test/failing-bin:$PATH" install/hermes.sh --binary "$CORE" --replace-plugin
```

Passing, with D-159: the installer says the start failed, and that it put back the plugin, Hermes's settings and
the account store as it was before the new core opened it. A failed start leaves the core's service in place, and
it may be running ([`move.md`](move.md#steps)): remove it at once (`install/hermes.sh --uninstall`), then go back
as in F3a. The old connector opens the home and answers `<W8>`.
Without D-159, the old connector cannot open the home, and only the import's backup brings it back
([Going back](#going-back)).

Then move Hermes for real, as M6, from the snapshot or from where F3 left it.

### F4: A third person joins the owner's chat

On Codex and on Claude Code. In the owner's two-person chat, start a long task that runs commands (on Codex, the
H7 subagent prompt). While it works, add *C* to that chat from White Noise. Watch what *C* sees.

Passing:

- From the moment *C* joins, the chat is no longer the owner's: *C* sees the agent's words, the turn, approvals,
  questions and errors, and no reasoning, tools, plans or notices
  ([D-136](decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)).
- **[D-158]** Nothing of the full activity reaches *C*, even what the core handed to MDK just before *C* joined:
  MDK refuses such a send because someone joined, and the core holds it back. Without D-158, such a send may
  reach *C* ([D-148](decisions.md#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit)).
- **Known gap, not a failure:** on Codex, a subagent's words in its thread may reach *C*. MDK has no way yet to
  limit a thread message to two people. Note whether they did.
- The turn goes on and finishes; the owner's approvals still work.

Then remove *C*. The chat is the owner's again from the next send: full activity comes back *(inferred: D-136
reads the members before each send)*.

## Going back

- **Before a core has opened the home**, going back is easy: delete the settings the import wrote and start the
  old services again, as the import said when it finished ([`move.md`](move.md#going-back)).
- **[D-159] If an installer fails after the new core opened the home**, it puts the account store back with the
  old core or plugin, and the old connector can start again (F3b).
- **After a move that worked, there is no going back.** The first time the core opens the home, MDK upgrades the
  store, and MDK's migration 0226 at the current pin keeps older builds from opening it. The import's backup
  (`~/.marmot-agents/<name>.backup-<time>`) can be put in place of the home, but only safely if the agent has
  done nothing since: an older store can reuse keys or run a prompt twice. On this machine, go back by restoring
  a snapshot instead.

## Reporting results

Fill in one table, a column per harness. Write `pass`, `fail`, `not seen` (White Noise could not show it), or
`n/a`.

| | Claude Code | Codex | OpenCode | Pi | Goose | Hermes | OpenClaw | Cursor |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| T installed today's connector | | | | | | | | n/a |
| H1 to H8 history | | | | | | | | n/a |
| Import dry run as expected | | | | | | | | n/a |
| Move | | | | | | | | fresh |
| C1 same identity | | | | | | | | n/a |
| C2 same groups | | | | | | | | n/a |
| C3 nothing old read again | | | | | | | | n/a |
| C4 gap message (once / none) | | | | | | | | n/a |
| C5 catching up | | | | | | | | |
| C6 who may prompt | | | | | | | | |
| C7 who may approve | | | | n/a | | | | |
| C8 files | | | | | | | | |
| C9 full activity | | | | | | | | |
| C10 items edited | | | | | | | | |
| C11 a question | | | | | | | | |
| C12 subagents | n/a | | n/a | n/a | n/a | n/a | n/a | n/a |
| C13 Stop | | | | | | | | |
| C14 who may invite | | | | | | | | |
| F1 kill mid-turn | n/a | | n/a | n/a | n/a | | n/a | n/a |
| F2 network | | | | | | | | |
| F3a / F3b failed upgrade | n/a | n/a | n/a | n/a | n/a | | n/a | n/a |
| F4 third person | | | n/a | n/a | n/a | n/a | n/a | n/a |

Also write down: the `marmot-agents` commit, the White Noise build, and each harness's release.

When something fails, collect:

- the step, what you expected, and what you saw, in your own words;
- the time, in UTC, to the minute;
- the core's log around that time: `journalctl --user -u marmot-agent-<name> --since "<time>"` for a service, or
  the `tmux` window's output. The core's logs hold no message text, keys, ids, relay addresses or paths
  ([D-071](decisions.md#d-071-what-may-be-logged)), but read them before sharing;
- the state of the services: `systemctl --user list-units 'wn-*' 'marmot-agent-*'`;
- for an installer or the import, the lines it printed that say what failed.

Do not collect, and never paste into an issue:

- keys, npubs, hex public keys, group or account ids, event ids, or relay addresses. The import's own output
  names old sender settings with their values (and those can be keys), and `marmot-agent groups` prints group ids:
  replace each with `<key>` or `<group>`;
- message text, prompts or model output, beyond the marker words `<W1>` to `<W8>`;
- the logs of `wn-agent`, the harnesses, Hermes or OpenClaw, which can hold all of these: read them yourself,
  and say what they showed in words;
- anything from an account home, `bootstrap.json` or the import's backup;
- your user name in a path: write `~` instead.

## On macOS, later

The same plan, with these changes:

- Apple silicon only. Our core has no build for Intel Macs, though MDK's connectors do, so an Intel install
  cannot move over.
- Services are LaunchAgents: today's `org.marmot.wn-agent.<name>` and `org.marmot.wn-<harness>`, ours
  `org.marmot.marmot-agent.<name>`. The import stops today's with `launchctl` and keeps their files. Use
  `launchctl kickstart -k` and `launchctl bootout` for starting and stopping, and `launchctl kill KILL` for F1
  *(inferred)*.
- Claude Code's sign-in is in the Keychain, which the installer checks *(inferred from Claude Code's
  documentation, not checked on a Mac)*.
- Hermes's plugin swap is always the two-step way: macOS's `mv` cannot exchange two folders.
- F2: turn Wi-Fi off. No snapshots: back up the account homes with Time Machine or a copy before each move.
- Build with Xcode's command-line tools instead of `build-essential`.
