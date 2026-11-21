//! The core running: MDK on one side, a driver on the other, and the conversation between them.
//!
//! One task does everything, so the conversation is never touched from two places. It waits for
//! whichever comes first, a group message or a driver event, hands it to the conversation, and
//! carries out the acts that come back one at a time, telling the conversation how each went. A
//! driver request is always awaited to the end (D-066); only the wait for the driver's next event
//! is dropped when a group message comes first, which the trait allows (D-058).
//!
//! What the core keeps across a restart is one file, written after every change: the
//! conversation's state, and for each group how far it has read, in the order messages arrived
//! on this machine. A message is marked read before it reaches the driver, so a crash never hands
//! the driver the same prompt twice (D-037). MDK does not replay what arrived while a program was
//! down, so on start, and whenever it falls behind, the core reads what was stored after that
//! mark.

use crate::config::Config;
use crate::conversation::{Act, Conversation, Saved, Sending};
use crate::keys::{EventKey, GroupKey};
use crate::marmot::{Incoming, Marmot, Message};
use marmot_agent_driver::Driver;
use marmot_app::{AppError, MarmotAppEvent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// How many times a send that failed for any reason but MDK being busy is tried again.
const SEND_TRIES: u32 = 5;

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
}

impl<D: Driver> Core<D> {
    /// A core for the agent on `marmot`, driving `driver`, with the settings in `config`, keeping
    /// its state in `store`.
    pub fn new(marmot: Marmot, driver: D, config: &Config, store: Store) -> Result<Self, String> {
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
            config: config.clone(),
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
        loop {
            tokio::select! {
                event = self.driver.next_event() => match event {
                    Some(emitted) => {
                        let acts = self.conversation.emitted(emitted);
                        self.carry_out(acts).await;
                    }
                    None => break,
                },
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

    /// Publishes the agent's name from the settings as its profile, so people see it.
    async fn publish_name(&mut self) {
        let Some(name) = self.config.name.clone() else {
            return;
        };
        let profile = marmot_app::UserProfileMetadata {
            name: Some(name.clone()),
            display_name: Some(name),
            ..marmot_app::UserProfileMetadata::default()
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
                self.conversation.requested(&request, result)
            }
            Act::Control(control) => {
                let request = control.request.clone();
                let result = self.driver.control(control).await;
                self.conversation.requested(&request, result)
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
