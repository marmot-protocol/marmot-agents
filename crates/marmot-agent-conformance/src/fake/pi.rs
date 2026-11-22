//! A fake `pi --mode rpc`, grown from `crates/marmot-agent-pi/tests/fake_pi.py`: Pi's records over
//! lines, in the shapes `fixtures/recordings/pi` holds.
//!
//! Pi's driver starts one program for each chat. The program it is given is `relay.py`, which
//! connects to a Unix socket the fake listens on, says which chat it serves and with which
//! arguments it was started, and passes bytes both ways. Each connection is one Pi process, served
//! by the engine with a [`Pi`] of its own, so the test program scripts every process and can kill
//! one chat's Pi. As Pi does, a session gets its file only once it holds a message;
//! `--session <id>` takes up a session that has one and exits otherwise, and `--session-id <id>`
//! takes it up or starts an empty one with that id.

use super::{FILE, Fake, Heard, Outbox, Protocol, Script, fresh};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;

/// What outlives one Pi process: the sessions that have a file, and the chat each process
/// serves, as Pi's driver names it.
#[derive(Default)]
pub struct Store {
    files: HashSet<String>,
    /// The chat key each chat's processes were started with, by the chat named in its prompts.
    tags: HashMap<String, String>,
}

/// A shared store, for every process the fake starts.
pub type Shared = Arc<Mutex<Store>>;

static CLOCK: AtomicU64 = AtomicU64::new(0);

/// Milliseconds since the epoch, never the same twice: Pi names a turn and its messages by their
/// time, and a real model call takes longer than a millisecond.
fn now() -> u64 {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let mut last = CLOCK.load(Ordering::Relaxed);
    loop {
        let next = wall.max(last + 1);
        match CLOCK.compare_exchange(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => last = seen,
        }
    }
}

struct Run {
    text: String,
    steered: Vec<String>,
    tool: Option<String>,
    asked: Option<String>,
    /// The call of the driver's `send_file` tool, waiting in its dialog for the driver's answer.
    file: Option<String>,
}

/// The title of the dialog the driver's extension asks in, as `crates/marmot-agent-pi` names it.
const SEND_FILE_REQUEST: &str = "marmot-agent send_file";

/// Whether Pi was started with the driver's extension, which gives it the `send_file` tool: a
/// file, named after `--extension`, that registers the tool and asks in the driver's dialog.
fn has_send_file(args: &[String]) -> bool {
    let at = args.iter().position(|a| a == "--extension");
    let source = at
        .and_then(|at| args.get(at + 1))
        .and_then(|path| std::fs::read_to_string(path).ok());
    source.is_some_and(|s| s.contains("name: \"send_file\"") && s.contains(SEND_FILE_REQUEST))
}

/// One Pi process.
pub struct Pi {
    store: Shared,
    tag: String,
    session: String,
    resumed: bool,
    /// The session asked for has no file: the process exits as it starts.
    missing: bool,
    /// The folder the process was started in, as the relay says.
    folder: Option<String>,
    /// It loaded the driver's extension, and so has the `send_file` tool.
    sends_files: bool,
    thinking: String,
    run: Option<Run>,
}

fn model(id: &str) -> Value {
    json!({"id": id, "name": id, "provider": "fake"})
}

fn respond(command: &Value, data: Option<Value>, out: &mut Outbox) {
    let mut record = json!({"id": command["id"], "type": "response", "command": command["type"], "success": true});
    if let Some(data) = data {
        record["data"] = data;
    }
    out.reply(record);
}

fn assistant(content: Value, stop: &str, out: &mut Outbox) {
    let message = json!({"role": "assistant", "content": content, "usage": {"input": 10, "output": 2},
        "stopReason": stop, "timestamp": now()});
    let mut start = message.clone();
    start["content"] = json!([]);
    start["stopReason"] = json!("pending");
    out.say(json!({"type": "message_start", "message": start}));
    out.say(json!({"type": "message_end", "message": message}));
}

