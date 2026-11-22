//! Everything outside the core, in one place: the simulated MDK (its storage, its live events,
//! what it does with each send), the people who post, the scripted harness behind the driver,
//! and the faults the schedule has armed. The core reaches it through [`crate::side::SimSide`]
//! and [`crate::driver::SimDriver`]; the oracles read it and nothing of the core.

use crate::schedule::{Faults, Outcome, Posted, What};
use marmot_agent::carrier::{self, Asked, ITEM_KIND, REQUEST_KIND};
use marmot_agent::marmot::{Live, Message};
use marmot_agent::{EventKey, GroupKey, PubKey};
use marmot_agent_driver::{
    Action as DriverAction, Answer, Control, Emitted, Given, Output, Prompt, Reason, Refusal,
};
use marmot_agent_events::body::{
    Approval, Body, Choice, Effect, Message as Words, Session, StartedBy, Turn,
};
use marmot_agent_events::{AgentEvent, Header, State};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tokio::sync::mpsc;
use tokio::time::Instant;

/// The kind of a chat message.
pub const CHAT: u64 = 9;
/// The kind the simulated MDK gives an edit of the agent's words. Not a kind MDK uses: an edit
/// only needs to be told apart here.
pub const EDIT: u64 = 1_000_009;

/// One thing that happened, at a time on the virtual clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    /// Milliseconds since the run started.
    pub ms: u64,
    /// The step of the schedule it happened in, or `None` in the fair part at the end.
    pub step: Option<usize>,
    /// What happened, named as the shared vocabulary names it.
    pub what: String,
}

/// A stored event: something a person sent, or something the agent sent.
#[derive(Clone, Debug)]
pub struct Row {
    /// Its place in storage order.
    pub order: i64,
    /// The chat.
    pub chat: usize,
    /// Its id.
    pub event: EventKey,
    /// Whether the agent sent it.
    pub from_agent: bool,
    /// Its kind.
    pub kind: u64,
    /// Its content.
    pub content: String,
    /// Its tags.
    pub tags: Vec<Vec<String>>,
    /// When its sender says it was sent, in seconds.
    pub recorded_at: u64,
    /// When it was stored.
    pub ms: u64,
    /// For an edit: the message it edits.
    pub edits: Option<EventKey>,
    /// For a message a person posted: its number in the schedule.
    pub msg: Option<usize>,
}

/// A message a person posted, and how far it got.
#[derive(Clone, Debug)]
pub struct Msg {
    /// What it is.
    pub posted: Posted,
    /// Its id.
    pub event: EventKey,
    /// Its content.
    pub content: String,
    /// Its tags.
    pub tags: Vec<Vec<String>>,
    /// When it was posted, in seconds.
    pub recorded_at: u64,
    /// Its place in storage, once stored.
    pub stored: Option<i64>,
    /// Whether it was delivered live, or lost on the way.
    pub heard: bool,
    /// Waiting for its chat to be accepted: to be stored, and heard.
    pub deferred: Option<bool>,
    /// When the core last read it, live or from storage, in milliseconds.
    pub read_ms: Option<u64>,
}

/// A chat with the agent.
#[derive(Clone, Debug)]
pub struct Chat {
    /// Its group.
    pub group: GroupKey,
    /// Whether the agent was invited and has not decided.
    pub invited: bool,
    /// Whether the agent accepted.
    pub accepted: bool,
}

/// A send the simulated MDK refused, for the oracles.
#[derive(Clone, Debug)]
pub struct RefusedSend {
    /// The chat.
    pub chat: usize,
    /// For an item: its id and version.
    pub item: Option<(String, u64)>,
    /// The requests it would have settled.
    pub settles: Vec<EventKey>,
    /// For the agent's words: their text.
    pub words: Option<String>,
    /// For an item: the state the refused version had.
    pub state: Option<State>,
    /// Whether it was a notice of the core's own, such as the `failed` notice that answers a
    /// refused send: the core drops a refused notice, so nothing loops.
    pub notice: bool,
}

/// A send that goes out later than the core looked for it.
#[derive(Clone, Debug)]
pub struct Landing {
    /// When it lands.
    pub at: Instant,
    /// The row it becomes.
    pub row: Row,
}

/// What the scripted harness is to do with the next request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Refuse it.
    Refuse,
    /// Take it after this many milliseconds.
    Slow(u64),
    /// Never answer.
    Hang,
    /// Take it and never show its effect.
    Ignore,
}

/// What the driver does with a request, decided when it arrives.
pub enum Decision {
    /// A crash is due here: the request never reaches the harness.
    Crash,
    /// Answer at once.
    Now(Result<(), Refusal>),
    /// Answer after this many milliseconds.
    Slow(u64, Result<(), Refusal>),
    /// Never answer.
    Hang,
}

