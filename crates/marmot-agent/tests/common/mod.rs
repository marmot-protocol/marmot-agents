//! What the end-to-end tests share: a person and an agent on MDK's test relay, in one runtime,
//! and the core for the agent.

#![allow(dead_code)]

use cgka_traits::GroupId;
use marmot_agent::carrier::{self, ITEM_KIND};
use marmot_agent::config::Config;
use marmot_agent::marmot::Marmot;
use marmot_agent::run::{Core, Store};
use marmot_agent::{EventKey, PubKey};
use marmot_agent_driver::Driver;
use marmot_app::{
    AccountSetupRequest, AppMessageQuery, AppMessageRecord, MarmotApp, MarmotAppRuntime,
};
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
        let settings =
            format!("home = \"/unused\"\nrelays = []\nowner = \"{person_key}\"\n{settings}");
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
}
