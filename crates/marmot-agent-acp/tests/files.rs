//! Sending a file (D-090): once the settings switch it on, the driver gives every session it opens
//! its own MCP server, with one tool, `send_file`; a call of it reaches the core as a request to send
//! a file for the chat that made the call, and the core's answer goes back as the tool's result.
//!
//! The stand-in agent here plays the agent's part over a pipe, and calls the tool over HTTP as
//! OpenCode, Goose and Cursor do, with the conformance suite's MCP client.

use marmot_agent_acp::Acp;
use marmot_agent_conformance::fake::mcp::Server;
use marmot_agent_driver::{
    Action, Chat, Control, Driver, Output, Person, Prompt, Reason, Refusal, RequestId, SendFile,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

/// The stand-in's side of the pipe.
struct Stand {
    lines: Lines<BufReader<tokio::io::ReadHalf<DuplexStream>>>,
    out: tokio::io::WriteHalf<DuplexStream>,
    /// The prompts the driver sent, in order.
    prompts: Vec<Value>,
}

impl Stand {
    async fn next(&mut self) -> Value {
        let line = self.lines.next_line().await.unwrap().expect("a message");
        serde_json::from_str(&line).unwrap()
    }

    async fn send(&mut self, message: Value) {
        self.out
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn reply(&mut self, to: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": to["id"], "result": result}))
            .await;
    }

    /// Answers the driver's `session/new` with `session`, and gives back what it asked.
    async fn opens(&mut self, session: &str) -> Value {
        let new = self.next().await;
        assert_eq!(new["method"], "session/new");
        self.reply(&new, json!({"sessionId": session})).await;
        let prompt = self.next().await;
        assert_eq!(prompt["method"], "session/prompt");
        self.prompts.push(prompt);
        new
    }
}

/// How the stand-in greets the driver, and whether the driver serves it files.
struct Agent {
    /// Whether it says it takes MCP servers over HTTP.
    http: bool,
    /// The name it gives: OpenCode's keeps one server for every session in a folder.
    name: &'static str,
    /// Whether the settings switch file sending on.
    serve: bool,
}

const STAND_IN: Agent = Agent {
    http: true,
    name: "Stand In",
    serve: true,
};

const OPENCODE: Agent = Agent {
    http: true,
    name: "OpenCode",
    serve: true,
};

const GOOSE: Agent = Agent {
    http: true,
    name: "goose",
    serve: true,
};

async fn connected(agent: Agent) -> (Acp, Stand) {
    let (driver_side, stand_side) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(stand_side);
    let mut stand = Stand {
        lines: BufReader::new(read).lines(),
        out: write,
        prompts: Vec::new(),
    };
    let (driver_read, driver_write) = tokio::io::split(driver_side);
    let (http, name) = (agent.http, agent.name);
    let greeting = tokio::spawn(async move {
        let hello = stand.next().await;
        let mut capabilities = json!({"loadSession": true});
        if http {
            capabilities["mcpCapabilities"] = json!({"http": true, "sse": false});
        }
        let agent = json!({"protocolVersion": 1, "agentCapabilities": capabilities,
                           "agentInfo": {"name": name, "version": "1.0.0"}});
        stand.reply(&hello, agent).await;
        stand
    });
    let mut acp = Acp::connect(driver_read, driver_write, "/work/demo", "standin")
        .await
        .unwrap();
    if agent.serve {
        let served = acp.serve_files().await.unwrap();
        assert_eq!(served, agent.http, "served only to an agent that takes it");
    }
    (acp, greeting.await.unwrap())
}

fn chat(id: &str, folder: Option<&str>) -> Chat {
    Chat {
        id: id.into(),
        group: false,
        name: None,
        folder: folder.map(PathBuf::from),
    }
}

fn prompt(chat_id: &str, message: &str, folder: Option<&str>, session: Option<&str>) -> Prompt {
    Prompt {
        message: message.into(),
        text: "send it".into(),
        chat: chat(chat_id, folder),
        sender: Person {
            id: "u-1".into(),
            name: "Alice".into(),
        },
        reply_to: None,
        attachments: Vec::new(),
        session: session.map(Into::into),
    }
}

/// The server a session request handed the agent, checked for its shape: its address and token.
fn server_of(request: &Value) -> (Server, String) {
    let servers = request["params"]["mcpServers"].as_array().expect("a list");
    assert_eq!(servers.len(), 1, "one server: {servers:?}");
    let server = &servers[0];
    assert_eq!(server["type"], "http");
    assert_eq!(server["name"], "marmot");
    let header = &server["headers"][0];
    assert_eq!(header["name"], "Authorization");
    let token = header["value"].as_str().unwrap();
    let token = token.strip_prefix("Bearer ").expect("a bearer token");
    let mut found = Server::from_params(&request["params"]);
    (found.remove(0), token.to_string())
}

/// What an agent does to call the tool: greet the server, list its tools, call it with `path` and
/// `meta`. Gives back the call's result.
async fn call_tool(server: &Server, path: &str, meta: Value) -> Value {
    let meta = (!meta.is_null()).then_some(meta);
    server
        .call("send_file", json!({"path": path}), meta)
        .await
        .expect("the server answers the call")
}

/// The text of a tool's result, and whether it is an error.
fn said(result: &Value) -> (String, bool) {
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    (text.to_string(), result["isError"] == true)
}

/// The next request to send a file, skipping events.
async fn next_file(acp: &mut Acp) -> SendFile {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(5), acp.next_event()).await;
        match next
            .expect("a file within the time")
            .expect("the driver runs")
        {
            Output::SendFile(file) => return file,
            Output::Event(_) => {}
        }
    }
}

