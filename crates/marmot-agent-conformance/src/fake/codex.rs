//! A fake `codex app-server`, grown from the stand-in in the Codex driver's tests: JSON-RPC over
//! lines, in the shapes `fixtures/recordings/codex` holds. One process serves every thread.
//!
//! It starts subagents as `fixtures/recordings/codex/subagents` shows them: each a thread of its
//! own on the same connection, named by an item of the turn that starts it, with no
//! `thread/started`, and with every turn naming the turn the person started (`rootTurnId`). On the
//! older runtime the subagent's turn starts before the item that names it ends, so it speaks
//! first. Interrupting a turn leaves its subagents running, and its wait for them is never ended.
//! A word in the prompt asks for one ([`Spawn`]); what the subagent does is the prompt's other
//! words, as for any turn.

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
    /// The turn the person started, for a subagent's turn; its own id for one the driver started.
    root: String,
    text: String,
    steered: Vec<String>,
    /// The command it runs, if any.
    command: Option<String>,
    /// The item that waits for its subagents, while it does, and the subagents it waits for.
    wait: Option<String>,
    children: Vec<String>,
}

/// What a prompt asks of a subagent, by a word in its text. The subagent's own prompt is the text
/// without that word, so its other words say what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spawn {
    /// `#spawn`: one subagent, on Codex's newer runtime (`subAgentActivity`); the turn waits for
    /// it, then answers.
    Wait,
    /// `#spawn1`: the same on the older runtime (`collabAgentToolCall`), where the subagent's turn
    /// starts before the item that names it is complete.
    Older,
    /// `#nest`: a subagent that starts one of its own, and waits for it.
    Nest,
    /// `#outlive`: a subagent the turn does not wait for: the turn ends while it works.
    Outlive,
}

impl Spawn {
    /// What `text` asks for, and the subagent's own prompt.
    fn of(text: &str) -> Option<(Self, String)> {
        let words = [
            ("#spawn1", Spawn::Older, ""),
            ("#spawn", Spawn::Wait, ""),
            ("#nest", Spawn::Nest, "#spawn"),
            ("#outlive", Spawn::Outlive, ""),
        ];
        let (word, spawn, then) = words.into_iter().find(|(w, ..)| text.contains(w))?;
        Some((spawn, text.replacen(word, then, 1)))
    }
}

/// A subagent's thread: the thread that started it, and its path, as Codex names one.
struct Sub {
    parent: String,
    path: String,
    older: bool,
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
    /// Each subagent's thread.
    subs: HashMap<String, Sub>,
    /// Each thread's last turn, running or not: what a subagent's end is reported in.
    last_turn: HashMap<String, String>,
    asked: HashMap<i64, Asked>,
    /// Requests Codex resolved itself, as at a turn's end, with the thread that asked.
    resolved: HashMap<i64, String>,
    next_request: i64,
}

impl Codex {
    /// A process that knows the threads in `store`.
    pub fn new(store: Shared) -> Self {
        Self {
            store,
            threads: HashMap::new(),
            subs: HashMap::new(),
            last_turn: HashMap::new(),
            asked: HashMap::new(),
            resolved: HashMap::new(),
            next_request: 0,
        }
    }

    fn turn_mark(thread: &str, turn: &str, root: &str, status: &str) -> Value {
        json!({"threadId": thread, "turn": {"id": turn, "rootTurnId": root, "items": [],
            "status": status, "error": null}})
    }

