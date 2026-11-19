//! The Codex driver.
//!
//! It speaks `codex app-server`, Codex's JSON-RPC interface over standard input and output, with
//! message types of its own (D-055), and turns what Codex says into agent events.
//!
//! This is the first half: [`Translator`], which reads the messages and makes the events. It does
//! no input or output, and is tested by replaying the recordings in `fixtures/recordings/codex`
//! against the events the prototype makes for each. The second half, which runs Codex and
//! implements the driver trait, is not written yet.

mod messages;
mod translate;

pub use translate::{Direction, NOT_SENT, Translator};
