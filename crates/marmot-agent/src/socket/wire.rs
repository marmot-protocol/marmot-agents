//! The socket's messages as types: what the core writes, and what it reads from a connector
//! (`docs/design/socket-protocol.md`, "The messages, as built").
//!
//! The socket writes every message through these types and reads every message through them, so
//! the JSON Schema made from them, `fixtures/socket/schema.json`, cannot fall behind what the core
//! does. The test in `corpus.rs` fails when the committed schema, or the committed examples beside
//! it, differ from what these types and the socket make.
//!
//! The values here are the driver trait's (D-058), written the way the socket writes them. The
//! trait's own types say nothing about the socket, so each is turned into its message here, field by
//! field: a field added to the trait does not compile until it has a place on the socket.

use marmot_agent_driver as driver;
use serde::{Deserialize, Serialize};

/// The version of JSON-RPC every message names (D-033).
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub enum JsonRpc {
    /// JSON-RPC 2.0.
    #[default]
    #[serde(rename = "2.0")]
    V2,
}

/// The id of a connector's request: a number or text, as JSON-RPC allows. The core's own requests
/// are numbered.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum Id {
    /// A number.
    Number(serde_json::Number),
    /// Text.
    Text(String),
}

// --------------------------------------------------------------------------- from the core

/// A request from the core (D-058).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct CoreRequestMessage {
    /// Always `2.0`.
    pub jsonrpc: JsonRpc,
    /// The core's id for the request, from 1 on each connection.
    pub id: i64,
    /// The method and its params.
    #[serde(flatten)]
    pub request: CoreRequest,
}

/// What the core asks a connector to do. Each is answered as soon as the connector has taken it or
/// refused it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum CoreRequest {
    /// A message for the agent.
    Prompt(PromptParams),
    /// A person's answer to an approval or a question.
    Answer(AnswerParams),
    /// Something a person does unprompted.
    Control(ControlParams),
}

/// A message for the agent. A slash command is one too: it is passed exactly as typed (D-031).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct PromptParams {
    /// The core's id for the message.
    pub message: String,
    /// The message's text.
    pub text: String,
    /// The chat it was sent in.
    pub chat: Chat,
    /// Who sent it.
    pub sender: Person,
    /// Files sent with it, each written to disk by the core.
    pub attachments: Vec<Attachment>,
    /// The message it replies to, when it is a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Quoted>,
    /// The session to resume, only when the core resumes one (D-065).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// A chat (D-036).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Chat {
    /// The core's id for the chat.
    pub id: String,
    /// Whether the chat has more than one person in it.
    pub group: bool,
    /// The chat's name; `null` when it has none.
    pub name: Option<String>,
    /// The folder the chat chose for its sessions, as a whole path with links resolved, only when
    /// it chose one (D-101).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// A person (D-036).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Person {
    /// The core's id for the person.
    pub id: String,
    /// The person's name.
    pub name: String,
}

/// A file sent with a prompt.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Attachment {
    /// Where the core wrote the file.
    pub path: String,
    /// The file's name.
    pub name: String,
    /// The file's media type, such as `image/png`.
    pub media_type: String,
}

/// The message a prompt replies to.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Quoted {
    /// The core's id for the message replied to.
    pub message: String,
    /// Its text.
    pub text: String,
    /// Who wrote it.
    pub author: Person,
}

/// A person's answer to an approval or a question. It goes to the harness's own answer call
/// (D-028).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct AnswerParams {
    /// The core's id for this answer, which the event that shows its effect names in `settles`.
    pub request: String,
    /// The item being answered.
    pub item: String,
    /// Who answered.
    pub by: Person,
    /// What they answered.
    pub given: Given,
}

/// What an answer holds, by the ids the item offered.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Given {
    /// To an approval: the id of one choice.
    Choice(String),
    /// To a question item: one entry for each question answered.
    Questions(Vec<Reply>),
}

/// The answer to one question, as it is delivered to the harness. For a secret question it holds
/// the secret (D-041).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Reply {
    /// The id of the question.
    pub question: String,
    /// The ids of the options chosen.
    pub options: Vec<String>,
    /// What the person typed; `null` when they typed nothing.
    pub text: Option<String>,
}

