//! M3's scenario for the ACP driver, with real agents: through MDK's test relay, a person invites
//! the agent, has a command approved and run, interrupts a turn, starts a new session, and has the
//! core crash in the middle of a turn and carries on, with nothing run twice (`docs/plan.md`,
//! D-074). Then the agent sends a file from its working folder with the driver's `send_file` tool,
//! which the person downloads and reads (D-090). It is the Codex scenario, run against OpenCode,
//! Goose and Cursor.
//!
//! It approves only the exact command each step asks for, once, in a throwaway folder. Each needs
//! its agent, so it runs only when asked for:
//!
//! ```sh
//! # OpenCode, on the host, with a throwaway home: needs `opencode`, and OPENCODE_MODEL, the model as
//! # OpenCode names it, in the environment or in tests.env (see tests.env.example).
//! cargo test -p marmot-agent --test acp opencode -- --ignored --nocapture
//! # Goose, in a throwaway container: needs Docker, and a model: MODEL_BASE_URL, MODEL_NAME and
//! # MODEL_KEY_FILE, in the environment or in tests.env (see tests.env.example).
//! cargo test -p marmot-agent --test acp goose -- --ignored --nocapture
//! # Cursor, on the host's own sign-in: needs `cursor-agent`, signed in.
//! cargo test -p marmot-agent --test acp cursor -- --ignored --nocapture
//! ```

mod common;

use common::{Chat, TURN, TestModel, World, files_sent, lines, tests_env, within};
use marmot_agent::carrier::Asked;
use marmot_agent_acp::Acp;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use std::path::Path;

/// How the scenario starts one agent: its name, and the command that runs it, given the work
/// folder and a folder the agent may keep its own state in across the core's restart.
struct Agent {
    name: &'static str,
    command: fn(&Path, &Path) -> Vec<String>,
    /// Whether the agent can reach the driver's tool server, on the loopback address, to send a
    /// file (D-090), and if so how the title of its approval of the tool starts, when it asks: the
    /// tool as the agent names it, the server's name joined to the tool's.
    files: Option<&'static str>,
}

const OPENCODE: Agent = Agent {
    name: "opencode",
    command: |_work, home| {
        let home = home.display();
        // A throwaway home: none of the machine's settings, skills or sign-ins, and the model
        // tests.env names (D-100). OpenCode asks before every command and edit.
        let config = serde_json::json!({
            "model": tests_env("OPENCODE_MODEL"),
            "permission": {"bash": "ask", "edit": "ask"},
        });
        vec![
            "env".into(),
            format!("HOME={home}"),
            format!("XDG_CONFIG_HOME={home}/.config"),
            format!("XDG_DATA_HOME={home}/.local/share"),
            format!("XDG_STATE_HOME={home}/.local/state"),
            format!("XDG_CACHE_HOME={home}/.cache"),
            format!("OPENCODE_CONFIG_CONTENT={config}"),
            "OPENCODE_DISABLE_AUTOUPDATE=1".into(),
            "opencode".into(),
            "acp".into(),
        ]
    },
    // OpenCode asks nothing before the tool, with its default permissions.
    files: Some("marmot_send_file"),
};

const GOOSE: Agent = Agent {
    name: "goose",
    command: |_work, home| {
        let model = TestModel::read();
        let wrapper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/goose/goose-acp.sh");
        // In approve mode Goose asks before every tool call, which the scenario needs; Goose's own
        // default, auto, asks nothing.
        let mut command = vec!["env".into(), "GOOSE_MODE=approve".into()];
        command.extend(model.env_args());
        command.extend([
            wrapper.display().to_string(),
            model.key_file,
            home.display().to_string(),
        ]);
        command
    },
    // Goose takes the server, but its container has a network of its own: the host's loopback
    // address, where the driver serves it, is not reachable from inside.
    files: None,
};

const CURSOR: Agent = Agent {
    name: "cursor",
    // The host's own sign-in. Cursor asks before every command that is not on its allow-list.
    command: |_work, _home| vec!["cursor-agent".into(), "acp".into()],
    files: Some("marmot-send_file"),
};

async fn start(agent: &Agent, work: &Path, home: &Path) -> Acp {
    let command = (agent.command)(work, home);
    let mut acp = Acp::start(&command, &work.display().to_string(), agent.name)
        .await
        .expect("the agent starts");
    // File sending is switched on, as the settings do with `send_files = true`.
    if agent.files.is_some() {
        assert!(acp.serve_files().await.unwrap(), "the agent takes the tool");
    }
    acp
}

async fn scenario(agent: Agent) {
    let work = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let settings = format!(
        "[acp]\nname = \"{}\"\ncommand = [\"unused\"]\ncwd = \"{}\"\nsend_files = {}\n",
        agent.name,
        work.path().display(),
        agent.files.is_some()
    );
    let world = World::new(&settings).await;
    world.runtime.start().await.unwrap();
    let mut core = world.start(start(&agent, work.path(), home.path()).await);

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
    core = world.start(start(&agent, work.path(), home.path()).await);
    chat.turn_at(&cut_off, State::Cancelled).await;
    let follow_up = chat
        .say("Which shell command did you run last? Reply with only the command. Run nothing.")
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

    let Some(tool) = agent.files else {
        core.stop().await;
        world.runtime.shutdown().await;
        return;
    };

    // 5. A file from the working folder, sent with the driver's own tool, arrives as an
    //    attachment the person can read. Cursor asks before it calls any MCP tool, and Goose in
    //    approve mode before every tool: an approval of exactly that tool, in this turn, is
    //    allowed, once. Nothing else is.
    let asked = chat
        .say("Send the file runs.txt to the chat with your send_file tool, then reply with the word done.")
        .await;
    let mut allowed = false;
    for _ in 0..TURN {
        if !files_sent(&world, &chat.group).await.is_empty() {
            break;
        }
        let turn = chat.turns_for(&asked).into_iter().next();
        let waiting = turn.and_then(|t| chat.approval_waiting(&t, |title| title.starts_with(tool)));
        if !allowed && let Some((target, read)) = waiting {
            chat.allow_once(&target, &read).await;
            allowed = true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    // Counted once the turn is over, so a second file sent later in it is seen.
    let sent = chat.turn_for(&asked).await;
    chat.turn_at(&sent, State::Done).await;
    let files = files_sent(&world, &chat.group).await;
    assert_eq!(files.len(), 1, "one file sent");
    assert_eq!(files[0].0, "runs.txt");
    assert_eq!(files[0].1, b"ran\n");
    assert_eq!(
        files[0].2.as_deref(),
        Some(sent.as_str()),
        "tagged with its turn"
    );
    println!("sent a file");

    core.stop().await;
    world.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs opencode installed and a model"]
async fn opencode_end_to_end() {
    scenario(OPENCODE).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Docker and a model"]
async fn goose_end_to_end() {
    scenario(GOOSE).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs cursor-agent installed and signed in"]
async fn cursor_end_to_end() {
    scenario(CURSOR).await;
}
