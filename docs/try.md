# Trying the core

Status: describes what is built today: Codex built in, and Claude Code through the socket (M1 and M2 of the
[plan](plan.md)). Agent items travel as kinds 1201 and 1202
([D-112](decisions.md#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)), which
MDK carries since 2026-11-22; a White Noise app shows them only once it is built on that MDK. A terminal client
plays the person instead.

Everything runs on one machine, with nothing else needed but a signed-in `codex`. To install an agent as a
service of your account instead, use a script in `install/`, as the [README](../README.md#installing-an-agent)
says. Building works on Linux, on x86_64 or arm64, and on macOS with Apple silicon, and needs the pinned Odin
compiler that MDK uses: install it once with `bash tools/odin/install.sh <absolute folder>` and set `ODIN` to
the `odin` program in that folder, as "Verification" in [`AGENTS.md`](../AGENTS.md) says.

## Steps

1. Build both programs, and make a folder for Codex to work in:

   ```sh
   cargo build -p marmot-agent -p marmot-agent-chat
   mkdir -p /tmp/try/work
   ```

2. Start the client. It runs a relay on port 7777, makes your account in the folder you give, and prints your
   key. Leave it running.

   ```sh
   target/debug/marmot-agent-chat /tmp/try/person --serve 7777
   ```

3. Write the agent's settings, with your key as the owner:

   ```toml
   home = "/tmp/try/agent"
   name = "Codex"                  # what you will see it called
   relays = ["ws://127.0.0.1:7777"]
   owner = "<your key>"
   white_noise = false             # only the relay above
   allow_loopback_relays = true

   [codex]
   cwd = "/tmp/try/work"           # where Codex works
   approval_policy = "untrusted"   # so Codex asks before most commands
   ```

4. Make the agent's identity, which prints its key, and run it:

   ```sh
   target/debug/marmot-agent init agent.toml
   target/debug/marmot-agent run agent.toml
   ```

5. In the client, invite the agent and talk to it:

   ```text
   :invite <the agent's key>
   Run `touch hello.txt` in the shell.
   ```

Each of the agent's items shows on one line, again each time it changes, starting with its chat's label, which
names the group and the agent in it. When it asks, type `:allow` or `:deny` when one choice allows or denies, or
`:choose` one of the choices shown; answer a question with `:answer <text>` or `:pick` an option, or, when it says so,
with a plain message. In a group, the agent hears only a message that mentions it, so start that one with the
`nostr:` address the client shows; a secret is typed only in a chat of you and the agent alone, and otherwise in the
harness's terminal. The shortcuts answer in the chat you talk in; an item from another chat says how it is answered
there, and `:chat <label>` switches to that chat and shows again what waits in it. `:stop` interrupts a turn, `:new`
starts a new session, and `:set <setting> <value>` changes one of the session's settings. A message that starts
with a slash goes to Codex as typed. The client has no threads: a Codex subagent's words, which an app shows in the
subagent's thread, are shown in line, as `<label> <agent>, in the thread of <its task>: <words>`.

Your two-person chat with the agent is sent everything it does. A group is sent its words, its turns, what it
asks and its errors, and its reasoning, tools, plans and notices only if its settings list the group under
`full_activity` ([D-136](decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)).
To find a group's id, stop the agent and print the groups it is in, each with its id, its name, whether it is your
two-person chat and whether `full_activity` lists it:

```sh
target/debug/marmot-agent groups agent.toml
```

The ids are printed to the terminal and nowhere else. Then add `full_activity = ["<the group's id>"]` to the
settings, above the `[codex]` section, and run the agent again.

The relay keeps nothing when the client stops, so start again from step 2 with fresh folders.

The agent can send a file from its working folder with its `send_file` tool (Codex, Claude Code and Pi), but
never one from its own home,
`/tmp/try/agent`, which holds its keys, even if the working folder held it, unless the settings say
`send_files_from_agent_home = true`
([D-099](decisions.md#d-099-files-from-the-agents-own-home-are-refused-by-default-and-the-owner-may-allow-them)). The encrypted file goes to the
Blossom servers the group names, which the app that made the group chose. Sending one has not been tried by
hand with this client; if the group names none, the chat shows the file refused as failed.

## With Claude Code

The same steps work with Claude Code in place of Codex, through the socket. It also needs Node 24.

1. In the agent's settings, replace the `[codex]` section with an empty `[connector]` section. The driver tells
   the core the folder it works in, so the agent sends files from there, and a chat may move it to a folder
   inside it (D-101).
2. Install the driver's packages once: `npm ci` in `packages/claude-code`.
3. Run `marmot-agent run agent.toml` as before. It now waits for the driver.
4. Start the driver, giving it the agent's home and the folder to work in:

   ```sh
   node packages/claude-code/src/main.ts --home /tmp/try/agent --cwd /tmp/try/work --permission-mode default
   ```

Then invite the agent and talk to it in the client as before.

## With an installed agent

The scripts in `install/` set an agent up with the harness you already have
([D-102](decisions.md#d-102-the-installers-connect-to-the-harness-the-person-already-has)). To try one with the
client in place of White Noise, give it the client's relay and no service, and run its `start` script. There
is no release yet, so give the script the core built in step 1:

```sh
target/debug/marmot-agent-chat /tmp/try/person --serve 7777     # as in step 2; note your key
install/opencode.sh --owner <your key> --local-relay ws://127.0.0.1:7777 --cwd /tmp/try/work \
    --binary target/debug/marmot-agent --no-service
~/.local/share/marmot-agent/agents/opencode/start
```

Then `:invite` the key the script printed, as in step 5. For Hermes or OpenClaw, the script adds the plugin to
your own gateway: start or restart the gateway once the core runs. `--uninstall` takes the service, and the
plugin, out again; the agent's folder stays until you remove it.

## Moving an existing install

To move one of today's connector installs over, keeping its identity and its chats, see [`move.md`](move.md).
