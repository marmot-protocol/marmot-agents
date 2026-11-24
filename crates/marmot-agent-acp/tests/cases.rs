//! What no recording holds. ACP lets an agent stream a message with no `messageId`; its chunks
//! still make one item, and a message after its thinking is an item of its own. A turn ends only
//! with the reply to the prompt that started it, even one the types cannot read. The prototype has
//! the same cases (`tools/prototype/test_translate.py`). The shapes are those of
//! `fixtures/recordings/acp/opencode/simple.jsonl`, with the ids left out.

use marmot_agent_acp::{Direction, Translator};
use marmot_agent_events::{AgentEvent, Body, State};

fn run_turn(chunks: &[(&str, &str)]) -> Vec<(&'static str, String)> {
    let mut acp = Translator::new();
    let mut events = Vec::new();
    let mut feed = |direction, message: String| events.extend(acp.feed(direction, &message));
    feed(
        Direction::Out,
        r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/work/demo","mcpServers":[]}}"#.into(),
    );
    feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s1"}}"#.into(),
    );
    feed(
        Direction::Out,
        r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"s1","prompt":[]}}"#.into(),
    );
    for (kind, text) in chunks {
        feed(
            Direction::In,
            format!(
                r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"sessionUpdate":"{kind}","content":{{"type":"text","text":"{text}"}}}}}}}}"#
            ),
        );
    }
    feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":2,"result":{"stopReason":"end_turn"}}"#.into(),
    );
    events
        .iter()
        .filter_map(|e| match e.body() {
            Body::Message(_) => Some(("message", e.header().text.clone())),
            Body::Reasoning(_) => Some(("reasoning", e.header().text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn chunks_with_no_id_make_one_item() {
    let made = run_turn(&[
        ("agent_message_chunk", "Hel"),
        ("agent_message_chunk", "lo"),
    ]);
    assert_eq!(made, vec![("message", "Hello".to_string())]);
}

#[test]
fn thinking_then_words_make_two_items() {
    let made = run_turn(&[
        ("agent_thought_chunk", "Hm"),
        ("agent_message_chunk", "Hi"),
        ("agent_message_chunk", "!"),
    ]);
    assert_eq!(
        made,
        vec![
            ("reasoning", "Hm".to_string()),
            ("message", "Hi!".to_string())
        ]
    );
}

/// The turns a translator reported: each turn event's turn and state.
fn turns(events: &[AgentEvent]) -> Vec<(String, State)> {
    events
        .iter()
        .filter(|e| matches!(e.body(), Body::Turn(_)))
        .map(|e| {
            let turn = e.header().turn.as_ref().map(|t| t.as_str().to_string());
            (turn.unwrap_or_default(), e.header().state)
        })
        .collect()
}

/// A translator with a session and a turn started by the prompt with id 2.
fn started() -> (Translator, Vec<AgentEvent>) {
    started_in("s1")
}

/// A translator with session `session` and a turn started by the prompt with id 2.
fn started_in(session: &str) -> (Translator, Vec<AgentEvent>) {
    let mut acp = Translator::new();
    let mut events = Vec::new();
    for (direction, message) in [
        (
            Direction::Out,
            r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/work/demo","mcpServers":[]}}"#,
        ),
        (
            Direction::In,
            r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s1"}}"#,
        ),
        (
            Direction::Out,
            r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"s1","prompt":[]}}"#,
        ),
    ] {
        let message = message.replace(
            r#""sessionId":"s1""#,
            &format!(r#""sessionId":"{session}""#),
        );
        events.extend(acp.feed(direction, &message));
    }
    (acp, events)
}

#[test]
fn a_prompt_during_a_turn_and_its_refusal_leave_the_turn_running() {
    let (mut acp, mut events) = started();
    events.extend(acp.feed(
        Direction::Out,
        r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"s1","prompt":[]}}"#,
    ));
    let refused = acp.feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32600,"message":"busy"}}"#,
    );
    assert!(matches!(refused.as_slice(), [e] if matches!(e.body(), Body::Error(_))));
    events.extend(refused);
    assert_eq!(turns(&events), vec![("s1:2".to_string(), State::Running)]);
    events.extend(acp.feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":2,"result":{"stopReason":"end_turn"}}"#,
    ));
    assert_eq!(
        turns(&events),
        vec![
            ("s1:2".to_string(), State::Running),
            ("s1:2".to_string(), State::Done)
        ]
    );
}

