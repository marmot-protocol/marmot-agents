//! The local socket: a driver that is a separate program (D-003, D-033 to D-038).
//!
//! A connector, such as the Claude Code driver, connects to the core's Unix socket and speaks
//! JSON-RPC 2.0, one message per line. It opens with a hello carrying its token and what its
//! harness can do; then the core sends it `prompt`, `answer` and `control` requests, and it sends
//! the core `event` notifications and, when the agent sends a file, `send_file` requests (D-090). To the core it is a driver like any other: [`SocketDriver`]
//! implements the same trait as the Codex driver, and the messages carry the trait's shapes
//! (D-058). The exact fields are in `docs/design/socket-protocol.md`.
//!
//! The socket is in a folder only the owner can open, and the token is in a file only the owner
//! can read (D-038). One connection is one agent (D-035). No line over 4 MB is read or written
//! (D-033), and a harness's own message reaches the chat exactly as the connector sent it (D-007).

use marmot_agent_driver::{
    Action, Answer, ApprovalCommand, Capabilities, Control, Driver, Emitted, FilesIn, Given,
    Harness, MessageId, MidTurn, Output, Position, Prompt, Reason, Refusal, RequestId, SendFile,
    Sessions,
};
use marmot_agent_events::AgentEvent;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};

/// The protocol a connector must speak (D-020).
pub const PROTOCOL: &str = "marmot.agent-control.v3";
/// The largest message either side may send (D-033).
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;
/// How long a connector has to give its hello.
pub const HELLO_WITHIN: Duration = Duration::from_secs(10);
/// How long a connector has to take or refuse a request: it answers once it has handed the
/// request to its harness, not when the harness is done (D-058).
pub const ANSWER_WITHIN: Duration = Duration::from_secs(30);

/// Where a connector finds the core: the socket, and the token it must give.
pub struct Endpoint {
    /// The socket, in a folder only the owner can open.
    pub socket: PathBuf,
    /// The connector's token, in a file only the owner can read.
    pub token_file: PathBuf,
}

impl Endpoint {
    /// The socket and the token for an agent whose home is `home`.
    pub fn in_home(home: &Path) -> Self {
        Self {
            socket: home.join("socket").join("marmot-agent.sock"),
            token_file: home.join("connector.token"),
        }
    }

    /// Opens the socket, making its folder private, and gives the token, making it if there is
    /// none or the file is damaged.
    pub fn listen(&self) -> io::Result<(UnixListener, String)> {
        let folder = self.socket.parent().expect("the socket is in a folder");
        std::fs::create_dir_all(folder)?;
        private_folder(folder)?;
        if self.socket.exists() {
            std::fs::remove_file(&self.socket)?;
        }
        let listener = UnixListener::bind(&self.socket)?;
        Ok((listener, self.token()?))
    }

    /// The token: 32 random bytes as hex. A file that holds anything else, empty included, is
    /// replaced, so a crash while one was written can never leave a token that is easy to give.
    fn token(&self) -> io::Result<String> {
        if let Ok(text) = std::fs::read_to_string(&self.token_file) {
            let token = text.trim();
            if token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(token.to_string());
            }
        }
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(io::Error::other)?;
        let token = hex::encode(bytes);
        let temporary = self.token_file.with_extension("new");
        let _ = std::fs::remove_file(&temporary);
        write_private(&temporary, token.as_bytes())?;
        std::fs::rename(&temporary, &self.token_file)?;
        Ok(token)
    }
}

