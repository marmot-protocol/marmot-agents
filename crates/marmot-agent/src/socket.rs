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
//! (D-033), no more than [`HELD_MAX`] is held for a connection (D-145), and a harness's own
//! message reaches the chat exactly as the connector sent it (D-007).
//!
//! Every message is written and read through the types in [`wire`], and the JSON Schema made from
//! them is `fixtures/socket/schema.json`, with examples of each message beside it.

#[cfg(test)]
mod corpus;
pub mod lines;
pub mod wire;

use marmot_agent_driver::{
    Answer, Capabilities, ChatId, Control, Driver, Emitted, Harness, MessageId, Output, Prompt,
    Reason, Refusal, RequestId, SendFile,
};
use marmot_agent_events::AgentEvent;
use marmot_agent_events::body::Body;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
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
/// The most the core holds for one connection, in the bytes of the lines it read, while it waits
/// for the answer to a request, or to answer a connector's repeated `send_file` (D-145): eight
/// messages of the largest size. A connector that sends more is given up.
pub const HELD_MAX: usize = 8 * MAX_MESSAGE;
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
        // The token before the socket: a connector that finds the socket must find the token
        // beside it, and a stale socket is gone before the token says the core is starting.
        let token = self.token()?;
        let listener = UnixListener::bind(&self.socket)?;
        Ok((listener, token))
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
/// arrives, without being held ([`lines::Splitter`]). What is read so far is kept here, so a read
/// that is dropped half way loses nothing.
struct Lines {
    inner: BufReader<OwnedReadHalf>,
    splitter: lines::Splitter,
}

impl Lines {
    fn new(read: OwnedReadHalf) -> Self {
        Self {
            inner: BufReader::new(read),
            splitter: lines::Splitter::new(MAX_MESSAGE),
        }
    }

