# The socket protocol

Status: draft. Decided: there is a local socket
([D-003](../decisions.md#d-003-the-boundary-to-other-languages-is-a-local-socket)), and it speaks
`marmot.agent-control.v3` with a hello, a driver section and a chat section, no live preview stream, and no
support for v2 ([D-020](../decisions.md#d-020-the-socket-speaks-agent-control-v3-with-a-clean-cutover)).
Also decided, on 2026-11-18: the framing, what the core does for every connector, one connection per agent,
the ids a connector sees, how often a prompt is delivered, and the credential
([D-033](../decisions.md#d-033-the-sockets-framing-is-json-rpc-20-one-message-per-line) to
[D-038](../decisions.md#d-038-each-connector-has-its-own-token)). They are listed [at the end](#what-was-decided).

Still a draft: the exact fields of each message, and the list of capabilities. Not designed: the individual
requests of the chat section, and administration
([Q-05](../open-questions.md#q-05-how-is-the-agent-administered)).

## What it is for

Some connectors cannot be compiled into the core, because the harness's interface exists only in another
language. They run as their own programs and talk to the core over a socket on the same machine.

| Program | Language | Why it is separate |
| --- | --- | --- |
| Claude Code driver | TypeScript | Built on the official Claude Agent SDK |
| Cursor driver | TypeScript | Built on the official Cursor SDK |
| Hermes plugin | Python | Must run inside Hermes |
| OpenClaw plugin | TypeScript | Must run inside OpenClaw |
| Third-party connectors, later | Any | [D-014](../decisions.md#d-014-outside-developers-can-write-connectors-later) |

## The principle

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/socket-dark.svg">
  <img alt="A connector and the core exchange messages over a local socket. The core sends prompts, answers and controls. The connector sends agent events and, for agents that act in the chat, chat requests. Keys, relays and group state stay in the core." src="../images/socket-light.svg">
</picture>

A program on the socket is a driver. It does over the socket exactly what a built-in driver does through the
Rust trait: it sends agent events up and receives prompts, answers and controls down. The core treats both the
same.

A program on the socket never sees keys, ciphertext, relays or group state. It also never sees an event kind
or a tag. It speaks in the terms of [the event model](event-model.md), and the core turns that into Marmot
events.

## What the core does, so that no connector has to

Decided as [D-034](../decisions.md#d-034-the-core-does-the-shared-work-and-a-connector-speaks-only-in-agent-events).

| Job | Why it belongs in the core |
| --- | --- |
| Check who may prompt and who may approve | One place for the owner's rules ([D-010](../decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)). Everything a connector receives is already allowed |
| Tell a person who was refused | Hermes drops a disallowed sender silently, and D-010 wants them told |
| Decide whether a group message is for this agent | A connector receives only messages that are |
| Turn an agent event into a chat message, an agent item or an edit | [D-022](../decisions.md#d-022-the-agents-words-are-ordinary-chat-messages), [D-023](../decisions.md#d-023-one-kind-carries-every-agent-item), [D-025](../decisions.md#d-025-an-agent-item-is-updated-with-an-edit) |
| Number the items in a turn and the versions of an item | |
| Apply the size limit | [D-026](../decisions.md#d-026-an-agent-item-is-at-most-64-kb), [D-057](../decisions.md#d-057-the-core-applies-the-64-kb-limit) |
| Decide who is alerted | [D-024](../decisions.md#d-024-who-an-agent-event-alerts) |
| Report the result of an answer or a control to the chat | [Answers and controls](answers-and-controls.md#3-the-agents-reply-is-what-counts) |
| Close open items when a connector goes away | A harness that is killed sends nothing |

## Framing

[JSON-RPC 2.0](https://www.jsonrpc.org/specification), one message per line, over a Unix socket
([D-033](../decisions.md#d-033-the-sockets-framing-is-json-rpc-20-one-message-per-line)).

v2 already puts one JSON message per line on a Unix socket, which is easy to write from any language and easy
to record for tests. v3 keeps that and adds JSON-RPC's three message shapes: a request with an id, a response
that carries the same id, and a notification that expects no response. Either side can send a request. Codex's
app server and ACP use the same framing, so libraries exist in every language we need.

A message is at most 4 MB. A connector cuts anything larger before sending it.

Windows has not been looked at.

## The hello

A connection opens with one request from the connector.

| Field | Meaning |
| --- | --- |
| `protocol` | `marmot.agent-control.v3` |
| `connector` | Its name and release version |
| `harness` | The harness's name and version |
| `token` | The credential. See [who may connect](#who-may-connect) |
| `capabilities` | What the harness can do. See [capabilities](#capabilities) |

The core replies with its own version and the agent's display name. It refuses a protocol version it does not
support, or a connector too old for it, with an error that names the version it needs. That part comes from
[issue 11](https://github.com/marmot-protocol/marmot-agents/issues/11): drivers that run as separate programs
can fall behind the core, and this makes that easy to see.

**One connection is one agent.** The token decides which. Each harness install is its own agent
([D-035](../decisions.md#d-035-each-harness-install-is-its-own-identity-with-its-own-sessions)).

## The driver section

### From the core to the connector

Three requests. Each gets a response as soon as the connector has taken it or refused it. What the harness then
does arrives as agent events.

| Request | Carries |
| --- | --- |
| `prompt` | The message's id and text. The chat: an id, whether it is a group, its name. The sender: an id and a name. For a reply: the id of the message replied to, its text and its author. Attachments, each as a path to a file the core wrote, with a name and a type. The session, when the core chooses it |
| `answer` | The item being answered, who answered, and the choices: ids the item offered, and text where the question allows it. One answer can cover several questions |
| `control` | The action, what it acts on, who sent it, and the action's details. See [answers and controls](answers-and-controls.md#5-controls-a-few-shared-ones-and-whatever-the-harness-offers) |

A connector may refuse with one of these reasons: `not_supported`, `busy`, `unknown_item`, `not_waiting`,
`already_answered`, `nothing_to_do`, `invalid`, `failed`. The core turns a refusal into the notice the chat
sees.

Why each part of a prompt is there:

- **The sender and the chat, on every prompt.** Hermes and OpenClaw route a prompt inside their own runtime.
  Hermes keeps one session per sender, even in a group. OpenClaw keeps one per group.
- **The quoted text and its author, for a reply.** Hermes cannot look a message up. OpenClaw could answer from
  the id alone, and takes the quote when it is given.
- **The message's id.** A gateway's own message tools act on it, for example to react to it.

**Ids are the core's own** ([D-036](../decisions.md#d-036-the-ids-a-connector-sees-are-the-cores-own)). A chat's id and a
sender's id are stable values the core makes up for this install. They are not the group's id or the person's public key. A gateway keeps its own logs and transcripts,
and nothing in them should identify a group or a person to anyone else. Names are passed as they are, because
the model needs them.

### From the connector to the core

One notification, `event`, with no response. It is an agent event as [the event model](event-model.md) defines
it.

| Field | Meaning |
| --- | --- |
| `item` | The connector's own id for the item. The same id again means the item changed |
| `type`, `state` | One of the item types, and its state |
| `title`, `text` | The plain rendering |
| `body` | The type's own fields |
| `native` | The harness's own message |
| `session`, `turn` | The session and the turn it belongs to |
| `prompts` | On a turn: the ids of the messages it responds to |

Rules:

- **Whole items only.** An event repeats the entire item. There is no "text so far"
  ([D-012](../decisions.md#d-012-word-by-word-streaming-is-not-required-now)).
- **Order is the order sent.** The core numbers items as they arrive.
- **The first event of an item creates it. A later one edits it.** The connector does not need to know which.
- **Send everything, at full size.** The core applies the size limit.
- **Typing signals and bookkeeping are never sent.** They are a third to four fifths of what a gateway emits.

## The chat section

Hermes and OpenClaw give their agents message tools, so an agent can act in the chat on its own initiative. The
chat section is requests from the connector to the core: send, edit and delete a message, add and remove a
reaction, send a file, and read history.

What an agent may do is decided
([D-042](../decisions.md#d-042-an-agent-may-act-in-its-own-chat-and-may-not-change-a-group-or-the-account)):

- It may send a message, edit or delete a message of its own, add and remove a reaction, and read history.
- It may do so only in the chat its session belongs to, with the ids the core gave it.
- It may not change a group or the account: no renaming, creating groups, inviting or removing people. That
  waits for [administration](#administration).

These are ordinary Marmot actions. A delete is an ordinary deletion of a chat message, and the tool call that
caused it still arrives as a `tool` item.

How an agent sends a file is not decided
([Q-23](../open-questions.md#q-23-how-does-an-agent-send-a-file-to-the-chat)). The individual requests are not
designed yet.

## Capabilities

The connector states in the hello what its harness can do. The core and the apps read these and never check a
harness's name. This list is a draft, drawn from the recordings. It will change when the first drivers are
built. A proposal that turns it into types is under
[Q-16](../open-questions.md#q-16-how-does-a-driver-say-what-it-supports). It keeps seven of these flags and
leaves out `tool_items`, `questions` and `reasoning`.

| Capability | Values | Codex | Claude Code | Hermes | OpenClaw |
| --- | --- | --- | --- | --- | --- |
| `interrupt` | yes or no | Yes | Yes | Yes | Yes |
| `mid_turn`: what happens to a prompt during a turn | `fold`, `restart`, `queue` | `fold` | `restart` | `fold` | `queue` |
| `sessions`: who picks the session | `core` or `own` | `core` | `core` | `own` | `own` |
| `resume`: a session survives a restart | yes or no | Yes | Yes | Not recorded | Not recorded |
| `tool_items`: tools reported as items | yes or no | Yes | Yes | No | Yes |
| `approvals` | yes or no | Yes | Yes | Yes | Yes, if the owner switched forwarding on |
| `questions`: how a question is answered | `request`, `message` or both | Both | `request` | `message` | `request` |
| `reasoning` | yes or no | No | Yes | No | Yes, when set to stream |
| `files_in`: attachments on a prompt | yes or no | Images | Images | Not recorded | Not recorded |
| `approval_commands`: typed commands that need the right to approve | a list; each entry says whether it counts anywhere in a message or only at the start | None | None | `/approve`, `/deny`, `/yolo`, `/approvals` and more | `/approve`, `/exec` anywhere in a message, `/elevated` and more |

`approval_commands` exists because Hermes and OpenClaw accept an approval typed as a message. A typed message
is a prompt, and needs only the right to prompt. So the core checks a message that starts with one of these
commands against the right to approve ([D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights)). A command
that changes the harness's permission mode belongs on the list too. Both gateways have several, and OpenClaw
honours one of them anywhere in a message, so each entry says where it counts
([D-047](../decisions.md#d-047-a-listed-command-says-where-in-a-message-it-counts)). The full lists are in
[answers and controls](answers-and-controls.md#typed-commands-that-widen-what-an-agent-may-do).

Codex is a built-in driver and does not use the socket. It is in the table because the same capabilities apply
to the Rust trait.

## When things go wrong

| What happens | What the core does |
| --- | --- |
| The connection drops during a turn | Closes the turn and every open item, with the reason. The chat shows the turn as interrupted |
| The connector comes back | Takes a new hello. Nothing is replayed |
| A prompt was sent and no response came before the drop | Reports it as not delivered. It is never sent again |
| An event names an unknown turn or session | Accepts it. A harness can produce work outside a turn |
| A message is not valid | Answers with an error and keeps the connection |

A prompt is delivered at most once ([D-037](../decisions.md#d-037-a-prompt-is-delivered-at-most-once)). Running a prompt
twice is worse than losing one, because the person can see a prompt that was lost and send it again. The rest
of what happens on a restart is open
([Q-27](../open-questions.md#q-27-what-happens-to-a-turn-when-the-core-or-a-driver-restarts)).

## Who may connect

Decided as [D-038](../decisions.md#d-038-each-connector-has-its-own-token), as enough for now:

- The socket is in a folder that only the owner's user account can open.
- Each connector has its own token, in a file only that user can read. The core creates it when the owner adds
  the connector. The token says which agent the connector drives, and that it is a connector and not an
  administrator.
- Administration needs a different token.

What this gives and does not give: it stops other users of the machine, it tells connectors apart, and it keeps
a connector from using administration by accident. It does not stop a program running as the same user, which
can read the token files and the core's own data. A harness with a shell runs as that user. Real separation
needs the core to run as a different user, which is possible later and not planned now.

## An example

Alice asks for a command to be run, and allows it. `→` is from the core, `←` from the connector. Fields are
shortened.

```text
← {"id":1,"method":"hello","params":{"protocol":"marmot.agent-control.v3","connector":{"name":"claude","version":"0.1.0"},"harness":{"name":"claude-code","version":"2.1"},"token":"…","capabilities":{…}}}
→ {"id":1,"result":{"core":{"version":"0.1.0"},"agent":{"name":"Claude on the laptop"}}}

→ {"id":7,"method":"prompt","params":{"message":"m-31","text":"run the tests","chat":{"id":"c-2","group":true,"name":"Family"},"sender":{"id":"u-1","name":"Alice"}}}
← {"id":7,"result":{}}
← {"method":"event","params":{"item":"turn-4","type":"turn","state":"running","title":"Working","prompts":["m-31"]}}
← {"method":"event","params":{"item":"cmd-9","type":"tool","state":"running","title":"Running a command","text":"cargo test","turn":"turn-4"}}
← {"method":"event","params":{"item":"ap-2","type":"approval","state":"waiting","title":"Allow this command?","body":{"subject":"cmd-9","choices":[{"id":"once","label":"Allow once","effect":"allow"},{"id":"deny","label":"Deny","effect":"deny"}]},"turn":"turn-4"}}

→ {"id":8,"method":"answer","params":{"item":"ap-2","by":{"id":"u-1","name":"Alice"},"choices":["once"]}}
← {"id":8,"result":{}}
← {"method":"event","params":{"item":"ap-2","type":"approval","state":"done","text":"Answered: Allow once","turn":"turn-4"}}
← {"method":"event","params":{"item":"cmd-9","type":"tool","state":"done","title":"Ran a command","body":{"exit_code":0},"turn":"turn-4"}}
← {"method":"event","params":{"item":"msg-5","type":"message","state":"done","text":"All 42 tests pass.","turn":"turn-4"}}
← {"method":"event","params":{"item":"turn-4","type":"turn","state":"done","title":"Finished"}}
```

In the chat this is one message from the agent, and three rows that each updated in place.

## What it replaces

MDK's `wn-agent` daemon speaks `marmot.agent-control.v2`: one JSON message per line over a Unix socket. It has
54 request types, most of them about chat messages, account management and maintenance. Agent activity is a
small corner of it.

v3 turns that around. Agent events and controls are the centre.

There is no bridge between the two. Every client of v2 is ours, so the connectors move to v3 in one cutover.

| In v2 | In v3 |
| --- | --- |
| Chat requests: send, edit, delete, react, media, history | The chat section |
| Agent activity and operation events | The driver section, in full |
| Live preview stream, seven requests | Left out until streaming is decided |
| Account, profile, relay lists, allow-list, invite policy, maintenance, diagnostics | Not available to connectors; see below |
| Debug requests | Not carried over |
| An authentication token in every message | Authentication once, in the hello |

## Administration

Open: [Q-05](../open-questions.md#q-05-how-is-the-agent-administered).

In v2 any connected client can administer the agent: change who may invite it, publish its profile, edit its
relay lists. In v3 a connector cannot. The harness is the thing being controlled, and it should not be able to
change the rules about who controls it.

The proposal is that the owner administers the agent through a configuration file and the core's own
command line, and later from the chat. The command line still has to reach the running core, so v3 gets an
administration section that only the owner's tools may use. Its messages are not designed here.

## What was decided

| Point | Decision |
| --- | --- |
| The framing is JSON-RPC 2.0, one message per line | [D-033](../decisions.md#d-033-the-sockets-framing-is-json-rpc-20-one-message-per-line) |
| The core does the jobs listed above, and a connector speaks only in agent events | [D-034](../decisions.md#d-034-the-core-does-the-shared-work-and-a-connector-speaks-only-in-agent-events) |
| One connection is one agent, chosen by the token | [D-035](../decisions.md#d-035-each-harness-install-is-its-own-identity-with-its-own-sessions) |
| Chat and sender ids are the core's own | [D-036](../decisions.md#d-036-the-ids-a-connector-sees-are-the-cores-own) |
| A prompt is delivered at most once | [D-037](../decisions.md#d-037-a-prompt-is-delivered-at-most-once) |
| A typed command that approves is checked against the right to approve | [D-032](../decisions.md#d-032-every-action-needs-one-of-two-rights) |
| A token per connector is enough for now, with its limit stated | [D-038](../decisions.md#d-038-each-connector-has-its-own-token) |
| A message sent during a turn goes straight to the harness, which is what `mid_turn` reports | [D-040](../decisions.md#d-040-a-message-sent-during-a-turn-goes-straight-to-the-harness) |
| What an agent may do through the chat section | [D-042](../decisions.md#d-042-an-agent-may-act-in-its-own-chat-and-may-not-change-a-group-or-the-account) |