/// What the harness was handed, for the oracles.
#[derive(Clone, Debug)]
pub struct Delivery {
    /// The driver's life it reached, from 1.
    pub life: u32,
    /// When.
    pub ms: u64,
    /// The message it came from, when the harness can tell.
    pub msg: Option<usize>,
    /// Its kind: `prompt`, `answer` or `control`.
    pub kind: &'static str,
    /// The core's id for the chat.
    pub chat: Option<String>,
    /// For a prompt: the session it names.
    pub session: Option<String>,
    /// For a prompt: whether this life had already taken a prompt or a new session for the chat.
    pub chat_taken: bool,
    /// For an answer: the item it answers.
    pub item: Option<String>,
}

/// An event the harness emitted, for the oracles.
#[derive(Clone, Debug)]
pub struct Emission {
    /// The driver's life.
    pub life: u32,
    /// When it was handed to the core.
    pub ms: u64,
    /// The core's id for the chat.
    pub chat: String,
    /// The item's id.
    pub item: String,
    /// The requests it settles, by the core's ids.
    pub settles: Vec<String>,
    /// For a session item: its session.
    pub session: Option<String>,
    /// Its state.
    pub state: State,
    /// For the agent's words: their text, which goes out as a chat message.
    pub words: Option<String>,
}

/// Where a scripted turn has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    SessionStart,
    TurnStart,
    ApprovalOpen,
    Waiting,
    Words,
    TurnEnd,
    Done,
}

/// A turn the scripted harness is working on.
#[derive(Clone, Debug)]
struct Script {
    chat: String,
    turn: String,
    session: String,
    prompt: String,
    approval: bool,
    stage: Stage,
    /// An answer taken for its approval: the request, and whether it is ignored.
    answer: Option<(String, bool)>,
    /// An interrupt taken for it.
    interrupt: Option<String>,
}

/// A control's effect the harness is still to show.
#[derive(Clone, Debug)]
enum Effecting {
    NewSession {
        request: String,
        chat: String,
        old: Option<String>,
    },
    Set {
        request: String,
        chat: String,
        by_words: bool,
    },
}

/// The scripted harness behind the driver.
#[derive(Default)]
pub struct Harness {
    /// The current life, from 1; each restart starts a new one.
    pub life: u32,
    /// Where the current life's events go; `None` once it has stopped.
    pub out: Option<mpsc::UnboundedSender<Output>>,
    /// What the next requests meet.
    pub moves: VecDeque<Move>,
    next: u64,
    seq: u64,
    /// This life's session for each chat.
    sessions: BTreeMap<String, String>,
    /// Chats this life has taken a prompt or a new session for.
    taken: BTreeSet<String>,
    scripts: Vec<Script>,
    effects: Vec<Effecting>,
    /// Every request handed to the harness, in every life.
    pub deliveries: Vec<Delivery>,
    /// Every event it emitted, in every life.
    pub emissions: Vec<Emission>,
    /// Messages whose delivery a crash cut off: they never reached the harness.
    pub lost: Vec<usize>,
    /// For an answer or a setting taken: the message it came from, and when it was taken.
    pub requests: BTreeMap<String, (usize, u64)>,
    /// Every session started, with the core's id for its chat.
    pub sessions_started: Vec<(String, String)>,
    /// Sessions a new session ended: never started again.
    ended: BTreeSet<String>,
    /// Whether this life holds a request it never answers.
    pub hung: bool,
}

/// The simulated world.
pub struct World {
    /// When the run started.
    pub start: Instant,
    /// The agent.
    pub agent: PubKey,
    /// The person who owns it and posts everything.
    pub owner: PubKey,
    /// Whether the simulated MDK keeps its contract (see [`crate::schedule::Family::Disorder`]).
    pub contract: bool,
    /// The chats.
    pub chats: Vec<Chat>,
    /// Everything stored, in storage order.
    pub rows: Vec<Row>,
    /// What people post, by number; `None` until posted.
    pub msgs: Vec<Option<Msg>>,
    /// The schedule's messages.
    pub posted: Vec<Posted>,
    next_order: i64,
    next_event: u64,
    /// The live events waiting for the core, for the life that is running.
    pub live: VecDeque<Live>,
    /// Whether a core is subscribed.
    pub listening: bool,
    /// What the next sends meet.
    pub outcomes: VecDeque<Outcome>,
    /// How many lookups are still to fail.
    pub lookup_errors: u32,
    /// Whether the next save fails.
    pub save_fails: bool,
    /// The effect a crash comes before, counting down.
    pub crash_after: Option<u32>,
    /// A crash is due: the core is stuck at it, to be killed.
    pub crashed: bool,
    /// Sends that land later.
    pub landings: Vec<Landing>,
    /// Sends MDK refused.
    pub refused: Vec<RefusedSend>,
    /// The scripted harness.
    pub harness: Harness,
    /// What happened.
    pub trace: Vec<Trace>,
    /// The schedule's step being run.
    pub step: Option<usize>,
    /// The faults made by hand.
    pub faults: Faults,
    /// A clean stop was asked of the running core.
    pub stopping: bool,
    /// A save failed in this life, so the core stops with an error.
    pub unsaved: bool,
}

/// A 64-character hex id.
fn hex_id(prefix: u64, n: u64) -> String {
    format!("{prefix:016x}{n:048x}")
}