#[cfg(unix)]
fn private_folder(folder: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Lines read from the connection, none over [`MAX_MESSAGE`]: a longer one is passed over as it
/// arrives, without being held. What is read so far is kept here, so a read that is dropped half
/// way loses nothing.
struct Lines {
    inner: BufReader<OwnedReadHalf>,
    line: Vec<u8>,
    too_long: bool,
}

impl Lines {
    fn new(read: OwnedReadHalf) -> Self {
        Self {
            inner: BufReader::new(read),
            line: Vec::new(),
            too_long: false,
        }
    }

    /// The next line, without its newline. `None` once the connection is closed.
    async fn next(&mut self) -> Option<Vec<u8>> {
        loop {
            let available = self.inner.fill_buf().await.ok()?;
            if available.is_empty() {
                return None;
            }
            let (part, end) = match available.iter().position(|&b| b == b'\n') {
                Some(at) => (&available[..at], Some(at + 1)),
                None => (available, None),
            };
            if !self.too_long {
                if self.line.len() + part.len() > MAX_MESSAGE {
                    self.too_long = true;
                    self.line = Vec::new();
                } else {
                    self.line.extend_from_slice(part);
                }
            }
            let used = end.unwrap_or(available.len());
            self.inner.consume(used);
            if end.is_some() {
                let too_long = std::mem::replace(&mut self.too_long, false);
                let line = std::mem::take(&mut self.line);
                if !too_long {
                    return Some(line);
                }
            }
        }
    }
}

/// One message as it arrived. `params` and `result` keep the exact text the connector wrote, so
/// a harness's own message inside an event survives as it was (D-007).
#[derive(Deserialize)]
struct Envelope {
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: Option<String>,
    params: Option<Box<RawValue>>,
    result: Option<Box<RawValue>>,
    error: Option<Value>,
}

/// Why a connection was not taken. Names nothing the connector sent (D-071).
#[derive(Debug)]
pub enum HelloError {
    /// The connection closed, failed or was silent before a hello.
    Gone,
    /// The first message was not a hello the core understands.
    NotHello,
    /// The connector speaks another protocol version.
    Protocol,
    /// The token is not this agent's.
    Token,
}

impl std::fmt::Display for HelloError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Gone => "the connector went, or said nothing, before its hello",
            Self::NotHello => "the connector did not open with a hello",
            Self::Protocol => "the connector speaks another protocol version",
            Self::Token => "the connector's token is not this agent's",
        })
    }
}

/// A connector on the socket, as a driver.
pub struct SocketDriver {
    lines: Lines,
    writer: OwnedWriteHalf,
    harness: Harness,
    capabilities: Capabilities,
    next_id: i64,
    /// Events, and files to send, read while a request waited for its response, for the next call
    /// of `next_event`.
    held: VecDeque<Output>,
    /// The connector's `send_file` requests the core has not answered, by the id the core was
    /// given: the connector's own id, as it wrote it.
    files: HashMap<String, Value>,
    /// `send_file` requests that reused the id of one still unanswered, to answer `invalid`. They
    /// are answered at the next write that is awaited to its end, never while waiting for the
    /// next event, which may be dropped partway (D-058).
    duplicates: Vec<Value>,
    /// The number of the next event in each session, which the core gives (D-034).
    seq: HashMap<String, u64>,
    answer_within: Duration,
    gone: bool,
}

