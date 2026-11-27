# Current state

What is true of the system now, in one page, so that nobody has to read every decision to find out.
[`decisions.md`](decisions.md) stays the record: it says why, and when two disagree, it wins. Each line here
links to the decisions behind it; what a later decision replaced is left out. What is not decided is in
[`open-questions.md`](open-questions.md), and what is built next in [`plan.md`](plan.md).

Brought up to date on 2026-11-27, at `master` commit `83ff68e`, through [D-140](decisions.md#d-140-an-answer-or-a-control-the-agent-cannot-read-is-refused), [D-142](decisions.md#d-142-how-subagent-threads-are-built), [D-143](decisions.md#d-143-one-issue-tracks-the-ci-jobs-that-flake), [D-144](decisions.md#d-144-property-tests-run-on-every-pull-request-kani-in-the-full-gate-and-fuzzing-nightly-on-a-pinned-nightly-compiler), [D-145](decisions.md#d-145-the-core-bounds-what-it-holds-for-a-connector-and-the-files-it-sends-at-once), [D-146](decisions.md#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id), [D-147](decisions.md#d-147-a-request-whose-effect-a-connector-showed-before-its-connection-dropped-was-taken), [D-148](decisions.md#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit), [D-149](decisions.md#d-149-stop-in-a-subagents-thread-stops-that-subagent-alone), [D-150](decisions.md#d-150-the-core-matches-a-typed-command-as-the-harness-reads-it-failing-towards-asking-for-the-right-to-approve), [D-151](decisions.md#d-151-the-hermes-plugin-asks-hermess-own-check-of-who-may-answer-an-approval-card-or-a-question), [D-152](decisions.md#d-152-a-send-mdk-was-handed-in-a-form-its-chat-is-no-longer-sent-is-asked-after-never-handed-again), [D-154](decisions.md#d-154-an-install-takes-over-a-killed-upgrade-only-for-the-same-agent-and-the-same-gateway), [D-155](decisions.md#d-155-the-hermes-plugin-keeps-each-request-to-confirm-a-command-as-hermes-keeps-it), [D-156](decisions.md#d-156-the-no_effect-net-counts-when-output-reached-the-core-not-when-the-core-read-it), [D-157](decisions.md#d-157-the-core-goes-on-with-the-event-mdk-names-for-a-send-it-holds-and-never-hands-such-a-send-to-mdk-again), [D-158](decisions.md#d-158-the-core-does-not-limit-a-send-to-the-chats-members-a-send-in-flight-when-the-owner-adds-someone-may-reach-them), [D-159](decisions.md#d-159-an-upgrade-keeps-a-copy-of-the-agents-account-store-and-puts-it-back-with-the-old-core), [D-160](decisions.md#d-160-the-coding-installers-take-an-install-moved-over-and-run-it-as-a-service), [D-161](decisions.md#d-161-ci-no-longer-runs-on-pushes-to-master), [D-162](decisions.md#d-162-pull-requests-record-verification-review-and-release-impact-trailers-on-the-tip-commit), [D-163](decisions.md#d-163-hourly-canary-runs-the-full-gate-on-master-and-refreshes-build-caches), [D-164](decisions.md#d-164-nightly-verification-runs-against-the-verified-canary-branch), [D-165](decisions.md#d-165-four-independent-release-tracks-decoupled-by-the-socket-protocol-version), [D-166](decisions.md#d-166-per-track-releases-compatibility-manifests-and-protected-release-tags), [D-167](decisions.md#d-167-socket-and-plugin-installers-fetch-compatible-standalone-connector-releases-atomically), [D-168](decisions.md#d-168-always-on-revert-only-bisect-agent-watches-hourly-canary-and-activates-meltdown-mode), [D-169](decisions.md#d-169-team-pull-requests-use-a-fast-static-gate-backed-by-hourly-canary-and-the-bisect-agent), [D-170](decisions.md#d-170-google-antigravity-cli-is-driven-through-acp), [D-171](decisions.md#d-171-the-flaky-tracker-names-individual-tests-from-machine-readable-junit-results), [D-172](decisions.md#d-172-an-uninstall-does-not-put-back-the-plugin-a---replace-plugin-move-replaced), [D-173](decisions.md#d-173-marmot-agent-init-refuses-a-home-with-an-identity-before-it-opens-its-store-and-no-core-starts-on-a-home-being-put-back), [D-174](decisions.md#d-174-the-pi-driver-remembers-across-a-restart-which-sessions-held-a-message), [D-175](decisions.md#d-175-an-agent-sends-its-session-item-when-it-joins-a-chat-and-an-agents-messages-are-not-prompts), [D-176](decisions.md#d-176-the-session-item-says-who-in-the-chat-may-prompt-and-who-may-approve), [D-177](decisions.md#d-177-the-session-item-carries-the-sessions-title-its-project-comes-later-to-full-activity-chats-only), [D-178](decisions.md#d-178-an-install-counts-a-start-as-done-once-the-core-and-claude-codes-connector-say-they-are-ready), [D-179](decisions.md#d-179-the-owner-shown-in-a-session-item-is-a-field-of-the-item-not-a-p-tag), [D-180](decisions.md#d-180-a-prompt-a-driver-refuses-as-having-ended-waits-for-the-next-driver-whenever-the-core-handed-it-on), [D-181](decisions.md#d-181-an-app-should-not-offer-to-answer-a-secret-question-in-a-chat-of-more-than-two-members), [D-182](decisions.md#d-182-nothing-in-a-chat-proves-that-a-member-is-an-agent-or-whose-it-is), [D-183](decisions.md#d-183-the-agents-words-carry-settles), [D-184](decisions.md#d-184-the-specification-of-agents-in-chats-is-proposed-to-the-marmot-repository), [D-185](decisions.md#d-185-a-replys-quote-is-what-the-chat-shows-now-and-a-message-no-longer-shown-is-quoted-without-its-text), and [D-186](decisions.md#d-186-the-acp-driver-sends-a-message-during-a-turn-with-the-agents-steering-extension-when-it-announces-one). A change that adds or replaces
a decision updates this page in the same change ([D-114](decisions.md#d-114-a-current-state-page-sits-beside-this-record-and-every-new-decision-updates-it)).

## What it is for

- Connectors that put AI agent harnesses into Marmot group chats, built here, not in MDK ([D-001](decisions.md#d-001-connectors-live-in-this-repository)).
- The main user controls their own agents from the phone, with as much control as at the terminal; sharing an
  agent with a group is first-class, and every member sees what it says and asks; for the pilot, its full activity
  only when the owner lists the group ([D-017](decisions.md#d-017-who-this-is-for), [D-011](decisions.md#d-011-full-control-from-the-phone), [D-009](decisions.md#d-009-groups-are-first-class-and-every-member-sees-agent-activity), [D-136](decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)).
- Claude Code, Codex, OpenCode, Pi, Goose, Cursor, Antigravity, Hermes and OpenClaw are all supported, and more will come;
  outside developers may write connectors later ([D-013](decisions.md#d-013-keep-every-current-connector-and-expect-more), [D-080](decisions.md#d-080-cursor-is-driven-through-acp), [D-170](decisions.md#d-170-google-antigravity-cli-is-driven-through-acp), [D-014](decisions.md#d-014-outside-developers-can-write-connectors-later)).
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
- Every send but a file's, and but a later version of a subagent's words in its thread, goes through MDK's
  token-based sends, so it can be tried again safely. That later version is MDK's plain edit of the thread message,
  which keeps no token: tried again after an answer that was lost, it shows the same words again ([D-142](decisions.md#d-142-how-subagent-threads-are-built), point 4). A send that is dropped for
  good is stood in for once; four limits are accepted ([D-109](decisions.md#d-109-how-the-cores-sends-survive-a-crash-as-built), [D-097](decisions.md#d-097-what-stands-in-for-a-send-that-is-dropped), [D-096](decisions.md#d-096-four-limits-of-the-core-that-the-simulator-and-the-model-accept)).
- A restart closes the running turn and its open items as interrupted, and settles open requests as failed.
  Nothing is sent again, and a prompt is delivered at most once ([D-070](decisions.md#d-070-a-restart-closes-the-turn-as-interrupted), [D-037](decisions.md#d-037-a-prompt-is-delivered-at-most-once)).
  A prompt whose answer a dropped connection lost is told as perhaps not delivered ([D-129](decisions.md#d-129-a-prompt-whose-answer-a-dropped-connection-lost-is-told-as-perhaps-not-delivered)),
  unless an event the core read before the drop already showed its turn; a request such an event settled was
  taken ([D-147](decisions.md#d-147-a-request-whose-effect-a-connector-showed-before-its-connection-dropped-was-taken)).
  Messages a driver that ended was never given wait for the next driver ([D-139](decisions.md#d-139-prompts-a-driver-that-ended-was-never-given-wait-for-the-next-driver)),
  and so does a prompt the core handed on that the driver refused as having ended, none of it reaching it, and
  nobody is told; an answer or a control refused so is settled with that refusal ([D-180](decisions.md#d-180-a-prompt-a-driver-refuses-as-having-ended-waits-for-the-next-driver-whenever-the-core-handed-it-on)).
- The core waits for every request it hands a driver. A harness that stops reading for a minute is stopped, and
  its requests are refused as failed; a connector on the socket, after 30 seconds ([D-066](decisions.md#d-066-the-core-finishes-every-request-it-starts), [D-098](decisions.md#d-098-a-harness-that-stops-reading-ends-its-connection), [D-138](decisions.md#d-138-a-connector-that-stops-reading-is-given-up-in-a-requests-time)).
  A connector is given up at once when what the core holds for it while a request waits, the lines it sent
  meanwhile and the refusals it owes for repeated `send_file` ids together, would pass 32 MB ([D-145](decisions.md#d-145-the-core-bounds-what-it-holds-for-a-connector-and-the-files-it-sends-at-once)).
- No word-by-word streaming, and the QUIC preview stack is removed ([D-012](decisions.md#d-012-word-by-word-streaming-is-not-required-now), [D-067](decisions.md#d-067-the-quic-preview-stack-is-removed)).
- Logs never hold message text, prompts, output, native messages, keys, public keys, event or group ids, relay
  addresses, paths, or a harness's session or turn ids ([D-071](decisions.md#d-071-what-may-be-logged)).

## What reaches the chat

- The agent's words are ordinary chat messages, kind 9, tagged with their turn. Remarks made during a turn are
  tagged `commentary` and do not alert ([D-022](decisions.md#d-022-the-agents-words-are-ordinary-chat-messages), [D-078](decisions.md#d-078-the-agents-remarks-during-a-turn-do-not-alert)).
  Words that show an answer's or a control's effect carry a `settles` tag for it, as an item does ([D-183](decisions.md#d-183-the-agents-words-carry-settles)).
- Everything else is an agent item, kind 1201, one of eleven types, `session` among them. A later version is
  MDK's edit, kind 1009, carrying the whole item with a higher version ([D-023](decisions.md#d-023-one-kind-carries-every-agent-item), [D-030](decisions.md#d-030-a-session-is-an-item-the-eleventh-type), [D-025](decisions.md#d-025-an-agent-item-is-updated-with-an-edit), [D-112](decisions.md#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)).
- The tags are fixed, and the message that started a turn is `started_by` ([D-076](decisions.md#d-076-the-tags-of-kinds-1201-and-1202), [D-112](decisions.md#d-112-the-core-sends-kinds-1201-1202-and-edits-and-a-turns-message-is-started_by)). The old shapes
  of 1201 and 1202 are not kept ([D-051](decisions.md#d-051-kinds-1201-and-1202-take-the-new-shape-and-the-old-one-is-not-kept)).
- The kinds, their edits and the tags on the agent's words are proposed to the Marmot specification as one
  feature document, `features/agents-in-chats.md`, with no MIP number; the draft's open choices are taken as it
  recommends ([D-184](decisions.md#d-184-the-specification-of-agents-in-chats-is-proposed-to-the-marmot-repository)).
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
- A send of the full activity the core handed to MDK while the chat was the owner's, and that MDK keeps and posts
  later, can reach someone who joined in between: only the owner can add someone there, so that is accepted for
  good, and the core does not limit sends to the chat's members ([D-148](decisions.md#d-148-for-the-pilot-a-send-of-the-full-activity-that-mdk-keeps-and-posts-after-someone-joins-is-an-accepted-limit),
  [D-158](decisions.md#d-158-the-core-does-not-limit-a-send-to-the-chats-members-a-send-in-flight-when-the-owner-adds-someone-may-reach-them)).
- Such a send, handed in a form the chat is no longer sent, is never handed to MDK again: the core asks MDK what
  became of it and takes the event MDK names, which MDK names while it holds the send too. One MDK no longer
  knows is told to the chat as perhaps not delivered ([D-152](decisions.md#d-152-a-send-mdk-was-handed-in-a-form-its-chat-is-no-longer-sent-is-asked-after-never-handed-again), [D-157](decisions.md#d-157-the-core-goes-on-with-the-event-mdk-names-for-a-send-it-holds-and-never-hands-such-a-send-to-mdk-again)).
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
- A control whose effect never shows is settled by the core after two minutes, as `no_effect` ([D-087](decisions.md#d-087-the-core-settles-a-request-whose-effect-never-shows)), once the core has read everything that reached it by then ([D-156](decisions.md#d-156-the-no_effect-net-counts-when-output-reached-the-core-not-when-the-core-read-it)).
- Three shared controls, `interrupt`, `new_session` and `set`. Settings, actions and slash commands come from the
  harness, in the session item, filled in by the connector where the harness says nothing ([D-029](decisions.md#d-029-three-shared-controls-and-the-rest-comes-from-the-harness), [D-044](decisions.md#d-044-a-connector-supplies-what-its-harness-does-not-say),
  [D-045](decisions.md#d-045-a-session-items-commands-keep-their-names-and-lose-descriptions-first), [D-046](decisions.md#d-046-a-setting-change-is-an-edit-and-a-notice-only-when-nobody-asked-for-it)).
- A message starting with a slash goes to the harness as typed; the core has no typed commands ([D-031](decisions.md#d-031-a-slash-command-is-a-message-for-the-harness)).
- Two rights, prompt and approve, both the owner's alone by default, set in the settings file. Answering an
  approval and changing a setting marked as needing it (the permission mode, a chat's folder) need the right to
  approve; prompting, answering a question and every other control need the right to prompt ([D-010](decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve),
  [D-032](decisions.md#d-032-every-action-needs-one-of-two-rights), [D-039](decisions.md#d-039-controls-need-the-right-to-prompt), [D-043](decisions.md#d-043-a-setting-says-which-right-it-needs)).
- The session item says, for its chat, who may prompt and who may approve: `everyone`, or the keys of the
  chat's members who hold the right, the owner among them, edited when the members or the settings change
  ([D-176](decisions.md#d-176-the-session-item-says-who-in-the-chat-may-prompt-and-who-may-approve)). They are the
  agent's statement, for display: an app must not grant or deny anything on them, and only the agent enforces them
  ([D-182](decisions.md#d-182-nothing-in-a-chat-proves-that-a-member-is-an-agent-or-whose-it-is)).
- A member that has sent a session item in a chat is an agent; its messages are not prompts, and get no
  notice, and its answers and controls are refused, unless the owner lists it by key for the right they need.
  The agent sends its own first session item when it joins a chat, and once to each chat it was already in,
  naming no session, with the owner's optional `label` and, when `show_owner` is set, the owner's key; nothing
  goes in its public profile
  ([D-175](decisions.md#d-175-an-agent-sends-its-session-item-when-it-joins-a-chat-and-an-agents-messages-are-not-prompts)).
  A session item proves nothing, since anyone can send one: nothing in a chat proves that a member is an agent, or
  whose, a known limit ([D-182](decisions.md#d-182-nothing-in-a-chat-proves-that-a-member-is-an-agent-or-whose-it-is)).
  The owner's key travels as an `owner` field of the session's own fields, not as a `p` tag
  ([D-179](decisions.md#d-179-the-owner-shown-in-a-session-item-is-a-field-of-the-item-not-a-p-tag)).
- The session item carries the harness's title for the session when it gives one, to every chat; a project
  (label, branch, pull request) comes later, to full-activity chats only
  ([D-177](decisions.md#d-177-the-session-item-carries-the-sessions-title-its-project-comes-later-to-full-activity-chats-only)).
  The title is built, and edited as it changes, with no notice: from Claude Code's `session_title_changed`,
  ACP's `session_info_update` (Goose, Cursor, Antigravity), Codex's thread name and Pi's session name, the last
  two only when someone names the session. OpenCode, Hermes and OpenClaw give a plugin or client none
  ([a session's title](reference/harness-messages.md#a-sessions-title)). The project is not built yet.
- Each connector lists the typed commands that need the right to approve, by every name its harness runs them
  under, and where in a message each counts; the core reads a message as any harness may, and asks for that
  right whenever one could read a listed command ([D-150](decisions.md#d-150-the-core-matches-a-typed-command-as-the-harness-reads-it-failing-towards-asking-for-the-right-to-approve)). A
  request to confirm a command is a question; a standing approval is an ordinary answer ([D-047](decisions.md#d-047-a-listed-command-says-where-in-a-message-it-counts), [D-048](decisions.md#d-048-a-request-to-confirm-a-command-is-a-question),
  [D-049](decisions.md#d-049-a-standing-approval-is-an-ordinary-approval-answer)). Hermes's plugin answers a
  confirmation by its id, through Hermes's own call and after Hermes's own check of who may answer, so the
  answer resolves that confirmation or nothing; the command it confirms runs on after the core is answered.
  An approval card and a question are answered only for someone Hermes's own check lets too
  ([D-151](decisions.md#d-151-the-hermes-plugin-asks-hermess-own-check-of-who-may-answer-an-approval-card-or-a-question)).
  The plugin keeps each confirmation by Hermes's session and id, and closes it only on evidence: Hermes's reply
  to an answer, a newer one in its session, or Hermes's own 300-second limit passing (both shown cancelled),
  never because Hermes no longer holds it, and never with a turn; while a typed answer waits for Hermes's
  reply, only that reply or the limit closes it; so one Hermes dropped without a word shows
  waiting until that limit. The command it confirms settles by its effect, or by the answer or close that says
  it will not run. Every Hermes reply goes to the turn of the message it replies to, or else to that sender's
  session outside any turn
  ([D-155](decisions.md#d-155-the-hermes-plugin-keeps-each-request-to-confirm-a-command-as-hermes-keeps-it)). A
  command Hermes drops after a stall of over five seconds shows only that Hermes gave no reply
  ([D-146](decisions.md#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id)).
- Approval behaviour is the harness's own; the core adds no modes and answers nothing itself ([D-021](decisions.md#d-021-approval-behaviour-belongs-to-the-harness)).
- In a group the agent answers when mentioned or replied to; in a two-person chat, always ([D-075](decisions.md#d-075-in-a-group-the-agent-answers-when-mentioned-or-replied-to-in-a-direct-chat-always)).
- A reply's quote is what the chat shows now: an edited message as edited, and a deleted one by its author
  alone, without its text ([D-185](decisions.md#d-185-a-replys-quote-is-what-the-chat-shows-now-and-a-message-no-longer-shown-is-quoted-without-its-text)).
- A message sent during a turn goes straight to the harness, which folds it in, restarts or queues it
  ([D-040](decisions.md#d-040-a-message-sent-during-a-turn-goes-straight-to-the-harness)). OpenCode folds it in,
  through its steering extension to ACP; the other ACP agents restart (provisional,
  [D-186](decisions.md#d-186-the-acp-driver-sends-a-message-during-a-turn-with-the-agents-steering-extension-when-it-announces-one)).
- A secret answer is accepted only in a two-person chat. The membership check is not bound to the MLS epoch the
  answer is sent under, an accepted risk ([D-041](decisions.md#d-041-a-secret-answer-is-accepted-only-in-a-two-person-chat), [D-137](decisions.md#d-137-a-secret-answers-membership-check-is-not-bound-to-the-epoch-it-is-sent-under)).
  An app should not offer to answer a secret question in a chat of more than two members ([D-181](decisions.md#d-181-an-app-should-not-offer-to-answer-a-secret-question-in-a-chat-of-more-than-two-members)).
- An agent may act only in its own chat, and may not change a group or the account; the chat section that
  would carry this is not built ([D-042](decisions.md#d-042-an-agent-may-act-in-its-own-chat-and-may-not-change-a-group-or-the-account)).

## Drivers

- Each driver speaks its harness's official interface where one exists ([D-006](decisions.md#d-006-prefer-official-interfaces-and-libraries)), implements one trait, and
  states seven capability flags ([D-058](decisions.md#d-058-the-driver-trait-and-the-capability-flags)), an eighth, whether its harness tells
  its subagents apart ([D-142](decisions.md#d-142-how-subagent-threads-are-built)), and a ninth, `stop_subagent`, whether a Stop can stop one
  subagent while the turn goes on ([D-149](decisions.md#d-149-stop-in-a-subagents-thread-stops-that-subagent-alone)).
- Codex: native, in Rust, on `codex app-server`, with types of its own. One translator per thread; a working
  folder and a narrow sandbox by default; sessions kept so they resume ([D-055](decisions.md#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own), [D-060](decisions.md#d-060-a-codex-translator-follows-one-thread-and-the-transport-routes), [D-061](decisions.md#d-061-a-codex-session-starts-in-a-working-folder-with-a-safe-sandbox-unless-configured), [D-062](decisions.md#d-062-what-codex-reads-for-a-chat-message),
  [D-063](decisions.md#d-063-codex-keeps-a-session-so-it-can-be-resumed), [D-064](decisions.md#d-064-the-codex-driver-declines-an-mcp-servers-form-until-forms-are-designed)).
- Codex's subagents are on, and shown in threads: a `subagent` tool item in the main chat, one `subagent` task
  for each subagent, its row, which each of its turns opens again, and everything else it does in its thread,
  its words naming that tool item as `parent` ([D-117](decisions.md#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them), [D-142](decisions.md#d-142-how-subagent-threads-are-built)). Stop stops the agent and every
  subagent under its turn, settled once when the last has ended, or as failed if one Codex refused to stop runs
  on; a Stop in a subagent's thread stops that subagent and those it started alone
  ([D-149](decisions.md#d-149-stop-in-a-subagents-thread-stops-that-subagent-alone)); a Stop naming no item, after the turn, stops what still works beside it; a new session stops the old one's
  subagents ([D-117](decisions.md#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them)).
  One a Stop waits for then is followed until Codex says whether it stopped, which settles the Stop
  ([D-132](decisions.md#d-132-a-stop-waiting-for-a-subagent-is-settled-by-codex-after-a-new-session)).
- The Codex driver refuses, with Codex's own error codes, every request of Codex's that no chat takes: one it
  cannot show, one for a thread a chat left, and one for a thread never seen once no session is being picked and
  no subagent started ([D-131](decisions.md#d-131-the-codex-driver-refuses-a-request-of-codexs-that-no-chat-takes)).
- Pi: native, in Rust, on `pi --mode rpc`, with types of its own, running the person's own `pi` by default
  ([D-086](decisions.md#d-086-the-pi-driver-reads-pis-rpc-messages-with-types-of-its-own), [D-102](decisions.md#d-102-the-installers-connect-to-the-harness-the-person-already-has), [D-092](decisions.md#d-092-the-choices-made-while-building-m3-are-accepted)).
  A session that held a message and whose file has gone is not made again empty, after a restart too: the prompt
  is refused and the chat told ([D-174](decisions.md#d-174-the-pi-driver-remembers-across-a-restart-which-sessions-held-a-message)).
- OpenCode, Goose, Cursor and Antigravity: the ACP driver in the core ([D-005](decisions.md#d-005-which-driver-each-harness-gets), [D-080](decisions.md#d-080-cursor-is-driven-through-acp), [D-092](decisions.md#d-092-the-choices-made-while-building-m3-are-accepted), [D-170](decisions.md#d-170-google-antigravity-cli-is-driven-through-acp)).
- Claude Code: a TypeScript program on the Agent SDK, over the socket, loading none of the owner's Claude Code
  settings files ([D-005](decisions.md#d-005-which-driver-each-harness-gets), [D-082](decisions.md#d-082-the-claude-code-driver-loads-no-claude-code-settings-files)).
- A prompt names a session only to resume one ([D-065](decisions.md#d-065-a-prompt-names-a-session-only-to-resume-one)).

## Subagents

Built in the core, the Codex driver and MDK ([D-141](decisions.md#d-141-subagent-threads-are-built-before-the-swap), [D-142](decisions.md#d-142-how-subagent-threads-are-built)); the apps have their part still to do.

- Where a harness tells subagents apart, each subagent is a thread in the agent's own group. Its row is its
  `task` item; its items carry NIP-22's tags, uppercase for the top-level subagent's `task` item and lowercase
  for the immediate parent's; its words are kind 1111 comments. An edit repeats the root tags
  ([D-133](decisions.md#d-133-an-edit-of-an-item-in-a-subagents-thread-repeats-the-threads-root-tags), [D-121](decisions.md#d-121-a-subagent-is-a-thread-in-the-agents-own-group-with-nip-22s-tags-as-nip-22-defines-them), [D-122](decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar)).
- The agent's comments in its own threads do not alert. The app shows a running bar of open `task` items, lifts
  waiting approvals into it, and folds finished threads ([D-122](decisions.md#d-122-a-subagents-words-what-alerts-and-the-apps-running-bar)).
- A driver names the subagent each event belongs to by its `task` item, and says whether its harness tells
  subagents apart; only Codex's does. The core places an item when its first event goes out, under its
  subagent's `task` item, and keeps that place; with no `task` item sent to the chat, it goes in the main chat.
  A subagent's words are MDK's thread messages, a later version MDK's plain edit of one, and words whose
  `task` item has disappeared go in the main chat; its items carry a `subagent` tag naming their `task` item
  ([D-142](decisions.md#d-142-how-subagent-threads-are-built)).
- Stop in the main chat stops every subagent the agent started, even after its turn; Stop in a thread stops that
  subagent and those it started. A comment in a thread is a prompt only where the driver allows it and the
  writer may prompt, and never goes to the parent ([D-123](decisions.md#d-123-stop-and-writing-in-a-subagents-thread)).
  Not built yet: no driver lets a person write to a subagent, so the core reads no comment in a thread
  ([D-142](decisions.md#d-142-how-subagent-threads-are-built)). A Stop in a thread names that subagent to a driver
  that says it can stop one, `stop_subagent`, which only Codex's does; to any other it stops the whole turn
  ([D-149](decisions.md#d-149-stop-in-a-subagents-thread-stops-that-subagent-alone)).
- A driver closes open `task` items when it can no longer follow them; the app keeps counting a quiet row and
  says when it was last heard from. A harness that does not tell subagents apart keeps them as `tool` items. An
  app shows a held edit of an item sent before the member joined as the item ([D-124](decisions.md#d-124-what-keeps-subagents-reliable-closes-harnesses-that-do-not-tell-them-apart-and-late-joiners)).
- The Codex transport gives each subagent's thread a translator in its parent's chat, and interrupts subagents
  itself ([D-125](decisions.md#d-125-the-codex-transport-follows-subagents-threads)).
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
- The core decides who may prompt and approve; the gateway allows what the core lets through ([D-084](decisions.md#d-084-the-core-decides-first-and-a-gateway-allows-what-the-core-lets-through)),
  except that Hermes's plugin also asks Hermes's own check of who may answer before it answers a request to confirm
  a command, an approval card or a question, so where Hermes's own settings limit who may answer, an answer the
  core allows can still be refused, and the item keeps waiting ([D-146](decisions.md#d-146-the-hermes-plugin-answers-a-request-to-confirm-a-command-by-its-id),
  [D-151](decisions.md#d-151-the-hermes-plugin-asks-hermess-own-check-of-who-may-answer-an-approval-card-or-a-question)).
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
- Four tracks (`marmot-agent`, `claude-code`, `openclaw`, `hermes`) are versioned separately with real SemVer
  versions (`0.1.0`) and declare `socketProtocol` (`marmot.agent-control.v3`). CI guards version consistency and
  protocol alignment on every run and checks direction-aware compatibility of `fixtures/socket/schema.json` on pull
  requests (`tools/ci/check_socket_compat.py`). `.github/workflows/release.yml` ships each track on its own tag
  (`v*`, `claude-code-v*`, `openclaw-v*`, `hermes-v*`, protected against deletion/updates/unsigned pushes by ruleset
  `#28179915`, excluding `*-rc.*`) and plans/cuts `-rc.N` prereleases from `canary`, publishing `SHA256SUMS` and
  `latest.json` compatibility manifests with zero Rust build for connector tracks. The socket and plugin installers
  (`claude-code`, `openclaw`, `hermes`) support `--connector-version` and auto-discover the newest protocol-compatible
  standalone connector release, verifying `SHA256SUMS` and `socket_protocol` and staging core + connector atomically
  ([D-165](decisions.md#d-165-four-independent-release-tracks-decoupled-by-the-socket-protocol-version), [D-166](decisions.md#d-166-per-track-releases-compatibility-manifests-and-protected-release-tags), [D-167](decisions.md#d-167-socket-and-plugin-installers-fetch-compatible-standalone-connector-releases-atomically)).

## Files

- An agent's file goes to the chat as an encrypted attachment, tagged with its turn, only from folders the
  settings allow; every harness that can send one does ([D-090](decisions.md#d-090-an-agent-sends-a-file-as-an-encrypted-attachment-from-an-allowed-folder), [D-107](decisions.md#d-107-every-harness-that-lets-an-agent-send-a-file-gets-file-sending-before-the-swap)).
- Two files upload at once and 16 are held at most; one more is refused as busy, and no upload outlives the core
  or its driver ([D-145](decisions.md#d-145-the-core-bounds-what-it-holds-for-a-connector-and-the-files-it-sends-at-once)).
- Every harness the core gives a tool gets `send_file(path)`; Hermes and OpenClaw turn their own media into the
  same request; ACP agents get it from a loopback MCP server, on by default (including Antigravity when
  `mcpCapabilities` is omitted, unless explicitly disabled) ([D-108](decisions.md#d-108-how-an-agents-file-reaches-the-core-as-built), [D-110](decisions.md#d-110-an-acp-agent-gets-the-file-tool-from-a-loopback-mcp-server-on-by-default), [D-170](decisions.md#d-170-google-antigravity-cli-is-driven-through-acp)).
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
  Only a run for the same agent and the same gateway finishes a killed upgrade that holds a plugin change;
  any other puts it back first. A gateway's install takes the install lock before it reads the gateway, a
  first install too, and one refused then leaves nothing ([D-154](decisions.md#d-154-an-install-takes-over-a-killed-upgrade-only-for-the-same-agent-and-the-same-gateway)).
  Before it puts another core in place, an upgrade stops the agent and copies its account home into its own
  record, under MDK's lock, and puts it back with the old core, leaving the agent stopped and printing how to
  start it; the copy goes when the upgrade ends, and a run with another core puts a killed upgrade that holds
  one back first. A copy that cannot go back leaves the new core in place.
  `marmot-agent init` refuses a home that holds an identity before it opens the store, and the agent's start
  refuses a home while it is being put back ([D-173](decisions.md#d-173-marmot-agent-init-refuses-a-home-with-an-identity-before-it-opens-its-store-and-no-core-starts-on-a-home-being-put-back)).
  An upgrade that started the agent and ended cannot be undone: the old core cannot open the store the new one
  moved forward. Other agents' homes are not copied ([D-159](decisions.md#d-159-an-upgrade-keeps-a-copy-of-the-agents-account-store-and-puts-it-back-with-the-old-core)).
- A gateway install moves over with its installer's `--replace-plugin`. The OpenClaw installer finishes a
  half-done move by editing OpenClaw's settings file itself, after a copy; the gateway's conversation starts
  fresh in each chat. `--replace-plugin` lists the gateway's work folder in `files_from` beside the old
  connector's folders, in the line the import wrote ([D-119](decisions.md#d-119-how-a-gateway-install-moves-over-its-work-folder-a-half-done-openclaw-move-a-fresh-conversation)).
  `--uninstall` takes our plugin out but does not put the replaced one back; going back from `backups/before-move` is by hand ([D-172](decisions.md#d-172-an-uninstall-does-not-put-back-the-plugin-a---replace-plugin-move-replaced)).
- A coding install (Codex, Claude Code, OpenCode, Pi, Goose) moves over with the import, its settings written to the
  agent's folder, then its installer, with no flag: the installer takes settings imported from its own harness's
  connector and refuses another's. It uses those settings as written, `cwd` too until the move is done, and the
  identity in the old home, never runs `init`, and runs the core, and Claude Code's connector, as services. Claude Code needs `--cwd`. A work folder
  that holds the installer's data folder, as the import's default (your home) does, or holds or lies in the old
  home or its backup, is refused. The first start
  of any agent moved over is copied, and put back on a failure, whatever core was installed before ([D-160](decisions.md#d-160-the-coding-installers-take-an-install-moved-over-and-run-it-as-a-service)).
- A start counts as done once the core says it is ready, its store open and its driver started (on Linux through
  systemd's notify protocol, `Type=notify`; on macOS in a file), and Claude Code's connector once the core took its
  hello. Only then does an upgrade end, its copy of the store go, or a move count as done; one not ready within two
  minutes is put back as a failed start ([D-178](decisions.md#d-178-an-install-counts-a-start-as-done-once-the-core-and-claude-codes-connector-say-they-are-ready)).

## Verification and how the work runs

- Each driver is tested by replaying its recordings, against a stand-in, and by one end-to-end scenario with
  the real harness ([D-074](decisions.md#d-074-every-driver-is-tested-by-replay-and-by-one-end-to-end-scenario)).
- Beyond that: a conformance suite, a deterministic simulator, TLA+ models, proofs, property tests, fuzzing,
  mutation testing and privacy checks ([D-093](decisions.md#d-093-the-core-and-the-drivers-are-verified-in-layers-with-tla-models-at-the-top)).
- Recordings are of scripted scenarios only, scrubbed; Cursor's and Antigravity's keep only their built-in commands, and Antigravity's neutralise `_meta.antigravity.tier` ([D-085](decisions.md#d-085-cursor-recordings-keep-only-cursors-built-in-commands), [D-170](decisions.md#d-170-google-antigravity-cli-is-driven-through-acp)).
- CI on a pull request stays under 15 minutes; long checks run nightly ([D-094](decisions.md#d-094-ci-on-a-pull-request-stays-under-15-minutes-and-the-rest-runs-nightly)).
- Real Claude Code and Codex run their scenarios in CI on the model double, with no model key in CI at all;
  cassettes are recorded through numes by hand, a cassette older than 14 days is a warning on a pull request and
  fails the nightly run, and the gateways stay scripted ([D-127](decisions.md#d-127-real-harnesses-run-in-ci-on-the-model-double-with-no-model-key)).
- CI runs in three tiers under one required check, `CI passed` (`tools/ci/passed.py`):
  - **Team pull requests** (same-repo branches by `write`/`maintain`/`admin` collaborators without `ci:full`) run a ~45–60s fast static gate with zero Rust compilation: verified PR commit signatures, `tree-state` check (fails closed on runs while `"red"` unless labeled `tree-fix` or `revert`), hard-enforced D-162 commit trailers, `Rust format` (`cargo fmt --all --check`), `ShellCheck`, `Recordings`, `TypeScript`, `Python`, and `Documents`. **External / fork pull requests** (and Team PRs labeled `ci:full`) fail closed into the full Rust/Clippy/tests/simulator/conformance/install-script/TLA+ suite, and external PRs require a write collaborator to review and merge the head commit.
  - **Hourly `canary`** (`.github/workflows/canary.yml`, skipping when `master == canary`) runs the full gate (everything, the simulator at 200 runs, the Docker move-over tests, and Kani proofs), refreshes `master`'s Rust build caches once per batch, and fast-forwards the protected `canary` branch on green; pushes to `master` do not trigger CI. An always-on revert-only bisect agent (`tools/ci/bisect_agent.py`, running under its own `marmot-agents-bot` user on the CI host) watches `canary.yml`, classifies failures into four buckets, bisects deterministic regressions into signed `revert` PRs, and freezes `tree-state` (`"red"`) with White Noise / Marmot alerts in Meltdown Mode when automated revert cannot recover `master`.
  - **Nightly** runs against the verified `canary` branch (new simulator seeds, larger TLA+ bounds, coverage, a 64th of the core's mutants, the cassettes), and its failed jobs open issues
  ([D-169](decisions.md#d-169-team-pull-requests-use-a-fast-static-gate-backed-by-hourly-canary-and-the-bisect-agent), [D-161](decisions.md#d-161-ci-no-longer-runs-on-pushes-to-master), [D-163](decisions.md#d-163-hourly-canary-runs-the-full-gate-on-master-and-refreshes-build-caches), [D-164](decisions.md#d-164-nightly-verification-runs-against-the-verified-canary-branch), [D-168](decisions.md#d-168-always-on-revert-only-bisect-agent-watches-hourly-canary-and-activates-meltdown-mode)).
- Property tests run on external/`ci:full` pull requests and hourly `canary` at 256 cases each, with every fuzz target's check over its seeds;
  the Kani proofs run in hourly `canary` (and on `ci:full` / manual full-tier runs); fuzzing, five minutes a target, and the
  property tests at 10,000 cases run nightly. cargo-fuzz and Kani are pinned with checksums and share one pinned
  nightly compiler ([D-144](decisions.md#d-144-property-tests-run-on-every-pull-request-kani-in-the-full-gate-and-fuzzing-nightly-on-a-pinned-nightly-compiler), [D-161](decisions.md#d-161-ci-no-longer-runs-on-pushes-to-master), [D-163](decisions.md#d-163-hourly-canary-runs-the-full-gate-on-master-and-refreshes-build-caches), [D-169](decisions.md#d-169-team-pull-requests-use-a-fast-static-gate-backed-by-hourly-canary-and-the-bisect-agent)).
- One issue, "Flaky job tracker", ranks the CI, Canary and Nightly jobs and individual tests that failed and passed
  when they were run again, kept up to date by `.github/workflows/flake-report.yml` from the runs' lists of jobs
  and JUnit XML artifacts uploaded on failure by trusted same-repository runs (`cargo-nextest`, Node 24's junit
  reporter, and `tools/ci/unittest_junit.py`); it reads no logs, stdout, stderr, or failure messages, and ignores
  fork PR artifacts ([D-143](decisions.md#d-143-one-issue-tracks-the-ci-jobs-that-flake), [D-164](decisions.md#d-164-nightly-verification-runs-against-the-verified-canary-branch), [D-171](decisions.md#d-171-the-flaky-tracker-names-individual-tests-from-machine-readable-junit-results)).
- The Rust workspace builds with MDK's compiler and forbids unsafe code ([D-053](decisions.md#d-053-the-rust-workspace-builds-with-mdks-compiler-and-forbids-unsafe-code)).
- Work follows `plan.md`; the builder decides what is internal and merges once CI passes and review left no
  blocker. Every commit is signed, and Team PR tip commits enforce `Local-Checks`, `Local-Review`, and `Release-Impact`
  trailers in CI (advisory on external/fork PRs) ([D-068](decisions.md#d-068-work-follows-the-plan-and-the-builder-decides-what-is-internal), [D-054](decisions.md#d-054-changes-arrive-as-small-signed-pull-requests-that-ci-checks), [D-162](decisions.md#d-162-pull-requests-record-verification-review-and-release-impact-trailers-on-the-tip-commit), [D-169](decisions.md#d-169-team-pull-requests-use-a-fast-static-gate-backed-by-hourly-canary-and-the-bisect-agent)).
- Decisions and design live in this repository; work items are issues. Documents serve people and agents
  ([D-016](decisions.md#d-016-decisions-and-design-live-in-this-repository-work-items-live-in-issues), [D-018](decisions.md#d-018-documents-serve-both-people-and-agents)).
