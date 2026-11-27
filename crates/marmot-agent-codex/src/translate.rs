//! What Codex says, turned into agent events.
//!
//! The translator reads every message on the connection, in both directions, in the order they
//! went. It needs the client's side too: a reply to Codex's approval request is how it knows the
//! answer, and the request that went before a turn says who started it. It does no input or
//! output of its own, so it is tested by replaying recordings
//! (`fixtures/recordings/codex`), and must produce the events the prototype produces for each
//! (`fixtures/recordings/codex/expected`).
//!
//! The native message of an event is the whole `params` of Codex's message, or the whole `result`
//! of a reply, as the exact text that arrived. An event the driver makes up, such as a command it
//! closes because Codex never did, carries none. Nothing is cut to fit a size: the core applies
//! the limit (D-057).
//!
//! `items` translates item and plan notifications. `interactions` handles approvals and
//! request-backed questions in both directions. `session` handles replies from Codex and
//! session settings.

mod interactions;
mod items;
mod session;

use self::interactions::Pending;
use crate::messages::{
    self, Envelope, ErrorNote, Id, Message as NativeMessage, RequestResolved, Rerouted,
    ThreadSettingsUpdated, ThreadStarted, TokenUsageUpdated, Tokens, TurnNote,
};
use crate::send::{self, Setup};
use marmot_agent_driver::{Action as DriverAction, Prompt, Reason, Refusal};
use marmot_agent_events::body::{
    AnswerVia, Error, Notice, NoticeKind, Session, StartedBy, Task, Turn, Usage,
};
use marmot_agent_events::{AgentEvent, Body, Header, Native, State};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// Which way a message went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From Codex to the driver.
    In,
    /// From the driver to Codex.
    Out,
}

/// Codex's requests the driver asks someone in the chat about, or answers itself.
const REQUESTS: &[&str] = &[
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "item/permissions/requestApproval",
    "item/tool/requestUserInput",
    "mcpServer/elicitation/request",
];

/// Messages Codex sends that never become an event of their own. Some still change what the
/// driver knows: the settings, the thread, the tokens a turn used (rule 11 of the event model), or
/// close a request Codex no longer waits on.
pub const NOT_SENT: &[&str] = &[
    "initialized",
    "thread/started",
    "thread/status/changed",
    "thread/settings/updated",
    "thread/tokenUsage/updated",
    "account/updated",
    "account/rateLimits/updated",
    "remoteControl/status/changed",
    "mcpServer/startupStatus/updated",
    "turn/moderationMetadata",
    "turn/diff/updated",
    "serverRequest/resolved",
    "skills/changed",
    "thread/name/updated",
    "thread/closed",
    "thread/environment/connected",
    "model/verification",
    "thread/readState/changed",
    "thread/goal/cleared",
    "thread/goal/updated",
    "deprecationNotice",
];

#[derive(Default)]
struct Known {
    model: Option<String>,
    effort: Option<String>,
    approval_policy: Option<String>,
    sandbox: Option<String>,
    mode: Option<String>,
    version: Option<String>,
}

/// A request the client sent: its method, and the session and turn it was sent in.
struct Sent {
    method: String,
    owner: (Option<String>, Option<String>),
}

/// What a translator that follows a subagent's thread shows it as (D-117, D-121): in the chat of
/// the turn that started it, in that chat's session and under the turn the person started, as one
/// `task` item, its row, which each of its turns opens again; everything else it does names that
/// item as the subagent it belongs to, so the core puts it in the subagent's thread; and its words
/// name the item that gave it its work.
#[derive(Clone, Default)]
struct Subagent {
    /// The chat's session, which every event names.
    session: Option<String>,
    /// For a subagent another subagent started: that one's `task` item, which this one's names
    /// as the subagent it belongs to (D-121).
    parent: Option<String>,
    /// The item that gave the subagent its work, as the chat shows it: what started it, or what
    /// gave it more.
    subject: Option<String>,
    /// What it is for: its path, or what it was asked.
    description: Option<String>,
    /// Whether `subject` was given for the turn running now. A turn that starts before the item
    /// that gives it work shows the last turn's until that item comes (#146).
    claimed: bool,
    /// Every item that has given it work. Codex names each more than once (its start, its end),
    /// and only the first gives work: a repeat read late, after newer work, gives none (#146).
    given: HashSet<String>,
    /// The turn the person started that its last turn ran under: what it says between turns
    /// belongs to.
    root: Option<String>,
    /// The work its next turn is for: the item that gave it, and what it is for. The newest
    /// given, while a turn runs or between turns, and taken when the next turn starts.
    next: Option<(String, Option<String>)>,
    /// The newest of its turns to start, whose state the row shows: the end of an earlier one,
    /// read late, does not change it.
    latest: Option<String>,
    /// The row as the latest turn last showed it, for an earlier turn's end read late: its body,
    /// state, title and text, before what [`Subagent::failed`] adds.
    row: Option<(Body, State, String, String)>,
    /// What people need to know of earlier turns whose ends were read late: each that failed, with
    /// its error. The row's text says it from then on.
    failed: Vec<String>,
}

