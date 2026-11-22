//! Sending a file (D-090): the driver gives every session it opens its own MCP server, with one
//! tool, `send_file`; a call of it reaches the core as a request to send a file for the chat that
//! made the call, and the core's answer goes back as the tool's result.
//!
//! The stand-in agent here plays the agent's part over a pipe, and calls the tool over HTTP as
//! OpenCode, Goose and Cursor do: `initialize`, `notifications/initialized`, `tools/list`, then
//! `tools/call`, each a request of its own.

use marmot_agent_acp::Acp;
use marmot_agent_driver::{Chat, Driver, Output, Person, Prompt, Reason, Refusal, SendFile};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};

/// The stand-in's side of the pipe.
struct Stand {
    lines: Lines<BufReader<tokio::io::ReadHalf<DuplexStream>>>,
    out: tokio::io::WriteHalf<DuplexStream>,
}

impl Stand {
    async fn next(&mut self) -> Value {
        let line = self.lines.next_line().await.unwrap().expect("a message");
        serde_json::from_str(&line).unwrap()
    }

    async fn reply(&mut self, to: &Value, result: Value) {
        let message = json!({"jsonrpc": "2.0", "id": to["id"], "result": result});
        self.out
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    /// Answers the driver's `session/new` with `session`, and gives back what it asked.
    async fn opens(&mut self, session: &str) -> Value {
        let new = self.next().await;
        assert_eq!(new["method"], "session/new");
        self.reply(&new, json!({"sessionId": session})).await;
        let prompt = self.next().await;
        assert_eq!(prompt["method"], "session/prompt");
        new
    }
}

/// A stand-in agent that says it takes MCP servers over HTTP when `http`.
async fn connected(http: bool) -> (Acp, Stand) {
    let (driver_side, stand_side) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(stand_side);
    let mut stand = Stand {
        lines: BufReader::new(read).lines(),
        out: write,
    };
    let (driver_read, driver_write) = tokio::io::split(driver_side);
    let greeting = tokio::spawn(async move {
        let hello = stand.next().await;
        let mut capabilities = json!({"loadSession": true});
        if http {
            capabilities["mcpCapabilities"] = json!({"http": true, "sse": false});
        }
        let agent = json!({"protocolVersion": 1, "agentCapabilities": capabilities,
                           "agentInfo": {"name": "Stand In", "version": "1.0.0"}});
        stand.reply(&hello, agent).await;
        stand
    });
    let acp = Acp::connect(driver_read, driver_write, "/work/demo", "standin")
        .await
        .unwrap();
    (acp, greeting.await.unwrap())
}

fn prompt(chat: &str, message: &str, folder: Option<&str>, session: Option<&str>) -> Prompt {
    Prompt {
        message: message.into(),
        text: "send it".into(),
        chat: Chat {
            id: chat.into(),
            group: false,
            name: None,
            folder: folder.map(PathBuf::from),
        },
        sender: Person {
            id: "u-1".into(),
            name: "Alice".into(),
        },
        reply_to: None,
        attachments: Vec::new(),
        session: session.map(Into::into),
    }
}

/// The server a session request handed the agent: its address and the chat's token.
fn server_of(request: &Value) -> (String, String) {
    let servers = request["params"]["mcpServers"].as_array().expect("a list");
    assert_eq!(servers.len(), 1, "one server: {servers:?}");
    let server = &servers[0];
    assert_eq!(server["type"], "http");
    assert_eq!(server["name"], "marmot");
    let url = server["url"].as_str().expect("an address").to_string();
    let header = &server["headers"][0];
    assert_eq!(header["name"], "Authorization");
    let token = header["value"].as_str().unwrap();
    let token = token.strip_prefix("Bearer ").expect("a bearer token");
    (url, token.to_string())
}

/// One HTTP request to the server, as an MCP client makes it: the status, and the JSON answer.
async fn post(url: &str, token: Option<&str>, body: &Value) -> (u16, Value) {
    let address = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .expect("an http address");
    let path = &url[url.find(address).unwrap() + address.len()..];
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let body = body.to_string();
    let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-11-25\r\n\
         {auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, rest) = raw.split_once("\r\n\r\n").expect("an HTTP response");
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap();
    let chunked = head.to_lowercase().contains("transfer-encoding: chunked");
    let text = if chunked {
        unchunk(rest)
    } else {
        rest.to_string()
    };
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn unchunk(mut rest: &str) -> String {
    let mut text = String::new();
    while let Some((size, after)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        text.push_str(&after[..size]);
        rest = &after[size + 2..];
    }
    text
}

/// What an agent does to call the tool: greet the server, list its tools, call it with `path` and
/// `meta`. Gives back the call's result.
async fn call_tool(url: &str, token: &str, path: &str, meta: Value) -> Value {
    let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "stand-in", "version": "0"}}});
    let (status, greeted) = post(url, Some(token), &hello).await;
    assert_eq!(status, 200, "{greeted}");
    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let (status, _) = post(url, Some(token), &initialized).await;
    assert!(status == 202 || status == 200, "{status}");
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (_, tools) = post(url, Some(token), &list).await;
    let tools = tools["result"]["tools"].as_array().expect("tools").clone();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "send_file");
    assert_eq!(tools[0]["inputSchema"]["required"], json!(["path"]));
    let mut params = json!({"name": "send_file", "arguments": {"path": path}});
    if !meta.is_null() {
        params["_meta"] = meta;
    }
    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": params});
    let (status, answered) = post(url, Some(token), &call).await;
    assert_eq!(status, 200, "{answered}");
    answered["result"].clone()
}

