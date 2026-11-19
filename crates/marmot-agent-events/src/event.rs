//! One agent event: the parts every item type shares.

use crate::body::Body;
use crate::{ItemType, State};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
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
    /// The order in which the driver produced its events, counting from 1.
    pub seq: u64,
    /// Where the item stands.
    pub state: State,
    /// A few words: "Running a command", "Allow this command?".
    pub title: String,
    /// The plain rendering, readable on its own by a client that does not know the item type.
    pub text: String,
    /// The item type and the fields that belong to it.
    #[serde(flatten)]
    pub body: Body,
    /// The harness's own message. An event the driver makes up itself has none: nothing arrives
    /// from a harness when the driver closes an item the harness never closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<Native>,
}

impl AgentEvent {
    /// The item type, which a client uses to choose how to draw the item.
    pub fn item_type(&self) -> ItemType {
        self.body.item_type()
    }
}

/// A harness's own message, kept exactly as it arrived.
///
/// A driver must never drop or change it (D-007). The payload is therefore the text of the JSON
/// the harness sent, not a value read from it: spacing, the order of fields and the way a number
/// was written all survive.
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
