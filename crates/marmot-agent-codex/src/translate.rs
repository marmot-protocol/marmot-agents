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
    QuestionReply, Rerouted, Sandbox, ThreadSettingsUpdated, ThreadStarted, TokenUsageUpdated,
    Tokens, TurnNote, UserInput,
};
use marmot_agent_events::body::{
    Action, AnswerVia, Answered, Approval, ApprovalAnswer, Asked, Change, ChangeKind, Choice,
    Effect, Error, Message, Notice, NoticeKind, Offered, Phase, Plan, PlanEntry, PlanEntryState,
    Question, Reasoning, Session, Setting, StartedBy, Tool, ToolKind, Turn, Usage,
};
use marmot_agent_events::{AgentEvent, Body, Header, Native, State};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::HashMap;

/// Which way a message went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From Codex to the driver.
    In,
    /// From the driver to Codex.
    Out,
}

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

/// Turns one Codex connection's messages into agent events.
#[derive(Default)]
pub struct Translator {
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
    requests: HashMap<Id, String>,
    /// Codex's requests that the client has not answered, by id.
    asked: HashMap<Id, Pending>,
    /// For each turn, the thread's tokens before its first model call, and the thread's total
    /// since. `last` in Codex's notice is one model call; a turn used the increase in the total.
    usage: HashMap<Option<String>, (Tokens, Tokens)>,
    /// The client's requests that will start a turn, oldest first, and who they count as.
    starts: Vec<(Id, StartedBy)>,
    started_by: Option<StartedBy>,
    /// The thread the message being translated names, if it names one.
    message_thread: Option<String>,
    known: Known,
    models: Option<Vec<String>>,
    modes: Option<Vec<String>>,
    /// Messages that could not be read as JSON-RPC at all.
    unreadable: u64,
    /// Methods with no rule. Each is a gap in the driver.
    unknown: Vec<String>,
}

impl Translator {
    /// A translator for a new connection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Methods Codex sent that the driver has no rule for, in the order they came. A newer Codex
    /// can add one; it is then the driver's to decide.
    pub fn unknown(&self) -> &[String] {
        &self.unknown
    }

    /// How many messages could not be read as JSON-RPC.
    pub fn unreadable(&self) -> u64 {
        self.unreadable
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
        let turn_item = format!("turn:{}", show(self.turn.as_deref()));
        if let Some(prev) = self.open_event(&turn_item) {
            let body = prev.body().clone();
            self.emit(
                &mut out,
                &turn_item,
                body,
                State::Cancelled,
                "Interrupted",
                "The harness stopped before the turn finished.",
                None,
            );
        }
        self.requests.clear();
        self.asked.clear();
        self.starts.clear();
        self.usage.clear();
        self.turn = None;
        out
    }

    fn client_sent(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        if let Some(method) = &m.method {
            if matches!(method.as_str(), "turn/start" | "turn/steer") {
                self.close_message_questions(State::Done, "Answered by the next message", out);
            }
            if let Some(id) = &m.id {
                self.requests.insert(id.clone(), method.clone());
                match method.as_str() {
                    "turn/start" => self.starts.push((id.clone(), StartedBy::User)),
                    "thread/compact/start" => self.starts.push((id.clone(), StartedBy::Control)),
                    _ => {}
                }
            }
            return;
        }
        self.answer(m, out);
    }

