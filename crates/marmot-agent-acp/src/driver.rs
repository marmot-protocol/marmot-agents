//! The transport: runs an ACP agent on standard input and output, gives each chat a translator of
//! its own, and is the driver the core holds.
//!
//! One agent process serves every chat of one agent identity. ACP names the session in every
//! notification and request from the agent, so each goes to the chat whose session it names, and a
//! reply goes to the chat that made the request. Each message the driver sends is fed to that
//! chat's translator as well, so the translator learns what was sent the same way whoever sent it.
//! An answer or a control is settled once, by the event that shows its effect, or by the agent
//! refusing it.
//!
//! The agent can be any command: `opencode acp`, `cursor-agent acp`, or Goose through
//! `tools/goose/goose-acp.sh`, which runs it in a throwaway container.
//!
//! When file sending is on ([`Acp::serve_files`]; on unless the settings switch it off, D-110), an
//! agent that takes MCP servers over HTTP is given the driver's own on every session it
//! opens, with one tool, `send_file` (D-090, [`crate::mcp`]). A call goes to the chat whose session
//! it names, only when the token it carries was given to that chat, or, for OpenCode, to a chat in
//! the same folder; a call that names no session (Cursor) goes to the chat of its token. It becomes
//! a request for the core to send the file.

use crate::mcp::{self, Call, Named, Owner, Server, Tokens};
use crate::{Direction, Translator};
use marmot_agent_driver::{
    Action, Answer, Capabilities, ChatId, Control, Driver, Emitted, FilesIn, Harness, MessageId,
    MidTurn, Output, Prompt, Reason, Refusal, RequestId, SendFile, Sessions,
};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, Command};

type Reader = Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>;

/// How long the driver waits for the agent to end a turn it was asked to cancel, or a run it took
/// up by itself, before it gives up.
const ENDING: Duration = Duration::from_secs(60);

/// A message for the agent's input, and who waits to hear whether it was written.
type Line = (String, Option<tokio::sync::oneshot::Sender<bool>>);

/// What a request the driver sent is for.
enum Purpose {
    /// Nothing follows from it but what the chat's translator reads in the reply.
    Plain,
    /// A prompt: the turn it starts responds to this message.
    Prompt(MessageId),
    /// A control, settled by its effect or by the agent refusing it.
    Control(RequestId),
    /// A setting changed: settled by the agent's reply, whatever it changed.
    Set(RequestId),
}

/// What shows that a control has had its effect.
#[derive(Clone)]
enum Effect {
    /// The turn it interrupted has ended.
    TurnEnded(String),
    /// A session other than this one is running.
    NewSession(Option<String>),
}

/// One chat: its translator, the controls waiting for their effect, the request picking its
/// session, while one is waiting, with the folder it opens the session in, and the folder its
/// session works in once the agent has opened it there.
struct Chat {
    translator: Translator,
    effects: Vec<(RequestId, Effect)>,
    picking: Option<String>,
    opening: Option<String>,
    cwd: Option<String>,
}

/// Which chats a token for the driver's MCP server may send for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Its own chat: the agent keeps the servers of each session (Goose, Cursor).
    Chat,
    /// Every chat in its folder: OpenCode keeps one server of a name for every session in a
    /// folder, the one handed over last, so a call carries the token of whichever chat opened a
    /// session there last, and must name its session.
    Folder,
}

/// Where the core's word on a file goes: the tool call waiting for it.
type FileReply = tokio::sync::oneshot::Sender<Result<(), Refusal>>;

/// What the driver read next: a line from the agent, or a call of its tool.
enum Read {
    Line(io::Result<Option<String>>),
    Call(Call),
}

