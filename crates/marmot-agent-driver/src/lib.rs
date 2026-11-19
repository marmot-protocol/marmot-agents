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

pub use capabilities::{
    ApprovalCommand, Capabilities, FilesIn, MidTurn, Position, Questions, Sessions,
};
use marmot_agent_events::AgentEvent;
pub use request::{
    Action, Answer, Attachment, Chat, ChatId, Control, Given, MessageId, Person, PersonId, Prompt,
    Quoted, Reason, Refusal, Reply, RequestId,
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

/// An agent event, as a driver hands it to the core, with what it follows from.
#[derive(Clone, Debug, PartialEq)]
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
    pub settles: Vec<RequestId>,
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
///             tool_items: false,
///             approvals: false,
///             questions: Questions::default(),
///             reasoning: false,
///             files_in: FilesIn::None,
///             approval_commands: Vec::new(),
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
    use super::{Answer, Given, Person, Reply};

    // A reply holds what a person answered, and for a secret question that is the secret,
    // whether they typed it or chose it. It must not reach a log through a debug print of the
    // request it travels in.
    #[test]
    fn an_answer_printed_for_debugging_does_not_show_what_was_answered() {
        let reply = Reply {
            question: "password".into(),
            options: vec!["option-7".into()],
            text: Some("hunter2".into()),
        };
        let answer = Answer {
            request: "r-1".into(),
            item: "question-1".into(),
            by: Person {
                id: "u-1".into(),
                name: "Alice".into(),
            },
            given: Given::Questions(vec![reply]),
        };
        let printed = format!("{answer:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(!printed.contains("option-7"), "{printed}");
        assert!(
            printed.contains("password") && printed.contains("question-1"),
            "{printed}"
        );
    }
}
