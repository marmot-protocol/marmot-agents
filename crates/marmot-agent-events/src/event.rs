//! One agent event: the parts every item type shares.

use crate::State;
use crate::body::Body;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::fmt;

/// Declares an id that is text on the wire and its own type in Rust, so that the id of a turn
/// cannot be passed where the id of an item is wanted.
macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
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

        impl From<String> for $name {
            fn from(id: String) -> Self {
                Self(id)
            }
        }
    };
}

id!(
    /// The harness's own id for a session.
    SessionId
);
id!(
    /// The id of a turn.
    TurnId
);
id!(
    /// The id of an item. It stays the same for the life of the item: an event with an id that
    /// was sent before replaces the earlier one.
    ItemId
);

/// What a driver sends for every step of an agent's work.
///
/// An item changes by being sent again. A tool call is sent when it starts, with the state
/// `running`, and again when it finishes, with the state `done` and its output. Both events have
/// the same `item`.
///
/// On the wire the item type is `type` and its own fields are `body`. Here the two are one value,
/// [`Body`], so a type cannot be paired with another type's fields.
///
/// An event cannot be changed once it is built, because one rule ties its parts together: a
/// question item that has a secret question carries a native message only while it is waiting
/// for an answer (D-056). An event that breaks the rule cannot be built, and cannot be read from JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Unchecked")]
pub struct AgentEvent {
    #[serde(flatten)]
    header: Header,
    #[serde(flatten)]
    body: Body,
    #[serde(skip_serializing_if = "Option::is_none")]
    native: Option<Native>,
}

impl AgentEvent {
    /// An event. `native` is the harness's own message. An event the driver makes up itself has
    /// none: nothing arrives from a harness when the driver closes an item the harness never
    /// closed.
    ///
    /// It is refused if it could repeat the answer to a secret question. While such a question
    /// is waiting and unanswered, the harness's message is the request, which holds no answer.
    /// Otherwise it may be the response, which does. So a question item with a secret question
    /// must come with no native message unless its state is `waiting` and it records no answer.
    ///
    /// The types cannot read a native message. A driver that hands over the response and calls
    /// the item waiting and unanswered is not caught here; that is for the driver's own tests.
    pub fn new(header: Header, body: Body, native: Option<Native>) -> Result<Self, RepeatsSecret> {
        if let Body::Question(question) = &body {
            let secret = question.questions().iter().any(|q| q.secret);
            let still_asking = header.state == State::Waiting && question.answer().is_none();
            if secret && native.is_some() && !still_asking {
                return Err(RepeatsSecret);
            }
        }
        Ok(Self {
            header,
            body,
            native,
        })
    }

    /// The parts every item shares.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The item type and the fields that belong to it.
    pub fn body(&self) -> &Body {
        &self.body
    }

    /// The harness's own message, when the event has one.
    pub fn native(&self) -> Option<&Native> {
        self.native.as_ref()
    }
}

/// The parts every item shares, whatever its type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// Which harness produced the event.
    pub harness: String,
    /// The harness's session. A harness can produce an event before it has named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// The turn the event belongs to. A session item and some notices belong to none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnId>,
    /// The item this event creates or replaces.
    pub item: ItemId,
    /// The event's place among the events of its session, from 1, in the order the driver
    /// produced them (D-059).
    pub seq: u64,
    /// Where the item stands.
    pub state: State,
    /// A few words: "Running a command", "Allow this command?".
    pub title: String,
    /// The plain rendering, readable on its own by a client that does not know the item type.
    pub text: String,
}

/// An event would have carried a native message that could repeat the answer to a secret
/// question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatsSecret;

impl fmt::Display for RepeatsSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a secret question carries a native message only while it is waiting")
    }
}

impl std::error::Error for RepeatsSecret {}

/// An event as it is read from JSON, before the rule above is checked.
#[derive(Deserialize)]
struct Unchecked {
    #[serde(flatten)]
    header: Header,
    #[serde(flatten)]
    body: Body,
    #[serde(default)]
    native: Option<Native>,
}

impl TryFrom<Unchecked> for AgentEvent {
    type Error = RepeatsSecret;

    fn try_from(read: Unchecked) -> Result<Self, RepeatsSecret> {
        Self::new(read.header, read.body, read.native)
    }
}

/// A harness's own message, kept exactly as it arrived.
///
/// A driver must never drop or change it (D-007). The payload is therefore the text of the JSON
/// the harness sent, not a value read from it: spacing, the order of fields and the way a number
/// was written all survive.
///
/// Two things leave it out of an event. A secret question that is no longer waiting carries
/// none (D-056), which [`AgentEvent::new`] holds. And the core leaves it out of an item over 64 KB
/// (D-026, D-057). A driver never leaves it out to fit that limit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Native {
    /// What the harness calls this message, such as `item/completed`.
    pub kind: String,
    /// The message itself.
    pub payload: Box<RawValue>,
}

impl PartialEq for Native {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.payload.get() == other.payload.get()
    }
}
