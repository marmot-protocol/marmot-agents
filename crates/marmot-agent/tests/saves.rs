//! What a durable save costs the core when one chat has a backlog of sends and another waits for
//! an approval (issue #155). A measurement, not a check: it is ignored, and run by hand.
//!
//! ```sh
//! cargo test --release -p marmot-agent --test saves -- --ignored --nocapture
//! ```
//!
//! The core runs on a stand-in for MDK with two chats: a group, whose sends MDK is busy for, so
//! everything the agent says there waits in the outbox, and the owner's two-person chat, where an
//! approval waits. For each backlog the agent says that many things in the group, then the owner
//! answers the approval, then the core stops. It prints, for each backlog: how big the state file
//! is, how many saves each step asked for and the writes and bytes the store made of them, the
//! time from the owner's answer reaching the core to the driver getting it, the time a stop
//! takes, once the answer's step has ended, and what one save of that state takes alone. The
//! state is saved in `MARMOT_AGENT_SAVES_DIR` (by default the system's temporary folder), so it
//! can be run on the file system to be measured; `MARMOT_AGENT_SAVES_BACKLOGS` lists the backlogs
//! (by default `0,50,500`). Results, with the machine they came from, are in
//! `docs/design/verification.md` ("What a save costs").

use marmot_agent::carrier::Asked;
use marmot_agent::config::Config;
use marmot_agent::marmot::{
    FileFailed, Incoming, Live, Message, NotNow, Page, SendFailed, Side, Subscribed,
};
use marmot_agent::run::{Core, Kept, Store, Writes};
use marmot_agent::{EventKey, GroupKey, Heard, HeardRequest, PubKey, Sending, Token, Where};
use marmot_agent_driver::{
    Answer, Capabilities, Control, Driver, Emitted, FilesIn, Harness, MidTurn, Output, Prompt,
    Refusal, Sessions,
};
use marmot_agent_events::body::{
    Approval, Body, Choice, Effect, Message as Words, StartedBy, Turn,
};
use marmot_agent_events::{AgentEvent, Header, State};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc, oneshot};

const OWNER: &str = "abababababababababababababababababababababababababababababababab";
const AGENT: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
/// The group, whose sends MDK is busy for.
const BUSY: &str = "01";
/// The owner's two-person chat, where the approval waits.
const DIRECT: &str = "02";
/// How long each thing the agent says in the group is: about a paragraph.
const WORDS: usize = 1024;

/// A stand-in for MDK: the two chats, what the owner said in them, and every send, which MDK is
/// busy for in [`BUSY`] and takes in [`DIRECT`].
#[derive(Clone)]
struct Mdk {
    agent: PubKey,
    stored: Arc<Mutex<Vec<Message>>>,
    stored_now: Arc<Notify>,
    /// The first event of the approval, once it went out.
    approval: Arc<Mutex<Option<EventKey>>>,
    sends: Arc<AtomicUsize>,
}

impl Mdk {
    /// The owner sends a message of `kind` to `group`.
    fn owner_says(&self, group: &str, kind: u64, text: &str) {
        let mut stored = self.stored.lock().unwrap();
        let order = stored.len() as i64 + 1;
        stored.push(Message {
            group: GroupKey::from(group),
            event: EventKey::from(format!("m{order}").as_str()),
            sender: PubKey::from(OWNER),
            sender_name: None,
            kind,
            content: text.into(),
            tags: Vec::new(),
            recorded_at: 1,
            received_at: 1,
            order: Some(order),
        });
        drop(stored);
        self.stored_now.notify_one();
    }
}

fn place(group: &GroupKey) -> Where {
    let direct = group.as_str() == DIRECT;
    Where {
        group: group.clone(),
        name: (!direct).then(|| "Team".into()),
        direct,
        members: if direct { 2 } else { 3 },
    }
}

impl Side for Mdk {
    type Events = ();

    fn agent(&self) -> &PubKey {
        &self.agent
    }

    async fn next(&self, _events: &mut ()) -> Live {
        self.stored_now.notified().await;
        let last = self.stored.lock().unwrap().last().cloned().unwrap();
        Live::Message(Message {
            order: None,
            ..last
        })
    }

    async fn publish_name(&self, _name: &str, _relays: &[String]) -> Result<(), NotNow> {
        Ok(())
    }

