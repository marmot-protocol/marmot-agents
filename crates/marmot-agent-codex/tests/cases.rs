//! Cases no recording holds, with messages written by hand in the shapes Codex's schema gives
//! them. The recordings are the main test (`replay.rs`); `tools/prototype/test_translate.py`
//! holds the same secret question for the prototype.

use marmot_agent_codex::{Direction, Translator};
use marmot_agent_events::body::{Answered, NoticeKind, StartedBy};
use marmot_agent_events::{AgentEvent, Body, State};

const PASSWORD: &str = "hunter2";

/// A translator whose session is `th1`, which most cases name: a translator makes no event for a
/// thread before its session is picked (D-060).
fn bound() -> Translator {
    let mut translator = Translator::new();
    translator.feed(Direction::Out, PICK);
    translator.feed(Direction::In, &thread("th1"));
    translator
}

/// The events `messages` make in a [`bound`] translator.
fn feed(messages: &[(Direction, &str)]) -> Vec<AgentEvent> {
    let mut translator = bound();
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
    let mut translator = bound();
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

// The end of an older turn, read after the next has started, is that turn's: it closes what was
// open in it and nothing of the running turn, and says who started it (#143).
#[test]
fn a_late_end_of_an_older_turn_closes_only_that_turn() {
    let turn = |method: &str, id: &str| {
        format!(
            r#"{{"method": "{method}", "params": {{"threadId": "th1", "turn": {{"id": "{id}", "items": [], "status": "completed", "error": null}}}}}}"#
        )
    };
    let command = |turn: &str, id: &str| {
        format!(
            r#"{{"method": "item/started", "params": {{"threadId": "th1", "turnId": "{turn}", "item": {{"type": "commandExecution", "id": "{id}", "command": "sleep 60", "cwd": "/w", "status": "inProgress"}}}}}}"#
        )
    };
    let mut translator = bound();
    let start = r#"{"id": 1, "method": "turn/start", "params": {"threadId": "th1", "input": []}}"#;
    translator.feed(Direction::Out, start);
    for message in [
        turn("turn/started", "t0"),
        command("t0", "cmd0"),
        turn("turn/started", "t1"),
        command("t1", "cmd1"),
    ] {
        translator.feed(Direction::In, &message);
    }
    let late = translator.feed(Direction::In, &turn("turn/completed", "t0"));
    let shown: Vec<(&str, Option<&str>, State)> = (late.iter())
        .map(|e| {
            let h = e.header();
            (
                h.item.as_str(),
                h.turn.as_ref().map(|t| t.as_str()),
                h.state,
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("cmd0", Some("t0"), State::Cancelled),
            ("turn:t0", Some("t0"), State::Done)
        ]
    );
    assert_eq!(turn_body(&late[1]).started_by, StartedBy::User);
    assert_eq!(translator.turn(), Some("t1"));
    let end = translator.feed(Direction::In, &turn("turn/completed", "t1"));
    let items: Vec<&str> = end.iter().map(|e| e.header().item.as_str()).collect();
    assert_eq!(items, ["cmd1", "turn:t1"]);
    assert_eq!(turn_body(&end[1]).started_by, StartedBy::Agent);
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
    let mut translator = bound();
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

// The person's next message answers a question asked in a message once Codex takes it: while it
// is on its way, or after Codex refuses it, the question still waits (#36).
#[test]
fn the_next_message_answers_a_question_asked_in_a_message() {
    let follow_up = |id: u32| {
        format!(
            r#"{{"id": {id}, "method": "turn/start", "params": {{"threadId": "th1", "input": []}}}}"#
        )
    };
    let (first, second, th1) = (follow_up(3), follow_up(4), thread("th1"));
    let refused = r#"{"id": 3, "error": {"code": -32600, "message": "busy"}}"#;
    let taken = r#"{"id": 4, "result": {"turn": {"id": "t2", "items": [], "status": "inProgress", "error": null}}}"#;
    // Each question event is in the session and turn it was asked in.
    let in_t1 = |state| (state, Some("th1".to_string()), Some("t1".to_string()));
    let steps = [
        (Direction::Out, PICK, vec![]),
        (Direction::In, th1.as_str(), vec![]),
        (Direction::In, TURN_STARTED, vec![]),
        (
            Direction::In,
            ASKS_IN_A_MESSAGE,
            vec![in_t1(State::Waiting)],
        ),
        (Direction::In, TURN_DONE, vec![]),
        (Direction::Out, first.as_str(), vec![]), // on its way
        (Direction::In, refused, vec![]),
        (Direction::Out, second.as_str(), vec![]),
        (Direction::In, taken, vec![in_t1(State::Done)]),
    ];
    let mut translator = Translator::new();
    for (direction, message, questions) in steps {
        let events = translator.feed(direction, message);
        assert_eq!(question_events(&events), questions, "{message}");
    }
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
    let mut translator = bound();
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
    let mut translator = Translator::new();
    let events: Vec<AgentEvent> = [
        (Direction::Out, PICK.to_string()),
        (Direction::In, thread("th1")),
        (Direction::Out, PICK.to_string()),
        (Direction::In, thread("th2")),
        (Direction::Out, PICK.to_string()),
        (Direction::In, thread("th1")),
    ]
    .iter()
    .flat_map(|(direction, message)| translator.feed(*direction, message))
    .collect();
    let th1: Vec<State> = events
        .iter()
        .filter(|e| e.header().item.as_str() == "session:th1")
        .map(|e| e.header().state)
        .collect();
    assert_eq!(th1, [State::Running, State::Done, State::Running]);
}

fn owned(event: &AgentEvent) -> (Option<String>, Option<String>) {
    let h = event.header();
    (
        h.session.as_ref().map(|s| s.as_str().to_string()),
        h.turn.as_ref().map(|t| t.as_str().to_string()),
    )
}

#[test]
fn a_refused_request_stays_where_it_was_sent() {
    let update = r#"{"id": 7, "method": "turn/settings/update", "params": {"threadId": "th1", "turnId": "t1", "model": "m2"}}"#;
    let refused = r#"{"id": 7, "error": {"code": -32600, "message": "no such turn"}}"#;
    let sent_in_t1 = (Some("th1".to_string()), Some("t1".to_string()));
    // The turn ends before the refusal comes; or the chat moves to another session.
    for later in [
        vec![TURN_DONE.to_string()],
        vec![TURN_DONE.to_string(), PICK.to_string(), thread("th2")],
    ] {
        let mut messages = vec![
            (Direction::Out, PICK.to_string()),
            (Direction::In, thread("th1")),
            (Direction::In, TURN_STARTED.to_string()),
            (Direction::Out, update.to_string()),
        ];
        for message in later {
            let direction = if message == PICK {
                Direction::Out
            } else {
                Direction::In
            };
            messages.push((direction, message));
        }
        messages.push((Direction::In, refused.to_string()));
        let mut translator = Translator::new();
        let events: Vec<_> = (messages.iter())
            .flat_map(|(d, m)| translator.feed(*d, m))
            .collect();
        let error = events.last().unwrap();
        assert!(matches!(error.body(), Body::Error(_)));
        assert_eq!(owned(error), sent_in_t1);
    }
}

#[test]
fn an_mcp_approval_names_the_tool_it_is_for() {
    let tool = |id: &str, name: &str| {
        format!(
            r#"{{"method": "item/started", "params": {{"threadId": "th1", "turnId": "t1", "item": {{"type": "mcpToolCall", "id": "{id}", "server": "demo", "tool": "{name}", "status": "inProgress", "arguments": {{}}}}}}}}"#
        )
    };
    let approval = |id: u32, meta: &str| {
        format!(
            r#"{{"id": {id}, "method": "mcpServer/elicitation/request", "params": {{"threadId": "th1", "turnId": "t1", "serverName": "demo", "mode": "form", "_meta": {{"codex_approval_kind": "mcp_tool_call"{meta}}}, "message": "Allow?", "requestedSchema": {{"type": "object", "properties": {{}}}}}}}}"#
        )
    };
    let events = feed(&[
        (Direction::In, TURN_STARTED),
        (Direction::In, &tool("a", "define_word")),
        (Direction::In, &tool("b", "spell_word")),
        (
            Direction::In,
            &approval(0, r#", "tool_name": "spell_word""#),
        ),
        // Without the tool's name there are two it could be, and it names neither.
        (Direction::In, &approval(1, "")),
    ]);
    let subjects: Vec<_> = events
        .iter()
        .filter_map(|e| match e.body() {
            Body::Approval(a) => Some(a.subject.as_ref().map(|s| s.as_str().to_string())),
            _ => None,
        })
        .collect();
    assert_eq!(subjects, [Some("b".to_string()), None]);
}

#[test]
fn a_turn_the_old_session_was_asked_for_is_not_the_new_sessions() {
    let compact = r#"{"id": 8, "method": "thread/compact/start", "params": {"threadId": "th1"}}"#;
    let turn2 = r#"{"method": "turn/started", "params": {"threadId": "th2", "turn": {"id": "t2", "items": [], "status": "inProgress", "error": null}}}"#;
    let events = feed(&[
        (Direction::Out, PICK),
        (Direction::In, &thread("th1")),
        (Direction::Out, compact),
        (Direction::Out, PICK),
        (Direction::In, &thread("th2")),
        (Direction::In, turn2),
    ]);
    assert_eq!(
        turn_body(events.last().unwrap()).started_by,
        StartedBy::Agent
    );
}

#[test]
fn token_usage_for_another_thread_is_not_counted_in_this_turn() {
    let usage = |thread: &str, total_out: u64, last_out: u64| {
        format!(
            r#"{{"method": "thread/tokenUsage/updated", "params": {{"threadId": "{thread}", "turnId": "t1", "tokenUsage": {{"total": {{"inputTokens": 0, "outputTokens": {total_out}}}, "last": {{"inputTokens": 0, "outputTokens": {last_out}}}}}}}}}"#
        )
    };
    let mut translator = Translator::new();
    let mut events = Vec::new();
    for (direction, message) in [
        (Direction::Out, PICK.to_string()),
        (Direction::In, thread("th1")),
        (Direction::In, TURN_STARTED.to_string()),
        (Direction::In, usage("th9", 5000, 5000)),
        (Direction::In, usage("th1", 40, 40)),
        (Direction::In, TURN_DONE.to_string()),
    ] {
        events.extend(translator.feed(direction, &message));
    }
    let used = turn_body(events.last().unwrap())
        .usage
        .clone()
        .expect("usage");
    assert_eq!(used.output_tokens, Some(40));
    assert_eq!(translator.other_threads(), 1);
}

// `serverRequest/resolved` for another thread is counted as another thread's message (D-060), as
// every other message for another thread is.
#[test]
fn a_request_resolved_in_another_thread_is_counted() {
    let mut translator = Translator::new();
    for (direction, message) in [
        (Direction::Out, PICK.to_string()),
        (Direction::In, thread("th1")),
        (Direction::In, TURN_STARTED.to_string()),
        (
            Direction::In,
            r#"{"id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "ls"}}"#.to_string(),
        ),
        (
            Direction::In,
            r#"{"method": "serverRequest/resolved", "params": {"threadId": "th9", "requestId": 0}}"#.to_string(),
        ),
    ] {
        translator.feed(direction, &message);
    }
    assert_eq!(translator.other_threads(), 1);
}

// A message from Codex with a method and no params is not one Codex writes. Read as if its params
// were empty, it made an event whose native message, `{}`, never arrived (D-007): the fuzzer found
// it with a `thread/started` whose `params` key was misspelt. It is counted as unreadable and
// shows nothing; a request among such messages is still refused, as Codex waits for a reply.
#[test]
fn a_message_with_no_params_makes_up_no_native_message() {
    let mut translator = Translator::new();
    for notification in [
        r#"{"method":"thread/started","paramw":{"thread":{"id":"th1"}}}"#,
        r#"{"method":"item/started"}"#,
        r#"{"method":"turn/started"}"#,
    ] {
        let events = translator.feed(Direction::In, notification);
        assert!(events.is_empty(), "{notification}: {events:?}");
    }
    assert_eq!(translator.unreadable(), 3);
    assert!(
        translator
            .feed(
                Direction::In,
                r#"{"id":7,"method":"item/commandExecution/requestApproval"}"#
            )
            .is_empty()
    );
    let replies = translator.take_replies();
    assert_eq!(replies.len(), 1, "the request is refused");
    assert!(replies[0].contains("\"error\""));
}

// What Codex says of a thread before the reply to `thread/start` picks the session is another
// thread's (D-060): it is counted, and neither shows a session nor gives the one picked its
// settings. Here thread `thB` reports a model before the reply picks `th1`, which names none
// (#37).
#[test]
fn a_thread_reported_before_the_session_is_picked_gives_it_nothing() {
    let mut translator = Translator::new();
    let early = r#"{"method": "thread/started", "params": {"thread": {"id": "thB", "model": "test-model"}}}"#;
    assert!(translator.feed(Direction::In, early).is_empty());
    translator.feed(Direction::Out, PICK);
    let picked = translator.feed(Direction::In, &thread("th1"));
    let [session] = &picked[..] else {
        panic!("the session item alone: {picked:?}");
    };
    assert_eq!(session.header().item.as_str(), "session:th1");
    let Body::Session(session) = session.body() else {
        panic!("a session");
    };
    let model = session.settings.iter().find(|s| s.id == "model");
    assert_eq!(model.and_then(|s| s.value.as_deref()), None);
    assert_eq!(translator.other_threads(), 1);
}

// Codex counts its requests from 0 again when it restarts. A request in no turn that reuses an
// earlier one's id is a new item, and answering it leaves the earlier one's card as it was (#37).
#[test]
fn a_restarted_codex_reusing_the_id_of_a_request_in_no_turn_makes_a_new_card() {
    let asked = r#"{"id": 0, "method": "mcpServer/elicitation/request", "params": {"threadId": "th1", "serverName": "demo", "mode": "form", "_meta": {"codex_approval_kind": "mcp_tool_call"}, "message": "Allow?", "requestedSchema": {"type": "object", "properties": {}}}}"#;
    let mut translator = bound();
    let first = translator.feed(Direction::In, asked);
    translator.harness_stopped();
    let second = translator.feed(Direction::In, asked);
    let item = |events: &[AgentEvent]| events[0].header().item.as_str().to_string();
    assert_ne!(item(&first), item(&second));
    let answer = r#"{"id": 0, "result": {"action": "accept", "content": null}}"#;
    let answered = translator.feed(Direction::Out, answer);
    let items: Vec<&str> = answered.iter().map(|e| e.header().item.as_str()).collect();
    assert_eq!(items, [item(&second)]);
}

/// A subagent's turn, as its own thread `sub1` reports it.
fn sub_turn(method: &str, turn: &str) -> String {
    format!(
        r#"{{"method": "{method}", "params": {{"threadId": "sub1", "turn": {{"id": "{turn}", "rootTurnId": "t1", "items": [], "status": "completed", "error": null}}}}}}"#
    )
}

/// The subject of the subagent's row, in the last of `events` that shows it.
fn subject(events: &[AgentEvent]) -> Option<String> {
    (events.iter().rev()).find_map(|e| match e.body() {
        Body::Task(task) => Some(task.subject.as_ref().map(|s| s.as_str().to_string())),
        _ => None,
    })?
}

// A subagent's turn can start before the item that gives it its work: it shows the work of the
// turn before until that item comes, and then that item's, as do its words. Codex names each item
// twice, at its start and its end, and an end read late gives no work: not the newest item's,
// nor an older one's (#146).
#[test]
fn work_given_after_a_subagents_turn_started_is_that_turns() {
    let mut sub = Translator::subagent("sub1", Some("th1"), None);
    let mut events = sub.given_work("a1", Some("/root/a"));
    events.extend(sub.feed(Direction::In, &sub_turn("turn/started", "st1")));
    assert_eq!(subject(&events).as_deref(), Some("a1"));
    sub.feed(Direction::In, &sub_turn("turn/completed", "st1"));

    let events = sub.feed(Direction::In, &sub_turn("turn/started", "st2"));
    assert_eq!(subject(&events).as_deref(), Some("a1"), "until a2 comes");
    let events = sub.given_work("a2", Some("/root/b"));
    assert_eq!(
        subject(&events).as_deref(),
        Some("a2"),
        "the running turn's"
    );
    let Some(Body::Task(task)) = events.last().map(AgentEvent::body) else {
        panic!("the row");
    };
    assert_eq!(
        task.description.as_deref(),
        Some("/root/b"),
        "and what a2 is for"
    );
    let said = r#"{"method": "item/completed", "params": {"threadId": "sub1", "turnId": "st2", "item": {"type": "agentMessage", "id": "w2", "text": "done", "phase": "final_answer"}}}"#;
    let words = sub.feed(Direction::In, said);
    let Body::Message(message) = words[0].body() else {
        panic!("its words");
    };
    assert_eq!(message.parent.as_ref().map(|p| p.as_str()), Some("a2"));

    // Each item's end, a1's read late: neither gives the next turn work.
    assert!(sub.given_work("a2", Some("/root/b")).is_empty());
    assert!(sub.given_work("a1", Some("/root/a")).is_empty());
    let events = sub.feed(Direction::In, &sub_turn("turn/completed", "st2"));
    assert_eq!(subject(&events).as_deref(), Some("a2"));
    let events = sub.feed(Direction::In, &sub_turn("turn/started", "st3"));
    assert_eq!(subject(&events).as_deref(), Some("a2"), "not a1 again");
}

/// The titles the session items `events` show, in order.
fn titles(events: &[AgentEvent]) -> Vec<Option<String>> {
    events
        .iter()
        .filter_map(|e| match e.body() {
            Body::Session(s) => Some(s.title.clone()),
            _ => None,
        })
        .collect()
}

// D-177: a thread's name is the session's title. Codex does not name a thread by itself (0.171.0,
// checked by hand), so it has one when it was named before it was resumed, or when it is named
// while it runs, which `thread/name/updated` reports, with null for no name. Another thread's
// name is not this session's.
#[test]
fn a_threads_name_is_the_sessions_title() {
    let mut translator = Translator::new();
    let events: Vec<AgentEvent> = [
        (Direction::Out, r#"{"id": 100, "method": "thread/resume", "params": {}}"#),
        (
            Direction::In,
            r#"{"id": 100, "result": {"thread": {"id": "th1", "cliVersion": "0.171.0", "name": "Fix the build"}}}"#,
        ),
        (
            Direction::In,
            r#"{"method": "thread/name/updated", "params": {"threadId": "th2", "threadName": "Another thread"}}"#,
        ),
        (
            Direction::In,
            r#"{"method": "thread/name/updated", "params": {"threadId": "th1", "threadName": "Fix the tests"}}"#,
        ),
        (
            Direction::In,
            r#"{"method": "thread/settings/updated", "params": {"threadId": "th1", "threadSettings": {"model": "m1"}}}"#,
        ),
        (
            Direction::In,
            r#"{"method": "thread/name/updated", "params": {"threadId": "th1", "threadName": null}}"#,
        ),
    ]
    .iter()
    .flat_map(|(direction, message)| translator.feed(*direction, message))
    .collect();
    assert_eq!(
        titles(&events),
        [
            Some("Fix the build".into()),
            Some("Fix the tests".into()),
            Some("Fix the tests".into()),
            None
        ]
    );
}
