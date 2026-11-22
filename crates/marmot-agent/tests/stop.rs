//! The core program stops when its service manager stops it, even while its harness is still
//! starting: a stop during start-up is not left waiting for the harness to answer. A core stopped
//! after a message arrives hears the chat again when it is started on the same home; stopped just
//! after one, it does not yet, for MDK's #3602.

mod common;

use common::{relay, within};
use marmot_account::AccountHome;
use marmot_agent::PubKey;
use marmot_agent::config::Config;
use marmot_agent::marmot::{self, Marmot};
use marmot_agent::run::{Core, Store};
use marmot_agent_driver::{
    Answer, Capabilities, Control, Driver, FilesIn, Harness, MidTurn, Output, Prompt, Refusal,
    Sessions,
};
use marmot_app::{AccountSetupRequest, MarmotApp, MarmotAppConfig, MarmotAppRuntime};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_start_up_ends_the_core() {
    storage_sqlite::relax_storage_for_test_process();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let relay = nostr_relay_builder::MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    // The agent's identity, made as `marmot-agent init` makes it.
    let settings = MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true);
    let app = MarmotApp::with_relay_and_config(&home, url.clone(), settings);
    let runtime = MarmotAppRuntime::new(app);
    let endpoint = cgka_traits::TransportEndpoint(url.clone());
    let setup = AccountSetupRequest {
        default_relays: vec![endpoint.clone()],
        bootstrap_relays: vec![endpoint],
        publish_initial_key_package: true,
        ..AccountSetupRequest::default()
    };
    runtime
        .create_identity(setup.relay_options_only())
        .await
        .unwrap();
    runtime.shutdown().await;

    // An ACP agent that never answers its greeting: it says it has started, then waits.
    let started = dir.path().join("started");
    let agent = dir.path().join("agent.sh");
    std::fs::write(&agent, "#!/bin/sh\ntouch \"$1\"\nexec sleep 300\n").unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let config = dir.path().join("settings.toml");
    std::fs::write(
        &config,
        format!(
            "home = {home:?}\nrelays = [{url:?}]\nowner = \"{}\"\nwhite_noise = false\n\
             allow_loopback_relays = true\n\n[acp]\nname = \"silent\"\n\
             command = [\"sh\", {agent:?}, {started:?}]\ncwd = {work:?}\n",
            "ab".repeat(32),
            home = home.display().to_string(),
            agent = agent.display().to_string(),
            started = started.display().to_string(),
            work = work.display().to_string(),
        ),
    )
    .unwrap();

    let mut core = tokio::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .arg("run")
        .arg(&config)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !started.exists() {
        assert!(
            Instant::now() < deadline,
            "the core never started its harness"
        );
        if let Some(status) = core.try_wait().unwrap() {
            panic!("the core ended before starting its harness: {status}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let pid = core.id().unwrap().to_string();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(killed.success());
    let status = tokio::time::timeout(Duration::from_secs(10), core.wait())
        .await
        .expect("the core went on waiting for its harness after it was stopped")
        .unwrap();
    assert!(status.success(), "the core stops cleanly: {status}");
}

/// A harness that writes each prompt's text to a file, one a line, and emits nothing.
struct Noting(PathBuf);

impl Driver for Noting {
    fn harness(&self) -> Harness {
        Harness {
            name: "noting".into(),
            version: None,
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: false,
            mid_turn: MidTurn::Queue,
            sessions: Sessions::Core,
            resume: false,
            approvals: false,
            files_in: FilesIn::None,
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.0)
            .unwrap();
        writeln!(file, "{}", prompt.text).unwrap();
        Ok(())
    }

    async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
        Ok(())
    }

    async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
        Ok(())
    }

    async fn next_event(&mut self) -> Option<Output> {
        std::future::pending().await
    }
}

/// The core as its own program, as `marmot-agent run` runs it, with [`Noting`] for its harness:
/// on its home, until SIGTERM, then MDK shut down. It notes in `started` that MDK has started.
/// The restart test runs this test binary again with this test alone and the settings in its
/// environment. It does nothing when run on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run by the restart test, as the core's program"]
async fn core_program() {
    let Some(settings) = std::env::var_os("CORE_PROGRAM_SETTINGS") else {
        return;
    };
    let dir = Path::new(&settings).parent().unwrap().to_path_buf();
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    let config = Config::read(Path::new(&settings)).unwrap();
    let (home, app, runtime) = marmot::open(&config).unwrap();
    let account = marmot::identity(&home).unwrap();
    let events = runtime.subscribe();
    runtime.start().await.unwrap();
    std::fs::write(dir.join("started"), "").unwrap();
    let marmot = Marmot {
        runtime: runtime.clone(),
        app,
        label: account.label,
        agent: PubKey(account.account_id_hex),
        faults: Default::default(),
    };
    let driver = Noting(dir.join("prompts"));
    let core = Core::new(marmot, driver, &config, Store::new(&config.home)).unwrap();
    core.run(events, async move {
        term.recv().await;
    })
    .await
    .unwrap();
    runtime.shutdown().await;
}

/// The core's program on the settings in `dir`, started once `started` is gone.
fn start_core(dir: &Path) -> tokio::process::Child {
    let _ = std::fs::remove_file(dir.join("started"));
    tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "core_program", "--ignored", "--nocapture"])
        .env("CORE_PROGRAM_SETTINGS", dir.join("settings.toml"))
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

