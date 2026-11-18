# The event model

Status: draft 0. The principles are decided
([D-007](../decisions.md#d-007-one-shared-event-vocabulary),
[D-008](../decisions.md#d-008-pass-everything-through)). Everything else on this page is a proposal: the item
types, the fields and the rules. It is no longer a guess, though. A prototype applies it to 64 recorded sessions
with Codex, Claude Code and Hermes, and every native message either becomes an agent event or is deliberately
not sent.
See [Checked against real sessions](#checked-against-real-sessions). The open points are
[here](../open-questions.md#the-event-model).

This is the most important design in the project. Every driver produces these events and every client draws
them.

## The idea

A harness emits its own events in its own format. A driver turns each one into a **Marmot agent event** with
three parts.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/event-dark.svg">
  <img alt="A native harness event becomes one Marmot agent event with three parts: an item type from the shared vocabulary, a plain rendering any client can show, and the untouched native payload." src="../images/event-light.svg">
</picture>

| Part | What it is | Who uses it |
| --- | --- | --- |
| Item type | One of a small shared list | Clients, to choose how to draw it |
| Plain rendering | A title and some text | Any client, including one that does not know the item type |
| Native payload | The harness's own event, unchanged | Clients that want to show more for a harness they know |

Marmot chat already works this way for ordinary messages: people send unsigned Nostr-shaped events of kinds that
clients know how to render. Agent events extend that set.

## The eleven item types

| Type | What it shows |
| --- | --- |
| `turn` | A turn started or ended, who or what started it, how it ended, and what it used |
| `message` | Text the agent says to the people in the chat |
| `reasoning` | The agent's thinking, when the harness provides it |
| `tool` | A tool the agent used. Its `kind` says which sort: `command`, `edit`, `read`, `search`, `fetch`, `mcp`, `subagent` or `other` |
| `approval` | The agent wants permission for one action, with the choices offered |
| `question` | The agent asks the person something |
| `plan` | The agent's plan or to-do list |
| `task` | Work that runs beside a turn or outlives it: a subagent, a background job |
| `notice` | Compaction, the agent waiting, a retry, a warning, a model change, the output of a command |
| `error` | Something failed outside any one item |
| `session` | A session with the harness: what it is, and the settings, actions and slash commands it offers |

`session` is the newest, added on 2026-11-18 ([D-030](../decisions.md#d-030-a-session-is-an-item-the-eleventh-type)). The
recordings were checked against the other ten, and the prototype does not produce it yet.

A command, a file edit and a web search are all the `tool` type, told apart by `kind`. One harness reports them
as separate things and the other as one generic tool call; a single type with a kind fits both.

## An agent event

| Field | Meaning |
| --- | --- |
| `harness` | Which harness produced it |
| `session` | The harness's session |
| `turn` | The turn it belongs to |
| `item` | The item's id. It stays the same for the life of the item |
| `seq` | Order within the session |
| `type` | One of the item types |
| `state` | `running`, `waiting`, `done`, `failed`, `declined` or `cancelled` |
| `title` | A few words: "Running a command", "Allow this command?" |
| `text` | The plain rendering, readable on its own |
| `body` | Fields that depend on the item type |
| `native` | The harness's own message: its kind and its payload |

**An item changes by being sent again.** A tool call is sent when it starts, with state `running`, and again when
it finishes, with state `done` and its output. The second event has the same `item` and replaces the first. A
client always draws the latest one.

**`waiting` means a person is needed.** An approval or a question is `waiting` until someone answers.

## One turn, as events

This is a real recording, [`codex/approval_allowed`](../../fixtures/recordings/codex/approval_allowed.jsonl),
translated by the prototype. Sixty-one native messages become eight events. Ids, native payloads, repeated
fields and the shell wrapper around the command are left out.

| # | Type | State | Title | Text |
| --- | --- | --- | --- | --- |
| 1 | `turn` | `running` | Working | |
| 2 | `message` | `done` | Message | I'll run `python3 greet.py` and report its output. |
| 3 | `tool` | `running` | Running a command | `python3 greet.py` |
| 4 | `approval` | `waiting` | Allow this command? | `python3 greet.py` |
| 5 | `approval` | `done` | Allow this command? | Answered: Allow once |
| 6 | `tool` | `done` | Ran a command | `python3 greet.py` |
| 7 | `message` | `done` | Message | It printed: Hello, world |
| 8 | `turn` | `done` | Finished | |

Events 4 and 5 are the same item, and so are 3 and 6. A chat therefore shows six things for this turn, two of
which changed while the person watched.

To see any recording this way:

```sh
python3 tools/prototype/translate.py fixtures/recordings/claude/question.jsonl --brief
```

## What each type carries

Only the fields in `body` differ.

| Type | Body |
| --- | --- |
| `turn` | `started_by`: `user`, `agent` or `control`. On ending: `duration_ms`, `usage`. Whether there is a fourth source, something on the owner's machine outside the harness, is [Q-25](../open-questions.md#q-25-can-something-outside-the-chat-and-the-harness-start-a-turn) |
| `message` | `phase`: `commentary` or `final`, when the harness says. `parent`, for a subagent's message |
| `reasoning` | Nothing beyond the text |
| `tool` | `kind`, `name`. For a command: `command`, `cwd`, then `exit_code`, `duration_ms`, `output`. For an edit: `changes`, each with a path and a diff. For a search: `query`. Otherwise `input` and `output` |
| `approval` | `subject`: the item awaiting permission. `choices`: each with an `id`, a `label` and an `effect` of `allow` or `deny`. Once answered: `answer` |
| `question` | `answer_via`: `request` or `message`. `questions`: each with an `id`, the `text`, `options`, whether `multiple` selections are allowed, whether free text is allowed, and whether the answer is `secret`. Once answered: `answer` |
| `plan` | `text`, or `entries` each with a text and a state |
| `task` | `kind`, `description`, `subject`, and `usage` once known |
| `notice` | `kind`: `compaction`, `waiting`, `retry`, `warning`, `model_changed`, `command_output` |
| `error` | What was being attempted, and the harness's error code |
| `session` | `settings`: each with an `id`, a `label`, its `value` and the `options` it accepts. `actions`: each with an `id`, a `label` and a `description`. `commands`: each with a `name`, a `description` and a `hint` |

## Answers and controls

People act on the agent in two ways.

**Answers** respond to something the agent asked: an `approval` or a `question`. An answer names the item it
answers and the choice or choices made, by id.

**Controls** are things a person does unprompted: interrupt, start a new session, change a setting such as the
model.

How they are carried, what the rules are and who may send which is decided:
[`answers-and-controls.md`](answers-and-controls.md). The `session` item tells an app what a harness lets it
set and do.

## What a driver has to do

The recordings turned several vague intentions into rules. A driver that skips one of these produces events that
look right and are wrong.

1. **Give every choice its own id, and keep the harness's value.** A Codex decision can be an object. The id on
   the wire is never the native value.
2. **Offer the choices the harness accepts, not only the ones it lists.** Codex accepts "decline" without
   listing it, and lists nothing at all for a file change.
3. **Close open items when a turn ends, and close the turn when the harness stops.** An interrupted Codex
   command never reports that it stopped, and a harness that is killed mid-turn sends nothing at all.
4. **Recognise questions and plans that arrive as tool calls, or twice.** Claude Code asks questions and proposes
   plans through tools. Hermes sends a question in a structured form and again as text. Each becomes one
   `question` or `plan`, not two things.
5. **Decide the kind of a tool call.** Claude Code gives only a tool name.
6. **Say what started a turn.** A turn can follow a person's message, the agent's own background work, or a
   control such as compaction.
7. **Read failure from the right place.** Claude Code reports a failed turn with a subtype of `success` and an
   error flag.
8. **Ask for reasoning.** Claude Code sends empty thinking blocks unless summarised thinking is switched on.
   Codex sent none in any recording.
9. **Never show a secret answer to the group.** A Codex question can be marked secret.
10. **State what is never sent.** About half of all native traffic is partial text, and a quarter more is
    bookkeeping. Each driver lists the native messages it drops.

## Checked against real sessions

The prototype is [`tools/prototype/translate.py`](../../tools/prototype/translate.py). It is a design tool, not
driver code.

| | |
| --- | --- |
| Recordings | 64: 24 Codex, 26 Claude Code, 14 Hermes |
| Native messages | 4,775 |
| Agent events produced | 519 |
| Native messages with no rule | 0 |
| Items left open at the end of a recording | 0 |

The events by type: 170 `turn`, 124 `tool`, 97 `message`, 64 `notice`, 44 `approval`, 8 `question`, 8 `task`,
2 `plan`, 1 `error`, 1 `reasoning`.

**A gateway fits, with less to say.** Hermes was added after the two coding harnesses, as a check that the ten
types are not shaped around them. Its 2,618 native calls became 120 events with no new type needed. But it uses
only five of the ten: `turn`, `message`, `notice`, `approval` and `question`. It produces no `tool` items at all,
because Hermes gives a platform a formatted line of text about a tool, not the tool call. Those lines can only
become notices, and 51 of its 120 events are notices. See
[`../reference/hermes-messages.md`](../reference/hermes-messages.md).

**What changed since the first proposal**

| Finding | Change |
| --- | --- |
| Commands, edits and searches are separate items in one harness and one generic tool call in the other | One `tool` type with a `kind` |
| Background tasks are common in real use and had no item | The `task` type, which also covers subagents |
| Codex offers an object as a decision, and accepts decisions it did not list | Driver-assigned choice ids, and rule 2 above |
| A Codex question can be a field on an ordinary message | `answer_via` on a question |
| A question can hold several questions, allow several selections, or be secret | `questions` is a list; `multiple` and `secret` on each |
| Compaction is a turn of its own in Codex; background work starts a turn in Claude Code | `started_by` on a turn |
| Steering stays in the turn for Codex and ends it for Claude Code | "Steer" is a request the core makes; the driver reports what happened to the turn |

Detail and evidence for each: [`../reference/harness-messages.md`](../reference/harness-messages.md).

**What is still unchecked**

- A to-do list that changes during a turn. Neither harness produced one, so `plan` with `entries` is untested.
- Reasoning, beyond a single recording.
- Failures other than a bad model: a rate limit, an expired sign-in, a crash.
- Volume. In one developer's real use a turn averaged about 12 tool calls, 7 reasoning blocks and 3.5 pieces of
  text. With each tool call sent twice that is roughly 35 events per turn
  ([Q-12](../open-questions.md#q-12-how-many-events-is-too-many)).
- The other five harnesses.

## What is deliberately not in the model

- **The meaning of a choice.** "Allow once" and "Allow for this session" are the harness's concepts. A choice
  carries a label and whether it allows or denies; nothing more.
- **Guarantees about what the harness did.** An answer was delivered to the harness. What happened next arrives
  as the harness's own later events.
- **Layout.** There are no cards, columns or styling instructions. Clients decide how each item looks.

## Not designed yet

- **A file the agent sends to the chat.** There is no item for it. This is a known gap
  ([Q-23](../open-questions.md#q-23-how-does-an-agent-send-a-file-to-the-chat)).
- **How an event is carried, and size limits.** Decided; see [`event-carrier.md`](event-carrier.md). The
  exact tags are not fixed.
- **The exact tags of an answer and of each control.** Their shape is decided; see
  [`answers-and-controls.md`](answers-and-controls.md).
