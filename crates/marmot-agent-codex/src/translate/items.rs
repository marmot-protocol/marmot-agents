use super::{Translator, native, native_text, notice, show};
use crate::messages::{ChangeKind as NativeChange, ItemNote, PlanUpdated};
use marmot_agent_events::body::{
    AnswerVia, Asked, Change, ChangeKind, Message, Notice, NoticeKind, Offered, Phase, Plan,
    PlanEntry, PlanEntryState, Question, Reasoning, Tool, ToolKind,
};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::{Value, json};

impl Translator {
    pub(super) fn item(&mut self, method: &str, params: &str, out: &mut Vec<AgentEvent>) {
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

    pub(super) fn plan(&mut self, method: &str, params: &str, out: &mut Vec<AgentEvent>) {
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

/// Rounds half to even, as the prototype's Python does.
fn round_half_even(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        (r - x.signum()) as i64
    } else {
        r as i64
    }
}