/// The ACP driver: one agent process, serving every chat of one agent.
pub struct Acp {
    reader: Reader,
    /// Each message for the agent goes whole to a task that writes it, so a future the core drops
    /// never leaves half a message on the agent's input. A request's message comes with a channel
    /// the task says on whether it was written.
    writer: tokio::sync::mpsc::UnboundedSender<Line>,
    /// The task that writes, which holds the agent's input. Ending the connection ends it, so a
    /// message it has not finished writing never reaches the agent later.
    writing: tokio::task::JoinHandle<()>,
    /// The process, when the driver started it. It is killed when the driver is dropped.
    child: Option<Child>,
    /// The driver's name for the agent, until the agent names itself.
    name: String,
    version: Option<String>,
    /// What the agent said it can do: take up a session again, with or without replaying it.
    load: bool,
    resume: bool,
    /// The folder a new session works in.
    cwd: String,
    next_id: i64,
    /// The `initialize` request and its reply, which every chat's translator reads first.
    greeting: Vec<(Direction, String)>,
    chats: HashMap<ChatId, Chat>,
    /// Each chat's session, by the agent's session id.
    sessions: HashMap<String, ChatId>,
    /// Every session a chat has had, and the chat: a session belongs to its chat for good.
    owners: HashMap<String, ChatId>,
    /// The driver's requests the agent has not answered, by id: the chat each is for, and what for.
    requests: HashMap<String, (ChatId, Purpose)>,
    /// The prompt behind each `session/prompt` sent, by its id, until its turn starts.
    starting: HashMap<String, MessageId>,
    /// The prompts each running turn responds to.
    prompts: HashMap<(ChatId, String), Vec<MessageId>>,
    /// Messages naming a session no chat has yet, kept while a session is being picked.
    early: Vec<String>,
    /// Events read, and files asked to be sent, not yet handed to the core.
    ready: VecDeque<Output>,
    /// The agent has gone.
    gone: bool,
    /// Messages for no chat.
    unrouted: u64,
    /// Chats whose turn the driver asked the agent to cancel, until the turn ends. A request the
    /// agent makes meanwhile is answered as cancelled at once.
    cancelling: HashSet<ChatId>,
    /// Sessions in which Goose is running something by itself: it says so in
    /// `_meta.goose.activeRunId`, and refuses a prompt until it clears it.
    busy: HashSet<String>,
    /// Each tool call seen, by its id, with the session it was in. Cursor's `cursor/update_todos`
    /// names its tool call and no session.
    calls: HashMap<String, Vec<String>>,
    /// How long the driver waits for the agent to answer a session request, end a cancelled turn,
    /// or end a run of its own.
    patience: Duration,
    /// Whether the agent takes MCP servers over HTTP, and which chats its tokens may send for.
    takes_http: bool,
    scope: Scope,
    /// Whether the agent names, in every call of its tool, the session that made it (OpenCode,
    /// Goose): a call of such an agent that names none is refused.
    names_caller: bool,
    /// The driver's MCP server, once the settings switch file sending on and the agent takes it,
    /// and the token for it of each chat and folder.
    mcp: Option<Server>,
    tokens: Tokens,
    issued: HashMap<Owner, String>,
    /// The tool calls handed to the core, by the driver's id for each, with their chat, until the
    /// core answers.
    files: HashMap<String, (ChatId, FileReply)>,
    next_file: u64,
}

