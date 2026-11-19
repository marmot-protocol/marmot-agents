//! What a driver says its harness can do.
//!
//! The core and the apps read these and never check a harness's name. A connector on the socket
//! will state the same facts in its hello. How they are written there is for the socket to
//! define: these are plain Rust values. The list is a proposal (`docs/open-questions.md`, Q-16),
//! drawn from the draft in `docs/design/socket-protocol.md`.

/// What a harness can do, as its driver states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Whether a running turn can be stopped.
    pub interrupt: bool,
    /// What the harness does with a prompt that arrives during a turn. An app can then say what
    /// will happen before the message is sent (D-040).
    pub mid_turn: MidTurn,
    /// Who picks the session a prompt goes to.
    pub sessions: Sessions,
    /// Whether a session survives a restart of the harness.
    pub resume: bool,
    /// Whether the harness reports a tool as an item. One gateway reports only a line of text.
    pub tool_items: bool,
    /// Whether the harness asks for approval through the chat.
    pub approvals: bool,
    /// How a question is answered. A harness can use both ways, or ask no questions.
    pub questions: Questions,
    /// Whether the harness reports its thinking.
    pub reasoning: bool,
    /// Which attachments a prompt may carry.
    pub files_in: FilesIn,
    /// Typed commands that need the right to approve (D-032, D-047). The core checks every
    /// message against this list, because a typed message otherwise needs only the right to
    /// prompt. A driver always states it, even when it is empty: leaving it out by mistake would
    /// switch the check off.
    pub approval_commands: Vec<ApprovalCommand>,
}

/// What a harness does with a prompt that arrives while a turn is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MidTurn {
    /// Folds it into the running turn.
    Fold,
    /// Ends the turn and starts another.
    Restart,
    /// Holds it until the turn ends.
    Queue,
}

/// Who picks the session a prompt goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sessions {
    /// The core names the session on each prompt.
    Core,
    /// The harness routes prompts to sessions by its own rules, as a gateway does.
    Own,
}

/// The ways a harness has a question answered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Questions {
    /// By an answer that points at the question item.
    pub request: bool,
    /// By the person's next chat message.
    pub message: bool,
}

/// Which attachments a prompt may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilesIn {
    /// None.
    None,
    /// Images only.
    Images,
    /// Any file.
    Any,
}

/// A typed command that needs the right to approve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalCommand {
    /// The command as typed, such as `/approve`.
    pub command: String,
    /// Where in a message it counts.
    pub counts: Position,
}

/// Where in a message a typed command counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Position {
    /// Only at the start of the message.
    Start,
    /// Anywhere in the message.
    Anywhere,
}
