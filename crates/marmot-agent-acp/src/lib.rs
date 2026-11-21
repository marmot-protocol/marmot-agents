//! The ACP driver.
//!
//! It speaks the Agent Client Protocol (ACP) as a client, the way an editor does, to an agent such
//! as `opencode acp`, and turns what the agent says into agent events. It reads ACP's messages with
//! the official ACP types, from `agent-client-protocol-schema`, the crate the official library
//! `agent-client-protocol` re-exports them from (D-005, D-006).
//!
//! It has two halves. [`Translator`] reads the messages in both directions and makes the
//! events, and builds the replies to the agent's requests. It does no input or output, and is tested by replaying the recordings in
//! `fixtures/recordings/acp` against the events the prototype makes for each. [`Acp`] is the
//! transport: it runs the agent, gives each chat a translator, and implements the driver trait.

mod driver;
mod translate;

pub use driver::Acp;
pub use translate::{Direction, Translator};
