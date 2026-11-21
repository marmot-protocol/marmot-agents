//! What an ACP agent says, turned into agent events.
//!
//! The translator reads every message on the connection, in both directions, in the order they
//! went. It needs the client's side too: a turn is the client's `session/prompt`, and the client's
//! reply to a permission request is how it knows the answer. It does no input or output of its own,
//! so it is tested by replaying recordings (`fixtures/recordings/acp`), and must produce the events
//! the prototype produces for each (`expected/` beside them).
//!
//! What only ACP brings:
//!
//! - **A turn has no id.** It starts with the client's `session/prompt` and ends with the reply to
//!   it. The translator names it after the session and that request's id, so the driver must never
//!   use a request id twice in a session, across a restart too.
//! - **Words and thinking arrive in chunks.** An item is sent whole, never as the text so far
//!   (D-012). The chunks of one message become one `message` or `reasoning` item, sent when anything
//!   else arrives or the turn ends. Its native message is the list of the chunks' `params`, in order,
//!   each the exact text that arrived.
//! - **`session/load` replays the conversation** before the agent replies to it. The chat already
//!   holds all of it, so nothing in the replay becomes an event.
//!
//! The native message of any other event is the whole `params` of the agent's message, or the whole
//! `result` or `error` of a reply, as the exact text that arrived. An event the driver makes up
//! carries none.

use agent_client_protocol_schema::v1::{
    ContentBlock, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationContentValue, ElicitationMode, ElicitationScope, InitializeResponse,
    LoadSessionResponse, NewSessionResponse, PlanEntryStatus, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    ResumeSessionResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOptions, SessionNotification, SessionUpdate, SetSessionConfigOptionResponse,
    StopReason, ToolCallContent, ToolCallStatus, ToolKind as AcpKind,
};
use marmot_agent_events::body::{
    AnswerVia, Answered, Approval, ApprovalAnswer, Asked, Change, ChangeKind, Choice, Command,
    Effect, Error, Message, Offered, Plan, PlanEntry, PlanEntryState, Question, Reasoning, Session,
    Setting, StartedBy, Tool, ToolKind, Turn, Usage,
};
use marmot_agent_events::{AgentEvent, Body, Header, Native, State};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::{HashMap, HashSet};

/// Which way a message went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From the agent to the driver.
    In,
    /// From the driver to the agent.
    Out,
}

/// A JSON-RPC message, with its parts kept as the text that arrived.
#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(default, borrow)]
    id: Option<&'a RawValue>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default, borrow)]
    params: Option<&'a RawValue>,
    #[serde(default, borrow)]
    result: Option<&'a RawValue>,
    #[serde(default, borrow)]
    error: Option<&'a RawValue>,
}

/// What an error reply says.
#[derive(Deserialize)]
struct ErrorReply {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
}

/// A request the client sent: its method, the session it names, and where it was sent.
struct Sent {
    method: String,
    session: Option<String>,
    owner: Owner,
}

/// A request from the agent that the client has not answered.
enum Asked_ {
    /// Each choice's id, and the option id the agent takes for it.
    Approval {
        item: String,
        options: Vec<(String, String)>,
    },
    /// For each question, each option's id and the value the form takes for it, and the field
    /// that holds an answer of one's own, if the form has one.
    Question {
        item: String,
        values: HashMap<String, Vec<(String, String)>>,
        custom: HashMap<String, String>,
    },
}

/// The message or thinking being streamed.
#[derive(Clone)]
struct Streamed {
    kind: Chunk,
    texts: String,
    payloads: Vec<String>,
    owner: Owner,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Chunk {
    Message,
    Thought,
}

impl Chunk {
    fn name(self) -> &'static str {
        match self {
            Self::Message => "agent_message_chunk",
            Self::Thought => "agent_thought_chunk",
        }
    }
}

/// What the agent has said about a tool call, merged: a later update replaces each field it has.
#[derive(Default)]
struct ToolCall {
    title: Option<String>,
    name: Option<String>,
    kind: Option<AcpKind>,
    status: Option<ToolCallStatus>,
    content: Option<Vec<ToolCallContent>>,
    locations: Option<Vec<String>>,
    raw_input: Option<Value>,
    raw_output: Option<Value>,
}

/// The session and turn an event belongs to.
type Owner = (Option<String>, Option<String>);

/// Turns the messages of one ACP connection into agent events, for one chat.
pub struct Translator {
    harness: String,
    version: Option<String>,
    session: Option<String>,
    turn: Option<String>,
    /// Events made in each session, for each event's `seq`.
    seq: HashMap<Option<String>, u64>,
    /// Items not yet finished, in the order they started, with their last event.
    open: Vec<(String, AgentEvent)>,
    /// Each session item's body as it was last sent.
    sessions: HashMap<String, Session>,
    /// The client's requests not yet answered, by the id's JSON text.
    sent: HashMap<String, Sent>,
    /// The agent's requests not yet answered, by the id's JSON text.
    asked: HashMap<String, Asked_>,
    /// The session being replayed by `session/load`, until the reply.
    loading: Option<String>,
    /// The id of the message or thinking being streamed.
    buffer: Option<String>,
    /// Every message and thinking streamed, by id, with its chunks so far.
    streamed: HashMap<String, Streamed>,
    tools: HashMap<String, ToolCall>,
    /// The last event sent for each tool call: its state, title, text and body.
    last_tool: HashMap<String, (State, String, String, Tool)>,
    /// Tool calls whose permission was refused in this turn.
    declined: HashSet<String>,
    /// The client asked to cancel the turn.
    cancelled: bool,
    config: Vec<SessionConfigOption>,
    commands: Vec<Command>,
    /// Items each turn made with no id of the agent's.
    counted: HashMap<Option<String>, u64>,
    /// Tool calls that are really a question, which the form asks.
    hidden: HashSet<String>,
    /// The id of the `session/prompt` that started the turn, as its JSON text.
    turn_request: Option<String>,
    unreadable: u64,
    unknown: Vec<String>,
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

impl Translator {
    /// A translator for a new connection.
    pub fn new() -> Self {
        Self {
            harness: "acp".into(),
            version: None,
            session: None,
            turn: None,
            seq: HashMap::new(),
            open: Vec::new(),
            sessions: HashMap::new(),
            sent: HashMap::new(),
            asked: HashMap::new(),
            loading: None,
            buffer: None,
            streamed: HashMap::new(),
            tools: HashMap::new(),
            last_tool: HashMap::new(),
            declined: HashSet::new(),
            cancelled: false,
            config: Vec::new(),
            commands: Vec::new(),
            counted: HashMap::new(),
            hidden: HashSet::new(),
            turn_request: None,
            unreadable: 0,
            unknown: Vec::new(),
        }
    }

