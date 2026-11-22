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
//!   (D-058, D-070);
//! - a chat of a coding harness has a folder, set by someone who may approve, that applies from
//!   its next session (D-101).

use crate::carrier::{Asked, Item, SETTLES_MAX};
use crate::chat_folder::{FOLDER, FolderRule, LABEL};
use crate::keys::{EventKey, GroupKey, PubKey};
use crate::rights::{Right, Rights, needed_for};
use marmot_agent_driver::{
    Action, Answer, Attachment, Capabilities, Chat, ChatId, Control, Emitted, Given, MessageId,
    Person, PersonId, Prompt, Quoted, Reason, Refusal, Reply, RequestId,
};
use marmot_agent_events::body::{Body, Notice, NoticeKind, Setting};
use marmot_agent_events::{AgentEvent, Header, ItemId, SessionId, State, TurnId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;

/// A request older than this that points at nothing, such as "new session", is dropped (D-028).
pub const LATE_SECS: u64 = 5 * 60;
/// How many handled events are remembered, so that one delivered twice acts once.
const SEEN: usize = 4096;
/// How many finished items are kept, beyond those still open and each chat's session item; and
/// how many forgotten ones the core still knows where they stood.
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
    /// Something to send to a group is queued. The program saves the conversation, so the send
    /// is on disk with its version and what it settles before it goes (S1, S3), then sends what
    /// [`Conversation::next_send`] gives for the group, in order, calling
    /// [`Conversation::sent`] with each event's id. The conversation counts on every send going
    /// out, since an item's later versions, and the requests they settle, follow from it: a send
    /// that fails is tried again until it goes out, one MDK refuses is handed to
    /// [`Conversation::refused`], and one still queued when the core stops is sent when it
    /// starts again. What is sent lives only in the queue.
    Send {
        /// The group.
        group: GroupKey,
    },
}

/// What to send to a group.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
        /// The answers and controls whose effect they show (D-028). Only a new message carries
        /// them: an edit cannot carry tags.
        settles: Vec<EventKey>,
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

/// What names one try of a send.
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    /// The core's name for the send: the same each time it is tried, across restarts, and what
    /// [`Conversation::attempting`], [`Conversation::rejected`] and [`Conversation::refused`]
    /// take.
    pub send: String,
    /// The token MDK keeps the send under (MDK's `client_token`). A send tried again under it
    /// gives back the event MDK made of it, and nothing goes out twice. A new one only once MDK
    /// dropped a try of it ([`Conversation::rejected`]).
    pub mdk: String,
    /// For a later version of an item, or an edit of the agent's words: the token MDK took the
    /// first event under, which the edit names. `None` when an older core sent that message,
    /// without one.
    pub edits: Option<String>,
    /// When the send was first handed to MDK under [`Token::mdk`], in seconds since 1970: MDK
    /// lets go of a token with the message it made, once a group's messages disappear, so a
    /// send tried long after may no longer be known to MDK for what it was.
    pub tried_at: Option<u64>,
}

/// Prints nothing of the tokens, which are only names (D-071).
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token").finish_non_exhaustive()
    }
}

/// A new salt for a state file: 16 random bytes, in hex. Without the system's randomness, the
/// time: a salt only has to differ from the one of a state file that was lost.
fn salt() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        bytes = nanos.to_le_bytes();
    }
    hex::encode(bytes)
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
    /// The folder each chat chose for its sessions, resolved, by the core's chat id (D-101). A
    /// chat that chose none works in the driver's own folder.
    #[serde(default)]
    folders: BTreeMap<String, PathBuf>,
    /// Every item sent and not yet forgotten, by the core's name for it.
    items: BTreeMap<String, Record>,
    /// The order finished items were finished in, oldest first, so the oldest are forgotten.
    finished: VecDeque<String>,
    /// Where the items forgotten most recently stood, oldest first, so one whose events come
    /// again goes on from there instead of starting a new row.
    #[serde(default)]
    forgotten: VecDeque<Forgotten>,
    /// Requests handed to the driver and not settled, by the core's id for them.
    pending: BTreeMap<String, Pending>,
    /// What is to be sent and has not been sent yet, oldest first: kept so a send that failed,
    /// or that the core stopped before, goes out as it was when the core starts again (S1, S3).
    #[serde(default)]
    outbox: VecDeque<Queued>,
    /// The next number for an id the core makes up.
    next: u64,
    /// Made once, at random, for this state file, and part of every token handed to MDK: a core
    /// whose state file was lost starts its names again, and its tokens are not ones MDK already
    /// holds for the sends of the state file before.
    #[serde(default)]
    salt: String,
    /// Whether the items' first events went out as kind 1201, which MDK edits. A state file from
    /// before has `false`: its items' first events went out under a temporary kind (D-069, replaced
    /// by D-112), which MDK does not edit, so the next version of each goes out as a first event
    /// again.
    #[serde(default)]
    kind_1201: bool,
}

impl Saved {
    /// Takes up a state file whose items' first events went out under the temporary kind of
    /// D-069, which no edit applies to: each item's next version goes out as a new kind 1201
    /// event, the first of the item in the chat from then on. The agent's words were chat
    /// messages then too, and are edited as before. Where forgotten items stood is forgotten.
    fn move_to_kind_1201(&mut self) {
        for record in self.items.values_mut() {
            if !matches!(record.last.body(), Body::Message(_)) {
                record.first = None;
                record.first_token = None;
            }
        }
        for queued in &mut self.outbox {
            if let Sending::Item { item, .. } = &mut queued.what {
                item.first = None;
            }
        }
        self.forgotten.clear();
        self.kind_1201 = true;
    }

