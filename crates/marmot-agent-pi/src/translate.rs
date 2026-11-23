//! What Pi says, turned into agent events.
//!
//! The translator reads every record on one Pi's standard input and output, in the order they
//! went, in both directions: what the driver sent says what an extension's question was answered
//! with, and that the running turn is being aborted. It does no input or output
//! of its own, so it is tested by replaying the recordings (`fixtures/recordings/pi`), and must
//! produce the events the prototype produces for each (`fixtures/recordings/pi/expected`).
//!
//! The native message of an event is the whole record, as the exact text that arrived. An event
//! the driver makes up, such as a tool it closes because Pi never did, carries none. Nothing is
//! cut to fit a size: the core applies the limit (D-057).
//!
//! Pi has no turn id. A turn here is a run, from `agent_start` to `agent_settled`, which covers
//! Pi's own retries and any message steered into it. It is named after the first message the run
//! reports, normally the person's, by the session and that message's timestamp, so a resumed
//! session never reuses the id of a turn from before Pi restarted.

use marmot_agent_driver::{Answer, Given, Reason, Refusal};
use marmot_agent_events::body::{
    Action, AnswerVia, Answered, Asked, Change, ChangeKind, Command, Error, Message, Notice,
    NoticeKind, Offered, Question, Reasoning, Session, Setting, StartedBy, Tool, ToolKind, Turn,
    Usage,
};
use marmot_agent_events::{AgentEvent, Body, Header, Native, State};
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::HashMap;

/// Which way a record went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From Pi to the driver.
    In,
    /// From the driver to Pi.
    Out,
}

/// Records Pi sends that never become an event of their own, and why (rule 11 of the event
/// model). Some still change what the driver knows.
pub const NOT_SENT: &[(&str, &str)] = &[
    ("turn_start", "one model call started"),
    (
        "turn_end",
        "one model call ended; its message was already sent",
    ),
    (
        "agent_end",
        "a low-level run ended; agent_settled ends the turn",
    ),
    ("queue_update", "the queue of steered messages changed"),
    ("entry_appended", "an entry was added to the session file"),
    ("message_update", "partial text and tool arguments"),
    ("tool_execution_update", "partial tool output"),
    ("session_info_changed", "the session's display name changed"),
    ("auto_retry_end", "a retry ended; the turn says how"),
    (
        "bash_execution_update",
        "output of a command the client ran itself",
    ),
];

/// The extension UI requests that wait for an answer.
pub(crate) const DIALOGS: &[&str] = &["select", "confirm", "input", "editor"];
/// Extension UI requests that only decorate a terminal.
const DECORATIONS: &[&str] = &["setStatus", "setWidget", "setTitle", "set_editor_text"];

/// An extension's question that waits for its answer: how it asked, and the labels it offered.
struct Dialog {
    method: String,
    labels: Vec<String>,
}

/// Turns the records of one Pi, serving one chat, into agent events.
#[derive(Default)]
pub struct Translator {
    /// Every event made so far.
    made: u64,
    /// Events made in each session, for each event's `seq`.
    seq: HashMap<Option<String>, u64>,
    session: Option<String>,
    turn: Option<String>,
    /// Items not yet finished, in the order they started, with their last event.
    open: Vec<(String, AgentEvent)>,
    /// Each session item's body as it was last sent.
    sessions: HashMap<String, Value>,
    /// Pi's version, which the driver asks `pi --version` for, since RPC does not say it.
    version: Option<String>,
    /// Pi started a run for a prompt since the last turn started.
    prompted: bool,
    /// A run has started and its turn is not yet named.
    starting: bool,
    /// The client asked Pi to abort the running turn.
    aborting: bool,
    started_by: Option<StartedBy>,
    /// The last model call's error, until a call succeeds.
    failure: Option<String>,
    /// The tokens the open turn used: input, then output.
    usage: (u64, u64),
    model: Option<String>,
    thinking: Option<String>,
    models: Option<Vec<String>>,
    levels: Option<Vec<String>>,
    commands: Option<Vec<Command>>,
    /// The driver has asked for everything a session item shows.
    ready: bool,
    /// Extension questions waiting for an answer, by the request's id.
    dialogs: HashMap<String, Dialog>,
    /// Every extension question asked, by the request's id, and whether it was answered.
    asked: HashMap<String, bool>,
    compactions: u64,
    /// Records that could not be read as JSON.
    unreadable: u64,
    /// Record types with no rule. Each is a gap in the driver.
    unknown: Vec<String>,
}

