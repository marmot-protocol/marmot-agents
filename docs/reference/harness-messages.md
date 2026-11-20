# Reference: what Codex and Claude Code actually send

This page is detailed on purpose. It records what two harnesses send over their native interfaces, taken from
real sessions, and how each message relates to the proposed [event model](../design/event-model.md).

## Sources

| Source | What it is | How it was used |
| --- | --- | --- |
| [`fixtures/recordings`](../../fixtures/recordings/README.md) | 57 scripted sessions, 28 with Codex and 29 with Claude Code, recorded on 2026-11-17 and 2026-11-18, and six more Codex sessions with subagents, recorded on 2026-11-20 | Read in full. Every claim on this page about order, fields and behaviour comes from these |
| [`data/claude-sdk-message-structures.txt`](data/claude-sdk-message-structures.txt) | The structure of 176,652 Claude Agent SDK messages from four days of real use on one developer's machine | Counts and field names only. No content was read or kept |
| Codex's generated protocol schema | `codex app-server generate-json-schema`, codex-cli 0.169.0 | The list of methods |
| [`tools/prototype/translate.py`](../../tools/prototype/translate.py) | A prototype that turns every recording into proposed agent events | To check that nothing is left without a home |

Limits of this evidence:

- The scripted sessions use small models, short prompts and a three-file project. They show the shape of each
  message, not typical volume.
- The real-use counts come from one person doing long, autonomous coding sessions through T3 Code, with
  approvals switched off. They show volume and variety, but no approvals.
- Only Codex and Claude Code are covered. Each recording is one harness version on one day.

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

### What appeared in the recordings

1,750 messages in 24 sessions.

| Group | Messages | Share |
| --- | --- | --- |
| Text and output deltas | 833 | 48% |
| Bookkeeping: thread status, token usage, rate limits, moderation metadata, MCP start-up, account, settings | 425 | 24% |
| Items starting and completing | 211 | 12% |
| Client requests and all responses | 204 | 12% |
| Turns starting and completing, diff updates | 65 | 4% |
| Requests to the client: approvals and questions | 7 | under 1% |

### Items

Everything the agent does is an **item** with a stable `id`. It arrives as `item/started` and later as
`item/completed` with the same id.

| Item type | Recorded in | Fields that matter |
| --- | --- | --- |
| `userMessage` | Every turn | An echo of the prompt. A steering message appears as one too |
| `agentMessage` | Almost every turn | `text`; `phase` is `commentary` or `final_answer`; `questions`, when present, is a list of `{title, options}` |
| `reasoning` | 3 sessions | `summary` and `content`. **Both were empty every time**, including at high effort with detailed summaries requested |
| `commandExecution` | 11 sessions | `command`, `cwd`, `status` (`completed`, `failed`, `declined`), `exitCode`, `durationMs`, `aggregatedOutput`, and `commandActions`, which classifies the command as `read`, `search` or `unknown` |
| `fileChange` | 2 sessions | `changes`: a list of `{path, kind, diff}`; `status` |
| `mcpToolCall` | `mcp_tool` | `server`, `tool`, `arguments`, `status`, `result`, `error` |
| `webSearch` | `web_search` | `query`, `action`, `results` |
| `plan` | `plan_mode` | `text`: the plan as Markdown |
| `contextCompaction` | `compact` | Start and end only |
| `sleep` | `question` | `durationMs`. The agent waiting for something |

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

### Three shapes of approval

| What needs approval | Request | Choices offered | Answer |
| --- | --- | --- | --- |
| A command | `item/commandExecution/requestApproval`, naming the item | `availableDecisions`: `accept`, an object `{acceptWithExecpolicyAmendment: {...}}`, `cancel` | `{decision: ...}` |
| A file change | `item/fileChange/requestApproval`, naming the item | None listed | `{decision: ...}` |
| An MCP tool call | `mcpServer/elicitation/request`, marked `codex_approval_kind: "mcp_tool_call"` | None listed; the request says the choice can persist for the session or always | `{action: "accept" or "decline", content: {}}` |

- **A decision is not always a plain string.** One of the offered decisions is an object.
- **The list is not complete.** `decline` was not in it, and answering `decline` worked: the item completed
  with status `declined`.
- **An MCP tool call always needs approval.** With the approval policy set to never ask, Codex refused the call
  outright: "MCP tool call requires approval, but approval policy is never".

### Two shapes of question

