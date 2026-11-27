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
//! Codex's subagents are shown flat in the chat of the turn that started them (D-117). Each is a
//! thread of its own on the connection, which the item that starts it names; the driver gives each
//! subagent's thread a translator of its own, in the chat of the thread that started it. A message
//! for a thread no chat knows yet is held until the item that names its thread arrives, since a
//! subagent can speak first, as can a session being resumed. Codex waits for a reply to each of
//! its requests, so one for no chat is refused: at once, or, while a session is picked or a
//! subagent started, once none is. Interrupting a turn does not stop Codex's subagents, so Stop
//! interrupts every turn under the one it names, each parent before the subagents it started, and
//! any whose start is read while it waits. Each turn is interrupted once, however many Stops wait
//! for it: a Stop joins the interrupt already written, which keeps what Codex said of it until the
//! turn's end is read. A Stop is settled once, by the end of the last turn it waits for, or as
//! failed by Codex's refusal to stop one whose end is still unread. When the chat's session is
//! replaced its subagents are interrupted and their open items close, except one a Stop waits for:
//! that one is followed until Codex says what came of the interrupt and of its turn, which settle
//! the Stop, and its items close then. They close too when Codex stops.

use crate::{
    Direction, SEND_FILE, Setup, Translator, collaboration_mode_list, initialize, initialized,
    model_list, tool_reply,
};
use marmot_agent_driver::{
    Action, Answer, Capabilities, ChatId, Control, Driver, Emitted, FilesIn, Harness, MessageId,
    MidTurn, Output, Prompt, Ready, Reason, Refusal, RequestId, SendFile, Sessions,
};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, Command};

type Reader = Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>;

/// How long one of the core's requests waits for Codex before the driver gives up on it: to read
/// what the driver writes, to pick a session, or to answer a request before it; and how long
/// start-up waits for Codex to reply to its greeting.
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
    /// The interrupt of one turn, sent once for every Stop that waits for that turn: refused,
    /// they no longer wait for its end.
    Interrupt(Part),
}

/// A turn a Stop waits for: the subagent's thread it runs in, or none for the chat's session, and
/// the turn's own id.
type Part = (Option<String>, String);

/// How many messages for threads no chat knows yet are held, at most. Past it the oldest is let
/// go, and counted as routed nowhere.
const HELD: usize = 512;

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
    /// The subagent's thread whose translator the message is for, or none for the chat's session.
    who: Option<String>,
    text: String,
    request: Option<(i64, Purpose)>,
    settles: Vec<RequestId>,
}

/// A Stop under way in a chat (D-117). Codex does not stop a turn's subagents with it, and a
/// subagent's turn can start after the Stop was sent, from a start Codex sent before it: each is
/// stopped as its start is read, while the Stop waits.
#[derive(Clone)]
struct Stopping {
    /// The turn the Stop names: a subagent's turn under it is stopped too. `None` for a Stop of
    /// one subagent (D-149), which stops only the threads it names and those they start.
    root: Option<String>,
    /// The threads being stopped: a subagent one of them started is stopped too.
    threads: Vec<String>,
    /// The turns it waits for, still running.
    waits: Vec<Part>,
    /// The turns Codex refused to stop, with the event that showed the refusal. One whose end is
    /// still unread when the rest have ended means the Stop failed, and that event settles it.
    refused: Vec<(Part, Option<AgentEvent>)>,
}

/// The interrupt of one turn a Stop waits for (D-117): written once, however many Stops wait for
/// that turn, and kept, with what Codex said of it, until the turn's end is read. That a turn's
/// end is unread is what this says, not whether a translator still follows its thread.
struct Interrupt {
    /// The interrupt written and not answered yet.
    sent: Option<i64>,
    /// Codex refused it. A later Stop of that turn writes another.
    refused: bool,
}

/// What shows that a control has had its effect.
#[derive(Clone)]
enum Effect {
    /// Every turn a Stop interrupted has ended.
    Ended(Stopping),
    /// The session item shows this setting with this value.
    Setting(String, String),
    /// A turn started by the driver's request with this id has started.
    TurnStartedBy(i64),
    /// A session other than this one is running, started in this folder, the chat's (D-101).
    NewSession(Option<String>, Option<PathBuf>),
}

/// One chat: its translator, the controls waiting for their effect, and the folder its session
/// was started in, when the chat chose one (D-101).
#[derive(Default)]
struct Chat {
    translator: Translator,
    /// A translator for each subagent's thread shown in this chat (D-117), and the thread that
    /// started each.
    subagents: HashMap<String, Translator>,
    parents: HashMap<String, String>,
    effects: Vec<(RequestId, Effect)>,
    /// The interrupts of the turns Stops wait for, by turn.
    interrupts: HashMap<Part, Interrupt>,
    /// Subagents of a session the chat has left, which a Stop still waits for: each is followed
    /// only for what Codex says of its interrupt and of its turn's end, which settle the Stop.
    leaving: HashMap<String, Translator>,
    folder: Option<PathBuf>,
}

impl Chat {
    /// The translator for the chat's session, or for one of its subagents' threads.
    fn translator(&self, who: Option<&str>) -> Option<&Translator> {
        match who {
            None => Some(&self.translator),
            Some(thread) => (self.subagents.get(thread)).or_else(|| self.leaving.get(thread)),
        }
    }

    fn translator_mut(&mut self, who: Option<&str>) -> Option<&mut Translator> {
        match who {
            None => Some(&mut self.translator),
            Some(thread) => match self.subagents.get_mut(thread) {
                Some(translator) => Some(translator),
                None => self.leaving.get_mut(thread),
            },
        }
    }

    /// Whether a Stop still waits for a turn of `thread`'s, or holds Codex's refusal to stop one.
    fn awaited(&self, thread: &str) -> bool {
        let of = |(who, _): &Part| who.as_deref() == Some(thread);
        (self.effects.iter()).any(|(_, effect)| match effect {
            Effect::Ended(stopping) => {
                stopping.waits.iter().any(of) || stopping.refused.iter().any(|(p, _)| of(p))
            }
            _ => false,
        })
    }

    /// Whether `part` has an interrupt written that Codex has not refused: a Stop of it joins
    /// that one.
    fn interrupting(&self, part: &Part) -> bool {
        self.interrupts.get(part).is_some_and(|i| !i.refused)
    }

