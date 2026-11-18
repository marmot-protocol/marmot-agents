# Reference: what OpenClaw hands a channel plugin

This page is detailed on purpose. It records what the OpenClaw agent runtime gives a channel plugin, taken from
real sessions, and what that means for the OpenClaw connector and the socket protocol.

OpenClaw is a gateway, like Hermes: an agent runtime with its own chats, users and sessions, where a channel
plugin is how a chat network plugs in. Hermes is covered in [`hermes-messages.md`](hermes-messages.md), and
the coding harnesses in [`harness-messages.md`](harness-messages.md).

## Sources

| Source | What it is |
| --- | --- |
| [`fixtures/recordings/openclaw`](../../fixtures/recordings/README.md#openclaw) | 26 scripted sessions, recorded on 2026-11-17. 12 more, recorded on 2026-11-18, are covered in [Controls and sessions](#controls-and-sessions) only |
| OpenClaw | Release 2026.11.5 (build `e13cf15`), the newest stable release on that day |
| Model | `z-ai/glm-5.3-flash:free` through an OpenAI-compatible endpoint |
| Recorder | [`tools/record/openclaw`](../../tools/record/README.md#openclaw): a channel plugin that plays a scripted chat and records every call in both directions |

Limits of this evidence:

- One OpenClaw version, one model, one run of each scenario. Treat each finding as a lead to confirm, not a law.
- **The recorder asks for everything.** It registers every progress callback OpenClaw offers, and its container
  switches on tool summaries, reasoning and block streaming, which a fresh OpenClaw leaves off. One scenario,
  `tool_call_plain`, records the opposite: no callbacks and the defaults.
- **In OpenClaw the plugin drives the receive path.** The recorder is one way to write it. Where a finding
  depends on a choice the recorder made, this page says so.
- The model is the free route of the model used for Hermes. The plain model id began refusing every request
  part-way through the work. `model_refused` records that refusal on purpose.
- Not exercised: media, polls, threads, reactions from users, scheduled jobs and heartbeats, subagents,
  compaction, plans, plugin approvals, approval expiry, several accounts, the pairing code exchange, native
  command menus and live draft previews.

## How a channel plugs in

A plugin is a package with a manifest. It registers a channel: a bundle of optional parts called adapters, for
configuration, sending, message actions, approvals, prompt hints, and starting an account.

OpenClaw starts an account by calling `gateway.startAccount`. The call carries a **channel runtime**: the
plugin's handle on OpenClaw. From then on the plugin does the receiving. For each chat message the recorder
made four calls:

| Call | What comes back |
| --- | --- |
| `routing.resolveAgentRoute` | Which agent and which session this chat belongs to |
| `inbound.ingress.resolveStable` | Whether this sender may run the agent here, with a reason |
| `inbound.buildContext` | The message as OpenClaw will see it: text, sender, chat, reply, command rights |
| `inbound.run` | Runs the turn. It finishes when the turn does |

This is the first difference from Hermes. Hermes takes one event and decides everything inside. OpenClaw makes
the decisions, but **the plugin asks for each one and acts on it**.

Everything comes back two ways: as callbacks handed to `inbound.run`, and as calls on the plugin's adapters.

## What OpenClaw called

1,655 calls in 26 sessions. The plugin made 162 calls of its own for 41 scripted messages and 4 presses.

| Call | Count | What it is |
| --- | --- | --- |
| `inbound.log` | 304 | Progress of the receive path: ingest, assemble, record, dispatch, finalize |
| `dispatcher.onReplyStart` | 225 | "The agent is working", repeated every six seconds |
| `messaging.normalizeTarget`, other `messaging.*` | 177 | Asks the plugin to tidy or check a chat address |
| `deliver` | 86 | A message to post in the chat. See below |
| `dispatcher.onDeliverySettled`, `onIdle`, `onSettled`, `onCleanup` | 232 | Delivery bookkeeping |
| `onPartialReply` | 54 | The reply so far, while the model is still writing |
| `onItemEvent` | 53 | A tool started, changed or ended |
| `onToolResult` | 51 | A formatted line about a tool |
| `onAssistantMessageStart` | 48 | The model began a message |
| `onModelSelected` | 35 | The provider, model and thinking level for the turn |
| `groups.resolveRequireMention` | 34 | Asks whether this group needs a mention |
| `actions.describeMessageTool`, `agentPrompt.messageToolHints` | 64 | Asks what the channel can do, to tell the agent |
| `onTypingController` | 33 | Hands over an object for controlling the typing signal |
| `onPreparedAgentRunStart` | 30 | A run began, with its run id and session |
| `onAgentRunTerminalOutcome` | 29 | A run ended: `completed` or `failed` |
| `onToolStart` | 27 | A tool call, with its name and arguments |
| `onObservedReplyDelivery`, `onBlockReplyQueued` | 53 | Delivery bookkeeping |
| `gateway.startAccount`, `gateway.abort` | 26 each | The account started; OpenClaw asked it to stop |
| `onCommandOutput` | 18 | Output, exit code and duration of a command |
| `onReasoningEnd` | 12 | The model finished thinking |
| `message.send.payload` | 10 | A message OpenClaw sends on its own, outside a turn |
| `actions.supportsAction`, `actions.handleAction` | 11 | The agent used the message tool |
| `approvalCapability.*` | 8 | Approval hooks, when the plugin claims them |
| `onReasoningStream` | 5 | The model's thinking so far |
| `onApprovalEvent` | 4 | An approval was denied or failed |

Registered and never called: `onAgentRunStart`, `onFollowupQueueDisposition`, `onVisibleWorkSessions`,
`onReplyStart`, `onTypingCleanup`, `onReasoningProgress`, `onNarrationUpdate`, `onPlanUpdate`, `onPatchSummary`,
`onCompactionStart`, `onCompactionEnd`, `onQueuedFollowupAdmitted`, `onQueuedFollowupSettled`, the dispatcher's
`onError` and `onSkip`, the three typing callbacks, `message.send.text`, `message.send.media`, the heartbeat
typing calls, the pairing notice and the security lookups.

## Replies arrive through one callback, in three kinds

`deliver(payload, info)` is the one callback a plugin must supply. `info.kind` says what the payload is.

| Kind | Count | What it carried |
| --- | --- | --- |
| `tool` | 51 | 25 lines for a tool starting, 24 for a tool ending with its output, 2 questions |
| `block` | 26 | A finished block of the agent's reply. Every agent answer but one arrived this way |
| `final` | 9 | 3 acknowledgements of `/approve`, 2 command outputs, 2 errors, 1 stop notice, and 1 agent answer |

**With block streaming on, the agent's answer never arrives as `final`.** The one `final` that is an agent
answer comes from `tool_call_plain`, which switches block streaming off. The Marmot plugin in MDK passes on
`final` only and disables block streaming (read in its source), which is consistent with this.

The payload is an object, not a string. Fields seen:

| Field | Seen on |
| --- | --- |
| `text` | Every delivery |
| `isError` | The two model errors |
| `presentation` | The two questions and the `/status` output: blocks of text, buttons and a table |
| `presentationTextMode` | With `presentation`; the value was `fallback`, meaning `text` holds the same content as plain text |
| `channelData` | The two questions: the question id and the option values |
| `audioAsVoice`, `replyToTag`, `replyToCurrent` | Set to false on replies |

For a `block`, and for the one `final` that is an agent answer, `info` also carries `assistantMessageIndex`,
the position of the model message it came from. None of the eight `final` deliveries that OpenClaw wrote
itself carries it.

## Tool activity is structured

This is the largest difference from Hermes, which gives a platform formatted lines and nothing else. In
`tool_call` the agent ran one command. The plugin was told, in order:

```text
onItemEvent     kind tool, name exec, phase start, status running, title "Exec print text", toolCallId …
onToolStart     name exec, phase start, args {"command": "echo recorder-demo"}, toolCallId …
deliver (tool)  `print text`
onCommandOutput phase delta, output "recorder-demo", status running
onItemEvent     phase end, status completed
onCommandOutput phase end, output "recorder-demo", status completed, exitCode 0, durationMs 410, cwd …
deliver (tool)  `print text` followed by the output in a code block
```

| Callback | Fields |
| --- | --- |
| `onToolStart` | `toolCallId`, `name`, `phase` (`start`, `update`), `args`: the arguments the model gave |
| `onItemEvent` | `itemId`, `toolCallId`, `kind`, `name`, `title`, `meta`, `phase` (`start`, `update`, `end`), `status` (`running`, `completed`, `failed`, `blocked`), `commandBearing` |
| `onCommandOutput` | `toolCallId`, `name`, `phase` (`delta`, `end`), `status`, `output`, `exitCode`, `durationMs`, `cwd` |
| `onToolResult` | A payload like those of `deliver`: the same formatted line |
| `onApprovalEvent` | `phase` (`resolved`), `kind` (`exec`), `status` (`denied`, `failed`), `toolCallId`, `message` |

Tools seen through `onToolStart`: `exec` 11 times, `message` 5, `ask_user` 4, `dispatch_action` 4, `edit` 2 and
`write` 1. `dispatch_action` is a wrapper: in the question scenarios each `ask_user` call arrived twice, once as
`dispatch_action` with the real tool inside its arguments and once as `ask_user`.

For tools that are not commands there is no result callback with fields. The result arrives only as text, in
`onToolResult` and in the `tool` delivery. In `reply` the agent wrote a file: `onToolStart` carried the path
and the content, and the outcome came as the line `Successfully wrote 48 bytes to …`.

**The same activity arrives three times**: as structured callbacks, as `onToolResult`, and as `tool`
deliveries. A plugin must pick one.

**None of this arrives unless the plugin asks.** `tool_call_plain` makes the same request with no callbacks
registered and OpenClaw's defaults. The plugin received exactly one call that mattered: `deliver` with the
answer, kind `final`.

## Text while it is being written, and thinking

- `onPartialReply` carries the reply so far, growing: `R`, then `Ran it — the output was: recorder-demo`.
  54 calls in 19 recordings.
- `onReasoningStream` carries the model's thinking so far, in the same way. It was called only in `reasoning`,
  which sets reasoning to `stream`. With reasoning set to `on`, as in every other scenario, the plugin got
  `onReasoningEnd` and no thinking text. Why `on` produced none is not established.

## Sessions: one for each group, and one for all direct chats

OpenClaw names the session when the plugin asks for a route. Two names were seen:

```text
agent:main:recorder:group:family     37 times, for every message in the group
agent:main:main                       4 times, for every direct message
```

- **A group is one session, whoever writes.** In `two_senders` Alice told the agent her favourite animal and
  Bob asked what it was. The answer came in four seconds. Hermes, with a session for each sender, took six
  minutes.
- **All direct chats share one session by default.** The route result says `dmScope: "main"`. In
  `two_direct_messages` Alice and Bob wrote from separate direct chats. Both were routed to `agent:main:main`,
  with the same session id, and Bob's turn started at sequence 7 of the same transcript. Asked what the last
  person had said, the agent refused: "I can't share what someone else told me in their own conversation".
  That refusal was the model's choice, not a barrier in OpenClaw.

The plugin chooses what it reports as a direct chat. The Marmot plugin in MDK reports every conversation as a
group, so each gets its own session (read in its source, not recorded here).

## Who sent it, and where

OpenClaw knows only what the plugin puts in the context. The recorder supplied the sender's id and name, the
chat's id, kind and label, the message id, and the id of the message replied to. `inbound.buildContext`
turned that into the fields OpenClaw uses: `SenderId`, `SenderName`, `From`, `To`, `ChatType`, `ChatId`,
`ConversationLabel`, `GroupSubject`, `MessageSid`, `ReplyToId`, `WasMentioned`, `CommandAuthorized`.

One detail found while building the recorder, not in a committed recording: for a group, `from` must name the
group, in the form `<channel>:group:<id>`. When the recorder passed the sender there, OpenClaw asked the
plugin about a group named after the sender.

## A reply works from the id alone

In `reply` Alice sent three messages, each with a code. Bob replied to the middle one and asked which code it
held. The plugin supplied only the id of the message replied to. The agent answered with the right code.
The recording cannot show how OpenClaw told the model which message was meant; that it puts message ids in
the prompt is inferred.

In `reply_quoted` the plugin also supplied the quoted text and its author, in a chat with no history. The
context then carried `ReplyToBody`, `ReplyToSender` and `ReplyToIsQuote`, and the agent answered from the
quote.

Hermes needed the quote; with the id alone it had to search past sessions.

## OpenClaw decides who may speak, and the plugin acts on it

The plugin hands the resolver the sender, the chat, and the channel's settings: a policy for direct chats, a
policy for groups, and the allow-lists. The result has an `admission`, a `decision`, a `reasonCode` and the
gate that decided.

| Scenario | Admission | Decision | Reason |
| --- | --- | --- | --- |
| 38 messages that ran | `dispatch` | `allow` | `activation_allowed` |
| `not_allowed`: Mallory, with only Alice on the group's list | `drop` | `block` | `group_policy_not_allowlisted` |
| `pairing`: Mallory in a direct chat, under the pairing policy | `pairing-required` | `pairing` | `dm_policy_pairing_required` |
| `not_mentioned`: Alice, in a group that requires a mention | `skip` | `allow` | `activation_skipped` |

In the three cases that did not run, OpenClaw did nothing further. No reply, no notice, no call on the plugin.
What happens next is the plugin's choice, and the recorder chose to do nothing. Alice's next message in
`not_allowed` and `not_mentioned` was answered normally.

So a disallowed sender is silent here as with Hermes, but for a different reason. Hermes drops the message
inside itself. OpenClaw returns a verdict with a reason, and the plugin could tell the person.

The same result says whether the sender may use commands (`commandAccess.authorized`). The plugin passes that
on as `CommandAuthorized`.

## Approvals

OpenClaw can ask before running a command. The scenarios set its command mode to `ask`, which asks about any
command not on its allow-list.

**By default nothing reaches the chat.** In `approval_not_forwarded` the command waited. The plugin was told
the tool had started and then nothing: no prompt, no approval call. The command was still waiting when the
recording ended 60 seconds later; it then failed with `approval-request-failed`.

**With forwarding switched on, the prompt arrives as a message.** The owner sets `approvals.exec` in
OpenClaw's configuration to forward approvals to the chat they came from. The plugin then receives
`message.send.payload` with the prompt as text and this beside it:

| Field of `channelData.execApproval` | Value in the recording |
| --- | --- |
| `approvalId` | An id for this one approval |
| `approvalSlug` | Its first eight characters |
| `approvalKind` | `exec` |
| `agentId`, `sessionKey` | The agent and session it belongs to |
| `allowedDecisions` | `allow-once`, `allow-always`, `deny` |
| `state` | `pending`, and later `resolved` |

The text says `Reply with: /approve <id> allow-once|allow-always|deny`, and that it expires in 30 minutes.

**It can be answered as a chat message.** In `approval_allowed` and `approval_denied` Alice sent
`/approve <id> allow-once` or `deny`. Within a tenth of a second OpenClaw sent `✅ Exec approval allowed once.
Resolved by Chat approval (recorder:alice).` Its reply to the command itself, `✅ Approval allow-once submitted
for <id>.`, came last: 12 to 13 ms after the waiting turn had ended, in all three recordings that use
`/approve`.

**Or by a press, if the plugin claims an approval capability.** In `approval_cards_allowed` and
`approval_cards_denied` the plugin claims one. OpenClaw then:

1. Calls `approvalCapability.render.exec.buildPendingPayload` with the whole request, so the plugin can draw
   its own card: `id`, `createdAtMs`, `expiresAtMs`, and a `request` with `command`, `cwd`, `host`, `security`,
   `ask`, `resolvedPath`, `commandAnalysis`, `allowedDecisions`, `agentId`, `sessionKey`, `runId`,
   `toolCallId`, and where the turn came from (`turnSourceChannel`, `turnSourceTo`, `turnSourceAccountId`).
2. On a press, calls `approvalCapability.authorizeActorAction` with the account, the sender's id and the kind
   of approval. **OpenClaw asks the plugin whether this person may approve.** The recorder said yes.
3. Returns the outcome to the plugin: `applied`, and the approval with its `status`, `decision` and `resolver`.
4. Calls `buildResolvedPayload`, and sends the "resolved" message.

Without the capability a press is refused. In `approval_press_unclaimed` the same press returned
`approval not found`. The `/approve` message sent afterwards worked.

In every form the agent is told the outcome and carries on. After a denial the tool result reads `Exec denied
(… user-denied) … an operator declined this execution approval`, `onApprovalEvent` reports `denied`, and the
agent said the command had not run. No `onApprovalEvent` was seen for an approval that was allowed, or when
one was requested.

## A question is structured, and must be answered through its own call

The agent has an `ask_user` tool only when the plugin declares that it supports question buttons. Both
question scenarios declare it.

The question arrives as a `tool` delivery with three forms of the same thing:

```text
text          Question for you: … 1. Hello  2. Hi … Reply with the number, the option text, or your own answer.
presentation  blocks of text, and buttons: Hello, Hi, Other…
              each button has an action: {type: "question", questionId, optionValue}
channelData   askUser: {questionId, optionValues: ["Hello", "Hi"]}
```

- **Answered by a press** (`question`): the plugin called `questionGatewayRuntime.resolveOption` with the
  question id, the option and the sender. It returned `answered`. The tool result reached the agent at once
  and it replied `Hello`.
- **Answered by a chat message** (`question_text_answer`): Alice sent `Hello` as an ordinary message. It was
  **not** taken as the answer. It waited behind the running turn for fifteen minutes, until the question
  expired with `no_answer`. The agent then guessed, and Alice's `Hello` ran as a new turn of its own.
  OpenClaw's documentation says a plain reply can answer a question on supported channels; with this plugin it
  did not. Whether a plugin must do something to enable that is not established.

In both runs the model's first `ask_user` call had the wrong arguments and was rejected, and its second
succeeded. The rejection arrived like any failed tool.

## Controls are chat messages

| Control | What happened |
| --- | --- |
| A message during a turn (`mid_turn_message`) | Held, with no notice to the chat and no call on the plugin, until the running turn ended 36 seconds later. Then it ran as a turn of its own |
| `/stop` (`stop`) | `⚙️ Agent was aborted.`, kind `final`, within a tenth of a second. The stopped run reported no outcome: no `onAgentRunTerminalOutcome`. The `inbound.run` call for the stopped message did return, 38 ms before the notice |
| `/help`, `/status` (`commands`) | Answered by OpenClaw itself in under a second, kind `final`, with no model call |
| `/approve` | As above |

There is no separate control interface. A control is a message whose text starts with a slash, from a sender
OpenClaw has judged may use commands.

The commands for a new session and for settings were recorded on 2026-11-18. See
[Controls and sessions](#controls-and-sessions).

`/status` reported the queue mode as `steer`. The recording shows the message waiting and then running
separately, not being folded into the running turn.

## The agent's message tool

OpenClaw gives the agent one `message` tool and asks the plugin which actions the channel supports. The
recorder claimed `send`, `react`, `edit` and `delete`.

| Action | What reached the plugin |
| --- | --- |
| `react` | `actions.handleAction` with the emoji. The message was taken from the turn: `toolContext.currentMessageId` |
| `send` | `actions.handleAction` with the text and the chat |
| `delete` | `actions.handleAction` with the id of the message |
| `edit` | Nothing. OpenClaw refused it first: `Delegated recorder:edit requires the exact current conversation and account for this plugin` |

Each call also says who asked: `requesterSenderId` (`alice`) and `senderIsOwner` (false), with the session and
the chat the turn belongs to.

In `message_actions` the turn ended straight after the agent's `send`, with no closing reply. In
`message_edit_delete` the agent passed `final: false` with its `send`, and the turn went on. That a `send`
without it counts as the reply is inferred from these two runs.

## Things OpenClaw does on its own

- **A typing signal every six seconds** while a turn runs: 225 calls, the second largest count.
- **It reports a model failure as a failure.** In `model_refused` the provider refused the request. The run
  ended with `failed`, and the plugin received a `final` delivery marked `isError`: `⚠️ The AI service reported
  a billing problem…`. Hermes reported the same situation as a success.
- **It then stops calling that provider for a while.** The next message, four seconds later, was answered
  without a model call: `⚠️ Agent failed before reply: Inline API key for provider "recorded" is temporarily
  disabled… Retry after about 10 minutes`.
- **Command output, approval notices and the stop notice** arrive beside the agent's words.
- No notice appeared on the first message of a session. Hermes sends one every time.

## Compared with Hermes

| | Hermes | OpenClaw |
| --- | --- | --- |
| Who drives receiving | Hermes, from one event | The plugin, in four calls |
| Tool calls | Formatted lines only | Structured callbacks with name, arguments, status, output and exit code, and the formatted lines as well |
| Text while writing | Not offered to a plain platform | `onPartialReply` |
| Thinking | Not seen | `onReasoningStream`, when set to stream |
| Sessions | One per sender, even in a group | One per group; one for all direct chats |
| A disallowed sender | Dropped inside Hermes, silently | A verdict with a reason; the plugin decides what to do |
| Approval as text | `/approve`, no id | `/approve <id> <decision>`, only if the owner switched forwarding on |
| Approval as a card | Structured, when the platform has buttons | Structured, when the plugin claims the capability; OpenClaw asks the plugin who may approve |
| Question | Structured, and answered by a chat message | Structured, and answered through its own call; a chat message was not taken |
| Message during a turn | Folded into the running turn, with a notice | Held without a notice, then run as its own turn |
| Stop | `/stop`; the turn ends as `cancelled` | `/stop`; the stopped run reports nothing |
| Model failure | Reported as success | Reported as `failed`, with `isError` |
| Telling runtime text from agent text | A metadata hint on some messages | `info.kind`, `isError` and `channelData` |
| Typing calls | Four fifths of all calls | One in seven |

## Through the prototype translator

The prototype that turns recordings into the proposed agent events
([`tools/prototype`](../../tools/prototype/README.md)) was extended to OpenClaw on 2026-11-17. The question
was whether OpenClaw fits the ten item types and six states of
[the event model](../design/event-model.md), and what does not fit. The prototype is a design tool, not
connector code. This section reports what it showed. It decides nothing.

| | |
| --- | --- |
| Recorded lines: calls in both directions, their results, scripted messages, presses and recorder notes | 3,655 |
| Of those, calls from OpenClaw | 1,655 |
| Agent events produced | 150 |
| Items those events describe | 95 |
| Lines with no rule | 0 |
| Items left open at the end of a recording | 0 |

The events by type: 63 `turn`, 34 `tool`, 29 `message`, 10 `approval`, 6 `notice`, 4 `question`, 3 `error`,
1 `reasoning`. There is no `plan` and no `task`, because no scenario produced one.

Not checked: anything the recordings do not hold (the list is under [Sources](#sources)), a second run of any
scenario, another OpenClaw version, and a chat with two turns running at once.

### What became an event, and what did not

Of the 1,655 calls from OpenClaw, 1,505 are not sent and 150 went into 145 events. The other five events
came from what OpenClaw returned to the plugin's own calls.

| Not sent | Calls |
| --- | --- |
| The receive-path log | 304 |
| Delivery bookkeeping | 285 |
| The typing signal | 258 |
| Address tidying | 177 |
| Questions OpenClaw asks the plugin: what the channel can do, whether a group needs a mention, who may approve | 109 |
| The other two copies of tool activity: `onToolResult`, and `tool` deliveries that repeat an item | 92 |
| Text, thinking and command output so far | 61 |
| `onAssistantMessageStart`, before a message has text | 48 |
| `onModelSelected`. The model is kept and put on the turn | 35 |
| Calls that only repeat or prepare another: the start of a tool before its arguments, the second copy of a wrapped call, a command's end before its output, an update with nothing new | 55 |
| The account starting and stopping | 51 |
| The rest: thinking that ended with no text, approval prompts and outcomes that arrived a second time, the calls around a tool call that became a question or a message | 30 |

### What fit

| Item type | Built from | Items | Events |
| --- | --- | --- | --- |
| `turn` | `onPreparedAgentRunStart` and `onAgentRunTerminalOutcome` | 32 | 63 |
| `tool` | `onToolStart` for the start and the arguments; `onCommandOutput` for the end of a command, with output, exit code, duration and directory; `onItemEvent` for the end of any other tool | 17 | 34 |
| `message` | 26 `block` deliveries, 1 `final` delivery, 2 uses of the message tool | 29 | 29 |
| `approval` | The forwarded message, or the approval capability's calls | 5 | 10 |
| `question` | The `tool` delivery that carries `channelData.askUser`, with the questions and options taken from the tool call's arguments | 2 | 4 |
| `notice` | `final` deliveries that answer a slash command | 6 | 6 |
| `error` | 2 `final` deliveries marked `isError`, and 1 refused press | 3 | 3 |
| `reasoning` | The last `onReasoningStream` text, sent when `onReasoningEnd` arrives | 1 | 1 |

- **No new item type or state was needed**, and none was added.
- **Tool calls are items.** Of the 17, 9 are commands, 3 are file edits and 5 are other tools. Hermes had none.
- **An approval is one item in both forms.** Where the plugin claims the approval capability the item is built
  from the structured request, and the forwarded message is not sent again.
- **A question is one item**, though it arrives as a tool call, twice, and then as a delivery. The answer by
  press closes it through `questionGatewayRuntime.resolveOption`.
- **The translator closes what OpenClaw leaves open.** On the reply to `/stop` it cancels the running command
  and the turn. In `approval_not_forwarded` the run had reported no outcome when the account stopped, and the
  turn is closed there.

### What did not fit

**1. The agent's message tool.** Five calls in two recordings. This is the clearest case.

| Action | Calls | What the prototype made of it | What that leaves |
| --- | --- | --- | --- |
| `send` | 2 | A `message` item, not a tool call | It is text the agent says to the chat. In `message_actions` it was the agent's only reply |
| `react` | 1 | A `tool` item of kind `other`, with the chat and the message reacted to in its body | No item type says the agent acted on somebody's message. A client that draws tool calls shows "used a tool" for a thumbs-up |
| `delete` | 1 | A `tool` item of kind `other`, whose `subject` is the `message` item that was deleted | The deleted message's own item stays `done`. An item changes only by being sent again, and none of the six states means "withdrawn" |
| `edit` | 1 | A `tool` item, `failed` | OpenClaw refused the edit before asking the plugin. An edit that goes through was not recorded, so it was not mapped |

**2. Replies to slash commands belong to no turn.** OpenClaw answers `/help`, `/status` and `/approve` itself,
with no run. Five of the six notices have an empty `turn`. The three replies to `/approve` arrive just after
the turn they unblocked has ended. Only the reply to `/stop` falls inside a turn.

**3. A turn carries little.**

- A run ends with one word, `completed` or `failed`. The `turn` item has no duration and no usage.
- Every turn is `started_by: user`. A message held behind a running turn starts its run later with no sign of
  the wait.
- In `model_refused` the second run reported `failed` and had never reported a start. Its turn is one event,
  `failed`, that was never `running`.
- With no callbacks registered (`tool_call_plain`) the first sign of a turn is the answer itself. The
  translator opens the turn there and closes it when `inbound.run` returns, 19 ms later.

**4. A denied command is first a failed command.** `onItemEvent` and `onCommandOutput` report `failed`.
`onApprovalEvent` says `denied` one call later. In the two recordings where `/approve` denied the command, the
`tool` item is sent as `failed` and then again as `declined`, and the `approval` item closes after both. In
the recording where a press denied it, the outcome arrives first and names the tool call, so the item goes
straight to `declined`.

**5. An approval that never reached the chat has no item.** In `approval_not_forwarded` the command is a
`tool` item that runs for 61 seconds and fails. Nothing was asked of anyone, so there is no `approval` item
and nothing `waiting`. `onApprovalEvent` reports `failed`; the translator does not send it, because the tool
item already failed with that text.

**6. A question that nobody answered closes as `cancelled`.** In `question_text_answer` the question expired
after fifteen minutes. There is no state for "expired". The callback reports the tool as `completed`.

**7. An error is text only.** The two model failures carry no code and nothing about what was being attempted.
Each arrives after its turn has already ended as `failed`.

**8. Smaller gaps.**

| Gap | Count |
| --- | --- |
| A `message` with no `phase`. A `block` delivery does not say whether it is commentary or the answer | 27 of 29 |
| A `tool` item whose result is the formatted line, not fields | 8 of 17 |
| A file edit with a path and the tool's arguments, and no diff | 3 |

**Produced nothing, because OpenClaw said nothing:** the three messages it did not admit, and the two it held
behind a running turn.

### Where the translator goes by order, and what only the wording holds

The translator reads fields: `info.kind`, `isError`, `assistantMessageIndex`, `channelData`, tool names and
ids. It matches none of OpenClaw's wording. It does go by the order of calls in five places.

| The translator assumes | Times | Would a plugin know? |
| --- | --- | --- |
| A `final` delivery with no message index answers the oldest slash command still waiting | 6 | Yes. Each `inbound.run` is given its own callbacks, so a plugin knows which message a delivery answers (read in the recorder's source). The recording does not write that down |
| The `tool` delivery straight after the end of a tool that is not a command is that tool's result | 8 | No. Neither `onToolResult` nor the delivery carries a tool id |
| A question delivery belongs to the `ask_user` call in progress | 2 | No. The delivery carries a question id, not a tool call id |
| An approval forwarded as a message is about the one command that is running | 3 | No, without the approval capability. `channelData.execApproval` names no tool call |
| The decision on an approval answered with `/approve` is the one the client sent | 3 | No. The message that says `resolved` does not say which way |

What only the wording holds, and the translator therefore does not have:

- Which way an approval went, when the plugin did not carry the answer. Not seen. The translator would
  report it as a line with no rule.
- Why a question closed with no answer through the plugin. The result line says `no_answer`.
- Why a tool that is not a command failed. The result line says, for example, that the arguments were
  invalid.
- The command an approval is about, when the plugin does not claim the capability. It is in the prompt text,
  and the item's text is the whole prompt.

### Details found while writing the rules

- **`assistantMessageIndex` separates the agent's words from OpenClaw's.** All 27 deliveries of agent text
  carry it, `block` or `final`. None of the 8 `final` deliveries that OpenClaw wrote does. `info.kind` alone
  does not separate them.
- **A wrapped call can be told from its copy.** `onItemEvent` names both `ask_user`; only `onToolStart` says
  `dispatch_action`. The copy's id is `tool_call:<first id>:ask_user:<n>`. The status `blocked` was seen only
  on the copy; the first call said `failed`. The first call's end can arrive twice, the second time with
  `hideFromChannelProgress`.
- **`onCommandOutput` reports the end twice**: once with only the status, then with the output, exit code,
  duration and directory. Of 18 calls, 2 are output so far, 8 are the bare end and 8 the full end.
- **Starts and outcomes do not pair up.** 30 starts and 29 outcomes hide two runs with no outcome (`stop`,
  `approval_not_forwarded`) and one outcome with no start (`model_refused`).
- **The order of a denial differs.** After `/approve <id> deny` the tool fails first and the `resolved`
  message follows. After a press, the result of the press comes first.
- **A message-tool action can be tied to its tool call.** The `idempotencyKey` handed to
  `actions.handleAction` ends with the tool call id.

## What this means for the design

1. **The OpenClaw connector can report tool calls as items.** `onToolStart` gives the tool and its arguments,
   `onItemEvent` its state, `onCommandOutput` the output and exit code. This is what
   [D-008](../decisions.md#d-008-pass-everything-through) asks for and Hermes cannot give. The connector must
   register the callbacks; nothing arrives otherwise.
2. **It must choose one of three copies.** The structured callbacks, `onToolResult` and the `tool` deliveries
   describe the same activity. Posting more than one would show each tool call several times.
3. **Results of tools other than commands are text.** A file write has structured arguments and a text
   outcome. The native payload for those is the formatted line.
4. **The connector has to pass on `block` deliveries, not only `final`**, or run with block streaming off.
   With it on, the answer is a `block`.
5. **Approvals need two things the connector does not control by default.** The owner must switch approval
   forwarding on in OpenClaw, or an approval waits unseen. And the plugin must claim the approval capability
   to get the structured request and to accept a press. An installer would have to set the first.
6. **OpenClaw asks the plugin who may approve.** That call is a place where the plugin can ask the core, which
   is what [D-010](../decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve) needs.
7. **A question needs its own answer message on the socket.** A press goes back through a dedicated call with
   the question id. Sending the answer as a prompt left the question unanswered for fifteen minutes.
8. **Who may speak can be decided before OpenClaw, inside it, or both.** The plugin supplies the policy and
   lists that OpenClaw judges by, and receives a verdict it can act on. So the plugin can tell a disallowed
   person, which D-010 wants. Which list governs is open
   ([Q-22](../open-questions.md#q-22-whose-rules-govern-a-gateway)).
9. **OpenClaw decides its own sessions, and the plugin shapes them.** A group is one session. All direct
   chats share one unless the plugin or the owner arranges otherwise. A private messenger must not leave that
   default in place by accident.
10. **The socket must carry the sender, the chat and the message id on every prompt**, and the id of the
    message replied to. The quoted text is optional here.
11. **For OpenClaw, controls are text**, and need a sender OpenClaw accepts for commands. "Interrupt" becomes
    `/stop`. A stopped run reports no outcome, so the connector has to end the turn itself.
12. **A message sent during a turn waits silently.** If the group should be told, the connector or the core
    has to say so. The core keeps no queue of its own
    ([D-040](../decisions.md#d-040-a-message-sent-during-a-turn-goes-straight-to-the-harness)).
13. **OpenClaw talks for itself**, but marks it better than Hermes: by kind, by `isError`, and by
    `channelData`. Somebody still has to decide which of its notices a group should see.
14. **The message tool is a second way for the agent to post.** `send`, `react` and `delete` arrive as
    structured calls with the person who asked. The connector must carry them out or not claim them.
15. **Typing calls and the receive-path log are a third of the traffic** and must never become events.

## Controls and sessions

Added on 2026-11-18. The design gives every harness three shared controls: interrupt, new session, and set
([D-029](../decisions.md#d-029-three-shared-controls-and-the-rest-comes-from-the-harness)). It describes a
session to apps with a `session` item
([D-030](../decisions.md#d-030-a-session-is-an-item-the-eleventh-type)). The recordings of 2026-11-17 confirmed
only interrupt. This section records what OpenClaw does for the other two, which typed commands change what
the agent may do without asking, and what OpenClaw will hand a plugin as data.

It reports what was recorded and read. It decides nothing.

### What was read

| Source | What it is |
| --- | --- |
| [`fixtures/recordings/openclaw`](../../fixtures/recordings/README.md#openclaw) | 12 more scripted sessions, recorded on 2026-11-18 |
| OpenClaw | The same release, 2026.11.5 (build `e13cf15`) |
| Model | `z-ai/glm-5.3-flash:free`. `model_switch` switches to `z-ai/glm-5.3:free` |
| OpenClaw's own documentation, shipped in the package | `docs/tools/slash-commands.md`, `elevated.md`, `exec.md`, `permission-modes.md`, `exec-approvals.md`, `docs/gateway/permission-modes.md`, `docs/plugins/hooks.md` and its reference |
| The built package | The built-in command list (`commands-registry.data`), and the type declarations of the plugin SDK |

OpenClaw ships as built JavaScript, so its handlers were not read as source. Where this section says "read in
the documentation", the claim is OpenClaw's own and was not recorded.

The recorder gained two abilities for this. It can register typed plugin hooks, and it can ask OpenClaw
through the plugin SDK for its commands, its models and a session's stored entry. Each is off unless a
scenario switches it on. The 26 recordings of 2026-11-17 used neither, and are unchanged.

Limits:

- One run of each scenario, one OpenClaw version, one model.
- The agent in these recordings is not sandboxed. It runs commands on the gateway's own host. Elevated mode
  is about leaving a sandbox, so `elevated` shows only part of what it does.
- Every sender in these recordings is one OpenClaw lets use commands, except where a scenario says otherwise.
- Not recorded: `/reset soft`, `/new <model>`, `/compact`, `/fast`, `/queue <mode>`, `/elevated` with a
  sandboxed agent, `/exec host=…`, a session permission mode, `/acp` and `/codex` with a harness behind them,
  `/bash` switched on, and a direct chat.

### Which command is which control

| Control | What a person types | Recording | What the plugin is handed |
| --- | --- | --- | --- |
| Interrupt | `/stop` | `stop` (2026-11-17) | A `final` delivery: `⚙️ Agent was aborted.` |
| New session | `/new`, or `/reset` | `new_session`, `reset_session` | A `final` delivery marked `isStatusNotice`: `✅ New session started.` |
| Set the model | `/model <provider/model>`. `/model default` goes back | `model_switch` | A `final` delivery: `Model set to … for this session only` |
| Set the thinking level | `/think <level>` | `settings` | A `final` delivery: `Thinking level set to high.` |
| Set verbose output, reasoning | `/verbose on`, `off` or `full`; `/reasoning on`, `off` or `stream` | `settings` | A `final` delivery |
| Leave the sandbox | `/elevated on`, `ask`, `full` or `off` | `elevated` | A `final` delivery: `⚙️ Elevated mode set to full (auto-approve).` |
| Set the command policy for one message | `/exec security=… ask=…`, in front of the message | `exec_directive` | A `block` delivery marked `isStatusNotice`, then the run |
| List the commands | `/help`, `/commands` | `commands` (2026-11-17), `control_catalog` | A `final` delivery, text |
| List the models | `/models`, `/models <provider>` | `control_catalog`, `model_switch` | A `final` delivery, text |
| Show the session | `/status` | `commands`, `new_session` | A `final` delivery, with a table beside the text |

Every one of these is a chat message, from a sender OpenClaw accepts for commands.

A setting command sent alone changes the session and is answered. Sent with no value it shows the current
one and the values accepted, as text: `Current thinking level: low. Options: default, off, minimal, low,
medium, high, ultra.`

### A new session

In `new_session` Alice told the agent a code word, sent `/new`, and asked for the code word.

**Neither the session key nor the session id changed.**

| | Before `/new` | After |
| --- | --- | --- |
| `sessionKey` in `onPreparedAgentRunStart` | `agent:main:recorder:group:family` | The same |
| `sessionId` in `onPreparedAgentRunStart` | `be554125-f808-4a20-a97a-2a88ddff440e` | The same |
| `generation` in the same call | `45ba35c5…` | The same |
| `maxSeq` in the same call | 3 | 23: the numbering went on |
| `sessionStartedAt` in the stored entry | The time of the first message | The time of `/new` |
| `lifecycleRevision` in the stored entry | Absent | A new id |

The session was nonetheless new. Asked for the code word, the agent answered `I don't know — this session
just started`. `/status` showed the session's duration starting again.

**Where a plugin can see the change.**

| Signal | Structured? | What it carries |
| --- | --- | --- |
| The reply, `✅ New session started.` | Partly | It is a `final` delivery with `isStatusNotice: true`. Only this reply, the one to `/reset`, and the line that acknowledges an `/exec` directive carried that mark. It does not say which session, or that one ended |
| The stored session entry, if the plugin asks for it | Yes | `lifecycleRevision` and `sessionStartedAt` change |
| The typed plugin hook `session_end` | Yes | `reason: "new"`, the session key, the session id, and `nextSessionId`, which was the same id |
| The typed plugin hook `session_start` | Yes | The session key and id, and `resumedFrom`, the same id again |
| The typed plugin hook `before_reset` | Yes | The whole conversation that is being dropped: 12 messages, with the path of the session's file |

So a channel plugin that does nothing extra is told of a new session by one reply, and a plugin that wants
more has to register hooks or read the stored entry. **A connector cannot use the session id to tell one
session from the next.** It has to count resets itself.

`before_reset` handed over the old conversation although the plugin had not been granted access to
conversations: in the same recording `session_end` reported the ended transcript as `available: false,
reason: conversation-access-required`. That is outside this section's subject and was not looked into.

`reset_session` repeats the scenario with `/reset`, and went the same way. The reply was `✅ Session reset.`,
marked `isStatusNotice`. The hooks fired with `reason: "reset"`. The key and the id stayed, and the agent no
longer knew the code word. OpenClaw's documentation describes `/new` as archiving the session and `/reset` as
resetting it in place. In a group chat these two recordings show no difference a plugin can see.

### Changing the model

In `model_switch` the configuration listed two models. Alice sent `/models recorded`, then
`/model recorded/z-ai/glm-5.3:free`, ran a turn, and sent `/model default`.

- **`/models recorded` lists models as text**: `Models (recorded …) — showing 1-2 of 2`, then the two ids.
- **The switch is answered in text**: `Model set to recorded/z-ai/glm-5.3:free for this session only;
  configured default unchanged.`
- **The next run names the new model** in `onModelSelected`, as every run does. This is the confirmation a
  plugin gets without asking.
- **The stored entry holds the choice**: `providerOverride`, `modelOverride`, and `modelOverrideSource:
  "user"`. After `/model default` the first two are gone and the third says `default`.
- **A model that does not exist was accepted.** `/model recorded/no-such-model` was answered `Model set to
  recorded/no-such-model for this session only`. No run followed, so what happens next was not recorded.
  Codex accepted an unknown model in the same way, and failed on the next turn.

### Other settings

In `settings` Alice sent `/think high`, `/verbose off` and `/reasoning off`, and ran a turn.

Each was answered in text. The stored entry then held `thinkingLevel: "high"`, `verboseLevel: "off"` and
`reasoningLevel: "off"`, and the next run's `onModelSelected` said `thinkLevel: "high"`. `/think nonsense` was
refused in text, with the accepted values.

Before any of these are set, the stored entry has no such fields: a setting that is at its default is absent,
not listed with its default value. `onModelSelected` does give the thinking level in force. For verbose
output and reasoning the default is in the configuration and in the text of `/status`.

### Commands that change what the agent may do without asking

This is the list a connector needs for
[D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights). It was built from the built-in command
list, which a plugin can read as data (51 commands), and from OpenClaw's documentation of each.

| Command | What it does | Status | Evidence |
| --- | --- | --- | --- |
| `/exec security=full ask=off <message>` | Sets the command policy for the run that message starts | Recorded | `exec_directive`: under the `ask` mode, `rm -rf /tmp/recorder-demo-dir` ran with no approval. `exec_directive_once`: the same command ran with no approval behind the directive, and was asked about when sent again a minute later without it |
| `/elevated on`, `ask`, `full` | Lets a sandboxed agent run commands outside the sandbox. `full` also skips approvals where the policy already allows it | Recorded, with an agent that is not sandboxed | `elevated`: accepted for Alice, and the stored entry said `elevatedLevel: "full"`. The approval was still asked. Refused for Bob, who was not on the list of people allowed to use it |
| `/approve <id> allow-always` | Answers an approval, and trusts that command from then on | Recorded | `approve_always`: Alice answered `allow-always`. Bob then asked for the same command, and it ran with no prompt |
| `/config set <path>=<value>` | Writes OpenClaw's configuration file, which holds the command mode and every other setting | Recorded | `owner_commands`: refused, as owner-only. `owner_config`: with Alice made the owner and the command switched on, `/config set tools.exec.mode=full` was answered `⚙️ Config updated`, and `/exec` then reported `security=full, ask=off`. No command was run after it |
| `/debug set <path>=<value>` | Overrides a configuration value until restart | Recorded. It did not change the command policy | `owner_commands`: refused, as owner-only. `owner_config`: `/debug set tools.exec.mode=full` was answered `⚙️ Debug override set`. `/exec` still reported `security=allowlist, ask=on-miss`, and the next command was asked about |
| `/bash <command>`, or `! <command>` | Runs a command on the host directly, with no agent involved | Recorded as refused | `owner_commands`: `⚠️ bash is disabled. Set commands.bash=true to enable.` It is off unless the owner switches it on, and then also needs elevated mode to be available (read in the documentation) |
| `/allowlist add`, `remove` | Changes who may talk to the agent in this channel | Read in the documentation, not recorded | It needs `commands.config` to be on. `owner_commands` records `/allowlist` alone, which lists |
| `/acp permissions`, `/acp set-mode` | Change the permissions of a coding harness that OpenClaw drives over ACP | Recorded as refused | `owner_commands`: `/acp status` was refused as owner-only. No such harness was set up |
| `/codex permissions` | The same for the Codex harness plugin | Read in the documentation, not recorded | |
| `/mcp set`, `/plugins install`, `enable` | Add tools or code | Recorded as refused | `owner_commands`: `/mcp show` refused as owner-only, `/plugins list` as disabled |

Looked for and not found:

- **A chat command that sets a session's permission mode.** OpenClaw has one per session: `read-only`,
  `guarded`, `workspace` or `full`. Its documentation describes changing it from the web interface, and from
  the gateway's own protocol. The built-in command list has no command for it, and a search of the built
  package for typed commands named `/permissions`, `/mode`, `/approvals`, `/yolo`, `/trust`, `/sudo` and
  `/bypass` found none.
- **A command that switches approvals off for a whole session.** `/exec security=… ask=…` sent alone was
  answered `⚙️ Exec policy for this run only` and changed nothing: `/exec` afterwards showed the same
  defaults. The documentation says the same.

**What a plugin sees when an approval is skipped.** In `exec_directive` the plugin was handed one `block`
delivery marked `isStatusNotice`, `⚙️ Exec policy for this run only (security=full, ask=off).`, before the
run began. Then the run, the tool call and its output, as for any command. Nothing on the tool call says that
it ran without the approval it would otherwise have needed.

**Who could do it.** Alice, who sent it, was an ordinary member of the group. With the same set-up OpenClaw
refused her every owner-only command in `owner_commands`, and it reports `senderIsOwner: false` for her
wherever a recording shows that field. The documentation says `/exec` is honoured for any sender who may use
commands, and that a directive from anyone else is treated as plain text. `exec_directive_unauthorized` records the other case, with OpenClaw's `commands.allowFrom` naming only
Alice. Bob's message with the directive and the request started no run and got no reply. The whole message
was dropped, the request with it, which is stricter than "treated as plain text". The plugin was told through
the return of `inbound.run`, which carried `beforeAgentRunBlocked: true` and `deliberateSilentTerminalReply:
true`. Bob's bare `/exec` was dropped in the same way.

This differs from what the documentation says elsewhere, that an `/exec` override "can only tighten" a
session's policy. That sentence is about a session with a permission mode set. These sessions had none.

### OpenClaw has its own owner

Recorded in `owner_commands`, with OpenClaw's defaults. Alice, an allowed member, was refused `/config`,
`/debug`, `/mcp`, `/acp`, `/send` and `/activation`, each with:

```text
You are not authorized to use this owner-only command. Ask the operator to run
`openclaw config set commands.ownerAllowFrom '["recorder:alice"]'` in a terminal to make this sender a
command owner.
```

`/bash` and `/plugins` were refused as switched off. `/allowlist`, `/queue`, `/exec`, `/elevated`, `/think`,
`/model`, `/new` and `/stop` were not owner-only.

So OpenClaw has three levels: a sender it ignores, a sender who may use commands, and an owner. The middle
level is one switch for all commands, with no notion of "approve" as against "prompt".

A plugin passes OpenClaw a verdict on that middle level with every message, as `CommandAuthorized`. The
recorder passes on what OpenClaw's own check of the sender returned. That value is not the last word. In
`exec_directive_unauthorized` it was true for Bob, and OpenClaw dropped his commands all the same, because the
configuration's `commands.allowFrom` named only Alice. Whether a plugin can refuse commands for a sender by
passing false was not recorded.

In `owner_config` the configuration named Alice as the owner and switched `/config` and `/debug` on. She
could then write the configuration from the chat, as the table above records, and `/activation mention` was
accepted. So for an owner, a typed command can change the command mode of the whole gateway. Nothing the
plugin is handed marks that but the text of the reply.

### What OpenClaw hands over as data

Nothing arrives unasked but the model and the thinking level, in `onModelSelected` before each run. The rest
is there for a plugin that asks, through the plugin SDK. `control_catalog` records each call.

| Wanted | Call | What came back |
| --- | --- | --- |
| The commands | `listChatCommands()`, from `openclaw/plugin-sdk/native-command-registry` | 51 commands. Each has a key, its typed names, a description, a category, a tier, and its arguments |
| The values a setting accepts | The same call. An argument lists its `choices`. Where they depend on the model, `resolveCommandArgChoices` gives them | `think`: default, off, minimal, low, medium, high. `verbose`: on, off, full. `reasoning`: on, off, stream. `elevated`: on, off, ask, full. `exec`: the values of `host`, `security` and `ask` |
| The models | `buildModelsProviderData(cfg, agentId)`, from `openclaw/plugin-sdk/models-provider-runtime` | The models by provider, and the default |
| A session's settings | `getSessionEntryAsync({agentId, sessionKey})`, from `openclaw/plugin-sdk/session-store-runtime` | The stored entry: the session id, and each setting someone has changed |
| That a session was replaced | The hooks `session_end`, `session_start` and `before_reset`, registered with `api.on(…)` | See [A new session](#a-new-session) |

Three limits on this:

- **The command list does not say what a command may do.** Nothing in an entry marks `/exec` or `/elevated`
  as changing permissions, or `/config` as owner-only. The tier (`essential`, `standard`, `power`) is about
  what to show first.
- **The accepted values and the text disagreed once.** For the thinking level the call returned six values.
  The reply to `/think` named seven, with `ultra`.
- **The stored entry holds changes, not the state.** See [Other settings](#other-settings).

`/status` is the one reply that carries structure beside its text: a `presentation` with a table of rows such
as `Modes: think low · fast off · reasoning on · verbose:full`. The values are still inside strings.

Not called: the plugin SDK also declares a function to set a session's model
(`applySessionModelSelection`) and one to change a stored entry (`applySessionEntryPatch`). If they do what
their names say, a plugin could set the model or a setting with a call and not a typed command. That was not
tried.

Not examined: the gateway's own protocol, which OpenClaw's web interface uses. The type declarations show
that it lists sessions with their settings and the thinking levels each accepts. It is not part of what a
channel plugin is handed.

### The new recordings through the prototype translator

The prototype was extended to the new recordings on 2026-11-18. All 38 OpenClaw recordings translate with
no line left without a rule and no item left open.

| | First 26 recordings | All 38 |
| --- | --- | --- |
| Session items | 24 | 36 |
| Edits of a session item | 0 | 9 |
| Largest session item, without the native message | 0.3 KB | 4.9 KB |

What changed in the rules:

- **A session item now holds what the plugin asked for.** The settings in the stored entry, with the two that
  bear on permissions marked as needing the right to approve
  ([D-043](../decisions.md#d-043-a-setting-says-which-right-it-needs)). The values each setting accepts and
  the commands, from the command list. The models, from the model list.
- **A replaced session ends one item and starts another.** The prototype names an OpenClaw session by its
  session key, as before, and adds a count: the second session under a key is `…:family#2`. It does so on
  `session_end` when the hook is registered, and otherwise on the reply marked `isStatusNotice`.
- **A directive in front of a message is a notice** of kind `directive`. The prototype still matches none of
  OpenClaw's wording.
- **A typed command is counted as the control it asks for, whether or not OpenClaw accepted it.** Only the
  wording of the reply, or the stored entry, says which.

### What a connector has to supply from its own knowledge

[D-044](../decisions.md#d-044-a-connector-supplies-what-its-harness-does-not-say) says a connector fills in
what its harness does not report. OpenClaw reports far more than Hermes, if asked. What is left:

1. **The command for each shared control**: `/stop`, `/new`, `/model <provider/model>`. The command list
   names them and does not say which control each is.
2. **Which settings and commands need the right to approve.** Nothing OpenClaw hands over marks them. The
   table above is the list for this version.
3. **The default of a setting nobody has changed**, other than the model and the thinking level.
4. **A count of sessions.** The key and the id stay the same across `/new`.
5. **Whether a typed command took effect.** The reply is text. The stored entry can be read again. A command
   that OpenClaw drops gets no reply at all; the return of `inbound.run` says so.

Two points for the people deciding, not for the connector:

- **A directive is part of an ordinary message.** `/exec security=full ask=off` works in front of a prompt.
  Under D-032 the core checks a message that starts with a listed command against the right to approve. The
  documentation also describes directives placed inside a message; that was not recorded. If it holds, a
  check of how a message starts is not enough for OpenClaw.
- **Anyone who may use commands could skip an approval**, with the set-up recorded here. What narrowed it was
  OpenClaw's `commands.allowFrom`. Whether that is acceptable is the owner's choice of OpenClaw configuration,
  under [D-021](../decisions.md#d-021-approval-behaviour-belongs-to-the-harness). A connector's installer
  would have to know it.
