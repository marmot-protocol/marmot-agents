//! The transport: runs one Pi for each chat, feeds each chat's translator, and is the driver the
//! core holds.
//!
//! Pi in RPC mode holds one session at a time, so each chat gets a Pi process of its own, started
//! when the chat first needs it. A session is resumed by starting Pi with `--session <id>`. A
//! task reads each Pi's output into one channel, so the driver waits on every chat at once, and a
//! wait that is dropped loses nothing. Another task writes to each Pi, so a record the driver sends
//! while it reads is written whole even if the wait is dropped.
//!
//! Every record the driver sends is fed to the chat's translator too, once it has been written, so
//! the translator learns what was sent the same way whoever sent it, and an answer that never
//! reached Pi does not show as given. A request returns once its record has been written
//! to Pi, and does not wait for Pi to respond: Pi acknowledges a prompt only after an extension's
//! command or input handler has finished, and such a handler can be waiting for an answer to a
//! question, which the core can take only once the request has returned. Pi's response is read
//! like any record, and says what came of the request: for a prompt, the turn it joins.
//! Only the questions asked when Pi starts are waited for, and only for [`STARTUP_LIMIT`].
//!
//! A record is waited for only until the driver's patience runs out ([`PATIENCE`] unless set).
//! The task that writes to a chat's Pi keeps that deadline itself and says once what came of the
//! record: written in full, or not, in which case it has already let go of Pi's standard input
//! and of everything queued behind the record. A Pi that has not read it by then has stopped
//! working (D-066, D-098): that chat's Pi is stopped, and the request is refused as failed. The
//! chat's next prompt starts a new Pi; other chats' Pis run on.
//!
//! Every Pi the driver starts loads the driver's own extension, which gives Pi a tool, `send_file`
//! (D-090). Pi's RPC mode lets a client add no tool, so the tool asks the driver through Pi's
//! extension UI, in an `input` dialog titled [`SEND_FILE_REQUEST`]. The driver takes that dialog
//! as the request: it never reaches the translator, and so never shows as a question. It hands the
//! core a [`SendFile`], and answers the dialog with how the send went, which the tool gives the
//! model as its result. The tool call itself shows as a `tool` item, from Pi's own records.

use crate::translate::DIALOGS;
use crate::{Direction, Translator};
use marmot_agent_driver::{
    Action, Answer, Capabilities, ChatId, Control, Driver, Emitted, FilesIn, Harness, MessageId,
    MidTurn, Output, Prompt, Reason, Refusal, RequestId, SendFile, Sessions,
};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// The environment variable that names the chat a Pi serves, as a hash of the chat's id in
/// lowercase hex. A script that runs Pi elsewhere, such as `tools/pi/pi.sh`, keeps each chat's
/// sessions apart by it.
pub const CHAT_VARIABLE: &str = "MARMOT_AGENT_CHAT";

/// The environment variable that names the driver's extension file, which each Pi is started with
/// (`--extension <it>`). A script that runs Pi elsewhere makes that file readable there:
/// `tools/pi/pi.sh` mounts it read-only at the same path.
pub const EXTENSION_VARIABLE: &str = "MARMOT_AGENT_PI_EXTENSION";

/// The driver's extension, which gives Pi the `send_file` tool. It is built into the driver and
/// written out when the driver starts.
const EXTENSION: &str = include_str!("../extension/send-file.ts");

/// The title of the dialog the extension's `send_file` tool asks the driver in.
pub const SEND_FILE_REQUEST: &str = "marmot-agent send_file";

/// How long a Pi that was asked to stop has before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// How long a starting Pi has to answer what the session item shows. Pi 1.6.0 reads its standard
/// input only once its extensions' start-up handlers have finished, so one that waits for an
/// answer would never let it answer.
pub const STARTUP_LIMIT: Duration = Duration::from_secs(60);

/// How long a request waits for its record to be written to a chat's Pi, unless set.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// How Pi is run.
#[derive(Clone)]
pub struct Setup {
    /// The program: `pi`, or a script that runs Pi somewhere else, such as in a container.
    pub program: String,
    /// Arguments that go before Pi's own, for such a script.
    pub args: Vec<String>,
    /// The working folder.
    pub cwd: PathBuf,
}

/// Leaves out the program, its arguments and the working folder, which hold paths, such as the
/// model key file's: a path is not logged (D-071).
impl fmt::Debug for Setup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Setup")
            .field("args", &self.args.len())
            .finish_non_exhaustive()
    }
}

impl Setup {
    /// The command that runs a chat's Pi in `folder`, the chat's, or else the setup's own (D-101),
    /// with the driver's `extension`.
    fn command(&self, chat: &ChatId, folder: Option<&Path>, extension: &Path) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(folder.unwrap_or(&self.cwd))
            .env(CHAT_VARIABLE, chat_key(chat))
            .env(EXTENSION_VARIABLE, extension)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Pi writes diagnostics there, which can hold paths and words from the chat (D-071).
            .stderr(Stdio::null());
        command
    }
}

