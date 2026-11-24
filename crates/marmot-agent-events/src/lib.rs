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

/// Implements `Debug` to print a type's name and nothing it holds (D-071).
macro_rules! opaque_debug {
    ($($name:ident),* $(,)?) => {
        $(
            impl std::fmt::Debug for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str(concat!(stringify!($name), " { .. }"))
                }
            }
        )*
    };
}
pub(crate) use opaque_debug;

pub mod body;
mod event;

pub use body::Body;
pub use event::{AgentEvent, Header, ItemId, Native, RepeatsSecret, SessionId, TurnId};
use serde::{Deserialize, Serialize};

/// Where an item stands. An item changes by being sent again with a new state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    use super::{AgentEvent, Header, Native, RepeatsSecret, State};
    use serde_json::json;

    fn header(state: State) -> Header {
        Header {
            harness: "codex".into(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            subagent: None,
            item: "i1".into(),
            seq: 3,
            state,
            title: "Running a command".into(),
            text: "python3 greet.py".into(),
        }
    }

    fn event(state: State, body: Body, native: Option<Native>) -> AgentEvent {
        AgentEvent::new(header(state), body, native).unwrap()
    }

    fn native(payload: &str) -> Native {
        Native {
            kind: "item/started [commandExecution]".into(),
            payload: serde_json::value::RawValue::from_string(payload.into()).unwrap(),
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
        let question = body::Question::new(body::AnswerVia::Request, None, Vec::new()).unwrap();
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

    fn asked(id: &str, secret: bool) -> body::Asked {
        body::Asked {
            id: id.into(),
            header: None,
            text: "?".into(),
            options: Vec::new(),
            multiple: false,
            allow_text: true,
            secret,
        }
    }

    fn a_secret_question() -> body::Question {
        let questions = vec![asked("name", false), asked("password", true)];
        body::Question::new(body::AnswerVia::Request, None, questions).unwrap()
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
        let native = native(r#"{"id":"i1"}"#);
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
        assert_eq!(event.native().unwrap().payload.get(), payload);
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
        assert_eq!(event.header().title, "Allow this command?");
        assert_eq!(event.header().text, "rm -rf build");
        assert!(matches!(event.body(), Body::Approval(_)));
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
        let mut header = header(State::Done);
        header.session = None;
        header.turn = None;
        let event = AgentEvent::new(header, Body::Notice(notice), None).unwrap();
        let written = serde_json::to_value(&event).unwrap();
        for absent in ["session", "turn", "native"] {
            assert!(written.get(absent).is_none(), "{absent} was written");
        }
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
    // be put there, and an item that carries one cannot be read.
    #[test]
    fn a_secret_answer_is_refused_in_the_answer_fields() {
        let open = a_secret_question();
        let refused = open
            .clone()
            .answered(vec![answered("password", Some("hunter2"))]);
        assert_eq!(
            refused.unwrap_err(),
            body::BadQuestion::SecretAnswer {
                question: "password".into()
            }
        );

        let chosen = body::Answered {
            question: "password".into(),
            options: vec!["o1".into()],
            text: None,
        };
        assert!(open.clone().answered(vec![chosen]).is_err());

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
        assert!(error.contains("secret question"), "{error}");
        assert!(
            !error.contains("password") && !error.contains("hunter2"),
            "{error}"
        );
    }

    // An answer has to say which question it is for, or the check above could be stepped round:
    // by an id nobody asked, or by a secret question hiding behind a public one with the same id.
    #[test]
    fn an_answer_names_exactly_one_question() {
        let elsewhere = a_secret_question().answered(vec![answered("passw0rd", Some("hunter2"))]);
        assert_eq!(
            elsewhere.unwrap_err(),
            body::BadQuestion::NotAsked {
                question: "passw0rd".into()
            }
        );

        let public_first = vec![asked("field", false), asked("field", true)];
        let twins = body::Question::new(body::AnswerVia::Request, None, public_first);
        assert_eq!(
            twins.unwrap_err(),
            body::BadQuestion::SameId {
                question: "field".into()
            }
        );

        let twins_as_json = json!({
            "answer_via": "request",
            "questions": [
                {"id": "field", "text": "?"},
                {"id": "field", "text": "?", "secret": true},
            ],
            "answer": [{"question": "field", "text": "hunter2"}],
        });
        assert!(serde_json::from_value::<body::Question>(twins_as_json).is_err());
    }

    // D-056: a question item with a secret question carries a native message only while it is
    // waiting. Then the message is the request, which holds no answer. Afterwards it may be the
    // response, which does. The rule does not depend on an answer having been filled in.
    #[test]
    fn a_secret_question_carries_a_native_message_only_while_waiting() {
        let response = r#"{"answers":{"password":{"answers":["hunter2"]}}}"#;
        let open = a_secret_question();
        let answered = open
            .clone()
            .answered(vec![answered("password", None)])
            .unwrap();

        let waiting = AgentEvent::new(
            header(State::Waiting),
            Body::Question(open.clone()),
            Some(native("{}")),
        );
        assert!(waiting.is_ok());

        // Answered, with the answer left out of the item, or with no answer filled in at all.
        for body in [answered.clone(), open.clone()] {
            for state in [State::Done, State::Cancelled, State::Failed] {
                let refused = AgentEvent::new(
                    header(state),
                    Body::Question(body.clone()),
                    Some(native(response)),
                );
                assert_eq!(refused.unwrap_err(), RepeatsSecret);
            }
        }

        // A header that still says waiting does not let an answered question through.
        let stale = AgentEvent::new(
            header(State::Waiting),
            Body::Question(answered.clone()),
            Some(native(response)),
        );
        assert_eq!(stale.unwrap_err(), RepeatsSecret);

        let sent = AgentEvent::new(header(State::Done), Body::Question(answered), None).unwrap();
        let mut written = serde_json::to_value(&sent).unwrap();
        assert!(!written.to_string().contains("hunter2"));
        assert_eq!(
            serde_json::from_value::<AgentEvent>(written.clone()).unwrap(),
            sent
        );

        written["native"] = json!({"kind": "response", "payload": {"answers": {"password": {"answers": ["hunter2"]}}}});
        let error = serde_json::from_value::<AgentEvent>(written)
            .unwrap_err()
            .to_string();
        assert!(error.contains("secret"), "{error}");

        // A question with nothing secret in it keeps its native message in every state.
        let public =
            body::Question::new(body::AnswerVia::Request, None, vec![asked("name", false)])
                .unwrap();
        assert!(
            AgentEvent::new(
                header(State::Done),
                Body::Question(public),
                Some(native(response))
            )
            .is_ok()
        );
    }

    // What an event prints for debugging holds nothing anyone said or did, and no ids the harness
    // made, so it can be logged (D-071).
    #[test]
    fn a_debug_print_shows_where_an_item_stands_and_nothing_it_holds() {
        let printed = format!(
            "{:?}",
            event(
                State::Running,
                a_command(),
                Some(native(r#"{"secret":"greet"}"#))
            )
        );
        for held in ["greet", "/work", "Running a command", "s1", "t1", "i1"] {
            assert!(!printed.contains(held), "{held} in {printed}");
        }
        for shown in ["codex", "Running", "tool", "item/started"] {
            assert!(printed.contains(shown), "{shown} not in {printed}");
        }
    }

    // Each type that holds something prints none of it on its own, not only inside an event.
    #[test]
    fn the_parts_of_an_item_print_nothing_they_hold() {
        let Body::Tool(tool) = a_command() else {
            unreachable!()
        };
        let printed = format!("{tool:?}");
        assert!(
            !printed.contains("greet") && !printed.contains("/work"),
            "{printed}"
        );
        let bad = body::BadQuestion::SecretAnswer {
            question: "password".into(),
        };
        for printed in [format!("{bad:?}"), bad.to_string()] {
            assert!(!printed.contains("password"), "{printed}");
        }
    }
}
