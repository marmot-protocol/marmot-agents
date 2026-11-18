# Reference: what OpenClaw hands a channel plugin

This page is detailed on purpose. It records what the OpenClaw agent runtime gives a channel plugin, taken from
real sessions, and what that means for the OpenClaw connector and the socket protocol.

OpenClaw is a gateway, like Hermes: an agent runtime with its own chats, users and sessions, where a channel
plugin is how a chat network plugs in. Hermes is covered in [`hermes-messages.md`](hermes-messages.md), and
the coding harnesses in [`harness-messages.md`](harness-messages.md).

## Sources

| Source | What it is |
| --- | --- |
| [`fixtures/recordings/openclaw`](../../fixtures/recordings/README.md#openclaw) | 26 scripted sessions, recorded on 2026-11-17 |
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

For a `block`, `info` also carries `assistantMessageIndex`, the position of the model message it came from.

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
`/approve <id> allow-once` or `deny`. OpenClaw replied `✅ Approval allow-once submitted for <id>.` and sent a
second message, `✅ Exec approval allowed once. Resolved by Chat approval (recorder:alice).`

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
| `/stop` (`stop`) | `⚙️ Agent was aborted.`, kind `final`, within a tenth of a second. The stopped run reported no outcome |
| `/help`, `/status` (`commands`) | Answered by OpenClaw itself in under a second, kind `final`, with no model call |
| `/approve` | As above |

There is no separate control interface. A control is a message whose text starts with a slash, from a sender
OpenClaw has judged may use commands.

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