/// The name a chat's own state goes by: a hash of its id, so the id itself is not in it.
fn chat_key(chat: &ChatId) -> String {
    Sha256::digest(chat.0.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A line one chat's Pi wrote, or its end.
struct Line {
    chat: ChatId,
    generation: u64,
    text: Option<String>,
}

/// What shows that a control has had its effect.
#[derive(Clone)]
enum Effect {
    /// The turn it interrupted has ended.
    TurnEnded(String),
    /// The session item shows this setting with this value.
    Setting(String, String),
    /// A session other than this one is running.
    NewSession(Option<String>),
    /// A compaction has ended.
    Compacted,
}

/// What a command the driver sent waits for from Pi's response.
enum Waiting {
    /// A request that waits for the response itself.
    Reply,
    /// A prompt. The response says whether a run started for it, or it joined the running one,
    /// or an extension handled it.
    Prompt(MessageId),
    /// A control, which a refusal settles. What its success leads to comes after.
    Control(RequestId, Then),
}

/// What a control's successful response leads to.
#[derive(Clone, Copy)]
enum Then {
    /// Nothing: an event Pi sends shows the effect.
    Nothing,
    /// The session item shows the setting. Pi says nothing when a setting keeps its value, so
    /// the item is shown again if nothing has settled the control yet.
    ShowSession,
    /// A new session has started: the driver asks what its session item shows.
    Reopen,
}

/// How a starting Pi takes up a session.
#[derive(Clone, Copy, PartialEq)]
enum Resume {
    /// Opens the session's file, and exits if there is none.
    Open,
    /// Opens the session's file, or starts an empty session with that id if there is none.
    Create,
}

/// Why a Pi did not start.
#[derive(PartialEq)]
enum Unstarted {
    /// It exited, as Pi does when the session asked for has no file.
    Exited,
    /// Anything else: it could not be run, asked a question before it was ready, was too slow,
    /// or came up in another session.
    Failed,
}

/// A record to write to a Pi, by when, and who waits to hear what came of it.
struct Write {
    bytes: Vec<u8>,
    deadline: Instant,
    written: oneshot::Sender<Written>,
}

/// What came of a record handed to the task that writes to a Pi. It says so once, and only after
/// it is settled: a record not written in full by its deadline never reaches Pi whole, as the task
/// has dropped Pi's standard input, and its queue, before it says so.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Written {
    /// Every byte, the record's last newline included, is in the pipe.
    Yes,
    /// Pi's input closed: it has gone.
    Failed,
    /// Pi did not read it by the deadline: it has stopped reading.
    Stalled,
}

/// A running Pi.
struct Process {
    writes: mpsc::UnboundedSender<Write>,
    child: Option<Child>,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}

/// Stops Pi: its standard input closes, and the program gets SIGTERM, then SIGKILL if it has not
/// gone within [`STOP_GRACE`]. A script that runs Pi elsewhere cleans up on SIGTERM:
/// `tools/pi/pi.sh` kills its container, which SIGKILL would leave running whatever Pi was doing.
impl Drop for Process {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort(); // the writer holds Pi's standard input
        let Some(mut child) = self.child.take() else {
            return;
        };
        let pid = (child.id())
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw);
        if let Some(pid) = pid {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if tokio::time::timeout(STOP_GRACE, child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            });
        }
    }
}

/// One chat: its Pi, its translator, and what waits on Pi.
#[derive(Default)]
struct Chat {
    translator: Translator,
    process: Option<Process>,
    /// Which Pi the chat is on: lines from an earlier one are dropped.
    generation: u64,
    /// Controls waiting for their effect.
    effects: Vec<(RequestId, Effect)>,
    /// Commands sent that Pi has not responded to, by id.
    waiting: HashMap<String, Waiting>,
    /// Responses read, as Pi wrote them, for the request waiting on them, which feeds them to
    /// the translator once it has checked them.
    responses: HashMap<String, String>,
    /// Prompts Pi started a run for, or steered into a run not yet named: the next turn responds
    /// to them.
    pending: Vec<MessageId>,
    /// The prompts the running turn responds to.
    prompts: Vec<MessageId>,
    /// The turn `prompts` belong to.
    turn: Option<String>,
    /// The chat's Pi is starting: what it asks then cannot be answered, and a control waiting
    /// for the start is refused, not failed, if it does not come up.
    opening: bool,
    /// The starting Pi asked a question.
    asked_at_start: bool,
    /// The folder the chat's Pi was started in, when the chat chose one (D-101).
    folder: Option<PathBuf>,
}

/// A file the agent asked to send, waiting for the core to say how it went.
struct FileCall {
    chat: ChatId,
    /// The Pi that asked: an answer goes only to it.
    generation: u64,
    /// The id of the extension's dialog, which the answer names.
    dialog: String,
}

impl Chat {
    /// Takes a control off the list of those waiting for their effect; false if it was not there.
    fn unsettled(&mut self, request: &RequestId) -> bool {
        let before = self.effects.len();
        self.effects.retain(|(r, _)| r != request);
        self.effects.len() != before
    }
}

/// The Pi driver: a Pi for each chat of one agent.
pub struct Pi {
    setup: Setup,
    version: Option<String>,
    chats: HashMap<ChatId, Chat>,
    /// Every session a chat has had, and the chat: a session belongs to its chat for good.
    owners: HashMap<String, ChatId>,
    /// Sessions known to have held a message, so Pi has written their file.
    held: HashSet<String>,
    lines: mpsc::UnboundedReceiver<Line>,
    sender: mpsc::UnboundedSender<Line>,
    /// Events, and files to send, read and not yet handed to the core.
    ready: VecDeque<Output>,
    /// Files the agent asked to send that the core has not answered for, by the driver's id.
    files: HashMap<String, FileCall>,
    /// The folder that holds the driver's extension, removed with the driver.
    extension: tempfile::TempDir,
    next_id: u64,
    /// How long a request waits for its record to be written.
    patience: Duration,
}