impl SocketDriver {
    /// Takes a connection's hello, within [`HELLO_WITHIN`]: the right protocol and this agent's
    /// token, or it is refused with an error the connector can read.
    pub async fn greet(stream: UnixStream, token: &str, agent: &str) -> Result<Self, HelloError> {
        let (read, mut writer) = stream.into_split();
        let mut lines = Lines::new(read);
        let line = tokio::time::timeout(HELLO_WITHIN, lines.next())
            .await
            .ok()
            .flatten()
            .ok_or(HelloError::Gone)?;
        let hello: Value = serde_json::from_slice(&line).map_err(|_| HelloError::NotHello)?;
        let id = hello.get("id").cloned().unwrap_or(Value::Null);
        let refuse = |code: i64, message: &str| json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
        if hello["method"] != "hello" {
            let _ = write_line(&mut writer, &refuse(-32600, "open with a hello")).await;
            return Err(HelloError::NotHello);
        }
        let params = &hello["params"];
        if params["protocol"] != PROTOCOL {
            let needed = format!("the core speaks {PROTOCOL}");
            let _ = write_line(&mut writer, &refuse(-32001, &needed)).await;
            return Err(HelloError::Protocol);
        }
        let given = params["token"].as_str();
        if !given.is_some_and(|given| !token.is_empty() && same(given, token)) {
            let _ = write_line(&mut writer, &refuse(-32002, "not this agent's token")).await;
            return Err(HelloError::Token);
        }
        let Some(capabilities) = capabilities(&params["capabilities"]) else {
            let refusal = refuse(-32602, "capabilities missing or invalid");
            let _ = write_line(&mut writer, &refusal).await;
            return Err(HelloError::NotHello);
        };
        let harness = Harness {
            name: params["harness"]["name"]
                .as_str()
                .unwrap_or("connector")
                .to_string(),
            version: params["harness"]["version"].as_str().map(String::from),
        };
        let welcome = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"core": {"version": env!("CARGO_PKG_VERSION")}, "agent": {"name": agent}},
        });
        write_line(&mut writer, &welcome)
            .await
            .map_err(|_| HelloError::Gone)?;
        Ok(Self {
            lines,
            writer,
            harness,
            capabilities,
            next_id: 0,
            held: VecDeque::new(),
            files: HashMap::new(),
            duplicates: Vec::new(),
            seq: HashMap::new(),
            answer_within: ANSWER_WITHIN,
            gone: false,
        })
    }

    /// Changes how long the connector has to take or refuse a request.
    pub fn answer_within(mut self, limit: Duration) -> Self {
        self.answer_within = limit;
        self
    }

    /// Sends a request and waits for its response, holding back the events read meanwhile
    /// (D-058). A connection that drops, or a connector that has not answered in time, refuses it
    /// as failed and is given up: a prompt is then not delivered, and never sent again (D-037).
    async fn request(&mut self, method: &str, params: Value) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        self.answer_duplicates().await;
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if write_line(&mut self.writer, &message).await.is_err() {
            self.gone = true;
            return Err(Refusal::new(Reason::Failed));
        }
        let deadline = tokio::time::Instant::now() + self.answer_within;
        loop {
            let Ok(next) = tokio::time::timeout_at(deadline, self.read()).await else {
                self.gone = true;
                return Err(Refusal::new(Reason::Failed));
            };
            let Some(envelope) = next else {
                return Err(Refusal::new(Reason::Failed));
            };
            let is_response = envelope.jsonrpc.as_deref() == Some("2.0")
                && envelope.method.is_none()
                && envelope.id == Some(Value::from(id));
            if is_response {
                match (&envelope.result, &envelope.error) {
                    (Some(_), None) => return Ok(()),
                    (None, Some(error)) => return Err(refusal(error)),
                    _ => {}
                }
            }
            if let Some(output) = self.output(&envelope) {
                self.held.push_back(output);
            }
        }
    }

    /// Answers each `send_file` that reused an unanswered id: `invalid`, and nothing was sent.
    async fn answer_duplicates(&mut self) {
        for id in std::mem::take(&mut self.duplicates) {
            if self.gone {
                return;
            }
            let message = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000,
                "message": "invalid", "data": {"reason": "invalid",
                "detail": "Another file with this request's id is still being sent."}}});
            if write_line(&mut self.writer, &message).await.is_err() {
                self.gone = true;
            }
        }
    }

    /// What a message from the connector is for the core: an event, a file to send, or nothing.
    fn output(&mut self, envelope: &Envelope) -> Option<Output> {
        if let Some(file) = self.send_file(envelope) {
            return Some(Output::SendFile(file));
        }
        self.event(envelope).map(Output::Event)
    }

    /// A `send_file` request (D-090): the chat and the file's path. One the core cannot read is
    /// still handed on, with what could be read, so the core answers it with a refusal: the
    /// connector waits for an answer.
    fn send_file(&mut self, envelope: &Envelope) -> Option<SendFile> {
        if envelope.method.as_deref() != Some("send_file") {
            return None;
        }
        let id = envelope.id.clone().filter(|id| !id.is_null())?;
        let params: Value = envelope
            .params
            .as_ref()
            .and_then(|raw| serde_json::from_str(raw.get()).ok())
            .unwrap_or(Value::Null);
        let key = id.to_string();
        if self.files.contains_key(&key) {
            // The same id again, while the first is unanswered: refused, and not sent.
            self.duplicates.push(id);
            return None;
        }
        self.files.insert(key.clone(), id);
        Some(SendFile {
            id: key,
            chat: params["chat"].as_str().unwrap_or_default().into(),
            path: params["path"].as_str().unwrap_or_default().into(),
        })
    }

    /// The next message, or `None` once the connection is gone. A line that is not a JSON
    /// message is passed over.
    async fn read(&mut self) -> Option<Envelope> {
        if self.gone {
            return None;
        }
        loop {
            let Some(line) = self.lines.next().await else {
                self.gone = true;
                return None;
            };
            if let Ok(envelope) = serde_json::from_slice(&line) {
                return Some(envelope);
            }
        }
    }

    /// An `event` notification as an event, with the harness and its place in its session filled
    /// in by the core. Its fields are kept as the text they arrived as, so the native message is
    /// exact. Anything else, or an event the core cannot read, is `None`.
    fn event(&mut self, envelope: &Envelope) -> Option<Emitted> {
        if envelope.method.as_deref() != Some("event") || envelope.id.is_some() {
            return None;
        }
        let mut fields: BTreeMap<String, Box<RawValue>> =
            serde_json::from_str(envelope.params.as_ref()?.get()).ok()?;
        let text = |raw: Option<Box<RawValue>>| -> Option<String> {
            serde_json::from_str(raw?.get()).ok()
        };
        let ids = |raw: Option<Box<RawValue>>| -> Vec<String> {
            raw.and_then(|raw| serde_json::from_str(raw.get()).ok())
                .unwrap_or_default()
        };
        let chat = text(fields.remove("chat"));
        let prompts = ids(fields.remove("prompts"));
        let settles = ids(fields.remove("settles"));
        let session = fields
            .get("session")
            .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
            .unwrap_or_default();
        let seq = self.seq.entry(session).or_insert(0);
        *seq += 1;
        fields.insert(
            "harness".into(),
            serde_json::value::to_raw_value(&self.harness.name).ok()?,
        );
        fields.insert("seq".into(), serde_json::value::to_raw_value(&*seq).ok()?);
        let whole = serde_json::to_string(&fields).ok()?;
        let event: AgentEvent = serde_json::from_str(&whole).ok()?;
        Some(Emitted {
            event,
            chat: chat.map(marmot_agent_driver::ChatId),
            prompts: prompts.into_iter().map(MessageId).collect(),
            settles: settles.into_iter().map(RequestId).collect(),
        })
    }
}

