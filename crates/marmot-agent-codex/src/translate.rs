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

use crate::messages::{
    self, ApprovalReply, ApprovalRequest, ChangeKind as NativeChange, Elicitation, Envelope,
    ErrorNote, ErrorReply, Id, ItemNote, ListReply, Message as NativeMessage, PlanUpdated, Policy,
    QuestionReply, RequestResolved, Rerouted, Sandbox, ThreadSettingsUpdated, ThreadStarted,
    TokenUsageUpdated, Tokens, TurnNote, UserInput,
};
use crate::send::{self, Setup};
use marmot_agent_driver::{
    Action as DriverAction, Answer as DriverAnswer, Given, Prompt, Reason, Refusal,
};
use marmot_agent_events::body::{
    Action, AnswerVia, Answered, Approval, ApprovalAnswer, Asked, Change, ChangeKind, Choice,
    Effect, Error, Message, Notice, NoticeKind, Offered, Phase, Plan, PlanEntry, PlanEntryState,
    Question, Reasoning, Session, Setting, StartedBy, Task, Tool, ToolKind, Turn, Usage,
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
/// driver knows: the settings, the thread, the tokens a turn used (rule 11 of the event model).
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

/// What the driver waits to hear back about.
enum Pending {
    Approval {
        item: String,
        /// Each choice's id, and the decision Codex takes for it.
        decisions: Vec<(String, Value)>,
        choices: Vec<Choice>,
        /// Where the reply says what was chosen.
        reply: Reply,
        /// For a permission request, what Codex asked for.
        asked_for: Option<Value>,
    },
    Question {
        item: String,
    },
}

/// Where a reply to an approval says what was chosen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    /// Under `decision`.
    Decision,
    /// Under `action`, for an MCP server.
    Action,
    /// As `permissions` granted for a `scope`. An empty grant denies.
    Grant,
}

/// The settings Codex has stated for the thread, merged.
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

/// What a translator that follows a subagent's thread shows it as (D-117): in the chat of the turn
/// that started it, in that chat's session and under the turn the person started, each of its
/// turns a `task` item, and its words naming the item that gave it its work.
#[derive(Clone, Default)]
struct Subagent {
    /// The chat's session, which every event names.
    session: Option<String>,
    /// The item that gave the subagent its work, as the chat shows it: what started it, or what
    /// gave it more.
    subject: Option<String>,
    /// What it is for: its path, or what it was asked.
    description: Option<String>,
    /// The turn the person started that its last turn ran under: what it says between turns
    /// belongs to.
    root: Option<String>,
    /// The work its next turn is for: the item that gave it, and what it is for. The newest
    /// given, while a turn runs or between turns, and taken when the next turn starts.
    next: Option<(String, Option<String>)>,
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
    started_by: Option<StartedBy>,
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
    /// (D-117). It never picks a session: the thread is the subagent's from the start.
    pub fn subagent(thread: &str, session: Option<&str>) -> Self {
        Self {
            session: Some(thread.to_string()),
            subagent: Some(Subagent {
                session: session.map(String::from),
                ..Subagent::default()
            }),
            ..Self::default()
        }
    }

    /// The item that gave a subagent its work, and what that work is, as its next turn shows them:
    /// the newest given waits for that turn (`next`). A task already running with no subject,
    /// because its turn started before the item that names it came, takes them now: the events
    /// returned show it. A task running with a subject keeps it, and its words theirs. Codex
    /// names one item more than once (its start, its end): a repeat of the item the running or
    /// last task names gives no new work. Nothing for a translator that follows the chat's
    /// session.
    pub fn given_work(&mut self, subject: &str, description: Option<&str>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let item = self.turn_item(self.turn.as_deref());
        let running = self.open_event(&item);
        let Some(sub) = self.subagent.as_mut() else {
            return out;
        };
        let description = description.filter(|d| !d.is_empty()).map(String::from);
        let has_subject = match running.as_ref().map(AgentEvent::body) {
            Some(Body::Task(task)) => task.subject.is_some(),
            _ => true,
        };
        if has_subject {
            if sub.subject.as_deref() != Some(subject) {
                sub.next = Some((subject.to_string(), description));
            }
            return out;
        }
        let Some(prev) = running else {
            return out;
        };
        let Body::Task(task) = prev.body() else {
            return out;
        };
        sub.subject = Some(subject.to_string());
        sub.description = description.or(sub.description.take());
        let sub = sub.clone();
        let native = prev.native().cloned();
        let body = Body::Task(Task {
            kind: task.kind.clone(),
            description: task.description.clone().or(sub.description.clone()),
            subject: sub.subject.map(Into::into),
        });
        let text = sub.description.unwrap_or_default();
        let title = "Subagent working";
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
        self.close_open_items(&mut out, false);
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

    /// A request of Codex's that Codex said was resolved before the driver could show it, as one
    /// held for a session being resumed can be. It is shown as it would have been, with its native
    /// message, but closed, as cancelled: nobody may answer it, and Codex waits for no reply, so
    /// none is made (#142, D-008).
    pub fn withdrawn(&mut self, message: &str) -> Vec<AgentEvent> {
        let replies = self.outbox.len();
        let shown = self.feed(Direction::In, message);
        self.outbox.truncate(replies);
        if let Ok(Some(id)) = serde_json::from_str::<Envelope<'_>>(message).map(|m| m.id) {
            self.asked.remove(&id);
        }
        let mut out = Vec::new();
        for event in shown {
            if event.header().state != State::Waiting {
                out.push(event);
                continue;
            }
            let header = event.header();
            let item = header.item.as_str().to_string();
            let (title, text) = (header.title.clone(), header.text.clone());
            let native = event.native().cloned();
            let body = event.body().clone();
            self.emit_owned(
                &mut out,
                owner(&event),
                &item,
                body,
                State::Cancelled,
                &title,
                &text,
                native,
            );
        }
        out
    }

