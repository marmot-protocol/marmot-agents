# Recording harness sessions

These tools record real sessions with a harness, so that drivers and the event model can be checked against
what the harness actually sends. The recordings they produced are in
[`fixtures/recordings`](../../fixtures/recordings/README.md).

| Tool | What it does |
| --- | --- |
| `record_codex.py` | Drives `codex app-server` through a scripted scenario and saves both directions |
| `claude/record_claude.ts` | Does the same for Claude Code through the Claude Agent SDK |
| `record_acp.py` | Does the same for an agent that speaks ACP, such as `opencode acp`, or Goose in its container |
| `record_pi.py` | Does the same for `pi --mode rpc`, with Pi in a container; see [Pi](#pi) |
| `scrub.py` | Removes paths and account details from raw recordings |
| `inventory.py` | Lists every kind of message in a set of recordings, with field names and types |
| `demo/` | The small project every scenario runs in |
| `demo_mcp_server.py` | A minimal MCP server with one tool, for the `mcp_tool` scenario |
| `hermes/` | A container and a plugin that record the Hermes gateway; see [Hermes](#hermes) |
| `openclaw/` | A container and a plugin that record the OpenClaw gateway; see [OpenClaw](#openclaw) |

## Before you start

- Recording uses your own signed-in Codex and Claude Code, and a small amount of your subscription.
- Each scenario lets the agent run commands and edit files in the project directory you give it. Give it a
  throwaway copy of `demo/`, never a real project.
- Raw recordings contain your paths and account details. Keep them outside the repository.
- OpenCode, Goose, Pi, Hermes and OpenClaw run on a model you choose; nothing here has a default
  ([D-100](../../docs/decisions.md#d-100-the-installers-defaults)). Copy
  [`tests.env.example`](../../tests.env.example) to `tests.env` at the top of the repository, which git ignores,
  and fill in `MODEL_BASE_URL`, `MODEL_NAME` and `MODEL_KEY_FILE`, the file that holds the endpoint's key, and,
  for OpenCode, `OPENCODE_MODEL`, the model as OpenCode names it. The scenarios against real harnesses read the
  same file. A variable set in the environment wins; a name the file gives twice is an error.

## Record

```sh
work=$(mktemp -d)
cp -r tools/record/demo "$work/demo" && git -C "$work/demo" init -q && git -C "$work/demo" add -A \
  && git -C "$work/demo" -c user.name=recorder -c user.email=recorder@example.invalid commit -q -m demo
mkdir -p "$work/raw/codex" "$work/raw/claude"

# Codex: needs the `codex` command. No dependencies.
python3 tools/record/record_codex.py --list
python3 tools/record/record_codex.py approval_allowed "$work/demo" "$work/raw/codex/approval_allowed.jsonl"

# Claude: needs `bun` and the SDK.
(cd tools/record/claude && bun install)
bun tools/record/claude/record_claude.ts --list
bun tools/record/claude/record_claude.ts approval_allowed "$work/demo" "$work/raw/claude/approval_allowed.jsonl"

# An ACP agent: needs the agent's command, `opencode` by default. No dependencies.
mkdir -p "$work/raw/acp/opencode"
python3 tools/record/record_acp.py --list
python3 tools/record/record_acp.py approval_allowed "$work/demo" "$work/raw/acp/opencode/approval_allowed.jsonl"

# Goose, over ACP, in a throwaway container (needs Docker and a model key file; see tools/goose/goose-acp.sh).
mkdir -p "$work/raw/acp/goose"
python3 tools/record/record_acp.py approval_allowed "$work/demo" "$work/raw/acp/goose/approval_allowed.jsonl" \
  --agent "$PWD/tools/goose/goose-acp.sh <model-key-file>"
```

Reset the demo project between scenarios, because some of them edit it:

```sh
git -C "$work/demo" checkout -q . && git -C "$work/demo" clean -qfd
```

## Scrub, then check

```sh
python3 tools/record/scrub.py "$work/raw" fixtures/recordings \
  --map "$work/demo=/work/demo" --map "$work=/work/tmp" --map "$HOME=/home/user"
```

Give the most specific path first. The scrubber exits with an error if anything that looks private is left in
its output. A clean exit is necessary, not sufficient: read the diff before you commit.

## Look at what you recorded

```sh
python3 tools/record/inventory.py fixtures/recordings/codex --kinds   # kinds of message and counts
python3 tools/record/inventory.py fixtures/recordings/claude          # with every field and its type
```

## Hermes

Hermes is recorded in a container, with a plugin that plays the chat.

```sh
tools/record/hermes/record.sh tools/record/hermes/scenarios/simple.json out/simple.jsonl [model-key-file] [model]
```

- **What it needs.** Docker, and a model: `MODEL_BASE_URL`, `MODEL_NAME` and `MODEL_KEY_FILE`, as
  [above](#before-you-start). A key file or a model name given as an argument wins.
- **Every run starts from zero.** The container gets a fresh data volume and is removed afterwards. To reuse a
  configured Hermes across runs, run the image yourself with a named volume on `/data`.
- **The key is mounted read-only when the container starts.** It is never built into the image or written to
  the output.
- **What is in the image.** Hermes at a pinned commit, and the plugin in `hermes/plugin`. No Marmot, no relay,
  no account of ours. Change `HERMES_AGENT_REF` in the `Dockerfile` to record a newer Hermes.
- **A scenario is a JSON file** in `hermes/scenarios`: the chats, and a list of steps. A step is a chat message
  from a named sender, a wait for Hermes to call a method or send some text, a press of an approval button, a
  pick from the model picker, a request to Hermes for its commands or a chat's model settings as data, or a
  pause. The step types are listed at the top of `hermes/plugin/__init__.py`.
- **A scenario can change the set-up** with an `env` block: who is allowed, what the plugin claims to support
  (`RECORDER_APPROVAL_BUTTONS`, `RECORDER_MODEL_PICKER`), which plugin hooks it registers
  (`RECORDER_PLUGIN_HOOKS`), and a JSON object merged into Hermes's configuration (`HERMES_CONFIG_PATCH`).
- **Plugin hooks are off unless a scenario names them.** A Hermes plugin can register hooks beside its
  platform, such as `on_session_reset` and `pre_command`. The scenarios recorded on 2026-11-17 register none.
- **Reasoning effort.** Hermes asks the model for `medium` by default, and some endpoints refuse values they do
  not offer. The container sets `high`; override it with `REASONING_EFFORT`.
- **To see why a run failed**, set `RECORD_LOG` to a file. The gateway's own log is written there.

Scrub the output like any other recording, with the files in a folder named `hermes`.

## OpenClaw

OpenClaw is recorded the same way as Hermes: in a container, with a plugin that plays the chat.

```sh
tools/record/openclaw/record.sh tools/record/openclaw/scenarios/simple.json out/simple.jsonl [model-key-file] [model]
```

- **What it needs.** Docker, and a model: `MODEL_BASE_URL`, `MODEL_NAME` and `MODEL_KEY_FILE`, as
  [above](#before-you-start). A key file or a model name given as an argument wins.
- **Every run starts from zero.** The container gets a fresh data volume and is removed afterwards.
- **The key is mounted read-only when the container starts.** It is never built into the image, and OpenClaw's
  configuration file only names the variable that holds it. The plugin also replaces the key with a marker if
  it ever appears in something it writes, whole or in a masked form that keeps its last characters.
- **Do not send `/model status`.** OpenClaw answers it with the key in a masked form: the first and the last
  eight characters. The plugin now replaces that too, but a recording is no place to test it.
- **What is in the image.** OpenClaw at a pinned release, and the plugin in `openclaw/plugin`. No Marmot, no
  relay, no account of ours. Change `OPENCLAW_VERSION` in the `Dockerfile` to record a newer OpenClaw.
- **The plugin is a channel written for recording**, not a copy of the Marmot channel in MDK. It registers
  every callback OpenClaw offers a channel and records each call. It also records the calls it makes into
  OpenClaw, because in OpenClaw the plugin asks for the route, for the decision on who may speak, and for the
  turn to run.
- **A scenario is a JSON file** in `openclaw/scenarios`: the chats, and a list of steps. A step is a chat
  message from a named sender, a wait for OpenClaw to call something or deliver some text, a press of an
  approval or question button, a request to OpenClaw through the plugin SDK for its commands, its models or a
  session's stored entry as data, or a pause. The step types are listed at the top of
  `openclaw/plugin/index.js`.
- **Plugin hooks are off unless a scenario names them** in `RECORDER_PLUGIN_HOOKS`, for example
  `session_start,session_end,before_reset`. The scenarios recorded on 2026-11-17 register none.
- **A scenario can change the set-up** with an `env` block: who is allowed, whether commands need approval,
  what the plugin claims to support, and how much OpenClaw is asked to send to a chat. The settings are listed
  at the top of `openclaw/entrypoint.sh` and `openclaw/plugin/index.js`.
- **The defaults ask for everything.** The container switches on OpenClaw's tool summaries, reasoning and
  block streaming, which are all off in a fresh OpenClaw. `tool_call_plain` records the same request with them
  off.
- **To see why a run failed**, set `RECORD_LOG` to a file. The gateway's own log is written there.

Scrub the output like any other recording, with the files in a folder named `openclaw`.

## Pi

Pi is recorded through its RPC mode, with Pi in a throwaway container built from
[`tools/pi`](../pi/Dockerfile), the same one the Pi driver runs. Nothing is installed on the machine.

```sh
python3 tools/record/record_pi.py --list
python3 tools/record/record_pi.py command "$work/demo" "$work/raw/pi/command.jsonl" [model-key-file]
```

- **What it needs.** Docker, and a model: `MODEL_BASE_URL`, `MODEL_NAME` and `MODEL_KEY_FILE`, as
  [above](#before-you-start). A key file given as an argument wins.
- **The key is mounted read-only when the container starts.** Pi reads it from that file whenever it calls the
  model; it is never built into the image, put in an environment variable or written anywhere.
- **Every recording starts from zero.** Pi's state, its settings and its sessions, is a fresh temporary folder
  for each recording, removed afterwards. `resume` and `restart_mid_turn` start a second Pi on the same folder.
- **The recorder opens a session as the driver does**, so the driver's translation can be checked against what
  it really sends: `get_state`, `get_available_models`, `get_available_thinking_levels` and `get_commands`
  before the first prompt, and after a new session. It notes `pi --version` first.
- **A message sent during a turn** goes as a `prompt` with `streamingBehavior: steer`, as the driver sends it.
- **The `gate_*` scenarios** load Pi's example permission-gate extension, which is in the image.

Scrub the output like any other recording, with the files in a folder named `pi`.

## Adding a scenario

Add a function to `SCENARIOS` in `record_codex.py` and an entry to `SCENARIOS` in `record_claude.ts`, with the
same name and the same prompt where the harnesses allow it. Then record, scrub, and add a row to the table in
`fixtures/recordings/README.md`. If a harness does not behave as the scenario intends, keep the recording and
say so there: what a harness does instead is the useful part.

## Notes

- The ACP recorder runs the agent with `HOME` and the XDG folders set to a throwaway folder beside the project
  (or `--home`), so it reads none of your settings, skills, plugins or sign-ins and adds nothing to your history.
  For OpenCode that means a model it reaches without signing in, such as one of its free ones, which the
  recordings use; `OPENCODE_MODEL` names it, unless `--config` does. `--config` passes OpenCode a
  configuration as JSON, and `--agent` names another agent's command. The scrubber reads ACP recordings from
  `raw/acp/<agent>/`.
- Goose runs in a container built from `tools/goose`, with the work folder mounted at the same path and its
  home in the recorder's throwaway folder, so a session survives the restart in `resume` and `load`. The key
  file is mounted read-only; `MODEL_BASE_URL` and `MODEL_NAME` come from the environment or `tests.env`, as
  [above](#before-you-start). Approval scenarios run Goose in its `approve` mode; the others in Goose's own
  default, `auto`.
- Cursor runs on the machine's own sign-in (`--own-home`; no credential is copied), so its sessions go into
  your Cursor history. It asks before every command unless started with `--force`, so scenarios that need no
  approval use `--agent-auto "cursor-agent --force acp"`. Its command list names your own skills and entries
  synced from the account; the scrubber keeps only Cursor's built-in commands
  ([D-085](../../docs/decisions.md#d-085-cursor-recordings-keep-only-cursors-built-in-commands)). Read the
  scrubbed output by hand all the same: a model that goes looking can read files outside the project.
- Antigravity runs on the machine's own Google sign-in (`--own-home`), with `--agent "agy acp"` and
  `--agent-auto "agy acp --dangerously-skip-permissions"`. The scrubber trims `available_commands_update` to
  Antigravity's built-in slash commands and skills, neutralises `_meta.antigravity.tier`, and scrubs mapped
  paths split across streamed chunks.

- The Codex recorder starts the app server with `--disable hooks`, so your own Codex hooks do not run. Threads
  are started as ephemeral, so nothing is added to your Codex history. Nine scenarios are the exception,
  because each has to keep its thread: `resume`, `restart_mid_turn`, `resume_with_settings`, and the six
  `subagent` scenarios, since a subagent cannot start from a thread Codex does not keep.
- The subagent recordings are kept in their own folder, `fixtures/recordings/codex/subagents`. The scrubber reads
  a raw folder named after the harness, so record them into a separate raw folder, scrub it into a staging
  folder, and move the scrubbed files from `<staging>/codex/` into `fixtures/recordings/codex/subagents/`.
- Claude Code keeps every session. Each Claude recording adds a small session to your Claude Code history,
  under a project named after the demo directory.
- The Claude recorder passes `settingSources: []`, so your own Claude Code settings are not loaded, and uses the
  `haiku` model to keep the cost down.
- The pinned SDK version in `claude/package.json` is a few days behind the newest on purpose: some package
  managers refuse releases younger than a week.
