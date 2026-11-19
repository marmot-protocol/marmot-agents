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
//! The trait and the capabilities are a proposal (`docs/open-questions.md`, Q-16). Nothing here
//! says how these values are written on the socket; that is for the socket to define.
//!
//! This crate needs no async runtime. A driver chooses its own.

mod capabilities;
mod request;

pub use capabilities::{ApprovalCommand, Capabilities, FilesIn, MidTurn, Position, Sessions};
use marmot_agent_events::AgentEvent;
pub use request::{
    Action, Answer, Attachment, Chat, ChatId, Control, Given, MessageId, Person, PersonId, Prompt,
    Quoted, Reason, Refusal, Reply, RequestId,
};
use std::fmt;
use std::future::Future;

/// A harness's name and version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Harness {
    /// The name, such as `codex`. Nothing above a driver may act on it.
    pub name: String,
    /// The version, when the harness has said.
    pub version: Option<String>,
}

/// An agent event, as a driver hands it to the core, with what it follows from.
#[derive(Clone, PartialEq)]
pub struct Emitted {
    /// The event.
    pub event: AgentEvent,
    /// The chat the event belongs to. A driver knows it from the prompt or the control that led
    /// to the event, and states it whenever it does. The core needs it for a session it has not
    /// seen before: a harness can open a session and report it before any turn starts, and the
    /// turn may never start. Without the chat the core would have to guess where the session
    /// belongs, and a wrong guess shows one chat's activity in another.
    pub chat: Option<ChatId>,
    /// On a turn: the prompts it responds to. Empty on every other item.
    pub prompts: Vec<MessageId>,
    /// The answers and controls whose effect this event shows: the approval that was answered,
    /// the turn that was interrupted, the session whose setting changed (D-028). An event that
    /// follows from no request has none. That is how the core tells a setting someone changed
    /// from one the harness changed by itself, which gets a notice (D-046).
    ///
    /// An event can settle a request by showing that it failed. And a request the driver took
    /// may never be settled by an event at all, if the harness goes first: Codex takes a request
    /// to compact before the turn that does the work has started. When a driver ends, the core
    /// settles every request that is still open with a notice that it failed.
    pub settles: Vec<RequestId>,
}

/// Shows where an event came from and what it settles, and nothing the agent said or did: no
/// text, no fields of the item, no native message. A log must never hold those.
impl fmt::Debug for Emitted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Emitted")
            .field("item", &self.event.header().item)
            .field("state", &self.event.header().state)
            .field("chat", &self.chat)
            .field("prompts", &self.prompts)
            .field("settles", &self.settles)
            .finish_non_exhaustive()
    }
}

/// One harness, driven for one agent.
///
/// A request returns as soon as the harness has taken it or refused it. What the harness then
/// does arrives as events, and the event that shows a request's effect names it in
/// [`Emitted::settles`].
///
/// A driver is used from one task: every method takes `&mut self`. A caller that waits for an
/// event and for a request at the same time drops the future from [`next_event`] to make the
/// request, so that future must be safe to drop: an event is either returned or still to come,
/// never lost. A request that waits for the harness keeps reading from it meanwhile, and holds
/// back the events it reads for the next call of [`next_event`].
///
/// The smallest driver has a harness that never does anything. It shows the two things every
/// driver shares: the requests are `async fn`, and a driver with nothing to report waits. It does
/// not end.
///
/// ```
/// use marmot_agent_driver::*;
///
/// struct Silent;
///
/// impl Driver for Silent {
///     fn harness(&self) -> Harness {
///         Harness { name: "silent".into(), version: None }
///     }
///
///     fn capabilities(&self) -> Capabilities {
///         Capabilities {
///             interrupt: false,
///             mid_turn: MidTurn::Queue,
///             sessions: Sessions::Core,
///             resume: false,
///             approvals: false,
///             files_in: FilesIn::None,
///             // A harness with no such command still says so. This one has one, to show how.
///             approval_commands: vec![ApprovalCommand { command: "/approve".into(), counts: Position::Start }],
///         }
///     }
///
///     async fn prompt(&mut self, _prompt: Prompt) -> Result<(), Refusal> {
///         Err(Refusal::new(Reason::NotSupported))
///     }
///
///     async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
///         Err(Refusal::new(Reason::UnknownItem))
///     }
///
///     async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
///         Err(Refusal::new(Reason::NotSupported))
///     }
///
///     async fn next_event(&mut self) -> Option<Emitted> {
///         // Nothing to report, now or later. `None` would say the harness has gone.
///         std::future::pending().await
///     }
/// }
///
/// // The core holds a driver by its type, and can move it between threads.
/// fn held_by_the_core<D: Driver + 'static>(driver: D) -> impl Send {
///     driver
/// }
/// let _ = held_by_the_core(Silent);
/// ```
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

    /// The next event, in the order the driver produced it. It waits while there is nothing to
    /// report, however long that is.
    ///
    /// `None` means the harness has gone and no more will come. A driver that sees its harness
    /// stop closes what it can first. Whatever is still open then, the core closes, exactly as it
    /// does when a connector's connection drops (D-034).
    fn next_event(&mut self) -> impl Future<Output = Option<Emitted>> + Send;
}

