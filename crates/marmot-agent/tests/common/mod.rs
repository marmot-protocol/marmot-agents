//! What the end-to-end tests share: a person and an agent on MDK's test relay, in one runtime,
//! and the core for the agent.

#![allow(dead_code)]

use cgka_traits::GroupId;
use marmot_agent::carrier::{self, Asked, ITEM_KIND, Read};
use marmot_agent::config::Config;
use marmot_agent::marmot::{self, Marmot, Subscribed};
use marmot_agent::run::{Core, Store};
use marmot_agent::{EventKey, PubKey};
use marmot_agent_driver::{
    Answer, Capabilities, Control, Driver, Emitted, FilesIn, Harness, MidTurn, Output, Prompt,
    Refusal, Sessions,
};
use marmot_agent_events::body::{Body, Effect, Message, StartedBy, Turn};
use marmot_agent_events::{AgentEvent, Header, SessionId, State, TurnId};
use marmot_app::{
    AccountSetupRequest, AppMessageQuery, AppMessageRecord, MarmotApp, MarmotAppRuntime,
};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// MDK's live events, subscribed to, as the core runs on them.
pub type Live = Subscribed<tokio::sync::broadcast::Receiver<marmot_app::MarmotAppEvent>>;

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

/// A harness that answers every prompt with "echo: " and the prompt's text, in one turn, and
/// counts the prompts it was given.
pub struct Echo {
    events: mpsc::UnboundedReceiver<Emitted>,
    to_self: mpsc::UnboundedSender<Emitted>,
    prompts: Arc<AtomicUsize>,
    seq: u64,
}

impl Echo {
    pub fn new(prompts: Arc<AtomicUsize>) -> Self {
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
        // Only a turn names the prompts it responds to.
        let prompts = if matches!(body, Body::Turn(_)) {
            vec![prompt.message.clone()]
        } else {
            Vec::new()
        };
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
            prompts,
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

    async fn next_event(&mut self) -> Option<Output> {
        self.events.recv().await.map(Output::from)
    }
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
    _relay: nostr_relay_builder::LocalRelay,
}

/// A running core, and how to stop it.
pub struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), String>>,
}

impl World {
    /// The person owns the agent; `settings` are added to the agent's settings.
    pub async fn new(settings: &str) -> Self {
        Self::with_slow_files(settings, std::time::Duration::ZERO).await
    }

    /// As [`World::new`], with a Blossom server that takes `delay` to accept each file.
    pub async fn with_slow_files(settings: &str, delay: std::time::Duration) -> Self {
        storage_sqlite::relax_storage_for_test_process();
        let home = tempfile::tempdir().unwrap();
        let relay = relay().await;
        let url = relay.url().await.to_string();
        // As `MarmotApp::with_relay`, with a stand-in Blossom server on this machine as where a
        // new group keeps its files, as a deployment names its own; the agent's files go there
        // as they would in use.
        let mut deployment = marmot_app::MarmotDeployment::white_noise();
        deployment.new_group_media_endpoints = vec![blossom(delay).await];
        let app_settings = marmot_app::MarmotAppConfig::white_noise()
            .with_deployment(deployment)
            .with_allow_loopback_relay_endpoints(true)
            .with_open_ranking_provider(None, Vec::new())
            .with_allow_loopback_blob_endpoints(true);
        let app = MarmotApp::with_relay_and_config(home.path(), url.clone(), app_settings);
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
            faults: Default::default(),
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
        self.start_with(driver, Store::new(self.state.path()))
    }

    /// As [`World::start`], keeping the state in `store`.
    pub fn start_with(&self, driver: impl Driver + 'static, store: Store) -> Running {
        self.run(self.core(driver, store))
    }

    /// The core for the agent with `driver`, keeping its state in `store`, not yet running.
    pub fn core<D: Driver>(&self, driver: D, store: Store) -> Core<D> {
        Core::new(self.marmot.clone(), driver, &self.config, store).unwrap()
    }