/// Reads what the agent has said until it says nothing for a moment. No file may be asked.
async fn settle(acp: &mut Acp) {
    while let Ok(Some(output)) =
        tokio::time::timeout(Duration::from_millis(100), acp.next_event()).await
    {
        assert!(matches!(output, Output::Event(_)), "a file was asked");
    }
}

/// The call is refused at once, and nothing is handed to the core.
async fn refused(acp: &mut Acp, server: Server, meta: Value) -> String {
    let call = tokio::spawn(async move { call_tool(&server, "notes.txt", meta).await });
    let handed = async {
        loop {
            match acp.next_event().await {
                Some(Output::SendFile(_)) => return true,
                Some(Output::Event(_)) => {}
                None => return false,
            }
        }
    };
    let result = tokio::select! {
        result = call => result.unwrap(),
        _ = handed => panic!("a file was handed to the core"),
    };
    let (text, error) = said(&result);
    assert!(error && text.starts_with("Not sent"), "{text}");
    text
}

/// Two chats open sessions `s1` and `s2` in that order, chat 1 in `folder`, chat 2 in the driver's.
async fn two_chats(acp: &mut Acp, stand: Stand, folder: Option<&str>) -> (Stand, [Value; 2]) {
    let agent = tokio::spawn(async move {
        let mut stand = stand;
        let first = stand.opens("s1").await;
        let second = stand.opens("s2").await;
        (stand, [first, second])
    });
    acp.prompt(prompt("c-1", "m-1", folder, None))
        .await
        .unwrap();
    acp.prompt(prompt("c-2", "m-2", None, None)).await.unwrap();
    agent.await.unwrap()
}

