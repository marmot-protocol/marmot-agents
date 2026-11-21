//! What the core does with each message, answer, control and agent event.
//!
//! [`Conversation`] holds no connection to anything. The Marmot side hands it what arrived in a
//! group, the driver's events and results are handed to it too, and it answers with [`Act`]s: a
//! request for the driver, or something to send to a group. The program that runs the core
//! carries the acts out, one at a time, and tells the conversation how each went. That keeps
//! every rule about people, sessions and items testable without MDK or a harness.
//!
//! The rules it applies, by decision:
//!
//! - who the agent answers: everyone in a direct chat, and in a group only a message that
//!   mentions it or replies to it (D-075);
//! - who may do what: the right to prompt or to approve (D-010, D-032, D-039, D-043, D-047), and
//!   a secret answer only in a two-person chat (D-041);
//! - the ids a driver sees are the core's own (D-036);
//! - a prompt names a session only to resume one (D-065);
//! - each answer or control is settled once: by the event that shows its effect, or by a notice
//!   that says why it had none (D-028);
//! - a stopped driver, or a core that stopped, leaves no item open and no request unsettled
//!   (D-058, D-070).

use crate::carrier::{Asked, Item};
use crate::keys::{EventKey, GroupKey, PubKey};
use crate::rights::{Right, Rights, needed_for};
use marmot_agent_driver::{
    Action, Answer, Attachment, Capabilities, Chat, ChatId, Control, Emitted, Given, MessageId,
    Person, PersonId, Prompt, Quoted, Reason, Refusal, Reply, RequestId,
};
use marmot_agent_events::body::{Body, Notice, NoticeKind};
use marmot_agent_events::{AgentEvent, Header, ItemId, SessionId, State, TurnId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// A request older than this that points at nothing, such as "new session", is dropped (D-028).
pub const LATE_SECS: u64 = 5 * 60;
/// How many handled events are remembered, so that one delivered twice acts once.
const SEEN: usize = 4096;
/// How many finished items are kept, beyond those still open and each chat's session item.
const KEPT: usize = 2048;

/// A chat as the Marmot side knows it.
#[derive(Clone, PartialEq, Eq)]
pub struct Where {
    /// The group.
    pub group: GroupKey,
    /// The group's name, when it has one.
    pub name: Option<String>,
    /// Whether it is a direct chat: the agent and one person.
    pub direct: bool,
    /// How many members it has, the agent included.
    pub members: usize,
}

/// A chat message the agent received.
#[derive(Clone, PartialEq, Eq)]
pub struct Heard {
    /// Where it was sent.
    pub chat: Where,
    /// Its event.
    pub event: EventKey,
    /// Who wrote it.
    pub author: PubKey,
    /// Their name, as the model needs it.
    pub author_name: String,
    /// Its text.
    pub text: String,
    /// The message it replies to.
    pub reply_to: Option<HeardQuote>,
    /// Whether it mentions the agent.
    pub mentions_agent: bool,
    /// Files sent with it, already written to disk.
    pub attachments: Vec<Attachment>,
}

/// The message a chat message replies to.
#[derive(Clone, PartialEq, Eq)]
pub struct HeardQuote {
    /// Its event.
    pub event: EventKey,
    /// Who wrote it.
    pub author: PubKey,
    /// Their name.
    pub author_name: String,
    /// Its text.
    pub text: String,
}

/// An answer or a control the agent received (D-027).
#[derive(Clone, PartialEq, Eq)]
pub struct HeardRequest {
    /// Where it was sent.
    pub chat: Where,
    /// Its event.
    pub event: EventKey,
    /// Who sent it.
    pub author: PubKey,
    /// Their name.
    pub author_name: String,
    /// How old it is, in seconds.
    pub age_secs: u64,
    /// The first event of the item it acts on, when it names one.
    pub target: Option<EventKey>,
    /// What it asks.
    pub asked: Asked,
}

/// Something the program running the core is to do.
#[derive(Debug)]
pub enum Act {
    /// Hand a prompt to the driver, then call [`Conversation::prompted`].
    Prompt(Prompt),
    /// Hand an answer to the driver, then call [`Conversation::requested`].
    Answer(Answer),
    /// Hand a control to the driver, then call [`Conversation::requested`].
    Control(Control),
    /// Send something to a group, then call [`Conversation::sent`] with the event's id. The
    /// conversation counts on every send going out, since an item's later versions, and the
    /// requests they settle, follow from it: the program tries a failed send again until it goes
    /// out or the core stops.
    Send {
        /// The group.
        group: GroupKey,
        /// What to send.
        what: Sending,
    },
}

/// What to send to a group.
pub enum Sending {
    /// A version of an agent item.
    Item {
        /// The core's name for the item, handed back to [`Conversation::sent`].
        key: String,
        /// The item.
        item: Box<Item>,
    },
    /// The agent's words, a chat message (D-022).
    Words {
        /// The core's name for the item, handed back to [`Conversation::sent`].
        key: String,
        /// What the agent said.
        text: String,
        /// The turn they belong to.
        turn: Option<TurnId>,
        /// Whether the harness marked them as a remark made while it works, which does not
        /// alert (D-078).
        remark: bool,
        /// The message they replace, when the words changed after they were sent.
        edit: Option<EventKey>,
    },
}

/// Names what is sent and nothing it holds (D-071).
impl std::fmt::Debug for Sending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Item { item, .. } => f
                .debug_struct("Item")
                .field("event", &item.event)
                .field("version", &item.version)
                .finish_non_exhaustive(),
            Self::Words { edit, .. } => f
                .debug_struct("Words")
                .field("edit", &edit.is_some())
                .finish_non_exhaustive(),
        }
    }
}

