//! The transport against a stand-in agent: a task that plays an ACP agent over an in-memory pipe,
//! answering as OpenCode did in the recordings. It checks what the recordings cannot: what the
//! driver sends, and that each answer and control is settled by the event that shows its effect.

use marmot_agent_acp::Acp;
use marmot_agent_driver::{
    Action, Answer, Chat, Control, Driver, Emitted, Given, Output, Person, Prompt, RequestId,
};
use marmot_agent_events::{Body, State};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, WriteHalf};

/// The stand-in's side of the pipe.
struct Stand {
    lines: Lines<BufReader<tokio::io::ReadHalf<DuplexStream>>>,
    out: WriteHalf<DuplexStream>,
}

impl Stand {
    async fn next(&mut self) -> Value {
        let line = self
            .lines
            .next_line()
            .await
            .unwrap()
            .expect("a message from the driver");
        serde_json::from_str(&line).unwrap()
    }

    async fn send(&mut self, message: Value) {
        self.out
            .write_all(message.to_string().as_bytes())
            .await
            .unwrap();
        self.out.write_all(b"\n").await.unwrap();
    }

    async fn reply(&mut self, to: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": to["id"], "result": result}))
            .await;
    }

    async fn update(&mut self, update: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": update}}))
            .await;
    }
}

async fn connected() -> (Acp, Stand) {
    let (driver_side, stand_side) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(stand_side);
    let mut stand = Stand {
        lines: BufReader::new(read).lines(),
        out: write,
    };
    let (driver_read, driver_write) = tokio::io::split(driver_side);
    let greeting = tokio::spawn(async move {
        let hello = stand.next().await;
        assert_eq!(hello["method"], "initialize");
        assert_eq!(
            hello["params"]["clientCapabilities"]["elicitation"],
            json!({"form": {}})
        );
        let agent = json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true},
                           "agentInfo": {"name": "Stand In", "version": "1.0.0"}});
        stand.reply(&hello, agent).await;
        stand
    });
    let acp = Acp::connect(driver_read, driver_write, "/work/demo", "standin")
        .await
        .unwrap();
    (acp, greeting.await.unwrap())
}

fn person() -> Person {
    Person {
        id: "u-1".into(),
        name: "Alice".into(),
    }
}

fn prompt(text: &str) -> Prompt {
    Prompt {
        message: "m-1".into(),
        text: text.into(),
        chat: Chat {
            id: "c-1".into(),
            group: false,
            name: None,
            folder: None,
        },
        sender: person(),
        reply_to: None,
        attachments: Vec::new(),
        session: None,
    }
}

/// The next event the driver hands the core.
async fn event(acp: &mut Acp) -> Emitted {
    acp.next_event()
        .await
        .and_then(Output::into_event)
        .expect("an event")
}

#[tokio::test]
async fn a_prompt_starts_a_session_and_a_turn_that_responds_to_it() {
    let (mut acp, mut stand) = connected().await;
    assert_eq!(acp.harness().version.as_deref(), Some("1.0.0"));
    let agent = tokio::spawn(async move {
        let new = stand.next().await;
        assert_eq!(new["method"], "session/new");
        assert_eq!(new["params"]["cwd"], "/work/demo");
        stand.reply(&new, json!({"sessionId": "s1"})).await;
        let asked = stand.next().await;
        assert_eq!(asked["method"], "session/prompt");
        assert_eq!(asked["params"]["prompt"][0]["text"], "hello");
        stand.update(json!({"sessionUpdate": "agent_message_chunk", "messageId": "m", "content": {"type": "text", "text": "hi"}})).await;
        stand.reply(&asked, json!({"stopReason": "end_turn"})).await;
        stand
    });
    acp.prompt(prompt("hello")).await.unwrap();
    let mut seen = Vec::new();
    while seen.len() < 4 {
        seen.push(event(&mut acp).await);
    }
    agent.await.unwrap();
    let kinds: Vec<(&str, State)> = seen
        .iter()
        .map(|e| {
            let kind = match e.event.body() {
                Body::Session(_) => "session",
                Body::Turn(_) => "turn",
                Body::Message(_) => "message",
                _ => "other",
            };
            (kind, e.event.header().state)
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("session", State::Running),
            ("turn", State::Running),
            ("message", State::Done),
            ("turn", State::Done)
        ]
    );
    assert_eq!(seen[0].event.header().harness, "stand-in");
    assert_eq!(seen[1].prompts, vec!["m-1".into()]);
    assert_eq!(seen[3].prompts, vec!["m-1".into()]);
}

