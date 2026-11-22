//! The transport: runs `codex app-server`, gives each chat a translator of its own, and is the
//! driver the core holds.
//!
//! Each message from Codex goes to the chat whose session it names, or, for a reply, to the chat
//! that made the request (D-060). Each message the driver sends is fed to that chat's translator
//! as well, so the translator learns what was sent the same way whoever sent it. Every request the
//! driver sends is kept, with what it is for, until Codex answers it; an answer or a control is
//! settled once, by the event that shows its effect or by Codex refusing it.
//!
//! When a translator refuses a request as busy, the driver waits only if the wait ends without
//! anyone in the chat doing anything: Codex has yet to answer a request before it. Otherwise the
//! request is refused as busy, so the core can go on, for instance to answer an approval.
//!
//! Every session the driver starts is given one tool of the driver's own, `send_file`, so the
//! agent can send a file to the chat (D-090). When Codex calls it, the driver hands the core the
//! file to send, and replies to Codex once the core says how it went.
//!
//! Subagents are switched off when Codex starts: how a chat shows them is a proposal not yet
//! decided (`docs/design/subagents.md`), and without them an approval from a subagent would have
//! nowhere to go.

use crate::{
    Direction, SEND_FILE, Setup, Translator, collaboration_mode_list, initialize, initialized,
    model_list, tool_reply,
};
use marmot_agent_driver::{
    Action, Answer, Capabilities, ChatId, Control, Driver, Emitted, FilesIn, Harness, MessageId,
    MidTurn, Output, Prompt, Reason, Refusal, RequestId, SendFile, Sessions,
};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, Command};

type Reader = Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>;

/// How long one of the core's requests waits for Codex before the driver gives up on it: to read
/// what the driver writes, to pick a session, or to answer a request before it.
const PATIENCE: Duration = Duration::from_secs(60);
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

/// What a request the driver sent is for.
enum Purpose {
    /// Nothing follows from it: a list of models or modes, or picking a session, which the
    /// chat's translator follows.
    Plain,
    /// A prompt: a new turn, or, with the turn, one joined to a running turn.
    Prompt(MessageId, Option<String>),
    /// A control, settled by its effect or by Codex refusing it; with the effect, until the
    /// message is written and the effect starts to be watched for.
    Control(RequestId, Option<Effect>),
}

/// A message for Codex, and what follows once all of it is written. Nothing follows a message
/// that was not written: an answer Codex never received is not shown as given.
struct Frame {
    bytes: Vec<u8>,
    written: usize,
    then: Option<Written>,
}

/// What follows a message for a chat once it is written.
struct Written {
    chat: ChatId,
    text: String,
    request: Option<(i64, Purpose)>,
    settles: Vec<RequestId>,
}

/// What shows that a control has had its effect.
#[derive(Clone)]
enum Effect {
    /// The turn it interrupted has ended.
    TurnEnded(String),
    /// The session item shows this setting with this value.
    Setting(String, String),
    /// A turn started by the driver's request with this id has started.
    TurnStartedBy(i64),
    /// A session other than this one is running.
    NewSession(Option<String>),
}

/// One chat: its translator, and the controls waiting for their effect.
#[derive(Default)]
struct Chat {
    translator: Translator,
    effects: Vec<(RequestId, Effect)>,
}

