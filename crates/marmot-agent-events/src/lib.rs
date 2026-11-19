//! The item types that every Marmot agent driver produces.
//!
//! A driver turns each native harness message into an agent event. Every event names one item
//! type from a shared list and one state. This crate will hold the whole event: the fields of
//! each type, the plain rendering and the native payload. Today it holds the two lists.
//!
//! The lists follow `docs/design/event-model.md`. That page is a draft, so they can still change.

use std::fmt;

/// What an item is. A client uses it to choose how to draw the item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemType {
    /// A turn started or ended.
    Turn,
    /// Text the agent says to the people in the chat.
    Message,
    /// The agent's thinking, when the harness provides it.
    Reasoning,
    /// A tool the agent used: a command, an edit, a search and so on.
    Tool,
    /// The agent wants permission for one action.
    Approval,
    /// The agent asks the person something.
    Question,
    /// The agent's plan or to-do list.
    Plan,
    /// Work that runs beside a turn or outlives it: a subagent, a background job.
    Task,
    /// Compaction, the agent waiting, a retry, a warning, a model change, the output of a command.
    Notice,
    /// Something failed outside any one item.
    Error,
    /// A session with the harness, and the settings, actions and slash commands it offers.
    Session,
}

impl ItemType {
    /// Every item type, in the order of the design page.
    pub const ALL: [Self; 11] = [
        Self::Turn,
        Self::Message,
        Self::Reasoning,
        Self::Tool,
        Self::Approval,
        Self::Question,
        Self::Plan,
        Self::Task,
        Self::Notice,
        Self::Error,
        Self::Session,
    ];

    /// The name of this type in an agent event.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Message => "message",
            Self::Reasoning => "reasoning",
            Self::Tool => "tool",
            Self::Approval => "approval",
            Self::Question => "question",
            Self::Plan => "plan",
            Self::Task => "task",
            Self::Notice => "notice",
            Self::Error => "error",
            Self::Session => "session",
        }
    }
}

impl fmt::Display for ItemType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where an item stands. An item changes by being sent again with a new state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// The work is in progress.
    Running,
    /// A person is needed: an approval or a question that nobody has answered.
    Waiting,
    /// It finished.
    Done,
    /// It finished with an error.
    Failed,
    /// A person refused it.
    Declined,
    /// It was stopped before it finished.
    Cancelled,
}

impl State {
    /// Every state, in the order of the design page.
    pub const ALL: [Self; 6] = [
        Self::Running,
        Self::Waiting,
        Self::Done,
        Self::Failed,
        Self::Declined,
        Self::Cancelled,
    ];

    /// The name of this state in an agent event.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{ItemType, State};

    #[test]
    fn item_types_have_the_names_of_the_design_page() {
        let names: Vec<&str> = ItemType::ALL.iter().map(|t| t.as_str()).collect();
        assert_eq!(
            names,
            [
                "turn",
                "message",
                "reasoning",
                "tool",
                "approval",
                "question",
                "plan",
                "task",
                "notice",
                "error",
                "session",
            ]
        );
    }

    #[test]
    fn states_have_the_names_of_the_design_page() {
        let names: Vec<&str> = State::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "running",
                "waiting",
                "done",
                "failed",
                "declined",
                "cancelled"
            ]
        );
    }
}