    /// Runs `core`, as [`World::start`] does.
    pub fn run<D: Driver + 'static>(&self, core: Core<D>) -> Running {
        self.run_on(core, marmot::subscribe(&self.runtime))
    }

    /// As [`World::start`], on events subscribed to earlier, as when the harness takes a while to
    /// start: what MDK emitted since reaches the core live, as an event emitted after the core
    /// started does, even for a message MDK stored before.
    pub fn start_on(&self, driver: impl Driver + 'static, live: Live) -> Running {
        self.run_on(self.core(driver, Store::new(self.state.path())), live)
    }

    fn run_on<D: Driver + 'static>(&self, core: Core<D>, live: Live) -> Running {
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            core.run(live, async {
                let _ = stopped.await;
            })
            .await
        });
        Running { stop, task }
    }

    /// Sends something to a group under `token` as the core does, asking MDK again while it has
    /// the send queued, as the core does through its retries, for up to 30 seconds.
    pub async fn send(
        &self,
        group: &marmot_agent::GroupKey,
        what: &marmot_agent::Sending,
        token: &marmot_agent::Token,
    ) -> Result<EventKey, marmot_agent::marmot::SendFailed> {
        for _ in 0..3000 {
            match self.marmot.send(group, what, token).await {
                Err(marmot_agent::marmot::SendFailed::Pending) => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                done => return done,
            }
        }
        panic!("MDK kept the send queued for 30 seconds");
    }

    /// Waits until the agent has accepted the group, and gives its id as text.
    pub async fn joined(&self, group: &GroupId) -> String {
        let group_hex = hex::encode(group.as_slice());
        eventually(|| {
            let groups = self.marmot.groups().unwrap_or_default();
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
    /// Stops the core and waits for it to finish, which it must do without an error.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap().expect("the core stops cleanly");
    }

    /// Waits for a core that stopped by itself, and gives what it ended with. A panic fails.
    pub async fn ended(self) -> Result<(), String> {
        self.task.await.expect("the core does not panic")
    }

    /// Whether the core stops by itself within `seconds`.
    pub async fn stops_within(&self, seconds: u64) -> bool {
        for _ in 0..seconds * 10 {
            if self.task.is_finished() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// Ends the core as a crash does: at once, closing nothing and saving nothing more. Its
    /// harness goes with it.
    pub async fn crash(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

/// MDK's test relay, without the limit it puts by default on how many events each connection may
/// send a minute (60), as MDK's own harness relay does: a long scenario would otherwise stall on it.
pub async fn relay() -> nostr_relay_builder::LocalRelay {
    use nostr_relay_builder::builder::RateLimit;
    let unlimited = RateLimit {
        notes_per_minute: u32::MAX,
        ..RateLimit::default()
    };
    let builder = nostr_relay_builder::RelayBuilder::default().rate_limit(unlimited);
    let relay = nostr_relay_builder::LocalRelay::new(builder);
    relay.run().await.unwrap();
    relay
}

/// A stand-in Blossom server, for the files an agent sends: it keeps each uploaded blob under its
/// hash and serves it back, taking `delay` to accept each one.
pub async fn blossom(delay: std::time::Duration) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let blobs: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>> =
        std::sync::Arc::default();
    let base = url.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let blobs = blobs.clone();
            let base = base.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 8192];
                let end = loop {
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8_lossy(&request[..end]).to_string();
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                while request.len() < end + length {
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let body = request[end..end + length].to_vec();
                let mut words = head.split_whitespace();
                let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                let (status, kind, reply) = match method {
                    "PUT" => {
                        tokio::time::sleep(delay).await;
                        let hash = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&body));
                        let descriptor = serde_json::json!({
                            "url": format!("{base}/{hash}.bin"), "sha256": hash,
                            "size": body.len(), "type": "application/octet-stream", "uploaded": 1,
                        });
                        blobs.lock().unwrap().insert(hash, body);
                        (201, "application/json", descriptor.to_string().into_bytes())
                    }
                    "GET" => {
                        let hash = path.trim_start_matches('/').split('.').next().unwrap_or("");
                        match blobs.lock().unwrap().get(hash) {
                            Some(blob) => (200, "application/octet-stream", blob.clone()),
                            None => (404, "text/plain", b"not found".to_vec()),
                        }
                    }
                    _ => (404, "text/plain", b"not found".to_vec()),
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    reply.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&reply).await;
            });
        }
    });
    url
}

