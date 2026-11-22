//! A fake ACP agent, grown from the in-memory agent in the ACP driver's tests: JSON-RPC over
//! lines, in the shapes `fixtures/recordings/acp` holds. One process serves every session.
//!
//! It comes in three flavours where the recorded agents differ:
//!
//! - **OpenCode** offers `session/resume`, which replays nothing.
//! - **Goose** offers only `session/load`, which replays the session, numbers its sessions by day
//!   and a counter, says in `_meta.goose.activeRunId` when it runs something, and carries on by
//!   itself, after a load, with a turn a crash cut off, until it is cancelled.
//! - **Cursor** offers only `session/load`, gives no name or version, and asks the client to take
//!   its to-do list (`cursor/update_todos`) naming only the tool call.

use super::{Heard, Outbox, Protocol, Script, fresh};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Which recorded agent the fake is shaped like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    OpenCode,
    Goose,
    Cursor,
}

/// What outlives one agent process: each session's past, and the sessions whose turn a crash cut.
#[derive(Default)]
pub struct Store {
    sessions: HashMap<String, Vec<String>>,
    cut: HashSet<String>,
    counter: u64,
}

/// A shared store, for every process the fake starts.
pub type Shared = Arc<Mutex<Store>>;

struct Turn {
    prompt: Value,
    text: String,
    call: Option<String>,
}

enum Asked {
    Permission { session: String },
    Form { session: String },
    Todos,
}

/// One ACP agent process.
pub struct Acp {
    flavor: Flavor,
    store: Shared,
    sessions: HashMap<String, Option<Turn>>,
    /// Sessions running something by themselves (Goose).
    own_runs: HashSet<String>,
    asked: HashMap<i64, Asked>,
    next_request: i64,
}

fn update(session: &str, update: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": update}})
}

fn config(model: &str) -> Value {
    json!([{"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
        "options": [{"value": "m1", "name": "M1"}, {"value": "m2", "name": "M2"}]}])
}

impl Acp {
    /// A process shaped like `flavor`, that knows the sessions in `store`.
    pub fn new(flavor: Flavor, store: Shared) -> Self {
        Self {
            flavor,
            store,
            sessions: HashMap::new(),
            own_runs: HashSet::new(),
            asked: HashMap::new(),
            next_request: 0,
        }
    }