/// Something a person does unprompted (D-029).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ControlParams {
    /// The core's id for this control, which the event that shows its effect names in `settles`.
    pub request: String,
    /// Who sent it.
    pub by: Person,
    /// What to do, and what to do it to.
    pub action: ControlAction,
}

/// What a control does.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlAction {
    /// Stop the running turn.
    Interrupt {
        /// The turn to stop.
        turn: String,
    },
    /// End a chat's session, if it has one, and start a fresh one. A chat that moved folder gets one
    /// whose `chat` names the new folder (D-101).
    NewSession {
        /// The chat the new session is for.
        chat: Chat,
        /// The session to end; `null` for a chat that has had none.
        session: Option<String>,
    },
    /// Change one setting of a session.
    Set {
        /// The session.
        session: String,
        /// The id of the setting, from the session item.
        setting: String,
        /// The new value.
        value: String,
    },
    /// One of the harness's own actions, such as compacting.
    Own {
        /// The session.
        session: String,
        /// The id of the action, from the session item.
        id: String,
    },
}

/// The core's answer to a hello it took.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct WelcomeMessage {
    /// Always `2.0`.
    pub jsonrpc: JsonRpc,
    /// The hello's id.
    pub id: Option<Id>,
    /// The core and the agent.
    pub result: Welcome,
}

/// What the core says of itself and the agent, in answer to a hello.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Welcome {
    /// The core.
    pub core: CoreVersion,
    /// The agent the connection drives (D-035).
    pub agent: AgentName,
}

/// The core's release.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct CoreVersion {
    /// Its version.
    pub version: String,
}

/// The agent's display name.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct AgentName {
    /// The name.
    pub name: String,
}

// ---------------------------------------------------------------------- from a connector

/// A request from a connector.
#[cfg(test)]
#[derive(schemars::JsonSchema)]
pub struct ConnectorRequestMessage {
    /// Always `2.0`.
    pub jsonrpc: JsonRpc,
    /// The connector's own id for the request. The hello's is its first.
    pub id: Id,
    /// The method and its params.
    #[serde(flatten)]
    pub request: ConnectorRequest,
}

/// What a connector asks of the core.
#[cfg(test)]
#[derive(schemars::JsonSchema)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum ConnectorRequest {
    /// The connection's first message.
    Hello(HelloParams),
    /// The agent sends a file to the chat (D-090; a proposal).
    SendFile(SendFileParams),
}

/// A connector's hello. One connection is one agent, and the token says which (D-035, D-038).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct HelloParams {
    /// `marmot.agent-control.v3` (D-020).
    pub protocol: String,
    /// The connector: its name and release version.
    pub connector: Connector,
    /// The harness: its name and, when it has said, its version.
    pub harness: Harness,
    /// The connector's token (D-038).
    pub token: String,
    /// What the harness can do: the seven flags, every one (D-058), and `subagents` when its
    /// harness tells them apart (D-142).
    pub capabilities: Capabilities,
    /// The folder the harness works in when a chat chose none, as a whole path, for a harness a
    /// chat may move to another folder (D-101; provisional). A gateway's plugin leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// A connector's name and release version.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Connector {
    /// The name.
    pub name: String,
    /// The release version.
    pub version: String,
}

/// A harness's name and version.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Harness {
    /// The name, such as `claude-code`. Nothing above a driver acts on it.
    pub name: String,
    /// The version, when the harness has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl From<Harness> for driver::Harness {
    fn from(Harness { name, version }: Harness) -> Self {
        Self { name, version }
    }
}

/// What a harness can do (D-058).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Capabilities {
    /// Whether a running turn can be stopped.
    pub interrupt: bool,
    /// What the harness does with a prompt that arrives during a turn (D-040).
    pub mid_turn: MidTurn,
    /// Who picks the session a prompt goes to.
    pub sessions: Sessions,
    /// Whether a session survives a restart of the harness.
    pub resume: bool,
    /// Whether the harness asks for approval through the chat.
    pub approvals: bool,
    /// Which attachments a prompt may carry.
    pub files_in: FilesIn,
    /// Typed commands that need the right to approve, even when there are none (D-032, D-047).
    pub approval_commands: Vec<ApprovalCommand>,
    /// Whether the harness tells its subagents apart, so each event of one names its `task` item
    /// and is shown in its thread (D-124, D-142). False when left out: a connector written
    /// before it shows subagents in the agent's own activity.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subagents: bool,
}