impl World {
    /// A world with `chats` chats and the messages `posted`.
    pub fn new(chats: u8, posted: Vec<Posted>, contract: bool) -> Self {
        let chats = (0..chats)
            .map(|c| Chat {
                group: GroupKey(hex_id(0x6c, u64::from(c))),
                invited: false,
                accepted: false,
            })
            .collect();
        Self {
            start: Instant::now(),
            agent: PubKey(hex_id(0xa9, 1)),
            owner: PubKey(hex_id(0x0e, 1)),
            contract,
            chats,
            rows: Vec::new(),
            msgs: vec![None; posted.len()],
            posted,
            next_order: 0,
            next_event: 0,
            live: VecDeque::new(),
            listening: false,
            outcomes: VecDeque::new(),
            lookup_errors: 0,
            save_fails: false,
            crash_after: None,
            crashed: false,
            landings: Vec::new(),
            refused: Vec::new(),
            harness: Harness::default(),
            trace: Vec::new(),
            step: None,
            faults: Faults::default(),
            stopping: false,
            unsaved: false,
        }
    }

    /// Milliseconds on the virtual clock.
    pub fn ms(&self) -> u64 {
        Instant::now().duration_since(self.start).as_millis() as u64
    }

    /// Seconds on the virtual clock, as a sender's clock gives them.
    pub fn secs(&self) -> u64 {
        1_000_000 + self.ms() / 1000
    }

    /// Notes what happened.
    pub fn note(&mut self, what: impl Into<String>) {
        let ms = self.ms();
        let step = self.step;
        self.trace.push(Trace {
            ms,
            step,
            what: what.into(),
        });
    }

    /// The chat a group is.
    pub fn chat_of(&self, group: &GroupKey) -> Option<usize> {
        self.chats.iter().position(|c| c.group == *group)
    }

    fn new_event(&mut self) -> EventKey {
        self.next_event += 1;
        EventKey(hex_id(0xe0, self.next_event))
    }

    fn new_order(&mut self) -> i64 {
        self.next_order += 1;
        self.next_order
    }

    /// Whether a crash is due at this effect; counts it otherwise.
    pub fn effect(&mut self, what: &str) -> bool {
        let Some(n) = self.crash_after else {
            return false;
        };
        if n <= 1 {
            self.crash_after = None;
            self.crashed = true;
            self.note(format!("Crash(before {what})"));
            return true;
        }
        self.crash_after = Some(n - 1);
        false
    }

    /// Stores the sends whose time has come.
    pub fn land(&mut self) {
        let now = Instant::now();
        let (due, later): (Vec<Landing>, Vec<Landing>) = std::mem::take(&mut self.landings)
            .into_iter()
            .partition(|l| l.at <= now);
        self.landings = later;
        for landing in due {
            let mut row = landing.row;
            row.order = self.new_order();
            row.ms = self.ms();
            self.note(format!("Send(landed late, {})", describe(&row)));
            self.rows.push(row);
        }
    }

    // ------------------------------------------------------------------------- people and MDK

    /// The message numbered `msg`, posted now if it was not yet: its content, and for an answer
    /// the chat's latest approval as the person's app has it now.
    pub fn post(&mut self, msg: usize) -> Option<&mut Msg> {
        let posted = *self.posted.get(msg)?;
        if self.msgs[msg].is_none() {
            let chat = usize::from(posted.chat);
            if chat >= self.chats.len() {
                return None;
            }
            let event = self.new_event();
            let (content, tags) = match posted.what {
                What::Prompt { approval } => {
                    let text = if approval {
                        format!("p{msg}a")
                    } else {
                        format!("p{msg}")
                    };
                    (text, Vec::new())
                }
                what => {
                    let (target, asked) = match what {
                        What::Answer => (
                            self.latest_approval(chat),
                            Asked::Choice {
                                choice: format!("allow-{msg}"),
                            },
                        ),
                        What::Interrupt => (None, Asked::Interrupt),
                        What::NewSession => (None, Asked::NewSession),
                        _ => (
                            None,
                            Asked::Set {
                                setting: "mode".into(),
                                value: format!("v{msg}"),
                            },
                        ),
                    };
                    let out = carrier::request(&self.agent, target.as_ref(), &asked, "asked");
                    (out.content, out.tags)
                }
            };
            let recorded_at = self.secs();
            self.msgs[msg] = Some(Msg {
                posted,
                event,
                content,
                tags,
                recorded_at,
                stored: None,
                heard: false,
                deferred: None,
                read_ms: None,
            });
        }
        self.msgs[msg].as_mut()
    }

    /// The first event of the latest approval the agent sent to a chat.
    fn latest_approval(&self, chat: usize) -> Option<EventKey> {
        let mut firsts: BTreeMap<String, EventKey> = BTreeMap::new();
        let mut latest = None;
        for row in self.rows.iter().filter(|r| r.chat == chat && r.from_agent) {
            if row.kind != ITEM_KIND {
                continue;
            }
            let Some(read) = carrier::read_item(&row.content, &row.tags) else {
                continue;
            };
            let id = read.item.event.header().item.as_str().to_string();
            let first = read.item.first.clone().unwrap_or_else(|| row.event.clone());
            firsts.entry(id.clone()).or_insert(first);
            if matches!(read.item.event.body(), Body::Approval(_)) {
                latest = Some(id);
            }
        }
        latest.and_then(|id| firsts.get(&id).cloned())
    }