- **A request, in plan mode.** `item/tool/requestUserInput` carries a list of questions. Each has an `id`, a
  `header`, the `question`, `options` with a `label` and a `description`, a flag `isOther` for a free-text
  answer, and a flag **`isSecret`**. The answer is `{answers: {<question id>: {answers: [<label>]}}}`.
- **A field on a message, in default mode.** Asked to put a multiple-choice question, Codex sent an
  `agentMessage` whose `questions` field held the title and the options. It then ran three 60-second `sleep`
  items and ended the turn saying it was still waiting. Nothing asked the client for an answer. The answer to
  such a question is simply the user's next message.

### Controls

| Control | What happened |
| --- | --- |
| Interrupt | `turn/interrupt` returned at once, and `turn/completed` arrived with status `interrupted`. **The running command never got an `item/completed`.** |
| Steer | `turn/steer` returned at once. The new message appeared as a `userMessage` item inside the same turn, and the agent took it into account before finishing |
| Second turn | `turn/start` on the same thread |
| Resume | After the app server was restarted, `thread/resume` with the thread id; the next turn remembered the earlier one |
| Compact | `thread/compact/start` ran as **a turn of its own**, with one `contextCompaction` item and no user message |
| List models | `model/list` returned eight models, each with a display name, a description and the reasoning efforts it supports |
| Switch model | `model` on the next `turn/start`; the thread's settings then showed the new model |
| Plan mode | `collaborationMode: {mode: "plan", ...}` on `turn/start`. The agent asked its question, then produced a `plan` item and edited nothing |
| List modes | `collaborationMode/list` returned two: `default` and `plan` |
| Change a setting between turns | `thread/settings/update` with the new model returned `{}`, and `thread/settings/updated` followed within a millisecond, with no prompt sent. The same call changed the approval policy and the sandbox, and the next command then asked for approval |
| Change a setting during a turn | `turn/settings/update` was refused: "turn settings updates require the step_model_switching feature" |
| New session | A second `thread/start`. The new thread did not remember the first. `thread/unsubscribe` on the first returned `{"status": "unsubscribed"}`, and no message said the thread was over |
| Resume, with the settings sent again | `thread/resume` with `approvalPolicy` and `sandbox`. The thread came back with the sandbox it started with |

**A resumed thread does not keep its sandbox.** In `resume` and `restart_mid_turn` the thread was started with
a sandbox limited to the project, and `thread/resume` was sent without one. The thread came back with full
access, which is the default set in the Codex configuration of the recording machine. In
`resume_with_settings` the same settings were sent with the resume, and they held.

The model list had ten models on 2026-11-18 and eight the day before.

### An approval nobody answers

In `approval_unanswered` the client did not answer a command approval. Codex set the thread's status to
`active` with the flag `waitingOnApproval`, and then sent nothing at all for six minutes. It did not time out,
remind or give up. When the client interrupted, the turn completed as `interrupted` and the request was marked
resolved.

### Killed in the middle of a turn

In `restart_mid_turn` the app server was killed while a command was running, and a new one was started.
`thread/resume` returned the thread as idle, with the earlier turn marked `interrupted`. That turn's history held
the user's message and the agent's first message. **The command that was running was not in it.** Nothing was
run again. Asked what had happened, the agent said the command had been running when it was stopped and that it
could not tell whether it finished.

### Failures

A turn started with a model that does not exist produced a `warning` notification, then an `error`
notification with a code (`modelNotFound`), then `turn/completed` with status `failed` and the same error. The
`turn/start` request itself succeeded.

### Things that did not happen

- **No to-do plan.** Told twice to use its plan tool in default mode, Codex did the work without one. The schema
  has `turn/plan/updated`; it was never sent.
- **No reasoning text**, as noted above.

### Subagents

Recorded on 2026-11-20 with codex-cli 0.169.0, in
[`fixtures/recordings/codex/subagents`](../../fixtures/recordings/codex/subagents): six scenarios, each prompt
asking for subagents in so many words. Each recording starts with `model/list`, whose reply gives each model's
`multiAgentVersion`: `gpt-5.6-luna` is `v1`, and the nine other models offered are `v2`. Five scenarios ran on
`gpt-6.1-sol` (`v2`) and one, `subagent_v1`, on `gpt-5.6-luna`.

**A subagent is a thread of its own.** Its turns, items, approvals and token usage arrive on the same
connection, each naming the subagent's `threadId`. No `thread/started` is sent for it in any recording. Two
things link it to its parent:

- **An item in the parent's turn names the subagent's thread**: `agentThreadId` on `subAgentActivity` (`v2`),
  or `receiverThreadIds` on `collabAgentToolCall` (`v1`).