impl Driver for SocketDriver {
    fn harness(&self) -> Harness {
        self.harness.clone()
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let person = |p: &marmot_agent_driver::Person| json!({"id": p.id.0, "name": p.name});
        let mut params = json!({
            "message": prompt.message.0,
            "text": prompt.text,
            "chat": {"id": prompt.chat.id.0, "group": prompt.chat.group, "name": prompt.chat.name},
            "sender": person(&prompt.sender),
            "attachments": prompt.attachments.iter().map(|a| json!({
                "path": a.path.to_string_lossy(), "name": a.name, "media_type": a.media_type,
            })).collect::<Vec<_>>(),
        });
        if let Some(quoted) = &prompt.reply_to {
            params["reply_to"] = json!({
                "message": quoted.message.0, "text": quoted.text, "author": person(&quoted.author),
            });
        }
        if let Some(session) = &prompt.session {
            params["session"] = session.as_str().into();
        }
        self.request("prompt", params).await
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        let given = match &answer.given {
            Given::Choice(choice) => json!({"choice": choice}),
            Given::Questions(replies) => json!({"questions": replies.iter().map(|r| json!({
                "question": r.question, "options": r.options, "text": r.text,
            })).collect::<Vec<_>>()}),
        };
        let params = json!({
            "request": answer.request.0,
            "item": answer.item.as_str(),
            "by": {"id": answer.by.id.0, "name": answer.by.name},
            "given": given,
        });
        self.request("answer", params).await
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        let action = match &control.action {
            Action::Interrupt { turn } => json!({"type": "interrupt", "turn": turn.as_str()}),
            Action::NewSession { chat, session } => json!({
                "type": "new_session",
                "chat": {"id": chat.id.0, "group": chat.group, "name": chat.name},
                "session": session.as_ref().map(|s| s.as_str()),
            }),
            Action::Set {
                session,
                setting,
                value,
            } => json!({
                "type": "set", "session": session.as_str(), "setting": setting, "value": value,
            }),
            Action::Own { session, id } => {
                json!({"type": "own", "session": session.as_str(), "id": id})
            }
        };
        let params = json!({
            "request": control.request.0,
            "by": {"id": control.by.id.0, "name": control.by.name},
            "action": action,
        });
        self.request("control", params).await
    }