    /// The next line, without its newline. `None` once the connection is closed.
    async fn next(&mut self) -> Option<Vec<u8>> {
        loop {
            let available = self.inner.fill_buf().await.ok()?;
            if available.is_empty() {
                return None;
            }
            let (used, line) = self.splitter.take(available);
            self.inner.consume(used);
            if line.is_some() {
                return line;
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

/// What an event shows of a request: a prompt named by a turn of its own chat, or an answer or a
/// control settled.
enum Shows {
    Prompt(MessageId, ChatId),
    Settles(RequestId),
}

/// A connector on the socket, as a driver.
pub struct SocketDriver {
    lines: Lines,
    writer: OwnedWriteHalf,
    harness: Harness,
    capabilities: Capabilities,
    /// The folder the harness works in when a chat chose none, when the connector states one in
    /// its hello (D-101).
    folder: Option<PathBuf>,
    next_id: i64,
    /// Events, and files to send, read while a request waited for its response, for the next call
    /// of `next_event`, each with the length of the line it came in.
    held: VecDeque<(Output, usize)>,
    /// The bytes of the lines behind what is held, and behind the duplicates to answer: at most
    /// [`HELD_MAX`], or the connector is given up (D-145).
    holding: usize,
    /// The connector's `send_file` requests the core has not answered, by the id the core was
    /// given: the connector's own id, as it wrote it.
    files: HashMap<String, Value>,
    /// `send_file` requests that reused the id of one still unanswered, to answer `invalid`. They
    /// are answered at the next write that is awaited to its end, never while waiting for the
    /// next event, which may be dropped partway (D-058).
    duplicates: Vec<(Value, usize)>,
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
        // The hello and the core's answer to it, in the same time.
        let deadline = tokio::time::Instant::now() + HELLO_WITHIN;
        let line = tokio::time::timeout_at(deadline, lines.next())
            .await
            .ok()
            .flatten()
            .ok_or(HelloError::Gone)?;
        let hello: Value = serde_json::from_slice(&line).map_err(|_| HelloError::NotHello)?;
        let id = hello
            .get("id")
            .and_then(|id| wire::Id::deserialize(id).ok());
        let refuse = |code: i64, message: &str| wire::ErrorMessage {
            jsonrpc: wire::JsonRpc::V2,
            id: id.clone(),
            error: wire::RpcError {
                code,
                message: message.into(),
                data: None,
            },
        };
        if hello["method"] != "hello" {
            let _ = write_line(&mut writer, &refuse(-32600, "open with a hello"), deadline).await;
            return Err(HelloError::NotHello);
        }
        let params = &hello["params"];
        if params["protocol"] != PROTOCOL {
            let needed = format!("the core speaks {PROTOCOL}");
            let _ = write_line(&mut writer, &refuse(-32001, &needed), deadline).await;
            return Err(HelloError::Protocol);
        }
        let given = params["token"].as_str();
        if !given.is_some_and(|given| !token.is_empty() && same(given, token)) {
            let _ = write_line(
                &mut writer,
                &refuse(-32002, "not this agent's token"),
                deadline,
            )
            .await;
            return Err(HelloError::Token);
        }
        // Read field by field, as before the schema: the schema checks what this repository's
        // connectors send, and the core stays as lenient as it was with what it can do without.
        let Ok(capabilities) = wire::Capabilities::deserialize(&params["capabilities"]) else {
            let refusal = refuse(-32602, "capabilities missing or invalid");
            let _ = write_line(&mut writer, &refusal, deadline).await;
            return Err(HelloError::NotHello);
        };
        let capabilities = Capabilities::from(capabilities);
        // Optional: a connector whose harness keeps a workspace of its own states none (D-101).
        let folder = match params.get("folder") {
            None | Some(Value::Null) => None,
            Some(Value::String(folder)) if Path::new(folder).is_absolute() => {
                Some(PathBuf::from(folder))
            }
            Some(_) => {
                let refusal = refuse(-32602, "folder is not a whole path");
                let _ = write_line(&mut writer, &refusal, deadline).await;
                return Err(HelloError::NotHello);
            }
        };
        let harness = Harness {
            name: params["harness"]["name"]
                .as_str()
                .unwrap_or("connector")
                .to_string(),
            version: params["harness"]["version"].as_str().map(String::from),
        };
        let welcome = wire::WelcomeMessage {
            jsonrpc: wire::JsonRpc::V2,
            id,
            result: wire::Welcome {
                core: wire::CoreVersion {
                    version: env!("CARGO_PKG_VERSION").into(),
                },
                agent: wire::AgentName { name: agent.into() },
            },
        };
        write_line(&mut writer, &welcome, deadline)
            .await
            .map_err(|_| HelloError::Gone)?;
        Ok(Self {
            lines,
            writer,
            harness,
            capabilities,
            folder,
            next_id: 0,
            held: VecDeque::new(),
            holding: 0,
            files: HashMap::new(),
            duplicates: Vec::new(),
            seq: HashMap::new(),
            answer_within: ANSWER_WITHIN,
            gone: false,
        })
    }

    /// Numbers each session's events after `last`, the number each reached before this
    /// connection: a session goes on across connections, and an app orders a second's items by
    /// their numbers, so a later event must never take an earlier one's (D-059).
    pub fn numbered_after(mut self, last: HashMap<String, u64>) -> Self {
        self.seq = last;
        self
    }

    /// Changes how long the connector has to take or refuse a request.
    pub fn answer_within(mut self, limit: Duration) -> Self {
        self.answer_within = limit;
        self
    }

    /// Sends a request and waits for its response, holding back the events read meanwhile
    /// (D-058). A connection that drops, or a connector that has not taken the request in time,
    /// refuses it as failed and is given up: a prompt is then perhaps not delivered, and never
    /// sent again (D-037, D-129). The time is for writing the request and answering it both, so a
    /// connector that stops reading cannot hold the core either. A request whose effect an event
    /// read meanwhile showed was taken all the same: the event settles an answer or a control, or
    /// names a prompt as its turn's (D-147).
    async fn request(&mut self, request: wire::CoreRequest) -> Result<(), Refusal> {
        let shows = match &request {
            wire::CoreRequest::Prompt(p) => {
                Shows::Prompt(MessageId(p.message.clone()), ChatId(p.chat.id.clone()))
            }
            wire::CoreRequest::Answer(a) => Shows::Settles(RequestId(a.request.clone())),
            wire::CoreRequest::Control(c) => Shows::Settles(RequestId(c.request.clone())),
        };
        let mut shown = false;
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        let deadline = tokio::time::Instant::now() + self.answer_within;
        self.answer_duplicates(deadline).await;
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        self.next_id += 1;
        let id = self.next_id;
        let message = wire::CoreRequestMessage {
            jsonrpc: wire::JsonRpc::V2,
            id,
            request,
        };
        // Once any of it is written, the connector may have the request, and the answer is what
        // is lost.
        let unanswered = Refusal {
            unanswered: true,
            ..Refusal::new(Reason::Failed)
        };
        match write_line(&mut self.writer, &message, deadline).await {
            Ok(()) => {}
            Err(Unwritten::Nothing) => {
                self.gone = true;
                return Err(Refusal::new(Reason::Failed));
            }
            Err(Unwritten::Part) => {
                self.gone = true;
                return Err(unanswered);
            }
        }
        loop {
            let Ok(next) = tokio::time::timeout_at(deadline, self.read()).await else {
                self.gone = true;
                return if shown { Ok(()) } else { Err(unanswered) };
            };
            let Some((envelope, size)) = next else {
                return if shown { Ok(()) } else { Err(unanswered) };
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
            if let Some(output) = self.output(&envelope, size) {
                // A line past the budget is not kept, so what it would have shown is not.
                if !self.hold(size) {
                    if let Output::SendFile(file) = &output {
                        self.files.remove(&file.id);
                    }
                    return if shown { Ok(()) } else { Err(unanswered) };
                }
                if let Output::Event(e) = &output {
                    shown |= match &shows {
                        Shows::Prompt(m, chat) => {
                            matches!(e.event.body(), Body::Turn(_))
                                && e.chat.as_ref() == Some(chat)
                                && e.prompts.contains(m)
                        }
                        Shows::Settles(r) => e.settles.contains(r),
                    };
                }
                self.held.push_back((output, size));
            }
            // A repeated `send_file` whose refusal would not fit gave the connector up.
            if self.gone {
                return if shown { Ok(()) } else { Err(unanswered) };
            }
        }
    }

    /// Counts a line of `size` bytes against what the core holds for this connection, if it fits
    /// in [`HELD_MAX`]. If it does not, it is not held: the connector is given up, as one that
    /// does not answer is, and nothing more is read from it (D-145).
    fn hold(&mut self, size: usize) -> bool {
        if self.holding + size <= HELD_MAX {
            self.holding += size;
            return true;
        }
        if !self.gone {
            eprintln!(
                "marmot-agent: the connector sent more than the core holds for it, so it was given up"
            );
        }
        self.gone = true;
        false
    }

    /// Answers each `send_file` that reused an unanswered id: `invalid`, and nothing was sent.
    /// Each is written by `deadline`, or the connector is given up.
    async fn answer_duplicates(&mut self, deadline: tokio::time::Instant) {
        for (id, size) in std::mem::take(&mut self.duplicates) {
            self.holding -= size;
            if self.gone {
                continue;
            }
            let message = refused(
                wire::Id::deserialize(&id).ok(),
                Refusal {
                    reason: Reason::Invalid,
                    detail: Some("Another file with this request's id is still being sent.".into()),
                    unanswered: false,
                },
            );
            if write_line(&mut self.writer, &message, deadline)
                .await
                .is_err()
            {
                self.gone = true;
            }
        }
    }

    /// What a message from the connector is for the core: an event, a file to send, or nothing.
    fn output(&mut self, envelope: &Envelope, size: usize) -> Option<Output> {
        if let Some(file) = self.send_file(envelope, size) {
            return Some(Output::SendFile(file));
        }
        self.event(envelope).map(Output::Event)
    }

    /// A `send_file` request (D-090): the chat and the file's path. One the core cannot read is
    /// still handed on, with what could be read, so the core answers it with a refusal: the
    /// connector waits for an answer.
    fn send_file(&mut self, envelope: &Envelope, size: usize) -> Option<SendFile> {
        if envelope.method.as_deref() != Some("send_file") {
            return None;
        }
        let id = envelope.id.clone().filter(|id| !id.is_null())?;
        // Each field on its own, so a chat that can be read still gets the refusal's notice when
        // the path cannot.
        let params: Value = envelope
            .params
            .as_ref()
            .and_then(|raw| serde_json::from_str(raw.get()).ok())
            .unwrap_or(Value::Null);
        let key = id.to_string();
        if self.files.contains_key(&key) {
            // The same id again, while the first is unanswered: refused, and not sent. Its
            // refusal waits for the next write, so it is held.
            if self.hold(size) {
                self.duplicates.push((id, size));
            }
            return None;
        }
        self.files.insert(key.clone(), id);
        Some(SendFile {
            id: key,
            chat: params["chat"].as_str().unwrap_or_default().into(),
            path: params["path"].as_str().unwrap_or_default().into(),
        })
    }

    /// The next message, with the length of its line, or `None` once the connection is gone. A
    /// line that is not a JSON message is passed over.
    async fn read(&mut self) -> Option<(Envelope, usize)> {
        if self.gone {
            return None;
        }
        loop {
            let Some(line) = self.lines.next().await else {
                self.gone = true;
                return None;
            };
            if let Ok(envelope) = serde_json::from_slice(&line) {
                return Some((envelope, line.len()));
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
        // What the event follows from, each field on its own and as lenient as before the schema:
        // one the core cannot read is left out, and the event is still read.
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
        for filled in wire::CORE_FILLS {
            fields.remove(filled);
        }
        let session = fields
            .get("session")
            .and_then(|raw| serde_json::from_str::<String>(raw.get()).ok())
            .unwrap_or_default();
        // The next number in the session, taken only once the event is read, so one the core
        // passes over leaves no gap.
        let seq = self.seq.get(&session).copied().unwrap_or(0) + 1;
        fields.insert(
            "harness".into(),
            serde_json::value::to_raw_value(&self.harness.name).ok()?,
        );
        fields.insert("seq".into(), serde_json::value::to_raw_value(&seq).ok()?);
        let whole = serde_json::to_string(&fields).ok()?;
        let event: AgentEvent = serde_json::from_str(&whole).ok()?;
        self.seq.insert(session, seq);
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

    fn folder(&self) -> Option<PathBuf> {
        self.folder.clone()
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let params = wire::PromptParams::from(&prompt);
        self.request(wire::CoreRequest::Prompt(params)).await
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        let params = wire::AnswerParams::from(&answer);
        self.request(wire::CoreRequest::Answer(params)).await
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        let params = wire::ControlParams::from(&control);
        self.request(wire::CoreRequest::Control(params)).await
    }

    async fn next_event(&mut self) -> Option<Output> {
        loop {
            if let Some((output, size)) = self.held.pop_front() {
                self.holding -= size;
                return Some(output);
            }
            // Reading keeps what it has read in `self`, and nothing is written here, so this
            // future is safe to drop (D-058). Duplicates to answer pile up here until the next
            // write, so they count against the budget too, and one that does not fit ends the
            // reading.
            let (envelope, size) = self.read().await?;
            if let Some(output) = self.output(&envelope, size) {
                return Some(output);
            }
        }
    }

    async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
        // Written in the time a request has, or the connector is given up.
        let deadline = tokio::time::Instant::now() + self.answer_within;
        self.answer_duplicates(deadline).await;
        let Some(connector_id) = self.files.remove(&id) else {
            return;
        };
        if self.gone {
            return;
        }
        let id = wire::Id::deserialize(&connector_id).ok();
        let written = match result {
            Ok(()) => {
                write_line(
                    &mut self.writer,
                    &wire::TakenMessage {
                        jsonrpc: wire::JsonRpc::V2,
                        id,
                        result: wire::Taken {},
                    },
                    deadline,
                )
                .await
            }
            Err(refusal) => write_line(&mut self.writer, &refused(id, refusal), deadline).await,
        };
        if written.is_err() {
            self.gone = true;
        }
    }
}

/// Why a message was not written whole.
#[derive(Debug)]
enum Unwritten {
    /// None of it: the connector cannot have it.
    Nothing,
    /// Part of it, perhaps all but the line's end: the connector may read it as whole.
    Part,
}

/// Writes one message as a line, by `deadline`. One over the limit is not written (D-033). A
/// connector that stops reading fills the socket and leaves the write waiting: it is not waited
/// for past `deadline`, and the connection is not used again.
async fn write_line(
    writer: &mut OwnedWriteHalf,
    message: &impl Serialize,
    deadline: tokio::time::Instant,
) -> Result<(), Unwritten> {
    let Ok(mut line) = serde_json::to_string(message) else {
        return Err(Unwritten::Nothing);
    };
    if line.len() > MAX_MESSAGE {
        return Err(Unwritten::Nothing);
    }
    line.push('\n');
    let bytes = line.as_bytes();
    let mut written = 0;
    let whole = tokio::time::timeout_at(deadline, async {
        while written < bytes.len() {
            match writer.write(&bytes[written..]).await {
                Ok(0) | Err(_) => return false,
                Ok(wrote) => written += wrote,
            }
        }
        true
    })
    .await;
    match whole {
        Ok(true) => Ok(()),
        _ if written == 0 => Err(Unwritten::Nothing),
        _ => Err(Unwritten::Part),
    }
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

/// A connector's refusal: its reason, and its words for the chat. An error the core cannot read as
/// a refusal is one that failed.
fn refusal(error: &Value) -> Refusal {
    let data = &error["data"];
    // A connector that gives its reason only as the error's message is still understood.
    let reason = wire::Reason::deserialize(&data["reason"])
        .or_else(|_| wire::Reason::deserialize(&error["message"]))
        .map_or(Reason::Failed, Reason::from);
    Refusal {
        reason,
        detail: data["detail"].as_str().map(String::from),
        unanswered: false,
    }
}

/// The core's refusal of a connector's request: the reason, as the error's message too, and its
/// words for the agent.
fn refused(id: Option<wire::Id>, refusal: Refusal) -> wire::ErrorMessage {
    let reason = wire::Reason::from(refusal.reason);
    wire::ErrorMessage {
        jsonrpc: wire::JsonRpc::V2,
        id,
        error: wire::RpcError {
            code: -32000,
            message: reason.word().into(),
            data: Some(wire::Refused {
                reason,
                detail: refusal.detail,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncReadExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn hello(token: Value) -> String {
        let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {
            "protocol": PROTOCOL, "token": token, "harness": {"name": "script"},
            "connector": {"name": "script", "version": "0"},
            "capabilities": {"interrupt": false, "mid_turn": "queue", "sessions": "core",
                "resume": false, "approvals": false, "files_in": "none", "approval_commands": []},
        }});
        format!("{hello}\n")
    }

    fn a_prompt() -> wire::CoreRequest {
        let person = wire::Person {
            id: "u-1".into(),
            name: "A".into(),
        };
        wire::CoreRequest::Prompt(wire::PromptParams {
            message: "m-1".into(),
            text: "hi".into(),
            chat: wire::Chat {
                id: "c-1".into(),
                group: false,
                name: None,
                folder: None,
            },
            sender: person,
            attachments: Vec::new(),
            reply_to: None,
            session: None,
        })
    }

    async fn greeted(core: UnixStream) -> SocketDriver {
        SocketDriver::greet(core, TOKEN, "agent")
            .await
            .expect("a good hello")
    }

    // A connector that sees the socket the moment it appears finds the token beside it.
    #[tokio::test]
    async fn the_token_is_there_once_the_socket_is() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::in_home(home.path());
        let (socket, token_file) = (endpoint.socket.clone(), endpoint.token_file.clone());
        let watcher = std::thread::spawn(move || {
            loop {
                if socket.exists() {
                    return std::fs::read_to_string(&token_file);
                }
                std::hint::spin_loop();
            }
        });
        let (_listener, token) = endpoint.listen().unwrap();
        let read = watcher
            .join()
            .unwrap()
            .expect("the token, once the socket is there");
        assert_eq!(read.trim(), token);
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
        let result = driver.request(a_prompt()).await.unwrap_err();
        assert_eq!(result.reason, Reason::Failed);
        assert!(result.unanswered, "the connector may have it");
        assert!(driver.next_event().await.is_none(), "given up");
        // A request made after the connector was given up never reached it.
        assert!(!driver.request(a_prompt()).await.unwrap_err().unanswered);
        let mut seen = vec![0u8; 1024];
        let _ = connector.read(&mut seen).await;
    }

    // A connector that stops reading cannot hold the core: a request bigger than the socket can
    // hold is given up within the request's time, as one not taken is, and the connector with it.
    // Part of it may have reached the connector, so a prompt is told as perhaps not passed.
    #[tokio::test]
    async fn a_connector_that_stops_reading_is_given_up() {
        let (core, mut connector) = UnixStream::pair().unwrap();
        connector
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core)
            .await
            .answer_within(Duration::from_millis(100));
        let mut big = a_prompt();
        if let wire::CoreRequest::Prompt(params) = &mut big {
            params.text = "x".repeat(MAX_MESSAGE / 2);
        }
        let result = tokio::time::timeout(Duration::from_secs(5), driver.request(big))
            .await
            .expect("the write is given up in time")
            .unwrap_err();
        assert_eq!(result.reason, Reason::Failed);
        assert!(
            result.unanswered,
            "part of it may have reached the connector"
        );
        assert!(driver.next_event().await.is_none(), "given up");
        drop(connector);
    }

    // The request's time covers its write and its answer together: a connector that reads the
    // request only late in that time, and answers after it ran out, is given up, though the
    // answer came well within the time from when it read the request (D-138).
    #[tokio::test]
    async fn a_late_read_leaves_the_answer_only_what_is_left_of_the_time() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (core, mut connector) = UnixStream::pair().unwrap();
        connector
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let within = Duration::from_millis(1000);
        let mut driver = greeted(core).await.answer_within(within);
        let mut big = a_prompt();
        if let wire::CoreRequest::Prompt(params) = &mut big {
            params.text = "x".repeat(MAX_MESSAGE / 2);
        }
        let peer = tokio::spawn(async move {
            // Most of the time goes before the connector reads at all.
            tokio::time::sleep(within * 7 / 10).await;
            let mut reader = BufReader::new(connector);
            let (mut welcome, mut line) = (String::new(), String::new());
            reader.read_line(&mut welcome).await.unwrap();
            reader.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            // Answered past the request's time, but well within the time from the read.
            tokio::time::sleep(within * 6 / 10).await;
            let taken = json!({"jsonrpc": "2.0", "id": request["id"], "result": {}});
            let _ = reader
                .get_mut()
                .write_all(format!("{taken}\n").as_bytes())
                .await;
            reader
        });
        let result = driver.request(big).await.unwrap_err();
        assert_eq!(result.reason, Reason::Failed);
        assert!(result.unanswered, "written whole, so the connector has it");
        assert!(driver.next_event().await.is_none(), "given up");
        drop(peer.await.unwrap());
    }

    // The connection drops after the connector sent the event that shows a request's effect, and
    // before its response: the request was taken, and the event settles it, once. Refused as
    // failed, it would be settled twice: by the refusal, and by the event.
    #[tokio::test]
    async fn a_request_an_event_settled_before_the_connection_dropped_was_taken() {
        use marmot_agent_driver::{Answer, Given, Person};
        let (core, connector) = UnixStream::pair().unwrap();
        let (read, mut write) = connector.into_split();
        write
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core).await;
        let peer = tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            let _welcome = lines.next_line().await;
            let _answer = lines.next_line().await;
            let shown = json!({"jsonrpc": "2.0", "method": "event", "params": {
                "item": "ap-2", "type": "approval", "state": "done", "title": "Allowed", "text": "",
                "body": {"choices": [{"id": "c0", "label": "Allow", "effect": "allow"}],
                    "answer": {"choice": "c0"}},
                "session": "s-1", "turn": "t-1", "chat": "c-1", "settles": ["r-0"]}});
            write
                .write_all(format!("{shown}\n").as_bytes())
                .await
                .unwrap();
            // Gone before the response.
        });
        let answer = Answer {
            request: "r-0".into(),
            item: "ap-2".into(),
            by: Person {
                id: "u-1".into(),
                name: "Alice".into(),
            },
            given: Given::Choice("c0".into()),
        };
        assert_eq!(
            driver.answer(answer).await,
            Ok(()),
            "taken: its effect was shown"
        );
        peer.await.unwrap();
        let Some(Output::Event(shown)) = driver.next_event().await else {
            panic!("the event that settles it is handed on");
        };
        assert_eq!(shown.settles, vec![RequestId::from("r-0")]);
        assert!(
            driver.next_event().await.is_none(),
            "and then the connection is gone"
        );
    }

    // The same for a prompt: a turn that names it came before the connection dropped, so it was
    // delivered, and is not told as perhaps not delivered (D-129).
    #[tokio::test]
    async fn a_prompt_a_turn_named_before_the_connection_dropped_was_delivered() {
        let (core, connector) = UnixStream::pair().unwrap();
        let (read, mut write) = connector.into_split();
        write
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core).await;
        let peer = tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            let _welcome = lines.next_line().await;
            let _prompt = lines.next_line().await;
            let turn = json!({"jsonrpc": "2.0", "method": "event", "params": {
                "item": "t-1", "type": "turn", "state": "running", "title": "Working", "text": "",
                "body": {"started_by": "user"}, "session": "s-1", "turn": "t-1", "chat": "c-1",
                "prompts": ["m-1"]}});
            write
                .write_all(format!("{turn}\n").as_bytes())
                .await
                .unwrap();
        });
        assert_eq!(
            driver.request(a_prompt()).await,
            Ok(()),
            "delivered: a turn responds to it"
        );
        peer.await.unwrap();
        assert!(matches!(driver.next_event().await, Some(Output::Event(_))));
        assert!(driver.next_event().await.is_none());
    }

