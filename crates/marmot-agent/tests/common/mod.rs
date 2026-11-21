//! What the end-to-end tests share: a person and an agent on MDK's test relay, in one runtime,
//! and the core for the agent.

#![allow(dead_code)]

use cgka_traits::GroupId;
use marmot_agent::carrier::{self, Asked, ITEM_KIND, Read};
use marmot_agent::config::Config;
use marmot_agent::marmot::Marmot;
use marmot_agent::run::{Core, Store};
use marmot_agent::{EventKey, PubKey};
use marmot_agent_driver::Driver;
use marmot_agent_events::body::{Body, Effect};
use marmot_agent_events::{SessionId, State, TurnId};
use marmot_app::{
    AccountSetupRequest, AppMessageQuery, AppMessageRecord, MarmotApp, MarmotAppRuntime,
};
use std::path::Path;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Waits, up to `seconds`, for `found` to give something.
pub async fn within<T>(seconds: u64, mut found: impl FnMut() -> Option<T>) -> T {
    for _ in 0..seconds * 10 {
        if let Some(value) = found() {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("not within {seconds} seconds");
}

/// Waits, up to 30 seconds, for `found` to give something.
pub async fn eventually<T>(found: impl FnMut() -> Option<T>) -> T {
    within(30, found).await
}

/// A person and an agent on MDK's test relay, in one runtime.
pub struct World {
    pub runtime: MarmotAppRuntime,
    pub marmot: Marmot,
    pub person: String,
    pub agent: String,
    pub config: Config,
    pub state: tempfile::TempDir,
    /// The test relay's address.
    pub relay: String,
    _home: tempfile::TempDir,
    _relay: nostr_relay_builder::MockRelay,
}

/// A running core, and how to stop it.
pub struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl World {
    /// The person owns the agent; `settings` are added to the agent's settings.
    pub async fn new(settings: &str) -> Self {
        storage_sqlite::relax_storage_for_test_process();
        let home = tempfile::tempdir().unwrap();
        let relay = nostr_relay_builder::MockRelay::run().await.unwrap();
        let url = relay.url().await.to_string();
        let app = MarmotApp::with_relay(home.path(), url.clone());
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
        let person_key = person.account.account_id_hex.clone();
        let settings = format!(
            "home = \"/unused\"\nrelays = [\"{url}\"]\nowner = \"{person_key}\"\n{settings}"
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
            relay: url,
            _home: home,
            _relay: relay,
        }
    }

    /// Starts the core with `driver`, as a restart does: from the state it kept.
    pub fn start(&self, driver: impl Driver + 'static) -> Running {
        let events = self.runtime.subscribe();
        let store = Store::new(self.state.path());
        let core = Core::new(self.marmot.clone(), driver, &self.config, store).unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(core.run(events, async {
            let _ = stopped.await;
        }));
        Running { stop, task }
    }

    /// Waits until the agent has accepted the group, and gives its id as text.
    pub async fn joined(&self, group: &GroupId) -> String {
        let group_hex = hex::encode(group.as_slice());
        eventually(|| {
            let groups = self.marmot.groups();
            groups.iter().any(|g| g.as_str() == group_hex).then_some(())
        })
        .await;
        group_hex
    }

    /// What the agent sent to a group, of one kind, as the person has it, oldest first.
    pub fn sent_by_agent(&self, group_hex: &str, kind: u64) -> Vec<AppMessageRecord> {
        let query = AppMessageQuery {
            group_id_hex: Some(group_hex.to_string()),
            kinds: Some(vec![kind]),
            limit: Some(500),
        };
        self.runtime
            .messages_with_query(&self.person, query)
            .unwrap()
            .into_iter()
            .filter(|m| m.sender == self.agent)
            .collect()
    }

    /// Every version of every agent item in a group, read back, oldest first, each with the id
    /// of its own event.
    pub fn items(&self, group_hex: &str) -> Vec<(EventKey, carrier::Read)> {
        self.sent_by_agent(group_hex, ITEM_KIND)
            .into_iter()
            .filter_map(|m| {
                let read = carrier::read_item(&m.plaintext, &m.tags)?;
                Some((EventKey(m.message_id_hex), read))
            })
            .collect()
    }

    /// Waits for the agent's words with exactly `text`.
    pub async fn words(&self, group_hex: &str, text: &str) -> AppMessageRecord {
        eventually(|| {
            let words = self.sent_by_agent(group_hex, 9);
            words.into_iter().find(|m| m.plaintext == text)
        })
        .await
    }
}

impl Running {
    /// Stops the core and waits for it to finish.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap();
    }

    /// Ends the core as a crash does: at once, closing nothing and saving nothing more. Its
    /// harness goes with it.
    pub async fn crash(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

// -------------------------------------------------------------------- the scenarios' chat

/// How long a harness's turn may take, unless a scenario gives its chat a longer limit.
pub const TURN: u64 = 180;

/// The item's first event: what an answer or a control names.
pub fn first(event: &EventKey, read: &Read) -> EventKey {
    read.item.first.clone().unwrap_or_else(|| event.clone())
}

pub fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |text| text.lines().count())
}

/// A chat between the person and the agent, as the scenarios drive it.
pub struct Chat<'a> {
    pub world: &'a World,
    pub group: GroupId,
    pub hex: String,
    /// How long, in seconds, the agent may take to show what is waited for: normally `TURN`.
    pub turn: u64,
}