impl Pi {
    /// A process started with `args`, in `folder`, for the chat Pi's driver names `tag`.
    pub fn new(args: &[String], tag: &str, folder: Option<String>, store: Shared) -> Self {
        let after = |flag: &str| {
            let at = args.iter().position(|a| a == flag)?;
            args.get(at + 1).cloned()
        };
        let (session, resumed, missing) = if let Some(id) = after("--session") {
            let missing = !store.lock().unwrap().files.contains(&id);
            (id, true, missing)
        } else if let Some(id) = after("--session-id") {
            (id, true, false)
        } else {
            (fresh("pi-session"), false, false)
        };
        Self {
            store,
            tag: tag.to_string(),
            session,
            resumed,
            missing,
            folder,
            sends_files: has_send_file(args),
            thinking: "low".into(),
            run: None,
        }
    }

    fn finish(&mut self, words: &str, aborted: bool, out: &mut Outbox) {
        let Some(run) = self.run.take() else {
            return;
        };
        if let Some(call) = &run.tool {
            let result = json!({"content": [{"type": "text", "text": if aborted { "Command aborted" } else { "" }}]});
            out.say(
                json!({"type": "tool_execution_end", "toolCallId": call, "toolName": "bash",
                "result": result, "isError": aborted}),
            );
        }
        if aborted {
            assistant(json!([]), "aborted", out);
        } else {
            for steered in &run.steered {
                let user = json!({"role": "user", "content": [{"type": "text", "text": steered}], "timestamp": now()});
                out.say(json!({"type": "message_start", "message": user}));
                out.say(json!({"type": "message_end", "message": user}));
            }
            let mut text = format!("{words}: {}", run.text);
            for steered in &run.steered {
                text.push_str(" and ");
                text.push_str(steered);
            }
            assistant(json!([{"type": "text", "text": text}]), "stop", out);
        }
        out.say(json!({"type": "agent_end", "messages": [], "willRetry": false}));
        out.say(json!({"type": "agent_settled", "aborted": aborted}));
    }

    fn start_run(&mut self, text: &str, out: &mut Outbox) {
        self.store
            .lock()
            .unwrap()
            .files
            .insert(self.session.clone());
        out.say(json!({"type": "agent_start"}));
        let user = json!({"role": "user", "content": [{"type": "text", "text": text}], "timestamp": now()});
        out.say(json!({"type": "message_start", "message": user}));
        out.say(json!({"type": "message_end", "message": user}));
        let mut run = Run {
            text: text.to_string(),
            steered: Vec::new(),
            tool: None,
            asked: None,
            file: None,
        };
        let script = Script::of(text);
        if script == Script::File && self.sends_files {
            // The extension's tool asks the driver, in its dialog, and waits for the answer.
            let call = fresh("call");
            let args = json!({"path": FILE});
            assistant(
                json!([{"type": "toolCall", "id": call, "name": "send_file", "arguments": args}]),
                "toolUse",
                out,
            );
            out.say(json!({"type": "tool_execution_start", "toolCallId": call, "toolName": "send_file", "args": args}));
            let asked = fresh("ui");
            out.say(
                json!({"type": "extension_ui_request", "id": asked, "method": "input",
                "title": SEND_FILE_REQUEST, "placeholder": args.to_string()}),
            );
            run.asked = Some(asked);
            run.file = Some(call);
        }
        if matches!(script, Script::Sleep | Script::Die | Script::Ask) {
            let call = fresh("call");
            let args = json!({"command": format!("echo {text}")});
            assistant(
                json!([{"type": "toolCall", "id": call, "name": "bash", "arguments": args}]),
                "toolUse",
                out,
            );
            if script != Script::Ask {
                out.say(json!({"type": "tool_execution_start", "toolCallId": call, "toolName": "bash", "args": args}));
            }
            run.tool = Some(call);
        }
        if matches!(script, Script::Ask | Script::Quiz) {
            // Pi asks nobody before a tool: an extension asks, in a dialog.
            let asked = fresh("ui");
            out.say(
                json!({"type": "extension_ui_request", "id": asked, "method": "select",
                "title": format!("Which? {text}"), "options": ["Yes", "No"]}),
            );
            run.asked = Some(asked);
        }
        self.run = Some(run);
        match script {
            Script::Reply => self.finish("ok", false, out),
            Script::File if !self.sends_files => self.finish("no send_file tool", false, out),
            Script::Die => out.die(),
            _ => {}
        }
    }
}

