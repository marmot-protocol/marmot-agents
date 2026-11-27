//! What no recording holds, with records written by hand in the shapes Pi's RPC documentation
//! gives them (`docs/rpc-commands.md` and `docs/json.md` of Pi 1.6.0). The recordings are the
//! main test (`replay.rs`).

use marmot_agent_events::{AgentEvent, Body};
use marmot_agent_pi::{Direction, Translator};

/// The titles the session items `records` make show, in order.
fn titles(records: &[&str]) -> Vec<Option<String>> {
    let mut pi = Translator::new();
    pi.set_version(Some("1.6.0".into()));
    let events: Vec<AgentEvent> = (records.iter())
        .flat_map(|r| pi.feed(Direction::In, r))
        .collect();
    assert!(pi.unknown().is_empty(), "{:?}", pi.unknown());
    events
        .iter()
        .filter_map(|e| match e.body() {
            Body::Session(s) => Some(s.title.clone()),
            _ => None,
        })
        .collect()
}

const COMMANDS: &str = r#"{"id": "2", "type": "response", "command": "get_commands", "success": true, "data": {"commands": []}}"#;

// D-177: Pi's session name is the session's title. Pi names no session by itself: an extension
// or `--name` does, and `get_state` says it, or `session_info_changed` when it changes, with no
// `name` when it was cleared. Each change is an edit of the session item.
#[test]
fn pis_session_name_is_the_sessions_title() {
    let state = r#"{"id": "1", "type": "response", "command": "get_state", "success": true, "data": {"sessionId": "s1", "sessionName": "Fix the build", "thinkingLevel": "low"}}"#;
    assert_eq!(
        titles(&[
            state,
            COMMANDS,
            r#"{"type": "session_info_changed", "name": "Fix the tests"}"#,
            r#"{"type": "session_info_changed"}"#,
        ]),
        [
            Some("Fix the build".into()),
            Some("Fix the tests".into()),
            None
        ]
    );
}

// A new session starts with no title: the one before keeps its own.
#[test]
fn a_new_session_takes_no_title_from_the_one_before() {
    let named = r#"{"id": "1", "type": "response", "command": "get_state", "success": true, "data": {"sessionId": "s1", "sessionName": "Fix the build"}}"#;
    let fresh = r#"{"id": "4", "type": "response", "command": "get_state", "success": true, "data": {"sessionId": "s2"}}"#;
    assert_eq!(
        titles(&[named, COMMANDS, fresh, COMMANDS]),
        [
            Some("Fix the build".into()), // s1
            Some("Fix the build".into()), // s1, ended
            None                          // s2
        ]
    );
}
