//! What the core hands a driver, and how a driver refuses.
//!
//! These are the three requests of the socket protocol's driver section
//! (`docs/design/socket-protocol.md`): a prompt, an answer and a control. Everything a driver
//! receives has already been checked by the core: the sender may prompt, or may approve.
//!
//! Printing any of these for debugging shows which things a request is about, by their ids, and
//! what kind of request it is. It never shows a value: no message text, no name, no answer, no
//! new setting, nothing about an attachment, none of the harness's own words. A log must never
//! hold those (D-071). The ids shown are the core's own, and the ids of settings and actions that
//! the harness published. A question's id is not shown: the agent writes it, and it can say what
//! was asked. Nor is a session's or an item's: the harness made them.

use marmot_agent_events::{ItemId, SessionId, TurnId};
use std::fmt;
use std::path::PathBuf;

/// Declares an id that the core makes up for this install (D-036). It is never a group's id or a
/// person's public key, so a harness's own logs identify nobody.
macro_rules! core_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name(pub String);

        impl $name {
            /// The id as text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(id: &str) -> Self {
                Self(id.to_string())
            }
        }
    };
}

core_id!(
    /// The core's id for a chat.
    ChatId
);
core_id!(
    /// The core's id for a person.
    PersonId
);
core_id!(
    /// The core's id for a chat message.
    MessageId
);
core_id!(
    /// The core's id for one answer or one control. The driver hands it back on the event that
    /// shows what the request did, so the core can tell that event from a change nobody asked
    /// for (D-028, D-046).
    RequestId
);

/// A chat, as a driver sees it. Printed for debugging, it shows the chat's id and whether it is a
/// group, and neither its name nor its folder (D-071).
#[derive(Clone, PartialEq, Eq)]
pub struct Chat {
    /// The core's id for the chat.
    pub id: ChatId,
    /// Whether the chat has more than one person in it.
    pub group: bool,
    /// The chat's name, when it has one.
    pub name: Option<String>,
    /// The folder the chat chose for its sessions, as a whole path with every link followed
    /// (D-101). A driver starts the chat's next session there, and never moves a running one:
    /// the folder applies from the next session. `None` when the chat chose none, and the driver
    /// uses its own folder ([`Driver::folder`](crate::Driver::folder)). Only a driver that states
    /// a folder of its own is handed one.
    pub folder: Option<PathBuf>,
}

/// A person, as a driver sees them. The name is passed as it is, because the model needs it.
#[derive(Clone, PartialEq, Eq)]
pub struct Person {
    /// The core's id for the person.
    pub id: PersonId,
    /// The person's name.
    pub name: String,
}

/// A message for the agent. A slash command is one too: it is passed exactly as typed (D-031).
#[derive(Clone, PartialEq, Eq)]
pub struct Prompt {
    /// The message's id. A turn names the prompts it responds to by this id.
    pub message: MessageId,
    /// The message's text.
    pub text: String,
    /// The chat it was sent in.
    pub chat: Chat,
    /// Who sent it.
    pub sender: Person,
    /// The message it replies to, when it is a reply.
    pub reply_to: Option<Quoted>,
    /// Files sent with it. The core has already written each one to disk.
    pub attachments: Vec<Attachment>,
    /// The session to resume, and only that: the chat's own session from before the driver
    /// started, until the driver has taken a prompt or a new session for the chat (D-065).
    /// Otherwise none, and the prompt goes to the chat's current session, or a new one.
    pub session: Option<SessionId>,
}

/// The message a prompt replies to. The text is quoted because some harnesses cannot look a
/// message up by its id.
#[derive(Clone, PartialEq, Eq)]
pub struct Quoted {
    /// The id of the message replied to.
    pub message: MessageId,
    /// Its text.
    pub text: String,
    /// Who wrote it.
    pub author: Person,
}

/// A file sent with a prompt.
#[derive(Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Where the core wrote the file.
    pub path: PathBuf,
    /// The file's name.
    pub name: String,
    /// The file's media type, such as `image/png`.
    pub media_type: String,
}

impl fmt::Debug for Chat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chat")
            .field("id", &self.id)
            .field("group", &self.group)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Person {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Person")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prompt")
            .field("message", &self.message)
            .field("chat", &self.chat)
            .field("sender", &self.sender)
            .field("reply_to", &self.reply_to)
            .field("attachments", &self.attachments.len())
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Quoted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Quoted")
            .field("message", &self.message)
            .field("author", &self.author)
            .finish_non_exhaustive()
    }
}

/// Shows nothing. The path and the name are the person's, and a media type can carry a name too.
impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attachment").finish_non_exhaustive()
    }
}

/// A person's answer to an approval or a question.
///
/// It goes to the harness's own answer call and never to the model as a prompt (D-028).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The core's id for this answer.
    pub request: RequestId,
    /// The item being answered.
    pub item: ItemId,
    /// Who answered.
    pub by: Person,
    /// What they answered.
    pub given: Given,
}

/// What an answer holds. It names choices by the ids the item offered.
#[derive(Clone, PartialEq, Eq)]
pub enum Given {
    /// To an approval: the id of one choice.
    Choice(String),
    /// To a question item: one entry for each question answered.
    Questions(Vec<Reply>),
}