    async fn next_event(&mut self) -> Option<Output> {
        loop {
            if let Some(output) = self.held.pop_front() {
                return Some(output);
            }
            // Reading keeps what it has read in `self`, and nothing is written here, so this
            // future is safe to drop (D-058).
            let envelope = self.read().await?;
            if let Some(output) = self.output(&envelope) {
                return Some(output);
            }
        }
    }

    async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
        self.answer_duplicates().await;
        let Some(connector_id) = self.files.remove(&id) else {
            return;
        };
        if self.gone {
            return;
        }
        let message = match result {
            Ok(()) => json!({"jsonrpc": "2.0", "id": connector_id, "result": {}}),
            Err(refusal) => {
                let word = reason_word(refusal.reason);
                let mut data = json!({"reason": word});
                if let Some(detail) = refusal.detail {
                    data["detail"] = detail.into();
                }
                json!({"jsonrpc": "2.0", "id": connector_id,
                    "error": {"code": -32000, "message": word, "data": data}})
            }
        };
        if write_line(&mut self.writer, &message).await.is_err() {
            self.gone = true;
        }
    }
}

/// Writes one message as a line. One over the limit is not written (D-033).
async fn write_line(writer: &mut OwnedWriteHalf, message: &Value) -> io::Result<()> {
    let mut line = message.to_string();
    if line.len() > MAX_MESSAGE {
        return Err(io::Error::other("a message over the limit"));
    }
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}

/// Compares two tokens in time that does not depend on where they differ.
fn same(given: &str, token: &str) -> bool {
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |differ, (a, b)| differ | (a ^ b))
            == 0
}

/// A connector's refusal: its reason word, and its words for the chat.
fn refusal(error: &Value) -> Refusal {
    let data = &error["data"];
    let word = data["reason"]
        .as_str()
        .or(error["message"].as_str())
        .unwrap_or("failed");
    let reason = match word {
        "not_supported" => Reason::NotSupported,
        "busy" => Reason::Busy,
        "unknown_item" => Reason::UnknownItem,
        "not_waiting" => Reason::NotWaiting,
        "already_answered" => Reason::AlreadyAnswered,
        "nothing_to_do" => Reason::NothingToDo,
        "invalid" => Reason::Invalid,
        "not_allowed" => Reason::NotAllowed,
        _ => Reason::Failed,
    };
    Refusal {
        reason,
        detail: data["detail"].as_str().map(String::from),
    }
}

/// The word for a reason, as the socket writes it.
fn reason_word(reason: Reason) -> &'static str {
    match reason {
        Reason::NotSupported => "not_supported",
        Reason::Busy => "busy",
        Reason::UnknownItem => "unknown_item",
        Reason::NotWaiting => "not_waiting",
        Reason::AlreadyAnswered => "already_answered",
        Reason::NothingToDo => "nothing_to_do",
        Reason::Invalid => "invalid",
        Reason::Failed => "failed",
        Reason::NotAllowed => "not_allowed",
    }
}