#[tokio::test]
async fn an_approval_is_answered_and_an_interrupt_settled_by_the_turn_ending() {
    let (mut acp, mut stand) = connected().await;
    let (to_agent, mut from_test) = tokio::sync::mpsc::channel::<()>(1);
    let agent = tokio::spawn(async move {
        let new = stand.next().await;
        stand.reply(&new, json!({"sessionId": "s1"})).await;
        let asked = stand.next().await;
        stand.update(json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "ls", "kind": "execute", "status": "pending", "rawInput": {"command": "ls"}})).await;
        stand
            .send(json!({"jsonrpc": "2.0", "id": 7, "method": "session/request_permission", "params": {
                "sessionId": "s1", "toolCall": {"toolCallId": "t1", "title": "ls", "kind": "execute", "rawInput": {"command": "ls"}},
                "options": [{"optionId": "once", "name": "Allow once", "kind": "allow_once"},
                            {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}}))
            .await;
        let answered = stand.next().await;
        assert_eq!(answered["id"], 7);
        assert_eq!(
            answered["result"],
            json!({"outcome": {"outcome": "selected", "optionId": "once"}})
        );
        stand.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress"})).await;
        from_test.recv().await;
        let cancel = stand.next().await;
        assert_eq!(cancel["method"], "session/cancel");
        assert_eq!(cancel["params"]["sessionId"], "s1");
        stand
            .reply(&asked, json!({"stopReason": "cancelled"}))
            .await;
        stand
    });
    acp.prompt(prompt("list the files")).await.unwrap();
    let mut turn = None;
    let approval = loop {
        let e = event(&mut acp).await;
        turn = turn.or_else(|| e.event.header().turn.clone());
        if let Body::Approval(a) = e.event.body() {
            break (e.event.header().item.clone(), a.choices[0].id.clone());
        }
    };
    let turn = turn.expect("the turn");
    let answer = Answer {
        request: RequestId::from("r-1"),
        item: approval.0.clone(),
        by: person(),
        given: Given::Choice(approval.1),
    };
    acp.answer(answer).await.unwrap();
    let answered = event(&mut acp).await;
    assert!(matches!(answered.event.body(), Body::Approval(_)));
    assert_eq!(answered.settles, vec![RequestId::from("r-1")]);
    to_agent.send(()).await.unwrap();
    let stop = Control {
        request: RequestId::from("r-2"),
        by: person(),
        action: Action::Interrupt { turn },
    };
    acp.control(stop).await.unwrap();
    let ended = loop {
        let e = event(&mut acp).await;
        if matches!(e.event.body(), Body::Turn(_)) {
            break e;
        }
    };
    agent.await.unwrap();
    assert_eq!(ended.event.header().state, State::Cancelled);
    assert_eq!(ended.settles, vec![RequestId::from("r-2")]);
}

/// A stand-in over a pipe that holds only `buffer` bytes, so a write can be left waiting.
async fn connected_with(buffer: usize) -> (Acp, Stand) {
    let (driver_side, stand_side) = tokio::io::duplex(buffer);
    let (read, write) = tokio::io::split(stand_side);
    let mut stand = Stand {
        lines: BufReader::new(read).lines(),
        out: write,
    };
    let (driver_read, driver_write) = tokio::io::split(driver_side);
    let greeting = tokio::spawn(async move {
        let hello = stand.next().await;
        let agent = json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true},
                           "agentInfo": {"name": "Stand In", "version": "1.0.0"}});
        stand.reply(&hello, agent).await;
        stand
    });
    let acp = Acp::connect(driver_read, driver_write, "/work/demo", "standin")
        .await
        .unwrap();
    (acp, greeting.await.unwrap())
}

impl Stand {
    async fn update_in(&mut self, session: &str, update: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": update}}))
            .await;
    }