impl Subagent {
    /// The row's text: `text`, and what earlier turns that failed left to say.
    fn row_text(&self, text: &str) -> String {
        std::iter::once(text.to_string())
            .chain(self.failed.iter().cloned())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Turns the messages of one chat's Codex session into agent events.
///
/// A translator follows one Codex thread at a time: the chat's session, which the client picks with
/// `thread/start` or `thread/resume` and can replace, or a subagent's thread
/// ([`Translator::subagent`]). One connection can carry several threads, and it is the transport's
/// job to give each translator only its own thread's messages. One that names another thread makes
/// no event here and is counted ([`Translator::other_threads`]).
#[derive(Default)]
pub struct Translator {
    /// Set when the thread followed is a subagent's.
    subagent: Option<Subagent>,
    /// For each of a subagent's turns, the turn the person started (`rootTurnId`).
    roots: HashMap<String, String>,
    /// Every event made so far. Some made-up item ids use it.
    made: u64,
    /// Events made in each session, for each event's `seq`.
    seq: HashMap<Option<String>, u64>,
    session: Option<String>,
    turn: Option<String>,
    /// Items not yet finished, in the order they started, with their last event.
    open: Vec<(String, AgentEvent)>,
    /// Each session item's body as it was last sent.
    sessions: HashMap<String, Session>,
    /// The client's requests that have not been answered, by id.
    requests: HashMap<Id, Sent>,
    /// For each turn a request started, that request's id.
    turn_requests: HashMap<String, i64>,
    /// Items answered by the client's reply, so a second answer is told so. Cleared when the
    /// session is replaced.
    answered: HashSet<String>,
    /// Replies the driver sends without asking anyone, for the transport to send and feed back.
    outbox: Vec<String>,
    /// Changes of settings sent that Codex has not answered yet, by request: the model and the
    /// effort each asks for. Codex answers every request, so none can wait for ever.
    asked_settings: HashMap<Id, (Option<String>, Option<String>)>,
    /// Codex's requests that the client has not answered, by id.
    asked: HashMap<Id, Pending>,
    /// For each turn, the thread's tokens before its first model call, and the thread's total
    /// since. `last` in Codex's notice is one model call; a turn used the increase in the total.
    usage: HashMap<Option<String>, (Tokens, Tokens)>,
    /// The client's requests that will start a turn, oldest first: who they count as, and the
    /// thread they name.
    starts: Vec<(Id, StartedBy, Option<String>)>,
    /// Who started each turn that has not ended: the end of one read late is still its own.
    started_by: HashMap<Option<String>, StartedBy>,
    known: Known,
    models: Option<Vec<String>>,
    modes: Option<Vec<String>>,
    /// Messages that could not be read as JSON-RPC at all.
    unreadable: u64,
    /// Messages for a thread that is not the session.
    other_threads: u64,
    /// Methods with no rule. Each is a gap in the driver.
    unknown: Vec<String>,
}

impl Translator {
    /// A translator for a new connection.
    pub fn new() -> Self {
        Self::default()
    }

    /// A translator for a subagent's `thread`, shown in the chat whose session is `session`
    /// (D-117). It never picks a session: the thread is the subagent's from the start. `parent`
    /// is the subagent's thread that started it, for a subagent started by another.
    pub fn subagent(thread: &str, session: Option<&str>, parent: Option<&str>) -> Self {
        Self {
            session: Some(thread.to_string()),
            subagent: Some(Subagent {
                session: session.map(String::from),
                parent: parent.map(task_of),
                ..Subagent::default()
            }),
            ..Self::default()
        }
    }

    /// For a subagent's thread, its `task` item: its row in the chat (D-121).
    pub fn task_item(&self) -> Option<String> {
        self.subagent.as_ref()?;
        Some(task_of(self.session.as_deref().unwrap_or_default()))
    }

    /// The item that gave a subagent its work, and what that work is, as its next turn shows them:
    /// the newest given waits for that turn (`next`). A task already running with no work given
    /// for its turn, because the turn started before the item that names it came, takes them now:
    /// the events returned show it. A task running with work given for it keeps it, and its words
    /// theirs. Codex names one item more than once (its start, its end): only the first gives
    /// work. Nothing for a translator that follows the chat's session.
    pub fn given_work(&mut self, subject: &str, description: Option<&str>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let item = self.turn_item(self.turn.as_deref());
        let running = self.open_event(&item);
        let Some(sub) = self.subagent.as_mut() else {
            return out;
        };
        if !sub.given.insert(subject.to_string()) {
            return out;
        }
        let description = description.filter(|d| !d.is_empty()).map(String::from);
        let Some(prev) = running.filter(|_| !sub.claimed) else {
            sub.next = Some((subject.to_string(), description));
            return out;
        };
        let Body::Task(task) = prev.body() else {
            return out;
        };
        sub.subject = Some(subject.to_string());
        sub.description = description.or(sub.description.take());
        sub.claimed = true;
        let sub = sub.clone();
        let native = prev.native().cloned();
        let body = Body::Task(Task {
            kind: task.kind.clone(),
            description: sub.description.clone(),
            subject: sub.subject.clone().map(Into::into),
        });
        let base = sub.description.clone().unwrap_or_default();
        let text = sub.row_text(&base);
        let title = "Subagent working";
        // The row as the latest turn shows it now, for an earlier turn's end read late.
        self.remember_row(&body, State::Running, title, &base);
        self.emit(&mut out, &item, body, State::Running, title, &text, native);
        out
    }

    /// An item id the translator makes up, numbered by the events it has made. A subagent's
    /// names its thread too: each translator of a chat counts from 0, and the chat keys every
    /// item by its id (D-117). The chat's own stay as they were.
    fn made_up(&self, kind: &str) -> String {
        match (&self.subagent, &self.session) {
            (Some(_), Some(thread)) => format!("{kind}:{thread}:{}", self.made),
            _ => format!("{kind}:{}", self.made),
        }
    }

    /// Whether the thread followed is a subagent's.
    pub fn is_subagent(&self) -> bool {
        self.subagent.is_some()
    }

    /// For a subagent's running turn, the turn the person started, which its events name.
    pub fn root_turn(&self) -> Option<&str> {
        let turn = self.turn.as_deref()?;
        Some(self.roots.get(turn).map_or(turn, String::as_str))
    }

    /// For a subagent's running turn, the turn the person started as Codex names it
    /// (`rootTurnId`), when it does.
    pub fn named_root_turn(&self) -> Option<&str> {
        self.roots.get(self.turn.as_deref()?).map(String::as_str)
    }

    /// Methods Codex sent that the driver has no rule for, in the order they came. A newer Codex
    /// can add one; it is then the driver's to decide.
    pub fn unknown(&self) -> &[String] {
        &self.unknown
    }

    /// Replies to send to Codex at once, which nobody is asked for: a form the chat cannot show is
    /// declined, and a request the driver cannot show refused, so Codex does not wait for it. The
    /// driver sends each and feeds it back, as it does every message it sends.
    pub fn take_replies(&mut self) -> Vec<String> {
        std::mem::take(&mut self.outbox)
    }

    /// How many messages could not be read as JSON-RPC.
    pub fn unreadable(&self) -> u64 {
        self.unreadable
    }

    /// How many messages named a thread that is not the session. They made no event. Any is a
    /// message the transport gave the wrong translator.
    pub fn other_threads(&self) -> u64 {
        self.other_threads
    }

    /// The Codex thread followed: the chat's session, once one is picked.
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// The turn running in the session, once Codex has said it started.
    pub fn turn(&self) -> Option<&str> {
        self.turn.as_deref()
    }

    /// The request that started a turn: a `turn/start` or a compaction the driver sent. None for
    /// a turn Codex started by itself.
    pub fn turn_request(&self, turn: &str) -> Option<i64> {
        self.turn_requests.get(turn).copied()
    }

    /// One message, as the exact text that went over the connection. Returns the events it makes,
    /// in order; most messages make none.
    pub fn feed(&mut self, direction: Direction, message: &str) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let Ok(envelope) = serde_json::from_str::<Envelope<'_>>(message) else {
            self.unreadable += 1;
            return out;
        };
        match direction {
            Direction::Out => self.client_sent(&envelope, &mut out),
            Direction::In => self.codex_sent(&envelope, &mut out),
        }
        out
    }