/// What the core keeps across a restart.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// The core's id for each group it has seen (D-036).
    chats: BTreeMap<GroupKey, String>,
    /// The core's id for each person it has seen (D-036).
    people: BTreeMap<PubKey, String>,
    /// Each chat's session, by the core's chat id (D-065).
    sessions: BTreeMap<String, SessionId>,
    /// Every item sent and not yet forgotten, by the core's name for it.
    items: BTreeMap<String, Record>,
    /// The order finished items were finished in, oldest first, so the oldest are forgotten.
    finished: VecDeque<String>,
    /// Requests handed to the driver and not settled, by the core's id for them.
    pending: BTreeMap<String, Pending>,
    /// The next number for an id the core makes up.
    next: u64,
}

/// Prints how much is kept, and nothing of it (D-071).
impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved")
            .field("chats", &self.chats.len())
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

/// An item the core has sent.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Record {
    group: GroupKey,
    chat: String,
    /// The item's first event, once it was sent.
    first: Option<EventKey>,
    version: u64,
    /// The latest version the driver made.
    last: AgentEvent,
    /// The agent's words, sent as a chat message, and what they said when last sent.
    words: Option<String>,
    /// For a turn: the message that started it, carried by every version.
    #[serde(default)]
    started_by: Option<EventKey>,
}

impl Record {
    fn open(&self) -> bool {
        matches!(self.last.header().state, State::Running | State::Waiting)
    }
}

/// A request the driver was handed and that is not settled yet. Kept across a restart, so a core
/// that stopped settles it when it starts again (D-070).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Pending {
    event: EventKey,
    group: GroupKey,
    chat: String,
    /// For a new session: the driver took it, so the chat's saved session is not named again.
    new_session: bool,
    /// For an answer: the item it answers, so a second answer to it is refused.
    #[serde(default)]
    answers: Option<String>,
}

/// The conversation: every rule about people, sessions and items.
pub struct Conversation {
    agent: PubKey,
    harness: String,
    rights: Rights,
    capabilities: Capabilities,
    saved: Saved,
    /// Chats the driver has taken a prompt or a new session for since it started (D-065).
    taken: HashSet<String>,
    /// The core's id for each prompt, back to its event, so a turn can name what started it.
    messages: HashMap<String, EventKey>,
    seen: HashSet<EventKey>,
    seen_order: VecDeque<EventKey>,
}

