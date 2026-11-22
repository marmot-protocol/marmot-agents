//! Driving a driver as the core does, with the checker watching every step.
//!
//! [`Run`] holds one driver on a fake harness. It makes the requests the core makes, waits for each
//! to return (it never drops one, D-066), takes events from `next_event`, and hands everything to
//! the [`Checker`]. In `dropping` mode it waits for events as a core busy with other work does: it
//! polls `next_event` once and drops the future, again and again, so a future is dropped at every
//! point where the driver waits.

use crate::check::{Checker, Violation, broke, mark};
use crate::fake::{Fake, Heard};
use marmot_agent_driver::{
    Action, Answer, Chat, ChatId, Control, Driver, Emitted, Given, MessageId, Output, Person,
    Prompt, Refusal, Reply, RequestId, SendFile,
};
use marmot_agent_events::body::Effect;
use marmot_agent_events::{Body, SessionId, TurnId};
use std::future::Future;
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

/// One driver under test, with its fake harness: how to start it, and what its harness can be
/// made to do.
pub trait Subject: Send {
    /// The driver.
    type Driver: Driver + 'static;

    /// Starts a harness process and the driver on it. Called again after a restart: the harness
    /// keeps what it saved, as a real one keeps its sessions on disk.
    fn start(&mut self) -> impl Future<Output = Self::Driver> + Send;

    /// The fake harness.
    fn fake(&self) -> &Fake;

    /// Kills the harness that serves `chat`, now.
    fn kill(&mut self, chat: &ChatId);

    /// Whether one harness serves every chat, so killing it stops them all. Pi has one for each.
    fn shared_harness(&self) -> bool;

    /// A setting the harness offers and a new value for it.
    fn setting(&self) -> (String, String);

    /// The request the harness can be made never to answer, to see that the driver still returns.
    fn never(&self) -> &'static str;

    /// The chat a harness process served, by the tag it was started with. Only a driver with a
    /// harness for each chat tags them.
    fn chat_of(&self, tag: &str) -> Option<ChatId> {
        let _ = tag;
        None
    }

    /// The tag of the harness process that serves `chat`, once a prompt has shown it. Only a
    /// driver with a harness for each chat tags them.
    fn tag_of(&self, chat: &ChatId) -> Option<String> {
        let _ = chat;
        None
    }

    /// The prompt is checked as handed when the harness has read anything more, rather than by its
    /// text: a replay does not read its prompts.
    fn loose(&self) -> bool {
        false
    }

    /// A folder a chat may move the harness to, other than the driver's own (D-101). It must be a
    /// folder on disk for a driver that starts a program there.
    fn other_folder(&self) -> std::path::PathBuf {
        "/work/elsewhere".into()
    }

    /// Whether the driver gives the harness a tool to send a file to the chat (D-090), which the
    /// fake calls for [`Script::File`](crate::fake::Script::File). One that gives none must never
    /// hand over a file.
    fn sends_files(&self) -> bool {
        false
    }

    /// Whether the harness may send a reply after what follows from it, or answer a later request
    /// first. Codex and ACP agents may; Pi answers a prompt before its run reports anything, so
    /// its fake is made slow instead.
    fn reorders(&self) -> bool {
        true
    }
}

/// One run of a scenario: a driver, its fake harness, and the checker.
pub struct Run<S: Subject> {
    /// What is under test.
    pub subject: S,
    /// The driver, as the subject started it.
    pub driver: S::Driver,
    /// The checks.
    pub check: Checker,
    /// Waits for events are dropped at every point the driver waits.
    pub dropping: bool,
    /// How long a wait lasts before what it waits for counts as lost.
    pub patience: Duration,
    /// Requests are checked as handed when the harness has read anything more.
    pub loose: bool,
    /// Every event the driver handed over, in order.
    pub seen: Vec<Emitted>,
    /// Every file the driver asked the core to send, in order, not yet taken by
    /// [`Run::next_file`].
    pub files: std::collections::VecDeque<SendFile>,
    /// The folder each chat moved to, once the driver took the new session that moved it, as the
    /// core keeps it (D-101).
    pub folders: std::collections::HashMap<String, std::path::PathBuf>,
    next_message: u64,
    next_request: u64,
}

fn person() -> Person {
    Person {
        id: "u-1".into(),
        name: "Alice".into(),
    }
}

fn chat(id: &str, folder: Option<&std::path::PathBuf>) -> Chat {
    Chat {
        id: id.into(),
        group: false,
        name: None,
        folder: folder.cloned(),
    }
}