impl From<Capabilities> for driver::Capabilities {
    fn from(c: Capabilities) -> Self {
        let Capabilities {
            interrupt,
            mid_turn,
            sessions,
            resume,
            approvals,
            files_in,
            approval_commands,
            subagents,
        } = c;
        Self {
            interrupt,
            subagents,
            mid_turn: match mid_turn {
                MidTurn::Fold => driver::MidTurn::Fold,
                MidTurn::Restart => driver::MidTurn::Restart,
                MidTurn::Queue => driver::MidTurn::Queue,
            },
            sessions: match sessions {
                Sessions::Core => driver::Sessions::Core,
                Sessions::Own => driver::Sessions::Own,
            },
            resume,
            approvals,
            files_in: match files_in {
                FilesIn::None => driver::FilesIn::None,
                FilesIn::Images => driver::FilesIn::Images,
                FilesIn::Any => driver::FilesIn::Any,
            },
            approval_commands: approval_commands
                .into_iter()
                .map(
                    |ApprovalCommand { command, counts }| driver::ApprovalCommand {
                        command,
                        counts: match counts {
                            Position::Start => driver::Position::Start,
                            Position::Anywhere => driver::Position::Anywhere,
                        },
                    },
                )
                .collect(),
        }
    }
}

/// What a harness does with a prompt that arrives while a turn is running.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MidTurn {
    /// Folds it into the running turn.
    Fold,
    /// Ends the turn and starts another.
    Restart,
    /// Holds it until the turn ends.
    Queue,
}

/// Who picks the session a prompt goes to.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Sessions {
    /// The core keeps each chat's session (D-065).
    Core,
    /// The harness routes prompts by its own rules, as a gateway does.
    Own,
}

/// Which attachments a prompt may carry.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FilesIn {
    /// None.
    None,
    /// Images only.
    Images,
    /// Any file.
    Any,
}

/// A typed command that needs the right to approve.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ApprovalCommand {
    /// The command as typed, such as `/approve`.
    pub command: String,
    /// Where in a message it counts.
    pub counts: Position,
}

/// Where in a message a typed command counts.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Position {
    /// Only at the start of the message.
    Start,
    /// Anywhere in the message.
    Anywhere,
}

/// A file the agent asks the core to send to a chat (D-090; a proposal).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct SendFileParams {
    /// The core's id for the chat.
    pub chat: String,
    /// The file, as an absolute path.
    pub path: String,
}

/// A notification from a connector. It gets no response.
#[cfg(test)]
#[derive(schemars::JsonSchema)]
pub struct ConnectorNotificationMessage {
    /// Always `2.0`.
    pub jsonrpc: JsonRpc,
    /// The method and its params.
    #[serde(flatten)]
    pub notification: ConnectorNotification,
}

/// What a connector tells the core.
#[cfg(test)]
#[derive(schemars::JsonSchema)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum ConnectorNotification {
    /// An agent event.
    Event(EventParams),
}

/// An agent event as `marmot-agent-events` writes it, without `harness` and `seq`, which the core
/// fills in, and with what the event follows from: its chat, the prompts a turn responds to, and the
/// requests it settles.
#[cfg(test)]
#[derive(schemars::JsonSchema)]
pub struct EventParams {
    /// The event.
    #[serde(flatten)]
    pub event: ConnectorEvent,
    /// What the event follows from.
    #[serde(flatten)]
    pub extras: EventExtras,
}

/// An agent event's schema, less the fields the core fills in: the events crate's own schema, so
/// it cannot fall behind the event either.
#[cfg(test)]
pub struct ConnectorEvent;

#[cfg(test)]
impl schemars::JsonSchema for ConnectorEvent {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ConnectorEvent".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = marmot_agent_events::AgentEvent::json_schema(generator);
        if let Some(properties) = schema
            .get_mut("properties")
            .and_then(serde_json::Value::as_object_mut)
        {
            for field in CORE_FILLS {
                properties.remove(field);
            }
        }
        if let Some(required) = schema
            .get_mut("required")
            .and_then(serde_json::Value::as_array_mut)
        {
            required.retain(|field| !CORE_FILLS.iter().any(|filled| field == filled));
        }
        schema
    }
}

