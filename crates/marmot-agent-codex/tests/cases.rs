//! Cases no recording holds, with messages written by hand in the shapes Codex's schema gives
//! them. The recordings are the main test (`replay.rs`); `tools/prototype/test_translate.py`
//! holds the same secret question for the prototype.

use marmot_agent_codex::{Direction, Translator};
use marmot_agent_events::body::{Answered, NoticeKind};
use marmot_agent_events::{AgentEvent, Body, State};

const PASSWORD: &str = "hunter2";

fn feed(messages: &[(Direction, &str)]) -> Vec<AgentEvent> {
    let mut translator = Translator::new();
    let events = messages
        .iter()
        .flat_map(|(direction, message)| translator.feed(*direction, message))
        .collect();
    assert!(
        translator.unknown().is_empty(),
        "{:?}",
        translator.unknown()
    );
    events
}

fn question(secret: bool) -> String {
    format!(
        r#"{{"id": 7, "method": "item/tool/requestUserInput", "params": {{"threadId": "th1", "turnId": "t1", "itemId": "call1", "questions": [
            {{"id": "name", "header": "Name", "question": "Who are you?", "isOther": true, "isSecret": false, "options": []}},
            {{"id": "password", "header": "Password", "question": "Password?", "isOther": true, "isSecret": {secret}, "options": []}}]}}}}"#
    )
}

fn answer() -> String {
    format!(
        r#"{{"id": 7, "result": {{"answers": {{"name": {{"answers": ["Alice"]}}, "password": {{"answers": ["{PASSWORD}"]}}}}}}}}"#
    )
}

fn questions(events: &[AgentEvent]) -> Vec<&AgentEvent> {
    events
        .iter()
        .filter(|e| matches!(e.body(), Body::Question(_)))
        .collect()
}

#[test]
fn an_answered_secret_question_repeats_neither_the_answer_nor_the_reply() {
    let events = feed(&[
        (Direction::In, &question(true)),
        (Direction::Out, &answer()),
    ]);
    let [asked, answered] = questions(&events)[..] else {
        panic!("two question events");
    };
    assert_eq!(asked.header().state, State::Waiting);
    assert!(
        asked.native().is_some(),
        "a waiting question keeps Codex's request"
    );

    assert_eq!(answered.header().state, State::Done);
    assert!(answered.native().is_none());
    let Body::Question(q) = answered.body() else {
        unreachable!()
    };
    assert_eq!(
        q.answer(),
        Some(
            &[
                Answered {
                    question: "name".into(),
                    options: vec![],
                    text: Some("Alice".into()),
                },
                Answered {
                    question: "password".into(),
                    options: vec![],
                    text: None,
                },
            ][..]
        )
    );
    let written = serde_json::to_string(answered).expect("JSON");
    assert!(!written.contains(PASSWORD), "{written}");
}

#[test]
fn a_question_with_nothing_secret_keeps_the_reply() {
    let events = feed(&[
        (Direction::In, &question(false)),
        (Direction::Out, &answer()),
    ]);
    let answered = questions(&events)[1];
    assert!(
        answered
            .native()
            .expect("the reply")
            .payload
            .get()
            .contains(PASSWORD)
    );
}

