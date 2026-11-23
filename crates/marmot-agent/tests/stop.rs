//! The core program stops when its service manager stops it, even while its harness is still
//! starting: a stop during start-up is not left waiting for the harness to answer. A core stopped
//! after a message arrives hears the chat again when it is started on the same home; stopped just
//! after one, or just after one, does too, since MDK's fix for its #3602.

mod common;

use common::{relay, within};
use marmot_account::AccountHome;
use marmot_agent::PubKey;
use marmot_agent::config::Config;
use marmot_agent::marmot::{self, Marmot};
use marmot_agent::socket::{Endpoint, PROTOCOL};
use marmot_app::{AccountSetupRequest, MarmotApp, MarmotAppConfig, MarmotAppRuntime};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_start_up_ends_the_core() {
    storage_sqlite::relax_storage_for_test_process();
    marmot_agent::marmot::start_sqlite();
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

/// The core's program, `marmot-agent run`, on the settings in `dir`, driving a connector on its
/// socket.
fn start_core(dir: &Path) -> tokio::process::Child {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .arg("run")
        .arg(dir.join("settings.toml"))
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

/// The prompts a connector was given, in order.
type Prompts = Arc<Mutex<Vec<String>>>;

/// A scripted connector on the socket of the core whose home is `home`, once the core listens
/// there, which it does once MDK has started. It answers every request and notes each prompt's
/// text, and emits nothing; it ends when the core closes the connection.
async fn connect(home: &Path, prompts: Prompts) -> JoinHandle<()> {
    let endpoint = Endpoint::in_home(home);
    let stream = within(60, || {
        std::os::unix::net::UnixStream::connect(&endpoint.socket).ok()
    })
    .await;
    stream.set_nonblocking(true).unwrap();
    let token = std::fs::read_to_string(&endpoint.token_file).unwrap();
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {
        "protocol": PROTOCOL,
        "connector": {"name": "script", "version": "0.0.1"},
        "harness": {"name": "script", "version": "1"},
        "token": token.trim(),
        "capabilities": {
            "interrupt": false, "mid_turn": "queue", "sessions": "core", "resume": false,
            "approvals": false, "files_in": "none", "approval_commands": [],
        },
    }});
    let (read, mut write) = UnixStream::from_std(stream).unwrap().into_split();
    write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .unwrap();
    let mut lines = BufReader::new(read).lines();
    let welcome: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert!(
        welcome.get("result").is_some(),
        "the core takes the connector"
    );
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            let answer = json!({"jsonrpc": "2.0", "id": request["id"], "result": {}});
            if write
                .write_all(format!("{answer}\n").as_bytes())
                .await
                .is_err()
            {
                return;
            }
            if request["method"] == "prompt" {
                let text = request["params"]["text"].as_str().unwrap_or_default();
                prompts.lock().unwrap().push(text.to_string());
            }
        }
    })
}

/// A person writes in a chat with the agent; the core's program is stopped `pause` after its
/// harness has the message, and started again on the same home; the person writes again.
async fn restarted_after(pause: Duration) {
    storage_sqlite::relax_storage_for_test_process();
    marmot_agent::marmot::start_sqlite();
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

    // The agent, made as `marmot-agent init` makes it, on a home of its own, for a connector on
    // its socket.
    let home = dir.join("agent");
    std::fs::write(
        dir.join("settings.toml"),
        format!(
            "home = {home:?}\nrelays = [{url:?}]\nowner = \"{person}\"\nwhite_noise = false\n\
             allow_loopback_relays = true\n\n[connector]\n",
            home = home.display().to_string(),
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
    let prompts = Prompts::default();
    let core = start_core(dir);
    let connector = connect(&home, prompts.clone()).await;
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
    within(60, || (*prompts.lock().unwrap() == ["first"]).then_some(())).await;
    tokio::time::sleep(pause).await;
    stop_core(core).await;
    connector.await.unwrap();

    // Started again on the same home, it hears the person's next message.
    let core = start_core(dir);
    let connector = connect(&home, prompts.clone()).await;
    person_runtime
        .send_message(&person, &group, b"second".to_vec())
        .await
        .unwrap();
    let second = |p: &[String]| p.iter().any(|p| p == "second");
    let mut heard = false;
    for _ in 0..600 {
        heard = second(&prompts.lock().unwrap());
        if heard {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop_core(core).await;
    connector.await.unwrap();
    person_runtime.shutdown().await;
    assert!(
        heard,
        "the harness never had \"second\" within 60 seconds; the agent's home stored {:?}",
        stored(dir, &group_hex)
    );
    // Anything handed over twice would have come by the time the core stopped.
    assert_eq!(
        *prompts.lock().unwrap(),
        ["first", "second"],
        "each message once"
    );
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
    let page = marmot.stored(&group, 0, 1000).unwrap();
    page.messages.into_iter().map(|m| m.content).collect()
}

// MDK's #3602: a home stopped just after a message arrives was deaf to that chat for the next
// program that opened it, as seen with `wn-agent`. The core, stopped by SIGTERM the moment its
// harness has a message, must hear the next one after it starts again. Up to MDK 0.19.0 it did
// not, in a third to a half of the runs (5 of 12 on MDK 0.19.0): the restarted home never stored
// the person's next message. MDK's fix (#3608) is in the revision the core uses since 2026-11-22,
// and the test passed 15 runs of 15 on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_core_stopped_just_after_a_message_hears_the_next() {
    restarted_after(Duration::ZERO).await;
}

// As above, stopped five seconds after the message, which `wn-agent` survived. Up to MDK 0.19.0
// the core missed the next message on CI twice all the same (#96); on MDK's fix it passed 15 runs
// of 15.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_core_stopped_a_while_after_a_message_hears_the_next() {
    restarted_after(Duration::from_secs(5)).await;
}
