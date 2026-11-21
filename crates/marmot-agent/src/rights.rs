//! Who may prompt and who may approve (D-010, D-032, D-039, D-047).

use crate::keys::PubKey;
use marmot_agent_driver::{ApprovalCommand, Position};
use serde::{Deserialize, Serialize};

/// Who holds a right. The owner always does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Who {
    /// The owner alone, the default (D-010).
    #[default]
    Owner,
    /// The owner and these people.
    People(Vec<PubKey>),
    /// Everyone in the chat.
    Everyone,
}

/// The two rights the owner hands out (D-032).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rights {
    /// Who owns the agent.
    pub owner: PubKey,
    /// Who may prompt, use controls and answer questions (D-039).
    pub prompt: Who,
    /// Who may answer approvals and change what the agent may do without asking (D-032).
    pub approve: Who,
}

/// One of the two rights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Right {
    /// To prompt.
    Prompt,
    /// To approve.
    Approve,
}

impl Rights {
    /// Whether `person` holds `right`.
    pub fn allows(&self, person: &PubKey, right: Right) -> bool {
        if *person == self.owner {
            return true;
        }
        let who = match right {
            Right::Prompt => &self.prompt,
            Right::Approve => &self.approve,
        };
        match who {
            Who::Owner => false,
            Who::People(people) => people.contains(person),
            Who::Everyone => true,
        }
    }
}

/// The right a message needs: to approve when it carries one of the harness's typed commands
/// that approve or widen what the agent may do, where that command counts (D-047), and to prompt
/// otherwise. A harness reads a command in any case and with a bot's name after it (`/yolo@bot`),
/// so the match ignores case and ends at anything that cannot continue a command's name.
pub fn needed_for(text: &str, commands: &[ApprovalCommand]) -> Right {
    let text = text.to_ascii_lowercase();
    let found = commands.iter().any(|c| {
        let command = c.command.to_ascii_lowercase();
        match c.counts {
            Position::Start => is_word_at(text.trim_start(), 0, &command),
            Position::Anywhere => text
                .match_indices(command.as_str())
                .any(|(at, _)| is_word_at(&text, at, &command)),
        }
    });
    if found { Right::Approve } else { Right::Prompt }
}

/// Whether `command` stands at `at` in `text` as a command of its own: what follows cannot
/// continue its name (a letter, a digit, `_` or `-`), so `/approve`, `/approve.` and
/// `/approve@bot` count and `/approved` does not. Before it may be anything, since a command can
/// follow punctuation.
fn is_word_at(text: &str, at: usize, command: &str) -> bool {
    text[at..].starts_with(command)
        && text[at + command.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'))
}
