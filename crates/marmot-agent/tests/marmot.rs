//! The core end to end through MDK: a person and the agent, two accounts on MDK's test relay, and
//! the core's loop between the agent's account and a scripted stand-in for a harness. What the
//! person sees is read back from the person's own storage.

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
use marmot_app::{AccountSetupRequest, AppMessageQuery, MarmotApp, MarmotAppRuntime};
use std::time::Duration;
use tokio::sync::mpsc;

/// A harness that answers every prompt with "echo: " and the prompt's text, in one turn.
struct Echo {
    events: mpsc::UnboundedReceiver<Emitted>,
    to_self: mpsc::UnboundedSender<Emitted>,
    seq: u64,
}

impl Echo {
    fn new() -> Self {
        let (to_self, events) = mpsc::unbounded_channel();
        Self {
            events,
            to_self,
            seq: 0,
        }
    }

    fn emit(&mut self, prompt: &Prompt, item: &str, state: State, body: Body, text: &str) {
        self.seq += 1;
        let header = Header {
            harness: "echo".into(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            item: item.into(),
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
        let turn = || {
            Body::Turn(Turn {
                started_by: StartedBy::User,
                duration_ms: None,
                usage: None,
            })
        };
        self.emit(&prompt, "turn", State::Running, turn(), "Working");
        let words = format!("echo: {}", prompt.text);
        let message = Body::Message(Message::default());
        self.emit(&prompt, "words", State::Done, message, &words);
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

// A person who owns the agent invites it to a direct chat and writes to it. The core accepts the
// invitation, hands the message to the harness as a prompt, and the person receives the agent's
// words as a chat message and its turn as an item that names the message it answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_writes_to_the_agent_and_reads_its_answer() {
    storage_sqlite::relax_storage_for_test_process();
    let dir = tempfile::tempdir().unwrap();
    let relay = nostr_relay_builder::MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let app = MarmotApp::with_relay(dir.path(), url.clone());
    let runtime = MarmotAppRuntime::new(app.clone());
    let endpoint = cgka_traits::TransportEndpoint(url.clone());
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
    let agent_key = agent.account.account_id_hex.clone();
    let person_key = person.account.account_id_hex.clone();

    let settings = format!(
        "home = \"/unused\"\nrelays = []\nowner = \"{person_key}\"\n[codex]\ncwd = \"/unused\"\n"
    );
    let config = Config::parse(&settings).unwrap();
    let events = runtime.subscribe();
    runtime.start().await.unwrap();
    let marmot = Marmot {
        runtime: runtime.clone(),
        app: app.clone(),
        label: agent.account.label.clone(),
        agent: PubKey(agent_key.clone()),
    };
    let state = tempfile::tempdir().unwrap();
    let core = Core::new(
        marmot.clone(),
        Echo::new(),
        &config,
        Store::new(state.path()),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let running = tokio::spawn(core.run(events, async {
        let _ = stopped.await;
    }));

    let group = runtime
        .start_direct_conversation(&person_key, &agent_key)
        .await
        .unwrap();
    let group_hex = hex::encode(group.as_slice());
    eventually(|| {
        marmot
            .groups()
            .iter()
            .any(|g| g.as_str() == group_hex)
            .then_some(())
    })
    .await;

    runtime
        .send_message(&person_key, &group, b"hello".to_vec())
        .await
        .unwrap();

    let seen = |kind: u64| {
        let query = AppMessageQuery {
            group_id_hex: Some(group_hex.clone()),
            kinds: Some(vec![kind]),
            limit: Some(50),
        };
        runtime
            .messages_with_query(&person_key, query)
            .unwrap()
            .into_iter()
            .filter(|m| m.sender == agent_key)
            .collect::<Vec<_>>()
    };
    let words = eventually(|| seen(9).into_iter().find(|m| m.plaintext == "echo: hello")).await;
    assert!(
        words
            .tags
            .iter()
            .any(|t| t == &vec!["turn".to_string(), "t1".to_string()])
    );
    let turn = eventually(|| {
        seen(ITEM_KIND).into_iter().find_map(|m| {
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
    assert!(
        turn.item.started_by.is_some(),
        "the turn names the message it answered"
    );

    let _ = stop.send(());
    running.await.unwrap();
    runtime.shutdown().await;
}