    /// The Codex process ended. Nothing it was doing will report again, so the driver closes the
    /// open items and the open turn itself.
    pub fn harness_stopped(&mut self) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        self.close_open_items(&mut out, false, None);
        let turn_item = self.turn_item(self.turn.as_deref());
        if let Some(prev) = self.open_event(&turn_item) {
            let body = prev.body().clone();
            let title = if self.subagent.is_some() {
                "Subagent stopped"
            } else {
                "Interrupted"
            };
            self.emit(
                &mut out,
                &turn_item,
                body,
                State::Cancelled,
                title,
                "The harness stopped before the turn finished.",
                None,
            );
        }
        self.requests.clear();
        self.asked_settings.clear();
        self.outbox.clear(); // nothing is left to answer
        self.asked.clear();
        self.starts.clear();
        self.usage.clear();
        self.turn = None;
        out
    }

    fn client_sent(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        if let Some(method) = &m.method {
            if let Some(id) = &m.id {
                let owner = (self.session.clone(), self.turn.clone());
                let sent = Sent {
                    method: method.clone(),
                    owner,
                };
                self.requests.insert(id.clone(), sent);
                if method == "thread/settings/update" {
                    let params: Value = m.params.map_or(Value::Null, |p| {
                        serde_json::from_str(p.get()).unwrap_or(Value::Null)
                    });
                    let said = |key: &str| params[key].as_str().map(String::from);
                    self.asked_settings
                        .insert(id.clone(), (said("model"), said("effort")));
                }
                let by = match method.as_str() {
                    "turn/start" => Some(StartedBy::User),
                    "thread/compact/start" => Some(StartedBy::Control),
                    _ => None,
                };
                if let Some(by) = by {
                    let thread = named_thread(m.params.map_or("{}", RawValue::get));
                    self.starts.push((id.clone(), by, thread));
                }
            }
            return;
        }
        self.answer(m, out);
    }

    fn codex_sent(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        let replies = self.outbox.len();
        self.codex_said(m, out);
        // Codex waits for a reply to each of its requests, and takes an error as a refusal: an
        // approval denied, a question with no answers, a form declined. So a request nobody in
        // the chat is asked about, and that the driver did not answer itself, is refused at once:
        // one it cannot show (a question with two questions of one id, params it cannot read) as
        // invalid, one of a kind it has no rule for as unknown. A call of the driver's own tool is
        // the transport's to answer.
        let (Some(method), Some(id)) = (m.method.as_deref(), &m.id) else {
            return;
        };
        let params = m.params.map_or("{}", RawValue::get);
        if method == "item/tool/call"
            || self.names_another_thread(named_thread(params).as_deref())
            || self.asked.contains_key(id)
            || self.outbox.len() > replies
        {
            return;
        }
        let error = if REQUESTS.contains(&method) {
            json!({"code": -32602, "message": "Invalid params"})
        } else {
            json!({"code": -32601, "message": "Method not found"})
        };
        self.outbox
            .push(json!({"id": id, "error": error}).to_string());
    }

    fn codex_said(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        let Some(method) = m.method.as_deref() else {
            return self.response(m, out);
        };
        // Codex writes params with every method. Read from nothing, a message without them would
        // make an event whose native message never arrived (D-007), so it is unreadable; a request
        // among them is still refused ([`Translator::codex_sent`]).
        let Some(params) = m.params.map(RawValue::get) else {
            self.unreadable += 1;
            return;
        };
        if method.to_lowercase().contains("delta") {
            return; // partial text and output
        }
        if NOT_SENT.contains(&method) {
            match method {
                "thread/tokenUsage/updated" => {
                    let Some(p) = self.read::<TokenUsageUpdated>(method, params) else {
                        return;
                    };
                    if self.for_another_thread(p.thread_id.as_deref()) {
                        return;
                    }
                    let usage = p.token_usage.unwrap_or_default();
                    let total = usage.total.unwrap_or_default();
                    let last = usage.last.unwrap_or_default();
                    let before = Tokens {
                        input_tokens: Some(less(&total.input_tokens, &last.input_tokens)),
                        output_tokens: Some(less(&total.output_tokens, &last.output_tokens)),
                    };
                    self.usage
                        .entry(p.turn_id)
                        .and_modify(|(_, latest)| *latest = total.clone())
                        .or_insert((before, total));
                }
                "thread/started" => {
                    let Some(p) = self.read::<ThreadStarted>(method, params) else {
                        return;
                    };
                    // The reply to the client's request picks the session; this reports on it.
                    let thread = p.thread.unwrap_or_default();
                    if self.for_another_thread(thread.id.as_deref()) {
                        return;
                    }
                    self.thread_settings(&thread, method, params, out);
                }
                "thread/settings/updated" => {
                    let Some(p) = self.read::<ThreadSettingsUpdated>(method, params) else {
                        return;
                    };
                    if self.for_another_thread(p.thread_id.as_deref()) {
                        return;
                    }
                    let settings = p.thread_settings.unwrap_or_default();
                    self.thread_settings(&settings, method, params, out);
                }
                "serverRequest/resolved" => {
                    // Codex waits no longer, so no answer may reach it, and the item closes now,
                    // though the turn goes on (#36). One already answered keeps its answer.
                    let Some(p) = self.read::<RequestResolved>(method, params) else {
                        return;
                    };
                    if self.for_another_thread(p.thread_id.as_deref()) {
                        return;
                    }
                    let Some(asked) = p.request_id.and_then(|id| self.asked.remove(&id)) else {
                        return;
                    };
                    let (Pending::Approval { item, .. } | Pending::Question { item }) = asked;
                    let Some(prev) = self.open_event(&item) else {
                        return;
                    };
                    let header = prev.header();
                    let (title, text) = (header.title.clone(), header.text.clone());
                    let body = prev.body().clone();
                    let native = native(method, params);
                    let state = State::Cancelled;
                    self.emit_owned(out, owner(&prev), &item, body, state, &title, &text, native);
                }
                _ => {}
            }
            return;
        }
        if self.for_another_thread(named_thread(params).as_deref()) {
            return;
        }
        self.handle(method, m, params, out);
    }

    /// Whether a message names a thread that is not the session. It is counted, and makes no
    /// event.
    fn for_another_thread(&mut self, thread: Option<&str>) -> bool {
        let other = self.names_another_thread(thread);
        self.other_threads += u64::from(other);
        other
    }

    /// Whether `thread` is named and is not the session. Before the reply to `thread/start` or
    /// `thread/resume` picks the session, every thread is another's (D-060), so nothing Codex says
    /// of a thread can report on, or change, a session not yet picked (#37).
    fn names_another_thread(&self, thread: Option<&str>) -> bool {
        thread.is_some() && thread != self.session.as_deref()
    }

    fn handle(&mut self, method: &str, m: &Envelope<'_>, params: &str, out: &mut Vec<AgentEvent>) {
        match method {
            "turn/started" => self.turn_started(method, params, out),
            "turn/completed" => self.turn_completed(method, params, out),
            "item/started" | "item/completed" => self.item(method, params, out),
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                self.approval(method, m.id.clone(), params, out)
            }
            "item/permissions/requestApproval" => {
                self.permissions(method, m.id.clone(), params, out)
            }
            "item/tool/requestUserInput" => self.user_input(method, m.id.clone(), params, out),
            "mcpServer/elicitation/request" => self.elicitation(method, m.id.clone(), params, out),
            // The driver carries out its own tool; the dynamicToolCall item shows it.
            "item/tool/call" => {}
            "turn/plan/updated" => self.plan(method, params, out),
            "thread/compacted" => {
                let item = self.made_up("compaction");
                let body = Body::Notice(notice(NoticeKind::Compaction));
                let native = native(method, params);
                self.emit(
                    out,
                    &item,
                    body,
                    State::Done,
                    "Conversation compacted",
                    "",
                    native,
                );
            }
            "model/rerouted" => {
                let Some(p) = self.read::<Rerouted>(method, params) else {
                    return;
                };
                let item = self.made_up("model");
                let text = format!(
                    "{} to {}",
                    show(p.from_model.as_deref()),
                    show(p.to_model.as_deref())
                );
                let body = Body::Notice(Notice {
                    kind: NoticeKind::ModelChanged,
                    duration_ms: None,
                    reason: p.reason,
                });
                let native = native(method, params);
                self.emit(
                    out,
                    &item,
                    body,
                    State::Done,
                    "Model changed",
                    &text,
                    native,
                );
            }
            "warning" => {
                let Some(p) = self.read::<NativeMessage>(method, params) else {
                    return;
                };
                let item = self.made_up("warning");
                let body = Body::Notice(notice(NoticeKind::Warning));
                let text = p.message.unwrap_or_default();
                let native = native(method, params);
                self.emit(out, &item, body, State::Done, "Warning", &text, native);
            }
            "error" => {
                let Some(p) = self.read::<ErrorNote>(method, params) else {
                    return;
                };
                let item = self.made_up("error");
                let text = p.error.and_then(|e| e.message).unwrap_or_default();
                let body = Body::Error(Error {
                    will_retry: p.will_retry,
                    ..Error::default()
                });
                let native = native(method, params);
                self.emit(out, &item, body, State::Failed, "Error", &text, native);
            }
            other => self.unknown.push(other.to_string()),
        }
    }

    // -- turns

    fn turn_started(&mut self, method: &str, params: &str, out: &mut Vec<AgentEvent>) {
        let Some(p) = self.read::<TurnNote>(method, params) else {
            return;
        };
        let turn = p.turn.unwrap_or_default();
        self.turn = turn.id;
        if let Some(sub) = self.subagent.as_mut() {
            sub.latest = self.turn.clone();
            // Work given while the last turn ran is this one's.
            sub.claimed = sub.next.is_some();
            if let Some((subject, description)) = sub.next.take() {
                sub.subject = Some(subject);
                sub.description = description.or(sub.description.take());
            }
            let sub = sub.clone();
            return self.subagent_started(sub, turn.root_turn_id, method, params, out);
        }
        // The oldest request that starts a turn started this one. With none, Codex started it.
        let started_by = if self.starts.is_empty() {
            StartedBy::Agent
        } else {
            let (request, by, _) = self.starts.remove(0);
            if let (Some(turn), Id::Number(request)) = (&self.turn, request) {
                self.turn_requests.insert(turn.clone(), request);
            }
            by
        };
        self.started_by.insert(self.turn.clone(), started_by);
        let item = format!("turn:{}", show(self.turn.as_deref()));
        let body = Body::Turn(Turn {
            started_by,
            duration_ms: None,
            usage: None,
        });
        let native = native(method, params);
        self.emit(out, &item, body, State::Running, "Working", "", native);
    }

    fn turn_completed(&mut self, method: &str, params: &str, out: &mut Vec<AgentEvent>) {
        let Some(note) = self.read::<TurnNote>(method, params) else {
            return;
        };
        let turn = note.turn.unwrap_or_default();
        // The end of a turn other than the current one, read late, ends only that turn, and is
        // shown as that turn's: the current turn, and what is open in it, stay (#143).
        let current = match (&turn.id, &self.turn) {
            (Some(ended), Some(current)) => ended == current,
            _ => true,
        };
        let ended = if current {
            self.turn.clone()
        } else {
            turn.id.clone()
        };
        // A question answered by the person's next message still waits after a turn that
        // completed. Anything else open will never report again.
        let completed = turn.status.as_deref() == Some("completed");
        if current {
            self.close_open_items(out, completed, None);
        } else if self.subagent.is_none() {
            // A subagent's items are shown under the person's turn, whichever of its own turns
            // they were in, so only the session's can be told apart.
            self.close_open_items(out, completed, ended.as_deref());
        }
        if self.subagent.is_some() {
            return self.subagent_ended(turn, current, method, params, out);
        }
        let (state, title) = match turn.status.as_deref() {
            Some("interrupted") => (State::Cancelled, "Interrupted"),
            Some("failed") => (State::Failed, "Failed"),
            _ => (State::Done, "Finished"),
        };
        let text = turn.error.and_then(|e| e.message).unwrap_or_default();
        let usage = self.usage.remove(&turn.id).map(|(before, total)| Usage {
            input_tokens: Some(less(&total.input_tokens, &before.input_tokens)),
            output_tokens: Some(less(&total.output_tokens, &before.output_tokens)),
        });
        let body = Body::Turn(Turn {
            started_by: (self.started_by.remove(&ended)).unwrap_or(StartedBy::User),
            duration_ms: turn.duration_ms,
            usage,
        });
        let item = format!("turn:{}", show(turn.id.as_deref()));
        let native = native(method, params);
        let owner = (self.session.clone(), ended);
        self.emit_owned(out, owner, &item, body, state, title, &text, native);
        if let Some(id) = &turn.id {
            self.turn_requests.remove(id); // nothing more comes for this turn
        }
        if current {
            self.turn = None; // nothing that follows belongs to this turn
        }
    }

    // -- a subagent's turns

    /// The item a turn is shown as: the turn itself, or, for a subagent, its one `task` item,
    /// which each of its turns opens again (D-121).
    fn turn_item(&self, turn: Option<&str>) -> String {
        match self.task_item() {
            Some(task) => task,
            None => format!("turn:{}", show(turn)),
        }
    }

    /// A subagent's turn starts: it is shown as a task, under the turn the person started.
    fn subagent_started(
        &mut self,
        sub: Subagent,
        root: Option<String>,
        method: &str,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        if let (Some(turn), Some(root)) = (&self.turn, root) {
            self.roots.insert(turn.clone(), root.clone());
            if let Some(sub) = self.subagent.as_mut() {
                sub.root = Some(root);
            }
        }
        let item = self.turn_item(self.turn.as_deref());
        let base = sub.description.clone().unwrap_or_default();
        let text = sub.row_text(&base);
        let body = Body::Task(Task {
            kind: "subagent".into(),
            description: sub.description,
            subject: sub.subject.map(Into::into),
        });
        let native = native(method, params);
        let title = "Subagent working";
        self.remember_row(&body, State::Running, title, &base);
        self.emit(out, &item, body, State::Running, title, &text, native);
    }

    /// Keeps the row as the subagent's latest turn shows it.
    fn remember_row(&mut self, body: &Body, state: State, title: &str, text: &str) {
        if let Some(sub) = self.subagent.as_mut() {
            sub.row = Some((body.clone(), state, title.into(), text.into()));
        }
    }

    /// A subagent's turn ends, and its task with it. `current` when it is the turn the subagent
    /// is in.
    fn subagent_ended(
        &mut self,
        turn: messages::TurnInfo,
        current: bool,
        method: &str,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let (state, title) = match turn.status.as_deref() {
            Some("interrupted") => (State::Cancelled, "Subagent stopped"),
            Some("failed") => (State::Failed, "Subagent failed"),
            _ => (State::Done, "Subagent finished"),
        };
        let item = self.turn_item(turn.id.as_deref());
        let native = native(method, params);
        // The end of an earlier turn read late, after a later one started: the row shows the
        // later turn, running or ended, and the end is shown on it (D-007), so a Stop waiting for
        // that turn is settled. A failure is said on the row from then on, with its error.
        let sub = self.subagent.clone().unwrap_or_default();
        let late = turn.id.is_some() && sub.latest.is_some() && turn.id != sub.latest;
        if late && let Some((body, state, title, base)) = sub.row {
            if turn.status.as_deref() == Some("failed") {
                let error = turn.error.as_ref().and_then(|e| e.message.clone());
                let said = match error {
                    Some(error) => format!("An earlier turn failed: {error}"),
                    None => "An earlier turn failed".to_string(),
                };
                if let Some(sub) = self.subagent.as_mut() {
                    sub.failed.push(said);
                }
            }
            let text = (self.subagent.as_ref()).map_or_else(|| base.clone(), |s| s.row_text(&base));
            let owner = (self.session.clone(), self.turn.clone().or(turn.id));
            return self.emit_owned(out, owner, &item, body, state, &title, &text, native);
        }
        let body = match self.open_event(&item) {
            Some(prev) => prev.body().clone(),
            None => {
                let sub = self.subagent.clone().unwrap_or_default();
                Body::Task(Task {
                    kind: "subagent".into(),
                    description: sub.description,
                    subject: sub.subject.map(Into::into),
                })
            }
        };
        let base = (turn.error.and_then(|e| e.message))
            .or_else(|| self.subagent.as_ref().and_then(|s| s.description.clone()))
            .unwrap_or_default();
        let text = (self.subagent.as_ref()).map_or_else(|| base.clone(), |s| s.row_text(&base));
        if let Some(id) = &turn.id {
            self.usage.remove(&Some(id.clone()));
        }
        let ended = if current { self.turn.clone() } else { turn.id };
        let owner = (self.session.clone(), ended);
        self.remember_row(&body, state, title, &base);
        self.emit_owned(out, owner, &item, body, state, title, &text, native);
        if current {
            self.turn = None; // nothing that follows belongs to this turn
        }
    }

    // -- events

    /// A turn ended. Anything still running will never report again, so the driver closes it:
    /// everything open, or only what is in turn `only`.
    fn close_open_items(
        &mut self,
        out: &mut Vec<AgentEvent>,
        keep_message_questions: bool,
        only: Option<&str>,
    ) {
        let open: Vec<(String, AgentEvent)> = self.open.clone();
        for (item, prev) in open {
            let turn = prev.header().turn.as_ref().map(|t| t.as_str());
            if only.is_some() && turn != only {
                continue;
            }
            match prev.body() {
                // Tasks and sessions may outlive the turn; the turn closes itself.
                Body::Turn(_) | Body::Task(_) | Body::Session(_) => continue,
                Body::Question(q)
                    if keep_message_questions && q.answer_via() == AnswerVia::Message =>
                {
                    continue;
                }
                _ => {}
            }
            let header = prev.header();
            let (title, text) = (header.title.clone(), header.text.clone());
            let body = prev.body().clone();
            self.emit_owned(
                out,
                owner(&prev),
                &item,
                body,
                State::Cancelled,
                &title,
                &text,
                None,
            );
        }
    }

    /// A question answered by the person's next message: that message answers it, or a new
    /// session leaves it unanswered. It stays in the session and turn it was asked in.
    fn close_message_questions(&mut self, state: State, text: &str, out: &mut Vec<AgentEvent>) {
        let open: Vec<(String, AgentEvent)> = self.open.clone();
        for (item, prev) in open {
            let Body::Question(q) = prev.body() else {
                continue;
            };
            if q.answer_via() != AnswerVia::Message {
                continue;
            }
            let (title, body) = (prev.header().title.clone(), prev.body().clone());
            self.emit_owned(out, owner(&prev), &item, body, state, &title, text, None);
        }
    }

    /// Reads a message's payload. One the driver cannot read is a shape it does not know: it is
    /// noted, and makes no event, rather than being taken as empty.
    fn read<'a, T: Deserialize<'a>>(&mut self, what: &str, text: &'a str) -> Option<T> {
        match serde_json::from_str(text) {
            Ok(value) => Some(value),
            Err(_) => {
                self.unknown
                    .push(format!("{what}: a shape the driver cannot read"));
                None
            }
        }
    }

    fn open_event(&self, item: &str) -> Option<AgentEvent> {
        self.open
            .iter()
            .find(|(open, _)| open == item)
            .map(|(_, ev)| ev.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        out: &mut Vec<AgentEvent>,
        item: &str,
        body: Body,
        state: State,
        title: &str,
        text: &str,
        native: Option<Native>,
    ) {
        let owner = (self.session.clone(), self.turn.clone());
        self.emit_owned(out, owner, item, body, state, title, text, native);
    }

    /// An event for an item that belongs to `owner`, a session and a turn, which need not be the
    /// current ones: an item the driver closes keeps the session and turn it was in.
    #[allow(clippy::too_many_arguments)]
    fn emit_owned(
        &mut self,
        out: &mut Vec<AgentEvent>,
        (session, turn): (Option<String>, Option<String>),
        item: &str,
        body: Body,
        state: State,
        title: &str,
        text: &str,
        native: Option<Native>,
    ) {
        self.made += 1;
        // A subagent's events are the chat's: in its session, under the turn the person started.
        // Each names the subagent it belongs to by its `task` item, and the `task` item itself the
        // subagent that started it, if one did (D-121).
        let task = self.task_item();
        let (session, turn, subagent) = match &self.subagent {
            Some(sub) => {
                let root = turn.map(|t| self.roots.get(&t).cloned().unwrap_or(t));
                let of = match task {
                    Some(task) if task == item => sub.parent.clone(),
                    task => task,
                };
                (sub.session.clone(), root.or(sub.root.clone()), of)
            }
            None => (session, turn, None),
        };
        let seq = self.seq.entry(session.clone()).or_default();
        *seq += 1;
        let header = Header {
            harness: "codex".into(),
            session: session.map(Into::into),
            turn: turn.map(Into::into),
            subagent: subagent.map(Into::into),
            item: item.into(),
            seq: *seq,
            state,
            title: title.into(),
            text: text.into(),
        };
        // A secret question that is no longer waiting cannot carry its native message (D-056).
        let event = match AgentEvent::new(header.clone(), body.clone(), native) {
            Ok(event) => event,
            Err(_) => match AgentEvent::new(header, body, None) {
                Ok(event) => event,
                Err(_) => return,
            },
        };
        let waiting = matches!(state, State::Running | State::Waiting);
        match self.open.iter().position(|(open, _)| open == item) {
            Some(at) if waiting => self.open[at].1 = event.clone(),
            Some(at) => {
                self.open.remove(at);
            }
            None if waiting => self.open.push((item.to_string(), event.clone())),
            None => {}
        }
        out.push(event);
    }
}

