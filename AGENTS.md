# AGENTS.md - marmot-agents

Rules and routing for coding agents working in this repository. `README.md` is the overview for people; do not
duplicate it here.

## What this is

Connectors that put AI agent harnesses (Claude Code, Codex, Hermes and others) into Marmot group chats.

**Status: the first code.** There is a Rust workspace under `crates/` with three crates, and CI that checks
them: `marmot-agent-events`, the agent event and its eleven item types; `marmot-agent-driver`, the trait a
driver implements; and `marmot-agent-codex`, the Codex driver, of which only the translation is written. No
socket or core exists yet. What is being built now is
the item types, the driver trait and the Codex driver
([D-052](docs/decisions.md#d-052-the-codex-driver-is-built-first)). The core program is called `marmot-agent`
([D-050](docs/decisions.md#d-050-the-core-program-is-called-marmot-agent)). Everything else is design and
decisions under `docs/`, plus recorded harness sessions and the tools that made them. The connectors that ship
today are in the MDK repository (`integrations/`, `crates/agent-connector`, `crates/agent-control`); they are
being replaced by what is designed here, not extended.

## Read these first

1. [`docs/README.md`](docs/README.md): the design in three minutes.
2. [`docs/decisions.md`](docs/decisions.md): what is decided. Treat every entry as settled.
3. [`docs/open-questions.md`](docs/open-questions.md): what is not decided.

## Where to go

| Task | Start here |
| --- | --- |
| Understand how the parts fit | `docs/design/architecture.md` |
| Work on what agents send to the chat | `docs/design/event-model.md`, then `docs/design/event-carrier.md` |
| Work on what people send back to an agent | `docs/design/answers-and-controls.md` |
| Work on the boundary to TypeScript and Python programs | `docs/design/socket-protocol.md` |
| Add or change a harness driver | `docs/design/drivers.md`, then `docs/reference/harness-interfaces.md` |
| Check what today's MDK connectors do | `docs/reference/current-connectors.md` |
| See how T3 Code, Buzz and ACP solve this | `docs/reference/prior-art.md` |
| See what Codex and Claude Code really send | `docs/reference/harness-messages.md`, then `fixtures/recordings/` |
| See what Hermes hands a platform plugin | `docs/reference/hermes-messages.md` |
| See what MDK supports for several identities in one program | `docs/reference/mdk-multiple-identities.md` |
| See what OpenClaw hands a channel plugin | `docs/reference/openclaw-messages.md` |
| Record a new session or scenario | `tools/record/README.md` |
| Try the event model against the recordings | `tools/prototype/README.md` |
| Change a diagram | `docs/images/generate.py` |
| Work on the item types in Rust | `crates/marmot-agent-events`, after `docs/design/event-model.md` |
| Work on the driver trait or the capability flags | `crates/marmot-agent-driver`, after `docs/design/drivers.md` |
| Work on the Codex driver | `crates/marmot-agent-codex`, after `fixtures/recordings/codex` and its `expected/` events |
| Change the compiler, the workspace settings or CI | `rust-toolchain.toml`, `Cargo.toml`, `.cargo/config.toml`, `.github/workflows/ci.yml` |
| Change the link check | `lychee.toml` |

## The shape, in brief

- One Rust **core program** holds the Marmot side (through MDK as a library) and the conversation handling.
- Each harness has a **driver**: it speaks the harness's own interface and translates to shared Marmot agent
  events.
- Rust drivers (Codex, Pi, ACP) are built into the core behind one trait. Claude Code and Cursor drivers are
  TypeScript programs; Hermes and OpenClaw are plugins inside those runtimes. All four connect to the core over
  a **local socket**.
- Every agent event carries an item type from a shared vocabulary, a plain rendering, and the harness's native
  payload.

## How we work

- **The project owner decides.** When you reach a choice that is the owner's, give your recommendation and the
  reason, and carry on with work that does not depend on it.
- **One small pull request per step**, each on its own branch from `master`. Never merge. The owner merges.
- **Sign every commit.** If signing or `gh` is not set up on the machine, stop and say so.
- **Do not touch the MDK repository.** Read it when you need to. Changes to it are the owner's to arrange.
- **Build only what has been asked for.** Today that is the item types, the driver trait and the Codex driver
  ([D-052](docs/decisions.md#d-052-the-codex-driver-is-built-first)). Do not start the core, the socket or
  another driver.
- **Report plainly.** If a test fails or you skipped a step, say so.

These are recorded as
[D-054](docs/decisions.md#d-054-changes-arrive-as-small-signed-pull-requests-that-ci-checks).

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
prompts or model output into documents, fixtures, tests or logs. The detailed logging rules are not written yet
([Q-13](docs/open-questions.md#q-13-what-may-be-logged)).

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

For a change to a diagram, which CI does not check:

```sh
python3 docs/images/generate.py   # commit the regenerated files
git diff --stat                   # the SVG files must not change unless generate.py did
```

For a change to the recordings, the recording tools, the prototype or the event model:

```sh
python3 tools/record/inventory.py fixtures/recordings/codex --kinds    # every file must parse; CI does not run this
python3 tools/record/inventory.py fixtures/recordings/claude --kinds   # nor this
python3 -m unittest discover -s tools/prototype -p 'test_*.py'         # cases no recording holds
python3 tools/prototype/translate.py --check fixtures/recordings/codex fixtures/recordings/claude \
    fixtures/recordings/hermes fixtures/recordings/openclaw
python3 tools/prototype/translate.py --expected fixtures/recordings/codex --check   # the Codex driver's expected events
```

The `--check` over the four folders must end with "native messages with no translation: none". If you change
the event model, change `tools/prototype/translate.py` to match and run it again.

The Codex driver is tested against `fixtures/recordings/codex/expected`, which the prototype writes. When the
prototype's Codex output changes on purpose, write them again with
`python3 tools/prototype/translate.py --expected fixtures/recordings/codex` and commit them. Never write them
again only to make the driver's test pass: when the two disagree, the recording and the event model decide
which is wrong.

Before relying on a claim in `docs/reference/harness-messages.md`, find it in a recording. Each recording is
one harness version on one day.
