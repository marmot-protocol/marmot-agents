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
        events.extend(acp.feed(direction, message));
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
