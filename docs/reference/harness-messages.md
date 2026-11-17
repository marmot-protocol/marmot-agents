# Reference: what Codex and Claude Code actually send

This page is detailed on purpose. It records what two harnesses send over their native interfaces, taken from
real sessions, and how each message relates to the proposed [event model](../design/event-model.md).

## Sources

| Source | What it is | How it was used |
| --- | --- | --- |
| [`fixtures/recordings`](../../fixtures/recordings/README.md) | 26 scripted sessions, 12 with Codex and 14 with Claude Code, recorded on 2026-11-17 | Read in full. Every claim on this page about order, fields and behaviour comes from these |
| [`data/claude-sdk-message-structures.txt`](data/claude-sdk-message-structures.txt) | The structure of 176,652 Claude Agent SDK messages from four days of real use on one developer's machine | Counts and field names only. No content was read or kept |
| Codex's generated protocol schema | `codex app-server generate-json-schema`, codex-cli 0.169.0 | The list of methods |

Limits of this evidence:

- The scripted sessions use small models, short prompts and a three-file project. They show the shape of each
  message, not typical volume.
- The real-use counts come from one person doing long, autonomous coding sessions through T3 Code, with
  approvals switched off. They show volume and variety, but no approvals.
- Only Codex and Claude Code are covered.

To look at any message kind in full, run `python3 tools/record/inventory.py fixtures/recordings/codex` or
`.../claude`.

## Codex

Interface: `codex app-server`, JSON-RPC over stdio. A client sends requests and receives responses,
notifications and a few requests of its own to answer.

### Size of the protocol

| | Count |
| --- | --- |
| Requests a client can send | 108 |
| Notifications the server sends | 81 |
| Requests the server sends to the client | 9 |

The server's requests are the ones a driver must be able to answer: `item/commandExecution/requestApproval`,
`item/fileChange/requestApproval`, `item/permissions/requestApproval`, `item/tool/requestUserInput`,
`item/tool/call`, `mcpServer/elicitation/request`, `openai/form`, and two for sign-in.

### What appeared in the recordings

934 messages in 12 sessions.

| Group | Messages | Share |
| --- | --- | --- |
| Text deltas (`item/agentMessage/delta`) | 476 | 51% |
| Bookkeeping: thread status, token usage, rate limits, moderation metadata, MCP start-up, account, settings | 209 | 22% |
| Items starting and completing | 119 | 13% |
| Client requests and all responses | 94 | 10% |
| Turns starting and completing, diff updates | 32 | 3% |
| Approval requests | 4 | under 1% |

### Items

Everything the agent does is an **item** with a stable `id`. It arrives as `item/started` and later as
`item/completed` with the same id.

| Item type | Seen in | Fields that matter |
| --- | --- | --- |
| `userMessage` | Every turn | An echo of the prompt. A steering message appears as one too |
| `agentMessage` | Every turn | `text`; `phase` is `commentary` or `final_answer`; `questions`, when present, is a list of `{title, options}` |
| `reasoning` | Rarely at low effort | `summary`, `content` |
| `commandExecution` | 9 scenarios | `command`, `cwd`, `status` (`completed`, `failed`, `declined`), `exitCode`, `durationMs`, `aggregatedOutput`, and `commandActions`, which classifies the command as `read`, `search` or `unknown` |
| `fileChange` | 2 scenarios | `changes`: a list of `{path, kind, diff}`; `status` |
| `sleep` | `question` only | `durationMs`. The agent waiting for something |

The schema lists more item-related notifications that these sessions did not trigger, among them
`item/plan/delta`, `turn/plan/updated`, `item/mcpToolCall/progress` and `item/reasoning/summaryTextDelta`.

### A turn with an approval

From `approval_allowed`, bookkeeping left out:

```text
> turn/start
  turn/started
  item/started    userMessage
  item/completed  userMessage
  item/started    agentMessage        (commentary)
  item/completed  agentMessage
  item/started    commandExecution
  item/commandExecution/requestApproval   <- a request; the turn waits
> response {decision: "accept"}
  serverRequest/resolved
  item/completed  commandExecution    status=completed
  item/started    agentMessage        (final answer)
  item/completed  agentMessage
  turn/completed                      status=completed
```

- The approval request names the item it is about (`itemId`) and lists `availableDecisions`. In the recording
  that list was `accept`, an object `{acceptWithExecpolicyAmendment: {...}}`, and `cancel`.
- **A decision is not always a plain string.** One of the offered decisions is an object.
- **The list is not complete.** `decline` was not in it, and answering `decline` worked: the item completed
  with status `declined`.
