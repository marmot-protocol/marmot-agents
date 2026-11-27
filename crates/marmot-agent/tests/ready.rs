//! The core program says it is ready, to systemd (`NOTIFY_SOCKET`) and in the file launchd's
//! launcher names (`MARMOT_AGENT_READY_FILE`), once it has opened the agent's store and its harness
//! has answered its start, and not before (D-178). The installer counts a start as done only then,
//! so a core that fails after it opened the store is put back.

mod common;

use common::relay;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::net::UnixDatagram;

/// An agent made with `marmot-agent init`, on MDK's test relay, driving the ACP agent `command`.
/// Returns its settings and its home.
fn agent(dir: &Path, url: &str, command: &[String]) -> (PathBuf, PathBuf) {
    let home = dir.join("home");
    let work = dir.join("work");
    std::fs::create_dir(&work).unwrap();
    let settings = dir.join("settings.toml");
    std::fs::write(
        &settings,
        format!(
            "home = {home:?}\nrelays = [{url:?}]\nowner = \"{}\"\nwhite_noise = false\n\
             allow_loopback_relays = true\n\n[acp]\nname = \"stand-in\"\ncommand = {command:?}\n\
             cwd = {work:?}\n",
            "ab".repeat(32),
            home = home.display().to_string(),
            work = work.display().to_string(),
        ),
    )
    .unwrap();
    let init = std::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .arg("init")
        .arg(&settings)
        .output()
        .unwrap();
    assert!(init.status.success(), "the agent's identity is made");
    (settings, home)
}

/// `marmot-agent run`, as its service runs it, with a socket to say it is ready on and a file.
fn start(settings: &Path, notify: &Path, file: &Path) -> tokio::process::Child {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .arg("run")
        .arg(settings)
        .env("NOTIFY_SOCKET", notify)
        .env("MARMOT_AGENT_READY_FILE", file)
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

/// Whether something holds MDK's lock on the home, as a core does while it has the store open.
fn store_open(home: &Path) -> bool {
    let lock = std::fs::File::open(home.join(".marmot-runtime.lock")).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockShared).is_err()
}

async fn stop(mut core: tokio::process::Child) {
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

#[tokio::test(flavor = "multi_thread")]
async fn the_core_is_ready_once_its_store_is_open_and_its_harness_answered() {
    let dir = tempfile::tempdir().unwrap();
    let relay = relay().await;
    let acp = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/stand_in_acp.py");
    let command = [
        "python3".into(),
        acp.display().to_string(),
        "stand-in".into(),
        "acp".into(),
    ];
    let (settings, home) = agent(dir.path(), &relay.url().await.to_string(), &command);
    let notify = dir.path().join("notify");
    let socket = UnixDatagram::bind(&notify).unwrap();
    let file = dir.path().join("core.ready");
    let core = start(&settings, &notify, &file);

    let mut said = [0; 64];
    let n = tokio::time::timeout(Duration::from_secs(60), socket.recv(&mut said))
        .await
        .expect("the core says it is ready")
        .unwrap();
    assert_eq!(&said[..n], b"READY=1");
    assert!(
        store_open(&home),
        "the store is open by the time the core is ready"
    );
    assert!(
        file.is_file(),
        "the file launchd's launcher names is made too"
    );
    stop(core).await;
}

/// A core whose harness never answers its start, as when an ACP agent's handshake fails, has
/// opened the store but is not ready: the installer counts it as a failed start.
#[tokio::test(flavor = "multi_thread")]
async fn a_core_whose_harness_has_not_answered_is_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let relay = relay().await;
    // An ACP agent that says it has started, then waits, never answering its greeting.
    let started = dir.path().join("started");
    let silent = dir.path().join("agent.sh");
    std::fs::write(&silent, "#!/bin/sh\ntouch \"$1\"\nexec sleep 300\n").unwrap();
    let command = [
        "sh".into(),
        silent.display().to_string(),
        started.display().to_string(),
    ];
    let (settings, home) = agent(dir.path(), &relay.url().await.to_string(), &command);
    let notify = dir.path().join("notify");
    let socket = UnixDatagram::bind(&notify).unwrap();
    let file = dir.path().join("core.ready");
    let core = start(&settings, &notify, &file);

    let deadline = Instant::now() + Duration::from_secs(60);
    while !started.exists() {
        assert!(
            Instant::now() < deadline,
            "the core never started its harness"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        store_open(&home),
        "the store is open while the harness starts"
    );
    let mut said = [0; 64];
    let heard = tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut said)).await;
    assert!(heard.is_err(), "nothing is said before the harness answers");
    assert!(!file.exists(), "and no file is made");
    stop(core).await;
}
