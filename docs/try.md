# Trying the core

Status: describes what is built today: Codex built in, and Claude Code through the socket (M1 and M2 of the
[plan](plan.md)). Agent items travel under a temporary kind
until MDK takes the new shape ([D-069](decisions.md#d-069-until-mdk-takes-the-new-shape-agent-items-travel-under-a-temporary-kind)),
so no White Noise app shows them yet. A terminal client plays the person instead.

Everything runs on one machine, with nothing else needed but a signed-in `codex`. Building needs the pinned Odin
compiler that MDK uses: see "Verification" in [`AGENTS.md`](../AGENTS.md).

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

Each of the agent's items shows on one line, again each time it changes. When it asks, type `:allow` (once)
or `:deny`, or `:choose` one of the choices shown; answer a question with `:answer <text>` or `:pick` an option. `:stop` interrupts a turn, `:new` starts a new session, and `:set <setting> <value>` changes one of
the session's settings. A message that starts with a slash goes to Codex as typed.

The relay keeps nothing when the client stops, so start again from step 2 with fresh folders.

## With Claude Code

The same steps work with Claude Code in place of Codex, through the socket. It also needs Node 24.

1. In the agent's settings, replace the `[codex]` section with an empty `[connector]` section.
2. Install the driver's packages once: `npm ci` in `packages/claude-code`.
3. Run `marmot-agent run agent.toml` as before. It now waits for the driver.
4. Start the driver, giving it the agent's home and the folder to work in:

   ```sh
   node packages/claude-code/src/main.ts --home /tmp/try/agent --cwd /tmp/try/work --permission-mode default
   ```

Then invite the agent and talk to it in the client as before.
