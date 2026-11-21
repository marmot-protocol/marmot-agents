# Drivers

Status: draft. Which driver each harness gets is decided
([D-005](../decisions.md#d-005-which-driver-each-harness-gets)), and so is the preference for official
libraries ([D-006](../decisions.md#d-006-prefer-official-interfaces-and-libraries)), and so are the trait and
the capability flags ([D-058](../decisions.md#d-058-the-driver-trait-and-the-capability-flags)), and so is how
the Codex driver splits its work ([D-060](../decisions.md#d-060-a-codex-translator-follows-one-thread-and-the-transport-routes)).
So is how drivers are tested
([D-074](../decisions.md#d-074-every-driver-is-tested-by-replay-and-by-one-end-to-end-scenario)).

## What a driver does

A driver connects one harness to the core. It does two things:

1. **It speaks the harness's own interface.** It starts the harness, holds a two-way session open, and tracks
   the running turn and any requests the harness is waiting on.
2. **It translates.** Native events become [agent events](event-model.md). Answers and controls from the chat
   become native calls.

It does not decide who is allowed to do anything, and it does not touch Marmot. See
[`architecture.md`](architecture.md).

## The drivers

| Harness | Driver | Language | Interface it speaks | Builds on |
| --- | --- | --- | --- | --- |
| Codex | Native | Rust | `codex app-server`, JSON-RPC over stdio | Types of its own, with Codex's schema as the reference ([D-055](../decisions.md#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own)) |
| Pi | Native | Rust | `pi --mode rpc`, JSON lines over stdio | `pi-codes` |
| Goose | ACP, with a small Goose flavour | Rust | `goose acp` | `agent-client-protocol` |
| OpenCode | ACP | Rust | `opencode acp` | `agent-client-protocol` |
| Other ACP agents | ACP | Rust | Each agent's ACP command | `agent-client-protocol` |
| Claude Code | Native | TypeScript | The Claude Agent SDK | `@anthropic-ai/claude-agent-sdk` |
| Cursor | ACP | Rust | `cursor-agent acp` | `agent-client-protocol` ([D-080](../decisions.md#d-080-cursor-is-driven-through-acp)) |
| Hermes | Plugin | Python | Hermes's platform plugin interface | Hermes itself |
| OpenClaw | Plugin | TypeScript | OpenClaw's channel plugin interface | OpenClaw itself |

The facts behind each choice are in [`../reference/harness-interfaces.md`](../reference/harness-interfaces.md).

### Why some are native and some use ACP

The Agent Client Protocol (ACP) is an open standard that many harnesses speak. One ACP driver covers all of
them, so a new harness can be a line of configuration.

ACP's standard part covers what harnesses have in common. What makes a harness distinctive mostly travels in
harness-specific extensions, or is missing. So:

- **A harness gets a native driver** when it matters enough that losing features is not acceptable, or when its
  ACP support is poor. That is Codex, Claude Code and Pi.
- **Everything else uses the ACP driver.** Where a harness needs something the standard lacks, the ACP driver
  gets a small "flavour" for it, as Goose does for steering.

Because all drivers sit behind the same trait, a harness can move from ACP to a native driver later without any
change above the driver.

### Why Hermes and OpenClaw are plugins

Hermes and OpenClaw are not coding tools driven by one person. They are agent runtimes with their own idea of
users, channels and routing. ACP models one editor user per session, so it has no place for who sent a message
or which group it came from. Their own plugin interfaces do. Each plugin runs inside its runtime and connects to
the core over [the socket](socket-protocol.md).

### What Hermes can and cannot report

Recordings show that Hermes hands a platform formatted text, not tool events: a line such as "Running a
command" arrives through the same call as a reply. Only approvals and questions arrive in a structured form. So
the Hermes connector can report messages, notices, approvals and questions, and cannot report tool calls as
items without a change on the Hermes side. See
[`../reference/hermes-messages.md`](../reference/hermes-messages.md).

## The driver trait

Every Rust driver implements one trait. A first version exists as code, in
[`crates/marmot-agent-driver`](../../crates/marmot-agent-driver). It has the operations of
[the socket's driver section](socket-protocol.md#the-driver-section) and no others, because a program on the
socket does the same job as messages:

- say what the harness is, and what it can do (capabilities);
- take a prompt;
- take an answer to a waiting request;
- take a control: interrupt, new session, change a setting, or one of the harness's own actions;
- hand back the next agent event.

Three things a driver does are not operations of their own. A prompt goes to its chat's session, and names
one only to resume it, so opening and resuming one happen when a prompt arrives
([D-065](../decisions.md#d-065-a-prompt-names-a-session-only-to-resume-one)). A message sent during a turn is a prompt like any other, so there
is no "steer". And "new session" is a control.

It is decided ([D-058](../decisions.md#d-058-the-driver-trait-and-the-capability-flags)). The core waits for
every request to return
([D-066](../decisions.md#d-066-the-core-finishes-every-request-it-starts)).

## Capabilities

Each driver declares what its harness can do: interrupt a turn, what it does with a message sent during one,
resume a session, take attachments, and so on. The core and the clients read these flags and never check a
harness's name. A driver states every flag; none is optional.

The flags, with what Codex states for each, are in
[D-058](../decisions.md#d-058-the-driver-trait-and-the-capability-flags).

## Testing with recorded sessions

Record a real session with a harness: every message in both directions. Replay it through the driver in a test
and check the agent events that come out. When a harness changes its interface, a new recording shows the
difference.

Hand-written test messages are a guess about what the harness sends. Recordings are what it sends.

The Codex driver is tested this way: each of its recordings must give the events the prototype gives
([`crates/marmot-agent-codex/tests/replay.rs`](../../crates/marmot-agent-codex/tests/replay.rs)). Every driver
is tested so, and by one end-to-end scenario
([D-074](../decisions.md#d-074-every-driver-is-tested-by-replay-and-by-one-end-to-end-scenario)).

## The Codex driver

It has two halves: a translator, which turns the messages of one chat's Codex session into agent events and
builds what the driver sends, and a transport, which runs `codex app-server` and gives each translator its own
thread's messages ([D-060](../decisions.md#d-060-a-codex-translator-follows-one-thread-and-the-transport-routes)).
A session starts in a working folder with a narrow sandbox unless configured otherwise
([D-061](../decisions.md#d-061-a-codex-session-starts-in-a-working-folder-with-a-safe-sandbox-unless-configured)),
Codex reads a group message with its sender's name
([D-062](../decisions.md#d-062-what-codex-reads-for-a-chat-message)), Codex keeps each session so it can be
resumed ([D-063](../decisions.md#d-063-codex-keeps-a-session-so-it-can-be-resumed)), and a form from an MCP
server is declined for now ([D-064](../decisions.md#d-064-the-codex-driver-declines-an-mcp-servers-form-until-forms-are-designed)). Both halves are written
([`crates/marmot-agent-codex`](../../crates/marmot-agent-codex)). The transport runs one Codex for every chat of an
agent, and starts it with subagents switched off until how a chat shows them is decided
([subagents as threads](subagents.md)).

## The Claude Code driver

A separate TypeScript program, [`packages/claude-code`](../../packages/claude-code), that runs Claude Code
through the official Claude Agent SDK (pinned exactly, at 0.3.326, which runs Claude Code 2.1.326) and connects
to the core over the [socket](socket-protocol.md#the-messages-as-built). The core is started with a
`[connector]` section in place of `[codex]`; then:

```sh
cd packages/claude-code
npm ci
node src/main.ts --home <agent home> --cwd <project folder> [--model <model>] [--permission-mode default]
```

It finds the socket and its token in the agent's home, and exits when the core closes the socket. It loads no
Claude Code settings files (the SDK's `settingSources` is empty,
[D-082](../decisions.md#d-082-the-claude-code-driver-loads-no-claude-code-settings-files)), so no allow rule or hook in the owner's own
Claude Code settings approves or runs anything in a chat; a side effect is that it reads no `CLAUDE.md` either.
It has the
same two halves as the Codex driver. The translator (`src/translate.ts`) turns one chat's Claude Code messages
into agent events and is tested against the recordings. The transport (`src/connector.ts`) runs one SDK
session per chat:

- A prompt starts the chat's session if none is running, resuming the one the core names
  ([D-065](../decisions.md#d-065-a-prompt-names-a-session-only-to-resume-one)), and goes in as a user message,
  written as for Codex ([D-062](../decisions.md#d-062-what-codex-reads-for-a-chat-message)). The next turn
  names the prompts sent since the last one. Only a fresh session takes `--model` and `--permission-mode`; a
  resumed one keeps the model and permission mode it last showed. If Claude Code stops in the middle of a
  turn, the turn ends cancelled and the next prompt resumes the session.
- An approval waits in Claude Code's `canUseTool` callback until the answer comes. "Allow once" allows the
  call as asked, a standing choice allows it with the rule Claude Code suggested, and the last choice denies.
  A question from `AskUserQuestion` is answered the same way.
- Interrupt calls `interrupt()`. New session closes the chat's SDK session, and the next prompt starts a
  fresh one. `set` changes the permission mode or the model.
- The event that shows an answer's or a control's effect names it
  ([D-028](../decisions.md#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)): the
  approval's next edit, the end of the interrupted turn, the session item's edit that shows the new setting
  (sent as soon as Claude Code takes it), or the end of the old session.
- An event too large for a line of the socket (4 MB) goes without its native message, and if it is still too
  large, with its longest texts cut in the middle until it fits. Its `prompts` and `settles` are kept.

Its capabilities: it can interrupt; a message sent during a turn ends the turn and starts another (`restart`,
[D-040](../decisions.md#d-040-a-message-sent-during-a-turn-goes-straight-to-the-harness)); the core names the
sessions, and they can be resumed; approvals come to the chat; no approval commands. It takes no files yet
(`files_in` is `none`, [D-083](../decisions.md#d-083-the-core-passes-no-files-to-a-harness-yet)): Claude Code reads images, and
passing them on is a later step.

## The ACP driver

Built into the core, in [`crates/marmot-agent-acp`](../../crates/marmot-agent-acp), with the same two halves as
the Codex driver. It reads ACP with the official types (`agent-client-protocol-schema`). The translator turns one
chat's ACP session into agent events and is tested against the OpenCode, Goose and Cursor recordings; the
transport runs one agent process for every chat of an agent, routes each message to the chat whose session it
names, and is tested against a stand-in agent. What each agent sends is in
[`reference/harness-messages.md`](../reference/harness-messages.md#acp-opencode).

The core is started with an `[acp]` section in place of `[codex]`:

```toml
[acp]
name = "opencode"                 # what events call the agent until it names itself; Cursor never does
command = ["opencode", "acp"]     # or ["cursor-agent", "acp"], or Goose's container, below
cwd = "/home/me/work"
```

- **Goose runs in a throwaway container.** `tools/goose/goose-acp.sh <model-key-file> [goose-home]` builds
  the image from [`tools/goose`](../../tools/goose/Dockerfile) (Goose pinned, with its checksum) and runs it
  with `docker run -i --rm`, speaking ACP on its standard input and output. The work folder is mounted at the
  same path, so the paths the core and Goose name agree; Goose's sessions live in `goose-home`, so they survive
  the container; the model key file is mounted read-only and never copied. So the command is
  `["<repo>/tools/goose/goose-acp.sh", "<model-key-file>", "<goose-home>"]`, and `GOOSE_MODE=approve` in its
  environment makes Goose ask before every tool call.
- **OpenCode and Cursor run on the host.** Cursor uses the machine's own sign-in.
- **A prompt** starts the chat's session if it has none (`session/new`), or takes up the one the core names
  ([D-065](../decisions.md#d-065-a-prompt-names-a-session-only-to-resume-one)) with `session/resume` when the
  agent offers it and `session/load` otherwise; what `session/load` replays is not sent again. After taking a
  session up, the driver cancels whatever the agent carried on with by itself (Goose resumes the turn a crash
  cut off, and refuses a prompt until it ends) and waits until the agent has been quiet for two seconds: a
  restart closes the turn ([D-070](../decisions.md#d-070-a-restart-closes-the-turn-as-interrupted)). The prompt
  is written as for Codex ([D-062](../decisions.md#d-062-what-codex-reads-for-a-chat-message)), as text only.
  A turn is named after the session and the prompt's request id; request ids start from the time, so a turn
  never takes the name of one from before a restart.
- **An approval** is answered with the option of the choice made; **a question** (an elicitation form) with
  the values of the options chosen, and typed text in the question's free-text field.
- **Interrupt** answers the requests still waiting as cancelled, as ACP asks, then sends `session/cancel`.
  A message sent during a turn does the same, waits for the turn to end, then prompts (`restart`). **New
  session** sends `session/new`, only when no turn is running. **`set`** sends `session/set_config_option`.
- **Requests nobody is asked about** are answered at once: Cursor's `cursor/update_todos` is taken, anything
  else unknown is refused, so the agent never waits for ever.

Its capabilities: it can interrupt; a message during a turn restarts it; the core names the sessions, which
can be resumed when the agent can load or resume them; approvals come to the chat; no approval commands; no
files ([D-083](../decisions.md#d-083-the-core-passes-no-files-to-a-harness-yet)).

## Adding a harness

1. **Does it speak ACP well?** Add it as a configuration entry for the ACP driver.
2. **Does it need something ACP lacks?** Add a flavour to the ACP driver for that one thing.
3. **Does it matter enough, or is its ACP support poor?** Write a native driver: in Rust if the interface is a
   documented wire protocol, or as a separate program over the socket if the official library is in another
   language.
4. **Is it a runtime with its own users and channels?** Write a plugin for it that connects over the socket.
