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
//! What the core keeps across a restart is one file, written after every change: the
//! conversation's state, and for each group how far it has read, in the order messages arrived
//! on this machine. A message is marked read before it reaches the driver, so a crash never hands
//! the driver the same prompt twice (D-037). MDK does not replay what arrived while a program was
//! down, so on start, and whenever it falls behind, the core reads what was stored after that
//! mark.

use crate::config::Config;
use crate::conversation::{Act, Conversation, Saved, Sending};
use crate::files::Folders;
use crate::keys::{EventKey, GroupKey};
use crate::marmot::{Incoming, Marmot, Message};
use crate::socket::SocketDriver;
use marmot_agent_driver::{Driver, Output, Reason, Refusal, SendFile};
use marmot_app::{AppError, MarmotAppEvent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

/// How many times a send that failed for any reason but MDK being busy is tried again.
const SEND_TRIES: u32 = 5;

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

/// Where the kept state lives.
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// The state file in `dir`.
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("marmot-agent-state.json"),
        }
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

/// The core: MDK, a driver, and the conversation.
pub struct Core<D: Driver> {
    marmot: Marmot,
    driver: D,
    conversation: Conversation,
    read: BTreeMap<GroupKey, i64>,
    store: Store,
    config: Config,
    /// The folders the agent may send files from (D-090).
    folders: Folders,
    /// The core's clock, which never jumps, for how long a control has waited (D-087).
    started: std::time::Instant,
}

impl<D: Driver> Core<D> {
    /// A core for the agent on `marmot`, driving `driver`, with the settings in `config`, keeping
    /// its state in `store`.
    pub fn new(marmot: Marmot, driver: D, config: &Config, store: Store) -> Result<Self, String> {
        // The working folder, the default, is the person's to choose; a listed folder is
        // checked, since an import or a hand may have put one in the wrong place.
        if let Some(listed) = &config.files_from {
            crate::files::apart_from_home(listed, &config.home)?;
        }
        let kept = store.load()?;
        let conversation = Conversation::new(
            marmot.agent.clone(),
            driver.harness().name,
            config.rights(),
            driver.capabilities(),
            kept.conversation,
        );
        Ok(Self {
            marmot,
            driver,
            conversation,
            read: kept.read,
            store,
            folders: Folders::new(&config.file_folders()),
            config: config.clone(),
            started: std::time::Instant::now(),
        })
    }

    /// Runs until the driver ends or `stop` resolves. MDK's runtime must already have been
    /// subscribed to (`events`) and started, so nothing that arrives during start-up is missed.
    pub async fn run(
        mut self,
        mut events: tokio::sync::broadcast::Receiver<MarmotAppEvent>,
        stop: impl std::future::Future<Output = ()>,
    ) {
        self.publish_name().await;
        let recovered = self.conversation.recover();
        self.carry_out_whole(recovered).await;
        self.consider_invitations().await;
        self.catch_up().await;
        tokio::pin!(stop);
        let mut clock = tokio::time::interval(std::time::Duration::from_secs(10));
        // Files being uploaded say here how they went. A file still uploading when the core
        // stops is not waited for: the driver it would answer goes with the core.
        let (uploaded, mut uploads) = mpsc::unbounded_channel::<Uploaded>();
        loop {
            tokio::select! {
                _ = clock.tick() => loop {
                    let acts = self.conversation.overdue(self.clock());
                    if acts.is_empty() {
                        break;
                    }
                    self.carry_out(acts).await;
                },
                event = self.driver.next_event() => match event {
                    Some(Output::Event(emitted)) => {
                        let acts = self.conversation.emitted(emitted);
                        self.carry_out(acts).await;
                    }
                    Some(Output::SendFile(file)) => self.send_file(file, &uploaded).await,
                    None => break,
                },
                Some(done) = uploads.recv() => self.file_done(done).await,
                event = events.recv() => match event {
                    Ok(event) => self.on_marmot(event).await,
                    Err(RecvError::Lagged(_)) => self.catch_up().await,
                    Err(RecvError::Closed) => break,
                },
                () = &mut stop => break,
            }
        }
        let ended = self.conversation.ended();
        self.carry_out_whole(ended).await;
    }