impl Protocol for Pi {
    fn start(&mut self, out: &mut Outbox, heard: &mut Vec<Heard>) {
        if self.missing {
            out.die(); // "No session found"
            return;
        }
        heard.push(Heard::Opened {
            session: self.session.clone(),
            resumed: self.resumed,
            folder: self.folder.clone(),
        });
    }

    fn hear(&mut self, command: &Value, out: &mut Outbox, heard: &mut Vec<Heard>) {
        let kind = command["type"].as_str().unwrap_or_default();
        match kind {
            "get_state" => respond(
                command,
                Some(json!({"model": model("m1"), "thinkingLevel": self.thinking,
                    "isStreaming": self.run.is_some(), "sessionId": self.session})),
                out,
            ),
            "get_available_models" => respond(
                command,
                Some(json!({"models": [model("m1"), model("m2")]})),
                out,
            ),
            "get_available_thinking_levels" => respond(
                command,
                Some(json!({"levels": ["off", "low", "high"]})),
                out,
            ),
            "get_commands" => respond(command, Some(json!({"commands": []})), out),
            "prompt" => {
                let text = command["message"].as_str().unwrap_or_default().to_string();
                if let Some(chat) = crate::check::marked_chat(&text) {
                    let mut store = self.store.lock().unwrap();
                    store.tags.insert(chat, self.tag.clone());
                }
                heard.push(Heard::Prompt {
                    session: self.session.clone(),
                    text: text.clone(),
                    refused: false,
                });
                if let Some(run) = &mut self.run {
                    run.steered.push(text);
                    respond(command, Some(json!({"disposition": "queued"})), out);
                } else {
                    respond(command, Some(json!({"disposition": "started"})), out);
                    self.start_run(&text, out);
                }
            }
            "abort" => {
                if self.run.is_some() {
                    heard.push(Heard::Control {
                        session: Some(self.session.clone()),
                        what: "interrupt".into(),
                    });
                    self.finish("stopped", true, out);
                }
                respond(command, None, out);
            }
            "extension_ui_response" => {
                let asked = self.run.as_ref().and_then(|r| r.asked.clone());
                if asked.as_deref() != command["id"].as_str() {
                    return;
                }
                if let Some(call) = self.run.as_mut().and_then(|r| r.file.take()) {
                    // The driver's answer to its own tool: how the send went.
                    let value = command["value"].as_str().unwrap_or_default();
                    let answer: Value = serde_json::from_str(value).unwrap_or(Value::Null);
                    let text = answer["text"].as_str().unwrap_or_default().to_string();
                    let sent = answer["sent"] == true;
                    heard.push(Heard::FileResult {
                        session: self.session.clone(),
                        sent,
                        text: text.clone(),
                    });
                    let result = json!({"content": [{"type": "text", "text": text}]});
                    out.say(json!({"type": "tool_execution_end", "toolCallId": call, "toolName": "send_file",
                        "result": result, "isError": !sent}));
                    self.finish("filed", false, out);
                    return;
                }
                let text = command["value"].as_str().map(String::from);
                heard.push(Heard::Answer {
                    session: self.session.clone(),
                    allow: None,
                    text,
                });
                let tool = self.run.as_ref().and_then(|r| r.tool.clone());
                if let Some(call) = tool {
                    out.say(json!({"type": "tool_execution_start", "toolCallId": call, "toolName": "bash", "args": {}}));
                }
                self.finish("answered", false, out);
            }
            "new_session" => {
                heard.push(Heard::Control {
                    session: Some(self.session.clone()),
                    what: "new_session".into(),
                });
                self.session = fresh("pi-session");
                heard.push(Heard::Opened {
                    session: self.session.clone(),
                    resumed: false,
                    folder: self.folder.clone(),
                });
                respond(command, Some(json!({"cancelled": false})), out);
            }
            "set_model" => {
                heard.push(Heard::Control {
                    session: Some(self.session.clone()),
                    what: "set".into(),
                });
                let id = command["modelId"].as_str().unwrap_or("m1");
                respond(command, Some(model(id)), out);
            }
            "set_thinking_level" => {
                heard.push(Heard::Control {
                    session: Some(self.session.clone()),
                    what: "set".into(),
                });
                let level = command["level"].as_str().unwrap_or("low").to_string();
                if level != self.thinking {
                    self.thinking = level.clone();
                    out.say(json!({"type": "thinking_level_changed", "level": level}));
                }
                respond(command, None, out);
            }
            _ => out.reply(
                json!({"id": command["id"], "type": "response", "command": kind,
                "success": false, "error": format!("Unknown command: {kind}")}),
            ),
        }
    }
}

