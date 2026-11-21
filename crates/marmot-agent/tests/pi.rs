//! M3's scenario for Pi, with a real Pi in a container: through MDK's test relay, a person invites
//! the agent, allows a command an extension asks about, interrupts a turn, starts a new session,
//! and has the core crash in the middle of a turn and carries on, with nothing run twice
//! (`docs/plan.md`, D-074). The crash takes Pi's container with it.
//!
//! Pi asks nobody before it acts, so it has no approvals. It runs here with the permission gate
//! Pi ships as an example extension, which asks through Pi's extension UI before a dangerous
//! command: that question comes to the chat, and the person's answer goes back to Pi.
//!
//! Pi runs through `tools/pi/pi.sh`, in a throwaway container, in a throwaway folder. It needs
//! Docker, and a file holding a key for the model endpoint the script uses, named by
//! `PI_MODEL_KEY_FILE`. So it runs only when asked for:
//!
//! ```sh
//! PI_MODEL_KEY_FILE=<key file> cargo test -p marmot-agent --test pi -- --ignored --nocapture
//! ```

mod common;

use common::{Chat, TURN, World, lines, within};
use marmot_agent::EventKey;
use marmot_agent::carrier::{Asked, AskedReply, Read};
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use marmot_agent_pi::Pi;
use std::path::Path;
use std::time::Instant;

/// The example extension Pi ships that asks before a dangerous command, as installed in the image.
const GATE: &str = "/usr/local/lib/node_modules/@earendil-works/pi-coding-agent/examples/extensions/permission-gate.ts";

async fn pi(world: &World) -> Pi {
    let setup = world.config.pi_setup().expect("a [pi] section");
    Pi::start(setup).await.expect("pi starts")
}

/// The containers running with `work` mounted: the chat's Pi, while it runs.
fn containers(work: &Path) -> Vec<String> {
    let output = std::process::Command::new("docker")
        .args(["ps", "-q", "--filter"])
        .arg(format!("volume={}", work.display()))
        .output()
        .expect("docker runs");
    assert!(output.status.success(), "docker ps works");
    (String::from_utf8_lossy(&output.stdout).split_whitespace())
        .map(String::from)
        .collect()
}

/// Answers "Yes", once, to the waiting question about exactly `command`, and gives back the
/// answer's event id.
async fn allow(chat: &Chat<'_>, command: &str) -> EventKey {
    println!("waiting for the question about `{command}`");
    let (target, read): (EventKey, Read) = within(TURN, || {
        chat.latest().into_iter().find(|(_, r)| {
            let header = r.item.event.header();
            matches!(r.item.event.body(), Body::Question(_))
                && header.state == State::Waiting
                && header.text.contains(command)
        })
    })
    .await;
    let Body::Question(question) = read.item.event.body() else {
        unreachable!()
    };
    let asked = &question.questions()[0];
    let yes = asked.options.iter().find(|o| o.label == "Yes").unwrap();
    let answer = Asked::Questions {
        questions: vec![AskedReply {
            question: asked.id.clone(),
            options: vec![yes.id.clone()],
            text: None,
        }],
    };
    chat.ask(Some(&target), &answer, "Yes").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Docker and a model key"]
async fn pi_end_to_end() {
    let began = Instant::now();
    let key = std::env::var("PI_MODEL_KEY_FILE").expect("PI_MODEL_KEY_FILE names the model key");
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/pi/pi.sh");
    let settings = format!(
        "name = \"Pi\"\n[pi]\ncwd = \"{}\"\nprogram = \"{}\"\nargs = [\"{key}\", \"{}\", \"--extension\", \"{GATE}\"]\n",
        work.path().display(),
        script.display(),
        state.path().display(),
    );
    let world = World::new(&settings).await;
    world.runtime.start().await.unwrap();
    let mut core = world.start(pi(&world).await);

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
    let lap = |what: &str| println!("{what} after {:.0} s", began.elapsed().as_secs_f64());

    // 1. A command the extension asks about, allowed once and run.
    let asked = chat
        .say(&format!("{only} `rm -rf scratch && touch marker.txt`"))
        .await;
    let allowed = allow(&chat, "rm -rf scratch && touch marker.txt").await;
    within(TURN, || {
        work.path().join("marker.txt").exists().then_some(())
    })
    .await;
    let answered = chat.effect_of(&allowed).await;
    assert!(matches!(answered.item.event.body(), Body::Question(_)));
    let turn = chat.turn_for(&asked).await;
    chat.turn_at(&turn, State::Done).await;
    lap("asked, allowed and run");

    // 2. A turn interrupted once its command has started.
    let asked = chat
        .say(&format!("{only} `echo started >> started.txt && sleep 30`"))
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
    lap("interrupted");

    // 3. A new session.
    let before = chat.sessions();
    let fresh = chat.ask(None, &Asked::NewSession, "New session").await;
    let started = chat.effect_of(&fresh).await;
    assert!(matches!(started.item.event.body(), Body::Session(_)));
    let session = started.item.event.header().session.clone().unwrap();
    assert!(!before.contains(&session), "a new session");
    lap("new session");

    // 4. The core crashes while a command runs. The next core closes the turn the crash cut off,
    //    and the chat carries on in the same session with nothing run twice.
    let runs = work.path().join("runs.txt");
    let asked = chat
        .say(&format!("{only} `echo ran >> runs.txt && sleep 20`"))
        .await;
    within(TURN, || (lines(&runs) > 0).then_some(())).await;
    let cut_off = chat.turn_for(&asked).await;
    assert_eq!(containers(work.path()).len(), 1, "one Pi, for the one chat");
    core.crash().await;
    // Pi's container goes with the core, and the command it was running with it.
    within(15, || containers(work.path()).is_empty().then_some(())).await;
    lap("crashed mid-turn");
    core = world.start(pi(&world).await);
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
    lap("carried on after the crash");

    core.stop().await;
    world.runtime.shutdown().await;
}
