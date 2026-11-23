//! Taking over one of today's connector installs (D-089). Today's connector itself, MDK's
//! `agent-connector` that is `wn-agent`, makes the home and joins a person's chat there, and the
//! files its installer writes are put around it. The import backs the home up and writes the settings, and the core then
//! runs on the home in place: the same identity, in the same chat, without being invited again.

#[path = "../../marmot-agent/tests/common/mod.rs"]
mod common;
mod old_connector;

use common::{Echo, eventually, within};
use marmot_account::AccountHome;
use marmot_agent::PubKey;
use marmot_agent::config::Config;
use marmot_agent::marmot::{self, Marmot};
use marmot_agent::rights::Who;
use marmot_agent::run::{Core, Store};
use marmot_agent_import::{self as import, Connector, Options};
use old_connector::{Invited, Noted, chat_messages, invited, places};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

/// What the Codex installer writes around the home besides what `wn-agent` keeps in it: the
/// harness's settings, and the two systemd user units.
fn installed_codex(root: &Path, home: &Path, agent: &str, person: &str, relay: &str) {
    let h = home.display();
    std::fs::write(
        home.join("dev/wn-codex.env"),
        format!(
            "MARMOT_HOME={h}\nMARMOT_AGENT_SOCKET={h}/dev/wn-agent.sock\n\
             WN_CODEX_ACCOUNT_ID_HEX={agent}\nWN_CODEX_ALLOWED_SENDERS_HEX={person}\n\
             WN_CODEX_BIN=codex\nMARMOT_HARNESS_EXECUTION_PROFILE=inherit\n\
             WN_CODEX_TIMEOUT_SECS=3600\n"
        ),
    )
    .unwrap();
    let units = root.join(".config/systemd/user");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::write(
        units.join("wn-agent-codex.service"),
        format!(
            "[Unit]\nDescription=Marmot wn-agent connector\n\n[Service]\n\
             ExecStart=\"/x/bin/wn-agent\" \"--home\" \"{h}\" \"--socket\" \"{h}/dev/wn-agent.sock\" \
             \"--relay\" \"{relay}\" \"--allow-loopback-relays\"\nRestart=always\n\n\
             [Install]\nWantedBy=default.target\n"
        ),
    )
    .unwrap();
    std::fs::write(
        units.join("wn-codex.service"),
        format!(
            "[Unit]\nRequires=wn-agent-codex.service\n\n[Service]\n\
             Environment=\"MARMOT_HOME={h}\"\n\
             Environment=\"WN_CODEX_ALLOWED_SENDERS_HEX={person}\"\n\
             Environment=\"WN_CODEX_TIMEOUT_SECS=3600\"\n\
             ExecStart=\"/x/bin/wn-codex\"\n"
        ),
    )
    .unwrap();
}

/// Today's connector, as its own program: run by `old_connector::invited`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run by the import test, as the old connector"]
async fn old_connector() {
    old_connector::serve().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_imported_codex_install_keeps_its_identity_and_its_chat() {
    imported(false).await;
}

// As above, and then the person writes in the chat again, and the agent answers. Up to MDK
// 0.19.0, MDK delivered nothing sent to that chat once the core had started on the home (7 runs
// of 7, waiting up to 300 s), for MDK #3602. On MDK's fix (#3608), in the revision the core uses
// since 2026-11-22, it passed 15 runs of 15.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_imported_codex_install_answers_in_its_chat() {
    imported(true).await;
}

