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
    let warn = |thread: &str| {
        format!(
            r#"{{"method": "warning", "params": {{"threadId": "{thread}", "message": "careful"}}}}"#
        )
    };
    let mut translator = Translator::new();
    let mut events = Vec::new();
    for (direction, message) in [
        (Direction::Out, PICK.to_string()),
        (Direction::In, thread("th1")),
        (Direction::In, warn("th1")),
        (Direction::Out, PICK.to_string()),
        (Direction::In, thread("th2")),
        (Direction::In, warn("th2")),
        // A late message from the first thread is for another translator: the transport routes
        // each thread's messages. Here it makes no event, and is counted.
        (Direction::In, warn("th1")),
    ] {
        events.extend(translator.feed(direction, &message));
    }
    let seqs: Vec<(String, u64)> = events
        .iter()
        .map(|e| {
            let h = e.header();
            let session = h.session.as_ref().map(|s| s.as_str().to_string());
            (session.unwrap_or_default(), h.seq)
        })
        .collect();
    // th1: its session item, a warning, its session ending; th2: its item, a warning.
    assert_eq!(
        seqs,
        [("th1", 1), ("th1", 2), ("th1", 3), ("th2", 1), ("th2", 2)]
            .map(|(s, n)| (s.to_string(), n))
    );
    assert!(matches!(events[1].body(), Body::Notice(n) if n.kind == NoticeKind::Warning));
    assert_eq!(translator.other_threads(), 1);
}

const TURN_STARTED: &str = r#"{"method": "turn/started", "params": {"threadId": "th1", "turn": {"id": "t1", "items": [], "status": "inProgress", "error": null}}}"#;
const TURN_DONE: &str = r#"{"method": "turn/completed", "params": {"threadId": "th1", "turn": {"id": "t1", "items": [], "status": "completed", "error": null, "durationMs": 900}}}"#;

fn turn_body(event: &AgentEvent) -> &marmot_agent_events::body::Turn {
    let Body::Turn(turn) = event.body() else {
        panic!("a turn event");
    };
    turn
}

#[test]
fn a_turn_reports_every_model_call_it_made() {
    // `last` is one call; `total` is the thread. Two calls of 37 and 19 output tokens follow
    // 1,000 tokens from earlier turns.
    let usage = |total_in: u64, total_out: u64, last_in: u64, last_out: u64| {
        format!(
            r#"{{"method": "thread/tokenUsage/updated", "params": {{"threadId": "th1", "turnId": "t1", "tokenUsage": {{"total": {{"inputTokens": {total_in}, "outputTokens": {total_out}}}, "last": {{"inputTokens": {last_in}, "outputTokens": {last_out}}}}}}}}}"#
        )
    };
    let events = feed(&[
        (Direction::In, TURN_STARTED),
        (Direction::In, &usage(1100, 1037, 100, 37)),
        (Direction::In, &usage(1300, 1056, 200, 19)),
        (Direction::In, TURN_DONE),
    ]);
    let used = turn_body(&events[1]).usage.clone().expect("usage");
    assert_eq!(
        (used.input_tokens, used.output_tokens),
        (Some(300), Some(56))
    );
}

#[test]
fn a_request_between_starting_a_turn_and_its_start_does_not_change_who_started_it() {
    let events = feed(&[
        (
            Direction::Out,
            r#"{"id": 1, "method": "turn/start", "params": {"threadId": "th1", "input": []}}"#,
        ),
        (
            Direction::Out,
            r#"{"id": 2, "method": "model/list", "params": {}}"#,
        ),
        (Direction::In, TURN_STARTED),
    ]);
    assert_eq!(
        turn_body(&events[0]).started_by,
        marmot_agent_events::body::StartedBy::User
    );
}

#[test]
fn a_turn_nobody_asked_for_was_started_by_the_agent() {
    let events = feed(&[(Direction::In, TURN_STARTED)]);
    assert_eq!(
        turn_body(&events[0]).started_by,
        marmot_agent_events::body::StartedBy::Agent
    );
}

#[test]
fn what_follows_a_turn_does_not_belong_to_it() {
    let refused = r#"{"id": 5, "error": {"code": -32600, "message": "no such setting"}}"#;
    let events = feed(&[
        (Direction::In, TURN_STARTED),
        (Direction::In, TURN_DONE),
        (
            Direction::Out,
            r#"{"id": 5, "method": "thread/settings/update", "params": {"threadId": "th1"}}"#,
        ),
        (Direction::In, refused),
    ]);
    assert!(matches!(events[2].body(), Body::Error(_)));
    assert_eq!(events[2].header().turn, None);
}

