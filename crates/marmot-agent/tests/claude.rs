//! M2's scenario, with a real Claude Code: the Claude Code driver (`packages/claude-code`) connects
//! to the core's socket, and through MDK's test relay a person invites the agent, has a command
//! approved and run, interrupts a turn while it waits for an approval, starts a new session, and
//! has the driver crash in the middle of a turn and carries on, with nothing run twice
//! (`docs/plan.md`, D-074). Then the agent sends a file from its working folder, which the person
//! downloads and reads (D-090), and the chat moves the agent to a folder inside it, where its next
//! session works (D-101).
//!
//! It approves only the exact command each step asks for, once, in a throwaway folder. It runs
//! twice. On the model double (`tools/model_double`), Claude Code is the real one and only its
//! model is scripted: it needs Node 24 and `npm ci` run in `packages/claude-code`, and no key or
//! sign-in, so it always runs. On a signed-in Claude Code and its real model, it runs only when
//! asked for:
//!
//! ```sh
//! cargo test -p marmot-agent --test claude -- --ignored --nocapture
//! ```

mod common;
mod model_double;

use common::{Chat, TURN, World, files_sent, lines, within};
use marmot_agent::carrier::Asked;
use marmot_agent::marmot::now;
use marmot_agent::run::serve;
use marmot_agent::socket::Endpoint;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use model_double::Double;
use serde_json::{Value, json};
use std::path::Path;
use tokio::process::{Child, Command};

/// The model Claude Code runs on.
enum Model {
    /// The real one, as the machine's Claude Code is signed in.
    SignedIn,
    /// The model double at this address, with a throwaway home for Claude Code, so nothing of the
    /// machine's own sign-in or settings is read or touched.
    Double(String, tempfile::TempDir),
}

impl Model {
    fn double(double: &Double) -> Self {
        Self::Double(double.url.clone(), tempfile::tempdir().unwrap())
    }
}

/// Starts the Claude Code driver for the agent whose home is `home`, working in `work`.
fn driver(home: &Path, work: &Path, model: &Model) -> Child {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/claude-code");
    let mut command = Command::new("node");
    command
        .arg("src/main.ts")
        .arg("--home")
        .arg(home)
        .arg("--cwd")
        .arg(work)
        .args(["--model", "haiku", "--permission-mode", "default"])
        .current_dir(package)
        .kill_on_drop(true);
    if let Model::Double(url, own) = model {
        // Only what Node needs from this machine; Claude Code's home and settings are new, and its
        // key is a dummy the double takes.
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", own.path())
            .env("CLAUDE_CONFIG_DIR", own.path().join("claude"))
            .env("ANTHROPIC_BASE_URL", url)
            .env("ANTHROPIC_API_KEY", "model-double")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    }
    command.spawn().expect("node runs the driver")
}

/// A model reply that runs `command` with Claude Code's shell tool.
fn bash(command: &str) -> Value {
    json!({"call": "Bash", "input": {"command": command, "description": "Run the command asked for"}})
}

