//! M1's scenario, with a real Codex: through MDK's test relay, a person invites the agent, has a
//! command approved and run, interrupts a turn, starts a new session, and has the core crash in
//! the middle of a turn and carries on, with nothing run twice (`docs/plan.md`, D-074). Then the
//! agent sends a file from its working folder, which the person downloads and reads (D-090), and
//! the chat moves the agent to a folder inside it, where its next session works (D-101).
//!
//! It approves only the exact command each step asks for, once, in a throwaway folder. It runs
//! twice. On the model double (`tools/model_double`), Codex is the real one and only its model is
//! scripted: it needs `codex` installed, and no key or sign-in, so it always runs. On a signed-in
//! Codex and its real model, it runs only when asked for:
//!
//! ```sh
//! cargo test -p marmot-agent --test codex -- --ignored --nocapture
//! ```

mod common;
mod model_double;

use common::{Chat, TURN, World, files_sent, lines, within};
use marmot_agent::carrier::Asked;
use marmot_agent_codex::Codex;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use model_double::Double;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

/// The program the driver starts as `codex`: the machine's own, signed in, or one that runs it
/// on the model double.
struct Model {
    program: String,
    _home: Option<tempfile::TempDir>,
}

impl Model {
    fn signed_in() -> Self {
        Self {
            program: "codex".into(),
            _home: None,
        }
    }

    /// Codex with a throwaway home, `CODEX_HOME`, whose settings name the double as the model's
    /// provider, with a dummy key: nothing of the machine's own sign-in or settings is read or
    /// touched. The driver starts it through a small script that sets that up.
    fn double(double: &Double) -> Self {
        let home = tempfile::tempdir().unwrap();
        let codex_home = home.path().join("codex");
        std::fs::create_dir(&codex_home).unwrap();
        let settings = format!(
            "model = \"scripted\"\nmodel_provider = \"double\"\n\n[model_providers.double]\n\
             name = \"model double\"\nbase_url = \"{}/v1\"\nwire_api = \"responses\"\n\
             env_key = \"MODEL_DOUBLE_KEY\"\n",
            double.url
        );
        std::fs::write(codex_home.join("config.toml"), settings).unwrap();
        let program = home.path().join("codex-on-the-double");
        let script = format!(
            "#!/bin/sh\nexec env -i PATH=\"$PATH\" HOME='{}' CODEX_HOME='{}' MODEL_DOUBLE_KEY=model-double codex \"$@\"\n",
            home.path().display(),
            codex_home.display()
        );
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            program: program.display().to_string(),
            _home: Some(home),
        }
    }
}

async fn codex(world: &World, model: &Model) -> Codex {
    let setup = world.config.setup().unwrap();
    Codex::start(&model.program, setup)
        .await
        .expect("codex starts")
}

/// A model reply that runs `command` with Codex's shell tool.
fn exec(command: &str) -> Value {
    json!({"call": "exec_command", "input": {"cmd": command}})
}