    async fn incoming(&self, message: Message) -> Result<Option<Incoming>, NotNow> {
        let chat = place(&message.group);
        if message.kind == 1202 {
            return Ok(Some(Incoming::Asked(HeardRequest {
                chat,
                event: message.event,
                author: message.sender,
                author_name: "Owner".into(),
                age_secs: 0,
                target: self.approval.lock().unwrap().clone(),
                asked: Ok(Asked::Choice {
                    choice: "allow".into(),
                }),
            })));
        }
        Ok(Some(Incoming::Heard(Heard {
            chat,
            event: message.event,
            author: message.sender,
            author_name: "Owner".into(),
            text: message.content,
            reply_to: None,
            mentions_agent: true,
            attachments: Vec::new(),
        })))
    }

    fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)> {
        Vec::new()
    }

    async fn decide(&self, _group: &GroupKey, _accept: bool) -> bool {
        unreachable!("no invitations")
    }

    fn groups(&self) -> Result<Vec<GroupKey>, NotNow> {
        Ok(vec![GroupKey::from(BUSY), GroupKey::from(DIRECT)])
    }

    fn two_people(&self, group: &GroupKey) -> Result<Option<Vec<PubKey>>, NotNow> {
        Ok((group.as_str() == DIRECT).then(|| vec![self.agent.clone(), PubKey::from(OWNER)]))
    }

    fn stored(&self, group: &GroupKey, after: i64, limit: usize) -> Result<Page, NotNow> {
        let stored = self.stored.lock().unwrap();
        let messages: Vec<Message> = (stored.iter())
            .filter(|m| &m.group == group && m.order > Some(after))
            .take(limit)
            .cloned()
            .collect();
        let last = messages.last().and_then(|m| m.order);
        let more = messages.len() == limit;
        Ok(Page {
            messages,
            last,
            more,
        })
    }

    async fn send(
        &self,
        group: &GroupKey,
        what: &Sending,
        token: &Token,
    ) -> Result<EventKey, SendFailed> {
        if group.as_str() == BUSY {
            return Err(SendFailed::Busy);
        }
        let n = self.sends.fetch_add(1, SeqCst);
        let event = EventKey::from(format!("s{n}-{}", token.mdk).as_str());
        if let Sending::Item { item, .. } = what
            && matches!(item.event.body(), Body::Approval(_))
        {
            self.approval.lock().unwrap().get_or_insert(event.clone());
        }
        Ok(event)
    }

    async fn send_file(
        &self,
        _group: &GroupKey,
        _file: &marmot_agent::files::Outgoing,
        _turn: Option<&marmot_agent_events::TurnId>,
    ) -> Result<EventKey, FileFailed> {
        unreachable!("no files")
    }
}

/// How far the core is with the outputs the bench handed it.
#[derive(Clone, Copy)]
struct Taken {
    /// How many it has taken.
    count: usize,
    /// Whether it waits for the next: the step that took the last has ended.
    waiting: bool,
    /// When it last began to wait.
    since: Instant,
}

/// A harness the bench hands its events to. It passes on each prompt, and notes when it is given
/// the answer and how many saves had been made by then.
struct Harnessed {
    outputs: mpsc::UnboundedReceiver<Output>,
    prompts: mpsc::UnboundedSender<Prompt>,
    taken: Arc<Mutex<Taken>>,
    answered: Option<oneshot::Sender<(Instant, Count)>>,
    saves: Arc<AtomicUsize>,
    writes: Writes,
}

impl Driver for Harnessed {
    fn harness(&self) -> Harness {
        Harness {
            name: "bench".into(),
            version: None,
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: false,
            mid_turn: MidTurn::Queue,
            sessions: Sessions::Core,
            resume: false,
            approvals: true,
            files_in: FilesIn::None,
            approval_commands: Vec::new(),
            subagents: false,
            stop_subagent: false,
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let _ = self.prompts.send(prompt);
        Ok(())
    }

    async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
        // The core is in a step until it asks for the next output: so the bench, once it has the
        // answer, never sees the step as ended before it has.
        self.taken.lock().unwrap().waiting = false;
        if let Some(answered) = self.answered.take() {
            let count = Count {
                saves: self.saves.load(SeqCst),
                writes: self.writes.count(),
                bytes: self.writes.bytes(),
            };
            let _ = answered.send((Instant::now(), count));
        }
        Ok(())
    }

    async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
        unreachable!("no control")
    }

    async fn next_event(&mut self) -> Option<Output> {
        {
            let mut taken = self.taken.lock().unwrap();
            taken.waiting = true;
            taken.since = Instant::now();
        }
        let output = self.outputs.recv().await;
        let mut taken = self.taken.lock().unwrap();
        taken.count += 1;
        taken.waiting = false;
        output
    }

    async fn file_sent(&mut self, _id: String, _result: Result<(), Refusal>) {}
}