/// The files the agent sent to a group, as the person has them: each message's attachment,
/// downloaded and decrypted, with its file name and the turn it was tagged with, oldest first.
pub async fn files_sent(world: &World, group: &GroupId) -> Vec<(String, Vec<u8>, Option<String>)> {
    let hex = hex::encode(group.as_slice());
    let mut files = Vec::new();
    for message in world.sent_by_agent(&hex, 9) {
        let Some(imeta) = message.tags.iter().find(|t| t[0] == "imeta") else {
            continue;
        };
        let turn = message
            .tags
            .iter()
            .find(|t| t[0] == "turn")
            .map(|t| t[1].clone());
        let reference =
            marmot_app::media_attachment_from_imeta_tag(imeta, message.source_epoch, true).unwrap();
        let name = reference.file_name.clone();
        let downloaded = world
            .runtime
            .download_media(&world.person, group, reference)
            .await
            .unwrap();
        files.push((name, downloaded.plaintext, turn));
    }
    files
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
        self.allow_once(&target, &read).await
    }

    /// An approval waiting now, in `turn`, whose title passes `asks_for`.
    #[allow(dead_code)] // Only the ACP scenario has a tool approved.
    pub fn approval_waiting(
        &self,
        turn: &TurnId,
        asks_for: impl Fn(&str) -> bool,
    ) -> Option<(EventKey, Read)> {
        self.latest().into_iter().find(|(_, r)| {
            let header = r.item.event.header();
            matches!(r.item.event.body(), Body::Approval(_))
                && header.state == State::Waiting
                && header.turn.as_ref() == Some(turn)
                && asks_for(&header.title)
        })
    }

    /// Allows a waiting approval once: with the choice that says so, or else the first that allows,
    /// as Codex and Claude Code put "Allow once" first. Goose puts its standing choice first.
    pub async fn allow_once(&self, target: &EventKey, read: &Read) -> EventKey {
        let Body::Approval(approval) = read.item.event.body() else {
            panic!("not an approval")
        };
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
        self.ask(Some(target), &asked, &once.label).await
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

    /// The folder a session's item shows, the core's own setting (D-101).
    #[allow(dead_code)] // Not every scenario moves a chat.
    pub fn folder_of(&self, session: &SessionId) -> Option<String> {
        self.items().into_iter().find_map(|(_, r)| {
            let Body::Session(shown) = r.item.event.body() else {
                return None;
            };
            if r.item.event.header().session.as_ref() != Some(session) {
                return None;
            }
            let setting = shown.settings.iter().find(|s| s.id == "folder")?;
            setting.value.clone()
        })
    }
}

/// The model a scenario against a real harness runs on: an OpenAI-compatible endpoint, the model,
/// and the file that holds the endpoint's key (D-100). Each is read from the environment, or else
/// from `tests.env` at the top of the repository, which git ignores (`tests.env.example` shows it).
/// There is no default; CI's nightly workflow sets the same names from its secrets.
pub struct TestModel {
    /// `MODEL_BASE_URL`.
    pub url: String,
    /// `MODEL_NAME`.
    pub name: String,
    /// `MODEL_KEY_FILE`.
    pub key_file: String,
}

impl TestModel {
    pub fn read() -> Self {
        Self {
            url: tests_env("MODEL_BASE_URL"),
            name: tests_env("MODEL_NAME"),
            key_file: tests_env("MODEL_KEY_FILE"),
        }
    }

    /// The endpoint and the model as the container scripts take them, from their environment.
    pub fn env(&self) -> [(&'static str, &str); 2] {
        [("MODEL_BASE_URL", &self.url), ("MODEL_NAME", &self.name)]
    }

    /// The same, as arguments for `env`, for a program the core starts.
    pub fn env_args(&self) -> Vec<String> {
        (self.env().iter())
            .map(|(name, value)| format!("{name}={value}"))
            .collect()
    }
}

/// The setting `name` for the scenarios against real harnesses (D-100): from the environment, or
/// else from `tests.env` at the top of the repository, one `NAME=value` per line. There is no
/// default: an unset name stops the scenario, saying what to set, and so does a name the file gives
/// twice, since the readers in `tools/record` would not know which to take either.
pub fn tests_env(name: &str) -> String {
    if let Some(value) = std::env::var(name).ok().filter(|v| !v.is_empty()) {
        return value;
    }
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests.env");
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut values = (text.lines())
        .filter_map(|line| line.trim().split_once('='))
        .filter(|(key, _)| key.trim_end() == name)
        .map(|(_, value)| value.trim().to_string());
    let value = values.next();
    assert!(
        values.next().is_none(),
        "tests.env sets {name} more than once; keep one"
    );
    value.filter(|v| !v.is_empty()).unwrap_or_else(|| {
        panic!("set {name}, in the environment or in tests.env (see tests.env.example)")
    })
}