    async fn ask_permission(&mut self, id: u64, session: &str, call: &str) {
        self.update_in(session, json!({"sessionUpdate": "tool_call", "toolCallId": call, "title": "ls", "kind": "execute", "status": "pending", "rawInput": {"command": "ls"}})).await;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": "session/request_permission", "params": {
            "sessionId": session, "toolCall": {"toolCallId": call, "title": "ls", "kind": "execute", "rawInput": {"command": "ls"}},
            "options": [{"optionId": "once", "name": "Allow once", "kind": "allow_once"},
                        {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}}))
            .await;
    }

    /// Sends a message as the exact text given.
    async fn send_text(&mut self, line: &str) {
        self.out.write_all(line.as_bytes()).await.unwrap();
        self.out.write_all(b"\n").await.unwrap();
    }

    /// Asserts that the driver sends nothing for `millis` milliseconds.
    async fn hears_nothing(&mut self, millis: u64) {
        let next =
            tokio::time::timeout(Duration::from_millis(millis), self.lines.next_line()).await;
        assert!(next.is_err(), "the driver sent {next:?}");
    }
}

fn prompt_in(chat: &str, message: &str, session: Option<&str>) -> Prompt {
    Prompt {
        message: message.into(),
        chat: Chat {
            id: chat.into(),
            group: false,
            name: None,
            folder: None,
        },
        session: session.map(Into::into),
        ..prompt("hello")
    }
}

/// Two sessions can name their tool calls alike. An approval is named after its session too, so
/// an answer in one chat never answers the other chat's request.
#[tokio::test]
async fn the_same_tool_call_in_two_chats_is_two_approvals() {
    let (mut acp, mut stand) = connected().await;
    let agent = tokio::spawn(async move {
        for session in ["s1", "s2"] {
            let new = stand.next().await;
            stand.reply(&new, json!({"sessionId": session})).await;
            let asked = stand.next().await;
            assert_eq!(asked["params"]["sessionId"], session);
        }
        stand.ask_permission(10, "s1", "call_001").await;
        stand.ask_permission(20, "s2", "call_001").await;
        let answered = stand.next().await;
        assert_eq!(
            answered["id"], 10,
            "the answer goes to the first chat's request"
        );
        stand
    });
    acp.prompt(prompt_in("c-1", "m-1", None)).await.unwrap();
    acp.prompt(prompt_in("c-2", "m-2", None)).await.unwrap();
    let mut approvals = Vec::new();
    while approvals.len() < 2 {
        let e = event(&mut acp).await;
        if let Body::Approval(a) = e.event.body() {
            approvals.push((
                e.chat.clone(),
                e.event.header().item.clone(),
                a.choices[0].id.clone(),
            ));
        }
    }
    assert_ne!(approvals[0].1, approvals[1].1, "two items");
    let (_, item, choice) = approvals
        .iter()
        .find(|(chat, _, _)| chat.as_ref().map(|c| c.as_str()) == Some("c-1"))
        .cloned()
        .unwrap();
    let answer = Answer {
        request: RequestId::from("r-1"),
        item,
        by: person(),
        given: Given::Choice(choice),
    };
    acp.answer(answer).await.unwrap();
    let answered = event(&mut acp).await;
    assert_eq!(answered.chat.as_ref().map(|c| c.as_str()), Some("c-1"));
    assert_eq!(answered.settles, vec![RequestId::from("r-1")]);
    agent.await.unwrap();
}