/// An event in `prompt`'s chat and session.
fn event(prompt: &Prompt, item: &str, state: State, body: Body, text: &str) -> Output {
    let header = Header {
        harness: "bench".into(),
        session: prompt.session.clone(),
        turn: Some(format!("turn-{}", prompt.chat.id.0).as_str().into()),
        item: item.into(),
        seq: 0,
        state,
        title: "Words".into(),
        text: text.into(),
        subagent: None,
    };
    Output::Event(Emitted {
        event: AgentEvent::new(header, body, None).unwrap(),
        chat: Some(prompt.chat.id.clone()),
        prompts: vec![prompt.message.clone()],
        settles: Vec::new(),
    })
}

fn turn() -> Body {
    Body::Turn(Turn {
        started_by: StartedBy::User,
        duration_ms: None,
        usage: None,
    })
}

/// The saves the core asked for, and the writes and bytes the store made of them.
#[derive(Clone, Copy, Default)]
struct Count {
    saves: usize,
    writes: u64,
    bytes: u64,
}

impl std::ops::Sub for Count {
    type Output = Count;

    fn sub(self, before: Count) -> Count {
        Count {
            saves: self.saves - before.saves,
            writes: self.writes - before.writes,
            bytes: self.bytes - before.bytes,
        }
    }
}

impl std::fmt::Display for Count {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}, {} B)", self.saves, self.writes, self.bytes)
    }
}

/// What one backlog measured.
struct Measured {
    backlog: usize,
    state_bytes: u64,
    build: Count,
    build_last_event: Duration,
    answer: Count,
    answer_handoff: Duration,
    stop: Count,
    stop_time: Duration,
    one_save: Duration,
    serialize: Duration,
}

