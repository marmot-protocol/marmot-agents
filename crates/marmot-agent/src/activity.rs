//! Which chats are sent the agent's full activity (D-136).
//!
//! The owner's two-person chat with the agent is sent everything the agent does. Every other chat
//! is sent what the people in it need to follow the agent and to answer it: its words, the turn's
//! start and end, its approvals and questions, its errors and its session. Its reasoning, tools,
//! plans, tasks, notices and a subagent's words go to such a chat only when the owner lists it in
//! the settings, or sets `full_activity = "all"`. The core decides this, not a driver (D-004).

use crate::keys::GroupKey;
use marmot_agent_events::AgentEvent;
use marmot_agent_events::body::Body;
use serde::Deserialize;

/// Which chats, besides the owner's two-person chat with the agent, are sent its full activity:
/// the setting `full_activity` (D-136). None by default.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Written")]
pub enum FullActivity {
    /// The groups listed, by MDK's group id in hex.
    Groups(Vec<GroupKey>),
    /// Every chat.
    All,
}

impl Default for FullActivity {
    fn default() -> Self {
        Self::Groups(Vec::new())
    }
}

/// The setting as written: the word `all`, or a list of group ids.
#[derive(Deserialize)]
#[serde(untagged)]
enum Written {
    Word(String),
    Groups(Vec<GroupKey>),
}

impl TryFrom<Written> for FullActivity {
    type Error = &'static str;

    fn try_from(written: Written) -> Result<Self, Self::Error> {
        match written {
            Written::Word(word) if word == "all" => Ok(Self::All),
            Written::Word(_) => Err("`all`, or a list of group ids"),
            Written::Groups(groups) => groups
                .into_iter()
                .map(|group| {
                    let id = group.as_str().trim();
                    let hex = !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit());
                    // As MDK gives it, so a group listed in either case is recognised.
                    hex.then(|| GroupKey(id.to_ascii_lowercase()))
                        .ok_or("a group id in hex")
                })
                .collect::<Result<_, _>>()
                .map(Self::Groups),
        }
    }
}

impl FullActivity {
    /// Whether the owner set `group` to be sent the agent's full activity.
    pub fn includes(&self, group: &GroupKey) -> bool {
        match self {
            Self::All => true,
            Self::Groups(groups) => groups.contains(group),
        }
    }
}

/// Whether every chat is sent `event`, whatever the owner set: what the people in a chat need to
/// follow the agent and to answer it (D-136).
///
/// - The agent's words are what it says to them, and the turn's start and end show it working.
/// - An approval or a question waits for someone in the chat to answer it, a subagent's too.
/// - An error says why the agent stopped or did not do what was asked.
/// - The session says what can be set and done, which the controls people send need (D-029).
///
/// Its reasoning, tools, plans and tasks are how it works, and a notice and a subagent's words
/// are about that work: those are its full activity.
pub fn for_every_chat(event: &AgentEvent) -> bool {
    match event.body() {
        Body::Turn(_)
        | Body::Approval(_)
        | Body::Question(_)
        | Body::Error(_)
        | Body::Session(_) => true,
        Body::Message(message) => message.parent.is_none(),
        Body::Reasoning(_) | Body::Tool(_) | Body::Plan(_) | Body::Task(_) | Body::Notice(_) => {
            false
        }
    }
}