#[tokio::test]
async fn each_session_is_given_the_drivers_server_with_its_chats_own_token() {
    let (mut acp, stand) = connected(STAND_IN).await;
    let (mut stand, [first, second]) = two_chats(&mut acp, stand, Some("/work/other")).await;
    let (server, one) = server_of(&first);
    let (same, two) = server_of(&second);
    assert_eq!(server.url, same.url, "one server for every chat");
    assert!(
        server.url.starts_with("http://127.0.0.1:"),
        "{}",
        server.url
    );
    assert!(
        !server.url.contains(&one) && !server.url.contains(&two),
        "no token in an address"
    );
    assert_ne!(one, two, "each chat has its own token");
    assert_eq!(one.len(), 32);
    assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
    let tools = server.tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "send_file");
    assert_eq!(tools[0]["inputSchema"]["required"], json!(["path"]));
    // A session taken up again is given the server too.
    let agent = tokio::spawn(async move {
        let load = stand.next().await;
        assert_eq!(load["method"], "session/load");
        let (_, token) = server_of(&load);
        stand.reply(&load, json!({})).await;
        token
    });
    let again = prompt("c-3", "m-3", None, Some("s9"));
    let _ = tokio::time::timeout(Duration::from_millis(300), acp.prompt(again)).await;
    let three = agent.await.unwrap();
    assert!(three != one && three != two);
    // No event the chats see carries a token.
    while let Ok(Some(output)) =
        tokio::time::timeout(Duration::from_millis(50), acp.next_event()).await
    {
        if let Output::Event(e) = output {
            let shown = serde_json::to_string(&e.event).unwrap();
            for token in [&one, &two, &three] {
                assert!(!shown.contains(token.as_str()), "a token in {shown}");
            }
        }
    }
}

// Until the core asks the driver to serve files (which it does unless the settings switch it off,
// D-110), the driver listens on nothing and hands the agent no server, even one that takes it.
#[tokio::test]
async fn no_server_is_given_until_the_settings_switch_file_sending_on() {
    let off = Agent {
        serve: false,
        ..STAND_IN
    };
    let (mut acp, stand) = connected(off).await;
    let (_stand, [first, second]) = two_chats(&mut acp, stand, None).await;
    assert_eq!(first["params"]["mcpServers"], json!([]));
    assert_eq!(second["params"]["mcpServers"], json!([]));
}

#[tokio::test]
async fn an_agent_that_takes_no_server_over_http_is_given_none() {
    let none = Agent {
        http: false,
        ..STAND_IN
    };
    let (mut acp, stand) = connected(none).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand, None).await;
    assert_eq!(first["params"]["mcpServers"], json!([]));
}

// OpenCode names the session in `_meta["ai.opencode/sessionID"]` and keeps one server for every
// session in a folder, with the token of the chat that opened a session there last: the call goes
// to the chat whose session it names, and a relative path is made whole from that chat's folder.
#[tokio::test]
async fn opencode_calls_go_to_the_chat_in_the_folder_whose_session_they_name() {
    let (mut acp, stand) = connected(OPENCODE).await;
    let (_stand, [_, second]) = two_chats(&mut acp, stand, None).await;
    let (server, _) = server_of(&second);
    let meta = json!({"ai.opencode/sessionID": "s1", "progressToken": 2});
    let call = tokio::spawn(async move { call_tool(&server, "out/notes.txt", meta).await });
    let file = next_file(&mut acp).await;
    assert_eq!(file.chat.as_str(), "c-1");
    assert_eq!(file.path, PathBuf::from("/work/demo/out/notes.txt"));
    acp.file_sent(file.id, Ok(())).await;
    let result = call.await.unwrap();
    assert_eq!(said(&result), ("Sent.".to_string(), false));
}

// An OpenCode token is good for its folder's chats only: a call naming a chat in another folder is
// refused, and so is one that names no session, or names it in a shape the driver cannot read. The
// driver never falls back to the chat whose token it is.
#[tokio::test]
async fn opencode_calls_outside_the_tokens_folder_or_without_a_session_are_refused() {
    let (mut acp, stand) = connected(OPENCODE).await;
    let (_stand, [first, second]) = two_chats(&mut acp, stand, Some("/work/other")).await;
    let (other, _) = server_of(&first);
    let (own, _) = server_of(&second);
    for (server, meta) in [
        (&own, json!({"ai.opencode/sessionID": "s1"})),
        (&other, json!({"ai.opencode/sessionID": "s2"})),
        (&own, Value::Null),
        (&own, json!({"progressToken": 2})),
        (&own, json!({"ai.opencode/sessionID": 7})),
        (&own, json!({"ai.opencode/sessionID": ""})),
    ] {
        refused(&mut acp, server.clone(), meta).await;
    }
    // Each chat's own session, through its own folder's server, still works.
    let meta = json!({"ai.opencode/sessionID": "s1"});
    let call = tokio::spawn(async move { call_tool(&other, "notes.txt", meta).await });
    let file = next_file(&mut acp).await;
    assert_eq!(file.chat.as_str(), "c-1");
    assert_eq!(file.path, PathBuf::from("/work/other/notes.txt"));
    acp.file_sent(file.id, Ok(())).await;
    call.await.unwrap();
}