/// What the model does at each step of the scenario, by words of the step's prompt.
fn script() -> Value {
    let then_done = |call: Value| json!([[call], [{"text": "done"}]]);
    json!({"prompts": [
        {"when": "`touch marker.txt`", "replies": [
            [{"thinking": "One command, exactly as asked."}, {"text": "Running it."}, bash("touch marker.txt")],
            [{"text": "done"}]]},
        {"when": "`touch never.txt`", "replies": then_done(bash("touch never.txt"))},
        {"when": "Reply with the word fresh.", "replies": [[{"text": "fresh"}]]},
        {"when": "`echo ran >> runs.txt && sleep 20`",
         "replies": then_done(bash("echo ran >> runs.txt && sleep 20"))},
        {"when": "Which shell command did you run last?",
         "replies": [[{"text": "echo ran >> runs.txt && sleep 20"}]]},
        {"when": "Send the file runs.txt",
         "replies": then_done(json!({"call": "mcp__marmot__send_file", "input": {"path": "runs.txt"}}))},
        {"when": "`touch moved.txt`", "replies": then_done(bash("touch moved.txt"))},
    ]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Node, the driver's packages and a signed-in Claude Code"]
async fn claude_code_end_to_end() {
    end_to_end(&Model::SignedIn).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claude_code_on_the_model_double() {
    let double = Double::scripted(&script()).await;
    end_to_end(&Model::double(&double)).await;
    let asked = double.all_answered().await;
    println!("the model double answered {asked} requests");
}

/// The cassette recorded from a real model for `one_turn`, by `record_claude_code_turn`.
fn cassette() -> std::path::PathBuf {
    model_double::repository().join("fixtures/cassettes/claude/approval.jsonl")
}

/// A turn replayed from a cassette recorded from a real model: the requests this Claude Code
/// makes must still match the ones the recording was made from (`tools/model_double`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claude_code_replays_a_recorded_turn() {
    let double = Double::replay(&cassette()).await;
    let work = demo();
    one_turn(&Model::double(&double), work.path()).await;
    let asked = double.all_answered().await;
    println!("the cassette answered {asked} requests");
}

/// Records `one_turn` from a real model into a cassette, through the model double in a container
/// that holds the key (`tools/model_double/README.md` says how to run it and what to do next).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Docker, a model and its key"]
async fn record_claude_code_turn() {
    let setting = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    let key = std::path::PathBuf::from(setting("MODEL_DOUBLE_KEY_FILE"));
    let out = std::path::PathBuf::from(setting("MODEL_DOUBLE_OUT"));
    let work = std::path::PathBuf::from(setting("MODEL_DOUBLE_DEMO"));
    let sdk = model_double::repository()
        .join("packages/claude-code/node_modules/@anthropic-ai/claude-agent-sdk/package.json");
    let sdk: Value = serde_json::from_str(&std::fs::read_to_string(sdk).unwrap()).unwrap();
    let version = sdk["claudeCodeVersion"].as_str().unwrap();
    let harness = ("claude-code", version, "anthropic");
    let upstream = setting("MODEL_DOUBLE_UPSTREAM");
    let double = Double::record(
        &key,
        &upstream,
        &setting("MODEL_DOUBLE_MODEL"),
        &out,
        harness,
    )
    .await;
    one_turn(&Model::double(&double), &work).await;
    let asked = double.all_answered().await;
    println!("recorded {asked} requests");
}

/// A recording double's container holds the key, so it goes with its test, even one that fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Docker"]
async fn a_recording_container_goes_with_its_test() {
    let files = tempfile::tempdir().unwrap();
    let key = files.path().join("key");
    std::fs::write(&key, "not-a-key").unwrap();
    let out = files.path().join("out");
    let (named, name) = tokio::sync::oneshot::channel();
    let failed = tokio::spawn(async move {
        let harness = ("claude-code", "0", "anthropic");
        let double = Double::record(&key, "http://127.0.0.1:9", "m", &out, harness).await;
        named.send(double.container.clone().unwrap()).unwrap();
        panic!("a recording that fails");
    })
    .await;
    assert!(failed.is_err());
    let name = name.await.unwrap();
    let listed = std::process::Command::new("docker")
        .args(["ps", "-a", "-q", "--filter"])
        .arg(format!("name=^{name}$"))
        .output()
        .unwrap();
    assert!(listed.status.success());
    let left = String::from_utf8(listed.stdout).unwrap();
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", &name])
        .output();
    assert!(
        left.trim().is_empty(),
        "the container {name} outlived its test"
    );
}

/// A throwaway copy of the project the recordings are made in, `tools/record/demo`.
fn demo() -> tempfile::TempDir {
    let work = tempfile::tempdir().unwrap();
    let from = model_double::repository().join("tools/record/demo");
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), work.path().join(entry.file_name())).unwrap();
    }
    work
}

/// One turn in `work`: a command asked for, approved once and run, and the turn done.
async fn one_turn(model: &Model, work: &Path) {
    let home = tempfile::tempdir().unwrap();
    let world = World::new("name = \"Claude\"\n[connector]\n").await;
    world.runtime.start().await.unwrap();
    let (listener, token) = Endpoint::in_home(home.path()).listen().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (marmot, config) = (world.marmot.clone(), world.config.clone());
    let store = world.state.path().to_path_buf();
    let serving = tokio::spawn(async move {
        let stop = async {
            let _ = stopped.await;
        };
        serve(marmot, &config, &store, listener, token, now(), stop).await
    });
    let mut claude = driver(home.path(), work, model);
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let hex = world.joined(&group).await;
    let chat = Chat {
        world: &world,
        group,
        hex,
        turn: TURN,
    };
    let asked = chat
        .say("Run exactly this shell command and nothing else, in the foreground, then reply with the word done: `python3 greet.py`")
        .await;
    chat.approve("python3 greet.py").await;
    let turn = chat.turn_for(&asked).await;
    chat.turn_at(&turn, State::Done).await;
    println!("approved, run and done");
    let _ = stop.send(());
    serving.await.unwrap().unwrap();
    let _ = claude.kill().await;
    world.runtime.shutdown().await;
}