impl Translator {
    /// A translator for a new Pi.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pi's version, from `pi --version`, which every session item carries (D-030, D-044).
    pub fn set_version(&mut self, version: Option<String>) {
        self.version = version;
    }

    /// Record types Pi sent that the driver has no rule for, in the order they came.
    pub fn unknown(&self) -> &[String] {
        &self.unknown
    }

    /// How many records could not be read as JSON.
    pub fn unreadable(&self) -> u64 {
        self.unreadable
    }

    /// The session, once Pi has named it.
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// The running turn, once it is named.
    pub fn turn(&self) -> Option<&str> {
        self.turn.as_deref()
    }

    /// Whether a run is going on, named or not.
    pub fn running(&self) -> bool {
        self.turn.is_some() || self.starting
    }

    /// Reads one record, which went `direction`, and gives the events it makes.
    pub fn feed(&mut self, direction: Direction, record: &str) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let Ok(m) = serde_json::from_str::<Value>(record) else {
            self.unreadable += 1;
            return out;
        };
        let t = m["type"].as_str().unwrap_or_default().to_string();
        match direction {
            Direction::Out => self.client_sent(&t, &m, record, &mut out),
            Direction::In if t == "response" => self.response(&m, record, &mut out),
            Direction::In => self.pi_sent(&t, &m, record, &mut out),
        }
        out
    }

    /// Pi ended. Nothing it was doing will report again: the driver closes the open items and the
    /// turn. The session stays the chat's, to be resumed.
    pub fn harness_stopped(&mut self) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        self.close_open_items(&mut out);
        if let Some(turn) = self.turn.clone() {
            let item = format!("turn:{turn}");
            if let Some(prev) = self.open_event(&item) {
                let text = "The harness stopped before the turn finished.";
                self.emit(
                    &mut out,
                    &item,
                    prev.body().clone(),
                    State::Cancelled,
                    "Interrupted",
                    text,
                    None,
                );
            }
        }
        self.turn = None;
        self.starting = false;
        self.prompted = false;
        self.aborting = false;
        self.dialogs.clear();
        out
    }

    // -- what the driver sent

    fn client_sent(&mut self, t: &str, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        match t {
            "extension_ui_response" => self.answered(m, record, out),
            "abort" if self.running() => self.aborting = true,
            _ => {}
        }
    }

    fn response(&mut self, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        let id = m["id"].as_str().unwrap_or_default();
        let command = m["command"].as_str().unwrap_or_default().to_string();
        let data = &m["data"];
        let kind = format!("response to {command}");
        if m["success"].as_bool() != Some(true) {
            let compacted =
                self.compactions > 0 && self.open_event(&self.compaction_item()).is_none();
            if command == "compact" && compacted {
                return; // the compaction notice already says why
            }
            // Pi refused a command. The turn is not affected; the chat is told.
            let body = Body::Error(Error {
                request: Some(command.clone()),
                ..Error::default()
            });
            let text = m["error"].as_str().unwrap_or_default();
            let item = format!("refused:{id}");
            self.emit(
                out,
                &item,
                body,
                State::Failed,
                "Pi refused a request",
                text,
                native(&kind, record),
            );
            return;
        }
        match command.as_str() {
            "get_state" => {
                let sid = data["sessionId"].as_str().map(String::from);
                if let Some(old) = self.session.clone()
                    && sid.as_deref() != Some(old.as_str())
                {
                    self.end_session(&old, out);
                    self.ready = false;
                }
                self.session = sid;
                let model = &data["model"];
                self.model = model.is_object().then(|| model_name(model));
                self.thinking = data["thinkingLevel"].as_str().map(String::from);
            }
            "get_available_models" => {
                let models = data["models"].as_array().cloned().unwrap_or_default();
                self.models = Some(models.iter().map(model_name).collect());
            }
            "get_available_thinking_levels" => {
                let levels = data["levels"].as_array().cloned().unwrap_or_default();
                self.levels = Some(levels.iter().map(text_of).collect());
            }
            "get_commands" => {
                let commands = data["commands"].as_array().cloned().unwrap_or_default();
                let commands = commands.iter().map(|c| Command {
                    name: text_of(&c["name"]),
                    description: c["description"].as_str().map(String::from),
                    hint: String::new(),
                });
                self.commands = Some(commands.collect());
                self.ready = true;
            }
            "set_model" => {
                if data["provider"].as_str().is_some() {
                    self.model = Some(model_name(data));
                }
            }
            // Pi says the level it set, which can differ from the one asked for, in
            // `thinking_level_changed`, before it responds; and nothing if the level is unchanged.
            "set_thinking_level" => {}
            "new_session" if data["cancelled"].as_bool() == Some(true) => {
                // An extension kept the session. Nothing changed; the chat is told.
                let body = Body::Error(Error {
                    request: Some(command.clone()),
                    ..Error::default()
                });
                let item = format!("refused:{id}");
                let text = "An extension cancelled the new session.";
                self.emit(
                    out,
                    &item,
                    body,
                    State::Failed,
                    "Pi refused a request",
                    text,
                    native(&kind, record),
                );
                return;
            }
            "new_session" => {
                if let Some(old) = self.session.take() {
                    self.end_session(&old, out);
                    self.ready = false;
                }
                return;
            }
            // A run Pi started for a prompt: its turn was started by the person. Pi says so
            // before the run reports anything, and only after the run before it has ended.
            "prompt" => {
                if data["disposition"] == "started" {
                    self.prompted = true;
                }
                return;
            }
            _ => return,
        }
        self.announce(&kind, record, out);
    }

    /// An edit of the session item as it is now, even if nothing in it changed: it shows the
    /// effect of a setting changed to the value it had. None before the item has been sent.
    pub fn show_session(&mut self, kind: &str, record: &str) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        if let Some(sid) = &self.session {
            self.sessions.remove(&format!("session:{sid}"));
            self.announce(kind, record, &mut out);
        }
        out
    }

    /// An error the driver makes when Pi stopped before it acted on what it was sent: a control
    /// still waiting for its effect, or a prompt it had not taken. It is no record of Pi's, so it
    /// carries none.
    pub fn stopped_before_acting(&mut self) -> AgentEvent {
        let mut out = Vec::new();
        let item = format!("stopped:{}", self.made + 1);
        let text = "Pi stopped before it carried out a request.";
        let body = Body::Error(Error::default());
        self.emit(
            &mut out,
            &item,
            body,
            State::Failed,
            "Pi stopped",
            text,
            None,
        );
        out.pop().expect("an error repeats no secret")
    }

    /// Sends the session item, or an edit of it, once the driver has asked for everything it
    /// shows, and only when something in it changed.
    fn announce(&mut self, kind: &str, record: &str, out: &mut Vec<AgentEvent>) {
        let Some(sid) = self.session.clone() else {
            return;
        };
        if !self.ready {
            return;
        }
        let session = Session {
            harness_version: self.version.clone().unwrap_or_default(),
            settings: vec![
                setting("model", "Model", self.model.clone(), self.models.clone()),
                setting(
                    "thinking",
                    "Thinking",
                    self.thinking.clone(),
                    self.levels.clone(),
                ),
            ],
            actions: vec![Action {
                id: "compact".into(),
                label: "Compact".into(),
                description: Some("Summarise the conversation so far, to free up context".into()),
            }],
            commands: self.commands.clone().unwrap_or_default(),
        };
        let item = format!("session:{sid}");
        let shown = serde_json::to_value(&session).unwrap_or(Value::Null);
        if self.sessions.get(&item) == Some(&shown) {
            return;
        }
        self.sessions.insert(item.clone(), shown);
        let text = (session.settings.iter())
            .filter_map(|s| s.value.as_ref().map(|v| format!("{}: {v}", s.label)))
            .collect::<Vec<_>>()
            .join(", ");
        let turn = self.turn.take(); // a session belongs to no turn
        self.emit(
            out,
            &item,
            Body::Session(session),
            State::Running,
            "Session",
            &text,
            native(kind, record),
        );
        self.turn = turn;
    }

    /// Nothing Pi sends says a session is over. The driver ends the item itself, when the chat's
    /// session is replaced.
    fn end_session(&mut self, sid: &str, out: &mut Vec<AgentEvent>) {
        let item = format!("session:{sid}");
        let Some(prev) = self.open_event(&item) else {
            return;
        };
        let turn = self.turn.take();
        let session = self.session.replace(sid.to_string());
        let (body, text) = (prev.body().clone(), prev.header().text.clone());
        self.emit(out, &item, body, State::Done, "Session ended", &text, None);
        self.session = session;
        self.turn = turn;
    }

    // -- what Pi sent

    fn pi_sent(&mut self, t: &str, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        if NOT_SENT.iter().any(|(kind, _)| *kind == t) {
            return;
        }
        match t {
            "agent_start" => {
                if self.turn.is_none() {
                    self.starting = true; // named by the first message the run reports
                }
            }
            "agent_settled" => self.agent_settled(m, record, out),
            "message_start" => {
                let message = &m["message"];
                if self.starting && message["role"] == "user" {
                    self.open_turn(t, &message["timestamp"], record, out);
                }
            }
            "message_end" => self.message_end(m, record, out),
            "tool_execution_start" => self.tool_start(m, record, out),
            "tool_execution_end" => self.tool_end(m, record, out),
            "auto_retry_start" => {
                self.ensure_turn(m, record, out);
                let turn = self.turn.clone().unwrap_or_default();
                let item = format!("retry:{turn}:{}", m["attempt"]);
                let text = m["errorMessage"].as_str().unwrap_or_default();
                let body = Body::Notice(notice(NoticeKind::Retry));
                self.emit(
                    out,
                    &item,
                    body,
                    State::Done,
                    "Retrying",
                    text,
                    native(t, record),
                );
            }
            "compaction_start" => {
                self.compactions += 1;
                let item = self.compaction_item();
                let body = Body::Notice(notice(NoticeKind::Compaction));
                let title = "Compacting the conversation";
                self.emit(
                    out,
                    &item,
                    body,
                    State::Running,
                    title,
                    "",
                    native(t, record),
                );
            }
            "compaction_end" => {
                let item = self.compaction_item();
                let error = m["errorMessage"].as_str();
                let (state, title) = if m["aborted"].as_bool() == Some(true) {
                    (State::Cancelled, "Compaction stopped")
                } else if error.is_some_and(|e| !e.is_empty()) {
                    (State::Failed, "Compaction failed")
                } else {
                    (State::Done, "Conversation compacted")
                };
                let body = Body::Notice(notice(NoticeKind::Compaction));
                self.emit(
                    out,
                    &item,
                    body,
                    state,
                    title,
                    error.unwrap_or_default(),
                    native(t, record),
                );
            }
            "thinking_level_changed" => {
                self.thinking = m["level"].as_str().map(String::from);
                self.announce(t, record, out);
            }
            "extension_ui_request" => self.extension_ui(m, record, out),
            "extension_error" => {
                let item = format!("extension-error:{}", self.made + 1);
                let body = Body::Error(Error {
                    request: m["event"].as_str().map(String::from),
                    ..Error::default()
                });
                let text = m["error"].as_str().unwrap_or_default();
                self.emit(
                    out,
                    &item,
                    body,
                    State::Failed,
                    "An extension failed",
                    text,
                    native(t, record),
                );
            }
            other => self.unknown.push(other.to_string()),
        }
    }

    fn compaction_item(&self) -> String {
        format!(
            "compaction:{}:{}",
            show(self.session.as_deref()),
            self.compactions
        )
    }

    // -- turns

    fn open_turn(
        &mut self,
        kind: &str,
        timestamp: &Value,
        record: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        self.starting = false;
        let stamp = match timestamp {
            Value::Null => format!("seq{}", self.made + 1),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        self.turn = Some(format!("{}:{stamp}", show(self.session.as_deref())));
        let started_by = if self.prompted {
            StartedBy::User
        } else {
            StartedBy::Agent
        };
        self.started_by = Some(started_by);
        self.prompted = false;
        self.failure = None;
        self.usage = (0, 0);
        let item = format!("turn:{}", self.turn.as_deref().unwrap_or_default());
        let body = Body::Turn(Turn {
            started_by,
            duration_ms: None,
            usage: None,
        });
        self.emit(
            out,
            &item,
            body,
            State::Running,
            "Working",
            "",
            native(kind, record),
        );
    }

    /// Opens the turn with whatever record comes first, when the run did not start with a
    /// message from the person.
    fn ensure_turn(&mut self, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        if self.turn.is_none() {
            let kind = m["type"].as_str().unwrap_or_default().to_string();
            self.open_turn(&kind, &m["message"]["timestamp"], record, out);
        }
    }

    fn agent_settled(&mut self, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        let Some(turn) = self.turn.clone() else {
            self.starting = false; // a run that reported nothing
            return;
        };
        self.close_open_items(out);
        let (state, title) = if m["aborted"].as_bool() == Some(true) {
            (State::Cancelled, "Interrupted")
        } else if self.failure.is_some() {
            (State::Failed, "Failed")
        } else {
            (State::Done, "Finished")
        };
        let text = match state {
            State::Failed => self.failure.clone().unwrap_or_default(),
            _ => String::new(),
        };
        let body = Body::Turn(Turn {
            started_by: self.started_by.unwrap_or(StartedBy::User),
            duration_ms: None,
            usage: Some(Usage {
                input_tokens: Some(self.usage.0),
                output_tokens: Some(self.usage.1),
            }),
        });
        let item = format!("turn:{turn}");
        self.emit(
            out,
            &item,
            body,
            state,
            title,
            &text,
            native("agent_settled", record),
        );
        self.turn = None;
        self.aborting = false;
    }

    // -- messages

    fn message_end(&mut self, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        let message = &m["message"];
        if message["role"] != "assistant" {
            return; // the person's message echoed, the system prompt, or a tool's result
        }
        self.ensure_turn(m, record, out);
        let usage = &message["usage"];
        self.usage.0 += usage["input"].as_u64().unwrap_or(0);
        self.usage.1 += usage["output"].as_u64().unwrap_or(0);
        self.failure = (message["stopReason"] == "error").then(|| {
            let error = message["errorMessage"].as_str().unwrap_or_default();
            if error.is_empty() {
                "The model call failed.".to_string()
            } else {
                error.to_string()
            }
        });
        let stamp = stamp(&message["timestamp"]);
        let blocks = message["content"].as_array().cloned().unwrap_or_default();
        for (n, block) in blocks.iter().enumerate() {
            let item = format!("{stamp}:{n}");
            match block["type"].as_str().unwrap_or_default() {
                "text" => {
                    let text = block["text"].as_str().unwrap_or_default();
                    if !text.trim().is_empty() {
                        let body = Body::Message(Message::default());
                        self.emit(
                            out,
                            &item,
                            body,
                            State::Done,
                            "Message",
                            text,
                            native("message_end", record),
                        );
                    }
                }
                "thinking" => {
                    let text = block["thinking"].as_str().unwrap_or_default();
                    if !text.trim().is_empty() {
                        let body = Body::Reasoning(Reasoning {});
                        self.emit(
                            out,
                            &item,
                            body,
                            State::Done,
                            "Thinking",
                            text,
                            native("message_end", record),
                        );
                    }
                }
                "toolCall" => {} // sent when it runs (tool_execution_start)
                other => self.unknown.push(format!("message_end block [{other}]")),
            }
        }
    }

    // -- tools

    fn tool_start(&mut self, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        self.ensure_turn(m, record, out);
        let name = m["toolName"].as_str().unwrap_or_default();
        let args = &m["args"];
        let kind = match name {
            "bash" => ToolKind::Command,
            "edit" | "write" => ToolKind::Edit,
            "read" => ToolKind::Read,
            "grep" | "find" | "ls" => ToolKind::Search,
            _ => ToolKind::Other,
        };
        let mut body = tool(kind, name);
        match kind {
            ToolKind::Command => body.command = given(&args["command"]),
            ToolKind::Edit | ToolKind::Read => body.path = given(&args["path"]),
            ToolKind::Search => body.query = given(&args["pattern"]).or(given(&args["path"])),
            _ => body.input = Some(args.clone()),
        }
        let text = [&body.command, &body.path, &body.query]
            .into_iter()
            .find_map(|v| v.clone().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| name.to_string());
        let item = m["toolCallId"].as_str().unwrap_or_default().to_string();
        let title = tool_title(kind, State::Running);
        self.emit(
            out,
            &item,
            Body::Tool(body),
            State::Running,
            &title,
            &text,
            native("tool_execution_start", record),
        );
    }

    fn tool_end(&mut self, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        let item = m["toolCallId"].as_str().unwrap_or_default().to_string();
        let Some(prev) = self.open_event(&item) else {
            return; // a tool that never started
        };
        let Body::Tool(mut body) = prev.body().clone() else {
            return;
        };
        let result = &m["result"];
        let parts = result["content"].as_array().cloned().unwrap_or_default();
        let output: String = (parts.iter())
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
            .collect();
        // A tool Pi stops because the client aborted the turn ends as an error: it was cancelled.
        let state = match (m["isError"].as_bool() == Some(true), self.aborting) {
            (false, _) => State::Done,
            (true, true) => State::Cancelled,
            (true, false) => State::Failed,
        };
        if let Some(code) = result["structuredContent"]["exit_code"].as_i64() {
            body.exit_code = Some(code);
        }
        if let Some(ms) = m["durationMs"].as_u64() {
            body.duration_ms = Some(ms);
        }
        if let Some(patch) = result["details"]["patch"].as_str()
            && body.kind == ToolKind::Edit
            && !patch.is_empty()
            && state == State::Done
        {
            let change = if m["toolName"] == "write" {
                ChangeKind::Add
            } else {
                ChangeKind::Update
            };
            body.changes = vec![Change {
                path: body.path.clone().unwrap_or_default(),
                diff: patch.to_string(),
                change: Some(change),
            }];
        }
        if state != State::Done && body.kind != ToolKind::Command {
            body.error = Some(output); // a command's output and exit code say it themselves
        } else {
            body.output = Some(output);
        }
        let title = tool_title(body.kind, state);
        let text = prev.header().text.clone();
        self.emit(
            out,
            &item,
            Body::Tool(body),
            state,
            &title,
            &text,
            native("tool_execution_end", record),
        );
    }

    // -- what an extension asks

    fn extension_ui(&mut self, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        let method = m["method"].as_str().unwrap_or_default();
        let id = m["id"].as_str().unwrap_or_default().to_string();
        let kind = format!("extension_ui_request ({method})");
        if DECORATIONS.contains(&method) {
            return;
        }
        if method == "notify" {
            // What a command an extension ran has to say, or a warning.
            let info = m["notifyType"].as_str().is_none_or(|t| t == "info");
            let about = if info {
                NoticeKind::CommandOutput
            } else {
                NoticeKind::Warning
            };
            let text = m["message"].as_str().unwrap_or_default();
            let body = Body::Notice(notice(about));
            self.emit(
                out,
                &format!("notify:{id}"),
                body,
                State::Done,
                "Notice",
                text,
                native(&kind, record),
            );
            return;
        }
        if !DIALOGS.contains(&method) {
            self.unknown.push(kind);
            return;
        }
        let labels: Vec<String> = match method {
            "select" => (m["options"].as_array().cloned().unwrap_or_default())
                .iter()
                .map(|o| o.as_str().map_or_else(|| o.to_string(), String::from))
                .collect(),
            "confirm" => vec!["Yes".into(), "No".into()],
            _ => Vec::new(),
        };
        let options = (labels.iter().enumerate())
            .map(|(n, label)| Offered {
                id: format!("o{n}"),
                label: label.clone(),
                description: None,
            })
            .collect();
        let title = m["title"].as_str().unwrap_or_default();
        let text = match m["message"].as_str() {
            Some(message) if !message.is_empty() => format!("{title}\n\n{message}"),
            _ => title.to_string(),
        };
        let asked = Asked {
            id: "q0".into(),
            header: None,
            text: text.clone(),
            options,
            multiple: false,
            allow_text: matches!(method, "input" | "editor"),
            secret: false,
        };
        let Ok(question) = Question::new(AnswerVia::Request, None, vec![asked]) else {
            return;
        };
        self.asked.insert(id.clone(), false);
        self.dialogs.insert(
            id.clone(),
            Dialog {
                method: method.to_string(),
                labels,
            },
        );
        let item = format!("question:{id}");
        self.emit(
            out,
            &item,
            Body::Question(question),
            State::Waiting,
            "Question",
            &text,
            native(&kind, record),
        );
    }

    fn answered(&mut self, m: &Value, record: &str, out: &mut Vec<AgentEvent>) {
        let id = m["id"].as_str().unwrap_or_default();
        let item = format!("question:{id}");
        let (Some(dialog), Some(prev)) = (self.dialogs.remove(id), self.open_event(&item)) else {
            return;
        };
        let Body::Question(question) = prev.body().clone() else {
            return;
        };
        let title = prev.header().title.clone();
        let kind = "extension_ui_response";
        let cancelled = m["cancelled"].as_bool() == Some(true);
        self.asked.insert(id.to_string(), !cancelled);
        if cancelled {
            let body = Body::Question(question);
            self.emit(
                out,
                &item,
                body,
                State::Cancelled,
                &title,
                "Not answered",
                native(kind, record),
            );
            return;
        }
        let chosen = match m.get("confirmed") {
            Some(confirmed) => Some(
                if confirmed.as_bool() == Some(true) {
                    "Yes"
                } else {
                    "No"
                }
                .to_string(),
            ),
            None => m["value"].as_str().map(String::from),
        };
        let mut answer = Answered {
            question: "q0".into(),
            options: Vec::new(),
            text: None,
        };
        match chosen
            .as_ref()
            .and_then(|c| dialog.labels.iter().position(|l| l == c))
        {
            Some(at) => answer.options = vec![format!("o{at}")],
            None => answer.text = chosen,
        }
        let Ok(question) = question.answered(vec![answer]) else {
            return;
        };
        self.emit(
            out,
            &item,
            Body::Question(question),
            State::Done,
            &title,
            "Answered",
            native(kind, record),
        );
    }

    // -- events

    fn close_open_items(&mut self, out: &mut Vec<AgentEvent>) {
        let open: Vec<(String, AgentEvent)> = self.open.clone();
        for (item, prev) in open {
            if matches!(
                prev.body(),
                Body::Turn(_) | Body::Task(_) | Body::Session(_)
            ) {
                continue; // sessions outlive the turn; the turn closes itself
            }
            // Pi resolves a dialog itself when its run ends (an abort, or the dialog's timeout)
            // and drops a reply that comes later, so a question closed here takes no answer.
            if let Some(id) = item.strip_prefix("question:") {
                self.dialogs.remove(id);
            }
            let header = prev.header();
            let (title, text) = (header.title.clone(), header.text.clone());
            let owner = (
                header.session.as_ref().map(|s| s.as_str().to_string()),
                header.turn.as_ref().map(|t| t.as_str().to_string()),
            );
            self.emit_owned(
                out,
                owner,
                &item,
                prev.body().clone(),
                State::Cancelled,
                &title,
                &text,
                None,
            );
        }
    }

    fn open_event(&self, item: &str) -> Option<AgentEvent> {
        (self.open.iter())
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
        let seq = self.seq.entry(session.clone()).or_default();
        *seq += 1;
        let header = Header {
            harness: "pi".into(),
            session: session.map(Into::into),
            turn: turn.map(Into::into),
            item: item.into(),
            seq: *seq,
            state,
            title: title.into(),
            text: text.into(),
        };
        // No question Pi asks is secret, so every event can carry its native message.
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

// -- what the driver sends

impl Translator {
    /// The reply to an extension's question that is waiting: the option chosen, or the text
    /// typed. Pi's extensions take a label or text, so the reply carries the label of the option
    /// the person chose.
    pub fn reply(&self, answer: &Answer) -> Result<String, Refusal> {
        let Some(id) = answer.item.as_str().strip_prefix("question:") else {
            return Err(Refusal::new(Reason::UnknownItem));
        };
        let Some(dialog) = self.dialogs.get(id) else {
            return Err(Refusal::new(match self.asked.get(id) {
                Some(true) => Reason::AlreadyAnswered,
                Some(false) => Reason::NotWaiting,
                None => Reason::UnknownItem,
            }));
        };
        let Given::Questions(replies) = &answer.given else {
            return Err(Refusal::new(Reason::Invalid));
        };
        let [reply] = replies.as_slice() else {
            return Err(Refusal::new(Reason::Invalid));
        };
        if reply.question != "q0" {
            return Err(Refusal::new(Reason::Invalid));
        }
        let label = match (reply.options.as_slice(), &reply.text) {
            ([option], None) => {
                let at = option
                    .strip_prefix('o')
                    .and_then(|n| n.parse::<usize>().ok());
                match at.and_then(|at| dialog.labels.get(at)) {
                    Some(label) => label.clone(),
                    None => return Err(Refusal::new(Reason::Invalid)),
                }
            }
            ([], Some(text)) if matches!(dialog.method.as_str(), "input" | "editor") => {
                text.clone()
            }
            _ => return Err(Refusal::new(Reason::Invalid)),
        };
        let reply = match dialog.method.as_str() {
            "confirm" => {
                serde_json::json!({"type": "extension_ui_response", "id": id, "confirmed": label == "Yes"})
            }
            _ => serde_json::json!({"type": "extension_ui_response", "id": id, "value": label}),
        };
        Ok(reply.to_string())
    }
}

fn native(kind: &str, record: &str) -> Option<Native> {
    let payload = RawValue::from_string(record.to_string()).ok()?;
    Some(Native {
        kind: kind.to_string(),
        payload,
    })
}

/// A model as a setting's value: its provider and its id.
fn model_name(model: &Value) -> String {
    format!("{}/{}", text_of(&model["provider"]), text_of(&model["id"]))
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

fn stamp(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

/// A string that is there and not empty, as the prototype's `or` reads one.
fn given(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(String::from)
}

fn show(value: Option<&str>) -> String {
    value.unwrap_or("None").to_string()
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

fn setting(id: &str, label: &str, value: Option<String>, options: Option<Vec<String>>) -> Setting {
    Setting {
        id: id.to_string(),
        label: label.to_string(),
        value,
        options,
        approve: false,
    }
}

/// What a tool item's title says, by its kind and its state, as the prototype writes it.
fn tool_title(kind: ToolKind, state: State) -> String {
    let (running, done) = match kind {
        ToolKind::Command => ("Running a command", "Ran a command"),
        ToolKind::Edit => ("Editing a file", "Edited a file"),
        ToolKind::Read => ("Reading a file", "Read a file"),
        ToolKind::Search => ("Searching", "Searched"),
        ToolKind::Fetch => ("Fetching", "Fetched"),
        ToolKind::Mcp | ToolKind::Other => ("Using a tool", "Used a tool"),
        ToolKind::Subagent => ("Starting a subagent", "Started a subagent"),
    };
    match state {
        State::Running => running.to_string(),
        State::Done => done.to_string(),
        other => format!("{running}: {}", state_word(other)),
    }
}

fn state_word(state: State) -> &'static str {
    match state {
        State::Running => "running",
        State::Waiting => "waiting",
        State::Done => "done",
        State::Failed => "failed",
        State::Declined => "declined",
        State::Cancelled => "cancelled",
    }
}
