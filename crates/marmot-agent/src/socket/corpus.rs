//! The socket's schema and its examples, as committed in `fixtures/socket`.
//!
//! The schema is made from the message types in [`wire`](super::wire). The examples from the core
//! are what [`SocketDriver`] writes on a real socket; those from a connector are each read by it,
//! and must come back out the same, so the core is shown to read every field of each. The Claude
//! Code and OpenClaw packages and the Hermes plugin check themselves against both files.
//!
//! When either test fails because the types changed on purpose, write both files again with
//! `MARMOT_AGENT_WRITE_SOCKET=1 cargo test -p marmot-agent --lib socket::corpus`, and commit them.

use super::wire::{self, FromConnector, FromCore};
use super::*;
use marmot_agent_driver::{
    Action, Attachment, Chat, ChatId, Given, Person, PersonId, Quoted, Reply,
};
use schemars::generate::SchemaSettings;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::mpsc;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// Where the files are, from this crate.
const FOLDER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/socket");
/// Set to write the files again.
const WRITE: &str = "MARMOT_AGENT_WRITE_SOCKET";

/// The schema: every message from the core, and every message from a connector.
fn schema() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let from_core = generator.subschema_for::<FromCore>();
    let from_connector = generator.subschema_for::<FromConnector>();
    let definitions = generator.take_definitions(true);
    let mut schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": PROTOCOL,
        "description": "Every message on the core's local socket, made from the types in \
            crates/marmot-agent/src/socket/wire.rs. A message from the core is a FromCore; one from \
            a connector is a FromConnector. The core passes over fields it does not know; this \
            schema admits none, so a connector is checked against exactly what the core reads. \
            Written by the test in crates/marmot-agent/src/socket/corpus.rs; do not edit it by hand.",
        "anyOf": [from_core, from_connector],
        "$defs": definitions,
    });
    close(&mut schema, false);
    schema
}

/// Admits no field an object does not name: `unevaluatedProperties` on every object schema with
/// properties, except one that is a branch of another's `oneOf`, `anyOf` or `allOf`, whose fields
/// count as the parent's.
fn close(schema: &mut Value, branch: bool) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    let named = object.contains_key("properties");
    if named && !branch && !object.contains_key("additionalProperties") {
        object.insert("unevaluatedProperties".into(), false.into());
    }
    for (key, value) in object.iter_mut() {
        match key.as_str() {
            "oneOf" | "anyOf" | "allOf" => {
                for each in value.as_array_mut().into_iter().flatten() {
                    close(each, named || branch);
                }
            }
            "properties" | "$defs" => {
                for each in value
                    .as_object_mut()
                    .into_iter()
                    .flat_map(|m| m.values_mut())
                {
                    close(each, false);
                }
            }
            "items" | "additionalProperties" => close(value, false),
            _ => {}
        }
    }
}

/// A file as committed, as text.
fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    text
}

/// Checks `name` in the folder holds `value`, or writes it there when asked to.
fn committed(name: &str, value: &Value) {
    let path = Path::new(FOLDER).join(name);
    let made = pretty(value);
    if std::env::var_os(WRITE).is_some() {
        std::fs::create_dir_all(FOLDER).unwrap();
        std::fs::write(&path, made).unwrap();
        return;
    }
    let held = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        held == made,
        "fixtures/socket/{name} is not what the socket's types make. If they changed on purpose, \
         write it again with `{WRITE}=1 cargo test -p marmot-agent --lib socket::corpus` and commit \
         it; the Claude Code, OpenClaw and Hermes checks then show which client must change too."
    );
}

#[test]
fn the_committed_schema_is_the_one_the_types_make() {
    committed("schema.json", &schema());
}

/// Whether the schema names the field at `path` under `node`, in any of its branches.
fn names(schema: &Value, node: &Value, path: &[&str]) -> bool {
    let mut node = node;
    while let Some(name) = node["$ref"].as_str() {
        node = &schema["$defs"][name.trim_start_matches("#/$defs/")];
    }
    let Some((first, rest)) = path.split_first() else {
        return true;
    };
    let branches = ["anyOf", "oneOf", "allOf"]
        .iter()
        .filter_map(|k| node[*k].as_array())
        .flatten();
    if branches.into_iter().any(|b| names(schema, b, path)) {
        return true;
    }
    match *first {
        "[]" => node
            .get("items")
            .is_some_and(|items| names(schema, items, rest)),
        field => node["properties"]
            .get(field)
            .is_some_and(|f| names(schema, f, rest)),
    }
}

