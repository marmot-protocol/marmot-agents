# Current state

What is true of the system now, in one page, so that nobody has to read every decision to find out.
[`decisions.md`](decisions.md) stays the record: it says why, and when two disagree, it wins. Each line here
links to the decisions behind it; what a later decision replaced is left out. What is not decided is in
[`open-questions.md`](open-questions.md), and what is built next in [`plan.md`](plan.md).

Brought up to date on 2026-11-24, at `master` commit `062491e`, through [D-140](decisions.md#d-140-an-answer-or-a-control-the-agent-cannot-read-is-refused), and [D-147](decisions.md#d-147-a-request-whose-effect-a-connector-showed-before-its-connection-dropped-was-taken). A change that adds or replaces
a decision updates this page in the same change ([D-114](decisions.md#d-114-a-current-state-page-sits-beside-this-record-and-every-new-decision-updates-it)).

## What it is for

- Connectors that put AI agent harnesses into Marmot group chats, built here, not in MDK ([D-001](decisions.md#d-001-connectors-live-in-this-repository)).
- The main user controls their own agents from the phone, with as much control as at the terminal; sharing an
  agent with a group is first-class, and every member sees what it says and asks; for the pilot, its full activity
  only when the owner lists the group ([D-017](decisions.md#d-017-who-this-is-for), [D-011](decisions.md#d-011-full-control-from-the-phone), [D-009](decisions.md#d-009-groups-are-first-class-and-every-member-sees-agent-activity), [D-136](decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)).
- Claude Code, Codex, OpenCode, Pi, Goose, Cursor, Hermes and OpenClaw are all supported, and more will come;
  outside developers may write connectors later ([D-013](decisions.md#d-013-keep-every-current-connector-and-expect-more), [D-080](decisions.md#d-080-cursor-is-driven-through-acp), [D-014](decisions.md#d-014-outside-developers-can-write-connectors-later)).
- MIT licence, public repository ([D-019](decisions.md#d-019-mit-licence-public-repository)).

## The core and MDK

- One Rust program, `marmot-agent`, holds the Marmot side and the conversation handling, with MDK as a
  library, never its command-line tool ([D-002](decisions.md#d-002-one-rust-core-program-with-mdk-as-a-library), [D-050](decisions.md#d-050-the-core-program-is-called-marmot-agent)).
- Four layers: driver, translation, conversation handling, Marmot side. Nothing above a driver knows which
  harness is behind it ([D-004](decisions.md#d-004-four-layers-with-drivers-behind-one-trait)).
- One agent identity per core process, with one harness, one account home and one settings file. Each harness
  install is its own contact, and each chat has its own session ([D-072](decisions.md#d-072-one-agent-identity-per-core-process-until-the-swap), [D-035](decisions.md#d-035-each-harness-install-is-its-own-identity-with-its-own-sessions)).
- One settings file, read at start, sets the account home, relays, owner, rights, who may invite it, the
  harness, the agent's name, and which groups are sent its full activity ([D-136](decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)). There is no administration over the socket or from the chat ([D-073](decisions.md#d-073-standing-settings-live-in-a-configuration-file),
  [D-079](decisions.md#d-079-the-settings-name-the-agent)).
- The core reads each group from its own mark. A group it has never read starts after what arrived before the
  core did. A group MDK cannot read for a moment is read again, backing off to 10 s ([D-111](decisions.md#d-111-a-group-the-core-has-never-read-starts-after-what-arrived-before-the-core-did), [D-106](decisions.md#d-106-the-core-keeps-reading-again-a-group-mdk-cannot-read-for-a-moment)).
- Every send but a file's goes through MDK's token-based sends, so it can be tried again safely. A send that is dropped for
  good is stood in for once; four limits are accepted ([D-109](decisions.md#d-109-how-the-cores-sends-survive-a-crash-as-built), [D-097](decisions.md#d-097-what-stands-in-for-a-send-that-is-dropped), [D-096](decisions.md#d-096-four-limits-of-the-core-that-the-simulator-and-the-model-accept)).
- A restart closes the running turn and its open items as interrupted, and settles open requests as failed.
  Nothing is sent again, and a prompt is delivered at most once ([D-070](decisions.md#d-070-a-restart-closes-the-turn-as-interrupted), [D-037](decisions.md#d-037-a-prompt-is-delivered-at-most-once)).
  A prompt whose answer a dropped connection lost is told as perhaps not delivered ([D-129](decisions.md#d-129-a-prompt-whose-answer-a-dropped-connection-lost-is-told-as-perhaps-not-delivered)),
  unless an event the core read before the drop already showed its turn; a request such an event settled was
  taken ([D-147](decisions.md#d-147-a-request-whose-effect-a-connector-showed-before-its-connection-dropped-was-taken)).
  Messages a driver that ended was never given wait for the next driver ([D-139](decisions.md#d-139-prompts-a-driver-that-ended-was-never-given-wait-for-the-next-driver)).
- The core waits for every request it hands a driver. A harness that stops reading for a minute is stopped, and
  its requests are refused as failed; a connector on the socket, after 30 seconds ([D-066](decisions.md#d-066-the-core-finishes-every-request-it-starts), [D-098](decisions.md#d-098-a-harness-that-stops-reading-ends-its-connection), [D-138](decisions.md#d-138-a-connector-that-stops-reading-is-given-up-in-a-requests-time)).
- No word-by-word streaming, and the QUIC preview stack is removed ([D-012](decisions.md#d-012-word-by-word-streaming-is-not-required-now), [D-067](decisions.md#d-067-the-quic-preview-stack-is-removed)).
- Logs never hold message text, prompts, output, native messages, keys, public keys, event or group ids, relay
  addresses, paths, or a harness's session or turn ids ([D-071](decisions.md#d-071-what-may-be-logged)).

## What reaches the chat

- The agent's words are ordinary chat messages, kind 9, tagged with their turn. Remarks made during a turn are
  tagged `commentary` and do not alert ([D-022](decisions.md#d-022-the-agents-words-are-ordinary-chat-messages), [D-078](decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert)).
- Everything else is an agent item, kind 1201, one of eleven types, `session` among them. A later version is
  MDK's edit, kind 1009, carrying the whole item with a higher version ([D-023](decisions.md#d-023-one-kind-carries-every-agent-item), [D-030](decisions.md#d-030-a-session-is-an-item-the-eleventh-type), [D-025](decisions.md#d-025-an-agent-item-is-updated-with-an-edit), [D-112](decisions.md#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)).
- The tags are fixed, and the message that started a turn is `started_by` ([D-076](decisions.md#d-076-the-tags-of-kinds-1201-and-1202), [D-112](decisions.md#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)). The old shapes
  of 1201 and 1202 are not kept ([D-051](decisions.md#d-051-kinds-1201-and-1202-take-the-new-shape-and-the-old-one-is-not-kept)).
- The core's own notices carry `seq` from 1, and a `refused` notice about a message names it with `started_by`.
  An item begun under the temporary kind, or whose first event disappeared, goes on as a new kind 1201 event;
  one whose first event was deleted gets a `failed` notice. The agent's words settle at most what fits MDK's
  limit on a chat message's tags, and their turn settles the rest ([D-120](decisions.md#d-120-the-choices-made-in-moving-to-kinds-1201-and-1202-as-built)).
- Each item carries a type from the shared vocabulary, a plain rendering and the harness's native payload, and
  everything the agent does is passed through: reasoning, tools, results, plans ([D-007](decisions.md#d-007-one-shared-event-vocabulary), [D-059](decisions.md#d-059-eight-points-about-what-an-agent-event-carries), [D-008](decisions.md#d-008-pass-everything-through)).
- For the pilot, that full activity goes only to the owner's two-person chat with the agent, and to the groups
  the owner lists as `full_activity` in the settings, or every chat with `"all"`. Every other chat is sent the
  agent's words, its turns, its approvals and questions, a subagent's too, its errors, its session and a
  harness's notice that settles a request; not its reasoning, tools, plans, tasks, other notices or a subagent's
  words, and no native payload. The owner's chat is one whose members MDK says are exactly the agent and the
  owner, read before each send. The core applies it, and settles what an item it holds back settles by the
  item's turn ([D-136](decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)).
- Three exceptions to the native payload: a secret question no longer waiting carries none, for the pilot a chat
  without the full activity is sent none ([D-136](decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)), and the core leaves
  it out of an item over 64 KB, then cuts the output. A driver never cuts anything ([D-056](decisions.md#d-056-a-secret-question-carries-a-native-message-only-while-it-is-waiting), [D-026](decisions.md#d-026-an-agent-item-is-at-most-64-kb),
  [D-057](decisions.md#d-057-the-core-applies-the-64-kb-limit)).
- Alerts: the final answer, a waiting approval or question, and a failed turn. Nothing else ([D-024](decisions.md#d-024-who-an-agent-event-alerts),
  [D-078](decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert)).

## Answers, controls and rights

- An answer or a control is kind 1202. It is a request, settled by exactly one event from the agent: the edit
  that shows its effect, or a `refused` notice with a reason ([D-027](decisions.md#d-027-answers-and-controls-are-one-kind-of-event-kind-1202), [D-028](decisions.md#d-028-an-answer-or-a-control-is-a-request-and-the-agents-reply-settles-it), [D-077](decisions.md#d-077-a-refused-notice-says-why-something-was-not-acted-on)).
- One the core cannot read is refused too: `not_supported` for an action it does not know, `invalid` otherwise,
  and `not_allowed` for someone with neither right ([D-140](decisions.md#d-140-an-answer-or-a-control-the-agent-cannot-read-is-refused)).
- A control whose effect never shows is settled by the core after two minutes, as `no_effect` ([D-087](decisions.md#d-087-the-core-settles-a-request-whose-effect-never-shows)).
- Three shared controls, `interrupt`, `new_session` and `set`. Settings, actions and slash commands come from the
  harness, in the session item, filled in by the connector where the harness says nothing ([D-029](decisions.md#d-029-three-shared-controls-and-the-rest-comes-from-the-harness), [D-044](decisions.md#d-044-a-connector-supplies-what-its-harness-does-not-say),
  [D-045](decisions.md#d-045-a-session-items-commands-keep-their-names-and-lose-descriptions-first), [D-046](decisions.md#d-046-a-setting-change-is-an-edit-and-a-notice-only-when-nobody-asked-for-it)).
- A message starting with a slash goes to the harness as typed; the core has no typed commands ([D-031](decisions.md#d-031-a-slash-command-is-a-message-for-the-harness)).
- Two rights, prompt and approve, both the owner's alone by default, set in the settings file. Answering an
  approval and changing a setting marked as needing it (the permission mode, a chat's folder) need the right to
  approve; prompting, answering a question and every other control need the right to prompt ([D-010](decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve),
  [D-032](decisions.md#d-032-every-action-needs-one-of-two-rights), [D-039](decisions.md#d-039-controls-need-the-right-to-prompt), [D-043](decisions.md#d-043-a-setting-says-which-right-it-needs)).
- Each connector lists the typed commands that need the right to approve, and where in a message each counts. A
  request to confirm a command is a question; a standing approval is an ordinary answer ([D-047](decisions.md#d-047-a-listed-command-says-where-in-a-message-it-counts), [D-048](decisions.md#d-048-a-request-to-confirm-a-command-is-a-question),
  [D-049](decisions.md#d-049-a-standing-approval-is-an-ordinary-approval-answer)).
- Approval behaviour is the harness's own; the core adds no modes and answers nothing itself ([D-021](decisions.md#d-021-approval-behaviour-belongs-to-the-harness)).
- In a group the agent answers when mentioned or replied to; in a two-person chat, always ([D-075](decisions.md#d-075-in-a-group-the-agent-answers-when-mentioned-or-replied-to-in-a-direct-chat-always)).
- A message sent during a turn goes straight to the harness, which folds it in, restarts or queues it
  ([D-040](decisions.md#d-040-a-message-sent-during-a-turn-goes-straight-to-the-harness)).
- A secret answer is accepted only in a two-person chat. The membership check is not bound to the MLS epoch the
  answer is sent under, an accepted risk ([D-041](decisions.md#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat), [D-137](decisions.md#d-137-a-secret-answers-membership-check-is-not-bound-to-the-epoch-it-is-sent-under)).
- An agent may act only in its own chat, and may not change a group or the account; the chat section that
  would carry this is not built ([D-042](decisions.md#d-042-an-agent-may-act-in-its-own-chat-and-may-not-change-a-group-or-the-account)).

## Drivers

- Each driver speaks its harness's official interface where one exists ([D-006](decisions.md#d-006-prefer-official-interfaces-and-libraries)), implements one trait, and
  states seven capability flags ([D-058](decisions.md#d-058-the-driver-trait-and-the-capability-flags)).
- Codex: native, in Rust, on `codex app-server`, with types of its own. One translator per thread; a working
  folder and a narrow sandbox by default; sessions kept so they resume ([D-055](decisions.md#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own), [D-060](decisions.md#d-060-a-codex-translator-follows-one-thread-and-the-transport-routes), [D-061](decisions.md#d-061-a-codex-session-starts-in-a-working-folder-with-a-safe-sandbox-unless-configured), [D-062](decisions.md#d-062-what-codex-reads-for-a-chat-message),
  [D-063](decisions.md#d-063-codex-keeps-a-session-so-it-can-be-resumed), [D-064](decisions.md#d-064-the-codex-driver-declines-an-mcp-servers-form-until-forms-are-designed)).
- Codex's subagents are on, and shown flat in the chat that started them: a `subagent` tool item, a `subagent`
  task for each of its turns, its words naming that tool item as `parent` in the event the driver gives the
  core; the chat message the core sends for them carries no such link yet. Stop stops the agent and every
  subagent under its turn, settled once when the last has ended, or as failed if one Codex refused to stop runs
  on; a Stop naming no item, after the turn, stops what still works beside it; a new session stops the old one's
  subagents ([D-117](decisions.md#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them)).
  One a Stop waits for then is followed until Codex says whether it stopped, which settles the Stop
  ([D-132](decisions.md#d-132-a-stop-waiting-for-a-subagent-is-settled-by-codex-after-a-new-session)).
- The Codex driver refuses, with Codex's own error codes, every request of Codex's that no chat takes: one it
  cannot show, one for a thread a chat left, and one for a thread never seen once no session is being picked and
  no subagent started ([D-131](decisions.md#d-131-the-codex-driver-refuses-a-request-of-codexs-that-no-chat-takes)).
- Pi: native, in Rust, on `pi --mode rpc`, with types of its own, running the person's own `pi` by default
  ([D-086](decisions.md#d-086-the-pi-driver-reads-pis-rpc-messages-with-types-of-its-own), [D-102](decisions.md#d-102-the-installers-connect-to-the-harness-the-person-already-has), [D-092](decisions.md#d-092-the-choices-made-while-building-m3-are-accepted)).
- OpenCode, Goose and Cursor: the ACP driver in the core ([D-005](decisions.md#d-005-which-driver-each-harness-gets), [D-080](decisions.md#d-080-cursor-is-driven-through-acp), [D-092](decisions.md#d-092-the-choices-made-while-building-m3-are-accepted)).
- Claude Code: a TypeScript program on the Agent SDK, over the socket, loading none of the owner's Claude Code
  settings files ([D-005](decisions.md#d-005-which-driver-each-harness-gets), [D-082](decisions.md#d-082-the-claude-code-driver-loads-no-claude-code-settings-files)).
- A prompt names a session only to resume one ([D-065](decisions.md#d-065-a-prompt-names-a-session-only-to-resume-one)).

## Subagents

Decided, and not built: the core, the drivers, MDK and the apps each have their part to do.

- Where a harness tells subagents apart, each subagent is a thread in the agent's own group. Its row is its
  `task` item; its items carry NIP-22's tags, uppercase for the top-level subagent's `task` item and lowercase
  for the immediate parent's; its words are kind 1111 comments. An edit repeats the root tags
  ([D-133](decisions.md#d-133-an-edit-of-an-item-in-a-subagents-thread-repeats-the-threads-root-tags), [D-121](decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them), [D-122](decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar)).
- The agent's comments in its own threads do not alert. The app shows a running bar of open `task` items, lifts
  waiting approvals into it, and folds finished threads ([D-122](decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar)).
- Stop in the main chat stops every subagent the agent started, even after its turn; Stop in a thread stops that
  subagent and those it started. A comment in a thread is a prompt only where the driver allows it and the
  writer may prompt, and never goes to the parent ([D-123](decisions.md#d-123-stop-and-writing-in-a-subagents-thread)).
- A driver closes open `task` items when it can no longer follow them; the app keeps counting a quiet row and
  says when it was last heard from. A harness that does not tell subagents apart keeps them as `tool` items. An
  app shows a held edit of an item sent before the member joined as the item ([D-124](decisions.md#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners)).
- The Codex transport gives each subagent's thread a translator in its parent's chat, and interrupts subagents
  itself ([D-125](decisions.md#d-125-the-codex-transport-follows-subagents-threads)). It is built, and shows them flat until threads are ([D-117](decisions.md#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them)).
- No separate MLS group for subagents ([D-126](decisions.md#d-126-no-mls-group-for-subagents)).

## Channel plugins

- Hermes and OpenClaw are channel plugins: each runs inside its gateway as one of its channels, over the socket,
  with a narrower contract than a driver; the gateway owns sessions, media, its own allow-lists and how much tool
  activity it sends ([D-113](decisions.md#d-113-hermes-and-openclaw-are-channel-plugins-with-a-narrower-contract-than-a-driver)).
  Python for Hermes, TypeScript for OpenClaw
  ([D-005](decisions.md#d-005-which-driver-each-harness-gets), [D-003](decisions.md#d-003-the-boundary-to-other-languages-is-a-local-socket)).
- The contract in `design/drivers.md` holds as written: sessions are always the gateway's own; no `folder` in the
  hello and no `send_file` tool from the core; everything the gateway hands its channel is passed through, at
  the gateway's own verbosity, so Hermes shows no `tool` items; controls go only as the gateway's typed
  commands; the gateway's own check is opened so the core's counts; an internal the plugin relies on may be
  watched by the end-to-end scenario ([D-118](decisions.md#d-118-the-channel-plugin-contract-is-accepted-as-written)). The socket's chat section, for the gateways' message tools,
  is not designed, so OpenClaw's message tool is offered no actions
  ([Q-32](open-questions.md#q-32-what-does-the-sockets-chat-section-carry)).
- The core decides who may prompt and approve; the gateway allows what the core lets through ([D-084](decisions.md#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through)).
- What they send and how they behave was accepted from M3; OpenClaw's `/model status` may show the masked key
  ([D-092](decisions.md#d-092-the-choices-made-while-building-m3-are-accepted), [D-088](decisions.md#d-088-openclaws-model-status-may-show-the-masked-model-key)).

## The socket

- A Unix socket, `marmot.agent-control.v3`, JSON-RPC 2.0, one message per line, at most 4 MB, with no v2
  compatibility ([D-003](decisions.md#d-003-the-boundary-to-other-languages-is-a-local-socket), [D-020](decisions.md#d-020-the-socket-speaks-agent-control-v3-with-a-clean-cutover), [D-033](decisions.md#d-033-the-sockets-framing-is-json-rpc-20-one-message-per-line)).
- The messages as built are accepted: hello with token and flags, prompt, answer, control, and events
  ([D-081](decisions.md#d-081-the-socket-messages-as-built-are-accepted)).
- A connector speaks only in agent events. The core does the shared work, gives it ids of its own, and gives
  each connector its own token ([D-034](decisions.md#d-034-the-core-does-the-shared-work-and-a-connector-speaks-only-in-agent-events), [D-036](decisions.md#d-036-the-ids-a-connector-sees-are-the-cores-own), [D-038](decisions.md#d-038-each-connector-has-its-own-token)).
- A JSON Schema and examples made from the core's own types (`fixtures/socket`) check that the three connectors
  here send and read only what the core does; the core itself stays lenient, and OpenClaw no longer sends the
  fields the core passes over ([D-115](decisions.md#d-115-the-sockets-schema-is-made-from-the-cores-own-types-and-checks-this-repositorys-connectors)).

## Files

- An agent's file goes to the chat as an encrypted attachment, tagged with its turn, only from folders the
  settings allow; every harness that can send one does ([D-090](decisions.md#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder), [D-107](decisions.md#d-107-every-harness-that-lets-an-agent-send-a-file-gets-file-sending-before-the-swap)).
- Every harness the core gives a tool gets `send_file(path)`; Hermes and OpenClaw turn their own media into the
  same request; ACP agents get it from a loopback MCP server, on by default ([D-108](decisions.md#d-108-how-an-agents-file-reaches-the-core-as-built), [D-110](decisions.md#d-110-an-acp-agent-gets-the-file-tool-from-a-loopback-mcp-server-on-by-default)).
- Files inside the agent's own home are refused unless the owner allows them; a bind mount is an accepted
  limit ([D-099](decisions.md#d-099-files-from-the-agents-own-home-are-refused-by-default-and-the-owner-may-allow-them), [D-103](decisions.md#d-103-a-bind-mount-into-the-agents-home-is-an-accepted-limit-of-d-099)).
- The core passes no files to a harness yet ([D-083](decisions.md#d-083-the-core-passes-no-files-to-a-harness-yet)).

## Folders

- A chat of a coding harness can move to another folder, set by someone who may approve. It starts a new
  session there. Hermes and OpenClaw keep their own workspace ([D-101](decisions.md#d-101-a-chat-can-move-a-coding-harness-to-another-folder)).
- The owner's `folders` setting lists where chats may work: the install folder by default, `~` for the home's
  folders, `/` for anywhere. Never the agent's own home ([D-104](decisions.md#d-104-how-a-chats-folder-works-as-built)).
- A folder move whose session shows after the two-minute settle still moves the chat ([D-128](decisions.md#d-128-a-folder-move-whose-session-shows-late-still-moves-the-chat)).

## Installers and the move

- One install script per harness, on Linux and macOS, as a service of the person's account. Release binaries
  are built by CI and are not signed yet ([D-091](decisions.md#d-091-one-install-script-per-harness-and-release-binaries-from-ci)).
- The installers never install a harness: they use the one the person has, and add our plugin to their Hermes
  or OpenClaw ([D-102](decisions.md#d-102-the-installers-connect-to-the-harness-the-person-already-has)).
- Defaults: where things go, service names, White Noise's relays, and no default model endpoint ([D-100](decisions.md#d-100-the-installers-defaults)).
- In a gateway, the installers change only the plugin's platform or channel, Hermes's `MARMOT_ALLOW_ALL_USERS`,
  and, if it is off, OpenClaw's approval forwarding for Marmot chats; they print the restart ([D-105](decisions.md#d-105-which-gateway-settings-the-installers-change),
  [D-084](decisions.md#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through)).
- An install moves over with `marmot-agent import`, which uses the old account home in place, backs it up
  first, and prints the paths needed to undo it ([D-089](decisions.md#d-089-an-install-moves-over-by-importing-its-account-home-in-place), [D-095](decisions.md#d-095-the-import-prints-the-paths-it-needs-to-undo-a-move)).
- Where the import cannot tell exactly which sender policy the old connector applied to the account, such as
  which of OpenClaw's named accounts ran on the home, told only from settings written out literally, or a
  systemd environment file line read otherwise by version, only the owner may prompt, and it names each old
  setting that may have applied on the home, and its value, to check by hand ([D-135](decisions.md#d-135-the-import-gives-owner-only-rights-where-it-cannot-tell-the-old-sender-policy-and-names-what-to-check)).
- A gateway's plugin change is put back if the installer fails or is stopped, and a re-run after a kill always
  finishes or puts it back; a few named moments after a kill leave neither plugin working until then ([D-116](decisions.md#d-116-a-gateways-plugin-swap-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)).
- An upgrade puts the new core and support files in place together, once both are prepared, and puts back
  the ones it found if it fails or is stopped, with Pi's and Goose's image and our plugin in a gateway; a
  re-run after a kill finishes it or puts it back. One install runs in a home at a time ([D-134](decisions.md#d-134-an-upgrade-of-the-core-and-its-support-files-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)).
- A gateway install moves over with its installer's `--replace-plugin`. The OpenClaw installer finishes a
  half-done move by editing OpenClaw's settings file itself, after a copy; the gateway's conversation starts
  fresh in each chat. `--replace-plugin` lists the gateway's work folder in `files_from` beside the old
  connector's folders, in the line the import wrote ([D-119](decisions.md#d-119-how-a-gateway-install-moves-over-its-work-folder-a-half-done-openclaw-move-a-fresh-conversation)).

## Verification and how the work runs

- Each driver is tested by replaying its recordings, against a stand-in, and by one end-to-end scenario with
  the real harness ([D-074](decisions.md#d-074-every-driver-is-tested-by-replay-and-by-one-end-to-end-scenario)).
- Beyond that: a conformance suite, a deterministic simulator, TLA+ models, proofs, property tests, fuzzing,
  mutation testing and privacy checks ([D-093](decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top)).
- Recordings are of scripted scenarios only, scrubbed; Cursor's keep only its built-in commands ([D-085](decisions.md#d-085-cursor-recordings-keep-only-cursors-built-in-commands)).
- CI on a pull request stays under 15 minutes; long checks run nightly ([D-094](decisions.md#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly)).
- Real Claude Code and Codex run their scenarios in CI on the model double, with no model key in CI at all;
  cassettes are recorded through numes by hand, a cassette older than 14 days is a warning on a pull request and
  fails the nightly run, and the gateways stay scripted ([D-127](decisions.md#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key)).
- CI runs in three tiers: a pull request (aimed at under 10 minutes; the TLA+ models only when the models or the
  code they follow changed), the merge queue and each push to `master` (everything, the simulator at 200 runs and the
  Docker move-over tests), and a nightly run (new simulator seeds, larger TLA+ bounds, coverage, a 64th of the
  core's mutants, the cassettes), whose failed jobs open issues. One job, `CI passed`, is the only required
  check, and fails unless every job its tier expects ran and passed ([D-130](decisions.md#d-130-ci-runs-in-three-tiers-with-one-required-check)).
- The Rust workspace builds with MDK's compiler and forbids unsafe code ([D-053](decisions.md#d-053-the-rust-workspace-builds-with-mdks-compiler-and-forbids-unsafe-code)).
- Work follows `plan.md`; the builder decides what is internal and merges once CI passes and review left no
  blocker. Every commit is signed ([D-068](decisions.md#d-068-work-follows-the-plan-and-the-builder-decides-what-is-internal), [D-054](decisions.md#d-054-changes-arrive-as-small-signed-pull-requests-that-ci-checks)).
- Decisions and design live in this repository; work items are issues. Documents serve people and agents
  ([D-016](decisions.md#d-016-decisions-and-design-live-in-this-repository-work-items-live-in-issues), [D-018](decisions.md#d-018-documents-serve-both-people-and-agents)).
