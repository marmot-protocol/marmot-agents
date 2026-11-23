//! M3's scenario for Hermes: the Hermes plugin (`python/marmot_hermes`) runs inside Hermes in a
//! container and connects to the core's socket, and through MDK's test relay a person has a prompt
//! answered, has a command approved and run, interrupts a turn while it waits for an approval,
//! starts a new session, has Hermes crash in the middle of a turn and carries on, with nothing
//! run twice (`docs/plan.md`, D-074), has the agent send a file (D-090), and switches the model and
//! back.
//!
//! Each command it asks for is harmless and runs in a throwaway folder mounted into the container.
//! It needs Docker, and a model: `MODEL_BASE_URL`, `MODEL_NAME` and `MODEL_KEY_FILE`, the file
//! that holds the endpoint's key, in the environment or in `tests.env` (`tests.env.example` says
//! how; there is no default). It runs only when asked for:
//!
//! ```sh
//! cargo test -p marmot-agent --test hermes -- --ignored --nocapture
//! ```
//!
//! `HERMES_LOG` names a file that keeps the plugin's own log lines, which name what went wrong by
//! kind only (D-071).

mod common;

use common::{Chat, TestModel, World, files_sent, lines, within};
use marmot_agent::carrier::{self, Asked};
use marmot_agent::marmot::now;
use marmot_agent::run::serve;
use marmot_agent::socket::Endpoint;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use std::path::{Path, PathBuf};
use std::process::Command as Std;
use std::time::Instant;
use tokio::process::{Child, Command};

/// How long Hermes may take to show what is waited for. Hermes waits two minutes before it tries a
/// model call again, and a free model can need that more than once, so this is longer than the
/// other harnesses' `common::TURN`.
const TURN: u64 = 600;

/// Hermes in a container, with its own home in a Docker volume that outlives the container.
struct Hermes {
    name: String,
    volume: String,
    home: PathBuf,
    work: PathBuf,
    model: TestModel,
    child: Option<Child>,
}

impl Hermes {
    fn new(home: &Path, work: &Path, model: TestModel) -> Self {
        let id = std::process::id();
        let volume = format!("marmot-hermes-test-{id}");
        let made = Std::new("docker")
            .args(["volume", "create", &volume])
            .output()
            .expect("docker runs");
        assert!(made.status.success(), "a Docker volume for Hermes's home");
        Self {
            name: format!("marmot-hermes-test-{id}"),
            volume,
            home: home.to_path_buf(),
            work: work.to_path_buf(),
            model,
            child: None,
        }
    }

    /// Starts the container. The plugin inside connects to the core by itself.
    fn start(&mut self) {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/hermes/run.sh");
        let mut command = Command::new(script);
        command
            .arg(&self.home)
            .arg(&self.work)
            .arg(&self.volume)
            .arg(&self.model.key_file)
            .envs(self.model.env())
            .env("HERMES_CONTAINER", &self.name)
            .kill_on_drop(true);
        self.child = Some(command.spawn().expect("the Hermes container starts"));
    }

    /// Ends the container at once, as a crash does: Hermes, and the plugin in it, go mid-turn.
    async fn crash(&mut self) {
        let _ = Std::new("docker").args(["kill", &self.name]).output();
        if let Some(mut child) = self.child.take() {
            let _ = child.wait().await;
        }
    }
}

/// Waits for the agent to say something containing `needle`, ignoring case, in a chat message
/// after the first `after` of them. Hermes's commentary, its lines about a tool and its heartbeats,
/// starts with an emoji and is not an answer.
async fn words_with(chat: &Chat<'_>, after: usize, needle: &str) {
    within(TURN, || {
        let words = chat.world.sent_by_agent(&chat.hex, 9);
        words.into_iter().skip(after).find(|m| {
            m.plaintext.chars().next().is_some_and(|c| c.is_ascii())
                && m.plaintext.to_lowercase().contains(needle)
        })
    })
    .await;
}

