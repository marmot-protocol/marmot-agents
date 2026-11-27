//! The parts of Codex's messages that the translation reads (D-055).
//!
//! Each type holds only the fields the driver uses. Everything else stays in the native message,
//! which is kept as the exact text that arrived. Every field is optional, because a newer Codex
//! may leave one out, and a message that lacks a field still has to be translated as far as it
//! can be.
//!
//! The reference is Codex's own schema, from `codex app-server generate-json-schema
//! --experimental`, and the recordings in `fixtures/recordings/codex`.

use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;
use std::fmt;

/// A JSON-RPC id. Codex uses numbers; the protocol allows text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum Id {
    Number(i64),
    Text(String),
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Text(t) => f.write_str(t),
        }
    }
}

/// Any message, before its method says what it holds. Codex leaves out `"jsonrpc"`.
#[derive(Deserialize)]
pub(crate) struct Envelope<'a> {
    #[serde(default)]
    pub id: Option<Id>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(borrow, default)]
    pub params: Option<&'a RawValue>,
    #[serde(borrow, default)]
    pub result: Option<&'a RawValue>,
    #[serde(borrow, default)]
    pub error: Option<&'a RawValue>,
}

// ------------------------------------------------------------------------------ notifications

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TokenUsageUpdated {
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub token_usage: Option<TokenUsage>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct TokenUsage {
    /// The whole thread so far.
    pub total: Option<Tokens>,
    /// The last model call.
    pub last: Option<Tokens>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Tokens {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ThreadStarted {
    pub thread: Option<Settings>,
}

/// `serverRequest/resolved`: Codex no longer waits for the client's reply to one of its requests,
/// because the client replied or because Codex settled it itself, as on an interrupt.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct RequestResolved {
    pub thread_id: Option<String>,
    pub request_id: Option<Id>,
}

/// `thread/name/updated`: a thread was named, or its name cleared, which is null.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ThreadNameUpdated {
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ThreadSettingsUpdated {
    pub thread_id: Option<String>,
    pub thread_settings: Option<Settings>,
}

/// A thread's settings. Codex states them in three places, under different names: the reply to
/// `thread/start` or `thread/resume`, `thread/started`, and `thread/settings/updated`.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Settings {
    pub id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub reasoning_effort: Option<String>,
    pub approval_policy: Option<Policy>,
    pub sandbox_policy: Option<Sandbox>,
    pub sandbox: Option<Sandbox>,
    pub collaboration_mode: Option<Mode>,
    pub cli_version: Option<String>,
    /// In a thread itself, its name, which is null when it has none (D-177). Absent elsewhere.
    #[serde(deserialize_with = "present")]
    pub name: Option<Option<String>>,
    /// In a reply, the thread itself, which says the version.
    pub thread: Option<Box<Settings>>,
}

/// A field that is there, even as null, as against one that is not.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// An approval policy: a word, or an object whose one key names its kind, such as `granular`.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum Policy {
    Word(String),
    Object(serde_json::Map<String, Value>),
}

/// A sandbox, named by a word or by an object with a `type`.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum Sandbox {
    Word(String),
    Object {
        #[serde(default, rename = "type")]
        kind: Option<String>,
    },
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct Mode {
    pub mode: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct TurnNote {
    pub turn: Option<TurnInfo>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TurnInfo {
    pub id: Option<String>,
    /// The turn the person started: this one's own id, or, for a subagent's turn, at any depth,
    /// the top-level turn's.
    pub root_turn_id: Option<String>,
    pub status: Option<String>,
    pub error: Option<Message>,
    pub duration_ms: Option<u64>,
}

/// Anything with a `message`: a turn's error, a warning.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct Message {
    pub message: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ItemNote<'a> {
    #[serde(borrow)]
    pub item: Option<Item<'a>>,
    /// The turn the item is in, which can have ended: what a parent's thread says of a subagent
    /// can come after the parent's turn (recorded).
    pub turn_id: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Item<'a> {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub id: Option<String>,
    pub text: Option<String>,
    pub phase: Option<String>,
    pub questions: Option<Vec<MessageQuestion>>,
    pub summary: Option<Vec<Value>>,
    pub content: Option<Vec<Value>>,
    pub status: Option<String>,
    pub command_actions: Option<Vec<CommandAction>>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<u64>,
    pub aggregated_output: Option<String>,
    pub changes: Option<Vec<FileChange>>,
    pub server: Option<String>,
    pub tool: Option<String>,
    pub arguments: Option<Value>,
    /// An MCP call's result, kept as the text it arrived as.
    #[serde(borrow)]
    pub result: Option<&'a RawValue>,
    pub error: Option<Value>,
    pub query: Option<String>,
    pub path: Option<String>,
    /// A client's own tool: the namespace it was given in, what it said back, and whether it
    /// worked.
    pub namespace: Option<String>,
    pub content_items: Option<Vec<ContentItem>>,
    pub success: Option<bool>,
    /// A subagent: what happened to it (`subAgentActivity`), the thread it runs in and its path;
    /// or the threads a tool for subagents names (`collabAgentToolCall`), what it was asked, and
    /// how each subagent stood.
    #[serde(rename = "kind")]
    pub activity: Option<String>,
    pub agent_thread_id: Option<String>,
    pub agent_path: Option<String>,
    pub receiver_thread_ids: Option<Vec<String>>,
    pub prompt: Option<String>,
    pub agents_states: Option<std::collections::BTreeMap<String, AgentState>>,
}

/// How a subagent stood, as a tool for subagents last saw it.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct AgentState {
    pub status: Option<String>,
    pub message: Option<String>,
}

