//! The transport against a stand-in agent: a task that plays an ACP agent over an in-memory pipe,
//! answering as OpenCode did in the recordings. It checks what the recordings cannot: what the
//! driver sends, and that each answer and control is settled by the event that shows its effect.

use marmot_agent_acp::Acp;
use marmot_agent_driver::{
    Action, Answer, Chat, Control, Driver, Emitted, Given, Person, Prompt, RequestId,
};
use marmot_agent_events::{Body, State};
use serde_json::{Value, json};
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
        },
        sender: person(),
        reply_to: None,
        attachments: Vec::new(),
        session: None,
    }
}

/// The next event the driver hands the core.
async fn event(acp: &mut Acp) -> Emitted {
    acp.next_event().await.expect("an event")
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