/// The text of a tool's result.
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

/// Two chats open sessions in that order: as OpenCode keeps one server for every session in a
/// folder, the second chat's token is the one the agent then sends for both.
async fn two_chats(acp: &mut Acp, stand: Stand) -> (Stand, [Value; 2]) {
    let agent = tokio::spawn(async move {
        let mut stand = stand;
        let first = stand.opens("s1").await;
        let second = stand.opens("s2").await;
        (stand, [first, second])
    });
    acp.prompt(prompt("c-1", "m-1", Some("/work/other"), None))
        .await
        .unwrap();
    acp.prompt(prompt("c-2", "m-2", None, None)).await.unwrap();
    agent.await.unwrap()
}

#[tokio::test]
async fn each_session_is_given_the_drivers_server_with_its_chats_own_token() {
    let (mut acp, stand) = connected(true).await;
    let (mut stand, [first, second]) = two_chats(&mut acp, stand).await;
    let (url, one) = server_of(&first);
    let (same, two) = server_of(&second);
    assert_eq!(url, same, "one server for every chat");
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    assert!(
        !url.contains(&one) && !url.contains(&two),
        "no token in an address"
    );
    assert_ne!(one, two, "each chat has its own token");
    assert_eq!(one.len(), 32);
    assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
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

#[tokio::test]
async fn an_agent_that_takes_no_server_over_http_is_given_none() {
    let (mut acp, stand) = connected(false).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand).await;
    assert_eq!(first["params"]["mcpServers"], json!([]));
}

// OpenCode names the session in `_meta["ai.opencode/sessionID"]` and sends the last chat's token
// for every session in a folder: the call goes to the chat whose session it names, and a relative
// path is made whole from that chat's folder.
#[tokio::test]
async fn a_call_goes_to_the_chat_whose_session_it_names() {
    let (mut acp, stand) = connected(true).await;
    let (_stand, [_, second]) = two_chats(&mut acp, stand).await;
    let (url, token) = server_of(&second);
    let meta = json!({"ai.opencode/sessionID": "s1", "progressToken": 2});
    let call = tokio::spawn(async move { call_tool(&url, &token, "out/notes.txt", meta).await });
    let file = next_file(&mut acp).await;
    assert_eq!(file.chat.as_str(), "c-1");
    assert_eq!(file.path, PathBuf::from("/work/other/out/notes.txt"));
    acp.file_sent(file.id, Ok(())).await;
    let result = call.await.unwrap();
    assert_eq!(said(&result), ("Sent.".to_string(), false));
}

// Goose names it in `_meta["agent-session-id"]`.
#[tokio::test]
async fn goose_names_the_session_under_its_own_key() {
    let (mut acp, stand) = connected(true).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand).await;
    let (url, token) = server_of(&first);
    let meta = json!({"agent-session-id": "s2", "agent-working-dir": "/somewhere/else"});
    let call = tokio::spawn(async move { call_tool(&url, &token, "notes.txt", meta).await });
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
    let (mut acp, stand) = connected(true).await;
    let (_stand, [_, second]) = two_chats(&mut acp, stand).await;
    let (url, token) = server_of(&second);
    let call =
        tokio::spawn(async move { call_tool(&url, &token, "/etc/hostname", Value::Null).await });
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
    let (mut acp, stand) = connected(true).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand).await;
    let (url, token) = server_of(&first);
    let meta = json!({"ai.opencode/sessionID": "child-of-s1"});
    let call = tokio::spawn(async move { call_tool(&url, &token, "notes.txt", meta).await });
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
    assert!(error, "{text}");
    assert!(text.starts_with("Not sent"), "{text}");
}

#[tokio::test]
async fn a_request_without_a_token_the_driver_gave_is_forbidden() {
    let (mut acp, stand) = connected(true).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand).await;
    let (url, _) = server_of(&first);
    let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "x", "version": "0"}}});
    assert_eq!(post(&url, None, &hello).await.0, 403);
    let wrong = "0".repeat(32);
    assert_eq!(post(&url, Some(&wrong), &hello).await.0, 403);
}

#[tokio::test]
async fn a_call_the_driver_can_no_longer_answer_is_not_sent() {
    let (mut acp, stand) = connected(true).await;
    let (_stand, [first, _]) = two_chats(&mut acp, stand).await;
    let (url, token) = server_of(&first);
    let call = tokio::spawn(async move { call_tool(&url, &token, "notes.txt", Value::Null).await });
    let _file = next_file(&mut acp).await;
    // Holding the server open, the driver is dropped before the core answers.
    let held = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(acp);
    });
    let ended = tokio::time::timeout(Duration::from_secs(5), call).await;
    held.await.unwrap();
    // The call ends: answered as not sent, or cut off with its connection as the server stops.
    match ended.expect("the call ends once the driver has gone") {
        Ok(result) => {
            let (text, error) = said(&result);
            assert!(error && text.starts_with("Not sent"), "{text}");
        }
        Err(cut_off) => assert!(cut_off.is_panic(), "{cut_off}"),
    }
}