    /// Whether a core from before MDK kept tokens wrote this: it has sends it handed to MDK,
    /// without a token, and no salt. Under a token now, any of them could go out a second time.
    pub fn tried_without_tokens(&self) -> bool {
        self.salt.is_empty() && self.outbox.iter().any(|q| q.attempted)
    }
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
    /// The token MDK took the first event under: an edit of it, a later version of an item or of
    /// the agent's words, names it. `None` for words an older core sent, which MDK took without
    /// a token.
    #[serde(default)]
    first_token: Option<String>,
    version: u64,
    /// The latest version the driver made.
    last: AgentEvent,
    /// The agent's words, sent as a chat message, and what they said when last sent.
    words: Option<String>,
    /// For a turn: the message that started it, carried by every version.
    #[serde(default)]
    started_by: Option<EventKey>,
    /// For the agent's words: the requests they settle, held until the words go out as a new
    /// chat message, which carries them (D-028).
    #[serde(default)]
    held: Vec<String>,
    /// Whether the latest version of it that went out closed it: then a later final version
    /// that is dropped leaves the chat showing it closed, and the core does not close it again.
    #[serde(default)]
    closed_in_chat: bool,
}

/// Something queued to be sent to a group.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Queued {
    group: GroupKey,
    /// The core's name for this send, the same each time it is tried, across restarts: for an
    /// item, its key and version. With [`Queued::rejected`] and the state file's salt it makes
    /// the token MDK keeps the send under ([`Token::mdk`]), so a send tried again is the one MDK
    /// already has, and goes out once.
    token: String,
    what: Sending,
    /// For a turn's version that settles what the agent's words showed: the send of those words,
    /// so that if MDK refuses them, the version settles nothing (S1).
    #[serde(default)]
    shown_by: Option<String>,
    /// Whether it was handed to MDK at least once.
    #[serde(default)]
    attempted: bool,
    /// How many times MDK took it and dropped it before it went out. Each such try is over for
    /// good, and MDK refuses its token again, so the next try is made under a new one.
    #[serde(default)]
    rejected: u32,
    /// When it was first handed to MDK under its token now, in seconds since 1970 ([`Token`]).
    #[serde(default)]
    tried_at: Option<u64>,
    /// Whether it stands in for a send that was dropped: an item's close, or a notice that
    /// settles what a dropped notice settled. Nothing stands in for it in turn, so a group MDK
    /// refuses everything for, as one that is gone, ends the chain there.
    #[serde(default)]
    last_resort: bool,
    /// Whether it is a `failed` notice that tells the chat of a dropped send: one that is dropped
    /// in turn is replaced once, even when it settles nothing, so the chat is told of the gap.
    #[serde(default)]
    tells: bool,
}

impl Sending {
    /// The core's name for the item.
    fn key(&self) -> &str {
        match self {
            Self::Item { key, .. } | Self::Words { key, .. } => key,
        }
    }
}

/// Where a forgotten item stood: enough for its next version to follow on.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Forgotten {
    key: String,
    first: Option<EventKey>,
    #[serde(default)]
    first_token: Option<String>,
    version: u64,
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
    /// For a new session that moves the chat to a folder: the folder, the chat's once the new
    /// session shows (D-101).
    #[serde(default)]
    folder: Option<PathBuf>,
    /// For an answer: the item it answers, so a second answer to it is refused.
    #[serde(default)]
    answers: Option<String>,
    /// For a control the driver took: when it is settled if no effect has shown, on the core's
    /// clock (D-087). Not kept across a restart, which settles every request anyway (D-070).
    #[serde(skip)]
    deadline: Option<u64>,
}

/// How long a control the driver took may wait for its effect before the core settles it.
pub const EFFECT_LIMIT_SECS: u64 = 120;

/// What a send MDK refused is settled with.
const NOT_DELIVERED: &str = "Something the agent sent could not be delivered to the chat.";

/// What a notice says of a send given up without knowing whether it went out.
const NOT_KNOWN: &str = "Something the agent sent may not have been delivered to the chat.";

/// The title of the core's close of an item whose closing version was not delivered.
const CLOSED_TITLE: &str = "Not delivered";

/// Its text.
const CLOSED_TEXT: &str = "This could not be delivered to the chat.";

/// What a request the driver took and that had no effect is settled with (D-087).
const NO_EFFECT: &str = "The agent took this, but nothing has come of it.";

/// The conversation: every rule about people, sessions and items.
pub struct Conversation {
    agent: PubKey,
    harness: String,
    rights: Rights,
    capabilities: Capabilities,
    /// The rule for a chat's folder, when the driver works in one a chat may move (D-101).
    folders: Option<FolderRule>,
    saved: Saved,
    /// Chats the driver has taken a prompt or a new session for since it started (D-065).
    taken: HashSet<String>,
    /// The core's id for each prompt, back to its event, so a turn can name what started it.
    messages: HashMap<String, EventKey>,
    seen: HashSet<EventKey>,
    seen_order: VecDeque<EventKey>,
    /// A bug put back for the simulator: what the agent's words settle is ignored.
    #[cfg(feature = "sim-bugs")]
    bug_settles_only_from_items: bool,
}

impl Conversation {
    /// A conversation for `agent`, whose driver drives `harness` and says it can do
    /// `capabilities`, taking up what was `saved` before a restart.
    pub fn new(
        agent: PubKey,
        harness: impl Into<String>,
        rights: Rights,
        capabilities: Capabilities,
        mut saved: Saved,
    ) -> Self {
        if saved.salt.is_empty() {
            saved.salt = salt();
        }
        if !saved.kind_1201 {
            saved.move_to_kind_1201();
        }
        Self {
            agent,
            harness: harness.into(),
            rights,
            capabilities,
            folders: None,
            saved,
            taken: HashSet::new(),
            messages: HashMap::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            #[cfg(feature = "sim-bugs")]
            bug_settles_only_from_items: false,
        }
    }