    // Only a turn of the prompt's own chat responds to it: a message that names it, or another
    // chat's turn that does, is no sign it was delivered, and the dropped connection leaves it
    // perhaps not delivered (D-129).
    #[tokio::test]
    async fn a_prompt_named_only_by_another_event_before_the_connection_dropped_is_not_delivered() {
        let message = json!({"item": "m-a", "type": "message", "state": "done", "title": "Message",
            "text": "hi", "body": {"phase": "final"}, "session": "s-1", "turn": "t-1", "chat": "c-1",
            "prompts": ["m-1"]});
        let elsewhere = json!({"item": "t-2", "type": "turn", "state": "running", "title": "Working",
            "text": "", "body": {"started_by": "user"}, "session": "s-2", "turn": "t-2", "chat": "c-2",
            "prompts": ["m-1"]});
        for shown in [message, elsewhere] {
            let (core, connector) = UnixStream::pair().unwrap();
            let (read, mut write) = connector.into_split();
            write
                .write_all(hello(json!(TOKEN)).as_bytes())
                .await
                .unwrap();
            let mut driver = greeted(core).await;
            let event = json!({"jsonrpc": "2.0", "method": "event", "params": shown});
            let peer = tokio::spawn(async move {
                let mut lines = BufReader::new(read).lines();
                let _welcome = lines.next_line().await;
                let _prompt = lines.next_line().await;
                write
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .unwrap();
            });
            assert!(
                driver.request(a_prompt()).await.is_err(),
                "not delivered: no turn of its chat responds to it, after {shown}"
            );
            peer.await.unwrap();
            assert!(matches!(driver.next_event().await, Some(Output::Event(_))));
        }
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
        let result = driver.request(a_prompt()).await;
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
            unanswered: false,
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

    /// An event of about `size` bytes, as a line.
    fn a_big_event(size: usize) -> String {
        let text = "x".repeat(size);
        format!(
            r#"{{"jsonrpc":"2.0","method":"event","params":{{"item":"i","type":"notice","state":"done","title":"t","text":"{text}","body":{{"kind":"warning"}},"native":{{"kind":"k","payload":{{}}}}}}}}"#
        ) + "\n"
    }

    // A connector that goes on sending while the core waits for its answer, more than the core
    // holds for one connection, is given up then, not when the request's time runs out: what the
    // core holds stays within the budget, the line that would pass it is not kept, the request is
    // told as perhaps taken, and what was held still reaches the core (D-145).
    #[tokio::test]
    async fn a_connector_that_sends_more_than_the_core_holds_is_given_up() {
        let (core, connector) = UnixStream::pair().unwrap();
        let (_read, mut write) = connector.into_split();
        write
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core).await;
        // Lines of exactly a mebibyte before their newline, so that the budget holds a whole
        // number of them.
        let size = 1024 * 1024;
        let line = a_big_event(size + 1 - a_big_event(0).len());
        assert_eq!(line.len(), size + 1);
        let flooding = tokio::spawn(async move {
            // It never answers, and stops only when the core lets go of the connection.
            while write.write_all(line.as_bytes()).await.is_ok() {}
        });
        let result = tokio::time::timeout(Duration::from_secs(10), driver.request(a_prompt()))
            .await
            .expect("given up before the request's time ran out")
            .unwrap_err();
        assert_eq!(result.reason, Reason::Failed);
        assert!(result.unanswered, "the connector may have the prompt");
        let mut held = 0;
        while let Some(output) = driver.next_event().await {
            assert!(output.into_event().is_some(), "the events it sent");
            held += 1;
        }
        assert_eq!(
            held,
            HELD_MAX / size,
            "as many as fit, and not the one past them"
        );
        drop(driver);
        flooding.await.unwrap();
    }

