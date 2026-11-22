//! The checks: what a driver must keep to, looked at after every step.
//!
//! The checker sees what the suite asks of the driver and what it answers, every output from
//! [`next_event`](marmot_agent_driver::Driver::next_event), and what the fake harness heard. It
//! knows nothing of any harness. Each failure names the property it breaks, by the id
//! `docs/design/verification.md` gives it:
//!
//! | Id | What is checked here |
//! | --- | --- |
//! | S1 | Each answer and control is settled at most once: by a refusal, or by the `settles` of one event that is not a message, of its own chat. An event the driver made up, with no native message (D-059, point 3), such as an item it closes when the harness stops, shows no effect and settles nothing, unless it is an error that settles it as failed. At the end, each request the driver took is settled, unless the driver ended, when the core settles it. A request from before a restart is never settled after it. |
//! | S2 | After the harness stops, every item of the chats it served is closed, but for the session's own item: a session outlives its harness (Pi's driver takes it up again in a new Pi), and when the driver ends the core closes what is left (D-034). |
//! | S4 | No prompt reaches the harness twice. |
//! | S5 | Every event names its chat, and a session, a turn and an item stay with the chat they were first seen in. An event whose text carries a chat's mark is that chat's, as is every prompt a turn names. An answer reaches the harness in the session of its own chat, and with what was chosen. |
//! | S11 | A request returns, and once it has returned the harness has it, with the driver not polled again. No line the harness reads is half a message. Dropping the wait for an event at any point loses nothing (the scenario that drops waits checks what arrives). |
//! | S12 | The harness takes up a session again only when a prompt named it, or when it is the chat's own current session (a driver restarting a harness that died). Never a session no chat had. |
//! | S13 | An item id, once closed, is never used again in its chat, and an id from before a restart is never used after it; only a session's own item returns with its session. An item keeps its session. |
//! | SEQ | `seq` rises within a session (D-059, point 4). Not in the property table; named here so a search finds it. |

use crate::fake::Heard;
use marmot_agent_driver::{ChatId, Emitted, MessageId, Output, Refusal, RequestId};
use marmot_agent_events::{Body, State};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// A property the driver broke, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// The property's id, such as `S1`.
    pub property: &'static str,
    /// What happened, in the suite's words: ids and kinds, never a value from the harness.
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.property, self.detail)
    }
}

impl std::error::Error for Violation {}

/// A violation of `property`.
pub fn broke(property: &'static str, detail: impl Into<String>) -> Violation {
    Violation {
        property,
        detail: detail.into(),
    }
}

/// The mark a chat's prompts carry, so what the harness says in answer can be traced to it.
pub fn mark(chat: &str) -> String {
    format!("<<{chat}>>")
}

