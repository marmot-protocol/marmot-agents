//! A connector on the core's socket, scripted: it speaks the socket protocol as a driver outside
//! the core does, in place of a real one.

use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// What the scripted connector does with a prompt.
#[derive(Clone, Copy)]
pub enum Script {
    /// Answers "echo: " and the prompt's text, in one turn.
    Echo,
    /// Starts a turn and then goes, as a connector that crashes does.
    Vanish,
}

/// The texts of the prompts a scripted connector was given, oldest first.
pub type Heard = Arc<Mutex<Vec<String>>>;

pub async fn send(stream: &mut tokio::net::unix::OwnedWriteHalf, message: Value) {
    let line = format!("{message}\n");
    stream.write_all(line.as_bytes()).await.unwrap();
}

/// Connects, gives `hello`, and gives back the core's answer to it.
pub async fn connect(socket: &Path, hello: Value) -> (Value, UnixStream) {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (read, mut write) = stream.into_split();
    send(&mut write, hello).await;
    let mut lines = BufReader::new(read).lines();
    let answer: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    let stream = lines.into_inner().into_inner().reunite(write).unwrap();
    (answer, stream)
}

/// Runs the scripted connector on a connection until the core or the script ends it. It takes
/// every request and does nothing with any but a prompt, whose text it adds to `heard`. Its item
/// ids start with `name`, so two connections never reuse one: a reused id would edit the earlier
/// item.
pub async fn run(stream: UnixStream, script: Script, name: &str, heard: Heard) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut turns = 0;
    while let Ok(Some(line)) = lines.next_line().await {
        let request: Value = serde_json::from_str(&line).unwrap();
        if request.get("id").is_some() {
            send(
                &mut write,
                json!({"jsonrpc": "2.0", "id": request["id"], "result": {}}),
            )
            .await;
        }
        if request["method"] != "prompt" {
            continue;
        }
        turns += 1;
        let params = &request["params"];
        let text = params["text"].as_str().unwrap().to_string();
        heard.lock().unwrap().push(text.clone());
        let event = |item: String, kind: &str, state: &str, text: String, body: Value| {
            json!({"jsonrpc": "2.0", "method": "event", "params": {
                "item": item, "type": kind, "state": state, "title": kind, "text": text,
                "body": body, "session": "s1", "turn": format!("{name}-t{turns}"),
                "chat": params["chat"]["id"], "prompts": [params["message"]],
            }})
        };
        let turn = json!({"started_by": "user"});
        let running = event(
            format!("{name}-turn{turns}"),
            "turn",
            "running",
            "Working".into(),
            turn.clone(),
        );
        send(&mut write, running).await;
        if let Script::Vanish = script {
            return;
        }
        let words = format!("echo: {text}");
        let said = event(
            format!("{name}-msg{turns}"),
            "message",
            "done",
            words,
            json!({}),
        );
        send(&mut write, said).await;
        let done = event(
            format!("{name}-turn{turns}"),
            "turn",
            "done",
            "Done".into(),
            turn,
        );
        send(&mut write, done).await;
    }
}
