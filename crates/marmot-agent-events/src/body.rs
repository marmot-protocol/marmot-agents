//! The fields each item type carries.
//!
//! These follow the table "What each type carries" in `docs/design/event-model.md`. A field that
//! is absent is left out of the JSON. A field this module does not name is passed over when an
//! event is read: the list of fields is still a proposal, and an event must stay readable to a
//! core that is older than the driver that sent it.

use crate::event::ItemId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// An item type together with its own fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "body", rename_all = "snake_case")]
pub enum Body {
    /// A turn started or ended.
    Turn(Turn),
    /// Text the agent says to the people in the chat.
    Message(Message),
    /// The agent's thinking.
    Reasoning(Reasoning),
    /// A tool the agent used.
    Tool(Tool),
    /// The agent wants permission for one action.
    Approval(Approval),
    /// The agent asks the person something.
    Question(Question),
    /// The agent's plan or to-do list.
    Plan(Plan),
    /// Work that runs beside a turn or outlives it.
    Task(Task),
    /// Something a person should know that is not the agent's own words.
    Notice(Notice),
    /// Something failed outside any one item.
    Error(Error),
    /// A session with the harness.
    Session(Session),
}

// ---------------------------------------------------------------------------------------- turn

/// A turn: who or what started it, and once it has ended, how long it took and what it used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// What started the turn.
    pub started_by: StartedBy,
    /// How long the turn took, once it has ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// What the turn used, once it has ended and the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// What started a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartedBy {
    /// A person's message.
    User,
    /// The agent itself, after background work.
    Agent,
    /// A control, such as compacting the conversation.
    Control,
}

/// The tokens a turn used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens read by the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Tokens written by the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

// ------------------------------------------------------------------------------------- message

/// Text the agent says. The text itself is the event's `text`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Whether this is a remark made while working or the final answer, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    /// For a subagent's message: the item it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ItemId>,
}

/// Where in a turn a message falls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// A remark made while the agent is still working.
    Commentary,
    /// The agent's answer.
    Final,
}

// ----------------------------------------------------------------------------------- reasoning

/// The agent's thinking. It carries nothing beyond the event's `text`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reasoning {}

// ---------------------------------------------------------------------------------------- tool

/// A tool the agent used.
///
/// The fields a tool has depend on its kind, but not strictly: a command that only reads files
/// has the kind `read` and still has a `command`. So they are all optional.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// Which sort of tool it is.
    pub kind: ToolKind,
    /// The tool's own name, such as `shell`.
    pub name: String,
    /// A command: what was run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// A command: the folder it ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// A command: its exit code, once it has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// How long the tool took, once it has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// What the tool printed or returned, once it has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// An edit: the files it changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
    /// A search: what was searched for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// A file the tool read, such as an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Any other tool: what it was called with, as the harness gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// Why the tool failed, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The sort of tool. A harness that gives only a tool name leaves the driver to decide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// A shell command.
    Command,
    /// A change to files.
    Edit,
    /// Reading a file.
    Read,
    /// A search, of files or of the web.
    Search,
    /// Fetching a web address.
    Fetch,
    /// A tool on an MCP server.
    Mcp,
    /// Work handed to a subagent.
    Subagent,
    /// Anything else.
    Other,
}

/// One file changed by an edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// The file.
    pub path: String,
    /// The change, as a diff.
    pub diff: String,
    /// Whether the file was added, changed or deleted, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<ChangeKind>,
}

/// What an edit did to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// The file is new.
    Add,
    /// The file existed and was changed.
    Update,
    /// The file was removed.
    Delete,
}

// ------------------------------------------------------------------------------------ approval

/// The agent wants permission for one action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    /// The item that is waiting for permission, such as the command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<ItemId>,
    /// What a person may answer.
    pub choices: Vec<Choice>,
    /// The answer, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<ApprovalAnswer>,
}

/// One thing a person may answer to an approval.
///
/// The id is the driver's own. It is never the harness's value for the choice, which can be an
/// object. The driver keeps the harness's value and looks it up by this id when an answer comes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// The id an answer names.
    pub id: String,
    /// What a button shows: "Allow once".
    pub label: String,
    /// Whether choosing it lets the action happen.
    pub effect: Effect,
}