    /// MDK stores a message (`Store`). One for a chat not yet accepted waits for it.
    pub fn store(&mut self, msg: usize) {
        let Some(m) = self.post(msg) else { return };
        if m.stored.is_some() {
            return;
        }
        let chat = usize::from(m.posted.chat);
        if !self.chats[chat].accepted {
            let m = self.msgs[msg].as_mut().expect("posted");
            if m.deferred.is_none() {
                m.deferred = Some(false);
                self.note(format!("Store(m{msg}, waits for the chat)"));
            }
            return;
        }
        let order = self.new_order();
        let ms = self.ms();
        let m = self.msgs[msg].as_mut().expect("posted");
        m.stored = Some(order);
        let row = Row {
            order,
            chat,
            event: m.event.clone(),
            from_agent: false,
            kind: if m.posted.is_request() {
                REQUEST_KIND
            } else {
                CHAT
            },
            content: m.content.clone(),
            tags: m.tags.clone(),
            recorded_at: m.recorded_at,
            ms,
            edits: None,
            msg: Some(msg),
        };
        self.rows.push(row);
        self.note(format!("Store(m{msg})"));
    }

    /// A message arrives live (`Hear`, or `Ask` for a request). Within MDK's contract it is
    /// stored first, and every message of its chat stored before it and not yet heard is heard
    /// first, in order.
    pub fn hear(&mut self, msg: usize) {
        let Some(m) = self.post(msg) else { return };
        if m.heard {
            return;
        }
        let chat = usize::from(m.posted.chat);
        if !self.chats[chat].accepted {
            let m = self.msgs[msg].as_mut().expect("posted");
            if m.deferred != Some(true) {
                m.deferred = Some(true);
                self.note(format!("Hear(m{msg}, waits for the chat)"));
            }
            return;
        }
        if self.contract {
            self.store(msg);
            let order = self.msgs[msg].as_ref().and_then(|m| m.stored);
            let earlier: Vec<usize> = (0..self.msgs.len())
                .filter(|&i| {
                    self.msgs[i].as_ref().is_some_and(|m| {
                        usize::from(m.posted.chat) == chat
                            && !m.heard
                            && m.stored.is_some()
                            && m.stored <= order
                    })
                })
                .collect();
            let mut earlier = earlier;
            earlier.sort_by_key(|&i| self.msgs[i].as_ref().and_then(|m| m.stored));
            for i in earlier {
                self.deliver_live(i);
            }
        } else {
            self.deliver_live(msg);
        }
    }

    fn deliver_live(&mut self, msg: usize) {
        let m = self.msgs[msg].as_mut().expect("posted");
        m.heard = true;
        let name = if m.posted.is_request() { "Ask" } else { "Hear" };
        let message = Message {
            group: self.chats[usize::from(m.posted.chat)].group.clone(),
            event: m.event.clone(),
            sender: self.owner.clone(),
            sender_name: Some("Owner".into()),
            kind: if m.posted.is_request() {
                REQUEST_KIND
            } else {
                CHAT
            },
            content: m.content.clone(),
            tags: m.tags.clone(),
            recorded_at: m.recorded_at,
            order: None,
        };
        if self.listening {
            self.live.push_back(Live::Message(message));
            self.note(format!("{name}(m{msg})"));
        } else {
            self.note(format!("{name}(m{msg}, lost: no core)"));
        }
    }

    /// The live stream lags: what the core has not taken yet is lost, and it is told.
    pub fn lag(&mut self) {
        if !self.listening {
            return;
        }
        let lost = self.live.len();
        self.live.retain(|l| !matches!(l, Live::Message(_)));
        self.live.push_back(Live::Lagged);
        self.note(format!("Lag({lost} lost)"));
    }

    /// The agent is invited to a chat.
    pub fn invite(&mut self, chat: usize) {
        let Some(c) = self.chats.get_mut(chat) else {
            return;
        };
        if c.accepted || c.invited {
            return;
        }
        c.invited = true;
        if self.listening {
            self.live.push_back(Live::Groups);
        }
        self.note(format!("Invite(c{chat})"));
    }

    /// The agent accepted a chat: what waited for it is stored, and heard if it was to be.
    pub fn accepted(&mut self, chat: usize) {
        self.chats[chat].invited = false;
        self.chats[chat].accepted = true;
        self.note(format!("Accept(c{chat})"));
        let waiting: Vec<(usize, bool)> = (0..self.msgs.len())
            .filter_map(|i| {
                let m = self.msgs[i].as_ref()?;
                (usize::from(m.posted.chat) == chat)
                    .then_some(())
                    .and(m.deferred.map(|hear| (i, hear)))
            })
            .collect();
        for (msg, _) in &waiting {
            self.msgs[*msg].as_mut().expect("posted").deferred = None;
            self.store(*msg);
        }
        for (msg, hear) in waiting {
            if hear {
                self.hear(msg);
            }
        }
    }