/// The chat whose mark a text carries.
pub fn marked_chat(text: &str) -> Option<String> {
    let start = text.find("<<")? + 2;
    let end = start + text[start..].find(">>")?;
    Some(text[start..end].to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Taken {
    Waiting,
    Yes,
    Refused,
}

#[derive(Debug)]
struct Request {
    chat: Option<ChatId>,
    taken: Taken,
    settled: u32,
    /// Made of a driver before a restart.
    old: bool,
}

#[derive(Debug)]
struct Item {
    open: bool,
    session: Option<String>,
    session_item: bool,
}

/// What the suite knows, and what it checks after every step.
#[derive(Default)]
pub struct Checker {
    requests: HashMap<RequestId, Request>,
    prompts: HashMap<MessageId, ChatId>,
    /// Sessions a prompt named, by chat.
    named: HashMap<ChatId, HashSet<String>>,
    owners: HashMap<String, ChatId>,
    turns: HashMap<String, ChatId>,
    items: HashMap<(ChatId, String), Item>,
    /// Item ids from before a restart, with the session of a session's item.
    retired: HashMap<(ChatId, String), Option<String>>,
    /// Each chat's current session, in this driver's life.
    current: HashMap<ChatId, String>,
    seq: HashMap<String, u64>,
    /// Chats whose harness has stopped; `everything` when the whole harness did.
    stopped: HashSet<ChatId>,
    everything: bool,
    ended: bool,
    /// What answers the harness should hear: the chat, and what was chosen.
    expected: Vec<(ChatId, Option<bool>)>,
    /// How much of what the harness heard has been checked.
    heard_at: usize,
    prompts_heard: HashSet<String>,
    violations: Vec<Violation>,
}

impl Checker {
    /// A checker that has seen nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every violation so far, in order.
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }

    /// The first violation, if any.
    pub fn first(&self) -> Option<&Violation> {
        self.violations.first()
    }

    fn fail(&mut self, property: &'static str, detail: impl Into<String>) {
        self.violations.push(broke(property, detail));
    }

    /// Records a violation the suite found itself, such as a wait that ran out.
    pub fn record(&mut self, violation: Violation) {
        self.violations.push(violation);
    }

    /// The chat that owns a session, as the events showed.
    pub fn owner(&self, session: &str) -> Option<&ChatId> {
        self.owners.get(session)
    }

    /// The chat's current session, as the events showed.
    pub fn session_of(&self, chat: &ChatId) -> Option<&String> {
        self.current.get(chat)
    }

    // -- what the suite asks

    /// A prompt is about to be handed to the driver.
    pub fn prompting(&mut self, message: &MessageId, chat: &ChatId, session: Option<&str>) {
        self.prompts.insert(message.clone(), chat.clone());
        if let Some(session) = session {
            let named = self.named.entry(chat.clone()).or_default();
            named.insert(session.to_string());
        }
    }

    /// An answer or a control for `chat` is about to be handed to the driver.
    pub fn requesting(&mut self, request: &RequestId, chat: Option<&ChatId>) {
        self.requests.insert(
            request.clone(),
            Request {
                chat: chat.cloned(),
                taken: Taken::Waiting,
                settled: 0,
                old: false,
            },
        );
    }

    /// The harness should hear an answer in `chat`'s session: allowed or not, for an approval.
    pub fn expect_answer(&mut self, chat: &ChatId, allow: Option<bool>) {
        self.expected.push((chat.clone(), allow));
    }

    /// The driver answered an answer or a control.
    pub fn requested(&mut self, request: &RequestId, result: &Result<(), Refusal>) {
        let Some(r) = self.requests.get_mut(request) else {
            return;
        };
        r.taken = if result.is_ok() {
            Taken::Yes
        } else {
            Taken::Refused
        };
        if result.is_err() && r.settled > 0 {
            self.fail(
                "S1",
                format!("{request:?} was refused after an event settled it"),
            );
        }
    }

    /// The whole harness has stopped, or only `chat`'s.
    pub fn harness_stopped(&mut self, chat: Option<&ChatId>) {
        match chat {
            Some(chat) => {
                self.stopped.insert(chat.clone());
            }
            None => self.everything = true,
        }
    }

    /// The harness of `chat` runs again, as it does when Pi's driver starts a chat's Pi anew.
    pub fn harness_back(&mut self, chat: &ChatId) {
        self.stopped.remove(chat);
    }

    fn is_stopped(&self, chat: &ChatId) -> bool {
        self.everything || self.stopped.contains(chat)
    }

    /// The driver is dropped and another starts. What the old one made is from before the restart.
    pub fn restarted(&mut self) {
        for ((chat, item), it) in self.items.drain() {
            let session = it.session_item.then_some(it.session).flatten();
            self.retired.insert((chat, item), session);
        }
        for request in self.requests.values_mut() {
            request.old = true;
        }
        self.current.clear();
        self.seq.clear();
        self.named.clear();
        self.stopped.clear();
        self.everything = false;
        self.ended = false;
    }

    // -- what the driver hands over

    /// The driver's next output, or its end.
    pub fn output(&mut self, output: Option<&Output>) {
        match output {
            None => self.ended = true,
            Some(Output::SendFile(_)) => {}
            Some(Output::Event(emitted)) => self.event(emitted),
        }
    }

    fn event(&mut self, e: &Emitted) {
        let header = e.event.header();
        let item = header.item.as_str().to_string();
        let Some(chat) = e.chat.clone() else {
            self.fail("S5", format!("item {item} names no chat"));
            return;
        };
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        let turn = header.turn.as_ref().map(|t| t.as_str().to_string());
        let open = matches!(header.state, State::Running | State::Waiting);
        let session_item = matches!(e.event.body(), Body::Session(_));

        // S5: a session, a turn, an item and a mark stay with their chat.
        if let Some(s) = &session {
            match self.owners.get(s) {
                Some(owner) if *owner != chat => {
                    let detail = format!("session of {owner:?} shown in {chat:?} (item {item})");
                    self.fail("S5", detail);
                }
                Some(_) => {}
                None => {
                    self.owners.insert(s.clone(), chat.clone());
                }
            }
            if session_item && header.state == State::Running {
                self.current.insert(chat.clone(), s.clone());
            }
        }
        if let Some(t) = &turn {
            match self.turns.get(t) {
                Some(owner) if *owner != chat => {
                    self.fail("S5", format!("turn of {owner:?} shown in {chat:?}"));
                }
                Some(_) => {}
                None => {
                    self.turns.insert(t.clone(), chat.clone());
                }
            }
        }
        if let Some(marked) = marked_chat(&header.text)
            && marked != chat.0
        {
            let detail = format!("words from chat {marked} shown in {chat:?} (item {item})");
            self.fail("S5", detail);
        }
        for prompt in &e.prompts {
            match self.prompts.get(prompt) {
                Some(owner) if *owner != chat => {
                    self.fail("S5", format!("{prompt:?} of {owner:?} named in {chat:?}"));
                }
                Some(_) => {}
                None => self.fail("S5", format!("{prompt:?} was never prompted")),
            }
        }

        // S1: what the event settles.
        for request in &e.settles {
            let Some(r) = self.requests.get_mut(request) else {
                self.fail("S1", format!("{request:?} settled, and never asked"));
                continue;
            };
            r.settled += 1;
            let (settled, taken, old, owner) = (r.settled, r.taken, r.old, r.chat.clone());
            if old {
                self.fail(
                    "S1",
                    format!("{request:?} from before a restart settled after it"),
                );
            }
            if settled > 1 {
                self.fail("S1", format!("{request:?} settled {settled} times"));
            }
            if taken == Taken::Refused {
                self.fail("S1", format!("{request:?} refused, then settled again"));
            }
            if owner.as_ref().is_some_and(|o| *o != chat) {
                let detail = format!("{request:?} of {owner:?} settled in {chat:?}");
                self.fail("S5", detail);
            }
            if matches!(e.event.body(), Body::Message(_)) {
                self.fail("S1", format!("{request:?} settled by a message"));
            }
            // An event the driver makes up carries no native message (D-059, point 3): the
            // harness said nothing, so it shows no effect. Only an error may settle a request,
            // as failed, as Pi's driver does when a chat's Pi stops before acting.
            let made_up = e.event.native().is_none();
            if made_up && !matches!(e.event.body(), Body::Error(_)) {
                let detail =
                    format!("{request:?} settled by item {item}, which the driver made up");
                self.fail("S1", detail);
            }
        }

        // S13: an id is never used again.
        let key = (chat.clone(), item.clone());
        if let Some(retired) = self.retired.get(&key) {
            let returning = session_item && retired.is_some() && *retired == session;
            if !returning && !self.items.contains_key(&key) {
                self.fail(
                    "S13",
                    format!("item {item} of {chat:?} used again after a restart"),
                );
            }
        }
        let mut problems = Vec::new();
        match self.items.get_mut(&key) {
            Some(it) => {
                if !it.open && !session_item {
                    problems.push(format!(
                        "item {item} of {chat:?} used again after it closed"
                    ));
                }
                if it.session != session {
                    problems.push(format!("item {item} moved to another session"));
                }
                it.open = open;
            }
            None => {
                self.items.insert(
                    key,
                    Item {
                        open,
                        session: session.clone(),
                        session_item,
                    },
                );
            }
        }
        for problem in problems {
            self.fail("S13", problem);
        }

        // SEQ: rising within a session.
        if let Some(s) = &session {
            let last = self.seq.insert(s.clone(), header.seq).unwrap_or(0);
            if header.seq <= last {
                let detail = format!("seq {} after {last} (item {item})", header.seq);
                self.fail("SEQ", detail);
            }
        }
    }

    // -- what the harness heard

    /// Checks what the harness heard so far: S4, S5 for prompts and answers, S11's whole lines
    /// and S12. Call it once the driver has handed over what it had.
    pub fn heard(&mut self, heard: &[Heard], malformed: &[String]) {
        if let Some(line) = malformed.first() {
            let detail = format!(
                "the harness read {} lines that are not whole messages, the first {} bytes long",
                malformed.len(),
                line.len()
            );
            self.fail("S11", detail);
        }
        let fresh = heard.get(self.heard_at..).unwrap_or_default();
        self.heard_at = heard.len();
        for h in fresh {
            match h {
                Heard::Prompt { session, text } => {
                    let message = text.split_whitespace().nth(1).unwrap_or_default();
                    if !self.prompts_heard.insert(message.to_string()) {
                        self.fail("S4", format!("prompt {message} reached the harness twice"));
                    }
                    let chat = marked_chat(text);
                    let owner = self.owners.get(session).map(|c| c.0.clone());
                    if let (Some(chat), Some(owner)) = (&chat, &owner)
                        && chat != owner
                    {
                        let detail =
                            format!("a prompt of chat {chat} reached a session of {owner}");
                        self.fail("S5", detail);
                    }
                }
                Heard::Opened {
                    session,
                    resumed: true,
                } => {
                    let Some(owner) = self.owners.get(session).cloned() else {
                        self.fail("S12", "a session no chat had was taken up");
                        continue;
                    };
                    let named = self.named.get(&owner).is_some_and(|n| n.contains(session));
                    let current = self.current.get(&owner) == Some(session);
                    if !named && !current {
                        let detail = format!("a session of {owner:?} taken up, unasked");
                        self.fail("S12", detail);
                    }
                }
                Heard::Answer { session, allow, .. } => {
                    let owner = self.owners.get(session).cloned();
                    let at = (self.expected.iter())
                        .position(|(chat, a)| Some(chat) == owner.as_ref() && a == allow);
                    match at {
                        Some(at) => {
                            self.expected.remove(at);
                        }
                        None => {
                            let detail = format!(
                                "the harness heard an answer in a session of {owner:?} that no one in that chat gave"
                            );
                            self.fail("S5", detail);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // -- at the end

    /// After the harness stopped and the driver handed over what it had: no item of a stopped
    /// chat is left open but its session's (S2).
    pub fn closed_after_stop(&mut self) {
        let open: Vec<String> = (self.items.iter())
            .filter(|(_, it)| it.open && !it.session_item)
            .filter(|((chat, _), _)| self.is_stopped(chat))
            .map(|((chat, item), _)| format!("{item} of {chat:?}"))
            .collect();
        if !open.is_empty() {
            let detail = format!("left open after the harness stopped: {}", open.join(", "));
            self.fail("S2", detail);
        }
    }

    /// Every answer and control the driver took is settled, unless the driver has ended, when the
    /// core settles what is left (S1). Requests from before a restart were the old driver's.
    pub fn all_settled(&mut self) {
        if self.ended {
            return;
        }
        let unsettled: Vec<String> = (self.requests.iter())
            .filter(|(_, r)| r.taken == Taken::Yes && r.settled == 0 && !r.old)
            .map(|(id, _)| format!("{id:?}"))
            .collect();
        if !unsettled.is_empty() {
            let detail = format!("taken and never settled: {}", unsettled.join(", "));
            self.fail("S1", detail);
        }
    }

    /// The harness heard every answer it should have.
    pub fn answers_heard(&mut self) {
        if !self.expected.is_empty() {
            let detail = format!("{} answers never reached the harness", self.expected.len());
            self.fail("S11", detail);
        }
    }
}