/// Imports a Codex install of the old connector and runs the core on its home. With `answers`,
/// the person writes in the chat after the move, and the agent answers.
async fn imported(answers: bool) {
    let Invited {
        root,
        home: old,
        url,
        person_runtime,
        person,
        agent,
        group,
        group_hex,
        relay: _relay,
    } = invited(".marmot-agents/codex", "codex-harness-agent").await;
    installed_codex(root.path(), &old, &agent, &person, &url);

    // The plan reads the install and changes nothing.
    let work = root.path().join("work");
    let options = Options {
        home: old.clone(),
        harness: None,
        owner: None,
        cwd: Some(work.to_string_lossy().into_owned()),
        settings: None,
        leave_running: Vec::new(),
        places: places(root.path()),
        now: std::time::SystemTime::now(),
    };
    let plan = import::plan(&options, &mut Noted::default()).unwrap();
    assert_eq!(plan.connector, Connector::Codex);
    assert_eq!(plan.owner.as_str(), person, "the one person allowed");
    assert_eq!(plan.prompt, Who::Owner, "the owner was the only sender");
    assert!(plan.invite.is_empty());
    assert!(!plan.backup.exists() && !plan.settings.exists());
    let mut printed = Vec::new();
    plan.print(&mut printed).unwrap();
    let printed = String::from_utf8(printed).unwrap();
    assert!(
        !printed.contains(&person) && !printed.contains(&url),
        "keys and relays are counted, not shown"
    );
    assert!(printed.contains("time limits"), "what did not map is said");

    // Carried out: the services stopped, harness first, the home backed up, the settings written.
    let mut services = Noted::default();
    let mut said = Vec::new();
    import::carry_out(&options, &plan, &mut services, &mut said).unwrap();
    assert_eq!(services.0, ["wn-codex.service", "wn-agent-codex.service"]);
    let units = root.path().join(".config/systemd/user");
    assert!(units.join("wn-agent-codex.service").exists(), "kept");
    // The backup was checked against the home before the settings were written beside it.
    let said = String::from_utf8(said).unwrap();
    assert!(said.contains("checked against the home"), "{said}");
    assert!(!plan.backup.join("marmot-agent.toml").exists());
    let accounts = AccountHome::open(&plan.backup).accounts().unwrap();
    assert_eq!(accounts[0].account_id_hex, agent);
    let mode = std::fs::metadata(&plan.backup)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    let mode = std::fs::metadata(&plan.settings)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let mut config = Config::read(&plan.settings).unwrap();
    assert_eq!(config.home, old, "the home, in place");
    assert_eq!(config.relays, std::slice::from_ref(&url), "the old relays");
    assert!(config.allow_loopback_relays);
    assert_eq!(config.owner.as_str(), person);
    let codex = config.codex.as_ref().expect("a Codex section");
    assert_eq!(codex.cwd, work.to_string_lossy());
    assert_eq!(codex.program, "codex");
    assert!(
        config.file_folders().is_empty(),
        "the old connector sent no files"
    );
    // A second import of the same home is refused: its settings are there.
    assert!(import::plan(&options, &mut Noted::default()).is_err());

    // The core runs on the home in place, as the same identity, already in the chat. White
    // Noise's own relays are out of reach in a test.
    config.white_noise = false;
    let (home, app, runtime) = marmot::open(&config).unwrap();
    let account = marmot::identity(&home).unwrap();
    assert_eq!(account.account_id_hex, agent, "the same identity");
    let live = marmot::subscribe(&runtime);
    let since = live.since;
    runtime.start().await.unwrap();
    let marmot = Marmot {
        runtime: runtime.clone(),
        app,
        label: account.label,
        agent: PubKey(account.account_id_hex),
        faults: Default::default(),
    };
    assert!(
        marmot
            .groups()
            .unwrap()
            .iter()
            .any(|g| g.as_str() == group_hex),
        "still in the chat"
    );
    let prompts = Arc::new(AtomicUsize::new(0));
    let core = Core::new(
        marmot,
        Echo::new(prompts.clone()),
        &config,
        Store::new(&config.home),
    )
    .unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(core.run(live, async {
        let _ = stopped.await;
    }));

    // On its first start the core reads nothing already stored as a prompt: it marks where
    // each chat stands. Once it has, nobody invites it again: the person writes in the same
    // chat, and the agent answers.
    let state = config.home.join("marmot-agent-state.json");
    eventually(|| {
        let kept = std::fs::read_to_string(&state).ok()?;
        kept.contains(&group_hex).then_some(())
    })
    .await;
    if answers {
        // MDK gives the time a message arrived in whole seconds: one that arrived in the second
        // the core subscribed in is history.
        within(5, || (marmot::now() > since).then_some(())).await;
        person_runtime
            .send_message(&person, &group, b"after the move".to_vec())
            .await
            .unwrap();
        // MDK can take some seconds to settle a chat after a program first opens it.
        within(90, || {
            let said = chat_messages(&person_runtime, &person, &group_hex);
            said.iter()
                .any(|m| m.sender == agent && m.plaintext == "echo: after the move")
                .then_some(())
        })
        .await;
    } else {
        // MDK can take some seconds to settle a chat after a program first opens it, and
        // announces what it fetched then: history announced late would be read by now.
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    let said = chat_messages(&person_runtime, &person, &group_hex);
    assert!(
        !said.iter().any(|m| m.plaintext == "echo: before the move"),
        "what the old connector read is not a prompt"
    );
    assert_eq!(prompts.load(Ordering::SeqCst), usize::from(answers));

    let _ = stop.send(());
    task.await.unwrap().unwrap();
    runtime.shutdown().await;
    person_runtime.shutdown().await;
}

/// An option where a value belongs is refused before anything is read or changed: once,
/// `--settings --dry-run` wrote the settings to a file called `--dry-run` and carried the import
/// out.
#[test]
fn an_option_given_as_a_value_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("codex");
    std::fs::create_dir_all(&home).unwrap();
    let owner = AccountHome::open(root.path().join("person"))
        .create_account("person")
        .unwrap()
        .account_id_hex;
    AccountHome::open(&home)
        .create_account("codex-harness-agent")
        .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .args(["import", &home.to_string_lossy(), "--owner", &owner])
        .args(["--settings", "--dry-run"])
        .env("HOME", root.path())
        .env_remove("XDG_CONFIG_HOME")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("`--settings` needs a value"), "{said}");
    let left: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let mut left = left;
    left.sort();
    assert_eq!(left, ["codex", "person"], "no backup and no settings");
}