/// Prints each version of each item as the person's side reads it, and each of the agent's chat
/// messages, so that a run that fails can be followed: the item type, its state and the sizes only,
/// never what anything says (D-071).
fn watch(world: &World, hex: &str, started: Instant) -> tokio::task::JoinHandle<()> {
    let (runtime, person, agent, hex) = (
        world.runtime.clone(),
        world.person.clone(),
        world.agent.clone(),
        hex.to_string(),
    );
    tokio::spawn(async move {
        let mut seen = std::collections::HashSet::new();
        loop {
            let query = marmot_app::AppMessageQuery {
                group_id_hex: Some(hex.clone()),
                kinds: Some(vec![9, carrier::ITEM_KIND, carrier::EDIT_KIND]),
                limit: Some(1000),
            };
            for m in runtime
                .messages_with_query(&person, query)
                .unwrap_or_default()
            {
                if m.sender != agent || !seen.insert(m.message_id_hex.clone()) {
                    continue;
                }
                let elapsed = started.elapsed().as_secs();
                match carrier::read_item(m.kind, &m.plaintext, &m.tags) {
                    Some(read) => println!(
                        "  {elapsed:4}s {} v{} {:?}, text {} bytes",
                        kind(read.item.event.body()),
                        read.item.version,
                        read.item.event.header().state,
                        read.item.event.header().text.len(),
                    ),
                    None => println!("  {elapsed:4}s words, {} bytes", m.plaintext.len()),
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    })
}

/// An item's type, by name.
fn kind(body: &Body) -> &'static str {
    match body {
        Body::Turn(_) => "turn",
        Body::Message(_) => "message",
        Body::Reasoning(_) => "reasoning",
        Body::Tool(_) => "tool",
        Body::Approval(_) => "approval",
        Body::Question(_) => "question",
        Body::Plan(_) => "plan",
        Body::Task(_) => "task",
        Body::Notice(_) => "notice",
        Body::Error(_) => "error",
        Body::Session(_) => "session",
    }
}

impl Drop for Hermes {
    fn drop(&mut self) {
        let _ = Std::new("docker").args(["rm", "-f", &self.name]).output();
        let _ = Std::new("docker")
            .args(["volume", "rm", "-f", &self.volume])
            .output();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Docker and a model"]
async fn hermes_end_to_end() {
    let model = TestModel::read();
    let started = Instant::now();
    let work = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    // The agent may send the files in the work folder, as the installer lists Hermes's (D-090).
    let settings = format!(
        "name = \"Hermes\"\nfiles_from = [{:?}]\n[connector]\n",
        work.path()
    );
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
    let lap = |what: &str| println!("{what} after {}s", started.elapsed().as_secs());
    let model_name = model.name.clone();
    let mut hermes = Hermes::new(home.path(), work.path(), model);
    hermes.start();

    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    // The core takes the invitation once the plugin has connected, which waits for Hermes to start.
    let group_hex = hex::encode(group.as_slice());
    within(TURN, || {
        let groups = world.marmot.groups().unwrap_or_default();
        groups.iter().any(|g| g.as_str() == group_hex).then_some(())
    })
    .await;
    let hex = world.joined(&group).await;
    lap("joined");
    let watching = watch(&world, &hex, started);
    let chat = Chat {
        world: &world,
        group,
        hex,
        turn: TURN,
    };
    let only = "Use your terminal tool to run exactly this shell command and nothing else, in the \
                foreground, then reply with the word done:";

    // 1. A prompt, answered.
    let asked = chat
        .say("Reply with the single word marmot, and nothing else. Do not use any tools.")
        .await;
    let turn = chat.turn_for(&asked).await;
    words_with(&chat, 0, "marmot").await;
    chat.turn_at(&turn, State::Done).await;
    lap("answered");

    // 2. A command Hermes flags as dangerous, approved once and run.
    let approved = work.path().join("approved");
    std::fs::create_dir(&approved).unwrap();
    // The container mounts the work folder at its own path.
    let command = format!("rm -rf {}", approved.display());
    let asked = chat.say(&format!("{only} `{command}`")).await;
    let allowed = chat.approve(&command).await;
    let answered = chat.effect_of(&allowed).await;
    assert!(matches!(answered.item.event.body(), Body::Approval(_)));
    within(TURN, || (!approved.exists()).then_some(())).await;
    let turn = chat.turn_for(&asked).await;
    chat.turn_at(&turn, State::Done).await;
    lap("approved and run");

    // 3. A turn interrupted while it waits for an approval, which then never runs.
    let never = work.path().join("never");
    std::fs::create_dir(&never).unwrap();
    let asked = chat
        .say(&format!("{only} `rm -rf {}`", never.display()))
        .await;
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
    assert!(never.exists(), "the command never ran");
    lap("interrupted");

    // 4. A new session: the old one ends, and the next prompt opens another, even across a restart
    //    of Hermes in between.
    let before = chat.sessions();
    let fresh = chat.ask(None, &Asked::NewSession, "New session").await;
    let ended = chat.effect_of(&fresh).await;
    assert!(matches!(ended.item.event.body(), Body::Session(_)));
    hermes.crash().await;
    hermes.start();
    let said = world.sent_by_agent(&chat.hex, 9).len();
    let asked = chat
        .say("Reply with the single word fresh, and nothing else. Do not use any tools.")
        .await;
    let turn = chat.turn_for(&asked).await;
    words_with(&chat, said, "fresh").await;
    let done = chat.turn_at(&turn, State::Done).await;
    let session = done.item.event.header().session.clone().unwrap();
    assert!(!before.contains(&session), "a new session");
    assert!(never.exists(), "the interrupted command never ran");
    lap("new session");

    // 5. Hermes crashes while a command runs. The core closes the turn the crash cut off; Hermes
    //    comes back and carries on in the same session, with nothing run twice.
    let runs = work.path().join("runs.txt");
    let asked = chat
        .say(&format!(
            "{only} `echo ran >> {} && sleep 30`",
            runs.display()
        ))
        .await;
    within(TURN, || (lines(&runs) > 0).then_some(())).await;
    let cut_off = chat.turn_for(&asked).await;
    hermes.crash().await;
    lap("crashed mid-turn");
    chat.turn_at(&cut_off, State::Cancelled).await;
    hermes.start();
    let said = world.sent_by_agent(&chat.hex, 9).len();
    let follow_up = chat
        .say("Which shell command did you run last? Reply with only the command. Do not use any tools.")
        .await;
    let answer = chat.turn_for(&follow_up).await;
    // Hermes keeps a session whose turn a crash cut off paused until the lost turn is dropped; the
    // plugin drops it, and the follow-up is answered.
    words_with(&chat, said, "").await;
    let done = chat.turn_at(&answer, State::Done).await;
    assert_eq!(done.item.event.header().session.as_ref(), Some(&session));
    assert_eq!(lines(&runs), 1, "the command ran once");
    assert_eq!(
        chat.turns_for(&asked).len(),
        1,
        "the prompt started one turn"
    );
    lap("carried on after the crash");

    // 6. A file the agent names in its answer, in the work folder, reaches the chat as an
    //    attachment the person can read (D-090).
    let report = work.path().join("report.txt");
    std::fs::write(&report, "a report\n").unwrap();
    let asked = chat
        .say(&format!(
            "Reply with only this line, exactly, and nothing else. Do not use any tools.\nMEDIA:{}",
            report.display()
        ))
        .await;
    let turn = chat.turn_for(&asked).await;
    chat.turn_at(&turn, State::Done).await;
    let files = sent_files(&world, &chat).await;
    assert_eq!(files.len(), 1, "one file sent");
    assert_eq!(files[0].0, "report.txt");
    assert_eq!(files[0].1, b"a report\n");
    lap("sent a file");

    // 7. The model switched from the chat, and back. The plugin types `/model` for Hermes and
    //    tells from its reply that it was taken, by Hermes's own catalog of replies
    //    (`agent.i18n.t`), which Hermes does not publish for plugins. The switch is to the model
    //    Hermes runs, named, since the endpoint may offer no other: Hermes takes it as a switch and
    //    keeps it as the session's own model, which the reset then clears (read in Hermes's source,
    //    `gateway/slash_commands_model.py`; the scripted run in `import_gateways.rs` switches to
    //    another).
    for value in [model_name.as_str(), "reset"] {
        let asked = Asked::Set {
            setting: "model".into(),
            value: value.into(),
        };
        let set = chat.ask(None, &asked, value).await;
        let shown = chat.effect_of(&set).await;
        let Body::Session(session) = shown.item.event.body() else {
            panic!("not the session's item")
        };
        let now = session.settings.iter().find(|s| s.id == "model");
        assert_eq!(
            now.and_then(|m| m.value.as_deref()),
            Some(model_name.as_str())
        );
    }
    lap("switched the model and back");

    watching.abort();
    let _ = stop.send(());
    serving.await.unwrap().unwrap();
    hermes.crash().await;
    world.runtime.shutdown().await;
}

/// The files the agent sent to the chat, once there is one, or none after a turn's time.
async fn sent_files(world: &World, chat: &Chat<'_>) -> Vec<(String, Vec<u8>, Option<String>)> {
    let started = Instant::now();
    loop {
        let files = files_sent(world, &chat.group).await;
        if !files.is_empty() || started.elapsed() > std::time::Duration::from_secs(TURN) {
            return files;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
