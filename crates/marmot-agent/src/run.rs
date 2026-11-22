//! The core running: MDK on one side, a driver on the other, and the conversation between them.
//!
//! One task does everything, so the conversation is never touched from two places. It waits for
//! whichever comes first, a group message or a driver event, hands it to the conversation, and
//! carries out the acts that come back one at a time, telling the conversation how each went. A
//! driver request is always awaited to the end (D-066); only the wait for the driver's next event
//! is dropped when a group message comes first, which the trait allows (D-058). The one piece of
//! work done on the side is a file the agent sends (D-090): uploading it can take long, so it runs
//! as a task of its own, and how it went comes back to this one, which tells the driver.
//!
//! What the core keeps across a restart is one file: the conversation's state, what it means to
//! send, and for each group how far it has read, in the order messages arrived on this machine.
//! It is written before anything leaves the core: a message is marked read before it reaches the
//! driver, so a crash never hands the driver the same prompt twice (D-037); a request is pending
//! on disk before the driver gets it, so a crash leaves it to be settled (D-070); and a version
//! of an item is queued on disk, with what it settles, before it is sent, so a crash leaves it to
//! be sent again as it was (S1, S3). When the file cannot be written the core stops where it is,
//! as a crash would, and the next start takes up what is on disk. MDK does not replay what
//! arrived while a program was down, so on start, and whenever it falls behind, the core reads
//! what was stored after the read mark.