// Goose keeps a server for each session and names the session in `_meta["agent-session-id"]`: the
// session must be the token's chat's own. A call through one chat's server naming another chat's
// session is refused, and so is one naming none, which Goose always names.
#[tokio::test]
async fn goose_calls_must_name_a_session_of_the_tokens_chat() {
    let (mut acp, stand) = connected(GOOSE).await;
    let (_stand, [first, second]) = two_chats(&mut acp, stand, None).await;
    let (one, _) = server_of(&first);
    let (two, _) = server_of(&second);
    refused(&mut acp, one.clone(), json!({"agent-session-id": "s2"})).await;
    refused(&mut acp, two.clone(), json!({"agent-session-id": 2})).await;
    refused(&mut acp, one, Value::Null).await;
    refused(&mut acp, two.clone(), json!({"progressToken": 2})).await;
    let meta = json!({"agent-session-id": "s2", "agent-working-dir": "/somewhere/else"});
    let call = tokio::spawn(async move { call_tool(&two, "notes.txt", meta).await });
    let file = next_file(&mut acp).await;
    assert_eq!(file.chat.as_str(), "c-2");
    // The chat's own folder, never the one the agent says.
    assert_eq!(file.path, PathBuf::from("/work/demo/notes.txt"));
    acp.file_sent(file.id, Ok(())).await;
    call.await.unwrap();
}

// Cursor names no session, and keeps a server for each session: the call goes to the chat whose
// token it carries. An absolute path stays as it is, and a refusal reaches the agent in words.
#[tokio::test]
async fn a_call_that_names_no_session_goes_to_the_chat_of_its_token() {
    let (mut acp, stand) = connected(STAND_IN).await;
    let (_stand, [_, second]) = two_chats(&mut acp, stand, None).await;
    let (server, _) = server_of(&second);
    let call = tokio::spawn(async move { call_tool(&server, "/etc/hostname", Value::Null).await });
    let file = next_file(&mut acp).await;
    assert_eq!(file.chat.as_str(), "c-2");
    assert_eq!(file.path, PathBuf::from("/etc/hostname"));
    let refused = Refusal {
        reason: Reason::NotAllowed,
        detail: Some("the file is not in a folder files may be sent from.".into()),
    };
    acp.file_sent(file.id, Err(refused)).await;
    let result = call.await.unwrap();
    assert_eq!(
        said(&result),
        (
            "Not sent: the file is not in a folder files may be sent from.".to_string(),
            true
        )
    );
}

// A call that names a session no chat has is refused at once: the driver cannot tell where it
// belongs, and never guesses.
#[tokio::test]
async fn a_call_naming_a_session_no_chat_has_is_refused() {
    let (mut acp, stand) = connected(OPENCODE).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand, None).await;
    let (server, _) = server_of(&first);
    refused(
        &mut acp,
        server,
        json!({"ai.opencode/sessionID": "child-of-s1"}),
    )
    .await;
}