    /// Offers each chat a folder setting, by `rule`: for a driver that works in a folder of its own,
    /// which a chat may move (D-101).
    ///
    /// A folder a chat chose earlier is checked again by `rule`, since the owner may have changed
    /// the settings: one no longer allowed, or no longer there, is the chat's no more, and nor is
    /// its session there, so the chat's next prompt starts a session in the driver's own folder.
    pub fn with_folders(mut self, rule: FolderRule) -> Self {
        let chosen = std::mem::take(&mut self.saved.folders);
        for (chat, folder) in chosen {
            let still = (folder.to_str()).and_then(|f| rule.check(f, rule.default_folder()).ok());
            if still.as_ref() == Some(&folder) {
                self.saved.folders.insert(chat, folder);
            } else {
                self.saved.sessions.remove(&chat);
            }
        }
        self.folders = Some(rule);
        self
    }

    /// The folder a chat's sessions work in: the one it chose, or else the driver's own. `None`
    /// when the driver states no folder.
    pub fn chat_folder(&self, chat: &ChatId) -> Option<PathBuf> {
        let rule = self.folders.as_ref()?;
        Some(
            (self.saved.folders.get(&chat.0).cloned())
                .unwrap_or_else(|| rule.default_folder().to_path_buf()),
        )
    }

    /// The folder a chat chose for its sessions, if it chose one (D-101).
    pub fn chosen_folder(&self, chat: &ChatId) -> Option<PathBuf> {
        self.folders.as_ref()?;
        self.saved.folders.get(&chat.0).cloned()
    }

