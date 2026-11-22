//! A fake `codex app-server`, grown from the stand-in in the Codex driver's tests: JSON-RPC over
//! lines, in the shapes `fixtures/recordings/codex` holds. One process serves every thread.

use super::{Heard, Outbox, Protocol, Script, file_for, fresh};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// What outlives one Codex process: the threads it can resume, and those started with the
/// driver's `send_file` tool, which Codex keeps with the thread.
#[derive(Default)]
pub struct Store {
    threads: HashSet<String>,
    send_file: HashSet<String>,
}

/// A shared store, for every process the fake starts.
pub type Shared = Arc<Mutex<Store>>;

struct Turn {
    id: String,
    text: String,
    steered: Vec<String>,
    /// The command it runs, if any.
    command: Option<String>,
}

enum Asked {
    Approval {
        thread: String,
    },
    Question {
        thread: String,
    },
    /// A call of the driver's own tool, `send_file`.
    Tool {
        thread: String,
    },
}

/// One Codex process.
pub struct Codex {
    store: Shared,
    threads: HashMap<String, Option<Turn>>,
    asked: HashMap<i64, Asked>,
    next_request: i64,
}

impl Codex {
    /// A process that knows the threads in `store`.
    pub fn new(store: Shared) -> Self {
        Self {
            store,
            threads: HashMap::new(),
            asked: HashMap::new(),
            next_request: 0,
        }
    }

    fn turn_mark(thread: &str, turn: &str, status: &str) -> Value {
        json!({"threadId": thread, "turn": {"id": turn, "items": [], "status": status, "error": null}})
    }

    fn command(thread: &str, turn: &str, item: &str, command: &str, status: &str) -> Value {
        let method = if status == "inProgress" {
            "item/started"
        } else {
            "item/completed"
        };
        json!({"method": method, "params": {"threadId": thread, "turnId": turn, "item": {
            "type": "commandExecution", "id": item, "command": command, "cwd": "/work/demo",
            "status": status, "commandActions": [], "aggregatedOutput": null, "exitCode": null}}})
    }

