//! `tools/pi/pi.sh`, without Docker. It mounts the working folder at its own path, read-write. If
//! that folder held Pi's state folder or the model key, the chat's Pi would get every chat's
//! sessions and the key through it, so the script refuses to start, before it touches Docker. What
//! it asks of Docker otherwise is checked against a stand-in (`fake_docker.sh`): that asking Pi
//! its version mounts none of the state, and that a container still being created when the script
//! is stopped is removed.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn launch(work: &Path, key: &Path, state: &Path) -> Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/pi/pi.sh");
    Command::new("sh")
        .arg(script)
        .arg(key)
        .arg(state)
        .arg("--version")
        .current_dir(work)
        .env("MARMOT_AGENT_CHAT", "ab12")
        .output()
        .expect("sh runs")
}

fn refused(output: &Output, what: &str) {
    assert_eq!(output.status.code(), Some(2), "refused");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains(what), "says why: {said}");
}

#[test]
fn the_working_folder_may_not_hold_the_state_or_the_key() {
    let outside = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let key = outside.path().join("key");
    std::fs::write(&key, "not a key").unwrap();

    // The state folder inside the working folder, named directly or through a link from outside.
    let inner = work.path().join("state");
    refused(&launch(work.path(), &key, &inner), "state folder");
    let link = outside.path().join("state-link");
    symlink(work.path(), &link).unwrap();
    refused(
        &launch(work.path(), &key, &link.join("state")),
        "state folder",
    );
    // The working folder itself, or inside the state folder.
    refused(&launch(work.path(), &key, work.path()), "state folder");
    let nested = outside.path().join("work");
    std::fs::create_dir(&nested).unwrap();
    refused(&launch(&nested, &key, outside.path()), "state folder");

    // The key inside the working folder, named directly or through a link from outside.
    let inner_key = work.path().join("key");
    std::fs::write(&inner_key, "not a key").unwrap();
    let state = tempfile::tempdir().unwrap();
    refused(&launch(work.path(), &inner_key, state.path()), "key file");
    let key_link = outside.path().join("key-link");
    symlink(&inner_key, &key_link).unwrap();
    refused(&launch(work.path(), &key_link, state.path()), "key file");

    // A working folder reached through a link is resolved too.
    let work_link = outside.path().join("work-link");
    symlink(work.path(), &work_link).unwrap();
    refused(&launch(&work_link, &key, &inner), "state folder");
}

/// The script, run in `work` with the stand-in for Docker, which keeps its log in `docker`.
fn with_fake_docker(work: &Path, docker: &Path, key: &Path, state: &Path) -> Command {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let bin = docker.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    symlink(tests.join("fake_docker.sh"), bin.join("docker")).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let mut command = Command::new("sh");
    command
        .arg(tests.join("../../../tools/pi/pi.sh"))
        .arg(key)
        .arg(state)
        .current_dir(work)
        .env("PATH", path)
        .env("FAKE_DOCKER", docker)
        .env("MODEL_BASE_URL", "https://models.example/v1")
        .env("MODEL_NAME", "some/model")
        .env_remove("MARMOT_AGENT_CHAT");
    command
}

fn log(docker: &Path) -> String {
    std::fs::read_to_string(docker.join("log")).unwrap_or_default()
}

/// Folders for one run: the working folder, the stand-in's, a key, and a state root not yet made.
fn folders() -> (tempfile::TempDir, tempfile::TempDir, PathBuf, PathBuf) {
    let (work, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let key = outside.path().join("key");
    std::fs::write(&key, "not a key").unwrap();
    let state = outside.path().join("state");
    (work, outside, key, state)
}

#[test]
fn asking_the_version_mounts_none_of_the_state() {
    let (work, outside, key, state) = folders();
    let docker = outside.path().join("docker");
    // As the driver asks: its own arguments, then `--version`.
    let output = with_fake_docker(work.path(), &docker, &key, &state)
        .args(["--extension", "gate.ts", "--version"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0.0.0-fake");
    let log = log(&docker);
    let mount = (log.lines())
        .find_map(|l| l.strip_prefix("mount ")?.strip_suffix(":/state"))
        .expect("a folder is mounted as the state");
    assert!(!Path::new(mount).starts_with(&state), "{mount}");
    assert!(!state.exists(), "the state root is not made");
}

#[test]
fn a_container_still_being_created_is_removed_when_the_script_is_stopped() {
    let (work, outside, key, state) = folders();
    let docker = outside.path().join("docker");
    let mut child = with_fake_docker(work.path(), &docker, &key, &state)
        .args(["--mode", "rpc"])
        .env("FAKE_DOCKER_SLOW", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !log(&docker).contains("creating") {
        assert!(Instant::now() < deadline, "docker run is called");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Stopped as the driver stops it, while Docker has not yet made the container.
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(143));
    assert!(log(&docker).contains("removed c-"), "{}", log(&docker));
    let left = std::fs::read_dir(&docker).unwrap().flatten();
    let left: Vec<_> = (left.map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|name| name.starts_with("c-"))
        .collect();
    assert!(left.is_empty(), "no container is left: {left:?}");
}

// The driver names its extension in MARMOT_AGENT_PI_EXTENSION: the script mounts that one file,
// read-only, at its own path, so `--extension <it>` loads it in the container. A file in the state
// folder is refused, since mounting it would hand one chat's Pi another chat's sessions.
#[test]
fn the_drivers_extension_is_mounted_read_only_and_never_from_the_state() {
    let (work, outside, key, state) = folders();
    let docker = outside.path().join("docker");
    let extension = outside.path().join("ext").join("send-file.ts");
    std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
    std::fs::write(&extension, "export default function () {}").unwrap();
    let extension = std::fs::canonicalize(&extension).unwrap();
    let output = with_fake_docker(work.path(), &docker, &key, &state)
        .args(["--version"])
        .env("MARMOT_AGENT_PI_EXTENSION", &extension)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let wanted = format!("mount {0}:{0}:ro", extension.display());
    assert!(
        log(&docker).lines().any(|l| l == wanted),
        "{}",
        log(&docker)
    );

    // Named through a link, as when the temporary folder is one: the file is mounted at the name
    // given, so the `--extension` argument the driver passed finds it in the container too.
    let link = outside.path().join("link");
    std::os::unix::fs::symlink(extension.parent().unwrap(), &link).unwrap();
    let through = link.join("send-file.ts");
    let linked = outside.path().join("docker-link");
    let output = with_fake_docker(work.path(), &linked, &key, &state)
        .args(["--version"])
        .env("MARMOT_AGENT_PI_EXTENSION", &through)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let wanted = format!("mount {}:{}:ro", extension.display(), through.display());
    assert!(
        log(&linked).lines().any(|l| l == wanted),
        "{}",
        log(&linked)
    );

    std::fs::create_dir_all(&state).unwrap();
    let inside = state.join("send-file.ts");
    std::fs::write(&inside, "export default function () {}").unwrap();
    let output = with_fake_docker(work.path(), &outside.path().join("docker2"), &key, &state)
        .args(["--version"])
        .env("MARMOT_AGENT_PI_EXTENSION", &inside)
        .output()
        .unwrap();
    refused(&output, "state folder");
    let missing = outside.path().join("none.ts");
    let output = with_fake_docker(work.path(), &outside.path().join("docker3"), &key, &state)
        .args(["--version"])
        .env("MARMOT_AGENT_PI_EXTENSION", &missing)
        .output()
        .unwrap();
    refused(&output, "must name a file");
}