    fn reply(id: &Value, result: Value, out: &mut Outbox) {
        out.reply(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn refuse(id: &Value, out: &mut Outbox) {
        out.reply(
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "refused"}}),
        );
    }

    fn run(&self, session: &str, running: bool, out: &mut Outbox) {
        if self.flavor == Flavor::Goose {
            let run = if running {
                Value::String(fresh("run"))
            } else {
                Value::Null
            };
            out.say(update(
                session,
                json!({"sessionUpdate": "session_info_update", "_meta": {"goose": {"activeRunId": run}}}),
            ));
        }
    }

    fn ask(&mut self, method: &str, params: Value, asked: Asked, out: &mut Outbox) {
        let id = self.next_request;
        self.next_request += 1;
        self.asked.insert(id, asked);
        out.say(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    /// Ends the session's turn with `stop`, after the agent's words when it completed.
    fn finish(&mut self, session: &str, words: Option<&str>, stop: &str, out: &mut Outbox) {
        let Some(Some(turn)) = self.sessions.get_mut(session).map(Option::take) else {
            return;
        };
        if let Some(call) = &turn.call {
            let status = if stop == "end_turn" {
                "completed"
            } else {
                "failed"
            };
            out.say(update(
                session,
                json!({"sessionUpdate": "tool_call_update", "toolCallId": call, "status": status}),
            ));
        }
        if let Some(words) = words {
            let text = format!("{words}: {}", turn.text);
            out.say(update(
                session,
                json!({"sessionUpdate": "agent_message_chunk", "messageId": fresh("msg"),
                "content": {"type": "text", "text": text}}),
            ));
            let mut store = self.store.lock().unwrap();
            store
                .sessions
                .entry(session.to_string())
                .or_default()
                .push(turn.text.clone());
        }
        self.asked.retain(|_, a| match a {
            Asked::Permission { session: s } | Asked::Form { session: s } => s != session,
            Asked::Todos => true,
        });
        self.run(session, false, out);
        Self::reply(&turn.prompt, json!({"stopReason": stop}), out);
    }

    fn new_session(&mut self) -> String {
        let mut store = self.store.lock().unwrap();
        store.counter += 1;
        let session = match self.flavor {
            Flavor::OpenCode => fresh("ses"),
            Flavor::Goose => format!("20261122_{}", store.counter),
            Flavor::Cursor => fresh("cursor"),
        };
        store.sessions.insert(session.clone(), Vec::new());
        session
    }

    fn prompt(&mut self, id: &Value, session: &str, text: &str, out: &mut Outbox) {
        self.run(session, true, out);
        let mut turn = Turn {
            prompt: id.clone(),
            text: text.to_string(),
            call: None,
        };
        let script = Script::of(text);
        if matches!(script, Script::Ask | Script::Sleep | Script::Die) {
            let call = fresh("call");
            let status = if script == Script::Ask {
                "pending"
            } else {
                "in_progress"
            };
            out.say(update(
                session,
                json!({"sessionUpdate": "tool_call", "toolCallId": call, "title": format!("echo {text}"),
                "kind": "execute", "status": status, "rawInput": {"command": format!("echo {text}")}}),
            ));
            turn.call = Some(call);
        }
        let call = turn.call.clone();
        self.sessions.insert(session.to_string(), Some(turn));
        match script {
            // The ACP driver gives the agent no tool to send a file yet.
            Script::Reply | Script::File => self.finish(session, Some("ok"), "end_turn", out),
            Script::Ask => {
                if self.flavor == Flavor::Cursor {
                    let params = json!({"toolCallId": call, "merge": false,
                        "todos": [{"id": "1", "content": "echo", "status": "in_progress"}]});
                    self.ask("cursor/update_todos", params, Asked::Todos, out);
                }
                let params = json!({"sessionId": session,
                    "toolCall": {"toolCallId": call, "title": format!("echo {text}"), "kind": "execute",
                        "rawInput": {"command": format!("echo {text}")}},
                    "options": [{"optionId": "once", "name": "Allow once", "kind": "allow_once"},
                                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]});
                let asked = Asked::Permission {
                    session: session.to_string(),
                };
                self.ask("session/request_permission", params, asked, out);
            }
            Script::Quiz => {
                let params = json!({"mode": "form", "sessionId": session, "message": "Questions",
                    "requestedSchema": {"type": "object", "required": [], "properties": {"q0": {
                        "type": "string", "title": "Pick", "description": format!("Which? {text}"),
                        "oneOf": [{"const": "Yes", "title": "Yes"}, {"const": "No", "title": "No"}]}}}});
                let asked = Asked::Form {
                    session: session.to_string(),
                };
                self.ask("elicitation/create", params, asked, out);
            }
            Script::Sleep => {}
            Script::Die => {
                self.store.lock().unwrap().cut.insert(session.to_string());
                out.die();
            }
        }
    }
}

impl Protocol for Acp {
    fn hear(&mut self, frame: &Value, out: &mut Outbox, heard: &mut Vec<Heard>) {
        let id = &frame["id"];
        let params = &frame["params"];
        let session = params["sessionId"].as_str().unwrap_or_default().to_string();
        let Some(method) = frame["method"].as_str() else {
            let Some(asked) = id.as_i64().and_then(|id| self.asked.remove(&id)) else {
                return;
            };
            let result = &frame["result"];
            match asked {
                Asked::Todos => {}
                Asked::Permission { session } => {
                    if result["outcome"]["outcome"] != "selected" {
                        return; // cancelled: the turn is ending
                    }
                    let allow = result["outcome"]["optionId"] == "once";
                    heard.push(Heard::Answer {
                        session: session.clone(),
                        allow: Some(allow),
                        text: None,
                    });
                    let words = if allow { "allowed" } else { "denied" };
                    self.finish(&session, Some(words), "end_turn", out);
                }
                Asked::Form { session } => {
                    if result["action"] != "accept" {
                        return;
                    }
                    let text = result["content"]["q0"].as_str().map(String::from);
                    heard.push(Heard::Answer {
                        session: session.clone(),
                        allow: None,
                        text,
                    });
                    self.finish(&session, Some("answered"), "end_turn", out);
                }
            }
            return;
        };
        match method {
            "initialize" => {
                let result = match self.flavor {
                    Flavor::OpenCode => json!({"protocolVersion": 1,
                        "agentCapabilities": {"loadSession": true, "sessionCapabilities": {"resume": {}}},
                        "agentInfo": {"name": "opencode", "version": "0.0.0-fake"}}),
                    Flavor::Goose => json!({"protocolVersion": 1,
                        "agentCapabilities": {"loadSession": true, "sessionCapabilities": {}},
                        "agentInfo": {"name": "goose", "version": "0.0.0-fake"}}),
                    Flavor::Cursor => json!({"protocolVersion": 1,
                        "agentCapabilities": {"loadSession": true, "sessionCapabilities": {"list": {}}}}),
                };
                Self::reply(id, result, out);
            }
            "session/new" => {
                let session = self.new_session();
                self.sessions.insert(session.clone(), None);
                Self::reply(
                    id,
                    json!({"sessionId": session, "configOptions": config("m1")}),
                    out,
                );
                heard.push(Heard::Opened {
                    session,
                    resumed: false,
                    folder: params["cwd"].as_str().map(String::from),
                });
            }
            "session/resume" | "session/load" => {
                let offered = match self.flavor {
                    Flavor::OpenCode => method == "session/resume",
                    _ => method == "session/load",
                };
                let past = self.store.lock().unwrap().sessions.get(&session).cloned();
                let (true, Some(past)) = (offered, past) else {
                    return Self::refuse(id, out);
                };
                if method == "session/load" {
                    for text in past {
                        out.say(update(
                            &session,
                            json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": text}}),
                        ));
                        out.say(update(
                            &session,
                            json!({"sessionUpdate": "agent_message_chunk", "messageId": fresh("msg"),
                            "content": {"type": "text", "text": format!("ok: {text}")}}),
                        ));
                    }
                }
                self.sessions.insert(session.clone(), None);
                // Goose carries on by itself with a turn a crash cut off.
                let cut = self.store.lock().unwrap().cut.remove(&session);
                if self.flavor == Flavor::Goose && cut {
                    self.own_runs.insert(session.clone());
                    self.run(&session, true, out);
                }
                Self::reply(id, json!({"configOptions": config("m1")}), out);
                heard.push(Heard::Opened {
                    session,
                    resumed: true,
                    folder: params["cwd"].as_str().map(String::from),
                });
            }
            "session/prompt" => {
                let text = params["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                // Heard even when refused: a copy turned away still reached the agent.
                let refused = !matches!(self.sessions.get(&session), Some(None))
                    || self.own_runs.contains(&session);
                heard.push(Heard::Prompt {
                    session: session.clone(),
                    text: text.clone(),
                    refused,
                });
                if refused {
                    return Self::refuse(id, out); // no such session, or busy
                }
                self.prompt(id, &session, &text, out);
            }
            "session/cancel" => {
                heard.push(Heard::Control {
                    session: Some(session.clone()),
                    what: "interrupt".into(),
                });
                if self.own_runs.remove(&session) {
                    self.run(&session, false, out);
                }
                self.finish(&session, None, "cancelled", out);
            }
            "session/set_config_option" => {
                if !self.sessions.contains_key(&session) {
                    return Self::refuse(id, out);
                }
                let value = params["value"].as_str().unwrap_or("m1");
                Self::reply(id, json!({"configOptions": config(value)}), out);
                heard.push(Heard::Control {
                    session: Some(session),
                    what: "set".into(),
                });
            }
            _ if frame.get("id").is_some() => {
                out.reply(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}));
            }
            _ => {}
        }
    }

    fn ended(&mut self) {
        let mut store = self.store.lock().unwrap();
        for (session, turn) in &self.sessions {
            if turn.is_some() {
                store.cut.insert(session.clone());
            }
        }
    }
}