// What a connector sends that the types do not hold is listed, each an open question (Q-01), and
// the checks in the other languages allow only those. One the types come to hold comes off the list.
#[test]
fn what_the_core_passes_over_is_not_in_its_types() {
    let schema = schema();
    let text = std::fs::read_to_string(Path::new(FOLDER).join("passed-over.json")).unwrap();
    let listed: Value = serde_json::from_str(&text).unwrap();
    let fields = listed["fields"].as_array().unwrap();
    assert!(!fields.is_empty());
    for entry in fields {
        let (kind, field) = (
            entry["type"].as_str().unwrap(),
            entry["field"].as_str().unwrap(),
        );
        let def: String = kind
            .split('_')
            .map(|w| w[..1].to_uppercase() + &w[1..])
            .collect();
        let body = &schema["$defs"][&def];
        assert!(body.is_object(), "{kind} is not an item type");
        let path = field.replace("[]", ".[]");
        let path: Vec<&str> = path.split('.').filter(|p| !p.is_empty()).collect();
        assert!(
            !names(&schema, body, &path),
            "the types hold {kind}'s {field} now: take it off fixtures/socket/passed-over.json, and \
             out of Q-01 in docs/open-questions.md"
        );
        // The field's parent is one the types hold, so the path is not mistyped.
        assert!(
            names(&schema, body, &path[..path.len() - 1]),
            "{kind}'s {field}"
        );
    }
}

// ------------------------------------------------------------------------------ examples

/// One example: a name, which side sends it, and the message.
struct Example {
    name: &'static str,
    from: &'static str,
    message: Value,
}

fn from_core(name: &'static str, message: Value) -> Example {
    Example {
        name,
        from: "core",
        message,
    }
}

fn from_connector(name: &'static str, message: Value) -> Example {
    Example {
        name,
        from: "connector",
        message,
    }
}

fn capabilities() -> Value {
    json!({"interrupt": true, "mid_turn": "restart", "sessions": "core", "resume": true,
        "approvals": true, "files_in": "images", "approval_commands": []})
}

fn hello(token: &str, params: Value) -> Value {
    let mut params = params;
    params["protocol"] = PROTOCOL.into();
    params["token"] = token.into();
    json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": params})
}

/// A coding harness's hello, with the folder it works in.
fn coding_hello() -> Value {
    hello(
        TOKEN,
        json!({"connector": {"name": "marmot-agent-example", "version": "0.1.0"},
            "harness": {"name": "example", "version": "1.2.3"},
            "capabilities": capabilities(), "folder": "/work/demo"}),
    )
}

/// A gateway's hello: its own sessions, typed commands, no folder and no harness version.
fn gateway_hello() -> Value {
    hello(
        TOKEN,
        json!({"connector": {"name": "marmot-agent-gateway", "version": "0.1.0"},
            "harness": {"name": "gateway"},
            "capabilities": {"interrupt": true, "mid_turn": "queue", "sessions": "own",
                "resume": true, "approvals": true, "files_in": "none",
                "approval_commands": [{"command": "/approve", "counts": "start"},
                    {"command": "/exec", "counts": "anywhere"}]}}),
    )
}

/// The core's end of a socket, and the connector's: what the core wrote, line by line, and a way to
/// write as the connector.
struct Wire {
    driver: SocketDriver,
    written: mpsc::UnboundedReceiver<Value>,
    connector: tokio::net::unix::OwnedWriteHalf,
}

impl Wire {
    /// A connection greeted with `hello`. The welcome is the first line written.
    async fn open(hello: &Value) -> Self {
        let (core, connector) = UnixStream::pair().unwrap();
        let (read, mut connector) = connector.into_split();
        connector
            .write_all(format!("{hello}\n").as_bytes())
            .await
            .unwrap();
        let written = lines(read);
        let driver = SocketDriver::greet(core, TOKEN, "Agent")
            .await
            .expect("the hello is taken");
        Self {
            driver,
            written,
            connector,
        }
    }

    async fn send(&mut self, message: &Value) {
        self.connector
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn next(&mut self) -> Value {
        self.written.recv().await.expect("a line from the core")
    }
}

/// Every line written to `read`, as JSON.
fn lines(read: OwnedReadHalf) -> mpsc::UnboundedReceiver<Value> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                return;
            }
        }
    });
    rx
}