/// After a session is loaded, Goose runs the cut-off turn by itself, says so in
/// `_meta.goose.activeRunId`, and refuses a prompt until it clears it. The driver cancels the run
/// and holds the prompt until then.
#[tokio::test]
async fn a_prompt_waits_for_goose_to_end_the_run_it_took_up() {
    let (mut acp, mut stand) = connected().await;
    let agent = tokio::spawn(async move {
        let load = stand.next().await;
        assert_eq!(load["method"], "session/load");
        let busy = json!({"sessionUpdate": "session_info_update", "_meta": {"goose": {"activeRunId": "resume_1"}}});
        stand.update_in("s1", busy).await;
        stand.reply(&load, json!({})).await;
        let cancel = stand.next().await;
        assert_eq!(cancel["method"], "session/cancel");
        // A permission Goose asks for in the run it took up is answered as cancelled.
        stand.ask_permission(5, "s1", "call_resumed").await;
        let answered = stand.next().await;
        assert_eq!(answered["id"], 5);
        assert_eq!(
            answered["result"],
            json!({"outcome": {"outcome": "cancelled"}})
        );
        // Longer than any pause a driver might take for idle: Goose can be silent while busy.
        stand.hears_nothing(3000).await;
        let idle = json!({"sessionUpdate": "session_info_update", "_meta": {"goose": {"activeRunId": null}}});
        stand.update_in("s1", idle).await;
        let asked = stand.next().await;
        assert_eq!(asked["method"], "session/prompt");
        stand
    });
    acp.prompt(prompt_in("c-1", "m-1", Some("s1")))
        .await
        .unwrap();
    agent.await.unwrap();
}

/// What the driver answers by itself goes whole to the agent, even when the core gives up waiting
/// for an event while the answer is still being written.
#[tokio::test]
async fn a_dropped_wait_leaves_no_half_message() {
    let (mut acp, stand) = connected_with(48).await;
    let Stand { mut lines, mut out } = stand;
    // The stand-in writes from a task of its own: the pipe holds less than one message.
    let writing = tokio::spawn(async move {
        for id in [1, 2] {
            let ask = json!({"jsonrpc": "2.0", "id": id, "method": "cursor/update_todos", "params": {"todos": []}});
            out.write_all(format!("{ask}\n").as_bytes()).await.unwrap();
        }
        out
    });
    for _ in 0..4 {
        let waited = tokio::time::timeout(Duration::from_millis(50), acp.next_event()).await;
        assert!(waited.is_err(), "no event comes");
    }
    for id in [1, 2] {
        // A whole message, or this fails to read.
        let line = lines.next_line().await.unwrap().expect("a reply");
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id);
        assert_eq!(reply["result"], json!({}));
    }
    writing.await.unwrap();
}

/// A message during a turn cancels it. A permission the agent asks for after the cancel is
/// answered as cancelled at once, so the turn can end and the new prompt go.
#[tokio::test]
async fn a_permission_asked_while_cancelling_is_answered_as_cancelled() {
    let (mut acp, mut stand) = connected().await;
    let agent = tokio::spawn(async move {
        let new = stand.next().await;
        stand.reply(&new, json!({"sessionId": "s1"})).await;
        let first = stand.next().await;
        let cancel = stand.next().await;
        assert_eq!(cancel["method"], "session/cancel");
        stand.ask_permission(9, "s1", "call_late").await;
        let answered = stand.next().await;
        assert_eq!(answered["id"], 9);
        assert_eq!(
            answered["result"],
            json!({"outcome": {"outcome": "cancelled"}})
        );
        stand
            .reply(&first, json!({"stopReason": "cancelled"}))
            .await;
        let second = stand.next().await;
        assert_eq!(second["method"], "session/prompt");
        assert_eq!(second["params"]["prompt"][0]["text"], "again");
        stand
    });
    acp.prompt(prompt_in("c-1", "m-1", None)).await.unwrap();
    let again = Prompt {
        text: "again".into(),
        ..prompt_in("c-1", "m-2", None)
    };
    acp.prompt(again).await.unwrap();
    agent.await.unwrap();
}

