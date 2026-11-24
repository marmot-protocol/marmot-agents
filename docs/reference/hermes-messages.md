# Reference: what Hermes hands a platform

This page is detailed on purpose. It records what the Hermes agent runtime gives a chat platform plugin, taken
from real sessions, and what that means for the Hermes connector and the socket protocol.

Hermes is a gateway, not a coding harness. Codex and Claude Code are driven by one person through a control
interface, and are covered in [`harness-messages.md`](harness-messages.md). Hermes has its own idea of chats,
users and sessions, and a platform plugin is how a chat network plugs into it.

## Sources

| Source | What it is |
| --- | --- |
| [`fixtures/recordings/hermes`](../../fixtures/recordings/README.md#hermes) | 14 scripted sessions, recorded on 2026-11-17. 13 more, recorded on 2026-11-18, are covered in [Controls and sessions](#controls-and-sessions) only |
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

Hermes flags a command it considers dangerous and asks before running it. Whether it asks a person depends on
its approval mode, which the recordings of 2026-11-18 look at: see
[Controls and sessions](#commands-that-change-what-the-agent-may-do-without-asking).

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

One exception was recorded on 2026-11-18, the model picker, with the commands for a new session and for
settings. See [Controls and sessions](#controls-and-sessions).

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

## Through the prototype translator

The prototype that turns recordings into the proposed agent events
([`tools/prototype`](../../tools/prototype/README.md)) was extended to Hermes, as a check that the event model
is not shaped around coding harnesses.

| | |
| --- | --- |
| Native calls and scripted messages | 2,618 |
| Agent events produced | 120 |
| Calls with no rule | 0 |

The events by type: 51 `notice`, 40 `turn`, 17 `message`, 10 `approval`, 2 `question`.

- **No new item type was needed.** Hermes fits the ten.
- **It uses five of them.** There is no `tool`, `reasoning`, `plan`, `task` or `error`, because Hermes does not
  hand those to a platform as such.
- **Notices carry what tools would have.** The 18 tool progress lines became notices of kind `activity`, and
  Hermes's own messages became notices of kinds `runtime`, `progress`, `steered`, `stopped` and
  `command_output`.
- **The translator tells them apart by their first character.** A line is taken as tool activity because it
  starts with one of ten emoji, and as a runtime notice because it starts with another. That is fragile: it
  depends on Hermes's wording. The `metadata` hint is firmer but covers less.
- **The question was sent twice**, once structured and once as text. The translator keeps the structured one.

Changed on 2026-11-21, when the Hermes plugin was built against these rules
([`python/marmot_hermes`](../../python/marmot_hermes/translate.py)). The item types have no notice kind for
tool activity, progress, steering or a stop, so the prototype no longer makes them: a tool line and a heartbeat
are messages in the commentary phase, which is how Hermes shows them on every platform; Hermes's own notices
(no home channel, steered, interrupted) are of kind `warning`; and Hermes's answer to a slash command, a refusal
included, is of kind `command_output`, found by the command it replies to. A request to confirm a typed command
is a `question` ([D-048](../decisions.md#d-048-a-request-to-confirm-a-command-is-a-question)). The counts above
are from before that change.

## What this means for the design

1. **The Hermes connector cannot report tool calls as items.** Hermes gives a platform formatted lines, not
   tool events. The connector can send those lines as messages or notices, marked by the metadata hint, and no
   more. Getting real tool events would need a different Hermes interface or a change in Hermes. Hermes's
   plugin hooks include two for tool calls, which were not tried: see
   [What Hermes hands over as data](#what-hermes-hands-over-as-data).
2. **Approvals and questions are the exception.** Both have structured calls, with ids and choices, and map
   cleanly onto the `approval` and `question` items. The connector should claim approval buttons to get them.
3. **The socket must carry the sender and the chat on every prompt**, and for a reply, the quoted text and its
   author. Hermes cannot recover any of these itself.
4. **Hermes decides its own sessions.** A group is one session per sender. The core cannot assume that one chat
   is one session.
5. **Hermes drops a disallowed sender silently.** Decision
   [D-010](../decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve) wants that person told, so
   the core has to do it before the message reaches Hermes.
6. **Two allow-lists exist.** Hermes has its own, and the core has the owner's. The core's governs, and Hermes's is
   set to allow whatever the core lets through ([D-084](../decisions.md#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through)).
7. **Hermes talks for itself.** Notices, heartbeats and command output arrive beside the agent's words. The
   connector has to tell them apart, and somebody has to decide which of them a group should see.
8. **For Hermes, controls are text.** "Interrupt" from the chat becomes the message `/stop`. The connector
   translates controls into slash commands.
9. **Typing calls are four fifths of the traffic** and must never become events.

## Media a platform is handed

Read in Hermes's source at commit `40d1e95f` (`gateway/platforms/base.py`, `gateway/cwd_placeholder.py`,
`hermes_cli/config_defaults.py`) on 2026-11-22. Not recorded: no session sent a file. What the plugin does with
it is in [a gateway's files](../design/drivers.md#a-gateways-files).

- **How the agent asks.** A `MEDIA:<path>` line in its answer, which Hermes takes out of the text, or a bare
  path to a local file it recognises. Other parts of Hermes (scheduled jobs, notifications, `/export`) call the
  same methods.
- **What the platform is called with.** `_deliver_media_attachments` sends images in one
  `send_multiple_images(chat_id, images, metadata, human_delay)`, as `(file://<quoted path>, "")` pairs, unless
  the answer asks for documents; then each other file through `send_voice` (audio from a `MEDIA:` line),
  `send_video` or `send_document(chat_id, file_path, metadata=…)`. The base class's own versions post a notice
  that the file could not be sent. Each returns a `SendResult`; when it fails, Hermes posts its own notice,
  naming the file, unless `raw_response` holds `failure_notice_sent: true`.
- **Which paths reach the platform.** `validate_media_delivery_path` resolves links and passes an absolute path
  to an existing file, outside a list of system and credential folders (`/etc`, `~/.ssh`, `~/.config`, Hermes's
  own credential files and more). In strict mode (`HERMES_MEDIA_DELIVERY_STRICT`) only Hermes's caches, the
  folders in `HERMES_MEDIA_ALLOW_DIRS` and files made in the last ten minutes pass.
- **Where its files are.** Generated images, speech and screenshots go to `$HERMES_HOME/cache/…`, beside the
  files people sent in on any platform (inferred from the cache folders Hermes trusts and their names). Its commands run in `terminal.cwd`; the default, `.`, means the home of
  the account Hermes runs as, for a gateway with the local terminal backend.

## Controls and sessions

Added on 2026-11-18. The design gives every harness three shared controls: interrupt, new session, and set
([D-029](../decisions.md#d-029-three-shared-controls-and-the-rest-comes-from-the-harness)). It describes a
session to apps with a `session` item
([D-030](../decisions.md#d-030-a-session-is-an-item-the-eleventh-type)). The recordings of 2026-11-17 confirmed
only interrupt. This section records what Hermes does for the other two, which typed commands change what the
agent may do without asking, and what Hermes will hand a plugin as data.

It reports what was recorded and read. It decides nothing.

### What was read

| Source | What it is |
| --- | --- |
| [`fixtures/recordings/hermes`](../../fixtures/recordings/README.md#hermes) | 13 more scripted sessions, recorded on 2026-11-18 |
| Hermes Agent | The same commit, `40d1e95f` (v0.23.2) |
| Model | `z-ai/glm-5.3-flash:free`. `model_switch` and `model_picker` switch to `z-ai/glm-5.3:free` |
| Hermes's source at that commit | The command registry (`hermes_cli/commands.py`), the gateway's command handlers (`gateway/slash_commands*.py`, `gateway/run_busy.py`), who may run a command (`gateway/slash_access.py`), the approval code (`tools/approval_yolo.py`, `hermes_cli/approval_mode.py`, `hermes_cli/write_approval_commands.py`), a chat's model settings (`gateway/platforms/session_model.py`), the plugin hooks (`hermes_cli/plugins.py`) and the list of what a plugin may import (`hermes_cli/data/plugin_api.json`) |

The recorder gained three abilities for this. It can register plugin hooks, claim a model picker, and ask
Hermes for its commands and a chat's model settings. Each is off unless a scenario switches it on. The 14
recordings of 2026-11-17 used none of them, and are unchanged.

Limits:

- One run of each scenario, one Hermes version, one model.
- Where this section says "read in the source", nothing was recorded. Where it says "not found", the places
  searched are named.
- Two scenarios did not go as scripted. Both are kept, under their own names, and both are findings:
  `approvals_unordered` and `smart_approved`.
- Not recorded: `/resume`, `/sessions`, `/branch`, `/undo`, `/retry` and `/compress`; `/approve always` across
  a restart; `/yolo` across a restart or a new session; a `/model` switch that Hermes asks to confirm; the
  confirmation of `/new` answered with `/always` or `/cancel`, or by a button; a slash command that runs the
  model; a direct chat.

### Which command is which control

| Control | What a person types | Recording | What the platform is handed |
| --- | --- | --- | --- |
| Interrupt | `/stop` | `stop` (2026-11-17) | Text: `⚡ Stopped. You can continue this session.` |
| New session | `/new`, or its alias `/reset` | `new_session` | First a request to confirm. After `/approve`, text: `✨ Session reset! Starting fresh.` |
| Set the model | `/model <id>`. `/model reset` goes back to the default | `model_switch` | Text: `Model switched to …` |
| Set the model, from a list | `/model`, then a pick, if the platform claims a model picker | `model_picker` | The providers and their models as a list, and a callback to call with the pick |
| Set the reasoning effort | `/reasoning <level>` | `setting_reasoning` | Text: `🧠 ✓ Reasoning effort set to …` |
| Switch approvals off for this session | `/yolo`, a toggle | `yolo` | Text: `⚡ YOLO mode **ON** for this session — all commands auto-approved.` |
| Set the approval mode for the whole profile | `/approvals manual`, `smart` or `off` | `approvals_off`, `approvals_unordered` | Text: `Approval mode: off (persistent profile setting).` |
| List the commands | `/help`, `/commands` | `commands` (2026-11-17), `control_catalog` | Text, in pages |
| Show the session | `/status` | `commands`, `new_session` | Text, with the session id and the model |

Every one of these is a chat message. The one exception is the pick from the model picker, which is a call.

### A new session

In `new_session` Alice told the agent a code word, sent `/new`, and asked for the code word.

**Hermes asks before it starts a new session.** `/new` did not reset anything. Hermes first called
`send_slash_confirm` on the adapter with a title (`/new`), the prompt, the session key and a confirmation id.
The recorder does not claim buttons, so that call returned "not supported", and Hermes sent the same prompt as
text:

```text
⚠️ **Confirm /new**
This starts a fresh session and discards the current conversation history.
Choose: Approve Once, Always Approve, Cancel
Text fallback: reply `/approve`, `/always`, or `/cancel`.
```

Alice sent `/approve`, and the reset happened. So `/approve` has two meanings in Hermes: it allows a command
the agent wants to run, and it confirms a command the person typed. `/always` and `/cancel` are accepted only
as answers to such a confirmation; neither is in the command registry or in `/help`. The same confirmation
guards `/reset`, `/undo` and `/reload-mcp`, and a `/model` switch that Hermes judges costly (read in the
source, `gateway/run_busy.py` and `gateway/slash_commands_model.py`). `/always` switches the confirmation off
for good, by writing `approvals.destructive_slash_confirm: false` to the configuration.

**What changes, and what does not.**

| | Before `/new` | After |
| --- | --- | --- |
| The session key, as Hermes names it | `agent:main:recorder:group:family:alice` | The same |
| The session id | `20261118_085957_fe71da09` | `20261118_090105_892a187a` |

The session key appears in several structured places: the confirmation call, the approval card, and the answer
to `get_session_model_settings`. It did not change. The session id did.

**Where a platform can see the change.**

| Signal | Structured? | What it carries |
| --- | --- | --- |
| The reply to `/approve`: `✨ Session reset! Starting fresh.` with the model and provider | No, text | Nothing a program can rely on but its wording |
| `/status`, sent before and after | No, text | The session id |
| The plugin hook `on_session_finalize` | Yes | The old and the new session id, and the reason `new_session` |
| The plugin hook `on_session_reset` | Yes | The same, 7 ms later |

So **a platform adapter is handed no signal that a session was replaced**, apart from text. No method of the
adapter is called. A plugin gets a signal only if it registers a plugin hook, which is a separate part of the
plugin interface from the platform adapter.

Even then the hooks do not name the chat. `on_session_reset` carries session ids and no session key. A plugin
learns which chat a session id belongs to from `on_session_start`, which fires on the first turn of a session
and also names no chat. The prototype ties it to the turn in progress. The two approval hooks are the only
ones recorded that carry the session id and the session key together.

**The agent still knew the code word.** After the reset it answered `Your code word is **marmot-7** (recorded
on 2026-11-18 in your user profile)`. During the first turn Hermes had written it to its memory, which is kept
across sessions. A new session in Hermes drops the conversation, not what the agent chose to remember. The
notice `📬 No home channel is set…` was sent again on the first message of the new session.

Read in the source, not recorded: a new session also clears `/yolo` and the approvals given "for this
session" (`gateway/run_agent_cache.py`).

### Changing the model

In `model_switch` Alice sent `/model`, then `/model z-ai/glm-5.3:free`, ran a turn, and sent `/model reset`.

- **`/model` alone lists models as text.** `Current: z-ai/glm-5.3-flash:free on Custom endpoint`, then five
  model ids and `(+320 more)`.
- **The switch is answered in text**: `Model switched to z-ai/glm-5.3:free`, `Provider: Custom endpoint`,
  `Context: 262,144 tokens`, `(session only — add --global to persist)`.
- **The next turn used the new model.** `/status` then showed it.
- **`/model reset`** was answered `Session model override cleared — now using z-ai/glm-5.3-flash:free`.

The model is the one setting Hermes will report as data. See [What Hermes hands over as
data](#what-hermes-hands-over-as-data).

### How the plugin tells what became of a command

Read on 2026-11-23 in Hermes's source at commit `40d1e95f`: `gateway/slash_commands_model.py`
(`_handle_model_command_locked`, `_handle_model_reset`, `_commit_model_switch_locked`),
`gateway/slash_commands_session.py` (`_handle_reset_command`), `gateway/platforms/session_model.py`,
`agent/i18n.py`, `hermes_cli/plugins.py` and `locales/en.yaml`. Not run; the recordings hold only the
switch and the `reset_done` reply.

**There is no structured outcome.** A command's handler returns a string, which the gateway sends with
`send`. No hook reports a command's result: `pre_command` fires before the handler and returns nothing,
and Hermes registers no hook after it. `get_session_model_settings` gives the model in effect, not whether
a switch was taken, and a switch to the model already in use leaves it as it was.

**The reply is made from a catalog.** Every reply below is `agent.i18n.t(key, **fields)`, in the language
Hermes is set to (`HERMES_LANGUAGE`, then `display.language`, then English), falling back to English, then
to the bare key. A platform plugin runs in Hermes's process, so it can call `t` itself and know a reply by
its key. That is what the plugin does: it renders each key with a mark in place of each field, and matches
the reply's first line against it. `t` is not in Hermes's published list of what a plugin may use
(inferred: the list was not read again for this).

What `/model <id>` and `/model reset` can answer, from the pinned source:

| Key, and its English text | When | Taken? |
| --- | --- | --- |
| `gateway.model.switched`: ``Model switched to `{model}` ``, then more lines | A switch committed; also a `/model reset` in a local session, which is a switch to the default | Yes |
| `gateway.model.reset_done`: ``Session model override cleared — now using `{model}` …`` | `/model reset` with an override to clear | Yes |
| `gateway.model.reset_none`: ``No session model override is active — already using `{model}` …`` | `/model reset` with no override: nothing changes | Yes |
| `gateway.model.error_prefix`: `Error: {error}` | The model or provider not found, the switch failed on the running agent, `--once` in a local session, or the code on disk newer than the gateway | No |
| `❌ ` and the parser's message, not from the catalog | Arguments Hermes cannot parse | No |
| `gateway.model.guard_confirm` | A costly or restricted model: a question, sent with `send_slash_confirm` | Not yet; its reply to the answer is the outcome |
| `gateway.model.switch_cancelled` | The answer to that question was cancel | No |
| `gateway.model.current_label` and the list | No model named | No |

What `/new` can answer: `gateway.reset.header_default` (`✨ Session reset! Starting fresh.`),
`gateway.reset.header_new` (`✨ New session started!`), or `gateway.reset.header_titled` for `/new <title>`,
each followed by more lines. On Telegram a topic's own header replaces them; it does not reach this
platform. Anything else, such as a refusal for want of rights, is not a new session.

**A model read can say nothing.** `get_session_model_settings` returns None without a runner or for a profile
route it refuses, and it names the session with a blank model when neither the session, the channel nor
`config.yaml` names one. The plugin treats both as no answer: it shows nothing, settles nothing, and asks
again at the next turn when it was the session's first read.

### The model picker

In `model_picker` the recorder claimed a model picker, by defining `send_model_picker` on its adapter. `/model`
then produced no text. Hermes called that method with:

| Field | Value in the recording |
| --- | --- |
| `providers` | One entry: `slug` `custom`, `name` `Custom endpoint`, `models` with one id, `total_models` 1, `is_current` true, and where the list came from |
| `current_model`, `current_provider` | `z-ai/glm-5.3-flash:free`, `custom` |
| `session_key` | The session it is for |
| `on_model_selected` | A function to call with the chat, the model and the provider |

The recorder called that function with `z-ai/glm-5.3:free`, which was not on the list. Hermes accepted it. The
function returned text for the platform to show: `Model switched to z-ai/glm-5.3:free`, with a warning that
Hermes could not check the model against the endpoint. `get_session_model_settings` then reported the new
model, and the next turn used it.

So the picker is a second way to set the model, and the only control in these recordings that is a call and
not a typed command.

### Another setting: the reasoning effort

In `setting_reasoning`, `/reasoning` answered with the effort (`high`), its scope (`global config`) and the
accepted values, all as text. `/reasoning low` answered `🧠 ✓ Reasoning effort set to low (session only — add
--global to persist)`, and a second `/reasoning` showed `low`, scope `session override`. `/status` does not
show the effort. No call reports it.

Hermes has more settings of this kind, each a command that answers in text: `/fast`, `/voice`, `/busy`,
`/personality`, `/footer`. `/busy` changes what a message sent during a turn does (`queue`, `steer` or
`interrupt`), which the design asks each connector to state. Of these only `/busy status` and `/personality`
were sent, in `write_approval`, and neither changed anything. `/busy status` answered `Busy input mode:
interrupt`.

### Commands that change what the agent may do without asking

This is the list a connector needs for
[D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights). It was built by reading every command in
the registry that the gateway accepts (69 of 105) and its handler, and by searching the handlers for where the
approval state is written (`toggle_session_yolo`, `approve_session`, `approve_permanent`, `set_config_value`,
`write_approval`).

| Command | What it does | Status | Evidence |
| --- | --- | --- | --- |
| `/yolo` | Toggles "auto-approve every command" for this session | Recorded | `yolo`: with it on, `rm -rf /tmp/recorder-demo-dir` ran with no prompt. With it off again, the same request was asked about |
| `/approvals off` | Sets the approval mode of the whole profile to `off`, in the configuration file | Recorded | `approvals_off`: the mode was `smart`. After `/approvals off`, `rm -rf /tmp/recorder-demo-dir` ran with no prompt |
| `/approvals smart` | Sets it to `smart`: a second model decides whether a person is asked. This is the default | Recorded as the default | `smart_approved`: that model approved `rm -rf /tmp/recorder-demo-dir` and nobody was asked |
| `/approvals manual` | Sets it to `manual`: always ask | Recorded | `approvals_unordered`: with it set, the request went straight to the chat |
| `/approve session` | Answers an approval, and allows that kind of command for the rest of the session | Recorded | `approve_session`: the reply said `pattern approved for this session`. The same request, sent again, ran with no prompt |
| `/approve always` | Answers an approval, and allows that kind of command for good, in the configuration file | Recorded | `approve_always`: the reply said `pattern approved permanently`. Bob, who has a session of his own, then asked for the same command, and it ran with no prompt |
| `/memory approval off` | Lets the agent write to its memory without asking | Recorded, the switch only | `write_approval`: answered `memory.write_approval set to 'off'`. It was off to begin with, so by default the agent writes to its memory without asking, as it did in `new_session`. No memory write was recorded with it on |
| `/skills approval off` | The same for skills | Recorded, the switch only | `write_approval`: `/skills approval on` was answered `skills.write_approval set to 'on'`. Switching it off was not sent |

A standing approval is wider than the command that was asked about. Hermes approves a pattern, and the
pattern is the reason the command was flagged: `delete in root path`, in every recording here. The hooks name
it as `pattern_key`. So `/approve always` on one `rm -rf` allows every later command flagged for that reason,
for every person and session of the profile. In `approve_always` the second command ran with no call on the
adapter and no hook to say an approval had been skipped; the agent's own reply mentioned it.

Looked for and not found:

- **A command that raises privileges or changes a sandbox.** Hermes has no `/elevated`, `/sudo`, `/exec` or
  `/sandbox` among its 105 commands.
- **A command that edits the configuration in general.** `/config` exists and is for the terminal only.
- **A way to give `/yolo` a value.** It only toggles. A connector that sends it must know the current state,
  which Hermes reports only in the reply.

Commands that are near this list and not on it:

| Command | Why it is mentioned |
| --- | --- |
| `/approve`, `/deny` | Answer one approval. Already on the connector's list |
| `/always` | Answers a confirmation of a typed command and switches that confirmation off for good. It is about the person's own commands, not the agent's |
| `/codex-runtime` | Switches the agent to another runtime, which has permission settings of its own. Not examined |
| `/goal`, `/loop`, `/heartbeat`, `/bg` | Make the agent run again without a new message from anyone. They do not change approvals. Not recorded |
| `/reload-mcp`, `/restart`, `/update` | Change the tools or the running program. Not recorded |

**What a platform sees when approvals are switched off.** In `yolo`, after `/yolo`, the platform was handed
the one-line reply, then `💻 Running rm -rf /tmp/recorder-demo-dir`, then the agent's answer. No call marked
the command as having skipped an approval. The hook `pre_approval_request` did not fire either.

**The default mode already skips some approvals.** Hermes's approval mode is `smart` unless someone changes
it: `/approvals` answered `Approval mode: smart` in `control_catalog`. In `smart_approved` the request for
`rm -rf /tmp/recorder-demo-dir` produced no prompt. The hooks show why: `pre_approval_request` with
`surface: smart`, then `post_approval_response` with `choice: smart_approve` and `decided_by: aux_llm`,
14 seconds later. The command ran. In `yolo` the same request, with the same settings, was passed on to a
person 30 seconds after the same first hook. So with the default mode, whether anyone is asked about a given
command differs from run to run. A platform that registers no hooks cannot tell an approval that the second
model gave from a command that needed none.

### Hermes has its own list of who may use commands

Read in `gateway/slash_access.py` and recorded in `command_not_admin`.

A platform's configuration can name admins, for direct chats (`allow_admin_from`) and for groups
(`group_allow_admin_from`), and the commands other people may use (`user_allowed_commands`). With no admin
named, everyone who may talk to Hermes may use every command. That is the default, and `/whoami` said so:
`Tier: unrestricted (no admin list configured for this scope)`.

With Alice named as the group's admin, Bob was refused `/yolo`, `/approvals off`, `/model`, `/new` and
`/stop`, each with `⛔ /yolo is admin-only here…`. He could still use `/help` and `/whoami`, and `/help` listed
only those two. The refusal is text. The hook `pre_command` did not fire for a refused command.

Two things follow. A person who is not an admin cannot interrupt. And the split is all or nothing: the list
of commands other people may use is one list, with no notion of "approve" as against "prompt". A person given
`approve` is given `deny` too.

Who may answer an approval is a separate setting, `approvals.approvers`. Its default is `["*"]`: anyone who
may talk to Hermes in that chat (read in `tools/approval_context.py`, not recorded with any other value).

### Commands are answered out of order

`approvals_unordered` was meant to show `/approvals off`. Hermes took 19 seconds to answer the first command
of the session. The recorder had moved on, so five messages were waiting at once. The answers came back in the
order second, first, fifth, fourth:

```text
/approvals           ->  Approval mode: off      (sent first, answered second)
/approvals off       ->  Approval mode: off      (answered first)
/approvals manual    ->  Approval mode: manual   (answered last)
/approvals           ->  Approval mode: off      (sent last, answered third)
```

Each answer names the message it replies to, so they can be matched. But an answer does not describe the
state at the time its command was sent. The first `/approvals` was sent before anything had changed and
reported `off`. The last was sent after `/approvals manual` and also reported `off`. The mode ended as
`manual`. A connector that sends a control as a command cannot assume that commands are handled in the order
sent, and cannot take a reply as the current value without knowing what else was waiting.

### What Hermes hands over as data

| Wanted | As data? | Where |
| --- | --- | --- |
| The commands | Not through the plugin interface | See below |
| The models | Yes | `list_session_model_picker_providers(source)` on the adapter: for each provider its `slug`, `name`, `models`, `total_models` and `is_current`. Or `send_model_picker`, if the platform claims it |
| The model in use | Yes | `get_session_model_settings(source)` on the adapter: `session_key`, `model`, `provider`, `base_url` |
| The reasoning effort, `/yolo`, the approval mode, or any other setting | No | Only the text of the command's reply |
| The session id | Only through plugin hooks | `on_session_start`, `on_session_reset`, `pre_approval_request` |
| That a session was replaced | Only through plugin hooks | `on_session_reset` |

Both model calls are in Hermes's published list of what a plugin may use. In `control_catalog` the first
returned the model in use, and the second returned one provider, `custom`, with one model. The list was that
short because this call does not ask a custom endpoint for its models (read in its source). `/model` as text
named five models and `(+320 more)`.

**The commands.** `/help` and `/commands` answer in text, in pages: `/commands` reported 110 in 6 pages. No
method of the adapter and no hook returns the list. The list does exist as data inside Hermes, as
`COMMAND_REGISTRY` in `hermes_cli/commands.py`, and a plugin runs in the same program and can import it.
Hermes's own Discord and Telegram adapters do. It is not in the published list, and Hermes's plugin check
reports such an import as a warning. `control_catalog` records what it holds: 105 commands, 69 of them
accepted by the gateway, each with a name, a description, a category, aliases, a hint for its arguments, and
its sub-commands. Nothing in an entry says that a command changes what the agent may do, or that it needs an
admin.

**The plugin hooks.** A Hermes plugin can register hooks beside its platform. Nine were registered for these
recordings:

| Hook | Fired | What it carried |
| --- | --- | --- |
| `pre_command` | Before each slash command that ran. Not for a refused one, and not for `/approve` or `/deny` | The command, its arguments, the session key |
| `on_session_start` | On the first turn of a session | The session id and the model |
| `on_session_end` | At the end of every turn, despite its name | The session id, a turn id, how the turn ended |
| `on_session_finalize`, `on_session_reset` | When `/new` was confirmed | The old and the new session id |
| `pre_approval_request` | When a command needed approval, once for the second model and once for a person | The command, why it was flagged, the session key, the session id, a tool call id |
| `post_approval_response` | When an approval was answered or timed out | The same, and the choice. Seen: `session`, `always`, `deny`, `timeout`, and `smart_approve` with `decided_by: aux_llm` |
| `on_session_open`, `agent_loop_stopped` | Never, in these recordings | |

Hermes's list of hooks is longer (read in `hermes_cli/plugins.py`). It includes `pre_tool_call` and
`post_tool_call`. Those were not registered and not recorded. They bear on the first conclusion of this page,
that the Hermes connector cannot report tool calls as items: that conclusion holds for the platform adapter,
and has not been tested against the hooks.

Hermes also has a second interface, used by its own desktop and terminal clients, which offers a command
catalogue and settings as data (read in passing in `hermes_cli/commands.py`, which builds it). It is not what
a platform plugin uses, and was not examined.

### The new recordings through the prototype translator

The prototype was extended to the new recordings on 2026-11-18. All 27 Hermes recordings translate with no
line left without a rule and no item left open.

| | First 14 recordings | All 27 |
| --- | --- | --- |
| Session items | 16 | 32 |
| Edits of a session item | 0 | 29 |
| Largest session item, without the native message | Empty | 9.0 KB |

What changed in the rules:

- **A session item now holds what Hermes reports.** The model and the provider, when the plugin asks or a hook
  says. The accepted models, when the plugin asks. The commands, when the plugin reads the registry.
- **A replaced session ends one item and starts another.** The prototype names a Hermes session after the chat
  and the sender, as before, and adds a count: `group:family:alice`, then `group:family:alice#2`. It does so
  on `on_session_reset` when the hook is registered, and otherwise on the reply that starts with `✨`.
- **Four kinds of setting are read from wording.** After `/yolo`, `/approvals <mode>`, `/reasoning <level>`
  and `/memory approval <on|off>` the prototype puts the value into the session item from the text of the
  reply. It marks all but the reasoning effort as needing the right to approve
  ([D-043](../decisions.md#d-043-a-setting-says-which-right-it-needs)). This is as fragile as the rules for
  tool lines.
- **The request to confirm `/new` is an `approval` item.** It is the nearest type. It differs from every other
  approval in the recordings: it is about something a person asked for, not something the agent wants to do.
- **The reply to a slash command is a notice** that names the command, whatever its first character. A refusal
  is a notice of kind `refused`.

The largest session item is the one in `control_catalog`, which carries 69 commands with their descriptions.

### After a crash

Seen on 2026-11-21 in the Hermes plugin's scenario (`crates/marmot-agent/tests/hermes.rs`), same Hermes commit,
and read in `gateway/session_ingress.py`. Hermes was killed while a turn ran a command, and started again on
the same home. The first message to that session after the restart was not run. Hermes answered it with:

```text
⏸️ This conversation is paused: a previous turn did not finish when Hermes restarted, and only you know
whether to send it again. Your message is saved. Send /discard to drop the lost turn and continue (it is not
re-run), or /reset to start a fresh conversation.
```

and reported the turn complete. The lost turn is never run again by itself. `/discard` lets the saved message
run. The notice is text only; the reason (`unknown_execution`) is not handed to a platform. Also seen: the
session was otherwise kept, under the same session key, so the session survives a restart.

### What a connector has to supply from its own knowledge

[D-044](../decisions.md#d-044-a-connector-supplies-what-its-harness-does-not-say) says a connector fills in
what its harness does not report. For Hermes, from the above:

1. **The command for each shared control**: `/stop`, `/new`, `/model <id>`.
2. **That `/new` is answered with a question**, and that the answer is `/approve`.
3. **Every setting but the model**: that it exists, its values, and the command that sets it. For `/yolo`,
   also that it is a toggle.
4. **The value of those settings**, by reading the wording of replies, or by keeping its own record of what
   it sent.
5. **Which settings and commands need the right to approve.** Nothing Hermes hands over marks them. The table
   above is the list for this version.
6. **The session's identity.** The session key is stable and the adapter is given it. The session id, and the
   fact that it changed, come only from plugin hooks.
7. **The command list**, from the registry Hermes does not publish, or from the connector's own copy.

Two points for the people deciding, not for the connector:

- **`/approve` cannot be put under one right.** It answers an approval and it confirms `/new`. Under D-032 a
  message that starts with `/approve` needs the right to approve. A person with only the right to prompt could
  then send `/new` and not confirm it.
- **The default approval mode asks a model, not a person, first.** Decision
  [D-021](../decisions.md#d-021-approval-behaviour-belongs-to-the-harness) leaves approval behaviour to the
  harness, so this is Hermes behaving as designed. It does mean that a group sees no approval for some
  commands that Hermes itself flags as dangerous.

## What the plugin relies on outside the published plugin interface

Listed on 2026-11-23 by reading the plugin's code at that day's `master` (`python/marmot_hermes`) and its tests,
for Hermes at commit `40d1e95f`. Not re-checked against a newer Hermes, and Hermes's published list of what a
plugin may use was not read again for this: where this table says a call is not in it, that is from
[what Hermes hands over as data](#what-hermes-hands-over-as-data); where it says it is not known, nobody has
checked. A [channel plugin](../design/drivers.md#channel-plugins) lists here everything it uses that Hermes may
not publish for plugins, with the test that fails when it changes. The rest is the platform adapter's own
interface (`BasePlatformAdapter`, `MessageEvent`, `SendResult`) and `ctx.register_platform`.

| What | Read for | Published? | What fails when it changes |
| --- | --- | --- | --- |
| `COMMAND_REGISTRY` and `_is_gateway_available`, from `hermes_cli.commands` | The commands a session item lists ([D-044](../decisions.md#d-044-a-connector-supplies-what-its-harness-does-not-say)) | No; Hermes's plugin check warns about the import | The move-over test, `a_hermes_install_moves_over` in `crates/marmot-agent/tests/import_gateways.rs`, which runs real Hermes on the scripted model double and checks that the session item lists `/new` (added 2026-11-24). The plugin logs a warning and the session item lists no commands; `python/tests` fake it |
| `resolve_gateway_approval_card`, from `tools.approval` | Answering an approval card, as the person who answered | Yes (read on 2026-11-24, below) | The approval step of the end-to-end scenario: the command is never run |
| `resolve_gateway_clarify`, from `tools.clarify_gateway` | Answering a question | Yes (read on 2026-11-24, below) | The same move-over test: the scripted model asks a question with Hermes's `clarify` tool, the person answers with the second choice, and the model must be handed it as the tool's `user_response` (added 2026-11-24). `python/tests` fake the call |
| `clear_if_stale`, `get_pending` (and the `created_at` of what it gives), `resolve` and `DEFAULT_TIMEOUT_SECONDS`, from `tools.slash_confirm` | Answering a request to confirm a command by its id, while Hermes would still run it ([D-146](../decisions.md#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id)) | No (read on 2026-11-24, below) | The move-over test, `a_hermes_install_moves_over`, which runs real Hermes on the scripted model double, asks for a new session from the chat, answers Hermes's request to confirm it, and checks that Hermes's reply comes back outside any turn, which a typed `/approve` would not. `python/tests` stand in for the module as the pinned commit has it |
| `slash_confirm_refusal` and `refusal_text`, from `gateway.approval_actor` | Asking Hermes whether the person who answered a request to confirm a command may answer it, with the chat's settings, and Hermes's words when not ([D-146](../decisions.md#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id)) | Yes (read on 2026-11-24, below) | The same step of the move-over test, whose Hermes names no admins, so the check lets the answer through; a change that breaks the import fails it. `python/tests` stand in for the check |
| `t`, from `agent.i18n` | Telling from a reply to `/model` or `/new` whether Hermes took it ([above](#how-the-plugin-tells-what-became-of-a-command)) | Not known | For `/model`: the move-over test, `a_hermes_install_moves_over` in `crates/marmot-agent/tests/import_gateways.rs`, which runs real Hermes on the scripted model double, switches the model and resets it, and fails when a reply is not known as taken (`gateway.model.switched`, `gateway.model.reset_done`); the end-to-end scenario's last step does the same on a real model. For `/new`: the scenario's new session, step 4. `python/tests` read the pinned English texts from `python/tests/hermes_phrases.json`. The plugin refuses both controls as `not_supported` when `t` knows none of the keys |
| `_mark_connected` and `_mark_disconnected`, helpers of `BasePlatformAdapter` | Telling Hermes the platform is up or down | Not known; the leading underscore suggests not (inferred) | The scenario's first step, if a change makes the call fail: the platform then does not start (inferred, not tried) |

Each row now has a test that fails when it changes, as [D-118](../decisions.md#d-118-the-channel-plugin-contract-is-accepted-as-written)
asks; the last two were added on 2026-11-24. The move-over test runs in CI's merge-queue tier, not on a pull request.

Read on 2026-11-24 in Hermes's source at commit `40d1e95f`, in the image `tools/record/hermes` builds: Hermes's
list of what a plugin may import, `hermes_cli/data/plugin_api.json`, names `resolve_gateway_approval_card` and
`resolve_gateway_clarify` under "Public helpers for platform plugins", with `slash_confirm_refusal` from
`gateway.approval_actor`, and names nothing of `tools.slash_confirm`. The base adapter's `send_slash_confirm` tells
a platform with buttons to resolve the confirmation through `GatewayRunner._resolve_slash_confirm`, which that
commit does not have; Hermes's own Telegram and relay platforms call `tools.slash_confirm.resolve`. That call runs
the confirmation only when the id it is given is the one waiting in the session, once, and within 300 seconds of
the request (`DEFAULT_TIMEOUT_SECONDS`), and says `None` both when it ran nothing and when what it ran gave no
text. It checks no sender: each platform asks `slash_confirm_refusal` first, with the person who pressed and the
platform's settings (Telegram passes its `config.extra`), which reads the admins of the chat's kind
(`allow_admin_from` for a direct chat, `group_allow_admin_from` for a group) and lets only someone who may run the
confirmed command answer when there are any; with none, everyone Hermes admits may. The text a person types is read by
`_hm_slash_confirm_reply` in `gateway/run_inbound.py`, which leaves it to an approval when one waits in the
session (`has_blocking_approval`) and otherwise answers whichever confirmation waits there, by no id.
