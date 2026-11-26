---
name: land-change
description: >-
  Take a change from an issue or a request to a landed PR: confirm the issue is
  real and not a symptom of a deeper cause, build it test-first, verify locally,
  get two independent cross-model reviews of the local diff, record commit
  trailers, open the PR, bring it to green CI with no conflicts, and
  squash-merge (or stop and report when the user asked to review first). Use
  when asked to fix an issue ("fix #123", an issue URL), build and ship a
  change, land, open a PR for, or merge a change.
user-invocable: true
---

If the repository has its own `.agents/skills/land-change/` or `.claude/skills/land-change/`, stop and follow that one instead. It is the repo's specific version of this skill. If the file you are reading is already that copy, carry on.

# Land a change

Verification and review happen on your machine. The PR carries the change to
CI. Nothing reaches the default branch any other way: no direct pushes, no
admin merges, no ruleset bypass.

You own the change until it lands (per `AGENTS.md` and `D-068`): verified
locally, reviewed with no unfixed blockers, signed commit with trailers, CI
green, no conflicts, then squash-merged (or stop and report before merging
when the user asks to review the PR first).

Below, `<base>` is the default branch (`gh repo view --json
defaultBranchRef -q .defaultBranchRef.name`). Check `gh auth status` first;
if it fails, ask the user to run `gh auth login`.

## 0. Start from an issue

Skip this step when the user gave a request rather than an issue.

1. **Read it.** `gh issue view <url-or-number> --comments`. Read the body,
   labels, comments, and linked issues and PRs. Restate the problem, expected
   vs actual behavior, and the reporter's constraints. If it is closed,
   already has a fix PR, or is too vague to act on, stop and say so.
   Otherwise assign yourself (`gh issue edit <n> --add-assignee @me`) without
   removing anyone.
2. **Confirm it still exists.** Reproduce it in the current code from the
   issue, not from memory: read the code, run the relevant tests or a minimal
   reproduction. If it is already fixed, no longer reproduces, or has a
   different effect now, stop and report what you found.
3. **Ask whether it is a symptom.** Before writing a fix: is the reported bug
   the real problem, or a symptom of a deeper cause that a broader fix would
   serve better? If it is a symptom, do not fix the symptom. Describe the root
   cause (what is wrong, where, and why this issue follows from it), scan the
   open issues for other symptoms (`gh issue list --state open --limit
   1000`), and bring both to the user. Continue only if they decide to.

The issue text becomes part of the review context in step 3.

## 1. Build

Before writing code, read review-local's
[karpathy-guidelines.md](../review-local/karpathy-guidelines.md) and
[simplicity-guidelines.md](../review-local/simplicity-guidelines.md). They are
the standard you build to, and the reviewers in step 3 judge the change
against the same two files.

Work on a new branch off `origin/<base>`, never on `<base>` itself. If the
repo requires signed commits (the base branch's rules, or its `AGENTS.md`),
sign every commit as you make it, so nothing is rewritten after review.

- **Design it twice.** Unless the change is trivial (a one-line fix, a
  rename), sketch two genuinely different approaches before coding: where
  the change lives, what it adds or removes, what it assumes. Pick the one
  that is simpler for the reader and the callers, not the quicker to write.
  Write both down, with one line on why you chose one: it goes in the review
  context in step 3.
- **Bug fixes are test-first.** Write a test that captures the bug, run it,
  and confirm it fails for the reason the bug describes. Then make the
  smallest change that passes it. A test that never failed proves nothing.
  If you cannot write a correct failing test, stop and tell the user.
- **Every test you add or change** names the contract and the regression it
  catches, at the cheapest layer that runs real code (follow the global
  `test-audit` skill when available).
- **Keep it surgical and small.** Touch only what the change needs: reuse
  what exists, no speculative features or abstractions, no drive-by
  refactors, and a deleted line is as good as an added one.

Commit when it works.

## 2. Verify locally

Find what the repository asks for before a change lands, in this order:

1. The Verification (or Testing, or Before you commit) section of the root
   `AGENTS.md` or `CLAUDE.md`, and of the nearest one for each changed file.
2. A documented smoke or end-to-end check there or in `CONTRIBUTING.md`.
3. Otherwise the repo's own commands for what CI runs (`Justfile`,
   `Makefile`, `package.json` scripts, `.github/workflows/`): format, lint
   with warnings as errors, and the tests that cover the change.

