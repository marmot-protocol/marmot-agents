//! What no recording holds. ACP lets an agent stream a message with no `messageId`; its chunks
//! still make one item, and a message after its thinking is an item of its own. The prototype has
//! the same cases (`tools/prototype/test_translate.py`). The shapes are those of
//! `fixtures/recordings/acp/opencode/simple.jsonl`, with the ids left out.

use marmot_agent_acp::{Direction, Translator};
use marmot_agent_events::Body;

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