impl Chat<'_> {
    /// Sends a message, and gives back its event's id.
    pub async fn say(&self, text: &str) -> EventKey {
        let summary = self
            .world
            .runtime
            .send_message(&self.world.person, &self.group, text.as_bytes().to_vec())
            .await
            .unwrap();
        EventKey(summary.message_ids[0].clone())
    }

    /// Sends an answer or a control, and gives back its event's id.
    pub async fn ask(&self, target: Option<&EventKey>, asked: &Asked, words: &str) -> EventKey {
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
    pub fn items(&self) -> Vec<(EventKey, Read)> {
        self.world.items(&self.hex)
    }

    /// Each item's latest version, with its first event.
    pub fn latest(&self) -> Vec<(EventKey, Read)> {
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
    pub fn command_of(&self, item: &str) -> Option<String> {
        self.latest()
            .into_iter()
            .find_map(|(_, r)| match r.item.event.body() {
                Body::Tool(tool) if r.item.event.header().item.as_str() == item => {
                    tool.command.clone()
                }
                _ => None,
            })
    }

    /// Approves, once, the waiting approval for exactly `command`, and gives back the request's
    /// event id. An approval for anything else is never answered.
    pub async fn approve(&self, command: &str) -> EventKey {
        println!("waiting for the approval of `{command}`");
        let (target, read) = within(self.turn, || {
            self.latest().into_iter().find(|(_, r)| {
                let Body::Approval(approval) = r.item.event.body() else {
                    return false;
                };
                let asks_for = approval
                    .subject
                    .as_ref()
                    .and_then(|subject| self.command_of(subject.as_str()))
                    .unwrap_or_else(|| r.item.event.header().text.clone());
                // Exactly the command asked for: as typed, as Codex wraps it in a shell, or as
                // Cursor quotes it.
                let exact = asks_for == command
                    || asks_for.ends_with(&format!(" -lc '{command}'"))
                    || asks_for == format!("`{command}`");
                r.item.event.header().state == State::Waiting && exact
            })
        })
        .await;
        let Body::Approval(approval) = read.item.event.body() else {
            unreachable!()
        };
        // The choice that allows once: the one that says so, or else the first that allows, as
        // Codex and Claude Code put "Allow once" first. Goose puts its standing choice first.
        let allows = || {
            approval
                .choices
                .iter()
                .filter(|c| c.effect == Effect::Allow)
        };
        let once = allows()
            .find(|c| c.label.to_lowercase().contains("once"))
            .or_else(|| allows().next())
            .unwrap();
        let asked = Asked::Choice {
            choice: once.id.clone(),
        };
        self.ask(Some(&target), &asked, &once.label).await
    }

    /// Waits for a request to be settled by its effect, never by a notice saying why it had none
    /// (D-028), and gives back the version that shows it.
    pub async fn effect_of(&self, request: &EventKey) -> Read {
        let read = within(self.turn, || {
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
    pub fn turns_for(&self, prompt: &EventKey) -> Vec<TurnId> {
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
    pub async fn turn_at(&self, turn: &TurnId, state: State) -> Read {
        within(self.turn, || {
            self.latest().into_iter().map(|(_, r)| r).find(|r| {
                matches!(r.item.event.body(), Body::Turn(_))
                    && r.item.event.header().turn.as_ref() == Some(turn)
                    && r.item.event.header().state == state
            })
        })
        .await
    }

    /// Waits for the turn a message started, and gives its id.
    pub async fn turn_for(&self, prompt: &EventKey) -> TurnId {
        within(self.turn, || self.turns_for(prompt).into_iter().next()).await
    }

    pub fn sessions(&self) -> Vec<SessionId> {
        self.items()
            .into_iter()
            .filter(|(_, r)| matches!(r.item.event.body(), Body::Session(_)))
            .filter_map(|(_, r)| r.item.event.header().session.clone())
            .collect()
    }
}