/// A subagent's `task` item, by its thread: one for the whole of its life (D-121).
fn task_of(thread: &str) -> String {
    format!("task:{thread}")
}

/// The thread a message's params name, if they name one.
fn named_thread(params: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Named {
        #[serde(default, rename = "threadId")]
        thread_id: Option<String>,
    }
    serde_json::from_str::<Named>(params).ok()?.thread_id
}

fn native(kind: &str, payload: &str) -> Option<Native> {
    Some(native_text(kind, payload))
}

fn native_text(kind: &str, payload: &str) -> Native {
    Native {
        kind: kind.to_string(),
        payload: RawValue::from_string(payload.to_string())
            .unwrap_or_else(|_| RawValue::from_string("{}".into()).expect("valid JSON")),
    }
}

/// The session and turn an event belongs to.
fn owner(event: &AgentEvent) -> (Option<String>, Option<String>) {
    let h = event.header();
    (
        h.session.as_ref().map(|s| s.as_str().to_string()),
        h.turn.as_ref().map(|t| t.as_str().to_string()),
    )
}

/// A count less another, as Codex reports them; a count it left out is nought.
fn less(a: &Option<u64>, b: &Option<u64>) -> u64 {
    a.unwrap_or(0).saturating_sub(b.unwrap_or(0))
}