/// One part of what a client's own tool said back to Codex.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct ContentItem {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub text: Option<String>,
}

/// A question that arrives as a field of an ordinary message.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct MessageQuestion {
    pub title: Option<String>,
    pub options: Option<Vec<String>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct CommandAction {
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct FileChange {
    pub path: Option<String>,
    pub kind: Option<ChangeKind>,
    pub diff: Option<String>,
}

/// What happened to a file, named by a word or by an object with a `type`.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum ChangeKind {
    Word(String),
    Object {
        #[serde(default, rename = "type")]
        kind: Option<String>,
    },
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct PlanUpdated {
    pub turn_id: Option<String>,
    pub explanation: Option<String>,
    pub plan: Option<Vec<Step>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct Step {
    pub step: Option<String>,
    pub status: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Rerouted {
    pub from_model: Option<String>,
    pub to_model: Option<String>,
    pub reason: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ErrorNote {
    pub error: Option<Message>,
    pub will_retry: Option<bool>,
}

// ----------------------------------------------------------------------------- server requests

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ApprovalRequest {
    pub turn_id: Option<String>,
    /// The decisions Codex lists. Each is a word or an object with one key.
    pub available_decisions: Option<Vec<Value>>,
    pub item_id: Option<String>,
    pub command: Option<String>,
    pub reason: Option<String>,
    /// For a permission request: what Codex asks for. A grant gives it back.
    pub permissions: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Elicitation {
    pub turn_id: Option<String>,
    pub server_name: Option<String>,
    #[serde(rename = "_meta")]
    pub meta: Option<ElicitationMeta>,
    pub message: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct ElicitationMeta {
    pub codex_approval_kind: Option<String>,
    pub tool_name: Option<String>,
    /// For an MCP tool approval: how long an approval may last beyond this call, `"session"` or
    /// `"always"`, alone or in a list.
    pub persist: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct UserInput {
    pub turn_id: Option<String>,
    pub item_id: Option<String>,
    pub questions: Option<Vec<UserQuestion>>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct UserQuestion {
    pub id: Option<String>,
    pub header: Option<String>,
    pub question: Option<String>,
    pub is_other: Option<bool>,
    pub is_secret: Option<bool>,
    pub options: Option<Vec<UserOption>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct UserOption {
    pub label: Option<String>,
    pub description: Option<String>,
}

// -------------------------------------------------------------------------------------- replies

/// The client's reply to an approval request: the decision under `decision`, or for an MCP
/// server's approval under `action`, with how long an approval lasts under `_meta.persist`. A reply
/// to a request for permissions has neither: it grants `permissions`, for a `scope` of the turn or
/// the session, and an empty grant denies.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct ApprovalReply {
    pub decision: Option<Value>,
    pub action: Option<Value>,
    #[serde(rename = "_meta")]
    pub meta: Option<Value>,
    pub permissions: Option<serde_json::Map<String, Value>>,
    pub scope: Option<String>,
}

/// The client's reply to one of Codex's questions: labels, under `answers`.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct QuestionReply {
    pub answers: Option<Ordered<Given>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct Given {
    pub answers: Option<Vec<Value>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct ListReply {
    pub data: Option<Vec<Listed>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct Listed {
    pub id: Option<String>,
    pub mode: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct ErrorReply {
    pub message: Option<String>,
    pub code: Option<i64>,
}

/// A JSON object's entries in the order they arrived. Codex's forms list their fields in the order
/// they are to be asked.
pub(crate) struct Ordered<T>(pub Vec<(String, T)>);

impl<T> Default for Ordered<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Ordered<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Entries<T> {
            type Value = Ordered<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some((key, value)) = map.next_entry()? {
                    entries.push((key, value));
                }
                Ok(Ordered(entries))
            }
        }
        deserializer.deserialize_map(Entries(std::marker::PhantomData))
    }
}
