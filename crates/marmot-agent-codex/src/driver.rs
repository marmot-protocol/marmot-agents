//! The transport: runs `codex app-server`, gives each chat a translator of its own, and is the
//! driver the core holds.
//!
//! Each message from Codex goes to the chat whose session it names, or, for a reply, to the chat
//! that made the request (D-060). Each message the driver sends is fed to that chat's translator
//! as well, so the translator learns what was sent the same way whoever sent it. When a
//! translator refuses a request as busy, because Codex has yet to answer one before it, the driver
//! keeps reading from Codex and asks again.
//!
//! Subagents are switched off when Codex starts: how a chat shows them is a proposal not yet
//! decided ([Q-30], `docs/design/subagents.md`), and without them an approval from a subagent
//! would have nowhere to go.
//!
//! [Q-30]: https://github.com/marmot-protocol/marmot-agents/blob/master/docs/open-questions.md

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

/// What shows that a control the driver sent has had its effect.
enum Settle {
    /// The turn it interrupted has ended.
    TurnEnded(String),
    /// The session item has been edited.
    SessionEdited,
    /// A turn started by this request has started.
    TurnStartedBy(i64),
    /// A session other than this one is running.
    NewSession(Option<String>),
}

/// One chat: its translator, and the controls waiting for their effect.
#[derive(Default)]
struct Chat {
    translator: Translator,
    settling: Vec<(RequestId, Settle)>,
}