/// Where Pi's driver finds the fake: a listener on a Unix socket, and the relay program.
pub struct Server {
    dir: tempfile::TempDir,
    store: Shared,
    fake: Fake,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    /// Listens for Pi processes, each served by `fake`.
    pub fn start(fake: &Fake) -> std::io::Result<Self> {
        Self::serving(fake, None)
    }

    /// Listens for Pi processes, each replaying `recording`, with the fake for what it does not
    /// hold.
    pub fn replaying(
        fake: &Fake,
        recording: &str,
        progress: Arc<super::Progress>,
    ) -> std::io::Result<Self> {
        Self::serving(fake, Some((recording.to_string(), progress)))
    }

    fn serving(
        fake: &Fake,
        recording: Option<(String, Arc<super::Progress>)>,
    ) -> std::io::Result<Self> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("relay.py"), include_str!("relay.py"))?;
        let listener = UnixListener::bind(dir.path().join("pi.sock"))?;
        let store = Shared::default();
        let (shared, serving) = (store.clone(), fake.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (read, write) = stream.into_split();
                let (store, fake) = (shared.clone(), serving.clone());
                let recording = recording.clone();
                tokio::spawn(async move {
                    let mut read = BufReader::new(read);
                    let mut hello = String::new();
                    if read.read_line(&mut hello).await.is_err() {
                        return;
                    }
                    let hello: Value = serde_json::from_str(&hello).unwrap_or(Value::Null);
                    let tag = hello["chat"].as_str().unwrap_or_default().to_string();
                    let args: Vec<String> = (hello["args"].as_array().into_iter().flatten())
                        .filter_map(|a| a.as_str().map(String::from))
                        .collect();
                    let folder = hello["cwd"].as_str().map(String::from);
                    let pi = Pi::new(&args, &tag, folder, store);
                    let protocol: Box<dyn Protocol> = match &recording {
                        Some((text, progress)) => {
                            Box::new(super::Replay::new(text, pi).counting(progress.clone()))
                        }
                        None => Box::new(pi),
                    };
                    fake.serve(read, write, protocol, Some(tag));
                });
            }
        });
        Ok(Self {
            dir,
            store,
            fake: fake.clone(),
            task,
        })
    }

    /// The program Pi's driver runs, its arguments, and the folder to run it in.
    pub fn program(&self) -> (String, Vec<String>, PathBuf) {
        let relay = self.dir.path().join("relay.py");
        let socket = self.dir.path().join("pi.sock");
        let args = vec![relay.display().to_string(), socket.display().to_string()];
        ("python3".into(), args, self.dir.path().to_path_buf())
    }

    /// The chat a Pi started with `tag` serves, once a prompt has shown it.
    pub fn chat_of(&self, tag: &str) -> Option<String> {
        let store = self.store.lock().unwrap();
        (store.tags.iter())
            .find(|(_, t)| *t == tag)
            .map(|(chat, _)| chat.clone())
    }

    /// The tag of the Pi serving `chat`, once a prompt has shown it.
    pub fn tag_of(&self, chat: &str) -> Option<String> {
        self.store.lock().unwrap().tags.get(chat).cloned()
    }

    /// The Pi serving `chat` dies now.
    pub fn kill_chat(&self, chat: &str) {
        let tag = self.store.lock().unwrap().tags.get(chat).cloned();
        if let Some(tag) = tag {
            self.fake.kill_tagged(&tag);
        }
    }
}
