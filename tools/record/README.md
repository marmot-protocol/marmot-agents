# Recording harness sessions

These tools record real sessions with a harness, so that drivers and the event model can be checked against
what the harness actually sends. The recordings they produced are in
[`fixtures/recordings`](../../fixtures/recordings/README.md).

| Tool | What it does |
| --- | --- |
| `record_codex.py` | Drives `codex app-server` through a scripted scenario and saves both directions |
| `claude/record_claude.ts` | Does the same for Claude Code through the Claude Agent SDK |
| `scrub.py` | Removes paths and account details from raw recordings |
| `inventory.py` | Lists every kind of message in a set of recordings, with field names and types |
| `demo/` | The small project every scenario runs in |
| `demo_mcp_server.py` | A minimal MCP server with one tool, for the `mcp_tool` scenario |

## Before you start

- Recording uses your own signed-in Codex and Claude Code, and a small amount of your subscription.
- Each scenario lets the agent run commands and edit files in the project directory you give it. Give it a
  throwaway copy of `demo/`, never a real project.
- Raw recordings contain your paths and account details. Keep them outside the repository.

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

## Adding a scenario

Add a function to `SCENARIOS` in `record_codex.py` and an entry to `SCENARIOS` in `record_claude.ts`, with the
same name and the same prompt where the harnesses allow it. Then record, scrub, and add a row to the table in
`fixtures/recordings/README.md`. If a harness does not behave as the scenario intends, keep the recording and
say so there: what a harness does instead is the useful part.

## Notes

- The Codex recorder starts the app server with `--disable hooks`, so your own Codex hooks do not run. Threads
  are started as ephemeral, so nothing is added to your Codex history. The `resume` scenario is the exception:
  it has to keep its thread.
- Claude Code keeps every session. Each Claude recording adds a small session to your Claude Code history,
  under a project named after the demo directory.
- The Claude recorder passes `settingSources: []`, so your own Claude Code settings are not loaded, and uses the
  `haiku` model to keep the cost down.
- The pinned SDK version in `claude/package.json` is a few days behind the newest on purpose: some package
  managers refuse releases younger than a week.