/// The Codex driver: one `codex app-server` process, serving every chat of one agent.
pub struct Codex {
    reader: Reader,
    writer: Writer,
    /// The process, when the driver started it. It is killed when the driver is dropped.
    child: Option<Child>,
    setup: Setup,
    version: Option<String>,
    next_id: i64,
    chats: HashMap<ChatId, Chat>,
    /// Each chat's session, by Codex thread.
    threads: HashMap<String, ChatId>,
    /// The driver's requests Codex has not answered, and the chat each is for.
    requests: HashMap<i64, ChatId>,
    /// The core's answer or control behind each of the driver's requests, so that a refusal from
    /// Codex settles it.
    behind: HashMap<i64, RequestId>,
    /// The prompt each `turn/start` or `turn/steer` carried.
    messages: HashMap<i64, MessageId>,
    /// The prompts each turn responds to.
    prompts: HashMap<(ChatId, String), Vec<MessageId>>,
    /// Events read and not yet handed to the core.
    ready: VecDeque<Emitted>,
    /// Replies a translator sends without asking anyone, waiting to be written.
    later: VecDeque<(ChatId, String)>,
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
            child: None,
            setup,
            version: None,
            next_id: 1,
            chats: HashMap::new(),
            threads: HashMap::new(),
            requests: HashMap::new(),
            behind: HashMap::new(),
            messages: HashMap::new(),
            prompts: HashMap::new(),
            ready: VecDeque::new(),
            later: VecDeque::new(),
            gone: false,
            unrouted: 0,
        };
        let id = codex.id();
        codex.write(&initialize(id)).await?;
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
        codex.write(&initialized()).await?;
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

    async fn write(&mut self, text: &str) -> io::Result<()> {
        self.writer.write_all(text.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await
    }

    /// Sends a message for a chat, and feeds it to that chat's translator. The events it makes
    /// settle `settles`.
    async fn send(
        &mut self,
        chat: &ChatId,
        id: Option<i64>,
        text: &str,
        settles: Vec<RequestId>,
    ) -> Result<(), Refusal> {
        if self.gone || self.write(text).await.is_err() {
            self.stopped();
            return Err(Refusal::new(Reason::Failed));
        }
        if let Some(id) = id {
            self.requests.insert(id, chat.clone());
        }
        self.feed(chat, Direction::Out, text, settles);
        Ok(())
    }

    /// Reads one message from Codex and handles it. False once Codex has gone.
    async fn read(&mut self) -> bool {
        if self.gone {
            return false;
        }
        match self.reader.next_line().await {
            Ok(Some(line)) => {
                self.route(&line);
                self.write_later().await;
                true
            }
            Ok(None) | Err(_) => {
                self.stopped();
                false
            }
        }
    }

    /// Writes the replies a translator sends without asking anyone.
    async fn write_later(&mut self) {
        while let Some((chat, reply)) = self.later.pop_front() {
            if self.send(&chat, None, &reply, Vec::new()).await.is_err() {
                return;
            }
        }
    }

    /// Gives a message from Codex to the chat it is for.
    fn route(&mut self, line: &str) {
        let message: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        let chat = if message.get("method").is_some() {
            let params = &message["params"];
            let thread = (params["threadId"].as_str()).or(params["thread"]["id"].as_str());
            thread.and_then(|t| self.threads.get(t)).cloned()
        } else {
            message["id"]
                .as_i64()
                .and_then(|id| self.requests.remove(&id))
        };
        let Some(chat) = chat else {
            self.unrouted += 1;
            return;
        };
        // A refusal of an answer or a control settles it: the event it makes shows that it failed.
        let refused = (message["id"].as_i64())
            .filter(|_| message.get("error").is_some() && message.get("method").is_none())
            .and_then(|id| self.behind.remove(&id));
        if let Some(request) = &refused
            && let Some(c) = self.chats.get_mut(&chat)
        {
            c.settling.retain(|(r, _)| r != request);
        }
        self.feed(&chat, Direction::In, line, refused.into_iter().collect());
    }

    /// Feeds a message to a chat's translator, and makes its events ready.
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
        // Replies nobody is asked for, such as declining a form, go to Codex at once.
        for reply in replies {
            self.later.push_back((chat.clone(), reply));
        }
    }

    fn ready_event(&mut self, chat: &ChatId, event: AgentEvent, mut settles: Vec<RequestId>) {
        let c = self.chats.get_mut(chat).expect("a chat that was fed");
        let header = event.header();
        let turn = header.turn.as_ref().map(|t| t.as_str().to_string());
        let ended = !matches!(header.state, State::Running | State::Waiting);
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        c.settling.retain(|(request, waits)| {
            let effect = match (waits, event.body()) {
                (Settle::TurnEnded(t), Body::Turn(_)) => ended && turn.as_deref() == Some(t),
                (Settle::SessionEdited, Body::Session(_)) => true,
                (Settle::TurnStartedBy(r), Body::Turn(_)) => {
                    let started = turn.as_deref().and_then(|t| c.translator.turn_request(t));
                    started == Some(*r)
                }
                (Settle::NewSession(old), Body::Session(_)) => {
                    header.state == State::Running && session != *old
                }
                _ => false,
            };
            if effect {
                settles.push(request.clone());
            }
            !effect
        });
        let prompts = match (event.body(), turn) {
            (Body::Turn(_), Some(turn)) => {
                let started = c.translator.turn_request(&turn);
                let messages = &self.messages;
                (self.prompts.entry((chat.clone(), turn)))
                    .or_insert_with(|| {
                        started
                            .and_then(|r| messages.get(&r))
                            .cloned()
                            .into_iter()
                            .collect()
                    })
                    .clone()
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
            let events = self
                .chats
                .get_mut(&chat)
                .expect("a chat")
                .translator
                .harness_stopped();
            for event in events {
                self.ready_event(&chat, event, Vec::new());
            }
        }
    }
}

// -- the core's requests

impl Codex {
    /// A chat's translator, made the first time the chat is seen. A new chat asks Codex for its
    /// models and modes, which the session item offers.
    async fn chat(&mut self, chat: &ChatId) -> Result<(), Refusal> {
        if self.chats.contains_key(chat) {
            return Ok(());
        }
        self.chats.insert(chat.clone(), Chat::default());
        let id = self.id();
        self.send(chat, Some(id), &model_list(id), Vec::new())
            .await?;
        let id = self.id();
        self.send(chat, Some(id), &collaboration_mode_list(id), Vec::new())
            .await
    }