    fn client_sent(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        if let Some(method) = &m.method {
            if matches!(method.as_str(), "turn/start" | "turn/steer") {
                self.close_message_questions(State::Done, "Answered by the next message", out);
            }
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
        let thread = named_thread(params);
        let other = thread.is_some() && self.session.is_some() && thread != self.session;
        if method == "item/tool/call"
            || other
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
        let params = m.params.map_or("{}", RawValue::get);
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
                    // Codex waits no longer, so no answer may reach it, even before the turn's
                    // end closes the item. The item is left to close as it would.
                    let Some(p) = self.read::<RequestResolved>(method, params) else {
                        return;
                    };
                    if self.for_another_thread(p.thread_id.as_deref()) {
                        return;
                    }
                    if let Some(id) = &p.request_id {
                        self.asked.remove(id);
                    }
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

    /// Whether a message names a thread that is not the session, once there is one. It is
    /// counted, and makes no event.
    fn for_another_thread(&mut self, thread: Option<&str>) -> bool {
        let other = thread.is_some() && self.session.is_some() && thread != self.session.as_deref();
        self.other_threads += u64::from(other);
        other
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
            // Work given while the last turn ran is this one's.
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
        self.started_by = Some(started_by);
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
        // The end of a turn other than the current one, read late, ends only that turn: what is
        // open, and the current turn, are the current turn's (#143).
        let current = match (&turn.id, &self.turn) {
            (Some(ended), Some(current)) => ended == current,
            _ => true,
        };
        // A question answered by the person's next message still waits after a turn that
        // completed. Anything else open will never report again.
        let completed = turn.status.as_deref() == Some("completed");
        if current {
            self.close_open_items(out, completed);
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
            started_by: self.started_by.unwrap_or(StartedBy::User),
            duration_ms: turn.duration_ms,
            usage,
        });
        let item = format!("turn:{}", show(turn.id.as_deref()));
        let native = native(method, params);
        self.emit(out, &item, body, state, title, &text, native);
        if let Some(id) = &turn.id {
            self.turn_requests.remove(id); // nothing more comes for this turn
        }
        if current {
            self.turn = None; // nothing that follows belongs to this turn
        }
    }

    // -- items

    fn item(&mut self, method: &str, params: &str, out: &mut Vec<AgentEvent>) {
        let Some(note) = self.read::<ItemNote<'_>>(method, params) else {
            return;
        };
        let it = note.item.unwrap_or_default();
        // What a parent's thread says of a subagent can come after the parent's turn has ended
        // (recorded): it stays under the turn it names.
        let in_turn = (self.session.clone(), note.turn_id.or(self.turn.clone()));
        let started = method == "item/started";
        let kind = it.kind.as_deref();
        let native_kind = format!("{method} [{}]", show(kind));
        let native = || Some(native_text(&native_kind, params));
        let iid = show(it.id.as_deref());
        let finished = |status: Option<&str>| match status {
            Some("failed") => State::Failed,
            Some("declined") => State::Declined,
            _ => State::Done,
        };
        let running_or = |done: State| if started { State::Running } else { done };
        match kind {
            Some("userMessage") => {} // the harness's echo of the person's message
            Some("agentMessage") => {
                if started {
                    return; // a message has no text until it is complete
                }
                // A phase the event model does not name is left out; the native message keeps it.
                let phase = match it.phase.as_deref() {
                    Some("final_answer") => Some(Phase::Final),
                    Some("commentary") => Some(Phase::Commentary),
                    _ => None,
                };
                // A subagent's words name the item that gave it its work (D-117).
                let parent = (self.subagent.as_ref()).and_then(|s| s.subject.clone());
                let body = Body::Message(Message {
                    phase,
                    parent: parent.map(Into::into),
                });
                let text = it.text.clone().unwrap_or_default();
                self.emit(out, &iid, body, State::Done, "Message", &text, native());
                let asked: Vec<Asked> = it
                    .questions
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(n, q)| Asked {
                        id: format!("q{n}"),
                        header: None,
                        text: q.title.unwrap_or_default(),
                        options: q
                            .options
                            .unwrap_or_default()
                            .into_iter()
                            .enumerate()
                            .map(|(k, label)| Offered {
                                id: format!("o{k}"),
                                label,
                                description: None,
                            })
                            .collect(),
                        multiple: false,
                        allow_text: true,
                        secret: false,
                    })
                    .collect();
                if !asked.is_empty() {
                    let text = asked[0].text.clone();
                    if let Ok(question) = Question::new(AnswerVia::Message, None, asked) {
                        let item = format!("{iid}:question");
                        let body = Body::Question(question);
                        self.emit(
                            out,
                            &item,
                            body,
                            State::Waiting,
                            "Question",
                            &text,
                            native(),
                        );
                    }
                }
            }
            Some("reasoning") => {
                let text = it
                    .summary
                    .iter()
                    .flatten()
                    .chain(it.content.iter().flatten())
                    .map(reasoning_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                if started || text.trim().is_empty() {
                    return;
                }
                let body = Body::Reasoning(Reasoning {});
                self.emit(out, &iid, body, State::Done, "Thinking", &text, native());
            }
            Some("commandExecution") => {
                let state = running_or(finished(it.status.as_deref()));
                let actions: Vec<Option<&str>> = it
                    .command_actions
                    .iter()
                    .flatten()
                    .map(|a| a.kind.as_deref())
                    .collect();
                let tool_kind = if !actions.is_empty() && actions.iter().all(|a| *a == Some("read"))
                {
                    ToolKind::Read
                } else if actions.contains(&Some("search")) {
                    ToolKind::Search
                } else {
                    ToolKind::Command
                };
                let mut tool = tool(tool_kind, "shell");
                tool.command = it.command.clone();
                tool.cwd = it.cwd.clone();
                if !started {
                    tool.exit_code = it.exit_code;
                    tool.duration_ms = it.duration_ms;
                    tool.output = Some(it.aggregated_output.clone().unwrap_or_default());
                }
                let title = match state {
                    State::Running => "Running a command",
                    State::Failed => "Command failed",
                    State::Declined => "Command not allowed",
                    _ => "Ran a command",
                };
                let text = it.command.clone().unwrap_or_default();
                self.emit(out, &iid, Body::Tool(tool), state, title, &text, native());
            }
            Some("fileChange") => {
                let state = running_or(finished(it.status.as_deref()));
                let changes: Vec<Change> = it
                    .changes
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| Change {
                        path: c.path.unwrap_or_default(),
                        diff: c.diff.unwrap_or_default(),
                        change: c.kind.and_then(change_kind),
                    })
                    .collect();
                let text = changes
                    .iter()
                    .map(|c| c.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let title = match state {
                    State::Running => "Editing files",
                    State::Failed => "Edit failed",
                    State::Declined => "Edit not allowed",
                    _ => "Edited files",
                };
                let mut tool = tool(ToolKind::Edit, "apply_patch");
                tool.changes = changes;
                self.emit(out, &iid, Body::Tool(tool), state, title, &text, native());
            }
            Some("mcpToolCall") => {
                let state = if started {
                    State::Running
                } else if it.status.as_deref() == Some("failed") {
                    State::Failed
                } else {
                    State::Done
                };
                let name = format!(
                    "{}.{}",
                    show(it.server.as_deref()),
                    show(it.tool.as_deref())
                );
                let mut tool = tool(ToolKind::Mcp, &name);
                tool.input = it.arguments.filter(|v| !v.is_null());
                if !started {
                    // The result as the text it arrived as.
                    tool.output = it.result.map(|r| r.get().to_string());
                    // Codex sends a failed call's error as an object with a message. The item
                    // takes the message; the object stays in the native message.
                    tool.error = match it.error {
                        Some(Value::Object(e)) => {
                            e.get("message").and_then(Value::as_str).map(str::to_string)
                        }
                        Some(Value::String(e)) => Some(e),
                        _ => None,
                    };
                }
                let title = if started {
                    "Using a tool"
                } else {
                    "Used a tool"
                };
                self.emit(out, &iid, Body::Tool(tool), state, title, &name, native());
            }
            Some("dynamicToolCall") => {
                // A tool the driver gave the session, such as send_file (D-090). The driver
                // carries it out on Codex's `item/tool/call`; this item shows it like any tool.
                let state = if started {
                    State::Running
                } else if it.status.as_deref() == Some("failed") {
                    State::Failed
                } else {
                    State::Done
                };
                let name = match it.namespace.as_deref() {
                    Some(namespace) => format!("{namespace}.{}", show(it.tool.as_deref())),
                    None => show(it.tool.as_deref()).to_string(),
                };
                let mut tool = tool(ToolKind::Other, &name);
                tool.input = it.arguments.filter(|v| !v.is_null());
                if !started {
                    let said = (it.content_items.iter().flatten())
                        .filter(|c| c.kind.as_deref() == Some("inputText"))
                        .map(|c| c.text.clone().unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if it.success == Some(false) {
                        tool.error = Some(said);
                    } else {
                        tool.output = Some(said);
                    }
                }
                let title = if started {
                    "Using a tool"
                } else {
                    "Used a tool"
                };
                self.emit(out, &iid, Body::Tool(tool), state, title, &name, native());
            }
            Some("webSearch") => {
                let state = running_or(State::Done);
                let mut tool = tool(ToolKind::Search, "web_search");
                tool.query = it.query.clone();
                let title = if started {
                    "Searching the web"
                } else {
                    "Searched the web"
                };
                let text = it.query.unwrap_or_default();
                self.emit(out, &iid, Body::Tool(tool), state, title, &text, native());
            }
            Some("plan") => {
                if started {
                    return; // a plan has no text until it is complete
                }
                let body = Body::Plan(Plan {
                    text: it.text.clone(),
                    entries: Vec::new(),
                });
                let text = it.text.unwrap_or_default();
                self.emit(out, &iid, body, State::Done, "Plan", &text, native());
            }
            Some("sleep") => {
                let state = running_or(State::Done);
                let seconds = round_half_even(it.duration_ms.unwrap_or(0) as f64 / 1000.0);
                let text = format!("Waiting up to {seconds} seconds");
                let body = Body::Notice(Notice {
                    kind: NoticeKind::Waiting,
                    duration_ms: it.duration_ms,
                    reason: None,
                });
                self.emit(out, &iid, body, state, "Waiting", &text, native());
            }
            Some("contextCompaction") => {
                let state = running_or(State::Done);
                let body = Body::Notice(notice(NoticeKind::Compaction));
                let title = "Compacting the conversation";
                self.emit(out, &iid, body, state, title, "", native());
            }
            Some("imageView") => {
                let state = running_or(State::Done);
                let mut tool = tool(ToolKind::Read, "view_image");
                tool.path = it.path.clone();
                let text = it.path.unwrap_or_default();
                let title = "Viewing an image";
                self.emit(out, &iid, Body::Tool(tool), state, title, &text, native());
            }
            Some("subAgentActivity") => {
                // Codex's newer runtime: a subagent started, given more work, or ended. What the
                // subagent does is its own thread's, which the transport shows here (D-117).
                let (doing, done) = match it.activity.as_deref() {
                    Some("started") => ("Starting a subagent", "Started a subagent"),
                    Some("interacted") => {
                        ("Giving a subagent more work", "Gave a subagent more work")
                    }
                    Some("completed") => ("A subagent finished", "A subagent finished"),
                    Some("interrupted") => ("A subagent was stopped", "A subagent was stopped"),
                    _ => ("Subagent activity", "Subagent activity"),
                };
                let state = running_or(State::Done);
                let title = if started { doing } else { done };
                let tool = tool(ToolKind::Subagent, "subAgentActivity");
                let text = it.agent_path.unwrap_or_default();
                let body = Body::Tool(tool);
                self.emit_owned(out, in_turn, &iid, body, state, title, &text, native());
            }
            Some("collabAgentToolCall") => {
                // A tool for subagents: on the older runtime the one that starts them, and on
                // both the one that waits for them.
                let name = it.tool.clone().unwrap_or_default();
                let (kind, doing, done) = collab_tool(&name);
                let state = if started {
                    State::Running
                } else {
                    match it.status.as_deref() {
                        Some("failed") => State::Failed,
                        Some("interrupted") => State::Cancelled,
                        _ => State::Done,
                    }
                };
                let mut tool = tool(kind, &name);
                if let Some(prompt) = it.prompt.clone().filter(|p| !p.is_empty()) {
                    tool.input = Some(json!({"prompt": prompt}));
                }
                if !started {
                    let said: Vec<String> = (it.agents_states.iter().flatten())
                        .filter_map(|(_, s)| s.message.clone())
                        .collect();
                    tool.output = (!said.is_empty()).then(|| said.join("\n"));
                }
                // One that failed or was stopped is not titled as having worked.
                let title = match state {
                    State::Running => doing,
                    State::Failed => "A subagent tool failed",
                    State::Cancelled => "A subagent tool was stopped",
                    _ => done,
                };
                let text = it.prompt.unwrap_or_default();
                let body = Body::Tool(tool);
                self.emit_owned(out, in_turn, &iid, body, state, title, &text, native());
            }
            _ => self.unknown.push(native_kind.clone()),
        }
    }

    // -- a subagent's turns

    /// The item a turn is shown as: the turn itself, or, for a subagent, its task (D-117).
    fn turn_item(&self, turn: Option<&str>) -> String {
        if self.subagent.is_some() {
            format!("task:{}", show(turn))
        } else {
            format!("turn:{}", show(turn))
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
        let body = Body::Task(Task {
            kind: "subagent".into(),
            description: sub.description.clone(),
            subject: sub.subject.map(Into::into),
        });
        let text = sub.description.unwrap_or_default();
        let native = native(method, params);
        self.emit(
            out,
            &item,
            body,
            State::Running,
            "Subagent working",
            &text,
            native,
        );
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
        let text = (turn.error.and_then(|e| e.message))
            .or_else(|| self.subagent.as_ref().and_then(|s| s.description.clone()))
            .unwrap_or_default();
        if let Some(id) = &turn.id {
            self.usage.remove(&Some(id.clone()));
        }
        let native = native(method, params);
        self.emit(out, &item, body, state, title, &text, native);
        if current {
            self.turn = None; // nothing that follows belongs to this turn
        }
    }

    // -- approvals and questions

    fn approval(&mut self, method: &str, id: Option<Id>, params: &str, out: &mut Vec<AgentEvent>) {
        let Some(p) = self.read::<ApprovalRequest>(method, params) else {
            return;
        };
        let offered = p
            .available_decisions
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| vec![Value::from("accept"), Value::from("decline")]);
        let mut decisions = Vec::new();
        let mut choices = Vec::new();
        let count = offered.len();
        for (n, decision) in offered.into_iter().enumerate() {
            // A decision the driver does not know is not offered: its effect would be a guess.
            let Some((effect, label)) = decision_choice(&decision) else {
                continue;
            };
            let id = format!("c{n}");
            choices.push(Choice {
                id: id.clone(),
                label,
                effect,
            });
            decisions.push((id, decision));
        }
        // Codex accepts "decline" although it does not always list it (rule 2).
        if !decisions.iter().any(|(_, d)| *d == "decline") {
            let id = format!("c{count}");
            choices.push(Choice {
                id: id.clone(),
                label: "Deny".to_string(),
                effect: Effect::Deny,
            });
            decisions.push((id, Value::from("decline")));
        }
        let what = if method.contains("commandExecution") {
            "command"
        } else if method.contains("fileChange") {
            "file change"
        } else {
            "permission"
        };
        let item = self.request_item("approval", p.turn_id.as_deref(), id.as_ref());
        if let Some(id) = id {
            self.asked.insert(
                id,
                Pending::Approval {
                    item: item.clone(),
                    decisions,
                    choices: choices.clone(),
                    reply: Reply::Decision,
                    asked_for: None,
                },
            );
        }
        let body = Body::Approval(Approval {
            subject: p.item_id.map(Into::into),
            choices,
            answer: None,
        });
        let text = p.command.or(p.reason).unwrap_or_default();
        let title = format!("Allow this {what}?");
        let native = native(&format!("{method} (request)"), params);
        self.emit(out, &item, body, State::Waiting, &title, &text, native);
    }

    /// Codex asks for more permissions. The reply grants some, for the turn or for the session, or
    /// grants none, which denies them.
    fn permissions(
        &mut self,
        method: &str,
        id: Option<Id>,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let Some(p) = self.read::<ApprovalRequest>(method, params) else {
            return;
        };
        let choice = |id: &str, label: &str, effect| Choice {
            id: id.into(),
            label: label.into(),
            effect,
        };
        let choices = vec![
            choice("c0", "Allow for this turn", Effect::Allow),
            choice("c1", "Allow for this session", Effect::Allow),
            choice("c2", "Deny", Effect::Deny),
        ];
        let item = self.request_item("approval", p.turn_id.as_deref(), id.as_ref());
        if let Some(id) = id {
            self.asked.insert(
                id,
                Pending::Approval {
                    item: item.clone(),
                    decisions: Vec::new(),
                    choices: choices.clone(),
                    reply: Reply::Grant,
                    asked_for: p.permissions.clone(),
                },
            );
        }
        let body = Body::Approval(Approval {
            subject: p.item_id.map(Into::into),
            choices,
            answer: None,
        });
        let text = p.reason.unwrap_or_default();
        let native = native(&format!("{method} (request)"), params);
        let title = "Allow this permission?";
        self.emit(out, &item, body, State::Waiting, title, &text, native);
    }

    /// An MCP server asks the person for something. Codex also uses this to approve an MCP tool
    /// call.
    fn elicitation(
        &mut self,
        method: &str,
        id: Option<Id>,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let Some(p) = self.read::<Elicitation>(method, params) else {
            return;
        };
        let native = native(&format!("{method} (request)"), params);
        let text = p.message.unwrap_or_default();
        let meta = p.meta.unwrap_or_default();
        let approval = meta.codex_approval_kind;
        if approval.as_deref() == Some("mcp_tool_call") {
            let choices = vec![
                Choice {
                    id: "c0".into(),
                    label: "Allow once".into(),
                    effect: Effect::Allow,
                },
                Choice {
                    id: "c1".into(),
                    label: "Deny".into(),
                    effect: Effect::Deny,
                },
            ];
            let decisions = vec![
                ("c0".to_string(), Value::from("accept")),
                ("c1".to_string(), Value::from("decline")),
            ];
            let item = self.request_item("approval", p.turn_id.as_deref(), id.as_ref());
            if let Some(id) = id {
                self.asked.insert(
                    id,
                    Pending::Approval {
                        item: item.clone(),
                        decisions,
                        choices: choices.clone(),
                        reply: Reply::Action,
                        asked_for: None,
                    },
                );
            }
            // The MCP tool waiting for this approval: the open one with the server and tool it
            // names. Codex can run several at once; with no single match, it names none.
            let name = match (&p.server_name, &meta.tool_name) {
                (Some(server), Some(tool)) => Some(format!("{server}.{tool}")),
                _ => None,
            };
            let tools: Vec<&String> = (self.open.iter())
                .filter(|(_, ev)| match ev.body() {
                    Body::Tool(t) => t.kind == ToolKind::Mcp && name.iter().all(|n| *n == t.name),
                    _ => false,
                })
                .map(|(item, _)| item)
                .collect();
            let subject = match tools[..] {
                [item] => Some(item.as_str().into()),
                _ => None,
            };
            let body = Body::Approval(Approval {
                subject,
                choices,
                answer: None,
            });
            return self.emit(
                out,
                &item,
                body,
                State::Waiting,
                "Allow this tool?",
                &text,
                native,
            );
        }
        // A form from an MCP server. The chat cannot show one yet, so the driver declines it at
        // once, so that Codex does not wait, and says so in the chat (D-064).
        if let Some(id) = &id {
            let reply = json!({"id": id, "result": {"action": "decline", "content": null}});
            self.outbox.push(reply.to_string());
        }
        let item = self.request_item("form", p.turn_id.as_deref(), id.as_ref());
        let body = Body::Notice(notice(NoticeKind::Warning));
        let title = "A form the chat cannot show was declined";
        self.emit(out, &item, body, State::Done, title, &text, native);
    }

    fn user_input(
        &mut self,
        method: &str,
        id: Option<Id>,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let Some(p) = self.read::<UserInput>(method, params) else {
            return;
        };
        let asked: Vec<Asked> = p
            .questions
            .unwrap_or_default()
            .into_iter()
            .map(|q| Asked {
                id: q.id.unwrap_or_default(),
                header: q.header,
                text: q.question.unwrap_or_default(),
                options: q
                    .options
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(k, o)| Offered {
                        id: format!("o{k}"),
                        label: o.label.unwrap_or_default(),
                        description: o.description,
                    })
                    .collect(),
                multiple: false,
                allow_text: q.is_other.unwrap_or(false),
                secret: q.is_secret.unwrap_or(false),
            })
            .collect();
        let text = asked.first().map(|q| q.text.clone()).unwrap_or_default();
        let item = self.request_item("question", p.turn_id.as_deref(), id.as_ref());
        let native = native(&format!("{method} (request)"), params);
        let subject = p.item_id.map(Into::into);
        self.ask(out, id, &item, subject, asked, &text, native);
    }

