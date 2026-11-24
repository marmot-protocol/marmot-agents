# InstallUpgrade: a TLA+ model of an install's upgrade

Status: built and checked with TLC on small bounds, as the verification page plans for the parts of the system
that face crashes and reordering (see [the inventory](inventory.md)). The model, its bounds and its switches are
internal and change as the work goes. Read on 2026-11-24 against master `7118005`, on branch
`t3/survey-and-model-state-machines`, which also fixes the two findings below.

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
none); the image the tag `marmot-agents-pi` names, and `marmot-agents-pi:staging`; and our plugin in the person's
Hermes, a copy of the `hermes` part. Goose's image is handled by the same functions as Pi's, so `pi` stands for
both; OpenClaw's plugin change is the same as Hermes's, so `hermes` stands for both (inferred from the code: both
call `image_commit` and `change_begin` alike).

| Action | In the code |
| --- | --- |
| `Start(s, h)` | The person runs `install/<h>.sh`: one of the first runs, or one after a run left something under way |
| `Lock(s)` | `lock_installs` in `install_core`: `flock` on `install.lock`, waiting while another run holds it; then `install_core` looks for `$MA_UPGRADE` |
| `Restore(s)` | `change_restore` (since the fix), at the top of `upgrade_begin` and in `finish`: a plugin change moved into the record as the upgrade ended goes back to its place |
| `Begin(s)` | `upgrade_begin`: takes over a record whose list names only parts this run replaces, adding its own (`upgrade_list`); puts back one it cannot finish (`upgrade_put_back other`); or makes a record (`.upgrade.new`, renamed into place whole) |
| `Prepare(s)` | `harness_prepare`: Pi's `docker build -t marmot-agents-pi:staging`; `MA_PREPARED` is set |
| `Publish(s)`, `Core(s)` | `publish_support`: `upgrade_begin`, then `install` and `mv -f` of the new core |
| `Keep(s)`, `Put(s)` | `upgrade_keep` for each part (the part there before moved into the record, or what a killed run put there removed), then `mv` of the staged part into `$MA_LIB` |
| `Connect(s)`, `Replace(s)` | `change_begin` (a copy of the plugin and the record naming it, or the record a killed run left, which this run finishes), then `replace_dir` into the person's Hermes |
| `StartServices(s)` | `start_services`: where an install most often fails (`systemctl`) |
| `Commit(s)`, `Tag(s)` | `image_commit`: since the fix, the image the tag names written into the record (`$MA_UPGRADE/image`) unless a killed run wrote it; then `docker tag` and `docker rmi` of the staging tag |
| `End(s)`, `Retire(s)` | `upgrade_end`: since the fix, the plugin change's record moved into the upgrade's (`plugin-change`, with `plugin-change.path`); then `upgrade_retire`, one `mv` that ends both |
| `ChangeEnd(s)` | `change_end` and the rest of `main`: the copies of our old plugin dropped (before the fix, the change's record removed here); the run ends |
| `Fail(s)` | A command fails, or TERM, INT or HUP arrives: the trap runs `finish`, with those signals ignored |
| `FinishChange(s)`, `FinishChangeEnd(s)` | `finish` → `change_put_back`: the plugin's copy put back, the record removed |
| `FinishUpgrade(s)` | `finish` → `upgrade_put_back`, given `MA_UPGRADING` |
| `PutBackCore`, `PutBackPart`, `PutBackMove`, `PutBackImage`, `PutBackRetire` | `upgrade_put_back`, from `finish` or from `upgrade_begin`: the core from its copy; each listed part (a kept one by `rm` then `mv`, an absent one removed, one never moved left); since the fix, the image tag from the record (`image_put_back_from`); then `upgrade_retire` |
| `FinishImage(s)` | `finish` → `harness_put_back` → `image_put_back`: before the fix, the tag put back on the image this run found, given `IMAGE_TAGGED`; since, only the staging tag taken away. The run ends |
| `Kill(s)` | SIGKILL: the run ends where it is; the system lets the lock go |

## What is not

- **A command that fails while putting back.** `upgrade_put_back` and `change_put_back` go on and say what they
  could not put back, and where the copies are; the model's put backs never fail. Neither does a `docker tag` or
  `docker rmi` (both may be refused when docker is gone).
- **The two renames of `replace_dir` without `mv --exchange`** (macOS): a kill between them leaves the person's
  gateway with neither plugin, which `docs/move.md` states. The model has the exchange, one step.
- **What the agent's own folder holds** (settings, launchers, services, `before-move`): written by one agent's
  install only, and put back by none.
- **The old connector's plugin replaced on a move** (`--replace-plugin`): its put back is the same as our own
  plugin's, and the old connector speaks to MDK's connector, not to this core, so no mismatch follows from it.
  `install/test_install.py` `Interrupted` stops such a move at every step.
- **Dry runs**, `--uninstall` (which gives a left plugin change up), and the stale folders a later run removes
  (`lib.new.*`, `.upgrade.done.*`).
- **A run's own files staged in `lib.new.$$`**, which no other run reads.
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
| U1 | Once no install is under way and none left an upgrade or a plugin change under way, the core is of the release of the files every install replaces, Pi's image tag names the image Pi's scripts were made for, and our plugin in the person's Hermes is the one the Hermes files hold: never the new core with the old files, or the other way round, wherever a run was killed or stopped | invariant `U1` |
| U2 | One install at a time is past the lock | invariant `U2` |
| L1 | What a run left under way is finished or put back, once the person runs installers again | `<>[]Quiescent` |

U1 says what [D-134](../../docs/decisions.md#d-134-an-upgrade-of-the-core-and-its-support-files-is-put-back-on-failure-and-finished-by-a-re-run-after-a-kill)
promises. It allows another harness's own folder to stay as it was when one harness upgrades the core: an install
replaces only its harness's parts, by design. While something is left under way, anything may be mixed; the next
run of that harness finishes it or puts it back, and L1 says it does.

The liveness property holds under strong fairness for each slot's steps and, with `FairPerson`, for each
harness's `Start`. Kills and failures are bounded, so they end.

## Configurations and results

Checked on 2026-11-24 with TLC v1.7.4 (Java 21, in the pinned image) by `tools/tla/check.py`, as the other
models are: the cases that must pass on 4 workers, those that must fail on one. Every configuration has an older
install to upgrade but the first, 2 kills and 2 runs that fail or are stopped, unless it says otherwise.

| Case | Configuration | Expected | Distinct states | Counterexample |
| --- | --- | --- | ---: | --- |
| `install-pi` | `InstallUpgrade.pi.cfg` (Pi in a container; a run after a kill may be Codex's) | every property holds | 8,228 | |
| `install-hermes` | `InstallUpgrade.hermes.cfg` (Hermes with our plugin; then maybe Codex) | every property holds | 11,730 | |
| `install-lock` | `InstallUpgrade.lock.cfg` (two Claude Code installs at once; then maybe Codex) | every property holds | 11,455 | |
| `install-first` | `InstallUpgrade.first.cfg` (a first install of Pi, nothing there before) | every property holds | 5,076 | |
| `install-all` | `InstallUpgrade.all.cfg` (an older install of each harness, a Hermes install, then any) | every property holds | 92,983 | |
| `install-unfair` | `InstallUpgrade.unfair.cfg` (no `FairPerson`, 1 kill, no failure) | L1 fails | 120 | |
| `install-bug-ImageOutsideRecord` | Pi's, the code before the fix | U1 fails | 1,734 | [`install-image-put-back-apart`](counterexamples/install-image-put-back-apart.json) |
| `install-bug-ChangeEndsApart` | Hermes's, the code before the fix | U1 fails | 1,929 | [`install-plugin-change-ends-apart`](counterexamples/install-plugin-change-ends-apart.json) |

Every case of this model together takes under a minute here (`install-all` about 25 s), so all of it runs on
every pull request that changes `install/`.

**Each check catches its bug.** On the configuration named, with U1 only:

| `Bug` | What it breaks | Configuration | Distinct states to the violation |
| --- | --- | --- | ---: |
| `ImageOutsideRecord` | the tag put back from what the run noted, apart from the record: the code before the fix | Pi's | 1,734 |
| `ChangeEndsApart` | the plugin change ended after the upgrade, apart: the code before the fix | Hermes's | 1,929 |
| `NoLock` | no lock: a second install takes over or puts back an upgrade the first is still making | two installs | 13,573 |
| `FreshRecord` | a run after a kill makes a record of its own over the one left, which holds the new core | Pi's | 2,590 |
| `KeepOverwrites` | `upgrade_keep` moves what is in place into the record even when the record holds the part there before | Pi's | 6,621 |
| `RecordAfterCore` | the new core is put in before the record is made | Pi's | 15 |

**No empty passes.** Each probe must be found false: `NeverTakenOver` (a run takes over an upgrade a killed run
left), `NeverPutBackOther` (Codex's install puts back Pi's), `NeverImagePutBack` (the tag is put back from the
record), `NeverUpgraded` (an upgrade ends with everything new), `NeverRestored` (a run killed once it put in the
new core and files has them put back by the next), on Pi's configuration; `NeverChangeFinished` (a run finishes a
plugin change a killed run left), on Hermes's; and `NeverWaited` (a second install waits for the lock), on two
installs. Each is reached within 3,800 distinct states.

## What the model found

Two windows in which an upgrade's parts end up apart, each reached by a TERM as well as by a kill. **Both are
fixed** (2026-11-24, this branch), each shown first by a test of `install/test_install.py` that stops an install
at every step (`test_every_step_keeps_the_image_with_its_scripts`, `test_every_step_keeps_the_hermes_plugin_with_its_core`),
and the model follows the fix:

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

The test of the image also needed the docker stand-in to keep an image no tag names, as docker does: before, the
put back of the tag failed in the stand-in, which hid the TERM case.

## Running

```sh
python3 tools/tla/check.py --model InstallUpgrade
tools/tla/run.sh formal/tla InstallUpgrade.tla InstallUpgrade.pi.cfg
```

CI runs this model's cases with `ActivityHoldBack`'s in the `TLA+ models (InstallUpgrade and ActivityHoldBack)`
job, in the merge queue and on `master` always, and on a pull request when it changes `install/`, or any other
code a model follows, `formal/`, `tools/tla/` or `.github/workflows/ci.yml`.