/// The Codex driver: one `codex app-server` process, serving every chat of one agent.
pub struct Codex {
    reader: Reader,
    writer: Writer,
    /// Messages to write to Codex, in order. Each stays here until all of it is written, so one
    /// cut short when a future is dropped is finished, whole, by the next write, and only then
    /// does what follows from it happen.
    unwritten: VecDeque<Frame>,
    /// What follows messages written and not yet flushed: it happens once the flush succeeds, so
    /// a writer that buffers and then fails does not leave an answer shown as given.
    unflushed: Vec<Written>,
    /// The process, when the driver started it. It is killed when the driver is dropped.
    child: Option<Child>,
    setup: Setup,
    version: Option<String>,
    next_id: i64,
    chats: HashMap<ChatId, Chat>,
    /// Each chat's session, by Codex thread.
    threads: HashMap<String, ChatId>,
    /// Every thread a chat has had, and the chat: a session belongs to its chat for good.
    owners: HashMap<String, ChatId>,
    /// The driver's requests Codex has not answered: the chat each is for, and what for.
    requests: HashMap<i64, (ChatId, Purpose)>,
    /// The prompt behind each `turn/start` sent, until its turn starts or Codex refuses it. It is
    /// kept from the moment it is sent: Codex can announce the turn before it replies.
    starting: HashMap<i64, MessageId>,
    /// The prompts each running turn responds to.
    prompts: HashMap<(ChatId, String), Vec<MessageId>>,
    /// Messages naming a thread no chat has yet, kept while a session is being picked.
    early: Vec<String>,
    /// Events, and files to send, read and not yet handed to the core.
    ready: VecDeque<Output>,
    /// Codex's calls of the driver's own tool that the core has not answered yet: the chat each
    /// came from, and Codex's id for it, by the id the core was given.
    calls: HashMap<String, (ChatId, Value)>,
    /// Codex has gone.
    gone: bool,
    /// How long a request waits for Codex (D-066).
    patience: Duration,
    /// When the core's request being handled gives up on Codex.
    deadline: tokio::time::Instant,
    /// Messages for no chat: Codex's own bookkeeping, and anything for a thread no chat has.
    unrouted: u64,
}

impl Codex {
    /// Starts `codex app-server` from `program`, with subagents switched off, and greets it.
    pub async fn start(program: &str, setup: Setup) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args([
                "app-server",
                "--disable",
                "multi_agent",
                "--disable",
                "multi_agent_v2",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(io::Error::other(
                "codex app-server has no standard input or output",
            ));
        };
        let mut codex = Self::connect(stdout, stdin, setup).await?;
        codex.child = Some(child);
        Ok(codex)
    }

    /// Greets a Codex app server that is already running, over its output and input.
    pub async fn connect(
        output: impl AsyncRead + Unpin + Send + 'static,
        input: impl AsyncWrite + Unpin + Send + 'static,
        setup: Setup,
    ) -> io::Result<Self> {
        let output: Box<dyn AsyncRead + Unpin + Send> = Box::new(output);
        let mut codex = Self {
            reader: BufReader::new(output).lines(),
            writer: Box::new(input),
            unwritten: VecDeque::new(),
            unflushed: Vec::new(),
            child: None,
            setup,
            version: None,
            next_id: 1,
            chats: HashMap::new(),
            threads: HashMap::new(),
            owners: HashMap::new(),
            requests: HashMap::new(),
            starting: HashMap::new(),
            prompts: HashMap::new(),
            early: Vec::new(),
            ready: VecDeque::new(),
            calls: HashMap::new(),
            gone: false,
            patience: PATIENCE,
            deadline: tokio::time::Instant::now() + PATIENCE,
            unrouted: 0,
        };
        let id = codex.id();
        codex.queue(&initialize(id), None);
        codex.flush().await?;
        let reply = loop {
            let line = codex.reader.next_line().await?;
            let Some(line) = line else {
                return Err(io::Error::other("codex app-server ended before it replied"));
            };
            let message: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if message["id"] == id && message.get("method").is_none() {
                break message;
            }
        };
        if let Some(error) = reply.get("error") {
            // Codex's own words can hold a path, and an error may be logged (D-071): only its code.
            let code = error["code"].as_i64().map(|c| format!(" (code {c})"));
            return Err(io::Error::other(format!(
                "codex app-server refused to start{}",
                code.unwrap_or_default()
            )));
        }
        // "codex_cli_rs/0.169.0 (…)": the version is the word after the first slash.
        codex.version = (reply["result"]["userAgent"].as_str())
            .and_then(|agent| agent.split_whitespace().next())
            .and_then(|name| name.split_once('/'))
            .map(|(_, version)| version.to_string());
        codex.queue(&initialized(), None);
        codex.flush().await?;
        Ok(codex)
    }