/// What a choice does. Nothing finer is in the model: "once" and "for this session" are the
/// harness's ideas, and the label carries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// The action may happen.
    Allow,
    /// The action may not happen.
    Deny,
}

/// How an approval was answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalAnswer {
    /// The id of the choice. Absent when the harness reports a decision the item did not offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
}

// ------------------------------------------------------------------------------------ question

/// The agent asks the person something. One item can hold several questions.
///
/// A question cannot be changed once it is built. Its questions have ids that differ, and its
/// answers are checked against them: an answer to a question marked `secret` names the question
/// and nothing else (D-041). That holds for a question built here and for one read from JSON.
///
/// The native message of the event is the other place a secret answer could be repeated.
/// [`AgentEvent::new`](crate::AgentEvent::new) covers that (D-056).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedQuestion")]
pub struct Question {
    answer_via: AnswerVia,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<ItemId>,
    questions: Vec<Asked>,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<Vec<Answered>>,
}

impl Question {
    /// A question that nobody has answered yet. It is refused if two of its questions share an
    /// id, since an answer could then not say which one it is for.
    pub fn new(
        answer_via: AnswerVia,
        subject: Option<ItemId>,
        questions: Vec<Asked>,
    ) -> Result<Self, BadQuestion> {
        for (place, asked) in questions.iter().enumerate() {
            if questions[..place]
                .iter()
                .any(|earlier| earlier.id == asked.id)
            {
                return Err(BadQuestion::SameId {
                    question: asked.id.clone(),
                });
            }
        }
        Ok(Self {
            answer_via,
            subject,
            questions,
            answer: None,
        })
    }

    /// How the harness expects the answer.
    pub fn answer_via(&self) -> AnswerVia {
        self.answer_via
    }

    /// The item the question belongs to, when it has one.
    pub fn subject(&self) -> Option<&ItemId> {
        self.subject.as_ref()
    }

    /// The questions asked.
    pub fn questions(&self) -> &[Asked] {
        &self.questions
    }

    /// The answers, once there are any: one entry for each question that was answered.
    pub fn answer(&self) -> Option<&[Answered]> {
        self.answer.as_deref()
    }

    /// The same question, answered. It is refused if an answer names a question that was not
    /// asked, or carries the answer to a secret question.
    pub fn answered(mut self, answers: Vec<Answered>) -> Result<Self, BadQuestion> {
        for answer in &answers {
            let Some(asked) = self.questions.iter().find(|q| q.id == answer.question) else {
                return Err(BadQuestion::NotAsked {
                    question: answer.question.clone(),
                });
            };
            if asked.secret && (answer.text.is_some() || !answer.options.is_empty()) {
                return Err(BadQuestion::SecretAnswer {
                    question: answer.question.clone(),
                });
            }
        }
        self.answer = Some(answers);
        Ok(self)
    }
}

/// Why a question item was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadQuestion {
    /// Two questions in one item have the same id.
    SameId {
        /// The id they share.
        question: String,
    },
    /// An answer names a question the item did not ask.
    NotAsked {
        /// The id the answer named.
        question: String,
    },
    /// An answer to a secret question carried the answer itself.
    SecretAnswer {
        /// The id of the secret question.
        question: String,
    },
}

impl fmt::Display for BadQuestion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SameId { question } => write!(f, "two questions have the id `{question}`"),
            Self::NotAsked { question } => write!(f, "the question `{question}` was not asked"),
            Self::SecretAnswer { question } => {
                write!(
                    f,
                    "the answer to the secret question `{question}` must not be repeated"
                )
            }
        }
    }
}

impl std::error::Error for BadQuestion {}

/// A question as it is read from JSON, before its answers are checked.
#[derive(Deserialize)]
struct UncheckedQuestion {
    answer_via: AnswerVia,
    #[serde(default)]
    subject: Option<ItemId>,
    questions: Vec<Asked>,
    #[serde(default)]
    answer: Option<Vec<Answered>>,
}

impl TryFrom<UncheckedQuestion> for Question {
    type Error = BadQuestion;

