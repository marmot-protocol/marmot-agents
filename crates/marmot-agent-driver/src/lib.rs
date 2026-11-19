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
/// A driver that only repeats each prompt back shows the shape. It is idle between turns, and
/// an idle driver waits: it does not end.
///
/// ```
/// use marmot_agent_driver::*;
/// use marmot_agent_events::body::{Body, Message, StartedBy, Turn};
/// use marmot_agent_events::{AgentEvent, Header, State};
/// use std::collections::VecDeque;
///
/// struct Echo {
///     ready: VecDeque<Emitted>,
///     seq: u64,
/// }
///
/// impl Echo {
///     fn emit(&mut self, item: &str, state: State, text: &str, body: Body, prompts: Vec<MessageId>) {
///         self.seq += 1;
///         let header = Header {
///             harness: "echo".into(),
///             session: None,
///             turn: Some("turn-1".into()),
///             item: item.into(),
///             seq: self.seq,
///             state,
///             title: String::new(),
///             text: text.into(),
///         };
///         // Only a question can be refused here, and this driver asks none.
///         let event = AgentEvent::new(header, body, None).expect("not a question");
///         self.ready.push_back(Emitted { event, prompts, settles: Vec::new() });
///     }
/// }
///
/// impl Driver for Echo {
///     fn harness(&self) -> Harness {
///         Harness { name: "echo".into(), version: None }
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
///     async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
///         let turn = || Body::Turn(Turn { started_by: StartedBy::User, duration_ms: None, usage: None });
///         // The turn names the prompt it responds to. The message inside it does not.
///         self.emit("turn-1", State::Running, "", turn(), vec![prompt.message]);
///         self.emit("message-1", State::Done, &prompt.text, Body::Message(Message::default()), Vec::new());
///         self.emit("turn-1", State::Done, "", turn(), Vec::new());
///         Ok(())
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
///         match self.ready.pop_front() {
///             Some(emitted) => Some(emitted),
///             // Nothing to report yet. `None` would say the harness has gone.
///             None => std::future::pending().await,
///         }
///     }
/// }
///
/// // The core holds a driver by its type, and its futures can move between threads.
/// fn held_by_the_core<D: Driver + 'static>(driver: D) -> impl Send {
///     driver
/// }
/// let _ = held_by_the_core(Echo { ready: VecDeque::new(), seq: 0 });
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
    use super::Reply;

    // A reply holds what a person typed, and for a secret question that is the secret. It must
    // not reach a log through a debug print of the request it travels in.
    #[test]
    fn a_reply_printed_for_debugging_does_not_show_what_was_typed() {
        let reply = Reply {
            question: "password".into(),
            options: vec!["o1".into()],
            text: Some("hunter2".into()),
        };
        let printed = format!("{reply:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(
            printed.contains("password") && printed.contains("o1"),
            "{printed}"
        );
    }
}
