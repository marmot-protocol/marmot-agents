//! M1's scenario, with a real Codex: through MDK's test relay, a person invites the agent, has a
//! command approved and run, interrupts a turn, starts a new session, and has the core crash in
//! the middle of a turn and carries on, with nothing run twice (`docs/plan.md`, D-074).
//!
//! It approves only the exact command each step asks for, once, in a throwaway folder. It needs
//! `codex` installed and signed in, so it runs only when asked for:
//!
//! ```sh
//! cargo test -p marmot-agent --test codex -- --ignored --nocapture
//! ```

mod common;

use common::{Chat, TURN, World, lines, within};
use marmot_agent::carrier::Asked;
use marmot_agent_codex::Codex;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;

async fn codex(world: &World) -> Codex {
    let setup = world.config.setup().unwrap();
    Codex::start("codex", setup).await.expect("codex starts")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs codex installed and signed in"]
async fn codex_end_to_end() {
    let work = tempfile::tempdir().unwrap();
    let settings = format!(
        "[codex]\ncwd = \"{}\"\napproval_policy = \"untrusted\"\n",
        work.path().display()
    );
    let world = World::new(&settings).await;
    world.runtime.start().await.unwrap();
    let mut core = world.start(codex(&world).await);

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
    chat.approve("sleep 30").await;
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
    chat.approve("sleep 20").await;
    within(TURN, || (lines(&runs) > 0).then_some(())).await;
    let cut_off = chat.turn_for(&asked).await;
    core.crash().await;
    println!("crashed mid-turn");
    core = world.start(codex(&world).await);
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

    core.stop().await;
    world.runtime.shutdown().await;
}
