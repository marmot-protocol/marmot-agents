//! M3's scenario for OpenClaw: the OpenClaw plugin (`packages/openclaw`), running inside a real
//! OpenClaw in a container, connects to the core's socket, and through MDK's test relay a person
//! invites the agent, has a command approved and run, stops a turn while it waits for an approval,
//! starts a new session, has the plugin's process crash in the middle of a turn and carries on,
//! with nothing run twice (`docs/plan.md`, D-074), and has the agent send a file (D-090).
//!
//! It approves only the exact command each step asks for, once, in a throwaway folder. It needs
//! Docker, and a model: `MODEL_BASE_URL`, `MODEL_NAME` and `MODEL_KEY_FILE`, the file that holds
//! the endpoint's key, in the environment or in `tests.env` (`tests.env.example` says how; there
//! is no default). The first run builds the image. The gateway's own log is read only
//! for the plugin's line that says it connected, and is neither printed nor kept: it may hold what
//! the agent was told (D-071). It runs only when asked for:
//!
//! ```sh
//! cargo test -p marmot-agent --test openclaw -- --ignored --nocapture
//! ```

mod common;

use common::{Chat, TURN, TestModel, World, files_sent, lines, within};
use marmot_agent::carrier::Asked;
use marmot_agent::marmot::now;
use marmot_agent::run::serve;
use marmot_agent::socket::Endpoint;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// How long OpenClaw may take to start and connect: the first start sets it up.
const START: u64 = 300;

/// OpenClaw with the plugin, in its container, and how many times it has connected.
struct OpenClaw {
    child: Child,
    name: String,
    connected: Arc<AtomicUsize>,
}

/// The folders a container is given.
struct Folders<'a> {
    home: &'a Path,
    state: &'a Path,
    work: &'a Path,
    model: &'a TestModel,
}