impl Acp {
    /// Starts the agent with `command`, its program and arguments, in `cwd`, and greets it. `name`
    /// is the driver's name for the agent, such as `opencode`, which events carry until the agent
    /// names itself.
    pub async fn start(command: &[String], cwd: &str, name: &str) -> io::Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| io::Error::other("no command to start the agent"))?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(io::Error::other(
                "the agent has no standard input or output",
            ));
        };
        let mut acp = Self::connect(stdout, stdin, cwd, name).await?;
        acp.child = Some(child);
        Ok(acp)
    }

    /// Greets an agent that is already running, over its output and input.
    pub async fn connect(
        output: impl AsyncRead + Unpin + Send + 'static,
        input: impl AsyncWrite + Unpin + Send + 'static,
        cwd: &str,
        name: &str,
    ) -> io::Result<Self> {
        let output: Box<dyn AsyncRead + Unpin + Send> = Box::new(output);
        let (writer, mut lines) = tokio::sync::mpsc::unbounded_channel::<Line>();
        let mut input = input;
        let writing = tokio::spawn(async move {
            while let Some((line, done)) = lines.recv().await {
                let written = async {
                    input.write_all(line.as_bytes()).await?;
                    input.write_all(b"\n").await?;
                    input.flush().await
                };
                let ok = written.await.is_ok();
                if let Some(done) = done {
                    let _ = done.send(ok);
                }
                if !ok {
                    break; // the agent's input is closed: the driver learns it from the next send
                }
            }
        });
        // Request ids start from the time, so a turn, which is named after the request that
        // started it, never takes the name of a turn from before a restart.
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
        let mut acp = Self {
            reader: BufReader::new(output).lines(),
            writer,
            writing,
            child: None,
            name: name.to_string(),
            version: None,
            load: false,
            resume: false,
            cwd: cwd.to_string(),
            next_id: millis * 1000,
            greeting: Vec::new(),
            chats: HashMap::new(),
            sessions: HashMap::new(),
            owners: HashMap::new(),
            requests: HashMap::new(),
            starting: HashMap::new(),
            prompts: HashMap::new(),
            early: Vec::new(),
            ready: VecDeque::new(),
            gone: false,
            unrouted: 0,
            cancelling: HashSet::new(),
            busy: HashSet::new(),
            calls: HashMap::new(),
            patience: ENDING,
            takes_http: false,
            scope: Scope::Chat,
            names_caller: false,
            mcp: None,
            tokens: Tokens::default(),
            issued: HashMap::new(),
            files: HashMap::new(),
            next_file: 0,
        };
        let id = acp.id();
        let hello = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                // The agent reads and writes files and runs commands itself; the client shows
                // forms, which is how an agent asks a question.
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false,
                    "elicitation": {"form": {}},
                },
                "clientInfo": {"name": "marmot-agent", "version": env!("CARGO_PKG_VERSION")},
            },
        })
        .to_string();
        acp.write(&hello).await?;
        let deadline = Instant::now() + ENDING;
        let greeting = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let read = tokio::time::timeout(left, acp.reader.next_line()).await;
            let Ok(read) = read else {
                return Err(io::Error::other("the agent did not reply in time"));
            };
            let Some(line) = read? else {
                return Err(io::Error::other("the agent ended before it replied"));
            };
            let message: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if message["id"] == id && message.get("method").is_none() {
                break (line, message);
            }
        };
        let (line, message) = greeting;
        if let Some(error) = message.get("error") {
            // The agent's own words can hold a path, and an error may be logged (D-071).
            let code = error["code"].as_i64().map(|c| format!(" (code {c})"));
            return Err(io::Error::other(format!(
                "the agent refused to start{}",
                code.unwrap_or_default()
            )));
        }
        let result = &message["result"];
        acp.version = result["agentInfo"]["version"].as_str().map(String::from);
        acp.load = result["agentCapabilities"]["loadSession"].as_bool() == Some(true);
        acp.resume = result["agentCapabilities"]["sessionCapabilities"]["resume"].is_object();
        // OpenCode, Goose and Cursor take MCP servers over HTTP; an agent that does not is given
        // none, and has no tool to send a file.
        acp.takes_http = result["agentCapabilities"]["mcpCapabilities"]["http"] == true;
        let name = result["agentInfo"]["name"].as_str().unwrap_or_default();
        if name.eq_ignore_ascii_case("opencode") {
            acp.scope = Scope::Folder;
        }
        acp.names_caller = ["opencode", "goose"]
            .iter()
            .any(|n| name.eq_ignore_ascii_case(n));
        acp.greeting = vec![(Direction::Out, hello), (Direction::In, line)];
        Ok(acp)
    }

    /// Gives the agent the driver's `send_file` tool from now on, when it takes MCP servers over
    /// HTTP: starts the driver's MCP server on the loopback address. True when the agent has the
    /// tool. Off unless called, which the core does unless the settings say `send_files = false`
    /// (D-110).
    pub async fn serve_files(&mut self) -> io::Result<bool> {
        if self.takes_http && self.mcp.is_none() {
            self.mcp = Some(Server::start(self.tokens.clone()).await?);
        }
        Ok(self.mcp.is_some())
    }

    /// How long to wait for the agent before giving up on it: a minute unless set.
    pub fn set_patience(&mut self, patience: Duration) {
        self.patience = patience;
    }

    /// Messages that went to no chat.
    pub fn unrouted(&self) -> u64 {
        self.unrouted
    }

    fn id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Hands a message to the task that writes it, and does not wait: for what the driver writes
    /// while it reads, in a future the core may drop.
    async fn write(&mut self, text: &str) -> io::Result<()> {
        self.writer
            .send((text.to_string(), None))
            .map_err(|_| io::Error::other("the agent's input is closed"))
    }

    /// Hands a message to the task that writes it, and waits until it is written: a request
    /// returns once the agent has it, and an answer the agent cannot read is not shown as given.
    /// An agent that has not read it within the patience has stopped working: the connection
    /// ends, so what was not written never reaches the agent after the request failed (D-066).
    async fn write_whole(&mut self, text: &str) -> io::Result<()> {
        let (done, written) = tokio::sync::oneshot::channel();
        let closed = || io::Error::other("the agent's input is closed");
        self.writer
            .send((text.to_string(), Some(done)))
            .map_err(|_| closed())?;
        match tokio::time::timeout(self.patience, written).await {
            Ok(Ok(true)) => Ok(()),
            Ok(_) => Err(closed()),
            Err(_) => {
                self.end();
                Err(io::Error::other("the agent stopped reading"))
            }
        }
    }

    /// Sends a message for a chat once it is written, and feeds it to that chat's translator; the
    /// events it makes settle `settles`. A request is kept, with what it is for, until the agent
    /// answers it. Only the core's requests send this way: the wait is not safe to drop.
    async fn send(
        &mut self,
        chat: &ChatId,
        request: Option<(String, Purpose)>,
        text: &str,
        settles: Vec<RequestId>,
    ) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        if self.write_whole(text).await.is_err() {
            self.stopped();
            return Err(Refusal::new(Reason::Failed));
        }
        if let Some((id, purpose)) = request {
            if let Purpose::Prompt(message) = &purpose {
                self.starting.insert(id.clone(), message.clone());
            }
            self.requests.insert(id, (chat.clone(), purpose));
        }
        self.feed(chat, Direction::Out, text, settles);
        Ok(())
    }

    /// Sends a request for a chat and gives back its id.
    async fn request(
        &mut self,
        chat: &ChatId,
        method: &str,
        params: Value,
        purpose: Purpose,
    ) -> Result<String, Refusal> {
        let id = self.id();
        let text =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        let id = id.to_string();
        if matches!(method, "session/new" | "session/load" | "session/resume")
            && let Some(c) = self.chats.get_mut(chat)
        {
            c.picking = Some(id.clone());
            c.opening = params["cwd"].as_str().map(String::from);
        }
        self.send(chat, Some((id.clone(), purpose)), &text, Vec::new())
            .await?;
        Ok(id)
    }

    /// Reads one message from the agent, or one call of its tool, and handles it. False once the
    /// agent has gone. Both waits are safe to drop: nothing of a line, nor a call, is lost.
    async fn read(&mut self) -> bool {
        if self.gone {
            return false;
        }
        let read = self.next_read().await;
        self.handle(read).await
    }

    /// The next line from the agent, or call of its tool, whichever comes first. Safe to drop.
    async fn next_read(&mut self) -> Read {
        match self.mcp.as_mut() {
            Some(server) => tokio::select! {
                line = self.reader.next_line() => Read::Line(line),
                Some(call) = server.calls.recv() => Read::Call(call),
            },
            None => Read::Line(self.reader.next_line().await),
        }
    }

    /// Handles what was read. False once the agent has gone.
    async fn handle(&mut self, read: Read) -> bool {
        match read {
            Read::Call(call) => {
                self.tool_call(call);
                true
            }
            Read::Line(Ok(Some(line))) => {
                self.route(&line).await;
                true
            }
            Read::Line(Ok(None) | Err(_)) => {
                self.stopped();
                false
            }
        }
    }

    /// The agent calls the driver's tool. The call goes to the chat whose session it names, which
    /// must be the chat the call's token was given to, or, for OpenCode, a chat whose session works
    /// in the token's folder: OpenCode keeps one server for every session in a folder, so its
    /// calls carry whichever chat's token came last there, and must name their session. A call that
    /// names none goes to the chat of its token, for an agent that never names it (Cursor); Goose
    /// and OpenCode always do, so one of theirs that names none is refused. A call naming a session no chat has, or
    /// another chat's, or naming it in a shape the driver cannot read, is refused, never guessed. A
    /// relative path is made whole from the chat's folder; the core takes only an absolute one,
    /// and follows any `..` itself.
    fn tool_call(&mut self, call: Call) {
        let refuse = |call: Call, detail: &str| {
            let refused = Refusal {
                reason: Reason::Invalid,
                detail: Some(detail.into()),
            };
            let _ = call.reply.send(Err(refused));
        };
        let chat = match (&call.session, self.scope) {
            (Named::Session(session), scope) => {
                (self.owners.get(session).cloned()).filter(|owner| match scope {
                    Scope::Chat => *owner == call.owner.chat,
                    Scope::Folder => self.folder_of(owner) == call.owner.folder,
                })
            }
            (Named::Nothing, Scope::Chat) if !self.names_caller => Some(call.owner.chat.clone()),
            (Named::Nothing, _) | (Named::Malformed, _) => None,
        };
        let Some(chat) = chat.filter(|c| self.chats.contains_key(c)) else {
            return refuse(call, "the driver cannot tell which chat asked.");
        };
        let Some(path) = call.path.clone().filter(|p| !p.is_empty()) else {
            return refuse(call, "the tool was not given a path.");
        };
        let path = PathBuf::from(self.folder_of(&chat)).join(path);
        self.next_file += 1;
        let id = format!("file-{}", self.next_file);
        self.files.insert(id.clone(), (chat.clone(), call.reply));
        self.ready
            .push_back(Output::SendFile(SendFile { id, chat, path }));
    }

    /// The folder a chat's session works in: the one the agent last opened it in, or else the
    /// driver's own.
    fn folder_of(&self, chat: &ChatId) -> String {
        (self.chats.get(chat).and_then(|c| c.cwd.clone())).unwrap_or_else(|| self.cwd.clone())
    }

    /// Whether a file the chat's agent asked to send waits for the core's word on it.
    fn filing(&self, chat: &ChatId) -> bool {
        self.files.values().any(|(c, _)| c == chat)
    }

    /// The MCP servers a chat's session in `folder` is given: the driver's, when it serves one,
    /// with the token of that chat and folder. A token that cannot be made leaves the session
    /// without the tool.
    fn servers(&mut self, chat: &ChatId, folder: &str) -> Value {
        let Some(server) = &self.mcp else {
            return json!([]);
        };
        let owner = Owner {
            chat: chat.clone(),
            folder: folder.to_string(),
        };
        let token = match self.issued.get(&owner) {
            Some(token) => token.clone(),
            None => {
                let Ok(token) = mcp::token() else {
                    return json!([]);
                };
                let Ok(mut tokens) = self.tokens.lock() else {
                    return json!([]);
                };
                tokens.insert(token.clone(), owner.clone());
                self.issued.insert(owner, token.clone());
                token
            }
        };
        server.for_session(&token)
    }

    /// Reads one message, or one call of the agent's tool, giving up at `deadline`. False once the
    /// agent has gone or the time is up. A call is made ready for the core, as when the core reads.
    async fn read_until(&mut self, deadline: Instant) -> bool {
        if self.gone {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        // Reading is safe to give up on: nothing of a line, nor a call, is lost.
        match tokio::time::timeout(left, self.next_read()).await {
            Err(_) => false,
            Ok(read) => self.handle(read).await,
        }
    }

    /// Gives a message from the agent to the chat it is for, and answers the agent's requests
    /// that nobody in the chat is asked about.
    async fn route(&mut self, line: &str) {
        let message: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            let named = message["params"]["sessionId"].as_str().map(String::from);
            let call = message["params"]["update"]["toolCallId"].as_str();
            if let (Some(s), Some(call)) = (&named, call) {
                let sessions = self.calls.entry(call.to_string()).or_default();
                if !sessions.contains(s) {
                    sessions.push(s.clone());
                }
            }
            // A request that names only its tool call is for that call's session, when only one
            // session has a call of that id.
            let session = named.or_else(|| {
                let call = message["params"]["toolCallId"].as_str()?;
                match self.calls.get(call).map(Vec::as_slice) {
                    Some([only]) => Some(only.clone()),
                    _ => None,
                }
            });
            let chat = session.as_ref().and_then(|s| self.sessions.get(s)).cloned();
            // Goose says when it runs something by itself, and when it is done.
            if let (Some(s), Some(run)) = (
                &session,
                message["params"]["update"]["_meta"]["goose"].get("activeRunId"),
            ) {
                if run.is_string() {
                    self.busy.insert(s.clone());
                } else {
                    self.busy.remove(s);
                    let idle = chat
                        .as_ref()
                        .filter(|c| self.translator(c).is_some_and(|t| t.turn().is_none()));
                    if let Some(c) = idle.cloned() {
                        self.cancelling.remove(&c);
                    }
                }
            }
            match &chat {
                Some(chat) => self.feed(chat, Direction::In, line, Vec::new()),
                // A session being picked can speak before the reply that picks it.
                None if session.is_some() && self.picking() => {
                    self.early.push(line.to_string());
                    return;
                }
                None => self.unrouted += 1,
            }
            if let Some(id) = message.get("id") {
                // A request no person answers: Cursor's to-do list is taken, anything else is
                // refused, so the agent does not wait for ever.
                let reply = match method {
                    "session/request_permission" | "elicitation/create"
                        if chat_known(&message, &self.sessions) =>
                    {
                        None
                    }
                    "cursor/update_todos" => {
                        Some(json!({"jsonrpc": "2.0", "id": id, "result": {}}))
                    }
                    _ => Some(
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}),
                    ),
                };
                if let Some(reply) = reply
                    && self.write(&reply.to_string()).await.is_err()
                {
                    self.stopped();
                }
                // A turn being cancelled asks nobody: what it asks now is answered as cancelled.
                if let Some(chat) = chat.filter(|c| self.cancelling.contains(c)) {
                    let replies = self
                        .translator(&chat)
                        .map(Translator::cancel_replies)
                        .unwrap_or_default();
                    for reply in replies {
                        // Read in a future the core may drop: handed over, not waited for.
                        if self.write(&reply).await.is_err() {
                            self.stopped();
                            break;
                        }
                        self.feed(&chat, Direction::Out, &reply, Vec::new());
                    }
                }
            }
            return;
        }
        let id = message.get("id").map(Value::to_string);
        let Some((chat, purpose)) = id.as_ref().and_then(|id| self.requests.remove(id)) else {
            self.unrouted += 1;
            return;
        };
        let refused = message.get("error").is_some();
        let mut settles = Vec::new();
        match purpose {
            Purpose::Plain => {}
            Purpose::Prompt(_) => {
                if refused && let Some(id) = &id {
                    self.starting.remove(id); // no turn will start
                }
            }
            // The agent refused a control: the event the refusal makes settles it, unless its
            // effect already did.
            Purpose::Control(request) => {
                if refused && self.unsettled(&chat, &request) {
                    settles.push(request);
                }
            }
            // A setting is settled by the reply, refused or not, even one that changed nothing.
            Purpose::Set(request) => settles.push(request),
        }
        // The session works in its folder once the agent has opened it there, and not before.
        if let Some(c) = self.chats.get_mut(&chat)
            && c.picking == id
        {
            c.picking = None;
            let opened = c.opening.take();
            if !refused && opened.is_some() {
                c.cwd = opened;
            }
        }
        self.feed(&chat, Direction::In, line, settles);
        // What a session said before it was picked goes to it now; what names a session still
        // unknown waits while a session is being picked.
        let (known, unknown): (Vec<String>, Vec<String>) = std::mem::take(&mut self.early)
            .into_iter()
            .partition(|line| session_of(line).is_some_and(|s| self.sessions.contains_key(&s)));
        self.early = unknown;
        for line in known {
            Box::pin(self.route(&line)).await;
        }
        if !self.picking() {
            self.unrouted += self.early.len() as u64;
            self.early.clear();
        }
    }

    /// Whether a request to start or take up a session is waiting for the agent, in any chat.
    fn picking(&self) -> bool {
        self.chats.values().any(|c| c.picking.is_some())
    }

    /// Takes a control off the list of those waiting for their effect; false if it was not there.
    fn unsettled(&mut self, chat: &ChatId, request: &RequestId) -> bool {
        let Some(c) = self.chats.get_mut(chat) else {
            return false;
        };
        let before = c.effects.len();
        c.effects.retain(|(r, _)| r != request);
        c.effects.len() != before
    }

    /// Feeds a message to a chat's translator and makes its events ready.
    fn feed(&mut self, chat: &ChatId, direction: Direction, text: &str, settles: Vec<RequestId>) {
        let Some(c) = self.chats.get_mut(chat) else {
            return;
        };
        let before = c.translator.session().map(String::from);
        let events = c.translator.feed(direction, text);
        let after = c.translator.session().map(String::from);
        let turn_request = c.translator.turn_request().map(String::from);
        // Cancelling lasts until the turn ends, and until Goose ends a run of its own.
        let own_run = after.as_ref().is_some_and(|s| self.busy.contains(s));
        if c.translator.turn().is_none() && !own_run {
            self.cancelling.remove(chat);
        }
        if before != after {
            if let Some(old) = before {
                self.sessions.remove(&old);
            }
            if let Some(new) = after {
                self.owners.insert(new.clone(), chat.clone());
                self.sessions.insert(new, chat.clone());
            }
        }
        // What a message settles goes with the event it made itself, the last: the translator
        // first sends any text it was holding.
        let mut settles = settles;
        let last = events.len().saturating_sub(1);
        for (at, event) in events.into_iter().enumerate() {
            let settled = if at == last {
                std::mem::take(&mut settles)
            } else {
                Vec::new()
            };
            self.ready_event(chat, event, settled, true, turn_request.as_deref());
        }
    }

    /// Makes an event ready for the core. An event the agent sent shows the effect of a control;
    /// one the driver made when the agent stopped does not (`seen` false): that control failed,
    /// and the core settles it so (D-058).
    fn ready_event(
        &mut self,
        chat: &ChatId,
        event: AgentEvent,
        mut settles: Vec<RequestId>,
        seen: bool,
        turn_request: Option<&str>,
    ) {
        let Some(c) = self.chats.get_mut(chat) else {
            return;
        };
        let header = event.header();
        let turn = header.turn.as_ref().map(|t| t.as_str().to_string());
        let ended = !matches!(header.state, State::Running | State::Waiting);
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        c.effects.retain(|(request, effect)| {
            let shown = seen
                && match (effect, event.body()) {
                    (Effect::TurnEnded(t), Body::Turn(_)) => ended && turn.as_deref() == Some(t),
                    (Effect::NewSession(old), Body::Session(_)) => {
                        header.state == State::Running && session != *old
                    }
                    _ => false,
                };
            if shown {
                settles.push(request.clone());
            }
            !shown
        });
        let prompts = match (event.body(), turn) {
            (Body::Turn(_), Some(turn)) => {
                let key = (chat.clone(), turn);
                if !self.prompts.contains_key(&key) {
                    let prompt = turn_request.and_then(|r| self.starting.remove(r));
                    self.prompts
                        .insert(key.clone(), prompt.into_iter().collect());
                }
                if ended {
                    self.prompts.remove(&key).unwrap_or_default() // nothing more comes for it
                } else {
                    self.prompts[&key].clone()
                }
            }
            _ => Vec::new(),
        };
        self.ready.push_back(Output::Event(Emitted {
            event,
            chat: Some(chat.clone()),
            prompts,
            settles,
        }));
    }

    /// The agent has gone: each translator closes what it can, and nothing more will come.
    fn stopped(&mut self) {
        if self.gone {
            return;
        }
        self.gone = true;
        let chats: Vec<ChatId> = self.chats.keys().cloned().collect();
        for chat in chats {
            let events = match self.chats.get_mut(&chat) {
                Some(c) => c.translator.harness_stopped(),
                None => continue,
            };
            for event in events {
                self.ready_event(&chat, event, Vec::new(), false, None);
            }
        }
        self.requests.clear();
        self.starting.clear();
        self.early.clear();
    }
}