use crate::config::Config;
use crate::conversation::{Act, Conversation, Saved, Sending};
use crate::files::Folders;
use crate::keys::GroupKey;
use crate::marmot::{FileFailed, Incoming, Live, Marmot, Message, SendFailed, Side};
use crate::socket::SocketDriver;
use marmot_agent_driver::{Driver, Output, Reason, Refusal, SendFile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// The longest a group's sends wait before the next try, after failing again and again.
const SEND_WAIT_MAX: Duration = Duration::from_secs(10);

/// How long MDK is given to finish a send whose outcome is not known, before the core looks for
/// it in what MDK stored and, not finding it, sends it again.
const SETTLE: Duration = Duration::from_secs(15);

/// How many times a send whose outcome is never known is tried. After the last try it is looked
/// for once more, when MDK has settled it, and only if it is not there is it given up as refused.
const UNKNOWN_TRIES: u32 = 10;

/// How many sends one group makes in one turn of the core's loop, so one chat's backlog, or a slow
/// MDK, does not hold up the others.
const DRAIN: usize = 8;

/// A send whose outcome is not known.
struct Unknown {
    /// When MDK will have settled it, so the core can tell from what it stored.
    settled: Instant,
    /// How many times it was tried without knowing.
    tries: u32,
}

/// The state file could not be written: the core stops where it is, as a crash would.
#[derive(Debug)]
struct Unsaved;

/// What the core says when it stops because it could not save.
const UNSAVED: &str = "the state file could not be written, so the core stopped";

/// How many times a file is offered to MDK while MDK says it is busy, about half a minute in all.
const FILE_BUSY_TRIES: u32 = 10;

/// How a file the agent asked to send went, for the driver.
struct Uploaded {
    id: String,
    chat: marmot_agent_driver::ChatId,
    result: Result<(), Refusal>,
}

/// What the core keeps across a restart.
#[derive(Default, Serialize, Deserialize)]
pub struct Kept {
    /// The conversation's state.
    pub conversation: Saved,
    /// For each group, the place in storage order of the last message read.
    pub read: BTreeMap<GroupKey, i64>,
}

/// Which save fails: one that would write a state it matches.
type FailWhen = dyn Fn(&Kept) -> bool + Send + Sync;

/// Where the kept state lives.
pub struct Store {
    path: PathBuf,
    /// A fault for the tests of a failed save, as the simulator will put one
    /// (`docs/design/verification.md`): none in use.
    fault: Option<(Arc<FailWhen>, Arc<AtomicBool>)>,
}

impl Store {
    /// The state file in `dir`.
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("marmot-agent-state.json"),
            fault: None,
        }
    }

    /// The state file in `dir`, where the first save of a state `when` matches fails, as on a
    /// full disk, and so does every save after it. The flag given back says whether it has.
    pub fn failing_when(
        dir: &Path,
        when: impl Fn(&Kept) -> bool + Send + Sync + 'static,
    ) -> (Self, Arc<AtomicBool>) {
        let failed = Arc::new(AtomicBool::new(false));
        let store = Self {
            fault: Some((Arc::new(when), failed.clone())),
            ..Self::new(dir)
        };
        (store, failed)
    }

    /// What was kept: nothing on a first start. A file that is there and cannot be read is an
    /// error, not a first start, since starting afresh would leave the chat's open items open.
    pub fn load(&self) -> Result<Kept, String> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "the state file is damaged; move it away to start afresh".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Kept::default()),
            Err(error) => Err(format!(
                "the state file could not be read ({})",
                error.kind()
            )),
        }
    }

    /// Writes the state whole: to a file only its owner can read, flushed to disk, then put in
    /// place of the old one, so a crash leaves the old file or the new one and never half of one.
    pub fn save(&self, kept: &Kept) -> std::io::Result<()> {
        if let Some((when, failed)) = &self.fault
            && (failed.load(SeqCst) || when(kept))
        {
            failed.store(true, SeqCst);
            return Err(std::io::Error::other("a save made to fail"));
        }
        let bytes = serde_json::to_vec(kept).map_err(std::io::Error::other)?;
        let temporary = self.path.with_extension("json.new");
        let mut file = private_file(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &self.path)?;
        if let Some(dir) = self.path.parent() {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::create(path)
}

/// What woke the core's loop: one of the things it waits for. One lives at a time, briefly, so
/// its size does not matter.
#[allow(clippy::large_enum_variant)]
enum Woke {
    /// The clock's tick, for the requests that have waited too long.
    Tick,
    /// A group's sends may be tried again.
    Retry,
    /// The driver's next event, or its end.
    Driver(Option<Output>),
    /// A file finished uploading.
    Uploaded(Uploaded),
    /// A live event from the Marmot side.
    Marmot(Live),
    /// The core is asked to stop.
    Stop,
}

/// How many things the core's loop waits for: the variants of [`Woke`].
const WAITS: usize = 6;

/// The core: MDK, a driver, and the conversation. It reaches MDK through [`Side`], which
/// [`Marmot`] is, so a simulator can stand in for MDK.
pub struct Core<D: Driver, M: Side = Marmot> {
    marmot: M,
    driver: D,
    conversation: Conversation,
    read: BTreeMap<GroupKey, i64>,
    store: Store,
    config: Config,
    /// The folders the agent may send files from (D-090).
    folders: Folders,
    /// The core's clock, which never jumps, for how long a control has waited (D-087). Tokio's,
    /// which is the system's except in a simulation, whose clock is virtual.
    started: Instant,
    /// For each group whose last send failed: when to try again, and how long it waited last.
    retry: HashMap<GroupKey, (Instant, Duration)>,
    /// Sends, by token, whose outcome is not known: they may have gone out.
    unknown: HashMap<String, Unknown>,
    /// How long MDK is given to settle a send whose outcome is not known: [`SETTLE`].
    settle: Duration,
    /// How many times such a send is tried: [`UNKNOWN_TRIES`].
    unknown_tries: u32,
    /// How many sends each group made in this turn of the loop, at most [`DRAIN`].
    drained: HashMap<GroupKey, usize>,
    /// Whether [`DRAIN`] holds: not when the core stops, which sends all it can.
    bounded: bool,
    /// Which of the things the loop waits for is looked at first in its next turn: each in its
    /// turn, so when several are ready at once none waits behind the others for long, and the
    /// same inputs always give the same order, which a simulation replays.
    first: usize,
    /// Faults put back for the simulator, to show its checks find them: none in use.
    #[cfg(feature = "sim-bugs")]
    bugs: Bugs,
}

/// Bugs fixed before, put back on purpose so the simulator can show it finds each
/// (`docs/design/verification.md`, "A check must be shown to catch its bug"). Only in a build
/// with the `sim-bugs` feature, which no release has.
#[cfg(feature = "sim-bugs")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bugs {
    /// A send is saved only after it went, not before: a crash in between forgets it (S2, S3).
    pub save_after_send: bool,
    /// A send whose outcome is not known is sent again at once, without looking for it (S1, S3).
    pub retry_ambiguous_blindly: bool,
    /// What the agent's words settle is ignored: settles are read only from items (S1).
    pub settles_only_from_items: bool,
}