/// A value Codex left out, written as the prototype writes it.
fn show(value: Option<&str>) -> String {
    value.unwrap_or("None").to_string()
}

fn notice(kind: NoticeKind) -> Notice {
    Notice {
        kind,
        duration_ms: None,
        reason: None,
    }
}

// -- what the driver sends

/// What the driver sends for the core's requests. Each method builds the message and changes
/// nothing: the driver writes it to Codex and feeds it here as [`Direction::Out`], as it does
/// every message, so the translator learns what was sent the same way whoever sent it.
impl Translator {
    /// The request that passes a prompt to the session: a new turn, or, while one runs, the same
    /// message added to it (`mid_turn: fold`).
    pub fn prompt(&self, id: i64, prompt: &Prompt) -> Result<String, Refusal> {
        let Some(thread) = &self.session else {
            return Err(Refusal::new(Reason::Busy)); // the driver picks a session first
        };
        if self.waiting_on_codex() {
            return Err(Refusal::new(Reason::Busy));
        }
        if prompt
            .session
            .as_ref()
            .is_some_and(|s| s.as_str() != thread)
        {
            // The driver resumes the session a prompt names before it sends the prompt.
            return Err(Refusal::new(Reason::UnknownItem));
        }
        let input = send::input(prompt)?;
        Ok(match &self.turn {
            Some(turn) => {
                let params = json!({"threadId": thread, "expectedTurnId": turn, "input": input});
                send::request(id, "turn/steer", params)
            }
            None => send::request(
                id,
                "turn/start",
                json!({"threadId": thread, "input": input}),
            ),
        })
    }

