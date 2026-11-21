# AGENTS.md - marmot-agents

Rules and routing for coding agents working in this repository. `README.md` is the overview for people; do not
duplicate it here.

## What this is

Connectors that put AI agent harnesses (Claude Code, Codex, Hermes and others) into Marmot group chats.

**Status: building towards the swap.** There is a Rust workspace under `crates/` with four crates, and CI that
checks them: `marmot-agent-events`, the agent event and its eleven item types; `marmot-agent-driver`, the trait
a driver implements; `marmot-agent-codex`, the Codex driver, which runs Codex and translates both ways, with
subagents switched off for now; and `marmot-agent`, the core, which runs one agent's account through MDK and
drives Codex, with a test that runs it end to end on MDK's test relay. The core also serves a local socket, and
`packages/claude-code` is the Claude Code driver, a TypeScript program that drives Claude Code through it, and
`packages/openclaw` is the OpenClaw plugin, a channel that runs inside OpenClaw, in a container, and connects to
it. `marmot-agent-acp` is the ACP driver, replayed against the OpenCode, Goose and Cursor recordings.
`marmot-agent-pi` is the Pi driver, built into the core like Codex's, which runs Pi in a container from
`tools/pi`. What is built next, and in what order, is in
[`docs/plan.md`](docs/plan.md). The core program is called `marmot-agent`
([D-050](docs/decisions.md#d-050-the-core-program-is-called-marmot-agent)). Everything else is design and
decisions under `docs/`, plus recorded harness sessions and the tools that made them. The connectors that ship
today are in the MDK repository (`integrations/`, `crates/agent-connector`, `crates/agent-control`); they are
being replaced by what is designed here, not extended.

## Read these first

1. [`docs/README.md`](docs/README.md): the design in three minutes.
2. [`docs/plan.md`](docs/plan.md): what is being built, in what order, and how the work runs.
3. [`docs/decisions.md`](docs/decisions.md): what is decided. Treat every entry as settled.
4. [`docs/open-questions.md`](docs/open-questions.md): what is not decided.

## Where to go

| Task | Start here |
| --- | --- |
| See what to build next | `docs/plan.md` |
| Try the core by hand | `docs/try.md`, then `crates/marmot-agent-chat` |
| Understand how the parts fit | `docs/design/architecture.md` |
| Work on what agents send to the chat | `docs/design/event-model.md`, then `docs/design/event-carrier.md` |
| Work on what people send back to an agent | `docs/design/answers-and-controls.md` |
| Work on how a chat shows subagents | `docs/design/subagents.md`, then `fixtures/recordings/codex/subagents` |
| Work on the boundary to TypeScript and Python programs | `docs/design/socket-protocol.md` |
| Add or change a harness driver | `docs/design/drivers.md`, then `docs/reference/harness-interfaces.md` |
| Check what today's MDK connectors do | `docs/reference/current-connectors.md` |
| See how T3 Code, Buzz and ACP solve this | `docs/reference/prior-art.md` |
| See what Codex, Claude Code and Pi really send | `docs/reference/harness-messages.md`, then `fixtures/recordings/` |
| See what Hermes hands a platform plugin | `docs/reference/hermes-messages.md` |
| See what MDK supports for several identities in one program | `docs/reference/mdk-multiple-identities.md` |
| See what OpenClaw hands a channel plugin | `docs/reference/openclaw-messages.md` |
| Record a new session or scenario | `tools/record/README.md` |
| Try the event model against the recordings | `tools/prototype/README.md` |
| Change a diagram | `docs/images/generate.py` |
| Work on the item types in Rust | `crates/marmot-agent-events`, after `docs/design/event-model.md` |
| Work on the driver trait or the capability flags | `crates/marmot-agent-driver`, after `docs/design/drivers.md` |
| Work on the Codex driver | `crates/marmot-agent-codex`, after `fixtures/recordings/codex` and its `expected/` events |
| Work on the Pi driver | `crates/marmot-agent-pi`, after `fixtures/recordings/pi`, its `expected/` events and the Pi section of `docs/design/drivers.md` |
| Work on the Claude Code driver's translation | `packages/claude-code/src/translate.ts`, after `fixtures/recordings/claude` and its `expected/` events |
| Work on the Claude Code driver's transport | `packages/claude-code/src/connector.ts`, after `docs/design/drivers.md` and `docs/design/socket-protocol.md` |
| Work on the ACP driver | `crates/marmot-agent-acp`, after `fixtures/recordings/acp` and its `expected/` events |
| Work on the OpenClaw plugin's translation | `packages/openclaw/src/translate.ts`, after `fixtures/recordings/openclaw` and its `expected/` events |
| Work on the OpenClaw plugin's transport or its container | `packages/openclaw/src/connector.ts` and `src/index.ts`, after `docs/design/drivers.md` and `docs/reference/openclaw-messages.md` |
| Work on the core | `crates/marmot-agent`, after `docs/plan.md` and `docs/design/architecture.md` |
| Change the compiler, the workspace settings or CI | `rust-toolchain.toml`, `Cargo.toml`, `.cargo/config.toml`, `.github/workflows/ci.yml` |
| Change the link check | `lychee.toml` |

## The shape, in brief

- One Rust **core program** holds the Marmot side (through MDK as a library) and the conversation handling.
- Each harness has a **driver**: it speaks the harness's own interface and translates to shared Marmot agent
  events.
- Rust drivers (Codex, Pi, ACP) are built into the core behind one trait; Cursor uses the ACP one. The Claude
  Code driver is a TypeScript program; Hermes and OpenClaw are plugins inside those runtimes. All three connect
  to the core over a **local socket**.
- Every agent event carries an item type from a shared vocabulary, a plain rendering, and the harness's native
  payload.

## How we work

- **The plan is the go-ahead.** Build what [`docs/plan.md`](docs/plan.md) lays out, milestone by milestone,
  without asking at each step. Build nothing outside it.
- **Decide what is internal yourself**: code layout, defaults, which cases to handle, tests. Record a choice in
  `docs/decisions.md` only when others can see it.
- **The project owner decides** what other programs, apps or people see, privacy and security, who may do
  what, any change to a decision, and the plan's scope and order. Gather those questions, normally once per
  milestone, each with your recommendation and the reason. Carry on meanwhile with the recommendation, marked
  as provisional, except on privacy or security, which wait.
- **One pull request per piece of work that makes sense alone**, on its own branch from `master`.
- **One round of review per code change**, by the two other models. Fix blockers, and check again only if the
  fix is large. Fix lesser findings when cheap; otherwise open an issue. Decisions and documents need none.
- **Merge** once CI passes and the review left no blocker unfixed. The owner reads each milestone's work at its
  end.
- **Sign every commit.** If signing or `gh` is not set up on the machine, stop and say so.
- **Do not touch the MDK repository.** Read it when you need to. Changes to it are the owner's to arrange.
- **Report plainly.** If a test fails or you skipped a step, say so.

These are recorded as
[D-054](docs/decisions.md#d-054-changes-arrive-as-small-signed-pull-requests-that-ci-checks) and
[D-068](docs/decisions.md#d-068-work-follows-the-plan-and-the-builder-decides-what-is-internal).

## Rules that are settled

Each rule links to its decision. Do not work around one; if you think it is wrong, say so and propose a
replacement entry.

- Drivers never decide who may prompt or approve, and never touch keys, groups or relays. The core does
  ([D-004](docs/decisions.md#d-004-four-layers-with-drivers-behind-one-trait),
  [D-010](docs/decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)).
- Nothing above a driver depends on which harness is behind it
  ([D-004](docs/decisions.md#d-004-four-layers-with-drivers-behind-one-trait)).
- Pass agent activity through: reasoning, tool calls, results, plans. Do not hide it
  ([D-008](docs/decisions.md#d-008-pass-everything-through)).
- Never drop the native payload when translating
  ([D-007](docs/decisions.md#d-007-one-shared-event-vocabulary)). There are two exceptions: a secret
  question that is no longer waiting carries none
  ([D-056](docs/decisions.md#d-056-a-secret-question-carries-a-native-message-only-while-it-is-waiting)),
  and the core leaves it out of an item over 64 KB. A driver never leaves out or cuts anything to fit
  that limit ([D-026](docs/decisions.md#d-026-an-agent-item-is-at-most-64-kb),
  [D-057](docs/decisions.md#d-057-the-core-applies-the-64-kb-limit)).
- Build on the vendor's official interface or library where one exists
  ([D-006](docs/decisions.md#d-006-prefer-official-interfaces-and-libraries)). Codex is the one exception
  to "library": its driver speaks `codex app-server` with types of its own, and links neither OpenAI's crates
  nor `codex-codes`
  ([D-055](docs/decisions.md#d-055-the-codex-driver-reads-codexs-messages-with-types-of-its-own)).
- Cross-language code talks to the core over the socket. Do not add linked libraries or C interfaces
  ([D-003](docs/decisions.md#d-003-the-boundary-to-other-languages-is-a-local-socket)).
- Use MDK as a Rust library. Do not shell out to its command-line tool
  ([D-002](docs/decisions.md#d-002-one-rust-core-program-with-mdk-as-a-library)).
- Prompting and approving default to the owner only
  ([D-010](docs/decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)).
- Build with the compiler in `rust-toolchain.toml`, which is MDK's. No unsafe code: `.cargo/config.toml` forbids
  it in every crate, and a `RUSTFLAGS` environment variable would replace that setting, so do not set one.
  Every crate lives under `crates/`, takes the workspace's settings and is named `marmot-agent-…`
  ([D-053](docs/decisions.md#d-053-the-rust-workspace-builds-with-mdks-compiler-and-forbids-unsafe-code),
  [D-050](docs/decisions.md#d-050-the-core-program-is-called-marmot-agent)).

## Rules from the MDK connectors do not carry over

The existing connectors have many rules in their own `AGENTS.md` files. This project starts from a blank slate.
A rule applies here only if it is in `docs/decisions.md`. In particular, "never expose reasoning or tool output"
was dropped on purpose.

`docs/reference/current-connectors.md` ends with a list of behaviours that work today and should not be lost.
Those are things to preserve, not rules to copy.

## Keeping the record

- **A decision made in conversation is not real until it is in `docs/decisions.md`.** Add it in the same change
  as the work it affects, with the date and the reason.
- **Never change the meaning of an old entry.** Add a new one and mark the old one as replaced.
- **When a question is settled, move it** from `docs/open-questions.md` to `docs/decisions.md`.
- **Every design page states its status** at the top: what on it is decided and what is a proposal. Keep that
  line true.
- **Work items are GitHub issues**, not files. The wiki is not used
  ([D-016](docs/decisions.md#d-016-decisions-and-design-live-in-this-repository-work-items-live-in-issues)).
- **An issue is a proposal, whoever opened it.** It settles nothing, even when it is titled after an open
  question or written as if the matter were closed. Read it as input. A point is decided only when the project
  owner accepts it and it is recorded in `docs/decisions.md`. Do not design or build around an issue that has
  not been accepted.

## Writing documents

There are two kinds, for two readers
([D-018](docs/decisions.md#d-018-documents-serve-both-people-and-agents)).

- **For people:** `README.md`, `docs/README.md` and `docs/design/`. Short, plain words, a diagram where it shows
  something prose cannot. A newcomer should be able to follow `docs/README.md` in three minutes. Put detail in a
  reference page and link to it.
- **For agents:** `docs/reference/`. As long and exact as needed. Each page starts by saying when its facts were
  read, how, and what was and was not checked.

Other rules:

- **Harness facts go stale in weeks.** Before relying on a version, command or capability in
  `docs/reference/`, re-check it and update the page and its date.
- **Mark what you inferred.** If a claim was concluded and not read in a source, say so.
- **Diagrams are generated.** Edit `docs/images/generate.py`, run `python3 docs/images/generate.py`, and commit
  the light and dark SVG files together. Never edit an SVG by hand. Embed a diagram with a `<picture>` element
  that offers both, as `docs/README.md` does.
- **Do not add `CLAUDE.md` files.** `AGENTS.md` is the one agent-instruction file, as in MDK.

## Privacy

This is a private messenger. Do not put real keys, account or group identifiers, relay addresses, message text,
prompts or model output into documents, fixtures, tests or logs. What a log may hold is in
[D-071](docs/decisions.md#d-071-what-may-be-logged).

Recorded sessions have their own rules:

- **Record only scripted scenarios in a throwaway copy of `tools/record/demo`.** Never commit a recording of
  real work, even scrubbed.
- **Keep raw recordings outside the repository.** They contain local paths and account details.
- **Run `tools/record/scrub.py` and read its output before committing.** It fails if it finds a path, e-mail
  address, host name or user name, but a clean exit is not proof. Check the diff yourself.
- **Real sessions may be analysed locally for structure only**: message kinds, field names, types and counts,
  as in `docs/reference/data/`. Never values.

## Related repositories

| Repository | Relation |
| --- | --- |
| `marmot-protocol/mdk` | The Marmot implementation this project depends on, and home of the connectors being replaced |
| `marmot-protocol/marmot` | The protocol specification; the agent event kinds will probably be specified there |
| `marmot-protocol/whitenoise-android`, `whitenoise-ios` and the other apps | The clients that render agent events |

## Verification

CI runs these commands on each pull request and on `master` (`.github/workflows/ci.yml`), with two exceptions
that are marked below: the diagrams and `tools/record/inventory.py`. Those two are checked only by you. Run
everything that applies before you push. If you change a command here, change it there.

For any change to Rust code, `Cargo.toml`, `Cargo.lock` or `rust-toolchain.toml`:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
```

`rustup` installs the pinned compiler the first time. `--locked` fails when `Cargo.lock` is out of date: update
it on purpose, with `cargo update` or by building once without the flag, and commit it.

The core links MDK, whose markdown parser is written in Odin and needs one exact Odin release to build. Install
it once with `bash tools/odin/install.sh <absolute folder>`, which checks its checksum, and set `ODIN` to the
`odin` program in that folder. The MDK revision, the `[patch]` entries in `Cargo.toml` and
`tools/odin/install.sh` move together.

For any change to a Markdown file, or to a file a Markdown file links to:

```sh
lychee .    # must report 0 errors
```

[lychee](https://github.com/lycheeverse/lychee) is a link checker. It is a tool, not a dependency of the code:
nothing in `Cargo.toml` names it. CI installs the release named in `.github/workflows/ci.yml`. Install the same
one, from its release page or with `cargo install lychee --locked --version <that version>`. If you cannot
install it, say so in the pull request and let CI run the check.

The settings are in `lychee.toml`. The check covers links inside the repository, and the headings they name.
It does not fetch web addresses; check those by hand.

For any change under `packages/claude-code` or `packages/openclaw`, in that package, with Node 24 (CI uses the
version named in `.github/workflows/ci.yml`):

```sh
cd packages/claude-code   # or packages/openclaw
npm ci
npm run check   # tsc --noEmit
npm test        # replays every recording of that harness against its expected events, and runs the
                # transport against fakes: a fake core and a fake SDK, or a fake OpenClaw
```

For a change to a diagram, which CI does not check:

```sh
python3 docs/images/generate.py   # commit the regenerated files
git diff --stat                   # the SVG files must not change unless generate.py did
```

For a change to the recordings, the recording tools, the prototype or the event model:

```sh
python3 tools/record/inventory.py fixtures/recordings/codex --kinds    # every file must parse; CI does not run this
python3 tools/record/inventory.py fixtures/recordings/claude --kinds   # nor this
python3 tools/record/inventory.py fixtures/recordings/acp/opencode --kinds   # nor this
python3 tools/record/inventory.py fixtures/recordings/acp/goose --kinds      # nor this
python3 tools/record/inventory.py fixtures/recordings/acp/cursor --kinds     # nor this
python3 tools/record/inventory.py fixtures/recordings/pi --kinds             # nor this
python3 -m unittest discover -s tools/prototype -p 'test_*.py'         # cases no recording holds
python3 tools/prototype/translate.py --check fixtures/recordings/codex fixtures/recordings/claude \
    fixtures/recordings/hermes fixtures/recordings/openclaw fixtures/recordings/acp/opencode \
    fixtures/recordings/acp/goose fixtures/recordings/acp/cursor fixtures/recordings/pi
python3 tools/prototype/translate.py --expected fixtures/recordings/codex --check   # the Codex driver's expected events
python3 tools/prototype/translate.py --expected fixtures/recordings/claude --check  # the Claude Code driver's
python3 tools/prototype/translate.py --expected fixtures/recordings/acp/opencode --check  # the ACP driver's
python3 tools/prototype/translate.py --expected fixtures/recordings/acp/goose --check     # and again
python3 tools/prototype/translate.py --expected fixtures/recordings/acp/cursor --check    # and again
python3 tools/prototype/translate.py --expected fixtures/recordings/pi --check      # the Pi driver's
python3 tools/prototype/translate.py --expected fixtures/recordings/openclaw --check  # the OpenClaw plugin's
```

The `--check` over the eight folders must end with "native messages with no translation: none". If you change
the event model, change `tools/prototype/translate.py` to match and run it again.

The Codex driver is tested against `fixtures/recordings/codex/expected`, the Claude Code driver against
`fixtures/recordings/claude/expected`, the ACP driver against `expected/` in each folder under
`fixtures/recordings/acp`, the Pi driver against `fixtures/recordings/pi/expected`, and the OpenClaw plugin
against `fixtures/recordings/openclaw/expected`. The prototype writes them all, and
`crates/marmot-agent-events/tests/expected.rs` checks that the core reads every event in them. When the
prototype's output for one harness
changes on purpose, write them again with `python3 tools/prototype/translate.py --expected` and that folder,
such as `fixtures/recordings/codex`, and commit them. Never write them
again only to make the driver's test pass: when the two disagree, the recording and the event model decide
which is wrong.

Before relying on a claim in `docs/reference/harness-messages.md`, find it in a recording. Each recording is
one harness version on one day.