impl<D: Driver, M: Side> Core<D, M> {
    /// A core for the agent on `marmot`, driving `driver`, with the settings in `config`, keeping
    /// its state in `store`.
    pub fn new(marmot: M, driver: D, config: &Config, store: Store) -> Result<Self, String> {
        // A listed folder is checked against the home, since an import or a hand may have put
        // one in the wrong place; whatever the folders, no file inside the home is sent unless
        // the owner allows it (D-099).
        let folders = Folders::from_config(config)?;
        let kept = store.load()?;
        let conversation = Conversation::new(
            marmot.agent().clone(),
            driver.harness().name,
            config.rights(),
            driver.capabilities(),
            kept.conversation,
        );
        let attempted = conversation.attempted_tokens();
        Ok(Self {
            marmot,
            driver,
            conversation,
            read: kept.read,
            store,
            folders,
            config: config.clone(),
            started: Instant::now(),
            retry: HashMap::new(),
            // A send left queued that was handed to MDK may have gone out just before the core
            // stopped.
            unknown: (attempted.into_iter())
                .map(|token| {
                    let settled = Instant::now();
                    (token, Unknown { settled, tries: 0 })
                })
                .collect(),
            settle: SETTLE,
            unknown_tries: UNKNOWN_TRIES,
            drained: HashMap::new(),
            bounded: true,
            first: 0,
            #[cfg(feature = "sim-bugs")]
            bugs: Bugs::default(),
        })
    }

    /// Puts back the bugs in `bugs`, for the simulator.
    #[cfg(feature = "sim-bugs")]
    pub fn with_bugs(mut self, bugs: Bugs) -> Self {
        self.bugs = bugs;
        self.conversation
            .settles_only_from_items(bugs.settles_only_from_items);
        self
    }

    /// Sends whose outcome is not known are looked for after `settle`, and tried `tries` times:
    /// for the tests, as the simulator will set them (`docs/design/verification.md`).
    pub fn unknown_sends(mut self, tries: u32, settle: Duration) -> Self {
        self.unknown_tries = tries;
        self.settle = settle;
        self
    }

    /// Runs until the driver ends or `stop` resolves. MDK's runtime must already have been
    /// subscribed to (`events`) and started, so nothing that arrives during start-up is missed.
    /// An error when the state file could not be written: the core stopped where it was.
    pub async fn run(
        mut self,
        events: M::Events,
        stop: impl Future<Output = ()>,
    ) -> Result<(), String> {
        self.run_until(events, stop).await.map_err(|Unsaved| {
            eprintln!("marmot-agent: {UNSAVED}");
            UNSAVED.to_string()
        })
    }