/// What the core answers a hello it refuses.
async fn refused_hello(hello: Value) -> Value {
    let (core, connector) = UnixStream::pair().unwrap();
    let (read, mut connector) = connector.into_split();
    connector
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .unwrap();
    let mut written = lines(read);
    assert!(SocketDriver::greet(core, TOKEN, "Agent").await.is_err());
    written.recv().await.expect("the refusal")
}

fn person(id: &str, name: &str) -> Person {
    Person {
        id: PersonId(id.into()),
        name: name.into(),
    }
}

fn dm() -> Chat {
    Chat {
        id: ChatId("c-1".into()),
        group: false,
        name: None,
        folder: None,
    }
}

fn group() -> Chat {
    Chat {
        id: ChatId("c-2".into()),
        group: true,
        name: Some("Family".into()),
        folder: Some("/work/other".into()),
    }
}

/// Every message from the core, as the socket writes it.
async fn from_the_core() -> Vec<Example> {
    let mut examples = Vec::new();
    let mut wire = Wire::open(&coding_hello()).await;
    let mut welcome = wire.next().await;
    // The core's own version changes with each release; the example keeps one.
    welcome["result"]["core"]["version"] = "0.1.0".into();
    examples.push(from_core("the welcome, in answer to a hello", welcome));

    for (name, message, code) in [
        (
            "a hello refused: another protocol",
            {
                let mut h = coding_hello();
                h["params"]["protocol"] = "marmot.agent-control.v2".into();
                h
            },
            -32001,
        ),
        (
            "a hello refused: not this agent's token",
            hello("0", json!({})),
            -32002,
        ),
        (
            "a hello refused: not a hello",
            json!({"jsonrpc": "2.0", "id": 1, "method": "event", "params": {}}),
            -32600,
        ),
        (
            "a hello refused: capabilities missing",
            {
                let mut h = coding_hello();
                h["params"].as_object_mut().unwrap().remove("capabilities");
                h
            },
            -32602,
        ),
        (
            "a hello refused: a folder that is not a whole path",
            {
                let mut h = coding_hello();
                h["params"]["folder"] = "work".into();
                h
            },
            -32602,
        ),
    ] {
        let refusal = refused_hello(message).await;
        assert_eq!(refusal["error"]["code"], code, "{name}");
        examples.push(from_core(name, refusal));
    }

    let prompts = [
        (
            "prompt: in a two-person chat",
            Prompt {
                message: MessageId("m-1".into()),
                text: "run the tests".into(),
                chat: dm(),
                sender: person("u-1", "Alice"),
                reply_to: None,
                attachments: Vec::new(),
                session: None,
            },
        ),
        (
            "prompt: a reply in a group that chose a folder, with an attachment, resuming a session",
            Prompt {
                message: MessageId("m-2".into()),
                text: "and this one?".into(),
                chat: group(),
                sender: person("u-2", "Bob"),
                reply_to: Some(Quoted {
                    message: MessageId("m-1".into()),
                    text: "run the tests".into(),
                    author: person("u-1", "Alice"),
                }),
                attachments: vec![Attachment {
                    path: "/work/inbox/a1/photo.png".into(),
                    name: "photo.png".into(),
                    media_type: "image/png".into(),
                }],
                session: Some("s-1".into()),
            },
        ),
    ];
    for (name, prompt) in prompts {
        examples.push(from_core(
            name,
            taken(&mut wire, Request::Prompt(prompt)).await,
        ));
    }

    let answers = [
        ("answer: an approval", Given::Choice("once".into())),
        (
            "answer: a question item, with options chosen and text typed",
            Given::Questions(vec![
                Reply {
                    question: "q0".into(),
                    options: vec!["o1".into()],
                    text: None,
                },
                Reply {
                    question: "q1".into(),
                    options: Vec::new(),
                    text: Some("blue".into()),
                },
            ]),
        ),
    ];
    for (n, (name, given)) in answers.into_iter().enumerate() {
        let answer = Answer {
            request: RequestId(format!("r-{n}")),
            item: "ap-2".into(),
            by: person("u-1", "Alice"),
            given,
        };
        examples.push(from_core(
            name,
            taken(&mut wire, Request::Answer(answer)).await,
        ));
    }

    let controls = [
        (
            "control: interrupt",
            Action::Interrupt { turn: "t-4".into() },
        ),
        (
            "control: a new session, ending one, in the folder the chat chose",
            Action::NewSession {
                chat: group(),
                session: Some("s-1".into()),
            },
        ),
        (
            "control: a new session, for a chat that has had none",
            Action::NewSession {
                chat: dm(),
                session: None,
            },
        ),
        (
            "control: set a setting",
            Action::Set {
                session: "s-1".into(),
                setting: "model".into(),
                value: "model-b".into(),
            },
        ),
        (
            "control: one of the harness's own actions",
            Action::Own {
                session: "s-1".into(),
                id: "compact".into(),
            },
        ),
    ];
    for (n, (name, action)) in controls.into_iter().enumerate() {
        let control = Control {
            request: RequestId(format!("r-c{n}")),
            by: person("u-1", "Alice"),
            action,
        };
        examples.push(from_core(
            name,
            taken(&mut wire, Request::Control(control)).await,
        ));
    }

    // The core answers the files a connector asks it to send.
    for (id, path) in [
        (json!(2), "/work/demo/report.pdf"),
        (json!("f-3"), "/etc/passwd"),
    ] {
        wire.send(&json!({"jsonrpc": "2.0", "id": id, "method": "send_file",
            "params": {"chat": "c-1", "path": path}}))
            .await;
    }
    let mut asked = Vec::new();
    for _ in 0..2 {
        match wire.driver.next_event().await {
            Some(Output::SendFile(file)) => asked.push(file),
            _ => panic!("a file to send"),
        }
    }
    wire.driver.file_sent(asked[0].id.clone(), Ok(())).await;
    examples.push(from_core("a file sent", wire.next().await));
    let refusal = Refusal {
        reason: Reason::NotAllowed,
        detail: Some("The file is outside the folders the agent may send files from.".into()),
    };
    wire.driver
        .file_sent(asked[1].id.clone(), Err(refusal))
        .await;
    examples.push(from_core("a file refused", wire.next().await));
    examples
}

