//! The Marmot side's own keys: a group, an event, a person's public key. They never reach a
//! driver, which sees the core's ids instead (D-036), and they are never printed (D-071).

use serde::{Deserialize, Serialize};

macro_rules! key {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// The key as text, as MDK gives it.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(key: &str) -> Self {
                Self(key.to_string())
            }
        }

        /// Prints nothing of the key: it identifies a group or a person (D-071).
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "(..)"))
            }
        }
    };
}

key!(
    /// A Marmot group, by MDK's group id.
    GroupKey
);
key!(
    /// An event in a group, by its id.
    EventKey
);
key!(
    /// A person, or the agent, by public key.
    PubKey
);