/// Measures one backlog. The hook that counts the saves never fails one; the store counts the
/// writes it makes of them.
async fn measure(backlog: usize, under: &std::path::Path) -> Measured {
    let state = tempfile::tempdir_in(under).unwrap();
    let saves = Arc::new(AtomicUsize::new(0));
    let counting = saves.clone();
    let (store, _) = Store::failing_when(state.path(), move |_: &Kept| {
        counting.fetch_add(1, SeqCst);
        false
    });
    let (store, writes) = store.counting_writes();
    let count = || Count {
        saves: saves.load(SeqCst),
        writes: writes.count(),
        bytes: writes.bytes(),
    };
    let settings = format!(
        "home = \"/unused\"\nrelays = [\"wss://relay.invalid\"]\nowner = \"{OWNER}\"\n[connector]\n"
    );
    let config = Config::parse(&settings).unwrap();
    let mdk = Mdk {
        agent: PubKey::from(AGENT),
        stored: Arc::default(),
        stored_now: Arc::default(),
        approval: Arc::default(),
        sends: Arc::default(),
    };
    let (outputs, outputs_in) = mpsc::unbounded_channel();
    let (prompts_out, mut prompts) = mpsc::unbounded_channel();
    let (answered, answer) = oneshot::channel();
    let taken = Arc::new(Mutex::new(Taken {
        count: 0,
        waiting: false,
        since: Instant::now(),
    }));
    let driver = Harnessed {
        outputs: outputs_in,
        prompts: prompts_out,
        taken: taken.clone(),
        answered: Some(answered),
        saves: saves.clone(),
        writes: writes.clone(),
    };
    let core = Core::new(mdk.clone(), driver, &config, store).unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let live = Subscribed {
        events: (),
        since: 0,
    };
    // How the core ended, once it has: a save that failed ends the bench with its error.
    let ended: Arc<Mutex<Option<Result<(), String>>>> = Arc::default();
    let ending = ended.clone();
    let running = tokio::spawn(async move {
        let ran = core
            .run(live, async {
                let _ = stopped.await;
            })
            .await;
        *ending.lock().unwrap() = Some(ran.clone());
        ran
    });
    // Waits until the core has taken `count` outputs and waits for the next, and gives when it
    // began to wait.
    let drained = |count: usize| {
        let (taken, ended) = (taken.clone(), ended.clone());
        async move {
            loop {
                let now = *taken.lock().unwrap();
                if now.count == count && now.waiting {
                    return now.since;
                }
                if let Some(ran) = ended.lock().unwrap().clone() {
                    panic!("the core ended before the bench was done: {ran:?}");
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    };

    // A prompt in each chat, so each has its chat and its turn.
    mdk.owner_says(BUSY, 9, "start");
    let busy = prompts.recv().await.unwrap();
    mdk.owner_says(DIRECT, 9, "start");
    let direct = prompts.recv().await.unwrap();
    let mut sent = 0;
    let mut send = |output| {
        outputs.send(output).unwrap();
        sent += 1;
        sent
    };
    send(event(&busy, "turn", State::Running, turn(), "Working"));
    send(event(&direct, "turn", State::Running, turn(), "Working"));
    let approval = Body::Approval(Approval {
        subject: None,
        choices: vec![Choice {
            id: "allow".into(),
            label: "Allow once".into(),
            effect: Effect::Allow,
        }],
        answer: None,
    });
    let at = send(event(
        &direct,
        "approval",
        State::Waiting,
        approval,
        "Allow?",
    ));
    drained(at).await;
    assert!(
        mdk.approval.lock().unwrap().is_some(),
        "the approval went out"
    );

    // The backlog: each thing the agent says in the group waits in the outbox.
    let text = "w".repeat(WORDS);
    let before = count();
    let mut last_event = Duration::ZERO;
    for n in 0..backlog {
        let words = Body::Message(Words::default());
        let output = event(&busy, &format!("words-{n}"), State::Done, words, &text);
        // From the event handed over to the core asking for the next, as the driver saw it.
        let started = Instant::now();
        let at = send(output);
        last_event = drained(at).await - started;
    }
    let build = count() - before;
    let state_file = state.path().join("marmot-agent-state.json");
    let state_bytes = std::fs::metadata(&state_file).unwrap().len();

    // The owner answers the approval: from the message reaching the core to the driver's answer.
    let before = count();
    let started = Instant::now();
    mdk.owner_says(DIRECT, 1202, "");
    let (handed, then) = answer.await.unwrap();
    let answer_handoff = handed - started;
    let answer = then - before;
    // The step that answered ends before the stop is measured, its last save included.
    drained(sent).await;

    // The core stops.
    let before = count();
    let started = Instant::now();
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();
    let stop_time = started.elapsed();
    let stop = count() - before;

    // One save of the state the core stopped with, alone, the median of a few.
    let kept = Store::new(state.path()).load().unwrap();
    let alone = tempfile::tempdir_in(under).unwrap();
    let mut times: Vec<Duration> = (0..9)
        .map(|_| {
            // A store of its own each time, which has written nothing, so each save is written.
            let store = Store::new(alone.path());
            let started = Instant::now();
            store.save(&kept).unwrap();
            started.elapsed()
        })
        .collect();
    times.sort();
    // Of which making the bytes, without writing them.
    let mut made: Vec<Duration> = (0..9)
        .map(|_| {
            let started = Instant::now();
            std::hint::black_box(serde_json::to_vec(&kept).unwrap());
            started.elapsed()
        })
        .collect();
    made.sort();

    Measured {
        backlog,
        state_bytes,
        build,
        build_last_event: last_event,
        answer,
        answer_handoff,
        stop,
        stop_time,
        one_save: times[times.len() / 2],
        serialize: made[made.len() / 2],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a measurement, run by hand"]
async fn what_a_save_costs_under_an_outbox_backlog() {
    let under = std::env::var_os("MARMOT_AGENT_SAVES_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let backlogs: Vec<usize> = std::env::var("MARMOT_AGENT_SAVES_BACKLOGS")
        .unwrap_or_else(|_| "0,50,500".into())
        .split(',')
        .map(|n| n.trim().parse().unwrap())
        .collect();
    println!(
        "backlog | state file | saves (writes, bytes) building it | per backlog event | last backlog event | saves (writes, bytes) before answer | answer to driver | saves (writes, bytes) in stop | stop | one save alone | of which serializing"
    );
    for backlog in backlogs {
        let m = measure(backlog, &under).await;
        let per = |n: f64| n / m.backlog.max(1) as f64;
        println!(
            "{} | {} B | {} | {:.2} ({:.2}) | {:?} | {} | {:?} | {} | {:?} | {:?} | {:?}",
            m.backlog,
            m.state_bytes,
            m.build,
            per(m.build.saves as f64),
            per(m.build.writes as f64),
            m.build_last_event,
            m.answer,
            m.answer_handoff,
            m.stop,
            m.stop_time,
            m.one_save,
            m.serialize,
        );
    }
}