    /// Starts sending a file the agent asked to send, as a task of its own so the chats go on
    /// meanwhile. Where it goes, the chat's group and running turn, is taken now (D-090).
    async fn send_file(&mut self, file: SendFile, uploaded: &mpsc::UnboundedSender<Uploaded>) {
        let SendFile { id, chat, path } = file;
        let Some((group, turn)) = self.conversation.file_place(&chat) else {
            let result = Err(Refusal {
                reason: Reason::Invalid,
                detail: Some("The agent named a chat it is not in.".into()),
            });
            self.file_done(Uploaded { id, chat, result }).await;
            return;
        };
        let (marmot, folders, uploaded) =
            (self.marmot.clone(), self.folders.clone(), uploaded.clone());
        tokio::spawn(async move {
            let result = upload(&marmot, folders, path, &group, turn).await;
            let _ = uploaded.send(Uploaded { id, chat, result });
        });
    }

    /// Tells the driver how a file went. A refusal is shown in the chat, naming no path (D-090).
    async fn file_done(&mut self, done: Uploaded) {
        if let Err(refusal) = &done.result {
            // Its kind only: a path or a file name is never logged (D-071).
            eprintln!("marmot-agent: a file was not sent ({:?})", refusal.reason);
            let acts = self.conversation.file_refused(&done.chat, refusal);
            self.carry_out(acts).await;
        }
        self.driver.file_sent(done.id, done.result).await;
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
        let relays = self
            .config
            .relays
            .iter()
            .map(|r| cgka_traits::TransportEndpoint(r.clone()))
            .collect();
        let current = self
            .marmot
            .runtime
            .fetch_current_user_profile_for_account_id(self.marmot.agent.as_str(), relays)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        if current.name.as_deref() == Some(&name) && current.display_name.as_deref() == Some(&name)
        {
            return;
        }
        let profile = marmot_app::UserProfileMetadata {
            name: Some(name.clone()),
            display_name: Some(name),
            ..current
        };
        let published = self
            .marmot
            .runtime
            .publish_user_profile_using_account_relays(&self.marmot.label, profile)
            .await;
        if published.is_err() {
            eprintln!("marmot-agent: the agent's name could not be published");
        }
    }

    async fn on_marmot(&mut self, event: MarmotAppEvent) {
        match event {
            MarmotAppEvent::MessageReceived(received)
                if received.account_id_hex == self.marmot.agent.as_str() =>
            {
                self.handle(Message::from(received.message)).await;
            }
            MarmotAppEvent::GroupJoined { account_id_hex, .. }
            | MarmotAppEvent::GroupEvent(marmot_app::RuntimeGroupEvent {
                account_id_hex, ..
            }) if account_id_hex == self.marmot.agent.as_str() => {
                self.consider_invitations().await;
            }
            _ => {}
        }
    }

    /// Accepts an invitation from someone who may add the agent, and declines the rest. A group
    /// the agent has just joined is read from its start: the agent cannot read anything sent
    /// before it joined, so everything stored is for it.
    async fn consider_invitations(&mut self) {
        let mut joined = false;
        for (group, welcomer) in self.marmot.invitations() {
            let allowed = welcomer.as_ref().is_some_and(|w| self.config.may_invite(w));
            if self.marmot.decide(&group, allowed).await.is_ok() && allowed {
                self.read.insert(group, 0);
                joined = true;
            }
        }
        if joined {
            self.save();
            self.catch_up().await;
        }
    }