#[test]
fn a_reply_with_a_stop_reason_nobody_knows_ends_the_turn_as_failed() {
    let (mut acp, mut events) = started();
    events.extend(acp.feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":2,"result":{"stopReason":"something_new"}}"#,
    ));
    assert_eq!(
        turns(&events),
        vec![
            ("s1:2".to_string(), State::Running),
            ("s1:2".to_string(), State::Failed)
        ]
    );
}

/// A permission request of the agent's, with its JSON-RPC id, for a tool call of session `s1`.
fn permission(id: &str, call: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"session/request_permission","params":{{"sessionId":"s1","toolCall":{{"toolCallId":"{call}"}},"options":[{{"optionId":"once","name":"Allow once","kind":"allow_once"}}]}}}}"#
    )
}

/// A form of the agent's, with its JSON-RPC id, for a tool call of session `s1`: one question
/// that takes typed text.
fn form(id: &str, call: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"elicitation/create","params":{{"mode":"form","sessionId":"s1","toolCallId":"{call}","message":"Questions","requestedSchema":{{"type":"object","properties":{{"q0":{{"type":"string","title":"Pick"}}}}}}}}}}"#
    )
}

// A second request for one tool call is an item of its own, an approval or a question, and its
// name must not be one a first request could have: here a tool call whose id holds the delimiter
// and the second request's id. Each item takes an answer that reaches its own request, and only it.
#[test]
fn a_repeated_request_takes_no_name_a_first_could_have() {
    use marmot_agent_driver::{Answer, Given, Person, Reply};
    let chosen = Given::Choice("c0".into());
    let typed = Given::Questions(vec![Reply {
        question: "q0".into(),
        options: Vec::new(),
        text: Some("Yes".into()),
    }]);
    type Request = fn(&str, &str) -> String;
    let kinds: [(&str, Request, Given); 2] =
        [("approval", permission, chosen), ("question", form, typed)];
    let first = [("9", "t:7"), ("5", "t"), ("7", "t")];
    let repeat_first = [("5", "t"), ("7", "t"), ("9", "t:7")];
    for (kind, request, given) in kinds {
        for asked in [first, repeat_first] {
            let (mut acp, _) = started();
            let mut items = Vec::new();
            for (id, call) in asked {
                for e in acp.feed(Direction::In, &request(id, call)) {
                    if matches!(e.body(), Body::Approval(_) | Body::Question(_)) {
                        items.push((id, e.header().item.clone()));
                    }
                }
            }
            let names: std::collections::HashSet<_> =
                items.iter().map(|(_, i)| i.clone()).collect();
            assert_eq!(
                names.len(),
                3,
                "{kind}: each request is an item of its own: {asked:?}"
            );
            for (id, item) in &items {
                let answer = Answer {
                    request: "r".into(),
                    item: item.clone(),
                    by: Person {
                        id: "u-1".into(),
                        name: "Alice".into(),
                    },
                    given: given.clone(),
                };
                let reply = acp.reply(&answer).expect("the item waits");
                let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(
                    reply["id"].to_string(),
                    *id,
                    "{kind}, {asked:?}: each reaches its own request"
                );
            }
        }
    }
}

