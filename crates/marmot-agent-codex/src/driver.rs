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
//! Subagents are switched off when Codex starts: how a chat shows them is a proposal not yet
//! decided (`docs/design/subagents.md`), and without them an approval from a subagent would have
//! nowhere to go.

use crate::{
    Direction, Setup, Translator, collaboration_mode_list, initialize, initialized, model_list,
};
use marmot_agent_driver::{
    Action, Answer, Capabilities, ChatId, Control, Driver, Emitted, FilesIn, Harness, MessageId,
    MidTurn, Prompt, Reason, Refusal, RequestId, Sessions,
};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, Command};

type Reader = Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

/// What a request the driver sent is for.
enum Purpose {
    /// A list of models or modes: nothing follows from it.
    List,
    /// Starting or resuming a session.
    Pick,
    /// A prompt: a new turn, or, with the turn, one joined to a running turn.
    Prompt(MessageId, Option<String>),
    /// A control, settled by its effect or by Codex refusing it.
    Control(RequestId),
}

/// What shows that a control has had its effect.
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
    /// Bytes to write to Codex. They stay here until written, so a write cut short when the core
    /// drops a future is finished, whole, by the next.
    unwritten: Vec<u8>,
    /// The process, when the driver started it. It is killed when the driver is dropped.
    child: Option<Child>,
    setup: Setup,
    version: Option<String>,
    next_id: i64,
    chats: HashMap<ChatId, Chat>,
    /// Each chat's session, by Codex thread.
    threads: HashMap<String, ChatId>,
    /// The driver's requests Codex has not answered: the chat each is for, and what for.
    requests: HashMap<i64, (ChatId, Purpose)>,
    /// The prompt behind each `turn/start` Codex has taken, until its turn starts.
    starting: HashMap<i64, MessageId>,
    /// The prompts each running turn responds to.
    prompts: HashMap<(ChatId, String), Vec<MessageId>>,
    /// Messages naming a thread no chat has yet, kept while a session is being picked.
    early: Vec<String>,
    /// Events read and not yet handed to the core.
    ready: VecDeque<Emitted>,
    /// Codex has gone.
    gone: bool,
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
            unwritten: Vec::new(),
            child: None,
            setup,
            version: None,
            next_id: 1,
            chats: HashMap::new(),
            threads: HashMap::new(),
            requests: HashMap::new(),
            starting: HashMap::new(),
            prompts: HashMap::new(),
            early: Vec::new(),
            ready: VecDeque::new(),
            gone: false,
            unrouted: 0,
        };
        let id = codex.id();
        codex.queue(&initialize(id));
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
            return Err(io::Error::other(format!(
                "codex app-server refused: {error}"
            )));
        }
        // "codex_cli_rs/0.169.0 (…)": the version is the word after the first slash.
        codex.version = (reply["result"]["userAgent"].as_str())
            .and_then(|agent| agent.split_whitespace().next())
            .and_then(|name| name.split_once('/'))
            .map(|(_, version)| version.to_string());
        codex.queue(&initialized());
        codex.flush().await?;
        Ok(codex)
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

    fn queue(&mut self, text: &str) {
        self.unwritten.extend_from_slice(text.as_bytes());
        self.unwritten.push(b'\n');
    }

    /// Writes what is queued. Cut short, it leaves the rest queued for the next call.
    async fn flush(&mut self) -> io::Result<()> {
        while !self.unwritten.is_empty() {
            let written = self.writer.write(&self.unwritten).await?;
            if written == 0 {
                return Err(io::Error::from(io::ErrorKind::WriteZero));
            }
            self.unwritten.drain(..written);
        }
        self.writer.flush().await
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
        self.queue(text);
        if let Some((id, purpose)) = request {
            self.requests.insert(id, (chat.clone(), purpose));
        }
        self.feed(chat, Direction::Out, text, settles);
        if self.flush().await.is_err() {
            self.stopped();
            return Err(Refusal::new(Reason::Failed));
        }
        Ok(())
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

    /// Gives a message from Codex to the chat it is for.
    fn route(&mut self, line: &str) {
        let message: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if message.get("method").is_some() {
            let params = &message["params"];
            let thread = (params["threadId"].as_str()).or(params["thread"]["id"].as_str());
            match thread.and_then(|t| self.threads.get(t)).cloned() {
                Some(chat) => self.feed(&chat, Direction::In, line, Vec::new()),
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
            Purpose::List | Purpose::Pick => {}
            Purpose::Prompt(..) if refused => {}
            // A prompt joins a running turn only once Codex has taken it.
            Purpose::Prompt(prompt, Some(turn)) => {
                if let Some(prompts) = self.prompts.get_mut(&(chat.clone(), turn)) {
                    prompts.push(prompt);
                }
            }
            Purpose::Prompt(prompt, None) => {
                self.starting.insert(id.expect("a numbered reply"), prompt);
            }
            // Codex refused a control: the event the refusal makes settles it, unless its effect
            // already did.
            Purpose::Control(request) => {
                if refused && self.unsettled(&chat, &request) {
                    settles.push(request);
                }
            }
        }
        self.feed(&chat, Direction::In, line, settles);
        if !self.picking() {
            for line in std::mem::take(&mut self.early) {
                self.route(&line);
            }
        }
    }

    /// Whether a request to start or resume a session is waiting for Codex.
    fn picking(&self) -> bool {
        self.requests
            .values()
            .any(|(_, purpose)| matches!(purpose, Purpose::Pick))
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
                self.threads.insert(new, chat.clone());
            }
        }
        let mut settles = settles;
        for event in events {
            self.ready_event(chat, event, std::mem::take(&mut settles));
        }
        for reply in replies {
            self.queue(&reply);
            self.feed(chat, Direction::Out, &reply, Vec::new());
        }
    }

    fn ready_event(&mut self, chat: &ChatId, event: AgentEvent, mut settles: Vec<RequestId>) {
        let c = self.chats.get_mut(chat).expect("a chat that was fed");
        let header = event.header();
        let turn = header.turn.as_ref().map(|t| t.as_str().to_string());
        let ended = !matches!(header.state, State::Running | State::Waiting);
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        c.effects.retain(|(request, effect)| {
            let shown = match (effect, event.body()) {
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
        self.ready.push_back(Emitted {
            event,
            chat: Some(chat.clone()),
            prompts,
            settles,
        });
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
                self.ready_event(&chat, event, Vec::new());
            }
        }
        self.requests.clear();
        self.starting.clear();
        self.early.clear();
        self.unwritten.clear();
    }
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
        let id = self.id();
        self.send(chat, Some((id, Purpose::List)), &model_list(id), Vec::new())
            .await?;
        let id = self.id();
        let modes = collaboration_mode_list(id);
        self.send(chat, Some((id, Purpose::List)), &modes, Vec::new())
            .await
    }

    /// Builds a request with `build` and sends it, returning its id. While the translator refuses
    /// it as busy only because Codex has yet to answer a request before it, reads on and asks
    /// again. Refused as busy for any other reason, such as a turn running, it stays refused.
    async fn ask(
        &mut self,
        chat: &ChatId,
        purpose: Purpose,
        build: impl Fn(&Translator, i64) -> Result<String, Refusal>,
    ) -> Result<i64, Refusal> {
        loop {
            let id = self.next_id;
            let translator = &self.chats.get(chat).expect("a chat").translator;
            match build(translator, id) {
                Ok(text) => {
                    self.next_id += 1;
                    self.send(chat, Some((id, purpose)), &text, Vec::new())
                        .await?;
                    return Ok(id);
                }
                Err(refusal) if refusal.reason == Reason::Busy && translator.waiting_on_codex() => {
                    if !self.read().await {
                        return Err(Refusal::new(Reason::Failed));
                    }
                }
                Err(refusal) => return Err(refusal),
            }
        }
    }

    /// Makes `session` the chat's session, starting a new one if it is none, and waits for Codex.
    async fn pick(&mut self, chat: &ChatId, session: Option<&str>) -> Result<(), Refusal> {
        let setup = self.setup.clone();
        let build = |t: &Translator, id| t.pick(id, session, &setup);
        let id = self.ask(chat, Purpose::Pick, build).await?;
        while self.requests.contains_key(&id) {
            if !self.read().await {
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

    /// The chat that has the answer's item waiting, or else the most telling refusal.
    fn chat_with_item(&self, answer: &Answer) -> Result<ChatId, Refusal> {
        let mut refusal = Refusal::new(Reason::UnknownItem);
        for (chat, c) in &self.chats {
            match c.translator.reply(answer) {
                Ok(_) => return Ok(chat.clone()),
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
        let chat = prompt.chat.id.clone();
        self.chat(&chat).await?;
        let current = self.chats[&chat].translator.session().map(String::from);
        let wanted = prompt.session.as_ref().map(|s| s.as_str().to_string());
        if current.is_none() || (wanted.is_some() && wanted != current) {
            self.pick(&chat, wanted.as_deref()).await?;
        }
        let message = Purpose::Prompt(prompt.message.clone(), None);
        let id = self
            .ask(&chat, message, |t, id| t.prompt(id, &prompt))
            .await?;
        // Built during a turn, it joins that turn; the translator built it that way.
        if let Some(turn) = self.chats[&chat].translator.turn().map(String::from)
            && let Some((_, Purpose::Prompt(_, joins))) = self.requests.get_mut(&id)
        {
            *joins = Some(turn);
        }
        Ok(())
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        let chat = self.chat_with_item(&answer)?;
        let text = self.chats[&chat].translator.reply(&answer)?;
        // The reply's event, fed back, shows the answer: it settles it.
        self.send(&chat, None, &text, vec![answer.request]).await
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
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
        let id = self
            .ask(&chat, Purpose::Control(request.clone()), build)
            .await?;
        let effect = match effect {
            Effect::TurnStartedBy(_) => Effect::TurnStartedBy(id),
            other => other,
        };
        // Nothing has been read since it was sent, so its effect is still to come.
        let c = self.chats.get_mut(&chat).expect("a chat");
        c.effects.push((request, effect));
        Ok(())
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
