//! The core running: MDK on one side, a driver on the other, and the conversation between them.
//!
//! One task does everything, so the conversation is never touched from two places. It waits for
//! whichever comes first, a group message or a driver event, hands it to the conversation, and
//! carries out the acts that come back one at a time, telling the conversation how each went. A
//! driver request is always awaited to the end (D-066); only the wait for the driver's next event
//! is dropped when a group message comes first, which the trait allows (D-058). The one piece of
//! work done on the side is a file the agent sends (D-090): uploading it can take long, so it runs
//! as a task of its own, a few at once, and how it went comes back to this one, which tells the
//! driver. The core owns those tasks, and none outlives it (D-145).
//!
//! What the core keeps across a restart is one file: the conversation's state, what it means to
//! send, and for each group how far it has read, in the order messages arrived on this machine.
//! It is written before anything leaves the core: a message is marked read before it reaches the
//! driver, so a crash never hands the driver the same prompt twice (D-037); a request is pending
//! on disk before the driver gets it, so a crash leaves it to be settled (D-070); and a version
//! of an item is queued on disk, with what it settles and the token MDK keeps it under, before it
//! is sent, so a crash leaves it to be sent again as it was, and MDK, which knows the token,
//! sends nothing twice (S1, S3). When the file cannot be written the core stops where it is,
//! as a crash would, and the next start takes up what is on disk. A group is only ever read from
//! its mark, page by page, in the order MDK stored its messages, a few in each turn of the loop,
//! and its mark moves as each is handled. MDK's live notice of a message only wakes its group's reader: MDK may announce a
//! message again, or one before another stored earlier, or store one without a notice, so a
//! notice never moves a mark (MDK #3603). MDK does not replay what arrived while a program was
//! down, so on start, whenever the core falls behind, and on the core's tick, every group is read
//! from its mark. A message MDK cannot read for a moment is not taken as read: its group waits,
//! and is read again from its mark, a little later each time, while the other groups go on (S10,
//! L3).

use crate::chat_folder::FolderRule;
use crate::config::Config;
use crate::conversation::{Act, Conversation, Saved, Sending};
use crate::files::Folders;
use crate::keys::{EventKey, GroupKey};
use crate::marmot::{
    FileFailed, Incoming, Live, Marmot, Message, NotNow, Outcome, SendFailed, Side, Subscribed,
};
use crate::socket::SocketDriver;
use marmot_agent_driver::{Driver, Output, Reason, Refusal, SendFile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::task::Poll;
use std::time::Duration;
use tokio::time::Instant;

/// The longest a group's sends wait before the next try, after failing again and again.
const SEND_WAIT_MAX: Duration = Duration::from_secs(10);

/// The longest a group waits before it asks again after a send MDK has queued.
const QUEUED_WAIT_MAX: Duration = Duration::from_millis(500);

/// How many files the agent sends are read and uploaded at once (D-145).
const FILES_AT_ONCE: usize = 2;

/// How many files the agent sends the core holds at once, those uploading included: one more is
/// refused as busy (D-145).
const FILES_HELD: usize = 16;

/// How long a stop waits, in all, for MDK to take into its engine the sends it has queued.
const STOP_WAIT: Duration = Duration::from_secs(60);

/// How many tries of one send MDK may take and drop before it went out, before the core gives
/// it up as refused. Each such try sent nothing, so the next is made under a new token.
pub const REJECTED_TRIES: u32 = 10;

/// Why the core does not start on a state file an older core left with sends it handed to MDK
/// before MDK kept tokens: sent again under a token, any of them could go out twice. No such core
/// was released.
const TRIED_WITHOUT_TOKENS: &str = "the state file was left by an older marmot-agent with sends \
    that may have gone out; move it away to start afresh";

/// Why the core does not start on a state file whose outbox has an item handed to MDK by a core
/// from before kind 1201: MDK keeps that try's token bound to the event in the old shape, and
/// refuses it in the new one, though it may have gone out. No such core was released.
const TRIED_BEFORE_KIND_1201: &str = "the state file was left by an older marmot-agent, from \
    before kind 1201, with an agent item that may have gone out; move it away to start afresh";

/// How many stored rows the core reads from MDK at a time.
const PAGE: usize = 100;

/// How many stored messages one group hands on in one turn of the core's loop, so a long
/// catch-up, or a harness slow to take prompts, does not hold up a stop or the other chats.
const READS: usize = 8;

/// How many sends one group makes in one turn of the core's loop, so one chat's backlog, or a slow
/// MDK, does not hold up the others.
const DRAIN: usize = 8;

/// When to read again what MDK could not read for a moment, and how long that waited.
struct Again {
    /// When it is read again; `None` while it is being read.
    at: Option<Instant>,
    /// How long it waited last.
    wait: Duration,
}

impl Again {
    /// The wait after `last`: 100 ms the first time, twice as long each time after, up to
    /// [`SEND_WAIT_MAX`]. `None` while `last` still waits, whose time stands.
    fn after(last: Option<&Again>) -> Option<Again> {
        let wait = match last {
            Some(Again { at: Some(_), .. }) => return None,
            Some(last) => (last.wait * 2).min(SEND_WAIT_MAX),
            None => Duration::from_millis(100),
        };
        let at = Some(Instant::now() + wait);
        Some(Again { at, wait })
    }
}

/// The state file could not be written: the core stops where it is, as a crash would.
#[derive(Debug)]
struct Unsaved;

/// What the core says when it stops because it could not save.
const UNSAVED: &str = "the state file could not be written, so the core stopped";

/// How long the core waits to try publishing the agent's name again after a try that failed, at
/// first and at most: twice as long after each.
const NAME_WAIT: Duration = Duration::from_secs(10);
const NAME_WAIT_MAX: Duration = Duration::from_secs(600);

/// How many times a file is offered to MDK while MDK says it is busy, about half a minute in all.
const FILE_BUSY_TRIES: u32 = 10;

/// How a file the agent asked to send went, for the driver.
struct Uploaded {
    id: String,
    chat: marmot_agent_driver::ChatId,
    result: Result<(), Refusal>,
}

/// A file the agent asked to send, waiting for its turn to upload. Where it goes, the chat's
/// group and running turn, and the folders it may come from, were taken when it was asked for
/// (D-090).
struct Waiting {
    id: String,
    chat: marmot_agent_driver::ChatId,
    path: PathBuf,
    group: GroupKey,
    turn: Option<marmot_agent_events::TurnId>,
    folders: Folders,
}

/// The files the agent asked to send that the core holds: at most [`FILES_AT_ONCE`] uploading,
/// each a task the core owns, and the rest waiting, [`FILES_HELD`] in all (D-145). A waiting file
/// is only its path until its turn comes, so the bytes held are those of the files uploading.
struct Files {
    uploading: tokio::task::JoinSet<Result<(), Refusal>>,
    /// Which file each upload is.
    running: HashMap<tokio::task::Id, (String, marmot_agent_driver::ChatId)>,
    waiting: VecDeque<Waiting>,
    /// One for each file that may be read at once. A read holds one until it has returned, even
    /// after its upload was stopped, since a read on its own thread cannot be stopped.
    reads: Arc<tokio::sync::Semaphore>,
    /// Whether the core takes them all back before it ends, so no read outlives it: it does
    /// unless it shares them with the cores before and after it, as [`serve`]'s do, and then
    /// whoever made them does.
    owns_reads: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            uploading: tokio::task::JoinSet::new(),
            running: HashMap::new(),
            waiting: VecDeque::new(),
            reads: Arc::new(tokio::sync::Semaphore::new(FILES_AT_ONCE)),
            owns_reads: true,
        }
    }
}

impl Files {
    /// How many files are held, uploading or waiting.
    fn held(&self) -> usize {
        self.running.len() + self.waiting.len()
    }

    /// How an upload went, once its task has ended: a task that did not finish, as when it
    /// panicked or was stopped, failed with `detail`.
    fn ended(&mut self, ended: Ended, detail: &str) -> Option<Uploaded> {
        let (task, result) = match ended {
            Ok((task, result)) => (task, result),
            Err(error) => {
                let failed = Refusal {
                    reason: Reason::Failed,
                    detail: Some(detail.into()),
                    unanswered: false,
                };
                (error.id(), Err(failed))
            }
        };
        let (id, chat) = self.running.remove(&task)?;
        Some(Uploaded { id, chat, result })
    }

    /// Stops every upload, waits for each to end, and, if the core owns the reads, for every
    /// read they started. It saves nothing, so it ends the files whether or not a save can be
    /// made. Gives how each upload went, and whether it was stopped, for the driver and the chat.
    async fn stop(&mut self) -> Vec<(Uploaded, bool)> {
        self.uploading.abort_all();
        let mut ended = Vec::new();
        while let Some(done) = self.uploading.join_next_with_id().await {
            let stopped = matches!(&done, Err(error) if error.is_cancelled());
            let detail = if stopped {
                "The agent stopped while the file was being sent."
            } else {
                "It could not be uploaded or sent."
            };
            if let Some(done) = self.ended(done, detail) {
                ended.push((done, stopped));
            }
        }
        // A read of a file whose upload was stopped goes on on its own thread until it returns:
        // it is waited for, so none goes on after the core. Its file reaches no MDK, since the
        // task that would have sent it is gone.
        if self.owns_reads {
            drop(self.reads.acquire_many(FILES_AT_ONCE as u32).await);
        }
        ended
    }
}

/// How an upload's task ended.
type Ended = Result<(tokio::task::Id, Result<(), Refusal>), tokio::task::JoinError>;

/// What the core keeps across a restart.
#[derive(Default, Serialize, Deserialize)]
pub struct Kept {
    /// The conversation's state.
    pub conversation: Saved,
    /// For each group, the place in storage order of the last message read.
    pub read: BTreeMap<GroupKey, i64>,
    /// For each group the core had no mark for when it first read it, the second it had
    /// subscribed in then: what arrived by then is the group's history (D-111). Saved with the
    /// group's first mark, so it holds after a restart too.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub history: BTreeMap<GroupKey, u64>,
}

/// Which save fails: one that would write a state it matches.
type FailWhen = dyn Fn(&Kept) -> bool + Send + Sync;

/// How many times a [`Store`] that counts them wrote its file, and how many bytes in all,
/// counted once each write is synced: what a save costs is measured by them (#155). Only the
/// store adds to them.
#[derive(Clone, Debug, Default)]
pub struct Writes(Arc<(AtomicU64, AtomicU64)>);

impl Writes {
    /// The writes so far.
    pub fn count(&self) -> u64 {
        self.0.0.load(SeqCst)
    }

    /// The bytes they wrote.
    pub fn bytes(&self) -> u64 {
        self.0.1.load(SeqCst)
    }

    fn add(&self, bytes: usize) {
        self.0.0.fetch_add(1, SeqCst);
        self.0.1.fetch_add(bytes as u64, SeqCst);
    }
}

/// Where the kept state lives.
pub struct Store {
    path: PathBuf,
    /// The bytes on disk, as the last save wrote them, so a save of the same state writes
    /// nothing (#155). `None` while the store does not know: before its first save, and after
    /// one that failed partway.
    written: std::sync::Mutex<Option<Vec<u8>>>,
    /// What it wrote, if it counts it.
    writes: Option<Writes>,
    /// A fault for the tests of a failed save, as the simulator will put one
    /// (`docs/design/verification.md`): none in use.
    fault: Option<(Arc<FailWhen>, Arc<AtomicBool>)>,
    /// A fault for the tests at the last step of a save, the folder's sync, once the new file is
    /// in place.
    #[cfg(test)]
    folder_sync_fails: AtomicBool,
}