    /// How many threads up a subagent's thread is from the chat's session.
    fn depth(&self, thread: &str) -> usize {
        let mut depth = 0;
        let mut at = thread;
        while let Some(parent) = self.parents.get(at) {
            depth += 1;
            at = parent;
            if depth > self.parents.len() {
                break; // no thread starts its own parent; this only guards the walk
            }
        }
        depth
    }
}

/// A message for a thread no chat has had, held until something names the thread.
struct Held {
    thread: String,
    line: String,
    /// Codex's id, while the message is a request Codex waits for a reply to: none for a
    /// notification, or once Codex says the request is resolved.
    request: Option<Value>,
    /// A request Codex said is resolved while it was held: it is shown closed (#142).
    resolved: bool,
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
    /// Each subagent's thread, and the chat it is shown in (D-117).
    subthreads: HashMap<String, ChatId>,
    /// Every thread a chat has had, and the chat: a session belongs to its chat for good.
    owners: HashMap<String, ChatId>,
    /// The driver's requests Codex has not answered: the chat each is for, the subagent's thread
    /// if it is for one, and what for.
    requests: HashMap<i64, (ChatId, Option<String>, Purpose)>,
    /// The prompt behind each `turn/start` sent, until its turn starts or Codex refuses it. It is
    /// kept from the moment it is sent: Codex can announce the turn before it replies.
    starting: HashMap<i64, MessageId>,
    /// The prompts each running turn responds to.
    prompts: HashMap<(ChatId, String), Vec<MessageId>>,
    /// Messages naming a thread no chat has had, oldest first: a session being picked, or a
    /// subagent, can speak before the message that names its thread.
    held: VecDeque<Held>,
    /// Held messages are being released.
    releasing: bool,
    /// Subagents being started on Codex's older runtime, which names a subagent's thread only
    /// when the `spawnAgent` item completes: the thread starting each, and the item.
    spawning: HashSet<(String, String)>,
    /// The sessions being resumed: each speaks before Codex's reply makes it its chat's, though
    /// the chat has it already.
    resuming: HashSet<String>,
    /// The threads of subagents a chat has let go of that a Stop still waits for, and the chat.
    leaving: HashMap<String, ChatId>,
    /// The last `seq` handed over in each chat's session: a subagent's events are numbered with
    /// the session's own (D-059, point 4).
    seq: HashMap<(ChatId, Option<String>), u64>,
    /// Events, and files to send, read and not yet handed to the core.
    ready: Ready,
    /// Codex's calls of the driver's own tool that the core has not answered yet: the chat each
    /// came from, the subagent's thread if it was one's, and Codex's id for it, by the id the core
    /// was given.
    calls: HashMap<String, (ChatId, Option<String>, Value)>,
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
    /// Starts `codex app-server` from `program`, with Codex's own settings for subagents, and
    /// greets it.
    pub async fn start(program: &str, setup: Setup) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args(["app-server"])
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
            subthreads: HashMap::new(),
            owners: HashMap::new(),
            requests: HashMap::new(),
            starting: HashMap::new(),
            prompts: HashMap::new(),
            held: VecDeque::new(),
            releasing: false,
            spawning: HashSet::new(),
            resuming: HashSet::new(),
            leaving: HashMap::new(),
            seq: HashMap::new(),
            ready: Ready::default(),
            calls: HashMap::new(),
            gone: false,
            patience: PATIENCE,
            deadline: tokio::time::Instant::now() + PATIENCE,
            unrouted: 0,
        };
        let id = codex.id();
        codex.queue(&initialize(id), None);
        codex.flush().await?;
        // A Codex that never replies fails start-up, so the program can end and be started again.
        let deadline = tokio::time::Instant::now() + PATIENCE;
        let reply = loop {
            let read = tokio::time::timeout_at(deadline, codex.reader.next_line()).await;
            let Ok(line) = read else {
                return Err(io::Error::other("codex app-server did not reply in time"));
            };
            let Some(line) = line? else {
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
            who,
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
                // Every Stop of that turn from now joins this interrupt.
                Purpose::Interrupt(part) => {
                    let c = self.chats.entry(chat.clone()).or_default();
                    let interrupt = Interrupt {
                        sent: Some(id),
                        refused: false,
                    };
                    c.interrupts.insert(part.clone(), interrupt);
                }
                _ => {}
            }
            self.requests
                .insert(id, (chat.clone(), who.clone(), purpose));
        }
        self.feed(&chat, who.as_deref(), Direction::Out, &text, settles);
    }

    /// Sends a message for a chat and feeds it to that chat's translator; the events it makes
    /// settle `settles`. A request is kept, with what it is for, until Codex answers it.
    async fn send(
        &mut self,
        chat: &ChatId,
        who: Option<&str>,
        request: Option<(i64, Purpose)>,
        text: &str,
        settles: Vec<RequestId>,
    ) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::new(Reason::Failed));
        }
        let then = Written {
            chat: chat.clone(),
            who: who.map(String::from),
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

    /// Gives a message from Codex to the chat it is for: to the translator of the thread it names,
    /// the chat's session or one of its subagents'.
    fn route(&mut self, line: &str) {
        let message: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if message.get("method").is_some() {
            let thread = thread_of(line);
            let thread = thread.as_deref();
            let calls_tool = message["method"] == "item/tool/call" && message.get("id").is_some();
            let to = thread.and_then(|t| self.whose(t));
            let to = to.or_else(|| thread.and_then(|t| self.started_under(&message, t)));
            if let (Some(_), Some(t)) = (&to, thread) {
                self.spawns(t, &message);
            }
            let request = message.get("id").is_some();
            // A subagent the chat let go of, that a Stop waits for: only its turn's end is read.
            let leaving = (to.is_none().then_some(thread).flatten())
                .and_then(|t| self.leaving.get(t).map(|c| (c.clone(), t.to_string())));
            if let Some((chat, thread)) = leaving {
                if message["method"] == "turn/completed" {
                    self.feed(&chat, Some(&thread), Direction::In, line, Vec::new());
                } else {
                    self.unrouted += 1;
                    if request {
                        self.refuse_unrouted(&message["id"], Some(&thread));
                    }
                }
                self.let_leaving_go();
                self.release();
                return;
            }
            match to {
                Some((chat, who)) => {
                    self.feed(&chat, who.as_deref(), Direction::In, line, Vec::new());
                    if calls_tool {
                        self.tool_call(&chat, who.as_deref(), &message);
                    }
                    self.link(&chat, &message);
                }
                // Codex waits for a reply to its call, so one for no chat is refused.
                None if calls_tool => {
                    self.unrouted += 1;
                    let refused = Err(Refusal::new(Reason::Invalid));
                    self.queue(&tool_reply(&message["id"], &refused), None);
                }
                // A thread being picked, or a subagent's, can speak before what names it, as can
                // a session being resumed. A request is held only while something may still name
                // its thread.
                None if thread
                    .is_some_and(|t| !self.owners.contains_key(t) || self.resuming.contains(t))
                    && (!request || self.naming()) =>
                {
                    self.hold(thread.unwrap_or_default(), line, &message);
                }
                // Codex waits for a reply to its request, so one for no chat is refused.
                None => {
                    self.unrouted += 1;
                    if request {
                        self.refuse_unrouted(&message["id"], thread);
                    }
                }
            }
            self.release();
            return;
        }
        let id = message["id"].as_i64();
        let Some((chat, who, purpose)) = id.and_then(|id| self.requests.remove(&id)) else {
            self.unrouted += 1;
            return;
        };
        let refused = message.get("error").is_some();
        let mut settles = Vec::new();
        let mut refused_stop = None;
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
            // Codex answered the interrupt of a turn. Refused, no Stop waits for that turn any
            // longer: one that waited for no other is settled by the refusal, and the others keep
            // it, to settle them if that turn's end is still unread once the rest have ended.
            Purpose::Interrupt(part) => {
                let id = id.expect("a numbered reply");
                let c = self.chats.entry(chat.clone()).or_default();
                let interrupt = (c.interrupts.get_mut(&part)).filter(|i| i.sent == Some(id));
                if let Some(interrupt) = interrupt {
                    interrupt.sent = None;
                    interrupt.refused = refused;
                    if refused {
                        settles = self.no_longer_waits(&chat, &part);
                        refused_stop = Some(part);
                    }
                }
            }
        }
        let before = self.ready.len();
        self.feed(&chat, who.as_deref(), Direction::In, line, settles);
        if let Some(part) = refused_stop {
            let shown = (self.ready.iter().skip(before)).find_map(|output| match output {
                Output::Event(e) if matches!(e.event.body(), Body::Error(_)) => {
                    Some(e.event.clone())
                }
                _ => None,
            });
            self.refusal_shown(&chat, &part, shown);
        }
        self.let_leaving_go();
        self.release();
    }

    /// The chat a thread is shown in, and the subagent's thread if it is one's.
    fn whose(&self, thread: &str) -> Option<(ChatId, Option<String>)> {
        if let Some(chat) = self.threads.get(thread) {
            return Some((chat.clone(), None));
        }
        let chat = self.subthreads.get(thread)?;
        Some((chat.clone(), Some(thread.to_string())))
    }

    /// A thread Codex says started as a subagent of a thread a chat shows (`parentThreadId`, from
    /// Codex 0.171.0): it is that chat's, from now.
    fn started_under(&mut self, message: &Value, thread: &str) -> Option<(ChatId, Option<String>)> {
        if message["method"] != "thread/started" || self.owners.contains_key(thread) {
            return None;
        }
        let parent = message["params"]["thread"]["parentThreadId"].as_str()?;
        let (chat, _) = self.whose(parent)?;
        self.adopt(&chat, parent, thread);
        Some((chat, Some(thread.to_string())))
    }

    /// The subagents a message from one of a chat's threads names: a thread not seen before is a
    /// subagent of the thread that names it, shown in the same chat, and the item that names a
    /// subagent's thread to give it work is what its next turn is shown as working for.
    fn link(&mut self, chat: &ChatId, message: &Value) {
        let Some(from) = thread_of_value(message) else {
            return;
        };
        for (thread, item, description, gives_work) in subagents_named(message) {
            if thread == from {
                continue;
            }
            let known = self.subthreads.get(&thread) == Some(chat);
            if !known {
                if self.owners.contains_key(&thread) {
                    continue; // a session or a subagent another chat has, or had
                }
                self.adopt(chat, &from, &thread);
            }
            if known && !gives_work {
                continue;
            }
            let c = self.chats.get_mut(chat).expect("a chat that was fed");
            let shown = (c.subagents.get_mut(&thread))
                .map(|t| t.given_work(&item, description.as_deref()))
                .unwrap_or_default();
            for event in shown {
                self.ready_event(chat, event, Vec::new(), true, None);
            }
        }
    }

    /// Makes `thread` a subagent of `parent`, shown in `chat`, in the chat's session.
    fn adopt(&mut self, chat: &ChatId, parent: &str, thread: &str) {
        let c = self.chats.get_mut(chat).expect("a chat that was fed");
        let session = c.translator.session().map(String::from);
        // A subagent another subagent started is shown in that one's thread (D-121).
        let started_by = c.subagents.contains_key(parent).then_some(parent);
        let translator = Translator::subagent(thread, session.as_deref(), started_by);
        c.subagents.insert(thread.to_string(), translator);
        c.parents.insert(thread.to_string(), parent.to_string());
        self.subthreads.insert(thread.to_string(), chat.clone());
        self.owners.insert(thread.to_string(), chat.clone());
    }

    /// Holds a message for a thread no chat has had, until something names its thread. Codex's
    /// word that it no longer waits for a request held here means that request needs no reply: it
    /// is shown closed once its thread is named, never as waiting (#142), and that word is held
    /// like any notification.
    fn hold(&mut self, thread: &str, line: &str, message: &Value) {
        if message["method"] == "serverRequest/resolved" {
            let id = &message["params"]["requestId"];
            for held in self.held.iter_mut().filter(|h| h.thread == thread) {
                if held.request.as_ref() == Some(id) {
                    held.request = None;
                    held.resolved = true;
                }
            }
        }
        let request = (message.get("id").cloned()).filter(|_| message.get("method").is_some());
        self.held.push_back(Held {
            thread: thread.to_string(),
            line: line.to_string(),
            request,
            resolved: false,
        });
        while self.held.len() > HELD {
            let Some(held) = self.held.pop_front() else {
                break;
            };
            self.unrouted += 1;
            if let Some(id) = held.request {
                self.refuse_unrouted(&id, Some(&held.thread));
            }
        }
    }

    /// Whether something may still name a thread no chat knows yet: a session being picked, or a
    /// subagent being started.
    fn naming(&self) -> bool {
        !self.spawning.is_empty() || self.chats.values().any(|c| c.translator.picking())
    }

    /// Follows the subagents a chat's thread is starting on Codex's older runtime: from the
    /// `spawnAgent` item's start until it completes, or the thread's turn ends.
    fn spawns(&mut self, thread: &str, message: &Value) {
        let item = &message["params"]["item"];
        let key = (
            thread.to_string(),
            item["id"].as_str().unwrap_or_default().to_string(),
        );
        match message["method"].as_str() {
            Some("item/started")
                if item["type"] == "collabAgentToolCall" && item["tool"] == "spawnAgent" =>
            {
                self.spawning.insert(key);
            }
            Some("item/completed") => {
                self.spawning.remove(&key);
            }
            Some("turn/completed") => self.spawning.retain(|(t, _)| t != thread),
            _ => {}
        }
    }

    /// Refuses a request of Codex's that no chat takes, so Codex does not wait for it: invalid
    /// params, naming the thread, as Codex's own server refuses what it cannot take. Codex takes
    /// any error as a refusal (an approval denied, a question with no answers).
    fn refuse_unrouted(&mut self, id: &Value, thread: Option<&str>) {
        let mut error = json!({"code": -32602, "message": "Invalid params"});
        if let Some(thread) = thread {
            error["data"] = json!({"threadId": thread});
        }
        self.queue(&json!({"id": id, "error": error}).to_string(), None);
    }

    /// Once nothing may name a thread no chat knows yet, a request still held for one is refused.
    /// What is held stays, in case its thread is named later; what Codex waits for does not.
    fn let_go(&mut self) {
        if self.releasing || self.naming() || !self.held.iter().any(|h| h.request.is_some()) {
            return;
        }
        let mut refused = Vec::new();
        self.held.retain(|h| match &h.request {
            Some(id) => {
                refused.push((id.clone(), h.thread.clone()));
                false
            }
            None => true,
        });
        for (id, thread) in refused {
            self.unrouted += 1;
            self.refuse_unrouted(&id, Some(&thread));
        }
    }

    /// What a thread said before it was named goes to it now, before anything it says next, in
    /// the order it came, across every thread now named. What names a thread still unknown waits.
    fn release(&mut self) {
        if self.releasing {
            return; // the release under way goes on with what is now named
        }
        self.releasing = true;
        while let Some(at) = (self.held.iter()).position(|h| self.whose(&h.thread).is_some()) {
            let held = self.held.remove(at).expect("a held message");
            if held.resolved {
                self.withdrawn(&held.thread, &held.line);
            } else {
                self.route(&held.line);
            }
        }
        self.releasing = false;
        self.let_go();
    }

    /// Shows a request of `thread`'s that Codex said was resolved while it was held: closed, with
    /// no reply to Codex, which waits for none (#142).
    fn withdrawn(&mut self, thread: &str, line: &str) {
        let Some((chat, who)) = self.whose(thread) else {
            return;
        };
        let c = self.chats.entry(chat.clone()).or_default();
        let Some(translator) = c.translator_mut(who.as_deref()) else {
            return;
        };
        for event in translator.withdrawn(line) {
            self.ready_event(&chat, event, Vec::new(), true, None);
        }
    }

    /// Codex calls the driver's own tool. A call to send a file goes to the core; any other is
    /// refused at once, since Codex waits for the reply.
    fn tool_call(&mut self, chat: &ChatId, who: Option<&str>, message: &Value) {
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
                unanswered: false,
                ended: false,
            });
            let reply = tool_reply(&id, &refused);
            let then = Written {
                chat: chat.clone(),
                who: who.map(String::from),
                text: reply.clone(),
                request: None,
                settles: Vec::new(),
            };
            self.queue(&reply, Some(then));
            return;
        };
        // A path the agent gives relative to its working folder is made whole here: the core
        // takes only an absolute one. An absolute path stays as it is.
        let cwd = (self.chats.get(chat).and_then(|c| c.folder.clone()))
            .unwrap_or_else(|| PathBuf::from(&self.setup.cwd));
        let path = cwd.join(path);
        let key = id.to_string();
        self.calls
            .insert(key.clone(), (chat.clone(), who.map(String::from), id));
        self.ready.push(
            now(),
            Output::SendFile(SendFile {
                id: key,
                chat: chat.clone(),
                path,
            }),
        );
    }

    /// No Stop of the chat waits for `part` any longer: Codex refused to stop it. Those that waited
    /// for no other are taken off the list, to be settled by the refusal; the others remember it.
    fn no_longer_waits(&mut self, chat: &ChatId, part: &Part) -> Vec<RequestId> {
        let Some(c) = self.chats.get_mut(chat) else {
            return Vec::new();
        };
        let mut settled = Vec::new();
        c.effects.retain_mut(|(request, effect)| {
            let Effect::Ended(stopping) = effect else {
                return true;
            };
            if !stopping.waits.contains(part) {
                return true;
            }
            stopping.waits.retain(|p| p != part);
            if stopping.waits.is_empty() {
                settled.push(request.clone());
                return false;
            }
            stopping.refused.push((part.clone(), None));
            true
        });
        settled
    }

    /// The event that showed Codex refusing to stop `part`, for the Stops still waiting for others.
    fn refusal_shown(&mut self, chat: &ChatId, part: &Part, shown: Option<AgentEvent>) {
        let Some(c) = self.chats.get_mut(chat) else {
            return;
        };
        for (_, effect) in &mut c.effects {
            if let Effect::Ended(stopping) = effect {
                for (p, event) in &mut stopping.refused {
                    if p == part && event.is_none() {
                        *event = shown.clone();
                    }
                }
            }
        }
    }

    /// A subagent's turn started in a chat while Stops there are under way, under the turn one
    /// stops or, when Codex names no such turn, started by a thread one stops: it is stopped too,
    /// once, and every such Stop waits for it (D-117).
    fn stop_late(&mut self, chat: &ChatId, thread: &str, turn: &str) {
        let Some(c) = self.chats.get_mut(chat) else {
            return;
        };
        let root = (c.subagents.get(thread)).and_then(|t| t.named_root_turn().map(String::from));
        let parent = c.parents.get(thread).cloned();
        let part: Part = (Some(thread.to_string()), turn.to_string());
        let mut waited = false;
        for (_, effect) in &mut c.effects {
            let Effect::Ended(stopping) = effect else {
                continue;
            };
            let started_by_one = (parent.as_ref()).is_some_and(|p| stopping.threads.contains(p));
            let under = match (&root, &stopping.root) {
                (Some(root), Some(stopped)) => root == stopped,
                (None, Some(_)) => started_by_one,
                // A Stop of one subagent: its own next turn, or one of a subagent it started.
                (_, None) => started_by_one || stopping.threads.iter().any(|t| t == thread),
            };
            if under && !stopping.waits.contains(&part) {
                stopping.waits.push(part.clone());
                stopping.threads.push(thread.to_string());
                waited = true;
            }
        }
        if !waited || c.interrupting(&part) {
            return;
        }
        let id = self.id();
        let action = Action::Interrupt {
            turn: turn.into(),
            subagent: None,
        };
        let c = &self.chats[chat];
        let built = (c.translator(Some(thread))).map(|t| t.control(id, &action, &self.setup));
        let Some(Ok(text)) = built else {
            return; // it can always be built for a turn running; the Stop waits for its end
        };
        let then = Written {
            chat: chat.clone(),
            who: Some(thread.to_string()),
            text: text.clone(),
            request: Some((id, Purpose::Interrupt(part))),
            settles: Vec::new(),
        };
        self.queue(&text, Some(then));
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

    /// Feeds a message to a translator of a chat's, the session's or a subagent's, makes its
    /// events ready, and sends the replies that need nobody's answer, such as declining a form.
    fn feed(
        &mut self,
        chat: &ChatId,
        who: Option<&str>,
        direction: Direction,
        text: &str,
        settles: Vec<RequestId>,
    ) {
        let c = self.chats.entry(chat.clone()).or_default();
        let Some(translator) = c.translator_mut(who) else {
            return;
        };
        let before = translator.session().map(String::from);
        let turn = translator.turn().map(String::from);
        let events = translator.feed(direction, text);
        let after = translator.session().map(String::from);
        let started = (translator.turn())
            .filter(|t| turn.as_deref() != Some(*t))
            .map(String::from);
        // The turn whose end Codex reports is the one it names, which need not be the current
        // one: an end read after the next turn started is still that turn's (#143).
        let named = (direction == Direction::In && text.contains("turn/completed"))
            .then(|| serde_json::from_str::<Value>(text).ok())
            .flatten()
            .filter(|m| m["method"] == "turn/completed")
            .and_then(|m| m["params"]["turn"]["id"].as_str().map(String::from));
        let reported = named.is_some();
        let ended = named.or_else(|| turn.filter(|t| translator.turn() != Some(t.as_str())));
        let replies = translator.take_replies();
        if before != after && who.is_none() {
            if let Some(old) = before {
                self.threads.remove(&old);
                self.spawning.retain(|(t, _)| *t != old); // what it starts, nothing will name
                self.let_subagents_go(chat); // its session has ended (D-070)
            }
            if let Some(new) = after {
                self.owners.insert(new.clone(), chat.clone());
                self.threads.insert(new, chat.clone());
            }
        }
        // The event that shows a turn's end: the last turn or subagent's task that ended, or, for
        // the end of a subagent's earlier turn read while its next one runs, its task, which
        // stays open for that next turn (D-142).
        let ending = ended.as_ref().and_then(|_| {
            let closed = events.iter().rposition(|e| {
                matches!(e.body(), Body::Turn(_) | Body::Task(_))
                    && !matches!(e.header().state, State::Running | State::Waiting)
            });
            closed.or_else(|| {
                let late = reported && who.is_some();
                (events.iter()).rposition(|e| late && matches!(e.body(), Body::Task(_)))
            })
        });
        let ended = ended.map(|turn| (who.map(String::from), turn));
        let mut settles = settles;
        for (n, event) in events.into_iter().enumerate() {
            let shows = if ending == Some(n) {
                ended.as_ref()
            } else {
                None
            };
            self.ready_event(chat, event, std::mem::take(&mut settles), true, shows);
        }
        if let (Some(thread), Some(turn)) = (who, started) {
            self.stop_late(chat, thread, &turn);
        }
        for reply in replies {
            let then = Written {
                chat: chat.clone(),
                who: who.map(String::from),
                text: reply.clone(),
                request: None,
                settles: Vec::new(),
            };
            self.queue(&reply, Some(then));
        }
    }

    /// The chat's session has ended: its subagents are stopped, since Codex does not stop them
    /// with it, their open items close, and nothing more they say is shown (D-070, D-117). One a
    /// Stop waits for is not interrupted again, and is followed until Codex says what came of
    /// that Stop's interrupt and of its turn, which settle the Stop; its items close then.
    fn let_subagents_go(&mut self, chat: &ChatId) {
        let Some(c) = self.chats.get_mut(chat) else {
            return;
        };
        let subagents = std::mem::take(&mut c.subagents);
        c.parents.clear();
        let mut closed = Vec::new();
        let mut threads: Vec<_> = subagents.into_iter().collect();
        threads.sort_by(|a, b| a.0.cmp(&b.0));
        for (thread, mut translator) in threads {
            self.subthreads.remove(&thread);
            self.spawning.retain(|(t, _)| *t != thread); // what it starts, nothing will name
            let c = self.chats.get_mut(chat).expect("a chat");
            if c.awaited(&thread) {
                c.leaving.insert(thread.clone(), translator);
                self.leaving.insert(thread, chat.clone());
                continue;
            }
            c.interrupts
                .retain(|(who, _), _| who.as_deref() != Some(thread.as_str()));
            if let Some(turn) = translator.turn().map(String::from) {
                let id = self.id();
                let action = Action::Interrupt {
                    turn: turn.as_str().into(),
                    subagent: None,
                };
                if let Ok(text) = translator.control(id, &action, &self.setup) {
                    let then = Written {
                        chat: chat.clone(),
                        who: Some(thread.clone()),
                        text: text.clone(),
                        request: Some((id, Purpose::Plain)),
                        settles: Vec::new(),
                    };
                    self.queue(&text, Some(then));
                }
            }
            closed.extend(translator.harness_stopped());
        }
        for event in closed {
            self.ready_event(chat, event, Vec::new(), false, None);
        }
    }

    /// Subagents a chat let go of, that no Stop waits for any longer: their open items close, and
    /// nothing more they say is read.
    fn let_leaving_go(&mut self) {
        if self.leaving.is_empty() {
            return;
        }
        let leaving: Vec<(String, ChatId)> = self.leaving.clone().into_iter().collect();
        for (thread, chat) in leaving {
            let Some(c) = self.chats.get_mut(&chat) else {
                self.leaving.remove(&thread);
                continue;
            };
            if c.awaited(&thread) {
                continue;
            }
            self.leaving.remove(&thread);
            c.interrupts
                .retain(|(who, _), _| who.as_deref() != Some(thread.as_str()));
            let closed = (c.leaving.remove(&thread))
                .map(|mut t| t.harness_stopped())
                .unwrap_or_default();
            for event in closed {
                self.ready_event(&chat, event, Vec::new(), false, None);
            }
        }
    }

    /// Makes an event ready for the core. An event Codex sent shows the effect of a control; one
    /// the driver made when Codex stopped does not (`seen` false): that control failed, and the
    /// core settles it so (D-058).
    /// `ended` is the turn whose end the event shows, if it shows one: the chat's own or a
    /// subagent's.
    fn ready_event(
        &mut self,
        chat: &ChatId,
        event: AgentEvent,
        mut settles: Vec<RequestId>,
        seen: bool,
        ended_turn: Option<&Part>,
    ) {
        let event = self.numbered(chat, event);
        let c = self.chats.get_mut(chat).expect("a chat that was fed");
        let header = event.header();
        let turn = header.turn.as_ref().map(|t| t.as_str().to_string());
        let ended = !matches!(header.state, State::Running | State::Waiting);
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        let mut moved = None;
        let mut failed = Vec::new();
        // A turn whose end Codex shows needs no interrupt any longer.
        if let (true, Some(part)) = (seen, ended_turn) {
            c.interrupts.remove(part);
        }
        let (translator, interrupts) = (&c.translator, &c.interrupts);
        c.effects.retain_mut(|(request, effect)| {
            let shown = seen
                && match (effect, event.body()) {
                    (Effect::Ended(stopping), _) => match ended_turn {
                        Some(part) if stopping.waits.contains(part) => {
                            stopping.waits.retain(|p| p != part);
                            if !stopping.waits.is_empty() {
                                return true;
                            }
                            // A turn Codex refused to stop, whose end is unread, runs on: the
                            // Stop failed, and the refusal settles it, once.
                            let failure = (stopping.refused.iter())
                                .filter(|(p, _)| interrupts.contains_key(p))
                                .find_map(|(_, refusal)| refusal.clone());
                            if let Some(refusal) = failure {
                                failed.push((request.clone(), refusal));
                                return false;
                            }
                            true
                        }
                        _ => false,
                    },
                    (Effect::Setting(id, value), Body::Session(s)) => (s.settings.iter())
                        .any(|setting| setting.id == *id && setting.value.as_ref() == Some(value)),
                    (Effect::TurnStartedBy(r), Body::Turn(_)) => {
                        let started = turn.as_deref().and_then(|t| translator.turn_request(t));
                        started == Some(*r)
                    }
                    (Effect::NewSession(old, folder), Body::Session(_)) => {
                        let new = header.state == State::Running && session != *old;
                        if new {
                            moved = Some(folder.clone());
                        }
                        new
                    }
                    _ => false,
                };
            if shown {
                settles.push(request.clone());
            }
            !shown
        });
        // The chat works in the new session's folder once Codex has started it.
        if let Some(folder) = moved {
            c.folder = folder;
        }
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
        self.ready.push(
            now(),
            Output::Event(Emitted {
                event,
                chat: Some(chat.clone()),
                prompts,
                settles,
            }),
        );
        for (request, refusal) in failed {
            self.ready_event(chat, refusal, vec![request], false, None);
        }
    }

    /// An event numbered in its chat's session: the chat's own translator and its subagents' make
    /// events in the same session, and `seq` rises across them all.
    fn numbered(&mut self, chat: &ChatId, event: AgentEvent) -> AgentEvent {
        let mut header = event.header().clone();
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        let seq = self.seq.entry((chat.clone(), session)).or_default();
        *seq += 1;
        if header.seq == *seq {
            return event;
        }
        header.seq = *seq;
        match AgentEvent::new(header, event.body().clone(), event.native().cloned()) {
            Ok(numbered) => numbered,
            Err(_) => event, // it was made once, with the same body and native message
        }
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
            let mut events = c.translator.harness_stopped();
            for translator in c.subagents.values_mut() {
                events.extend(translator.harness_stopped());
            }
            for (_, mut translator) in std::mem::take(&mut c.leaving) {
                events.extend(translator.harness_stopped());
            }
            c.interrupts.clear();
            for event in events {
                self.ready_event(&chat, event, Vec::new(), false, None);
            }
        }
        self.requests.clear();
        self.starting.clear();
        self.held.clear();
        self.spawning.clear();
        self.leaving.clear();
        self.calls.clear(); // a reply has nobody to go to
        self.unwritten.clear(); // never written: nothing follows from them
        self.unflushed.clear();
    }
}