    fn codex_sent(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
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
                    let thread = p.thread.unwrap_or_default();
                    self.use_thread(thread.id.clone(), out);
                    self.thread_settings(&thread, method, params, out);
                }
                "thread/settings/updated" => {
                    let Some(p) = self.read::<ThreadSettingsUpdated>(method, params) else {
                        return;
                    };
                    let settings = p.thread_settings.unwrap_or_default();
                    self.thread_settings(&settings, method, params, out);
                }
                _ => {}
            }
            return;
        }
        #[derive(Deserialize)]
        struct Named {
            #[serde(default, rename = "threadId")]
            thread_id: Option<String>,
        }
        self.message_thread = serde_json::from_str::<Named>(params)
            .ok()
            .and_then(|n| n.thread_id);
        self.handle(method, m, params, out);
        self.message_thread = None;
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
            "turn/plan/updated" => self.plan(method, params, out),
            "thread/compacted" => {
                let item = format!("compaction:{}", self.made);
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
                let item = format!("model:{}", self.made);
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
                let item = format!("warning:{}", self.made);
                let body = Body::Notice(notice(NoticeKind::Warning));
                let text = p.message.unwrap_or_default();
                let native = native(method, params);
                self.emit(out, &item, body, State::Done, "Warning", &text, native);
            }
            "error" => {
                let Some(p) = self.read::<ErrorNote>(method, params) else {
                    return;
                };
                let item = format!("error:{}", self.made);
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
        self.turn = p.turn.and_then(|t| t.id);
        // The oldest request that starts a turn started this one. With none, Codex started it.
        let started_by = if self.starts.is_empty() {
            StartedBy::Agent
        } else {
            self.starts.remove(0).1
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
        // A question answered by the person's next message still waits after a turn that
        // completed. Anything else open will never report again.
        let completed = turn.status.as_deref() == Some("completed");
        self.close_open_items(out, completed);
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
        self.turn = None; // nothing that follows belongs to this turn
    }

    // -- items

    fn item(&mut self, method: &str, params: &str, out: &mut Vec<AgentEvent>) {
        let Some(note) = self.read::<ItemNote<'_>>(method, params) else {
            return;
        };
        let it = note.item.unwrap_or_default();
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
                let body = Body::Message(Message {
                    phase,
                    ..Message::default()
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
            _ => self.unknown.push(native_kind.clone()),
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
        for (n, decision) in offered.into_iter().enumerate() {
            let key = match &decision {
                Value::String(word) => word.clone(),
                Value::Object(one) => one.keys().next().cloned().unwrap_or_default(),
                other => other.to_string(),
            };
            let id = format!("c{n}");
            let label = match key.as_str() {
                "accept" => "Allow once",
                "acceptForSession" => "Allow for this session",
                "decline" => "Deny",
                "cancel" => "Deny and stop",
                "acceptWithExecpolicyAmendment" => "Allow, and don't ask again for this command",
                other => other,
            };
            choices.push(Choice {
                id: id.clone(),
                label: label.to_string(),
                effect: if key.starts_with("accept") {
                    Effect::Allow
                } else {
                    Effect::Deny
                },
            });
            decisions.push((id, decision));
        }
        // Codex accepts "decline" although it does not always list it (rule 2).
        if !decisions.iter().any(|(_, d)| *d == "decline") {
            let id = format!("c{}", choices.len());
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
        let approval = p.meta.and_then(|m| m.codex_approval_kind);
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
                    },
                );
            }
            let subject = self.open.iter().find_map(|(item, ev)| match ev.body() {
                Body::Tool(t) if t.kind == ToolKind::Mcp => Some(item.as_str().into()),
                _ => None,
            });
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
        let asked: Vec<Asked> = p
            .requested_schema
            .and_then(|s| s.properties)
            .unwrap_or_default()
            .0
            .into_iter()
            .map(|(key, field)| Asked {
                text: field
                    .description
                    .or(field.title)
                    .unwrap_or_else(|| key.clone()),
                id: key,
                header: None,
                options: Vec::new(),
                multiple: false,
                allow_text: true,
                secret: false,
            })
            .collect();
        let item = self.request_item("question", p.turn_id.as_deref(), id.as_ref());
        self.ask(out, id, &item, None, asked, &text, native);
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
        // Two questions with one id could not be answered apart. The request stays unanswered
        // here; the native message is still the harness's own.
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
        let result = m.result.map_or("{}", RawValue::get);
        match asked {
            Pending::Approval {
                item,
                decisions,
                choices,
                reply: replied_by,
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
            Pending::Question { item } => {
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
        let method = m.id.as_ref().and_then(|id| self.requests.remove(id));
        if let Some(error) = m.error {
            self.starts.retain(|(id, _)| Some(id) != m.id.as_ref()); // that request starts no turn
            let Some(e) = self.read::<ErrorReply>("an error reply", error.get()) else {
                return;
            };
            // Numbered like every item the driver names, not by request id, which Codex reuses.
            let item = format!("error:{}", self.made);
            let body = Body::Error(Error {
                request: method.clone(),
                code: e.code,
                will_retry: None,
            });
            let text = e.message.unwrap_or_default();
            let kind = format!("error response to {}", show(method.as_deref()));
            let native = native(&kind, error.get());
            let title = "The harness refused a request";
            return self.emit(out, &item, body, State::Failed, title, &text, native);
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
        // An event from a message that names its thread belongs to that thread.
        let session = self.message_thread.clone().or_else(|| self.session.clone());
        let owner = (session, self.turn.clone());
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
/// (D-041). Codex replies with labels; an MCP server's form gets values.
fn answered(questions: &[Asked], reply: QuestionReply) -> Vec<Answered> {
    let given: Vec<(String, Vec<Value>)> = match reply.answers {
        Some(answers) => answers
            .0
            .into_iter()
            .map(|(id, given)| (id, given.answers.unwrap_or_default()))
            .collect(),
        None => reply
            .content
            .unwrap_or_default()
            .0
            .into_iter()
            .map(|(id, value)| (id, vec![value]))
            .collect(),
    };
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