/// The fields of an agent event the core fills in, so a connector leaves them out.
pub const CORE_FILLS: [&str; 2] = ["harness", "seq"];

/// What an `event` carries beyond the agent event (D-058).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct EventExtras {
    /// The core's id for the chat the event belongs to, whenever the connector knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<String>,
    /// On a turn: the ids of the messages it responds to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
    /// The ids of the answers and controls whose effect the event shows (D-028).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settles: Vec<String>,
}

// ----------------------------------------------------------------------------- both ways

/// A request taken: the connector's answer to a request of the core's, and the core's answer to a
/// file sent.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct TakenMessage {
    /// Always `2.0`.
    pub jsonrpc: JsonRpc,
    /// The request's id.
    pub id: Option<Id>,
    /// Nothing more.
    pub result: Taken,
}

/// The empty result of a request taken.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Taken {}

/// A request refused.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct ErrorMessage {
    /// Always `2.0`.
    pub jsonrpc: JsonRpc,
    /// The request's id; `null` when the request had none.
    pub id: Option<Id>,
    /// Why.
    pub error: RpcError,
}

/// A JSON-RPC error.
///
/// A refusal has the code `-32000`, the reason as its message, and `data`. A hello the core refuses
/// has no `data`: `-32001` for another protocol version, `-32002` for a token that is not this
/// agent's, `-32600` or `-32602` for a hello it cannot read.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct RpcError {
    /// The code.
    pub code: i64,
    /// The reason's word, or for a hello, what is wrong with it.
    pub message: String,
    /// A refusal's reason and words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Refused>,
}

/// Why a request was refused, and in words for the chat or the agent.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
pub struct Refused {
    /// Why.
    pub reason: Reason,
    /// More than the reason says. Every member of the chat reads it (D-009, D-041); the core's
    /// words on a file name no path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The reasons a request may be refused. Only the core refuses with `not_allowed` (D-090).
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug))]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The harness cannot do this.
    NotSupported,
    /// The harness cannot do this now.
    Busy,
    /// The driver does not know the item.
    UnknownItem,
    /// The item is not waiting for an answer.
    NotWaiting,
    /// The item has been answered.
    AlreadyAnswered,
    /// There is nothing to act on.
    NothingToDo,
    /// The request does not make sense.
    Invalid,
    /// The harness was asked and it failed.
    Failed,
    /// The core does not allow it: a file outside the folders the settings allow.
    NotAllowed,
}

impl Reason {
    /// The word for the reason, as the socket writes it.
    pub fn word(self) -> &'static str {
        match self {
            Self::NotSupported => "not_supported",
            Self::Busy => "busy",
            Self::UnknownItem => "unknown_item",
            Self::NotWaiting => "not_waiting",
            Self::AlreadyAnswered => "already_answered",
            Self::NothingToDo => "nothing_to_do",
            Self::Invalid => "invalid",
            Self::Failed => "failed",
            Self::NotAllowed => "not_allowed",
        }
    }
}

impl From<driver::Reason> for Reason {
    fn from(reason: driver::Reason) -> Self {
        match reason {
            driver::Reason::NotSupported => Self::NotSupported,
            driver::Reason::Busy => Self::Busy,
            driver::Reason::UnknownItem => Self::UnknownItem,
            driver::Reason::NotWaiting => Self::NotWaiting,
            driver::Reason::AlreadyAnswered => Self::AlreadyAnswered,
            driver::Reason::NothingToDo => Self::NothingToDo,
            driver::Reason::Invalid => Self::Invalid,
            driver::Reason::Failed => Self::Failed,
            driver::Reason::NotAllowed => Self::NotAllowed,
        }
    }
}

impl From<Reason> for driver::Reason {
    fn from(reason: Reason) -> Self {
        match reason {
            Reason::NotSupported => Self::NotSupported,
            Reason::Busy => Self::Busy,
            Reason::UnknownItem => Self::UnknownItem,
            Reason::NotWaiting => Self::NotWaiting,
            Reason::AlreadyAnswered => Self::AlreadyAnswered,
            Reason::NothingToDo => Self::NothingToDo,
            Reason::Invalid => Self::Invalid,
            Reason::Failed => Self::Failed,
            Reason::NotAllowed => Self::NotAllowed,
        }
    }
}