/// A request of the core's.
enum Request {
    Prompt(Prompt),
    Answer(Answer),
    Control(Control),
}

/// The line the core writes for `request`, which the connector then takes.
async fn taken(wire: &mut Wire, request: Request) -> Value {
    let driver = &mut wire.driver;
    let asking = async move {
        match request {
            Request::Prompt(p) => driver.prompt(p).await,
            Request::Answer(a) => driver.answer(a).await,
            Request::Control(c) => driver.control(c).await,
        }
    };
    let written = &mut wire.written;
    let connector = &mut wire.connector;
    let answering = async move {
        let line = written.recv().await.expect("the request");
        let ack = json!({"jsonrpc": "2.0", "id": line["id"], "result": {}});
        connector
            .write_all(format!("{ack}\n").as_bytes())
            .await
            .unwrap();
        line
    };
    let (result, line) = tokio::join!(asking, answering);
    result.expect("taken");
    line
}

/// An `event` notification.
fn event(params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": "event", "params": params})
}

/// One event of each item type, and of the ways an event says what it follows from.
fn events() -> Vec<(&'static str, Value)> {
    vec![
        (
            "event: a turn starts, responding to a prompt",
            json!({"item": "t-4", "type": "turn", "state": "running", "title": "Working", "text": "",
                "body": {"started_by": "user"}, "session": "s-1", "turn": "t-4", "chat": "c-1",
                "prompts": ["m-1"], "native": {"kind": "turn/started", "payload": {"id": "t-4"}}}),
        ),
        (
            "event: a turn ends, with what it used, settling an interrupt",
            json!({"item": "t-4", "type": "turn", "state": "cancelled", "title": "Stopped", "text": "",
                "body": {"started_by": "user", "duration_ms": 1200,
                    "usage": {"input_tokens": 10, "output_tokens": 2}},
                "session": "s-1", "turn": "t-4", "chat": "c-1", "settles": ["r-c0"]}),
        ),
        (
            "event: the agent's words",
            json!({"item": "msg-5", "type": "message", "state": "done", "title": "",
                "text": "All 42 tests pass.", "body": {"phase": "final"}, "session": "s-1",
                "turn": "t-4", "chat": "c-1"}),
        ),
        (
            "event: a subagent's words",
            json!({"item": "msg-6", "type": "message", "state": "done", "title": "", "text": "Found it.",
                "body": {"phase": "commentary", "parent": "task-1"}, "session": "s-1", "turn": "t-4",
                "chat": "c-1"}),
        ),
        (
            "event: reasoning",
            json!({"item": "r-1", "type": "reasoning", "state": "done", "title": "Thinking",
                "text": "Look at the tests first.", "body": {}, "session": "s-1", "turn": "t-4",
                "chat": "c-1"}),
        ),
        (
            "event: a command that ran",
            json!({"item": "cmd-9", "type": "tool", "state": "done", "title": "Ran a command",
                "text": "cargo test", "body": {"kind": "command", "name": "shell",
                    "command": "cargo test", "cwd": "/work/demo", "exit_code": 0, "duration_ms": 900,
                    "output": "ok"}, "session": "s-1", "turn": "t-4", "chat": "c-1"}),
        ),
        (
            "event: an edit, and a tool that failed",
            json!({"item": "ed-1", "type": "tool", "state": "failed", "title": "Edited a file",
                "text": "notes.txt", "body": {"kind": "edit", "name": "apply_patch",
                    "changes": [{"path": "/work/demo/notes.txt", "diff": "+hello", "change": "add"}],
                    "error": "disk full"}, "session": "s-1", "turn": "t-4", "chat": "c-1"}),
        ),
        (
            "event: any other tool, with what it was called with",
            json!({"item": "mcp-1", "type": "tool", "state": "running", "title": "Using a tool",
                "text": "lookup", "body": {"kind": "mcp", "name": "lookup", "query": "weather",
                    "path": "/work/demo/a.png", "input": {"city": "Oslo", "days": [1, 2]}},
                "session": "s-1", "turn": "t-4", "chat": "c-1"}),
        ),
        (
            "event: an approval waits",
            json!({"item": "ap-2", "type": "approval", "state": "waiting", "title": "Allow this command?",
                "text": "cargo test", "body": {"subject": "cmd-9", "choices": [
                    {"id": "once", "label": "Allow once", "effect": "allow"},
                    {"id": "deny", "label": "Deny", "effect": "deny"}]},
                "session": "s-1", "turn": "t-4", "chat": "c-1"}),
        ),
        (
            "event: an approval answered, settling the answer",
            json!({"item": "ap-2", "type": "approval", "state": "done", "title": "Allow this command?",
                "text": "Answered: Allow once", "body": {"subject": "cmd-9", "choices": [
                    {"id": "once", "label": "Allow once", "effect": "allow"},
                    {"id": "deny", "label": "Deny", "effect": "deny"}], "answer": {"choice": "once"}},
                "session": "s-1", "turn": "t-4", "chat": "c-1", "settles": ["r-0"]}),
        ),
        (
            "event: questions wait, one of them secret",
            json!({"item": "q-1", "type": "question", "state": "waiting", "title": "A question",
                "text": "Which colour? And the password?", "body": {"answer_via": "request",
                    "questions": [
                        {"id": "q0", "header": "Colour", "text": "Which colour?", "options": [
                            {"id": "o0", "label": "Red", "description": "Warm"},
                            {"id": "o1", "label": "Blue"}], "multiple": false, "allow_text": true},
                        {"id": "q1", "text": "The password?", "options": [], "multiple": false,
                            "allow_text": true, "secret": true}]},
                "session": "s-1", "turn": "t-4", "chat": "c-1",
                "native": {"kind": "request", "payload": {"questions": 2}}}),
        ),
        (
            "event: questions answered, the secret one by name only",
            json!({"item": "q-1", "type": "question", "state": "done", "title": "A question",
                "text": "Answered", "body": {"answer_via": "request", "subject": "cmd-9",
                    "questions": [
                        {"id": "q0", "text": "Which colour?", "options": [{"id": "o1", "label": "Blue"}],
                            "multiple": true, "allow_text": false},
                        {"id": "q1", "text": "The password?", "options": [], "multiple": false,
                            "allow_text": true, "secret": true}],
                    "answer": [{"question": "q0", "options": ["o1"]}, {"question": "q1"}]},
                "session": "s-1", "turn": "t-4", "chat": "c-1", "settles": ["r-1"]}),
        ),
        (
            "event: a plan as steps",
            json!({"item": "plan-1", "type": "plan", "state": "running", "title": "Plan",
                "text": "1. Read\n2. Fix", "body": {"text": "Read, then fix", "entries": [
                    {"text": "Read", "state": "done"}, {"text": "Fix", "state": "running"},
                    {"text": "Test", "state": "pending"}]},
                "session": "s-1", "turn": "t-4", "chat": "c-1"}),
        ),
        (
            "event: a task beside the turn",
            json!({"item": "task-1", "type": "task", "state": "running", "title": "A subagent",
                "text": "Searching", "body": {"kind": "subagent", "description": "Searching",
                    "subject": "cmd-9"}, "session": "s-1", "turn": "t-4", "chat": "c-1"}),
        ),
        (
            "event: a notice outside any turn",
            json!({"item": "n-1", "type": "notice", "state": "done", "title": "Waiting",
                "text": "Waiting 5 s", "body": {"kind": "waiting", "duration_ms": 5000,
                    "reason": "rate limited"}, "chat": "c-1"}),
        ),
        (
            "event: an error, with no chat named",
            json!({"item": "e-1", "type": "error", "state": "failed", "title": "Failed",
                "text": "The model refused.", "body": {"request": "turn/start", "code": 429,
                    "will_retry": true}, "session": "s-1"}),
        ),
        (
            "event: a session, with its settings, actions and commands, settling a set",
            json!({"item": "s-1", "type": "session", "state": "running", "title": "Session",
                "text": "", "body": {"harness_version": "1.2.3", "settings": [
                    {"id": "model", "label": "Model", "value": "model-b",
                        "options": ["model-a", "model-b"]},
                    {"id": "permission_mode", "label": "Permission mode", "approve": true}],
                    "actions": [{"id": "compact", "label": "Compact", "description": "Shorten"}],
                    "commands": [{"name": "review", "description": "Review a change", "hint": "[file]"},
                        {"name": "clear", "hint": ""}]},
                "session": "s-1", "chat": "c-1", "settles": ["r-c3"]}),
        ),
    ]
}

