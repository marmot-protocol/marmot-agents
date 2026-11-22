//! A group the core has no read mark for, as on an imported account's first run or after the state
//! file was lost: what arrived by the time the core subscribed is its history, never a prompt, and
//! what arrived after is read, once, in storage order (L3, S10), whatever order MDK stored and
//! announced them in. MDK is a scripted stand-in here, so the order things are stored in, when
//! they arrived and when they are announced are the test's to choose; the real core runs on it.

use marmot_agent::config::Config;
use marmot_agent::files::Outgoing;
use marmot_agent::marmot::{
    FileFailed, Incoming, Live, Message, NotNow, SendFailed, Side, Subscribed,
};
use marmot_agent::run::{Core, Store};
use marmot_agent::{EventKey, GroupKey, Heard, PubKey, Sending, Token, Where};
use marmot_agent_driver::{
    Answer, Capabilities, Control, Driver, FilesIn, Harness, MidTurn, Output, Prompt, Refusal,
    Sessions,
};
use marmot_agent_events::TurnId;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The second the core subscribed in, by the stand-in's clock.
const SINCE: u64 = 1_000_100;

fn owner() -> PubKey {
    PubKey(format!("0e{:062x}", 1))
}

fn group() -> GroupKey {
    GroupKey(format!("6c{:062x}", 1))
}

/// What the stand-in MDK stored, and how many of the next reads of it fail.
#[derive(Default)]
struct Storage {
    rows: Vec<Message>,
    fails: usize,
}

/// A stand-in MDK with one accepted group, whose storage the test fills.
#[derive(Clone)]
struct Mdk {
    agent: PubKey,
    storage: Arc<Mutex<Storage>>,
}

impl Mdk {
    fn new() -> Self {
        Self {
            agent: PubKey(format!("a9{:062x}", 1)),
            storage: Arc::default(),
        }
    }

    /// Stores a message from the owner, next in storage order, that arrived in second `arrived`,
    /// and gives it as MDK announces it live: with no place in storage.
    fn store(&self, text: &str, arrived: u64) -> Message {
        let mut storage = self.storage.lock().unwrap();
        let order = storage.rows.len() as i64 + 1;
        let message = Message {
            group: group(),
            event: EventKey(format!("e{:063x}", order)),
            sender: owner(),
            sender_name: None,
            kind: 9,
            content: text.into(),
            tags: Vec::new(),
            recorded_at: arrived,
            received_at: arrived,
            order: Some(order),
        };
        storage.rows.push(message.clone());
        Message {
            order: None,
            ..message
        }
    }

    /// The next `n` reads of what was stored fail.
    fn fail_reads(&self, n: usize) {
        self.storage.lock().unwrap().fails = n;
    }

    fn read_fails(&self) -> bool {
        let mut storage = self.storage.lock().unwrap();
        let fails = storage.fails > 0;
        storage.fails = storage.fails.saturating_sub(1);
        fails
    }
}

impl Side for Mdk {
    type Events = mpsc::UnboundedReceiver<Live>;

    fn agent(&self) -> &PubKey {
        &self.agent
    }

    async fn next(&self, events: &mut Self::Events) -> Live {
        match events.recv().await {
            Some(live) => live,
            None => std::future::pending().await,
        }
    }

    async fn publish_name(&self, _name: &str, _relays: &[String]) {}

    async fn incoming(&self, message: Message) -> Result<Option<Incoming>, NotNow> {
        Ok(Some(Incoming::Heard(Heard {
            chat: Where {
                group: message.group,
                name: None,
                direct: true,
                members: 2,
            },
            event: message.event,
            author: message.sender,
            author_name: "Owner".into(),
            text: message.content,
            reply_to: None,
            mentions_agent: false,
            attachments: Vec::new(),
        })))
    }

    fn order_of(&self, message: &Message) -> Result<Option<i64>, NotNow> {
        let storage = self.storage.lock().unwrap();
        Ok((storage.rows.iter())
            .find(|m| m.event == message.event)
            .and_then(|m| m.order))
    }

    fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)> {
        Vec::new()
    }

    async fn decide(&self, _group: &GroupKey, _accept: bool) -> bool {
        true
    }

    fn groups(&self) -> Result<Vec<GroupKey>, NotNow> {
        Ok(vec![group()])
    }

    fn stored(&self, _group: &GroupKey, after: i64) -> Result<Vec<Message>, NotNow> {
        if self.read_fails() {
            return Err(NotNow);
        }
        let storage = self.storage.lock().unwrap();
        Ok((storage.rows.iter())
            .filter(|m| m.order.is_some_and(|o| o > after))
            .cloned()
            .collect())
    }

    async fn send(
        &self,
        _group: &GroupKey,
        _what: &Sending,
        token: &Token,
    ) -> Result<EventKey, SendFailed> {
        Ok(EventKey(token.mdk.clone()))
    }

    async fn send_file(
        &self,
        _group: &GroupKey,
        _file: &Outgoing,
        _turn: Option<&TurnId>,
    ) -> Result<EventKey, FileFailed> {
        Err(FileFailed::Failed)
    }
}

/// A harness that takes every prompt, notes its text, and emits nothing.
struct Taker(Arc<Mutex<Vec<String>>>);

impl Driver for Taker {
    fn harness(&self) -> Harness {
        Harness {
            name: "taker".into(),
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
        self.0.lock().unwrap().push(prompt.text);
        Ok(())
    }

    async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
        Ok(())
    }

    async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
        Ok(())
    }

    async fn next_event(&mut self) -> Option<Output> {
        std::future::pending().await
    }
}

/// The core running on a stand-in MDK, with no read mark for its group.
struct Running {
    mdk: Mdk,
    live: mpsc::UnboundedSender<Live>,
    prompts: Arc<Mutex<Vec<String>>>,
    state: tempfile::TempDir,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

impl Running {
    /// Starts the core on `mdk`, subscribed in second [`SINCE`].
    fn start(mdk: Mdk) -> Self {
        let state = tempfile::tempdir().unwrap();
        let settings = format!(
            "home = \"/unused\"\nrelays = [\"wss://relay.invalid\"]\nowner = \"{}\"\n[connector]\n",
            owner().as_str()
        );
        let config = Config::parse(&settings).unwrap();
        let prompts = Arc::default();
        let driver = Taker(Arc::clone(&prompts));
        let core = Core::new(mdk.clone(), driver, &config, Store::new(state.path())).unwrap();
        let (announce, events) = mpsc::unbounded_channel();
        let (stop, stopped) = oneshot::channel::<()>();
        let live = Subscribed {
            events,
            since: SINCE,
        };
        let task = tokio::spawn(core.run(live, async {
            let _ = stopped.await;
        }));
        Self {
            mdk,
            live: announce,
            prompts,
            state,
            stop: Some(stop),
            task: Some(task),
        }
    }

    /// MDK announces `message` live.
    fn announce(&self, message: Message) {
        self.live.send(Live::Message(message)).unwrap();
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    /// The group's read mark, as saved.
    fn mark(&self) -> Option<i64> {
        let kept = Store::new(self.state.path()).load().unwrap();
        kept.read.get(&group()).copied()
    }

    /// Gives the core time to read, try again and deliver: far more than it needs.
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }

    async fn stop(mut self) {
        let _ = self.stop.take().unwrap().send(());
        let ended = self.task.take().unwrap().await.unwrap();
        assert!(ended.is_ok(), "the core ends cleanly");
        drop(self.mdk);
    }
}

// MDK received a prompt after the core subscribed and a history message before it, and stored
// them the other way round. The history is not a prompt, even though it comes after a message
// that is, and it is marked read, so a restart does not read it either.
#[tokio::test(start_paused = true)]
async fn history_stored_after_a_new_prompt_is_history() {
    let mdk = Mdk::new();
    // Where the group's messages end cannot be read when the core starts.
    mdk.fail_reads(1);
    let new = mdk.store("new", SINCE + 1);
    let old = mdk.store("old", SINCE);
    let core = Running::start(mdk);
    core.announce(new);
    core.announce(old);
    core.settle().await;
    assert_eq!(core.prompts(), ["new"]);
    assert_eq!(core.mark(), Some(2), "the history is marked read");
    core.stop().await;
}

// What a new group's history is does not hang on the order MDK announces messages in: when it
// stores a prompt after the core started and announces it only after the core read the group,
// the prompt is read from storage, once.
#[tokio::test(start_paused = true)]
async fn a_prompt_announced_after_a_read_that_found_it_is_read_once() {
    let mdk = Mdk::new();
    let old = mdk.store("old", SINCE - 5);
    // The group cannot be read when the core starts.
    mdk.fail_reads(1);
    let core = Running::start(mdk.clone());
    core.announce(old);
    // Stored before the core reads the group again, and announced after.
    let new = mdk.store("new", SINCE + 1);
    core.settle().await;
    core.announce(new);
    core.settle().await;
    assert_eq!(core.prompts(), ["new"]);
    core.stop().await;
}
