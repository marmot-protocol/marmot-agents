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

use crate::{Direction, Translator};
use marmot_agent_driver::{
    Action, Answer, Capabilities, ChatId, Control, Driver, Emitted, FilesIn, Harness, MessageId,
    MidTurn, Prompt, Reason, Refusal, RequestId, Sessions,
};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, Command};

type Reader = Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

/// What a request the driver sent is for.
enum Purpose {
    /// Nothing follows from it but what the chat's translator reads in the reply.
    Plain,
    /// A prompt: the turn it starts responds to this message.
    Prompt(MessageId),
    /// A control, settled by its effect or by the agent refusing it.
    Control(RequestId),
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
}

/// One chat: its translator, the controls waiting for their effect, and the request picking its
/// session, while one is waiting.
struct Chat {
    translator: Translator,
    effects: Vec<(RequestId, Effect)>,
    picking: Option<String>,
}

/// The ACP driver: one agent process, serving every chat of one agent.
pub struct Acp {
    reader: Reader,
    writer: Writer,
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
    /// Events read and not yet handed to the core.
    ready: VecDeque<Emitted>,
    /// The agent has gone.
    gone: bool,
    /// Messages for no chat.
    unrouted: u64,
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
        // Request ids start from the time, so a turn, which is named after the request that
        // started it, never takes the name of a turn from before a restart.
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
        let mut acp = Self {
            reader: BufReader::new(output).lines(),
            writer: Box::new(input),
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
        let reply = loop {
            let Some(line) = acp.reader.next_line().await? else {
                return Err(io::Error::other("the agent ended before it replied"));
            };
            let message: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if message["id"] == id && message.get("method").is_none() {
                break (line, message);
            }
        };
        let (line, message) = reply;
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
        acp.greeting = vec![(Direction::Out, hello), (Direction::In, line)];
        Ok(acp)
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

    async fn write(&mut self, text: &str) -> io::Result<()> {
        self.writer.write_all(text.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await
    }

    /// Sends a message for a chat and feeds it to that chat's translator; the events it makes
    /// settle `settles`. A request is kept, with what it is for, until the agent answers it.
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
        if self.write(text).await.is_err() {
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
        }
        self.send(chat, Some((id.clone(), purpose)), &text, Vec::new())
            .await?;
        Ok(id)
    }

    /// Reads one message from the agent and handles it. False once the agent has gone.
    async fn read(&mut self) -> bool {
        if self.gone {
            return false;
        }
        match self.reader.next_line().await {
            Ok(Some(line)) => {
                self.route(&line).await;
                true
            }
            Ok(None) | Err(_) => {
                self.stopped();
                false
            }
        }
    }

    /// Gives a message from the agent to the chat it is for, and answers the agent's requests
    /// that nobody in the chat is asked about.
    async fn route(&mut self, line: &str) {
        let message: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            let session = message["params"]["sessionId"].as_str().map(String::from);
            let chat = session.as_ref().and_then(|s| self.sessions.get(s)).cloned();
            match chat {
                Some(chat) => self.feed(&chat, Direction::In, line, Vec::new()),
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
        }
        if let Some(c) = self.chats.get_mut(&chat)
            && c.picking == id
        {
            c.picking = None;
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
        if before != after {
            if let Some(old) = before {
                self.sessions.remove(&old);
            }
            if let Some(new) = after {
                self.owners.insert(new.clone(), chat.clone());
                self.sessions.insert(new, chat.clone());
            }
        }
        let mut settles = settles;
        for event in events {
            self.ready_event(
                chat,
                event,
                std::mem::take(&mut settles),
                true,
                turn_request.as_deref(),
            );
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
                    (Effect::Setting(id, value), Body::Session(s)) => (s.settings.iter())
                        .any(|setting| setting.id == *id && setting.value.as_ref() == Some(value)),
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
        self.ready.push_back(Emitted {
            event,
            chat: Some(chat.clone()),
            prompts,
            settles,
        });
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
            },
        );
    }

    fn translator(&self, chat: &ChatId) -> Option<&Translator> {
        self.chats.get(chat).map(|c| &c.translator)
    }

    /// Reads on until no session is being picked for the chat.
    async fn picked(&mut self, chat: &ChatId) -> Result<(), Refusal> {
        while self.chats.get(chat).is_some_and(|c| c.picking.is_some()) {
            if !self.read().await {
                return Err(Refusal::new(Reason::Failed));
            }
        }
        Ok(())
    }

    /// Makes `session` the chat's session, starting a new one if it is none, and waits for the
    /// agent. A session is taken up with `session/resume` when the agent offers it, which replays
    /// nothing, and otherwise with `session/load`, whose replay the translator does not send again.
    async fn pick(&mut self, chat: &ChatId, session: Option<&str>) -> Result<(), Refusal> {
        let params = |id: Option<&str>| {
            let mut p = json!({"cwd": self.cwd, "mcpServers": []});
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
            (Some(now), Some(wanted)) if now == wanted => {
                self.stop_what_was_taken_up(chat).await;
                Ok(())
            }
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

    /// After a session is taken up again: an agent can carry on, on its own, with the turn a
    /// restart cut off (Goose does, and then refuses a prompt until it ends). A restart closes that
    /// turn (D-070), so the driver cancels whatever the agent took up and reads on until it has been
    /// quiet for a moment. ACP has no message that says a session is idle.
    async fn stop_what_was_taken_up(&mut self, chat: &ChatId) {
        if self.cancel(chat).await.is_err() {
            return;
        }
        let quiet = Duration::from_secs(2);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            // Reading a line is safe to give up on: nothing of a line is lost.
            match tokio::time::timeout(quiet, self.reader.next_line()).await {
                Err(_) => return,
                Ok(Ok(Some(line))) => self.route(&line).await,
                Ok(_) => {
                    self.stopped();
                    return;
                }
            }
        }
    }

    /// Cancels the chat's turn: tells the agent nobody will answer what it is waiting on, then
    /// asks it to stop (ACP's `session/cancel`).
    async fn cancel(&mut self, chat: &ChatId) -> Result<(), Refusal> {
        let Some(t) = self.translator(chat) else {
            return Err(Refusal::new(Reason::NothingToDo));
        };
        let (replies, session) = (t.cancel_replies(), t.session().map(String::from));
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
        // A message during a turn restarts it (`mid_turn: restart`).
        if self.translator(&chat).and_then(Translator::turn).is_some() {
            self.cancel(&chat).await?;
            while self.translator(&chat).and_then(Translator::turn).is_some() {
                if !self.read().await {
                    return Err(Refusal::new(Reason::Failed));
                }
            }
        }
        let current = self
            .translator(&chat)
            .and_then(Translator::session)
            .map(String::from);
        if current.is_none() || (wanted.is_some() && wanted != current) {
            self.pick(&chat, wanted.as_deref()).await?;
        }
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
                let chat = chat.id.clone();
                self.chat(&chat);
                self.picked(&chat).await?;
                // A session is replaced only when it is idle.
                if self.translator(&chat).and_then(Translator::turn).is_some() {
                    return Err(Refusal::new(Reason::Busy));
                }
                let old = session.as_ref().map(|s| s.as_str().to_string());
                if let Some(c) = self.chats.get_mut(&chat) {
                    c.effects.push((request.clone(), Effect::NewSession(old)));
                }
                let params = json!({"cwd": self.cwd, "mcpServers": []});
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
                if let Some(c) = self.chats.get_mut(&chat) {
                    c.effects.push((
                        request.clone(),
                        Effect::Setting(setting.clone(), value.clone()),
                    ));
                }
                let params =
                    json!({"sessionId": session.as_str(), "configId": setting, "value": value});
                self.request(
                    &chat,
                    "session/set_config_option",
                    params,
                    Purpose::Control(request),
                )
                .await?;
                Ok(())
            }
            // An ACP session offers no actions of its own.
            Action::Own { .. } => Err(Refusal::new(Reason::UnknownItem)),
        }
    }

    async fn next_event(&mut self) -> Option<Emitted> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Some(event);
            }
            if !self.read().await {
                return self.ready.pop_front();
            }
        }
    }
}
