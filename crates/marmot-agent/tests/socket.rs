//! The socket end to end: a connector speaks the protocol over the core's real Unix socket, and a
//! person talks to it through MDK's test relay. The connector here is a script; the Claude Code
//! driver will be the first real one.

mod common;

use common::{World, eventually};
use marmot_agent::run::serve;
use marmot_agent::socket::{Endpoint, PROTOCOL};
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

/// What the scripted connector does with a prompt.
#[derive(Clone, Copy)]
enum Script {
    /// Answers "echo: " and the prompt's text, in one turn.
    Echo,
    /// Starts a turn and then goes, as a connector that crashes does.
    Vanish,
}

/// The hello a connector sends.
fn hello(token: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {
        "protocol": PROTOCOL,
        "connector": {"name": "script", "version": "0.0.1"},
        "harness": {"name": "script", "version": "1"},
        "token": token,
        "capabilities": {
            "interrupt": false, "mid_turn": "queue", "sessions": "core", "resume": false,
            "approvals": false, "files_in": "none", "approval_commands": [],
        },
    }})
}

async fn send(stream: &mut tokio::net::unix::OwnedWriteHalf, message: Value) {
    let line = format!("{message}\n");
    stream.write_all(line.as_bytes()).await.unwrap();
}

/// Connects, gives the hello, and gives back the core's answer to it.
async fn connect(socket: &Path, token: &str) -> (Value, UnixStream) {
    let stream = UnixStream::connect(socket).await.unwrap();
    let (read, mut write) = stream.into_split();
    send(&mut write, hello(token)).await;
    let mut lines = BufReader::new(read).lines();
    let answer: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    let stream = lines.into_inner().into_inner().reunite(write).unwrap();
    (answer, stream)
}

/// Runs the scripted connector on a connection until the core or the script ends it. Its item ids
/// start with `name`, so two connections never reuse one: a reused id would edit the earlier item.
async fn run_connector(stream: UnixStream, script: Script, name: &str) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut turns = 0;
    while let Ok(Some(line)) = lines.next_line().await {
        let request: Value = serde_json::from_str(&line).unwrap();
        send(
            &mut write,
            json!({"jsonrpc": "2.0", "id": request["id"], "result": {}}),
        )
        .await;
        if request["method"] != "prompt" {
            continue;
        }
        turns += 1;
        let params = &request["params"];
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
        let words = format!("echo: {}", params["text"].as_str().unwrap());
        send(
            &mut write,
            event(
                format!("{name}-msg{turns}"),
                "message",
                "done",
                words,
                json!({}),
            ),
        )
        .await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connector_on_the_socket_drives_the_agent() {
    let world = World::new("name = \"Script\"\n[connector]\n").await;
    world.runtime.start().await.unwrap();
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::in_home(home.path());
    let (listener, token) = endpoint.listen().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let (marmot, config) = (world.marmot.clone(), world.config.clone());
    let store = world.state.path().to_path_buf();
    let served_token = token.clone();
    let task = tokio::spawn(async move {
        let stop = async {
            let _ = stopped.await;
        };
        serve(marmot, &config, &store, listener, served_token, stop).await
    });

    // A wrong token is refused, and the core keeps listening.
    let (refused, _) = connect(&endpoint.socket, "not the token").await;
    assert!(refused.get("error").is_some(), "{refused}");

    let (welcome, stream) = connect(&endpoint.socket, &token).await;
    assert_eq!(welcome["result"]["agent"]["name"], "Script");
    let first = tokio::spawn(run_connector(stream, Script::Vanish, "first"));
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let hex = world.joined(&group).await;

    // A connector that goes in the middle of a turn: the core closes the turn as interrupted.
    world
        .runtime
        .send_message(&world.person, &group, b"first".to_vec())
        .await
        .unwrap();
    first.await.unwrap();
    eventually(|| {
        world.items(&hex).into_iter().find(|(_, read)| {
            matches!(read.item.event.body(), Body::Turn(_))
                && read.item.event.header().state == State::Cancelled
        })
    })
    .await;

    // The next connector carries on.
    let (_, stream) = connect(&endpoint.socket, &token).await;
    let second = tokio::spawn(run_connector(stream, Script::Echo, "second"));
    world
        .runtime
        .send_message(&world.person, &group, b"second".to_vec())
        .await
        .unwrap();
    world.words(&hex, "echo: second").await;
    // Each turn has its own row: the first stays interrupted, the second finished. The words can
    // arrive before the turn's last version, so wait for it.
    let turns = || -> Vec<(String, State)> {
        let mut latest = std::collections::BTreeMap::new();
        // Versions sent in the same second come back in any order: the highest one counts.
        for (_, read) in world.items(&hex) {
            if matches!(read.item.event.body(), Body::Turn(_)) {
                let header = read.item.event.header();
                let entry = latest
                    .entry(header.item.as_str().to_string())
                    .or_insert((0, header.state));
                if read.item.version > entry.0 {
                    *entry = (read.item.version, header.state);
                }
            }
        }
        latest
            .into_iter()
            .map(|(item, (_, state))| (item, state))
            .collect()
    };
    let want = vec![
        ("first-turn1".to_string(), State::Cancelled),
        ("second-turn1".to_string(), State::Done),
    ];
    eventually(|| (turns() == want).then_some(())).await;

    let _ = stop.send(());
    task.await.unwrap().unwrap();
    second.abort();
    world.runtime.shutdown().await;
}