/// Stops the core's program as systemd stops a service, and waits for it to end cleanly.
async fn stop_core(mut core: tokio::process::Child) {
    let pid = core.id().unwrap().to_string();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(killed.success());
    let status = tokio::time::timeout(Duration::from_secs(30), core.wait())
        .await
        .expect("the core stops when asked")
        .unwrap();
    assert!(status.success(), "the core stops cleanly: {status}");
}

fn prompts(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("prompts")).unwrap_or_default();
    text.lines().map(str::to_string).collect()
}

/// A person writes in a chat with the agent; the core's program is stopped `pause` after its
/// harness has the message, and started again on the same home; the person writes again.
async fn restarted_after(pause: Duration) {
    storage_sqlite::relax_storage_for_test_process();
    let relay = relay().await;
    let url = relay.url().await.to_string();
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();

    // The person, in a home of their own.
    let person_home = dir.join("person");
    let person_app = MarmotApp::try_with_relays_and_account_home_and_config(
        &person_home,
        vec![url.clone()],
        AccountHome::open(&person_home),
        MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true),
    )
    .unwrap();
    let person_runtime = MarmotAppRuntime::new(person_app);
    let endpoint = cgka_traits::TransportEndpoint(url.clone());
    let setup = AccountSetupRequest {
        default_relays: vec![endpoint.clone()],
        bootstrap_relays: vec![endpoint],
        publish_initial_key_package: true,
        ..AccountSetupRequest::default()
    };
    let person = person_runtime
        .create_identity(setup)
        .await
        .unwrap()
        .account
        .account_id_hex;
    person_runtime.start().await.unwrap();

    // The agent, made as `marmot-agent init` makes it, on a home of its own.
    let home = dir.join("agent");
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        dir.join("settings.toml"),
        format!(
            "home = {home:?}\nrelays = [{url:?}]\nowner = \"{person}\"\nwhite_noise = false\n\
             allow_loopback_relays = true\n\n[codex]\ncwd = {work:?}\n",
            home = home.display().to_string(),
            work = work.display().to_string(),
        ),
    )
    .unwrap();
    let init = std::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .arg("init")
        .arg(dir.join("settings.toml"))
        .output()
        .unwrap();
    assert!(init.status.success(), "the agent's identity is made");
    let agent = String::from_utf8(init.stdout).unwrap().trim().to_string();

    // The core runs; the person starts a chat with the agent, and it joins and marks where the
    // chat stands.
    let core = start_core(dir);
    within(60, || dir.join("started").exists().then_some(())).await;
    let group = person_runtime
        .start_direct_conversation(&person, &agent)
        .await
        .unwrap();
    let group_hex = hex::encode(group.as_slice());
    let state = home.join("marmot-agent-state.json");
    within(60, || {
        let kept = std::fs::read_to_string(&state).ok()?;
        kept.contains(&group_hex).then_some(())
    })
    .await;

    // The person writes; the moment the harness has it, the core is stopped as a service is.
    person_runtime
        .send_message(&person, &group, b"first".to_vec())
        .await
        .unwrap();
    within(60, || (prompts(dir) == ["first"]).then_some(())).await;
    tokio::time::sleep(pause).await;
    stop_core(core).await;

    // Started again on the same home, it hears the person's next message.
    let core = start_core(dir);
    within(60, || dir.join("started").exists().then_some(())).await;
    person_runtime
        .send_message(&person, &group, b"second".to_vec())
        .await
        .unwrap();
    let mut heard = None;
    for _ in 0..600 {
        let seen = prompts(dir);
        if seen.iter().any(|p| p == "second") {
            heard = Some(seen);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop_core(core).await;
    person_runtime.shutdown().await;
    let Some(heard) = heard else {
        panic!(
            "the harness never had \"second\" within 60 seconds; the agent's home stored {:?}",
            stored(dir, &group_hex)
        );
    };
    assert_eq!(heard, ["first", "second"], "each message once");
}

/// The texts of the messages the agent's home stored in a group, opened once the core is stopped,
/// without starting MDK, which would fetch what it missed: where a message the harness never had
/// was lost.
fn stored(dir: &Path, group_hex: &str) -> Vec<String> {
    let config = Config::read(&dir.join("settings.toml")).unwrap();
    let (home, app, runtime) = marmot::open(&config).unwrap();
    let account = marmot::identity(&home).unwrap();
    let marmot = Marmot {
        runtime,
        app,
        label: account.label,
        agent: PubKey(account.account_id_hex),
        faults: Default::default(),
    };
    let group = marmot_agent::GroupKey(group_hex.to_string());
    let messages = marmot.stored(&group, 0).unwrap();
    messages.into_iter().map(|m| m.content).collect()
}

// MDK's #3602: a home stopped just after a message arrives was deaf to that chat for the next
// program that opened it, as seen with `wn-agent`. The core, stopped by SIGTERM the moment its
// harness has a message, must hear the next one after it starts again. It does not, in about
// half the runs (6 of 10, then 2 of 4, with MDK at the revision the core uses): the restarted
// home never stores the person's next message, so it never reaches the core.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "MDK #3602: a home stopped just after a message is deaf to that chat after a restart, in about half the runs"]
async fn a_core_stopped_just_after_a_message_hears_the_next() {
    restarted_after(Duration::ZERO).await;
}

// As above, stopped five seconds after the message, which `wn-agent` survived, and the core does
// too (10 runs of 10).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_core_stopped_a_while_after_a_message_hears_the_next() {
    restarted_after(Duration::from_secs(5)).await;
}