    /// Reads what was stored for each group after its mark: what arrived while the core was down,
    /// or while it fell behind. A group the core has never seen starts at its latest message.
    async fn catch_up(&mut self) {
        for group in self.marmot.groups() {
            let after = match self.read.get(&group) {
                Some(&after) => after,
                None => {
                    let latest = self.marmot.latest(&group);
                    self.read.insert(group, latest);
                    self.save();
                    continue;
                }
            };
            for message in self.marmot.stored(&group, after) {
                self.handle(message).await;
            }
        }
    }

    /// Hands one message to the conversation, once: it is marked read, and the mark saved, before
    /// anything is done with it.
    async fn handle(&mut self, message: Message) {
        let Some(order) = self.marmot.order_of(&message) else {
            return;
        };
        let group = message.group.clone();
        if self.read.get(&group).is_some_and(|&read| order <= read) {
            return;
        }
        self.read.insert(group, order);
        self.save();
        let acts = match self.marmot.incoming(message).await {
            Some(Incoming::Heard(heard)) => self.conversation.heard(heard),
            Some(Incoming::Asked(asked)) => self.conversation.asked(asked),
            None => Vec::new(),
        };
        self.carry_out(acts).await;
    }

    /// Carries out acts one at a time, and those that follow from each, saving after each.
    async fn carry_out(&mut self, acts: Vec<Act>) {
        let mut queue: VecDeque<Act> = acts.into();
        while let Some(act) = queue.pop_front() {
            let next = self.carry_out_one(act).await;
            queue.extend(next);
            self.save();
        }
    }

    /// Carries out acts that belong together, such as closing everything a stop left open, and
    /// saves only once all of them are done: a crash in between closes them all again on the next
    /// start, rather than leaving some open for good.
    async fn carry_out_whole(&mut self, acts: Vec<Act>) {
        let mut queue: VecDeque<Act> = acts.into();
        while let Some(act) = queue.pop_front() {
            let next = self.carry_out_one(act).await;
            queue.extend(next);
        }
        self.save();
    }

    async fn carry_out_one(&mut self, act: Act) -> Vec<Act> {
        match act {
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
            Act::Send { group, what } => {
                let key = match &what {
                    Sending::Item { key, .. } | Sending::Words { key, .. } => key.clone(),
                };
                if let Some(event) = self.send(&group, &what).await {
                    self.conversation.sent(&key, event);
                }
                Vec::new()
            }
        }
    }

    /// Sends, trying again for as long as MDK is busy, and a few times for anything else. MDK
    /// keeps a send in the group's queue when no relay takes it, so any other failure is one that
    /// trying again will not mend; that send is given up.
    async fn send(&mut self, group: &GroupKey, what: &Sending) -> Option<EventKey> {
        let mut tries = 0;
        let mut wait = Duration::from_millis(100);
        loop {
            match self.marmot.send(group, what).await {
                Ok(event) => return Some(event),
                Err(AppError::AccountWorkerBusy) => {}
                Err(_) => {
                    tries += 1;
                    // MDK's own words can name a group, so they are not printed (D-071).
                    eprintln!("marmot-agent: a send failed, try {tries}");
                    if tries == SEND_TRIES {
                        return None;
                    }
                }
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(10));
        }
    }

    fn save(&self) {
        let kept = Kept {
            conversation: self.conversation.saved().clone(),
            read: self.read.clone(),
        };
        if self.store.save(&kept).is_err() {
            eprintln!("marmot-agent: the state file could not be written");
        }
    }
}

/// Reads a file from the allowed folders and sends it to `group`. It is offered to MDK again only
/// while MDK says it is busy, which MDK says only before it has taken the file. Any other failure
/// may have come after MDK took it, such as a wait for MDK that ran out while the send went on, so
/// it is not tried again: that could post the file twice, and a post cannot be taken back.
async fn upload(
    marmot: &Marmot,
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
            Err(AppError::AccountWorkerBusy) => {}
            Err(_) => {
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
            () = &mut running => {}
            () = &mut stop => {
                let _ = stopping.send(true);
                running.await;
                return Ok(());
            }
        }
    }
}
