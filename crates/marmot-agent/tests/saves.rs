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
//! is, how many saves each step made, the time from the owner's answer reaching the core to the
//! driver getting it, the time a stop takes, and what one save of that state takes alone. The
//! state is saved in `MARMOT_AGENT_SAVES_DIR` (by default the system's temporary folder), so it
//! can be run on the file system to be measured; `MARMOT_AGENT_SAVES_BACKLOGS` lists the backlogs
//! (by default `0,50,500`). Results, with the machine they came from, are in
//! `docs/design/verification.md` ("What a save costs").

use marmot_agent::carrier::Asked;
use marmot_agent::config::Config;
use marmot_agent::marmot::{
    FileFailed, Incoming, Live, Message, NotNow, Page, SendFailed, Side, Subscribed,
};
use marmot_agent::run::{Core, Kept, Store};
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

/// A harness the bench hands its events to. It passes on each prompt, and notes when it is given
/// the answer and how many saves had been made by then.
struct Harnessed {
    outputs: mpsc::UnboundedReceiver<Output>,
    prompts: mpsc::UnboundedSender<Prompt>,
    /// How many outputs the core has taken, and whether it waits for the next.
    taken: Arc<Mutex<(usize, bool)>>,
    answered: Option<oneshot::Sender<(Instant, usize)>>,
    saves: Arc<AtomicUsize>,
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
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let _ = self.prompts.send(prompt);
        Ok(())
    }

    async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
        if let Some(answered) = self.answered.take() {
            let _ = answered.send((Instant::now(), self.saves.load(SeqCst)));
        }
        Ok(())
    }

    async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
        unreachable!("no control")
    }

    async fn next_event(&mut self) -> Option<Output> {
        self.taken.lock().unwrap().1 = true;
        let output = self.outputs.recv().await;
        let mut taken = self.taken.lock().unwrap();
        *taken = (taken.0 + 1, false);
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

/// What one backlog measured.
struct Measured {
    backlog: usize,
    state_bytes: u64,
    build_saves_per_event: f64,
    build_last_event: Duration,
    answer_saves: usize,
    answer_handoff: Duration,
    stop_saves: usize,
    stop: Duration,
    one_save: Duration,
    serialize: Duration,
}

/// Measures one backlog. The hook that counts the saves never fails one. With `writes`, it counts
/// only those whose state differs from the last, which the store writes, and the others it skips;
/// telling them apart takes making the bytes once more, so its times are not the ones reported.
async fn measure(backlog: usize, under: &std::path::Path, writes: bool) -> Measured {
    let state = tempfile::tempdir_in(under).unwrap();
    let saves = Arc::new(AtomicUsize::new(0));
    let counting = saves.clone();
    let last = Mutex::new(Vec::new());
    let (store, _) = Store::failing_when(state.path(), move |kept: &Kept| {
        if writes {
            let bytes = serde_json::to_vec(kept).unwrap();
            let mut last = last.lock().unwrap();
            if *last == bytes {
                return false;
            }
            *last = bytes;
        }
        counting.fetch_add(1, SeqCst);
        false
    });
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
    let taken = Arc::new(Mutex::new((0, false)));
    let driver = Harnessed {
        outputs: outputs_in,
        prompts: prompts_out,
        taken: taken.clone(),
        answered: Some(answered),
        saves: saves.clone(),
    };
    let core = Core::new(mdk.clone(), driver, &config, store).unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let live = Subscribed {
        events: (),
        since: 0,
    };
    let running = tokio::spawn(core.run(live, async {
        let _ = stopped.await;
    }));
    let drained = |count: usize| {
        let taken = taken.clone();
        async move {
            loop {
                if *taken.lock().unwrap() == (count, true) {
                    return;
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
    let before = saves.load(SeqCst);
    let mut last_event = Duration::ZERO;
    for n in 0..backlog {
        let words = Body::Message(Words::default());
        let at = send(event(
            &busy,
            &format!("words-{n}"),
            State::Done,
            words,
            &text,
        ));
        let started = Instant::now();
        drained(at).await;
        last_event = started.elapsed();
    }
    let build_saves = saves.load(SeqCst) - before;
    let state_file = state.path().join("marmot-agent-state.json");
    let state_bytes = std::fs::metadata(&state_file).unwrap().len();

    // The owner answers the approval: from the message reaching the core to the driver's answer.
    let before = saves.load(SeqCst);
    let started = Instant::now();
    mdk.owner_says(DIRECT, 1202, "");
    let (handed, saves_then) = answer.await.unwrap();
    let answer_handoff = handed - started;
    let answer_saves = saves_then - before;

    // The core stops.
    let before = saves.load(SeqCst);
    let started = Instant::now();
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();
    let stop_time = started.elapsed();
    let stop_saves = saves.load(SeqCst) - before;

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
        build_saves_per_event: build_saves as f64 / backlog.max(1) as f64,
        build_last_event: last_event,
        answer_saves,
        answer_handoff,
        stop_saves,
        stop: stop_time,
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
        "backlog | state file | saves (writes) per backlog event | last backlog event | saves (writes) before answer | answer to driver | saves (writes) in stop | stop | one save alone | of which serializing"
    );
    for backlog in backlogs {
        let m = measure(backlog, &under, false).await;
        let w = measure(backlog, &under, true).await;
        println!(
            "{} | {} B | {:.2} ({:.2}) | {:?} | {} ({}) | {:?} | {} ({}) | {:?} | {:?} | {:?}",
            m.backlog,
            m.state_bytes,
            m.build_saves_per_event,
            w.build_saves_per_event,
            m.build_last_event,
            m.answer_saves,
            w.answer_saves,
            m.answer_handoff,
            m.stop_saves,
            w.stop_saves,
            m.stop,
            m.one_save,
            m.serialize,
        );
    }
}