    /// Puts back, for the simulator, the bug that read settles only from items, never from the
    /// agent's words (`docs/design/verification.md`).
    #[cfg(feature = "sim-bugs")]
    pub fn settles_only_from_items(&mut self, on: bool) {
        self.bug_settles_only_from_items = on;
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
        // Requests first, so an answer whose item closes now is settled as failed (D-070), not
        // by the net for an item that stopped waiting.
        let mut acts = Vec::new();
        let pending: Vec<String> = self.saved.pending.keys().cloned().collect();
        for request in pending {
            acts.extend(self.refuse(&request, Refusal::new(Reason::Failed)));
        }
        acts.extend(self.close_open());
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
        let mut chat = chat;
        if let Some(folder) = self.moving_to(&chat.id.0) {
            chat.folder = Some(folder);
        }
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
        if heard.target.is_none() && heard.age_secs > LATE_SECS {
            return Vec::new();
        }
        // The right comes first: someone without it is told only that, and nothing of the item
        // the request names, whether it exists, waits or was answered (S6).
        let right = match &heard.asked {
            Asked::Choice { .. } => Right::Approve,
            // A folder changes what the agent can touch (D-101).
            Asked::Set { setting, .. } if self.moves_folder(setting) => Right::Approve,
            Asked::Set { setting, .. } => self.right_to_set(&chat.id.0, setting),
            _ => Right::Prompt,
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
        let target = heard
            .target
            .as_ref()
            .and_then(|t| self.item_by_first(&heard.chat.group, t));
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
        let act = match self.translate(&heard, &chat, target.as_ref(), &request, by) {
            Ok(act) => act,
            Err(refusal) => return self.refuse_now(&heard, &chat, refusal),
        };
        let moving =
            matches!(&heard.asked, Asked::Set { setting, .. } if self.moves_folder(setting));
        let new_session = matches!(heard.asked, Asked::NewSession) || moving;
        let folder = match &act {
            Act::Control(Control {
                action: Action::NewSession { chat, .. },
                ..
            }) if moving => chat.folder.clone(),
            _ => None,
        };
        self.saved.pending.insert(
            request.0.clone(),
            Pending {
                event: heard.event,
                group: heard.chat.group,
                chat: chat.id.0,
                new_session,
                folder,
                answers,
                deadline: None,
            },
        );
        vec![act]
    }

    /// The driver request a request becomes, or why it cannot be one.
    fn translate(
        &self,
        heard: &HeardRequest,
        chat: &Chat,
        target: Option<&AgentEvent>,
        request: &RequestId,
        by: Person,
    ) -> Result<Act, Refusal> {
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
                Act::Answer(answer)
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
                Act::Answer(answer)
            }
            Asked::Interrupt => {
                let turn = match target {
                    Some(t) => t.header().turn.clone(),
                    None => self.open_turn(&chat.id.0),
                };
                let turn = turn.ok_or(Refusal::new(Reason::NothingToDo))?;
                control(Action::Interrupt { turn })
            }
            Asked::NewSession => {
                let session = match target {
                    Some(t) => t.header().session.clone(),
                    None => self.saved.sessions.get(&chat.id.0).cloned(),
                };
                let chat = chat.clone();
                control(Action::NewSession { chat, session })
            }
            Asked::Set { setting, value } if self.moves_folder(setting) => {
                // It applies from the chat's next session, so it starts one there (D-101).
                let rule = self.folders.as_ref().expect("a driver with a folder");
                let current = self.chat_folder(&chat.id).expect("a driver with a folder");
                let folder = rule.check(value, &current)?;
                let session = match target {
                    Some(t) => t.header().session.clone(),
                    None => self.saved.sessions.get(&chat.id.0).cloned(),
                };
                let chat = Chat {
                    folder: Some(folder),
                    ..chat.clone()
                };
                control(Action::NewSession { chat, session })
            }
            Asked::Set { setting, value } => {
                let session = session_of(target)?;
                control(Action::Set {
                    session,
                    setting: setting.clone(),
                    value: value.clone(),
                })
            }
            Asked::Own { id } => {
                let session = session_of(target)?;
                control(Action::Own {
                    session,
                    id: id.clone(),
                })
            }
        })
    }

    /// The driver took an answer or a control, or refused it, at `now` on the core's clock, in
    /// seconds. A refusal settles it with a notice that says why (D-028).
    pub fn requested(
        &mut self,
        request: &RequestId,
        result: Result<(), Refusal>,
        now: u64,
    ) -> Vec<Act> {
        match result {
            Ok(()) => {
                if let Some(pending) = self.saved.pending.get_mut(&request.0) {
                    // An answer keeps its place until its item stops waiting, so a second answer
                    // is still refused (D-028); only a control is settled for want of an effect.
                    if pending.answers.is_none() {
                        pending.deadline = Some(now + EFFECT_LIMIT_SECS);
                    }
                    if pending.new_session {
                        self.taken.insert(pending.chat.clone());
                    }
                }
                Vec::new()
            }
            Err(refusal) => self.refuse(&request.0, refusal),
        }
    }

    /// The time is `now` on the core's clock, in seconds: settles with a notice one control the
    /// driver took whose effect has not shown by its deadline, as when a setting is set to the
    /// value it already had and the harness shows nothing (D-087). One at a time, so each is saved
    /// as settled before the next: call again until it gives nothing.
    pub fn overdue(&mut self, now: u64) -> Vec<Act> {
        let late = self
            .saved
            .pending
            .iter()
            .find(|(_, p)| p.deadline.is_some_and(|deadline| now >= deadline))
            .map(|(request, _)| request.clone());
        match late {
            Some(request) => self.settle_pending(&request, "no_effect", NO_EFFECT.into()),
            None => Vec::new(),
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
        self.moved(&chat, &emitted);
        let emitted = self.show_folder(&chat, emitted);
        let Some(group) = self.group_of(&chat) else {
            return Vec::new();
        };
        if let (Body::Session(_), Some(session)) = (emitted.event.body(), &header.session) {
            if header.state == State::Done {
                // The session has ended, as a new session ends the one before: it is no longer
                // the chat's to resume (D-065). One the core closed because something stopped is
                // closed as cancelled, and stays the chat's.
                if self.saved.sessions.get(&chat) == Some(session) {
                    self.saved.sessions.remove(&chat);
                }
            } else {
                self.saved.sessions.insert(chat.clone(), session.clone());
            }
        }
        let key = format!("{chat}/{}", header.item.as_str());
        let started_by = match emitted.event.body() {
            Body::Turn(_) => emitted
                .prompts
                .first()
                .and_then(|m| self.messages.get(&m.0).cloned()),
            _ => None,
        };
        let (first, first_token, version) =
            match self.saved.forgotten.iter().position(|f| f.key == key) {
                Some(at) if !self.saved.items.contains_key(&key) => {
                    let forgotten = self.saved.forgotten.remove(at).expect("found");
                    (forgotten.first, forgotten.first_token, forgotten.version)
                }
                _ => (None, None, 0),
            };
        let record = self
            .saved
            .items
            .entry(key.clone())
            .or_insert_with(|| Record {
                group: group.clone(),
                chat: chat.clone(),
                first,
                first_token,
                version,
                last: emitted.event.clone(),
                words: None,
                started_by: None,
                held: Vec::new(),
                closed_in_chat: false,
            });
        let was_waiting = record.last.header().state == State::Waiting;
        record.last = emitted.event.clone();
        if record.started_by.is_none() {
            record.started_by = started_by;
        }
        let open = record.open();
        // What the turn's next version settles: what words sent as an edit name, since an edit
        // cannot carry tags; what words that do not change name; and what is over the most one
        // event carries.
        let mut by_turn = Vec::new();
        let mut words = None;
        #[allow(unused_mut)]
        let mut named = emitted.settles.as_slice();
        #[cfg(feature = "sim-bugs")]
        if self.bug_settles_only_from_items {
            named = &[];
        }
        if let Body::Message(message) = emitted.event.body() {
            for request in named {
                let ours = (self.saved.pending.get(&request.0)).is_some_and(|p| p.group == group);
                if ours && !record.held.contains(&request.0) {
                    record.held.push(request.0.clone());
                }
            }
            let text = message_text(&header, message);
            if !text.is_empty() && record.words.as_deref() != Some(text.as_str()) {
                record.words = Some(text.clone());
                // A new chat message carries what the words settle.
                let mut held = std::mem::take(&mut record.held);
                let first_words = record.first.is_none()
                    && !(self.saved.outbox.iter()).any(|q| q.what.key() == key);
                let settles = if first_words {
                    by_turn = held.split_off(held.len().min(SETTLES_MAX));
                    take_pending(&mut self.saved.pending, &group, &held)
                } else {
                    by_turn = held;
                    Vec::new()
                };
                words = Some(Sending::Words {
                    key: key.clone(),
                    text,
                    turn: header.turn.clone(),
                    remark: message.phase == Some(marmot_agent_events::body::Phase::Commentary),
                    edit: record.first.clone(),
                    settles,
                });
            } else if !open {
                // The words are final and go out no more: what they name is not lost.
                by_turn = std::mem::take(&mut record.held);
            }
        }
        let mut acts: Vec<Act> = Vec::new();
        if let Some(words) = words {
            acts.push(self.queue(&group, words, None));
        }
        if matches!(emitted.event.body(), Body::Message(_)) {
            if !by_turn.is_empty() {
                // Shown by words still to go out: if MDK refuses them, the turn shows nothing.
                let words = (self.saved.outbox.iter().rev())
                    .find(|q| q.what.key() == key && matches!(q.what, Sending::Words { .. }))
                    .map(|q| q.token.clone());
                let turn = header.turn.as_ref();
                acts.extend(self.settle_by_turn(&key, &chat, &group, turn, by_turn, words));
            }
        } else {
            let requests: Vec<String> = emitted.settles.iter().map(|r| r.0.clone()).collect();
            let settles = take_pending(&mut self.saved.pending, &group, &requests);
            acts.extend(self.queue_versions(&key, &group, emitted.event.clone(), settles, None));
        }
        // An answer the driver took whose item stopped waiting without naming it had no effect,
        // and is settled now rather than when the driver stops (D-087).
        if was_waiting && header.state != State::Waiting {
            let unsettled: Vec<String> = (self.saved.pending.iter())
                .filter(|(_, p)| p.answers.as_deref() == Some(key.as_str()))
                .map(|(request, _)| request.clone())
                .collect();
            for request in unsettled {
                acts.extend(self.settle_pending(&request, "no_effect", NO_EFFECT.into()));
            }
        }
        if open {
            // Open again: it is not finished, and is not forgotten while it is open.
            if let Some(at) = self.saved.finished.iter().position(|k| *k == key) {
                self.saved.finished.remove(at);
            }
        } else {
            self.finished(key);
        }
        acts
    }

    /// Where a file the agent asks to send goes (D-090): the chat's group, and the chat's running
    /// turn, which tags it as the agent's words are tagged. `None` for a chat the core does not know.
    pub fn file_place(&self, chat: &ChatId) -> Option<(GroupKey, Option<TurnId>)> {
        let group = self.group_of(&chat.0)?;
        Some((group, self.open_turn(&chat.0)))
    }

    /// A file the agent asked to send was not sent: the chat is told why, in a refused notice
    /// that names no path (D-071, D-077, D-090).
    pub fn file_refused(&mut self, chat: &ChatId, refusal: &Refusal) -> Vec<Act> {
        let Some(group) = self.group_of(&chat.0) else {
            return Vec::new();
        };
        let text = match &refusal.detail {
            Some(detail) => format!("The agent's file was not sent. {detail}"),
            None => format!("The agent's file was not sent. {}", plain(refusal.reason)),
        };
        let chat = Chat {
            id: chat.clone(),
            group: true,
            name: None,
            folder: None,
        };
        let notice = self.notice(&group, &chat, reason_word(refusal.reason), text);
        vec![self.send_notice(&group, notice, None, Vec::new())]
    }

    /// Settles `requests` that the agent's words named and could not carry: by the next version of
    /// the words' turn, the same as the last and carrying what it settles, queued after the words.
    /// `shown_by` is the send of the words that show the effect, when it is still queued: if MDK
    /// refuses it, the turn's version settles nothing, and the requests are settled as failed.
    /// When the core has no such turn, they are held for the words' next new message, or for the
    /// net when the driver stops.
    fn settle_by_turn(
        &mut self,
        words: &str,
        chat: &str,
        group: &GroupKey,
        turn: Option<&TurnId>,
        requests: Vec<String>,
        shown_by: Option<String>,
    ) -> Vec<Act> {
        let found = turn.and_then(|turn| {
            (self.saved.items.iter())
                .find(|(_, r)| {
                    r.chat == chat
                        && matches!(r.last.body(), Body::Turn(_))
                        && r.last.header().turn.as_ref() == Some(turn)
                })
                .map(|(key, _)| key.clone())
        });
        let Some(key) = found else {
            if let Some(record) = self.saved.items.get_mut(words) {
                record.held.extend(requests);
            }
            return Vec::new();
        };
        let settles = take_pending(&mut self.saved.pending, group, &requests);
        if settles.is_empty() {
            return Vec::new();
        }
        let last = self.saved.items[&key].last.clone();
        self.queue_versions(&key, group, last, settles, shown_by)
    }

    /// Queues the next version of the item named `key`, with `event`, settling `settles`: as
    /// many versions, each the same, as it takes to carry them, at most [`SETTLES_MAX`] each, so
    /// every one fits (S7). `shown_by` is the send that shows their effect, if it is not this one.
    fn queue_versions(
        &mut self,
        key: &str,
        group: &GroupKey,
        event: AgentEvent,
        settles: Vec<EventKey>,
        shown_by: Option<String>,
    ) -> Vec<Act> {
        let mut chunks: Vec<Vec<EventKey>> =
            settles.chunks(SETTLES_MAX).map(<[_]>::to_vec).collect();
        if chunks.is_empty() {
            chunks.push(Vec::new());
        }
        let mut acts = Vec::new();
        for settles in chunks {
            let record = self
                .saved
                .items
                .get_mut(key)
                .expect("an item the core knows");
            record.version += 1;
            let what = Sending::Item {
                key: key.to_string(),
                item: Box::new(Item {
                    event: event.clone(),
                    version: record.version,
                    first: record.first.clone(),
                    started_by: record.started_by.clone(),
                    settles,
                }),
            };
            acts.push(self.queue(group, what, shown_by.clone()));
        }
        acts
    }

    /// MDK refused the send named `token`, and will refuse it again, or the core gave it up. When
    /// it was an edit of words whose message is gone (`gone`), the words go as a new message
    /// instead. Anything else is dropped, and the chat is told in a `failed` notice that settles,
    /// as failed, what it settled, and what a turn's version queued to settle for it would have
    /// (S1). When it was an item's closing version, with nothing later queued for the item and
    /// no earlier close of it in the chat, the item is closed in the chat all the same, by a
    /// version of the core's own (S2). When it was a notice that settled requests, or a `failed`
    /// notice that told of a dropped send, a `failed` notice takes its place, settling what it
    /// settled (L1, S3). What stands in for a dropped send is as small as the core can make it,
    /// leaving out the session and turn the driver chose when they are what is too large, so MDK
    /// has no size to refuse again, and is the last: when it is dropped too, as in a group that is
    /// gone, nothing takes its place, so nothing loops. Everything is queued for the same group.
    pub fn refused(&mut self, token: &str, gone: bool) {
        self.dropped(token, gone, NOT_DELIVERED);
    }

    /// The send named `token` is given up without knowing whether it went out: MDK no longer
    /// knows its token, and trying it again could send it twice. It is dropped as a refused one
    /// is ([`Conversation::refused`]), and the chat's notice says it may not have been delivered.
    pub fn lost(&mut self, token: &str) {
        self.dropped(token, false, NOT_KNOWN);
    }

    fn dropped(&mut self, token: &str, gone: bool, text: &str) {
        let Some(at) = self.saved.outbox.iter().position(|q| q.token == token) else {
            return;
        };
        let key = self.saved.outbox[at].what.key().to_string();
        if gone && matches!(self.saved.outbox[at].what, Sending::Words { .. }) {
            if let Some(record) = self.saved.items.get_mut(&key) {
                record.first = None;
                record.first_token = None;
            }
            if let Sending::Words { edit, .. } = &mut self.saved.outbox[at].what {
                *edit = None;
            }
            return;
        }
        let queued = self.saved.outbox.remove(at).expect("found");
        if !queued.last_resort {
            self.stand_in_for(token, queued, text);
        }
        self.forget_old();
    }

    /// What goes in place of a dropped send that was not itself a last resort.
    fn stand_in_for(&mut self, token: &str, queued: Queued, text: &str) {
        let key = queued.what.key().to_string();
        let notice = key.starts_with("notice/");
        let item = matches!(queued.what, Sending::Item { .. });
        let mut settles = match queued.what {
            Sending::Item { item, .. } => item.settles,
            Sending::Words { settles, .. } => settles,
        };
        // A turn's version that was to settle what these words showed shows nothing now.
        for later in &mut self.saved.outbox {
            if later.shown_by.as_deref() == Some(token)
                && let Sending::Item { item, .. } = &mut later.what
            {
                settles.append(&mut item.settles);
            }
        }
        if item && !notice {
            self.close_undelivered(&key, &queued.group);
        }
        // A notice that settled nothing only told the chat something: it is not told again,
        // unless what it told of was a dropped send, which the chat is told of once more.
        if notice && settles.is_empty() && !queued.tells {
            return;
        }
        let chat = (self.saved.items.get(&key).map(|r| r.chat.clone()))
            .or_else(|| self.saved.chats.get(&queued.group).cloned())
            .unwrap_or_default();
        let chat = Chat {
            id: ChatId(chat),
            group: true,
            name: None,
            folder: None,
        };
        let mut chunks: Vec<Vec<EventKey>> =
            settles.chunks(SETTLES_MAX).map(<[_]>::to_vec).collect();
        if chunks.is_empty() {
            chunks.push(Vec::new());
        }
        for settles in chunks {
            // Without the chat's session, which the driver chose and which may be what made the
            // dropped send too large: nothing the notice needs.
            let event = self.notice(&queued.group, &chat, "failed", text.into());
            let header = Header {
                session: None,
                ..event.header().clone()
            };
            let event = AgentEvent::new(header, event.body().clone(), None)
                .expect("a notice repeats no secret");
            let what = notice_item(event, None, settles);
            // In place of a notice, the last resort; in place of anything else, a notice like any.
            self.queue_as(&queued.group, what, None, notice);
            if let Some(queued) = self.saved.outbox.back_mut() {
                queued.tells = true;
            }
        }
    }

    /// Closes in the chat the item named `key`, whose closing version was dropped: when the core
    /// holds it closed, the chat does not already show it closed, and nothing later of it is
    /// queued, its next version is queued, in its final state, as small as it can be: its ids,
    /// state and version, a `warning` notice in place of its body, with no native message and
    /// settling nothing. When its session or turn id, which the driver chose, would make even
    /// that too large, they are left out: the item's id, first event, version and state stay.
    fn close_undelivered(&mut self, key: &str, group: &GroupKey) {
        let Some(record) = self.saved.items.get(key) else {
            return;
        };
        if record.open()
            || record.closed_in_chat
            || (self.saved.outbox.iter()).any(|q| q.what.key() == key)
        {
            return;
        }
        let header = Header {
            title: CLOSED_TITLE.into(),
            text: CLOSED_TEXT.into(),
            ..record.last.header().clone()
        };
        let body = Body::Notice(Notice {
            kind: NoticeKind::Warning,
            duration_ms: None,
            reason: Some("not_delivered".into()),
        });
        let event = AgentEvent::new(header.clone(), body.clone(), None)
            .expect("a notice repeats no secret");
        let mut item = Item {
            event,
            version: record.version + 1,
            // As it will go: naming the item's first event, filled in when it is sent.
            first: Some(
                record
                    .first
                    .clone()
                    .unwrap_or_else(|| EventKey("0".repeat(64))),
            ),
            started_by: record.started_by.clone(),
            settles: Vec::new(),
        };
        if crate::carrier::item(&item).is_err() {
            let header = Header {
                session: None,
                turn: None,
                ..header
            };
            item.event = AgentEvent::new(header, body, None).expect("a notice repeats no secret");
        }
        item.first = record.first.clone();
        let record = self.saved.items.get_mut(key).expect("found");
        record.version = item.version;
        let what = Sending::Item {
            key: key.to_string(),
            item: Box::new(item),
        };
        self.queue_as(group, what, None, true);
    }

    /// The send named `token` is about to be handed to MDK, `now` (seconds since 1970). Whether
    /// that is new, the first try under its token, and so to be saved before it is: what MDK
    /// takes is on disk first, with the token it is taken under and when it was first tried.
    pub fn attempting(&mut self, token: &str, now: u64) -> bool {
        match self.saved.outbox.iter_mut().find(|q| q.token == token) {
            Some(queued) if queued.tried_at.is_none() => {
                queued.attempted = true;
                queued.tried_at = Some(now);
                true
            }
            _ => false,
        }
    }

    /// MDK took the send named `token` and dropped it before it went out: nothing of it went,
    /// and its next try is made under a new token. How many times that has happened to it, to be
    /// saved before it is tried again.
    pub fn rejected(&mut self, token: &str) -> u32 {
        match self.saved.outbox.iter_mut().find(|q| q.token == token) {
            Some(queued) => {
                queued.rejected += 1;
                queued.tried_at = None;
                queued.rejected
            }
            None => 0,
        }
    }

    /// Everything queued for `group`, oldest first, each with its token, as
    /// [`Conversation::next_send`] gives it.
    pub fn queued(&self, group: &GroupKey) -> Vec<(Token, Sending)> {
        (self.saved.outbox.iter())
            .filter(|q| q.group == *group)
            .map(|q| self.as_sent(q))
            .collect()
    }

    /// The next thing to send to `group`, and the token that names this send each time it is
    /// tried. An item's first event is filled in now, from the version before it that went out,
    /// so a later version always names it, and the agent's words become an edit once their first
    /// message has gone. `None` when nothing is queued for the group.
    pub fn next_send(&self, group: &GroupKey) -> Option<(Token, Sending)> {
        let queued = self.saved.outbox.iter().find(|q| q.group == *group)?;
        Some(self.as_sent(queued))
    }

    fn as_sent(&self, queued: &Queued) -> (Token, Sending) {
        let mut what = queued.what.clone();
        let record = self.saved.items.get(what.key());
        let mut edits = None;
        if let Some(first) = record.and_then(|r| r.first.clone()) {
            // An edit names the token its first event went under.
            edits = record.and_then(|r| r.first_token.clone());
            match &mut what {
                Sending::Item { item, .. } => item.first = Some(first),
                Sending::Words { edit, settles, .. } => {
                    *edit = Some(first);
                    // Never: words carry what they settle only when they are the first message.
                    settles.clear();
                }
            }
        }
        let token = Token {
            send: queued.token.clone(),
            mdk: self.mdk_token(queued),
            edits,
            tried_at: queued.tried_at,
        };
        (token, what)
    }

    /// The token MDK keeps the send `queued` under, for its try now: made from the state file's
    /// salt, the send's name and how many of its tries MDK dropped, so it is the same each time
    /// the send is tried until MDK drops a try, and never one MDK holds for another send. A
    /// digest, since a name holds the item id the driver chose, whatever its length, and MDK
    /// takes at most 128 bytes.
    fn mdk_token(&self, queued: &Queued) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        for part in [self.saved.salt.as_bytes(), queued.token.as_bytes()] {
            digest.update((part.len() as u64).to_le_bytes());
            digest.update(part);
        }
        digest.update(queued.rejected.to_le_bytes());
        format!("marmot-agent-1-{}", hex::encode(digest.finalize()))
    }

    /// The groups something is queued for, in the order of their oldest send.
    pub fn queued_groups(&self) -> Vec<GroupKey> {
        let mut groups: Vec<GroupKey> = Vec::new();
        for queued in &self.saved.outbox {
            if !groups.contains(&queued.group) {
                groups.push(queued.group.clone());
            }
        }
        groups
    }

    /// The oldest send queued for the item named `key` went out, and `event` is its id. The first
    /// event of an item is what its later versions name.
    pub fn sent(&mut self, key: &str, event: EventKey) {
        let mut closed = None;
        let mut token = None;
        if let Some(at) = self.saved.outbox.iter().position(|q| q.what.key() == key) {
            token = Some(self.mdk_token(&self.saved.outbox[at]));
            let queued = self.saved.outbox.remove(at).expect("found");
            if let Sending::Item { item, .. } = &queued.what {
                closed = Some(!matches!(
                    item.event.header().state,
                    State::Running | State::Waiting
                ));
            }
        }
        if let Some(record) = self.saved.items.get_mut(key) {
            if record.first.is_none() {
                record.first = Some(event);
                record.first_token = token;
            }
            if let Some(closed) = closed {
                record.closed_in_chat = closed;
            }
        }
        self.forget_old();
    }

    /// Queues something to send, and says so. `shown_by` names the send that shows the effect of
    /// what it settles, when that is another.
    fn queue(&mut self, group: &GroupKey, what: Sending, shown_by: Option<String>) -> Act {
        self.queue_as(group, what, shown_by, false)
    }

    /// Queues something to send, as [`Conversation::queue`], and as a last resort when
    /// `last_resort`: nothing stands in for it if it is dropped.
    fn queue_as(
        &mut self,
        group: &GroupKey,
        what: Sending,
        shown_by: Option<String>,
        last_resort: bool,
    ) -> Act {
        let token = match &what {
            Sending::Item { key, item } => format!("{key}@{}", item.version),
            Sending::Words { key, .. } => format!("{key}@{}", self.make_id("w")),
        };
        self.saved.outbox.push_back(Queued {
            group: group.clone(),
            token,
            what,
            shown_by,
            attempted: false,
            rejected: 0,
            tried_at: None,
            last_resort,
            tells: false,
        });
        Act::Send {
            group: group.clone(),
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
        let folder = self
            .saved
            .folders
            .get(&id)
            .filter(|_| self.folders.is_some());
        Chat {
            folder: folder.cloned(),
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

    /// The folder a move the driver took is starting `chat`'s new session in, before the session
    /// has shown: a session the driver starts for a prompt meanwhile starts there (D-101).
    fn moving_to(&self, chat: &str) -> Option<PathBuf> {
        (self.saved.pending.values())
            .filter(|p| p.chat == chat && p.deadline.is_some())
            .find_map(|p| p.folder.clone())
    }

    /// A session item that shows the effect of a move makes the move's folder the chat's, as the
    /// session item makes the session the chat's: both are saved together, and a move the harness
    /// refuses changes neither (D-101).
    fn moved(&mut self, chat: &str, emitted: &Emitted) {
        if !matches!(emitted.event.body(), Body::Session(_)) {
            return;
        }
        for request in &emitted.settles {
            if let Some(pending) = self.saved.pending.get_mut(&request.0)
                && pending.chat == chat
                && let Some(folder) = pending.folder.take()
            {
                self.saved.folders.insert(chat.to_string(), folder);
            }
        }
    }

    /// Whether a `set` of `setting` moves the chat's folder: the core's own setting, for a driver
    /// that works in a folder a chat may move (D-101).
    fn moves_folder(&self, setting: &str) -> bool {
        setting == FOLDER && self.folders.is_some()
    }

    /// A session item, with the chat's folder among its settings, marked as needing the right to
    /// approve (D-101). A session keeps the folder it started in: once an item has shown it, its
    /// later versions show the same, even after the chat has moved.
    fn show_folder(&self, chat: &str, emitted: Emitted) -> Emitted {
        let Body::Session(session) = emitted.event.body() else {
            return emitted;
        };
        let Some(now) = self.chat_folder(&ChatId(chat.to_string())) else {
            return emitted;
        };
        let header = emitted.event.header();
        let key = format!("{chat}/{}", header.item.as_str());
        let kept = (self.saved.items.get(&key))
            .filter(|r| r.last.header().session == header.session)
            .and_then(|r| match r.last.body() {
                Body::Session(s) => (s.settings.iter())
                    .find(|x| x.id == FOLDER)
                    .and_then(|x| x.value.clone()),
                _ => None,
            });
        let mut session = session.clone();
        session.settings.retain(|s| s.id != FOLDER);
        session.settings.push(Setting {
            id: FOLDER.into(),
            label: LABEL.into(),
            value: Some(kept.unwrap_or_else(|| now.display().to_string())),
            options: None,
            approve: true,
        });
        let event = AgentEvent::new(
            header.clone(),
            Body::Session(session),
            emitted.event.native().cloned(),
        )
        .expect("a session item repeats no secret");
        Emitted { event, ..emitted }
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
        if !self.saved.finished.contains(&key) {
            self.saved.finished.push_back(key);
        }
        self.forget_old();
    }

    /// Forgets the oldest finished items over [`KEPT`] that may be forgotten: when an item
    /// finishes, and when a send goes, since a version waiting to be sent keeps its item.
    fn forget_old(&mut self) {
        if self.saved.finished.len() <= KEPT {
            return;
        }
        let queued: HashSet<String> = (self.saved.outbox.iter())
            .map(|q| q.what.key().to_string())
            .collect();
        while self.saved.finished.len() > KEPT {
            let at = (self.saved.finished.iter()).position(|k| self.may_forget(k, &queued));
            let Some(at) = at else {
                break;
            };
            let old = self.saved.finished.remove(at).expect("found");
            if let Some(record) = self.saved.items.remove(&old) {
                self.saved.forgotten.push_back(Forgotten {
                    key: old,
                    first: record.first,
                    first_token: record.first_token,
                    version: record.version,
                });
            }
            while self.saved.forgotten.len() > KEPT {
                self.saved.forgotten.pop_front();
            }
        }
    }

    /// Whether a finished item may be forgotten: not when it is open, nor when it is its chat's
    /// session, which a stop closes and the next prompt resumes (D-065, D-070), nor while a
    /// version of it waits to be sent (it is among `queued`).
    fn may_forget(&self, key: &str, queued: &HashSet<String>) -> bool {
        let Some(record) = self.saved.items.get(key) else {
            return true;
        };
        let session = matches!(record.last.body(), Body::Session(_))
            && record.last.header().session.is_some()
            && self.saved.sessions.get(&record.chat) == record.last.header().session.as_ref();
        !record.open() && !session && !queued.contains(key)
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
        self.settle_pending(request, reason_word(refusal.reason), not_acted_on(&refusal))
    }

    /// Settles a pending request with a `refused` notice giving `reason` and `text`.
    fn settle_pending(&mut self, request: &str, reason: &str, text: String) -> Vec<Act> {
        let Some(pending) = self.saved.pending.remove(request) else {
            return Vec::new();
        };
        let chat = Chat {
            id: ChatId(pending.chat),
            group: true,
            name: None,
            folder: None,
        };
        let notice = self.notice(&pending.group, &chat, reason, text);
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
        // A place in the session from 1, as MDK takes it (D-059): with the last its chat showed,
        // so a client that orders a second's items by it shows the notice after them.
        let seq = (self.saved.items.values())
            .filter(|r| r.chat == chat.id.0)
            .map(|r| r.last.header().seq)
            .max()
            .unwrap_or(0)
            .max(1);
        let header = Header {
            harness: self.harness.clone(),
            session: self.saved.sessions.get(&chat.id.0).cloned(),
            turn: None,
            item: ItemId(self.make_id("core")),
            seq,
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
        let what = notice_item(event, about, settles);
        self.queue(group, what, None)
    }
}

/// A notice of the core's own, to send: the first and only version of an item of its own.
fn notice_item(event: AgentEvent, about: Option<EventKey>, settles: Vec<EventKey>) -> Sending {
    Sending::Item {
        key: format!("notice/{}", event.header().item.as_str()),
        item: Box::new(Item {
            event,
            version: 1,
            first: None,
            started_by: about,
            settles,
        }),
    }
}

/// Removes from `pending` each of `requests` that belongs to `group`, and gives their events: what
/// an event in that group settles. A request from another chat is left as it is (S1, S5).
fn take_pending(
    pending: &mut BTreeMap<String, Pending>,
    group: &GroupKey,
    requests: &[String],
) -> Vec<EventKey> {
    let mut events = Vec::new();
    for request in requests {
        if pending.get(request).is_some_and(|p| p.group == *group)
            && let Some(taken) = pending.remove(request)
        {
            events.push(taken.event);
        }
    }
    events
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
        Reason::NotAllowed => "not_allowed",
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
        Reason::NotAllowed => "The agent may not do that.",
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