/// Polls a future once: its output if it is ready, and `None` if it is not, after which the
/// caller drops it.
async fn poll_once<F: Future>(future: F) -> Option<F::Output> {
    let mut future = pin!(future);
    std::future::poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Ready(output) => Poll::Ready(Some(output)),
        Poll::Pending => Poll::Ready(None),
    })
    .await
}

impl<S: Subject> Run<S> {
    /// Starts the subject's driver.
    pub async fn start(mut subject: S) -> Self {
        let driver = subject.start().await;
        Self {
            subject,
            driver,
            check: Checker::new(),
            dropping: false,
            patience: Duration::from_secs(5),
            loose: false,
            seen: Vec::new(),
            files: Default::default(),
            folders: Default::default(),
            next_message: 0,
            next_request: 0,
        }
    }

    /// The first violation so far.
    pub fn ok(&self) -> Result<(), Violation> {
        match self.check.first() {
            Some(v) => Err(v.clone()),
            None => Ok(()),
        }
    }

    fn fake(&self) -> &Fake {
        self.subject.fake()
    }

    /// Tells the checker of harness processes that died, before it sees anything they caused.
    fn notice_deaths(&mut self) {
        for tag in self.subject.fake().take_dead() {
            let chat = tag.and_then(|t| self.subject.chat_of(&t));
            if self.subject.shared_harness() {
                self.check.harness_stopped(None);
            } else if let Some(chat) = chat {
                self.check.harness_stopped(Some(&chat));
            }
        }
    }

    /// What became of a request the suite waited for: one that did not return within the
    /// patience never will (S11).
    fn returned(
        &mut self,
        what: &str,
        returned: Result<Result<(), Refusal>, tokio::time::error::Elapsed>,
    ) -> Result<Result<(), Refusal>, Violation> {
        let patience = self.patience;
        if returned.is_err() {
            self.half_lines()?;
        }
        returned.map_err(|_| {
            let v = broke("S11", format!("{what} did not return within {patience:?}"));
            self.check.record(v.clone());
            v
        })
    }

    /// A half line on the harness's input swallows the message after it, and a wait for what
    /// follows from that message runs out: the half line is the fault, and is reported first.
    fn half_lines(&mut self) -> Result<(), Violation> {
        let malformed = self.fake().malformed();
        self.check.half_lines(&malformed);
        self.ok()
    }