impl OpenClaw {
    /// Starts OpenClaw with the plugin for the agent whose home is `home`, and waits until the
    /// plugin has connected to the core.
    async fn start(folders: &Folders<'_>, n: usize) -> Self {
        let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/openclaw");
        // The script names the container after the state folder.
        let named = std::process::Command::new(package.join("run.sh"))
            .arg("--name")
            .arg(folders.state)
            .output()
            .expect("the script names the container");
        let name = String::from_utf8(named.stdout).unwrap().trim().to_string();
        println!("starting OpenClaw, {n}");
        let mut child = Command::new(package.join("run.sh"))
            .arg(folders.home)
            .arg(folders.state)
            .arg(folders.work)
            .arg(&folders.model.key_file)
            .envs(folders.model.env())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("the plugin's container runs");
        let connected = Arc::new(AtomicUsize::new(0));
        // The gateway's own log is read for the plugin's one line that says it connected, and is
        // never printed or kept: it may hold what the agent was told.
        for stream in [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        ]
        .into_iter()
        .flatten()
        {
            let connected = connected.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.contains("marmot-agent-openclaw: connected to the core") {
                        connected.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
        let started = Instant::now();
        within(START, || {
            (connected.load(Ordering::SeqCst) > 0).then_some(())
        })
        .await;
        println!("OpenClaw connected in {:.0?}", started.elapsed());
        Self {
            child,
            name,
            connected,
        }
    }

    /// Kills the container at once, as a crash does.
    async fn kill(mut self) {
        let _ = Command::new("docker")
            .args(["kill", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let _ = self.child.wait().await;
        let _ = self.connected;
    }
}

/// Prints each version of each agent item as it arrives: its type, state and title, never its
/// text.
fn trace(world: &World, hex: &str) -> tokio::task::JoinHandle<()> {
    let (runtime, person, agent, hex) = (
        world.runtime.clone(),
        world.person.clone(),
        world.agent.clone(),
        hex.to_string(),
    );
    tokio::spawn(async move {
        let mut shown = std::collections::HashSet::new();
        loop {
            let query = marmot_app::AppMessageQuery {
                group_id_hex: Some(hex.clone()),
                kinds: Some(vec![marmot_agent::carrier::ITEM_KIND]),
                limit: Some(500),
            };
            let messages = runtime
                .messages_with_query(&person, query)
                .unwrap_or_default();
            for m in messages.into_iter().filter(|m| m.sender == agent) {
                if !shown.insert(m.message_id_hex.clone()) {
                    continue;
                }
                if let Some(read) = marmot_agent::carrier::read_item(&m.plaintext, &m.tags) {
                    let header = read.item.event.header();
                    println!(
                        "  item {:?} {:?} {}",
                        read.item.event.body(),
                        header.state,
                        header.title
                    );
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    })
}

/// A test that fails drops its container with it: dropping the `docker` client alone would leave
/// the container running.
impl Drop for OpenClaw {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["kill", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Docker and a model"]
async fn openclaw_end_to_end() {
    let began = Instant::now();
    let model = TestModel::read();
    let work = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let folders = Folders {
        home: home.path(),
        state: state.path(),
        work: work.path(),
        model: &model,
    };
    // The agent may send the files in its workspace, as the installer lists OpenClaw's (D-090).
    let settings = format!(
        "name = \"OpenClaw\"\nfiles_from = [{:?}]\n[connector]\n",
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
    let mut openclaw = OpenClaw::start(&folders, 1).await;

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
    let tracing = trace(&world, &chat.hex);
    let only = "Use your exec tool to run exactly this shell command and nothing else, then reply with the word done:";

    // 1. A prompt answered, with a command that needs approval, approved once and run.
    let step = Instant::now();
    let asked = chat.say(&format!("{only} touch marker.txt")).await;
    let allowed = chat.approve("touch marker.txt").await;
    within(TURN, || {
        work.path().join("marker.txt").exists().then_some(())
    })
    .await;
    let answered = chat.effect_of(&allowed).await;
    assert!(matches!(answered.item.event.body(), Body::Approval(_)));
    let turn = chat.turn_for(&asked).await;
    chat.turn_at(&turn, State::Done).await;
    let words = world.sent_by_agent(&chat.hex, 9);
    assert!(!words.is_empty(), "the agent answered in words");
    // The first session item lists OpenClaw's commands and the models to choose from (D-029).
    let first_session = chat
        .items()
        .into_iter()
        .find_map(|(_, r)| match r.item.event.body() {
            Body::Session(s) => Some(s.clone()),
            _ => None,
        })
        .expect("a session item");
    assert!(
        !first_session.commands.is_empty(),
        "the session lists the commands"
    );
    let model = first_session.settings.iter().find(|s| s.id == "model");
    assert!(
        model
            .and_then(|m| m.options.as_ref())
            .is_some_and(|o| !o.is_empty()),
        "the session lists the models"
    );
    println!("approved and run, in {:.0?}", step.elapsed());

    // 2. A turn stopped while it waits for an approval, whose command then never runs.
    let step = Instant::now();
    let asked = chat.say(&format!("{only} touch never.txt")).await;
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
    println!("stopped, in {:.0?}", step.elapsed());

    // 3. A new session: the old one ends, and the next prompt opens another, even across a restart
    //    of the plugin in between.
    let step = Instant::now();
    let before = chat.sessions();
    let fresh = chat.ask(None, &Asked::NewSession, "New session").await;
    let ended = chat.effect_of(&fresh).await;
    assert!(matches!(ended.item.event.body(), Body::Session(_)));
    // The plugin restarts before the next prompt: the ended session must not come back.
    openclaw.kill().await;
    openclaw = OpenClaw::start(&folders, 2).await;
    let asked = chat.say("Reply with the word fresh.").await;
    let turn = chat.turn_for(&asked).await;
    let done = chat.turn_at(&turn, State::Done).await;
    let session = done.item.event.header().session.clone().unwrap();
    assert!(!before.contains(&session), "a new session");
    println!("new session, in {:.0?}", step.elapsed());

    // 4. The plugin's process crashes while a command runs. The core closes the turn the crash cut
    //    off; the next start carries on in the same session, with nothing run twice.
    let step = Instant::now();
    let runs = work.path().join("runs.txt");
    // One plain command: OpenClaw refuses to ask about a command with `&&` or `>>` in it, as it
    // cannot tie the approval to exactly what would run.
    std::fs::write(
        work.path().join("slow.sh"),
        "echo ran >> runs.txt\nsleep 30\n",
    )
    .unwrap();
    let command = "sh slow.sh";
    let asked = chat.say(&format!("{only} {command}")).await;
    chat.approve(command).await;
    within(TURN, || (lines(&runs) > 0).then_some(())).await;
    let cut_off = chat.turn_for(&asked).await;
    openclaw.kill().await;
    println!("crashed mid-turn");
    chat.turn_at(&cut_off, State::Cancelled).await;
    openclaw = OpenClaw::start(&folders, 3).await;
    let follow_up = chat
        .say("Without running anything, reply with only the shell command you ran last.")
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
    println!("carried on after the crash, in {:.0?}", step.elapsed());

    // 5. A file the agent names in its answer, in its workspace, reaches the chat as an attachment
    //    the person can read (D-090).
    let step = Instant::now();
    std::fs::write(work.path().join("report.txt"), "a report\n").unwrap();
    let asked = chat
        .say(&format!(
            "Without running anything, reply with only this line, exactly:\nMEDIA:{}",
            work.path().join("report.txt").display()
        ))
        .await;
    let turn = chat.turn_for(&asked).await;
    chat.turn_at(&turn, State::Done).await;
    let mut files = Vec::new();
    for _ in 0..TURN {
        files = files_sent(&world, &chat.group).await;
        if !files.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert_eq!(files.len(), 1, "one file sent");
    assert_eq!(files[0].0, "report.txt");
    assert_eq!(files[0].1, b"a report\n");
    println!("sent a file, in {:.0?}", step.elapsed());

    tracing.abort();
    let _ = stop.send(());
    serving.await.unwrap().unwrap();
    openclaw.kill().await;
    world.runtime.shutdown().await;
    println!("the scenario took {:.0?}", began.elapsed());
}
