# The model double

A small local server that speaks the model providers' wire formats, so a real harness runs against it with
only its model faked. Why, and what it proves, is in
[`docs/design/verification.md`](../../docs/design/verification.md#11-a-model-double). It uses Python's
standard library and nothing else.

| Format | Path | Harnesses |
| --- | --- | --- |
| Anthropic Messages | `POST …/v1/messages` | Claude Code |
| OpenAI Responses | `POST …/responses` | Codex |
| OpenAI Chat Completions | `POST …/chat/completions` | OpenCode, Goose, Pi, Hermes, OpenClaw |

```sh
python3 tools/model_double/double.py scripted <script.json>    # prints http://127.0.0.1:<port> first
python3 tools/model_double/double.py replay <cassette.jsonl>
python3 -m unittest discover -s tools/model_double -p 'test_*.py'
```

`double.py`'s opening comment says how a request is keyed, the form of a script and of a cassette, and what
each mode does. `crates/marmot-agent/tests/model_double` starts it from the Rust scenarios.

## Pointing a harness at it

| Harness | How | Isolated from the machine's own sign-in by |
| --- | --- | --- |
| Claude Code | `ANTHROPIC_BASE_URL=<url>`, `ANTHROPIC_API_KEY=<anything>` | a new `HOME` and `CLAUDE_CONFIG_DIR`, and a cleared environment |
| Codex | `config.toml` in a new `CODEX_HOME`: `model_provider` naming a `[model_providers.<id>]` with `base_url = "<url>/v1"`, `wire_api = "responses"` and `env_key` naming a variable that holds anything | a new `HOME` and `CODEX_HOME`, and a cleared environment |
| The rest | their OpenAI-compatible base URL, `<url>/v1` | their own homes, as their scenarios set them up |

Cursor cannot be pointed at it: its agent reaches models only through Cursor's own servers.

## Recording a cassette

Recording needs a real model, and a key for it. The double in `record` mode runs in a container with only the key
file mounted, read-only; the harness runs as usual and talks to it on the loopback address. Numes serves Chat
Completions only, so the double translates an Anthropic Messages or Responses request to Chat Completions, sends
it on unstreamed, and writes the reply back in the harness's format.

It needs Docker, Node 24, the Claude Code driver's packages (`npm ci --prefix packages/claude-code`; the test reads
the Claude Code version from them before it starts anything), a model at an OpenAI-compatible endpoint, and its
key in a file.

```sh
work=$(mktemp -d)
cp -r tools/record/demo "$work/demo"
MODEL_DOUBLE_KEY_FILE=<key file> MODEL_DOUBLE_UPSTREAM=<OpenAI-compatible base URL> \
  MODEL_DOUBLE_MODEL=<model> MODEL_DOUBLE_OUT="$work/raw" MODEL_DOUBLE_DEMO="$work/demo" \
  cargo test -p marmot-agent --test claude record_claude_code_turn -- --exact --ignored --nocapture
mkdir -p "$work/scrub/cassettes/claude"
cp "$work/raw/cassette.jsonl" "$work/scrub/cassettes/claude/approval.jsonl"
python3 tools/record/scrub.py "$work/scrub" "$work/clean" --map "$work/demo=/work/demo" --map "$HOME=/home/user"
```

The scrubber writes its output even when it finds something, and then exits 1: that output must not be
committed. Go on only if it exits 0, and read the cassette whole even then; a clean exit is not proof.

```sh
less "$work/clean/cassettes/claude/approval.jsonl"   # read it all
cp "$work/clean/cassettes/claude/approval.jsonl" fixtures/cassettes/claude/approval.jsonl
```

Only scripted scenarios, in a throwaway copy of `tools/record/demo`. A cassette holds no header, no key and no
request body: only each request's key and the model's reply.

## Staleness

`check_cassettes.py` reports every cassette older than 14 days and every harness with a release newer than the
one a cassette was recorded with. It needs no key. A pull request runs it offline, and its findings are
warnings that do not fail it. The nightly run (`.github/workflows/nightly.yml`) adds `--strict`, which exits 1
when a cassette is stale, and opens an issue with the output, newer releases included. A newer release alone
fails nothing: Claude Code ships most days, and each recording needs a key, so drift shows instead as a replay
that cannot match
([D-127](../../docs/decisions.md#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key)). A folder that is not there is an error, exit 2, in either.

```sh
python3 tools/model_double/check_cassettes.py fixtures/cassettes --offline   # as a pull request runs it
python3 tools/model_double/check_cassettes.py fixtures/cassettes --strict    # as the nightly run does
```
