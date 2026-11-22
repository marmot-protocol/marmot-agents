//! Deliberate faults, to show each check catches the bug it is for.
//!
//! [`Faulty`] wraps a subject so that its driver breaks one property on purpose, the way review
//! found real drivers break them. A test runs the scenario that should catch it, and expects that
//! property, and only that cause, to fail. A check that has never failed proves nothing.

use crate::check::{Violation, marked_chat};
use crate::fake::Fake;
use crate::run::Subject;
use marmot_agent_driver::{
    Action, Answer, Capabilities, ChatId, Control, Driver, Emitted, Given, Harness, Output, Prompt,
    Reason, Refusal, RequestId,
};
use marmot_agent_events::{AgentEvent, Body, State};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Writes bytes onto the harness's input as if the driver had written them.
pub type Injector = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// One bug, put in on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// S1: the first request an event settles is left out, so it is never settled.
    DropSettle,
    /// S1: each request an event settles is settled again by the next event.
    SettleTwice,
    /// S1: an interrupt the harness never acted on is settled by the turn the driver closes when
    /// the harness stops, as if it had worked. (Codex's driver once did this.)
    SettleOnStop,
    /// S2: what the driver closes when the harness stops is never handed over.
    LeaveOpen,
    /// S4: every prompt is handed to the harness twice.
    PromptTwice,
    /// S5: chat 2's events are handed over as chat 1's.
    WrongChat,
    /// S5: an event that carries its own chat's words carries the last words another chat's
    /// events carried after them, as a driver that mixes up its buffers would.
    MixWords,
    /// S5: chat 2's turns take chat 1's first turn's item id.
    ShareItem,
    /// S5: a question's answer is handed to the harness with another option than the one chosen.
    SwapChoice,
    /// S12: a prompt that names no session is sent naming the chat's first session.
    ResumeUnasked,
    /// S13: every later turn of a chat takes its first turn's item id.
    ReuseItem,
    /// SEQ: every third event's `seq` goes back to 1.
    SeqBack,
    /// S11: a wait for an event that is dropped leaves half a message on the harness's input.
    HalfFrame,
    /// S11: a wait for an event that is dropped loses the event it had in hand.
    LoseOnDrop,
    /// S11: an answer the driver could not hand over is taken anyway, and nothing settles it.
    TakeUnhanded,
    /// S11: every prompt after a chat's first is taken and never handed over. Caught only with a
    /// harness for each chat, which a later prompt starts again after a death.
    SwallowPrompt,
    /// L2 (the scenario checks nothing): an interrupt is refused without reaching the harness,
    /// and the turn is shown stopped all the same, with an event the driver makes up.
    MadeUpStop,
}

