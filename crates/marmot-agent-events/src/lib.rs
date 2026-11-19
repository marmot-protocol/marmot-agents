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

/// What an item is. A client uses it to choose how to draw the item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemType {
    /// A turn started or ended.
    Turn,
    /// Text the agent says to the people in the chat.
    Message,
    /// The agent's thinking, when the harness provides it.
    Reasoning,
    /// A tool the agent used: a command, an edit, a search and so on.
    Tool,
    /// The agent wants permission for one action.
    Approval,
    /// The agent asks the person something.
    Question,
    /// The agent's plan or to-do list.
    Plan,
    /// Work that runs beside a turn or outlives it: a subagent, a background job.
    Task,
    /// Compaction, the agent waiting, a retry, a warning, a model change, the output of a command.
    Notice,
    /// Something failed outside any one item.
    Error,
    /// A session with the harness, and the settings, actions and slash commands it offers.
    Session,
}

impl ItemType {
    /// The name of this type in an agent event.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Message => "message",
            Self::Reasoning => "reasoning",
            Self::Tool => "tool",
            Self::Approval => "approval",
            Self::Question => "question",
            Self::Plan => "plan",
            Self::Task => "task",
            Self::Notice => "notice",
            Self::Error => "error",
            Self::Session => "session",
        }
    }
}

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

impl State {
    /// The name of this state in an agent event.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
        }
    }
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

    fn one_of_each() -> Vec<Body> {
        let turn = body::Turn {
            started_by: body::StartedBy::User,
            duration_ms: None,
            usage: None,
        };
        let tool = body::Tool {
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
        };
        let approval = body::Approval {
            subject: None,
            choices: Vec::new(),
            answer: None,
        };
        let question = body::Question {
            answer_via: body::AnswerVia::Request,
            subject: None,
            questions: Vec::new(),
            answer: None,
        };
        let task = body::Task {
            kind: "subagent".into(),
            description: None,
            subject: None,
            usage: None,
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
            Body::Tool(tool),
            Body::Approval(approval),
            Body::Question(question),
            Body::Plan(body::Plan::default()),
            Body::Task(task),
            Body::Notice(notice),
            Body::Error(body::Error::default()),
            Body::Session(body::Session::default()),
        ]
    }

    // The names are part of the protocol: every client matches on them. A rename must fail here.
    #[test]
    fn states_have_the_names_of_the_design_page() {
        let names = [
            (State::Running, "running"),
            (State::Waiting, "waiting"),
            (State::Done, "done"),
            (State::Failed, "failed"),
            (State::Declined, "declined"),
            (State::Cancelled, "cancelled"),
        ];
        for (state, name) in names {
            assert_eq!(state.as_str(), name);
            assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
        }
    }

    // The eleven types, their names on the wire, and that `type` always matches the fields
    // beside it: a body cannot be sent under another type's name.
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
            assert_eq!(event.item_type().as_str(), name);
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
        let body = one_of_each().remove(3);
        let native = Native {
            kind: "item/started [commandExecution]".into(),
            payload: serde_json::value::RawValue::from_string(r#"{"id":"i1"}"#.into()).unwrap(),
        };
        let written = serde_json::to_value(event(State::Running, body, Some(native))).unwrap();
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
        let written = serde_json::to_string(&event).unwrap();
        assert!(
            written.contains(&format!(r#""payload":{payload}"#)),
            "{written}"
        );
    }

    // A driver and this vocabulary must not drift apart unnoticed: a field that no item type
    // has is refused, not dropped.
    #[test]
    fn a_field_the_vocabulary_does_not_have_is_refused() {
        let event = json!({
            "harness": "codex", "item": "i1", "seq": 1, "type": "message", "state": "done",
            "title": "Message", "text": "hi", "body": {"phase": "final", "mood": "cheerful"},
        });
        let error = serde_json::from_value::<AgentEvent>(event)
            .unwrap_err()
            .to_string();
        assert!(error.contains("mood"), "{error}");
    }

    // An event with no session, no turn and no native message leaves those fields out.
    #[test]
    fn absent_parts_are_left_out() {
        let mut event = event(
            State::Running,
            Body::Session(body::Session::default()),
            None,
        );
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
        assert_eq!(
            written["body"],
            json!({"settings": [], "actions": [], "commands": []})
        );
    }
}