- **Every turn carries `rootTurnId`** inside `turn`. On a top-level turn it is the turn's own id. On a
  subagent's turn, at any depth, it is the id of the top-level turn the person started. It still names that
  turn after the turn was interrupted.

**How the parent reports it differs by runtime.**

| | `v2` (`gpt-6.1-sol`) | `v1` (`gpt-5.6-luna`) |
| --- | --- | --- |
| Starting one | A `subAgentActivity` item with `kind: started`, `agentThreadId`, `agentPath` (such as `/root/count_lines`), `model` and `reasoningEffort` | A `collabAgentToolCall` with `tool: spawnAgent` and the `prompt`. The subagent's thread id is in `receiverThreadIds` only when the item completes |
| What it was asked | Not sent anywhere | In the `spawnAgent` item, and again as a `userMessage` item on the subagent's thread |
| Waiting for it | `collabAgentToolCall`, `tool: wait`, with no ids and no states | `collabAgentToolCall`, `tool: wait`, naming the subagent and, on completion, its `status` |
| Its end | A `subAgentActivity` item with `kind: completed`. In every recording it was sent in the same millisecond as the subagent's `turn/completed` | Only the `wait` item |
| More work for it later | A `subAgentActivity` item with `kind: interacted`; the subagent's thread gets a new turn | Not recorded |

`model` and `reasoningEffort` on `subAgentActivity` are set when a subagent starts and `null` after.

**Order.** A subagent's first message can come before the item that names it. In `subagent_v1` the subagent's
`thread/status/changed` arrived just before the `spawnAgent` item completed, which is the first message to give
its thread id. With `v2`, `item/started` for `subAgentActivity` already names the thread, and the status change
can fall between its `started` and `completed`. A router that drops messages for a thread it does not know yet
loses that first status.

**Nested subagents.** In `subagent_nested`, the subagent's own thread sends a `subAgentActivity` that starts a
third thread, and its `agentPath` extends its parent's (`/root/find_test/read_test`). No depth field is sent.

**Approvals.** In `subagent_approval`, the command approval came from the subagent's thread and turn, as an
ordinary server request on the same connection, and was answered by its id. Nothing marks it as a subagent's
except the thread it names.

**Interrupting the parent does not stop the subagent.** In `subagent_interrupt`, the client interrupted the
parent's turn while the subagent was running `sleep 60`. The parent's turn ended as `interrupted` at once. Its
`wait` item was never completed. The subagent carried on: its command finished about a minute later, its turn
completed, and the parent's thread then sent `subAgentActivity` `completed`, after the parent's turn had ended.
A client that wants Stop to stop everything has to interrupt each subagent's turn itself.

**A subagent needs a parent Codex keeps.** In a first attempt, not kept, the parent threads were throwaway
(`ephemeral: true`). Most `v2` runs failed: the agent reported a "thread-store error", "no rollout found". With
`ephemeral: false`, all six worked. Inferred: a `v2` subagent's thread is stored beside its parent's, so the
parent must be stored.

**Compared with T3 Code.** T3 Code's recordings of 0.156.1, read in its repository
(`apps/server/src/orchestration-v2/testkit/fixtures/subagent*`) on 2026-11-20, agree on all of the above that
they cover, with one difference: their `subAgentActivity` items have no `model` or `reasoningEffort`.

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
tool's name. A tool on an MCP server is named `mcp__<server>__<tool>`.

### Thinking is empty unless asked for

In the first recordings every `thinking` block arrived with no text, only a signature. With the option
`thinking: {type: "adaptive", display: "summarized"}`, the text arrives. In the real-use data, which had that
option on, 9,667 of 9,670 thinking blocks had text.

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
  assistant   [text]
  result / success
