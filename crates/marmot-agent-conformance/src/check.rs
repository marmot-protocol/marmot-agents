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
//! | S4 | No prompt reaches the harness twice; a copy the harness refused counts, since it reached it. |
//! | S5 | Every event names its chat, and a session, a turn and an item id stay with the chat they were first seen in. Every chat mark an event's text carries is its own chat's, as is every prompt a turn names. An answer reaches the harness in the session of its own chat, and with what was chosen: allowed or not, or the option's words. |
//! | S11 | A request returns, and once it has returned the harness has it, with the driver not polled again. No line the harness reads is half a message. Dropping the wait for an event at any point loses nothing (the scenario that drops waits checks what arrives). |
//! | S12 | The harness takes up a session again only when the chat's last prompt named it, or when it was the chat's current session as that prompt was made (a driver restarting a harness that died). Not a session an earlier prompt named, nor the session a resume itself just made current. Never a session no chat had. What the harness heard is checked before each prompt, against what the prompts before it allowed. |
//! | S13 | An item id, once closed, is never used again in its chat, and an id from before a restart is never used after it; only a session's own item returns with its session. An item keeps its session. |
//! | SEQ | `seq` rises within a session (D-059, point 4). Not in the property table; named here so a search finds it. |

use crate::fake::Heard;
use marmot_agent_driver::{ChatId, Emitted, MessageId, Output, Refusal, RequestId, SendFile};
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

/// The chat whose mark a text carries first: for a prompt, its own chat.
pub fn marked_chat(text: &str) -> Option<String> {
    marked_chats(text).into_iter().next()
}

/// Every chat whose mark a text carries, in order.
pub fn marked_chats(text: &str) -> Vec<String> {
    let mut chats = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<<") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(">>") else {
            break;
        };
        chats.push(after[..end].to_string());
        rest = &after[end + 2..];
    }
    chats
}

/// An answer the harness should hear.
#[derive(Debug)]
struct Expected {
    chat: ChatId,
    /// Allowed or not, for an approval.
    allow: Option<bool>,
    /// What was chosen or typed, for a question.
    text: Option<String>,
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
    /// The session the chat's last prompt named, if it named one: only that prompt may take it
    /// up.
    named: HashMap<ChatId, String>,
    owners: HashMap<String, ChatId>,
    turns: HashMap<String, ChatId>,
    /// The chat each item id was first seen in, across restarts.
    item_chats: HashMap<String, ChatId>,
    items: HashMap<(ChatId, String), Item>,
    /// Item ids from before a restart, with the session of a session's item.
    retired: HashMap<(ChatId, String), Option<String>>,
    /// Each chat's current session, in this driver's life.
    current: HashMap<ChatId, String>,
    /// Each chat's current session as its last prompt was made: what a driver may take up again
    /// unasked. Kept apart from `current`, which the resume's own events change.
    resumable: HashMap<ChatId, String>,
    seq: HashMap<String, u64>,
    /// Chats whose harness has stopped; `everything` when the whole harness did.
    stopped: HashSet<ChatId>,
    everything: bool,
    ended: bool,
    /// What answers the harness should hear.
    expected: Vec<Expected>,
    /// How each file the driver asked to send went, as the core told it, for the chat that asked:
    /// what the harness should hear as its tool's result.
    files: Vec<(ChatId, bool)>,
    /// How much of what the harness heard has been checked.
    heard_at: usize,
    prompts_heard: HashSet<String>,
    /// A half line has been reported.
    half_lines_seen: bool,
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