/// What the model does at each step of the scenario, by words of the step's prompt.
fn script() -> Value {
    let then_done = |call: Value| json!([[call], [{"text": "done"}]]);
    json!({"prompts": [
        {"when": "`touch marker.txt`", "replies": [
            [{"thinking": "One command, exactly as asked."}, {"text": "Running it."}, exec("touch marker.txt")],
            [{"text": "done"}]]},
        {"when": "`echo started >> started.txt && sleep 30`",
         "replies": then_done(exec("echo started >> started.txt && sleep 30"))},
        {"when": "`echo ran >> runs.txt && sleep 20`",
         "replies": then_done(exec("echo ran >> runs.txt && sleep 20"))},
        {"when": "Which shell command did you run last?",
         "replies": [[{"text": "echo ran >> runs.txt && sleep 20"}]]},
        {"when": "Send the file runs.txt",
         "replies": then_done(json!({"call": "send_file", "input": {"path": "runs.txt"}}))},
        {"when": "`touch moved.txt`", "replies": then_done(exec("touch moved.txt"))},
    ]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs codex installed and signed in"]
async fn codex_end_to_end() {
    end_to_end(&Model::signed_in()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_on_the_model_double() {
    let double = Double::scripted(&script()).await;
    end_to_end(&Model::double(&double)).await;
    let asked = double.all_answered().await;
    println!("the model double answered {asked} requests");
}

async fn end_to_end(model: &Model) {
    let work = tempfile::tempdir().unwrap();
    let settings = format!(
        "[codex]\ncwd = \"{}\"\napproval_policy = \"untrusted\"\n",
        work.path().display()
    );
    let world = World::new(&settings).await;
    world.runtime.start().await.unwrap();
    let mut core = world.start(codex(&world, model).await);

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
    let only = "Run exactly this shell command and nothing else, then reply with the word done:";

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

    // 2. A turn interrupted once its command has started.
    let asked = chat
        .say(&format!("{only} `echo started >> started.txt && sleep 30`"))
        .await;
    chat.approve("echo started >> started.txt && sleep 30")
        .await;
    within(TURN, || {
        (lines(&work.path().join("started.txt")) > 0).then_some(())
    })
    .await;
    let interrupt = chat.ask(None, &Asked::Interrupt, "Stop").await;
    let stopped = chat.effect_of(&interrupt).await;
    assert!(matches!(stopped.item.event.body(), Body::Turn(_)));
    assert_eq!(stopped.item.event.header().state, State::Cancelled);
    assert_eq!(stopped.item.started_by.as_ref(), Some(&asked));
    println!("interrupted");

    // 3. A new session.
    let before = chat.sessions();
    let fresh = chat.ask(None, &Asked::NewSession, "New session").await;
    let started = chat.effect_of(&fresh).await;
    assert!(matches!(started.item.event.body(), Body::Session(_)));
    let session = started.item.event.header().session.clone().unwrap();
    assert!(!before.contains(&session), "a new session");
    println!("new session");

    // 4. The core crashes while a command runs. The next core closes the turn the crash cut off,
    //    and the chat carries on in the same session with nothing run twice.
    let runs = work.path().join("runs.txt");
    let asked = chat
        .say(&format!("{only} `echo ran >> runs.txt && sleep 20`"))
        .await;
    chat.approve("echo ran >> runs.txt && sleep 20").await;
    within(TURN, || (lines(&runs) > 0).then_some(())).await;
    let cut_off = chat.turn_for(&asked).await;
    core.crash().await;
    println!("crashed mid-turn");
    core = world.start(codex(&world, model).await);
    chat.turn_at(&cut_off, State::Cancelled).await;
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

    // 5. A file from the working folder, sent with the driver's own tool, arrives as an
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

    // 6. The chat moves the agent to a folder inside its own: a new session starts there, and a
    //    command runs there.
    std::fs::create_dir(work.path().join("sub")).unwrap();
    let sub = std::fs::canonicalize(work.path().join("sub")).unwrap();
    let folder = Asked::Set {
        setting: "folder".into(),
        value: "sub".into(),
    };
    let moved = chat.ask(None, &folder, "Move to sub").await;
    let started = chat.effect_of(&moved).await;
    assert!(matches!(started.item.event.body(), Body::Session(_)));
    let session = started.item.event.header().session.clone().unwrap();
    assert_eq!(chat.folder_of(&session), Some(sub.display().to_string()));
    let asked = chat.say(&format!("{only} `touch moved.txt`")).await;
    chat.approve("touch moved.txt").await;
    within(TURN, || sub.join("moved.txt").exists().then_some(())).await;
    let turn = chat.turn_for(&asked).await;
    let done = chat.turn_at(&turn, State::Done).await;
    assert_eq!(done.item.event.header().session.as_ref(), Some(&session));
    println!("moved to another folder");

    core.stop().await;
    world.runtime.shutdown().await;
}
