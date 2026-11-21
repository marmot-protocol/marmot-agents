//! M1's scenario, with a real Codex: through MDK's test relay, a person invites the agent, has a
//! command approved and run, interrupts a turn, starts a new session, and restarts the core in the
//! middle of a turn and carries on, with nothing run twice (`docs/plan.md`, D-074).
//!
//! It needs `codex` installed and signed in, so it runs only when asked for:
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
use marmot_agent_events::{SessionId, State};

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

struct Chat<'a> {
    world: &'a World,
    group: GroupId,
    hex: String,
}

impl Chat<'_> {
    async fn say(&self, text: &str) {
        self.world
            .runtime
            .send_message(&self.world.person, &self.group, text.as_bytes().to_vec())
            .await
            .unwrap();
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

    fn items(&self) -> Vec<(EventKey, Read)> {
        self.world.items(&self.hex)
    }

    /// Waits for an item version that `wanted` picks.
    async fn item(&self, what: &str, wanted: impl Fn(&Read) -> bool) -> (EventKey, Read) {
        println!("waiting for {what}");
        within(TURN, || self.items().into_iter().find(|(_, r)| wanted(r))).await
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

    /// Approves the next approval that waits, and gives back the request's event id.
    async fn approve(&self) -> EventKey {
        println!("waiting for an approval");
        let (target, read) = within(TURN, || {
            self.latest().into_iter().find(|(_, r)| {
                matches!(r.item.event.body(), Body::Approval(_))
                    && r.item.event.header().state == State::Waiting
            })
        })
        .await;
        let Body::Approval(approval) = read.item.event.body() else {
            unreachable!()
        };
        let allow = approval
            .choices
            .iter()
            .find(|c| c.effect == Effect::Allow)
            .unwrap();
        let asked = Asked::Choice {
            choice: allow.id.clone(),
        };
        self.ask(Some(&target), &asked, &allow.label).await
    }

    /// The item version that settled a request, once one has.
    fn settled_by(&self, request: &EventKey) -> Option<Read> {
        self.items()
            .into_iter()
            .map(|(_, r)| r)
            .find(|r| r.item.settles.contains(request))
    }

    /// Waits for a request to be settled by its effect, never by a notice saying why it had none
    /// (D-028), and gives back the version that shows it.
    async fn effect_of(&self, request: &EventKey) -> Read {
        let read = within(TURN, || self.settled_by(request)).await;
        assert!(
            !matches!(read.item.event.body(), Body::Notice(_)),
            "refused: {}",
            read.item.event.header().text
        );
        read
    }

    fn sessions(&self) -> Vec<SessionId> {
        let mut sessions: Vec<SessionId> = self
            .items()
            .into_iter()
            .filter(|(_, r)| matches!(r.item.event.body(), Body::Session(_)))
            .filter_map(|(_, r)| r.item.event.header().session.clone())
            .collect();
        sessions.dedup();
        sessions
    }

    /// The commands run, one per item, that contain `text`.
    fn commands(&self, text: &str) -> usize {
        let mut ids: Vec<String> = self
            .items()
            .into_iter()
            .filter(|(_, r)| {
                matches!(r.item.event.body(), Body::Tool(t)
                if t.command.as_deref().is_some_and(|c| c.contains(text)))
            })
            .map(|(_, r)| r.item.event.header().item.as_str().to_string())
            .collect();
        ids.sort();
        ids.dedup();
        ids.len()
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

    // 1. A command that needs approval, approved and run.
    chat.say("Run exactly this shell command and nothing else: `touch marker.txt`. Then reply with the word done.")
        .await;
    let allowed = chat.approve().await;
    within(TURN, || {
        work.path().join("marker.txt").exists().then_some(())
    })
    .await;
    let answered = chat.effect_of(&allowed).await;
    assert!(matches!(answered.item.event.body(), Body::Approval(_)));
    println!("approved and run");

    // 2. A turn interrupted while its command runs.
    chat.say("Run exactly this shell command and nothing else: `sleep 30`.")
        .await;
    let _ = chat.approve().await;
    chat.item("the sleep running", |r| {
        matches!(r.item.event.body(), Body::Tool(t) if t.command.as_deref().is_some_and(|c| c.contains("sleep 30")))
    })
    .await;
    let interrupt = chat.ask(None, &Asked::Interrupt, "Stop").await;
    let stopped = chat.effect_of(&interrupt).await;
    assert!(matches!(stopped.item.event.body(), Body::Turn(_)));
    assert_ne!(stopped.item.event.header().state, State::Done);
    println!("interrupted");

    // 3. A new session.
    let before = chat.sessions();
    let fresh = chat.ask(None, &Asked::NewSession, "New session").await;
    let started = chat.effect_of(&fresh).await;
    assert!(matches!(started.item.event.body(), Body::Session(_)));
    let session = started.item.event.header().session.clone().unwrap();
    assert!(!before.contains(&session), "a new session");
    println!("new session");

    // 4. The core restarted while a command runs, and the chat carries on in the same session.
    chat.say("Run exactly this shell command and nothing else: `sleep 20`.")
        .await;
    let _ = chat.approve().await;
    let (_, sleeping) = chat
        .item("the second sleep running", |r| {
        matches!(r.item.event.body(), Body::Tool(t) if t.command.as_deref().is_some_and(|c| c.contains("sleep 20")))
    })
        .await;
    let cut_off = sleeping.item.event.header().turn.clone().unwrap();
    core.stop().await;
    println!("stopped mid-turn");
    core = world.start(codex(&world).await);
    chat.say("Which shell command did you run last? Reply with only the command.")
        .await;
    chat.item("an answer after the restart", |r| {
        matches!(r.item.event.body(), Body::Turn(_))
            && r.item.event.header().state == State::Done
            && r.item.event.header().session.as_ref() == Some(&session)
            && r.item.version > 1
    })
    .await;
    assert_eq!(chat.commands("sleep 20"), 1, "nothing ran twice");
    let closed = chat.latest().into_iter().any(|(_, r)| {
        matches!(r.item.event.body(), Body::Turn(_))
            && r.item.event.header().turn.as_ref() == Some(&cut_off)
            && r.item.event.header().state == State::Cancelled
    });
    assert!(closed, "the turn the restart cut off shows as interrupted");
    println!("carried on after the restart");

    core.stop().await;
    world.runtime.shutdown().await;
}