    /// Ends the thread's turn: the command, the agent's words, and the turn's end.
    fn finish(&mut self, thread: &str, words: &str, status: &str, out: &mut Outbox) {
        let Some(Some(turn)) = self.threads.get_mut(thread).map(Option::take) else {
            return;
        };
        if let Some(item) = &turn.command {
            let done = if status == "completed" {
                "completed"
            } else {
                "failed"
            };
            out.say(Self::command(thread, &turn.id, item, "echo", done));
        }
        if status == "completed" {
            let mut text = format!("{words}: {}", turn.text);
            for more in &turn.steered {
                text.push_str(" and ");
                text.push_str(more);
            }
            out.say(json!({"method": "item/completed", "params": {"threadId": thread, "turnId": turn.id,
                "item": {"type": "agentMessage", "id": fresh("msg"), "text": text, "phase": "final_answer"}}}));
        }
        // What the turn still asked is resolved with it.
        let ids: Vec<i64> = (self.asked.iter())
            .filter(|(_, a)| match a {
                Asked::Approval { thread: t }
                | Asked::Question { thread: t }
                | Asked::Tool { thread: t } => t == thread,
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.asked.remove(&id);
            out.say(json!({"method": "serverRequest/resolved", "params": {"threadId": thread, "requestId": id}}));
        }
        out.say(
            json!({"method": "turn/completed", "params": Self::turn_mark(thread, &turn.id, status)}),
        );
    }

    fn ask(&mut self, method: &str, params: Value, asked: Asked, out: &mut Outbox) {
        let id = self.next_request;
        self.next_request += 1;
        self.asked.insert(id, asked);
        out.say(json!({"id": id, "method": method, "params": params}));
    }

    fn start_turn(&mut self, id: &Value, thread: &str, text: &str, out: &mut Outbox) {
        let turn = fresh("turn");
        out.reply(json!({"id": id, "result": {"turn": {"id": turn, "items": [], "status": "inProgress", "error": null}}}));
        out.say(
            json!({"method": "turn/started", "params": Self::turn_mark(thread, &turn, "inProgress")}),
        );
        let mut running = Turn {
            id: turn.clone(),
            text: text.to_string(),
            steered: Vec::new(),
            command: None,
        };
        let script = Script::of(text);
        if matches!(script, Script::Ask | Script::Sleep | Script::Die) {
            let item = fresh("exec");
            out.say(Self::command(
                thread,
                &turn,
                &item,
                &format!("echo {text}"),
                "inProgress",
            ));
            running.command = Some(item);
        }
        let command = running.command.clone();
        self.threads.insert(thread.to_string(), Some(running));
        match script {
            Script::Reply => self.finish(thread, "ok", "completed", out),
            Script::File if !self.store.lock().unwrap().send_file.contains(thread) => {
                self.finish(thread, "no send_file tool", "completed", out);
            }
            Script::File => {
                let params = json!({"threadId": thread, "turnId": turn, "callId": fresh("call"),
                    "namespace": null, "tool": "send_file", "arguments": {"path": file_for(text)}});
                let asked = Asked::Tool {
                    thread: thread.to_string(),
                };
                self.ask("item/tool/call", params, asked, out);
            }
            Script::Ask => {
                let params = json!({"threadId": thread, "turnId": turn, "itemId": command,
                    "command": format!("echo {text}"), "cwd": "/work/demo",
                    "availableDecisions": ["accept", "decline"]});
                let asked = Asked::Approval {
                    thread: thread.to_string(),
                };
                self.ask("item/commandExecution/requestApproval", params, asked, out);
            }
            Script::Quiz => {
                let params = json!({"threadId": thread, "turnId": turn, "itemId": fresh("call"),
                    "questions": [{"id": "q1", "header": "Pick", "question": format!("Which? {text}"),
                    "isOther": false, "isSecret": false,
                    "options": [{"label": "Yes", "description": "yes"}, {"label": "No", "description": "no"}]}]});
                let asked = Asked::Question {
                    thread: thread.to_string(),
                };
                self.ask("item/tool/requestUserInput", params, asked, out);
            }
            Script::Sleep => {}
            Script::Die => out.die(),
        }
    }

    fn refuse(id: &Value, out: &mut Outbox) {
        out.reply(json!({"id": id, "error": {"code": -32600, "message": "refused"}}));
    }
}

impl Protocol for Codex {
    fn hear(&mut self, frame: &Value, out: &mut Outbox, heard: &mut Vec<Heard>) {
        let id = &frame["id"];
        let params = &frame["params"];
        let thread = params["threadId"].as_str().unwrap_or_default().to_string();
        let Some(method) = frame["method"].as_str() else {
            // A reply to something the harness asked.
            let Some(asked) = id.as_i64().and_then(|id| self.asked.remove(&id)) else {
                return;
            };
            let result = &frame["result"];
            match asked {
                Asked::Approval { thread } => {
                    let allow = result["decision"] == "accept";
                    heard.push(Heard::Answer {
                        session: thread.clone(),
                        allow: Some(allow),
                        text: None,
                    });
                    out.say(json!({"method": "serverRequest/resolved", "params": {"threadId": thread, "requestId": id}}));
                    let words = if allow { "allowed" } else { "denied" };
                    self.finish(&thread, words, "completed", out);
                }
                Asked::Question { thread } => {
                    let text = result["answers"]["q1"]["answers"][0]
                        .as_str()
                        .map(String::from);
                    heard.push(Heard::Answer {
                        session: thread.clone(),
                        allow: None,
                        text: text.clone(),
                    });
                    out.say(json!({"method": "serverRequest/resolved", "params": {"threadId": thread, "requestId": id}}));
                    self.finish(&thread, "answered", "completed", out);
                }
                Asked::Tool { thread } => {
                    let text = result["contentItems"][0]["text"]
                        .as_str()
                        .unwrap_or_default();
                    heard.push(Heard::FileResult {
                        session: thread.clone(),
                        sent: result["success"] == true,
                        text: text.to_string(),
                    });
                    out.say(json!({"method": "serverRequest/resolved", "params": {"threadId": thread, "requestId": id}}));
                    self.finish(&thread, "filed", "completed", out);
                }
            }
            return;
        };
        match method {
            "initialize" => {
                out.reply(
                    json!({"id": id, "result": {"userAgent": "codex_cli_rs/0.169.0 (fake)"}}),
                );
            }
            "initialized" => {}
            "model/list" => {
                out.reply(json!({"id": id, "result": {"data": [{"id": "m1"}, {"id": "m2"}]}}));
            }
            "collaborationMode/list" => {
                out.reply(json!({"id": id, "result": {"data": [{"mode": "default"}]}}));
            }
            "thread/start" => {
                let thread = fresh("thread");
                self.threads.insert(thread.clone(), None);
                let mut store = self.store.lock().unwrap();
                store.threads.insert(thread.clone());
                let tools = params["dynamicTools"].as_array().into_iter().flatten();
                if tools.into_iter().any(|t| t["name"] == "send_file") {
                    store.send_file.insert(thread.clone());
                }
                drop(store);
                out.reply(json!({"id": id, "result": {"thread": {"id": thread}, "model": "m1", "reasoningEffort": "low"}}));
                heard.push(Heard::Opened {
                    session: thread,
                    resumed: false,
                    folder: params["cwd"].as_str().map(String::from),
                });
            }
            "thread/resume" => {
                if !self.store.lock().unwrap().threads.contains(&thread) {
                    return Self::refuse(id, out);
                }
                self.threads.entry(thread.clone()).or_insert(None);
                out.reply(json!({"id": id, "result": {"thread": {"id": thread}, "model": "m1", "reasoningEffort": "low"}}));
                heard.push(Heard::Opened {
                    session: thread,
                    resumed: true,
                    folder: params["cwd"].as_str().map(String::from),
                });
            }
            "turn/start" => {
                let text = params["input"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                // Heard even when refused: a copy turned away still reached the harness.
                let refused = !matches!(self.threads.get(&thread), Some(None));
                heard.push(Heard::Prompt {
                    session: thread.clone(),
                    text: text.clone(),
                    refused,
                });
                if refused {
                    return Self::refuse(id, out); // no such thread here, or a turn running
                }
                self.start_turn(id, &thread, &text, out);
            }
            "turn/steer" => {
                let text = params["input"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let running = match self.threads.get_mut(&thread) {
                    Some(Some(turn)) if params["expectedTurnId"] == turn.id.as_str() => Some(turn),
                    _ => None,
                };
                heard.push(Heard::Prompt {
                    session: thread,
                    text: text.clone(),
                    refused: running.is_none(),
                });
                let Some(turn) = running else {
                    return Self::refuse(id, out); // no turn running, or another one
                };
                turn.steered.push(text);
                out.reply(json!({"id": id, "result": {"turnId": turn.id}}));
            }
            "turn/interrupt" => {
                let running = matches!(self.threads.get(&thread), Some(Some(t)) if params["turnId"] == t.id.as_str());
                if !running {
                    return Self::refuse(id, out);
                }
                out.reply(json!({"id": id, "result": {}}));
                heard.push(Heard::Control {
                    session: Some(thread.clone()),
                    what: "interrupt".into(),
                });
                self.finish(&thread, "stopped", "interrupted", out);
            }
            "thread/settings/update" => {
                if !self.threads.contains_key(&thread) {
                    return Self::refuse(id, out);
                }
                out.reply(json!({"id": id, "result": {}}));
                let mut settings = json!({});
                for (field, value) in params.as_object().into_iter().flatten() {
                    if field != "threadId" {
                        settings[field] = value.clone();
                    }
                }
                out.say(json!({"method": "thread/settings/updated", "params": {"threadId": thread, "threadSettings": settings}}));
                heard.push(Heard::Control {
                    session: Some(thread),
                    what: "set".into(),
                });
            }
            _ if frame.get("id").is_some() => {
                out.reply(
                    json!({"id": id, "error": {"code": -32601, "message": "Method not found"}}),
                );
            }
            _ => {}
        }
    }
}
