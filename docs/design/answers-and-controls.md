# Answers and controls

Status: decided on 2026-11-18, as [D-027](../decisions.md#d-027-answers-and-controls-are-one-kind-of-event-kind-1202)
to [D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights), and [D-039](../decisions.md#d-039-controls-need-the-right-to-prompt)
to [D-041](../decisions.md#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat). Later decisions
that came from the recordings are listed [at the end](#what-was-decided). None of it is in the Marmot
specification yet, and the exact tags are not fixed.

This is the second half of [how an agent event is carried](event-carrier.md): that page covers what an agent
sends to the chat, and this one covers what a person sends back.

## The two things a person sends

| | What it is | Examples |
| --- | --- | --- |
| **Answer** | A response to something the agent asked | "Allow once" on an approval. "Hello" on a question |
| **Control** | Something a person does unprompted | Interrupt. Switch model. Start a new session |

An ordinary chat message is neither. It is a prompt, and it stays a normal chat message.

## What it has to do

1. **Say exactly what it answers.** An approval must never be granted by a message meant for something else.
2. **Reach the right agent.** A group can hold more than one.
3. **Show everyone what happened.** In a group, every member sees who allowed what
   ([D-009](../decisions.md#d-009-groups-are-first-class-and-every-member-sees-agent-activity)).
4. **Stay true to each harness.** A harness offers its own models, modes and commands. The chat should offer
   those, not a list we invented ([D-008](../decisions.md#d-008-pass-everything-through)).
5. **Respect the owner's rules** about who may do what
   ([D-010](../decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)).

## What the recordings say

Read from the recordings of all four harnesses. See [`../reference/harness-messages.md`](../reference/harness-messages.md),
[`../reference/hermes-messages.md`](../reference/hermes-messages.md) and
[`../reference/openclaw-messages.md`](../reference/openclaw-messages.md).

| | Codex | Claude Code | Hermes | OpenClaw |
| --- | --- | --- | --- | --- |
| Answer an approval | A reply to a request, naming a decision | A reply to a callback: allow or deny | A call with a request id, or `/approve` | A call with an approval id, or `/approve <id>` |
| Answer a question | A reply with the chosen options | A reply through the approval callback | A chat message | Its own call with a question id. A chat message was not taken |
| Several questions in one request | Yes | Yes, and several selections each | No | No |
| Interrupt | A request | A call | The message `/stop` | The message `/stop` |
| Switch model | A field on the next turn | A call between turns | Not recorded | Not recorded |
| Switch mode | A field on the next turn | A setting | Not recorded | Not recorded |
| Compact | A request, which runs as its own turn | The message `/compact` | Not recorded | Not recorded |
| Slash commands | None in this interface | 59, sent as ordinary messages | Sent as ordinary messages | Sent as ordinary messages |

Three things follow:

- **Every harness names the request being answered.** All four give an approval an id. So an answer can always
  point at one exact item.
- **What can be set differs by harness.** There is no shared list of models or modes to build in.
- **Slash commands are already messages** in the three harnesses that have them.

## The design

### 1. One new kind carries both, kind 1202

An answer or a control is an event of kind 1202, sent by a person's app to the group. A tag says which action
it is ([D-027](../decisions.md#d-027-answers-and-controls-are-one-kind-of-event-kind-1202)).

Kind 1202 is reserved for agent events and has no definition in the specification. MDK today uses it for rows
an agent sends about its own operations. That use ends with the old connectors.

### 2. It names its agent, and what it acts on

| Part | Where | Example |
| --- | --- | --- |
| The plain rendering | `content` | `Allow once` |
| The agent it is for | a `p` tag, as mentions use | the agent's account |
| What it acts on | an `e` tag, as replies use | the approval item, or the turn to interrupt |
| The action | a tag | `answer`, `interrupt`, `set` |
| The action's details | a tag holding JSON | the choice's id; the setting and its new value |

An agent ignores an event that does not name it.

The `e` tag points at an item's first event, which is how an item is identified
([D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit)).

### 3. The agent's reply is what counts

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/answer-dark.svg">
  <img alt="A person presses Allow once. Their app sends an answer that points at the approval item. The agent checks it, passes it to the harness, and edits the approval item to say who answered. The edit is what every member's app shows." src="../images/answer-light.svg">
</picture>

Sending an answer or a control changes nothing by itself. It is a request. The agent resolves each one with
exactly one event that points back at it ([D-028](../decisions.md#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it)):

- **If it had an effect, the edit that shows the effect.** The approval item becomes "Answered: Allow once, by
  Alice". The turn becomes "Interrupted by Alice". The session's settings show the new model.
- **If it had none, a notice that says why**: not allowed, not supported by this harness, nothing to act on,
  already answered, or failed. One the agent cannot read is settled the same way: `not_supported` for an action
  it does not know, as from an app newer than the agent, and `invalid` for anything else it cannot read
  ([D-140](../decisions.md#d-140-an-answer-or-a-control-the-agent-cannot-read-is-refused)). The agent reads
  each tag as MDK does, and so as a person's app does: a `details` or `e` tag given twice, or an `e` that is not
  an event id, makes the request `invalid`, as a field given twice in `details` does, and one that names an
  agent twice, or two agents, is for none of them. (Provisional, 2026-11-24: found by the property test that checks the core against MDK's
  reader. Before, the core acted on the first of each, so it could act on a press no app shows.)
- **If the harness took a control and two minutes pass with nothing to show for it**, a notice with the reason
  `no_effect` ([D-087](../decisions.md#d-087-the-core-settles-a-request-whose-effect-never-shows)). An effect
  that shows later is still shown. What counts is when the harness's output reached the core, not when the core
  read it: the core settles a control this way only once it has read everything that reached it by the
  deadline, so an effect shown in time while the core was busy, as when it waited on the driver, still settles
  it. Output that arrives after the deadline does not hold the notice up. (Decided by the project owner on
  2026-11-25, for [#198](https://github.com/marmot-protocol/marmot-agents/issues/198).)

So there is one place to look for the truth: the agent's own items. An app shows a press as pending until the
agent's reply arrives. If the agent's machine is off, the press stays pending and nothing is assumed.

An answer or a control is not a row in the chat, does not alert anyone, and does not count as unread.

### 4. Rules for answers

Carried over from the earlier approvals design, MDK issue 858 and Marmot pull request 503. That design also
required an acknowledgement that harnesses do not give, and that part is dropped.

- **An answer counts only if it points at the exact item** and comes from someone allowed to give it.
- **Ordinary text is never an answer to an approval.** "Yes" or "go ahead" in the chat is a prompt.
- **An answer never reaches the model as a prompt.** It goes to the harness's own answer call.
- **An item is answered once.** The first valid answer the agent receives wins. Later ones get the notice
  "already answered".
- **The agent never answers for anyone**
  ([D-021](../decisions.md#d-021-approval-behaviour-belongs-to-the-harness)).

Added here:

- **An answer names choices by the ids the item offered.** A question that allows free text takes text as well.
- **One answer can cover several questions.** Codex and Claude Code both put several questions in one request.
- **An answer for an item that is no longer waiting has no effect.** The harness may have expired the request,
  or the turn may have ended.

Two exceptions stay, because the evidence forces them.

- **A question that is answered by a message.** A question whose `answer_via` is `message` has nothing to
  answer: the harness simply waits for the person's next message. Codex does this outside plan mode, and Hermes
  always. For such a question the reply is an ordinary chat message.
- **A gateway that takes an approval as text.** Hermes accepts `/approve` and `/deny` as chat messages, and
  OpenClaw accepts `/approve` with an id. A typed message is a prompt, so without a rule, someone who may prompt
  and may not approve could approve by typing. The rule: a connector lists the commands that need the right to
  approve, and the core checks every message against that list
  ([D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights)). A command that changes the permission
  mode belongs on the list too. Each entry says whether it counts anywhere in a message or only at the start
  ([D-047](../decisions.md#d-047-a-listed-command-says-where-in-a-message-it-counts)). The core reads a message as
  any harness may, in every case and after any space, and asks for the right to approve when any reading finds a
  listed command ([D-150](../decisions.md#d-150-the-core-matches-a-typed-command-as-the-harness-reads-it-failing-towards-asking-for-the-right-to-approve)).

### 5. Controls: a few shared ones, and whatever the harness offers

Only three actions mean the same thing everywhere ([D-029](../decisions.md#d-029-three-shared-controls-and-the-rest-comes-from-the-harness)):

| Action | What it does | Acts on |
| --- | --- | --- |
| `interrupt` | Stops the running turn, or, in a subagent's thread, that subagent and those it started ([D-149](../decisions.md#d-149-stop-in-a-subagents-thread-stops-that-subagent-alone)) | The turn, or a subagent's `task` item or an item in its thread |
| `new_session` | Ends this chat's session and starts a fresh one | The session |
| `set` | Changes one setting of the session | The session |

Everything else comes from the harness. Each driver publishes, per session:

- **Settings**: each with an id, a label, its current value and the values it accepts. Model and mode are
  settings. So is a harness's permission mode, which is how a chat changes it
  ([D-021](../decisions.md#d-021-approval-behaviour-belongs-to-the-harness)).
- **Actions** of its own: each with an id, a label and a description. Compacting is one.
- **Slash commands**: each with a name, a description and a hint, so an app can complete them as someone types.

An app draws its controls from that list. Nothing in an app or in the core checks a harness's name.

### 6. Where that list lives: a session item

The list has to be in the chat, so that an app has it before anyone presses anything, and has the last known
one when the agent's machine is off.

It is an eleventh item type, `session` ([D-030](../decisions.md#d-030-a-session-is-an-item-the-eleventh-type)). An agent
sends one when a session starts in a chat, and edits it when a setting changes or the session ends. It carries
the harness and its version, the session's id, and the three lists above.

This changed [the event model](event-model.md), which had ten types. It fits the evidence: Claude Code opens
every session with a message that lists its model, permission mode and slash commands, and Codex reports a
thread's settings.

The alternative was a `notice` of a new kind. A notice is a line of information for people. This list changes
over time, and an app has to find it by its type.

### 7. A slash command is a message

A message that starts with a slash is passed to the harness exactly as typed
([D-031](../decisions.md#d-031-a-slash-command-is-a-message-for-the-harness)). It is the harness's command, and three of the four
harnesses already take commands as messages.

The core has no typed commands of its own. Its controls are events of kind 1202, which an app sends from a
button or a menu. An app may also let someone type a control, but what it sends is still the event.

A harness has many commands of its own; Claude Code alone advertises 59. The core's controls cannot collide
with them, because one is text and the other is not.

The cost: a Marmot client with no support for agents can prompt an agent and use its slash commands, and cannot
approve, interrupt or change settings.

### 8. A message sent during a turn is still a message

Steering is not a control. A chat message that arrives while a turn is running is a prompt like any other. What
happens to it depends on the harness: Codex can fold it into the running turn, Hermes does so by itself, Claude
Code ends the turn and starts another, and OpenClaw holds it until the turn ends.

The core keeps no queue of its own. It passes the message on at once, and the harness does what it does
natively ([D-040](../decisions.md#d-040-a-message-sent-during-a-turn-goes-straight-to-the-harness)). Each
connector states which behaviour its harness has, so an app can say what will happen before the message is
sent. In a group, with a harness that ends the turn, a second person's message cuts off the first person's turn.

### 9. Who may send what

[D-010](../decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve) gives the owner two rights to
hand out: who may prompt, and who may approve. Both default to the owner alone. Every action needs one of them,
and there is no third right ([D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights)).

| Action | Right needed | Why |
| --- | --- | --- |
| Send a prompt or a slash command | Prompt | Except a command on the connector's list, which needs the right to approve |
| Answer a question | Prompt | The answer is information for the agent, like a prompt |
| Interrupt | Prompt | Anyone who can start work can stop it |
| Start a new session | Prompt | |
| Change a setting other than the permission mode, or use a harness's own action | Prompt | |
| Answer an approval | Approve | |
| Change the permission mode | Approve | It decides what the agent may do without asking, so it is a standing approval |
| Change a coding harness's folder | Approve | It changes what the agent can touch ([D-101](../decisions.md#d-101-a-chat-can-move-a-coding-harness-to-another-folder)) |

D-010 first put "use controls" with the right to approve. [D-039](../decisions.md#d-039-controls-need-the-right-to-prompt) moved
them to the right to prompt. The reason: a harness's slash commands are messages, and they include commands
that stop a turn, start a session and switch the model. Someone who may prompt can type those. A stricter rule
for the button than for the typed command would protect nothing.

Someone without the right gets the "not allowed" notice, which is the refusal D-010 asks for. An event the
agent cannot read cannot say which right it needs, so either right is enough to be told why it was not read, and
someone with neither is told only "not allowed"
([D-140](../decisions.md#d-140-an-answer-or-a-control-the-agent-cannot-read-is-refused)).

The folder is the one setting the core holds itself rather than the harness. For a coding harness the core adds
`folder` to each session item, marked as needing the right to approve, and a `set` of it starts the chat's next
session in the new folder; how, in [the drivers page](drivers.md#a-chats-folder) (built, provisional).

### 10. Late and repeated events

An answer or a control can arrive long after it was sent, for example when the agent's machine was off.

- **One that points at an item acts only if that item is still live.** A late "interrupt" for a turn that has
  ended does nothing. A late answer to an expired approval does nothing.
- **One that points at nothing is dropped if it is old.** A "new session" sent an hour ago should not act when
  the machine comes back. The limit is five minutes. That is a first value with no evidence behind it.
- **Each event acts at most once.** The agent remembers the ids it has resolved.

## Checked against the recordings

The prototype produces `session` items for every recording, and counts which native call each control was
seen as. First run on 2026-11-18 over 90 recordings. To fill the gaps it showed, 33 more were recorded the
same day: 7 with Codex and Claude Code, 13 with Hermes and 13 with OpenClaw. That makes 123.

```sh
python3 tools/prototype/translate.py --check fixtures/recordings/codex fixtures/recordings/claude \
    fixtures/recordings/hermes fixtures/recordings/openclaw
```

The result: 128 sessions and 79 edits of a session item, with no native message left without a rule.

### What each harness says about a session

| | Codex | Claude Code | Hermes | OpenClaw |
| --- | --- | --- | --- | --- |
| A session id | Yes | Yes, once the first turn starts | A key for the chat and the sender. The id behind it is not given to a platform | Yes, but it does not change when the session is replaced |
| Settings and their values | Model, reasoning effort, mode, approval policy, sandbox | Model, permission mode | The model, when asked. Everything else only as the text of a reply | Model and thinking level before each run. A session's changed settings, when asked |
| The values a setting accepts | Models and modes, when asked. Not approval policies or sandboxes | Models, when asked. Not permission modes | Models, when asked | Models, and the values each command's argument accepts, when asked |
| Slash commands | It has none on this interface | Names unasked. Descriptions and hints when asked | Not offered to a plugin. Hermes has 105 | 51, when asked |
| First reported | Before any prompt | Before any prompt when asked, except the session id and the full model name | At the first turn | At the first run |
| A session ending | Nothing says so | Nothing says so | A line of text. A plugin hook gives the old and new id, and not the chat | A status notice. Neither the key nor the id changes |
| Sessions in the recordings | 29 | 30 | 32 | 37 |
| Edits | 35 | 6 | 29 | 9 |
| Largest item, without the native message | 0.8 KB | 15 KB | 9 KB | 5 KB |

For the gateways, "when asked" means a call the plugin makes into the gateway. Whether those calls answer
before the first message arrives was not checked.

What the edits were:

- **Codex, once in every session.** It states a thread's settings in three places under different names. The
  first is incomplete, so the item is edited when the first turn starts: the mode appears, and the effort
  becomes the one the prompt asked for.
- **Codex, a setting changed by the client, four times.** The model three times, and the approval policy and
  sandbox together once. One model change was part of the first edit.
- **Codex, on resume, twice.** See below.
- **Codex, a session ended by the driver, once.**
- **Claude Code, six.** A model change, two permission mode changes, the command list gaining descriptions
  after the driver asked, the model's full name arriving with the first turn, and a session ended by the
  driver.

### What it showed

1. **A resumed Codex thread came back with a wider sandbox.** In the two recordings that resume a thread
   without sending its settings, it was started with a sandbox limited to the project and came back with full
   access, the default configured on the recording machine. A third recording sent the approval policy and
   the sandbox again with the resume, and they held. The driver must do that
   ([rule 10 for drivers](event-model.md#what-a-driver-has-to-do)). The session item is what made this visible.
2. **The two coding harnesses can describe a session before anyone prompts.** Codex reports a thread's
   settings in its response to `thread/start`. Claude Code says nothing unasked until the first turn, but when
   asked it returns its commands, models and permission mode in under two seconds, with no prompt sent. It
   does not give the session id until a turn starts. The gateways say nothing unasked until a message arrives.
3. **The gateways say little unasked, and more when asked.** In the first run Hermes's item was empty and
   OpenClaw's held the model and the thinking level. OpenClaw gives a plugin its 51 commands, its models and
   a session's settings as data. Hermes gives the model and the list of models; its commands are not offered
   to a plugin, and its other settings appear only in the text of a reply. Neither marks which commands or
   settings change what the agent may do. The rest is what
   [D-044](../decisions.md#d-044-a-connector-supplies-what-its-harness-does-not-say) covers.
4. **A setting does not say which right it needs.** Codex has two settings that need the right to approve,
   and Claude Code has one. This is what [D-043](../decisions.md#d-043-a-setting-says-which-right-it-needs) answers.
5. **Both coding harnesses confirm a changed setting at once.** Codex has a call for it,
   `thread/settings/update`, and reports the new settings within a millisecond, with no prompt sent. Claude
   Code reports a new permission mode in a status message. In the first 90 recordings Codex's model was
   changed as a field on the next prompt, which is confirmed only when that turn starts; the dedicated call is
   the one to use.
6. **Changing the permission settings works, and takes effect on the next action.** After Codex's approval
   policy and sandbox were tightened, its next command asked for approval. After Claude Code's permission mode
   was loosened, its next edit did not.
7. **No harness says clearly that a session is over.** A second Codex thread and a second Claude Code session
   each started clean, and nothing was sent about the first. Hermes says so in a line of text. OpenClaw sends
   a status notice, and its session key and id stay the same, so a connector has to count the resets itself.
   The driver ends the session item in every case.
8. **A setting cannot be changed during a running turn in Codex.** `turn/settings/update` was refused, because
   it needs a feature that is off by default. The refusal becomes the "not supported" reply.
9. **The lists change.** Codex listed eight models on 2026-11-17 and ten the next day.
10. **Both gateways have typed commands that switch approvals off.** See
    [below](#typed-commands-that-widen-what-an-agent-may-do).
11. **A new session does not clear everything.** After `/new`, the Hermes agent still knew a word from the
    earlier session, because it had saved it to Hermes's own memory.

What was decided from the first run, on 2026-11-18:

| Point | Decision |
| --- | --- |
| A setting carries a mark when changing it needs the right to approve | [D-043](../decisions.md#d-043-a-setting-says-which-right-it-needs) |
| A connector supplies the settings, accepted values and commands its harness does not report | [D-044](../decisions.md#d-044-a-connector-supplies-what-its-harness-does-not-say) |
| The command list keeps names and hints, and loses descriptions first | [D-045](../decisions.md#d-045-a-session-items-commands-keep-their-names-and-lose-descriptions-first) |
| A setting change is an edit, and a notice only when nobody asked for it | [D-046](../decisions.md#d-046-a-setting-change-is-an-edit-and-a-notice-only-when-nobody-asked-for-it) |

### Which native call each control is

Counts are how often each was recorded.

| Control | Codex | Claude Code | Hermes | OpenClaw |
| --- | --- | --- | --- | --- |
| Interrupt | `turn/interrupt` (2) | `interrupt()` (2), `abort()` (1) | The message `/stop` (1) | The message `/stop` (1) |
| New session | `thread/start` (29) | `query()` (30) | `/new`, then `/approve` to confirm (1) | `/new` (1), `/reset` (1) |
| Set the model | `thread/settings/update` (1), or a field on `turn/start` (2) | `setModel()` (1) | `/model <id>` (1), `/model reset` (1), or a pick from its model list (1) | `/model <provider/model>` (2), `/model default` (1) |
| Set another setting | The mode, as a field on `turn/start` (1) | Not recorded | `/reasoning <level>` (1) | `/think` (2), `/verbose` (1), `/reasoning` (1) |
| Set the permission mode | `thread/settings/update`, for the approval policy and the sandbox (1) | `setPermissionMode()` (1) | `/yolo` (4), `/approvals <mode>` (4) | `/exec security=… ask=…`, for one run (4). `/elevated <level>` (3) |
| The harness's own action | `thread/compact/start` (1) | The message `/compact` (1) | Not recorded | Not recorded |
| Resume a session | `thread/resume` (3) | `query()` with `resume` (2) | Not recorded | Not recorded |
| Answer an approval by typing | | | `/approve` (1), `/approve session` (1), `/approve always` (1), `/deny` (2) | `/approve` (9), `/approve <id> allow-always` (1) |

The three shared controls are now recorded for all four. For the two gateways every one of them is a typed
command, so their connectors turn a control into text.

### Typed commands that widen what an agent may do

Each connector lists the typed commands that need the right to approve, and the core checks a message that
starts with one ([D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights)). These are the lists, as found on
2026-11-18. "Recorded" means the effect was seen: a command that normally needs approval ran with none.

| Gateway | Command | What it does | Seen |
| --- | --- | --- | --- |
| Hermes | `/yolo` | Switches approvals off for the session | Recorded |
| Hermes | `/approvals off`, `smart`, `manual` | Sets who decides: nobody, a second model, or a person | Recorded |
| Hermes | `/approve session`, `/approve always` | Answers an approval and keeps answering it | Recorded |
| Hermes | `/memory approval`, `/skills approval` | Whether the agent may write its memory and skills unasked | The switch only |
| Hermes | `/codex-runtime` | Moves to a runtime with its own permissions | Found, not examined |
| OpenClaw | `/exec security=full ask=off` | Switches approvals off for the run that message starts | Recorded |
| OpenClaw | `/approve <id> allow-always` | Answers an approval and trusts that command from then on | Recorded |
| OpenClaw | `/elevated on`, `ask`, `full` | Lets a sandboxed agent run commands outside the sandbox | Accepted. The agent was not sandboxed, and the approval was still asked |
| OpenClaw | `/config set`, `/debug set` | Writes the gateway's configuration | Refused unless the sender is OpenClaw's owner and the command is switched on |
| OpenClaw | `/bash`, `!` | Runs a command on the host with no agent | Refused: off by default |
| OpenClaw | `/allowlist`, `/acp`, `/mcp`, `/plugins`, `/codex permissions` | Change who may talk to it, or add tools | Refused, or read in the documentation only |

Four things made the rule in D-032 too weak as it stood. The first three were decided on 2026-11-18.

1. **A command can sit anywhere in a message.** OpenClaw honoured `/exec security=full ask=off` at the start,
   in the middle and at the end of a message. Each time the command that followed ran with no approval, for
   an ordinary member of the group. A check of how a message starts misses two of the three. So each listed
   command says whether it counts anywhere in a message or only at the start, and the core checks accordingly
   ([D-047](../decisions.md#d-047-a-listed-command-says-where-in-a-message-it-counts)).
2. **One word does two jobs.** In Hermes `/approve` answers an approval, and also confirms `/new`. So a
   request to confirm a command is a `question` item, and a typed `/approve` is an approval answer only while
   an approval is waiting ([D-048](../decisions.md#d-048-a-request-to-confirm-a-command-is-a-question)). An
   answer to that question goes to Hermes by the confirmation's id, so it can answer nothing else, and only
   after Hermes's own check of who may answer
   ([D-146](../decisions.md#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id)).
3. **An approval can cover other people.** After one person answered "always", the same command from another
   person ran with no prompt, in both gateways. In Hermes the standing approval covers the reason a command
   was flagged, such as "delete in root path", and not the one command. A standing approval stays an
   ordinary answer that needs the right to approve, and the approval item says what it covers
   ([D-049](../decisions.md#d-049-a-standing-approval-is-an-ordinary-approval-answer)).
4. **Hermes can approve by itself.** In its default mode a second model judged a flagged command and approved
   it, and nobody was asked. That is the harness's own approval behaviour, which the owner sets on the harness
   ([D-021](../decisions.md#d-021-approval-behaviour-belongs-to-the-harness)).

Each gateway also has its own list of who may use such commands, separate from the owner's settings in the
core. The plugin sets them to admit whatever the core lets through
([D-084](../decisions.md#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through)).

## What is visible, and to whom

Answers and controls are group events. Every member's device receives them and keeps them. That is intended:
a group should see who approved a command.

It is a problem for one case. Codex can mark a question's answer as secret, such as a password. An answer sent
to a group is readable by every member, whatever the agent then does with it.

So a secret question can be answered from the chat only when the chat has two members, the agent and one
person ([D-041](../decisions.md#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat)). In any other
chat the question is shown, cannot be answered there, and says to answer it at the terminal. The agent's reply
says the question was answered and never repeats the answer. The answer does stay in that chat's history on
that person's devices.

## What older apps show

An app built on today's MDK shows a kind 1202 event as a generic agent row. So a press from an updated app
would appear there as a small row, such as "Allow once". An app that hides unknown kinds shows nothing.

## Options considered

| Option | Why not |
| --- | --- |
| Send answers and controls as chat messages with a tag | They would alert people, show as chat in every client, and could reach the model as a prompt by mistake |
| A reaction on the approval item as the answer | It cannot carry text or several choices, and any member's reaction looks the same as the owner's |
| A typed command for everything, such as `/approve` | It collides with the harness's own commands, and a typo approves nothing or the wrong thing |
| A shared list of models and modes | Each harness has its own, and they change with every release |
| A separate kind for answers and for controls | They share one shape: who it is for, what it acts on, an action and its details |
| The app decides whether an answer was accepted | Two people can answer at once. Only the agent knows which one the harness took |

## What it needs from the spec and from MDK

In the Marmot specification:

- kind 1202: its tags, the actions, and what a client that does not support it should do;
- the `session` item.

In MDK:

- build answers and controls, and expose that to the apps;
- give the apps each item's choices, and each session's settings, actions and commands;
- show a press as pending until the agent's reply arrives;
- keep kind 1202 out of the chat's rows, alerts and unread counts.

In the core:

- check who sent it against the owner's rules;
- pass it to the driver, and turn the driver's result into the edit or the notice.

## Not covered here

- **Choosing among past sessions.** "New session" is here. Listing and resuming older ones depends on how a
  chat maps to a session ([Q-07](../open-questions.md#q-07-how-does-a-chat-map-to-a-harness-session)).
- **Changing who may prompt and approve from the chat**
  ([Q-10](../open-questions.md#q-10-how-are-permissions-changed-from-the-chat)).

## What was decided

| Point | Decision |
| --- | --- |
| Answers and controls are one kind, 1202, naming the agent and the item they act on | [D-027](../decisions.md#d-027-answers-and-controls-are-one-kind-of-event-kind-1202) |
| The agent's reply settles each one. The rules for answers. Late and repeated events, with a limit of five minutes | [D-028](../decisions.md#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it) |
| Three shared controls, and everything else published by the driver | [D-029](../decisions.md#d-029-three-shared-controls-and-the-rest-comes-from-the-harness) |
| The list is carried by a `session` item, an eleventh type | [D-030](../decisions.md#d-030-a-session-is-an-item-the-eleventh-type) |
| A slash command is always a message for the harness | [D-031](../decisions.md#d-031-a-slash-command-is-a-message-for-the-harness) |
| Every action needs one of two rights | [D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights) |
| Controls need the right to prompt | [D-039](../decisions.md#d-039-controls-need-the-right-to-prompt) |
| A message sent during a turn goes straight to the harness | [D-040](../decisions.md#d-040-a-message-sent-during-a-turn-goes-straight-to-the-harness) |
| A secret answer is accepted only in a two-person chat | [D-041](../decisions.md#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat) |
| A listed command says where in a message it counts | [D-047](../decisions.md#d-047-a-listed-command-says-where-in-a-message-it-counts) |
| The core matches a typed command as any harness reads it, failing towards asking for the right to approve | [D-150](../decisions.md#d-150-the-core-matches-a-typed-command-as-the-harness-reads-it-failing-towards-asking-for-the-right-to-approve) |
| A request to confirm a command is a question | [D-048](../decisions.md#d-048-a-request-to-confirm-a-command-is-a-question) |
| Hermes's plugin answers a confirmation by its id, so the answer resolves it or nothing | [D-146](../decisions.md#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id) |
| A standing approval is an ordinary approval answer | [D-049](../decisions.md#d-049-a-standing-approval-is-an-ordinary-approval-answer) |
| A chat can move a coding harness to another folder, by someone who may approve, from its next session | [D-101](../decisions.md#d-101-a-chat-can-move-a-coding-harness-to-another-folder) |
| An answer or a control the agent cannot read is refused, as `not_supported` or `invalid`, after the right | [D-140](../decisions.md#d-140-an-answer-or-a-control-the-agent-cannot-read-is-refused) |
