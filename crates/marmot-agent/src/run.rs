//! The core running: MDK on one side, a driver on the other, and the conversation between them.
//!
//! One task does everything, so the conversation is never touched from two places. It waits for
//! whichever comes first, a group message or a driver event, hands it to the conversation, and
//! carries out the acts that come back one at a time, telling the conversation how each went. A
//! driver request is always awaited to the end (D-066); only the wait for the driver's next event
//! is dropped when a group message comes first, which the trait allows (D-058).
//!
//! What the core keeps across a restart is written to one file after every change: the
//! conversation's state, and for each group the last message handled, so that what arrived while
//! the core was down is handled when it starts again.

use crate::config::Config;
use crate::conversation::{Act, Conversation, Saved};
use crate::keys::{EventKey, GroupKey, PubKey};
use crate::marmot::{Incoming, Marmot, Message};
use marmot_agent_driver::Driver;
use marmot_app::MarmotAppEvent;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use tokio::sync::broadcast::error::RecvError;

/// How many times a failed send is tried again before it is given up.
const SEND_TRIES: u32 = 5;
/// How many handled messages are remembered across a restart, so that catching up skips them.
const REMEMBERED: usize = 1024;

/// What the core keeps across a restart.
#[derive(Default, Serialize, Deserialize)]
pub struct Kept {
    /// The conversation's state.
    pub conversation: Saved,
    /// For each group, when the last message handled was sent, in seconds.
    pub handled: BTreeMap<GroupKey, u64>,
    /// The most recent messages handled, oldest first, so a message is handled once even when a
    /// catch-up after a restart reads it again.
    #[serde(default)]
    pub recent: VecDeque<EventKey>,
}

/// Where the kept state lives, and writing it.
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

    /// What was kept, or nothing on a first start.
    pub fn load(&self) -> Kept {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the state, whole, so a crash leaves the old or the new file and never half of one.
    pub fn save(&self, kept: &Kept) {
        let Ok(bytes) = serde_json::to_vec(kept) else {
            return;
        };
        let temporary = self.path.with_extension("json.new");
        if std::fs::write(&temporary, bytes).is_ok() {
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

/// The core: MDK, a driver, and the conversation.
pub struct Core<D: Driver> {
    marmot: Marmot,
    driver: D,
    conversation: Conversation,
    handled: BTreeMap<GroupKey, u64>,
    recent: VecDeque<EventKey>,
    store: Store,
    /// Who may add the agent to a group.
    inviters: Vec<PubKey>,
}

impl<D: Driver> Core<D> {
    /// A core for the agent on `marmot`, driving `driver`, with the settings in `config`, keeping
    /// its state in `store`.
    pub fn new(marmot: Marmot, driver: D, config: &Config, store: Store) -> Self {
        let kept = store.load();
        let conversation = Conversation::new(
            marmot.agent.clone(),
            driver.harness().name,
            config.rights(),
            driver.capabilities(),
            kept.conversation,
        );
        let mut inviters = config.invite.clone();
        inviters.push(config.owner.clone());
        Self {
            marmot,
            driver,
            conversation,
            handled: kept.handled,
            recent: kept.recent,
            store,
            inviters,
        }
    }

    /// Runs until the driver ends or `stop` resolves. MDK's runtime must already have been
    /// subscribed to (`events`) and started, so nothing that arrives during start-up is missed.
    pub async fn run(
        mut self,
        mut events: tokio::sync::broadcast::Receiver<MarmotAppEvent>,
        stop: impl std::future::Future<Output = ()>,
    ) {
        let recovered = self.conversation.recover();
        self.carry_out(recovered).await;
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
        self.carry_out(ended).await;
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
    /// joined from now on is handled from now on: its history is not read as prompts.
    async fn consider_invitations(&mut self) {
        for (group, welcomer) in self.marmot.invitations() {
            let allowed = welcomer.as_ref().is_some_and(|w| self.inviters.contains(w));
            if self.marmot.decide(&group, allowed).await.is_ok() && allowed {
                self.handled.insert(group, crate::marmot::now());
                self.save();
            }
        }
    }

    /// Handles what was stored for each group after the last message handled: what arrived while
    /// the core was down, or while it fell behind. A group seen for the first time starts now.
    async fn catch_up(&mut self) {
        for group in self.marmot.groups() {
            let Some(&after) = self.handled.get(&group) else {
                self.handled.insert(group, crate::marmot::now());
                continue;
            };
            let missed: Vec<Message> = self
                .marmot
                .stored(&group)
                .into_iter()
                .filter(|m| m.recorded_at >= after && !self.recent.contains(&m.event))
                .collect();
            for message in missed {
                self.handle(message).await;
            }
        }
        self.save();
    }

    async fn handle(&mut self, message: Message) {
        if self.recent.contains(&message.event) {
            return;
        }
        self.recent.push_back(message.event.clone());
        if self.recent.len() > REMEMBERED {
            self.recent.pop_front();
        }
        let group = message.group.clone();
        let sent_at = message.recorded_at;
        let acts = match self.marmot.incoming(message).await {
            Some(Incoming::Heard(heard)) => self.conversation.heard(heard),
            Some(Incoming::Asked(asked)) => self.conversation.asked(asked),
            None => Vec::new(),
        };
        if self.handled.get(&group).is_none_or(|old| sent_at >= *old) {
            self.handled.insert(group, sent_at);
        }
        self.carry_out(acts).await;
    }

    /// Carries out acts one at a time, and those that follow from each.
    async fn carry_out(&mut self, acts: Vec<Act>) {
        let mut queue: VecDeque<Act> = acts.into();
        while let Some(act) = queue.pop_front() {
            let next = match act {
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
                        crate::conversation::Sending::Item { key, .. }
                        | crate::conversation::Sending::Words { key, .. } => key.clone(),
                    };
                    if let Some(event) = self.send(&group, &what).await {
                        self.conversation.sent(&key, event);
                    }
                    Vec::new()
                }
            };
            queue.extend(next);
            self.save();
        }
    }

    /// Sends, trying again a few times. MDK holds a send in the group's queue when no relay takes
    /// it, so a failure here is rare.
    async fn send(
        &self,
        group: &GroupKey,
        what: &crate::conversation::Sending,
    ) -> Option<EventKey> {
        for attempt in 0..SEND_TRIES {
            match self.marmot.send(group, what).await {
                Ok(event) => return Some(event),
                Err(_) => {
                    // MDK's own words can name a group, so they are not printed (D-071).
                    eprintln!("marmot-agent: a send failed, try {}", attempt + 1);
                    tokio::time::sleep(std::time::Duration::from_millis(200 << attempt)).await;
                }
            }
        }
        None
    }

    fn save(&self) {
        self.store.save(&Kept {
            conversation: self.conversation.saved().clone(),
            handled: self.handled.clone(),
            recent: self.recent.clone(),
        });
    }
}