    /// What the agent sent that the driver has no rule for, in the order it came.
    pub fn unknown(&self) -> &[String] {
        &self.unknown
    }

    /// How many messages could not be read: not JSON-RPC, or not the shape ACP gives them.
    pub fn unreadable(&self) -> u64 {
        self.unreadable
    }

    /// The agent's session followed: the chat's session, once one is picked.
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Reads one message, which went the way `direction` says, and gives the events it makes.
    pub fn feed(&mut self, direction: Direction, message: &str) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let Ok(m) = serde_json::from_str::<Envelope<'_>>(message) else {
            self.unreadable += 1;
            return out;
        };
        let params = m.params.map_or("{}", RawValue::get);
        if direction == Direction::In && m.method.as_deref() == Some("session/update") {
            self.update(params, &mut out);
            return out;
        }
        self.flush(&mut out); // anything else ends a message being streamed
        match (direction, m.method.as_deref()) {
            (Direction::Out, Some(method)) => self.client_request(m.id, method, params, &mut out),
            (Direction::Out, None) => self.client_answer(m.id, m.result, &mut out),
            (Direction::In, Some(method)) => self.agent_request(m.id, method, params, &mut out),
            (Direction::In, None) => self.response(&m, &mut out),
        }
        out
    }

    /// The agent process ended. Nothing it was doing will report again, so the driver closes the
    /// open items and the turn.
    pub fn harness_stopped(&mut self) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        self.flush(&mut out);
        self.close_open_items(&mut out);
        let item = format!("turn:{}", show(self.turn.as_deref()));
        if let Some(prev) = self.open_event(&item) {
            let body = prev.body().clone();
            self.emit(
                &mut out,
                &item,
                body,
                State::Cancelled,
                "Interrupted",
                "The agent stopped before the turn finished.",
                None,
            );
        }
        self.sent.clear();
        self.asked.clear();
        self.loading = None;
        self.forget_turn();
        out
    }

    /// What only a turn needs. A session that is resumed needs none of it.
    fn forget_turn(&mut self) {
        self.streamed.clear();
        self.tools.clear();
        self.last_tool.clear();
        self.counted.clear();
        self.declined.clear();
        self.hidden.clear();
        self.turn = None;
        self.turn_request = None;
        self.cancelled = false;
    }

    // -- streamed words and thinking

    fn flush(&mut self, out: &mut Vec<AgentEvent>) {
        let Some(item) = self.buffer.take() else {
            return;
        };
        let Some(s) = self.streamed.get(&item).cloned() else {
            return;
        };
        let (body, title) = match s.kind {
            Chunk::Message => (Body::Message(Message::default()), "Message"),
            Chunk::Thought => (Body::Reasoning(Reasoning::default()), "Thinking"),
        };
        let kind = format!("session/update [{}]", s.kind.name());
        let payload = format!("[{}]", s.payloads.join(","));
        self.emit_owned(
            out,
            s.owner,
            &item,
            body,
            State::Done,
            title,
            &s.texts,
            Some(native(&kind, &payload)),
        );
    }

    fn chunk(
        &mut self,
        kind: Chunk,
        block: &ContentBlock,
        message_id: Option<String>,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let ContentBlock::Text(text) = block else {
            let what = serde_json::to_value(block)
                .ok()
                .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            self.unknown.push(format!(
                "session/update [{}] with {what} content",
                kind.name()
            ));
            return;
        };
        let item = match message_id {
            Some(id) => id,
            None => {
                // ACP lets an agent leave the id out. Chunks with none belong to the message being
                // streamed, if it has none either; otherwise they start a new one.
                let prefix = format!("{}:{}:", show(self.turn.as_deref()), kind.name());
                match &self.buffer {
                    Some(current)
                        if current.starts_with(&prefix)
                            && self.streamed.get(current).is_some_and(|s| s.kind == kind) =>
                    {
                        current.clone()
                    }
                    _ => {
                        let n = self.counted.entry(self.turn.clone()).or_default();
                        *n += 1;
                        format!("{prefix}{n}")
                    }
                }
            }
        };
        let same = self.buffer.as_deref() == Some(item.as_str())
            && self.streamed.get(&item).is_some_and(|s| s.kind == kind);
        if !same {
            self.flush(out);
            // A message that goes on after something else came between its chunks is sent again
            // whole, as an edit of the item already sent.
            if self.streamed.get(&item).is_none_or(|s| s.kind != kind) {
                let owner = (self.session.clone(), self.turn.clone());
                self.streamed.insert(
                    item.clone(),
                    Streamed {
                        kind,
                        texts: String::new(),
                        payloads: Vec::new(),
                        owner,
                    },
                );
            }
            self.buffer = Some(item.clone());
        }
        if let Some(s) = self.streamed.get_mut(&item) {
            s.texts.push_str(&text.text);
            s.payloads.push(params.to_string());
        }
    }

    // -- what the client sends

    fn client_request(
        &mut self,
        id: Option<&RawValue>,
        method: &str,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let p: Value = serde_json::from_str(params).unwrap_or(Value::Null);
        let session = p
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(id) = id {
            let owner = (self.session.clone(), self.turn.clone());
            self.sent.insert(
                id.get().to_string(),
                Sent {
                    method: method.to_string(),
                    session: session.clone(),
                    owner,
                },
            );
        }
        match method {
            // ACP v1 has no message during a turn; OpenCode refuses one. The turn stays the first's.
            "session/prompt" if self.turn.is_some() => {}
            "session/prompt" => {
                self.turn = Some(format!("{}:{}", show(session.as_deref()), id_text(id)));
                self.turn_request = id.map(|id| id.get().to_string());
                self.cancelled = false;
                let item = format!("turn:{}", show(self.turn.as_deref()));
                let body = Body::Turn(Turn {
                    started_by: StartedBy::User,
                    duration_ms: None,
                    usage: None,
                });
                self.emit(out, &item, body, State::Running, "Working", "", None);
            }
            "session/cancel" => self.cancelled = true,
            "session/load" => self.loading = session,
            _ => {}
        }
    }

    fn client_answer(
        &mut self,
        id: Option<&RawValue>,
        result: Option<&RawValue>,
        out: &mut Vec<AgentEvent>,
    ) {
        let Some(asked) = id.and_then(|id| self.asked.remove(id.get())) else {
            return;
        };
        let item = match &asked {
            Asked_::Approval { item, .. } | Asked_::Question { item, .. } => item.clone(),
        };
        let Some(prev) = self.open_event(&item) else {
            return;
        };
        let result = result.map_or("{}", RawValue::get);
        let (title, body) = (prev.header().title.clone(), prev.body().clone());
        match (asked, body) {
            (Asked_::Approval { options, .. }, Body::Approval(approval)) => {
                let Ok(reply) = serde_json::from_str::<RequestPermissionResponse>(result) else {
                    self.unreadable += 1;
                    return;
                };
                let kind = "response to session/request_permission";
                let RequestPermissionOutcome::Selected(selected) = reply.outcome else {
                    return self.emit(
                        out,
                        &item,
                        Body::Approval(approval),
                        State::Cancelled,
                        &title,
                        "Not answered: the turn was cancelled",
                        Some(native(kind, result)),
                    );
                };
                let option = selected.option_id.0.to_string();
                let chosen = options
                    .iter()
                    .find(|(_, o)| *o == option)
                    .map(|(c, _)| c.clone());
                let choice = approval
                    .choices
                    .iter()
                    .find(|c| Some(&c.id) == chosen.as_ref())
                    .cloned();
                let effect = choice.as_ref().map_or(Effect::Deny, |c| c.effect);
                if effect == Effect::Deny
                    && let Some(subject) = &approval.subject
                {
                    self.declined.insert(subject.as_str().to_string());
                }
                let label = choice.map_or(option, |c| c.label);
                let state = if effect == Effect::Allow {
                    State::Done
                } else {
                    State::Declined
                };
                let body = Body::Approval(Approval {
                    answer: Some(ApprovalAnswer { choice: chosen }),
                    ..approval
                });
                self.emit(
                    out,
                    &item,
                    body,
                    state,
                    &title,
                    &format!("Answered: {label}"),
                    Some(native(kind, result)),
                );
            }
            (Asked_::Question { values, custom, .. }, Body::Question(question)) => {
                let Ok(reply) = serde_json::from_str::<CreateElicitationResponse>(result) else {
                    self.unreadable += 1;
                    return;
                };
                let kind = "response to elicitation/create";
                let content = match reply.action {
                    ElicitationAction::Accept(accept) => accept.content.unwrap_or_default(),
                    other => {
                        let state = if matches!(other, ElicitationAction::Decline) {
                            State::Declined
                        } else {
                            State::Cancelled
                        };
                        return self.emit(
                            out,
                            &item,
                            Body::Question(question),
                            state,
                            &title,
                            "Not answered",
                            Some(native(kind, result)),
                        );
                    }
                };
                let mut answers = Vec::new();
                for q in question.questions() {
                    // An answer of one's own comes in a field of its own.
                    let own = custom.get(&q.id).and_then(|field| content.get(field));
                    if !content.contains_key(&q.id) && own.is_none() {
                        continue;
                    }
                    let given: Vec<Value> = match content.get(&q.id) {
                        None => Vec::new(),
                        Some(ElicitationContentValue::StringArray(xs)) => {
                            xs.iter().cloned().map(Value::String).collect()
                        }
                        Some(other) => vec![serde_json::to_value(other).unwrap_or(Value::Null)],
                    };
                    let labels = values.get(&q.id).cloned().unwrap_or_default();
                    let mut options = Vec::new();
                    let mut typed = Vec::new();
                    for x in given {
                        match x.as_str().and_then(|s| labels.iter().find(|(_, v)| v == s)) {
                            Some((id, _)) => options.push(id.clone()),
                            None => {
                                typed.push(x.as_str().map_or_else(|| x.to_string(), str::to_string))
                            }
                        }
                    }
                    if let Some(ElicitationContentValue::String(own)) = own
                        && !own.is_empty()
                    {
                        typed.push(own.clone());
                    }
                    answers.push(Answered {
                        question: q.id.clone(),
                        options,
                        text: (!typed.is_empty()).then(|| typed.join("\n")),
                    });
                }
                let Ok(question) = question.answered(answers) else {
                    return;
                };
                self.emit(
                    out,
                    &item,
                    Body::Question(question),
                    State::Done,
                    &title,
                    "Answered",
                    Some(native(kind, result)),
                );
            }
            _ => {}
        }
    }

    // -- what the agent sends

    fn update(&mut self, params: &str, out: &mut Vec<AgentEvent>) {
        let Ok(note) = serde_json::from_str::<SessionNotification>(params) else {
            // An update the official types do not know, or one they cannot read.
            let kind = serde_json::from_str::<Value>(params).ok().and_then(|v| {
                v.pointer("/update/sessionUpdate")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            match kind {
                Some(kind) => {
                    self.flush(out);
                    self.unknown.push(format!("session/update [{kind}]"));
                }
                None => self.unreadable += 1,
            }
            return;
        };
        let replaying = self.loading.is_some();
        match note.update {
            SessionUpdate::AgentMessageChunk(c) => {
                if !replaying {
                    self.chunk(
                        Chunk::Message,
                        &c.content,
                        c.message_id.map(|m| m.0.to_string()),
                        params,
                        out,
                    );
                }
            }
            SessionUpdate::AgentThoughtChunk(c) => {
                if !replaying {
                    self.chunk(
                        Chunk::Thought,
                        &c.content,
                        c.message_id.map(|m| m.0.to_string()),
                        params,
                        out,
                    );
                }
            }
            SessionUpdate::UserMessageChunk(_) => self.flush(out), // an echo, or a replay
            SessionUpdate::ToolCall(call) => {
                self.flush(out);
                if !replaying {
                    let id = call.tool_call_id.0.to_string();
                    let t = self.tools.entry(id.clone()).or_default();
                    t.title = Some(call.title);
                    t.name = call.name.or(t.name.take());
                    t.kind = Some(call.kind);
                    t.status = Some(call.status);
                    t.content = Some(call.content);
                    t.locations = Some(
                        call.locations
                            .into_iter()
                            .map(|l| l.path.to_string_lossy().into_owned())
                            .collect(),
                    );
                    if call.raw_input.is_some() {
                        t.raw_input = call.raw_input;
                    }
                    if call.raw_output.is_some() {
                        t.raw_output = call.raw_output;
                    }
                    self.tool(&id, "session/update [tool_call]", params, out);
                }
            }
            SessionUpdate::ToolCallUpdate(update) => {
                self.flush(out);
                if !replaying {
                    let id = update.tool_call_id.0.to_string();
                    let f = update.fields;
                    let t = self.tools.entry(id.clone()).or_default();
                    if f.title.is_some() {
                        t.title = f.title;
                    }
                    if f.name.is_some() {
                        t.name = f.name;
                    }
                    if f.kind.is_some() {
                        t.kind = f.kind;
                    }
                    if f.status.is_some() {
                        t.status = f.status;
                    }
                    if f.content.is_some() {
                        t.content = f.content;
                    }
                    if let Some(locations) = f.locations {
                        t.locations = Some(
                            locations
                                .into_iter()
                                .map(|l| l.path.to_string_lossy().into_owned())
                                .collect(),
                        );
                    }
                    if f.raw_input.is_some() {
                        t.raw_input = f.raw_input;
                    }
                    if f.raw_output.is_some() {
                        t.raw_output = f.raw_output;
                    }
                    self.tool(&id, "session/update [tool_call_update]", params, out);
                }
            }
            SessionUpdate::Plan(plan) => {
                self.flush(out);
                if !replaying {
                    let entries: Vec<PlanEntry> = plan
                        .entries
                        .into_iter()
                        .map(|e| PlanEntry {
                            text: e.content,
                            state: match e.status {
                                PlanEntryStatus::InProgress => PlanEntryState::Running,
                                PlanEntryStatus::Completed => PlanEntryState::Done,
                                _ => PlanEntryState::Pending,
                            },
                        })
                        .collect();
                    let done = !entries.is_empty()
                        && entries.iter().all(|e| e.state == PlanEntryState::Done);
                    let text = entries
                        .iter()
                        .map(|e| {
                            format!(
                                "[{}] {}",
                                if e.state == PlanEntryState::Done {
                                    'x'
                                } else {
                                    ' '
                                },
                                e.text
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let item = format!("plan:{}", show(self.turn.as_deref()));
                    let state = if done { State::Done } else { State::Running };
                    let body = Body::Plan(Plan {
                        text: None,
                        entries,
                    });
                    self.emit(
                        out,
                        &item,
                        body,
                        state,
                        "Plan",
                        &text,
                        Some(native("session/update [plan]", params)),
                    );
                }
            }
            SessionUpdate::AvailableCommandsUpdate(update) => {
                self.flush(out);
                self.commands = update
                    .available_commands
                    .into_iter()
                    .map(|c| Command {
                        hint: serde_json::to_value(&c.input)
                            .ok()
                            .and_then(|v| v.get("hint").and_then(Value::as_str).map(str::to_string))
                            .unwrap_or_default(),
                        name: c.name,
                        description: Some(c.description),
                    })
                    .collect();
                self.announce("session/update [available_commands_update]", params, out);
            }
            SessionUpdate::ConfigOptionUpdate(update) => {
                self.flush(out);
                self.config = update.config_options;
                self.announce("session/update [config_option_update]", params, out);
            }
            // The context window, whose use a turn's reply states; and the session's title.
            SessionUpdate::UsageUpdate(_) | SessionUpdate::SessionInfoUpdate(_) => self.flush(out),
            other => {
                self.flush(out);
                let kind = serde_json::to_value(&other)
                    .ok()
                    .and_then(|v| {
                        v.get("sessionUpdate")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                self.unknown.push(format!("session/update [{kind}]"));
            }
        }
    }

    fn tool(&mut self, id: &str, kind_name: &str, params: &str, out: &mut Vec<AgentEvent>) {
        let Some(t) = self.tools.get(id) else {
            return;
        };
        if t.name.as_deref() == Some("question") {
            // OpenCode's question tool asks through a form, which becomes the one question item
            // (rule 4 of the event model).
            return;
        }
        let acp_kind = t.kind.unwrap_or(AcpKind::Other);
        let acp_name = serde_json::to_value(acp_kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "other".into());
        let kind = match acp_kind {
            AcpKind::Execute => ToolKind::Command,
            AcpKind::Edit | AcpKind::Delete | AcpKind::Move => ToolKind::Edit,
            AcpKind::Read => ToolKind::Read,
            AcpKind::Search => ToolKind::Search,
            AcpKind::Fetch => ToolKind::Fetch,
            _ => ToolKind::Other,
        };
        let mut state = match t.status.unwrap_or(ToolCallStatus::Pending) {
            ToolCallStatus::Completed => State::Done,
            ToolCallStatus::Failed => State::Failed,
            _ => State::Running,
        };
        if state == State::Failed && self.declined.contains(id) {
            state = State::Declined;
        } else if state == State::Failed && self.cancelled {
            state = State::Cancelled;
        }
        let raw = t.raw_input.as_ref().and_then(Value::as_object);
        let raw_str = |key: &str| {
            raw.and_then(|r| r.get(key))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let content = t.content.as_deref().unwrap_or_default();
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|c| match c {
                ToolCallContent::Content(c) => match &c.content {
                    ContentBlock::Text(text) => Some(text.text.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let output = (!texts.is_empty()).then(|| texts.join("\n"));
        let path = t
            .locations
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|p| !p.is_empty())
            .cloned();
        let title_text = t.title.clone().unwrap_or_default();
        let mut body = Tool {
            kind,
            name: t.name.clone().filter(|n| !n.is_empty()).unwrap_or(acp_name),
            command: None,
            cwd: None,
            exit_code: None,
            duration_ms: None,
            output,
            changes: Vec::new(),
            query: None,
            path: None,
            input: None,
            error: None,
        };
        let text = match kind {
            ToolKind::Command => {
                // ACP gives a command no field of its own. OpenCode puts it in the raw input,
                // which ACP leaves to the agent; until it does, the title stands in as its text.
                body.command = raw_str("command");
                body.cwd = raw
                    .and_then(|r| r.get("cwd"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // Nor an exit code. OpenCode puts one in `rawOutput.metadata.exit`, Cursor in
                // `rawOutput.exitCode`, Goose in `rawOutput.exit_code`.
                let ro = t.raw_output.as_ref();
                body.exit_code = [
                    ro.and_then(|o| o.get("metadata"))
                        .and_then(|m| m.get("exit")),
                    ro.and_then(|o| o.get("exitCode")),
                    ro.and_then(|o| o.get("exit_code")),
                ]
                .into_iter()
                .flatten()
                .find_map(Value::as_i64);
                if state == State::Done && body.exit_code.is_some_and(|c| c != 0) {
                    state = State::Failed;
                }
                body.command.clone().unwrap_or(title_text)
            }
            ToolKind::Edit => {
                body.changes = content
                    .iter()
                    .filter_map(|c| match c {
                        ToolCallContent::Diff(d) => Some(Change {
                            path: d.path.to_string_lossy().into_owned(),
                            change: Some(if d.old_text.is_none() {
                                ChangeKind::Add
                            } else {
                                ChangeKind::Update
                            }),
                            diff: changed_lines(d.old_text.as_deref(), &d.new_text),
                        }),
                        _ => None,
                    })
                    .collect();
                body.path = path.clone();
                let paths = body
                    .changes
                    .iter()
                    .map(|c| c.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                if !paths.is_empty() {
                    paths
                } else {
                    path.unwrap_or(title_text)
                }
            }
            ToolKind::Read => {
                body.path = path.clone();
                path.unwrap_or(title_text)
            }
            ToolKind::Search => {
                body.query = raw_str("pattern").or_else(|| raw_str("query"));
                body.query.clone().unwrap_or(title_text)
            }
            _ => {
                body.input = t.raw_input.clone();
                title_text
            }
        };
        let title = tool_title(kind, state);
        let now = (state, title.clone(), text.clone(), body.clone());
        if self.last_tool.get(id) == Some(&now) {
            return;
        }
        self.last_tool.insert(id.to_string(), now);
        self.emit(
            out,
            id,
            Body::Tool(body),
            state,
            &title,
            &text,
            Some(native(kind_name, params)),
        );
    }

    fn agent_request(
        &mut self,
        id: Option<&RawValue>,
        method: &str,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let key = id.map(|id| id.get().to_string()).unwrap_or_default();
        match method {
            "session/request_permission" => {
                let Ok(p) = serde_json::from_str::<RequestPermissionRequest>(params) else {
                    self.unreadable += 1;
                    return;
                };
                let subject = p.tool_call.tool_call_id.0.to_string();
                let mut choices = Vec::new();
                let mut options = Vec::new();
                for (n, o) in p.options.iter().enumerate() {
                    let kind = serde_json::to_value(o.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default();
                    let effect = if kind.starts_with("allow") {
                        Effect::Allow
                    } else {
                        Effect::Deny
                    };
                    choices.push(Choice {
                        id: format!("c{n}"),
                        label: o.name.clone(),
                        effect,
                    });
                    options.push((format!("c{n}"), o.option_id.0.to_string()));
                }
                let item = format!("approval:{subject}");
                let f = &p.tool_call.fields;
                let command = f
                    .raw_input
                    .as_ref()
                    .and_then(|r| r.get("command"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let text = command
                    .or_else(|| f.title.clone().filter(|t| !t.is_empty()))
                    .unwrap_or_default();
                let what = match f.kind {
                    Some(AcpKind::Execute) => "command",
                    Some(AcpKind::Edit | AcpKind::Delete | AcpKind::Move) => "edit",
                    _ => "action",
                };
                self.asked.insert(
                    key,
                    Asked_::Approval {
                        item: item.clone(),
                        options,
                    },
                );
                let body = Body::Approval(Approval {
                    subject: Some(subject.into()),
                    choices,
                    answer: None,
                });
                self.emit(
                    out,
                    &item,
                    body,
                    State::Waiting,
                    &format!("Allow this {what}?"),
                    &text,
                    Some(native(&format!("{method} (request)"), params)),
                );
            }
            "elicitation/create" => {
                let Ok(p) = serde_json::from_str::<CreateElicitationRequest>(params) else {
                    self.unreadable += 1;
                    return;
                };
                let ElicitationMode::Form(form) = &p.mode else {
                    self.unknown.push(format!("{method} (request)"));
                    return;
                };
                let subject = match &form.scope {
                    ElicitationScope::Session(scope) => {
                        scope.tool_call_id.as_ref().map(|t| t.0.to_string())
                    }
                    _ => None,
                };
                // The fields come in the order of their names: the official types keep them in a
                // sorted map.
                let schema = serde_json::to_value(&form.requested_schema).unwrap_or(Value::Null);
                let props = schema.get("properties").and_then(Value::as_object);
                // OpenCode adds a free-text field `<name>_custom` beside each question with
                // choices: an answer of one's own to that question, not a second question.
                let custom: HashMap<String, String> = props
                    .into_iter()
                    .flat_map(|p| p.keys())
                    .filter_map(|n| {
                        let base = n.strip_suffix("_custom")?;
                        props?
                            .contains_key(base)
                            .then(|| (base.to_string(), n.clone()))
                    })
                    .collect();
                let mut questions = Vec::new();
                let mut values = HashMap::new();
                for (name, f) in props.into_iter().flatten() {
                    if custom.values().any(|c| c == name) {
                        continue;
                    }
                    let many = f.get("type").and_then(Value::as_str) == Some("array");
                    let src = if many {
                        f.get("items").cloned().unwrap_or(Value::Null)
                    } else {
                        f.clone()
                    };
                    let listed = |key: &str| {
                        src.get(key)
                            .and_then(Value::as_array)
                            .filter(|a| !a.is_empty())
                            .cloned()
                    };
                    let offered: Vec<(String, Option<String>, Option<String>)> =
                        match listed("oneOf").or_else(|| listed("anyOf")) {
                            Some(list) => list
                                .iter()
                                .map(|o| {
                                    let value = o
                                        .get("const")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string();
                                    let title = o
                                        .get("title")
                                        .and_then(Value::as_str)
                                        .filter(|t| !t.is_empty())
                                        .map(str::to_string);
                                    let description = o
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .map(str::to_string);
                                    (value, title, description)
                                })
                                .collect(),
                            None => src
                                .get("enum")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(|v| (v.to_string(), Some(v.to_string()), None))
                                .collect(),
                        };
                    let title = f.get("title").and_then(Value::as_str).map(str::to_string);
                    let text = f
                        .get("description")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                        .map(str::to_string)
                        .or_else(|| title.clone().filter(|t| !t.is_empty()))
                        .unwrap_or_else(|| name.clone());
                    values.insert(
                        name.clone(),
                        offered
                            .iter()
                            .enumerate()
                            .map(|(k, (v, _, _))| (format!("o{k}"), v.clone()))
                            .collect::<Vec<_>>(),
                    );
                    questions.push(Asked {
                        id: name.clone(),
                        header: title,
                        text,
                        allow_text: offered.is_empty() || custom.contains_key(name),
                        options: offered
                            .into_iter()
                            .enumerate()
                            .map(|(k, (value, label, description))| Offered {
                                id: format!("o{k}"),
                                label: label.unwrap_or(value),
                                description,
                            })
                            .collect(),
                        multiple: many,
                        secret: false,
                    });
                }
                let item = match &subject {
                    Some(s) => format!("question:{s}"),
                    None => format!("question:{}:{}", show(self.turn.as_deref()), id_text(id)),
                };
                let text = questions
                    .first()
                    .map_or_else(|| p.message.clone(), |q| q.text.clone());
                let Ok(question) =
                    Question::new(AnswerVia::Request, subject.map(Into::into), questions)
                else {
                    self.unreadable += 1;
                    return;
                };
                self.asked.insert(
                    key,
                    Asked_::Question {
                        item: item.clone(),
                        values,
                        custom,
                    },
                );
                self.emit(
                    out,
                    &item,
                    Body::Question(question),
                    State::Waiting,
                    "Question",
                    &text,
                    Some(native(&format!("{method} (request)"), params)),
                );
            }
            _ => self.unknown.push(format!("{method} (request)")),
        }
    }

    fn response(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        let id = m.id.map(|id| id.get().to_string()).unwrap_or_default();
        let sent = self.sent.remove(&id);
        let method = sent.as_ref().map(|s| s.method.clone());
        let shown = show(method.as_deref());
        if let Some(error) = m.error {
            let reply: ErrorReply = serde_json::from_str(error.get()).unwrap_or(ErrorReply {
                code: None,
                message: None,
            });
            if method.as_deref() == Some("session/load") {
                self.loading = None;
            }
            let owner = sent.map_or((None, None), |s| s.owner);
            let item = format!("error:{}:{}", show(owner.0.as_deref()), id_text(m.id));
            let message = reply.message.unwrap_or_default();
            let kind = format!("error response to {shown}");
            let body = Body::Error(Error {
                request: method.clone(),
                code: reply.code,
                will_retry: None,
            });
            self.emit_owned(
                out,
                owner,
                &item,
                body,
                State::Failed,
                "The agent refused a request",
                &message,
                Some(native(&kind, error.get())),
            );
            if method.as_deref() == Some("session/prompt") && self.turn_request == Some(id) {
                self.end_turn(State::Failed, &message, None, &kind, error.get(), out);
            }
            return;
        }
        let result = m.result.map_or("{}", RawValue::get);
        let kind = format!("response to {shown}");
        match method.as_deref() {
            Some("initialize") => match serde_json::from_str::<InitializeResponse>(result) {
                Ok(r) => {
                    if let Some(info) = r.agent_info {
                        self.version = Some(info.version);
                        if !info.name.is_empty() {
                            self.harness = info.name.to_lowercase().replace(' ', "-");
                        }
                    }
                }
                Err(_) => self.unreadable += 1,
            },
            Some("session/new") => match serde_json::from_str::<NewSessionResponse>(result) {
                Ok(r) => {
                    self.use_session(r.session_id.0.to_string(), out);
                    if let Some(config) = r.config_options {
                        self.config = config;
                    }
                    self.announce(&kind, result, out);
                }
                Err(_) => self.unreadable += 1,
            },
            Some("session/load") => {
                self.loading = None;
                match serde_json::from_str::<LoadSessionResponse>(result) {
                    Ok(r) => self.resumed(
                        sent.and_then(|s| s.session),
                        r.config_options,
                        &kind,
                        result,
                        out,
                    ),
                    Err(_) => self.unreadable += 1,
                }
            }
            Some("session/resume") => match serde_json::from_str::<ResumeSessionResponse>(result) {
                Ok(r) => self.resumed(
                    sent.and_then(|s| s.session),
                    r.config_options,
                    &kind,
                    result,
                    out,
                ),
                Err(_) => self.unreadable += 1,
            },
            Some("session/set_config_option") => {
                match serde_json::from_str::<SetSessionConfigOptionResponse>(result) {
                    Ok(r) => {
                        if !r.config_options.is_empty() {
                            self.config = r.config_options;
                        }
                        self.announce(&kind, result, out);
                    }
                    Err(_) => self.unreadable += 1,
                }
            }
            // The reply to a prompt sent during a turn ends nothing.
            Some("session/prompt") if self.turn_request.as_deref() != Some(id.as_str()) => {}
            Some("session/prompt") => match serde_json::from_str::<PromptResponse>(result) {
                Ok(r) => {
                    let state = match r.stop_reason {
                        StopReason::EndTurn => State::Done,
                        StopReason::Cancelled => State::Cancelled,
                        _ => State::Failed,
                    };
                    let stop = serde_json::to_value(r.stop_reason)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default();
                    let text = if matches!(state, State::Done | State::Cancelled) {
                        String::new()
                    } else {
                        format!("The turn stopped: {stop}")
                    };
                    let usage = r.usage.map(|u| Usage {
                        input_tokens: Some(u.input_tokens),
                        output_tokens: Some(u.output_tokens),
                    });
                    self.end_turn(state, &text, usage, &kind, result, out);
                }
                Err(_) => {
                    // A reply the types cannot read, such as a stop reason they do not know, still
                    // ends the turn: as failed, which is what an unknown stop reason means.
                    self.unreadable += 1;
                    let v: Value = serde_json::from_str(result).unwrap_or(Value::Null);
                    let stop = v
                        .get("stopReason")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let usage = v.get("usage").filter(|u| u.is_object()).map(|u| Usage {
                        input_tokens: u.get("inputTokens").and_then(Value::as_u64),
                        output_tokens: u.get("outputTokens").and_then(Value::as_u64),
                    });
                    let text = format!("The turn stopped: {stop}");
                    self.end_turn(State::Failed, &text, usage, &kind, result, out);
                }
            },
            _ => {}
        }
    }

    fn resumed(
        &mut self,
        session: Option<String>,
        config: Option<Vec<SessionConfigOption>>,
        kind: &str,
        result: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        if let Some(session) = session {
            self.use_session(session, out);
        }
        if let Some(config) = config {
            self.config = config;
        }
        self.announce(kind, result, out);
    }

    fn end_turn(
        &mut self,
        state: State,
        text: &str,
        usage: Option<Usage>,
        kind: &str,
        payload: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        self.close_open_items(out);
        let title = match state {
            State::Done => "Finished",
            State::Cancelled => "Interrupted",
            _ => "Failed",
        };
        let item = format!("turn:{}", show(self.turn.as_deref()));
        let body = Body::Turn(Turn {
            started_by: StartedBy::User,
            duration_ms: None,
            usage,
        });
        self.emit(
            out,
            &item,
            body,
            state,
            title,
            text,
            Some(native(kind, payload)),
        );
        self.forget_turn();
    }

    // -- the session

    /// A chat's session is replaced, or resumed. ACP says nothing when a session is over, so the
    /// driver ends the earlier session's item.
    fn use_session(&mut self, sid: String, out: &mut Vec<AgentEvent>) {
        if let Some(old) = self.session.clone()
            && old != sid
        {
            self.end_session(&old, out);
            self.sessions.remove(&format!("session:{sid}"));
            self.config.clear();
            self.commands.clear();
        }
        self.session = Some(sid);
    }

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

    /// The session item: the agent's version, its configuration options and its commands. Sent
    /// only when it differs from the last one sent.
    fn announce(&mut self, kind: &str, payload: &str, out: &mut Vec<AgentEvent>) {
        let Some(sid) = self.session.clone() else {
            return;
        };
        let settings: Vec<Setting> = self
            .config
            .iter()
            .map(|o| {
                let (value, options) = match &o.kind {
                    SessionConfigKind::Select(select) => {
                        let flat: Vec<String> = match &select.options {
                            SessionConfigSelectOptions::Ungrouped(options) => {
                                options.iter().map(|x| x.value.0.to_string()).collect()
                            }
                            SessionConfigSelectOptions::Grouped(groups) => groups
                                .iter()
                                .flat_map(|g| g.options.iter().map(|x| x.value.0.to_string()))
                                .collect(),
                            _ => Vec::new(),
                        };
                        (
                            Some(select.current_value.0.to_string()),
                            (!flat.is_empty()).then_some(flat),
                        )
                    }
                    _ => (None, None),
                };
                Setting {
                    id: o.id.0.to_string(),
                    label: o.name.clone(),
                    value,
                    options,
                    approve: matches!(o.category, Some(SessionConfigOptionCategory::Mode)),
                }
            })
            .collect();
        let body = Session {
            harness_version: self.version.clone().unwrap_or_default(),
            settings,
            actions: Vec::new(),
            commands: self.commands.clone(),
        };
        let item = format!("session:{sid}");
        if self.sessions.get(&item) == Some(&body) {
            return;
        }
        self.sessions.insert(item.clone(), body.clone());
        let text = body
            .settings
            .iter()
            .filter_map(|s| s.value.as_ref().map(|v| format!("{}: {v}", s.label)))
            .collect::<Vec<_>>()
            .join(", ");
        let turn = self.turn.take();
        self.emit(
            out,
            &item,
            Body::Session(body),
            State::Running,
            "Session",
            &text,
            Some(native(kind, payload)),
        );
        self.turn = turn;
    }

    // -- events

    /// An item still open, by id.
    fn open_event(&self, item: &str) -> Option<AgentEvent> {
        self.open
            .iter()
            .find(|(open, _)| open == item)
            .map(|(_, e)| e.clone())
    }

    /// The turn ended: anything still running will never report again, so close it.
    fn close_open_items(&mut self, out: &mut Vec<AgentEvent>) {
        let open: Vec<(String, AgentEvent)> = self.open.clone();
        for (item, prev) in open {
            if matches!(
                prev.body(),
                Body::Turn(_) | Body::Task(_) | Body::Session(_)
            ) {
                continue;
            }
            let header = prev.header();
            let (title, text) = (header.title.clone(), header.text.clone());
            let owner = (
                header.session.as_ref().map(|s| s.as_str().to_string()),
                header.turn.as_ref().map(|t| t.as_str().to_string()),
            );
            let body = prev.body().clone();
            self.emit_owned(
                out,
                owner,
                &item,
                body,
                State::Cancelled,
                &title,
                &text,
                None,
            );
        }
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
    /// current ones.
    #[allow(clippy::too_many_arguments)]
    fn emit_owned(
        &mut self,
        out: &mut Vec<AgentEvent>,
        (session, turn): Owner,
        item: &str,
        body: Body,
        state: State,
        title: &str,
        text: &str,
        native: Option<Native>,
    ) {
        let seq = self.seq.entry(session.clone()).or_default();
        *seq += 1;
        let header = Header {
            harness: self.harness.clone(),
            session: session.map(Into::into),
            turn: turn.map(Into::into),
            item: item.into(),
            seq: *seq,
            state,
            title: title.into(),
            text: text.into(),
        };
        let Ok(event) = AgentEvent::new(header, body, native) else {
            return;
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

/// What a tool item is called in its state.
fn tool_title(kind: ToolKind, state: State) -> String {
    let (running, done) = match kind {
        ToolKind::Command => ("Running a command", "Ran a command"),
        ToolKind::Edit => ("Editing a file", "Edited a file"),
        ToolKind::Read => ("Reading a file", "Read a file"),
        ToolKind::Search => ("Searching", "Searched"),
        ToolKind::Fetch => ("Fetching", "Fetched"),
        ToolKind::Subagent => ("Starting a subagent", "Started a subagent"),
        ToolKind::Mcp | ToolKind::Other => ("Using a tool", "Used a tool"),
    };
    match state {
        State::Running => running.into(),
        State::Done => done.into(),
        State::Failed => format!("{running}: failed"),
        State::Declined => format!("{running}: declined"),
        State::Cancelled => format!("{running}: cancelled"),
        State::Waiting => format!("{running}: waiting"),
    }
}

/// A file an ACP agent changed, as a unified diff of one hunk with no context: the lines between
/// the longest common start and end of the old text and the new. ACP sends the whole old and new
/// text; this gives the same diff in the prototype and here.
fn changed_lines(old: Option<&str>, new: &str) -> String {
    let a: Vec<&str> = old.map_or_else(Vec::new, |o| o.split('\n').collect());
    let b: Vec<&str> = new.split('\n').collect();
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end = 0;
    while end < a.len() - start
        && end < b.len() - start
        && a[a.len() - 1 - end] == b[b.len() - 1 - end]
    {
        end += 1;
    }
    let gone = &a[start..a.len() - end];
    let added = &b[start..b.len() - end];
    if gone.is_empty() && added.is_empty() {
        return String::new();
    }
    // A hunk that removes or adds nothing names the line before it, as unified diffs do.
    let old_at = if gone.is_empty() { start } else { start + 1 };
    let new_at = if added.is_empty() { start } else { start + 1 };
    let mut diff = format!("@@ -{old_at},{} +{new_at},{} @@\n", gone.len(), added.len());
    for line in gone {
        diff.push_str(&format!("-{line}\n"));
    }
    for line in added {
        diff.push_str(&format!("+{line}\n"));
    }
    diff
}

fn native(kind: &str, payload: &str) -> Native {
    Native {
        kind: kind.to_string(),
        payload: RawValue::from_string(payload.to_string())
            .unwrap_or_else(|_| RawValue::from_string("{}".into()).expect("valid JSON")),
    }
}

/// A value the agent left out, written as the prototype writes it.
fn show(value: Option<&str>) -> String {
    value.unwrap_or("None").to_string()
}

/// A JSON-RPC id as a name: a string as it is, a number as it is written.
fn id_text(id: Option<&RawValue>) -> String {
    match id.map(|id| serde_json::from_str::<Value>(id.get())) {
        Some(Ok(Value::String(s))) => s,
        Some(_) => id.map_or_else(|| "None".into(), |id| id.get().to_string()),
        None => "None".into(),
    }
}
