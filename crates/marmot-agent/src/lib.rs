//! The core program, `marmot-agent`: the agent's member of a Marmot group (D-002, D-050).
//!
//! This crate has, so far, what the core decides without MDK or a harness: the rights people
//! hold ([`rights`]), how items, answers and controls travel for now ([`carrier`]), and what to do
//! with each message, answer, control and agent event ([`conversation`]). The Marmot side and the
//! program that runs it come next (`docs/plan.md`, M1).

pub mod carrier;
pub mod conversation;
pub mod keys;
pub mod rights;

pub use conversation::{Act, Conversation, Heard, HeardQuote, HeardRequest, Saved, Sending, Where};
pub use keys::{EventKey, GroupKey, PubKey};
pub use rights::{Right, Rights, Who};
