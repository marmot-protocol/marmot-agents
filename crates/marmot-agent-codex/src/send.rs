//! What the driver sends Codex, built from the driver's own types.
//!
//! Each function returns one JSON-RPC message as the text to write. The requests that depend on a
//! session, such as a prompt or an answer, are built by the [`Translator`](crate::Translator),
//! which knows the session; the ones here need nothing but their arguments.

use marmot_agent_driver::{Prompt, Reason, Refusal};
use serde_json::{Value, json};

/// The sandbox a session starts in: what Codex's commands may touch without asking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sandbox {
    /// Read anything, write nothing.
    ReadOnly,
    /// Write inside the working folder and the temporary folders.
    #[default]
    WorkspaceWrite,
    /// No sandbox.
    DangerFullAccess,
}

impl Sandbox {
    fn word(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

/// When Codex asks before it acts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApprovalPolicy {
    /// Before anything but a few commands known to be safe.
    Untrusted,
    /// When a command fails in the sandbox.
    OnFailure,
    /// When the model decides to, and before anything outside the sandbox.
    #[default]
    OnRequest,
    /// Never.
    Never,
}

impl ApprovalPolicy {
    fn word(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnFailure => "on-failure",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

/// Where a session works and what it may do, from the core's configuration (D-061). The driver
/// sends all three whenever it starts or resumes a session: a session resumed without them came
/// back with a wider sandbox than it started with (rule 10 of the event model).
#[derive(Clone, PartialEq, Eq)]
pub struct Setup {
    /// The working folder, an absolute path.
    pub cwd: String,
    /// The sandbox. The default keeps writes inside the working folder and the temporary folders.
    pub sandbox: Sandbox,
    /// When Codex asks. The default asks before anything outside the sandbox.
    pub approval_policy: ApprovalPolicy,
}

/// Leaves out the working folder: a path is not logged (D-071).
impl std::fmt::Debug for Setup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("sandbox", &self.sandbox)
            .field("approval_policy", &self.approval_policy)
            .finish_non_exhaustive()
    }
}

impl Setup {
    /// A working folder with the safe defaults.
    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            sandbox: Sandbox::default(),
            approval_policy: ApprovalPolicy::default(),
        }
    }

    fn params(&self) -> serde_json::Map<String, Value> {
        let mut params = serde_json::Map::new();
        params.insert("cwd".into(), self.cwd.clone().into());
        params.insert("approvalPolicy".into(), self.approval_policy.word().into());
        params.insert("sandbox".into(), self.sandbox.word().into());
        params
    }
}

/// The first message on a connection.
pub fn initialize(id: i64) -> String {
    let client =
        json!({"name": "marmot-agent", "title": "Marmot", "version": env!("CARGO_PKG_VERSION")});
    // Codex's settings calls and collaboration modes are experimental.
    let params = json!({"clientInfo": client, "capabilities": {"experimentalApi": true}});
    request(id, "initialize", params)
}

/// The notice that follows Codex's reply to `initialize`.
pub fn initialized() -> String {
    json!({"method": "initialized"}).to_string()
}

/// The name of the driver's own tool, which sends a file to the chat (D-090).
pub const SEND_FILE: &str = "send_file";

/// The tool every session the driver starts is given, so the agent can send a file to the chat
/// (D-090). Codex keeps it with the session, so a resumed session has it too.
pub fn send_file_tool() -> Value {
    json!({
        "type": "function",
        "name": SEND_FILE,
        "description": "Send a file from the working folder to the chat, as an attachment the people in the chat can open. Give its path, relative to the working folder or absolute.",
        "inputSchema": {
            "type": "object",
            "properties": {"path": {"type": "string", "description": "The file to send."}},
            "required": ["path"],
            "additionalProperties": false,
        },
    })
}

/// The driver's reply to Codex's call of its own tool: whether the file was sent, and if not, why,
/// in words the model reads.
pub fn tool_reply(id: &Value, result: &Result<(), Refusal>) -> String {
    let (text, success) = match result {
        Ok(()) => ("Sent.".to_string(), true),
        Err(refusal) => match &refusal.detail {
            Some(detail) => (format!("Not sent: {detail}"), false),
            None => ("Not sent.".to_string(), false),
        },
    };
    let content = json!([{"type": "inputText", "text": text}]);
    json!({"id": id, "result": {"contentItems": content, "success": success}}).to_string()
}

/// A new session. Codex keeps it on disk, so it can be resumed after a restart (D-063).
pub(crate) fn thread_start(id: i64, setup: &Setup) -> String {
    let mut params = setup.params();
    params.insert("ephemeral".into(), false.into());
    params.insert("dynamicTools".into(), json!([send_file_tool()]));
    request(id, "thread/start", params.into())
}

/// A session Codex kept, taken up again with the setup sent afresh.
pub(crate) fn thread_resume(id: i64, thread: &str, setup: &Setup) -> String {
    let mut params = setup.params();
    params.insert("threadId".into(), thread.into());
    request(id, "thread/resume", params.into())
}

/// The models Codex offers. The reply fills the model setting's options.
pub fn model_list(id: i64) -> String {
    request(id, "model/list", json!({}))
}

/// The collaboration modes Codex offers. The reply fills the mode setting's options.
pub fn collaboration_mode_list(id: i64) -> String {
    request(id, "collaborationMode/list", json!({}))
}

pub(crate) fn request(id: i64, method: &str, params: Value) -> String {
    json!({"id": id, "method": method, "params": params}).to_string()
}

/// A prompt as Codex's input: its text, and each image as a file Codex reads (D-062).
pub(crate) fn input(prompt: &Prompt) -> Result<Value, Refusal> {
    let mut input = vec![json!({"type": "text", "text": text(prompt), "text_elements": []})];
    for attachment in &prompt.attachments {
        // Codex takes images and nothing else (`files_in: images`).
        if !attachment.media_type.starts_with("image/") {
            return Err(Refusal::new(Reason::NotSupported));
        }
        let Some(path) = attachment.path.to_str() else {
            return Err(Refusal::new(Reason::Invalid));
        };
        input.push(json!({"type": "localImage", "path": path}));
    }
    Ok(input.into())
}

/// What Codex reads for a message. Codex has one user per thread, so in a group the text names
/// who wrote it, and a reply quotes the message it answers (D-062). A slash command goes exactly
/// as typed (D-031).
fn text(prompt: &Prompt) -> String {
    if prompt.text.starts_with('/') {
        return prompt.text.clone();
    }
    let mut text = String::new();
    if let Some(quoted) = &prompt.reply_to {
        let said = format!("{}: {}", quoted.author.name, quoted.text);
        for line in said.lines() {
            text.push_str("> ");
            text.push_str(line);
            text.push('\n');
        }
        text.push('\n');
    }
    if prompt.chat.group {
        text.push_str(&prompt.sender.name);
        text.push_str(": ");
    }
    text.push_str(&prompt.text);
    text
}
