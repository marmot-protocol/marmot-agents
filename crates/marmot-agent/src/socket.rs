//! The local socket: a driver that is a separate program (D-003, D-033 to D-038).
//!
//! A connector, such as the Claude Code driver, connects to the core's Unix socket and speaks
//! JSON-RPC 2.0, one message per line. It opens with a hello carrying its token and what its
//! harness can do; then the core sends it `prompt`, `answer` and `control` requests, and it sends
//! the core `event` notifications. To the core it is a driver like any other: [`SocketDriver`]
//! implements the same trait as the Codex driver, and the messages carry the trait's shapes
//! (D-058). The exact fields are in `docs/design/socket-protocol.md`.
//!
//! The socket is in a folder only the owner can open, and the token is in a file only the owner
//! can read (D-038). One connection is one agent (D-035).

use marmot_agent_driver::{
    Action, Answer, ApprovalCommand, Capabilities, Control, Driver, Emitted, FilesIn, Given,
    Harness, MessageId, MidTurn, Position, Prompt, Reason, Refusal, RequestId, Sessions,
};
use marmot_agent_events::AgentEvent;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};

/// The protocol a connector must speak (D-020).
pub const PROTOCOL: &str = "marmot.agent-control.v3";
/// The largest message either side may send (D-033).
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;

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

    /// Opens the socket, making its folder private, and gives the token, making it the first
    /// time.
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

    fn token(&self) -> io::Result<String> {
        match std::fs::read_to_string(&self.token_file) {
            Ok(token) => Ok(token.trim().to_string()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut bytes = [0u8; 32];
                getrandom::fill(&mut bytes).map_err(io::Error::other)?;
                let token = hex::encode(bytes);
                write_private(&self.token_file, token.as_bytes())?;
                Ok(token)
            }
            Err(error) => Err(error),
        }
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
    file.write_all(bytes)
}

/// Why a connection was not taken. Names nothing the connector sent (D-071).
#[derive(Debug)]
pub enum HelloError {
    /// The connection closed or failed before a hello.
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
            Self::Gone => "the connector went before its hello",
            Self::NotHello => "the connector did not open with a hello",
            Self::Protocol => "the connector speaks another protocol version",
            Self::Token => "the connector's token is not this agent's",
        })
    }
}

/// A connector on the socket, as a driver.
pub struct SocketDriver {
    reader: Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
    harness: Harness,
    capabilities: Capabilities,
    next_id: i64,
    /// Events read while a request waited for its response, for the next call of `next_event`.
    held: VecDeque<Emitted>,
    /// The number of the next event in each session, which the core gives (D-034).
    seq: HashMap<String, u64>,
    gone: bool,
}

impl SocketDriver {
    /// Takes a connection's hello: the right protocol and this agent's token, or it is refused
    /// with an error the connector can read.
    pub async fn greet(stream: UnixStream, token: &str, agent: &str) -> Result<Self, HelloError> {
        let (read, mut writer) = stream.into_split();
        let mut reader = BufReader::new(read).lines();
        let line = reader
            .next_line()
            .await
            .ok()
            .flatten()
            .ok_or(HelloError::Gone)?;
        let hello: Value = serde_json::from_str(&line).map_err(|_| HelloError::NotHello)?;
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
        if !same(params["token"].as_str().unwrap_or_default(), token) {
            let _ = write_line(&mut writer, &refuse(-32002, "not this agent's token")).await;
            return Err(HelloError::Token);
        }
        let Some(capabilities) = capabilities(&params["capabilities"]) else {
            let _ = write_line(
                &mut writer,
                &refuse(-32602, "capabilities missing or invalid"),
            )
            .await;
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
            reader,
            writer,
            harness,
            capabilities,
            next_id: 0,
            held: VecDeque::new(),
            seq: HashMap::new(),
            gone: false,
        })
    }

    /// Sends a request and waits for its response, holding back the events read meanwhile
    /// (D-058). A connection that drops before the response refuses it as failed: a prompt is
    /// then not delivered, and never sent again (D-037).
    async fn request(&mut self, method: &str, params: Value) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if write_line(&mut self.writer, &message).await.is_err() {
            self.gone = true;
            return Err(Refusal::new(Reason::Failed));
        }
        loop {
            let Some(message) = self.read().await else {
                return Err(Refusal::new(Reason::Failed));
            };
            if message["id"] == id && message.get("method").is_none() {
                return match message.get("error") {
                    None => Ok(()),
                    Some(error) => Err(refusal(error)),
                };
            }
            if let Some(emitted) = self.event(&message) {
                self.held.push_back(emitted);
            }
        }
    }

    /// The next message, or `None` once the connection is gone. A line that is not JSON, or is
    /// over the limit, is passed over.
    async fn read(&mut self) -> Option<Value> {
        loop {
            match self.reader.next_line().await {
                Ok(Some(line)) if line.len() <= MAX_MESSAGE => {
                    if let Ok(message) = serde_json::from_str(&line) {
                        return Some(message);
                    }
                }
                Ok(Some(_)) => {}
                _ => {
                    self.gone = true;
                    return None;
                }
            }
        }
    }

    /// An `event` notification as an event, with the harness and its place in its session filled
    /// in by the core. Anything else, or an event the core cannot read, is `None`.
    fn event(&mut self, message: &Value) -> Option<Emitted> {
        if message["method"] != "event" || message.get("id").is_some() {
            return None;
        }
        let mut params: Map<String, Value> = message["params"].as_object()?.clone();
        let chat = params
            .remove("chat")
            .and_then(|c| c.as_str().map(String::from));
        let ids = |value: Option<Value>| -> Vec<String> {
            value
                .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
                .unwrap_or_default()
        };
        let prompts = ids(params.remove("prompts"));
        let settles = ids(params.remove("settles"));
        params.insert("harness".into(), self.harness.name.clone().into());
        let session = params
            .get("session")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let seq = self.seq.entry(session).or_insert(0);
        *seq += 1;
        params.insert("seq".into(), (*seq).into());
        let event: AgentEvent = serde_json::from_value(Value::Object(params)).ok()?;
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
            } => {
                json!({"type": "set", "session": session.as_str(), "setting": setting, "value": value})
            }
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

    async fn next_event(&mut self) -> Option<Emitted> {
        loop {
            if let Some(emitted) = self.held.pop_front() {
                return Some(emitted);
            }
            // Reading a line can be dropped without losing it (tokio's `next_line` is cancel
            // safe), and nothing is written here, so this future is safe to drop (D-058).
            let message = self.read().await?;
            if let Some(emitted) = self.event(&message) {
                return Some(emitted);
            }
        }
    }
}

async fn write_line(writer: &mut OwnedWriteHalf, message: &Value) -> io::Result<()> {
    let mut line = message.to_string();
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
        _ => Reason::Failed,
    };
    Refusal {
        reason,
        detail: data["detail"].as_str().map(String::from),
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