    /// Stores what the agent sent.
    pub fn store_sent(
        &mut self,
        chat: usize,
        kind: u64,
        content: String,
        tags: Vec<Vec<String>>,
        edits: Option<EventKey>,
    ) -> Row {
        Row {
            order: 0,
            chat,
            event: self.new_event(),
            from_agent: true,
            kind,
            content,
            tags,
            recorded_at: self.secs(),
            ms: self.ms(),
            edits,
            msg: None,
        }
    }

    /// Puts a row in storage now.
    pub fn keep(&mut self, mut row: Row) -> EventKey {
        row.order = self.new_order();
        row.ms = self.ms();
        let event = row.event.clone();
        self.rows.push(row);
        event
    }

    // ------------------------------------------------------------------------------ the harness

    /// Starts the harness's next life, and gives where its events come from.
    pub fn new_life(&mut self) -> (u32, mpsc::UnboundedReceiver<Output>) {
        let h = &mut self.harness;
        h.life += 1;
        let (out, events) = mpsc::unbounded_channel();
        h.out = Some(out);
        h.moves.clear();
        h.sessions.clear();
        h.taken.clear();
        h.scripts.clear();
        h.effects.clear();
        h.hung = false;
        (h.life, events)
    }

    fn next_move(&mut self) -> Option<Move> {
        self.harness.moves.pop_front()
    }

    fn decided(&mut self, what: &str, result: Result<(), Refusal>, mv: Option<Move>) -> Decision {
        match mv {
            Some(Move::Hang) => {
                self.harness.hung = true;
                self.note(format!("Deliver({what}), hangs"));
                Decision::Hang
            }
            Some(Move::Slow(ms)) => {
                self.note(format!(
                    "Deliver({what}), {} after {ms} ms",
                    verdict(&result)
                ));
                Decision::Slow(ms, result)
            }
            _ => {
                self.note(format!("Deliver({what}), {}", verdict(&result)));
                Decision::Now(result)
            }
        }
    }

    /// The harness is handed a prompt.
    pub fn prompt(&mut self, life: u32, prompt: &Prompt) -> Decision {
        let msg = msg_number(&prompt.text, "p");
        if life != self.harness.life {
            return Decision::Hang;
        }
        if self.effect("Deliver") {
            if let Some(msg) = msg {
                self.harness.lost.push(msg);
            }
            return Decision::Crash;
        }
        let chat = prompt.chat.id.0.clone();
        let session = prompt.session.as_ref().map(|s| s.as_str().to_string());
        let ms = self.ms();
        let chat_taken = self.harness.taken.contains(&chat);
        self.harness.deliveries.push(Delivery {
            life,
            ms,
            msg,
            kind: "prompt",
            chat: Some(chat.clone()),
            session: session.clone(),
            chat_taken,
            item: None,
        });
        let mv = self.next_move();
        let what = format!("prompt m{}", msg.map_or("?".into(), |m| m.to_string()));
        if mv == Some(Move::Refuse) {
            return self.decided(&what, Err(Refusal::new(Reason::Busy)), mv);
        }
        if mv != Some(Move::Hang) {
            self.harness.taken.insert(chat.clone());
        }
        if mv == Some(Move::Ignore) || mv == Some(Move::Hang) {
            return self.decided(&what, Ok(()), mv);
        }
        let h = &mut self.harness;
        // A session named is resumed; otherwise this life's session for the chat goes on, or a
        // new one starts.
        let (session, start) = match (session, h.sessions.get(&chat)) {
            (Some(s), _) => (s, true),
            (None, Some(s)) => (s.clone(), false),
            (None, None) => {
                h.next += 1;
                let s = format!("s{}-{}", h.life, h.next);
                h.sessions_started.push((chat.clone(), s.clone()));
                (s, true)
            }
        };
        h.sessions.insert(chat.clone(), session.clone());
        h.next += 1;
        let turn = format!("t{}-{}", h.life, h.next);
        h.scripts.push(Script {
            chat,
            turn,
            session,
            prompt: prompt.message.0.clone(),
            approval: prompt.text.ends_with('a'),
            stage: if start {
                Stage::SessionStart
            } else {
                Stage::TurnStart
            },
            answer: None,
            interrupt: None,
        });
        self.decided(&what, Ok(()), mv)
    }

