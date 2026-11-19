//! The trait every Rust driver implements.
//!
//! A driver connects one harness to the core. It speaks the harness's own interface, and turns
//! what the harness sends into agent events. It never decides who may prompt or approve, and it
//! never touches keys, groups or relays (D-004, D-010).
//!
//! The trait has the operations of the socket protocol's driver section
//! (`docs/design/socket-protocol.md`), because a connector on the socket does the same job:
//!
//! | On the socket | Here |
//! | --- | --- |
//! | the hello: the harness and its capabilities | [`Driver::harness`], [`Driver::capabilities`] |
//! | the request `prompt` | [`Driver::prompt`] |
//! | the request `answer` | [`Driver::answer`] |
//! | the request `control` | [`Driver::control`] |
//! | the notification `event` | [`Driver::next_event`] |
//!
//! The trait and the capabilities are a proposal (`docs/open-questions.md`, Q-16).
//!
//! This crate needs no async runtime. A driver chooses its own.

mod capabilities;
mod request;

pub use capabilities::{
    ApprovalCommand, Capabilities, FilesIn, MidTurn, Position, Questions, Sessions,
};
use marmot_agent_events::AgentEvent;
pub use request::{
    Action, Answer, Attachment, Chat, ChatId, Control, Given, MessageId, Person, PersonId, Prompt,
    Quoted, Reason, Refusal,
};
use std::future::Future;

/// A harness's name and version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Harness {
    /// The name, such as `codex`. Nothing above a driver may act on it.
    pub name: String,
    /// The version, when the harness has said.
    pub version: Option<String>,
}

/// An agent event, as a driver hands it to the core.
#[derive(Clone, Debug, PartialEq)]
pub struct Emitted {
    /// The event.
    pub event: AgentEvent,
    /// On a turn: the prompts it responds to.
    pub prompts: Vec<MessageId>,
}

/// One harness, driven for one agent.
///
/// A request returns as soon as the harness has taken it or refused it. What the harness then
/// does arrives as events.
///
/// A driver is used from one task: every method takes `&mut self`. A caller that waits for an
/// event and for a request at the same time drops the future from [`next_event`] to make the
/// request, so that future must be safe to drop: an event is either returned or still to come,
/// never lost. A request that waits for the harness keeps reading from it meanwhile, and holds
/// back the events it reads for the next call of [`next_event`].
///
/// [`next_event`]: Driver::next_event
pub trait Driver: Send {
    /// What the harness is.
    fn harness(&self) -> Harness;

    /// What the harness can do.
    fn capabilities(&self) -> Capabilities;

    /// Pass a message to the harness. During a turn the harness does with it what it does
    /// natively, which [`Capabilities::mid_turn`] names (D-040).
    fn prompt(&mut self, prompt: Prompt) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Pass an answer to the request that is waiting for it.
    fn answer(&mut self, answer: Answer) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Carry out a control.
    fn control(&mut self, control: Control) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// The next event, in the order the driver produced it. `None` means the harness has gone
    /// and no more will come. The driver has by then closed every open item and the open turn.
    fn next_event(&mut self) -> impl Future<Output = Option<Emitted>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use marmot_agent_events::body::{Body, Message};
    use marmot_agent_events::{AgentEvent, Header, State};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    fn alice() -> Person {
        Person {
            id: "u-1".into(),
            name: "Alice".into(),
        }
    }

    /// A driver with no harness behind it: it answers every prompt with one message.
    struct Echo {
        waiting: VecDeque<Emitted>,
        seq: u64,
    }

    impl Driver for Echo {
        fn harness(&self) -> Harness {
            Harness {
                name: "echo".into(),
                version: None,
            }
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                interrupt: false,
                mid_turn: MidTurn::Queue,
                sessions: Sessions::Core,
                resume: false,
                tool_items: false,
                approvals: false,
                questions: Questions::default(),
                reasoning: false,
                files_in: FilesIn::None,
                approval_commands: Vec::new(),
            }
        }