- A file change is approved the same way, with `item/fileChange/requestApproval`. That request carried no
  list of decisions at all.

### Controls

| Control | What happened |
| --- | --- |
| Interrupt | `turn/interrupt` returned at once, and `turn/completed` arrived with status `interrupted`. **The running command never got an `item/completed`.** |
| Steer | `turn/steer` returned at once. The new message appeared as a `userMessage` item inside the same turn, and the agent took it into account before finishing |
| Second turn | `turn/start` on the same thread |

### The question that was not a request

Asked to put a multiple-choice question, Codex in its default mode sent an `agentMessage` whose `questions`
field held the title and the options. It then ran three 60-second `sleep` items, and ended the turn saying it
was still waiting. The protocol never asked the client for an answer. The answer to such a question is simply
the user's next message.

Codex also has a real question request, `item/tool/requestUserInput`. It was not triggered and still needs
recording.

## Claude Code

Interface: the Claude Agent SDK. The application calls `query()` with a stream of user messages and receives a
stream of messages back. Permission requests do not arrive in that stream; they arrive through a `canUseTool`
callback.

### What real use looks like

176,652 messages over 1,327 completed turns.

| Message | Count | Per turn |
| --- | --- | --- |
| `stream_event`: partial text and block boundaries | 92,368 | 70 |
| `assistant`: a finished block of text, thinking or tool use | 30,178 | 23 |
| `user`: tool results | 16,091 | 12 |
| `system / status` | 13,583 | 10 |
| `tool_progress` | 5,505 | 4 |
| `system / task_started`, `task_progress`, `task_updated`, `task_notification` | 9,636 | 7 |
| `command_lifecycle` | 2,561 | 2 |
| `system / background_tasks_changed` | 1,573 | 1 |
| `system / init` | 1,330 | 1 |
| `result` | 1,327 | 1 |
| `rate_limit_event` | 471 | |
| `system / session_title_changed` | 309 | |
| `system / vcs_state_changed` | 246 | |
| `system / code_change_published` | 196 | |
| `system / compact_boundary` | 17 | |
| `system / api_retry` | 14 | |
| `system / commands_changed` | 4 | |

Inside the `assistant` and `user` messages:

| Block | Count | Per turn |
| --- | --- | --- |
| Tool use | 16,007 | 12 |
| Tool result | 15,895 | 12 |
| Thinking | 9,532 | 7 |
| Text | 4,639 | 3.5 |

26 different tools were used. One tool, the shell, accounts for 88% of tool calls.

### How a tool call is represented

There is no typed item for a command or a file edit. A tool call is a `tool_use` block inside an `assistant`
message, with a tool `name`, an `id` and a free-form `input`. Its result arrives later as a `tool_result` block
inside a `user` message that carries the same id, with a structured `tool_use_result` beside it.

So whether a tool call is a command, a file change or a search is not stated. A driver has to decide from the
tool's name.

### A turn with an approval

From `approval_denied`:

```text
> query(options)
> user message                         (the prompt)
  system / init
  assistant   [tool_use: Bash]
  canUseTool(Bash, input, options)     <- a callback; the turn waits
> {behavior: "deny", message: "..."}
  user        [tool_result]            (the refusal, fed back to the model)
  assistant   [thinking]
  assistant   [text]
  result / success
```

- The request carries the tool name, its input, a `toolUseID`, a `requestId`, and sometimes `suggestions`:
  permission rules the user could add so as not to be asked again.
- The answer is `allow`, optionally with changed input, or `deny` with a message.
- **There is no list of choices.** The choices are fixed by the SDK: allow, deny, and whatever the suggestions
  offer.

### A question is a tool call answered through the approval callback

From `question`: the agent calls a tool named `AskUserQuestion`. Its input is a list of questions, each with a
`header`, `options` that have a `label` and a `description`, and a `multiSelect` flag. It arrives through
`canUseTool` like any approval. The answer is `allow` with the same input plus an `answers` object keyed by
the question text.

So one request can hold several questions, and a question can allow several selections.

### Controls

| Control | What happened |
| --- | --- |
| Interrupt | `interrupt()` returned `{still_queued: []}`. A `result / error_during_execution` followed, and `query()` threw |
| Steer | A user message with `priority: "now"` was sent mid-turn. **The running turn ended with a result, and a new turn began** with a fresh `system / init` |
| Second turn | Another user message on the same input stream |

### Work that outlives the turn