impl Conversation {
    /// A conversation for `agent`, whose driver drives `harness` and says it can do
    /// `capabilities`, taking up what was `saved` before a restart.
    pub fn new(
        agent: PubKey,
        harness: impl Into<String>,
        rights: Rights,
        capabilities: Capabilities,
        saved: Saved,
    ) -> Self {
        Self {
            agent,
            harness: harness.into(),
            rights,
            capabilities,
            saved,
            taken: HashSet::new(),
            messages: HashMap::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
        }
    }

    /// What to keep across a restart.
    pub fn saved(&self) -> &Saved {
        &self.saved
    }

    /// After a restart: closes as interrupted every item a stopped core left open, and settles as
    /// failed every request it left unsettled (D-070).
    pub fn recover(&mut self) -> Vec<Act> {
        self.ended()
    }

    /// The driver stopped: closes every item still open, and settles every request still open as
    /// failed (D-058, D-070).
    pub fn ended(&mut self) -> Vec<Act> {
        let mut acts = self.close_open();
        let pending: Vec<String> = self.saved.pending.keys().cloned().collect();
        for request in pending {
            acts.extend(self.refuse(&request, Refusal::new(Reason::Failed)));
        }
        acts
    }

    /// A chat message arrived.
    pub fn heard(&mut self, heard: Heard) -> Vec<Act> {
        if heard.author == self.agent || !self.first_time(&heard.event) {
            return Vec::new();
        }
        let addressed = heard.chat.direct
            || heard.mentions_agent
            || heard
                .reply_to
                .as_ref()
                .is_some_and(|q| q.author == self.agent);
        if !addressed {
            return Vec::new();
        }
        let chat = self.chat(&heard.chat);
        let right = needed_for(&heard.text, &self.capabilities.approval_commands);
        if !self.rights.allows(&heard.author, right) {
            let notice = self.notice(&heard.chat.group, &chat, "not_allowed", not_allowed(right));
            return vec![self.send_notice(
                &heard.chat.group,
                notice,
                Some(heard.event),
                Vec::new(),
            )];
        }
        let message = self.make_id("m");
        self.messages.insert(message.clone(), heard.event.clone());
        let reply_to = heard.reply_to.map(|q| Quoted {
            message: self.message_id_for(&q.event),
            text: q.text,
            author: self.person(&q.author, q.author_name),
        });
        let session = if self.taken.contains(&chat.id.0) {
            None
        } else {
            self.saved.sessions.get(&chat.id.0).cloned()
        };
        vec![Act::Prompt(Prompt {
            message: MessageId(message),
            text: heard.text,
            chat,
            sender: self.person(&heard.author, heard.author_name),
            reply_to,
            attachments: heard.attachments,
            session,
        })]
    }

    /// The driver took a prompt, or refused it. A refused prompt is not sent again (D-037); the
    /// person is told.
    pub fn prompted(&mut self, prompt: &Prompt, result: Result<(), Refusal>) -> Vec<Act> {
        match result {
            Ok(()) => {
                self.taken.insert(prompt.chat.id.0.clone());
                Vec::new()
            }
            Err(refusal) => {
                let Some(group) = self.group_of(&prompt.chat.id.0) else {
                    return Vec::new();
                };
                let about = self.messages.get(&prompt.message.0).cloned();
                let notice = self.notice(
                    &group,
                    &prompt.chat,
                    reason_word(refusal.reason),
                    not_delivered(&refusal),
                );
                vec![self.send_notice(&group, notice, about, Vec::new())]
            }
        }
    }