/// Every message from a connector, each read by the core and found to hold exactly what it says.
async fn from_a_connector() -> Vec<Example> {
    let mut examples = Vec::new();
    for (name, hello) in [
        ("hello: a coding harness, with its folder", coding_hello()),
        (
            "hello: a gateway, with typed commands and no folder",
            gateway_hello(),
        ),
    ] {
        let wire = Wire::open(&hello).await;
        let params = &hello["params"];
        let read = wire::HelloParams::deserialize(params).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), *params, "{name}");
        assert_eq!(wire.driver.harness, Harness::from(read.harness.clone()));
        assert_eq!(
            wire.driver.capabilities,
            Capabilities::from(read.capabilities.clone())
        );
        assert_eq!(wire.driver.folder, read.folder.map(PathBuf::from));
        examples.push(from_connector(name, hello));
    }

    let mut wire = Wire::open(&coding_hello()).await;
    let _welcome = wire.next().await;
    for (name, params) in events() {
        let message = event(params.clone());
        wire.send(&message).await;
        let emitted = wire
            .driver
            .next_event()
            .await
            .and_then(Output::into_event)
            .unwrap_or_else(|| panic!("{name}: the core reads it"));
        // The event, as the connector sent it: the core's own fields out, what it follows from in.
        let mut back = serde_json::to_value(&emitted.event).unwrap();
        for filled in wire::CORE_FILLS {
            back.as_object_mut().unwrap().remove(filled);
        }
        let extras = wire::EventExtras {
            chat: emitted.chat.map(|c| c.0),
            prompts: emitted.prompts.into_iter().map(|m| m.0).collect(),
            settles: emitted.settles.into_iter().map(|r| r.0).collect(),
        };
        let extras = serde_json::to_value(extras).unwrap();
        back.as_object_mut()
            .unwrap()
            .extend(extras.as_object().unwrap().clone());
        assert_eq!(back, params, "{name}: the core reads every field");
        examples.push(from_connector(name, message));
    }

    for (name, id, path) in [
        (
            "send_file: a numbered request",
            json!(2),
            "/work/demo/report.pdf",
        ),
        (
            "send_file: a request with an id that is text",
            json!("f-3"),
            "/work/demo/b.txt",
        ),
    ] {
        let message = json!({"jsonrpc": "2.0", "id": id, "method": "send_file",
            "params": {"chat": "c-1", "path": path}});
        wire.send(&message).await;
        let Some(Output::SendFile(file)) = wire.driver.next_event().await else {
            panic!("{name}: a file to send");
        };
        assert_eq!(
            (file.chat.0.as_str(), file.path.as_path()),
            ("c-1", Path::new(path))
        );
        examples.push(from_connector(name, message));
    }

    // The connector's answers to a request of the core's.
    let answers = [
        ("a request taken", json!({"result": {}}), Ok(())),
        (
            "a request refused, with words for the chat",
            json!({"error": {"code": -32000, "message": "busy",
                "data": {"reason": "busy", "detail": "A turn is running."}}}),
            Err(Refusal {
                reason: Reason::Busy,
                detail: Some("A turn is running.".into()),
            }),
        ),
        (
            "a request refused",
            json!({"error": {"code": -32000, "message": "unknown_item",
                "data": {"reason": "unknown_item"}}}),
            Err(Refusal::new(Reason::UnknownItem)),
        ),
    ];
    for (name, answer, expected) in answers {
        let control = Control {
            request: RequestId("r-9".into()),
            by: person("u-1", "Alice"),
            action: Action::Interrupt { turn: "t-4".into() },
        };
        let driver = &mut wire.driver;
        let written = &mut wire.written;
        let connector = &mut wire.connector;
        let reply = async move {
            let line = written.recv().await.expect("the request");
            let mut message = json!({"jsonrpc": "2.0", "id": line["id"]});
            message
                .as_object_mut()
                .unwrap()
                .extend(answer.as_object().unwrap().clone());
            connector
                .write_all(format!("{message}\n").as_bytes())
                .await
                .unwrap();
            message
        };
        let (result, message) = tokio::join!(driver.control(control), reply);
        assert_eq!(result, expected, "{name}");
        examples.push(from_connector(name, message));
    }
    examples
}