async fn end_to_end(model: &Model) {
    let work = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    // The connector states its folder in its hello, and files come from there (D-090, D-101).
    let settings = "name = \"Claude\"\n[connector]\n".to_string();
    let world = World::new(&settings).await;
    world.runtime.start().await.unwrap();
    let (listener, token) = Endpoint::in_home(home.path()).listen().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (marmot, config) = (world.marmot.clone(), world.config.clone());
    let store = world.state.path().to_path_buf();
    let serving = tokio::spawn(async move {
        let stop = async {
            let _ = stopped.await;
        };
        serve(marmot, &config, &store, listener, token, now(), stop).await
    });
    let mut claude = driver(home.path(), work.path(), model);

    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let hex = world.joined(&group).await;
    let chat = Chat {
        world: &world,
        group,
        hex,
        turn: TURN,
    };
    let only = "Run exactly this shell command and nothing else, in the foreground, then reply with the word done:";

    // 1. A command that needs approval, approved once and run.
    let asked = chat.say(&format!("{only} `touch marker.txt`")).await;
    let allowed = chat.approve("touch marker.txt").await;
    within(TURN, || {
        work.path().join("marker.txt").exists().then_some(())
    })
    .await;
    let answered = chat.effect_of(&allowed).await;
    assert!(matches!(answered.item.event.body(), Body::Approval(_)));
    let turn = chat.turn_for(&asked).await;
    chat.turn_at(&turn, State::Done).await;
    println!("approved and run");

    // 2. A turn interrupted while it waits for an approval, which then never runs.
    let asked = chat.say(&format!("{only} `touch never.txt`")).await;
    within(TURN, || {
        chat.latest().into_iter().find(|(_, r)| {
            matches!(r.item.event.body(), Body::Approval(_))
                && r.item.event.header().state == State::Waiting
        })
    })
    .await;
    let interrupt = chat.ask(None, &Asked::Interrupt, "Stop").await;
    let stopped_turn = chat.effect_of(&interrupt).await;
    assert!(matches!(stopped_turn.item.event.body(), Body::Turn(_)));
    assert_eq!(stopped_turn.item.event.header().state, State::Cancelled);
    assert_eq!(stopped_turn.item.started_by.as_ref(), Some(&asked));
    assert!(
        !work.path().join("never.txt").exists(),
        "the command never ran"
    );
    println!("interrupted");

    // 3. A new session: the old one ends, and the next prompt opens another, even across a restart
    //    of the driver in between.
    let before = chat.sessions();
    let fresh = chat.ask(None, &Asked::NewSession, "New session").await;
    let ended = chat.effect_of(&fresh).await;
    assert!(matches!(ended.item.event.body(), Body::Session(_)));
    // The driver restarts before the next prompt: the ended session must not come back.
    claude.kill().await.unwrap();
    claude = driver(home.path(), work.path(), model);
    let asked = chat.say("Reply with the word fresh.").await;
    let turn = chat.turn_for(&asked).await;
    let done = chat.turn_at(&turn, State::Done).await;
    let session = done.item.event.header().session.clone().unwrap();
    assert!(!before.contains(&session), "a new session");
    println!("new session");

    // 4. The driver crashes while a command runs. The core closes the turn the crash cut off;
    //    the next driver carries on in the same session, with nothing run twice.
    let runs = work.path().join("runs.txt");
    let asked = chat
        .say(&format!("{only} `echo ran >> runs.txt && sleep 20`"))
        .await;
    chat.approve("echo ran >> runs.txt && sleep 20").await;
    within(TURN, || (lines(&runs) > 0).then_some(())).await;
    let cut_off = chat.turn_for(&asked).await;
    claude.kill().await.unwrap();
    println!("crashed mid-turn");
    chat.turn_at(&cut_off, State::Cancelled).await;
    claude = driver(home.path(), work.path(), model);
    let follow_up = chat
        .say("Which shell command did you run last? Reply with only the command.")
        .await;
    let answer = chat.turn_for(&follow_up).await;
    let done = chat.turn_at(&answer, State::Done).await;
    assert_eq!(done.item.event.header().session.as_ref(), Some(&session));
    assert_eq!(lines(&runs), 1, "the command ran once");
    assert_eq!(
        chat.turns_for(&asked).len(),
        1,
        "the prompt started one turn"
    );
    println!("carried on after the crash");

    // 5. A file from the working folder, sent with the connector's own tool, arrives as an
    //    attachment the person can read.
    let asked = chat
        .say("Send the file runs.txt to the chat with your send_file tool, then reply with the word done.")
        .await;
    let sent = chat.turn_for(&asked).await;
    chat.turn_at(&sent, State::Done).await;
    let mut files = Vec::new();
    for _ in 0..TURN {
        files = files_sent(&world, &chat.group).await;
        if !files.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert_eq!(files.len(), 1, "one file sent");
    assert_eq!(files[0].0, "runs.txt");
    assert_eq!(files[0].1, b"ran\n");
    println!("sent a file");

    // 6. The chat moves the agent to a folder inside its own: the session ends, the next prompt
    //    opens one there, and a command runs there.
    std::fs::create_dir(work.path().join("sub")).unwrap();
    let sub = std::fs::canonicalize(work.path().join("sub")).unwrap();
    let folder = Asked::Set {
        setting: "folder".into(),
        value: "sub".into(),
    };
    let moved = chat.ask(None, &folder, "Move to sub").await;
    let ended = chat.effect_of(&moved).await;
    assert!(matches!(ended.item.event.body(), Body::Session(_)));
    let asked = chat.say(&format!("{only} `touch moved.txt`")).await;
    chat.approve("touch moved.txt").await;
    within(TURN, || sub.join("moved.txt").exists().then_some(())).await;
    let turn = chat.turn_for(&asked).await;
    let done = chat.turn_at(&turn, State::Done).await;
    let moved_to = done.item.event.header().session.clone().unwrap();
    assert_ne!(moved_to, session, "a new session");
    assert_eq!(chat.folder_of(&moved_to), Some(sub.display().to_string()));
    println!("moved to another folder");

    let _ = stop.send(());
    serving.await.unwrap().unwrap();
    let _ = claude.kill().await;
    world.runtime.shutdown().await;
}
