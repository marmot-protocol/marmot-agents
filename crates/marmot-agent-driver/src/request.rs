//! What the core hands a driver, and how a driver refuses.
//!
//! These are the three requests of the socket protocol's driver section
//! (`docs/design/socket-protocol.md`): a prompt, an answer and a control. Everything a driver
//! receives has already been checked by the core: the sender may prompt, or may approve.
//!
//! Printing any of these for debugging shows the core's own ids and nothing a person wrote: no
//! message text, no name, no answer. A log must never hold those (AGENTS.md, "Privacy").

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

/// A chat, as a driver sees it.
#[derive(Clone, PartialEq, Eq)]
pub struct Chat {
    /// The core's id for the chat.
    pub id: ChatId,
    /// Whether the chat has more than one person in it.
    pub group: bool,
    /// The chat's name, when it has one.
    pub name: Option<String>,
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
    /// The session it is for, when the core chooses the session.
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

impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attachment")
            .field("media_type", &self.media_type)
            .finish_non_exhaustive()
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Given {
    /// To an approval: the id of one choice.
    Choice(String),
    /// To a question item: one entry for each question answered.
    Questions(Vec<Reply>),
}

/// The answer to one question, as it is delivered to the harness.
///
/// This is not what the question item then records. It holds what the person answered, and for
/// a secret question that is the secret, whether they typed it or chose it. The item records
/// only that the question was answered (D-041). So neither the options nor the typed text are
/// shown when a reply is printed for debugging.
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
        f.debug_struct("Reply")
            .field("question", &self.question)
            .finish_non_exhaustive()
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Stop the running turn.
    Interrupt {
        /// The turn to stop.
        turn: TurnId,
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

/// A driver did not take a request. The core turns this into the notice the chat sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// Why.
    pub reason: Reason,
    /// The harness's own words, when it gave any.
    pub detail: Option<String>,
}

impl Refusal {
    /// A refusal with no words from the harness.
    pub fn new(reason: Reason) -> Self {
        Self {
            reason,
            detail: None,
        }
    }
}

/// Why a driver did not take a request. These are the eight reasons the socket page lists.
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
}