impl Pi {
    /// Asks Pi its version, which RPC does not say, and is ready to start a Pi for each chat.
    pub async fn start(setup: Setup) -> io::Result<Self> {
        let mut command = Command::new(&setup.program);
        command
            .args(&setup.args)
            .arg("--version")
            .current_dir(&setup.cwd)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let output = command.output().await?;
        if !output.status.success() {
            return Err(io::Error::other("pi --version failed"));
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // A folder of the driver's own, which only its user can open, holds the extension.
        let extension = tempfile::Builder::new()
            .prefix("marmot-agent-pi-")
            .tempdir()?;
        std::fs::write(extension.path().join("send-file.ts"), EXTENSION)?;
        let (sender, lines) = mpsc::unbounded_channel();
        Ok(Self {
            setup,
            version: (!version.is_empty()).then_some(version),
            chats: HashMap::new(),
            owners: HashMap::new(),
            held: HashSet::new(),
            lines,
            sender,
            ready: VecDeque::new(),
            files: HashMap::new(),
            extension,
            next_id: 1,
            patience: PATIENCE,
        })
    }

    /// How long a request waits for its record to be written to a chat's Pi before that Pi is
    /// stopped: [`PATIENCE`] unless set.
    pub fn set_patience(&mut self, patience: Duration) {
        self.patience = patience;
    }

    fn id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        id.to_string()
    }