    /// Once a request has returned, the harness has it, with the driver not polled again (S11).
    async fn handed(
        &mut self,
        what: &str,
        done: impl Fn(&Self, &[Heard]) -> bool,
    ) -> Result<(), Violation> {
        let deadline = tokio::time::Instant::now() + self.patience;
        loop {
            let heard = self.fake().heard();
            if done(self, &heard) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        self.half_lines()?;
        let v = broke("S11", format!("{what} returned before the harness had it"));
        self.check.record(v.clone());
        Err(v)
    }

    /// Whether, since `mark`, the harness serving `chat` read something and then died, so that it
    /// may have died reading what the driver wrote for `chat`. A death of another chat's harness
    /// does not count. `unknown` counts a harness that has served no chat yet: Pi's driver starts
    /// one for a chat's first prompt, and it can die before it reads the prompt that names it.
    fn died_reading(&self, mark: usize, chat: Option<&ChatId>, unknown: bool) -> bool {
        let shared = self.subject.shared_harness();
        self.fake().died_reading(mark, |tag| {
            if shared {
                return true;
            }
            match tag.and_then(|t| self.subject.chat_of(t)) {
                Some(served) => chat == Some(&served),
                None => unknown,
            }
        })
    }

    /// Sends a prompt from `chat` with `words`, which tell the fake what to do
    /// ([`Script`](crate::fake::Script)). Its text carries the chat's mark and its message id.
    pub async fn prompt(
        &mut self,
        chat_id: &str,
        words: &str,
    ) -> Result<Result<MessageId, Refusal>, Violation> {
        self.prompt_in(chat_id, words, None).await
    }

    /// A prompt that names `session`, as the core names a chat's session after a restart (D-065).
    pub async fn prompt_in(
        &mut self,
        chat_id: &str,
        words: &str,
        session: Option<&str>,
    ) -> Result<Result<MessageId, Refusal>, Violation> {
        self.next_message += 1;
        let message = MessageId(format!("m-{}", self.next_message));
        let text = format!("{} {} {words}", mark(chat_id), message.0);
        let chat_id_owned = ChatId::from(chat_id);
        let (heard, malformed) = (self.fake().heard(), self.fake().malformed());
        (self.check).prompting(&message, &chat_id_owned, session, &heard, &malformed);
        let prompt = Prompt {
            message: message.clone(),
            text,
            chat: chat(chat_id, self.folders.get(chat_id)),
            sender: person(),
            reply_to: None,
            attachments: Vec::new(),
            session: session.map(SessionId::from),
        };
        self.notice_deaths();
        let frames = self.fake().frames();
        let mark = self.fake().mark();
        let returned = tokio::time::timeout(self.patience, self.driver.prompt(prompt)).await;
        let result = self.returned("a prompt", returned)?;
        if result.is_ok() {
            if !self.subject.shared_harness() {
                self.check.harness_back(&chat_id_owned); // a chat's harness starts again
            }
            let token = format!(" {} ", message.0);
            let loose = self.subject.loose() || self.loose;
            // A replay cannot say what it read: that it read more is then all there is to check.
            // Nor can a harness that died reading what was written for this prompt. A death
            // before, or of another chat's harness, shows nothing of this prompt.
            let chat = chat_id_owned.clone();
            self.handed("a prompt", move |run, heard| {
                let read = run.fake().frames() > frames;
                (read && loose)
                    || run.died_reading(mark, Some(&chat), true)
                    || heard.iter().any(|h| {
                        matches!(h, Heard::Prompt { text, .. } if format!("{text} ").contains(&token))
                    })
            })
            .await?;
        }
        Ok(result.map(|()| message))
    }

    fn request_id(&mut self) -> RequestId {
        self.next_request += 1;
        RequestId(format!("r-{}", self.next_request))
    }

    /// Answers an approval or a question an event shows: allowing it, or the first option, when
    /// `allow`; otherwise refusing it, or the second option. Of the choices that allow or refuse,
    /// the one for this time only is taken where there is one, as a careful person would, and as
    /// the recordings did.
    pub async fn answer(
        &mut self,
        asked: &Emitted,
        allow: bool,
    ) -> Result<(RequestId, Result<(), Refusal>), Violation> {
        let (given, expect, expect_text) = match asked.event.body() {
            Body::Approval(a) => {
                let wanted = if allow { Effect::Allow } else { Effect::Deny };
                let once = |c: &&marmot_agent_events::body::Choice| {
                    let words = format!("{} {}", c.id, c.label).to_lowercase();
                    c.effect == wanted && words.contains("once")
                };
                let choice = (a.choices.iter().find(once))
                    .or_else(|| a.choices.iter().find(|c| c.effect == wanted))
                    .or(a.choices.first())
                    .expect("an approval offers a choice");
                (
                    Given::Choice(choice.id.clone()),
                    Some(choice.effect == Effect::Allow),
                    None,
                )
            }
            Body::Question(q) => {
                // The words the harness should hear for the first question: the label of the
                // option chosen, or what was typed.
                let words = q.questions().first().map(|asked| {
                    let at = usize::from(!allow).min(asked.options.len().saturating_sub(1));
                    (asked.options.get(at)).map_or_else(|| "yes".to_string(), |o| o.label.clone())
                });
                let replies = (q.questions().iter())
                    .map(|asked| {
                        let at = usize::from(!allow).min(asked.options.len().saturating_sub(1));
                        Reply {
                            question: asked.id.clone(),
                            options: asked
                                .options
                                .get(at)
                                .map(|o| o.id.clone())
                                .into_iter()
                                .collect(),
                            text: asked.options.is_empty().then(|| "yes".to_string()),
                        }
                    })
                    .collect();
                (Given::Questions(replies), None, words)
            }
            _ => panic!("only an approval or a question takes an answer"),
        };
        let request = self.request_id();
        let chat = asked.chat.clone();
        self.check.requesting(&request, chat.as_ref());
        let answer = Answer {
            request: request.clone(),
            item: asked.event.header().item.clone(),
            by: person(),
            given,
        };
        let before = self.answers_heard();
        let frames = self.fake().frames();
        let mark = self.fake().mark();
        let returned = tokio::time::timeout(self.patience, self.driver.answer(answer)).await;
        let result = self.returned("an answer", returned)?;
        self.check.requested(&request, &result);
        if result.is_ok() && self.subject.loose() {
            self.handed("an answer", move |run, _| run.fake().frames() > frames)
                .await?;
        } else if result.is_ok() {
            if let Some(chat) = &chat {
                self.check.expect_answer(chat, expect, expect_text);
            }
            // The harness heard it, or the chat's own harness died on reading what the driver
            // wrote after it was given, and cannot say what it heard. A harness that had died
            // before cannot have read it, nor can another chat's.
            self.handed("an answer", move |run, heard| {
                count_answers(heard) > before || run.died_reading(mark, chat.as_ref(), false)
            })
            .await?;
        }
        Ok((request, result))
    }

    fn answers_heard(&self) -> usize {
        count_answers(&self.fake().heard())
    }

    /// Sends a control for `chat`.
    pub async fn control(
        &mut self,
        chat_id: &str,
        action: Action,
    ) -> Result<(RequestId, Result<(), Refusal>), Violation> {
        let request = self.request_id();
        let chat = ChatId::from(chat_id);
        self.check.requesting(&request, Some(&chat));
        let control = Control {
            request: request.clone(),
            by: person(),
            action,
        };
        let frames = self.fake().frames();
        let returned = tokio::time::timeout(self.patience, self.driver.control(control)).await;
        let result = self.returned("a control", returned)?;
        self.check.requested(&request, &result);
        if result.is_ok() {
            // A control may be a harness's own message that it never answers, or a new process
            // (Pi, for a chat with none): handed is that the harness read something more.
            self.handed("a control", move |run, _| run.fake().frames() > frames)
                .await?;
        }
        Ok((request, result))
    }

    /// Interrupts a turn of `chat`.
    pub async fn interrupt(
        &mut self,
        chat_id: &str,
        turn: &TurnId,
    ) -> Result<(RequestId, Result<(), Refusal>), Violation> {
        let action = Action::Interrupt { turn: turn.clone() };
        self.control(chat_id, action).await
    }

    /// Asks for a new session for `chat`, ending `session`.
    pub async fn new_session(
        &mut self,
        chat_id: &str,
        session: Option<&str>,
    ) -> Result<(RequestId, Result<(), Refusal>), Violation> {
        let action = Action::NewSession {
            chat: chat(chat_id, self.folders.get(chat_id)),
            session: session.map(SessionId::from),
        };
        self.control(chat_id, action).await
    }

    /// A new session in `folder`, as the core asks for one when a chat moves (D-101): once the
    /// driver has taken it, the chat's prompts name the folder.
    pub async fn new_session_in(
        &mut self,
        chat_id: &str,
        session: Option<&str>,
        folder: &std::path::Path,
    ) -> Result<(RequestId, Result<(), Refusal>), Violation> {
        let action = Action::NewSession {
            chat: chat(chat_id, Some(&folder.to_path_buf())),
            session: session.map(SessionId::from),
        };
        let (request, result) = self.control(chat_id, action).await?;
        if result.is_ok() {
            self.folders
                .insert(chat_id.to_string(), folder.to_path_buf());
        }
        Ok((request, result))
    }

    /// Changes the harness's setting in `session`.
    pub async fn set(
        &mut self,
        chat_id: &str,
        session: &str,
    ) -> Result<(RequestId, Result<(), Refusal>), Violation> {
        let (setting, value) = self.subject.setting();
        let action = Action::Set {
            session: session.into(),
            setting,
            value,
        };
        self.control(chat_id, action).await
    }

    /// The driver's next output, or `None` once it has ended, as the core takes it: waiting, or,
    /// when dropping, polling once and dropping the wait until one comes.
    pub async fn next(&mut self) -> Option<Output> {
        let output = if self.dropping {
            loop {
                if let Some(output) = poll_once(self.driver.next_event()).await {
                    break output;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        } else {
            self.driver.next_event().await
        };
        self.notice_deaths();
        self.check.output(output.as_ref());
        match &output {
            Some(Output::Event(e)) => self.seen.push(e.clone()),
            Some(Output::SendFile(file)) => self.files.push_back(file.clone()),
            None => {}
        }
        output
    }

    /// The next file the driver asks the core to send, within the patience. If none comes, that
    /// breaks `why`.
    pub async fn next_file(
        &mut self,
        why: &'static str,
        what: &str,
    ) -> Result<SendFile, Violation> {
        let deadline = tokio::time::Instant::now() + self.patience;
        loop {
            if let Some(file) = self.files.pop_front() {
                return Ok(file);
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, self.next()).await {
                Ok(Some(_)) => self.ok()?,
                Ok(None) => return Err(self.lost(why, format!("the driver ended before {what}"))),
                Err(_) => return Err(self.lost(why, format!("{what} never came"))),
            }
        }
    }

    /// Tells the driver how a file it asked to send went, as the core does once it has sent it or
    /// refused it, and waits for that to return (S11). The harness should then hear it, in the
    /// session of the file's chat.
    pub async fn file_sent(
        &mut self,
        file: &SendFile,
        result: Result<(), Refusal>,
    ) -> Result<(), Violation> {
        self.check.expect_file(&file.chat, result.is_ok());
        let returned = tokio::time::timeout(
            self.patience,
            self.driver.file_sent(file.id.clone(), result),
        )
        .await;
        let patience = self.patience;
        returned.map_err(|_| {
            self.lost(
                "S11",
                format!("telling the driver a file's result did not return within {patience:?}"),
            )
        })
    }

    /// The first event seen that passes `found`, or else the next to come, within the patience.
    pub async fn wait(
        &mut self,
        why: &'static str,
        what: &str,
        found: impl Fn(&Emitted) -> bool,
    ) -> Result<Emitted, Violation> {
        if let Some(e) = self.seen.iter().find(|e| found(e)) {
            return Ok(e.clone());
        }
        self.until(why, what, found).await
    }

    /// Events until one passes `found`, which is returned. If none does within the patience, or
    /// the driver ends first, that breaks `why`.
    pub async fn until(
        &mut self,
        why: &'static str,
        what: &str,
        found: impl Fn(&Emitted) -> bool,
    ) -> Result<Emitted, Violation> {
        let deadline = tokio::time::Instant::now() + self.patience;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let next = tokio::time::timeout(left, self.next()).await;
            self.ok()?;
            let output = match next {
                Ok(Some(output)) => output,
                Ok(None) => {
                    self.half_lines()?;
                    return Err(self.lost(why, format!("the driver ended before {what}")));
                }
                Err(_) => {
                    self.half_lines()?;
                    return Err(self.lost(why, format!("{what} never came")));
                }
            };
            if let Some(e) = output.into_event()
                && found(&e)
            {
                return Ok(e);
            }
        }
    }

    fn lost(&mut self, why: &'static str, detail: String) -> Violation {
        let v = broke(why, detail);
        self.check.record(v.clone());
        v
    }

    /// Takes what the driver hands over until it has nothing for `quiet`, or ends. Returns
    /// whether it ended.
    pub async fn drain(&mut self, quiet: Duration) -> bool {
        loop {
            match tokio::time::timeout(quiet, self.next()).await {
                Ok(Some(_)) => {}
                Ok(None) => return true,
                Err(_) => return false,
            }
        }
    }

    /// Takes what the driver has, then checks what the harness heard.
    pub async fn checkpoint(&mut self) -> Result<(), Violation> {
        self.drain(Duration::from_millis(60)).await;
        let (heard, malformed) = (self.fake().heard(), self.fake().malformed());
        self.check.heard(&heard, &malformed);
        self.ok()
    }

    /// Kills the harness serving `chat`.
    pub fn kill(&mut self, chat_id: &str) {
        let chat = ChatId::from(chat_id);
        self.subject.kill(&chat);
        let all = self.subject.shared_harness();
        self.check.harness_stopped((!all).then_some(&chat));
    }

    /// Drops the driver and its harness, and starts both again, as a restart of the core does.
    pub async fn restart(&mut self) -> Result<(), Violation> {
        self.checkpoint().await?;
        self.check.restarted();
        self.seen.clear();
        let new = self.subject.start().await;
        drop(std::mem::replace(&mut self.driver, new));
        // The old harness sees its input close, and ends, before anything reaches the new one.
        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(())
    }

    /// The end of a scenario: what is left is handed over, what the harness heard is checked,
    /// every request the driver took is settled, and every answer reached the harness.
    pub async fn end(&mut self) -> Result<(), Violation> {
        self.checkpoint().await?;
        self.check.all_settled();
        if self.fake().deaths() == 0 {
            self.check.answers_heard();
        }
        self.ok()
    }
}

fn count_answers(heard: &[Heard]) -> usize {
    heard
        .iter()
        .filter(|h| matches!(h, Heard::Answer { .. }))
        .count()
}

/// Whether an event is a turn, in a state that has ended.
pub fn turn_ended(e: &Emitted) -> bool {
    matches!(e.event.body(), Body::Turn(_))
        && !matches!(
            e.event.header().state,
            marmot_agent_events::State::Running | marmot_agent_events::State::Waiting
        )
}

/// Whether an event is an approval or a question waiting for an answer.
pub fn waiting(e: &Emitted) -> bool {
    matches!(e.event.body(), Body::Approval(_) | Body::Question(_))
        && e.event.header().state == marmot_agent_events::State::Waiting
}
