//! The core program, `marmot-agent`: the agent's member of a Marmot group (D-002, D-050).
//!
//! - [`conversation`]: what the core does with each message, answer, control and agent event,
//!   with no connection to anything, so every rule can be tested alone.
//! - [`rights`]: who may prompt and who may approve.
//! - [`carrier`]: how items, answers and controls travel in a group, for now (D-069).
//! - [`marmot`]: the Marmot side, through MDK.
//! - [`run`]: the program loop that joins MDK, a driver and the conversation.
//! - [`config`]: the standing settings (D-073).

pub mod carrier;
pub mod config;
pub mod conversation;
pub mod keys;
pub mod marmot;
pub mod rights;
pub mod run;

pub use conversation::{Act, Conversation, Heard, HeardQuote, HeardRequest, Saved, Sending, Where};
pub use keys::{EventKey, GroupKey, PubKey};
pub use rights::{Right, Rights, Who};
