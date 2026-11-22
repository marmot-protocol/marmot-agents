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
//! | the notification `event` | [`Driver::next_event`], as [`Output::Event`] |
//! | the request `send_file`, from the connector | [`Driver::next_event`], as [`Output::SendFile`] |
//! | the core's response to `send_file` | [`Driver::file_sent`] |
//!
//! The trait and the capabilities are decided (`docs/decisions.md`, D-058). Sending a file
//! (D-090) is built on it, and is a proposal until the project owner accepts it. Nothing here
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
use std::path::PathBuf;

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
    /// to compact before the turn that does the work has started. When a driver ends, for any
    /// reason, the core settles every answer and control still open with a notice that it failed.
    /// A prompt is not settled this way: one the driver had not taken is reported as not
    /// delivered, and is never sent again (D-037, D-058).
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

/// What a driver hands the core next: an agent event, or a file the agent asks to send.
// An event is by far the more common, so it is not boxed to make the rarer request smaller.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Output {
    /// An agent event.
    Event(Emitted),
    /// The agent asks the core to send a file to a chat (D-090). The core checks the file is in a
    /// folder the settings allow, sends it as an encrypted attachment tagged with the chat's
    /// running turn, and tells the driver how it went through [`Driver::file_sent`].
    SendFile(SendFile),
}

impl Output {
    /// The event, when this is one.
    pub fn into_event(self) -> Option<Emitted> {
        match self {
            Self::Event(emitted) => Some(emitted),
            Self::SendFile(_) => None,
        }
    }
}

impl From<Emitted> for Output {
    fn from(emitted: Emitted) -> Self {
        Self::Event(emitted)
    }
}

/// A file the agent asks the core to send to a chat (D-090).
#[derive(Clone, PartialEq, Eq)]
pub struct SendFile {
    /// The driver's own id for this request, handed back in [`Driver::file_sent`]. Unique among
    /// the driver's requests that are not settled yet.
    pub id: String,
    /// The chat to send it to.
    pub chat: ChatId,
    /// The file, as an absolute path. A driver resolves a path the agent gave relative to its
    /// working folder before handing it on. The core reads only what is in a folder its settings
    /// allow, after following every link, so a path that leads out is refused.
    pub path: PathBuf,
}

/// Shows the chat, and neither the path nor the driver's id, which a harness made (D-071).
impl fmt::Debug for SendFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendFile")
            .field("chat", &self.chat)
            .finish_non_exhaustive()
    }
}

/// One harness, driven for one agent.
///
/// A request returns as soon as the driver has taken it or refused it. Taking it means the driver
/// has handed it to the harness and answers for what follows. It need not wait for the harness to
/// confirm, because a harness may never say: Codex's answer to one of its approval requests is a
/// reply that gets no reply, and the notice that follows also comes when the turn is interrupted
/// instead. What the harness then does arrives as events, and the event that shows a request's
/// effect names it in [`Emitted::settles`], including when the effect is that it failed.
///
/// A driver is used from one task: every method takes `&mut self`. A caller that waits for an
/// event and for a request at the same time drops the future from [`next_event`] to make the
/// request, so that future must be safe to drop: an event is either returned or still to come,
/// never lost. A request that waits for the harness keeps reading from it meanwhile, and holds
/// back the events it reads for the next call of [`next_event`]. The future from a request is
/// never dropped with the driver kept: the caller waits for it to return, or drops both (D-066).
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
///     async fn next_event(&mut self) -> Option<Output> {
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

    /// The folder the driver starts a chat's session in when the chat chose none: the agent's
    /// install folder (D-101). A driver that states one starts each chat's sessions in the folder
    /// [`Chat::folder`] names, when it names one, and the core offers each chat a setting to move
    /// it. `None`, the default, for a harness that keeps a workspace of its own, such as a gateway:
    /// the core then offers no such setting and names no folder.
    fn folder(&self) -> Option<PathBuf> {
        None
    }

    /// Pass a message to the harness. During a turn the harness does with it what it does
    /// natively, which [`Capabilities::mid_turn`] names (D-040).
    fn prompt(&mut self, prompt: Prompt) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Pass an answer to the request that is waiting for it.
    fn answer(&mut self, answer: Answer) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Carry out a control.
    fn control(&mut self, control: Control) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// The next event, or the next file the agent asks to send, in the order the driver
    /// produced them. It waits while there is nothing to report, however long that is.
    ///
    /// `None` means the harness has gone and no more will come. A driver that sees its harness
    /// stop closes what it can first. When a driver ends, for any reason, the core closes the
    /// items still open, as it does when a connector's connection drops (D-034), and settles
    /// answers and controls as [`Emitted::settles`] says (D-058).
    fn next_event(&mut self) -> impl Future<Output = Option<Output>> + Send;

    /// How a file the driver asked to send went: sent, or refused and why (D-090). The core calls
    /// it once for every [`Output::SendFile`], after the send, and awaits it to the end. A
    /// refusal's detail names no path. A driver whose harness cannot ask to send a file never
    /// hands one over, and need not implement this.
    fn file_sent(
        &mut self,
        id: String,
        result: Result<(), Refusal>,
    ) -> impl Future<Output = ()> + Send {
        let _ = (id, result);
        async {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Action, Answer, Attachment, Chat, Control, Emitted, Given, Person, Prompt, Quoted, Reason,
        Refusal, Reply, SendFile,
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
    // they answered or chose, what the agent or the harness said, or an id the harness made
    // (D-071). Anything a driver is handed or hands back, printed for debugging, shows the core's
    // own ids and no value.
    #[test]
    fn nothing_printed_for_debugging_shows_a_value() {
        let prompt = Prompt {
            message: "m-31".into(),
            text: "the launch code is 0000".into(),
            chat: Chat {
                id: "c-2".into(),
                group: true,
                name: Some("Family Chat".into()),
                folder: Some("/home/alice/secret-project".into()),
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
            &["m-31", "m-30", "c-2", "u-1"],
            &[
                "s-1",
                "launch code",
                "what is the code",
                "Alice Example",
                "Family Chat",
                "secret-project",
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
        // The agent writes a question's id, so it can say what was asked.
        shows(
            &format!("{:?}", answer(Given::Questions(vec![reply]))),
            &["r-1", "u-1"],
            &[
                "question-1",
                "password",
                "hunter2",
                "option-7",
                "Alice Example",
            ],
        );
        shows(
            &format!("{:?}", answer(Given::Choice("choice-allow".into()))),
            &["r-1"],
            &["question-1", "choice-allow"],
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
            &["r-2", "model"],
            &["s-1", "a-private-model", "Alice Example"],
        );

        let attachment = Attachment {
            path: "/home/alice/plan.pdf".into(),
            name: "plan.pdf".into(),
            media_type: "application/pdf; name=plan.pdf".into(),
        };
        shows(&format!("{attachment:?}"), &[], &["plan", "application"]);

        let file = SendFile {
            id: "call_secret".into(),
            chat: "c-2".into(),
            path: "/home/alice/plan.pdf".into(),
        };
        shows(&format!("{file:?}"), &["c-2"], &["call_secret", "plan"]);

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
            &["c-2", "m-31", "r-2"],
            &["item-9", "s-1", "t-1", "launch code"],
        );
    }
}