        async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
            self.seq += 1;
            let header = Header {
                harness: "echo".into(),
                session: prompt.session,
                turn: None,
                item: format!("m{}", self.seq).as_str().into(),
                seq: self.seq,
                state: State::Done,
                title: "Message".into(),
                text: prompt.text,
            };
            let event = AgentEvent::new(header, Body::Message(Message::default()), None)
                .expect("a message is not the answer to a secret question");
            self.waiting.push_back(Emitted {
                event,
                prompts: vec![prompt.message],
            });
            Ok(())
        }

        async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
            Err(Refusal::new(Reason::UnknownItem))
        }

        async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
            Err(Refusal::new(Reason::NotSupported))
        }

        async fn next_event(&mut self) -> Option<Emitted> {
            self.waiting.pop_front()
        }
    }

    /// Run a future that is ready at once. No runtime is needed for the driver above.
    fn now<T>(future: impl Future<Output = T>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        match pin!(future).poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the test driver should never wait"),
        }
    }

    fn requires_send<T: Send>(value: T) -> T {
        value
    }

    // The trait can be implemented and driven through a generic caller, as the core will hold
    // it, and its futures can move between threads.
    #[test]
    fn a_driver_takes_a_prompt_and_hands_back_events() {
        fn drive<D: Driver>(
            driver: &mut D,
        ) -> (Result<(), Refusal>, Option<Emitted>, Option<Emitted>) {
            let prompt = Prompt {
                message: "m-31".into(),
                text: "hello".into(),
                chat: Chat {
                    id: "c-2".into(),
                    group: true,
                    name: Some("Family".into()),
                },
                sender: alice(),
                reply_to: None,
                attachments: Vec::new(),
                session: Some("s-1".into()),
            };
            let taken = now(requires_send(driver.prompt(prompt)));
            (
                taken,
                now(requires_send(driver.next_event())),
                now(driver.next_event()),
            )
        }
        let mut driver = Echo {
            waiting: VecDeque::new(),
            seq: 0,
        };
        let (taken, first, second) = drive(&mut driver);
        assert_eq!(taken, Ok(()));
        let first = first.expect("one event");
        assert_eq!(first.event.header().text, "hello");
        assert_eq!(first.prompts, vec![MessageId::from("m-31")]);
        assert_eq!(second, None);
        let interrupt = Control {
            by: alice(),
            action: Action::Interrupt { turn: "t-1".into() },
        };
        assert_eq!(
            now(driver.control(interrupt)),
            Err(Refusal::new(Reason::NotSupported))
        );
    }

    // The names a connector states in its hello. They are in docs/design/socket-protocol.md,
    // and an app reads them: a rename must fail here.
    #[test]
    fn capabilities_are_written_as_the_socket_page_names_them() {
        let capabilities = Capabilities {
            interrupt: true,
            mid_turn: MidTurn::Fold,
            sessions: Sessions::Core,
            resume: true,
            tool_items: true,
            approvals: true,
            questions: Questions {
                request: true,
                message: true,
            },
            reasoning: false,
            files_in: FilesIn::Images,
            approval_commands: vec![ApprovalCommand {
                command: "/approve".into(),
                counts: Position::Anywhere,
            }],
        };
        assert_eq!(
            serde_json::to_value(&capabilities).unwrap(),
            json!({
                "interrupt": true,
                "mid_turn": "fold",
                "sessions": "core",
                "resume": true,
                "tool_items": true,
                "approvals": true,
                "questions": {"request": true, "message": true},
                "reasoning": false,
                "files_in": "images",
                "approval_commands": [{"command": "/approve", "where": "anywhere"}],
            })
        );
        for (value, name) in [
            (MidTurn::Fold, "fold"),
            (MidTurn::Restart, "restart"),
            (MidTurn::Queue, "queue"),
        ] {
            assert_eq!(serde_json::to_value(value).unwrap(), json!(name));
        }
        assert_eq!(serde_json::to_value(Sessions::Own).unwrap(), json!("own"));
    }

    // The eight reasons docs/design/socket-protocol.md lists. The core turns each into a notice.
    #[test]
    fn refusals_have_the_reasons_the_socket_page_lists() {
        let reasons = [
            (Reason::NotSupported, "not_supported"),
            (Reason::Busy, "busy"),
            (Reason::UnknownItem, "unknown_item"),
            (Reason::NotWaiting, "not_waiting"),
            (Reason::AlreadyAnswered, "already_answered"),
            (Reason::NothingToDo, "nothing_to_do"),
            (Reason::Invalid, "invalid"),
            (Reason::Failed, "failed"),
        ];
        for (reason, name) in reasons {
            assert_eq!(serde_json::to_value(reason).unwrap(), json!(name));
        }
    }

    // The three shared controls keep the names D-029 gives them, and an answer names choices by
    // id, as the example on the socket page does.
    #[test]
    fn requests_are_written_as_the_design_pages_describe() {
        let set = Action::Set {
            session: "s-1".into(),
            setting: "model".into(),
            value: "gpt-6.1-sol".into(),
        };
        assert_eq!(
            serde_json::to_value(&set).unwrap(),
            json!({"action": "set", "session": "s-1", "setting": "model", "value": "gpt-6.1-sol"})
        );
        let interrupt = Action::Interrupt { turn: "t-1".into() };
        assert_eq!(
            serde_json::to_value(&interrupt).unwrap(),
            json!({"action": "interrupt", "turn": "t-1"})
        );
        let new_session = Action::NewSession {
            session: "s-1".into(),
        };
        assert_eq!(
            serde_json::to_value(&new_session).unwrap(),
            json!({"action": "new_session", "session": "s-1"})
        );
        let answer = Answer {
            item: "ap-2".into(),
            by: alice(),
            given: Given::Choice("once".into()),
        };
        assert_eq!(
            serde_json::to_value(&answer).unwrap(),
            json!({"item": "ap-2", "by": {"id": "u-1", "name": "Alice"}, "given": {"choice": "once"}})
        );
    }
}
