//! The Codex driver.
//!
//! It speaks `codex app-server`, Codex's JSON-RPC interface over standard input and output, with
//! message types of its own (D-055), and turns what Codex says into agent events.
//!
//! It has two halves. [`Translator`] reads the messages and makes the events, and builds what the
//! driver sends for a prompt, an answer or a control. It does no input or output, and is tested by
//! replaying the recordings in `fixtures/recordings/codex` against the events the prototype makes
//! for each, and against what the recorded client sent. [`Codex`] is the transport: it runs
//! `codex app-server`, gives each chat a translator, and implements the driver trait. It is tested
//! against a stand-in for Codex, and against a real one when asked for.

mod driver;
mod messages;
mod send;
mod translate;

pub use driver::Codex;
pub use send::{
    ApprovalPolicy, SEND_FILE, Sandbox, Setup, collaboration_mode_list, initialize, initialized,
    model_list, send_file_tool, tool_reply,
};
pub use translate::{Direction, NOT_SENT, Translator};