impl Fault {
    /// The property the fault breaks, as a violation names it.
    pub fn breaks(self) -> &'static str {
        match self {
            Fault::DropSettle | Fault::SettleTwice | Fault::SettleOnStop => "S1",
            Fault::LeaveOpen => "S2",
            Fault::PromptTwice => "S4",
            Fault::WrongChat | Fault::MixWords | Fault::ShareItem | Fault::SwapChoice => "S5",
            Fault::ResumeUnasked => "S12",
            Fault::ReuseItem => "S13",
            Fault::SeqBack => "SEQ",
            Fault::HalfFrame | Fault::LoseOnDrop | Fault::TakeUnhanded | Fault::SwallowPrompt => {
                "S11"
            }
            Fault::MadeUpStop => "L2",
        }
    }

    /// What the violation must say, where more than one check could name the property: so that
    /// the check meant for it, and not another, is what catches it.
    pub fn detail(self) -> Option<&'static str> {
        match self {
            Fault::ResumeUnasked => Some("taken up, unasked"),
            Fault::HalfFrame => Some("not whole messages"),
            Fault::MixWords => Some("words from chat"),
            Fault::ShareItem => Some("an item of"),
            Fault::SwapChoice => Some("not as given"),
            Fault::SwallowPrompt => Some("a prompt returned before the harness had it"),
            Fault::MadeUpStop => Some("did not die on the interrupt"),
            _ => None,
        }
    }

    /// Whether `found` is this fault caught as it should be: the property it breaks, by the check
    /// meant for it.
    pub fn caught_as(self, found: &Result<(), Violation>) -> bool {
        match found {
            Err(v) => {
                v.property == self.breaks() && self.detail().is_none_or(|d| v.detail.contains(d))
            }
            Ok(()) => false,
        }
    }

    /// The scenario that catches it.
    pub fn caught_by(self) -> &'static str {
        match self {
            Fault::DropSettle | Fault::SettleTwice => "two_chats",
            Fault::SettleOnStop => "dies_with_requests_pending",
            Fault::LeaveOpen => "dies_mid_turn",
            Fault::PromptTwice => "two_chats",
            Fault::WrongChat | Fault::MixWords | Fault::ShareItem => "two_chats",
            Fault::SwapChoice => "answered_once",
            Fault::SwallowPrompt => "dies_mid_turn",
            Fault::MadeUpStop => "dies_with_requests_pending",
            Fault::ResumeUnasked => "new_session",
            Fault::ReuseItem => "new_session",
            Fault::SeqBack => "one_turn",
            Fault::HalfFrame | Fault::LoseOnDrop => "dropped_waits",
            Fault::TakeUnhanded => "answer_after_death",
        }
    }

    /// Every fault.
    pub const ALL: [Fault; 17] = [
        Fault::DropSettle,
        Fault::SettleTwice,
        Fault::SettleOnStop,
        Fault::LeaveOpen,
        Fault::PromptTwice,
        Fault::WrongChat,
        Fault::MixWords,
        Fault::ShareItem,
        Fault::SwapChoice,
        Fault::ResumeUnasked,
        Fault::ReuseItem,
        Fault::SeqBack,
        Fault::HalfFrame,
        Fault::LoseOnDrop,
        Fault::TakeUnhanded,
        Fault::SwallowPrompt,
        Fault::MadeUpStop,
    ];

    /// Whether only a driver with a harness for each chat can show the fault.
    pub fn needs_a_harness_per_chat(self) -> bool {
        self == Fault::SwallowPrompt
    }
}

/// A subject whose driver has a fault.
pub struct Faulty<S> {
    inner: S,
    fault: Fault,
    injector: Option<Injector>,
}

impl<S: Subject> Faulty<S> {
    /// `subject`, with `fault` put in. A fault that writes to the harness needs an `injector`.
    pub fn new(subject: S, fault: Fault, injector: Option<Injector>) -> Self {
        Self {
            inner: subject,
            fault,
            injector,
        }
    }
}

/// Runs the scenario that should catch `fault` against `subject` with the fault in, and gives
/// back what it found.
pub async fn caught<S: Subject>(
    subject: S,
    fault: Fault,
    injector: Option<Injector>,
) -> Result<(), Violation> {
    let faulty = Faulty::new(subject, fault, injector);
    crate::scenarios::run(fault.caught_by(), faulty).await
}

impl<S: Subject> Subject for Faulty<S> {
    type Driver = FaultyDriver<S::Driver>;

    async fn start(&mut self) -> Self::Driver {
        let inner = self.inner.start().await;
        FaultyDriver {
            inner,
            fault: self.fault,
            injector: self.injector.clone(),
            injected: Arc::new(AtomicBool::new(false)),
            settled: HashSet::new(),
            carried: Vec::new(),
            interrupts: Vec::new(),
            first_turn: HashMap::new(),
            first_session: HashMap::new(),
            count: 0,
            words: HashMap::new(),
            options: HashMap::new(),
            turns: HashMap::new(),
            seqs: HashMap::new(),
            queued: VecDeque::new(),
            prompted: HashSet::new(),
        }
    }

    fn fake(&self) -> &Fake {
        self.inner.fake()
    }

    fn kill(&mut self, chat: &ChatId) {
        self.inner.kill(chat);
    }

    fn shared_harness(&self) -> bool {
        self.inner.shared_harness()
    }

    fn setting(&self) -> (String, String) {
        self.inner.setting()
    }

    fn never(&self) -> &'static str {
        self.inner.never()
    }

    fn chat_of(&self, tag: &str) -> Option<ChatId> {
        self.inner.chat_of(tag)
    }

    fn loose(&self) -> bool {
        self.inner.loose()
    }

    fn reorders(&self) -> bool {
        self.inner.reorders()
    }
}