/// Whether the session a message from the agent names belongs to a chat.
fn chat_known(message: &Value, sessions: &HashMap<String, ChatId>) -> bool {
    message["params"]["sessionId"]
        .as_str()
        .is_some_and(|s| sessions.contains_key(s))
}

/// The session a message from the agent names, if it names one.
fn session_of(line: &str) -> Option<String> {
    let message: Value = serde_json::from_str(line).ok()?;
    message["params"]["sessionId"].as_str().map(String::from)
}

/// What the agent reads for a chat message, as Codex does (D-062): in a group the sender's name,
/// a reply's quote first, and a slash command exactly as typed. ACP has one user per session.
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

// -- the core's requests

impl Acp {
    /// Makes a chat's translator the first time the chat is seen.
    fn chat(&mut self, chat: &ChatId) {
        if self.chats.contains_key(chat) {
            return;
        }
        let mut translator = Translator::named(&self.name);
        for (direction, text) in &self.greeting {
            let _ = translator.feed(*direction, text);
        }
        self.chats.insert(
            chat.clone(),
            Chat {
                translator,
                effects: Vec::new(),
                picking: None,
                opening: None,
                cwd: None,
            },
        );
    }

    fn translator(&self, chat: &ChatId) -> Option<&Translator> {
        self.chats.get(chat).map(|c| &c.translator)
    }