    async fn run_until(
        &mut self,
        mut events: M::Events,
        stop: impl Future<Output = ()>,
    ) -> Result<(), Unsaved> {
        self.publish_name().await;
        // What a stopped core left queued goes first, then what it left open is closed.
        self.flush_all().await?;
        let recovered = self.conversation.recover();
        self.carry_out(recovered).await?;
        self.consider_invitations().await?;
        self.catch_up().await?;
        tokio::pin!(stop);
        let mut clock = tokio::time::interval(std::time::Duration::from_secs(10));
        // Files being uploaded say here how they went. A file still uploading when the core
        // stops is not waited for: the driver it would answer goes with the core.
        let (uploaded, mut uploads) = mpsc::unbounded_channel::<Uploaded>();
        loop {
            // Each group may send a few more events in this turn.
            self.drained.clear();
            let retry = self.retry.values().map(|(at, _)| *at).min();
            let retry = async move {
                match retry {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            // Whichever comes first. The waits not taken are dropped, which each allows: the
            // driver's by its trait (D-058), the others lose nothing.
            let first = self.first;
            self.first = (first + 1) % WAITS;
            let woke = {
                let mut tick = pin!(clock.tick());
                let mut retry = pin!(retry);
                let mut driver = pin!(self.driver.next_event());
                let mut upload = pin!(uploads.recv());
                let mut live = pin!(self.marmot.next(&mut events));
                let mut stop = stop.as_mut();
                std::future::poll_fn(|cx| {
                    for wait in (0..WAITS).map(|i| (first + i) % WAITS) {
                        let woke = match wait {
                            0 => tick.as_mut().poll(cx).map(|_| Woke::Tick),
                            1 => retry.as_mut().poll(cx).map(|()| Woke::Retry),
                            2 => driver.as_mut().poll(cx).map(Woke::Driver),
                            // The core holds a sender, so this never ends.
                            3 => match upload.as_mut().poll(cx) {
                                Poll::Ready(Some(done)) => Poll::Ready(Woke::Uploaded(done)),
                                _ => Poll::Pending,
                            },
                            4 => live.as_mut().poll(cx).map(Woke::Marmot),
                            _ => stop.as_mut().poll(cx).map(|()| Woke::Stop),
                        };
                        if woke.is_ready() {
                            return woke;
                        }
                    }
                    Poll::Pending
                })
                .await
            };
            match woke {
                Woke::Tick => loop {
                    let acts = self.conversation.overdue(self.clock());
                    if acts.is_empty() {
                        break;
                    }
                    self.carry_out(acts).await?;
                },
                Woke::Retry => self.flush_all().await?,
                Woke::Driver(Some(Output::Event(emitted))) => {
                    let acts = self.conversation.emitted(emitted);
                    self.carry_out(acts).await?;
                }
                Woke::Driver(Some(Output::SendFile(file))) => {
                    self.send_file(file, &uploaded).await?
                }
                Woke::Driver(None) => break,
                Woke::Uploaded(done) => self.file_done(done).await?,
                Woke::Marmot(Live::Message(message)) => self.handle(message).await?,
                Woke::Marmot(Live::Groups) => self.consider_invitations().await?,
                Woke::Marmot(Live::Other) => {}
                Woke::Marmot(Live::Lagged) => self.catch_up().await?,
                Woke::Marmot(Live::Closed) | Woke::Stop => break,
            }
        }
        // Everything the stop closes is queued and saved at once, before any of it is sent: a
        // crash in between sends what is left when the core starts again, and closes nothing
        // twice.
        let ended = self.conversation.ended();
        self.retry.clear();
        self.bounded = false;
        self.carry_out(ended).await?;
        self.flush_all().await
    }

    /// Starts sending a file the agent asked to send, as a task of its own so the chats go on
    /// meanwhile. Where it goes, the chat's group and running turn, is taken now (D-090).
    async fn send_file(
        &mut self,
        file: SendFile,
        uploaded: &mpsc::UnboundedSender<Uploaded>,
    ) -> Result<(), Unsaved> {
        let SendFile { id, chat, path } = file;
        let Some((group, turn)) = self.conversation.file_place(&chat) else {
            let result = Err(Refusal {
                reason: Reason::Invalid,
                detail: Some("The agent named a chat it is not in.".into()),
            });
            return self.file_done(Uploaded { id, chat, result }).await;
        };
        let (marmot, folders, uploaded) =
            (self.marmot.clone(), self.folders.clone(), uploaded.clone());
        tokio::spawn(async move {
            let result = upload(&marmot, folders, path, &group, turn).await;
            let _ = uploaded.send(Uploaded { id, chat, result });
        });
        Ok(())
    }

    /// Tells the driver how a file went. A refusal is shown in the chat, naming no path (D-090).
    async fn file_done(&mut self, done: Uploaded) -> Result<(), Unsaved> {
        if let Err(refusal) = &done.result {
            // Its kind only: a path or a file name is never logged (D-071).
            eprintln!("marmot-agent: a file was not sent ({:?})", refusal.reason);
            let acts = self.conversation.file_refused(&done.chat, refusal);
            self.carry_out(acts).await?;
        }
        self.driver.file_sent(done.id, done.result).await;
        Ok(())
    }

    /// Seconds on the core's clock.
    fn clock(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Publishes the agent's name from the settings in its profile, so people see it, keeping the
    /// rest of the profile as it is. Nothing is published when the name is already right.
    async fn publish_name(&mut self) {
        let Some(name) = self.config.name.clone() else {
            return;
        };
        self.marmot.publish_name(&name, &self.config.relays).await;
    }

    /// Accepts an invitation from someone who may add the agent, and declines the rest. A group
    /// the agent has just joined is read from its start: the agent cannot read anything sent
    /// before it joined, so everything stored is for it. The mark that says so is saved before
    /// the agent accepts, so a crash in between never leaves a group accepted with no mark, which
    /// would start it at its latest message and skip what arrived meanwhile.
    async fn consider_invitations(&mut self) -> Result<(), Unsaved> {
        let invitations: Vec<(GroupKey, bool)> = (self.marmot.invitations().into_iter())
            .map(|(group, welcomer)| {
                let allowed = welcomer.as_ref().is_some_and(|w| self.config.may_invite(w));
                (group, allowed)
            })
            .collect();
        let accepting: Vec<&GroupKey> = (invitations.iter())
            .filter(|(_, allowed)| *allowed)
            .map(|(group, _)| group)
            .collect();
        if !accepting.is_empty() {
            for group in accepting {
                self.read.insert(group.clone(), 0);
            }
            self.save()?;
        }
        let mut joined = false;
        for (group, allowed) in invitations {
            // A mark left for an invitation not accepted is never read: only accepted groups are.
            joined |= self.marmot.decide(&group, allowed).await && allowed;
        }
        if joined {
            self.catch_up().await?;
        }
        Ok(())
    }

    /// Reads what was stored for each group after its mark: what arrived while the core was down,
    /// or while it fell behind. A group the core has never seen starts at its latest message.
    async fn catch_up(&mut self) -> Result<(), Unsaved> {
        for group in self.marmot.groups() {
            let after = match self.read.get(&group) {
                Some(&after) => after,
                None => {
                    let latest = self.marmot.latest(&group);
                    self.read.insert(group, latest);
                    self.save()?;
                    continue;
                }
            };
            for message in self.marmot.stored(&group, after) {
                self.handle(message).await?;
            }
        }
        Ok(())
    }

    /// Hands one message to the conversation, once. What the conversation makes of it, such as a
    /// request pending for the driver or a refusal queued, is saved in one write with the mark
    /// that says it was read, before anything reaches the driver: a crash before that write leaves
    /// the message to be read again, and one after it leaves the request to be settled (D-037,
    /// D-070). A mark that cannot be saved stops the core before the message reaches the driver.
    async fn handle(&mut self, message: Message) -> Result<(), Unsaved> {
        let Some(order) = self.marmot.order_of(&message) else {
            return Ok(());
        };
        let group = message.group.clone();
        if self.read.get(&group).is_some_and(|&read| order <= read) {
            return Ok(());
        }
        let acts = match self.marmot.incoming(message).await {
            Some(Incoming::Heard(heard)) => self.conversation.heard(heard),
            Some(Incoming::Asked(asked)) => self.conversation.asked(asked),
            None => Vec::new(),
        };
        self.read.insert(group, order);
        // Saved before the first act, or at the end when there is none.
        self.carry_out(acts).await
    }

    /// Carries out acts one at a time, and those that follow from each. The conversation is
    /// saved before each act, so what led to it is on disk first: a request is pending before the
    /// driver gets it (D-070), and a send is queued with its version and what it settles before
    /// it goes (S1, S3). It is saved again at the end.
    async fn carry_out(&mut self, acts: Vec<Act>) -> Result<(), Unsaved> {
        let mut queue: VecDeque<Act> = acts.into();
        while let Some(act) = queue.pop_front() {
            if !(matches!(act, Act::Send { .. }) && self.bug_save_after_send()) {
                self.save()?;
            }
            let next = self.carry_out_one(act).await?;
            queue.extend(next);
        }
        self.save()
    }

    async fn carry_out_one(&mut self, act: Act) -> Result<Vec<Act>, Unsaved> {
        Ok(match act {
            Act::Prompt(prompt) => {
                let result = self.driver.prompt(prompt.clone()).await;
                self.conversation.prompted(&prompt, result)
            }
            Act::Answer(answer) => {
                let request = answer.request.clone();
                let result = self.driver.answer(answer).await;
                self.conversation.requested(&request, result, self.clock())
            }
            Act::Control(control) => {
                let request = control.request.clone();
                let result = self.driver.control(control).await;
                self.conversation.requested(&request, result, self.clock())
            }
            Act::Send { group } => {
                self.flush(&group).await?;
                Vec::new()
            }
        })
    }

    /// Sends what is queued for every group whose turn to try has come.
    async fn flush_all(&mut self) -> Result<(), Unsaved> {
        for group in self.conversation.queued_groups() {
            self.flush(&group).await?;
        }
        Ok(())
    }

    /// Sends what is queued for a group, oldest first, each marked sent and saved once it has
    /// gone, a few in each turn of the loop so other work and other groups go on between. A send MDK is busy for stays queued,
    /// and the group waits, longer each time, before it is tried again; the other groups go on
    /// meanwhile. A send whose outcome is not known may have gone out, so it is not sent again
    /// blindly: once MDK has had time to settle it, the core looks for it in what MDK stored, and
    /// sends it again only if it is not there (S3). A send MDK refuses, or one given up after its
    /// tries, is handed to the conversation, which drops it with a notice, closing an item whose
    /// closing version it was and settling in its place what a notice settled, or sends an edit
    /// whose message is gone as a new message. A send is otherwise never given up while the core runs, so a later version never
    /// goes out as if it were new, and one still queued when the core stops goes out when it
    /// starts again (S1, S3).
    async fn flush(&mut self, group: &GroupKey) -> Result<(), Unsaved> {
        if self
            .retry
            .get(group)
            .is_some_and(|(at, _)| *at > Instant::now())
        {
            return Ok(());
        }
        while let Some((token, what)) = self.conversation.next_send(group) {
            let made = self.drained.entry(group.clone()).or_default();
            if self.bounded && *made >= DRAIN {
                // More is queued: come back to it in the loop's next turn, after other work.
                let wait = self.retry.get(group).map_or(Duration::ZERO, |(_, w)| *w);
                self.retry.insert(group.clone(), (Instant::now(), wait));
                return Ok(());
            }
            *made += 1;
            let key = match &what {
                Sending::Item { key, .. } | Sending::Words { key, .. } => key.clone(),
            };
            if let Some(&Unknown { settled, tries }) = self.unknown.get(&token) {
                let taken = |event: &crate::keys::EventKey| self.conversation.is_first(event);
                match self.marmot.find_sent(group, &what, &taken) {
                    Ok(Some(event)) => {
                        self.unknown.remove(&token);
                        self.conversation.sent(&key, event);
                        self.save()?;
                        continue;
                    }
                    Ok(None) => {}
                    // Not knowing is not "not there": ask again later, never send blindly.
                    Err(_) => {
                        self.wait_longer(group);
                        return Ok(());
                    }
                }
                if settled > Instant::now() {
                    let wait = self.retry.get(group).map_or(Duration::ZERO, |(_, w)| *w);
                    self.retry.insert(group.clone(), (settled, wait));
                    return Ok(());
                }
                if tries >= self.unknown_tries {
                    // Tried as often as it is tried, and its last try is not in what MDK
                    // stored: given up, as MDK refused it.
                    self.give_up(&token, false)?;
                    continue;
                }
            }
            // Saved as tried before MDK has it, so a crash after leaves it to be looked for.
            if self.conversation.attempting(&token) && !self.bug_save_after_send() {
                self.save()?;
            }
            match self.marmot.send(group, &what, &token).await {
                Ok(event) => {
                    self.retry.remove(group);
                    self.unknown.remove(&token);
                    self.conversation.sent(&key, event);
                    self.save()?;
                }
                Err(failed) => {
                    let failed = self.bug_blind_retry(failed);
                    if let SendFailed::Unknown = failed {
                        // Looked for once MDK has settled it, and given up only after that.
                        let tries = self.unknown.get(&token).map_or(0, |u| u.tries) + 1;
                        let settled = Instant::now() + self.settle;
                        self.unknown
                            .insert(token.clone(), Unknown { settled, tries });
                    }
                    match failed {
                        SendFailed::Refused { gone } => self.give_up(&token, gone)?,
                        SendFailed::Busy | SendFailed::Unknown => {
                            if matches!(failed, SendFailed::Unknown) {
                                eprintln!("marmot-agent: a send failed; it will be tried again");
                            }
                            self.wait_longer(group);
                            return Ok(());
                        }
                    }
                }
            }
        }
        self.retry.remove(group);
        Ok(())
    }

    /// The send named `token` will not go out: the conversation drops it with a notice and what
    /// stands in for it (`Conversation::refused`), or sends an edit whose message is `gone` as a
    /// new message.
    fn give_up(&mut self, token: &str, gone: bool) -> Result<(), Unsaved> {
        // MDK's own words can name a group, so they are not printed (D-071).
        eprintln!("marmot-agent: a send was refused; the chat is told");
        self.unknown.remove(token);
        self.conversation.refused(token, gone);
        self.save()
    }

    /// The group's sends wait before they are tried again, longer each time.
    fn wait_longer(&mut self, group: &GroupKey) {
        let wait = (self.retry.get(group)).map_or(Duration::from_millis(100), |(_, w)| {
            (*w * 2).clamp(Duration::from_millis(100), SEND_WAIT_MAX)
        });
        (self.retry).insert(group.clone(), (Instant::now() + wait, wait));
    }

    /// Whether the bug "a send is saved only after it went" is put back.
    #[cfg(feature = "sim-bugs")]
    fn bug_save_after_send(&self) -> bool {
        self.bugs.save_after_send
    }

    #[cfg(not(feature = "sim-bugs"))]
    fn bug_save_after_send(&self) -> bool {
        false
    }

    /// What a failed send is taken as: as it is, or, with the bug "a send whose outcome is not
    /// known is sent again at once" put back, as busy.
    fn bug_blind_retry(&self, failed: SendFailed) -> SendFailed {
        #[cfg(feature = "sim-bugs")]
        if self.bugs.retry_ambiguous_blindly && failed == SendFailed::Unknown {
            return SendFailed::Busy;
        }
        failed
    }

    fn save(&self) -> Result<(), Unsaved> {
        let kept = Kept {
            conversation: self.conversation.saved().clone(),
            read: self.read.clone(),
        };
        self.store.save(&kept).map_err(|_| Unsaved)
    }
}

/// Reads a file from the allowed folders and sends it to `group`. It is offered to MDK again only
/// while MDK says it is busy, which MDK says only before it has taken the file. Any other failure
/// may have come after MDK took it, such as a wait for MDK that ran out while the send went on, so
/// it is not tried again: that could post the file twice, and a post cannot be taken back.
async fn upload(
    marmot: &impl Side,
    folders: Folders,
    path: PathBuf,
    group: &GroupKey,
    turn: Option<marmot_agent_events::TurnId>,
) -> Result<(), Refusal> {
    let read = tokio::task::spawn_blocking(move || folders.read(&path)).await;
    let outgoing = read.map_err(|_| Refusal::new(Reason::Failed))??;
    let mut wait = Duration::from_millis(100);
    for _ in 0..FILE_BUSY_TRIES {
        match marmot.send_file(group, &outgoing, turn.as_ref()).await {
            Ok(_) => return Ok(()),
            Err(FileFailed::Busy) => {}
            Err(FileFailed::Failed) => {
                // MDK's own words can name a group, so they are not printed (D-071).
                eprintln!("marmot-agent: a file's send failed");
                return Err(Refusal {
                    reason: Reason::Failed,
                    detail: Some("It could not be uploaded or sent.".into()),
                });
            }
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(10));
    }
    Err(Refusal {
        reason: Reason::Busy,
        detail: Some("The agent's account stayed busy. Try again later.".into()),
    })
}

/// Serves connectors on the socket, one at a time, until `stop` resolves. Each connector that
/// gives a good hello drives the agent until it goes; then the core closes what it left open, as
/// for any driver that ends (D-058), and waits for the next. What arrives in between is stored by
/// MDK, and read when the next connector has come.
pub async fn serve(
    marmot: Marmot,
    config: &Config,
    store: &Path,
    listener: tokio::net::UnixListener,
    token: String,
    stop: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let (stopping, stopped) = tokio::sync::watch::channel(false);
    let agent = config.name.clone().unwrap_or_else(|| "agent".into());
    tokio::pin!(stop);
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(_) => continue,
            },
            () = &mut stop => return Ok(()),
        };
        let greeted = tokio::select! {
            greeted = SocketDriver::greet(stream, &token, &agent) => greeted,
            () = &mut stop => return Ok(()),
        };
        let driver = match greeted {
            Ok(driver) => driver,
            Err(why) => {
                eprintln!("marmot-agent: {why}");
                continue;
            }
        };
        let events = marmot.runtime.subscribe();
        let core = Core::new(marmot.clone(), driver, config, Store::new(store))?;
        let mut until = stopped.clone();
        let running = core.run(events, async move {
            let _ = until.wait_for(|stopping| *stopping).await;
        });
        tokio::pin!(running);
        tokio::select! {
            ran = &mut running => ran?,
            () = &mut stop => {
                let _ = stopping.send(true);
                return running.await;
            }
        }
    }
}