#[cfg(test)]
mod tests {
    use super::{
        Action, Answer, Chat, Control, Emitted, Given, Person, Prompt, Quoted, Reason, Refusal,
        Reply,
    };
    use marmot_agent_events::body::{Body, Message};
    use marmot_agent_events::{AgentEvent, Header, Native, State};

    fn alice() -> Person {
        Person {
            id: "u-1".into(),
            name: "Alice Example".into(),
        }
    }

    fn shows(printed: &str, ids: &[&str], private: &[&str]) {
        for id in ids {
            assert!(printed.contains(id), "{id} is missing from {printed}");
        }
        for value in private {
            assert!(!printed.contains(value), "{value} is shown in {printed}");
        }
    }

    // This is a private messenger: a log must never hold what a person wrote, their name, what
    // they answered or chose, or what the agent or the harness said. Anything a driver is handed
    // or hands back, printed for debugging, shows which things it is about and no value.
    #[test]
    fn nothing_printed_for_debugging_shows_a_value() {
        let prompt = Prompt {
            message: "m-31".into(),
            text: "the launch code is 0000".into(),
            chat: Chat {
                id: "c-2".into(),
                group: true,
                name: Some("Family Chat".into()),
            },
            sender: alice(),
            reply_to: Some(Quoted {
                message: "m-30".into(),
                text: "what is the code?".into(),
                author: alice(),
            }),
            attachments: Vec::new(),
            session: Some("s-1".into()),
        };
        shows(
            &format!("{prompt:?}"),
            &["m-31", "m-30", "c-2", "u-1", "s-1"],
            &[
                "launch code",
                "what is the code",
                "Alice Example",
                "Family Chat",
            ],
        );

        // For a secret question the answer is the secret, whether it was typed or chosen.
        let reply = Reply {
            question: "password".into(),
            options: vec!["option-7".into()],
            text: Some("hunter2".into()),
        };
        let answer = |given| Answer {
            request: "r-1".into(),
            item: "question-1".into(),
            by: alice(),
            given,
        };
        shows(
            &format!("{:?}", answer(Given::Questions(vec![reply]))),
            &["r-1", "question-1", "password", "u-1"],
            &["hunter2", "option-7", "Alice Example"],
        );
        shows(
            &format!("{:?}", answer(Given::Choice("choice-allow".into()))),
            &["question-1"],
            &["choice-allow"],
        );

        let set = Action::Set {
            session: "s-1".into(),
            setting: "model".into(),
            value: "a-private-model".into(),
        };
        let control = Control {
            request: "r-2".into(),
            by: alice(),
            action: set,
        };
        shows(
            &format!("{control:?}"),
            &["r-2", "s-1", "model"],
            &["a-private-model", "Alice Example"],
        );

        let refusal = Refusal {
            reason: Reason::Failed,
            detail: Some("could not read /home/alice/secret.txt".into()),
        };
        shows(&format!("{refusal:?}"), &["Failed"], &["secret.txt"]);

        let header = Header {
            harness: "codex".into(),
            session: Some("s-1".into()),
            turn: Some("t-1".into()),
            item: "item-9".into(),
            seq: 1,
            state: State::Done,
            title: "Message".into(),
            text: "the launch code is 0000".into(),
        };
        let native = Native {
            kind: "item/completed".into(),
            payload: serde_json::value::RawValue::from_string(
                r#"{"text":"native launch code"}"#.into(),
            )
            .unwrap(),
        };
        let event =
            AgentEvent::new(header, Body::Message(Message::default()), Some(native)).unwrap();
        let emitted = Emitted {
            event,
            chat: Some("c-2".into()),
            prompts: vec!["m-31".into()],
            settles: vec!["r-2".into()],
        };
        shows(
            &format!("{emitted:?}"),
            &["item-9", "c-2", "m-31", "r-2"],
            &["launch code"],
        );
    }
}