    /// Reads on until no session is being picked for the chat.
    /// A session request the agent never answers ends the connection: a reply that came later
    /// must not install a session after the prompt has failed.
    async fn picked(&mut self, chat: &ChatId) -> Result<(), Refusal> {
        let deadline = Instant::now() + self.patience;
        while self.chats.get(chat).is_some_and(|c| c.picking.is_some()) {
            if !self.read_until(deadline).await {
                self.end();
                return Err(Refusal::new(Reason::Failed));
            }
        }
        Ok(())
    }

    /// Ends the connection: the agent is stopped, nothing more is written to it or read from it,
    /// and each chat closes what it had open. Nothing the agent says later is read, even when the
    /// driver did not start it and cannot stop it.
    fn end(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        self.writing.abort();
        let nothing: Box<dyn AsyncRead + Unpin + Send> = Box::new(tokio::io::empty());
        self.reader = BufReader::new(nothing).lines();
        self.stopped();
    }

    /// Makes `session` the chat's session, starting a new one if it is none, and waits for the
    /// agent. A session is taken up with `session/resume` when the agent offers it, which replays
    /// nothing, and otherwise with `session/load`, whose replay the translator does not send again.
    ///
    /// The session works in `folder`, the chat's, or else the driver's own (D-101).
    async fn pick(
        &mut self,
        chat: &ChatId,
        session: Option<&str>,
        folder: Option<&Path>,
    ) -> Result<(), Refusal> {
        let cwd = self.cwd_for(folder);
        let servers = self.servers(chat, &cwd);
        let params = |id: Option<&str>| {
            let mut p = json!({"cwd": cwd, "mcpServers": servers});
            if let Some(id) = id {
                p["sessionId"] = id.into();
            }
            p
        };
        let method = match session {
            None => "session/new",
            Some(_) if self.resume => "session/resume",
            Some(_) if self.load => "session/load",
            Some(_) => return Err(Refusal::new(Reason::NotSupported)),
        };
        // A session to take up is the chat's from the moment it is asked for, and what the agent
        // replays of it goes to the chat.
        if let Some(s) = session {
            self.owners.insert(s.to_string(), chat.clone());
            self.sessions.insert(s.to_string(), chat.clone());
        }
        let p = params(session);
        self.request(chat, method, p, Purpose::Plain).await?;
        self.picked(chat).await?;
        let now = self
            .translator(chat)
            .and_then(Translator::session)
            .map(String::from);
        match (now, session) {
            (Some(now), Some(wanted)) if now == wanted => Ok(()),
            (Some(_), None) => Ok(()),
            (_, Some(wanted)) => {
                self.sessions.remove(wanted);
                if self.translator(chat).and_then(Translator::session) != Some(wanted) {
                    self.owners.remove(wanted);
                }
                Err(Refusal::new(Reason::Failed)) // the agent refused it; the chat shows why
            }
            (None, None) => Err(Refusal::new(Reason::Failed)),
        }
    }