```

- The request carries the tool name, its input, a `toolUseID`, a `requestId`, and sometimes `suggestions`:
  permission rules the user could add so as not to be asked again.
- The answer is `allow`, optionally with changed input, or `deny` with a message.
- **There is no list of choices.** The choices are fixed by the SDK: allow, deny, and whatever the suggestions
  offer.

### Questions and plans are tool calls answered through the approval callback

- **A question.** The agent calls a tool named `AskUserQuestion`. Its input is a list of questions, each with a
  `header`, `options` that have a `label` and a `description`, and a `multiSelect` flag. It arrives through
  `canUseTool` like any approval. The answer is `allow` with the same input plus an `answers` object keyed by
  the question text. So one request can hold several questions, and a question can allow several selections.
- **A plan.** In plan mode the agent calls a tool named `ExitPlanMode` whose input holds the plan as text. It
  also arrives through `canUseTool`. Allowing it lets the agent start work; in the recording it then edited the
  file, asking approval for each step.

### Controls

| Control | What happened |
| --- | --- |
| Interrupt | `interrupt()` returned `{still_queued: []}`. A `result / error_during_execution` followed, and `query()` threw |
| Steer | A user message with `priority: "now"` was sent mid-turn. **The running turn ended with a result, and a new turn began** with a fresh `system / init` |
| Second turn | Another user message on the same input stream |
| Resume | A second `query()` with `resume: <session id>`; it remembered the earlier turn |
| Compact | The user message `/compact`. `system / status` said `compacting`, then a `compact_boundary` with token counts, then a `user` message holding the summary written for the model, then a result |
| List commands | `supportedCommands()` returned 59 commands, each with a name, a description and an argument hint |
| Run a command | The user message `/context`. Its output came back as an `assistant` text block whose model is `<synthetic>` |
| Switch model | `setModel("sonnet")`. A `user` message wrapped in `<local-command-stdout>` confirmed it, and the next reply came from the new model |
| Plan mode | `permissionMode: "plan"` |
| Change the permission mode | `setPermissionMode("acceptEdits")` between two turns. A `system / status` message carried the new mode, and the next `system / init` did too. The first edit had asked for approval; the second did not |
| New session | A second `query()` without `resume`. It got a new session id and did not remember the first. No message said the first session was over |
| Ask what the session offers, before any prompt | `initializationResult()` answered in under two seconds, with no prompt sent: 57 commands with descriptions and hints, 14 models, and the current permission mode. `supportedModels()` and `supportedCommands()` returned the same lists. **No `system / init` arrived in the five seconds before the first prompt**, so the session id and the full model name are known only once a turn starts |

`initializationResult()` also names the signed-in account. The recorder leaves that out.

### An approval nobody answers

In `approval_unanswered` the `canUseTool` callback was left unanswered. Claude Code sent nothing for six
minutes. It did not time out or withdraw the request. When the client interrupted, the tool call was reported
as rejected and the turn ended with an error result.

### Stopped in the middle of a turn

In `restart_mid_turn` the harness process was stopped and the session resumed with a second `query()`. Claude
Code had already moved the command to the background and ended its turn, so what was cut short was a background
task. On resume it sent a `task_notification` saying the task had been stopped, then an empty turn (`init`
followed at once by a result), and then answered the new prompt. Nothing was run again.

### Failures

A session started with a model that does not exist produced an `assistant` text block from the model
`<synthetic>` and then a `result` whose subtype is `success` but whose `is_error` is true. **The subtype alone
does not say whether a turn failed.**

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
| Commands, file changes, searches | Their own item types | A generic tool call; the type must be inferred from the tool's name |
| Approval request | A JSON-RPC request, in three shapes | A callback with the tool name and input |
| Choices | A list that can hold objects and can be incomplete, or none | Fixed: allow or deny, plus suggested rules |
| Question | A request in plan mode; a field on a message otherwise | A tool call answered through the approval callback |
| Secret answers | A question can be marked secret | Not seen |
| Plan | A `plan` item with text, in plan mode | A tool call holding the plan, answered through the approval callback |
| Reasoning text | Not available | Available when asked for |
| Steering | Stays inside the running turn | Ends the turn and starts another |
| Interrupt | The turn completes as `interrupted`; open items are left open | The turn ends with an error result |
| An approval nobody answers | Waits, with no limit seen in six minutes | Waits, with no limit seen in six minutes |
| Stopped mid-turn, then resumed | The turn shows as `interrupted`; the running command is gone from its history | A stopped background task is reported; nothing is rerun |
| Work after the turn ends | Not seen | Yes: background tasks and subagents |
| Compaction | A turn of its own, started by the client | Part of the turn that ran `/compact` |
| End of turn | `turn/completed` with a status | `result`; failure is in `is_error`, not the subtype |
| What a session offers, before any prompt | The response to `thread/start` has the settings. Models and modes when asked | Commands, models and the permission mode when asked. No session id until the first turn |
| Change a setting | `thread/settings/update`, confirmed at once. Or a field on the next prompt | `setModel()`, `setPermissionMode()` |
| Values a setting accepts | Models and modes are listed. Approval policies and sandboxes are not | Models are listed. Permission modes are not |
| A session ends | Nothing says so | Nothing says so |
| Resume | Does not keep the sandbox unless it is sent again | Not tested: both recordings that resume sent the permission mode again |
| Usage | `thread/tokenUsage/updated` during the turn | On `result`, with cost |
| Slash commands | None in this interface | A list from the SDK; run as ordinary user messages |
| Models | `model/list`; chosen per turn | `setModel()` between turns |

## How the messages map to the proposed items

The prototype translator applies this table to every recording: 2,157 native messages become 399 agent events,
and no native message is left without a rule. "Not sent" means the driver uses it or drops it.

| Proposed item | Codex | Claude Code |
| --- | --- | --- |
| `turn` | `turn/started`, `turn/completed` | `system / init`, `result` |
| `message` | `agentMessage` item | `text` block; local command output from `<synthetic>` |
| `reasoning` | `reasoning` item, when it has text | `thinking` block, when it has text |
| `tool`, kind `command` | `commandExecution` item | `tool_use` named `Bash` |
| `tool`, kind `edit` | `fileChange` item | `tool_use` named `Edit`, `Write` |
| `tool`, kind `read` or `search` | `commandExecution` whose `commandActions` say so; `webSearch` item | `tool_use` named `Read`, `Grep`, `Glob`, `WebSearch` |
| `tool`, kind `mcp` | `mcpToolCall` item | `tool_use` named `mcp__...` |
| `tool`, kind `subagent` | Not seen | `tool_use` named `Agent` |
| `approval` | The three approval requests | `canUseTool`, except for questions |
| `question` | `item/tool/requestUserInput`; `agentMessage.questions` | `canUseTool` for `AskUserQuestion` |
| `plan` | `plan` item | `canUseTool` for `ExitPlanMode` |
| `task` | Not seen | `system / task_*` |
| `notice` | `sleep`, `contextCompaction`, `warning`, `model/rerouted` | `system / status` while compacting, `compact_boundary`, `api_retry`, `<local-command-stdout>` |
| `error` | `error` notification; an error response | A `result` with `is_error` becomes a failed turn |
| Not sent | Deltas; `userMessage` echo; thread status; token usage; rate limits; moderation metadata; MCP start-up; account; settings; `serverRequest/resolved`; `turn/diff/updated` | `stream_event`; `thinking_tokens`; `rate_limit_event`; `tool_progress`; `command_lifecycle`; `background_tasks_changed`; the compaction summary; title, version-control and code-change notices |

## What this means for the design

These are findings. The proposals that follow from them are in
[`design/event-model.md`](../design/event-model.md#checked-against-real-sessions).

1. **The item model fits.** Both harnesses give each piece of work a stable id and report it at least twice.
   An event that replaces the earlier state of the same item matches both.
2. **Background work needs a home.** It is common in real use and was missing from the first list of items.
3. **A choice cannot be assumed to be a string.** Codex offers an object as a decision. The driver has to give
   each choice its own id and keep the native value itself.
4. **A harness's list of choices may be incomplete or absent.** Codex accepted a decision it had not listed,
   and two of its three approval requests list none.
5. **A question is not always a request.** Codex's can be a field on a message, answered by an ordinary reply.
6. **A question can be several questions, can allow several selections, and can be secret.** A secret answer
   must not be shown to a group.
7. **Questions and plans can arrive dressed as tool calls.** The driver has to recognise them, or the chat
   shows the same thing twice.
8. **Steering does not mean the same thing everywhere.** One harness folds the message into the turn; the other
   ends the turn.
9. **A turn is not always started by a person.** Background work, or a control such as compaction, can produce
   a turn of its own.
10. **An interrupt can leave items unfinished.** The driver must close them.
11. **Whether a turn failed is not always where you would look.** Claude Code reports a failure with subtype
    `success`.
12. **Reasoning is not always available.** Codex gave none. Claude Code gives it only when asked.
13. **Half of all traffic is partial text, and much of the rest is bookkeeping.** A driver needs a clear rule
    for what is never sent.
14. **Real turns are large.** About 12 tool calls and 7 reasoning blocks per turn in one developer's use.
15. **Claude Code does not say what kind of tool a call is.** The driver decides from the tool's name.
16. **Codex and Claude Code wait for an approval without limit.** Hermes expires one after five minutes. So
    under [D-021](../decisions.md#d-021-approval-behaviour-belongs-to-the-harness), an unanswered request in a
    coding harness holds its turn until someone answers or interrupts. The chat has to show plainly that the
    agent is waiting.
17. **A harness that stops mid-turn leaves the turn for the driver to close.** No message says the turn ended.
    Both harnesses resume the session afterwards without rerunning anything.