    /// An answer or a control arrived (D-027, D-028, D-039).
    pub fn asked(&mut self, heard: HeardRequest) -> Vec<Act> {
        if heard.author == self.agent || !self.first_time(&heard.event) {
            return Vec::new();
        }
        let chat = self.chat(&heard.chat);
        let target = heard
            .target
            .as_ref()
            .and_then(|t| self.item_by_first(&heard.chat.group, t));
        if heard.target.is_none() && heard.age_secs > LATE_SECS {
            return Vec::new();
        }
        if heard.target.is_some() && target.is_none() {
            return self.refuse_now(&heard, &chat, Refusal::new(Reason::UnknownItem));
        }
        let answering = matches!(heard.asked, Asked::Choice { .. } | Asked::Questions { .. });
        let answers = target.clone().filter(|_| answering);
        let target = target.map(|key| self.saved.items[&key].clone());
        // One that points at an item acts only while the item is live, and an answer only while
        // its item is waiting and nobody has answered it first (D-028).
        if let Some(record) = &target {
            let waiting = record.last.header().state == State::Waiting;
            let refusal = if answering && self.saved.pending.values().any(|p| p.answers == answers)
            {
                Some(Reason::AlreadyAnswered)
            } else if answering && !waiting {
                Some(Reason::NotWaiting)
            } else if !record.open() {
                Some(Reason::NothingToDo)
            } else {
                None
            };
            if let Some(reason) = refusal {
                return self.refuse_now(&heard, &chat, Refusal::new(reason));
            }
        }
        let target = target.map(|record| record.last);
        let by = self.person(&heard.author, heard.author_name.clone());
        let request = RequestId(self.make_id("r"));
        let (right, act) = match self.translate(&heard, &chat, target.as_ref(), &request, by) {
            Ok(found) => found,
            Err(refusal) => return self.refuse_now(&heard, &chat, refusal),
        };
        if !self.rights.allows(&heard.author, right) {
            let notice = self.notice(&heard.chat.group, &chat, "not_allowed", not_allowed(right));
            return vec![self.send_notice(
                &heard.chat.group,
                notice,
                None,
                vec![heard.event.clone()],
            )];
        }
        let new_session = matches!(heard.asked, Asked::NewSession);
        self.saved.pending.insert(
            request.0.clone(),
            Pending {
                event: heard.event,
                group: heard.chat.group,
                chat: chat.id.0,
                new_session,
                answers,
            },
        );
        vec![act]
    }

    /// The right a request needs and the driver request it becomes, or why it cannot be one.
    fn translate(
        &self,
        heard: &HeardRequest,
        chat: &Chat,
        target: Option<&AgentEvent>,
        request: &RequestId,
        by: Person,
    ) -> Result<(Right, Act), Refusal> {
        let control = |action| {
            Act::Control(Control {
                request: request.clone(),
                by: by.clone(),
                action,
            })
        };
        let session_of = |target: Option<&AgentEvent>| {
            target
                .and_then(|t| t.header().session.clone())
                .or_else(|| self.saved.sessions.get(&chat.id.0).cloned())
                .ok_or(Refusal::new(Reason::NothingToDo))
        };
        Ok(match &heard.asked {
            Asked::Choice { choice } => {
                let target = target.filter(|t| matches!(t.body(), Body::Approval(_)));
                let target = target.ok_or(Refusal::new(Reason::UnknownItem))?;
                let answer = Answer {
                    request: request.clone(),
                    item: target.header().item.clone(),
                    by: by.clone(),
                    given: Given::Choice(choice.clone()),
                };
                (Right::Approve, Act::Answer(answer))
            }
            Asked::Questions { questions } => {
                let target = target.ok_or(Refusal::new(Reason::UnknownItem))?;
                let Body::Question(question) = target.body() else {
                    return Err(Refusal::new(Reason::UnknownItem));
                };
                // A secret is answered only where nobody else reads it (D-041).
                let secret = question.questions().iter().any(|q| q.secret);
                if secret && heard.chat.members > 2 {
                    return Err(Refusal {
                        reason: Reason::NotSupported,
                        detail: Some("A secret answer is taken only in a two-person chat. Answer it at the terminal.".into()),
                    });
                }
                let replies = questions
                    .iter()
                    .map(|q| Reply {
                        question: q.question.clone(),
                        options: q.options.clone(),
                        text: q.text.clone(),
                    })
                    .collect();
                let answer = Answer {
                    request: request.clone(),
                    item: target.header().item.clone(),
                    by: by.clone(),
                    given: Given::Questions(replies),
                };
                (Right::Prompt, Act::Answer(answer))
            }
            Asked::Interrupt => {
                let turn = match target {
                    Some(t) => t.header().turn.clone(),
                    None => self.open_turn(&chat.id.0),
                };
                let turn = turn.ok_or(Refusal::new(Reason::NothingToDo))?;
                (Right::Prompt, control(Action::Interrupt { turn }))
            }
            Asked::NewSession => {
                let session = match target {
                    Some(t) => t.header().session.clone(),
                    None => self.saved.sessions.get(&chat.id.0).cloned(),
                };
                let chat = chat.clone();
                (Right::Prompt, control(Action::NewSession { chat, session }))
            }
            Asked::Set { setting, value } => {
                let session = session_of(target)?;
                (
                    self.right_to_set(&chat.id.0, setting),
                    control(Action::Set {
                        session,
                        setting: setting.clone(),
                        value: value.clone(),
                    }),
                )
            }
            Asked::Own { id } => {
                let session = session_of(target)?;
                (
                    Right::Prompt,
                    control(Action::Own {
                        session,
                        id: id.clone(),
                    }),
                )
            }
        })
    }