    /// The folder a session works in: the chat's, or else the driver's own (D-101).
    fn cwd_for(&self, folder: Option<&Path>) -> String {
        folder.map_or_else(|| self.cwd.clone(), |f| f.to_string_lossy().into_owned())
    }

    /// Before every prompt: Goose carries on, by itself, with the turn a restart cut off once the
    /// session is taken up again, says so in `_meta.goose.activeRunId`, and refuses a prompt until
    /// it clears it. A restart closes that turn (D-070), so the driver cancels the run and waits for
    /// Goose to clear it. Other agents say nothing of the kind, and are not waited for.
    async fn wait_for_own_run(&mut self, chat: &ChatId) -> Result<(), Refusal> {
        let Some(session) = self
            .translator(chat)
            .and_then(Translator::session)
            .map(String::from)
        else {
            return Ok(());
        };
        if !self.busy.contains(&session) {
            return Ok(());
        }
        self.cancel(chat).await?;
        let deadline = Instant::now() + self.patience;
        while self.busy.contains(&session) {
            // A run waiting on the core's word on a file cannot end while the core waits here.
            if self.filing(chat) {
                return Err(Refusal::new(Reason::Busy));
            }
            if !self.read_until(deadline).await {
                return Err(Refusal::new(Reason::Failed));
            }
        }
        Ok(())
    }