    fn try_from(read: UncheckedQuestion) -> Result<Self, BadQuestion> {
        let question = Self::new(read.answer_via, read.subject, read.questions)?;
        match read.answer {
            Some(answers) => question.answered(answers),
            None => Ok(question),
        }
    }
}

/// How a question is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerVia {
    /// By an answer that points at this item.
    Request,
    /// By the person's next chat message. The harness simply waits for it.
    Message,
}

/// One question in a question item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    /// The id an answer names.
    pub id: String,
    /// A short heading, when the harness gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// The question.
    pub text: String,
    /// The answers offered. Empty when the question takes only free text.
    #[serde(default)]
    pub options: Vec<Offered>,
    /// Whether several options may be chosen.
    #[serde(default)]
    pub multiple: bool,
    /// Whether the person may type an answer of their own.
    #[serde(default)]
    pub allow_text: bool,
    /// Whether the answer is a secret, such as a password. A secret answer is never shown to the
    /// chat, and is accepted only in a two-person chat (D-041).
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
}

/// One answer a question offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offered {
    /// The id an answer names.
    pub id: String,
    /// What a button shows.
    pub label: String,
    /// What choosing it means, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The answer to one question.
///
/// It names options by the ids the item offered. For a secret question it names the question and
/// nothing else: the answer itself is never repeated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answered {
    /// The id of the question.
    pub question: String,
    /// The ids of the options chosen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// What the person typed, when the question allows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------------------- plan

/// The agent's plan, as text or as a list of steps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The plan as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The plan as steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<PlanEntry>,
}

/// One step of a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEntry {
    /// The step.
    pub text: String,
    /// How far it has got.
    pub state: PlanEntryState,
}

/// How far a step of a plan has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryState {
    /// Not started.
    Pending,
    /// Being worked on.
    Running,
    /// Finished.
    Done,
}

// ---------------------------------------------------------------------------------------- task

/// Work that runs beside a turn or outlives it: a subagent, a background job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// The sort of task, in the harness's own word.
    pub kind: String,
    /// What the task is doing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The item that started the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<ItemId>,
}

// -------------------------------------------------------------------------------------- notice

/// Something a person should know that is not the agent's own words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    /// What the notice is about.
    pub kind: NoticeKind,
    /// For `waiting`: how long the agent will wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// For `model_changed`: why, when the harness says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What a notice is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// The conversation was compacted.
    Compaction,
    /// The agent is waiting.
    Waiting,
    /// The harness is trying again.
    Retry,
    /// A warning from the harness.
    Warning,
    /// The harness moved to another model.
    ModelChanged,
    /// The output of a command the harness ran itself.
    CommandOutput,
}

// --------------------------------------------------------------------------------------- error

/// Something failed outside any one item.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    /// What was being attempted, such as the request the harness refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// The harness's error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i64>,
    /// Whether the harness will try again by itself, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub will_retry: Option<bool>,
}

// ------------------------------------------------------------------------------------- session

/// A session with the harness: what an app can set and do in it (D-029, D-030).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The version of the harness (D-030). A driver that is not told it supplies it (D-044).
    pub harness_version: String,
    /// What can be set: the model, the mode, the permission mode and so on.
    #[serde(default)]
    pub settings: Vec<Setting>,
    /// The harness's own actions, such as compacting.
    #[serde(default)]
    pub actions: Vec<Action>,
    /// The harness's slash commands.
    #[serde(default)]
    pub commands: Vec<Command>,
}

/// One setting of a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    /// The id a `set` control names.
    pub id: String,
    /// What an app shows: "Model".
    pub label: String,
    /// The current value, when the harness has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The values it accepts. Absent when nobody has said what they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// Whether changing it needs the right to approve (D-043). A setting that decides what the
    /// agent may do without asking does.
    #[serde(default, skip_serializing_if = "is_false")]
    pub approve: bool,
}

/// An action of the harness's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// The id a control names.
    pub id: String,
    /// What an app shows.
    pub label: String,
    /// What it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One slash command. Its name and its hint are always present. A description is the first thing
/// left out when the item would be too large (D-045).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// The command, without its slash.
    pub name: String,
    /// What it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What to type after it. Empty when the command takes nothing.
    pub hint: String,
}

// The signature is serde's: `skip_serializing_if` passes a reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}