    #[allow(clippy::too_many_arguments)]
    fn ask(
        &mut self,
        out: &mut Vec<AgentEvent>,
        id: Option<Id>,
        item: &str,
        subject: Option<marmot_agent_events::ItemId>,
        asked: Vec<Asked>,
        text: &str,
        native: Option<Native>,
    ) {
        // Two questions with one id could not be answered apart. The request is refused, as one
        // the driver cannot show (`codex_sent`).
        let Ok(question) = Question::new(AnswerVia::Request, subject, asked) else {
            self.unknown
                .push("a question with two questions of one id".into());
            return;
        };
        if let Some(id) = id {
            self.asked.insert(
                id,
                Pending::Question {
                    item: item.to_string(),
                },
            );
        }
        let body = Body::Question(question);
        self.emit(out, item, body, State::Waiting, "Question", text, native);
    }

    /// The client answered a request from Codex.
    fn answer(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        let Some(asked) = m.id.as_ref().and_then(|id| self.asked.remove(id)) else {
            return;
        };
        let (Pending::Approval { item, .. } | Pending::Question { item, .. }) = &asked;
        self.answered.insert(item.clone());
        let result = m.result.map_or("{}", RawValue::get);
        match asked {
            Pending::Approval {
                item,
                decisions,
                choices,
                reply: replied_by,
                ..
            } => {
                let Some(prev) = self.open_event(&item) else {
                    return;
                };
                let Some(reply) = self.read::<ApprovalReply>("a reply", result) else {
                    return;
                };
                let (decision, chosen) = match replied_by {
                    Reply::Grant => {
                        let granted = reply.permissions.iter().flatten().any(|(_, v)| grants(v));
                        let chosen = match (granted, reply.scope.as_deref()) {
                            (false, _) => "c2",
                            (true, Some("session")) => "c1",
                            (true, _) => "c0",
                        };
                        (None, Some(chosen.to_string()))
                    }
                    Reply::Decision | Reply::Action => {
                        let decision = if replied_by == Reply::Action {
                            reply.action
                        } else {
                            reply.decision
                        };
                        let chosen = decisions
                            .iter()
                            .find(|(_, d)| Some(d) == decision.as_ref())
                            .map(|(id, _)| id.clone());
                        (decision, chosen)
                    }
                };
                let choice = chosen
                    .as_ref()
                    .and_then(|id| choices.iter().find(|c| &c.id == id));
                let label = match (choice, &decision) {
                    (Some(c), _) => c.label.clone(),
                    (None, Some(Value::String(word))) => word.clone(),
                    (None, Some(other)) => other.to_string(),
                    (None, None) => "None".to_string(),
                };
                let state = match choice.map(|c| c.effect) {
                    Some(Effect::Allow) => State::Done,
                    _ => State::Declined,
                };
                let Body::Approval(approval) = prev.body().clone() else {
                    return;
                };
                let body = Body::Approval(Approval {
                    answer: Some(ApprovalAnswer { choice: chosen }),
                    ..approval
                });
                let title = prev.header().title.clone();
                let text = format!("Answered: {label}");
                let native = native("response to the approval request", result);
                self.emit(out, &item, body, state, &title, &text, native);
            }
            Pending::Question { item, .. } => {
                let Some(prev) = self.open_event(&item) else {
                    return;
                };
                let Body::Question(question) = prev.body().clone() else {
                    return;
                };
                let Some(reply) = self.read::<QuestionReply>("a reply", result) else {
                    return;
                };
                let answers = answered(question.questions(), reply);
                let secret = question.questions().iter().any(|q| q.secret);
                let Ok(question) = question.answered(answers) else {
                    return;
                };
                // The reply holds the answer; a secret question's events no longer carry it
                // (D-056).
                let native = if secret {
                    None
                } else {
                    native("response to the question request", result)
                };
                let title = prev.header().title.clone();
                let body = Body::Question(question);
                self.emit(out, &item, body, State::Done, &title, "Answered", native);
            }
        }
    }