From `subagent`: the agent started a subagent, which ran in the background. A `result` arrived while it was
still working. Then came `task_progress`, `task_updated`, `task_notification`, a new `system / init`, more
`assistant` text and a second `result`, with no new prompt from the user.

A subagent's own messages arrive in the same stream as the main agent's. They are marked with a
`parent_tool_use_id` that names the tool call that started the subagent.

In real use, background-task messages are about 7 per turn.

## Side by side

| | Codex | Claude Code |
| --- | --- | --- |
| Unit of work | A typed item with an id | A content block with an id, inside a message |
| Start and finish | `item/started`, `item/completed` | `tool_use` block, then a `tool_result` block |
| Commands, file changes | Their own item types | A generic tool call; the type must be inferred from the tool's name |
| Approval request | A JSON-RPC request naming the item, with a list of decisions | A callback with the tool name and input, with fixed allow or deny |
| Decision | A string or an object | `allow` or `deny`, with optional changed input |
| Question | A field on an ordinary message, answered by the next message; or a separate request | A tool call answered through the approval callback |
| Steering | Stays inside the running turn | Ends the turn and starts another |
| Interrupt | The turn completes as `interrupted`; open items are left open | The turn ends with an error result |
| Work after the turn ends | Not seen | Yes: background tasks and subagents |
| End of turn | `turn/completed` with a status | `result` with a subtype |
| Usage | `thread/tokenUsage/updated` during the turn | On `result`, with cost |

## How the messages map to the proposed items

"Not sent" means the driver uses it or drops it, and nothing goes to the chat.

| Proposed item | Codex | Claude Code |
| --- | --- | --- |
| Message | `agentMessage` item | `text` block |
| Reasoning | `reasoning` item | `thinking` block |
| Command run | `commandExecution` item | `tool_use` named `Bash`, with its result |
| File change | `fileChange` item; `turn/diff/updated` | `tool_use` named `Edit` or `Write`, with its result |
| Search | `commandExecution` whose `commandActions` say `search` or `read` | `tool_use` named `Read`, `Grep`, `Glob`, `WebSearch` |
| Tool call | Other item types, such as MCP tool calls | Any other `tool_use` |
| Approval request | `item/*/requestApproval` | `canUseTool` for any tool except a question |
| Question | `agentMessage.questions`; `item/tool/requestUserInput` | `canUseTool` for `AskUserQuestion` |
| Subagent | Not seen | `tool_use` named `Agent`, with `task_*` messages |
| Plan | Not seen; `turn/plan/updated` exists | Not seen |
| Notice | `sleep` item; `thread/compacted`; `model/rerouted` | `system / status`, `compact_boundary`, `api_retry` |
| Error | `turn/completed` with an error; a failed item | `result` with an error subtype |
| Turn | `turn/started`, `turn/completed` | `system / init`, `result` |
| **No home yet** | | Background tasks: `task_started`, `task_progress`, `task_updated`, `task_notification`, `background_tasks_changed` |
| Not sent | Text deltas; `userMessage` echo; thread status; token usage; rate limits; moderation metadata; MCP start-up; account; settings | `stream_event`; `thinking_tokens`; `rate_limit_event`; `tool_progress`; `command_lifecycle`; title, version-control and code-change notices |

## What this means for the design

These are findings. The proposals that follow from them are in
[`design/event-model.md`](../design/event-model.md#checked-against-real-sessions).

1. **The item model fits.** Both harnesses give each piece of work a stable id and report it at least twice.
   An event that replaces the earlier state of the same item matches both.
2. **Background work needs a home.** It is common in real use and has no item in the proposed list.
3. **A choice cannot be assumed to be a string.** Codex offers an object as a decision. The driver has to give
   each choice its own id and keep the native value itself.
4. **A harness's list of choices may be incomplete.** Codex accepted a decision it had not listed.
5. **A question is not always a request.** Codex's can be a field on a message, answered by an ordinary reply.
6. **A question can be several questions, and can allow several selections.**
7. **Steering does not mean the same thing everywhere.** One harness folds the message into the turn; the other
   ends the turn.
8. **A turn is not always started by a person.** Background work can produce a turn of its own.
9. **An interrupt can leave items unfinished.** The driver must close them.
10. **Half of all traffic is partial text, and much of the rest is bookkeeping.** A driver needs a clear rule
    for what is never sent.
11. **Real turns are large.** About 12 tool calls and 7 reasoning blocks per turn in one developer's use.
12. **Claude Code does not say what kind of tool a call is.** The driver decides from the tool's name.