    /// The thread a person's chat has: a subagent's is its parent's, at any depth. A fake reports
    /// what it heard in the session the chat knows.
    fn root_of(&self, thread: &str) -> String {
        let mut thread = thread.to_string();
        while let Some(sub) = self.subs.get(&thread) {
            thread = sub.parent.clone();
        }
        thread
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
            self.resolved.insert(id, thread.to_string());
            out.say(json!({"method": "serverRequest/resolved", "params": {"threadId": thread, "requestId": id}}));
        }
        out.say(json!({"method": "turn/completed",
            "params": Self::turn_mark(thread, &turn.id, &turn.root, status)}));
        if self.subs.contains_key(thread) {
            self.sub_ended(thread, status, out);
        }
    }

    /// A subagent's turn has ended. On the newer runtime its parent's thread says so, in the
    /// parent's last turn, even one that has ended; a parent that waits for it, and for no other,
    /// goes on and answers.
    fn sub_ended(&mut self, thread: &str, status: &str, out: &mut Outbox) {
        let sub = &self.subs[thread];
        let (parent, path, older) = (sub.parent.clone(), sub.path.clone(), sub.older);
        let parent_turn = self.last_turn.get(&parent).cloned().unwrap_or_default();
        if !older {
            let kind = if status == "completed" {
                "completed"
            } else {
                "interrupted"
            };
            let item = json!({"type": "subAgentActivity", "id": fresh("subagent-ended"),
                "kind": kind, "agentThreadId": thread, "agentPath": path, "model": null,
                "reasoningEffort": null});
            for method in ["item/started", "item/completed"] {
                out.say(json!({"method": method, "params": {"threadId": parent,
                    "turnId": parent_turn, "item": item}}));
            }
        }
        let Some(Some(turn)) = self.threads.get_mut(&parent) else {
            return; // its turn has ended: nothing waits
        };
        turn.children.retain(|c| c != thread);
        let Some(wait) = turn.wait.clone().filter(|_| turn.children.is_empty()) else {
            return;
        };
        turn.wait = None;
        let receivers: Vec<&str> = if older { vec![thread] } else { vec![] };
        out.say(
            json!({"method": "item/completed", "params": {"threadId": parent,
            "turnId": parent_turn, "item": {"type": "collabAgentToolCall", "id": wait,
            "tool": "wait", "status": "completed", "senderThreadId": parent,
            "receiverThreadIds": receivers, "prompt": null}}}),
        );
        self.finish(&parent, "ok", "completed", out);
    }

    /// The turn starts a subagent, which works on `prompt`, and waits for it unless it is to
    /// outlive the turn.
    fn spawn(&mut self, thread: &str, turn: &str, spawn: Spawn, prompt: &str, out: &mut Outbox) {
        let child = fresh("thread");
        let item = fresh("call");
        let parent_path = self
            .subs
            .get(thread)
            .map_or("/root".into(), |s| s.path.clone());
        let path = format!("{parent_path}/{}", fresh("agent"));
        let root = (self.threads.get(thread).and_then(Option::as_ref))
            .map_or(turn.to_string(), |t| t.root.clone());
        let older = spawn == Spawn::Older;
        let sub = Sub {
            parent: thread.to_string(),
            path: path.clone(),
            older,
        };
        self.subs.insert(child.clone(), sub);
        self.threads.insert(child.clone(), None);
        let child_turn = fresh("turn");
        let started = json!({"method": "turn/started",
            "params": Self::turn_mark(&child, &child_turn, &root, "inProgress")});
        let active = json!({"method": "thread/status/changed",
            "params": {"threadId": child, "status": {"type": "active", "activeFlags": []}}});
        let note = |method: &str, item: Value| json!({"method": method, "params": {"threadId": thread, "turnId": turn, "item": item}});
        if older {
            let call = |status: &str, receivers: Vec<&str>| {
                json!({"type": "collabAgentToolCall", "id": item, "tool": "spawnAgent",
                    "status": status, "senderThreadId": thread, "receiverThreadIds": receivers,
                    "prompt": prompt, "model": null, "reasoningEffort": null})
            };
            out.say(note("item/started", call("inProgress", vec![])));
            // The subagent speaks before the item that names it is complete.
            out.say(active);
            out.say(started);
            out.say(note(
                "item/completed",
                call("completed", vec![child.as_str()]),
            ));
        } else {
            let activity = json!({"type": "subAgentActivity", "id": item, "kind": "started",
                "agentThreadId": child, "agentPath": path, "model": "m1", "reasoningEffort": "low"});
            out.say(note("item/started", activity.clone()));
            out.say(active);
            out.say(note("item/completed", activity));
            out.say(started);
        }
        if spawn == Spawn::Outlive {
            self.finish(thread, "ok", "completed", out);
        } else {
            let wait = fresh("call");
            let receivers: Vec<&str> = if older { vec![child.as_str()] } else { vec![] };
            out.say(note(
                "item/started",
                json!({"type": "collabAgentToolCall", "id": wait,
                "tool": "wait", "status": "inProgress", "senderThreadId": thread,
                "receiverThreadIds": receivers, "prompt": null}),
            ));
            if let Some(Some(t)) = self.threads.get_mut(thread) {
                t.wait = Some(wait);
                t.children.push(child.clone());
            }
        }
        self.run_turn(&child, &child_turn, &root, prompt, out);
    }

    fn ask(&mut self, method: &str, params: Value, asked: Asked, out: &mut Outbox) {
        let id = self.next_request;
        self.next_request += 1;
        self.asked.insert(id, asked);
        out.say(json!({"id": id, "method": method, "params": params}));
    }

    fn start_turn(&mut self, id: &Value, thread: &str, text: &str, out: &mut Outbox) {
        let turn = fresh("turn");
        out.reply(
            json!({"id": id, "result": {"turn": {"id": turn, "rootTurnId": turn,
            "items": [], "status": "inProgress", "error": null}}}),
        );
        out.say(json!({"method": "turn/started",
            "params": Self::turn_mark(thread, &turn, &turn, "inProgress")}));
        self.run_turn(thread, &turn, &turn, text, out);
    }

    /// A turn Codex has said started does what its text asks.
    fn run_turn(&mut self, thread: &str, turn: &str, root: &str, text: &str, out: &mut Outbox) {
        let mut running = Turn {
            id: turn.to_string(),
            root: root.to_string(),
            text: text.to_string(),
            steered: Vec::new(),
            command: None,
            wait: None,
            children: Vec::new(),
        };
        self.last_turn.insert(thread.to_string(), turn.to_string());
        if let Some((spawn, prompt)) = Spawn::of(text) {
            self.threads.insert(thread.to_string(), Some(running));
            return self.spawn(thread, turn, spawn, &prompt, out);
        }
        let script = Script::of(text);
        if matches!(script, Script::Ask | Script::Sleep | Script::Die) {
            let item = fresh("exec");
            out.say(Self::command(
                thread,
                turn,
                &item,
                &format!("echo {text}"),
                "inProgress",
            ));
            running.command = Some(item);
        }
        let command = running.command.clone();
        self.threads.insert(thread.to_string(), Some(running));
        // A subagent's words say they are its own.
        let words = if self.subs.contains_key(thread) {
            "sub"
        } else {
            "ok"
        };
        match script {
            Script::Reply => self.finish(thread, words, "completed", out),
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
                // A reply to a request Codex resolved itself reached it all the same: no one's
                // answer may come this late.
                if let Some(thread) = id.as_i64().and_then(|id| self.resolved.remove(&id)) {
                    heard.push(Heard::Answer {
                        session: thread,
                        allow: None,
                        text: None,
                    });
                }
                return;
            };
            let result = &frame["result"];
            match asked {
                Asked::Approval { thread } => {
                    let allow = result["decision"] == "accept";
                    heard.push(Heard::Answer {
                        session: self.root_of(&thread),
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
                        session: self.root_of(&thread),
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
                        session: self.root_of(&thread),
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
                    session: Some(self.root_of(&thread)),
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
