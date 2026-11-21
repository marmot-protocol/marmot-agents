# Reference: the interface each harness offers

This page is detailed on purpose. It records what was found about each harness's native interface and its
Agent Client Protocol (ACP) support, so that driver choices can be checked and revisited without redoing the
research.

- **Read on:** 2026-11-17, from public repositories and official documentation.
- **How:** a research agent read primary sources only (repositories, registries, vendor docs); no secondary
  articles. Nothing was run against a live harness. Items marked "inferred" were concluded from source, not
  stated in it. The crate and package versions under "Libraries" were checked directly against crates.io, npm
  and GitHub.
- **Versions move fast.** Most of these projects release weekly. Re-check before relying on a detail.
- **OpenCode's ACP was run on 2026-11-21.** The OpenCode entry and the `agent-client-protocol` row under
  "Libraries" were checked that day: OpenCode 2.3.2 was driven over `opencode acp` by a recorder, and the
  crate was built into this workspace. What OpenCode sent is in
  [`harness-messages.md`](harness-messages.md#acp-opencode).
- **Codex was read again on 2026-11-19.** The Codex entry, its two rows under "Libraries" and the whole of
  [Codex: which library to build on](#codex-which-library-to-build-on) are from that day, and that section
  says how each fact was checked. Three candidate libraries were built and run on this machine.
- **Pi was read again on 2026-11-21.** Its entry, its part of the `pi-codes` row under "Libraries" and
  [Pi: which library to build on](#pi-which-library-to-build-on) are from that day. Nothing else on this page
  was re-checked.

## Summary

| Harness | Richest native interface | ACP path | ACP maintained by | Our driver |
| --- | --- | --- | --- | --- |
| Claude Code | Claude Agent SDK (TypeScript, Python) | Adapter: `@agentclientprotocol/claude-agent-acp` | ACP organisation, not Anthropic | Native, TypeScript |
| Codex | `codex app-server`, JSON-RPC over stdio | Adapter: `@agentclientprotocol/codex-acp`, which wraps `app-server` | ACP organisation, not OpenAI | Native, Rust |
| OpenCode | `opencode serve`, HTTP and server-sent events | Built in: `opencode acp` | Vendor | ACP |
| Pi | `pi --mode rpc`, JSON lines over stdio | Third party: `pi-acp` | One individual | Native, Rust |
| Goose | `goose acp` (ACP is its native interface) | Built in | Vendor | ACP |
| Hermes | Platform plugin interface (Python) | Built in: `hermes acp` | Vendor | Plugin |
| OpenClaw | Channel plugin interface (TypeScript) | Built in: `openclaw acp` | Vendor | Plugin |
| Cursor | Cursor SDK (TypeScript) | Exists; not examined | Not examined | Native, TypeScript |

## Per harness

### Claude Code

- **Native.** `@anthropic-ai/claude-agent-sdk` (0.3.326 on npm, updated 2026-11-16). Official SDKs exist for
  TypeScript and Python only. Under the SDK is a stdio control protocol on the `claude` command with stream-json
  input and output.
- **ACP.** Not built in; requests to add it were closed or remain open upstream. The adapter is v0.107.3
  (2026-11-16) and speaks ACP v1 plus experimental v2.
- **ACP covers:** fork, plans, diffs, questions, session options, MCP servers, subagents as child sessions.
- **ACP lacks:** rewind and checkpoint restore; plugins; in-process hooks and tools (inferred). Steering uses a
  non-standard `_session/steering` method. Several extras work only with one specific client.
- **Rust.** The community crate `claude-codes` tracks the CLI release by release. It was not confirmed whether
  it handles approval requests.

### Codex

Read again on 2026-11-19. The newest release is 0.170.0 (2026-11-18). The recordings are 0.169.0
(2026-11-13). Codex had nine stable releases between 2026-10-27 and 2026-11-18.

- **Native.** `codex app-server` speaks JSON-RPC over stdio, without the `"jsonrpc": "2.0"` field.
- **OpenAI's Rust crates.** `openai/codex` holds `codex-rs/app-server-protocol` (the message types) and
  `codex-rs/app-server-client`, Apache-2.0. Neither is published on crates.io. The crates named
  `codex-app-server-protocol` and `codex-protocol` there are one upload each, version 0.63.0 of 2025-12-11,
  from `namastexlabs/codex`: a third-party fork, not OpenAI's.
- **The client crate does not speak stdio.** `codex-app-server-client` offers three ways to reach an app
  server. One runs it inside the calling program. One is a remote client for a separate app server, which
  always carries WebSocket frames, over TCP or a Unix socket. One is a client for OpenAI's cloud. None talks
  to `codex app-server` over stdio, which is the interface the recordings were made on. The crate depends on
  `codex-core` and on `codex-app-server` itself, so it is heavier than the protocol crate. It was read at
  `rust-v0.169.0` and not built. If the driver ever moves to the WebSocket transport, it is worth another
  look.
- **The protocol crate is heavy.** At `rust-v0.169.0` the workspace has 168 member crates and pins Rust
  1.95.0. The protocol crate alone builds 721 crates. See
  [the trial](#codex-which-library-to-build-on).
- **Codex publishes its own schema.** `codex app-server generate-json-schema --out <dir>` writes JSON Schema
  for the protocol, and `generate-ts` writes TypeScript. Both are marked experimental in the CLI's help. From
  0.169.0 the schema is 329 files, most of them one type each. It lists 86 notifications from the server, 109
  requests from a client and 10 requests from the server. With `--experimental` it is 479 files, with 184
  requests from a client and 11 from the server. These were counted on 2026-11-19 from the `method` lists in
  `ServerNotification.json`, `ClientRequest.json` and `ServerRequest.json`.
  [`harness-messages.md`](harness-messages.md#size-of-the-protocol) gives 81, 108 and 9 for the same command
  and version, read on 2026-11-17. The difference was not traced.
- **Part of what a driver needs is in the experimental half.** A client opts in with
  `capabilities.experimentalApi` in `initialize`, as our recorder does. Experimental, and used in the
  recordings: the requests `thread/settings/update`, `turn/settings/update` and `collaborationMode/list`, and
  the field `availableDecisions` on a command approval, which lists the choices Codex offers.
- **Every server notification carries `emittedAtMs`** beside `method` and `params`: all 1,731 in the
  recordings. The schema describes it on `ServerNotification`. The JSON-RPC notification type in OpenAI's
  crate and in `codex-codes` has no place for it.
- **The official TypeScript SDK is the weaker interface.** `@openai/codex-sdk` (0.170.0 on npm, 2026-11-18)
  wraps the CLI's JSON-lines mode, not `app-server`, according to its README.
- **ACP.** Not built in. The adapter is v2.7.0 (2026-11-16) and speaks ACP v1 and v2.
- **ACP covers:** fork, compaction, review, subagents, steering.
- **ACP lacks:** thread rollback, command execution and realtime (inferred from the adapter's method list).
  Background terminals and goals work only with one specific client.
- **Rust, community.** `codex-codes` (0.169.6 on crates.io, 2026-11-18, Apache-2.0) has types for the
  `app-server` protocol, generated from the stable schema by a script of its own, and sync and async clients.
  Its README names 0.170.0 as the Codex it was tested against.

### OpenCode

- **Native.** `opencode serve`, HTTP with server-sent events. T3 Code needs two adapters for it, one per major
  version.
- **ACP.** Built in and vendor-maintained: v2.3.2 run on 2026-11-21, ACP protocol version 1. `opencode acp`
  runs its own server; it needs no account for OpenCode's free models.
- **ACP covers:** load, resume, list, fork, close, delete, images, thoughts, diffs, form questions, model, effort
  and mode options, usage, MCP servers. Seen running: load (which replays the conversation), resume (which does
  not), thoughts, diffs, permission requests, form questions, configuration options, commands, and each turn's
  token usage in the reply to `session/prompt`.
- **ACP lacks:** plan updates (none found in source, and 2.3.2 has no to-do tool at all); undo and redo. A second
  prompt during a turn is an error; steering exists only as an extension, which `initialize` announces in
  `_meta.steering`. Subagent sessions are flattened into the parent (not checked). A form question is asked only
  if the client declares the `elicitation` capability. A command's exit code is only in `rawOutput`.

### Pi

Re-checked on 2026-11-21, by installing Pi 1.6.0 in a container, running its RPC mode, and reading the
documentation in its npm package and the source of `pi-codes` 1.6.0. The recordings of that day are in
[`fixtures/recordings/pi`](../../fixtures/recordings/README.md#pi).

- **Native.** `pi --mode rpc`, strict JSON lines over stdio: commands in (`prompt`, `steer`, `follow_up`,
  `abort`, `new_session`, `switch_session`, `get_state`, `set_model`, `set_thinking_level`, `compact` and
  more), a response to each and session events out. A prompt sent during a run must say whether it steers the
  run or follows it. Pi has no tool-approval gate of its own; an extension can ask the client through Pi's
  extension UI records. The package is `@earendil-works/pi-coding-agent`, 1.6.0, published 2026-11-20.
- **ACP.** Third party only (`pi-acp`, 0.0.35 on npm, published 2026-11-21; its issue count was not checked
  again). It supports only new, load, list and delete. Pi's own project closed its ACP pull requests.
- **Rust.** The community crate `pi-codes` drives RPC mode, including steering. See
  [Pi: which library to build on](#pi-which-library-to-build-on).

### Goose

- **Native is ACP.** `goose acp` over stdio; `goose serve` for network use. Goose's own desktop app uses it.
  v1.59.2 (2026-11-16), ACP v1. Run on 2026-11-21 in a container built from `tools/goose`; what it sent is in
  [`harness-messages.md`](harness-messages.md#acp-goose). It declares no `session/resume` and answers it with
  "Method not found". Its tool calls carry no `kind` and no `name`, and its edits no diff.
- **Standard surface:** load, list, close, delete, images, thoughts, tool calls, permissions, questions, session
  options, usage. No plan or diff output was found.
- **Everything else is non-standard:** about 111 methods under `_goose/unstable/`, including steering.

### Hermes

- **Native.** A platform plugin interface in Python. The plugin runs inside Hermes.
- **ACP.** `hermes acp` exists (v0.23.2, 2026-11-16), but the ACP toolset leaves out message delivery and
  scheduled tasks. Hermes can also act as an ACP client that drives other agents.
- **Why not ACP.** ACP models one editor user per session. It has no place for the sender of a message, group
  routing, proactive delivery or reactions (inferred).

### OpenClaw

- **Native.** A channel plugin interface in TypeScript. The plugin runs inside OpenClaw.
- **ACP.** `openclaw acp` is a bridge to its gateway (v2026.11.4, 2026-11-16). Its own documentation lists no
  plans, diffs, structured questions or model selection, and calls it "not a full ACP-native editor runtime".
- **Why not ACP.** The same reason as Hermes.

### Cursor

- **Native.** `@cursor/sdk` (1.4.0 on npm, updated 2026-11-15), TypeScript.
- **ACP.** Cursor has an ACP transport, `cursor-agent acp`, which this project uses
  ([D-080](../decisions.md#d-080-cursor-is-driven-through-acp)). T3 Code stopped using it and moved to the SDK.
- **Run on 2026-11-21**: Cursor Agent CLI 2026.11.19 over ACP on the machine's sign-in. What it sent is in
  [`harness-messages.md`](harness-messages.md#acp-cursor). It does not name itself in `initialize`, declares
  only `loadSession` and session `list`, and keeps its to-do list with its own `cursor/update_todos`.

## About ACP itself

- **Version.** v1 is current (schema v1.32.0, 2026-11-14). v2 has been a draft since 2026-07-20 (alpha 16 on
  2026-11-14); the maintainers say to keep v1 and put v2 behind a flag.
- **Standard update types in v1:** agent message chunk, user message chunk, tool call, tool call update, plan,
  available commands, session option update, usage, compaction, notice. Thought chunks are part of the schema.
- **Standard methods in v1:** `session/new`, `session/load`, `session/resume`, `session/prompt`,
  `session/cancel`, `session/close`, `session/set_config_option`, `session/request_permission`,
  `elicitation/create`.
- **Tool call kinds:** read, edit, delete, move, search, execute, think, fetch, switch mode, other.
- **Steering is not standard in v1.** The standard offers cancel and then a new prompt. A proposal to
  standardise steering has been open since 2026-05-19. In practice Claude, Codex and OpenCode share a
  non-standard `_session/steering` method.
- **v2 changes that matter here:** a prompt can be sent in any state and the agent decides whether it steers or
  queues; progress arrives as state updates; `session/load` becomes `session/resume` with replay; the client
  file-system and terminal methods are removed.
- **Extensions are used heavily.** Harness-specific data travels in `_meta` fields and methods with a leading
  underscore. Only `_session/steering` is shared across harnesses.
- **Transport.** stdio is the only specified transport.

## Libraries

| Library | Where | Version when checked | Notes |
| --- | --- | --- | --- |
| `agent-client-protocol` | crates.io | 5.3.0, 2026-11-14; 5.5.1 built on 2026-11-21 | Official Rust library, widely used. 5.5.1 resolves into this workspace beside MDK and adds about 70 crates. Its types are in `agent-client-protocol-schema` (1.21.2 for 5.5.1), which the ACP driver depends on alone (about 40 crates). The types switch on `serde_json`'s `preserve_order` for every crate built with it. A turn's usage is behind its `unstable_end_turn_token_usage` feature |
| `@anthropic-ai/claude-agent-sdk` | npm | 0.3.326, 2026-11-16 | Official |
| `@cursor/sdk` | npm | 1.4.0, 2026-11-15 | Official |
| `@openai/codex-sdk` | npm | 0.170.0, 2026-11-18 | Official, but wraps the JSON-lines mode |
| `codex-app-server-protocol`, `codex-app-server-client` | `openai/codex` on GitHub | Unpublished. Tried at `rust-v0.169.0`; the newest tag, `rust-v0.170.0` of 2026-11-18, was not built | Official Rust crates. At `rust-v0.169.0` the protocol crate does not build as a git dependency without OpenAI's lock file and patches, and cannot be linked beside MDK at `6f6d2017d`. See [the trial](#codex-which-library-to-build-on) |
| `claude-codes`, `codex-codes`, `pi-codes`, `opencode-codes` | crates.io, `meawoppl/rust-code-agent-sdks` | `codex-codes` 0.169.6, 2026-11-18; `claude-codes` 2.1.327, 2026-11-17; `pi-codes` 1.6.0, 2026-11-20 | Community. Each version names the CLI release it was tested against. One maintainer wrote 668 of 674 commits; 30 stars |

Other Rust crates for Claude Code on crates.io were stale when checked. One named `claude-agent-sdk` lists an
Anthropic repository that does not exist, so it is not official.

## Codex: which library to build on

This is the trial that the open question Q-15 asked for. Its recommendation was accepted on 2026-11-19 as
[D-055](../decisions.md#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own).

- **Run on:** 2026-11-19, on one Linux machine with 20 cores, with Rust 1.97.1, the compiler this repository
  pins.
- **Against:** the 28 recordings in [`fixtures/recordings/codex`](../../fixtures/recordings/codex): 2,001
  lines, of which 4 are the recorder's own markers and 1,997 are messages. They are codex-cli 0.169.0.
- **How:** each candidate was built in a throwaway project outside the repository. One program replayed every
  recording through it. For each message it asked two things: does the JSON-RPC envelope parse, and does the
  payload parse into the candidate's type for that method. A typed value was then written back to JSON and
  compared with what was recorded. Seven messages that no recording holds were added by hand, to see what
  each candidate does with something a newer Codex could send.
- **The trial programs are not in the repository.** They were about 850 lines and are not driver code.

### The candidates

| | What it is | As tried |
| --- | --- | --- |
| A | OpenAI's own `codex-app-server-protocol` | A git dependency on `openai/codex` at commit `d32063f9`, which is the tag `rust-v0.169.0` |
| B | The community crate `codex-codes` | 0.169.6 from crates.io, with only its `types` feature |
| C | Types of our own | Two ways: generated from Codex's schema, and written by hand for only the fields the translation reads |

### Building each one

| | A: OpenAI's crate | B: `codex-codes` | C: by hand |
| --- | --- | --- | --- |
| Builds as an ordinary dependency | No | Yes | Yes |
| Crates built | 721, of which 35 are from `openai/codex` | 14. With its clients, 31 | 11: `serde`, `serde_json` and what they need |
| Clean debug build | 102 s | 14 s. With its clients, 16 s | 3.6 s |
| Build folder | 4.5 GB | 270 MB | 98 MB |
| Native code it links | SQLite, AWS-LC, zstd, OpenSSL, and Linux sandboxing | None | None |

Build times are one run each on a warm machine. They show the scale, not exact figures.

**A needed three things before it built.**

1. *Four entries of OpenAI's `[patch.crates-io]`.* Without them Cargo fails to resolve: OpenAI's code asks
   for a `proxy` feature that exists only in OpenAI's fork of `tokio-tungstenite`, and for a fork of `sqlx`
   that accepts a newer SQLite binding than the released `sqlx` does. Cargo reads a `[patch]` section only from the root of the build, so whoever depends on the crate
   has to repeat it. The four are `tokio-tungstenite`, `tungstenite`, `sqlx-core` and `sqlx-sqlite`.
2. *Not the other two.* Copying OpenAI's whole patch section also names their forks of `crossterm` and V8.
   Cargo then fetched V8 with its Chromium submodules, about 2 GB, and that attempt was stopped after more than
   ten minutes.
3. *OpenAI's `Cargo.lock`.* A git dependency does not bring its lock file. With a fresh resolution the build
   failed: `rama-core 0.3.0-alpha.4` does not compile against `rama-error 0.3.0`, which Cargo chose where
   OpenAI's lock file has `0.3.0-alpha.4`. With `codex-rs/Cargo.lock` copied in as the starting point, it
   built.

**A cannot be linked beside MDK today.** The core has to link MDK
([D-002](../decisions.md#d-002-one-rust-core-program-with-mdk-as-a-library)). This was tried for one pair
of versions, Codex `rust-v0.169.0` and MDK `6f6d2017d`, by resolving one project that depends on A and on
`rusqlite` as MDK does. Cargo refuses:

- OpenAI's `codex-state`, which the protocol crate reaches through `codex-rollout`, needs
  `libsqlite3-sys ^0.39.1`.
- MDK (at `6f6d2017d`, 2026-11-13) uses `rusqlite 0.40.1`, which needs `libsqlite3-sys ^0.38.1`, and patches
  it to a pinned build of SQLCipher.
- Both link the native library `sqlite3`, and Cargo allows one such crate in a program.

There is a second clash, which was read and not run. OpenAI's fork of `tokio-tungstenite` is version 0.28.0,
the version MDK uses for its relay connections. The patch would have to sit in the core's manifest, so it
would replace MDK's WebSocket library with OpenAI's fork.

### What each one does with the recordings

The 1,997 messages are:

- 1,731 notifications from Codex, and 8 requests from Codex;
- 110 requests to Codex, and 31 `initialized` notifications to Codex;
- 109 successful responses from Codex, and 1 error response;
- 7 replies from the client to Codex's requests. One of the 8 requests was never answered.

The table has three levels, and they are different questions. *The envelope* is the JSON-RPC wrapper. *A
type* is the candidate's own value for one method's payload. *Written back* is that value turned into JSON
again and compared with the recording.

| | A: OpenAI's crate | B: `codex-codes` | C: by hand |
| --- | --- | --- | --- |
| **The envelope** | | | |
| Parses, of 1,997 | 1,997 | 1,997 | 1,997 |
| Written back is identical | 266 | 266 | 1,997 |
| **A type for the payload** | | | |
| Notifications from Codex, of 1,731 | 1,669. The other 62 fail | 1,600. The other 131 fail | 455 are read through a type. 1,276 need none |
| Requests from Codex, of 8 | 8 | 8 | 8 |
| Requests to Codex, of 110 | 110 | 106. It has no type for 4 | Built by hand. Not tried |
| `initialized`, of 31 | 31 | 0. It has no type for it | Built by hand. Not tried |
| Successful responses from Codex, of 109 | 109 | 106. It has no type for 3 | 35 are read. The rest need none |
| The error response from Codex, of 1 | None: it is read from the envelope | None: it is read from the envelope | None: it is read from the envelope |
| Replies to Codex's requests, of 7 | 7 | 7 | Built by hand. Not tried |
| Total that parsed into a type | 1,934 | 1,827 | 498 read |
| **That type written back** | | | |
| Identical | 1,759 | 1,294 | Not written back: the JSON is kept |
| Differs only in nulls and defaults | 106 | 433 | |
| A value lost or changed | 69 | 100 | |

Each column's last three rows add up to its total: 1,934 for A and 1,827 for B.

What is behind those numbers:

- **The envelope.** A and B both drop `emittedAtMs` from every notification, because their notification type
  has only `method` and `params`. Everything else in the envelope survives: `params` and `result` are kept as
  untyped JSON in all three.
- **The failures are an artefact of our scrubber, and still say something.** `tools/record/scrub.py` writes
  the word `redacted` over account details. Where Codex sends one of a fixed list of words, A and B refuse
  the whole message: `account/updated` and `remoteControl/status/changed` in both, and
  `account/rateLimits/updated` in B. Real traffic would not fail this way. A real new word would.
- **A's typed values lost nothing that these recordings can show.** Its 69 changed values are one field: the
  plan name in `rateLimits.planType`, on each of the 69 `account/rateLimits/updated` messages. A's type for
  that one field has a catch-all, so the scrubbed word parses and is written back as `unknown`. That is a
  statement about its types and about scrubbed traffic. Its envelope still drops `emittedAtMs`, and its 62
  failures are messages it did not read at all.
- **B loses what the driver needs most.** Its types come from the stable schema, so they have no place for
  experimental fields. It drops `availableDecisions` from all five command approvals: that is the list of
  choices a person is offered. It has no type for `thread/settings/update`, `turn/settings/update` or
  `collaborationMode/list`. It also drops the server name and the thread and turn ids from the MCP elicitation
  request, and several settings from `thread/started` and the responses to `thread/start` and `thread/resume`.
- **C reads less on purpose.** The translation reads fields of 17 kinds of message, and the message and code
  of an error response, which are in the envelope. For the other 1,276 notifications the method name is
  enough, either to pass the message on or to leave it out.

### What each one does with a message it does not know

These seven were written by hand.

| A newer Codex sends | A: OpenAI's crate | B: `codex-codes` | C: by hand |
| --- | --- | --- | --- |
| A notification with an unknown method | Error | Caught, payload kept whole | Caught, payload kept whole |
| A request with an unknown method | Error | Caught, payload kept whole | Caught, payload kept whole |
| A known notification with an unknown field | Parses. The field is gone from the typed value | Parses. The field is gone from the typed value | Parses. The JSON is kept |
| The same, in a nested object | Parses. The field is gone | Parses. The field is gone | Parses. The JSON is kept |
| An item of an unknown type | Error | Error | Caught, payload kept whole |
| A known field with an unknown word | Error | Error | Parses |
| A known notification without a required field | Error | Error | Error |

The "known field with an unknown word" was the `status` of a turn, which has no catch-all in A or B. A few
fields do have one, such as the plan name above, and for those an unknown word parses. So the answer depends
on the field, and nothing in the types says which ones are safe.

An error in A or B is an error for that one message. The envelope has already parsed, so a driver can still
fall back to the JSON. But it then needs that fallback for every message, which is most of what C is.

### Does the native message survive?

A driver must never drop the native message
([D-007](../decisions.md#d-007-one-shared-event-vocabulary)). None of the three keeps it for you.

- In A and B the typed value is not the message: it has lost unknown fields, and in B known ones too. The
  untyped envelope is closer, and still loses `emittedAtMs`.
- So with any of the three, the driver has to keep the line as it arrived and treat types as a way to read
  it. C is built that way from the start. With A or B it is a second copy beside theirs.

### Generating types from the schema

Tried with `typify` 0.10.1, the usual Rust generator for JSON Schema, on five files of the 0.169.0 schema. It
produced nothing:

| Schema file | Result |
| --- | --- |
| `codex_app_server_protocol.v2.schemas.json`, `ServerNotification.json` | Error: `CodexErrorInfo` flattens a property that is not an object |
| `ClientRequest.json` | The same error, for `ThreadItemsListCursor` |
| `codex_app_server_protocol.schemas.json` | Panic: a `$ref` it could not find |
| `ServerRequest.json` | Panic: "not yet implemented" |

So `typify` 0.10.1 cannot read this schema, and this trial produced no generated types. That is as far as
it goes. Other generators were not tried, nor were the several hundred schema files for single messages, nor
the generator `codex-codes` wrote for itself, which is a Python script in its repository.

Candidate B is the one example here of generated types, and it is generated from the stable schema only.
That is why it lacks the approval choices and the settings requests. A generator run on the schema from
`--experimental` would not have that gap. Generated types are therefore not ruled out by this trial. They
were not shown to work either.

### Who maintains each, and how fast it follows Codex

| | A: OpenAI's crate | B: `codex-codes` | C: by hand |
| --- | --- | --- | --- |
| Maintainer | OpenAI | One person: 668 of 674 commits. 30 stars, 6 forks | Us |
| Licence | Apache-2.0 | Apache-2.0 | MIT, ours |
| Published | No. An internal crate of a 168-crate workspace, versioned with it | crates.io, 109 versions since 2026-02-21 | Not applicable |
| Follows a Codex release | It is the release | Within a day, for 0.165, 0.166, 0.167 and 0.169. It skipped 0.168, which came out the same day as 0.169 | When we record that release |
| A stated promise of compatibility | None found for the Rust crate | Each version names the Codex it was tested against | Not applicable |

A is always current, but only at the commit we pin, with that commit's lock file and patches. Moving to a new
Codex means moving all three together, and re-checking the clash with MDK each time.

### Not checked

- Real, unscrubbed traffic. The raw recordings are kept outside the repository and were not used.
- A newer Codex than 0.169.0. The seven hand-written messages stand in for it.
- A's build on a clean CI runner, and on macOS.
- Whether B or C can be linked beside MDK. Neither brings native code, so no clash is expected (inferred).
- B's own clients. Only its types were tried, since the driver has its own transport half.
- Anything the driver writes with types of its own: requests, and replies to Codex's requests. See the
  recommendation.

### Recommendation

**Build the Codex driver on types of its own, written by hand (C).** Accepted on 2026-11-19 as
[D-055](../decisions.md#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own).

- The driver keeps every message as the JSON it arrived as. That is the native payload.
- It reads a message through small types that hold only the fields the translation uses, about 250 lines in
  the trial. An unknown method, an unknown item type or an unknown word becomes a catch-all, not an error.
- It adds `serde` and `serde_json`: 11 crates with what they need. The item types are expected to need the
  same two.
- Codex's own schema, from `codex app-server generate-json-schema --experimental` at the version we record,
  stays the reference.

**What the trial showed for C is reading.** It did not try writing. The driver sends 12 kinds of request in
the recordings and answers 4 kinds of request from Codex, and with C each of those is built by hand. An
approval can be read correctly and still be answered in the wrong shape, which would leave the turn waiting.
Three checks are meant to cover that, and none has been run:

- build each request and reply the recordings hold from the driver's own types, and require the same JSON
  that was recorded. That includes `initialized`, which the client sends after every handshake and which is
  a notification, not a request;
- check what the driver builds against Codex's schema;
- answer an approval with a decision that Codex offers as an object. All five recorded command approvals
  offer one, and none of the four recorded replies chose it, so the first two checks would pass a driver
  that turned it into a plain "accept". The test has to check the whole decision and the id of the request
  it answers.

All three belong with the transport half of the driver. If they turn out badly, this recommendation should be
looked at again before the driver is finished.

Two have been run, on 2026-11-20, in
[`crates/marmot-agent-codex/tests/send.rs`](../../crates/marmot-agent-codex/tests/send.rs). The first passes on
every message the recorded client sent, 146 of 148: two were the recorder's own and the driver never sends them
(`thread/unsubscribe`, and `turn/settings/update`, which Codex refused). The differences it allows are each named
there, such as the recorder's per-turn `effort`. The third passes: an approval answered with the decision Codex
offered as an object sends the whole object back, under the request's id. The second, against Codex's schema, is
left to the test against a real Codex, which sends each kind of request and sees whether Codex takes it.

This goes against the official library, which
[D-006](../decisions.md#d-006-prefer-official-interfaces-and-libraries) prefers where one exists. The reasons:

1. **It cannot be linked beside MDK today**, and the core must link MDK. That is the result for the pair
   that was tried, Codex `rust-v0.169.0` and MDK `6f6d2017d`; `rust-v0.170.0` was not built. The SQLite
   clash is settled by OpenAI's and MDK's release schedules, not by us, and can come back with any Codex
   release.
2. **It does not build as a dependency.** At `rust-v0.169.0` it needed OpenAI's lock file as a starting
   point and four of their patches in our root manifest, one of which would replace MDK's WebSocket library.
3. **It is 721 crates and native code** for a job that is reading JSON.
4. **It does not remove the work.** An unknown method, an unknown item type and an unknown status of a turn
   are each an error. The one field found with a catch-all, the plan name, turns an unknown word into
   `unknown` without saying so. So the driver would still keep the raw message and handle the unknown itself.

The driver still builds on the official interface, `codex app-server`, and on OpenAI's own schema. What it
does not use is an internal crate that OpenAI does not publish.

`codex-codes` is the lighter library that
[D-005](../decisions.md#d-005-which-driver-each-harness-gets) names as the fallback. It is not recommended
either: it leaves out the experimental half of the protocol, which holds the approval choices and the
settings requests, and it refuses an item type it does not know. It is well kept and worth reading when a
Codex release changes something.

What C costs:

- The types are ours to keep right. Nothing fails to compile when Codex changes. A new recording, replayed in
  CI, is what shows the change, and that is true of A and B as well, since each has to be moved by hand.
- The requests and replies the driver sends are built by hand, which is why the three checks above matter.
- If the driver comes to read far more of the protocol, this choice should be looked at again.

## Pi: which library to build on

Checked on 2026-11-21, against Pi 1.6.0. Nothing here was measured beyond reading `pi-codes` 1.6.0's source
and changelog and running Pi.

`pi-codes` keeps up with Pi: its 1.6.0 was published the day Pi 1.6.0 was, and each release says which Pi it was
tested against and what changed on the wire. It was still not used, for reasons like Codex's
([D-055](../decisions.md#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own)):

- **Its client runs only `pi` found on the path.** The driver runs Pi through a script that starts a
  container, and one Pi per chat, so it needs its own process handling anyway.
- **Its client reads one command at a time**, buffering events while it waits, has no way to answer Pi's
  extension UI requests, and pipes Pi's standard error without reading it, which can stall Pi once the pipe
  fills (inferred from the source; not seen).
- **Its types decode a record into Rust values**, so the record's exact text, which the event's native message
  must be ([D-059](../decisions.md#d-059-eight-points-about-what-an-agent-event-carries)), has to be kept
  beside them anyway, and an extension UI request is not modelled.
- **The translation reads few fields**, and reading them from the JSON is as short as mapping its types.

So the driver reads Pi's records as JSON, with nothing between. This changes what
[D-005](../decisions.md#d-005-which-driver-each-harness-gets) says the Pi driver builds on; the project owner
accepted it as [D-086](../decisions.md#d-086-the-pi-driver-reads-pis-rpc-messages-with-types-of-its-own). The cost is the same as for Codex: nothing fails to compile when Pi changes a record,
and a new recording, replayed in CI, is what shows it.

## Sources

- <https://github.com/agentclientprotocol/agent-client-protocol> (specification, releases, v2 draft)
- <https://github.com/agentclientprotocol/registry>
- <https://github.com/agentclientprotocol/claude-agent-acp> and <https://github.com/agentclientprotocol/codex-acp>
- <https://github.com/openai/codex> (`codex-rs/app-server-protocol`, `sdk/typescript`)
- <https://github.com/anomalyco/opencode>
- <https://github.com/earendil-works/pi> and <https://github.com/svkozak/pi-acp>
- `@earendil-works/pi-coding-agent` 1.6.0 from npm: its `docs/rpc.md`, `docs/rpc-commands.md`, `docs/json.md`
  and `docs/rpc-extension-ui.md`, and Pi itself in a container (2026-11-21)
- <https://crates.io/crates/pi-codes> 1.6.0, its source and changelog (2026-11-21)
- <https://github.com/aaif-goose/goose>
- <https://github.com/NousResearch/hermes-agent>
- <https://github.com/openclaw/openclaw>
- <https://github.com/meawoppl/rust-code-agent-sdks>
- <https://crates.io/crates/codex-codes>, <https://crates.io/crates/codex-app-server-protocol> and
  <https://crates.io/crates/codex-protocol> (versions, dates and owners, 2026-11-19)
- `codex app-server generate-json-schema`, run with codex-cli 0.169.0 (2026-11-19)
- <https://github.com/oxidecomputer/typify> (0.10.1, the schema generator that was tried)