    /// The harness is handed an answer.
    pub fn answer(&mut self, life: u32, answer: &Answer) -> Decision {
        if life != self.harness.life {
            return Decision::Hang;
        }
        let msg = match &answer.given {
            Given::Choice(choice) => msg_number(choice, "allow-"),
            Given::Questions(_) => None,
        };
        if self.effect("Deliver") {
            return Decision::Crash;
        }
        let ms = self.ms();
        let item = answer.item.as_str().to_string();
        let chat = (self.harness.scripts.iter())
            .find(|s| format!("appr-{}", s.turn) == item)
            .map(|s| s.chat.clone());
        self.harness.deliveries.push(Delivery {
            life,
            ms,
            msg,
            kind: "answer",
            chat,
            session: None,
            chat_taken: false,
            item: Some(item.clone()),
        });
        let mv = self.next_move();
        let what = format!("answer m{}", msg.map_or("?".into(), |m| m.to_string()));
        if mv == Some(Move::Refuse) {
            return self.decided(&what, Err(Refusal::new(Reason::Busy)), mv);
        }
        if mv == Some(Move::Hang) {
            return self.decided(&what, Ok(()), mv);
        }
        let request = answer.request.0.clone();
        let Some(script) = (self.harness.scripts.iter_mut()).find(|s| {
            format!("appr-{}", s.turn) == item && s.stage == Stage::Waiting && s.answer.is_none()
        }) else {
            return self.decided(&what, Err(Refusal::new(Reason::NotWaiting)), mv);
        };
        script.answer = Some((request.clone(), mv == Some(Move::Ignore)));
        if let Some(msg) = msg {
            self.harness.requests.insert(request, (msg, ms));
        }
        self.decided(&what, Ok(()), mv)
    }

    /// The harness is handed a control.
    pub fn control(&mut self, life: u32, control: &Control) -> Decision {
        if life != self.harness.life {
            return Decision::Hang;
        }
        let (kind, msg, chat) = match &control.action {
            DriverAction::Interrupt { .. } => ("interrupt", None, None),
            DriverAction::NewSession { chat, .. } => ("new session", None, Some(chat.id.0.clone())),
            DriverAction::Set { value, .. } => ("set", msg_number(value, "v"), None),
            DriverAction::Own { .. } => ("own", None, None),
        };
        if self.effect("Deliver") {
            return Decision::Crash;
        }
        let ms = self.ms();
        self.harness.deliveries.push(Delivery {
            life,
            ms,
            msg,
            kind: "control",
            chat: chat.clone(),
            session: None,
            chat_taken: false,
            item: None,
        });
        let mv = self.next_move();
        let what = format!("{kind} m{}", msg.map_or("?".into(), |m| m.to_string()));
        if mv == Some(Move::Refuse) {
            return self.decided(&what, Err(Refusal::new(Reason::Busy)), mv);
        }
        if mv == Some(Move::Hang) {
            return self.decided(&what, Ok(()), mv);
        }
        let request = control.request.0.clone();
        let ignore = mv == Some(Move::Ignore);
        let h = &mut self.harness;
        let result = match &control.action {
            DriverAction::Interrupt { turn } => {
                match (h.scripts.iter_mut())
                    .find(|s| s.turn == turn.as_str() && s.stage != Stage::Done)
                {
                    Some(script) => {
                        if !ignore {
                            script.interrupt = Some(request);
                        }
                        Ok(())
                    }
                    None => Err(Refusal::new(Reason::NothingToDo)),
                }
            }
            DriverAction::NewSession { chat, session } => {
                let chat = chat.id.0.clone();
                h.taken.insert(chat.clone());
                let old = (h.sessions.get(&chat).cloned())
                    .or_else(|| session.as_ref().map(|s| s.as_str().to_string()));
                if !ignore {
                    h.effects.push(Effecting::NewSession { request, chat, old });
                } else {
                    h.sessions.remove(&chat);
                }
                Ok(())
            }
            DriverAction::Set { session, .. } => {
                let chat = (h.sessions.iter())
                    .find(|(_, s)| s.as_str() == session.as_str())
                    .map(|(c, _)| c.clone())
                    .or_else(|| {
                        (h.sessions_started.iter())
                            .find(|(_, s)| s.as_str() == session.as_str())
                            .map(|(c, _)| c.clone())
                    });
                // Shown by the session's item or by the agent's words, or by nothing, which
                // leaves it to the core's net.
                let variant = msg.unwrap_or(0) % 3;
                if let (Some(chat), false, true) = (chat, ignore, variant != 0) {
                    let by_words =
                        variant == 2 || !h.sessions.values().any(|s| s == session.as_str());
                    h.effects.push(Effecting::Set {
                        request: request.clone(),
                        chat,
                        by_words,
                    });
                }
                if let Some(msg) = msg {
                    h.requests.insert(request, (msg, ms));
                }
                Ok(())
            }
            DriverAction::Own { .. } => Err(Refusal::new(Reason::NotSupported)),
        };
        self.decided(&what, result, mv)
    }

    /// Whether the harness has anything left to emit.
    pub fn has_work(&self) -> bool {
        self.harness.out.is_some()
            && (self.harness.scripts.iter().any(|s| s.stage != Stage::Done)
                || !self.harness.effects.is_empty())
    }