    /// How long a request waits for Codex before the driver gives up on it: a minute unless set.
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

    fn queue(&mut self, text: &str, then: Option<Written>) {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(b'\n');
        self.unwritten.push_back(Frame {
            bytes,
            written: 0,
            then,
        });
    }

    /// Writes what is queued, and does what follows each message once all of it is written. Cut
    /// short, it leaves the rest queued for the next call.
    async fn flush(&mut self) -> io::Result<()> {
        while let Some(frame) = self.unwritten.front_mut() {
            while frame.written < frame.bytes.len() {
                let written = self.writer.write(&frame.bytes[frame.written..]).await?;
                if written == 0 {
                    return Err(io::Error::from(io::ErrorKind::WriteZero));
                }
                frame.written += written;
            }
            if let Some(then) = self.unwritten.pop_front().and_then(|f| f.then) {
                self.unflushed.push(then);
            }
        }
        self.writer.flush().await?;
        for then in std::mem::take(&mut self.unflushed) {
            self.written(then);
        }
        Ok(())
    }

    /// What follows a message once it is written: its request is kept until Codex answers, and
    /// the chat's translator is fed it.
    fn written(&mut self, then: Written) {
        let Written {
            chat,
            text,
            request,
            settles,
        } = then;
        if let Some((id, mut purpose)) = request {
            match &mut purpose {
                // Codex can announce the turn before it replies: its prompt is kept from now.
                Purpose::Prompt(message, None) => {
                    self.starting.insert(id, message.clone());
                }
                Purpose::Control(request, effect) => {
                    if let Some(effect) = effect.take() {
                        let c = self.chats.entry(chat.clone()).or_default();
                        c.effects.push((request.clone(), effect));
                    }
                }
                _ => {}
            }
            self.requests.insert(id, (chat.clone(), purpose));
        }
        self.feed(&chat, Direction::Out, &text, settles);
    }