    /// A prompt is about to be handed to the driver. What the harness heard so far is checked
    /// first (`heard`, `malformed`), against what the earlier prompts allowed, since this one
    /// changes what a resume may take up.
    pub fn prompting(
        &mut self,
        message: &MessageId,
        chat: &ChatId,
        session: Option<&str>,
        heard: &[Heard],
        malformed: &[String],
    ) {
        self.heard(heard, malformed);
        self.prompts.insert(message.clone(), chat.clone());
        match self.current.get(chat) {
            Some(current) => self.resumable.insert(chat.clone(), current.clone()),
            None => self.resumable.remove(chat),
        };
        match session {
            Some(session) => self.named.insert(chat.clone(), session.to_string()),
            None => self.named.remove(chat),
        };
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

    /// The harness should hear an answer in `chat`'s session: allowed or not, for an approval;
    /// what was chosen or typed, for a question.
    pub fn expect_answer(&mut self, chat: &ChatId, allow: Option<bool>, text: Option<String>) {
        self.expected.push(Expected {
            chat: chat.clone(),
            allow,
            text,
        });
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
        self.resumable.clear();
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
        for marked in marked_chats(&header.text) {
            if marked != chat.0 {
                let detail = format!("words from chat {marked} shown in {chat:?} (item {item})");
                self.fail("S5", detail);
            }
        }
        match self.item_chats.get(&item) {
            Some(owner) if *owner != chat => {
                let detail = format!("an item of {owner:?} shown in {chat:?} (item {item})");
                self.fail("S5", detail);
            }
            Some(_) => {}
            None => {
                self.item_chats.insert(item.clone(), chat.clone());
            }
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
        self.half_lines(malformed);
        let fresh = heard.get(self.heard_at..).unwrap_or_default();
        self.heard_at = heard.len();
        for h in fresh {
            match h {
                Heard::Prompt { session, text, .. } => {
                    // A copy the harness refused reached it all the same.
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
                    ..
                } => {
                    let Some(owner) = self.owners.get(session).cloned() else {
                        self.fail("S12", "a session no chat had was taken up");
                        continue;
                    };
                    let named = self.named.get(&owner) == Some(session);
                    let current = self.resumable.get(&owner) == Some(session);
                    if !named && !current {
                        let detail = format!("a session of {owner:?} taken up, unasked");
                        self.fail("S12", detail);
                    }
                }
                Heard::FileResult { session, sent, .. } => {
                    let owner = self.owners.get(session).cloned();
                    let at = (self.files.iter())
                        .position(|(chat, s)| Some(chat) == owner.as_ref() && s == sent);
                    match at {
                        Some(at) => {
                            self.files.remove(at);
                        }
                        None => {
                            let detail = format!(
                                "a tool's result reached a session of {owner:?} that no send in that chat gave, or not as it went"
                            );
                            self.fail("S5", detail);
                        }
                    }
                }
                Heard::Answer {
                    session,
                    allow,
                    text,
                } => {
                    let owner = self.owners.get(session).cloned();
                    let at = self.expected.iter().position(|e| {
                        Some(&e.chat) == owner.as_ref()
                            && e.allow == *allow
                            && (e.text.is_none() || e.text == *text)
                    });
                    match at {
                        Some(at) => {
                            self.expected.remove(at);
                        }
                        None => {
                            let detail = format!(
                                "the harness heard an answer in a session of {owner:?} that no one in that chat gave, or not as given"
                            );
                            self.fail("S5", detail);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// S11: no line the harness read is half a message. Checked at every checkpoint, and before
    /// a wait that runs out is blamed, since a half line loses the message it swallowed.
    pub fn half_lines(&mut self, malformed: &[String]) {
        let Some(line) = malformed.first() else {
            return;
        };
        if self.half_lines_seen {
            return;
        }
        self.half_lines_seen = true;
        let detail = format!(
            "the harness read {} lines that are not whole messages, the first {} bytes long",
            malformed.len(),
            line.len()
        );
        self.fail("S11", detail);
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
        if !self.files.is_empty() {
            let detail = format!(
                "the harness never heard how {} files it asked to send went",
                self.files.len()
            );
            self.fail("D-090", detail);
        }
    }

    /// The core told the driver how a file `chat` asked to send went (D-090): the harness of that
    /// chat should hear it as its tool's result.
    pub fn expect_file(&mut self, chat: &ChatId, sent: bool) {
        self.files.push((chat.clone(), sent));
    }

    /// A file the driver asked to send names the chat whose harness asked, as far as the scenario
    /// can tell it (S5).
    pub fn file_asked(&mut self, file: &SendFile, asked_by: &ChatId) {
        if file.chat != *asked_by {
            let detail = format!(
                "a file asked for in {asked_by:?} was sent to {:?}",
                file.chat
            );
            self.fail("S5", detail);
        }
    }
}