#[test]
fn a_failed_mcp_call_names_its_error_and_keeps_the_object() {
    let failed = r#"{"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1", "item": {"type": "mcpToolCall", "id": "call2", "server": "files", "tool": "read", "status": "failed", "arguments": {}, "result": null, "error": {"message": "no such file", "code": 4}}}}"#;
    let events = feed(&[(Direction::In, failed)]);
    let Body::Tool(tool) = events[0].body() else {
        panic!("a tool item");
    };
    assert_eq!(events[0].header().state, State::Failed);
    assert_eq!(tool.error.as_deref(), Some("no such file"));
    assert!(
        events[0]
            .native()
            .expect("the native message")
            .payload
            .get()
            .contains(r#""code": 4"#)
    );
}

#[test]
fn a_phase_the_event_model_does_not_name_is_left_out() {
    let message = r#"{"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1", "item": {"type": "agentMessage", "id": "msg1", "text": "Almost there", "phase": "partial_answer"}}}"#;
    let events = feed(&[(Direction::In, message)]);
    let Body::Message(m) = events[0].body() else {
        panic!("a message item");
    };
    assert_eq!(m.phase, None);
    assert!(
        events[0]
            .native()
            .expect("the native message")
            .payload
            .get()
            .contains("partial_answer")
    );
}

#[test]
fn a_plans_explanation_is_its_text() {
    let plan = r#"{"method": "turn/plan/updated", "params": {"threadId": "th1", "turnId": "t1", "explanation": "Two steps", "plan": [{"step": "Read", "status": "completed"}, {"step": "Write", "status": "inProgress"}]}}"#;
    let events = feed(&[(Direction::In, plan)]);
    let Body::Plan(p) = events[0].body() else {
        panic!("a plan item");
    };
    assert_eq!(p.text.as_deref(), Some("Two steps"));
    assert_eq!(p.entries.len(), 2);
    assert_eq!(events[0].header().state, State::Running);
    assert_eq!(events[0].header().text, "[x] Read\n[ ] Write");
}

#[test]
fn the_native_message_is_the_text_that_arrived() {
    // Spacing, field order and the way a number is written all survive.
    let params = r#"{ "threadId":"th1",  "turnId" : "t1", "itemId":"x", "willRetry":false, "error":{"message":"slow down","n":1.50} }"#;
    let message = format!(r#"{{"method":"error","params":{params}}}"#);
    let events = feed(&[(Direction::In, &message)]);
    assert_eq!(
        events[0]
            .native()
            .expect("the native message")
            .payload
            .get(),
        params
    );
}

#[test]
fn a_command_codex_never_closed_is_closed_with_no_native_message() {
    let started = r#"{"method": "item/started", "params": {"threadId": "th1", "turnId": "t1", "item": {"type": "commandExecution", "id": "cmd1", "command": "sleep 60", "cwd": "/w", "status": "inProgress"}}}"#;
    let ended = r#"{"method": "turn/completed", "params": {"threadId": "th1", "turn": {"id": "t1", "status": "interrupted", "items": [], "error": null}}}"#;
    let events = feed(&[(Direction::In, started), (Direction::In, ended)]);
    let closed = &events[1];
    assert_eq!(closed.header().item.as_str(), "cmd1");
    assert_eq!(closed.header().state, State::Cancelled);
    assert!(closed.native().is_none(), "Codex said nothing about it");
    assert!(matches!(events[2].body(), Body::Turn(_)));
}

#[test]
fn a_harness_that_stops_mid_turn_closes_the_turn() {
    let started = r#"{"method": "turn/started", "params": {"threadId": "th1", "turn": {"id": "t1", "items": [], "status": "inProgress", "error": null}}}"#;
    let mut translator = Translator::new();
    translator.feed(Direction::In, started);
    let closed = translator.harness_stopped();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].header().state, State::Cancelled);
    assert!(closed[0].native().is_none());
}

#[test]
fn seq_counts_within_each_session() {
    let first = r#"{"method": "thread/started", "params": {"thread": {"id": "th1", "cliVersion": "0.169.0"}}}"#;
    let warn = r#"{"method": "warning", "params": {"threadId": "th1", "message": "careful"}}"#;
    let second = r#"{"method": "thread/started", "params": {"thread": {"id": "th2", "cliVersion": "0.169.0"}}}"#;
    let events = feed(&[
        (Direction::In, first),
        (Direction::In, warn),
        (Direction::In, second),
        (Direction::In, warn),
    ]);
    let seqs: Vec<(String, u64)> = events
        .iter()
        .map(|e| {
            let h = e.header();
            (
                h.session
                    .as_ref()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                h.seq,
            )
        })
        .collect();
    // th1: its session item, the warning, then its session ending; th2: its item, the warning.
    assert_eq!(
        seqs,
        [("th1", 1), ("th1", 2), ("th1", 3), ("th2", 1), ("th2", 2)]
            .map(|(s, n)| (s.to_string(), n))
    );
    assert!(matches!(events[1].body(), Body::Notice(n) if n.kind == NoticeKind::Warning));
}