/// A setting changed to the value it has is settled by the agent's reply.
#[tokio::test]
async fn a_setting_that_changes_nothing_is_settled_by_the_reply() {
    let (mut acp, mut stand) = connected().await;
    let options = json!([{"id": "model", "name": "Model", "category": "model", "type": "select",
                          "currentValue": "a", "options": [{"value": "a", "name": "A"}, {"value": "b", "name": "B"}]}]);
    let config = options.clone();
    let agent = tokio::spawn(async move {
        let new = stand.next().await;
        stand
            .reply(&new, json!({"sessionId": "s1", "configOptions": config}))
            .await;
        stand.next().await; // the prompt
        let set = stand.next().await;
        assert_eq!(set["method"], "session/set_config_option");
        assert_eq!(set["params"]["value"], "a");
        // Text the translator holds until something else arrives: the reply flushes it first.
        let chunk = json!({"sessionUpdate": "agent_message_chunk", "messageId": "m", "content": {"type": "text", "text": "thinking aloud"}});
        stand.update_in("s1", chunk).await;
        stand.reply(&set, json!({"configOptions": config})).await;
        stand
    });
    acp.prompt(prompt_in("c-1", "m-1", None)).await.unwrap();
    let set = Control {
        request: RequestId::from("r-3"),
        by: person(),
        action: Action::Set {
            session: "s1".into(),
            setting: "model".into(),
            value: "a".into(),
        },
    };
    acp.control(set).await.unwrap();
    let settled = loop {
        let e = event(&mut acp).await;
        if !e.settles.is_empty() {
            break e;
        }
    };
    assert!(matches!(settled.event.body(), Body::Session(_)));
    assert_eq!(settled.settles, vec![RequestId::from("r-3")]);
    agent.await.unwrap();
    let _ = options;
}

/// A stand-in that, like Cursor, does not name itself.
async fn connected_as_cursor() -> (Acp, Stand) {
    let (driver_side, stand_side) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(stand_side);
    let mut stand = Stand {
        lines: BufReader::new(read).lines(),
        out: write,
    };
    let (driver_read, driver_write) = tokio::io::split(driver_side);
    let greeting = tokio::spawn(async move {
        let hello = stand.next().await;
        stand
            .reply(
                &hello,
                json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true}}),
            )
            .await;
        stand
    });
    let acp = Acp::connect(driver_read, driver_write, "/work/demo", "cursor")
        .await
        .unwrap();
    (acp, greeting.await.unwrap())
}

/// Cursor's to-do list comes in a request that names its tool call and no session, as in
/// `fixtures/recordings/acp/cursor/plan.jsonl`. It goes to the chat whose session has that call.
#[tokio::test]
async fn cursors_to_do_list_reaches_the_chat_of_its_tool_call() {
    let (mut acp, mut stand) = connected_as_cursor().await;
    let call = "call-1-0\nfc_1_0";
    let agent = tokio::spawn(async move {
        for session in ["s1", "s2"] {
            let new = stand.next().await;
            stand.reply(&new, json!({"sessionId": session})).await;
            stand.next().await; // the prompt
        }
        let todo = json!({"sessionUpdate": "tool_call", "toolCallId": call, "title": "Update TODOs", "kind": "other", "status": "pending", "rawInput": {"_toolName": "updateTodos"}});
        stand.update_in("s2", todo).await;
        stand
            .send(
                json!({"jsonrpc": "2.0", "id": 0, "method": "cursor/update_todos", "params": {
                "toolCallId": call, "merge": false,
                "todos": [{"id": "1", "content": "Read greet.py", "status": "in_progress"}]}}),
            )
            .await;
        let taken = stand.next().await;
        assert_eq!(taken, json!({"jsonrpc": "2.0", "id": 0, "result": {}}));
        stand
    });
    acp.prompt(prompt_in("c-1", "m-1", None)).await.unwrap();
    acp.prompt(prompt_in("c-2", "m-2", None)).await.unwrap();
    let plan = loop {
        let e = event(&mut acp).await;
        assert!(
            !matches!(e.event.body(), Body::Tool(_)),
            "the to-do tool is not a tool item"
        );
        if matches!(e.event.body(), Body::Plan(_)) {
            break e;
        }
    };
    assert_eq!(plan.chat.as_ref().map(|c| c.as_str()), Some("c-2"));
    assert_eq!(plan.event.header().text, "[ ] Read greet.py");
    agent.await.unwrap();
}

/// A session request the agent never answers fails the prompt and ends the connection, so a late
/// reply cannot install a session.
#[tokio::test]
async fn a_session_request_never_answered_ends_the_connection() {
    let (mut acp, mut stand) = connected().await;
    acp.set_patience(Duration::from_millis(200));
    let refused = acp.prompt(prompt_in("c-1", "m-1", None)).await;
    assert!(refused.is_err());
    let new = stand.next().await;
    assert_eq!(new["method"], "session/new");
    stand.reply(&new, json!({"sessionId": "s1"})).await;
    assert!(
        acp.next_event().await.is_none(),
        "no session from the late reply"
    );
    assert!(acp.prompt(prompt_in("c-1", "m-2", None)).await.is_err());
}