// ------------------------------------------------------------------- from the trait's values

impl From<&driver::Person> for Person {
    fn from(driver::Person { id, name }: &driver::Person) -> Self {
        Self {
            id: id.0.clone(),
            name: name.clone(),
        }
    }
}

impl From<&driver::Chat> for Chat {
    fn from(
        driver::Chat {
            id,
            group,
            name,
            folder,
        }: &driver::Chat,
    ) -> Self {
        Self {
            id: id.0.clone(),
            group: *group,
            name: name.clone(),
            folder: folder.as_ref().map(|f| f.to_string_lossy().into_owned()),
        }
    }
}

impl From<&driver::Prompt> for PromptParams {
    fn from(prompt: &driver::Prompt) -> Self {
        let driver::Prompt {
            message,
            text,
            chat,
            sender,
            reply_to,
            attachments,
            session,
        } = prompt;
        Self {
            message: message.0.clone(),
            text: text.clone(),
            chat: chat.into(),
            sender: sender.into(),
            attachments: attachments
                .iter()
                .map(
                    |driver::Attachment {
                         path,
                         name,
                         media_type,
                     }| Attachment {
                        path: path.to_string_lossy().into_owned(),
                        name: name.clone(),
                        media_type: media_type.clone(),
                    },
                )
                .collect(),
            reply_to: reply_to.as_ref().map(
                |driver::Quoted {
                     message,
                     text,
                     author,
                 }| Quoted {
                    message: message.0.clone(),
                    text: text.clone(),
                    author: author.into(),
                },
            ),
            session: session.as_ref().map(|s| s.as_str().to_string()),
        }
    }
}

impl From<&driver::Answer> for AnswerParams {
    fn from(answer: &driver::Answer) -> Self {
        let driver::Answer {
            request,
            item,
            by,
            given,
        } = answer;
        Self {
            request: request.0.clone(),
            item: item.as_str().to_string(),
            by: by.into(),
            given: match given {
                driver::Given::Choice(choice) => Given::Choice(choice.clone()),
                driver::Given::Questions(replies) => Given::Questions(
                    replies
                        .iter()
                        .map(
                            |driver::Reply {
                                 question,
                                 options,
                                 text,
                             }| Reply {
                                question: question.clone(),
                                options: options.clone(),
                                text: text.clone(),
                            },
                        )
                        .collect(),
                ),
            },
        }
    }
}

impl From<&driver::Control> for ControlParams {
    fn from(control: &driver::Control) -> Self {
        let driver::Control {
            request,
            by,
            action,
        } = control;
        Self {
            request: request.0.clone(),
            by: by.into(),
            action: match action {
                driver::Action::Interrupt { turn } => ControlAction::Interrupt {
                    turn: turn.as_str().to_string(),
                },
                driver::Action::NewSession { chat, session } => ControlAction::NewSession {
                    chat: chat.into(),
                    session: session.as_ref().map(|s| s.as_str().to_string()),
                },
                driver::Action::Set {
                    session,
                    setting,
                    value,
                } => ControlAction::Set {
                    session: session.as_str().to_string(),
                    setting: setting.clone(),
                    value: value.clone(),
                },
                driver::Action::Own { session, id } => ControlAction::Own {
                    session: session.as_str().to_string(),
                    id: id.clone(),
                },
            },
        }
    }
}

// ------------------------------------------------------------------------------ the schema

/// Every message the core sends a connector. Only its schema is made, so its size does not matter.
#[cfg(test)]
#[allow(clippy::large_enum_variant)]
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
pub enum FromCore {
    /// A request: `prompt`, `answer` or `control`.
    Request(CoreRequestMessage),
    /// The answer to a hello the core took.
    Welcome(WelcomeMessage),
    /// A file sent.
    Taken(TakenMessage),
    /// A hello or a file refused.
    Refused(ErrorMessage),
}

/// Every message a connector sends the core.
#[cfg(test)]
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
pub enum FromConnector {
    /// A request: `hello` or `send_file`.
    Request(ConnectorRequestMessage),
    /// An `event`.
    Notification(ConnectorNotificationMessage),
    /// A request of the core's taken.
    Taken(TakenMessage),
    /// A request of the core's refused.
    Refused(ErrorMessage),
}