    /// The harness emits the next event of one of its turns or effects (`Emit`). With `force`, an
    /// approval no one answered is given up on.
    pub fn emit(&mut self, pick: u32, force: bool) {
        if self.harness.out.is_none() || (self.faults.harness_stalls && self.step.is_none()) {
            return;
        }
        let turns: Vec<usize> = (0..self.harness.scripts.len())
            .filter(|&i| self.harness.scripts[i].stage != Stage::Done)
            .collect();
        let effects = self.harness.effects.len();
        let n = turns.len() + effects;
        if n == 0 {
            return;
        }
        let chosen = pick as usize % n;
        let variant = (pick as usize / n) % 2;
        if chosen < turns.len() {
            self.emit_turn(turns[chosen], variant, force);
        } else {
            let effect = self.harness.effects.remove(chosen - turns.len());
            self.emit_effect(effect);
        }
    }

    fn emit_turn(&mut self, at: usize, variant: usize, force: bool) {
        let script = self.harness.scripts[at].clone();
        let turn_item = format!("turn-{}", script.turn);
        let approval_item = format!("appr-{}", script.turn);
        let words_item = format!("words-{}", script.turn);
        if let Some(request) = &script.interrupt {
            if script.stage == Stage::Waiting {
                self.send_event(
                    &script,
                    &approval_item,
                    State::Cancelled,
                    approval(),
                    Vec::new(),
                    "",
                );
            }
            let settles = vec![request.clone()];
            self.send_event(
                &script,
                &turn_item,
                State::Cancelled,
                turn(),
                settles,
                "Stopped",
            );
            self.harness.scripts[at].stage = Stage::Done;
            return;
        }
        let next = match script.stage {
            Stage::SessionStart if self.harness.ended.contains(&script.session) => {
                // A new session ended this one before the turn began: the turn goes to the
                // chat's session now, which has started already.
                let current = self.harness.sessions.get(&script.chat).cloned();
                self.harness.scripts[at].session = current.unwrap_or_default();
                Stage::TurnStart
            }
            Stage::SessionStart => {
                self.send_session(&script.chat, &script.session, State::Running, Vec::new());
                Stage::TurnStart
            }
            Stage::TurnStart => {
                self.send_event(
                    &script,
                    &turn_item,
                    State::Running,
                    turn(),
                    Vec::new(),
                    "Working",
                );
                if script.approval {
                    Stage::ApprovalOpen
                } else {
                    Stage::Words
                }
            }
            Stage::ApprovalOpen => {
                self.send_event(
                    &script,
                    &approval_item,
                    State::Waiting,
                    approval(),
                    Vec::new(),
                    "May I?",
                );
                Stage::Waiting
            }
            Stage::Waiting => match &script.answer {
                Some((request, true)) => {
                    let _ = request;
                    self.send_event(
                        &script,
                        &approval_item,
                        State::Done,
                        approval(),
                        Vec::new(),
                        "",
                    );
                    Stage::Words
                }
                Some((request, false)) if variant == 0 => {
                    let settles = vec![request.clone()];
                    self.send_event(
                        &script,
                        &approval_item,
                        State::Done,
                        approval(),
                        settles,
                        "",
                    );
                    Stage::Words
                }
                Some((request, false)) => {
                    // The agent's words show the answer's effect, then the approval ends
                    // without naming it.
                    let settles = vec![request.clone()];
                    let text = format!("w-{}-ok", script.turn);
                    let item = format!("ok-{}", script.turn);
                    self.send_event(&script, &item, State::Done, words(), settles, &text);
                    self.send_event(
                        &script,
                        &approval_item,
                        State::Done,
                        approval(),
                        Vec::new(),
                        "",
                    );
                    Stage::Words
                }
                None if force => {
                    self.send_event(
                        &script,
                        &approval_item,
                        State::Cancelled,
                        approval(),
                        Vec::new(),
                        "",
                    );
                    Stage::Words
                }
                None => Stage::Waiting,
            },
            Stage::Words => {
                let text = format!("w-{}", script.turn);
                self.send_event(
                    &script,
                    &words_item,
                    State::Done,
                    words(),
                    Vec::new(),
                    &text,
                );
                Stage::TurnEnd
            }
            Stage::TurnEnd => {
                self.send_event(&script, &turn_item, State::Done, turn(), Vec::new(), "Done");
                Stage::Done
            }
            Stage::Done => Stage::Done,
        };
        self.harness.scripts[at].stage = next;
    }

    fn emit_effect(&mut self, effect: Effecting) {
        match effect {
            Effecting::NewSession { request, chat, old } => {
                if let Some(old) = old {
                    self.send_session(&chat, &old, State::Done, Vec::new());
                    self.harness.ended.insert(old);
                }
                let h = &mut self.harness;
                h.next += 1;
                let new = format!("s{}-{}", h.life, h.next);
                h.sessions_started.push((chat.clone(), new.clone()));
                h.sessions.insert(chat.clone(), new.clone());
                self.send_session(&chat, &new, State::Running, vec![request]);
            }
            Effecting::Set {
                request,
                chat,
                by_words,
            } => {
                let session = self.harness.sessions.get(&chat).cloned();
                match session {
                    Some(session) if !by_words => {
                        self.send_session(&chat, &session, State::Running, vec![request]);
                    }
                    _ => {
                        self.harness.next += 1;
                        let n = self.harness.next;
                        let script = Script {
                            chat,
                            turn: String::new(),
                            session: session.unwrap_or_default(),
                            prompt: String::new(),
                            approval: false,
                            stage: Stage::Done,
                            answer: None,
                            interrupt: None,
                        };
                        let item = format!("set-{}-{n}", self.harness.life);
                        let text = format!("w-set-{}-{n}", self.harness.life);
                        self.send_event(&script, &item, State::Done, words(), vec![request], &text);
                    }
                }
            }
        }
    }

