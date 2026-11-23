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
//!
//! Each takes MCP servers over HTTP from the client, as the real ones do, and calls the driver's
//! `send_file` on `#file` the way each was seen to (2026-11-22): **OpenCode** keeps one server of a
//! name for every session in a folder, the one handed over last, and names the calling session in
//! `_meta["ai.opencode/sessionID"]`; **Goose** keeps the servers of each session and names it in
//! `_meta["agent-session-id"]`; **Cursor** keeps the servers of each session and names none.

use super::mcp::Server;
use super::{Heard, Outbox, Protocol, Script, file_for, fresh};
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

/// Tool calls come back over HTTP, by session, with whether they worked and the words the model
/// reads.
type Called = Arc<Mutex<Vec<(String, Option<(bool, String)>)>>>;

struct Turn {
    prompt: Value,
    text: String,
    call: Option<String>,
}

enum Asked {
    /// A permission request; with `#twice`, which of the two for one tool call it is.
    Permission {
        session: String,
        of_two: Option<u8>,
    },
    Form {
        session: String,
    },
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
    /// The MCP servers each session was handed, and each session's folder: OpenCode keeps them by
    /// folder instead, the last handed over for a folder serving every session there.
    servers: HashMap<String, Vec<Server>>,
    by_folder: HashMap<String, Vec<Server>>,
    folders: HashMap<String, String>,
    /// Tool calls that have come back over HTTP, by session: whether the server said it worked,
    /// or `None` when it could not be used.
    called: Called,
    waker: Arc<tokio::sync::Notify>,
    /// The sessions whose `send_file` call is still out, and whether the turn was cancelled
    /// meanwhile: like an agent awaiting its tool, the turn ends only once the call comes back.
    filing: HashMap<String, bool>,
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
            servers: HashMap::new(),
            by_folder: HashMap::new(),
            folders: HashMap::new(),
            called: Arc::default(),
            waker: Arc::default(),
            filing: HashMap::new(),
        }
    }

    /// Keeps the MCP servers a session request handed over, as the flavour does.
    fn keep_servers(&mut self, session: &str, params: &Value) {
        let servers = Server::from_params(params);
        let folder = params["cwd"].as_str().unwrap_or_default().to_string();
        self.folders.insert(session.to_string(), folder.clone());
        if self.flavor == Flavor::OpenCode {
            if !servers.is_empty() {
                self.by_folder.insert(folder, servers);
            }
        } else {
            self.servers.insert(session.to_string(), servers);
        }
    }

    /// The server the session's `send_file` call reaches, if it was given one.
    fn server_for(&self, session: &str) -> Option<Server> {
        let servers = match self.flavor {
            Flavor::OpenCode => self.by_folder.get(self.folders.get(session)?)?,
            _ => self.servers.get(session)?,
        };
        servers.first().cloned()
    }

    /// Calls `send_file` over HTTP, off the pipe; the result comes back through [`Protocol::woken`].
    fn send_file(&mut self, session: &str, text: &str, call: &str, out: &mut Outbox) -> bool {
        let Some(server) = self.server_for(session) else {
            return false;
        };
        let path = file_for(text);
        out.say(update(
            session,
            json!({"sessionUpdate": "tool_call", "toolCallId": call, "title": "marmot: send_file",
            "kind": "other", "status": "in_progress", "rawInput": {"path": path}}),
        ));
        let meta = match self.flavor {
            Flavor::OpenCode => Some(json!({"ai.opencode/sessionID": session, "progressToken": 2})),
            Flavor::Goose => Some(json!({"agent-session-id": session})),
            Flavor::Cursor => None,
        };
        self.filing.insert(session.to_string(), false);
        let (called, waker, session) =
            (self.called.clone(), self.waker.clone(), session.to_string());
        tokio::spawn(async move {
            let result = server.call("send_file", json!({"path": path}), meta).await;
            let result = result.ok().map(|r| {
                let text = r["content"][0]["text"].as_str().unwrap_or_default();
                (r["isError"] != true, text.to_string())
            });
            called.lock().unwrap().push((session, result));
            waker.notify_one();
        });
        true
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
            Asked::Permission { session: s, .. } | Asked::Form { session: s } => s != session,
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
        if script == Script::File {
            let call = fresh("call");
            turn.call = Some(call.clone());
            self.sessions.insert(session.to_string(), Some(turn));
            if !self.send_file(session, text, &call, out) {
                // Given no tool, the agent says so and the turn ends.
                if let Some(Some(turn)) = self.sessions.get_mut(session) {
                    turn.call = None;
                }
                self.finish(session, Some("no such tool"), "end_turn", out);
            }
            return;
        }
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
            Script::Reply => self.finish(session, Some("ok"), "end_turn", out),
            Script::Ask => {
                if self.flavor == Flavor::Cursor {
                    let params = json!({"toolCallId": call, "merge": false,
                        "todos": [{"id": "1", "content": "echo", "status": "in_progress"}]});
                    self.ask("cursor/update_todos", params, Asked::Todos, out);
                }
                // With `#twice`, the agent asks twice for one tool call, both waiting at once:
                // nothing in ACP stops it, and each request takes its own answer.
                let twice = text.contains("#twice");
                for n in 1..=(1 + u8::from(twice)) {
                    let params = json!({"sessionId": session,
                        "toolCall": {"toolCallId": call, "title": format!("echo {text}"), "kind": "execute",
                            "rawInput": {"command": format!("echo {text}")}},
                        "options": [{"optionId": "once", "name": "Allow once", "kind": "allow_once"},
                                    {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]});
                    let asked = Asked::Permission {
                        session: session.to_string(),
                        of_two: twice.then_some(n),
                    };
                    self.ask("session/request_permission", params, asked, out);
                }
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
            Script::Sleep | Script::File => {}
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
                Asked::Permission { session, of_two } => {
                    if result["outcome"]["outcome"] != "selected" {
                        return; // cancelled: the turn is ending
                    }
                    let allow = result["outcome"]["optionId"] == "once";
                    heard.push(Heard::Answer {
                        session: session.clone(),
                        allow: Some(allow),
                        text: of_two.map(|n| format!("request {n}")),
                    });
                    // The tool runs once every request for it is answered.
                    let more = (self.asked.values()).any(
                        |a| matches!(a, Asked::Permission { session: s, .. } if *s == session),
                    );
                    if more {
                        return;
                    }
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
                        "agentCapabilities": {"loadSession": true, "sessionCapabilities": {"resume": {}},
                            "mcpCapabilities": {"http": true, "sse": false}},
                        "agentInfo": {"name": "opencode", "version": "0.0.0-fake"}}),
                    Flavor::Goose => json!({"protocolVersion": 1,
                        "agentCapabilities": {"loadSession": true, "sessionCapabilities": {},
                            "mcpCapabilities": {"http": true, "sse": false}},
                        "agentInfo": {"name": "goose", "version": "0.0.0-fake"}}),
                    Flavor::Cursor => json!({"protocolVersion": 1,
                        "agentCapabilities": {"loadSession": true, "sessionCapabilities": {"list": {}},
                            "mcpCapabilities": {"http": true, "sse": true}}}),
                };
                Self::reply(id, result, out);
            }
            "session/new" => {
                let session = self.new_session();
                self.sessions.insert(session.clone(), None);
                self.keep_servers(&session, params);
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
                self.keep_servers(&session, params);
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
                // A turn waiting on its tool ends once the call comes back.
                if let Some(cancelled) = self.filing.get_mut(&session) {
                    *cancelled = true;
                    return;
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

    fn waker(&self) -> Option<Arc<tokio::sync::Notify>> {
        Some(self.waker.clone())
    }

    fn woken(&mut self, out: &mut Outbox, heard: &mut Vec<Heard>) {
        let called = std::mem::take(&mut *self.called.lock().unwrap());
        for (session, result) in called {
            // A server it could not use is a tool that failed: nothing is heard of a result.
            if let Some((sent, text)) = &result {
                heard.push(Heard::FileResult {
                    session: session.clone(),
                    sent: *sent,
                    text: text.clone(),
                });
            }
            if self.filing.remove(&session) == Some(true) {
                self.finish(&session, None, "cancelled", out);
                continue;
            }
            let words = match result {
                Some((true, _)) => "sent",
                Some((false, _)) => "not sent",
                None => "the tool failed",
            };
            self.finish(&session, Some(words), "end_turn", out);
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