    fn response(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        let sent = m.id.as_ref().and_then(|id| self.requests.remove(id));
        let (method, owner) = match sent {
            Some(Sent { method, owner }) => (Some(method), owner),
            None => (None, (self.session.clone(), self.turn.clone())),
        };
        // A change of settings Codex took holds from now, whether or not Codex reports it again.
        let asked = m.id.as_ref().and_then(|id| self.asked_settings.remove(id));
        if let (Some((model, effort)), None) = (asked, m.error) {
            self.known.model = model.or(self.known.model.take());
            self.known.effort = effort.or(self.known.effort.take());
        }
        if let Some(error) = m.error {
            self.starts.retain(|(id, ..)| Some(id) != m.id.as_ref()); // that request starts no turn

            let Some(e) = self.read::<ErrorReply>("an error reply", error.get()) else {
                return;
            };
            // Numbered like every item the driver names, not by request id, which Codex reuses.
            let item = self.made_up("error");
            let body = Body::Error(Error {
                request: method.clone(),
                code: e.code,
                will_retry: None,
            });
            let text = e.message.unwrap_or_default();
            let kind = format!("error response to {}", show(method.as_deref()));
            let native = native(&kind, error.get());
            let title = "The harness refused a request";
            let state = State::Failed;
            // It belongs where the request was sent, whatever has finished or changed since.
            return self.emit_owned(out, owner, &item, body, state, title, &text, native);
        }
        let result = m.result.map_or("{}", RawValue::get);
        match method.as_deref() {
            Some("model/list") => {
                let Some(list) = self.read::<ListReply>("a reply", result) else {
                    return;
                };
                self.models = Some(
                    list.data
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|x| x.id)
                        .collect(),
                );
                self.publish_lists("model/list", result, out);
            }
            Some("collaborationMode/list") => {
                let Some(list) = self.read::<ListReply>("a reply", result) else {
                    return;
                };
                self.modes = Some(
                    list.data
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|x| x.mode)
                        .collect(),
                );
                self.publish_lists("collaborationMode/list", result, out);
            }
            Some(started @ ("thread/start" | "thread/resume")) => {
                let Some(settings) = self.read::<messages::Settings>("a reply", result) else {
                    return;
                };
                if let Some(thread) = &settings.thread {
                    self.use_thread(thread.id.clone(), out);
                    let kind = format!("response to {started}");
                    self.thread_settings(&settings, &kind, result, out);
                }
            }
            _ => {}
        }
    }

    /// A list of models or modes that arrives after the session item reaches it.
    fn publish_lists(&mut self, method: &str, result: &str, out: &mut Vec<AgentEvent>) {
        if self.session.is_some() {
            let kind = format!("response to {method}");
            self.thread_settings(&messages::Settings::default(), &kind, result, out);
        }
    }

    /// A second thread replaces the first as this chat's session. Codex sends nothing to say the
    /// first is over, so the driver ends its session item.
    fn use_thread(&mut self, thread: Option<String>, out: &mut Vec<AgentEvent>) {
        if let Some(old) = self.session.clone()
            && thread.as_ref() != Some(&old)
        {
            let text = "Not answered: a new session started";
            self.close_message_questions(State::Cancelled, text, out);
            self.end_session(&old, out);
            self.known = Known::default();
            self.answered.clear();
            // A turn the old thread was asked to start is not this thread's.
            self.starts
                .retain(|(.., named)| named.as_ref() != Some(&old));
            // A session the chat goes back to runs again, though its settings may be unchanged.
            self.sessions
                .remove(&format!("session:{}", show(thread.as_deref())));
        }
        self.session = thread;
    }

    fn thread_settings(
        &mut self,
        src: &messages::Settings,
        native_kind: &str,
        native_text_: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let sandbox = src
            .sandbox_policy
            .as_ref()
            .or(src.sandbox.as_ref())
            .and_then(|s| match s {
                Sandbox::Word(word) => Some(word.clone()),
                Sandbox::Object { kind } => kind.clone(),
            });
        let version = match &src.thread {
            Some(thread) => thread.cli_version.clone(),
            None => src.cli_version.clone(),
        };
        let known = &mut self.known;
        for (slot, value) in [
            (&mut known.version, version),
            (&mut known.model, src.model.clone()),
            (
                &mut known.effort,
                src.effort.clone().or(src.reasoning_effort.clone()),
            ),
            (
                &mut known.approval_policy,
                src.approval_policy.as_ref().and_then(|p| match p {
                    Policy::Word(word) => Some(word.clone()),
                    Policy::Object(kinds) => kinds.keys().next().cloned(),
                }),
            ),
            (&mut known.sandbox, sandbox),
            (
                &mut known.mode,
                src.collaboration_mode.as_ref().and_then(|m| m.mode.clone()),
            ),
        ] {
            if value.is_some() {
                *slot = value;
            }
        }
        let k = &self.known;
        let settings = vec![
            setting(
                "model",
                "Model",
                k.model.clone(),
                self.models.clone(),
                false,
            ),
            setting("effort", "Reasoning effort", k.effort.clone(), None, false),
            setting("mode", "Mode", k.mode.clone(), self.modes.clone(), false),
            setting(
                "approval_policy",
                "Approval policy",
                k.approval_policy.clone(),
                None,
                true,
            ),
            setting("sandbox", "Sandbox", k.sandbox.clone(), None, true),
        ];
        let session = Session {
            harness_version: k.version.clone().unwrap_or_default(),
            settings,
            actions: vec![Action {
                id: "compact".into(),
                label: "Compact the conversation".into(),
                description: None,
            }],
            commands: Vec::new(),
        };
        if self.subagent.is_some() {
            return; // a subagent's thread is no session of the chat's (D-117)
        }
        let sid = show(self.session.as_deref());
        let item = format!("session:{sid}");
        if self.sessions.get(&item) == Some(&session) {
            return; // a harness that repeats its settings makes no edit
        }
        self.sessions.insert(item.clone(), session.clone());
        let text = session
            .settings
            .iter()
            .filter_map(|s| s.value.as_ref().map(|v| format!("{}: {v}", s.label)))
            .collect::<Vec<_>>()
            .join(", ");
        let turn = self.turn.take(); // a session belongs to no turn
        let native = native(native_kind, native_text_);
        let body = Body::Session(session);
        self.emit(out, &item, body, State::Running, "Session", &text, native);
        self.turn = turn;
    }

    /// Nothing Codex sends says a session is over. The driver ends the item itself, when the
    /// chat's session is replaced.
    fn end_session(&mut self, sid: &str, out: &mut Vec<AgentEvent>) {
        let item = format!("session:{sid}");
        let Some(prev) = self.open_event(&item) else {
            return;
        };
        let turn = self.turn.take();
        let (body, text) = (prev.body().clone(), prev.header().text.clone());
        self.emit(out, &item, body, State::Done, "Session ended", &text, None);
        self.turn = turn;
    }

    // -- other

    fn plan(&mut self, method: &str, params: &str, out: &mut Vec<AgentEvent>) {
        let Some(p) = self.read::<PlanUpdated>(method, params) else {
            return;
        };
        let entries: Vec<PlanEntry> = p
            .plan
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| {
                let state = match e.status.as_deref() {
                    Some("pending") => PlanEntryState::Pending,
                    Some("inProgress") => PlanEntryState::Running,
                    Some("completed") => PlanEntryState::Done,
                    _ => return None,
                };
                Some(PlanEntry {
                    text: e.step.unwrap_or_default(),
                    state,
                })
            })
            .collect();
        let text = entries
            .iter()
            .map(|e| {
                let mark = if e.state == PlanEntryState::Done {
                    'x'
                } else {
                    ' '
                };
                format!("[{mark}] {}", e.text)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let done = !entries.is_empty() && entries.iter().all(|e| e.state == PlanEntryState::Done);
        let state = if done { State::Done } else { State::Running };
        let item = format!("plan:{}", show(p.turn_id.as_deref()));
        let body = Body::Plan(Plan {
            text: p.explanation,
            entries,
        });
        let native = native(method, params);
        self.emit(out, &item, body, state, "Plan", &text, native);
    }

    // -- events

    /// A turn ended. Anything still running will never report again, so the driver closes it.
    fn close_open_items(&mut self, out: &mut Vec<AgentEvent>, keep_message_questions: bool) {
        let open: Vec<(String, AgentEvent)> = self.open.clone();
        for (item, prev) in open {
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

    /// The item for a request from Codex. Codex counts its requests from 0 again when it restarts,
    /// so the id names the turn as well, which keeps it unique.
    fn request_item(&self, kind: &str, turn: Option<&str>, id: Option<&Id>) -> String {
        let turn = turn.or(self.turn.as_deref());
        format!("{kind}:{}:{}", show(turn), show_id(id))
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
        let (session, turn) = match &self.subagent {
            Some(sub) => {
                let root = turn.map(|t| self.roots.get(&t).cloned().unwrap_or(t));
                (sub.session.clone(), root.or(sub.root.clone()))
            }
            None => (session, turn),
        };
        let seq = self.seq.entry(session.clone()).or_default();
        *seq += 1;
        let header = Header {
            harness: "codex".into(),
            session: session.map(Into::into),
            turn: turn.map(Into::into),
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

/// What the client's reply answered, for each question asked: the ids of the options whose labels
/// it gave, and anything else as typed text. A secret question records only that it was answered
/// (D-041). Codex replies with labels.
fn answered(questions: &[Asked], reply: QuestionReply) -> Vec<Answered> {
    let given: Vec<(String, Vec<Value>)> = (reply.answers.unwrap_or_default().0.into_iter())
        .map(|(id, given)| (id, given.answers.unwrap_or_default()))
        .collect();
    let mut out = Vec::new();
    for q in questions {
        let Some((_, values)) = given.iter().find(|(id, _)| *id == q.id) else {
            continue;
        };
        if q.secret {
            out.push(Answered {
                question: q.id.clone(),
                options: Vec::new(),
                text: None,
            });
            continue;
        }
        let mut options = Vec::new();
        let mut typed = Vec::new();
        for value in values {
            let offered = value
                .as_str()
                .and_then(|label| q.options.iter().find(|o| o.label == label));
            match (offered, value) {
                (Some(o), _) => options.push(o.id.clone()),
                (None, Value::String(text)) => typed.push(text.clone()),
                (None, other) => typed.push(other.to_string()),
            }
        }
        out.push(Answered {
            question: q.id.clone(),
            options,
            text: if typed.is_empty() {
                None
            } else {
                Some(typed.join("\n"))
            },
        });
    }
    out
}

/// What a decision Codex offers on an approval does, and what its button says: each kind Codex's
/// schema (0.169.0) documents. A network rule allows or denies by its own action, and lasts.
fn decision_choice(decision: &Value) -> Option<(Effect, String)> {
    let (effect, label) = match decision {
        Value::String(word) => match word.as_str() {
            "accept" => (Effect::Allow, "Allow once".to_string()),
            "acceptForSession" => (Effect::Allow, "Allow for this session".to_string()),
            "decline" => (Effect::Deny, "Deny".to_string()),
            "cancel" => (Effect::Deny, "Deny and stop".to_string()),
            _ => return None,
        },
        Value::Object(one) if one.len() == 1 => {
            if one.contains_key("acceptWithExecpolicyAmendment") {
                let label = "Allow, and don't ask again for this command";
                (Effect::Allow, label.to_string())
            } else {
                let rule = &one.get("applyNetworkPolicyAmendment")?["network_policy_amendment"];
                let host = rule["host"].as_str()?;
                match rule["action"].as_str()? {
                    "allow" => (Effect::Allow, format!("Always allow {host}")),
                    "deny" => (Effect::Deny, format!("Always deny {host}")),
                    _ => return None,
                }
            }
        }
        _ => return None,
    };
    Some((effect, label))
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

/// Whether a permission grant grants anything: a true flag or a non-empty list somewhere in it.
/// `{"network": {"enabled": false}}` and `{"fileSystem": {"read": [], "write": []}}` grant nothing.
fn grants(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.values().any(grants),
        Value::Array(items) => !items.is_empty(),
        Value::Bool(flag) => *flag,
        _ => false,
    }
}

/// A count less another, as Codex reports them; a count it left out is nought.
fn less(a: &Option<u64>, b: &Option<u64>) -> u64 {
    a.unwrap_or(0).saturating_sub(b.unwrap_or(0))
}

/// A value Codex left out, written as the prototype writes it.
fn show(value: Option<&str>) -> String {
    value.unwrap_or("None").to_string()
}

fn show_id(id: Option<&Id>) -> String {
    id.map_or_else(|| "None".to_string(), ToString::to_string)
}

fn reasoning_text(part: &Value) -> String {
    match part {
        Value::String(text) => text.clone(),
        Value::Object(o) => match o.get("text") {
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => part.to_string(),
        },
        other => other.to_string(),
    }
}

fn change_kind(kind: NativeChange) -> Option<ChangeKind> {
    let word = match kind {
        NativeChange::Word(word) => Some(word),
        NativeChange::Object { kind } => kind,
    };
    match word.as_deref() {
        Some("add") => Some(ChangeKind::Add),
        Some("update") => Some(ChangeKind::Update),
        Some("delete") => Some(ChangeKind::Delete),
        _ => None,
    }
}

/// A tool for subagents: whether it hands one work, and how it is titled while running and once
/// done.
fn collab_tool(name: &str) -> (ToolKind, &'static str, &'static str) {
    match name {
        "spawnAgent" => (
            ToolKind::Subagent,
            "Starting a subagent",
            "Started a subagent",
        ),
        "sendInput" | "sendMessage" | "followupTask" => (
            ToolKind::Subagent,
            "Giving a subagent more work",
            "Gave a subagent more work",
        ),
        "resumeAgent" => (
            ToolKind::Subagent,
            "Resuming a subagent",
            "Resumed a subagent",
        ),
        "wait" => (
            ToolKind::Other,
            "Waiting for subagents",
            "Waited for subagents",
        ),
        "closeAgent" => (ToolKind::Other, "Closing a subagent", "Closed a subagent"),
        "interruptAgent" => (ToolKind::Other, "Stopping a subagent", "Stopped a subagent"),
        "listAgents" => (ToolKind::Other, "Listing subagents", "Listed subagents"),
        _ => (
            ToolKind::Other,
            "Using a subagent tool",
            "Used a subagent tool",
        ),
    }
}

fn tool(kind: ToolKind, name: &str) -> Tool {
    Tool {
        kind,
        name: name.to_string(),
        command: None,
        cwd: None,
        exit_code: None,
        duration_ms: None,
        output: None,
        changes: Vec::new(),
        query: None,
        path: None,
        input: None,
        error: None,
    }
}

fn notice(kind: NoticeKind) -> Notice {
    Notice {
        kind,
        duration_ms: None,
        reason: None,
    }
}

fn setting(
    id: &str,
    label: &str,
    value: Option<String>,
    options: Option<Vec<String>>,
    approve: bool,
) -> Setting {
    Setting {
        id: id.to_string(),
        label: label.to_string(),
        value,
        options,
        approve,
    }
}

/// Rounds half to even, as the prototype's Python does.
fn round_half_even(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        (r - x.signum()) as i64
    } else {
        r as i64
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
            DriverAction::Interrupt { turn } => {
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

    /// The reply to the Codex request that an answer is for.
    pub fn reply(&self, answer: &DriverAnswer) -> Result<String, Refusal> {
        let refuse = |reason| Err(Refusal::new(reason));
        let item = answer.item.as_str();
        if self.answered.contains(item) {
            return refuse(Reason::AlreadyAnswered);
        }
        let Some((id, pending)) = self.asked.iter().find(|(_, p)| match p {
            Pending::Approval { item: i, .. } | Pending::Question { item: i, .. } => i == item,
        }) else {
            return refuse(if self.open_event(item).is_some() {
                Reason::NotWaiting
            } else {
                Reason::UnknownItem
            });
        };
        // An approval or question the driver closed, such as at a turn's end, takes no answer.
        if !self
            .open_event(item)
            .is_some_and(|e| e.header().state == State::Waiting)
        {
            return refuse(Reason::NotWaiting);
        }
        let result = match (pending, &answer.given) {
            (
                Pending::Approval {
                    decisions,
                    reply,
                    asked_for,
                    ..
                },
                Given::Choice(choice),
            ) => match reply {
                Reply::Decision | Reply::Action => {
                    let Some((_, decision)) = decisions.iter().find(|(c, _)| c == choice) else {
                        return refuse(Reason::Invalid);
                    };
                    if *reply == Reply::Decision {
                        json!({"decision": decision})
                    } else if *decision == "accept" {
                        json!({"action": decision, "content": {}})
                    } else {
                        json!({"action": decision, "content": null})
                    }
                }
                // c0 grants for the turn, c1 for the session, c2 grants nothing.
                Reply::Grant => match choice.as_str() {
                    "c0" => json!({"permissions": asked_for, "scope": "turn"}),
                    "c1" => json!({"permissions": asked_for, "scope": "session"}),
                    "c2" => json!({"permissions": {}}),
                    _ => return refuse(Reason::Invalid),
                },
            },
            (Pending::Question { item }, Given::Questions(replies)) => {
                let Some(Body::Question(question)) =
                    self.open_event(item).map(|e| e.body().clone())
                else {
                    return refuse(Reason::NotWaiting);
                };
                let mut answers = serde_json::Map::new();
                for given in replies {
                    let Some(asked) = question.questions().iter().find(|q| q.id == given.question)
                    else {
                        return refuse(Reason::Invalid);
                    };
                    // Codex's questions take labels; Codex turns them back into choices.
                    let mut said = Vec::new();
                    for option in &given.options {
                        let Some(offered) = asked.options.iter().find(|o| &o.id == option) else {
                            return refuse(Reason::Invalid);
                        };
                        said.push(offered.label.clone());
                    }
                    said.extend(given.text.clone());
                    answers.insert(given.question.clone(), json!({"answers": said}));
                }
                json!({"answers": answers})
            }
            _ => return refuse(Reason::Invalid),
        };
        Ok(json!({"id": id, "result": result}).to_string())
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