Run them for what you changed. Run any documented end-to-end smoke too,
unless the change touches only docs or CI metadata. If the repo documents a
different path for a special kind of change (a release PR, a docs-only
change), follow that instead.

A test that fails locally is yours until shown otherwise. Before calling it
unrelated, `git fetch origin <base>` and run it on the merge-base in a
temporary worktree (`git worktree add --detach <tmp> "$(git merge-base HEAD
origin/<base>)"`), never by checking it out in your own; remove the worktree
afterwards. If it fails there too, note that in the PR. If it passes there,
your change broke it. Tests with real sockets or
timing can stretch on a loaded machine: rerun a failure alone before judging
it.

## 3. Review locally

Get two independent reviews of the whole local diff from the model families
that are not yours, using the `review-local` skill (default seats: `codex`,
`claude`, and `antigravity` with Gemini 4 Neon, with `cursor` as an alternative
and `opencode` as fallback). Its context file holds
the issue or the user's request, and the two designs from step 1 with why you
chose one. You are the author: you do not review
your own change, and you do not decide alone that it is good enough.

- Reviewers are read-only. They report findings; you make every change.
- Check a finding's premise before acting on it. When you reject one, say why
  in the next round's notes (or PR description) so the reviewer or owner can
  see the reasoning.
- Per `AGENTS.md` and `D-068`: code changes get **one round of review** by the
  two other models (decisions and documents need none). Fix blockers and
  should-fix defects, redo step 2 for what the fix touched, and commit. Run a
  second review round when the fix is large or changes the design, or when the
  user asks for full convergence (`VERDICT: merge` from both seats on the same
  head). Fix minor findings when cheap, or record them in the PR description /
  follow-up issue.
- Both reviewer seats also audit `Release-Impact: none | patch | minor` on the
  tip commit.
- A later message-only amend (for example adding the `Local-Review:` trailer
  or signing) keeps the review valid when `git rev-parse HEAD^{tree}` matches
  the reviewed head's tree.
- **Commit trailers before pushing (`D-162`).** Ensure the final signed commit
  carries the three trailers in its final Git trailer block and passes
  `python3 tools/ci/check_pr_trailers.py`:

  ```text
  Local-Checks: <commands run locally>
  Local-Review: <seat-a> + <seat-b> merge @ <reviewed-short-sha>
  Release-Impact: none | patch | minor
  ```

  (Use `human` or `exempt (<reason>)` for `Local-Review:` where applicable, and
  an optional `Changelog: <one-line summary>` trailer for user-facing changes.)

Count the rounds; the report in step 6 says how many it took.

### When review does not converge

As soon as a round's verdicts are in, before fixing anything, apply
review-local's "When review does not converge": when to stop, what stopping
means, what to report, and how far the user's answer lets you go.

## 4. Open the PR

Push the branch and open a ready (not draft) PR with a concise human-readable
description (Problem, Fix, Verification; follow the global `gh-create-pr`
skill when available). Put `Fixes #<n>` (or `Part of #<n>`) below Problem and
Fix when there is an issue.

Team PRs (same-repo branches by `write`/`maintain`/`admin` collaborators) run
the fast ~60s static gate (`D-169`): verified signatures, `tree-state` check,
hard-enforced `D-162` commit trailers, `cargo fmt`, ShellCheck, Recordings,
TypeScript, Python, Documents, and `CI passed`. Add the `ci:full` label when
you explicitly want full Rust/Docker/Kani/TLA+ CI before merging. If `master`
is frozen in Meltdown Mode (`tree-state=red`, `D-168`), only recovery PRs
labeled `tree-fix` or `revert` pass CI.