/// A driver with a fault.
pub struct FaultyDriver<D> {
    inner: D,
    fault: Fault,
    injector: Option<Injector>,
    injected: Arc<AtomicBool>,
    settled: HashSet<RequestId>,
    carried: Vec<RequestId>,
    interrupts: Vec<RequestId>,
    first_turn: HashMap<ChatId, String>,
    first_session: HashMap<ChatId, String>,
    count: u64,
    /// The last words each chat's events carried with its own mark.
    words: HashMap<ChatId, String>,
    /// Each question item's options, by question.
    options: HashMap<String, Vec<(String, Vec<String>)>>,
    /// The last event of each turn.
    turns: HashMap<String, Emitted>,
    /// The last `seq` of each session.
    seqs: HashMap<String, u64>,
    /// Events the driver made up, handed over before the next from the driver.
    queued: VecDeque<Output>,
    /// Chats that have prompted.
    prompted: HashSet<ChatId>,
}

/// Fires the injector if dropped before it is disarmed.
struct Guard {
    injector: Option<Injector>,
    injected: Arc<AtomicBool>,
    armed: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.armed
            && !self.injected.swap(true, Ordering::SeqCst)
            && let Some(inject) = &self.injector
        {
            inject(br#"{"id": 999, "method": "half"#);
        }
    }
}

fn rebuilt(
    event: &AgentEvent,
    change: impl FnOnce(&mut marmot_agent_events::Header),
) -> AgentEvent {
    let mut header = event.header().clone();
    change(&mut header);
    AgentEvent::new(header, event.body().clone(), event.native().cloned()).expect("an event")
}

impl<D: Driver> FaultyDriver<D> {
    /// What the fault does to an output; `None` drops it.
    fn alter(&mut self, output: Output) -> Option<Output> {
        let Output::Event(mut e) = output else {
            return Some(output);
        };
        self.count += 1;
        let header = e.event.header().clone();
        let chat = e.chat.clone();
        if let (Some(chat), Some(session)) = (&chat, &header.session)
            && matches!(e.event.body(), Body::Session(_))
        {
            (self.first_session)
                .entry(chat.clone())
                .or_insert_with(|| session.as_str().to_string());
        }
        if let Some(session) = &header.session {
            let last = self.seqs.entry(session.as_str().to_string()).or_default();
            *last = (*last).max(header.seq);
        }
        if let (Body::Turn(_), Some(turn)) = (e.event.body(), &header.turn) {
            self.turns.insert(turn.as_str().to_string(), e.clone());
        }
        if let Body::Question(q) = e.event.body() {
            let options = (q.questions().iter())
                .map(|a| {
                    (
                        a.id.clone(),
                        a.options.iter().map(|o| o.id.clone()).collect(),
                    )
                })
                .collect();
            self.options
                .insert(header.item.as_str().to_string(), options);
        }
        match self.fault {
            Fault::DropSettle => {
                if !e.settles.is_empty() && self.settled.is_empty() {
                    let dropped = e.settles.remove(0);
                    self.settled.insert(dropped);
                }
            }
            Fault::SettleTwice => {
                let carried = std::mem::take(&mut self.carried);
                self.carried = e.settles.clone();
                e.settles.extend(carried);
            }
            Fault::SettleOnStop => {
                self.settled.extend(e.settles.iter().cloned());
                let closed = matches!(e.event.body(), Body::Turn(_))
                    && header.state == State::Cancelled
                    && e.settles.is_empty();
                if closed {
                    let open: Vec<RequestId> = (self.interrupts.iter())
                        .filter(|r| !self.settled.contains(*r))
                        .cloned()
                        .collect();
                    e.settles.extend(open);
                }
            }
            Fault::LeaveOpen => {
                if matches!(header.state, State::Cancelled) {
                    return None;
                }
            }
            Fault::WrongChat => {
                if chat.as_ref().is_some_and(|c| c.0 == "c-2") {
                    e.chat = Some("c-1".into());
                }
            }
            Fault::MixWords => {
                let own = chat
                    .as_ref()
                    .filter(|c| marked_chat(&header.text) == Some(c.0.clone()));
                if let Some(own) = own {
                    let other = (self.words.iter()).find(|(c, _)| *c != own);
                    if let Some((_, words)) = other {
                        let mixed = format!("{} {words}", header.text);
                        e.event = rebuilt(&e.event, |h| h.text = mixed);
                    }
                    self.words.insert(own.clone(), header.text.clone());
                }
            }
            Fault::ShareItem => {
                if let (Body::Turn(_), Some(chat)) = (e.event.body(), &chat) {
                    (self.first_turn)
                        .entry(chat.clone())
                        .or_insert_with(|| header.item.as_str().to_string());
                    let theirs = self.first_turn.get(&ChatId::from("c-1")).cloned();
                    if let (true, Some(theirs)) = (chat.0 == "c-2", theirs) {
                        e.event = rebuilt(&e.event, |h| h.item = theirs.as_str().into());
                    }
                }
            }
            Fault::ReuseItem => {
                if let (Body::Turn(_), Some(chat)) = (e.event.body(), &chat) {
                    let first = (self.first_turn)
                        .entry(chat.clone())
                        .or_insert_with(|| header.item.as_str().to_string())
                        .clone();
                    if first != header.item.as_str() {
                        e.event = rebuilt(&e.event, |h| h.item = first.as_str().into());
                    }
                }
            }
            Fault::SeqBack if self.count.is_multiple_of(3) => {
                e.event = rebuilt(&e.event, |h| h.seq = 1);
            }
            _ => {}
        }
        Some(Output::Event(e))
    }
}