// A repeated request's name must not be one a tool call could have either: a tool item is named
// `{session}:{tool call}`, and both are the agent's. Here the session is named like a repeat, so a
// repeat named `{kind}+{request}:{session}:{tool call}` is the name of the tool call
// `{session}:{tool call}` of that session; when that tool call is reported after the request, the
// core would take the tool event for the request's. The request keeps its own item, and its answer
// still reaches it.
#[test]
fn a_repeated_request_takes_no_name_a_tool_call_could_have() {
    use marmot_agent_driver::{Answer, Given, Person, Reply};
    let chosen = Given::Choice("c0".into());
    let typed = Given::Questions(vec![Reply {
        question: "q0".into(),
        options: Vec::new(),
        text: Some("Yes".into()),
    }]);
    type Request = fn(&str, &str) -> String;
    let kinds: [(&str, Request, Given); 2] =
        [("approval", permission, chosen), ("question", form, typed)];
    for (kind, request, given) in kinds {
        let session = format!("{kind}+7");
        let in_session = |m: String| {
            m.replace(
                r#""sessionId":"s1""#,
                &format!(r#""sessionId":"{session}""#),
            )
        };
        let (mut acp, _) = started_in(&session);
        let mut asked = Vec::new();
        for id in ["5", "7"] {
            for e in acp.feed(Direction::In, &in_session(request(id, "t"))) {
                if matches!(e.body(), Body::Approval(_) | Body::Question(_)) {
                    asked.push((id, e.header().item.clone()));
                }
            }
        }
        let tool = format!(
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{session}","update":{{"sessionUpdate":"tool_call","toolCallId":"{session}:t","title":"Run","kind":"execute","status":"pending"}}}}}}"#
        );
        let tools: Vec<_> = acp
            .feed(Direction::In, &tool)
            .iter()
            .filter(|e| matches!(e.body(), Body::Tool(_)))
            .map(|e| e.header().item.clone())
            .collect();
        assert_eq!(tools.len(), 1, "{kind}: the tool call is reported");
        for (id, item) in &asked {
            assert!(
                !tools.contains(item),
                "{kind}: request {id}'s item {item} is the tool call's",
                item = item.as_str()
            );
            let answer = Answer {
                request: "r".into(),
                item: item.clone(),
                by: Person {
                    id: "u-1".into(),
                    name: "Alice".into(),
                },
                given: given.clone(),
            };
            let reply = acp.reply(&answer).expect("the item waits");
            let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(
                reply["id"].to_string(),
                *id,
                "{kind}: the answer reaches its request"
            );
        }
    }
}

/// The plan items' entries, as text, in the order the events came.
fn plans(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| matches!(e.body(), Body::Plan(_)))
        .map(|e| e.header().text.clone())
        .collect()
}

#[test]
fn a_to_do_list_with_no_params_is_unreadable_and_leaves_the_plan_alone() {
    let (mut acp, _) = started();
    let first = acp.feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":7,"method":"cursor/update_todos","params":{"merge":false,"todos":[{"id":"1","content":"Read","status":"in_progress"}]}}"#,
    );
    assert_eq!(plans(&first), vec!["[ ] Read".to_string()]);
    // Not `{}`: a message with no params at all says nothing, so it neither clears the list nor
    // makes an event with a native message that never arrived (D-007).
    let bare = acp.feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":8,"method":"cursor/update_todos"}"#,
    );
    assert!(bare.is_empty(), "made {bare:?}");
    assert_eq!(acp.unreadable(), 1);
    let merged = acp.feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":9,"method":"cursor/update_todos","params":{"merge":true,"todos":[{"id":"2","content":"Write","status":"pending"}]}}"#,
    );
    assert_eq!(plans(&merged), vec!["[ ] Read\n[ ] Write".to_string()]);
    // A real empty object is what arrived, so it is the native message, and it empties the list.
    let empty = acp.feed(
        Direction::In,
        r#"{"jsonrpc":"2.0","id":10,"method":"cursor/update_todos","params":{}}"#,
    );
    assert_eq!(plans(&empty), vec![String::new()]);
    let native = empty[0].native().expect("a native message");
    assert_eq!(native.payload.get(), "{}");
    assert_eq!(acp.unreadable(), 1);
}