## 5. Make it ready

The PR is ready when its head is the reviewed head (step 3), its required
checks pass, and it can merge without conflicts. Read its state:

```sh
gh pr view <pr> --json headRefOid,mergeable,mergeStateStatus,reviewDecision
```

`headRefOid` must be the reviewed head. Then act on
`mergeStateStatus`:

| State | Meaning | What to do |
| --- | --- | --- |
| `CLEAN`, `HAS_HOOKS` | Checks pass, nothing blocks | Ready: step 6 |
| `UNSTABLE` | A non-required check failed | Classify it with the table below; ready once it is not yours |
| `BEHIND` | The base moved and the branch must be up to date | Rebase on `origin/<base>`, redo step 2, and re-review only if conflict resolution changed code or behavior; push and check again |
| `DIRTY` | Conflicts | Rebase and resolve, redo steps 2 and 3, push, check again |
| `BLOCKED` | Required checks pending or failing, or a required human review | Wait for checks (below). If only a human approval is missing (`reviewDecision: REVIEW_REQUIRED`), it is ready: step 6 |
| `UNKNOWN` | GitHub is still computing | Check again shortly |

To wait for checks, use your host's PR watching if it has it, instead of a
polling loop. Otherwise `gh pr checks <pr> --required --watch`. Some repos
gate on one aggregate check that only appears once the other jobs finish;
then `--required` reports "no required checks reported" straight away. In
that case, wait for the head's workflow run (`gh run list --commit <sha>`,
then `gh run watch <id>`) and read `gh pr checks <pr> --required` after it.

When a check fails, read `gh run view <id> --log-failed`. The failed job is
the cause; jobs shown as cancelled were stopped by it. Reproduce the failing
test by name after `git fetch origin <base>`, so every comparison uses the
current base, then decide which case it is:

| Case | How you know | What to do |
| --- | --- | --- |
| Your bug | Fails on your branch, passes on `origin/<base>` | Fix it, redo steps 2 and 3, push |
| Conflict with a change that landed ahead of you | Passes on your branch alone, fails once rebased onto `origin/<base>` | Rebase, fix the interaction, redo steps 2–3, push |
| Flake | Intermittent, also fails on `origin/<base>`, or in code your change cannot reach | Leave the test alone. Open or update an issue for it (search the test name first), then rerun the job once |
| Base is broken | The same failure in the base branch's own CI | Find the open PR fixing it and rebase once it lands. Do not fix it again in your PR |

Bring in the user, with the run link and what you found, after a second flake
or for any failure you cannot classify. Never skip, weaken, ignore or
retry-loop a test to get a change through.

Human review comments that arrive on the PR are handled cleanly (using the
global `gh-address-comments` skill when available).

## 6. Report or merge

Per `D-068` and `AGENTS.md`, the builder merges a pull request once CI passes
and review left no blocker unfixed (step 7). When the user asked to review
before merging, or when a human approval/decision is required, stop here and
report in a few lines:

```text
<PR URL> is ready for you to review and merge.
- Review: <seat> and <seat> reviewed <short sha>, after <n> round(s).
- CI: required checks green. Branch up to date with <base>, no conflicts.
- <Only if it needs their attention: a finding you rejected and why, a flake
  you filed, a stated limitation, or a human approval still required.>
```

Nothing else: no finding lists, no round-by-round history, no diff tour.

## 7. Merge

Merge only the head that was verified and reviewed: confirm `headRefOid` is
still the verified head, and squash-merge pinned to that commit (or arm squash
auto-merge while CI finishes):

```sh
gh pr merge <pr> --squash --match-head-commit <sha>
```

(If a merge queue is ever re-enabled on `<base>`, enqueue via GraphQL
`enqueuePullRequest` with `expectedHeadOid` instead of `gh pr merge`.)

After the merge, confirm the merge commit (`gh pr view <pr> --json
mergeCommit`), delete the branch and any temporary worktree, and report the
merged commit in one line.