#[test]
fn an_approval_policy_can_be_an_object() {
    let started = r#"{"method": "thread/started", "params": {"thread": {"id": "th1", "cliVersion": "0.169.0", "model": "m1", "approvalPolicy": {"granular": {"mcp_elicitations": true, "rules": true, "sandbox_approval": false}}}}}"#;
    let events = feed(&[(Direction::In, started)]);
    let Body::Session(session) = events[0].body() else {
        panic!("a session item");
    };
    let value = |id: &str| {
        session
            .settings
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.value.clone())
    };
    assert_eq!(value("approval_policy").as_deref(), Some("granular"));
    assert_eq!(
        value("model").as_deref(),
        Some("m1"),
        "the other settings survive"
    );
}

#[test]
fn a_permission_request_is_granted_for_a_scope_or_denied() {
    let asked = r#"{"id": 9, "method": "item/permissions/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "call9", "cwd": "/w", "startedAtMs": 1, "reason": "needs the network", "permissions": {"network": {"enabled": true}}}}"#;
    let reply = |grant: &str| format!(r#"{{"id": 9, "result": {grant}}}"#);
    for (grant, state, choice) in [
        (
            r#"{"permissions": {"network": {"enabled": true}}, "scope": "session"}"#,
            State::Done,
            "c1",
        ),
        (
            r#"{"permissions": {"network": {"enabled": true}}}"#,
            State::Done,
            "c0",
        ),
        (r#"{"permissions": {}}"#, State::Declined, "c2"),
        (
            r#"{"permissions": {"network": {"enabled": false}}}"#,
            State::Declined,
            "c2",
        ),
        (
            r#"{"permissions": {"fileSystem": {"read": [], "write": []}}}"#,
            State::Declined,
            "c2",
        ),
    ] {
        let events = feed(&[(Direction::In, asked), (Direction::Out, &reply(grant))]);
        let Body::Approval(approval) = events[1].body() else {
            panic!("an approval");
        };
        assert_eq!(events[1].header().state, state, "{grant}");
        assert_eq!(
            approval.answer.as_ref().and_then(|a| a.choice.as_deref()),
            Some(choice),
            "{grant}"
        );
    }
}

#[test]
fn a_turn_end_the_driver_cannot_read_closes_nothing() {
    let mut translator = Translator::new();
    let started = r#"{"method": "item/started", "params": {"threadId": "th1", "turnId": "t1", "item": {"type": "commandExecution", "id": "cmd1", "command": "make", "cwd": "/w", "status": "inProgress"}}}"#;
    translator.feed(Direction::In, started);
    let odd = r#"{"method": "turn/completed", "params": {"threadId": "th1", "turn": {"id": "t1", "status": "completed", "error": "a string where an object goes"}}}"#;
    assert!(translator.feed(Direction::In, odd).is_empty());
    assert_eq!(translator.unknown().len(), 1);
}

const ASKS_IN_A_MESSAGE: &str = r#"{"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1", "item": {"type": "agentMessage", "id": "msg1", "text": "Hello or Hi?", "phase": "final_answer", "questions": [{"title": "Which greeting?", "options": ["Hello", "Hi"]}]}}}"#;

/// The client asks for a new session. Codex's reply, [`thread`], picks it.
const PICK: &str = r#"{"id": 100, "method": "thread/start", "params": {}}"#;

fn thread(id: &str) -> String {
    format!(r#"{{"id": 100, "result": {{"thread": {{"id": "{id}", "cliVersion": "0.169.0"}}}}}}"#)
}

fn question_events(events: &[AgentEvent]) -> Vec<(State, Option<String>, Option<String>)> {
    events
        .iter()
        .filter(|e| matches!(e.body(), Body::Question(_)))
        .map(|e| {
            let h = e.header();
            (
                h.state,
                h.session.as_ref().map(|s| s.as_str().to_string()),
                h.turn.as_ref().map(|t| t.as_str().to_string()),
            )
        })
        .collect()
}

#[test]
fn the_next_message_answers_a_question_asked_in_a_message() {
    let events = feed(&[
        (Direction::Out, PICK),
        (Direction::In, &thread("th1")),
        (Direction::In, TURN_STARTED),
        (Direction::In, ASKS_IN_A_MESSAGE),
        (Direction::In, TURN_DONE),
        (
            Direction::Out,
            r#"{"id": 3, "method": "turn/start", "params": {"threadId": "th1", "input": []}}"#,
        ),
    ]);
    let th1 = Some("th1".to_string());
    let t1 = Some("t1".to_string());
    assert_eq!(
        question_events(&events),
        [
            (State::Waiting, th1.clone(), t1.clone()),
            (State::Done, th1, t1)
        ]
    );
}

#[test]
fn a_new_session_leaves_such_a_question_unanswered_in_its_own_session() {
    let events = feed(&[
        (Direction::Out, PICK),
        (Direction::In, &thread("th1")),
        (Direction::In, TURN_STARTED),
        (Direction::In, ASKS_IN_A_MESSAGE),
        (Direction::In, TURN_DONE),
        (Direction::Out, PICK),
        (Direction::In, &thread("th2")),
    ]);
    let th1 = Some("th1".to_string());
    let t1 = Some("t1".to_string());
    assert_eq!(
        question_events(&events),
        [
            (State::Waiting, th1.clone(), t1.clone()),
            (State::Cancelled, th1, t1)
        ]
    );
}

#[test]
fn a_restarted_codex_reusing_a_request_id_makes_a_new_approval() {
    let asked = |turn: &str| {
        format!(
            r#"{{"id": 0, "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th1", "turnId": "{turn}", "itemId": "c", "command": "ls"}}}}"#
        )
    };
    let mut translator = Translator::new();
    let first = translator.feed(Direction::In, &asked("t1"));
    translator.harness_stopped();
    let second = translator.feed(Direction::In, &asked("t2"));
    assert_ne!(first[0].header().item, second[0].header().item);
}

#[test]
fn a_list_of_models_that_arrives_late_reaches_the_session() {
    let events = feed(&[
        (
            Direction::Out,
            r#"{"id": 4, "method": "model/list", "params": {}}"#,
        ),
        (Direction::Out, PICK),
        (Direction::In, &thread("th1")),
        (
            Direction::In,
            r#"{"id": 4, "result": {"data": [{"id": "m1"}, {"id": "m2"}]}}"#,
        ),
    ]);
    let Body::Session(session) = events.last().expect("an event").body() else {
        panic!("a session edit");
    };
    let model = session
        .settings
        .iter()
        .find(|s| s.id == "model")
        .expect("model");
    assert_eq!(
        model.options,
        Some(vec!["m1".to_string(), "m2".to_string()])
    );
}

#[test]
fn a_question_that_outlived_its_turn_is_closed_in_that_turn() {
    let later = r#"{"method": "turn/started", "params": {"threadId": "th1", "turn": {"id": "t2", "items": [], "status": "inProgress", "error": null}}}"#;
    let interrupted = r#"{"method": "turn/completed", "params": {"threadId": "th1", "turn": {"id": "t2", "items": [], "status": "interrupted", "error": null}}}"#;
    let asked = [
        (Direction::Out, PICK.to_string()),
        (Direction::In, thread("th1")),
        (Direction::In, TURN_STARTED.to_string()),
        (Direction::In, ASKS_IN_A_MESSAGE.to_string()),
        (Direction::In, TURN_DONE.to_string()),
    ];
    let th1 = Some("th1".to_string());
    let t1 = Some("t1".to_string());
    let cancelled = (State::Cancelled, th1.clone(), t1.clone());

    // The harness stops.
    let mut translator = Translator::new();
    let mut events: Vec<AgentEvent> = asked
        .iter()
        .flat_map(|(d, m)| translator.feed(*d, m))
        .collect();
    events.extend(translator.harness_stopped());
    assert_eq!(question_events(&events).last(), Some(&cancelled));

    // A later turn, started by Codex itself, is interrupted.
    let mut translator = Translator::new();
    let mut events: Vec<AgentEvent> = asked
        .iter()
        .flat_map(|(d, m)| translator.feed(*d, m))
        .collect();
    events.extend(translator.feed(Direction::In, later));
    events.extend(translator.feed(Direction::In, interrupted));
    assert_eq!(question_events(&events).last(), Some(&cancelled));
}

#[test]
fn a_refused_request_and_an_error_notice_are_different_items() {
    let events = feed(&[
        (
            Direction::Out,
            r#"{"id": 3, "method": "thread/settings/update", "params": {"threadId": "th1"}}"#,
        ),
        (
            Direction::In,
            r#"{"id": 3, "error": {"code": -32600, "message": "no"}}"#,
        ),
        (
            Direction::In,
            r#"{"method": "error", "params": {"threadId": "th1", "error": {"message": "later"}, "willRetry": false}}"#,
        ),
    ]);
    assert_ne!(events[0].header().item, events[1].header().item);
}

#[test]
fn a_session_the_chat_goes_back_to_runs_again() {
    let events = feed(&[
        (Direction::Out, PICK),
        (Direction::In, &thread("th1")),
        (Direction::Out, PICK),
        (Direction::In, &thread("th2")),
        (Direction::Out, PICK),
        (Direction::In, &thread("th1")),
    ]);
    let th1: Vec<State> = events
        .iter()
        .filter(|e| e.header().item.as_str() == "session:th1")
        .map(|e| e.header().state)
        .collect();
    assert_eq!(th1, [State::Running, State::Done, State::Running]);
}