// A chat's folder changes only once the agent has opened a session there: after a move the agent
// refused, a relative path is still made whole from the folder the chat's session works in.
#[tokio::test]
async fn a_relative_path_after_a_refused_move_is_from_the_old_folder() {
    let (mut acp, stand) = connected(STAND_IN).await;
    let (mut stand, [first, _]) = two_chats(&mut acp, stand, None).await;
    let (server, _) = server_of(&first);
    let control = Control {
        request: RequestId("r-1".into()),
        by: Person {
            id: "u-1".into(),
            name: "Alice".into(),
        },
        action: Action::NewSession {
            chat: chat("c-1", Some("/work/other")),
            session: Some("s1".into()),
        },
    };
    // Chat 1's turn ends; a session is replaced only when it is idle.
    let ended = stand.prompts[0].clone();
    stand.reply(&ended, json!({"stopReason": "end_turn"})).await;
    settle(&mut acp).await;
    let agent = tokio::spawn(async move {
        let new = stand.next().await;
        assert_eq!(new["method"], "session/new");
        assert_eq!(new["params"]["cwd"], "/work/other");
        let error = json!({"code": -32603, "message": "no such folder"});
        stand
            .send(json!({"jsonrpc": "2.0", "id": new["id"], "error": error}))
            .await;
        stand
    });
    acp.control(control).await.unwrap();
    let _stand = agent.await.unwrap();
    settle(&mut acp).await;
    let meta = json!({"agent-session-id": "s1"});
    let call = tokio::spawn(async move { call_tool(&server, "notes.txt", meta).await });
    let file = next_file(&mut acp).await;
    assert_eq!(file.chat.as_str(), "c-1");
    assert_eq!(file.path, PathBuf::from("/work/demo/notes.txt"));
    acp.file_sent(file.id, Ok(())).await;
    call.await.unwrap();
}

#[tokio::test]
async fn a_request_without_a_token_the_driver_gave_is_forbidden() {
    let (mut acp, stand) = connected(STAND_IN).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand, None).await;
    let (server, _) = server_of(&first);
    let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "x", "version": "0"}}});
    let without = Server {
        headers: Vec::new(),
        ..server.clone()
    };
    assert_eq!(without.post(&hello).await.unwrap().0, 403);
    let wrong = Server {
        headers: vec![("Authorization".into(), format!("Bearer {}", "0".repeat(32)))],
        ..server
    };
    assert_eq!(wrong.post(&hello).await.unwrap().0, 403);
}

#[tokio::test]
async fn a_call_the_driver_can_no_longer_answer_is_not_sent() {
    let (mut acp, stand) = connected(STAND_IN).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand, None).await;
    let (server, _) = server_of(&first);
    let call = tokio::spawn(async move {
        server
            .call("send_file", json!({"path": "notes.txt"}), None)
            .await
    });
    let _file = next_file(&mut acp).await;
    // The driver is dropped before the core answers.
    drop(acp);
    let ended = tokio::time::timeout(Duration::from_secs(5), call).await;
    // The call ends: answered as not sent, or cut off with its connection as the server stops.
    match ended
        .expect("the call ends once the driver has gone")
        .unwrap()
    {
        Ok(result) => {
            let (text, error) = said(&result);
            assert!(error && text.starts_with("Not sent"), "{text}");
        }
        // Cut off: the connection closed with no answer, or was reset. An answer that is not
        // 200, or anything else, fails.
        Err(cut_off) => assert!(
            cut_off.to_string() == "no answer"
                || matches!(
                    cut_off.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                ),
            "{cut_off}"
        ),
    }
}

// A client that sends part of a call and then nothing does not hold the server open once the
// driver has gone: the connection is closed within a few seconds.
#[tokio::test]
async fn a_call_cut_off_halfway_does_not_hold_the_server_open() {
    let (mut acp, stand) = connected(STAND_IN).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand, None).await;
    let (server, token) = server_of(&first);
    let rest = server.url.strip_prefix("http://").unwrap();
    let (address, path) = rest.split_at(rest.find('/').unwrap());
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Content-Length: 100\r\n\r\n{{\"jsonrpc\""
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    settle(&mut acp).await;
    drop(acp);
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut rest),
    )
    .await;
    assert!(closed.is_ok(), "the connection is still open");
}