    /// The request that starts a new session, or resumes one Codex kept. A chat's session is
    /// replaced only while it is idle, and only once at a time (D-060).
    pub fn pick(&self, id: i64, session: Option<&str>, setup: &Setup) -> Result<String, Refusal> {
        if self.turn.is_some() || self.waiting_on_codex() {
            return Err(Refusal::new(Reason::Busy));
        }
        Ok(match session {
            Some(thread) => send::thread_resume(id, thread, setup),
            None => send::thread_start(id, setup),
        })
    }

    /// The request that carries out a control.
    pub fn control(
        &self,
        id: i64,
        action: &DriverAction,
        setup: &Setup,
    ) -> Result<String, Refusal> {
        let refuse = |reason| Err(Refusal::new(reason));
        // Only an interrupt is built while Codex has yet to answer: it is how a person stops
        // whatever that request started.
        if self.waiting_on_codex() && !matches!(action, DriverAction::Interrupt { .. }) {
            return refuse(Reason::Busy);
        }
        match action {
            DriverAction::Interrupt { turn, .. } => {
                let (Some(thread), Some(running)) = (&self.session, &self.turn) else {
                    return refuse(Reason::NothingToDo);
                };
                if running.as_str() != turn.as_str() {
                    return refuse(Reason::NothingToDo);
                }
                let params = json!({"threadId": thread, "turnId": running});
                Ok(send::request(id, "turn/interrupt", params))
            }
            DriverAction::NewSession { session, .. } => {
                // The session it replaces must still be the chat's: one asked for before another
                // replaced it is out of date. With no thread running, as in a driver started
                // again, the core names the chat's saved session, and nothing is out of date.
                if self.session.is_some()
                    && session.as_ref().map(|s| s.as_str()) != self.session.as_deref()
                {
                    return refuse(Reason::UnknownItem);
                }
                self.pick(id, None, setup)
            }
            DriverAction::Set {
                session,
                setting,
                value,
            } => {
                let thread = self.session_named(session.as_str())?;
                // A value the session already has changes nothing, and Codex may not report it.
                let k = &self.known;
                let now = match setting.as_str() {
                    "model" => &k.model,
                    "effort" => &k.effort,
                    "approval_policy" => &k.approval_policy,
                    "sandbox" => &k.sandbox,
                    "mode" => &k.mode,
                    _ => &None,
                };
                if now.as_deref() == Some(value.as_str()) {
                    return refuse(Reason::NothingToDo);
                }
                // Each value is one the session item shows, so it is what Codex reported.
                let (key, change) = match setting.as_str() {
                    "model" => ("model", json!(value)),
                    "effort" => ("effort", json!(value)),
                    "approval_policy" => ("approvalPolicy", json!(value)),
                    "sandbox" => ("sandboxPolicy", json!({"type": value})),
                    "mode" => {
                        // Codex takes a mode with the model it runs on.
                        let (Some(model), Some(effort)) = (&self.known.model, &self.known.effort)
                        else {
                            return refuse(Reason::Busy); // until Codex has said them
                        };
                        let settings = json!({"model": model, "reasoning_effort": effort, "developer_instructions": null});
                        (
                            "collaborationMode",
                            json!({"mode": value, "settings": settings}),
                        )
                    }
                    _ => return refuse(Reason::Invalid),
                };
                let params = json!({"threadId": thread, key: change});
                Ok(send::request(id, "thread/settings/update", params))
            }
            DriverAction::Own {
                session,
                id: action,
            } => {
                let thread = self.session_named(session.as_str())?;
                if action != "compact" {
                    return refuse(Reason::Invalid);
                }
                Ok(send::request(
                    id,
                    "thread/compact/start",
                    json!({"threadId": thread}),
                ))
            }
        }
    }

    /// Whether a request that starts a turn, picks the session or changes a setting is waiting
    /// for Codex: a wait that ends without anyone in the chat doing anything. Until Codex answers, what the translator knows of the session may be out of
    /// date, so it builds nothing but replies to Codex's requests and interrupts; the driver holds
    /// the rest and asks again.
    /// Whether a request to start or resume a session is waiting for Codex, whoever sent it.
    pub fn picking(&self) -> bool {
        (self.requests.values())
            .any(|sent| matches!(sent.method.as_str(), "thread/start" | "thread/resume"))
    }

    pub fn waiting_on_codex(&self) -> bool {
        self.picking() || !self.starts.is_empty() || !self.asked_settings.is_empty()
    }

    fn session_named(&self, session: &str) -> Result<&str, Refusal> {
        match &self.session {
            Some(thread) if thread == session => Ok(thread),
            _ => Err(Refusal::new(Reason::UnknownItem)),
        }
    }
}