/// The thread a message from Codex names, if it names one.
fn thread_of(line: &str) -> Option<String> {
    let message: Value = serde_json::from_str(line).ok()?;
    thread_of_value(&message)
}

fn thread_of_value(message: &Value) -> Option<String> {
    let params = &message["params"];
    let thread = params["threadId"]
        .as_str()
        .or(params["thread"]["id"].as_str());
    thread.map(String::from)
}

/// The subagents' threads an item names: on Codex's newer runtime the one a `subAgentActivity`
/// is about (`agentThreadId`), and on the older the ones a tool for subagents names
/// (`receiverThreadIds`). For each: the item, what the subagent is for (its path, or what it was
/// asked), and whether the item gives it work, so that its next turn works for that item.
fn subagents_named(message: &Value) -> Vec<(String, String, Option<String>, bool)> {
    if !matches!(
        message["method"].as_str(),
        Some("item/started" | "item/completed")
    ) {
        return Vec::new();
    }
    let item = &message["params"]["item"];
    let id = item["id"].as_str().unwrap_or_default().to_string();
    let text = |key: &str| item[key].as_str().map(String::from);
    match item["type"].as_str() {
        Some("subAgentActivity") => {
            let gives_work = matches!(item["kind"].as_str(), Some("started" | "interacted"));
            let thread = text("agentThreadId");
            (thread.into_iter())
                .map(|t| (t, id.clone(), text("agentPath"), gives_work))
                .collect()
        }
        Some("collabAgentToolCall") => {
            let gives_work = matches!(
                item["tool"].as_str(),
                Some("spawnAgent" | "sendInput" | "sendMessage" | "followupTask" | "resumeAgent")
            );
            let threads = item["receiverThreadIds"].as_array().into_iter().flatten();
            (threads.filter_map(Value::as_str))
                .map(|t| (t.to_string(), id.clone(), text("prompt"), gives_work))
                .collect()
        }
        _ => Vec::new(),
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
        // Both are queued before either is written, so a write cut short loses neither.
        for list in [model_list as fn(i64) -> String, collaboration_mode_list] {
            let id = self.id();
            let text = list(id);
            let then = Written {
                chat: chat.clone(),
                who: None,
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
        self.ask_in(chat, None, purpose, build).await
    }

    /// [`Codex::ask`], of the translator for one of the chat's subagents' threads, or, with none,
    /// for its session.
    async fn ask_in(
        &mut self,
        chat: &ChatId,
        who: Option<&str>,
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
            let c = self.chats.get(chat).expect("a chat");
            let translator = c.translator(who).ok_or(Refusal::new(Reason::NothingToDo))?;
            match build(translator, id) {
                Ok(text) => {
                    let purpose = purpose(translator, id);
                    self.next_id += 1;
                    self.send(chat, who, Some((id, purpose)), &text, Vec::new())
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

    /// The setup for a session in `folder`, the chat's, or the driver's own when it is none
    /// (D-101).
    fn setup_in(&self, folder: Option<&Path>) -> Setup {
        let mut setup = self.setup.clone();
        if let Some(folder) = folder {
            setup.cwd = folder.to_string_lossy().into_owned();
        }
        setup
    }

    /// Makes `session` the chat's session, starting a new one in `folder` if it is none, and waits
    /// for Codex.
    async fn pick(
        &mut self,
        chat: &ChatId,
        session: Option<&str>,
        folder: Option<&Path>,
    ) -> Result<(), Refusal> {
        // A session to resume is the chat's from the moment it is asked for, so no other chat can
        // ask for it meanwhile; if Codex refuses, a reservation made here is let go.
        let reserved = session
            .filter(|t| !self.owners.contains_key(*t))
            .map(String::from);
        if let Some(thread) = &reserved {
            self.owners.insert(thread.clone(), chat.clone());
        }
        let resuming = session.map(String::from);
        self.resuming.extend(resuming.clone());
        let picked = self.pick_now(chat, session, folder).await;
        if let Some(thread) = resuming {
            self.resuming.remove(&thread);
            // What it said waits no longer: the reply made it the chat's, or Codex refused it.
            if self.whose(&thread).is_none() {
                let mut dropped = Vec::new();
                self.held.retain(|h| {
                    let keep = h.thread != thread;
                    if !keep {
                        dropped.push(h.request.clone());
                    }
                    keep
                });
                for request in dropped {
                    self.unrouted += 1;
                    if let Some(id) = request {
                        self.refuse_unrouted(&id, Some(&thread));
                    }
                }
            }
        }
        if picked.is_err()
            && let Some(thread) = reserved
            && self.chats[chat].translator.session() != Some(thread.as_str())
        {
            self.owners.remove(&thread);
        }
        picked
    }

    async fn pick_now(
        &mut self,
        chat: &ChatId,
        session: Option<&str>,
        folder: Option<&Path>,
    ) -> Result<(), Refusal> {
        let setup = self.setup_in(folder);
        let build = |t: &Translator, id| t.pick(id, session, &setup);
        let id = self.ask(chat, |_, _| Purpose::Plain, build).await?;
        while self.requests.contains_key(&id) {
            if !self.read_in_time().await {
                return Err(Refusal::new(Reason::Failed));
            }
        }
        let now = self.chats[chat].translator.session().map(String::from);
        let picked = match (now, session) {
            (Some(now), Some(wanted)) if now == wanted => Ok(()),
            (Some(_), None) => Ok(()),
            _ => Err(Refusal::new(Reason::Failed)), // Codex refused it; the chat shows why
        };
        if picked.is_ok()
            && let Some(c) = self.chats.get_mut(chat)
        {
            c.folder = folder.map(Path::to_path_buf);
        }
        picked
    }

    /// The chat that has the answer's item waiting, and the reply to send it, or else the most
    /// telling refusal.
    /// The item may be a subagent's, shown in its parent's chat (D-117).
    fn chat_with_item(&self, answer: &Answer) -> Result<(ChatId, Option<String>, String), Refusal> {
        let mut refusal = Refusal::new(Reason::UnknownItem);
        for (chat, c) in &self.chats {
            let translators = std::iter::once((None, &c.translator))
                .chain(c.subagents.iter().map(|(t, tr)| (Some(t.clone()), tr)));
            for (who, translator) in translators {
                match translator.reply(answer) {
                    Ok(reply) => return Ok((chat.clone(), who, reply)),
                    Err(r) if r.reason == Reason::UnknownItem => {}
                    Err(r) => refusal = r,
                }
            }
        }
        Err(refusal)
    }

    /// The turns a Stop of `turn` stops (D-117): the chat's turn, if it is `turn` and running,
    /// and every subagent's running turn under it, by the turn the person started or by the links
    /// from a thread being stopped to the subagents it started; each parent before the subagents
    /// it started, so none is told to go on by a subagent that ended first.
    fn under(&self, turn: &str) -> Option<(ChatId, Stopping)> {
        for (chat, c) in &self.chats {
            let mut parts = Vec::new();
            let mut stopping: Vec<String> = Vec::new();
            if c.translator.turn() == Some(turn) {
                parts.push((None, turn.to_string()));
                stopping.extend(c.translator.session().map(String::from));
            }
            let mut threads: Vec<&String> = c.subagents.keys().collect();
            threads.sort_by_key(|t| (c.depth(t), t.to_string()));
            for thread in threads {
                let translator = &c.subagents[thread];
                let Some(own) = translator.turn() else {
                    continue;
                };
                // The turn Codex says it is under decides; only a subagent it names none for is
                // taken by the thread that started it.
                let under = match translator.named_root_turn() {
                    Some(root) => root == turn,
                    None => {
                        own == turn || (c.parents.get(thread)).is_some_and(|p| stopping.contains(p))
                    }
                };
                if under {
                    parts.push((Some(thread.clone()), own.to_string()));
                    stopping.push(thread.clone());
                }
            }
            if !parts.is_empty() {
                let stopping = Stopping {
                    root: Some(turn.to_string()),
                    threads: stopping,
                    waits: parts,
                    refused: Vec::new(),
                };
                return Some((chat.clone(), stopping));
            }
        }
        None
    }

    /// The turns a Stop of the subagent whose `task` item is `task` stops (D-123, D-149): that
    /// subagent's running turn and those of the subagents it started, at any depth, each parent
    /// before the subagents it started. Nothing else: not the person's turn, nor another subagent.
    fn under_subagent(&self, task: &str) -> Option<(ChatId, Stopping)> {
        for (chat, c) in &self.chats {
            let Some(top) = (c.subagents.iter())
                .find(|(_, t)| t.task_item().as_deref() == Some(task))
                .map(|(thread, _)| thread.clone())
            else {
                continue;
            };
            let mut threads: Vec<&String> = c.subagents.keys().collect();
            threads.sort_by_key(|t| (c.depth(t), t.to_string()));
            let mut stopping = vec![top];
            let mut parts = Vec::new();
            for thread in threads {
                let own = *thread == stopping[0];
                let started = (c.parents.get(thread)).is_some_and(|p| stopping.contains(p));
                if !own && !started {
                    continue;
                }
                if !own {
                    stopping.push(thread.clone());
                }
                if let Some(turn) = c.subagents[thread].turn() {
                    parts.push((Some(thread.clone()), turn.to_string()));
                }
            }
            if parts.is_empty() {
                return None;
            }
            let stopping = Stopping {
                root: None,
                threads: stopping,
                waits: parts,
                refused: Vec::new(),
            };
            return Some((chat.clone(), stopping));
        }
        None
    }

    /// Stops every turn the Stop waits for, of `chat`, settled once all have ended (D-028, D-117).
    async fn stop(
        &mut self,
        request: &RequestId,
        chat: &ChatId,
        stopping: Stopping,
    ) -> Result<(), Refusal> {
        let parts = stopping.waits.clone();
        let c = self.chats.get_mut(chat).expect("a chat");
        c.effects.push((request.clone(), Effect::Ended(stopping)));
        for part in parts {
            // What is queued is written first: an interrupt of this turn already sent is joined.
            if self.flush_in_time().await.is_err() {
                self.stopped();
                return Err(Refusal::new(Reason::Failed));
            }
            if self.chats[chat].interrupting(&part) {
                continue;
            }
            let (who, turn) = part.clone();
            let action = Action::Interrupt {
                turn: turn.as_str().into(),
                subagent: None,
            };
            let setup = self.setup.clone();
            let build = |t: &Translator, id| t.control(id, &action, &setup);
            let purpose = |_: &Translator, _| Purpose::Interrupt(part.clone());
            if let Err(refusal) = self.ask_in(chat, who.as_deref(), purpose, build).await {
                self.unsettled(chat, request); // refused: nothing settles it later
                return Err(refusal);
            }
        }
        Ok(())
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
            subagents: true,
            stop_subagent: true,
            approval_commands: Vec::new(),
        }
    }

    fn folder(&self) -> Option<PathBuf> {
        Some(PathBuf::from(&self.setup.cwd))
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::ended());
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
            let folder = prompt.chat.folder.clone();
            self.pick(&chat, wanted.as_deref(), folder.as_deref())
                .await?;
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
            return Err(Refusal::ended());
        }
        self.begin();
        // A message cut short is finished first: it may be an answer to the same item.
        if self.flush_in_time().await.is_err() {
            self.stopped();
            return Err(Refusal::new(Reason::Failed));
        }
        let (chat, who, text) = self.chat_with_item(&answer)?;
        // The reply's event, fed back, shows the answer: it settles it.
        let settles = vec![answer.request];
        self.send(&chat, who.as_deref(), None, &text, settles).await
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        if self.gone {
            return Err(Refusal::ended());
        }
        self.begin();
        let request = control.request.clone();
        let (chat, effect) = match &control.action {
            // The turn and every subagent under it, which Codex does not stop with it (D-117).
            // Or one subagent and those it started, and nothing else (D-149).
            Action::Interrupt { turn, subagent } => {
                let stopping = match subagent {
                    Some(task) => self.under_subagent(task.as_str()),
                    None => self.under(turn.as_str()),
                };
                let (chat, stopping) = stopping.ok_or_else(|| Refusal::new(Reason::NothingToDo))?;
                return self.stop(&request, &chat, stopping).await;
            }
            Action::NewSession { chat, session } => {
                let folder = chat.folder.clone();
                let chat = chat.id.clone();
                self.chat(&chat).await?;
                let old = session.as_ref().map(|s| s.as_str().to_string());
                (chat, Effect::NewSession(old, folder))
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
        // A new session starts in the chat's folder (D-101).
        let folder = match &effect {
            Effect::NewSession(_, folder) => folder.clone(),
            _ => None,
        };
        let setup = self.setup_in(folder.as_deref());
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
            if let Some(event) = self.ready.pop() {
                return Some(event);
            }
            if !self.read().await {
                return self.ready.pop();
            }
        }
    }

    fn arrived(&self) -> Option<std::time::Instant> {
        self.ready.arrived()
    }

    async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
        let Some((chat, who, codex_id)) = self.calls.remove(&id) else {
            return;
        };
        let reply = tool_reply(&codex_id, &result);
        self.begin();
        // A reply Codex cannot be given is lost with Codex, which `send` notes.
        let _ = (self.send(&chat, who.as_deref(), None, &reply, Vec::new())).await;
    }
}

/// Now, on the runtime's clock, as [`Driver::arrived`] gives it.
fn now() -> std::time::Instant {
    tokio::time::Instant::now().into_std()
}