    // A connector that sends the same file's id again and again, while the core only reads, is
    // given up too once what it holds to answer passes the budget: those answers wait for the next
    // write, so they are held (D-145).
    #[tokio::test]
    async fn a_connector_that_repeats_a_file_request_endlessly_is_given_up() {
        let (core, connector) = UnixStream::pair().unwrap();
        let (_read, mut write) = connector.into_split();
        write
            .write_all(hello(json!(TOKEN)).as_bytes())
            .await
            .unwrap();
        let mut driver = greeted(core).await;
        let ask = json!({"jsonrpc": "2.0", "id": 7, "method": "send_file",
            "params": {"chat": "c-1", "path": format!("/work/{}", "x".repeat(4096))}});
        let line = format!("{ask}\n");
        let flooding =
            tokio::spawn(async move { while write.write_all(line.as_bytes()).await.is_ok() {} });
        let Some(Output::SendFile(_)) = driver.next_event().await else {
            panic!("the first file to send");
        };
        let next = tokio::time::timeout(Duration::from_secs(10), driver.next_event())
            .await
            .expect("given up, not read for ever");
        assert!(next.is_none(), "given up");
        assert!(
            driver.holding <= HELD_MAX,
            "the refusal past the budget is not held"
        );
        assert_eq!(
            driver.holding,
            driver.duplicates.iter().map(|(_, s)| s).sum::<usize>()
        );
        drop(driver);
        flooding.await.unwrap();
    }
}

/// Bounded proofs (`docs/design/verification.md`, "5. Bounded proofs with Kani").
#[cfg(kani)]
mod proofs {
    use super::same;

    /// The connector's token is taken only when it is the core's, byte for byte (D-038): every
    /// pair of byte strings of up to 4 bytes. The comparison looks at every byte whatever it
    /// finds, so it takes as long for a near miss as for a far one.
    #[kani::proof]
    #[kani::unwind(6)]
    fn the_token_is_taken_only_when_it_is_the_cores() {
        let (a, b): ([u8; 4], [u8; 4]) = (kani::any(), kani::any());
        let (n, m): (usize, usize) = (kani::any(), kani::any());
        kani::assume(n <= 4 && m <= 4);
        kani::assume(a[..n].is_ascii() && b[..m].is_ascii());
        let given = std::str::from_utf8(&a[..n]).expect("ASCII");
        let token = std::str::from_utf8(&b[..m]).expect("ASCII");
        assert_eq!(same(given, token), given == token);
    }
}
