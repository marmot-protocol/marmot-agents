use super::{Direction, Translator, native, notice, owner};
use crate::messages::{
    ApprovalReply, ApprovalRequest, Elicitation, Envelope, Id, QuestionReply, UserInput,
};
use marmot_agent_driver::{Answer as DriverAnswer, Given, Reason, Refusal};
use marmot_agent_events::body::{
    AnswerVia, Answered, Approval, ApprovalAnswer, Asked, Choice, Effect, NoticeKind, Offered,
    Question, ToolKind,
};
use marmot_agent_events::{AgentEvent, Body, Native, State};
use serde_json::value::RawValue;
use serde_json::{Value, json};

/// What the driver waits to hear back about.
pub(super) enum Pending {
    Approval {
        item: String,
        /// Each choice's id, and the decision Codex takes for it: for an MCP server, the whole
        /// result it is sent.
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
pub(super) enum Reply {
    /// Under `decision`.
    Decision,
    /// Under `action`, for an MCP server.
    Action,
    /// As `permissions` granted for a `scope`. An empty grant denies.
    Grant,
}

impl Translator {
    pub(super) fn approval(
        &mut self,
        method: &str,
        id: Option<Id>,
        params: &str,
        out: &mut Vec<AgentEvent>,
    ) {
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
    pub(super) fn permissions(
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
    pub(super) fn elicitation(
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
            // An approval lasts for this call, or for as long as a scope Codex names in `persist`
            // (#35). A scope the driver does not know is not offered: how long it lasts would be
            // a guess.
            let scopes = match meta.persist {
                Some(Value::String(one)) => vec![one],
                Some(Value::Array(many)) => many
                    .into_iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                _ => Vec::new(),
            };
            let accept = json!({"action": "accept", "content": {}});
            let mut offered = vec![("Allow once", Effect::Allow, accept.clone())];
            for scope in scopes {
                let label = match scope.as_str() {
                    "session" => "Allow for this session",
                    "always" => "Always allow",
                    _ => continue,
                };
                let mut lasting = accept.clone();
                lasting["_meta"] = json!({"persist": scope});
                offered.push((label, Effect::Allow, lasting));
            }
            let decline = json!({"action": "decline", "content": null});
            offered.push(("Deny", Effect::Deny, decline));
            let mut choices = Vec::new();
            let mut decisions = Vec::new();
            for (n, (label, effect, result)) in offered.into_iter().enumerate() {
                let id = format!("c{n}");
                choices.push(Choice {
                    id: id.clone(),
                    label: label.into(),
                    effect,
                });
                decisions.push((id, result));
            }
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

    pub(super) fn user_input(
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
    pub(super) fn answer(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        let Some(asked) = m.id.as_ref().and_then(|id| self.asked.remove(id)) else {
            return;
        };
        let (Pending::Approval { item, .. } | Pending::Question { item, .. }) = &asked;
        self.answered.insert(item.clone());
        // The driver answers with a result; read from nothing, an answer without one would make
        // an event whose native message never arrived (D-007).
        let Some(result) = m.result.map(RawValue::get) else {
            self.unreadable += 1;
            return;
        };
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
                    Reply::Decision => {
                        let chosen = decisions
                            .iter()
                            .find(|(_, d)| Some(d) == reply.decision.as_ref())
                            .map(|(id, _)| id.clone());
                        (reply.decision, chosen)
                    }
                    // The choice is the action and, for an approval that lasts, its scope.
                    Reply::Action => {
                        let persist = reply.meta.as_ref().map_or(&Value::Null, |m| &m["persist"]);
                        let chosen = decisions
                            .iter()
                            .find(|(_, r)| {
                                Some(&r["action"]) == reply.action.as_ref()
                                    && r["_meta"]["persist"] == *persist
                            })
                            .map(|(id, _)| id.clone());
                        (reply.action, chosen)
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

    /// The item for a request from Codex. Codex counts its requests from 0 again when it restarts,
    /// so the id names the turn as well, which keeps it unique. A request in no turn is numbered
    /// as well, like every item the driver names (#37).
    fn request_item(&self, kind: &str, turn: Option<&str>, id: Option<&Id>) -> String {
        match turn.or(self.turn.as_deref()) {
            Some(turn) => format!("{kind}:{turn}:{}", show_id(id)),
            None => self.made_up(&format!("{kind}:None:{}", show_id(id))),
        }
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
                    } else {
                        decision.clone()
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

fn show_id(id: Option<&Id>) -> String {
    id.map_or_else(|| "None".to_string(), ToString::to_string)
}
