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

- **Native.** `codex app-server` speaks JSON-RPC over stdio. OpenAI's repository contains Rust crates for the
  protocol and a client (`codex-rs/app-server-protocol`, `codex-rs/app-server-client`), Apache-2.0. They are not
  published on crates.io; the crates of that name there are an old third-party fork. They pull in much of the
  Codex workspace.
- **The official TypeScript SDK is the weaker interface.** `@openai/codex-sdk` wraps the CLI's JSON-lines mode,
  not `app-server`, according to its README.
- **ACP.** Not built in. The adapter is v2.7.0 (2026-11-16) and speaks ACP v1 and v2.
- **ACP covers:** fork, compaction, review, subagents, steering.
- **ACP lacks:** thread rollback, command execution and realtime (inferred from the adapter's method list).
  Background terminals and goals work only with one specific client.
- **Rust.** The community crate `codex-codes` covers the `app-server` protocol, including approvals and
  interrupt.

### OpenCode

- **Native.** `opencode serve`, HTTP with server-sent events. T3 Code needs two adapters for it, one per major
  version.
- **ACP.** Built in and vendor-maintained: v2.3.0 (2026-11-13), ACP v1.
- **ACP covers:** load, resume, list, fork, images, thoughts, diffs, form questions, model, effort and mode
  options, usage, MCP servers.
- **ACP lacks:** plan updates (none found in source); undo and redo. A second prompt during a turn is an error;
  steering exists only as the non-standard `_session/steering`. Subagent sessions are flattened into the parent.

### Pi

- **Native.** `pi --mode rpc`, strict JSON lines over stdio, with prompt, steer and follow-up commands. Pi has
  no tool-approval gate of its own.
- **ACP.** Third party only (`pi-acp` v0.0.34, last commit 2026-10-06, about a hundred open issues). It supports
  only new, load, list and delete. Pi's own project closed its ACP pull requests.
- **Rust.** The community crate `pi-codes` drives RPC mode, including steering.

### Goose

- **Native is ACP.** `goose acp` over stdio; `goose serve` for network use. Goose's own desktop app uses it.
  v1.59.2 (2026-11-16), ACP v1.
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
- **ACP.** Cursor has an ACP transport. T3 Code stopped using it and moved to the SDK.
- **Not examined directly.** This entry rests on T3 Code's choice and the package registry.

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
| `agent-client-protocol` | crates.io | 5.3.0, 2026-11-14 | Official Rust library, widely used |
| `@anthropic-ai/claude-agent-sdk` | npm | 0.3.326, 2026-11-16 | Official |
| `@cursor/sdk` | npm | 1.4.0, 2026-11-15 | Official |
| `@openai/codex-sdk` | npm | 0.169.0, 2026-11-17 | Official, but wraps the JSON-lines mode |
| `codex-app-server-protocol`, `codex-app-server-client` | `openai/codex` on GitHub | Unpublished | Official Rust crates, by git dependency |
| `claude-codes`, `codex-codes`, `pi-codes`, `opencode-codes` | crates.io, `meawoppl/rust-code-agent-sdks` | `claude-codes` 2.1.327, 2026-11-17 | Community. Each version names the CLI release it was tested against. One maintainer wrote nearly all commits; 30 stars |

Other Rust crates for Claude Code on crates.io were stale when checked. One named `claude-agent-sdk` lists an
Anthropic repository that does not exist, so it is not official.

## Sources

- <https://github.com/agentclientprotocol/agent-client-protocol> (specification, releases, v2 draft)
- <https://github.com/agentclientprotocol/registry>
- <https://github.com/agentclientprotocol/claude-agent-acp> and <https://github.com/agentclientprotocol/codex-acp>
- <https://github.com/openai/codex> (`codex-rs/app-server-protocol`, `sdk/typescript`)
- <https://github.com/anomalyco/opencode>
- <https://github.com/earendil-works/pi> and <https://github.com/svkozak/pi-acp>
- <https://github.com/aaif-goose/goose>
- <https://github.com/NousResearch/hermes-agent>
- <https://github.com/openclaw/openclaw>
- <https://github.com/meawoppl/rust-code-agent-sdks>