    /// Cancels the chat's turn: tells the agent nobody will answer what it is waiting on, then
    /// asks it to stop (ACP's `session/cancel`).
    async fn cancel(&mut self, chat: &ChatId) -> Result<(), Refusal> {
        let Some(t) = self.translator(chat) else {
            return Err(Refusal::new(Reason::NothingToDo));
        };
        let (replies, session) = (t.cancel_replies(), t.session().map(String::from));
        self.cancelling.insert(chat.clone());
        for reply in replies {
            self.send(chat, None, &reply, Vec::new()).await?;
        }
        let note =
            json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}});
        self.send(chat, None, &note.to_string(), Vec::new()).await
    }

    /// The chat that has the answer's item waiting, and the reply to send it, or else the most
    /// telling refusal.
    fn chat_with_item(&self, answer: &Answer) -> Result<(ChatId, String), Refusal> {
        let mut refusal = Refusal::new(Reason::UnknownItem);
        for (chat, c) in &self.chats {
            match c.translator.reply(answer) {
                Ok(reply) => return Ok((chat.clone(), reply)),
                Err(r) if r.reason == Reason::UnknownItem => {}
                Err(r) => refusal = r,
            }
        }
        Err(refusal)
    }
}

impl Driver for Acp {
    fn harness(&self) -> Harness {
        Harness {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: true,
            // ACP v1 has no message during a turn: the driver cancels the turn, then prompts.
            mid_turn: MidTurn::Restart,
            sessions: Sessions::Core,
            resume: self.resume || self.load,
            approvals: true,
            files_in: FilesIn::None,
            approval_commands: Vec::new(),
        }
    }

