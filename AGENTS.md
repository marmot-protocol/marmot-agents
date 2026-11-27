# AGENTS.md - marmot-agents

Rules and routing for coding agents working in this repository. `README.md` is the overview for people; do not
duplicate it here.

## What this is

Connectors that put AI agent harnesses (Claude Code, Codex, Hermes and others) into Marmot group chats.

**Status: every harness is driven; building towards the swap (M4).** The Rust workspace under `crates/` has
ten crates, and CI checks them: `marmot-agent-events`, the agent event and its eleven item types;
`marmot-agent-driver`, the trait a driver implements; the built-in drivers `marmot-agent-codex` (Codex, with
its subagents shown in the chat that started them, each in a thread of its own,
[D-117](docs/decisions.md#d-117-codexs-subagents-are-switched-on-and-shown-flat-in-the-chat-that-started-them),
[D-142](docs/decisions.md#d-142-how-subagent-threads-are-built)),
`marmot-agent-acp` (OpenCode, Goose, Cursor and Antigravity over ACP) and `marmot-agent-pi` (Pi, in a container from
`tools/pi`); `marmot-agent`, the core, which runs one agent's account through MDK and
drives a built-in driver or serves a local socket; `marmot-agent-chat`, a terminal client for trying it;
`marmot-agent-sim`, a deterministic simulator that runs the real core against a simulated MDK and a scripted
driver, with faults, from a seed; and `marmot-agent-conformance`, the suite every driver must pass, on scriptable
fakes of the harnesses; and `marmot-agent-fuzz`, the fuzz targets for everything the core reads from outside.
Over the socket connect `packages/claude-code`, the Claude Code driver, and two **channel plugins**,
`packages/openclaw` for OpenClaw and `python/marmot_hermes` for Hermes, which run inside those gateways as one of
their channels, with a narrower contract than a driver
([D-113](docs/decisions.md#d-113-hermes-and-openclaw-are-channel-plugins-with-a-narrower-contract-than-a-driver)). Each driver
has a scenario that runs it end to end against the real harness on MDK's test relay. What is built next, and in what order, is in
[`docs/plan.md`](docs/plan.md). The core program is called `marmot-agent`
([D-050](docs/decisions.md#d-050-the-core-program-is-called-marmot-agent)). Everything else is design and
decisions under `docs/`, plus recorded harness sessions and the tools that made them. The connectors that ship
today are in the MDK repository (`integrations/`, `crates/agent-connector`, `crates/agent-control`); they are
being replaced by what is designed here, not extended.

## Read these first

1. [`docs/README.md`](docs/README.md): the design in three minutes.
2. [`docs/plan.md`](docs/plan.md): what is being built, in what order, and how the work runs.
3. [`docs/current-state.md`](docs/current-state.md): what is true of the system now, in five minutes, with a link
   to the decision behind each line.
4. [`docs/decisions.md`](docs/decisions.md): the record of what is decided, and why. Read the entries you need.
   Treat every entry as settled.
5. [`docs/open-questions.md`](docs/open-questions.md): what is not decided.

## Where to go

| Task | Start here |
| --- | --- |
| See what to build next | `docs/plan.md` |
| See what is true of the system now | `docs/current-state.md`, then the decisions it links to |
| Try the core by hand | `docs/try.md`, then `crates/marmot-agent-chat` |
| Understand how the parts fit | `docs/design/architecture.md` |
| Work on what agents send to the chat | `docs/design/event-model.md`, then `docs/design/event-carrier.md` |
| Work on what people send back to an agent | `docs/design/answers-and-controls.md` |
| Work on how a chat shows subagents | `docs/design/subagents.md`, then `docs/reference/subagents-prior-art.md` and `fixtures/recordings/codex/subagents` |
| Work on the boundary to TypeScript and Python programs | `docs/design/socket-protocol.md` |
| Change a socket message, or see every one exactly | `crates/marmot-agent/src/socket/wire.rs`, then `fixtures/socket` |
| Add or change a harness driver | `docs/design/drivers.md`, then `docs/reference/harness-interfaces.md` |
| See what a channel plugin must and need not do | `docs/design/drivers.md` ("Channel plugins"), then the gateway's reference page |
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
| Work on the Hermes plugin's translation | `python/marmot_hermes/translate.py`, after `fixtures/recordings/hermes` and its `expected/` events |
| Work on the Hermes plugin's transport | `python/marmot_hermes/connector.py` and `adapter.py`, after `docs/design/drivers.md` and `docs/reference/hermes-messages.md` |
| Work on the core | `crates/marmot-agent`, after `docs/plan.md` and `docs/design/architecture.md` |
| Work on the install scripts | `install/lib.sh`, then the harness's script beside it, after `README.md` ("Installing an agent") |
| Work on the release | `.github/workflows/release.yml`, `tools/release/package.sh`, `package_connector.sh`, `plan_release.py`, `resolve_release.py`, and `write_manifest.py` |
| Work on moving an install over | `crates/marmot-agent/src/import`, after `docs/move.md` and `docs/reference/current-connectors.md` |
| Run or change the end-to-end swap test on a test machine | `docs/swap-test.md`, after `docs/move.md` |
| Change the compiler, the workspace settings or CI | `rust-toolchain.toml`, `Cargo.toml`, `.cargo/config.toml`, `.github/workflows/ci.yml`, `nightly.yml` and `.github/actions/rust` |
| Plan or add tests, models or proofs | `docs/design/verification.md` |
| See which jobs are flaky, or fix one | The open "Flaky job tracker" issue, then `docs/design/verification.md` ("Flaky tests") and `tools/ci/flake_report.py` |
| Run a real harness on a scripted or recorded model, or record a cassette | `tools/model_double/README.md`, after `docs/design/verification.md` ("A model double") |
| Work on the simulator, or replay one of its failure capsules | `crates/marmot-agent-sim/README.md`, after `docs/design/verification.md` |
| Work on the driver conformance suite or its fake harnesses, or add a driver to it | `crates/marmot-agent-conformance/README.md`, after `docs/design/verification.md` |
| Add or change a property test, a fuzz target or a Kani proof | `docs/design/verification.md` (layers 5 to 7), then `crates/marmot-agent/tests/properties`, `crates/marmot-agent-fuzz/src/lib.rs` or the `proofs` modules |
| Change the link check | `lychee.toml` |

## The shape, in brief

- One Rust **core program** holds the Marmot side (through MDK as a library) and the conversation handling.
- Each harness has a **driver**: it speaks the harness's own interface and translates to shared Marmot agent
  events.
- Rust drivers (Codex, Pi, ACP) are built into the core behind one trait; Cursor uses the ACP one. The Claude
  Code driver is a TypeScript program. Hermes and OpenClaw are messaging gateways, and connect through a
  **channel plugin** inside each: the gateway keeps its sessions, media, allow-lists and tool verbosity, and the
  core still decides who may prompt and approve. All three connect to the core over a **local socket**.
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
- **One round of review per code change**, by the two other models (using
  [`.agents/skills/review-local`](.agents/skills/review-local/SKILL.md) and
  [`.agents/skills/land-change`](.agents/skills/land-change/SKILL.md)). Fix blockers, and check again only if the
  fix is large. Fix lesser findings when cheap; otherwise open an issue. Decisions and documents need none.
- **Record commit trailers on the final signed commit**
  ([D-162](docs/decisions.md#d-162-pull-requests-record-verification-review-and-release-impact-trailers-on-the-tip-commit),
  preserved on `master` under GitHub's `COMMIT_MESSAGES` squash setting), and verify them locally with
  `python3 tools/ci/check_pr_trailers.py`:
  - `Local-Checks: <commands run locally>`
  - `Local-Review: <seat-a> + <seat-b> merge @ <short-sha>` (or `human` / `exempt (<reason>)`)
  - `Release-Impact: none | patch | minor` (audited by both review seats)
  - Optional `Changelog: <one-line summary>` for user-facing changes.
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

- Drivers and channel plugins never decide who may prompt or approve, and never touch keys, groups or relays.
  The core does
  ([D-004](docs/decisions.md#d-004-four-layers-with-drivers-behind-one-trait),
  [D-010](docs/decisions.md#d-010-the-owner-sets-who-may-prompt-and-who-may-approve)).
- Nothing above a driver depends on which harness is behind it
  ([D-004](docs/decisions.md#d-004-four-layers-with-drivers-behind-one-trait)).
- Pass agent activity through: reasoning, tool calls, results, plans. Do not hide it
  ([D-008](docs/decisions.md#d-008-pass-everything-through)).
- Never drop the native payload when translating
  ([D-007](docs/decisions.md#d-007-one-shared-event-vocabulary)). There are three exceptions: a secret
  question that is no longer waiting carries none
  ([D-056](docs/decisions.md#d-056-a-secret-question-carries-a-native-message-only-while-it-is-waiting)),
  for the pilot the core sends none to a chat that is not sent the agent's full activity
  ([D-136](docs/decisions.md#d-136-for-the-pilot-the-agents-full-activity-goes-only-to-the-owners-two-person-chat-unless-the-owner-lists-the-group)),
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
- **A change that adds or replaces a decision updates `docs/current-state.md` in the same change**: the line it
  changes, its link, and the commit the page was brought up to date at
  ([D-114](docs/decisions.md#d-114-a-current-state-page-sits-beside-this-record-and-every-new-decision-updates-it)).
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

CI runs these commands (`.github/workflows/ci.yml`), with two exceptions that are marked below: the diagrams
and `tools/record/inventory.py`. Those two are checked only by you. Run everything that applies before you push:
team PRs run a fast static gate in CI and rely on local verification + hourly `canary`
([D-169](docs/decisions.md#d-169-team-pull-requests-use-a-fast-static-gate-backed-by-hourly-canary-and-the-bisect-agent)):

| Tier | When | What |
| --- | --- | --- |
| Fast Team PR | Same-repo PR by a `write`/`maintain`/`admin` collaborator without `ci:full`; ~45–60 seconds, zero Rust compile | Verified PR commit signatures, `tree-state` check (fails closed when `red` unless labeled `tree-fix` or `revert`), hard-enforced D-162 commit trailers, `Rust format` (`cargo fmt --all --check`), `ShellCheck`, `Recordings`, `TypeScript`, `Python`, `Documents`, and `CI passed` |
| External / fork PR (or `ci:full` label) | Fork PRs, non-collaborators, read-only members, or any PR labeled `ci:full`; under 10 minutes (`ci:full` runs the full gate) | All Rust Clippy, workspace tests, conformance, simulator, install scripts, and TLA+ models when watched paths changed; external PRs are merged only by a collaborator with write access after reviewing the head commit |
| Hourly `canary` | Hourly on `master` (`.github/workflows/canary.yml`), skipping when `master == canary`; aim under 30 minutes ([D-161](docs/decisions.md#d-161-ci-no-longer-runs-on-pushes-to-master), [D-163](docs/decisions.md#d-163-hourly-canary-runs-the-full-gate-on-master-and-refreshes-build-caches), [D-168](docs/decisions.md#d-168-always-on-revert-only-bisect-agent-watches-hourly-canary-and-activates-meltdown-mode)) | Full gate whatever changed, simulator at `MARMOT_AGENT_SIM_RUNS=200`, Docker move-over tests, Kani proofs, and `master` cache refresh; fast-forwards `canary` on green, or watched by the revert-only bisect agent |
| Nightly | Once a night on the verified `canary` branch (`.github/workflows/nightly.yml`, [D-164](docs/decisions.md#d-164-nightly-verification-runs-against-the-verified-canary-branch)), and by hand | The simulator on a new seed with many runs, the property tests at `PROPTEST_CASES=10000`, five minutes of each fuzz target, TLC on larger bounds, a coverage report, a 64th of the core's mutants, and `check_cassettes.py --strict` on the model double's cassettes; a failed job opens an issue |

The job `CI passed` needs every other job of `ci.yml` and is the one check `master` requires. It checks the run's
jobs against the list of names each tier expects (`EXPECTED` in `ci.yml`), so a new job, or a new leg of a matrix,
goes in that list and in its `needs`. Label a team PR `ci:full` to run the full gate before merging, or dispatch
either tier on a branch by hand: `gh workflow run ci.yml --ref <branch> -f tier=pull-request` (or `tier=full`).
While `tree-state` is `red` (Meltdown Mode), only PRs labeled `tree-fix` or `revert` pass the `tree-state` gate.
CI's Rust jobs also set `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=line-tables-only`, which change how fast
they build and what a cache holds, not what is tested.

For any change to Rust code, `Cargo.toml`, `Cargo.lock` or `rust-toolchain.toml`:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
```

`rustup` installs the pinned compiler the first time. `--locked` fails when `Cargo.lock` is out of date: update
it on purpose, with `cargo update` or by building once without the flag, and commit it.

The workspace's tests include the driver conformance suite (`crates/marmot-agent-conformance`), which runs every
built-in driver on fakes of its harness in about 30 seconds; its Pi tests need `python3`, as Pi's own do. For a
change to a driver, run it alone with `cargo test -p marmot-agent-conformance`.

The workspace's tests include the property tests (D-144): the core's, `cargo test -p marmot-agent --test
properties`, and every built-in translator over its recordings changed at random, `cargo test -p
marmot-agent-conformance --test translators`, each property at proptest's 256 cases unless `PROPTEST_CASES` says
otherwise (the nightly run sets 10,000). A failing case is written to the test's `proptest-regressions` file
(`crates/marmot-agent/tests/proptest-regressions/`, or beside `translators.rs`): commit it with the fix, so it runs
first from then on. They also run every fuzz target's check over its seeds (`crates/marmot-agent-fuzz`, its
`tests/seeds.rs`), on the pinned compiler.

The workspace's tests also run the real Claude Code and the real Codex through the core on the model double
(`tools/model_double`), which stands in for their model, so they need no key or sign-in. They need Node 24 with
`npm ci` run in `packages/claude-code`, and `codex` on the `PATH`, at the release CI installs.

CI runs the workspace's tests in two jobs side by side, the core's package (`-p marmot-agent`) and every other
(`--workspace --exclude marmot-agent --exclude marmot-agent-sim`), which together are the command above.

The workspace's tests include the simulator's at a small count of runs. CI's Rust jobs leave them out and run
them in a job of its own, in release: at the normal count on a pull request, and at 200 in hourly `canary`. For a
change to the core's run loop, conversation or Marmot side, run that too:

```sh
MARMOT_AGENT_SIM_RUNS=200 cargo test --locked --release -p marmot-agent-sim --all-features
```

Hourly `canary` also runs the move-over tests that need Docker and nothing else, no key and no sign-in: today's
Hermes and OpenClaw installs moved over, with a scripted model. Run them for a change to the import or the
install scripts:

```sh
cargo test --locked --all-features -p marmot-agent --test import_gateways -- --ignored --nocapture --test-threads=1
```

The socket connectors, Claude Code, Hermes and OpenClaw, run the same conformance suite over the core's real
socket, each with a scripted harness inside it (`crates/marmot-agent/tests/conformance_socket.rs`). They need
Python 3.14 and Node 24 with `npm ci` run in both packages, so the test is ignored in the core's tests and CI
runs it in a job of its own, `Conformance (socket)`. Run it for a change to a connector, the core's socket or
the suite:

```sh
cargo test --locked --all-features -p marmot-agent --test conformance_socket -- --ignored
```

Every other ignored test needs a model key or a signed-in harness, and CI runs none of them.

A failure prints the path of a capsule, and CI keeps the capsules as the run's `simulator-capsules` artifact;
`crates/marmot-agent-sim/README.md` says how to replay one.

The socket's messages are described by `fixtures/socket/schema.json`, made from the types in
`crates/marmot-agent/src/socket/wire.rs`, with examples of each message in `fixtures/socket/examples.json`
that the core writes and reads. The workspace's tests fail when either file differs from what the types and
the socket make. When a message changes on purpose, write both again and commit them:

```sh
MARMOT_AGENT_WRITE_SOCKET=1 cargo test -p marmot-agent --lib socket::corpus
```

The Claude Code and OpenClaw packages and the Hermes plugin check themselves against both files in their own
tests, so a change on one side only fails one of them. A field a connector sends that the core's types do not
hold fails too.

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
version named in `.github/actions/node/action.yml`):

```sh
cd packages/claude-code   # or packages/openclaw
npm ci
npm run check   # tsc --noEmit
npm test        # replays every recording of that harness against its expected events, runs the
                # transport against fakes: a fake core and a fake SDK, or a fake OpenClaw, and checks
                # every message both ways against fixtures/socket
```

For any change under `python/`, with Python 3.14 (CI uses the release named in `python/.python-version` and
`.github/workflows/ci.yml`). The Hermes plugin uses the standard library and what Hermes provides, and nothing
else:

```sh
cd python
python3 -m unittest discover -s tests   # replays every Hermes recording against its expected events,
                                        # runs the transport against a fake core and a fake Hermes, and
                                        # checks every message both ways against fixtures/socket
```

For any change under `install/` or `tools/release/`, or to `tools/tla/run.sh`, with
[ShellCheck](https://github.com/koalaman/shellcheck) at the release CI names (it is a tool, like lychee):

```sh
shellcheck -x install/*.sh tools/release/archive.sh tools/release/package.sh tools/release/package_connector.sh tools/tla/run.sh
python3 -m unittest discover -s install -p 'test_*.py'   # runs each install script against a throwaway
                                                         # home, with stand-ins for what it calls
```

`.github/workflows/release.yml` runs on per-track version tags and on manual `workflow_dispatch` (dry-run planning or
`-rc.N` cuts from `canary`), so check changes to it by reading it, by [actionlint](https://github.com/rhysd/actionlint)
if you have it, and with the `tools/release` tests below. Check a change to `ci.yml`, `nightly.yml` or
`.github/actions/` with actionlint too, and by running the tier it changes on its branch by hand (above);
`nightly.yml` can be run by hand only once it is on `master`. For a change to `ci.yml`'s jobs or to `tools/ci`,
which the `CI passed` job runs:

```sh
python3 -m unittest discover -s tools/ci -p 'test_*.py'   # the gate passes each tier, and fails when a job
                                                          # the tier expects never ran, was skipped or failed;
                                                          # the flaky job tracker counts each job that failed
                                                          # and then passed, once, and reads no log
python3 tools/ci/check_socket_compat.py --base origin/master   # per-track versions, protocol alignment, and
                                                               # direction-aware socket schema compatibility (D-165)
python3 -m unittest discover -s tools/release -p 'test_*.py'   # release planner, resolver, connector packaging, and manifests
```

For a change to a function a Kani proof covers (a `proofs` module under `#[cfg(kani)]` in the core: the rights
check, the socket's line reader and token comparison, and where the carrier's cut falls), with
[Kani](https://github.com/model-checking/kani) at the release CI names in `ci.yml` (a tool, like lychee: `cargo
install --locked kani-verifier --version <it>`, then `cargo kani setup`, which installs Kani's own nightly
compiler). The first run builds the core under Kani, which takes about 5 minutes on a fast machine:

```sh
cargo kani -p marmot-agent   # every proof holds, each over every input up to the bound it states
```

For a change to a fuzz target, or to code one reads, fuzz it for a while with cargo-fuzz at the release the
nightly workflow names, on the nightly compiler it names (`FUZZ_TOOLCHAIN`, with its `llvm-tools-preview`),
from its seeds. `cargo fuzz list --fuzz-dir crates/marmot-agent-fuzz` lists the targets:

```sh
cargo run -p marmot-agent-fuzz --example seeds -- target/fuzz-corpus
cargo +nightly-2026-08-21 fuzz run --fuzz-dir crates/marmot-agent-fuzz --features libfuzzer \
    --target x86_64-unknown-linux-gnu <target> target/fuzz-corpus/<target> -- -max_total_time=300
```

The `--target` is for cargo-fuzz's own release builds, which are built for musl and otherwise build the
targets for musl, where the sanitizer does not work. On another machine give its own triple.

An input that makes a target fail is written under `crates/marmot-agent-fuzz/artifacts/`, which git ignores.
Never commit one as it is: make it a test, as small as it can be, the way the property tests' cases are kept.

The same command tests `tools/ci/flake_report.py`, the script of the flaky job and test tracker
([D-143](docs/decisions.md#d-143-one-issue-tracks-the-ci-jobs-that-flake),
[D-171](docs/decisions.md#d-171-the-flaky-tracker-names-individual-tests-from-machine-readable-junit-results));
check its workflow, `.github/workflows/flake-report.yml`, with actionlint. It counts flaky jobs and names
individual failing tests from JUnit XML artifacts on trusted same-repository runs, reading no job logs, stdout,
stderr, or failure messages. The workflow runs only from `master`; once it is there,
`gh workflow run flake-report.yml` brings the tracker up to date by hand, and opens it if none is open.

For any change under `formal/` or `tools/tla/`, or to the code a model follows (the core for `CoreLifecycle`,
`formal/tla/README.md`; the core's socket and the Claude Code, OpenClaw and Hermes connectors for
`SocketProtocol`, `formal/tla/SocketProtocol.md`; the built-in drivers for `DriverTransport`,
`formal/tla/DriverTransport.md`), with Java 21, or Docker when there is no Java (the script then runs TLC in a
pinned Java image):

```sh
python3 tools/tla/check.py   # the fair models pass; every counterexample, deliberate defect and
                             # reachability probe fails as stated, and each counterexample's schedule
                             # is the committed one. TLC is downloaded once, pinned by version and
                             # checksum, by tools/tla/run.sh
```

The nightly workflow runs `python3 tools/tla/check.py --bounds larger`, safety on `CoreLifecycle`'s larger
bounds in `formal/tla/README.md`, which takes longer than a pull request may.

For a change to a diagram, which CI does not check:

```sh
python3 docs/images/generate.py   # commit the regenerated files
git diff --stat                   # the SVG files must not change unless generate.py did
```

For a change to the recordings, the recording tools, the model double, the prototype or the event model:

```sh
python3 tools/record/inventory.py fixtures/recordings/codex --kinds    # every file must parse; CI does not run this
python3 tools/record/inventory.py fixtures/recordings/claude --kinds   # nor this
python3 tools/record/inventory.py fixtures/recordings/acp/opencode --kinds   # nor this
python3 tools/record/inventory.py fixtures/recordings/acp/goose --kinds      # nor this
python3 tools/record/inventory.py fixtures/recordings/acp/cursor --kinds     # nor this
python3 tools/record/inventory.py fixtures/recordings/acp/antigravity --kinds  # nor this
python3 tools/record/inventory.py fixtures/recordings/pi --kinds             # nor this
python3 -m unittest discover -s tools/prototype -p 'test_*.py'         # cases no recording holds
python3 -m unittest discover -s tools/record -p 'test_*.py'            # the scrubber and the gateway recorders
shellcheck -x tools/record/*/record.sh tools/record/publish-recording.sh   # CI does not run this
python3 -m unittest discover -s tools/model_double -p 'test_*.py'      # the model double's wire formats
python3 tools/prototype/translate.py --check fixtures/recordings/codex fixtures/recordings/claude \
    fixtures/recordings/hermes fixtures/recordings/openclaw fixtures/recordings/acp/opencode \
    fixtures/recordings/acp/goose fixtures/recordings/acp/cursor fixtures/recordings/acp/antigravity \
    fixtures/recordings/pi
python3 tools/prototype/translate.py --expected fixtures/recordings/codex --check   # the Codex driver's expected events
python3 tools/prototype/translate.py --expected fixtures/recordings/claude --check  # the Claude Code driver's
python3 tools/prototype/translate.py --expected fixtures/recordings/acp/opencode --check  # the ACP driver's
python3 tools/prototype/translate.py --expected fixtures/recordings/acp/goose --check     # and again
python3 tools/prototype/translate.py --expected fixtures/recordings/acp/cursor --check    # and again
python3 tools/prototype/translate.py --expected fixtures/recordings/acp/antigravity --check  # and again
python3 tools/prototype/translate.py --expected fixtures/recordings/pi --check      # the Pi driver's
python3 tools/prototype/translate.py --expected fixtures/recordings/openclaw --check  # the OpenClaw plugin's
python3 tools/prototype/translate.py --expected fixtures/recordings/hermes --check  # the Hermes plugin's
```

The `--check` over the nine folders must end with "native messages with no translation: none". If you change
the event model, change `tools/prototype/translate.py` to match and run it again.

The Codex driver is tested against `fixtures/recordings/codex/expected`, the Claude Code driver against
`fixtures/recordings/claude/expected`, the ACP driver against `expected/` in each folder under
`fixtures/recordings/acp`, the Pi driver against `fixtures/recordings/pi/expected`, the OpenClaw plugin
against `fixtures/recordings/openclaw/expected`, and the Hermes plugin against
`fixtures/recordings/hermes/expected`. The prototype writes them all, and
`crates/marmot-agent-events/tests/expected.rs` checks that the core reads every event in them. When the
prototype's output for one harness
changes on purpose, write them again with `python3 tools/prototype/translate.py --expected` and that folder,
such as `fixtures/recordings/codex`, and commit them. Never write them
again only to make the driver's test pass: when the two disagree, the recording and the event model decide
which is wrong.

Before relying on a claim in `docs/reference/harness-messages.md`, find it in a recording. Each recording is
one harness version on one day.
