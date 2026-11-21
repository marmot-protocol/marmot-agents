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

use crate::translate::DIALOGS;
use crate::{Direction, Translator};
use marmot_agent_driver::{
    Action, Answer, Capabilities, ChatId, Control, Driver, Emitted, FilesIn, Harness, MessageId,
    MidTurn, Prompt, Reason, Refusal, RequestId, Sessions,
};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// The environment variable that names the chat a Pi serves, as a hash of the chat's id in
/// lowercase hex. A script that runs Pi elsewhere, such as `tools/pi/pi.sh`, keeps each chat's
/// sessions apart by it.
pub const CHAT_VARIABLE: &str = "MARMOT_AGENT_CHAT";

/// How long a Pi that was asked to stop has before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// How long a starting Pi has to answer what the session item shows. Pi 1.6.0 reads its standard
/// input only once its extensions' start-up handlers have finished, so one that waits for an
/// answer would never let it answer.
pub const STARTUP_LIMIT: Duration = Duration::from_secs(60);

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
    fn command(&self, chat: &ChatId) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .env(CHAT_VARIABLE, chat_key(chat))
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

/// A record to write to a Pi, and who waits until it is written.
struct Write {
    bytes: Vec<u8>,
    written: oneshot::Sender<bool>,
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
    /// Events read and not yet handed to the core.
    ready: VecDeque<Emitted>,
    next_id: u64,
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
            next_id: 1,
        })
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
    ) -> Result<(), Unstarted> {
        let mut command = self.setup.command(chat);
        command.args(["--mode", "rpc"]);
        if let Some(session) = session {
            let flag = match resume {
                Resume::Open => "--session",
                Resume::Create => "--session-id",
            };
            command.args([flag, session]);
        }
        let mut child = command.spawn().map_err(|_| Unstarted::Failed)?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
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
        let (writes, mut queue) = mpsc::unbounded_channel::<Write>();
        let writer = tokio::spawn(async move {
            while let Some(write) = queue.recv().await {
                let written = async {
                    stdin.write_all(&write.bytes).await?;
                    stdin.flush().await
                };
                let ok = written.await.is_ok();
                let _ = write.written.send(ok);
                if !ok {
                    return; // Pi has gone: its output ends too, and the reader says so
                }
            }
        });
        c.process = Some(Process {
            writes,
            child: Some(child),
            reader,
            writer,
        });
        c.opening = true;
        c.asked_at_start = false;
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
        if !matches!(written.await, Ok(true)) {
            return Err(Unstarted::Exited);
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

    /// Hands a record to the chat's Pi. A command with an id waits as `waiting` says. What is
    /// given back says when the record has been written, and whether it could be. The caller
    /// feeds the record to the translator once it is written.
    fn send(
        &mut self,
        chat: &ChatId,
        command: Option<(&str, Waiting)>,
        text: &str,
    ) -> Result<oneshot::Receiver<bool>, Refusal> {
        let Some(c) = self.chats.get_mut(chat) else {
            return Err(Refusal::new(Reason::Failed));
        };
        let Some(process) = c.process.as_ref() else {
            return Err(Refusal::new(Reason::Failed));
        };
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(b'\n');
        let (written, done) = oneshot::channel();
        if process.writes.send(Write { bytes, written }).is_err() {
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
        let id = command.as_ref().map(|(id, _)| id.to_string());
        let done = self.send(chat, command, text)?;
        if matches!(done.await, Ok(true)) {
            self.feed(chat, Direction::Out, text, settles);
            return Ok(());
        }
        if let Some(c) = self.chats.get_mut(chat)
            && let Some(id) = id
        {
            c.waiting.remove(&id);
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
        self.ready.push_back(Emitted {
            event,
            chat: Some(chat.clone()),
            prompts,
            settles,
        });
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
    /// to resume, or else the chat's own, started again if its Pi stopped.
    async fn ready_for(&mut self, chat: &ChatId, wanted: Option<&str>) -> Result<(), Refusal> {
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
            return (self.spawn(chat, None, Resume::Open).await)
                .map_err(|_| Refusal::new(Reason::Failed));
        };
        // Pi writes a session's file only once it holds a message. A session that never did, such
        // as a new one the core restarted after, is made again, empty, with its id. One known to
        // have held a message is not: its history is gone.
        let mut started = self.spawn(chat, Some(&resume), Resume::Open).await;
        if started == Err(Unstarted::Exited) && !self.held.contains(&resume) {
            started = self.spawn(chat, Some(&resume), Resume::Create).await;
        }
        started.map_err(|_| Refusal::new(Reason::Failed))
    }

    /// Sends a control that settles once its effect shows, and forgets the effect if the control
    /// never reached Pi.
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
        let c = self.chats.get_mut(chat).expect("a chat");
        c.effects.push((request.clone(), effect));
        let waiting = Waiting::Control(request.clone(), then);
        let sent = (self.hand(chat, Some((&id, waiting)), &record.to_string(), Vec::new())).await;
        if sent.is_err()
            && let Some(c) = self.chats.get_mut(chat)
        {
            c.unsettled(request);
        }
        sent
    }
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
        self.ready_for(&chat, wanted.as_deref()).await?;
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
                let chat = chat.id.clone();
                let old = session.as_ref().map(|s| s.as_str().to_string());
                let c = self.chats.entry(chat.clone()).or_default();
                if c.process.is_some() && c.translator.running() {
                    return Err(Refusal::new(Reason::Busy));
                }
                let effect = Effect::NewSession(old);
                if c.process.is_none() {
                    // No Pi to ask: a new one starts a new session.
                    c.effects.push((request.clone(), effect));
                    let started = self.spawn(&chat, None, Resume::Open).await;
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

    async fn next_event(&mut self) -> Option<Emitted> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Some(event);
            }
            self.read().await;
        }
    }
}