    fn folder(&self) -> Option<PathBuf> {
        Some(PathBuf::from(&self.cwd))
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        if !prompt.attachments.is_empty() {
            return Err(Refusal::new(Reason::NotSupported));
        }
        let chat = prompt.chat.id.clone();
        // The core names a session only to take it up again (D-065).
        let wanted = prompt.session.as_ref().map(|s| s.as_str().to_string());
        // A session belongs to its chat for good.
        if let Some(owner) = wanted.as_ref().and_then(|w| self.owners.get(w))
            && *owner != chat
        {
            return Err(Refusal::new(Reason::Invalid));
        }
        self.chat(&chat);
        self.picked(&chat).await?;
        // A message during a turn restarts it (`mid_turn: restart`). A turn waiting on the core's
        // word on a file cannot end while the core waits here for it to: the chat is busy.
        if self.translator(&chat).and_then(Translator::turn).is_some() {
            if self.filing(&chat) {
                return Err(Refusal::new(Reason::Busy));
            }
            self.cancel(&chat).await?;
            let deadline = Instant::now() + self.patience;
            while self.translator(&chat).and_then(Translator::turn).is_some() {
                if self.filing(&chat) {
                    return Err(Refusal::new(Reason::Busy));
                }
                if !self.read_until(deadline).await {
                    return Err(Refusal::new(Reason::Failed));
                }
            }
        }
        let current = self
            .translator(&chat)
            .and_then(Translator::session)
            .map(String::from);
        if current.is_none() || (wanted.is_some() && wanted != current) {
            let folder = prompt.chat.folder.clone();
            self.pick(&chat, wanted.as_deref(), folder.as_deref())
                .await?;
        }
        self.wait_for_own_run(&chat).await?;
        let session = self
            .translator(&chat)
            .and_then(Translator::session)
            .map(String::from);
        let params =
            json!({"sessionId": session, "prompt": [{"type": "text", "text": text(&prompt)}]});
        self.request(
            &chat,
            "session/prompt",
            params,
            Purpose::Prompt(prompt.message.clone()),
        )
        .await?;
        Ok(())
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        let (chat, text) = self.chat_with_item(&answer)?;
        // The reply's event, fed back, shows the answer: it settles it.
        self.send(&chat, None, &text, vec![answer.request]).await
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        let request = control.request.clone();
        match &control.action {
            Action::Interrupt { turn } => {
                let chat = (self.chats.iter())
                    .find(|(_, c)| c.translator.turn() == Some(turn.as_str()))
                    .map(|(chat, _)| chat.clone())
                    .ok_or_else(|| Refusal::new(Reason::NothingToDo))?;
                if let Some(c) = self.chats.get_mut(&chat) {
                    c.effects
                        .push((request, Effect::TurnEnded(turn.as_str().to_string())));
                }
                self.cancel(&chat).await
            }
            Action::NewSession { chat, session } => {
                let chat_folder = chat.folder.clone();
                let chat = chat.id.clone();
                self.chat(&chat);
                self.picked(&chat).await?;
                // A session is replaced only when it is idle.
                if self.translator(&chat).and_then(Translator::turn).is_some() {
                    return Err(Refusal::new(Reason::Busy));
                }
                // Only a session other than the one the chat is in shows the effect. The core
                // names the session it saved, and none before it has seen one, so the driver's own
                // comes first: an edit of the chat's session must not settle the request.
                let current = self.translator(&chat).and_then(Translator::session);
                let old = (current.map(str::to_string))
                    .or_else(|| session.as_ref().map(|s| s.as_str().to_string()));
                if let Some(c) = self.chats.get_mut(&chat) {
                    c.effects.push((request.clone(), Effect::NewSession(old)));
                }
                let cwd = self.cwd_for(chat_folder.as_deref());
                let params = json!({"cwd": cwd, "mcpServers": self.servers(&chat, &cwd)});
                self.request(&chat, "session/new", params, Purpose::Control(request))
                    .await?;
                Ok(())
            }
            Action::Set {
                session,
                setting,
                value,
            } => {
                let chat = (self.sessions.get(session.as_str()).cloned())
                    .ok_or_else(|| Refusal::new(Reason::UnknownItem))?;
                let params =
                    json!({"sessionId": session.as_str(), "configId": setting, "value": value});
                self.request(
                    &chat,
                    "session/set_config_option",
                    params,
                    Purpose::Set(request),
                )
                .await?;
                Ok(())
            }
            // An ACP session offers no actions of its own.
            Action::Own { .. } => Err(Refusal::new(Reason::UnknownItem)),
        }
    }

    async fn next_event(&mut self) -> Option<Output> {
        loop {
            if let Some(output) = self.ready.pop_front() {
                return Some(output);
            }
            if !self.read().await {
                return self.ready.pop_front();
            }
        }
    }

    async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
        // The agent hears it as the tool's result. One that has gone hears nothing.
        if let Some((_, reply)) = self.files.remove(&id) {
            let _ = reply.send(result);
        }
    }
}