    /// Starts the chat's Pi, resuming `session` if one is named, and asks what the session item
    /// shows. Fails if Pi does not start, does not have the session, asks a question before it is
    /// ready, or does not answer within [`STARTUP_LIMIT`]; a Pi that failed is stopped.
    async fn spawn(
        &mut self,
        chat: &ChatId,
        session: Option<&str>,
        resume: Resume,
        folder: Option<&Path>,
    ) -> Result<(), Unstarted> {
        let extension = self.extension.path().join("send-file.ts");
        let mut command = self.setup.command(chat, folder, &extension);
        command
            .args(["--mode", "rpc"])
            .arg("--extension")
            .arg(&extension);
        if let Some(session) = session {
            let flag = match resume {
                Resume::Open => "--session",
                Resume::Create => "--session-id",
            };
            command.args([flag, session]);
        }
        let mut child = command.spawn().map_err(|_| Unstarted::Failed)?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(Unstarted::Failed);
        };
        let version = self.version.clone();
        let c = self.chats.entry(chat.clone()).or_default();
        c.generation += 1;
        c.translator.set_version(version);
        let (sender, generation, who) = (self.sender.clone(), c.generation, chat.clone());
        let reader = tokio::spawn(async move {
            // Split on LF only, as Pi's framing asks: tokio's lines do.
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(text)) = lines.next_line().await {
                let line = Line {
                    chat: who.clone(),
                    generation,
                    text: Some(text),
                };
                if sender.send(line).is_err() {
                    return;
                }
            }
            let _ = sender.send(Line {
                chat: who,
                generation,
                text: None,
            });
        });
        let (writes, queue) = mpsc::unbounded_channel::<Write>();
        let writer = tokio::spawn(write_records(stdin, queue));
        c.process = Some(Process {
            writes,
            child: Some(child),
            reader,
            writer,
        });
        c.opening = true;
        c.asked_at_start = false;
        c.folder = folder.map(Path::to_path_buf);
        let opened = tokio::time::timeout(STARTUP_LIMIT, self.open(chat, session)).await;
        let opened = opened.unwrap_or(Err(Unstarted::Failed));
        if opened.is_err() {
            self.stopped(chat);
        }
        let c = self.chats.get_mut(chat).expect("a chat");
        c.opening = false;
        opened?;
        let now = c.translator.session().map(String::from).expect("checked");
        if resume == Resume::Open && session.is_some() {
            self.held.insert(now.clone()); // Pi found its file
        }
        self.owners.insert(now, chat.clone());
        Ok(())
    }

    /// The questions about what the session item shows: the state, the models, the thinking
    /// levels and the commands.
    const OPEN: [&str; 4] = [
        "get_state",
        "get_available_models",
        "get_available_thinking_levels",
        "get_commands",
    ];

    /// Asks a starting Pi for what the session item shows, and waits for every answer. The state
    /// comes first, and must name a session, `wanted` if one is wanted. It is checked before the
    /// translator reads it: a Pi in another session must not replace the chat's.
    async fn open(&mut self, chat: &ChatId, wanted: Option<&str>) -> Result<(), Unstarted> {
        for command in Self::OPEN {
            let id = self.id();
            let record = json!({"id": id, "type": command});
            let response = self.request(chat, &id, &record).await?;
            if command == "get_state" {
                let state: Value = serde_json::from_str(&response).unwrap_or(Value::Null);
                let now = state["data"]["sessionId"].as_str();
                if now.is_none() || wanted.is_some_and(|w| now != Some(w)) {
                    return Err(Unstarted::Failed);
                }
            }
            self.feed(chat, Direction::In, &response, Vec::new());
        }
        Ok(())
    }

    /// Sends a command to a starting Pi and waits for its response, which it gives back unread.
    async fn request(
        &mut self,
        chat: &ChatId,
        id: &str,
        record: &Value,
    ) -> Result<String, Unstarted> {
        let text = record.to_string();
        let written = self.send(chat, Some((id, Waiting::Reply)), &text);
        let written = written.map_err(|_| Unstarted::Failed)?;
        match written.await.unwrap_or(Written::Failed) {
            Written::Yes => {}
            Written::Failed => return Err(Unstarted::Exited),
            Written::Stalled => return Err(Unstarted::Failed),
        }
        self.feed(chat, Direction::Out, &text, Vec::new());
        loop {
            let c = self.chats.get_mut(chat).expect("a chat that was sent to");
            if c.asked_at_start {
                return Err(Unstarted::Failed);
            }
            if let Some(response) = c.responses.remove(id) {
                return Ok(response);
            }
            if c.process.is_none() {
                return Err(Unstarted::Exited);
            }
            self.read().await;
        }
    }

    /// Hands a record to the chat's Pi, to be written within the patience. A command with an id
    /// waits as `waiting` says. What is given back says, once it is settled, what came of the
    /// record. The caller feeds the record to the translator once it is written.
    fn send(
        &mut self,
        chat: &ChatId,
        command: Option<(&str, Waiting)>,
        text: &str,
    ) -> Result<oneshot::Receiver<Written>, Refusal> {
        let deadline = Instant::now() + self.patience;
        let Some(c) = self.chats.get_mut(chat) else {
            return Err(Refusal::new(Reason::Failed));
        };
        let Some(process) = c.process.as_ref() else {
            return Err(Refusal::new(Reason::Failed));
        };
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(b'\n');
        let (written, done) = oneshot::channel();
        let write = Write {
            bytes,
            deadline,
            written,
        };
        if process.writes.send(write).is_err() {
            return Err(Refusal::new(Reason::Failed));
        }
        if let Some((id, waiting)) = command {
            c.waiting.insert(id.to_string(), waiting);
        }
        Ok(done)
    }

    /// Hands a record to the chat's Pi, waits until it is written, and only then feeds it to the
    /// chat's translator; the events it makes settle `settles`. If it could not be written,
    /// nothing waits for its response and the translator never sees it: an answer that did not
    /// reach Pi leaves its question waiting, to be closed unanswered when Pi's end is read.
    async fn hand(
        &mut self,
        chat: &ChatId,
        command: Option<(&str, Waiting)>,
        text: &str,
        settles: Vec<RequestId>,
    ) -> Result<(), Refusal> {
        self.write(chat, command, text).await?;
        self.feed(chat, Direction::Out, text, settles);
        Ok(())
    }

    /// Hands a record to the chat's Pi and waits to hear what came of it. If it was not written,
    /// nothing waits for its response.
    ///
    /// A Pi that has not read the record within the patience has stopped working (D-066,
    /// D-098): the task that writes to it has already let go of its input, so the record never
    /// reaches it or a later Pi. It is stopped, and what it left open is closed. The request is
    /// refused, and so does not also count as one of the controls the stop fails, or as a prompt
    /// Pi had not taken.
    async fn write(
        &mut self,
        chat: &ChatId,
        command: Option<(&str, Waiting)>,
        text: &str,
    ) -> Result<(), Refusal> {
        let id = command.as_ref().map(|(id, _)| id.to_string());
        let done = self.send(chat, command, text)?;
        // The writer says by the deadline; if it is gone, so is Pi.
        let written = done.await.unwrap_or(Written::Failed);
        if written == Written::Yes {
            return Ok(());
        }
        if let Some(c) = self.chats.get_mut(chat)
            && let Some(id) = id
        {
            c.waiting.remove(&id);
        }
        if written == Written::Stalled {
            self.stopped(chat); // Pi stopped reading
        }
        Err(Refusal::new(Reason::Failed))
    }

    /// Reads one line from any chat's Pi and handles it. The driver holds a sender, so the
    /// channel never closes.
    async fn read(&mut self) {
        if let Some(line) = self.lines.recv().await {
            self.handle(line);
        }
    }

    fn handle(&mut self, line: Line) {
        let chat = line.chat;
        let Some(c) = self.chats.get_mut(&chat) else {
            return;
        };
        if line.generation != c.generation || c.process.is_none() {
            return; // from a Pi the chat has let go
        }
        let Some(text) = line.text else {
            self.stopped(&chat);
            return;
        };
        let record: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if record["type"] == "extension_ui_request"
            && record["method"] == "input"
            && record["title"] == SEND_FILE_REQUEST
        {
            let generation = c.generation;
            self.file_call(&chat, generation, &record);
            return;
        }
        if c.opening
            && record["type"] == "extension_ui_request"
            && DIALOGS.iter().any(|d| record["method"] == *d)
        {
            // Pi 1.6.0 reads no answer before it is ready: the start has failed.
            c.asked_at_start = true;
            return;
        }
        let id = (record["type"] == "response")
            .then(|| record["id"].as_str())
            .flatten();
        let waiting = id.and_then(|id| c.waiting.remove(id));
        let taken = record["success"].as_bool() == Some(true)
            && record["data"]["cancelled"].as_bool() != Some(true);
        let mut settles = Vec::new();
        let mut then = None;
        match waiting {
            None => {}
            Some(Waiting::Reply) => {
                let id = id.expect("a response").to_string();
                c.responses.insert(id, text);
                return; // the request waiting for it feeds it to the translator
            }
            Some(Waiting::Prompt(message)) if taken => {
                match record["data"]["disposition"].as_str() {
                    // A run started for it. Pi says so after the run before it has ended, and
                    // before the new one reports anything: the next turn responds to it.
                    Some("started") => c.pending.push(message),
                    // Steered into the running turn, or one starting that is not yet named.
                    Some("queued") if c.turn.is_some() => c.prompts.push(message),
                    Some("queued") => c.pending.push(message),
                    _ => {} // an extension handled it, and no turn responds to it
                }
            }
            Some(Waiting::Prompt(_)) => {} // Pi refused it, and the refusal's error says so
            // A control Pi refused, or an extension cancelled, is settled by the error that says
            // so.
            Some(Waiting::Control(request, _)) if !taken => {
                if c.unsettled(&request) {
                    settles.push(request);
                }
            }
            Some(Waiting::Control(request, after)) => then = Some((request, after)),
        }
        self.feed(&chat, Direction::In, &text, settles);
        match then {
            Some((request, Then::ShowSession)) => {
                let c = self.chats.get_mut(&chat).expect("a chat that was fed");
                if !c.effects.iter().any(|(r, _)| *r == request) {
                    return; // settled by the edit Pi's own record made
                }
                let kind = format!("response to {}", record["command"].as_str().unwrap_or(""));
                let events = c.translator.show_session(&kind, &text);
                if !events.is_empty() {
                    c.unsettled(&request);
                }
                let mut settles = vec![request];
                for event in events {
                    self.ready_event(&chat, event, std::mem::take(&mut settles));
                }
            }
            Some((_, Then::Reopen)) => {
                // Pi takes commands as they come, so these go only once the new session is there.
                for command in Self::OPEN {
                    let id = self.id();
                    let record = json!({"id": id, "type": command}).to_string();
                    if self.send(&chat, None, &record).is_ok() {
                        self.feed(&chat, Direction::Out, &record, Vec::new());
                    }
                }
            }
            Some((_, Then::Nothing)) | None => {}
        }
    }

    /// Feeds a record to a chat's translator and makes its events ready.
    fn feed(&mut self, chat: &ChatId, direction: Direction, text: &str, settles: Vec<RequestId>) {
        let c = self.chats.get_mut(chat).expect("a chat that was fed");
        let events = c.translator.feed(direction, text);
        if let Some(session) = c.translator.session() {
            self.owners.insert(session.to_string(), chat.clone());
        }
        let mut settles = settles;
        for event in events {
            self.ready_event(chat, event, std::mem::take(&mut settles));
        }
    }

    /// Makes an event ready for the core, with the prompts a turn responds to and the controls
    /// whose effect it shows.
    fn ready_event(&mut self, chat: &ChatId, event: AgentEvent, mut settles: Vec<RequestId>) {
        let header = event.header();
        if let (Body::Turn(_), Some(session)) = (event.body(), &header.session) {
            self.held.insert(session.as_str().to_string()); // a run starts with a message
        }
        let c = self.chats.get_mut(chat).expect("a chat that was fed");
        let turn = header.turn.as_ref().map(|t| t.as_str().to_string());
        let ended = !matches!(header.state, State::Running | State::Waiting);
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        c.effects.retain(|(request, effect)| {
            let shown = match (effect, event.body()) {
                (Effect::TurnEnded(t), Body::Turn(_)) => ended && turn.as_deref() == Some(t),
                (Effect::Setting(id, value), Body::Session(s)) => (s.settings.iter())
                    .any(|setting| setting.id == *id && setting.value.as_ref() == Some(value)),
                (Effect::NewSession(old), Body::Session(_)) => {
                    header.state == State::Running && session != *old
                }
                // Only a compaction that was asked for: Pi says why on its end.
                (Effect::Compacted, Body::Notice(_)) => {
                    ended && header.item.as_str().starts_with("compaction:") && manual(&event)
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
                if c.turn.as_ref() != Some(&turn) {
                    c.turn = Some(turn);
                    c.prompts = std::mem::take(&mut c.pending);
                }
                if ended {
                    c.turn = None;
                    std::mem::take(&mut c.prompts)
                } else {
                    c.prompts.clone()
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

    /// The extension's `send_file` tool asks to send a file: the request goes to the core, or, if
    /// it names no path, is answered at once, since the tool waits for the answer. A path the agent
    /// gives relative to the folder Pi works in is made whole here: the core takes only an absolute
    /// one. An absolute path stays as it is, and any `..` is left for the core to follow.
    fn file_call(&mut self, chat: &ChatId, generation: u64, record: &Value) {
        let dialog = record["id"].as_str().unwrap_or_default().to_string();
        let asked: Value = (record["placeholder"].as_str())
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or(Value::Null);
        let Some(path) = asked["path"].as_str().filter(|p| !p.is_empty()) else {
            let refused = Err(Refusal {
                reason: Reason::Invalid,
                detail: Some("it was not given a path.".into()),
                unanswered: false,
            });
            let reply = file_reply(&dialog, &refused);
            let _ = self.send(chat, None, &reply); // a Pi that has gone needs no answer
            return;
        };
        let c = self.chats.get(chat).expect("a chat that was read");
        let folder = c.folder.clone().unwrap_or_else(|| self.setup.cwd.clone());
        let id = format!("file-{}", self.id());
        let call = FileCall {
            chat: chat.clone(),
            generation,
            dialog,
        };
        self.files.insert(id.clone(), call);
        self.ready.push_back(Output::SendFile(SendFile {
            id,
            chat: chat.clone(),
            path: folder.join(path),
        }));
    }

    /// A chat's Pi has gone, or is let go: its translator closes what it can. Nothing Pi sends can
    /// show the effect of a control still waiting for one, so each such control has failed, and an
    /// error the driver makes settles it; so does a prompt Pi had not yet taken. The events the
    /// driver makes here settle nothing else. The chat keeps its session, and the next prompt
    /// resumes it in a new Pi. A Pi that stops while it starts leaves its controls to the request
    /// that started it, which is refused.
    fn stopped(&mut self, chat: &ChatId) {
        let c = self.chats.get_mut(chat).expect("a chat");
        if c.process.take().is_none() {
            return;
        }
        // The tool that asked has gone with its Pi: nothing waits for how its file went.
        self.files.retain(|_, call| call.chat != *chat);
        let c = self.chats.get_mut(chat).expect("a chat");
        let untaken =
            !c.pending.is_empty() || (c.waiting.values()).any(|w| matches!(w, Waiting::Prompt(_)));
        c.waiting.clear();
        c.responses.clear();
        c.pending.clear();
        let failed: Vec<RequestId> = if c.opening {
            Vec::new()
        } else {
            c.effects.drain(..).map(|(r, _)| r).collect()
        };
        let events = c.translator.harness_stopped();
        let error = (!failed.is_empty() || untaken).then(|| c.translator.stopped_before_acting());
        for event in events {
            self.ready_event(chat, event, Vec::new());
        }
        if let Some(error) = error {
            self.ready_event(chat, error, failed);
        }
    }

    /// The chat a session belongs to, among the chats that have it now.
    fn chat_of(&self, session: &str) -> Option<ChatId> {
        (self.chats.iter())
            .find(|(_, c)| c.translator.session() == Some(session))
            .map(|(chat, _)| chat.clone())
    }

    /// Makes sure the chat has a Pi on the session it should have: `wanted` if the core names one
    /// to resume, or else the chat's own, started again if its Pi stopped. A Pi started here works
    /// in `folder`, the chat's (D-101).
    async fn ready_for(
        &mut self,
        chat: &ChatId,
        wanted: Option<&str>,
        folder: Option<&Path>,
    ) -> Result<(), Refusal> {
        let c = self.chats.entry(chat.clone()).or_default();
        let current = c.translator.session().map(String::from);
        if c.process.is_some() {
            match wanted {
                None => return Ok(()),
                Some(w) if current.as_deref() == Some(w) => return Ok(()),
                Some(_) if c.translator.running() => return Err(Refusal::new(Reason::Busy)),
                // Pi holds one session: the one asked for is resumed in a new Pi.
                Some(_) => self.stopped(chat),
            }
        }
        let Some(resume) = wanted.map(String::from).or(current) else {
            return (self.spawn(chat, None, Resume::Open, folder).await)
                .map_err(|_| Refusal::new(Reason::Failed));
        };
        // Pi writes a session's file only once it holds a message. A session that never did, such
        // as a new one the core restarted after, is made again, empty, with its id. One known to
        // have held a message is not: its history is gone.
        let mut started = self.spawn(chat, Some(&resume), Resume::Open, folder).await;
        if started == Err(Unstarted::Exited) && !self.held.contains(&resume) {
            started = (self.spawn(chat, Some(&resume), Resume::Create, folder)).await;
        }
        started.map_err(|_| Refusal::new(Reason::Failed))
    }

    /// Sends a control that settles once its effect shows. The effect waits only once the control
    /// has reached Pi: one that did not is refused, and leaves nothing to clean up.
    async fn effect(
        &mut self,
        chat: &ChatId,
        request: &RequestId,
        effect: Effect,
        record: Value,
        then: Then,
    ) -> Result<(), Refusal> {
        let id = self.id();
        let mut record = record;
        record["id"] = Value::String(id.clone());
        let record = record.to_string();
        let waiting = Waiting::Control(request.clone(), then);
        self.write(chat, Some((&id, waiting)), &record).await?;
        let c = self
            .chats
            .get_mut(chat)
            .expect("a chat that was written to");
        c.effects.push((request.clone(), effect));
        self.feed(chat, Direction::Out, &record, Vec::new());
        Ok(())
    }
}

/// Writes the records queued for a Pi, in order, each by its deadline, and says what came of
/// each once it is settled. On the first that is not written in full, Pi's input and the queue
/// are dropped before anyone hears, so nothing more ever reaches that Pi.
async fn write_records<W: AsyncWrite + Unpin>(
    mut stdin: W,
    mut queue: mpsc::UnboundedReceiver<Write>,
) {
    while let Some(write) = queue.recv().await {
        let writing = async {
            stdin.write_all(&write.bytes).await?;
            stdin.flush().await
        };
        // The write is polled before the deadline: one that finishes at the deadline
        // counts. One that does not is dropped, so its last newline never goes.
        let written = match tokio::time::timeout_at(write.deadline, writing).await {
            Ok(Ok(())) => Written::Yes,
            Ok(Err(_)) => Written::Failed,
            Err(_) => Written::Stalled,
        };
        if written != Written::Yes {
            // Nothing more can reach this Pi before anyone hears, nor anything queued:
            // those waiting for it hear that it failed. If Pi has gone, its output ends
            // too, and the reader says so.
            drop(stdin);
            drop(queue);
            let _ = write.written.send(written);
            return;
        }
        let _ = write.written.send(written);
    }
}

/// The driver's answer to the `send_file` tool's dialog: whether the file was sent, and if not, why,
/// in words the model reads, as the tool gives them.
fn file_reply(dialog: &str, result: &Result<(), Refusal>) -> String {
    let (sent, text) = match result {
        Ok(()) => (true, "Sent.".to_string()),
        Err(refusal) => match &refusal.detail {
            Some(detail) => (false, format!("Not sent: {detail}")),
            None => (false, "Not sent.".to_string()),
        },
    };
    let value = json!({"sent": sent, "text": text}).to_string();
    json!({"type": "extension_ui_response", "id": dialog, "value": value}).to_string()
}

/// Whether an event is the end of a compaction that was asked for, as `compaction_end` says.
fn manual(event: &AgentEvent) -> bool {
    event.native().is_some_and(|native| {
        let record: Value = serde_json::from_str(native.payload.get()).unwrap_or(Value::Null);
        record["type"] == "compaction_end" && record["reason"] == "manual"
    })
}

/// What Pi reads for a chat message. Pi has one user per session, so in a group the text names
/// who wrote it, and a reply quotes the message it answers, as for Codex (D-062). A slash command
/// goes exactly as typed (D-031).
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

impl Driver for Pi {
    fn harness(&self) -> Harness {
        Harness {
            name: "pi".into(),
            version: self.version.clone(),
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: true,
            // A message sent during a turn is steered into it, before Pi's next model call.
            mid_turn: MidTurn::Fold,
            sessions: Sessions::Core,
            resume: true,
            // Pi asks nobody before it acts.
            approvals: false,
            files_in: FilesIn::None,
            approval_commands: Vec::new(),
        }
    }

    fn folder(&self) -> Option<PathBuf> {
        Some(self.setup.cwd.clone())
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let chat = prompt.chat.id.clone();
        if !prompt.attachments.is_empty() {
            return Err(Refusal::new(Reason::NotSupported));
        }
        // The core names a session only to resume it (D-065); one that belongs to another chat
        // would show its history here.
        let wanted = prompt.session.as_ref().map(|s| s.as_str().to_string());
        if let Some(owner) = wanted.as_ref().and_then(|w| self.owners.get(w))
            && *owner != chat
        {
            return Err(Refusal::new(Reason::Invalid));
        }
        let folder = prompt.chat.folder.clone();
        self.ready_for(&chat, wanted.as_deref(), folder.as_deref())
            .await?;
        let id = self.id();
        // Steering is ignored while Pi is idle, and needed while it runs: always ask for it, so
        // a prompt sent as a run starts or ends is never refused.
        let record = json!({
            "id": id,
            "type": "prompt",
            "message": text(&prompt),
            "streamingBehavior": "steer",
        });
        // Pi's response says which turn responds to it.
        let waiting = Waiting::Prompt(prompt.message.clone());
        (self.hand(&chat, Some((&id, waiting)), &record.to_string(), Vec::new())).await
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        let mut refusal = Refusal::new(Reason::UnknownItem);
        let mut found = None;
        for (chat, c) in &self.chats {
            match c.translator.reply(&answer) {
                Ok(reply) => {
                    found = Some((chat.clone(), reply));
                    break;
                }
                Err(r) if r.reason == Reason::UnknownItem => {}
                Err(r) => refusal = r,
            }
        }
        let (chat, reply) = found.ok_or(refusal)?;
        // The question's next edit, made from the reply, shows the answer: it settles it.
        self.hand(&chat, None, &reply, vec![answer.request]).await
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        let request = control.request.clone();
        match &control.action {
            Action::Interrupt { turn } => {
                let chat = (self.chats.iter())
                    .find(|(_, c)| c.translator.turn() == Some(turn.as_str()))
                    .map(|(chat, _)| chat.clone())
                    .ok_or_else(|| Refusal::new(Reason::NothingToDo))?;
                // Pi responds once the turn has stopped; the turn's end settles the control.
                let effect = Effect::TurnEnded(turn.as_str().to_string());
                let record = json!({"type": "abort"});
                (self.effect(&chat, &request, effect, record, Then::Nothing)).await
            }
            Action::NewSession { chat, session } => {
                let folder = chat.folder.clone();
                let chat = chat.id.clone();
                let c = self.chats.entry(chat.clone()).or_default();
                if c.process.is_some() && c.translator.running() {
                    return Err(Refusal::new(Reason::Busy));
                }
                // Two waiting at once would both be settled by the first switch, which is a
                // session other than the old one for each: one at a time.
                if (c.effects.iter()).any(|(_, e)| matches!(e, Effect::NewSession(_))) {
                    return Err(Refusal::new(Reason::Busy));
                }
                // Only a session other than the one the chat is in shows the effect. The core
                // names the session it saved, and none before it has seen one, so the driver's own
                // comes first: an edit of the chat's session must not settle the request.
                let old = (c.translator.session().map(str::to_string))
                    .or_else(|| session.as_ref().map(|s| s.as_str().to_string()));
                // Pi works where it was started: a session in another folder needs a new Pi
                // there (D-101). The Pi it replaces is let go, as one that stopped.
                if c.process.is_some() && c.folder != folder {
                    self.stopped(&chat);
                }
                let c = self.chats.get_mut(&chat).expect("a chat");
                let effect = Effect::NewSession(old);
                if c.process.is_none() {
                    // No Pi to ask: a new one starts a new session.
                    c.effects.push((request.clone(), effect));
                    let started = (self.spawn(&chat, None, Resume::Open, folder.as_deref())).await;
                    if started.is_err() {
                        let c = self.chats.get_mut(&chat).expect("a chat");
                        c.unsettled(&request);
                    }
                    return started.map_err(|_| Refusal::new(Reason::Failed));
                }
                // An extension can ask the chat before Pi switches sessions, and the answer can
                // come only once this request has returned: Pi's response is read later. A
                // refusal or a cancel settles the control; a new session's item does otherwise.
                let record = json!({"type": "new_session"});
                (self.effect(&chat, &request, effect, record, Then::Reopen)).await
            }
            Action::Set {
                session,
                setting,
                value,
            } => {
                let chat = self
                    .chat_of(session.as_str())
                    .ok_or_else(|| Refusal::new(Reason::UnknownItem))?;
                let record = match setting.as_str() {
                    "model" => {
                        let (provider, model) = value
                            .split_once('/')
                            .ok_or_else(|| Refusal::new(Reason::Invalid))?;
                        json!({"type": "set_model", "provider": provider, "modelId": model})
                    }
                    "thinking" => json!({"type": "set_thinking_level", "level": value}),
                    _ => return Err(Refusal::new(Reason::NotSupported)),
                };
                let effect = Effect::Setting(setting.clone(), value.clone());
                (self.effect(&chat, &request, effect, record, Then::ShowSession)).await
            }
            Action::Own { session, id } => {
                if id != "compact" {
                    return Err(Refusal::new(Reason::NotSupported));
                }
                let chat = self
                    .chat_of(session.as_str())
                    .ok_or_else(|| Refusal::new(Reason::UnknownItem))?;
                // A second compaction would make Pi abort the first: one at a time.
                let c = self.chats.get_mut(&chat).expect("a chat");
                if (c.effects.iter()).any(|(_, e)| matches!(e, Effect::Compacted)) {
                    return Err(Refusal::new(Reason::Busy));
                }
                // Pi responds once the summary is written, which can take a while: the request
                // returns now, and the compaction's end settles it.
                let record = json!({"type": "compact"});
                (self.effect(&chat, &request, Effect::Compacted, record, Then::Nothing)).await
            }
        }
    }

    async fn next_event(&mut self) -> Option<Output> {
        loop {
            if let Some(output) = self.ready.pop_front() {
                return Some(output);
            }
            self.read().await;
        }
    }

    /// The core says how a file the agent asked to send went: the tool's dialog is answered, in
    /// the Pi that asked, if it is still running. A Pi that has stopped reading is stopped, as for
    /// any record (D-098).
    async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
        let Some(call) = self.files.remove(&id) else {
            return;
        };
        let current = self.chats.get(&call.chat).map(|c| c.generation);
        if current != Some(call.generation) {
            return;
        }
        let reply = file_reply(&call.dialog, &result);
        let _ = self.write(&call.chat, None, &reply).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// A Pi that reads again right at a record's deadline either gets the record whole, and the
    /// writer says so, or never gets its last newline, nor anything queued after it, and the
    /// writer says it was not written. Pi starts reading from a few milliseconds before the
    /// deadline to just after it, so runs land on either side of the race and on it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_record_is_written_whole_or_never_reaches_pi() {
        let mut record = vec![b'x'; 63];
        record.push(b'\n');
        for run in 0..60u64 {
            let (pi, mut reads) = tokio::io::duplex(16); // the pipe holds less than the record
            let (writes, queue) = mpsc::unbounded_channel();
            let writer = tokio::spawn(write_records(pi, queue));
            let deadline = Instant::now() + Duration::from_millis(20);
            let (written, first) = oneshot::channel();
            let (also, second) = oneshot::channel();
            let bytes = record.clone();
            writes
                .send(Write {
                    bytes,
                    deadline,
                    written,
                })
                .unwrap();
            let bytes = b"after\n".to_vec();
            (writes.send(Write {
                bytes,
                deadline: deadline + Duration::from_secs(5),
                written: also,
            }))
            .unwrap();
            // Pi reads again at the deadline, until its input ends or both records are in.
            let reader = tokio::spawn(async move {
                let early = Duration::from_micros(500 * (run % 6));
                tokio::time::sleep_until(deadline + Duration::from_micros(500) - early).await;
                let mut got = Vec::new();
                let mut buf = [0u8; 64];
                while got.iter().filter(|b| **b == b'\n').count() < 2 {
                    match reads.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                got
            });
            let first = first.await.unwrap_or(Written::Failed);
            let second = second.await.unwrap_or(Written::Failed);
            drop(writes);
            let got = reader.await.unwrap();
            writer.await.unwrap();
            if first == Written::Yes {
                assert_eq!(second, Written::Yes);
                assert_eq!(got, [record.as_slice(), b"after\n"].concat());
            } else {
                assert_eq!(first, Written::Stalled);
                assert_ne!(second, Written::Yes);
                assert!(
                    !got.contains(&b'\n'),
                    "a record not written reached Pi whole"
                );
            }
        }
    }
}