    /// The driver took an answer or a control, or refused it. A refusal settles it with a notice
    /// that says why (D-028).
    pub fn requested(&mut self, request: &RequestId, result: Result<(), Refusal>) -> Vec<Act> {
        match result {
            Ok(()) => {
                if let Some(pending) = self.saved.pending.get(&request.0)
                    && pending.new_session
                {
                    self.taken.insert(pending.chat.clone());
                }
                Vec::new()
            }
            Err(refusal) => self.refuse(&request.0, refusal),
        }
    }

    /// An event from the driver.
    pub fn emitted(&mut self, emitted: Emitted) -> Vec<Act> {
        let header = emitted.event.header().clone();
        let chat = emitted
            .chat
            .as_ref()
            .map(|c| c.0.clone())
            .or_else(|| self.chat_of_session(header.session.as_ref()));
        let Some(chat) = chat else {
            return Vec::new();
        };
        let Some(group) = self.group_of(&chat) else {
            return Vec::new();
        };
        if let (Body::Session(_), Some(session)) = (emitted.event.body(), &header.session) {
            self.saved.sessions.insert(chat.clone(), session.clone());
        }
        let key = format!("{chat}/{}", header.item.as_str());
        let started_by = match emitted.event.body() {
            Body::Turn(_) => emitted
                .prompts
                .first()
                .and_then(|m| self.messages.get(&m.0).cloned()),
            _ => None,
        };
        let record = self
            .saved
            .items
            .entry(key.clone())
            .or_insert_with(|| Record {
                group: group.clone(),
                chat: chat.clone(),
                first: None,
                version: 0,
                last: emitted.event.clone(),
                words: None,
                started_by: None,
            });
        record.last = emitted.event.clone();
        if record.started_by.is_none() {
            record.started_by = started_by;
        }
        let act = if let Body::Message(message) = emitted.event.body() {
            let text = message_text(&header, message);
            if text.is_empty() || record.words.as_deref() == Some(text.as_str()) {
                None
            } else {
                record.words = Some(text.clone());
                Some(Sending::Words {
                    key: key.clone(),
                    text,
                    turn: header.turn.clone(),
                    remark: message.phase == Some(marmot_agent_events::body::Phase::Commentary),
                    edit: record.first.clone(),
                })
            }
        } else {
            let pending = &mut self.saved.pending;
            let settles = emitted
                .settles
                .iter()
                .filter_map(|r| pending.remove(&r.0))
                .map(|p| p.event)
                .collect();
            record.version += 1;
            Some(Sending::Item {
                key: key.clone(),
                item: Box::new(Item {
                    event: emitted.event.clone(),
                    version: record.version,
                    first: record.first.clone(),
                    started_by: record.started_by.clone(),
                    settles,
                }),
            })
        };
        let open = record.open();
        if !open {
            self.finished(key);
        }
        act.map(|what| Act::Send { group, what })
            .into_iter()
            .collect()
    }

    /// Something was sent, and `event` is its id. The first event of an item is what its later
    /// versions name.
    pub fn sent(&mut self, key: &str, event: EventKey) {
        if let Some(record) = self.saved.items.get_mut(key)
            && record.first.is_none()
        {
            record.first = Some(event);
        }
    }

    // --------------------------------------------------------------------------------- helpers

    fn first_time(&mut self, event: &EventKey) -> bool {
        if !self.seen.insert(event.clone()) {
            return false;
        }
        self.seen_order.push_back(event.clone());
        if self.seen_order.len() > SEEN
            && let Some(old) = self.seen_order.pop_front()
        {
            self.seen.remove(&old);
        }
        true
    }