/// The seven capability flags, every one, every time (D-058).
fn capabilities(value: &Value) -> Option<Capabilities> {
    let flag = |name: &str| value[name].as_bool();
    let mid_turn = match value["mid_turn"].as_str()? {
        "fold" => MidTurn::Fold,
        "restart" => MidTurn::Restart,
        "queue" => MidTurn::Queue,
        _ => return None,
    };
    let sessions = match value["sessions"].as_str()? {
        "core" => Sessions::Core,
        "own" => Sessions::Own,
        _ => return None,
    };
    let files_in = match value["files_in"].as_str()? {
        "none" => FilesIn::None,
        "images" => FilesIn::Images,
        "any" => FilesIn::Any,
        _ => return None,
    };
    let approval_commands = value["approval_commands"]
        .as_array()?
        .iter()
        .map(|c| {
            Some(ApprovalCommand {
                command: c["command"].as_str()?.to_string(),
                counts: match c["counts"].as_str()? {
                    "start" => Position::Start,
                    "anywhere" => Position::Anywhere,
                    _ => return None,
                },
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Capabilities {
        interrupt: flag("interrupt")?,
        mid_turn,
        sessions,
        resume: flag("resume")?,
        approvals: flag("approvals")?,
        files_in,
        approval_commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn hello(token: Value) -> String {
        let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {
            "protocol": PROTOCOL, "token": token, "harness": {"name": "script"},
            "capabilities": {"interrupt": false, "mid_turn": "queue", "sessions": "core",
                "resume": false, "approvals": false, "files_in": "none", "approval_commands": []},
        }});
        format!("{hello}\n")
    }

    async fn greeted(core: UnixStream) -> SocketDriver {
        SocketDriver::greet(core, TOKEN, "agent")
            .await
            .expect("a good hello")
    }

    // A hello with no token, or a token that is not a string, is refused, even against an
    // empty token on the core's side.
    #[tokio::test]
    async fn a_hello_without_a_string_token_is_refused() {
        for token in [Value::Null, json!(42), json!("")] {
            let (core, mut connector) = UnixStream::pair().unwrap();
            connector.write_all(hello(token).as_bytes()).await.unwrap();
            let refused = SocketDriver::greet(core, "", "agent").await;
            assert!(matches!(refused, Err(HelloError::Token)));
        }
    }

    // A line over the limit is passed over as it arrives, and the next one is read.
    #[tokio::test]
    async fn a_line_over_the_limit_is_passed_over() {
        let (core, mut connector) = UnixStream::pair().unwrap();
        let sending = tokio::spawn(async move {
            let long = vec![b'x'; MAX_MESSAGE + 1];
            connector.write_all(&long).await.unwrap();
            connector.write_all(b"\n").await.unwrap();
            connector
                .write_all(hello(json!(TOKEN)).as_bytes())
                .await
                .unwrap();
            connector
        });
        let driver = greeted(core).await;
        assert_eq!(driver.harness.name, "script");
        drop(sending.await.unwrap());
    }

    // A connector that does not take a request in time is given up, and the request refused as
    // failed, so a prompt is never sent twice (D-037).
    #[tokio::test]
    async fn a_request_not_taken_in_time_fails() {
        let (core, mut connector) = UnixStream::pair().unwrap();
        connector
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core)
            .await
            .answer_within(Duration::from_millis(100));
        let result = driver.request("prompt", json!({})).await;
        assert_eq!(result.unwrap_err().reason, Reason::Failed);
        assert!(driver.next_event().await.is_none(), "given up");
        let mut seen = vec![0u8; 1024];
        let _ = connector.read(&mut seen).await;
    }

    // A response with only an id, not a result or an error, is not taken as an acknowledgement.
    #[tokio::test]
    async fn an_incomplete_response_is_not_an_acknowledgement() {
        let (core, connector) = UnixStream::pair().unwrap();
        let (read, mut write) = connector.into_split();
        write
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core).await;
        let answering = tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            let _welcome = lines.next_line().await;
            let _request = lines.next_line().await;
            write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1}\n")
                .await
                .unwrap();
            drop(write);
        });
        let result = driver.request("prompt", json!({})).await;
        assert_eq!(result.unwrap_err().reason, Reason::Failed);
        answering.await.unwrap();
    }

    // A connector asks the core to send a file, and hears back how it went: sent, or refused
    // with the reason and the words for the agent (D-090).
    #[tokio::test]
    async fn a_connector_asks_to_send_a_file_and_hears_how_it_went() {
        let (core, connector) = UnixStream::pair().unwrap();
        let (read, mut write) = connector.into_split();
        write
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core).await;
        let mut lines = BufReader::new(read).lines();
        let _welcome = lines.next_line().await.unwrap();
        for id in [json!(7), json!("eight")] {
            let ask = json!({"jsonrpc": "2.0", "id": id, "method": "send_file",
                "params": {"chat": "c-1", "path": "/work/notes.txt"}});
            write
                .write_all(format!("{ask}\n").as_bytes())
                .await
                .unwrap();
        }
        let mut asked = Vec::new();
        for _ in 0..2 {
            match driver.next_event().await {
                Some(Output::SendFile(file)) => asked.push(file),
                _ => panic!("a file to send"),
            }
        }
        assert_eq!(asked[0].chat, "c-1".into());
        assert_eq!(asked[0].path, PathBuf::from("/work/notes.txt"));
        driver.file_sent(asked[0].id.clone(), Ok(())).await;
        let refusal = Refusal {
            reason: Reason::NotAllowed,
            detail: Some("The file is outside the folders.".into()),
        };
        driver.file_sent(asked[1].id.clone(), Err(refusal)).await;
        let sent: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(sent, json!({"jsonrpc": "2.0", "id": 7, "result": {}}));
        let refused: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(refused["id"], "eight");
        assert_eq!(refused["error"]["data"]["reason"], "not_allowed");
        assert_eq!(
            refused["error"]["data"]["detail"],
            "The file is outside the folders."
        );
    }

    // A second `send_file` with the id of one still unanswered is refused as invalid, not dropped:
    // the connector's tool would otherwise wait for an answer that never comes.
    #[tokio::test]
    async fn a_file_request_that_reuses_an_unanswered_id_is_refused() {
        let (core, connector) = UnixStream::pair().unwrap();
        let (read, mut write) = connector.into_split();
        write
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core).await;
        let mut lines = BufReader::new(read).lines();
        let _welcome = lines.next_line().await.unwrap();
        for path in ["/work/a.txt", "/work/b.txt"] {
            let ask = json!({"jsonrpc": "2.0", "id": 7, "method": "send_file",
                "params": {"chat": "c-1", "path": path}});
            write
                .write_all(format!("{ask}\n").as_bytes())
                .await
                .unwrap();
        }
        let Some(Output::SendFile(first)) = driver.next_event().await else {
            panic!("a file to send");
        };
        assert_eq!(first.path, PathBuf::from("/work/a.txt"));
        // The duplicate is read, held, and answered at the next write: here the first's answer.
        let more = tokio::time::timeout(Duration::from_millis(200), driver.next_event()).await;
        assert!(more.is_err(), "the duplicate is not handed to the core");
        driver.file_sent(first.id, Ok(())).await;
        let refused: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(refused["id"], 7);
        assert_eq!(refused["error"]["data"]["reason"], "invalid");
        let sent: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(sent, json!({"jsonrpc": "2.0", "id": 7, "result": {}}));
    }

    // The harness's own message reaches the core exactly as the connector wrote it (D-007).
    #[tokio::test]
    async fn a_native_message_arrives_exactly() {
        let (core, mut connector) = UnixStream::pair().unwrap();
        connector
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core).await;
        let payload = r#"{"z": 1.50,  "a": [2e3, 18446744073709551616]}"#;
        let event = format!(
            r#"{{"jsonrpc":"2.0","method":"event","params":{{"item":"i","type":"notice","state":"done","title":"t","text":"x","body":{{"kind":"warning"}},"native":{{"kind":"k","payload":{payload}}}}}}}"#
        );
        connector
            .write_all(format!("{event}\n").as_bytes())
            .await
            .unwrap();
        let emitted = driver
            .next_event()
            .await
            .and_then(Output::into_event)
            .expect("the event");
        assert_eq!(emitted.event.native().unwrap().payload.get(), payload);
        assert_eq!(emitted.event.header().harness, "script");
        assert_eq!(emitted.event.header().seq, 1);
    }
}
