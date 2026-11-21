//! M2's scenario, with a real Claude Code: the Claude Code driver (`packages/claude-code`) connects
//! to the core's socket, and through MDK's test relay a person invites the agent, has a command
//! approved and run, interrupts a turn while it waits for an approval, starts a new session, and
//! has the driver crash in the middle of a turn and carries on, with nothing run twice
//! (`docs/plan.md`, D-074).
//!
//! It approves only the exact command each step asks for, once, in a throwaway folder. It needs
//! Node 24, `npm ci` run in `packages/claude-code`, and a signed-in Claude Code, so it runs only
//! when asked for:
//!
//! ```sh
//! cargo test -p marmot-agent --test claude -- --ignored --nocapture
//! ```

mod common;

use common::{Chat, TURN, World, lines, within};
use marmot_agent::carrier::Asked;
use marmot_agent::run::serve;
use marmot_agent::socket::Endpoint;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use std::path::Path;
use tokio::process::{Child, Command};

/// Starts the Claude Code driver for the agent whose home is `home`, working in `work`.
fn driver(home: &Path, work: &Path) -> Child {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/claude-code");
    Command::new("node")
        .arg("src/main.ts")
        .arg("--home")
        .arg(home)
        .arg("--cwd")
        .arg(work)
        .args(["--model", "haiku", "--permission-mode", "default"])
        .current_dir(package)
        .kill_on_drop(true)
        .spawn()
        .expect("node runs the driver")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Node, the driver's packages and a signed-in Claude Code"]
async fn claude_code_end_to_end() {
    let work = tempfile::tempdir().unwrap();
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
        serve(marmot, &config, &store, listener, token, stop).await
    });
    let mut claude = driver(home.path(), work.path());

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
    claude = driver(home.path(), work.path());
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
    claude = driver(home.path(), work.path());
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

    let _ = stop.send(());
    serving.await.unwrap().unwrap();
    let _ = claude.kill().await;
    world.runtime.shutdown().await;
}