/// A request id may be spelled with escapes: `"\u0061"` is the id `"a"`. The driver must know
/// the request it showed, however its id was spelled, and answer it once, when the person does.
#[tokio::test]
async fn a_request_with_an_escaped_id_is_answered_once() {
    let (mut acp, mut stand) = connected().await;
    let agent = tokio::spawn(async move {
        let new = stand.next().await;
        stand.reply(&new, json!({"sessionId": "s1"})).await;
        let _asked = stand.next().await;
        stand.update(json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "ls", "kind": "execute", "status": "pending", "rawInput": {"command": "ls"}})).await;
        stand
            .send_text(r#"{"jsonrpc":"2.0","id":"\u0061","method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"t1","title":"ls","kind":"execute","rawInput":{"command":"ls"}},"options":[{"optionId":"once","name":"Allow once","kind":"allow_once"}]}}"#)
            .await;
        let answered = stand.next().await;
        assert_eq!(answered["id"], "a");
        assert_eq!(
            answered["result"],
            json!({"outcome": {"outcome": "selected", "optionId": "once"}}),
            "the request is answered as the person answered it"
        );
        stand.hears_nothing(500).await;
        stand
    });
    acp.prompt(prompt("list the files")).await.unwrap();
    let approval = loop {
        let e = event(&mut acp).await;
        if let Body::Approval(a) = e.event.body() {
            break (e.event.header().item.clone(), a.choices[0].id.clone());
        }
    };
    let answer = Answer {
        request: RequestId::from("r-1"),
        item: approval.0,
        by: person(),
        given: Given::Choice(approval.1),
    };
    acp.answer(answer).await.unwrap();
    let answered = event(&mut acp).await;
    assert_eq!(answered.settles, vec![RequestId::from("r-1")]);
    agent.await.unwrap();
}

/// A request from a session no chat has taken is answered all the same, so the agent does not
/// wait for ever. One that comes while a session is being picked is held, since the session being
/// picked can speak before the reply that picks it; when the reply names another session, the
/// request is refused as the official ACP library refuses a request for a session it does not
/// have: -32002, resource not found, with the session in `data`. So is one that comes when no
/// session is being picked.
#[tokio::test]
async fn a_request_from_a_session_no_chat_has_is_refused_as_not_found() {
    let (mut acp, mut stand) = connected().await;
    let agent = tokio::spawn(async move {
        let wait = Duration::from_secs(5);
        let new = stand.next().await;
        assert_eq!(new["method"], "session/new");
        stand.ask_permission(30, "s-gone", "t1").await;
        stand.reply(&new, json!({"sessionId": "s1"})).await;
        let mut heard = Vec::new();
        while heard.len() < 2 {
            let next = tokio::time::timeout(wait, stand.next()).await;
            heard.push(next.expect("the driver answers the request held while picking"));
        }
        let prompted = heard.iter().find(|m| m["method"] == "session/prompt");
        let prompted = prompted.expect("the prompt").clone();
        let refused = heard.iter().find(|m| m["id"] == 30);
        let refused = refused.expect("the held request is answered");
        let not_found = json!({"code": -32002, "message": "Resource not found",
                               "data": {"sessionId": "s-gone"}});
        assert_eq!(refused["error"], not_found, "held while picking");
        stand.ask_permission(31, "s-gone", "t2").await;
        let refused = tokio::time::timeout(wait, stand.next()).await;
        let refused = refused.expect("the driver answers a request for no chat's session");
        assert_eq!(refused["id"], 31);
        let not_found = json!({"code": -32002, "message": "Resource not found",
                               "data": {"sessionId": "s-gone"}});
        assert_eq!(refused["error"], not_found, "with no session being picked");
        stand
            .reply(&prompted, json!({"stopReason": "end_turn"}))
            .await;
        stand
    });
    acp.prompt(prompt("hello")).await.unwrap();
    loop {
        let e = event(&mut acp).await;
        if matches!(e.event.body(), Body::Turn(_)) && e.event.header().state == State::Done {
            break;
        }
    }
    agent.await.unwrap();
}