    fn send_session(&mut self, chat: &str, session: &str, state: State, settles: Vec<String>) {
        let script = Script {
            chat: chat.to_string(),
            turn: String::new(),
            session: session.to_string(),
            prompt: String::new(),
            approval: false,
            stage: Stage::Done,
            answer: None,
            interrupt: None,
        };
        let body = Body::Session(Session {
            harness_version: "sim".into(),
            settings: Vec::new(),
            actions: Vec::new(),
            commands: Vec::new(),
        });
        self.send_event(
            &script,
            &format!("sess-{session}"),
            state,
            body,
            settles,
            "Session",
        );
    }

    fn send_event(
        &mut self,
        script: &Script,
        item: &str,
        state: State,
        body: Body,
        settles: Vec<String>,
        text: &str,
    ) {
        self.harness.seq += 1;
        let is_turn = matches!(body, Body::Turn(_));
        let event_body_kind = if matches!(body, Body::Message(_)) {
            BodyKind::Words
        } else {
            BodyKind::Item
        };
        let is_session = matches!(body, Body::Session(_));
        let header = Header {
            harness: "sim".into(),
            session: (!script.session.is_empty()).then(|| script.session.as_str().into()),
            turn: (!script.turn.is_empty() && !is_session).then(|| script.turn.as_str().into()),
            item: item.into(),
            seq: self.harness.seq,
            state,
            title: item.into(),
            text: text.into(),
        };
        let event = AgentEvent::new(header, body, None).expect("no secret");
        let prompts = if is_turn && state == State::Running {
            vec![marmot_agent_driver::MessageId(script.prompt.clone())]
        } else {
            Vec::new()
        };
        let emitted = Emitted {
            event,
            chat: Some(marmot_agent_driver::ChatId(script.chat.clone())),
            prompts,
            settles: (settles.iter())
                .map(|r| marmot_agent_driver::RequestId(r.clone()))
                .collect(),
        };
        let ms = self.ms();
        let life = self.harness.life;
        self.harness.emissions.push(Emission {
            life,
            ms,
            chat: script.chat.clone(),
            item: item.to_string(),
            settles: settles.clone(),
            session: is_session.then(|| script.session.clone()),
            state,
            words: matches!(event_body_kind, BodyKind::Words).then(|| text.to_string()),
        });
        let named = if settles.is_empty() {
            String::new()
        } else {
            format!(", settles {}", settles.join(" "))
        };
        self.note(format!("Emit({item} {}{named})", state_word(state)));
        if let Some(out) = &self.harness.out {
            let _ = out.send(Output::from(emitted));
        }
    }

    /// The driver stops (`DriverStops`): no more events come, and the core sees it end.
    pub fn driver_stops(&mut self) {
        if self.harness.out.take().is_some() {
            self.note("DriverStops");
        }
    }
}

/// Whether an event goes out as the agent's words or as an item.
enum BodyKind {
    Words,
    Item,
}

/// A short, private-free description of a stored row, for the trace.
pub fn describe(row: &Row) -> String {
    if row.kind == ITEM_KIND
        && let Some(read) = carrier::read_item(&row.content, &row.tags)
    {
        let h = read.item.event.header();
        return format!(
            "c{} {} v{} {}",
            row.chat,
            h.item.as_str(),
            read.item.version,
            state_word(h.state)
        );
    }
    if row.kind == EDIT {
        return format!("c{} edit", row.chat);
    }
    format!("c{} words {}", row.chat, row.content)
}

/// A state's word.
pub fn state_word(state: State) -> &'static str {
    match state {
        State::Running => "running",
        State::Waiting => "waiting",
        State::Done => "done",
        State::Failed => "failed",
        State::Declined => "declined",
        State::Cancelled => "cancelled",
    }
}

fn verdict(result: &Result<(), Refusal>) -> &'static str {
    match result {
        Ok(()) => "Take",
        Err(_) => "Refuse",
    }
}

/// The number in a text the simulator wrote, after `prefix`.
fn msg_number(text: &str, prefix: &str) -> Option<usize> {
    let rest = text.strip_prefix(prefix)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn turn() -> Body {
    Body::Turn(Turn {
        started_by: StartedBy::User,
        duration_ms: None,
        usage: None,
    })
}

fn approval() -> Body {
    Body::Approval(Approval {
        subject: None,
        choices: vec![Choice {
            id: "allow".into(),
            label: "Allow".into(),
            effect: Effect::Allow,
        }],
        answer: None,
    })
}

fn words() -> Body {
    Body::Message(Words::default())
}
