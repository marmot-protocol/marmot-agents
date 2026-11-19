//! The agent events that every Marmot agent driver produces.
//!
//! A driver turns each native harness message into an [`AgentEvent`]. An event has three parts:
//!
//! - an item type from a shared list, with the fields that belong to it: [`Body`];
//! - a plain rendering that any client can show: a title and some text;
//! - the harness's own message, kept exactly as it arrived: [`Native`].
//!
//! Everything here follows `docs/design/event-model.md`. That page is a draft, so it can still
//! change. This crate knows nothing about any harness, about Marmot, or about how an event is
//! carried.

pub mod body;
mod event;

pub use body::Body;
pub use event::{AgentEvent, ItemId, Native, SessionId, TurnId};
use serde::{Deserialize, Serialize};

/// Where an item stands. An item changes by being sent again with a new state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The work is in progress.
    Running,
    /// A person is needed: an approval or a question that nobody has answered.
    Waiting,
    /// It finished.
    Done,
    /// It finished with an error.
    Failed,
    /// A person refused it.
    Declined,
    /// It was stopped before it finished.
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::body::{self, Body};
    use super::{AgentEvent, Native, State};
    use serde_json::json;

    fn event(state: State, body: Body, native: Option<Native>) -> AgentEvent {
        AgentEvent {
            harness: "codex".into(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            item: "i1".into(),
            seq: 3,
            state,
            title: "Running a command".into(),
            text: "python3 greet.py".into(),
            body,
            native,
        }
    }

    fn a_command() -> Body {
        Body::Tool(body::Tool {
            kind: body::ToolKind::Command,
            name: "shell".into(),
            command: Some("python3 greet.py".into()),
            cwd: Some("/work/demo".into()),
            exit_code: None,
            duration_ms: None,
            output: None,
            changes: Vec::new(),
            query: None,
            path: None,
            input: None,
            error: None,
        })
    }

    fn a_session() -> Body {
        Body::Session(body::Session {
            harness_version: "0.169.0".into(),
            settings: Vec::new(),
            actions: Vec::new(),
            commands: Vec::new(),
        })
    }

    fn one_of_each() -> Vec<Body> {
        let turn = body::Turn {
            started_by: body::StartedBy::User,
            duration_ms: None,
            usage: None,
        };
        let approval = body::Approval {
            subject: None,
            choices: Vec::new(),
            answer: None,
        };
        let question = body::Question::new(body::AnswerVia::Request, None, Vec::new());
        let task = body::Task {
            kind: "subagent".into(),
            description: None,
            subject: None,
        };
        let notice = body::Notice {
            kind: body::NoticeKind::Waiting,
            duration_ms: None,
            reason: None,
        };
        vec![
            Body::Turn(turn),
            Body::Message(body::Message::default()),
            Body::Reasoning(body::Reasoning {}),
            a_command(),
            Body::Approval(approval),
            Body::Question(question),
            Body::Plan(body::Plan::default()),
            Body::Task(task),
            Body::Notice(notice),
            Body::Error(body::Error::default()),
            a_session(),
        ]
    }

    fn a_secret_question() -> body::Question {
        let asked = |id: &str, secret: bool| body::Asked {
            id: id.into(),
            header: None,
            text: "?".into(),
            options: Vec::new(),
            multiple: false,
            allow_text: true,
            secret,
        };
        body::Question::new(
            body::AnswerVia::Request,
            None,
            vec![asked("name", false), asked("password", true)],
        )
    }

    fn answered(question: &str, text: Option<&str>) -> body::Answered {
        body::Answered {
            question: question.into(),
            options: Vec::new(),
            text: text.map(str::to_string),
        }
    }

    // The names are part of the protocol: every client matches on them. A rename must fail here.
    #[test]
    fn states_are_written_under_the_names_of_the_design_page() {
        let names = [
            (State::Running, "running"),
            (State::Waiting, "waiting"),
            (State::Done, "done"),
            (State::Failed, "failed"),
            (State::Declined, "declined"),
            (State::Cancelled, "cancelled"),
        ];
        for (state, name) in names {
            assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
        }
    }

    // The eleven types and their names on the wire. Each is read back as the type it was written
    // as, so a body cannot travel under another type's name.
    #[test]
    fn every_item_type_is_written_under_its_own_name() {
        let names = [
            "turn",
            "message",
            "reasoning",
            "tool",
            "approval",
            "question",
            "plan",
            "task",
            "notice",
            "error",
            "session",
        ];
        let bodies = one_of_each();
        assert_eq!(bodies.len(), names.len());
        for (body, name) in bodies.into_iter().zip(names) {
            let event = event(State::Running, body, None);
            let written = serde_json::to_value(&event).unwrap();
            assert_eq!(written["type"], json!(name));
            assert!(written["body"].is_object(), "{name} has no body object");
            let read: AgentEvent = serde_json::from_value(written).unwrap();
            assert_eq!(read, event);
        }
    }

    // What a client receives for one event, field by field.
    #[test]
    fn an_event_is_written_as_the_design_page_describes() {
        let native = Native {
            kind: "item/started [commandExecution]".into(),
            payload: serde_json::value::RawValue::from_string(r#"{"id":"i1"}"#.into()).unwrap(),
        };
        let written =
            serde_json::to_value(event(State::Running, a_command(), Some(native))).unwrap();
        assert_eq!(
            written,
            json!({
                "harness": "codex",
                "session": "s1",
                "turn": "t1",
                "item": "i1",
                "seq": 3,
                "type": "tool",
                "state": "running",
                "title": "Running a command",
                "text": "python3 greet.py",
                "body": {"kind": "command", "name": "shell", "command": "python3 greet.py", "cwd": "/work/demo"},
                "native": {"kind": "item/started [commandExecution]", "payload": {"id": "i1"}},
            })
        );
    }

    // D-007: the native message is never changed. Not its spacing, the order of its fields, or
    // the way a number was written.
    #[test]
    fn the_native_payload_survives_exactly() {
        let payload = r#"{"z": 1,  "a": [1.50, 2e3], "nested": {"b": null, "a": "é"}}"#;
        let text = format!(
            r#"{{"harness":"codex","item":"i1","seq":1,"type":"reasoning","state":"done","title":"Thinking","text":"","body":{{}},"native":{{"kind":"item/completed","payload":{payload}}}}}"#
        );
        let event: AgentEvent = serde_json::from_str(&text).unwrap();
        assert_eq!(event.native.as_ref().unwrap().payload.get(), payload);
        // Two events are equal only when their native messages are the same text. The same
        // values written another way are a different message.
        let respaced: AgentEvent =
            serde_json::from_str(&text.replace("  \"a\"", " \"a\"")).unwrap();
        assert_eq!(event, event.clone());
        assert_ne!(event, respaced);
        let written = serde_json::to_string(&event).unwrap();
        assert!(
            written.contains(&format!(r#""payload":{payload}"#)),
            "{written}"
        );
    }

    // The plain rendering exists so that an item can be shown without knowing its fields. An
    // event from a newer driver, with a field this version does not have, must still be read.
    #[test]
    fn a_field_this_version_does_not_know_does_not_cost_the_event() {
        let newer = json!({
            "harness": "codex", "item": "i1", "seq": 1, "type": "approval", "state": "waiting",
            "title": "Allow this command?", "text": "rm -rf build",
            "body": {"choices": [], "expires_at_ms": 1794910522002u64},
        });
        let event: AgentEvent = serde_json::from_value(newer).unwrap();
        assert_eq!(event.title, "Allow this command?");
        assert_eq!(event.text, "rm -rf build");
        assert!(matches!(event.body, Body::Approval(_)));
    }

    // An event with no session, no turn and no native message leaves those fields out. A notice
    // can be all three: a gateway answers a slash command itself, outside any turn.
    #[test]
    fn absent_parts_are_left_out() {
        let notice = body::Notice {
            kind: body::NoticeKind::CommandOutput,
            duration_ms: None,
            reason: None,
        };
        let mut event = event(State::Done, Body::Notice(notice), None);
        event.session = None;
        event.turn = None;
        let written = serde_json::to_value(&event).unwrap();
        let fields: Vec<&String> = written.as_object().unwrap().keys().collect();
        assert_eq!(
            fields,
            [
                "body", "harness", "item", "seq", "state", "text", "title", "type"
            ]
        );
    }

    // D-030 and D-045: a session always says the harness's version, and a command always has
    // its hint. Neither can be left out of what is written.
    #[test]
    fn a_session_always_carries_the_version_and_each_commands_hint() {
        let session = body::Session {
            harness_version: "0.169.0".into(),
            settings: Vec::new(),
            actions: Vec::new(),
            commands: vec![body::Command {
                name: "compact".into(),
                description: None,
                hint: String::new(),
            }],
        };
        assert_eq!(
            serde_json::to_value(&session).unwrap(),
            json!({"harness_version": "0.169.0", "settings": [], "actions": [], "commands": [{"name": "compact", "hint": ""}]})
        );
        let no_hint = json!({"harness_version": "0.169.0", "commands": [{"name": "compact"}]});
        assert!(serde_json::from_value::<body::Session>(no_hint).is_err());
        let no_version = json!({"settings": []});
        assert!(serde_json::from_value::<body::Session>(no_version).is_err());
    }

    // D-041: the answer to a secret question is never repeated in the item's answer. It cannot
    // be put there, and an item that carries one cannot be read. An answer must also name a
    // question that was asked, or the check on secret questions could be stepped round.
    #[test]
    fn a_secret_answer_is_refused_in_the_answer_fields() {
        let open = a_secret_question();
        let refused = open
            .clone()
            .answered(vec![answered("password", Some("hunter2"))]);
        assert_eq!(
            refused.unwrap_err(),
            body::BadAnswer::Secret {
                question: "password".into()
            }
        );

        let chosen = body::Answered {
            question: "password".into(),
            options: vec!["o1".into()],
            text: None,
        };
        assert!(open.clone().answered(vec![chosen]).is_err());

        let elsewhere = open
            .clone()
            .answered(vec![answered("passw0rd", Some("hunter2"))]);
        assert_eq!(
            elsewhere.unwrap_err(),
            body::BadAnswer::NotAsked {
                question: "passw0rd".into()
            }
        );

        let fine = open
            .answered(vec![
                answered("name", Some("Alice")),
                answered("password", None),
            ])
            .unwrap();
        assert_eq!(fine.answer().unwrap().len(), 2);
        let written = serde_json::to_value(&fine).unwrap();
        assert_eq!(
            written["answer"],
            json!([{"question": "name", "text": "Alice"}, {"question": "password"}])
        );
        assert_eq!(
            serde_json::from_value::<body::Question>(written.clone()).unwrap(),
            fine
        );

        let mut leaking = written;
        leaking["answer"][1]["text"] = json!("hunter2");
        let error = serde_json::from_value::<body::Question>(leaking)
            .unwrap_err()
            .to_string();
        assert!(error.contains("password"), "{error}");
    }
}
