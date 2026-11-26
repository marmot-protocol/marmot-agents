# InstallUpgrade: a TLA+ model of an install's upgrade

Status: built and checked with TLC on small bounds, as the verification page plans for the parts of the system
that face crashes and reordering (see [the inventory](inventory.md)). The model, its bounds and its switches are
internal and change as the work goes. Read on 2026-11-24 against master `7118005`, on branch
`t3/survey-and-model-state-machines`, which also fixes the first three findings below; brought up to date on
2026-11-25 against master `4d7baac`, on branch `install-two-agents`, which fixes the fourth (#180,
[D-154](../../docs/decisions.md#d-154-an-install-takes-over-a-killed-upgrade-only-for-the-same-agent-and-the-same-gateway)).

`InstallUpgrade.tla` models what an install script does to what every agent in a home shares
([D-134](../../docs/decisions.md#d-134-an-upgrade-of-the-core-and-its-support-files-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)):
the core, the support files, Pi's container image tag, and the plugin an earlier install put into a person's
Hermes
([D-116](../../docs/decisions.md#d-116-a-gateways-plugin-swap-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)).
It follows `install/lib.sh` and the harness scripts beside it. A run may be killed outright (SIGKILL) before any
step, its own steps of putting back included, or stopped (TERM, or a command that fails), after which its exit
trap puts back what it changed. After a kill the person runs an installer again, of the same harness or another.
Each result below is stated with its bounds; none of it says the installer is verified.

## What is modelled

Every shell command that changes something shared is one step. What is installed is `none`, `old` (as an earlier
install left it) or `new` (as this release has it): the core; the support files in parts, `S` for `LICENSE` and
`install`, which every install replaces, and `pi`, `hermes` and `claude` for each harness's own folder (Codex has
none); the image the tag `marmot-agents-pi` names, and `marmot-agents-pi:staging`; and our plugin in each Hermes
agent's Hermes, a copy of the `hermes` part. `Agents` names the agents of Hermes, each with a Hermes of its own
(`HERMES_HOME`) and a plugin change of its own (`$MA_AGENT_DIR/plugin-change`); the agents of the other
harnesses change nothing shared of their own, so one stands for all of each. Goose's image is handled by the same functions as Pi's, so `pi` stands for
both; OpenClaw's plugin change is the same as Hermes's, so `hermes` stands for both (inferred from the code: both
call `image_commit` and `change_begin` alike).

| Action | In the code |
| --- | --- |
| `Start(s, h, ag)` | The person runs `install/<h>.sh` for agent `ag` (`--name`): one of the first runs, or one after a run left something under way |
| `Lock(s)` | `lock_installs` in `install_core`: `flock` on `install.lock`, waiting while another run holds it; then `install_core` looks for `$MA_UPGRADE` |
| `Restore(s)` | `change_restore` (since the fix), at the top of `upgrade_begin` and in `finish`: a plugin change moved into the record as the upgrade ended goes back to its place |
| `Begin(s)` | `upgrade_begin`: takes over a record whose list names only parts this run replaces and, since the fourth fix, whose plugin changes not done are all this agent's (`changes_ours`), adding its own parts (`upgrade_list`); puts back one it cannot finish (`upgrade_put_back other`); or makes a record (`.upgrade.new`, renamed into place whole) |
| `Prepare(s)` | `harness_prepare`: Pi's `docker build -t marmot-agents-pi:staging`; `MA_PREPARED` is set |
| `Publish(s)`, `Core(s)` | `publish_support`: `upgrade_begin`, then `install` and `mv -f` of the new core |
| `Keep(s)`, `Put(s)` | `upgrade_keep` for each part (the part there before moved into the record, or what a killed run put there removed), then `mv` of the staged part into `$MA_LIB` |
| `Note(s)`, `Connect(s)`, `Replace(s)` | `change_begin`: since the second fix, the change's record listed in the upgrade's first (`change_note`, `$MA_UPGRADE/plugin-changes`); then a copy of the plugin and the record naming it, or the record a killed run left, which this run finishes; then `replace_dir` into the person's Hermes |
| `StartServices(s)` | `start_services`: where an install most often fails (`systemctl`) |
| `Commit(s)`, `Tag(s)` | `image_commit`: since the fix, the image the tag names written into the record (`$MA_UPGRADE/image`) unless a killed run wrote it; then `docker tag` and `docker rmi` of the staging tag |
| `End(s)`, `Retire(s)` | `upgrade_end`: since the fix, the plugin change's record moved into the upgrade's (`plugin-change`, with `plugin-change.path`); then `upgrade_retire`, one `mv` that ends both. The agent's Hermes is then meant to have the plugin this run put in (`want`, for U1) |
| `ChangeEnd(s)` | `change_end` and the rest of `main`: the copies of our old plugin dropped (before the fix, the change's record removed here); the run ends |
| `Fail(s)` | A command fails, or TERM, INT or HUP arrives: the trap runs `finish`, with those signals ignored |
| `FinishChange(s)`, `FinishChangeEnd(s)` | `finish` → `change_put_back`: the plugin's copy put back, the record removed; since the fourth fix, only for a change the upgrade does not list, which none is: one it lists goes back in `upgrade_put_back`'s replay, in its turn |
| `FinishUpgrade(s)` | `finish` → `upgrade_put_back`, given `MA_UPGRADING` |
| `PutBackPlugin`, `PutBackPluginEnd`, `PutBackCore`, `PutBackPart`, `PutBackMove`, `PutBackImage`, `PutBackRetire` | `upgrade_put_back`, from `finish` or from `upgrade_begin`: since the second fix, first each plugin change the record lists whose record is still there, this run's own too since the fourth (`upgrade_put_back_changes`: the plugin's copy put back, then the change's record removed, one change after another); the core from its copy; each listed part (a kept one by `rm` then `mv`, an absent one removed, one never moved left); since the fix, the image tag from the record (`image_put_back_from`); then `upgrade_retire` |
| `FinishImage(s)` | `finish` → `harness_put_back` → `image_put_back`: before the fix, the tag put back on the image this run found, given `IMAGE_TAGGED`; since, only the staging tag taken away. The run ends |
| `Kill(s)` | SIGKILL: the run ends where it is; the system lets the lock go |

## What is not

- **A command that fails while putting back.** `upgrade_put_back` and `change_put_back` go on and say what they
  could not put back, and where the copies are; the model's put backs never fail. Neither does a `docker tag` or
  `docker rmi` (both may be refused when docker is gone), nor a look at which image a tag names: when docker
  cannot say, `image_commit` stops the install before the tag moves, and `image_put_back_from` fails, which keeps
  the record (`image_id`; `install/test_install.py` stops the one at every step and checks the other).
  Two such failures keep the record too, which `PutBack` in `install/test_install.py` checks on lib.sh's own
  functions: a plugin change that cannot go back stops the ones listed before it, so none of them puts back
  older copies while it is still to go back; and a change `change_restore` cannot put back in its place stays
  in the record, which is not retired.
- **The two renames of `replace_dir` without `mv --exchange`** (macOS): a kill between them leaves the person's
  gateway with neither plugin, which `docs/move.md` states. The model has the exchange, one step.
- **Two agents with one gateway, and one agent moved to another gateway.** Each Hermes agent here has a Hermes
  of its own, so the order in which the plugin changes an upgrade lists go back changes nothing, and the model
  takes them in any order. The installer allows a second agent into one Hermes only while its settings name
  no other agent's home, read under the install lock, which a first install takes too, once an upgrade the
  run cannot finish is put back (`upgrade_recover`): so after
  the first's run was killed, before or after it wrote its home there, and not once it finished. An agent may
  also be run again with another `HERMES_HOME`; with the fourth fix, neither can leave a change listed behind
  another's, as `changes_ours` also asks that a change name this run's gateway. `install/test_install.py` `TwoAgents` checks both: two agents into one
  Hermes in the order A, B, A, the last failing, and one agent into a second Hermes. The order of the replay,
  the last first, which matters there, is checked on lib.sh's own functions (`PutBack`).
- **OpenClaw's agents**, which work as Hermes's do (`MA_GATEWAY` is the file `openclaw config file` names).
- **What the agent's own folder holds** (settings, launchers, services, `before-move`): written by one agent's
  install only, and put back by none.
- **The old connector's plugin replaced on a move** (`--replace-plugin`): its put back is the same as our own
  plugin's, and the old connector speaks to MDK's connector, not to this core, so no mismatch follows from it.
  `install/test_install.py` `Interrupted` stops such a move at every step.
- **Dry runs**, `--uninstall` (which gives a left plugin change up), and the stale folders a later run removes
  (`lib.new.*`, `.upgrade.done.*`).
- **A run's own files staged in `lib.new.$$`**, which no other run reads.
- **The agent's account store**, which an upgrade copies into its record before it puts a new core in place, and
  puts back with the core
  ([D-159](../../docs/decisions.md#d-159-an-upgrade-keeps-a-copy-of-the-agents-account-store-and-puts-it-back-with-the-old-core)).
  It is one more thing that goes with the record, as Pi's image tag does; `install/test_install.py` stops such an
  upgrade at every step and checks that the store is always the core's (`Upgrade`,
  `test_every_step_keeps_the_store_with_its_core`). Not modelled as of 2026-11-26.
- **Versions.** Every install installs the same new release; three releases would not change which steps run.

## Trusted assumptions

Besides the switches below: `mv` and `mv -f` of a file or folder within one file system are atomic; the record is
made whole under another name and renamed into place (`.upgrade.new`); every agent of a home holds the lock
through the same file, and the system lets it go when the run ends, however it ends; docker keeps an image a tag
no longer names (dangling) until something prunes it, so a tag can be put back on it by its id.

| Switch | When on | Shown when off |
| --- | --- | --- |
| `FairPerson` | After a run left something under way, the person runs an installer again, and each harness's installer in time | `install-unfair`: what a killed run left stays under way (L1) |

## Properties

| Id | Property | Form |
| --- | --- | --- |
| U1 | Once no install is under way and none left an upgrade under way, the core is of the release of the files every install replaces, Pi's image tag names the image Pi's scripts were made for, and our plugin in each Hermes agent's Hermes is the one the last install of that agent that ended put in, or, if none has, the one it had: never one a run that did not end left there, nor one newer than the Hermes files. With one agent, it is the one the Hermes files hold. Never the new core with the old files, or the other way round, wherever a run was killed or stopped | invariant `U1` |
| U3 | Once no install is under way and none left an upgrade under way, no plugin change is left under way either: it ended or was put back with its upgrade, so no record is left to put back copies older than what its Hermes has by then | invariant `U3` |
| U2 | One install at a time is past the lock | invariant `U2` |
| L1 | What a run left under way is finished or put back, once the person runs installers again | `<>[]Quiescent` |

U1 says what [D-134](../../docs/decisions.md#d-134-an-upgrade-of-the-core-and-its-support-files-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)
promises. It allows another harness's own folder to stay as it was when one harness upgrades the core: an install
replaces only its harness's parts, by design. While an upgrade is left under way, anything may be mixed; the next
run of that harness finishes it or puts it back, and L1 says it does. A plugin change left under way without an
upgrade is no excuse: a change is made only during an upgrade, and the gateway runs the plugin it has. U1 said
"nor a plugin change" until the third finding below, which is how the model missed it. With two agents, an
agent whose install has not run since the Hermes files changed keeps the plugin it had, by design: each agent's
install changes its own Hermes only. So U1 asks of each agent's plugin that it be the one its own last install
that ended put in (`want`, set as that install's upgrade ends), which a plugin a killed run left breaks.

The liveness property holds under strong fairness for each slot's steps and, with `FairPerson`, for each
harness's `Start`. Kills and failures are bounded, so they end.

## Configurations and results

Checked on 2026-11-25 with TLC v1.7.4 (Java 21, in the pinned image) by `tools/tla/check.py`, as the other
models are: the cases that must pass on 4 workers, those that must fail on one. Every configuration has an older
install to upgrade but the first, 2 kills and 2 runs that fail or are stopped, unless it says otherwise.

| Case | Configuration | Expected | Distinct states | Counterexample |
| --- | --- | --- | ---: | --- |
| `install-pi` | `InstallUpgrade.pi.cfg` (Pi in a container; a run after a kill may be Codex's) | every property holds | 9,113 | |
| `install-hermes` | `InstallUpgrade.hermes.cfg` (Hermes with our plugin; then maybe Codex) | every property holds | 7,275 | |
| `install-lock` | `InstallUpgrade.lock.cfg` (two Claude Code installs at once; then maybe Codex) | every property holds | 12,545 | |
| `install-first` | `InstallUpgrade.first.cfg` (a first install of Pi, nothing there before) | every property holds | 5,605 | |
| `install-all` | `InstallUpgrade.all.cfg` (an older install of each harness, a Hermes install, then any) | every property holds | 19,114 | |
| `install-agents` | `InstallUpgrade.agents.cfg` (two Hermes agents, each with its own Hermes; then either, or Codex) | every property holds | 14,007 | |
| `install-unfair` | `InstallUpgrade.unfair.cfg` (no `FairPerson`, 1 kill, no failure) | L1 fails | 120 | |
| `install-bug-ImageOutsideRecord` | Pi's, the code before the fix | U1 fails | 1,680 | [`install-image-put-back-apart`](counterexamples/install-image-put-back-apart.json) |
| `install-bug-ChangeEndsApart` | Hermes's, the code before the fix | U1 fails | 1,942 | [`install-plugin-change-ends-apart`](counterexamples/install-plugin-change-ends-apart.json) |
| `install-bug-ChangeOutsideRecord` | Hermes's, the code before the second fix | U1 fails | 4,693 | [`install-plugin-put-back-apart`](counterexamples/install-plugin-put-back-apart.json) |
| `install-bug-TakesOverAnother` | two agents', the code before the fourth fix | U1 fails | 11,899 | [`install-another-agents-change-left`](counterexamples/install-another-agents-change-left.json) |

Every case of this model together takes under a minute here, so all of it runs on every pull request that
changes `install/`. `install-all` has fewer states than before the second fix (92,983 then): another harness's
install no longer leaves a plugin change under way, for later runs to finish. With the defect
`ChangeOutsideRecord` it had 103,226. The fourth fix added `want` and the agents, and a run that fails no longer
puts its own plugin change back apart from the upgrade's, which moved `install-hermes`, `install-all` and
`ChangeEndsApart` by a few hundred states or fewer.

**Each check catches its bug.** On the configuration named, with U1 only:

| `Bug` | What it breaks | Configuration | Distinct states to the violation |
| --- | --- | --- | ---: |
| `ImageOutsideRecord` | the tag put back from what the run noted, apart from the record: the code before the fix | Pi's | 1,680 |
| `ChangeEndsApart` | the plugin change ended after the upgrade, apart: the code before the fix | Hermes's | 1,942 |
| `ChangeOutsideRecord` | the upgrade's record does not list the plugin change, so putting it back leaves the plugin: the code before the second fix | Hermes's | 4,693 |
| `TakesOverAnother` | a run takes over an upgrade that holds another agent's plugin change, and ends it with its own only: the code before the fourth fix | two agents' | 11,899 (U3 alone: 11,367) |
| `NoLock` | no lock: a second install takes over or puts back an upgrade the first is still making | two installs | 16,069 |
| `FreshRecord` | a run after a kill makes a record of its own over the one left, which holds the new core | Pi's | 3,101 |
| `KeepOverwrites` | `upgrade_keep` moves what is in place into the record even when the record holds the part there before | Pi's | 7,416 |
| `RecordAfterCore` | the new core is put in before the record is made | Pi's | 15 |

**No empty passes.** Each probe must be found false: `NeverTakenOver` (a run takes over an upgrade a killed run
left), `NeverPutBackOther` (Codex's install puts back Pi's), `NeverImagePutBack` (the tag is put back from the
record), `NeverUpgraded` (an upgrade ends with everything new), `NeverRestored` (a run killed once it put in the
new core and files has them put back by the next), on Pi's configuration; `NeverChangeFinished` (a run finishes a
plugin change a killed run left) and `NeverPluginPutBack` (a run puts a plugin change back with the upgrade that
lists it), on Hermes's; `NeverPutBackAnotherAgent` (a Hermes agent's install puts back an upgrade that holds
another agent's plugin change), on two agents'; and `NeverWaited` (a second install waits for the lock), on two
installs. Each is reached within 5,000 distinct states.

## What the model found

Three windows in which an upgrade's parts end up apart, and a fourth that review found. **All are fixed**: the
first three on 2026-11-24, each shown
first by a test of `install/test_install.py` that stops an install at every step
(`test_every_step_keeps_the_image_with_its_scripts`, `test_every_step_keeps_the_hermes_plugin_with_its_core`), and
the model follows the fixes. The model found the first two, each reached by a TERM as well as by a kill; review
found the third, which the model then missed because of how U1 was stated:

- **Pi's and Goose's image tag was put back apart from the scripts** (U1). `image_commit` noted the image the tag
  named in a variable of the run, and the exit trap put the tag back from it whenever the run did not end well,
  even once the upgrade was done: a TERM while the summary is printed left the new scripts running the old image.
  And a run killed after the tag moved and before the upgrade ended left the tag on the new image, so the next
  run that failed, or another harness's install, put the old scripts back under it (that run noted the new image
  as the one before). Now the image is written into the upgrade's record before the tag moves, unless a killed
  run wrote it, and the tag is put back with the record, by whatever puts the upgrade back.
- **The plugin change ended after the upgrade, apart** (U1). A TERM between `upgrade_end` and `change_end`, or a
  kill there followed by a run that failed, put the old plugin back in the person's Hermes over the new core.
  Now the change's record is moved into the upgrade's just before the one rename that ends the upgrade, so both
  end together, and is moved back to its place by a run that finds it there or is stopped before the rename.
- **Another harness's install put back the core and left the plugin** (U1, with U1 stated as above). A Hermes
  install killed once it had put our new plugin in, and then Codex's install, which cannot finish that upgrade:
  `upgrade_put_back other` put back the core and the support files and left the new plugin over the old core,
  with the change's record still in the Hermes agent's folder for a later Hermes install. A Hermes install that
  took the upgrade over and failed before its own change did the same. Now `change_begin` lists the change's
  record in the upgrade's before it writes it (`change_note`, `$MA_UPGRADE/plugin-changes`), and
  `upgrade_put_back` puts back each change listed whose record is still there, the last first, before the core.

- **A second agent of one harness ended another agent's change half made** (U1, U3; issue #180, fixed on
  2026-11-25, [D-154](../../docs/decisions.md#d-154-an-install-takes-over-a-killed-upgrade-only-for-the-same-agent-and-the-same-gateway)).
  Hermes agent a's install killed once it had put our new plugin into a's Hermes, then agent b's install, which
  replaces the same parts, took the upgrade over and ended it with its own plugin change only: a's Hermes kept
  the plugin of a run that never ended, and a's record of the change stayed, to put older copies back over a
  later install. Review found it; the model, with one agent, could not, and with two (`install-agents`) finds it
  as `TakesOverAnother`. Now `upgrade_begin` takes an upgrade over only when each plugin change it lists that is
  not done is this agent's, to this run's gateway (`changes_ours`), and puts it back otherwise; and a run that
  fails puts its own change back in the same replay as the others, the last first, which matters when two
  changes are to one gateway (above, "What is not"). `install/test_install.py` `TwoAgents` shows each first.

The test of the image also needed the docker stand-in to keep an image no tag names, as docker does: before, the
put back of the tag failed in the stand-in, which hid the TERM case. The sweeps of Pi's and Hermes's upgrades
also look at the core since #197, and after a run that fails accept only the older install: a put back that left
the newer core, or a failed run that ended the upgrade instead, passed them before.

## Running

```sh
python3 tools/tla/check.py --model InstallUpgrade
tools/tla/run.sh formal/tla InstallUpgrade.tla InstallUpgrade.pi.cfg
```

CI runs this model's cases with `ActivityHoldBack`'s in the `TLA+ models (InstallUpgrade and ActivityHoldBack)`
job, in the merge queue and on `master` always, and on a pull request when it changes `install/`, or any other
code a model follows, `formal/`, `tools/tla/` or `.github/workflows/ci.yml`.