    fn make_id(&mut self, prefix: &str) -> String {
        self.saved.next += 1;
        format!("{prefix}-{}", self.saved.next)
    }

    /// The driver's view of a chat, with the core's id for it (D-036).
    fn chat(&mut self, chat: &Where) -> Chat {
        let id = match self.saved.chats.get(&chat.group) {
            Some(id) => id.clone(),
            None => {
                let id = self.make_id("c");
                self.saved.chats.insert(chat.group.clone(), id.clone());
                id
            }
        };
        Chat {
            id: ChatId(id),
            group: !chat.direct,
            name: chat.name.clone(),
        }
    }

    /// The driver's view of a person, with the core's id for them (D-036).
    fn person(&mut self, key: &PubKey, name: String) -> Person {
        let id = match self.saved.people.get(key) {
            Some(id) => id.clone(),
            None => {
                let id = self.make_id("u");
                self.saved.people.insert(key.clone(), id.clone());
                id
            }
        };
        Person {
            id: PersonId(id),
            name,
        }
    }

    /// The core's id for a message that was not itself a prompt, such as one a prompt replies to.
    fn message_id_for(&mut self, event: &EventKey) -> MessageId {
        let id = self.make_id("m");
        self.messages.insert(id.clone(), event.clone());
        MessageId(id)
    }

    fn group_of(&self, chat: &str) -> Option<GroupKey> {
        self.saved
            .chats
            .iter()
            .find(|(_, id)| id.as_str() == chat)
            .map(|(group, _)| group.clone())
    }

    fn chat_of_session(&self, session: Option<&SessionId>) -> Option<String> {
        let session = session?;
        self.saved
            .sessions
            .iter()
            .find(|(_, s)| *s == session)
            .map(|(chat, _)| chat.clone())
    }

    fn item_by_first(&self, group: &GroupKey, first: &EventKey) -> Option<String> {
        self.saved
            .items
            .iter()
            .find(|(_, r)| r.group == *group && r.first.as_ref() == Some(first))
            .map(|(key, _)| key.clone())
    }

    /// The chat's running turn, for an interrupt that names no item.
    fn open_turn(&self, chat: &str) -> Option<TurnId> {
        self.saved
            .items
            .values()
            .filter(|r| r.chat == chat && r.open() && matches!(r.last.body(), Body::Turn(_)))
            .max_by_key(|r| r.last.header().seq)
            .and_then(|r| r.last.header().turn.clone())
    }

    /// The right a setting needs, from the chat's session item (D-043).
    fn right_to_set(&self, chat: &str, setting: &str) -> Right {
        let approve = self.saved.items.values().any(|r| {
            r.chat == chat
                && matches!(r.last.body(), Body::Session(s)
                    if s.settings.iter().any(|x| x.id == setting && x.approve))
        });
        if approve {
            Right::Approve
        } else {
            Right::Prompt
        }
    }

    /// A finished item is kept for a while, to recognise a late request that names it, without
    /// its native message, which nothing needs again.
    fn finished(&mut self, key: String) {
        if let Some(record) = self.saved.items.get_mut(&key)
            && record.last.native().is_some()
        {
            record.last = crate::carrier::rebuild(&record.last, record.last.body().clone());
        }
        if self.saved.finished.contains(&key) {
            return;
        }
        self.saved.finished.push_back(key);
        while self.saved.finished.len() > KEPT {
            if let Some(old) = self.saved.finished.pop_front() {
                self.saved.items.remove(&old);
            }
        }
    }

    /// Closes every open item as interrupted.
    fn close_open(&mut self) -> Vec<Act> {
        let open: Vec<String> = self
            .saved
            .items
            .iter()
            .filter(|(_, r)| r.open())
            .map(|(k, _)| k.clone())
            .collect();
        let mut acts = Vec::new();
        for key in open {
            let record = &self.saved.items[&key];
            let mut header = record.last.header().clone();
            header.state = State::Cancelled;
            let closed = AgentEvent::new(header, record.last.body().clone(), None)
                .expect("an event with no native message repeats no secret");
            let chat = ChatId(record.chat.clone());
            acts.extend(self.emitted(Emitted {
                event: closed,
                chat: Some(chat),
                prompts: Vec::new(),
                settles: Vec::new(),
            }));
        }
        acts
    }