impl<D: Driver> Driver for FaultyDriver<D> {
    fn harness(&self) -> Harness {
        self.inner.harness()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let mut prompt = prompt;
        let again = !self.prompted.insert(prompt.chat.id.clone());
        if self.fault == Fault::SwallowPrompt && again {
            return Ok(());
        }
        if self.fault == Fault::ResumeUnasked && prompt.session.is_none() {
            let first = self.first_session.get(&prompt.chat.id);
            prompt.session = first.map(|s| s.as_str().into());
        }
        if self.fault == Fault::PromptTwice {
            let _ = self.inner.prompt(prompt.clone()).await;
        }
        self.inner.prompt(prompt).await
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        let mut answer = answer;
        if let (Fault::SwapChoice, Given::Questions(replies)) = (self.fault, &mut answer.given) {
            let asked = self.options.get(answer.item.as_str());
            for reply in replies.iter_mut() {
                let offered = (asked.into_iter().flatten()).find(|(q, _)| *q == reply.question);
                let other =
                    offered.and_then(|(_, ids)| ids.iter().find(|o| !reply.options.contains(o)));
                if let Some(other) = other {
                    reply.options = vec![other.clone()];
                }
            }
        }
        let taken = self.inner.answer(answer).await;
        if self.fault == Fault::TakeUnhanded {
            return Ok(());
        }
        taken
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        if let (Fault::MadeUpStop, Action::Interrupt { turn }) = (self.fault, &control.action) {
            let stopped = self.turns.get(turn.as_str()).and_then(|last| {
                let session = last.event.header().session.as_ref()?.as_str().to_string();
                let seq = self.seqs.get(&session).copied().unwrap_or_default() + 1;
                let mut made_up = last.clone();
                let mut header = last.event.header().clone();
                header.state = State::Cancelled;
                header.seq = seq;
                made_up.event = AgentEvent::new(header, last.event.body().clone(), None).ok()?;
                made_up.settles.clear();
                Some((session, seq, made_up))
            });
            if let Some((session, seq, made_up)) = stopped {
                self.seqs.insert(session, seq);
                self.queued.push_back(Output::Event(made_up));
            }
            return Err(Refusal::new(Reason::Failed));
        }
        let interrupt = matches!(control.action, Action::Interrupt { .. });
        let request = control.request.clone();
        let result = self.inner.control(control).await;
        if interrupt && result.is_ok() {
            self.interrupts.push(request);
        }
        result
    }

    async fn next_event(&mut self) -> Option<Output> {
        if let Some(made_up) = self.queued.pop_front() {
            return Some(made_up);
        }
        loop {
            let mut guard = Guard {
                injector: self.injector.clone(),
                injected: self.injected.clone(),
                armed: self.fault == Fault::HalfFrame,
            };
            let output = self.inner.next_event().await;
            guard.armed = false;
            drop(guard);
            if self.fault == Fault::LoseOnDrop {
                tokio::task::yield_now().await;
            }
            let output = output?;
            if let Some(output) = self.alter(output) {
                return Some(output);
            }
        }
    }

    async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
        self.inner.file_sent(id, result).await;
    }
}
