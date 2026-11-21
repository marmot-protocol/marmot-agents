//! The core end to end through MDK: a person and the agent, two accounts on MDK's test relay, and
//! the core's loop between the agent's account and a scripted stand-in for a harness. What the
//! person sees is read back from the person's own storage.

mod common;

use common::{World, eventually};
use marmot_agent_driver::{
    Answer, Capabilities, Control, Driver, Emitted, FilesIn, Harness, MidTurn, Prompt, Refusal,
    Sessions,
};
use marmot_agent_events::body::{Body, Message, StartedBy, Turn};
use marmot_agent_events::{AgentEvent, Header, State};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

/// A harness that answers every prompt with "echo: " and the prompt's text, in one turn, and
/// counts the prompts it was given.
struct Echo {
    events: mpsc::UnboundedReceiver<Emitted>,
    to_self: mpsc::UnboundedSender<Emitted>,
    prompts: Arc<AtomicUsize>,
    seq: u64,
}

impl Echo {
    fn new(prompts: Arc<AtomicUsize>) -> Self {
        let (to_self, events) = mpsc::unbounded_channel();
        Self {
            events,
            to_self,
            prompts,
            seq: 0,
        }
    }

    fn emit(&mut self, prompt: &Prompt, item: &str, state: State, body: Body, text: &str) {
        self.seq += 1;
        let header = Header {
            harness: "echo".into(),
            session: Some("s1".into()),
            turn: Some(format!("t{}", self.prompts.load(Ordering::SeqCst)).into()),
            item: format!("{item}{}", self.prompts.load(Ordering::SeqCst)).into(),
            seq: self.seq,
            state,
            title: item.into(),
            text: text.into(),
        };
        let _ = self.to_self.send(Emitted {
            event: AgentEvent::new(header, body, None).unwrap(),
            chat: Some(prompt.chat.id.clone()),
            prompts: vec![prompt.message.clone()],
            settles: Vec::new(),
        });
    }
}

impl Driver for Echo {
    fn harness(&self) -> Harness {
        Harness {
            name: "echo".into(),
            version: None,
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: false,
            mid_turn: MidTurn::Queue,
            sessions: Sessions::Core,
            resume: false,
            approvals: false,
            files_in: FilesIn::None,
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        let turn = || {
            Body::Turn(Turn {
                started_by: StartedBy::User,
                duration_ms: None,
                usage: None,
            })
        };
        self.emit(&prompt, "turn", State::Running, turn(), "Working");
        let words = format!("echo: {}", prompt.text);
        self.emit(
            &prompt,
            "words",
            State::Done,
            Body::Message(Message::default()),
            &words,
        );
        self.emit(&prompt, "turn", State::Done, turn(), "Done");
        Ok(())
    }

    async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
        Ok(())
    }

    async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
        Ok(())
    }

    async fn next_event(&mut self) -> Option<Emitted> {
        self.events.recv().await
    }
}

// A person who owns the agent invites it to a direct chat and writes to it. The core accepts the
// invitation, hands the message to the harness as a prompt, and the person receives the agent's
// words as a chat message and its turn as an item that names the message it answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_writes_to_the_agent_and_reads_its_answer() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let prompts = Arc::new(AtomicUsize::new(0));
    world.runtime.start().await.unwrap();
    let core = world.start(Echo::new(prompts.clone()));
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let group_hex = world.joined(&group).await;
    world
        .runtime
        .send_message(&world.person, &group, b"hello".to_vec())
        .await
        .unwrap();

    let words = world.words(&group_hex, "echo: hello").await;
    assert!(words.tags.iter().any(|t| t[0] == "turn"));
    let turn = eventually(|| {
        let items = world.items(&group_hex).into_iter().map(|(_, read)| read);
        items
            .filter(|read| matches!(read.item.event.body(), Body::Turn(_)))
            .find(|read| read.item.event.header().state == State::Done)
    })
    .await;
    assert_eq!(turn.item.version, 2);
    assert!(
        turn.item.first.is_some(),
        "the later version names the first"
    );
    assert!(turn.item.started_by.is_some(), "the turn names its message");

    core.stop().await;
    world.runtime.shutdown().await;
}

// In a named group of two the agent answers only when addressed, as in any group (D-075); and a
// restart reads nothing again that the core had read before it stopped (D-037).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_group_of_two_is_a_group_and_a_restart_repeats_nothing() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let prompts = Arc::new(AtomicUsize::new(0));
    world.runtime.start().await.unwrap();
    let core = world.start(Echo::new(prompts.clone()));
    let named = world
        .runtime
        .create_group(
            &world.person,
            "Team",
            std::slice::from_ref(&world.agent),
            None,
        )
        .await
        .unwrap()
        .operation
        .expect("the group is made");
    world.joined(&named).await;
    let direct = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let direct_hex = world.joined(&direct).await;

    world
        .runtime
        .send_message(&world.person, &named, b"lunch, anyone?".to_vec())
        .await
        .unwrap();
    world
        .runtime
        .send_message(&world.person, &direct, b"first".to_vec())
        .await
        .unwrap();
    world.words(&direct_hex, "echo: first").await;
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "the group message was not a prompt"
    );

    core.stop().await;
    let core = world.start(Echo::new(prompts.clone()));
    world
        .runtime
        .send_message(&world.person, &direct, b"second".to_vec())
        .await
        .unwrap();
    world.words(&direct_hex, "echo: second").await;
    assert_eq!(prompts.load(Ordering::SeqCst), 2, "nothing read twice");

    core.stop().await;
    world.runtime.shutdown().await;
}