    /// Sends a message for a chat and feeds it to that chat's translator; the events it makes
    /// settle `settles`. A request is kept, with what it is for, until Codex answers it.
    async fn send(
        &mut self,
        chat: &ChatId,
        request: Option<(i64, Purpose)>,
        text: &str,
        settles: Vec<RequestId>,
    ) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        let then = Written {
            chat: chat.clone(),
            text: text.to_string(),
            request,
            settles,
        };
        self.queue(text, Some(then));
        if self.flush_in_time().await.is_err() {
            self.stopped();
            return Err(Refusal::new(Reason::Failed));
        }
        Ok(())
    }

    /// Writes what is queued, for one of the core's requests, giving up at its deadline. A Codex
    /// that has not read it by then has stopped working: the connection ends, so a message cut
    /// short is never finished, and nothing written later follows a request that failed (D-066).
    async fn flush_in_time(&mut self) -> io::Result<()> {
        // Safe to give up on: what is written is counted as it goes, and nothing follows a
        // message until all of it is written.
        match tokio::time::timeout_at(self.deadline, self.flush()).await {
            Ok(flushed) => flushed,
            Err(_) => {
                self.give_up();
                Err(io::Error::from(io::ErrorKind::TimedOut))
            }
        }
    }

    /// Reads one message from Codex and handles it. False once Codex has gone.
    async fn read(&mut self) -> bool {
        if self.gone {
            return false;
        }
        if self.flush().await.is_err() {
            self.stopped();
            return false;
        }
        match self.reader.next_line().await {
            Ok(Some(line)) => {
                self.route(&line);
                true
            }
            Ok(None) | Err(_) => {
                self.stopped();
                false
            }
        }
    }

    /// Reads one message, for one of the core's requests, giving up at its deadline. A Codex that
    /// has not answered by then has stopped working: the connection ends, so a reply that came
    /// later cannot pick a session after the request failed (D-066). False once Codex has gone.
    async fn read_in_time(&mut self) -> bool {
        // Safe to give up on: a message cut short stays queued, and nothing of a line is lost.
        match tokio::time::timeout_at(self.deadline, self.read()).await {
            Ok(more) => more,
            Err(_) => {
                self.give_up();
                false
            }
        }
    }

    /// One of the core's requests begins: it has the patience from now, for everything it waits
    /// for, writing included.
    fn begin(&mut self) {
        self.deadline = tokio::time::Instant::now() + self.patience;
    }

    /// Ends the connection with a Codex that stopped working: it is stopped, nothing more is
    /// written to it or read from it, and each chat closes what it had open. Nothing Codex says
    /// later is read, even when the driver did not start it and cannot stop it.
    fn give_up(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        self.writer = Box::new(tokio::io::sink());
        let nothing: Box<dyn AsyncRead + Unpin + Send> = Box::new(tokio::io::empty());
        self.reader = BufReader::new(nothing).lines();
        self.stopped();
    }

    /// Gives a message from Codex to the chat it is for.
    fn route(&mut self, line: &str) {
        let message: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if message.get("method").is_some() {
            let thread = thread_of(line);
            let thread = thread.as_deref();
            let calls_tool = message["method"] == "item/tool/call" && message.get("id").is_some();
            match thread.and_then(|t| self.threads.get(t)).cloned() {
                Some(chat) if calls_tool => {
                    self.feed(&chat, Direction::In, line, Vec::new());
                    self.tool_call(&chat, &message);
                }
                Some(chat) => self.feed(&chat, Direction::In, line, Vec::new()),
                // Codex waits for a reply to its call, so one for no chat is refused.
                None if calls_tool => {
                    self.unrouted += 1;
                    let refused = Err(Refusal::new(Reason::Invalid));
                    self.queue(&tool_reply(&message["id"], &refused), None);
                }
                // A thread being picked can speak before the reply that picks it.
                None if thread.is_some() && self.picking() => self.early.push(line.to_string()),
                None => self.unrouted += 1,
            }
            return;
        }
        let id = message["id"].as_i64();
        let Some((chat, purpose)) = id.and_then(|id| self.requests.remove(&id)) else {
            self.unrouted += 1;
            return;
        };
        let refused = message.get("error").is_some();
        let mut settles = Vec::new();
        match purpose {
            Purpose::Plain => {}
            Purpose::Prompt(_, None) if refused => {
                self.starting.remove(&id.expect("a numbered reply")); // no turn will start
            }
            Purpose::Prompt(..) if refused => {}
            // A prompt joins a running turn only once Codex has taken it.
            Purpose::Prompt(prompt, Some(turn)) => {
                if let Some(prompts) = self.prompts.get_mut(&(chat.clone(), turn)) {
                    prompts.push(prompt);
                }
            }
            Purpose::Prompt(_, None) => {} // its turn takes it from `starting`
            // Codex refused a control: the event the refusal makes settles it, unless its effect
            // already did.
            Purpose::Control(request, _) => {
                if refused && self.unsettled(&chat, &request) {
                    settles.push(request);
                }
            }
        }
        self.feed(&chat, Direction::In, line, settles);
        // What a thread said before its session was picked goes to it now, before anything it
        // says next; what names a thread still unknown waits while a session is being picked.
        let (known, unknown): (Vec<String>, Vec<String>) = std::mem::take(&mut self.early)
            .into_iter()
            .partition(|line| thread_of(line).is_some_and(|t| self.threads.contains_key(&t)));
        self.early = unknown;
        for line in known {
            self.route(&line);
        }
        if !self.picking() {
            self.unrouted += self.early.len() as u64;
            self.early.clear();
        }
    }

    /// Whether a request to start or resume a session is waiting for Codex, in any chat, whether
    /// a prompt or a control sent it.
    fn picking(&self) -> bool {
        self.chats.values().any(|c| c.translator.picking())
    }

    /// Codex calls the driver's own tool. A call to send a file goes to the core; any other is
    /// refused at once, since Codex waits for the reply.
    fn tool_call(&mut self, chat: &ChatId, message: &Value) {
        let id = message["id"].clone();
        let params = &message["params"];
        let path = params["arguments"]["path"]
            .as_str()
            .filter(|p| !p.is_empty());
        let ours = params["tool"] == SEND_FILE && params["namespace"].is_null();
        let Some(path) = path.filter(|_| ours) else {
            let refused = Err(Refusal {
                reason: Reason::Invalid,
                detail: Some("the driver has no such tool, or it was not given a path.".into()),
            });
            let reply = tool_reply(&id, &refused);
            let then = Written {
                chat: chat.clone(),
                text: reply.clone(),
                request: None,
                settles: Vec::new(),
            };
            self.queue(&reply, Some(then));
            return;
        };
        // A path the agent gives relative to its working folder is made whole here: the core
        // takes only an absolute one. An absolute path stays as it is.
        let path = std::path::Path::new(&self.setup.cwd).join(path);
        let key = id.to_string();
        self.calls.insert(key.clone(), (chat.clone(), id));
        self.ready.push_back(Output::SendFile(SendFile {
            id: key,
            chat: chat.clone(),
            path,
        }));
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

    /// Feeds a message to a chat's translator, makes its events ready, and sends the replies
    /// that need nobody's answer, such as declining a form.
    fn feed(&mut self, chat: &ChatId, direction: Direction, text: &str, settles: Vec<RequestId>) {
        let c = self.chats.entry(chat.clone()).or_default();
        let before = c.translator.session().map(String::from);
        let events = c.translator.feed(direction, text);
        let after = c.translator.session().map(String::from);
        let replies = c.translator.take_replies();
        if before != after {
            if let Some(old) = before {
                self.threads.remove(&old);
            }
            if let Some(new) = after {
                self.owners.insert(new.clone(), chat.clone());
                self.threads.insert(new, chat.clone());
            }
        }
        let mut settles = settles;
        for event in events {
            self.ready_event(chat, event, std::mem::take(&mut settles), true);
        }
        for reply in replies {
            let then = Written {
                chat: chat.clone(),
                text: reply.clone(),
                request: None,
                settles: Vec::new(),
            };
            self.queue(&reply, Some(then));
        }
    }

    /// Makes an event ready for the core. An event Codex sent shows the effect of a control; one
    /// the driver made when Codex stopped does not (`seen` false): that control failed, and the
    /// core settles it so (D-058).
    fn ready_event(
        &mut self,
        chat: &ChatId,
        event: AgentEvent,
        mut settles: Vec<RequestId>,
        seen: bool,
    ) {
        let c = self.chats.get_mut(chat).expect("a chat that was fed");
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
                    (Effect::TurnStartedBy(r), Body::Turn(_)) => {
                        let started = turn.as_deref().and_then(|t| c.translator.turn_request(t));
                        started == Some(*r)
                    }
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
                    let started = c.translator.turn_request(&key.1);
                    let prompt = started.and_then(|r| self.starting.remove(&r));
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

    /// Codex has gone: each translator closes what it can, and nothing more will come.
    fn stopped(&mut self) {
        if self.gone {
            return;
        }
        self.gone = true;
        let chats: Vec<ChatId> = self.chats.keys().cloned().collect();
        for chat in chats {
            let c = self.chats.get_mut(&chat).expect("a chat");
            let events = c.translator.harness_stopped();
            for event in events {
                self.ready_event(&chat, event, Vec::new(), false);
            }
        }
        self.requests.clear();
        self.starting.clear();
        self.early.clear();
        self.calls.clear(); // a reply has nobody to go to
        self.unwritten.clear(); // never written: nothing follows from them
        self.unflushed.clear();
    }
}

/// The thread a message from Codex names, if it names one.
fn thread_of(line: &str) -> Option<String> {
    let message: Value = serde_json::from_str(line).ok()?;
    let params = &message["params"];
    let thread = params["threadId"]
        .as_str()
        .or(params["thread"]["id"].as_str());
    thread.map(String::from)
}

// -- the core's requests

impl Codex {
    /// Makes a chat's translator the first time the chat is seen. A new chat asks Codex for its
    /// models and modes, which the session item offers.
    async fn chat(&mut self, chat: &ChatId) -> Result<(), Refusal> {
        if self.chats.contains_key(chat) {
            return Ok(());
        }
        self.chats.insert(chat.clone(), Chat::default());
        // Both are queued before either is written, so a write cut short loses neither.
        for list in [model_list as fn(i64) -> String, collaboration_mode_list] {
            let id = self.id();
            let text = list(id);
            let then = Written {
                chat: chat.clone(),
                text: text.clone(),
                request: Some((id, Purpose::Plain)),
                settles: Vec::new(),
            };
            self.queue(&text, Some(then));
        }
        if self.flush_in_time().await.is_err() {
            self.stopped();
            return Err(Refusal::new(Reason::Failed));
        }
        Ok(())
    }

    /// Builds a request with `build` and sends it, returning its id. While the translator refuses
    /// it as busy only because Codex has yet to answer a request before it, reads on and asks
    /// again. Refused as busy for any other reason, such as a turn running, it stays refused.
    async fn ask(
        &mut self,
        chat: &ChatId,
        purpose: impl Fn(&Translator, i64) -> Purpose,
        build: impl Fn(&Translator, i64) -> Result<String, Refusal>,
    ) -> Result<i64, Refusal> {
        loop {
            // A message cut short is finished first, so the translator builds from what was sent.
            if self.flush_in_time().await.is_err() {
                self.stopped();
                return Err(Refusal::new(Reason::Failed));
            }
            let id = self.next_id;
            let translator = &self.chats.get(chat).expect("a chat").translator;
            match build(translator, id) {
                Ok(text) => {
                    let purpose = purpose(translator, id);
                    self.next_id += 1;
                    self.send(chat, Some((id, purpose)), &text, Vec::new())
                        .await?;
                    return Ok(id);
                }
                Err(refusal) if refusal.reason == Reason::Busy && translator.waiting_on_codex() => {
                    if !self.read_in_time().await {
                        return Err(Refusal::new(Reason::Failed));
                    }
                }
                Err(refusal) => return Err(refusal),
            }
        }
    }

    /// Makes `session` the chat's session, starting a new one if it is none, and waits for Codex.
    async fn pick(&mut self, chat: &ChatId, session: Option<&str>) -> Result<(), Refusal> {
        // A session to resume is the chat's from the moment it is asked for, so no other chat can
        // ask for it meanwhile; if Codex refuses, a reservation made here is let go.
        let reserved = session
            .filter(|t| !self.owners.contains_key(*t))
            .map(String::from);
        if let Some(thread) = &reserved {
            self.owners.insert(thread.clone(), chat.clone());
        }
        let picked = self.pick_now(chat, session).await;
        if picked.is_err()
            && let Some(thread) = reserved
            && self.chats[chat].translator.session() != Some(thread.as_str())
        {
            self.owners.remove(&thread);
        }
        picked
    }

    async fn pick_now(&mut self, chat: &ChatId, session: Option<&str>) -> Result<(), Refusal> {
        let setup = self.setup.clone();
        let build = |t: &Translator, id| t.pick(id, session, &setup);
        let id = self.ask(chat, |_, _| Purpose::Plain, build).await?;
        while self.requests.contains_key(&id) {
            if !self.read_in_time().await {
                return Err(Refusal::new(Reason::Failed));
            }
        }
        let now = self.chats[chat].translator.session().map(String::from);
        match (now, session) {
            (Some(now), Some(wanted)) if now == wanted => Ok(()),
            (Some(_), None) => Ok(()),
            _ => Err(Refusal::new(Reason::Failed)), // Codex refused it; the chat shows why
        }
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

impl Driver for Codex {
    fn harness(&self) -> Harness {
        Harness {
            name: "codex".into(),
            version: self.version.clone(),
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: true,
            mid_turn: MidTurn::Fold,
            sessions: Sessions::Core,
            resume: true,
            approvals: true,
            files_in: FilesIn::Images,
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        self.begin();
        let chat = prompt.chat.id.clone();
        // The core names a session only to resume it (D-065).
        let wanted = prompt.session.as_ref().map(|s| s.as_str().to_string());
        // A session belongs to its chat for good: resuming another chat's would show its history
        // and activity here.
        if let Some(owner) = wanted.as_ref().and_then(|w| self.owners.get(w))
            && *owner != chat
        {
            return Err(Refusal::new(Reason::Invalid));
        }
        self.chat(&chat).await?;
        // A session being picked, by a control or an earlier prompt, is waited for before deciding
        // whether this prompt needs one.
        while self.chats[&chat].translator.picking() {
            if !self.read_in_time().await {
                return Err(Refusal::new(Reason::Failed));
            }
        }
        let current = self.chats[&chat].translator.session().map(String::from);
        if current.is_none() || (wanted.is_some() && wanted != current) {
            self.pick(&chat, wanted.as_deref()).await?;
        }
        // Built during a turn, it joins that turn: the translator builds it that way.
        let message = prompt.message.clone();
        let purpose =
            |t: &Translator, _| Purpose::Prompt(message.clone(), t.turn().map(String::from));
        self.ask(&chat, purpose, |t, id| t.prompt(id, &prompt))
            .await?;
        Ok(())
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        self.begin();
        // A message cut short is finished first: it may be an answer to the same item.
        if self.flush_in_time().await.is_err() {
            self.stopped();
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
        self.begin();
        let request = control.request.clone();
        let (chat, effect) = match &control.action {
            Action::Interrupt { turn } => {
                let chat = (self.chats.iter())
                    .find(|(_, c)| c.translator.turn() == Some(turn.as_str()))
                    .map(|(chat, _)| chat.clone())
                    .ok_or_else(|| Refusal::new(Reason::NothingToDo))?;
                (chat, Effect::TurnEnded(turn.as_str().to_string()))
            }
            Action::NewSession { chat, session } => {
                let chat = chat.id.clone();
                self.chat(&chat).await?;
                let old = session.as_ref().map(|s| s.as_str().to_string());
                (chat, Effect::NewSession(old))
            }
            Action::Set {
                session,
                setting,
                value,
            } => {
                let chat = (self.threads.get(session.as_str()).cloned())
                    .ok_or_else(|| Refusal::new(Reason::UnknownItem))?;
                (chat, Effect::Setting(setting.clone(), value.clone()))
            }
            Action::Own { session, .. } => {
                let chat = (self.threads.get(session.as_str()).cloned())
                    .ok_or_else(|| Refusal::new(Reason::UnknownItem))?;
                (chat, Effect::TurnStartedBy(0))
            }
        };
        let setup = self.setup.clone();
        let action = control.action.clone();
        let build = |t: &Translator, id| t.control(id, &action, &setup);
        // Its effect is watched for from the moment it is written.
        let purpose = |_: &Translator, id| {
            let effect = match &effect {
                Effect::TurnStartedBy(_) => Effect::TurnStartedBy(id),
                other => other.clone(),
            };
            Purpose::Control(request.clone(), Some(effect))
        };
        self.ask(&chat, purpose, build).await?;
        Ok(())
    }

    async fn next_event(&mut self) -> Option<Output> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Some(event);
            }
            if !self.read().await {
                return self.ready.pop_front();
            }
        }
    }

    async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
        let Some((chat, codex_id)) = self.calls.remove(&id) else {
            return;
        };
        let reply = tool_reply(&codex_id, &result);
        self.begin();
        // A reply Codex cannot be given is lost with Codex, which `send` notes.
        let _ = self.send(&chat, None, &reply, Vec::new()).await;
    }
}
