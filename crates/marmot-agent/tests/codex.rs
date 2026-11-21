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

use cgka_traits::GroupId;
use common::{World, within};
use marmot_agent::EventKey;
use marmot_agent::carrier::{self, Asked, Read};
use marmot_agent_codex::Codex;
use marmot_agent_events::body::{Body, Effect};
use marmot_agent_events::{SessionId, State, TurnId};
use std::path::Path;

/// How long a Codex turn may take.
const TURN: u64 = 180;

async fn codex(world: &World) -> Codex {
    let setup = world.config.setup().unwrap();
    Codex::start("codex", setup).await.expect("codex starts")
}

/// The item's first event: what an answer or a control names.
fn first(event: &EventKey, read: &Read) -> EventKey {
    read.item.first.clone().unwrap_or_else(|| event.clone())
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |text| text.lines().count())
}

struct Chat<'a> {
    world: &'a World,
    group: GroupId,
    hex: String,
}

impl Chat<'_> {
    /// Sends a message, and gives back its event's id.
    async fn say(&self, text: &str) -> EventKey {
        let summary = self
            .world
            .runtime
            .send_message(&self.world.person, &self.group, text.as_bytes().to_vec())
            .await
            .unwrap();
        EventKey(summary.message_ids[0].clone())
    }

    /// Sends an answer or a control, and gives back its event's id.
    async fn ask(&self, target: Option<&EventKey>, asked: &Asked, words: &str) -> EventKey {
        let agent = marmot_agent::PubKey(self.world.agent.clone());
        let out = carrier::request(&agent, target, asked, words);
        let summary = self
            .world
            .runtime
            .send_custom_event(
                &self.world.person,
                &self.group,
                out.kind,
                out.tags,
                out.content,
            )
            .await
            .unwrap();
        EventKey(summary.message_ids[0].clone())
    }

    /// Every version of every item, oldest first, each with its own event's id.
    fn items(&self) -> Vec<(EventKey, Read)> {
        self.world.items(&self.hex)
    }

    /// Each item's latest version, with its first event.
    fn latest(&self) -> Vec<(EventKey, Read)> {
        let mut latest: Vec<(EventKey, Read)> = Vec::new();
        for (event, read) in self.items() {
            let key = first(&event, &read);
            match latest.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) if slot.1.item.version < read.item.version => slot.1 = read,
                Some(_) => {}
                None => latest.push((key, read)),
            }
        }
        latest
    }

    /// The command a tool item ran, by the item's id.
    fn command_of(&self, item: &str) -> Option<String> {
        self.latest()
            .into_iter()
            .find_map(|(_, r)| match r.item.event.body() {
                Body::Tool(tool) if r.item.event.header().item.as_str() == item => {
                    tool.command.clone()
                }
                _ => None,
            })
    }

    /// Approves, once, the waiting approval for a command containing `command`, and gives back
    /// the request's event id. An approval for anything else is never answered.
    async fn approve(&self, command: &str) -> EventKey {
        println!("waiting for the approval of `{command}`");
        let (target, read) = within(TURN, || {
            self.latest().into_iter().find(|(_, r)| {
                let Body::Approval(approval) = r.item.event.body() else {
                    return false;
                };
                let asks_for = approval
                    .subject
                    .as_ref()
                    .and_then(|subject| self.command_of(subject.as_str()))
                    .unwrap_or_else(|| r.item.event.header().text.clone());
                r.item.event.header().state == State::Waiting && asks_for.contains(command)
            })
        })
        .await;
        let Body::Approval(approval) = read.item.event.body() else {
            unreachable!()
        };
        // The first choice that allows is "Allow once"; a standing one comes after it.
        let once = approval
            .choices
            .iter()
            .find(|c| c.effect == Effect::Allow)
            .unwrap();
        let asked = Asked::Choice {
            choice: once.id.clone(),
        };
        self.ask(Some(&target), &asked, &once.label).await
    }

    /// Waits for a request to be settled by its effect, never by a notice saying why it had none
    /// (D-028), and gives back the version that shows it.
    async fn effect_of(&self, request: &EventKey) -> Read {
        let read = within(TURN, || {
            let items = self.items().into_iter().map(|(_, r)| r);
            items.into_iter().find(|r| r.item.settles.contains(request))
        })
        .await;
        assert!(
            !matches!(read.item.event.body(), Body::Notice(_)),
            "refused: {}",
            read.item.event.header().text
        );
        read
    }

    /// The turns a message started, by turn id.
    fn turns_for(&self, prompt: &EventKey) -> Vec<TurnId> {
        let mut turns: Vec<TurnId> = self
            .items()
            .into_iter()
            .filter(|(_, r)| matches!(r.item.event.body(), Body::Turn(_)))
            .filter(|(_, r)| r.item.started_by.as_ref() == Some(prompt))
            .filter_map(|(_, r)| r.item.event.header().turn.clone())
            .collect();
        turns.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        turns.dedup();
        turns
    }

    /// Waits for a turn's latest version to stand at `state`, and gives it back.
    async fn turn_at(&self, turn: &TurnId, state: State) -> Read {
        within(TURN, || {
            self.latest().into_iter().map(|(_, r)| r).find(|r| {
                matches!(r.item.event.body(), Body::Turn(_))
                    && r.item.event.header().turn.as_ref() == Some(turn)
                    && r.item.event.header().state == state
            })
        })
        .await
    }

    /// Waits for the turn a message started, and gives its id.
    async fn turn_for(&self, prompt: &EventKey) -> TurnId {
        within(TURN, || self.turns_for(prompt).into_iter().next()).await
    }

    fn sessions(&self) -> Vec<SessionId> {
        self.items()
            .into_iter()
            .filter(|(_, r)| matches!(r.item.event.body(), Body::Session(_)))
            .filter_map(|(_, r)| r.item.event.header().session.clone())
            .collect()
    }
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