    /// Builds a request with `build`, sends it, and returns its id. While the translator refuses
    /// it as busy, because Codex has yet to answer one before it, reads on and asks again.
    async fn ask(
        &mut self,
        chat: &ChatId,
        settles: Vec<RequestId>,
        build: impl Fn(&Translator, i64) -> Result<String, Refusal>,
    ) -> Result<i64, Refusal> {
        loop {
            let id = self.next_id;
            let translator = &self.chats.get(chat).expect("a chat").translator;
            match build(translator, id) {
                Ok(text) => {
                    self.next_id += 1;
                    self.send(chat, Some(id), &text, settles).await?;
                    return Ok(id);
                }
                Err(refusal) if refusal.reason == Reason::Busy => {
                    if !self.read().await {
                        return Err(Refusal::new(Reason::Failed));
                    }
                }
                Err(refusal) => return Err(refusal),
            }
        }
    }

    /// Reads until Codex has answered request `id`.
    async fn answered(&mut self, id: i64) -> Result<(), Refusal> {
        while self.requests.contains_key(&id) {
            if !self.read().await {
                return Err(Refusal::new(Reason::Failed));
            }
        }
        Ok(())
    }

    /// Makes `session` the chat's session, starting a new one if it is none, and waits for Codex.
    async fn pick(&mut self, chat: &ChatId, session: Option<&str>) -> Result<(), Refusal> {
        let setup = self.setup.clone();
        let id = self
            .ask(chat, Vec::new(), |t, id| t.pick(id, session, &setup))
            .await?;
        self.answered(id).await?;
        let now = self.chats[chat].translator.session().map(String::from);
        match (now, session) {
            (Some(now), Some(wanted)) if now == wanted => Ok(()),
            (Some(_), None) => Ok(()),
            _ => Err(Refusal::new(Reason::Failed)), // Codex refused it; the chat shows why
        }
    }

    /// The chat that has `item` waiting, or else the first chat's refusal.
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
        let id = self
            .ask(&chat, Vec::new(), |t, id| t.prompt(id, &prompt))
            .await?;
        self.messages.insert(id, prompt.message.clone());
        // A message during a turn joins it: the turn responds to that prompt too.
        if let Some(turn) = self.chats[&chat].translator.turn().map(String::from)
            && let Some(prompts) = self.prompts.get_mut(&(chat.clone(), turn))
        {
            prompts.push(prompt.message);
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
        let (chat, waits) = match &control.action {
            Action::Interrupt { turn } => {
                let chat = (self.chats.iter())
                    .find(|(_, c)| c.translator.turn() == Some(turn.as_str()))
                    .map(|(chat, _)| chat.clone())
                    .ok_or_else(|| Refusal::new(Reason::NothingToDo))?;
                (chat, Settle::TurnEnded(turn.as_str().to_string()))
            }
            Action::NewSession { chat, session } => {
                let chat = chat.id.clone();
                self.chat(&chat).await?;
                let old = session.as_ref().map(|s| s.as_str().to_string());
                (chat, Settle::NewSession(old))
            }
            Action::Set { session, .. } => {
                let chat = (self.threads.get(session.as_str()).cloned())
                    .ok_or_else(|| Refusal::new(Reason::UnknownItem))?;
                (chat, Settle::SessionEdited)
            }
            Action::Own { session, .. } => {
                let chat = (self.threads.get(session.as_str()).cloned())
                    .ok_or_else(|| Refusal::new(Reason::UnknownItem))?;
                (chat, Settle::TurnStartedBy(0))
            }
        };
        let setup = self.setup.clone();
        let action = control.action.clone();
        let id = self
            .ask(&chat, Vec::new(), |t, id| t.control(id, &action, &setup))
            .await?;
        let waits = match waits {
            Settle::TurnStartedBy(_) => Settle::TurnStartedBy(id),
            other => other,
        };
        self.behind.insert(id, request.clone());
        self.chats
            .get_mut(&chat)
            .expect("a chat")
            .settling
            .push((request, waits));
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
