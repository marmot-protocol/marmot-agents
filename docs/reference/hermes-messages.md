# Reference: what Hermes hands a platform

This page is detailed on purpose. It records what the Hermes agent runtime gives a chat platform plugin, taken
from real sessions, and what that means for the Hermes connector and the socket protocol.

Hermes is a gateway, not a coding harness. Codex and Claude Code are driven by one person through a control
interface, and are covered in [`harness-messages.md`](harness-messages.md). Hermes has its own idea of chats,
users and sessions, and a platform plugin is how a chat network plugs into it.

## Sources

| Source | What it is |
| --- | --- |
| [`fixtures/recordings/hermes`](../../fixtures/recordings/README.md#hermes) | 14 scripted sessions, recorded on 2026-11-17 |
| Hermes Agent | Commit `40d1e95f`, the canary release of 2026-11-17 (v0.23.2) |
| Model | `z-ai/glm-5.3-flash` through an OpenAI-compatible endpoint |
| Recorder | [`tools/record/hermes`](../../tools/record/README.md#hermes): a platform plugin that plays a scripted chat and records every call Hermes makes on it |

Limits of this evidence:

- One Hermes version, one model, one run of each scenario. Treat each finding as a lead to confirm, not a law.
- The recorder is a platform with no special abilities, except in the two scenarios that claim approval buttons.
  A platform that claims more (streaming drafts, status text, media) would be handed more.
- The model was slow, about a minute per request. That lengthened every turn and produced more progress
  messages than a fast model would.
- Media, reactions, edits from users, scheduled jobs and Hermes's message tools were not exercised.

## How a platform plugs in

A plugin registers a platform with a name and a factory for an adapter. The adapter is a subclass of Hermes's
base adapter. It must implement four methods: `connect`, `disconnect`, `send` and `get_chat_info`. Everything
else has a default.

Messages go in through `handle_message(event)`. The event carries the text, a message id, and a **source**: the
chat id, name and type (`dm` or `group`), and the sender's id and name.

Everything comes back as calls on the adapter.

## What Hermes called

1,304 calls in 14 sessions.

| Call | Count | What it is |
| --- | --- | --- |
| `send_typing` | 1,026 | "The agent is working", repeated every two seconds |
| `stop_typing` | 96 | |
| `send` | 68 | A message to post in the chat. Almost everything arrives this way |
| `on_processing_start`, `on_processing_complete` | 20 each | A turn began; a turn ended, with an outcome |
| `format_tool_preview` | 18 | Asks the platform to format a short preview of a tool's input |
| `send_final_ledgered` | 16 | Wraps the final reply of a turn; it calls `send` |
| `connect`, `disconnect` | 14 each | |
| `edit_message` | 7 | Replace the text of a message Hermes sent earlier |
| `send_exec_approval` | 2 | An approval, when the platform has buttons |
| `send_clarify` | 1 | A question with choices |

## Almost everything is text through one call

The 68 `send` calls, sorted by what they carried:

| What | Count | Example |
| --- | --- | --- |
| A tool progress line | 18 | `💻 Running rm -rf /tmp/recorder-demo-dir` |
| A notice from Hermes itself: no home channel | 15 | `📬 No home channel is set for Recorder…` |
| The agent's final reply | 14 | |
| A progress heartbeat | 7 | `⏳ Working — 3 min — iteration 1, execute_code` |
| The agent's text during a turn | 3 | `Running that sleep now.` |
| An approval prompt, as text | 3 | `⚠️ Hermes wants to run a command that needs your OK…` |
| The output of a slash command | 2 | `📖 Hermes Commands…` |
| A question, as text | 1 | `❓ Which greeting should we use?…` |
| Other notices from Hermes: steered, stopped, approved, denied, approval timed out | 5 | `⚡ Stopped. You can continue this session.` |

So a platform receives the agent's words, tool activity, runtime notices and command output through the same
call, as text that Hermes has already formatted. **There is no call that says "the agent used this tool with
this input and got this result".**

The only machine-readable hint is the `metadata` argument:

| Metadata | Seen on |
| --- | --- |
| `notify`, with `reply_to` set | The final reply, and a few notices |
| `_interim_send` | Progress lines and text during a turn |
| `_interim_send` and `is_approval_prompt` | An approval prompt sent as text |
| None | Runtime notices, command output, heartbeats |

A tool progress line is edited as more tools run: Hermes calls `edit_message` to turn
`⚙️ tool_search…` into `⚙️ tool_search…` plus `🔍 Searching past sessions` on the next line.

## Sessions are per person, even in one group

Hermes names each session. The names seen:

```text
agent:main:recorder:group:family:alice
agent:main:recorder:group:family:bob
agent:main:recorder:dm:alice-dm
```

In a group, each sender has their own session. In `two_senders`, Alice told the agent her favourite animal, and
then Bob asked what it was. Bob's session did not contain Alice's message. The agent spent six minutes searching
past sessions and files before answering correctly.

## A reply needs its quoted text supplied

In `reply`, Bob replied to Alice's earlier message and asked what it said. The event Hermes received had the id
of the message replied to, and nothing else: the event's fields for the quoted text and its author were empty,
because the recorder did not fill them in. Hermes had no way to look the message up. It found the answer only by
searching past sessions.

A platform that wants replies to work has to put the quoted text and its author on the event itself.

## Approvals come in two forms

Hermes flags a command it considers dangerous and asks before running it.

**As text, by default.** The platform receives an ordinary `send`:

```text
⚠️ Hermes wants to run a command that needs your OK
…
Reply /approve to run it once, /approve session to allow this pattern for the rest of this session,
/approve always to allow it permanently, or /deny…
```

The person answers by sending `/approve` or `/deny` as a chat message. Hermes replies `✅ Command approved. The
agent is resuming...` or `❌ Command denied.`

**As a structured card, when the platform has buttons.** A platform says so by overriding one method. Hermes
then calls it with:

| Field | Value in the recording |
| --- | --- |
| `request_id` | An id for this one approval |
| `session_key` | The session it belongs to |
| `command` | `rm -rf /tmp/recorder-demo-dir` |
| `description` | `delete in root path`: why it was flagged |
| `text` | The whole prompt, ready to show |
| `actions` | `Allow Once` (`once`), `Allow Session` (`session`), `Always Allow` (`always`), `Deny` (`deny`), each with a style |

The platform answers by calling Hermes with the session, the choice, the request id and who pressed. Hermes
checks that the person may answer, and that the request is still open.

In both forms:

- **An unanswered approval expires.** After five minutes Hermes posts `⌛ Approval timed out after 5 minutes —
  the command was NOT run.`
- **The agent is told the outcome and carries on.** After a denial it said the command had not run and that it
  would not work around it.

The request id is new. The Hermes plugin in MDK was written against Hermes 0.19, whose approval had none, and
says so in a comment.

## A question is structured, and also sent as text

In `question`, Hermes called `send_clarify` with the question, a list of choices (`Hello (Recommended)`, `Hi`),
an id for the question and the session. The base adapter then sent the same thing as text:

```text
❓ Which greeting should we use?
Reply "skip" to skip this question.

  1. Hello (Recommended)
  2. Hi

Reply with the number, the option text, or your own answer.
```

The person answered by sending `Hello` as a chat message.

## Controls are chat messages

| Control | What happened |
| --- | --- |
| A message during a turn | Hermes steered. It posted `↪ Redirected current run… I'll adjust using your correction.` and folded the request into the running turn |
| `/stop` | `⚡ Stopped. You can continue this session.` The turn's outcome was `cancelled` |
| `/help`, `/status` | Answered by Hermes itself in under a second, with no model call |
| `/approve`, `/deny` | As above |

There is no separate control interface. A control is a message whose text starts with a slash.

## Who may talk to it

Hermes has its own allow-list for each platform. With only Alice allowed, a message from Mallory produced
**nothing at all**: no reply, no notice, and no call on the adapter. Alice's next message was answered
normally.

## Things Hermes does on its own

- **A notice on the first message of every session.** `📬 No home channel is set…`, with an instruction to type
  `/sethome`. It appeared 15 times in 14 recordings.
- **Progress heartbeats** during a long turn, about every three minutes.
- **A turn that failed at the model is reported as a success.** In an early run the model call was refused. The
  error text arrived through `send`, and `on_processing_complete` said `success`.
- **A turn can be reported complete and then started again.** For the first message of a session,
  `on_processing_complete` sometimes arrived within a second, followed by a second `on_processing_start`. A
  platform cannot treat the first "complete" as the end of the turn.

## What this means for the design

1. **The Hermes connector cannot report tool calls as items.** Hermes gives a platform formatted lines, not
   tool events. The connector can send those lines as messages or notices, marked by the metadata hint, and no
   more. Getting real tool events would need a different Hermes interface or a change in Hermes.
2. **Approvals and questions are the exception.** Both have structured calls, with ids and choices, and map
   cleanly onto the `approval` and `question` items. The connector should claim approval buttons to get them.
3. **The socket must carry the sender and the chat on every prompt**, and for a reply, the quoted text and its
   author. Hermes cannot recover any of these itself.
4. **Hermes decides its own sessions.** A group is one session per sender. The core cannot assume that one chat
   is one session.
5. **Hermes drops a disallowed sender silently.** Decision
   [D-010](../decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve) wants that person told, so
   the core has to do it before the message reaches Hermes.
6. **Two allow-lists exist.** Hermes has its own, and the core has the owner's. Which one governs, and how they
   stay consistent, is open ([Q-22](../open-questions.md#q-22-whose-rules-govern-a-gateway)).
7. **Hermes talks for itself.** Notices, heartbeats and command output arrive beside the agent's words. The
   connector has to tell them apart, and somebody has to decide which of them a group should see.
8. **For Hermes, controls are text.** "Interrupt" from the chat becomes the message `/stop`. The connector
   translates controls into slash commands.
9. **Typing calls are four fifths of the traffic** and must never become events.
