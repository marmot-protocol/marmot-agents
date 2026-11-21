//! The core end to end through MDK: a person and the agent, two accounts on MDK's test relay, and
//! the core's loop between the agent's account and a scripted stand-in for a harness. What the
//! person sees is read back from the person's own storage.

use cgka_traits::GroupId;
use marmot_agent::PubKey;
use marmot_agent::carrier::{self, ITEM_KIND};
use marmot_agent::config::Config;
use marmot_agent::marmot::Marmot;
use marmot_agent::run::{Core, Store};
use marmot_agent_driver::{
    Answer, Capabilities, Control, Driver, Emitted, FilesIn, Harness, MidTurn, Prompt, Refusal,
    Sessions,
};
use marmot_agent_events::body::{Body, Message, StartedBy, Turn};
use marmot_agent_events::{AgentEvent, Header, State};
use marmot_app::{
    AccountSetupRequest, AppMessageQuery, AppMessageRecord, MarmotApp, MarmotAppRuntime,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

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

/// Waits, up to a time limit, for `found` to give something.
async fn eventually<T>(mut found: impl FnMut() -> Option<T>) -> T {
    for _ in 0..300 {
        if let Some(value) = found() {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("not within 30 seconds");
}

/// A person and an agent on MDK's test relay, in one runtime, and the core for the agent.
struct World {
    runtime: MarmotAppRuntime,
    marmot: Marmot,
    person: String,
    agent: String,
    config: Config,
    state: tempfile::TempDir,
    prompts: Arc<AtomicUsize>,
    _home: tempfile::TempDir,
    _relay: nostr_relay_builder::MockRelay,
}

/// A running core, and how to stop it.
struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl World {
    async fn new() -> Self {
        storage_sqlite::relax_storage_for_test_process();
        let home = tempfile::tempdir().unwrap();
        let relay = nostr_relay_builder::MockRelay::run().await.unwrap();
        let url = relay.url().await.to_string();
        let app = MarmotApp::with_relay(home.path(), url.clone());
        let runtime = MarmotAppRuntime::new(app.clone());
        let endpoint = cgka_traits::TransportEndpoint(url);
        let setup = AccountSetupRequest {
            default_relays: vec![endpoint.clone()],
            bootstrap_relays: vec![endpoint],
            publish_initial_key_package: true,
            ..AccountSetupRequest::default()
        };
        let agent = runtime
            .create_identity(setup.relay_options_only())
            .await
            .unwrap();
        let person = runtime.create_identity(setup).await.unwrap();
        let person_key = person.account.account_id_hex.clone();
        let settings = format!(
            "home = \"/unused\"\nrelays = []\nowner = \"{person_key}\"\n[codex]\ncwd = \"/unused\"\n"
        );
        let marmot = Marmot {
            runtime: runtime.clone(),
            app,
            label: agent.account.label.clone(),
            agent: PubKey(agent.account.account_id_hex.clone()),
        };
        Self {
            runtime,
            marmot,
            person: person_key,
            agent: agent.account.account_id_hex,
            config: Config::parse(&settings).unwrap(),
            state: tempfile::tempdir().unwrap(),
            prompts: Arc::new(AtomicUsize::new(0)),
            _home: home,
            _relay: relay,
        }
    }

    /// Starts the core, as a restart does: from the state it kept.
    fn start(&self) -> Running {
        let events = self.runtime.subscribe();
        let driver = Echo::new(self.prompts.clone());
        let store = Store::new(self.state.path());
        let core = Core::new(self.marmot.clone(), driver, &self.config, store).unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(core.run(events, async {
            let _ = stopped.await;
        }));
        Running { stop, task }
    }

    /// Waits until the agent has accepted the group.
    async fn joined(&self, group: &GroupId) -> String {
        let group_hex = hex::encode(group.as_slice());
        eventually(|| {
            let groups = self.marmot.groups();
            groups.iter().any(|g| g.as_str() == group_hex).then_some(())
        })
        .await;
        group_hex
    }

    /// What the agent sent to a group, of one kind, as the person has it.
    fn sent_by_agent(&self, group_hex: &str, kind: u64) -> Vec<AppMessageRecord> {
        let query = AppMessageQuery {
            group_id_hex: Some(group_hex.to_string()),
            kinds: Some(vec![kind]),
            limit: Some(50),
        };
        self.runtime
            .messages_with_query(&self.person, query)
            .unwrap()
            .into_iter()
            .filter(|m| m.sender == self.agent)
            .collect()
    }

    async fn words(&self, group_hex: &str, text: &str) -> AppMessageRecord {
        eventually(|| {
            let words = self.sent_by_agent(group_hex, 9);
            words.into_iter().find(|m| m.plaintext == text)
        })
        .await
    }
}

impl Running {
    async fn stop(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap();
    }
}

// A person who owns the agent invites it to a direct chat and writes to it. The core accepts the
// invitation, hands the message to the harness as a prompt, and the person receives the agent's
// words as a chat message and its turn as an item that names the message it answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_writes_to_the_agent_and_reads_its_answer() {
    let world = World::new().await;
    world.runtime.start().await.unwrap();
    let core = world.start();
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
        world
            .sent_by_agent(&group_hex, ITEM_KIND)
            .into_iter()
            .find_map(|m| {
                let read = carrier::read_item(&m.plaintext, &m.tags)?;
                (read.item.event.header().state == State::Done).then_some(read)
            })
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
    let world = World::new().await;
    world.runtime.start().await.unwrap();
    let core = world.start();
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
        world.prompts.load(Ordering::SeqCst),
        1,
        "the group message was not a prompt"
    );

    core.stop().await;
    let core = world.start();
    world
        .runtime
        .send_message(&world.person, &direct, b"second".to_vec())
        .await
        .unwrap();
    world.words(&direct_hex, "echo: second").await;
    assert_eq!(
        world.prompts.load(Ordering::SeqCst),
        2,
        "nothing read twice"
    );

    core.stop().await;
    world.runtime.shutdown().await;
}
