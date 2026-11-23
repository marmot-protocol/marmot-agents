//! The socket end to end: a connector speaks the protocol over the core's real Unix socket, and a
//! person talks to it through MDK's test relay. The connector here is a script; the Claude Code
//! driver will be the first real one.

mod common;

use common::connector::{self, Heard, Script, send};
use common::{World, eventually};
use marmot_agent::marmot::now;
use marmot_agent::run::serve;
use marmot_agent::socket::{Endpoint, PROTOCOL};
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

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

/// Connects, gives the hello, and gives back the core's answer to it.
async fn connect(socket: &Path, token: &str) -> (Value, UnixStream) {
    connector::connect(socket, hello(token)).await
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
        serve(marmot, &config, &store, listener, served_token, now(), stop).await
    });

    // A wrong token is refused, and the core keeps listening.
    let (refused, _) = connect(&endpoint.socket, "not the token").await;
    assert!(refused.get("error").is_some(), "{refused}");

    let (welcome, stream) = connect(&endpoint.socket, &token).await;
    assert_eq!(welcome["result"]["agent"]["name"], "Script");
    let first = tokio::spawn(connector::run(
        stream,
        Script::Vanish,
        "first",
        Heard::default(),
    ));
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
    let second = tokio::spawn(connector::run(
        stream,
        Script::Echo,
        "second",
        Heard::default(),
    ));
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
    // Both connectors work in session `s1`, which goes on across them, and so do its numbers: an app
    // orders a second's items by them, so the second connector's must come after the first's (D-059).
    let seqs = |connector: &str| -> Vec<u64> {
        (world.items(&hex).into_iter())
            .filter(|(_, read)| {
                read.item
                    .event
                    .header()
                    .item
                    .as_str()
                    .starts_with(connector)
            })
            .map(|(_, read)| read.item.event.header().seq)
            .collect()
    };
    let (before, after) = (seqs("first-"), seqs("second-"));
    assert!(
        after.iter().min() > before.iter().max(),
        "the first connector's numbers {before:?}, the second's {after:?}"
    );

    let _ = stop.send(());
    task.await.unwrap().unwrap();
    second.abort();
    world.runtime.shutdown().await;
}

// The core numbers a connector's events in each session. An event it cannot read is passed over
// and takes no number, so the numbers it gives have no gap (G12).
#[tokio::test]
async fn an_event_the_core_cannot_read_takes_no_number() {
    use marmot_agent::socket::SocketDriver;
    use marmot_agent_driver::{Driver, Output};
    let (core_side, connector) = UnixStream::pair().unwrap();
    let (read, mut write) = connector.into_split();
    let connector = tokio::spawn(async move {
        send(&mut write, hello("token")).await;
        let mut lines = BufReader::new(read).lines();
        let _ = lines.next_line().await;
        let event = |state: &str| {
            json!({"jsonrpc": "2.0", "method": "event", "params": {
                "item": "i1", "type": "turn", "state": state, "title": "Turn", "text": "",
                "body": {"started_by": "user"}, "session": "s1", "turn": "t1", "chat": "c-1",
            }})
        };
        // No such state: the core cannot read it.
        send(&mut write, event("sleeping")).await;
        send(&mut write, event("running")).await;
        (write, lines)
    });
    let mut driver = SocketDriver::greet(core_side, "token", "agent")
        .await
        .unwrap();
    let Some(Output::Event(emitted)) = driver.next_event().await else {
        panic!("no event")
    };
    assert_eq!(emitted.event.header().state, State::Running);
    assert_eq!(emitted.event.header().seq, 1, "the first event read");
    drop(connector.await.unwrap());
}

// A connector may state, in its hello, the folder its harness works in when a chat chose none; the
// core then hands it each chat's chosen folder on a prompt and on a new session, and only when the
// chat chose one (D-101). A folder that is not a whole path is refused, and a hello with none is
// taken as before.
#[tokio::test]
async fn a_connector_states_its_folder_and_is_given_each_chats() {
    use marmot_agent::socket::SocketDriver;
    use marmot_agent_driver::{
        Action, Chat, Control, Driver, MessageId, Person, Prompt, RequestId,
    };
    let greet = |folder: Option<Value>| async move {
        let (core_side, connector) = UnixStream::pair().unwrap();
        let (read, mut write) = connector.into_split();
        let mut said = hello("token");
        if let Some(folder) = folder {
            said["params"]["folder"] = folder;
        }
        send(&mut write, said).await;
        let greeted = SocketDriver::greet(core_side, "token", "agent").await;
        (greeted, BufReader::new(read).lines(), write)
    };
    let (greeted, ..) = greet(Some(json!("relative/work"))).await;
    assert!(greeted.is_err(), "a folder that is not whole");
    let (greeted, ..) = greet(None).await;
    assert_eq!(greeted.unwrap().folder(), None);

    let (greeted, mut lines, mut write) = greet(Some(json!("/srv/work"))).await;
    let mut driver = greeted.unwrap();
    assert_eq!(driver.folder(), Some("/srv/work".into()));
    let _welcome = lines.next_line().await.unwrap().unwrap();
    let chat = |folder: Option<&str>| Chat {
        id: "c-1".into(),
        group: false,
        name: None,
        folder: folder.map(Into::into),
    };
    let person = Person {
        id: "u-1".into(),
        name: "Alice".into(),
    };
    let answering = tokio::spawn(async move {
        let mut seen = Vec::new();
        for _ in 0..3 {
            let line = lines.next_line().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            send(
                &mut write,
                json!({"jsonrpc": "2.0", "id": request["id"], "result": {}}),
            )
            .await;
            seen.push(request["params"].clone());
        }
        seen
    });
    for folder in [None, Some("/srv/work/a")] {
        let prompt = Prompt {
            message: MessageId("m-1".into()),
            text: "hello".into(),
            chat: chat(folder),
            sender: person.clone(),
            reply_to: None,
            attachments: Vec::new(),
            session: None,
        };
        driver.prompt(prompt).await.unwrap();
    }
    let control = Control {
        request: RequestId("r-1".into()),
        by: person.clone(),
        action: Action::NewSession {
            chat: chat(Some("/srv/work/b")),
            session: None,
        },
    };
    driver.control(control).await.unwrap();
    let seen = answering.await.unwrap();
    assert_eq!(
        seen[0]["chat"].get("folder"),
        None,
        "none chosen, none named"
    );
    assert_eq!(seen[1]["chat"]["folder"], "/srv/work/a");
    assert_eq!(seen[2]["action"]["chat"]["folder"], "/srv/work/b");
}