impl fmt::Debug for Given {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Choice(_) => f.write_str("Choice(..)"),
            Self::Questions(replies) => f.debug_tuple("Questions").field(replies).finish(),
        }
    }
}

/// The answer to one question, as it is delivered to the harness.
///
/// This is not what the question item then records. It holds what the person answered, and for
/// a secret question that is the secret, whether they typed it or chose it. The item records
/// only that the question was answered (D-041). So nothing in it is shown when a reply is printed
/// for debugging: not the options, not the typed text, and not the question's id, which the agent
/// writes and which can say what was asked.
#[derive(Clone, PartialEq, Eq)]
pub struct Reply {
    /// The id of the question.
    pub question: String,
    /// The ids of the options chosen.
    pub options: Vec<String>,
    /// What the person typed, when the question allows it.
    pub text: Option<String>,
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reply").finish_non_exhaustive()
    }
}

/// Something a person does unprompted (D-029).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    /// The core's id for this control.
    pub request: RequestId,
    /// Who sent it.
    pub by: Person,
    /// What to do, and what to do it to.
    pub action: Action,
}

/// What a control does. Three actions mean the same for every harness. The fourth is whatever
/// the harness published in its session item.
#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    /// Stop the running turn, or one subagent under it.
    Interrupt {
        /// The turn to stop: the person's turn, which every subagent under it is under too.
        turn: TurnId,
        /// For a Stop in a subagent's thread: that subagent's `task` item. Only it and the
        /// subagents it started are stopped, and the turn goes on (D-123, D-149). `None` stops
        /// the turn and every subagent under it. The core names one only to a driver that says it
        /// can stop one ([`Capabilities::stop_subagent`](crate::Capabilities::stop_subagent)).
        subagent: Option<ItemId>,
    },
    /// End a chat's session, if it has one, and start a fresh one.
    NewSession {
        /// The chat the new session is for.
        chat: Chat,
        /// The session to end. A chat that has had no session has none.
        session: Option<SessionId>,
    },
    /// Change one setting of a session.
    Set {
        /// The session.
        session: SessionId,
        /// The id of the setting, from the session item.
        setting: String,
        /// The new value.
        value: String,
    },
    /// One of the harness's own actions, such as compacting.
    Own {
        /// The session.
        session: SessionId,
        /// The id of the action, from the session item.
        id: String,
    },
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupt { turn, subagent } => f
                .debug_struct("Interrupt")
                .field("turn", turn)
                .field("subagent", subagent)
                .finish(),
            Self::NewSession { chat, session } => f
                .debug_struct("NewSession")
                .field("chat", chat)
                .field("session", session)
                .finish(),
            Self::Set {
                session,
                setting,
                value: _,
            } => f
                .debug_struct("Set")
                .field("session", session)
                .field("setting", setting)
                .finish_non_exhaustive(),
            Self::Own { session, id } => f
                .debug_struct("Own")
                .field("session", session)
                .field("id", id)
                .finish(),
        }
    }
}

/// A driver did not take a request. The core turns this into the notice the chat sees.
#[derive(Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Why.
    pub reason: Reason,
    /// More than the reason says, in words the core shows in the chat. Every member of the chat
    /// reads them (D-009), so they never repeat a person's answer to a secret question (D-041).
    /// A driver puts the harness's own words here only when it can be sure of that, and otherwise
    /// leaves them out.
    pub detail: Option<String>,
    /// The request reached the driver's side and no answer came, as when a connector's connection
    /// goes before it answers: the harness may have it. A prompt refused so is told as perhaps
    /// not delivered, and is still never sent again (D-037, D-129).
    pub unanswered: bool,
    /// The driver has ended, as when its connection or its harness is gone, and none of the
    /// request reached it. A prompt the core handed on without seeing that the driver had ended
    /// was never given, and waits for the next driver (D-139).
    pub ended: bool,
}

impl Refusal {
    /// A refusal with no words from the harness.
    pub fn new(reason: Reason) -> Self {
        Self {
            reason,
            detail: None,
            unanswered: false,
            ended: false,
        }
    }

    /// The refusal of a driver that has ended, of a request none of which reached it.
    pub fn ended() -> Self {
        Self {
            ended: true,
            ..Self::new(Reason::Failed)
        }
    }
}

impl fmt::Debug for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refusal")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Why a driver did not take a request, or why the core did not send a file a driver asked it to.
/// These are the reasons the socket page lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The harness cannot do this.
    NotSupported,
    /// The harness cannot do this now.
    Busy,
    /// The driver does not know the item.
    UnknownItem,
    /// The item is not waiting for an answer.
    NotWaiting,
    /// The item has been answered.
    AlreadyAnswered,
    /// There is nothing to act on, such as no running turn to interrupt.
    NothingToDo,
    /// The request does not make sense, such as a choice the item did not offer.
    Invalid,
    /// The harness was asked and it failed.
    Failed,
    /// The core does not allow it: a file outside the folders the settings allow (D-090). Only
    /// the core gives this reason; a driver refuses with the others.
    NotAllowed,
}