impl Store {
    /// The state file in `dir`.
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("marmot-agent-state.json"),
            written: std::sync::Mutex::new(None),
            writes: None,
            fault: None,
            #[cfg(test)]
            folder_sync_fails: AtomicBool::new(false),
        }
    }

    /// This store, counting how many times it writes its file and how many bytes, which the
    /// count given back shows.
    pub fn counting_writes(mut self) -> (Self, Writes) {
        let writes = Writes::default();
        self.writes = Some(writes.clone());
        (self, writes)
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
    /// A state the same as the last one this store wrote is on disk already, so nothing is
    /// written: what is durable is the same either way (#155).
    pub fn save(&self, kept: &Kept) -> std::io::Result<()> {
        // A fault meets every save, written or not, so a crash can be put at any of them.
        if let Some((when, failed)) = &self.fault
            && (failed.load(SeqCst) || when(kept))
        {
            failed.store(true, SeqCst);
            return Err(std::io::Error::other("a save made to fail"));
        }
        let bytes = serde_json::to_vec(kept).map_err(std::io::Error::other)?;
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        if written.as_ref() == Some(&bytes) {
            return Ok(());
        }
        // Until this save has ended well, what is on disk may be the old state or the new one.
        *written = None;
        let temporary = self.path.with_extension("json.new");
        let mut file = private_file(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &self.path)?;
        if let Some(dir) = self.path.parent() {
            #[cfg(test)]
            if self.folder_sync_fails.load(SeqCst) {
                return Err(std::io::Error::other("the folder's sync made to fail"));
            }
            std::fs::File::open(dir)?.sync_all()?;
        }
        if let Some(writes) = &self.writes {
            writes.add(bytes.len());
        }
        *written = Some(bytes);
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
    /// A file's upload ended.
    Uploaded(Ended),
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
    /// The files the agent asked to send that are uploading or waiting to (D-145).
    files: Files,
    /// The core's clock, which never jumps, for how long a control has waited (D-087). Tokio's,
    /// which is the system's except in a simulation, whose clock is virtual.
    started: Instant,
    /// When this core subscribed to MDK's live events ([`Subscribed`]), by the machine's clock,
    /// in whole seconds as MDK gives the time a message arrived; zero until it runs.
    since: u64,
    /// For each group the core had no read mark for when it first read it, as on an imported
    /// account's first run or after a lost state file, the second the core had subscribed in
    /// then: what arrived in it by that second is its history, marked read and never handed on,
    /// in this run and every one after (L3, D-111). Kept with the marks.
    history: BTreeMap<GroupKey, u64>,
    /// The groups with more stored after their marks than one turn of the core's loop reads:
    /// each is read on from its mark in the next turn, so a long catch-up holds up nothing else.
    behind: BTreeSet<GroupKey>,
    /// For each group whose last send failed: when to try again, and how long it waited last.
    retry: HashMap<GroupKey, (Instant, Duration)>,
    /// The groups whose next send MDK has queued, not yet taken into its engine.
    queued: HashSet<GroupKey>,
    /// For each group MDK could not read for a moment: when to read it again from its mark.
    /// Nothing more of it is read until then, so what comes after the message that could not be
    /// read never goes ahead of it. In order, so groups due at the same moment are read again in
    /// the same order every time.
    unread: BTreeMap<GroupKey, Again>,
    /// When to list the groups again, for a catch-up that MDK could not list them for.
    listing: Option<Again>,
    /// How many tries of a send MDK may drop before it is given up: [`REJECTED_TRIES`].
    rejected_tries: u32,
    /// How many sends each group made in this turn of the loop, at most [`DRAIN`].
    drained: HashMap<GroupKey, usize>,
    /// Whether [`DRAIN`] holds: not when the core stops, which sends all it can.
    bounded: bool,
    /// Which of the things the loop waits for is looked at first in its next turn: each in its
    /// turn, so when several are ready at once none waits behind the others for long, and the
    /// same inputs always give the same order, which a simulation replays.
    first: usize,
    /// What the driver gave while a catch-up, or the `no_effect` net, looked whether it had
    /// anything ready ([`Core::driver_ready`]): one output at most, with when it reached the
    /// driver on the core's clock, which the loop takes in the driver's turn.
    early: Option<(Output, u64)>,
    /// Whether the driver has ended: the loop ends, and nothing more is read for it.
    ended: bool,
    /// Faults put back for the simulator, to show its checks find them: none in use.
    #[cfg(feature = "sim-bugs")]
    bugs: Bugs,
}

/// Bugs fixed before, put back on purpose so the simulator can show it finds each
/// (`docs/design/verification.md`, "A check must be shown to catch its bug"). Only in a build
/// with the `sim-bugs` feature, which no release has. The core's hooks for them are named `bug_…`
/// (and `with_bugs`): the nightly mutation run leaves them out by that name, since only the
/// simulator exercises them.
#[cfg(feature = "sim-bugs")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bugs {
    /// A send is saved only after it went, not before: a crash in between forgets it (S2, S3).
    pub save_after_send: bool,
    /// A send whose outcome is not known is tried again under a new token, as if MDK had dropped
    /// it: MDK cannot tell it is the send it already has (S1, S3).
    pub retry_ambiguous_with_new_token: bool,
    /// What the agent's words settle is ignored: settles are read only from items (S1).
    pub settles_only_from_items: bool,
    /// A live message in a group with a mark is handled itself, and moves the mark to its place,
    /// as before MDK #3606: one announced before a message stored earlier passes it (S10, L3).
    pub mark_from_live: bool,
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
        if kept.conversation.tried_without_tokens() {
            return Err(TRIED_WITHOUT_TOKENS.into());
        }
        if kept.conversation.tried_before_kind_1201() {
            return Err(TRIED_BEFORE_KIND_1201.into());
        }
        let mut conversation = Conversation::new(
            marmot.agent().clone(),
            driver.harness().name,
            config.rights(),
            driver.capabilities(),
            kept.conversation,
        )
        .with_activity(config.full_activity.clone());
        // A driver that works in a folder of its own lets each chat move it (D-101).
        let folder = driver.folder();
        if let Some(folder) = &folder {
            let rule = FolderRule::new(folder, config.folders.as_deref(), &config.home);
            conversation = conversation.with_folders(rule);
        }
        // Files come from the folders listed, or else the harness's working folder, which a
        // connector states in its hello (D-090).
        let folders = if config.files_from.is_none() && config.file_folders().is_empty() {
            folders.others(folder.as_slice())
        } else {
            folders
        };
        Ok(Self {
            marmot,
            driver,
            conversation,
            read: kept.read,
            history: kept.history,
            store,
            folders,
            files: Files::default(),
            config: config.clone(),
            started: Instant::now(),
            since: 0,
            behind: BTreeSet::new(),
            retry: HashMap::new(),
            queued: HashSet::new(),
            unread: BTreeMap::new(),
            listing: None,
            rejected_tries: REJECTED_TRIES,
            drained: HashMap::new(),
            bounded: true,
            first: 0,
            early: None,
            ended: false,
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

    /// Shares `reads`, the permits for the files read at once, with the cores before and after
    /// it (D-145).
    /// The core does not wait for a read left going as it ends, so a read stalled on a slow
    /// folder holds up no next connector; it still holds its permit until it returns.
    fn sharing_reads(mut self, reads: Arc<tokio::sync::Semaphore>) -> Self {
        self.files.reads = reads;
        self.files.owns_reads = false;
        self
    }

    /// A send is given up after MDK dropped `tries` of its tries: for the tests.
    pub fn rejected_sends(mut self, tries: u32) -> Self {
        self.rejected_tries = tries;
        self
    }

    /// Runs until the driver ends or `stop` resolves. MDK's runtime must already have been
    /// subscribed to (`live`, as [`crate::marmot::subscribe`] does) and started, so nothing that
    /// arrives during start-up is missed; and subscribed to before the harness started, since
    /// what a group with no read mark received by then is its history. An error when the state
    /// file could not be written: the core stopped where it was.
    pub async fn run(
        mut self,
        live: Subscribed<M::Events>,
        stop: impl Future<Output = ()>,
    ) -> Result<(), String> {
        self.since = live.since;
        self.run_until(live.events, stop).await.map_err(|Unsaved| {
            eprintln!("marmot-agent: {UNSAVED}");
            UNSAVED.to_string()
        })
    }

    async fn run_until(
        &mut self,
        events: M::Events,
        stop: impl Future<Output = ()>,
    ) -> Result<(), Unsaved> {
        // The agent's name goes out beside everything else, as a task the core ends when it stops.
        let _naming = (self.config.name.clone()).map(|name| {
            let (marmot, relays) = (self.marmot.clone(), self.config.relays.clone());
            Ending(tokio::spawn(publish_name(marmot, name, relays)))
        });
        if let Err(unsaved) = self.run_loop(events, stop).await {
            // The core stops where it was, but its files end first: no upload or read it started
            // goes on after it, whether or not what they did can be saved (D-145).
            self.files.stop().await;
            return Err(unsaved);
        }
        // What the core took from the driver to look at is read before the stop, so a file it asks
        // to send is answered with the others (D-145). A save that fails meanwhile ends the files
        // as the loop's does.
        if let Some((output, _)) = self.early.take()
            && let Err(unsaved) = self.take(output).await
        {
            self.files.stop().await;
            return Err(unsaved);
        }
        self.end_files().await?;
        // Everything the stop closes is queued and saved at once, before any of it is sent: a
        // crash in between sends what is left when the core starts again, and closes nothing
        // twice.
        let ended = self.conversation.ended();
        self.retry.clear();
        self.bounded = false;
        self.carry_out(ended).await?;
        self.flush_all().await?;
        // What MDK has queued is asked after until its engine takes it, for a while, and what
        // comes after it is sent then. What MDK is busy for waits for the next start.
        let stopped = Instant::now();
        while !self.queued.is_empty() && stopped.elapsed() < STOP_WAIT {
            let next = (self.queued.iter())
                .filter_map(|g| self.retry.get(g).map(|(at, _)| *at))
                .min();
            tokio::time::sleep_until(next.unwrap_or_else(Instant::now)).await;
            for group in self.queued.clone() {
                self.flush(&group).await?;
            }
        }
        Ok(())
    }

    /// Runs until the driver ends or `stop` resolves, or a save fails.
    async fn run_loop(
        &mut self,
        mut events: M::Events,
        stop: impl Future<Output = ()>,
    ) -> Result<(), Unsaved> {
        // What a stopped core left queued goes first, then what it left open is closed.
        self.flush_all().await?;
        let recovered = self.conversation.recover();
        self.carry_out(recovered).await?;
        self.consider_invitations().await?;
        self.catch_up().await?;
        tokio::pin!(stop);
        let mut clock = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            // Each group may send a few more events in this turn.
            self.drained.clear();
            let sends = self.retry.values().map(|(at, _)| *at);
            let reads = (self.unread.values().chain(&self.listing)).filter_map(|a| a.at);
            // A group with more to read is read on in this turn, after other work.
            let behind = (!self.behind.is_empty()).then(Instant::now);
            let retry = sends.chain(reads).chain(behind).min();
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
            // The files waiting go once the driver waits for more.
            self.start_uploads().await;
            // A driver a catch-up, or the files, found ended is handed nothing more, once what it
            // gave before its end is read.
            if self.ended && self.early.is_none() {
                break;
            }
            let woke = {
                let mut tick = pin!(clock.tick());
                let mut retry = pin!(retry);
                // What the driver gave while a catch-up looked comes first in the driver's turn.
                let (early, driver) = (&mut self.early, &mut self.driver);
                let mut driver = pin!(async move {
                    match early.take() {
                        Some((output, _)) => Some(output),
                        None => driver.next_event().await,
                    }
                });
                let uploads = &mut self.files.uploading;
                let mut live = pin!(self.marmot.next(&mut events));
                let mut stop = stop.as_mut();
                std::future::poll_fn(|cx| {
                    for wait in (0..WAITS).map(|i| (first + i) % WAITS) {
                        let woke = match wait {
                            0 => tick.as_mut().poll(cx).map(|_| Woke::Tick),
                            1 => retry.as_mut().poll(cx).map(|()| Woke::Retry),
                            2 => driver.as_mut().poll(cx).map(Woke::Driver),
                            // With no upload under way, nothing to wait for here.
                            3 => match uploads.poll_join_next_with_id(cx) {
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
                Woke::Tick => {
                    self.settle_overdue().await?;
                    // MDK can store a message with no notice, as one imported: every group is
                    // read from its mark now and then, unless a catch-up waits to list them.
                    if self.listing.is_none() {
                        self.catch_up().await?;
                    }
                }
                Woke::Retry => {
                    self.flush_all().await?;
                    self.read_again().await?;
                }
                Woke::Driver(Some(output)) => {
                    self.take(output).await?;
                    self.settle_overdue().await?;
                }
                Woke::Driver(None) => break,
                Woke::Uploaded(ended) => {
                    let failed = "It could not be uploaded or sent.";
                    if let Some(done) = self.files.ended(ended, failed) {
                        self.file_done(done).await?;
                    }
                }
                Woke::Marmot(Live::Message(message)) => self.live(message).await?,
                Woke::Marmot(Live::Groups) => self.consider_invitations().await?,
                Woke::Marmot(Live::Other) => {}
                Woke::Marmot(Live::Lagged) => self.catch_up().await?,
                Woke::Marmot(Live::Closed) | Woke::Stop => break,
            }
        }
        Ok(())
    }

    /// Takes a file the agent asked to send. Where it goes, the chat's group and running turn,
    /// is taken now (D-090). It uploads as a task of its own, so the chats go on meanwhile, once
    /// fewer than [`FILES_AT_ONCE`] are uploading; with [`FILES_HELD`] already held, it is refused
    /// as busy (D-145).
    async fn send_file(&mut self, file: SendFile) -> Result<(), Unsaved> {
        let SendFile { id, chat, path } = file;
        if self.files.held() >= FILES_HELD {
            let result = Err(Refusal {
                reason: Reason::Busy,
                detail: Some(
                    "The agent is already sending as many files as it can. Try again once they \
                     are sent."
                        .into(),
                ),
                unanswered: false,
            });
            return self.file_done(Uploaded { id, chat, result }).await;
        }
        let Some((group, turn)) = self.conversation.file_place(&chat) else {
            let result = Err(Refusal {
                reason: Reason::Invalid,
                detail: Some("The agent named a chat it is not in.".into()),
                unanswered: false,
            });
            return self.file_done(Uploaded { id, chat, result }).await;
        };
        let folders = self.folders_for(&chat);
        self.files.waiting.push_back(Waiting {
            id,
            chat,
            path,
            group,
            turn,
            folders,
        });
        Ok(())
    }

    /// Starts the uploads of the files waiting, oldest first, while fewer than [`FILES_AT_ONCE`]
    /// are under way, unless the driver has ended or has something for the loop: a driver that
    /// ended, as a connector the core gave up, may still hand over what it read before, a file to
    /// send among it, and how a file went may be what ends it. So an upload starts only once the
    /// driver is waiting for more, and a file the driver asks for after its end is never offered
    /// to MDK (D-145).
    async fn start_uploads(&mut self) {
        if self.files.waiting.is_empty() || self.files.running.len() >= FILES_AT_ONCE {
            return;
        }
        if self.driver_ready().await {
            return;
        }
        while self.files.running.len() < FILES_AT_ONCE {
            let Some(file) = self.files.waiting.pop_front() else {
                return;
            };
            let (marmot, reads) = (self.marmot.clone(), self.files.reads.clone());
            let Waiting {
                id,
                chat,
                path,
                group,
                turn,
                folders,
            } = file;
            let task = (self.files.uploading)
                .spawn(async move { upload(marmot, reads, folders, path, &group, turn).await });
            self.files.running.insert(task.id(), (id, chat));
        }
    }

    /// Ends the files the core holds as it stops, or as its driver ends, so no upload outlives
    /// it (D-145). One uploading is stopped, and the driver hears it failed; MDK may have taken it
    /// already, so the chat is told nothing it may not be true of. One that ended meanwhile is told
    /// as it went. One waiting was never offered to MDK: the chat is told it was not sent.
    /// Every upload and read has ended before any of it is told, so a save that fails while it
    /// is told leaves none going. After a save that fails, the chat is told nothing more, but the
    /// driver still hears of every file, and then the save's error is the core's.
    async fn end_files(&mut self) -> Result<(), Unsaved> {
        let mut unsaved = None;
        let mut ended = Vec::new();
        for (done, stopped) in self.files.stop().await {
            ended.push((done, !stopped));
        }
        for file in std::mem::take(&mut self.files.waiting) {
            let result = Err(Refusal {
                reason: Reason::Failed,
                detail: Some("The agent stopped before it was sent.".into()),
                unanswered: false,
            });
            let (id, chat) = (file.id, file.chat);
            ended.push((Uploaded { id, chat, result }, true));
        }
        for (done, tell_chat) in ended {
            if tell_chat && unsaved.is_none() {
                if let Err(error) = self.file_done(done).await {
                    unsaved = Some(error);
                }
            } else {
                self.driver.file_sent(done.id, done.result).await;
            }
        }
        unsaved.map_or(Ok(()), Err)
    }

    /// The folders a file from `chat` may come from (D-090): those the settings list, or else the
    /// chat's folder, the one it chose or the harness's own (D-101). A file inside the account
    /// home is refused from a chat's folder as from any other, unless the owner allows it (D-099).
    fn folders_for(&self, chat: &marmot_agent_driver::ChatId) -> Folders {
        if self.config.files_from.is_some() {
            return self.folders.clone();
        }
        match self.conversation.chosen_folder(chat) {
            Some(chosen) => self.folders.others(std::slice::from_ref(&chosen)),
            None => self.folders.clone(),
        }
    }

    /// Tells the driver how a file went. A refusal is shown in the chat, naming no path (D-090).
    /// The driver hears of it even when the notice cannot be saved.
    async fn file_done(&mut self, done: Uploaded) -> Result<(), Unsaved> {
        let mut saved = Ok(());
        if let Err(refusal) = &done.result {
            // Its kind only: a path or a file name is never logged (D-071).
            eprintln!("marmot-agent: a file was not sent ({:?})", refusal.reason);
            let acts = self.conversation.file_refused(&done.chat, refusal);
            saved = self.carry_out(acts).await;
        }
        self.driver.file_sent(done.id, done.result).await;
        saved
    }

    /// Seconds on the core's clock.
    fn clock(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Accepts an invitation from someone who may add the agent, and declines the rest. A group
    /// the agent has just joined is read from its start: the agent cannot read anything sent
    /// before it joined, so everything stored is for it. The mark that says so is saved before
    /// the agent accepts, so a crash in between never leaves a group accepted with no mark, which
    /// would take what arrived meanwhile as history.
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
                self.history.remove(group);
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
    /// while it fell behind, or with no notice. A group with no mark starts where
    /// [`Core::start_of`] says. A group waiting to be read again is left to its time. When MDK
    /// cannot list the groups, the whole catch-up is tried again later: a failed list is not an
    /// account with no groups.
    async fn catch_up(&mut self) -> Result<(), Unsaved> {
        let groups = match self.marmot.groups() {
            Ok(groups) => groups,
            Err(NotNow) => {
                if self.listing.is_none() {
                    eprintln!("marmot-agent: the groups could not be listed; they will be again");
                }
                if let Some(again) = Again::after(self.listing.as_ref()) {
                    self.listing = Some(again);
                }
                return Ok(());
            }
        };
        self.listing = None;
        for group in groups {
            if !self.waits(&group) {
                self.catch_up_group(group).await?;
            }
        }
        Ok(())
    }

    /// Whether a group waits to be read again.
    fn waits(&self, group: &GroupKey) -> bool {
        self.unread.get(group).is_some_and(|a| a.at.is_some())
    }

    /// Reads again what is due: the catch-up MDK could not list the groups for, each group with
    /// more to read, then each group MDK could not read, from its mark, in order.
    async fn read_again(&mut self) -> Result<(), Unsaved> {
        let now = Instant::now();
        let due = |again: &Again| again.at.is_some_and(|at| at <= now);
        if let Some(listing) = self.listing.as_mut().filter(|a| due(a)) {
            listing.at = None;
            self.catch_up().await?;
        }
        let groups: Vec<GroupKey> = (self.unread.iter())
            .filter(|(_, again)| due(again))
            .map(|(group, _)| group.clone())
            .collect();
        // A group that waits is read at its time, as one of `groups` if that is now.
        for group in std::mem::take(&mut self.behind) {
            if !self.waits(&group) {
                self.catch_up_group(group).await?;
            }
        }
        for group in groups {
            if let Some(again) = self.unread.get_mut(&group) {
                again.at = None;
            }
            self.catch_up_group(group).await?;
        }
        Ok(())
    }

    /// Reads on in one group from its mark, in storage order: one page of what was stored after
    /// it, of which at most [`READS`] messages are handed on, each once, the mark moving past
    /// each, and past what the core does not read, such as the agent's own messages. When more
    /// is left, the group is read on from its mark in the next turn of the core's loop
    /// ([`Core::behind`]). When MDK cannot be read, or a message in it cannot, the group waits to
    /// be read again from its mark; when it was read, it waits no more. A group with no mark
    /// starts where [`Core::start_of`] says. When the driver has given something meanwhile, the
    /// group is read on from its mark after the loop has taken it in its turn, so neither waits
    /// behind the other for long; once the driver has ended, nothing more is read, and the next
    /// driver reads on from the mark (D-139).
    async fn catch_up_group(&mut self, group: GroupKey) -> Result<(), Unsaved> {
        self.behind.remove(&group);
        let mark = match self.read.get(&group) {
            Some(&mark) => mark,
            None => match self.start_of(&group)? {
                Some(start) => start,
                None => return Ok(()),
            },
        };
        let Ok(page) = self.marmot.stored(&group, mark, PAGE) else {
            self.read_later(&group);
            return Ok(());
        };
        let Some(last) = page.last else {
            self.read_through(&group);
            return Ok(());
        };
        for (handed, message) in page.messages.into_iter().enumerate() {
            // A driver that has ended is handed nothing more, and no mark moves past what it was
            // never given: the core ends, and the next driver is given it, once (D-139). What it
            // gave meanwhile is taken in its turn of the loop, and this group's turn comes after.
            if handed == READS || self.driver_ready().await {
                // The rest of the page is read again from the mark next turn.
                self.read_through(&group);
                self.behind.insert(group);
                return Ok(());
            }
            self.handle(message).await?;
            if self.waits(&group) {
                return Ok(());
            }
        }
        // Past the rows the core does not read too, so the next page starts after them. Not
        // saved now: read again after a crash, they are passed again.
        let mark = last.max(self.read.get(&group).copied().unwrap_or(last));
        self.read.insert(group.clone(), mark);
        self.read_through(&group);
        if page.more {
            self.behind.insert(group);
        }
        Ok(())
    }

    /// Whether the driver has something for the loop, an output or its end, looked at without
    /// waiting: its next event is asked for and dropped as soon as it is not ready, which the
    /// trait allows (D-058). One output given meanwhile is kept for the loop, which takes it in
    /// the driver's turn, and no more is asked for until it has. A connector whose connection
    /// went, or that the core gave up, has ended once its socket says so. Tokio's budget for a
    /// task, which can make a channel or a socket with something ready look empty once the core
    /// has read a lot in one go, is set aside for the look: the `no_effect` net takes "nothing
    /// ready" to mean that nothing more has reached the driver.
    async fn driver_ready(&mut self) -> bool {
        if self.ended || self.early.is_some() {
            return true;
        }
        let polled = {
            let mut next = pin!(tokio::task::unconstrained(self.driver.next_event()));
            std::future::poll_fn(|cx| Poll::Ready(next.as_mut().poll(cx))).await
        };
        match polled {
            Poll::Ready(Some(output)) => self.early = Some((output, self.arrival())),
            Poll::Ready(None) => self.ended = true,
            Poll::Pending => return false,
        }
        true
    }

    /// When the output the driver handed over last reached it, in seconds on the core's clock:
    /// when the driver says, or else now ([`Driver::arrived`]).
    fn arrival(&self) -> u64 {
        match self.driver.arrived() {
            Some(at) => Instant::from_std(at)
                .saturating_duration_since(self.started)
                .as_secs(),
            None => self.clock(),
        }
    }

    /// An output the driver handed over: an event, or a file to send.
    async fn take(&mut self, output: Output) -> Result<(), Unsaved> {
        match output {
            Output::Event(emitted) => {
                let acts = self.conversation.emitted(emitted);
                self.carry_out(acts).await
            }
            Output::SendFile(file) => self.send_file(file).await,
        }
    }

    /// The `no_effect` net (D-087): settles each control the driver took whose effect has not
    /// shown by its deadline, once the core has read every output that reached the driver by
    /// then. The core may have been busy past a deadline, as when it waited on the driver to take
    /// a slow request, while the effect was handed over in time: settled first, that would be
    /// untrue (S1). The driver hands its outputs over in the order they reached it, so the next
    /// one, which the core takes to look at if it is ready, is the oldest not read: a control
    /// whose deadline came before it reached the driver is settled now, and the rest wait while
    /// the core reads on, one output at a time in the driver's turn, looking again after each.
    /// What reaches the driver after a deadline does not hold it up, so a driver that is never
    /// idle holds up no deadline (L1).
    async fn settle_overdue(&mut self) -> Result<(), Unsaved> {
        let now = self.clock();
        if !self.conversation.is_overdue(now) {
            return Ok(());
        }
        self.driver_ready().await;
        let by = match &self.early {
            Some((_, arrived)) => now.min(arrived.saturating_sub(1)),
            None => now,
        };
        loop {
            let acts = self.conversation.overdue(by);
            if acts.is_empty() {
                return Ok(());
            }
            self.carry_out(acts).await?;
        }
    }

    /// Where a group with no mark starts, saved with the mark: what arrived by the time the core
    /// subscribed is its history, marked read, and the group starts just before the first
    /// message that arrived after, so neither is its history read as prompts nor is anything new
    /// skipped (L3). MDK gives that time in whole seconds, so a message that arrived in the second
    /// the core subscribed is history too. Which messages are history is never told by where
    /// MDK says the group ends, nor by which message is heard first: MDK stores a message some
    /// time after it arrived, and announces a sync's messages only once it has stored them all.
    /// `None` when MDK could not be read: the group waits to be read again.
    fn start_of(&mut self, group: &GroupKey) -> Result<Option<i64>, Unsaved> {
        let since = self.since;
        let (mut after, mut start) = (i64::MIN, None);
        loop {
            let Ok(page) = self.marmot.stored(group, after, PAGE) else {
                self.read_later(group);
                return Ok(None);
            };
            if let Some(first) = page.messages.iter().find(|m| m.received_at > since) {
                start = first.order.map(|order| order - 1);
                break;
            }
            let Some(last) = page.last else { break };
            (after, start) = (last, Some(last));
            if !page.more {
                break;
            }
        }
        let start = start.unwrap_or(0);
        // Where it starts is kept, and what is its history, so its first new message is read
        // there after a crash, and its history is never read as prompts.
        self.history.insert(group.clone(), since);
        self.read.insert(group.clone(), start);
        self.save()?;
        Ok(Some(start))
    }

    /// A group was read through: unless it waits again, it no longer waits at all.
    fn read_through(&mut self, group: &GroupKey) {
        if self.unread.get(group).is_some_and(|a| a.at.is_none()) {
            self.unread.remove(group);
        }
    }

    /// A message heard live wakes its group's reader, which reads the group from its mark: the
    /// notice moves no mark, so a message announced again, or before one stored earlier, is
    /// read in its place and once. A group with no mark is read from storage too, where its
    /// history is told apart from what is new whatever order MDK announces messages in
    /// ([`Core::start_of`]): a message that arrived by the time the core subscribed starts no
    /// read. A group that waits to be read again is read at its time.
    async fn live(&mut self, message: Message) -> Result<(), Unsaved> {
        #[cfg(feature = "sim-bugs")]
        if self.bug_marks_from_live(&message.group) {
            return self.bug_mark_from_live(message).await;
        }
        let group = message.group;
        if self.waits(&group) {
            return Ok(());
        }
        if !self.read.contains_key(&group) && message.received_at <= self.since {
            return Ok(());
        }
        self.catch_up_group(group).await
    }

    /// Hands one stored message of a group with a mark to the conversation, once. What the
    /// conversation makes of it, such as a request pending for the driver or a refusal queued, is
    /// saved in one write with the mark that says it was read, before anything reaches the
    /// driver: a crash before that write leaves the message to be read again, and one after it
    /// leaves the request to be settled (D-037, D-070). A mark that cannot be saved stops the
    /// core before the message reaches the driver. A message MDK cannot read for a moment is not
    /// marked read, and neither is anything after it in its group: the group is read again from
    /// its mark later (S10, L3). In a group the core had no mark for, a message that arrived by
    /// the time the core subscribed is history, even one stored after a message that arrived
    /// later: it is marked read and never handed on (L3).
    async fn handle(&mut self, message: Message) -> Result<(), Unsaved> {
        let group = message.group.clone();
        let (Some(&read), Some(order)) = (self.read.get(&group), message.order) else {
            return Ok(());
        };
        if order <= read {
            return Ok(());
        }
        if (self.history.get(&group)).is_some_and(|&since| message.received_at <= since) {
            self.read.insert(group, order);
            return self.save();
        }
        let acts = match self.marmot.incoming(message).await {
            Ok(Some(Incoming::Heard(heard))) => self.conversation.heard(heard),
            Ok(Some(Incoming::Asked(asked))) => self.conversation.asked(asked),
            Ok(None) => Vec::new(),
            Err(NotNow) => {
                self.read_later(&group);
                return Ok(());
            }
        };
        self.read.insert(group, order);
        // Saved before the first act, or at the end when there is none.
        self.carry_out(acts).await
    }

    /// MDK could not read the group for a moment: it is read again from its mark later, longer
    /// after each time it fails again, up to [`SEND_WAIT_MAX`]. It is never given up, since a
    /// message is never skipped; a group MDK no longer has is not read at all.
    fn read_later(&mut self, group: &GroupKey) {
        let last = self.unread.get(group);
        if last.is_none() {
            // Neither the group nor MDK's words are printed (D-071).
            eprintln!("marmot-agent: a message could not be read; it will be read again");
        }
        if let Some(again) = Again::after(last) {
            self.unread.insert(group.clone(), again);
        }
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
    /// gone, a few in each turn of the loop so other work and other groups go on between. Each
    /// send is handed to MDK under its token, which MDK keeps: a send tried again under it, after
    /// an answer that was lost or a restart, gives back the event it made, so nothing goes out
    /// twice (S3). A send MDK is busy for, or whose outcome is not known, stays queued, and the
    /// group waits, longer each time, before it is tried again under the same token; the other
    /// groups go on meanwhile. A send MDK took and dropped before it went out sent nothing, and
    /// is tried again under a new token, up to [`REJECTED_TRIES`] times. A send MDK refuses, or
    /// one given up after those tries, is handed to the conversation, which drops it with a
    /// notice, closing an item whose closing version it was and settling in its place what a
    /// notice settled, or sends an edit whose message is gone as a new message. A send is
    /// otherwise never given up while the core runs, so a later version never goes out as if it
    /// were new, and one still queued when the core stops goes out when it starts again (S1, S3).
    async fn flush(&mut self, group: &GroupKey) -> Result<(), Unsaved> {
        if self
            .retry
            .get(group)
            .is_some_and(|(at, _)| *at > Instant::now())
        {
            return Ok(());
        }
        loop {
            // Who is in the group is read before each send, each try again included: what the
            // group is sent follows who is in it now, not when the send was queued (D-136).
            match self.marmot.two_people(group) {
                Ok(two) => self.conversation.members_now(group, two.as_deref()),
                Err(NotNow) => {
                    self.wait_longer(group);
                    return Ok(());
                }
            }
            let Some((token, what)) = self.conversation.next_send(group) else {
                break;
            };
            let key = match &what {
                Sending::Item { key, .. } | Sending::Words { key, .. } => key.clone(),
            };
            // A send MDK was handed otherwise than the chat is sent now: the agent's full
            // activity, or an item with its native message or without (D-136). It is never
            // handed to MDK again: MDK takes a token it has let go of as a new send, and would
            // post it to the chat as it is now (#194). MDK is asked what became of it, and it
            // goes on as MDK has it. One MDK no longer knows may have gone out, and the chat is
            // told so (D-109, #182). One MDK never took is held back now if it is the full
            // activity, and else goes as the chat is sent now. Held back as if it had never gone,
            // it would settle its requests twice; tried again as the chat is sent now, MDK would
            // refuse its token as bound to another send (ActivityHoldBack.tla).
            if self.conversation.handed_otherwise(&token.send) {
                match self.marmot.outcome(group, &token) {
                    Ok(Outcome::Went(event)) => {
                        self.went(group, &key, event)?;
                        continue;
                    }
                    Ok(Outcome::Taken) => match self.conversation.event_of(&token.send) {
                        Some(event) => {
                            self.went(group, &key, event)?;
                            continue;
                        }
                        // MDK's answer when it took the send was lost, and MDK names no event
                        // while its engine has it. MDK still posts it, so it is never told as
                        // lost while MDK has it, which would settle its requests twice: it is
                        // asked after again until MDK names its event or lets go of its token,
                        // a little later each time. A stop does not wait for it: MDK's engine has
                        // it, and it goes out as it was handed, whatever the core does.
                        None => match self.hand_again(group, &token) {
                            Ok(true) => {}
                            Ok(false) | Err(NotNow) => {
                                self.wait_longer(group);
                                return Ok(());
                            }
                        },
                    },
                    Ok(Outcome::Queued) => {
                        self.wait_queued(group);
                        return Ok(());
                    }
                    Ok(Outcome::Dropped) => {
                        if self.dropped_try(group, &token.send)? {
                            continue;
                        }
                        return Ok(());
                    }
                    // A send whose event the core saved was taken by MDK: MDK having no record of
                    // its token means it let go of it since, as under a disappearing time made
                    // longer after, and it may have gone out (D-109).
                    Ok(Outcome::NotTaken) if self.conversation.event_of(&token.send).is_some() => {
                        self.forgotten(&token.send)?;
                        continue;
                    }
                    Ok(Outcome::NotTaken) => {
                        self.conversation.not_kept(&token.send);
                        self.save()?;
                        continue;
                    }
                    Ok(Outcome::Forgotten) => {
                        self.forgotten(&token.send)?;
                        continue;
                    }
                    Err(NotNow) => {
                        self.wait_longer(group);
                        return Ok(());
                    }
                }
            }
            let made = self.drained.entry(group.clone()).or_default();
            if self.bounded && *made >= DRAIN {
                // More is queued: come back to it in the loop's next turn, after other work.
                let wait = self.retry.get(group).map_or(Duration::ZERO, |(_, w)| *w);
                self.retry.insert(group.clone(), (Instant::now(), wait));
                return Ok(());
            }
            *made += 1;
            // Saved before MDK has it, so the token MDK keeps it under is on disk first.
            if self.conversation.attempting(&token.send, unix_now()) && !self.bug_save_after_send()
            {
                self.save()?;
            }
            match self.marmot.send(group, &what, &token).await {
                Ok(event) => self.went(group, &key, event)?,
                Err(failed) => match self.bug_new_token(&token.send, failed) {
                    SendFailed::Refused { gone } => self.give_up(&token.send, gone)?,
                    SendFailed::FirstGone => {
                        // Saved before the send is tried again, as a new first event or message.
                        self.conversation.first_gone(&token.send);
                        self.save()?;
                    }
                    SendFailed::ThreadGone => {
                        // Saved before the words are tried again, in the main chat.
                        self.conversation.thread_gone(&token.send);
                        self.save()?;
                    }
                    SendFailed::Forgotten => self.forgotten(&token.send)?,
                    SendFailed::Rejected => {
                        if self.dropped_try(group, &token.send)? {
                            continue;
                        }
                        return Ok(());
                    }
                    // MDK has it queued: asked after again soon, under the same token, and the
                    // core goes on meanwhile. The event MDK named is kept, should the send be
                    // asked after without being handed to MDK again.
                    SendFailed::Pending(event) => {
                        if self.conversation.took(&token.send, event) {
                            self.save()?;
                        }
                        self.wait_queued(group);
                        return Ok(());
                    }
                    failed @ (SendFailed::Busy | SendFailed::Unknown) => {
                        if failed == SendFailed::Unknown {
                            eprintln!("marmot-agent: a send failed; it will be tried again");
                        }
                        self.wait_longer(group);
                        return Ok(());
                    }
                },
            }
        }
        self.retry.remove(group);
        self.queued.remove(group);
        Ok(())
    }

    /// The send named `token` will not go out: the conversation drops it with a notice and what
    /// stands in for it (`Conversation::refused`), or sends an edit whose message is `gone` as a
    /// new message.
    fn give_up(&mut self, token: &str, gone: bool) -> Result<(), Unsaved> {
        // MDK's own words can name a group, so they are not printed (D-071).
        eprintln!("marmot-agent: a send was refused; the chat is told");
        self.conversation.refused(token, gone);
        self.save()
    }

    /// The oldest send queued for `group`, of the item named `key`, went out as `event`.
    fn went(&mut self, group: &GroupKey, key: &str, event: EventKey) -> Result<(), Unsaved> {
        self.retry.remove(group);
        self.queued.remove(group);
        self.conversation.sent(key, event);
        self.save()
    }

    /// MDK took the send named `token` and dropped it before it went out: nothing went, and the
    /// next try is under a new token, saved first, after a wait. The last try is saved with the
    /// give-up and what stands in for it, in one write. Whether the group's sends go on now, as
    /// the send was given up.
    fn dropped_try(&mut self, group: &GroupKey, token: &str) -> Result<bool, Unsaved> {
        let tries = self.conversation.rejected(token);
        if tries >= self.rejected_tries {
            self.give_up(token, false)?;
            return Ok(true);
        }
        self.save()?;
        eprintln!("marmot-agent: a send was dropped by MDK; it will be tried again");
        self.wait_longer(group);
        Ok(false)
    }

    /// The send named `token` is given up without knowing whether it went out, as MDK no longer
    /// knows it, or may let go of it before it could be handed to it again: the chat is told it
    /// may not have been delivered (D-109).
    fn forgotten(&mut self, token: &str) -> Result<(), Unsaved> {
        // MDK's own words can name a group, so they are not printed (D-071).
        eprintln!("marmot-agent: a send's outcome can no longer be known; the chat is told");
        self.conversation.lost(token);
        self.save()
    }

    /// Whether the send under `token`, which MDK's engine has without naming its event and whose
    /// event the core did not keep, is handed to MDK again under its token to learn its event:
    /// only in the form it was handed, which the core noted ([`Conversation::form_known`]), and
    /// while MDK will still know the token then ([`Side::lasts`]), since MDK takes a token it has
    /// let go of as a new send. This is the one second hand-over left (D-152), which the owner
    /// may yet remove: answering `false` always leaves the send asked after until MDK names its
    /// event or lets go of its token, and changes nothing else.
    fn hand_again(&self, group: &GroupKey, token: &crate::Token) -> Result<bool, NotNow> {
        if !self.conversation.form_known(&token.send) {
            return Ok(false);
        }
        self.marmot.lasts(group, token)
    }

    /// The group's sends wait before they are tried again, longer each time.
    fn wait_longer(&mut self, group: &GroupKey) {
        self.queued.remove(group);
        let wait = (self.retry.get(group)).map_or(Duration::from_millis(100), |(_, w)| {
            (*w * 2).clamp(Duration::from_millis(100), SEND_WAIT_MAX)
        });
        (self.retry).insert(group.clone(), (Instant::now() + wait, wait));
    }

    /// MDK has the group's next send queued: it is asked after again soon, a little later each
    /// time, up to [`QUEUED_WAIT_MAX`], and the group's sends after it wait for it.
    fn wait_queued(&mut self, group: &GroupKey) {
        self.queued.insert(group.clone());
        let wait = (self.retry.get(group)).map_or(Duration::from_millis(10), |(_, w)| {
            (*w * 2).clamp(Duration::from_millis(10), QUEUED_WAIT_MAX)
        });
        (self.retry).insert(group.clone(), (Instant::now() + wait, wait));
    }

    /// Whether the bug "a live message moves the mark to its place" is put back and applies to a
    /// message in `group`: one with a mark.
    #[cfg(feature = "sim-bugs")]
    fn bug_marks_from_live(&self, group: &GroupKey) -> bool {
        self.bugs.mark_from_live && self.read.contains_key(group)
    }

    /// The bug "a live message moves the mark to its place": the message is found in storage, by
    /// reading its group from its start, and handled itself.
    #[cfg(feature = "sim-bugs")]
    async fn bug_mark_from_live(&mut self, message: Message) -> Result<(), Unsaved> {
        let mut after = i64::MIN;
        loop {
            let Ok(page) = self.marmot.stored(&message.group, after, PAGE) else {
                return Ok(());
            };
            let Some(last) = page.last else {
                return Ok(());
            };
            if let Some(found) = (page.messages.into_iter()).find(|m| m.event == message.event) {
                return self.handle(found).await;
            }
            after = last;
        }
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

    /// With the bug "a send whose outcome is not known is tried again under a new token" put
    /// back: such a send's next try is made under a new token, as if MDK had dropped this one.
    fn bug_new_token(&mut self, token: &str, failed: SendFailed) -> SendFailed {
        #[cfg(feature = "sim-bugs")]
        if self.bugs.retry_ambiguous_with_new_token && failed == SendFailed::Unknown {
            self.conversation.rejected(token);
        }
        let _ = token;
        failed
    }

    fn save(&self) -> Result<(), Unsaved> {
        let kept = Kept {
            conversation: self.conversation.saved().clone(),
            read: self.read.clone(),
            history: self.history.clone(),
        };
        self.store.save(&kept).map_err(|_| Unsaved)
    }
}

/// Seconds since 1970, as a state file keeps when a send was first tried: unlike the core's clock,
/// the same across restarts.
fn unix_now() -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    now.map_or(0, |d| d.as_secs())
}

/// Publishes the agent's name in its profile, so people see it, keeping the rest of the profile as
/// it is; nothing when the name is already right. A try that fails, as when the profile could not
/// be read, is made again 10 s later, and twice as long after each one that fails, up to 10
/// minutes: never over a profile the core could not read (#139). It runs as a task of its own, as
/// a read of the relays can take long, and ends when it is done or the core stops.
async fn publish_name<M: Side>(marmot: M, name: String, relays: Vec<String>) {
    let mut wait = NAME_WAIT;
    while marmot.publish_name(&name, &relays).await.is_err() {
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(NAME_WAIT_MAX);
    }
}

/// A task that ends when this is dropped.
struct Ending(tokio::task::JoinHandle<()>);

impl Drop for Ending {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Reads a file from the allowed folders and sends it to `group`. It is offered to MDK again only
/// while MDK says it is busy, which MDK says only before it has taken the file. Any other failure
/// may have come after MDK took it, such as a wait for MDK that ran out while the send went on, so
/// it is not tried again: that could post the file twice, and a post cannot be taken back.
///
/// The read takes one of `reads` for as long as it runs, on its own thread, so the core can wait
/// for it to return even once this upload was stopped (D-145).
async fn upload(
    marmot: impl Side,
    reads: Arc<tokio::sync::Semaphore>,
    folders: Folders,
    path: PathBuf,
    group: &GroupKey,
    turn: Option<marmot_agent_events::TurnId>,
) -> Result<(), Refusal> {
    let reading = reads.acquire_owned().await;
    let side = marmot.clone();
    let read = tokio::task::spawn_blocking(move || {
        let _reading = reading;
        side.read_file(&folders, &path)
    })
    .await;
    let outgoing = read.map_err(|_| Refusal::new(Reason::Failed))??;
    let mut wait = Duration::from_millis(100);
    for tried in 1..=FILE_BUSY_TRIES {
        match marmot.send_file(group, &outgoing, turn.as_ref()).await {
            Ok(_) => return Ok(()),
            Err(FileFailed::Busy) => {}
            Err(FileFailed::Failed) => {
                // MDK's own words can name a group, so they are not printed (D-071).
                eprintln!("marmot-agent: a file's send failed");
                return Err(Refusal {
                    reason: Reason::Failed,
                    detail: Some("It could not be uploaded or sent.".into()),
                    unanswered: false,
                });
            }
        }
        if tried < FILE_BUSY_TRIES {
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(10));
        }
    }
    Err(Refusal {
        reason: Reason::Busy,
        detail: Some("The agent's account stayed busy. Try again later.".into()),
        unanswered: false,
    })
}

/// Serves connectors on the socket, one at a time, until `stop` resolves. Each connector that
/// gives a good hello drives the agent until it goes; then the core closes what it left open, as
/// for any driver that ends (D-058), and waits for the next. What arrives in between is stored by
/// MDK, and read when the next connector has come. `since` is when the program subscribed to MDK
/// ([`Subscribed`]), before any connector came: what arrives while one starts is not history.
///
/// The files being read are bounded across connectors: a read a connector's core left going, as
/// one stalled on a slow folder, holds up no next connector, and keeps its permit until it
/// returns. Its file reaches no MDK. Every read has returned before this does (D-145).
pub async fn serve(
    marmot: Marmot,
    config: &Config,
    store: &Path,
    listener: tokio::net::UnixListener,
    token: String,
    since: u64,
    stop: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let reads = Arc::new(tokio::sync::Semaphore::new(FILES_AT_ONCE));
    let served = serve_on(marmot, config, store, listener, token, since, &reads, stop).await;
    drop(reads.acquire_many(FILES_AT_ONCE as u32).await);
    served
}

/// [`serve`], with the cores sharing `reads`.
#[allow(clippy::too_many_arguments)]
async fn serve_on(
    marmot: Marmot,
    config: &Config,
    store: &Path,
    listener: tokio::net::UnixListener,
    token: String,
    since: u64,
    reads: &Arc<tokio::sync::Semaphore>,
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
        // A session goes on across connectors, and so do its numbers.
        let last = Store::new(store).load()?.conversation.last_numbers();
        let driver = driver.numbered_after(last);
        // Each core reads what it did not hear live from storage, from the program's start.
        let live = Subscribed {
            events: marmot.runtime.subscribe(),
            since,
        };
        let core = Core::new(marmot.clone(), driver, config, Store::new(store))?
            .sharing_reads(reads.clone());
        let mut until = stopped.clone();
        let running = core.run(live, async move {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{EventKey, PubKey};
    use crate::marmot::Page;
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    /// How each file went, as a driver was told, in order.
    type Told = Arc<Mutex<Vec<(String, Result<(), Refusal>)>>>;

    /// A stand-in for MDK that answers each file it is offered with the next of `answers`, and
    /// counts the offers. It does nothing else.
    #[derive(Clone)]
    struct Uploads {
        agent: PubKey,
        answers: Arc<Mutex<VecDeque<Result<EventKey, FileFailed>>>>,
        offers: Arc<Mutex<u32>>,
    }

    impl Uploads {
        fn answering(answers: Vec<Result<EventKey, FileFailed>>) -> Self {
            Self {
                agent: PubKey::from("agent"),
                answers: Arc::new(Mutex::new(answers.into())),
                offers: Arc::default(),
            }
        }

        fn offers(&self) -> u32 {
            *self.offers.lock().unwrap()
        }
    }

    impl Side for Uploads {
        type Events = ();

        fn agent(&self) -> &PubKey {
            &self.agent
        }

        async fn next(&self, _events: &mut ()) -> Live {
            unreachable!("only files are sent")
        }

        async fn publish_name(&self, _name: &str, _relays: &[String]) -> Result<(), NotNow> {
            Ok(())
        }

        async fn incoming(&self, _message: Message) -> Result<Option<Incoming>, NotNow> {
            unreachable!("only files are sent")
        }

        fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)> {
            Vec::new()
        }

        async fn decide(&self, _group: &GroupKey, _accept: bool) -> bool {
            unreachable!("only files are sent")
        }

        fn groups(&self) -> Result<Vec<GroupKey>, NotNow> {
            Ok(Vec::new())
        }

        fn two_people(&self, _group: &GroupKey) -> Result<Option<Vec<PubKey>>, NotNow> {
            unreachable!("only files are sent")
        }

        fn stored(&self, _group: &GroupKey, _after: i64, _limit: usize) -> Result<Page, NotNow> {
            unreachable!("only files are sent")
        }

        async fn send(
            &self,
            _group: &GroupKey,
            _what: &Sending,
            _token: &crate::Token,
        ) -> Result<EventKey, SendFailed> {
            unreachable!("only files are sent")
        }

        async fn send_file(
            &self,
            _group: &GroupKey,
            _file: &crate::files::Outgoing,
            _turn: Option<&marmot_agent_events::TurnId>,
        ) -> Result<EventKey, FileFailed> {
            *self.offers.lock().unwrap() += 1;
            let next = self.answers.lock().unwrap().pop_front();
            next.unwrap_or(Err(FileFailed::Busy))
        }

        fn outcome(&self, _group: &GroupKey, _token: &crate::Token) -> Result<Outcome, NotNow> {
            unreachable!("only files are sent")
        }

        fn lasts(&self, _group: &GroupKey, _token: &crate::Token) -> Result<bool, NotNow> {
            unreachable!("only files are sent")
        }
    }

    /// A stand-in for MDK with one group, whose `messages` were stored while the core was down,
    /// each read as nothing for the agent, and noted in `read`, by its place. Nothing arrives
    /// live, and nothing is sent.
    #[derive(Clone)]
    struct Backlog {
        agent: PubKey,
        messages: u32,
        read: Arc<Mutex<Vec<i64>>>,
    }

    impl Side for Backlog {
        type Events = ();

        fn agent(&self) -> &PubKey {
            &self.agent
        }

        async fn next(&self, _events: &mut ()) -> Live {
            std::future::pending().await
        }

        async fn publish_name(&self, _name: &str, _relays: &[String]) -> Result<(), NotNow> {
            Ok(())
        }

        fn two_people(&self, _group: &GroupKey) -> Result<Option<Vec<PubKey>>, NotNow> {
            Ok(None)
        }

        async fn incoming(&self, message: Message) -> Result<Option<Incoming>, NotNow> {
            self.read.lock().unwrap().extend(message.order);
            Ok(None)
        }

        fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)> {
            Vec::new()
        }

        async fn decide(&self, _group: &GroupKey, _accept: bool) -> bool {
            unreachable!("no invitations")
        }

        fn groups(&self) -> Result<Vec<GroupKey>, NotNow> {
            Ok(vec![GroupKey::from("g")])
        }

        fn stored(&self, group: &GroupKey, after: i64, limit: usize) -> Result<Page, NotNow> {
            let messages: Vec<Message> = (1..=i64::from(self.messages))
                .filter(|&order| order > after)
                .take(limit)
                .map(|order| Message {
                    group: group.clone(),
                    event: EventKey::from(format!("e{order}").as_str()),
                    sender: PubKey::from("person"),
                    sender_name: None,
                    kind: 9,
                    content: String::new(),
                    tags: Vec::new(),
                    recorded_at: 1,
                    received_at: 1,
                    order: Some(order),
                })
                .collect();
            let last = messages.last().and_then(|m| m.order);
            let more = messages.len() == limit;
            Ok(Page {
                messages,
                last,
                more,
            })
        }

        async fn send(
            &self,
            _group: &GroupKey,
            _what: &Sending,
            _token: &crate::Token,
        ) -> Result<EventKey, SendFailed> {
            unreachable!("nothing is sent")
        }

        async fn send_file(
            &self,
            _group: &GroupKey,
            _file: &crate::files::Outgoing,
            _turn: Option<&marmot_agent_events::TurnId>,
        ) -> Result<EventKey, FileFailed> {
            unreachable!("no file reaches MDK")
        }

        fn outcome(&self, _group: &GroupKey, _token: &crate::Token) -> Result<Outcome, NotNow> {
            unreachable!("nothing is sent")
        }

        fn lasts(&self, _group: &GroupKey, _token: &crate::Token) -> Result<bool, NotNow> {
            unreachable!("nothing is sent")
        }
    }

    /// A driver with `ready` files to send waiting, each for a chat the core does not know, which
    /// it refuses; it counts the refusals it was told of. Then it waits for good.
    struct Burst {
        ready: u32,
        told: Arc<Mutex<u32>>,
    }

    impl Driver for Burst {
        fn harness(&self) -> marmot_agent_driver::Harness {
            marmot_agent_driver::Harness {
                name: "burst".into(),
                version: None,
            }
        }

        fn capabilities(&self) -> marmot_agent_driver::Capabilities {
            marmot_agent_driver::Capabilities {
                interrupt: false,
                mid_turn: marmot_agent_driver::MidTurn::Queue,
                sessions: marmot_agent_driver::Sessions::Core,
                resume: false,
                approvals: false,
                files_in: marmot_agent_driver::FilesIn::None,
                subagents: false,
                stop_subagent: false,
                approval_commands: Vec::new(),
            }
        }

        async fn prompt(&mut self, _prompt: marmot_agent_driver::Prompt) -> Result<(), Refusal> {
            unreachable!("no prompt")
        }

        async fn answer(&mut self, _answer: marmot_agent_driver::Answer) -> Result<(), Refusal> {
            unreachable!("no answer")
        }

        async fn control(&mut self, _control: marmot_agent_driver::Control) -> Result<(), Refusal> {
            unreachable!("no control")
        }

        async fn next_event(&mut self) -> Option<Output> {
            if self.ready == 0 {
                return std::future::pending().await;
            }
            self.ready -= 1;
            Some(Output::SendFile(SendFile {
                id: format!("f{}", self.ready),
                chat: marmot_agent_driver::ChatId::from("nowhere"),
                path: PathBuf::from("/nowhere"),
            }))
        }

        async fn file_sent(&mut self, _id: String, _result: Result<(), Refusal>) {
            *self.told.lock().unwrap() += 1;
        }
    }

    /// A core with `stored` messages waiting in its one group, and a driver with `ready` outputs
    /// waiting, run until `stop`. What MDK was asked to read, by place, and how many of the
    /// driver's outputs the core took.
    async fn catch_up_with_a_burst(
        stored: u32,
        ready: u32,
        stop: impl FnOnce(Arc<Mutex<Vec<i64>>>) -> std::pin::Pin<Box<dyn Future<Output = ()>>>,
    ) -> (Vec<i64>, u32) {
        let dir = tempfile::tempdir().unwrap();
        let settings = format!(
            "home = \"/unused\"\nrelays = [\"wss://relay.invalid\"]\nowner = \"{}\"\n[connector]\n",
            "ab".repeat(32)
        );
        let config = Config::parse(&settings).unwrap();
        let read = Arc::new(Mutex::new(Vec::new()));
        let side = Backlog {
            agent: PubKey::from("agent"),
            messages: stored,
            read: read.clone(),
        };
        let told = Arc::new(Mutex::new(0));
        let driver = Burst {
            ready,
            told: told.clone(),
        };
        let core = Core::new(side, driver, &config, Store::new(dir.path())).unwrap();
        let live = Subscribed {
            events: (),
            since: 0,
        };
        core.run(live, stop(read.clone())).await.unwrap();
        let read = read.lock().unwrap().clone();
        let told = *told.lock().unwrap();
        (read, told)
    }

    // What the driver gives while the core reads what was stored is taken in the loop's turns, as
    // the rest of what is ready, not all of it first: a stop already there waits behind a few of
    // a burst, not the whole of it. Each turn takes one thing at most, starting one further on.
    #[tokio::test(start_paused = true)]
    async fn a_burst_found_while_catching_up_waits_its_turns() {
        let (_, told) = catch_up_with_a_burst(20, 50, |_| Box::pin(std::future::ready(()))).await;
        assert!(
            told <= WAITS as u32,
            "{told} of the burst went before the stop"
        );
    }

    // The catch-up that stopped for the driver's output reads on from its mark once the loop has
    // taken it: every stored message is read, once and in order, and all of the burst is taken.
    #[tokio::test(start_paused = true)]
    async fn a_catch_up_stopped_for_the_driver_reads_on_from_its_mark() {
        let (read, told) = catch_up_with_a_burst(20, 50, |read| {
            Box::pin(async move {
                while read.lock().unwrap().len() < 20 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
        })
        .await;
        assert_eq!(read, (1..=20).collect::<Vec<i64>>());
        assert_eq!(told, 50, "the whole burst");
    }

    /// Uploads a small file from a folder of its own to `side`, and says how it went and how long
    /// it took on tokio's clock.
    async fn upload_to(side: &Uploads) -> (Result<(), Refusal>, Duration) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "hello").unwrap();
        let folders = Folders::new(&[dir.path().to_path_buf()], None);
        let started = Instant::now();
        let reads = Arc::new(tokio::sync::Semaphore::new(1));
        let group = GroupKey::from("g");
        let result = upload(side.clone(), reads, folders, path, &group, None).await;
        (result, started.elapsed())
    }

    // A file MDK is busy for is offered again, a little later each time, until MDK takes it: it
    // goes out once (D-090).
    #[tokio::test(start_paused = true)]
    async fn a_file_mdk_is_busy_for_is_offered_again_until_taken() {
        let side = Uploads::answering(vec![
            Err(FileFailed::Busy),
            Err(FileFailed::Busy),
            Ok(EventKey::from("e1")),
        ]);
        let (result, took) = upload_to(&side).await;
        assert!(result.is_ok());
        assert_eq!(side.offers(), 3);
        assert_eq!(took, Duration::from_millis(300), "100 ms, then 200 ms");
    }

    // MDK busy all along: the file is given up as busy, after about half a minute of tries, and
    // not waited for after the last one.
    #[tokio::test(start_paused = true)]
    async fn a_file_mdk_stays_busy_for_is_given_up_as_busy() {
        let side = Uploads::answering(Vec::new());
        let (result, took) = upload_to(&side).await;
        let refusal = result.expect_err("given up");
        assert_eq!(refusal.reason, Reason::Busy);
        assert_eq!(side.offers(), FILE_BUSY_TRIES);
        assert!(
            took < Duration::from_secs(35),
            "no wait after the last try: {took:?}"
        );
    }

    // Any other failure may have come after MDK took the file, so it is not offered again: that
    // could post it twice. The refusal says it failed, and names no path (D-071).
    #[tokio::test(start_paused = true)]
    async fn a_file_whose_send_failed_is_not_offered_again() {
        let side = Uploads::answering(vec![Err(FileFailed::Failed), Ok(EventKey::from("e1"))]);
        let (result, _) = upload_to(&side).await;
        let refusal = result.expect_err("failed");
        assert_eq!(refusal.reason, Reason::Failed);
        assert!(!refusal.detail.unwrap_or_default().contains('/'));
        assert_eq!(side.offers(), 1);
    }

    // A save of the state already on disk writes nothing, so a step that changed nothing costs no
    // write and no sync; a state that changed is written whole, as before (#155).
    #[test]
    fn a_save_of_the_state_on_disk_writes_nothing() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let file = dir.path().join("marmot-agent-state.json");
        let inode = || std::fs::metadata(&file).unwrap().ino();
        let mut kept = Kept::default();
        store.save(&kept).unwrap();
        let first = inode();
        store.save(&kept).unwrap();
        assert_eq!(inode(), first, "the same state again is not written");
        kept.read.insert(GroupKey::from("g"), 7);
        store.save(&kept).unwrap();
        assert_ne!(inode(), first, "a changed state is written");
        assert_eq!(store.load().unwrap().read, kept.read);
    }

    // A save that failed once the new file was in place leaves the store not knowing what is on
    // disk: the next save is written whole, even of the state saved before, so a save that
    // returns well always leaves its own state on disk (#155).
    #[test]
    fn a_save_after_one_that_failed_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let a = Kept::default();
        let mut b = Kept::default();
        b.read.insert(GroupKey::from("g"), 7);
        store.save(&a).unwrap();
        // The new file goes in place, and the folder's sync, the last step, fails.
        store.folder_sync_fails.store(true, SeqCst);
        let failed = store.save(&b);
        store.folder_sync_fails.store(false, SeqCst);
        assert!(failed.is_err(), "the folder's sync failed");
        assert_eq!(
            store.load().unwrap().read,
            b.read,
            "the new file is in place"
        );
        store.save(&a).unwrap();
        assert_eq!(
            store.load().unwrap().read,
            a.read,
            "the state saved is on disk"
        );
    }

    // A fault meets a save whose state is the one on disk too, so the simulator can put a crash
    // at any save, the end of a step that changed nothing included.
    #[test]
    fn a_fault_fails_a_save_that_would_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let armed = Arc::new(AtomicBool::new(false));
        let when = armed.clone();
        let (store, failed) = Store::failing_when(dir.path(), move |_| when.load(SeqCst));
        let kept = Kept::default();
        store.save(&kept).unwrap();
        armed.store(true, SeqCst);
        assert!(
            store.save(&kept).is_err(),
            "the same state again still fails"
        );
        assert!(failed.load(SeqCst));
    }

    /// A stand-in for MDK with one group, the owner's chat with the agent, holding what the test
    /// stores there. Each file offered waits until the test opens the gate, counting how many
    /// wait at once; a wait that is dropped counts as ended. Every send is taken, and the text of
    /// each notice sent is kept. Reading a file to send waits while the test holds reads, and is
    /// counted too.
    #[derive(Clone)]
    struct Desk {
        agent: PubKey,
        owner: PubKey,
        stored: Arc<Mutex<Vec<Message>>>,
        stored_now: Arc<tokio::sync::Notify>,
        gate: tokio::sync::watch::Receiver<bool>,
        uploading: Arc<Mutex<(usize, usize)>>,
        offers: Arc<Mutex<u32>>,
        notices: Arc<Mutex<Vec<String>>>,
        /// Whether reads may go on, and the files being read now, and in all.
        reads_open: Arc<(Mutex<bool>, std::sync::Condvar)>,
        reading: Arc<Mutex<(usize, usize)>>,
    }

    /// One read of a file under way, for as long as it lives.
    struct Reading(Arc<Mutex<(usize, usize)>>);

    impl Drop for Reading {
        fn drop(&mut self) {
            self.0.lock().unwrap().0 -= 1;
        }
    }

    /// One upload under way, for as long as it lives.
    struct Uploading(Arc<Mutex<(usize, usize)>>);

    impl Uploading {
        fn new(counts: &Arc<Mutex<(usize, usize)>>) -> Self {
            let mut now = counts.lock().unwrap();
            now.0 += 1;
            now.1 = now.1.max(now.0);
            Self(counts.clone())
        }
    }

    impl Drop for Uploading {
        fn drop(&mut self) {
            self.0.lock().unwrap().0 -= 1;
        }
    }

    impl Desk {
        /// The owner says `text` in the chat.
        fn say(&self, text: &str) {
            let mut stored = self.stored.lock().unwrap();
            let order = stored.len() as i64 + 1;
            stored.push(Message {
                group: GroupKey::from("g"),
                event: EventKey::from(format!("m{order}").as_str()),
                sender: self.owner.clone(),
                sender_name: None,
                kind: 9,
                content: text.into(),
                tags: Vec::new(),
                recorded_at: 1,
                received_at: 1,
                order: Some(order),
            });
            self.stored_now.notify_one();
        }

        /// How many files are uploading now, and the most that were at once.
        fn uploading(&self) -> (usize, usize) {
            *self.uploading.lock().unwrap()
        }

        /// How many files are being read now, and how many reads started in all.
        fn reading(&self) -> (usize, usize) {
            *self.reading.lock().unwrap()
        }

        /// How many files MDK was offered.
        fn offers(&self) -> usize {
            *self.offers.lock().unwrap() as usize
        }

        /// How many notices the chat was sent that a file was not sent.
        fn not_sent(&self) -> usize {
            (self.notices.lock().unwrap().iter())
                .filter(|text| text.starts_with("The agent's file was not sent."))
                .count()
        }

        /// Holds every read of a file from now, or lets them go on.
        fn hold_reads(&self, held: bool) {
            let (open, changed) = &*self.reads_open;
            *open.lock().unwrap() = !held;
            changed.notify_all();
        }

        /// Holds every read of a file until the guard given back is dropped, if nothing lets
        /// them go before. A test that fails meanwhile lets them go as it unwinds, so it reports
        /// the failure: its runtime, as it ends, waits for every read still held.
        fn reads_held(&self) -> ReadsHeld {
            self.hold_reads(true);
            ReadsHeld(self.clone())
        }
    }

    /// Lets every read of a file go on when dropped.
    struct ReadsHeld(Desk);

    impl Drop for ReadsHeld {
        fn drop(&mut self) {
            self.0.hold_reads(false);
        }
    }

    impl Side for Desk {
        type Events = ();

        fn agent(&self) -> &PubKey {
            &self.agent
        }

        async fn next(&self, _events: &mut ()) -> Live {
            self.stored_now.notified().await;
            let last = self.stored.lock().unwrap().last().cloned().unwrap();
            Live::Message(Message {
                order: None,
                ..last
            })
        }

        async fn publish_name(&self, _name: &str, _relays: &[String]) -> Result<(), NotNow> {
            Ok(())
        }

        async fn incoming(&self, message: Message) -> Result<Option<Incoming>, NotNow> {
            let chat = crate::conversation::Where {
                group: message.group,
                name: None,
                direct: true,
                members: 2,
            };
            // `/new` stands for the owner's request for a new session, a control.
            if message.content == "/new" {
                return Ok(Some(Incoming::Asked(crate::conversation::HeardRequest {
                    chat,
                    event: message.event,
                    author: message.sender,
                    author_name: "Owner".into(),
                    age_secs: 0,
                    target: None,
                    asked: Ok(crate::carrier::Asked::NewSession),
                })));
            }
            Ok(Some(Incoming::Heard(crate::conversation::Heard {
                chat,
                event: message.event,
                author: message.sender,
                author_name: "Owner".into(),
                text: message.content,
                reply_to: None,
                mentions_agent: false,
                attachments: Vec::new(),
            })))
        }

        fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)> {
            Vec::new()
        }

        async fn decide(&self, _group: &GroupKey, _accept: bool) -> bool {
            unreachable!("no invitations")
        }

        fn groups(&self) -> Result<Vec<GroupKey>, NotNow> {
            Ok(vec![GroupKey::from("g")])
        }

        fn two_people(&self, _group: &GroupKey) -> Result<Option<Vec<PubKey>>, NotNow> {
            Ok(Some(vec![self.agent.clone(), self.owner.clone()]))
        }

        // Every send is taken, so this MDK keeps each token.
        fn outcome(&self, _group: &GroupKey, _token: &crate::Token) -> Result<Outcome, NotNow> {
            Ok(Outcome::Taken)
        }

        fn lasts(&self, _group: &GroupKey, _token: &crate::Token) -> Result<bool, NotNow> {
            Ok(true)
        }

        fn stored(&self, _group: &GroupKey, after: i64, limit: usize) -> Result<Page, NotNow> {
            let stored = self.stored.lock().unwrap();
            let messages: Vec<Message> = (stored.iter())
                .filter(|m| m.order > Some(after))
                .take(limit)
                .cloned()
                .collect();
            let last = messages.last().and_then(|m| m.order);
            let more = messages.len() == limit;
            Ok(Page {
                messages,
                last,
                more,
            })
        }

        async fn send(
            &self,
            _group: &GroupKey,
            what: &Sending,
            token: &crate::Token,
        ) -> Result<EventKey, SendFailed> {
            if let Sending::Item { item, .. } = what
                && matches!(item.event.body(), marmot_agent_events::Body::Notice(_))
            {
                let text = item.event.header().text.clone();
                self.notices.lock().unwrap().push(text);
            }
            // A send to MDK waits on its engine, and lets other tasks run meanwhile.
            tokio::task::yield_now().await;
            Ok(EventKey::from(format!("sent-{}", token.mdk).as_str()))
        }

        async fn send_file(
            &self,
            _group: &GroupKey,
            _file: &crate::files::Outgoing,
            _turn: Option<&marmot_agent_events::TurnId>,
        ) -> Result<EventKey, FileFailed> {
            *self.offers.lock().unwrap() += 1;
            let _uploading = Uploading::new(&self.uploading);
            let mut gate = self.gate.clone();
            let _ = gate.wait_for(|open| *open).await;
            Ok(EventKey::from("file"))
        }

        fn read_file(
            &self,
            folders: &Folders,
            path: &std::path::Path,
        ) -> Result<crate::files::Outgoing, Refusal> {
            {
                let mut reading = self.reading.lock().unwrap();
                reading.0 += 1;
                reading.1 += 1;
            }
            let _reading = Reading(self.reading.clone());
            let (open, changed) = &*self.reads_open;
            let _open = changed.wait_while(open.lock().unwrap(), |open| !*open);
            folders.read(path)
        }
    }

    /// A driver the test hands outputs to, which passes on each prompt it is given and notes how
    /// each file went. It ends when the test drops its sender.
    struct Hand {
        outputs: mpsc::UnboundedReceiver<Output>,
        prompts: mpsc::UnboundedSender<marmot_agent_driver::Prompt>,
        told: Told,
    }

    impl Driver for Hand {
        fn harness(&self) -> marmot_agent_driver::Harness {
            marmot_agent_driver::Harness {
                name: "hand".into(),
                version: None,
            }
        }

        fn capabilities(&self) -> marmot_agent_driver::Capabilities {
            marmot_agent_driver::Capabilities {
                interrupt: false,
                mid_turn: marmot_agent_driver::MidTurn::Queue,
                sessions: marmot_agent_driver::Sessions::Core,
                resume: false,
                approvals: false,
                files_in: marmot_agent_driver::FilesIn::None,
                approval_commands: Vec::new(),
                subagents: false,
                stop_subagent: false,
            }
        }

        async fn prompt(&mut self, prompt: marmot_agent_driver::Prompt) -> Result<(), Refusal> {
            let _ = self.prompts.send(prompt);
            Ok(())
        }

        async fn answer(&mut self, _answer: marmot_agent_driver::Answer) -> Result<(), Refusal> {
            unreachable!("no answer")
        }

        async fn control(&mut self, _control: marmot_agent_driver::Control) -> Result<(), Refusal> {
            unreachable!("no control")
        }

        async fn next_event(&mut self) -> Option<Output> {
            self.outputs.recv().await
        }

        async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
            self.told.lock().unwrap().push((id, result));
        }
    }

    /// A core on a [`Desk`], driving a [`Hand`], running until `stop` is sent, with what the
    /// test needs to steer and watch it.
    struct Desked {
        side: Desk,
        open: tokio::sync::watch::Sender<bool>,
        outputs: mpsc::UnboundedSender<Output>,
        prompts: mpsc::UnboundedReceiver<marmot_agent_driver::Prompt>,
        told: Told,
        stop: tokio::sync::oneshot::Sender<()>,
        running: tokio::task::JoinHandle<Result<(), String>>,
        folder: tempfile::TempDir,
        state: tempfile::TempDir,
        config: Config,
    }

    /// A core on a [`Desk`], with a folder it may send `notes.txt` from, driving `driver` until
    /// it ends or the stop is sent.
    struct OnDesk {
        side: Desk,
        open: tokio::sync::watch::Sender<bool>,
        stop: tokio::sync::oneshot::Sender<()>,
        running: tokio::task::JoinHandle<Result<(), String>>,
        folder: tempfile::TempDir,
        state: tempfile::TempDir,
        config: Config,
    }

    /// Runs a core on `side` with `config`, keeping its state in `store`, driving `driver`, and
    /// sharing `reads` with the cores before and after it, as [`serve`] does, if given. It runs
    /// until the stop given back is sent.
    fn run_on_desk(
        side: &Desk,
        config: &Config,
        store: Store,
        driver: impl Driver + 'static,
        reads: Option<Arc<tokio::sync::Semaphore>>,
    ) -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<Result<(), String>>,
    ) {
        let core = Core::new(side.clone(), driver, config, store).unwrap();
        let core = match reads {
            Some(reads) => core.sharing_reads(reads),
            None => core,
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let live = Subscribed {
            events: (),
            since: 0,
        };
        let running = tokio::spawn(core.run(live, async {
            let _ = stopped.await;
        }));
        (stop, running)
    }

    impl OnDesk {
        fn start(driver: impl Driver + 'static) -> Self {
            Self::start_with(driver, Store::new, None)
        }

        /// A core as [`OnDesk::start`] starts one, keeping its state in the store `store` makes
        /// for a folder, and sharing `reads`, if given.
        fn start_with(
            driver: impl Driver + 'static,
            store: impl FnOnce(&Path) -> Store,
            reads: Option<Arc<tokio::sync::Semaphore>>,
        ) -> Self {
            let folder = tempfile::tempdir().unwrap();
            std::fs::write(folder.path().join("notes.txt"), "hello").unwrap();
            let state = tempfile::tempdir().unwrap();
            let owner = "ab".repeat(32);
            let settings = format!(
                "home = \"/unused\"\nrelays = [\"wss://relay.invalid\"]\nowner = \"{owner}\"\n\
                 files_from = [\"{}\"]\n[connector]\n",
                folder.path().display()
            );
            let config = Config::parse(&settings).unwrap();
            let (open, gate) = tokio::sync::watch::channel(false);
            let side = Desk {
                agent: PubKey::from("cd".repeat(32).as_str()),
                owner: PubKey::from(owner.as_str()),
                stored: Arc::default(),
                stored_now: Arc::default(),
                gate,
                uploading: Arc::default(),
                offers: Arc::default(),
                notices: Arc::default(),
                reads_open: Arc::new((Mutex::new(true), std::sync::Condvar::new())),
                reading: Arc::default(),
            };
            let (stop, running) = run_on_desk(&side, &config, store(state.path()), driver, reads);
            Self {
                side,
                open,
                stop,
                running,
                folder,
                state,
                config,
            }
        }

        /// The file the agent may send.
        fn notes(&self) -> PathBuf {
            self.folder.path().join("notes.txt")
        }

        /// Waits for the core to end, within a few seconds.
        async fn ended(self) {
            tokio::time::timeout(Duration::from_secs(5), self.running)
                .await
                .expect("the core ends in time")
                .unwrap()
                .unwrap();
        }
    }

    /// A [`Hand`], and what the test keeps to steer and watch it.
    fn hand() -> (
        Hand,
        mpsc::UnboundedSender<Output>,
        mpsc::UnboundedReceiver<marmot_agent_driver::Prompt>,
        Told,
    ) {
        let (outputs, outputs_in) = mpsc::unbounded_channel();
        let (prompts_out, prompts) = mpsc::unbounded_channel();
        let told = Arc::new(Mutex::new(Vec::new()));
        let driver = Hand {
            outputs: outputs_in,
            prompts: prompts_out,
            told: told.clone(),
        };
        (driver, outputs, prompts, told)
    }

    impl Desked {
        fn start() -> Self {
            Self::start_with(Store::new, None)
        }

        /// A core as [`Desked::start`] starts one, keeping its state in the store `store` makes
        /// for a folder, and sharing `reads`, if given.
        fn start_with(
            store: impl FnOnce(&Path) -> Store,
            reads: Option<Arc<tokio::sync::Semaphore>>,
        ) -> Self {
            let (driver, outputs, prompts, told) = hand();
            let OnDesk {
                side,
                open,
                stop,
                running,
                folder,
                state,
                config,
            } = OnDesk::start_with(driver, store, reads);
            Self {
                side,
                open,
                outputs,
                prompts,
                told,
                stop,
                running,
                folder,
                state,
                config,
            }
        }

        /// The next core on the same desk, folder and state, driving a new [`Hand`] and sharing
        /// `reads`, as the next connector's is: what this one's driver was told stays in
        /// `told`, as the earlier driver's.
        fn again(self, reads: Arc<tokio::sync::Semaphore>) -> (Self, Told) {
            let (driver, outputs, prompts, told) = hand();
            let store = Store::new(self.state.path());
            let (stop, running) = run_on_desk(&self.side, &self.config, store, driver, Some(reads));
            let earlier = self.told;
            let next = Self {
                outputs,
                prompts,
                told,
                stop,
                running,
                ..self
            };
            (next, earlier)
        }

        /// The next prompt the driver is given, within a few seconds.
        async fn prompted(&mut self) -> marmot_agent_driver::Prompt {
            tokio::time::timeout(Duration::from_secs(5), self.prompts.recv())
                .await
                .expect("a prompt in time")
                .unwrap()
        }

        /// The agent asks to send `count` files to `chat`, numbered from `from`.
        fn send_files(&self, chat: &marmot_agent_driver::ChatId, from: usize, count: usize) {
            for n in from..from + count {
                let file = SendFile {
                    id: format!("f{n}"),
                    chat: chat.clone(),
                    path: self.folder.path().join("notes.txt"),
                };
                self.outputs.send(Output::SendFile(file)).unwrap();
            }
        }

        /// How each file went, as the driver was told, once `count` were.
        async fn told(&self, count: usize) -> Vec<(String, Result<(), Refusal>)> {
            until(|| (self.told.lock().unwrap().len() >= count).then_some(())).await;
            self.told.lock().unwrap().clone()
        }
    }

    /// Waits up to five seconds for `found` to give something.
    async fn until<T>(mut found: impl FnMut() -> Option<T>) -> T {
        for _ in 0..500 {
            if let Some(found) = found() {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("not in time");
    }

    // The agent asks for more files than the core works on at once: a few upload, some more wait
    // their turn, and the rest are refused as busy at once, so the files the core holds stay
    // bounded. Meanwhile the chat goes on: a prompt reaches the agent while every upload is stuck.
    // Once they get through, each waiting file goes, and never more at once (D-145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_core_uploads_a_few_files_at_once_and_holds_a_bounded_number() {
        let mut desk = Desked::start();
        desk.side.say("go");
        let chat = desk.prompted().await.chat.id;
        let asked = FILES_HELD + 4;
        desk.send_files(&chat, 0, asked);
        let busy = desk.told(asked - FILES_HELD).await;
        assert!(
            (busy.iter()).all(|(_, r)| r.as_ref().is_err_and(|r| r.reason == Reason::Busy)),
            "the files past those held are refused as busy: {busy:?}"
        );
        until(|| (desk.side.uploading().0 == FILES_AT_ONCE).then_some(())).await;
        // The chat is not held up by the files.
        desk.side.say("still there?");
        assert_eq!(desk.prompted().await.text, "still there?");
        assert_eq!(*desk.side.offers.lock().unwrap() as usize, FILES_AT_ONCE);
        desk.open.send(true).unwrap();
        let told = desk.told(asked).await;
        let sent = told.iter().filter(|(_, r)| r.is_ok()).count();
        assert_eq!(sent, FILES_HELD, "every file held went");
        assert_eq!(desk.side.uploading(), (0, FILES_AT_ONCE));
        let _ = desk.stop.send(());
        desk.running.await.unwrap().unwrap();
    }

    // Files the core holds when it stops, or when its driver ends as a lost connector does, end
    // with it: those uploading are stopped, those waiting are never offered to MDK, and nothing
    // the core started goes on uploading after it (D-145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_upload_outlives_the_core() {
        for driver_ends in [false, true] {
            let mut desk = Desked::start();
            desk.side.say("go");
            let chat = desk.prompted().await.chat.id;
            desk.send_files(&chat, 0, FILES_AT_ONCE + 3);
            until(|| (desk.side.uploading().0 == FILES_AT_ONCE).then_some(())).await;
            if driver_ends {
                drop(desk.outputs);
            } else {
                let _ = desk.stop.send(());
            }
            tokio::time::timeout(Duration::from_secs(5), desk.running)
                .await
                .expect("the core ends at once")
                .unwrap()
                .unwrap();
            assert_eq!(desk.side.uploading().0, 0, "no upload goes on");
            desk.open.send(true).unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(
                *desk.side.offers.lock().unwrap() as usize,
                FILES_AT_ONCE,
                "no file waiting was offered after the core ended"
            );
            let told = desk.told.lock().unwrap().clone();
            assert_eq!(told.len(), FILES_AT_ONCE + 3, "each file was answered");
            assert!(told.iter().all(|(_, r)| r.is_err()), "none was sent");
            // MDK may have taken a file that was uploading, so only the driver hears of those;
            // the chat is told of the files never offered (D-145).
            assert_eq!(
                desk.side.not_sent(),
                3,
                "the chat hears of the waiting files only"
            );
        }
    }

    // A save that fails while the core tells the chat of the files it stopped with still tells
    // the driver of every one of them, and then stops with the save's error (D-145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_save_as_the_core_stops_still_answers_every_file() {
        let failing = Arc::new(AtomicBool::new(false));
        let fail = failing.clone();
        let store = move |dir: &Path| Store::failing_when(dir, move |_| fail.load(SeqCst)).0;
        let mut desk = Desked::start_with(store, None);
        desk.side.say("go");
        let chat = desk.prompted().await.chat.id;
        let asked = FILES_AT_ONCE + 3;
        desk.send_files(&chat, 0, asked);
        until(|| (desk.side.uploading().0 == FILES_AT_ONCE).then_some(())).await;
        failing.store(true, SeqCst);
        let _ = desk.stop.send(());
        let ran = tokio::time::timeout(Duration::from_secs(5), desk.running)
            .await
            .expect("the core ends at once")
            .unwrap();
        assert_eq!(
            ran,
            Err(UNSAVED.to_string()),
            "the save's error is the core's"
        );
        let told = desk.told.lock().unwrap().clone();
        assert_eq!(told.len(), asked, "each file was answered: {told:?}");
        assert!(told.iter().all(|(_, r)| r.is_err()), "none was sent");
    }

    // A file being read when the core stops, or when its driver ends, is waited for: no read the
    // core started goes on after it, however slow the disk, and none of those files reaches MDK
    // (D-145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_read_outlives_the_core() {
        for driver_ends in [false, true] {
            let mut desk = Desked::start();
            desk.side.say("go");
            let chat = desk.prompted().await.chat.id;
            let _reads = desk.side.reads_held();
            desk.send_files(&chat, 0, FILES_AT_ONCE);
            until(|| (desk.side.reading().0 == FILES_AT_ONCE).then_some(())).await;
            if driver_ends {
                drop(desk.outputs);
            } else {
                let _ = desk.stop.send(());
            }
            let side = desk.side.clone();
            let slow_disk = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                side.hold_reads(false);
            });
            let ended = tokio::time::timeout(Duration::from_secs(5), desk.running).await;
            let reading = desk.side.reading().0;
            // Let any read still held go, so a failure is reported and does not hang the test.
            desk.side.hold_reads(false);
            slow_disk.await.unwrap();
            ended
                .expect("the core ends once the reads have")
                .unwrap()
                .unwrap();
            assert_eq!(reading, 0, "no read goes on after the core");
            assert_eq!(desk.side.offers(), 0, "no file read was offered to MDK");
            let told = desk.told.lock().unwrap().clone();
            assert_eq!(told.len(), FILES_AT_ONCE, "each file was answered");
            assert!(told.iter().all(|(_, r)| r.is_err()), "none was sent");
            assert_eq!(desk.side.not_sent(), 0, "the chat is told nothing of them");
        }
    }

    // A save that fails while files are being read stops the core where it was, but not before
    // every upload is stopped and every read has returned: none goes on after the core, and none
    // of those files reaches MDK (D-145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_save_still_ends_every_upload_and_read() {
        let failing = Arc::new(AtomicBool::new(false));
        let fail = failing.clone();
        let store = move |dir: &Path| Store::failing_when(dir, move |_| fail.load(SeqCst)).0;
        let mut desk = Desked::start_with(store, None);
        desk.side.say("go");
        let chat = desk.prompted().await.chat.id;
        let _reads = desk.side.reads_held();
        desk.send_files(&chat, 0, FILES_AT_ONCE);
        until(|| (desk.side.reading().0 == FILES_AT_ONCE).then_some(())).await;
        // The disk fills, and the next message read is a save the core cannot make.
        failing.store(true, SeqCst);
        desk.side.say("and then");
        let side = desk.side.clone();
        let slow_disk = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            side.hold_reads(false);
        });
        let ended = tokio::time::timeout(Duration::from_secs(5), desk.running).await;
        let reading = desk.side.reading().0;
        desk.side.hold_reads(false);
        slow_disk.await.unwrap();
        let ran = ended.expect("the core ends once the reads have").unwrap();
        assert_eq!(
            ran,
            Err(UNSAVED.to_string()),
            "the save's error is the core's"
        );
        assert_eq!(reading, 0, "no read goes on after the core");
        assert_eq!(desk.side.offers(), 0, "no file read was offered to MDK");
    }

    // A read left going when a connector goes holds up no next connector: the core it drove
    // ends at once, the next is served while the read goes on, and the read, once it returns,
    // reaches no MDK. It still holds its permit until then, so the reads under way stay bounded
    // across connectors (D-145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_read_left_going_holds_up_no_next_connector() {
        let reads = Arc::new(tokio::sync::Semaphore::new(FILES_AT_ONCE));
        let mut desk = Desked::start_with(Store::new, Some(reads.clone()));
        desk.open.send(true).unwrap();
        desk.side.say("go");
        let chat = desk.prompted().await.chat.id;
        let _reads = desk.side.reads_held();
        desk.send_files(&chat, 0, 1);
        until(|| (desk.side.reading().0 == 1).then_some(())).await;
        // The connector goes.
        let (outputs, _) = mpsc::unbounded_channel();
        drop(std::mem::replace(&mut desk.outputs, outputs));
        let running = std::mem::replace(&mut desk.running, tokio::spawn(async { Ok(()) }));
        let ended = tokio::time::timeout(Duration::from_secs(2), running).await;
        if ended.is_err() {
            // Let the read go, so the failure is reported and does not hang the test.
            desk.side.hold_reads(false);
        }
        ended
            .expect("the core ends with the read still going")
            .unwrap()
            .unwrap();
        assert_eq!(desk.side.reading().0, 1, "the read goes on");
        let (mut desk, earlier) = desk.again(reads.clone());
        desk.side.say("still there?");
        let prompt = desk.prompted().await;
        assert_eq!(prompt.text, "still there?");
        assert_eq!(
            reads.available_permits(),
            FILES_AT_ONCE - 1,
            "the read holds its permit"
        );
        // The disk comes back: the read returns, and its file goes nowhere.
        desk.side.hold_reads(false);
        until(|| (desk.side.reading().0 == 0).then_some(())).await;
        until(|| (reads.available_permits() == FILES_AT_ONCE).then_some(())).await;
        assert_eq!(
            desk.side.offers(),
            0,
            "the file read was not offered to MDK"
        );
        let told = earlier.lock().unwrap().clone();
        assert!(
            matches!(told.as_slice(), [(_, Err(_))]),
            "the driver it was for heard it failed: {told:?}"
        );
        // The next connector's files go as ever.
        desk.send_files(&prompt.chat.id, 1, 1);
        let told = desk.told(1).await;
        assert!(told[0].1.is_ok(), "its file was sent: {told:?}");
        assert_eq!(desk.side.offers(), 1);
        let _ = desk.stop.send(());
        desk.running.await.unwrap().unwrap();
    }

    /// A connector on the socket, as the test plays it: it has given its hello.
    struct Connector {
        lines: tokio::io::Lines<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>,
        write: tokio::net::unix::OwnedWriteHalf,
    }

    impl Connector {
        /// A connector and the core's driver for it, greeted.
        async fn greeted() -> (Self, SocketDriver) {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            const TOKEN: &str = "0123456789abcdef0123456789abcdef";
            let (core, connector) = tokio::net::UnixStream::pair().unwrap();
            let (read, mut write) = connector.into_split();
            let hello = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "hello",
                "params": {"protocol": crate::socket::PROTOCOL, "token": TOKEN,
                "harness": {"name": "script"}, "connector": {"name": "script", "version": "0"},
                "capabilities": {"interrupt": false, "mid_turn": "queue", "sessions": "core",
                    "resume": false, "approvals": false, "files_in": "none",
                    "approval_commands": []}}});
            write
                .write_all(format!("{hello}\n").as_bytes())
                .await
                .unwrap();
            let driver = SocketDriver::greet(core, TOKEN, "agent").await.unwrap();
            let mut lines = tokio::io::BufReader::new(read).lines();
            lines
                .next_line()
                .await
                .unwrap()
                .expect("the core's answer to the hello");
            (Self { lines, write }, driver)
        }

        /// The chat of the next prompt the core sends, and the prompt's id.
        async fn prompted(&mut self) -> (String, serde_json::Value) {
            loop {
                let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
                    .await
                    .expect("a prompt in time")
                    .unwrap()
                    .expect("the connection is open");
                let message: serde_json::Value = serde_json::from_str(&line).unwrap();
                if message["method"] == "prompt" {
                    let chat = message["params"]["chat"]["id"].as_str().unwrap();
                    return (chat.into(), message["id"].clone());
                }
            }
        }

        /// The connector takes the request `id`.
        async fn take(&mut self, id: serde_json::Value) {
            use tokio::io::AsyncWriteExt;
            let taken = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}});
            self.write
                .write_all(format!("{taken}\n").as_bytes())
                .await
                .unwrap();
        }

        /// The connector asks the core to send `path` to `chat`, under its id `id`.
        async fn send_file(&mut self, id: i64, chat: &str, path: &std::path::Path) {
            use tokio::io::AsyncWriteExt;
            let ask = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "send_file",
                "params": {"chat": chat, "path": path}});
            self.write
                .write_all(format!("{ask}\n").as_bytes())
                .await
                .unwrap();
        }
    }

    // A connector that sends a file to send, and then more than the core holds for it while a
    // prompt waits, is given up; what it sent still reaches the core, but no file reaches MDK
    // once it is given up, not even one it asked for before: the chat is told that file was not
    // sent (D-145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_file_starts_after_a_connector_is_given_up() {
        use tokio::io::AsyncWriteExt;
        let (mut connector, driver) = Connector::greeted().await;
        let desk = OnDesk::start(driver);
        desk.open.send(true).unwrap();
        desk.side.say("go");
        // It never takes the prompt.
        let (chat, _) = connector.prompted().await;
        connector.send_file(2, &chat, &desk.notes()).await;
        let event = |chat: Option<&str>, text: String| {
            let event = serde_json::json!({"jsonrpc": "2.0", "method": "event", "params": {
                "chat": chat, "item": "i", "type": "notice", "state": "done", "title": "t",
                "text": text, "body": {"kind": "warning"}, "native": {"kind": "k", "payload": {}}}});
            format!("{event}\n")
        };
        // One for the chat, which the core sends there, and then the flood.
        let mut write = connector.write;
        let said = event(Some(&chat), "working".into());
        write.write_all(said.as_bytes()).await.unwrap();
        let line = event(None, "x".repeat(1024 * 1024));
        let flooding =
            tokio::spawn(async move { while write.write_all(line.as_bytes()).await.is_ok() {} });
        let side = desk.side.clone();
        desk.ended().await;
        flooding.await.unwrap();
        assert_eq!(side.reading().1, 0, "the file was not read");
        assert_eq!(side.offers(), 0, "nor offered to MDK");
        assert_eq!(side.not_sent(), 1, "the chat is told it was not sent");
    }

    // A connector the core gives up while it answers how a file went, since the connector stopped
    // reading, starts no upload of the files still waiting: those were never offered, and the
    // chat is told they were not sent (D-145).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_file_waiting_does_not_start_once_its_connector_is_given_up() {
        let (mut connector, driver) = Connector::greeted().await;
        let desk = OnDesk::start(driver);
        desk.side.say("go");
        let (chat, prompt) = connector.prompted().await;
        connector.take(prompt).await;
        let files = FILES_AT_ONCE as i64 + 1;
        for id in 2..2 + files {
            connector.send_file(id, &chat, &desk.notes()).await;
        }
        until(|| (desk.side.uploading().0 == FILES_AT_ONCE).then_some(())).await;
        // It reads nothing more, so the core cannot tell it how its files went.
        let Connector { lines, write } = connector;
        let read = lines.into_inner().into_inner();
        let stream = read.reunite(write).unwrap().into_std().unwrap();
        stream.shutdown(std::net::Shutdown::Read).unwrap();
        desk.open.send(true).unwrap();
        let side = desk.side.clone();
        desk.ended().await;
        drop(stream);
        assert_eq!(
            side.reading().1,
            FILES_AT_ONCE,
            "only the first files were read"
        );
        assert_eq!(side.offers(), FILES_AT_ONCE, "and offered");
        assert_eq!(side.not_sent(), 1, "the chat is told the last was not sent");
    }

    /// A driver that takes every control at once, and each prompt but `go` at once too, which it
    /// takes only after `slow`. It hands the core what the test gives it, and counts the files
    /// among them it handed over. It says each output reached it at `at`, if given.
    struct Slowed {
        outputs: mpsc::UnboundedReceiver<Output>,
        slow: Duration,
        /// Each request it takes, by its kind, and the chat of each prompt.
        seen: mpsc::UnboundedSender<(&'static str, Option<marmot_agent_driver::ChatId>)>,
        handed: Arc<Mutex<usize>>,
        told: Told,
        at: Option<std::time::Instant>,
    }

    impl Driver for Slowed {
        fn harness(&self) -> marmot_agent_driver::Harness {
            marmot_agent_driver::Harness {
                name: "slowed".into(),
                version: None,
            }
        }

        fn capabilities(&self) -> marmot_agent_driver::Capabilities {
            marmot_agent_driver::Capabilities {
                interrupt: false,
                mid_turn: marmot_agent_driver::MidTurn::Queue,
                sessions: marmot_agent_driver::Sessions::Core,
                resume: false,
                approvals: false,
                files_in: marmot_agent_driver::FilesIn::None,
                approval_commands: Vec::new(),
                subagents: false,
                stop_subagent: false,
            }
        }

        async fn prompt(&mut self, prompt: marmot_agent_driver::Prompt) -> Result<(), Refusal> {
            let slow = prompt.text == "go";
            let kind = if slow { "slow" } else { "prompt" };
            let _ = self.seen.send((kind, Some(prompt.chat.id)));
            if slow {
                tokio::time::sleep(self.slow).await;
            }
            Ok(())
        }

        async fn answer(&mut self, _answer: marmot_agent_driver::Answer) -> Result<(), Refusal> {
            unreachable!("no answer")
        }

        async fn control(&mut self, _control: marmot_agent_driver::Control) -> Result<(), Refusal> {
            let _ = self.seen.send(("control", None));
            Ok(())
        }

        async fn next_event(&mut self) -> Option<Output> {
            let output = self.outputs.recv().await;
            if let Some(Output::SendFile(_)) = &output {
                *self.handed.lock().unwrap() += 1;
            }
            output
        }

        async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
            self.told.lock().unwrap().push((id, result));
        }

        fn arrived(&self) -> Option<std::time::Instant> {
            self.at
        }
    }

    // Review of #198: when a control was overdue, the `no_effect` net took what the driver had
    // ready, to read before it settled, and a stop that came then dropped the files among them:
    // the driver was never told how they went, nor the chat that they were not sent (D-145).
    // Here a control waits past its deadline while a prompt holds the core, the agent asks to
    // send files meanwhile, and the core is asked to stop. The prompts before shift where in the
    // loop's turn the core comes out of the slow prompt, so the tick, the driver's files and the
    // stop meet in each order. Every file the driver handed over is answered.
    #[tokio::test(start_paused = true)]
    async fn a_stop_as_the_net_looks_answers_every_file_handed_over() {
        let asked = FILES_AT_ONCE + 3;
        for before in 0..2 * WAITS {
            let (outputs, outputs_in) = mpsc::unbounded_channel();
            let (seen_out, mut seen) = mpsc::unbounded_channel();
            let (handed, told) = (Arc::new(Mutex::new(0)), Told::default());
            let driver = Slowed {
                outputs: outputs_in,
                slow: Duration::from_secs(150),
                seen: seen_out,
                handed: handed.clone(),
                told: told.clone(),
                at: None,
            };
            let desk = OnDesk::start(driver);
            for n in 0..before {
                desk.side.say(&format!("hi {n}"));
                assert_eq!(seen.recv().await.unwrap().0, "prompt");
            }
            desk.side.say("/new");
            assert_eq!(seen.recv().await.unwrap().0, "control");
            desk.side.say("go");
            let (kind, chat) = seen.recv().await.unwrap();
            assert_eq!(kind, "slow");
            for n in 0..asked {
                let file = SendFile {
                    id: format!("f{n}"),
                    chat: chat.clone().unwrap(),
                    path: desk.notes(),
                };
                outputs.send(Output::SendFile(file)).unwrap();
            }
            let _ = desk.stop.send(());
            desk.running.await.unwrap().unwrap();
            let handed = *handed.lock().unwrap();
            let told = told.lock().unwrap().clone();
            assert_eq!(
                told.len(),
                handed,
                "{before} before: each file handed over is answered: {told:?}"
            );
            assert!(told.iter().all(|(_, r)| r.is_err()), "none was sent");
            assert_eq!(
                desk.side.not_sent(),
                handed - desk.side.offers(),
                "{before} before: the chat hears of each file never offered"
            );
            drop(outputs);
        }
    }

    /// The agent's words `n` in `chat`, as a driver hands them over.
    fn words(chat: marmot_agent_driver::ChatId, n: u64) -> Output {
        let header = marmot_agent_events::Header {
            harness: "test".into(),
            session: Some("s1".into()),
            turn: None,
            subagent: None,
            item: format!("w{n}").into(),
            seq: n,
            state: marmot_agent_events::State::Done,
            title: "words".into(),
            text: format!("word {n}"),
        };
        let body = marmot_agent_events::Body::Message(Default::default());
        Output::Event(marmot_agent_driver::Emitted {
            event: marmot_agent_events::AgentEvent::new(header, body, None).unwrap(),
            chat: Some(chat),
            prompts: Vec::new(),
            settles: Vec::new(),
        })
    }

    // Review of #198: on a stop, the core reads the output the net held to look at, and a save that
    // failed then returned at once, before the files were ended, so a file being read went on after
    // the core (D-145). Here a file is being read, a control waits past its deadline while a prompt
    // holds the core, the agent says something meanwhile, which reached the driver in time, the
    // disk fills, and the core is asked to stop. The prompts before shift where in the loop's turn
    // the core comes out of the slow prompt, as above. However it ends, no read goes on after it.
    #[tokio::test(start_paused = true)]
    async fn a_failed_save_of_what_the_net_held_as_the_core_stops_still_ends_every_read() {
        for before in 1..=2 * WAITS {
            // The disk fills as the agent's words are kept: only taking them fails.
            let words_kept = |kept: &Kept| serde_json::to_string(kept).unwrap().contains("\"w1\"");
            let store = move |dir: &Path| Store::failing_when(dir, words_kept).0;
            let (outputs, outputs_in) = mpsc::unbounded_channel();
            let (seen_out, mut seen) = mpsc::unbounded_channel();
            let driver = Slowed {
                outputs: outputs_in,
                slow: Duration::from_secs(150),
                seen: seen_out,
                handed: Arc::default(),
                told: Told::default(),
                at: Some(Instant::now().into_std()),
            };
            let desk = OnDesk::start_with(driver, store, None);
            let reads = desk.side.reads_held();
            desk.side.say("hi 0");
            let (kind, chat) = seen.recv().await.unwrap();
            assert_eq!(kind, "prompt");
            let chat = chat.unwrap();
            let file = SendFile {
                id: "f0".into(),
                chat: chat.clone(),
                path: desk.notes(),
            };
            outputs.send(Output::SendFile(file)).unwrap();
            // The read runs on a thread of its own, which the paused clock does not wait for.
            for _ in 0..5_000 {
                if desk.side.reading().0 == 1 {
                    break;
                }
                tokio::task::yield_now().await;
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(desk.side.reading().0, 1, "the file is being read");
            for n in 1..before {
                desk.side.say(&format!("hi {n}"));
                assert_eq!(seen.recv().await.unwrap().0, "prompt");
            }
            desk.side.say("/new");
            assert_eq!(seen.recv().await.unwrap().0, "control");
            desk.side.say("go");
            assert_eq!(seen.recv().await.unwrap().0, "slow");
            // The net takes the second to look at once the core has read the first.
            outputs.send(words(chat.clone(), 2)).unwrap();
            outputs.send(words(chat, 1)).unwrap();
            let _ = desk.stop.send(());
            // A read under way keeps the paused clock from moving on by itself.
            let mut running = desk.running;
            for _ in 0..400 {
                if running.is_finished() {
                    break;
                }
                tokio::time::advance(Duration::from_secs(1)).await;
            }
            let (ended, reading) = (running.is_finished(), desk.side.reading().0);
            drop(reads);
            let ran = (&mut running).await.unwrap();
            assert!(
                !ended || reading == 0,
                "{before} before: a read went on after the core"
            );
            assert!(
                ran.is_ok() || ran == Err(UNSAVED.to_string()),
                "{before} before: {ran:?}"
            );
            drop(outputs);
        }
    }

    /// A driver that takes every control at once, and, once a control names a chat, always has
    /// the agent's words for it ready, so it is never idle, until `ends`.
    struct Chatty {
        chat: Option<marmot_agent_driver::ChatId>,
        said: u64,
        ends: Instant,
    }

    impl Driver for Chatty {
        fn harness(&self) -> marmot_agent_driver::Harness {
            marmot_agent_driver::Harness {
                name: "chatty".into(),
                version: None,
            }
        }

        fn capabilities(&self) -> marmot_agent_driver::Capabilities {
            marmot_agent_driver::Capabilities {
                interrupt: false,
                mid_turn: marmot_agent_driver::MidTurn::Queue,
                sessions: marmot_agent_driver::Sessions::Core,
                resume: false,
                approvals: false,
                files_in: marmot_agent_driver::FilesIn::None,
                approval_commands: Vec::new(),
                subagents: false,
                stop_subagent: false,
            }
        }

        async fn prompt(&mut self, _prompt: marmot_agent_driver::Prompt) -> Result<(), Refusal> {
            Ok(())
        }

        async fn answer(&mut self, _answer: marmot_agent_driver::Answer) -> Result<(), Refusal> {
            unreachable!("no answer")
        }

        async fn control(&mut self, control: marmot_agent_driver::Control) -> Result<(), Refusal> {
            if let marmot_agent_driver::Action::NewSession { chat, .. } = control.action {
                self.chat = Some(chat.id);
            }
            Ok(())
        }

        async fn next_event(&mut self) -> Option<Output> {
            let Some(chat) = self.chat.clone() else {
                return std::future::pending().await;
            };
            if Instant::now() >= self.ends {
                return None;
            }
            self.said += 1;
            Some(words(chat, self.said))
        }
    }

    // Review of #198: a driver that is never idle, whose output keeps reaching the core after a
    // control's deadline, does not hold off the `no_effect` net (D-087, L1): what reached the core
    // after the deadline is not waited for. Here the driver talks for 200 s after it takes a
    // control whose effect it never shows, and has words ready each time the core looks, while
    // the clock moves on two seconds each time the core lets the test run.
    #[tokio::test(start_paused = true)]
    async fn a_driver_never_idle_after_the_deadline_does_not_hold_off_the_net() {
        let driver = Chatty {
            chat: None,
            said: 0,
            ends: Instant::now() + Duration::from_secs(200),
        };
        let desk = OnDesk::start(driver);
        desk.side.say("/new");
        let side = desk.side.clone();
        let mut running = desk.running;
        loop {
            tokio::select! {
                ran = &mut running => break ran.unwrap().unwrap(),
                () = tokio::time::advance(Duration::from_secs(2)) => {}
            }
        }
        let notices = side.notices.lock().unwrap().clone();
        assert!(
            notices.iter().any(|n| n.starts_with("The agent took this")),
            "the net fired while the driver talked: {notices:?}"
        );
    }
}