    fn refuse(&mut self, request: &str, refusal: Refusal) -> Vec<Act> {
        let Some(pending) = self.saved.pending.remove(request) else {
            return Vec::new();
        };
        let chat = Chat {
            id: ChatId(pending.chat),
            group: true,
            name: None,
        };
        let notice = self.notice(
            &pending.group,
            &chat,
            reason_word(refusal.reason),
            not_acted_on(&refusal),
        );
        vec![self.send_notice(&pending.group, notice, None, vec![pending.event])]
    }

    fn refuse_now(&mut self, heard: &HeardRequest, chat: &Chat, refusal: Refusal) -> Vec<Act> {
        let notice = self.notice(
            &heard.chat.group,
            chat,
            reason_word(refusal.reason),
            not_acted_on(&refusal),
        );
        vec![self.send_notice(&heard.chat.group, notice, None, vec![heard.event.clone()])]
    }

    /// A notice the core makes up: why something was not acted on.
    fn notice(&mut self, _group: &GroupKey, chat: &Chat, reason: &str, text: String) -> AgentEvent {
        let header = Header {
            harness: self.harness.clone(),
            session: self.saved.sessions.get(&chat.id.0).cloned(),
            turn: None,
            item: ItemId(self.make_id("core")),
            seq: 0,
            state: State::Done,
            title: "Not acted on".into(),
            text,
        };
        let body = Body::Notice(Notice {
            kind: NoticeKind::Refused,
            duration_ms: None,
            reason: Some(reason.into()),
        });
        AgentEvent::new(header, body, None).expect("a notice repeats no secret")
    }

    fn send_notice(
        &mut self,
        group: &GroupKey,
        event: AgentEvent,
        about: Option<EventKey>,
        settles: Vec<EventKey>,
    ) -> Act {
        Act::Send {
            group: group.clone(),
            what: Sending::Item {
                key: format!("notice/{}", event.header().item.as_str()),
                item: Box::new(Item {
                    event,
                    version: 1,
                    first: None,
                    started_by: about,
                    settles,
                }),
            },
        }
    }
}

/// The text of the agent's words: the plain rendering, which for a message is what it said.
fn message_text(header: &Header, _message: &marmot_agent_events::body::Message) -> String {
    if matches!(header.state, State::Running) {
        return String::new();
    }
    header.text.clone()
}

fn reason_word(reason: Reason) -> &'static str {
    match reason {
        Reason::NotSupported => "not_supported",
        Reason::Busy => "busy",
        Reason::UnknownItem => "unknown_item",
        Reason::NotWaiting => "not_waiting",
        Reason::AlreadyAnswered => "already_answered",
        Reason::NothingToDo => "nothing_to_do",
        Reason::Invalid => "invalid",
        Reason::Failed => "failed",
    }
}

fn not_allowed(right: Right) -> String {
    match right {
        Right::Prompt => "Only people the owner chose can direct this agent.".into(),
        Right::Approve => "Only people the owner chose can approve what this agent does.".into(),
    }
}

fn plain(reason: Reason) -> &'static str {
    match reason {
        Reason::NotSupported => "The agent cannot do that.",
        Reason::Busy => "The agent is busy. Try again in a moment.",
        Reason::UnknownItem => "The agent does not know that item.",
        Reason::NotWaiting => "That is no longer waiting for an answer.",
        Reason::AlreadyAnswered => "That was already answered.",
        Reason::NothingToDo => "There is nothing to act on.",
        Reason::Invalid => "That does not make sense to the agent.",
        Reason::Failed => "The agent tried and it failed.",
    }
}

fn not_acted_on(refusal: &Refusal) -> String {
    match &refusal.detail {
        Some(detail) => format!("{} {detail}", plain(refusal.reason)),
        None => plain(refusal.reason).into(),
    }
}

fn not_delivered(refusal: &Refusal) -> String {
    format!(
        "Your message was not passed to the agent. {}",
        not_acted_on(refusal)
    )
}