#[tokio::test]
async fn the_committed_examples_are_what_the_core_writes_and_reads() {
    let mut examples = from_the_core().await;
    examples.extend(from_a_connector().await);
    // Each is a message its own side's type reads, and writes back the same.
    for example in &examples {
        let message = &example.message;
        let back = if example.from == "core" {
            round_trip_from_core(message)
        } else {
            round_trip_from_connector(message)
        };
        assert_eq!(back.as_ref(), Some(message), "{}", example.name);
    }
    let corpus = json!({
        "description": "Examples of every message on the core's socket: those from the core as it \
            writes them, those from a connector as the core reads them. Written by the test in \
            crates/marmot-agent/src/socket/corpus.rs, which checks them; do not edit them by hand.",
        "examples": examples.iter().map(|e| json!({
            "name": e.name, "from": e.from, "message": e.message,
        })).collect::<Vec<_>>(),
    });
    committed("examples.json", &corpus);
}

/// A message from the core, read as the type the core writes it from, and written again.
fn round_trip_from_core(message: &Value) -> Option<Value> {
    fn again<T: serde::de::DeserializeOwned + Serialize>(message: &Value) -> Option<Value> {
        serde_json::to_value(T::deserialize(message).ok()?).ok()
    }
    if message.get("method").is_some() {
        again::<wire::CoreRequestMessage>(message)
    } else if message.get("error").is_some() {
        again::<wire::ErrorMessage>(message)
    } else if message["result"].get("agent").is_some() {
        again::<wire::WelcomeMessage>(message)
    } else {
        again::<wire::TakenMessage>(message)
    }
}

/// A message from a connector, read through the types the core reads it with, and written again.
/// An event's own fields are the events crate's, which the test above reads back.
fn round_trip_from_connector(message: &Value) -> Option<Value> {
    fn again<T: serde::de::DeserializeOwned + Serialize>(message: &Value) -> Option<Value> {
        serde_json::to_value(T::deserialize(message).ok()?).ok()
    }
    let mut back = message.clone();
    match message["method"].as_str() {
        Some("hello") => back["params"] = again::<wire::HelloParams>(&message["params"])?,
        Some("send_file") => back["params"] = again::<wire::SendFileParams>(&message["params"])?,
        Some("event") => {}
        _ if message.get("error").is_some() => back = again::<wire::ErrorMessage>(message)?,
        _ => back = again::<wire::TakenMessage>(message)?,
    }
    Some(back)
}
