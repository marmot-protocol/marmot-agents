//! The core program stops when its service manager stops it, even while its harness is still
//! starting: a stop during start-up is not left waiting for the harness to answer.

use marmot_app::{AccountSetupRequest, MarmotApp, MarmotAppConfig, MarmotAppRuntime};
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
