//! The Marmot side: what arrives in the agent's groups, turned into what the conversation takes,
//! and what the conversation sends, handed to MDK (D-002).
//!
//! An agent item goes out as kind 1201 and its later versions as MDK's edits of it, and an answer
//! or a control comes in as kind 1202, as [`carrier`] writes and reads them (D-076). The agent's
//! words are chat messages carrying their turn (D-022).

use crate::activity::{FullActivity, owners_chat};
use crate::carrier::{self, REQUEST_KIND};
use crate::config::Config;
use crate::conversation::{Heard, HeardQuote, HeardRequest, Sending, Token, Where};
use crate::files::{Folders, Outgoing};
use crate::groups::Listed;
use crate::keys::{EventKey, GroupKey, PubKey};
use cgka_traits::GroupId;
use marmot_account::{AccountHome, AccountSummary};
use marmot_agent_driver::Refusal;
use marmot_agent_events::TurnId;
use marmot_app::{
    AppError, AppMessageQuery, AppMessageRecord, LocalSendStatus, MarmotApp, MarmotAppConfig,
    MarmotAppEvent, MarmotAppRuntime, MediaUploadAttachmentRequest, MediaUploadRequest,
};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The kind of a chat message.
const CHAT: u64 = 9;

/// Opens the account home the settings name: its accounts, and MDK's app and runtime on it.
/// This is what first opens the store, which MDK upgrades forward only. MDK holds the home for as
/// long as the app or a runtime made from it lives, so a second core on the same home is refused
/// here, and the two never write over each other's state, which lives in the same home.
pub fn open(config: &Config) -> Result<(AccountHome, MarmotApp, MarmotAppRuntime), String> {
    start_sqlite();
    let home = AccountHome::open(&config.home);
    let base = if config.white_noise {
        MarmotAppConfig::white_noise()
    } else {
        MarmotAppConfig::default()
    };
    let settings = base.with_allow_loopback_relay_endpoints(config.allow_loopback_relays);
    let app = MarmotApp::try_with_relays_and_account_home_and_config(
        &config.home,
        config.relays.clone(),
        home.clone(),
        settings,
    )
    .map_err(|_| "the account home could not be opened; is another core running on it?")?;
    let runtime = MarmotAppRuntime::new(app.clone());
    Ok((home, app, runtime))
}

/// Starts SQLite, and SQLCipher with it, on this thread, once in the process. Call it before
/// anything in the process opens a database through MDK.
///
/// SQLite marks itself started before SQLCipher's own start has run, outside any lock. A second
/// thread that opens and keys a database in that moment finds SQLCipher not started, and MDK
/// reports "An error occurred with PRAGMA key or rekey. PRAGMA key requires a key of one or more
/// characters" (SQLCipher 4.17, issue #96). So only the first databases a process opens, from
/// several threads at once, can fail, and more often the busier the machine. One connection
/// opened here first, behind a `Once`, runs both starts to the end before any other thread can
/// open one.
pub fn start_sqlite() {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        // A database in memory needs no file and no key. Should it fail, MDK's own open reports
        // why.
        let _ = rusqlite::Connection::open_in_memory();
    });
}

/// The home's one identity: one agent identity per core (D-072).
pub fn identity(home: &AccountHome) -> Result<AccountSummary, String> {
    let accounts = home
        .accounts()
        .map_err(|_| "the account home could not be read")?;
    let mut signing = accounts.into_iter().filter(|a| a.local_signing);
    match (signing.next(), signing.next()) {
        (Some(account), None) => Ok(account),
        _ => Err("the home must hold exactly one identity; run `marmot-agent init` first".into()),
    }
}

/// One message as the Marmot side gets it, live or from storage.
#[derive(Clone)]
pub struct Message {
    /// The group.
    pub group: GroupKey,
    /// The event's id.
    pub event: EventKey,
    /// Who sent it.
    pub sender: PubKey,
    /// Their name, when known.
    pub sender_name: Option<String>,
    /// Its kind.
    pub kind: u64,
    /// Its content.
    pub content: String,
    /// Its tags.
    pub tags: Vec<Vec<String>>,
    /// When the sender says it was sent, in seconds.
    pub recorded_at: u64,
    /// When it arrived here, by this machine's clock, in whole seconds: when MDK received it from
    /// a relay, which may be before it was stored and long before it is announced live.
    pub received_at: u64,
    /// Its place in this machine's storage, which only grows: the order messages arrived in here,
    /// whatever their senders' clocks say. Known for a stored message; a live one has none, since
    /// it only wakes its group's reader, which reads it from storage.
    pub order: Option<i64>,
}

/// Prints the kind and nothing else (D-071).
impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Message")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl From<AppMessageRecord> for Message {
    fn from(record: AppMessageRecord) -> Self {
        Self {
            group: GroupKey(record.group_id_hex),
            event: EventKey(record.message_id_hex),
            sender: PubKey(record.sender),
            sender_name: None,
            kind: record.kind,
            content: record.plaintext,
            tags: record.tags,
            recorded_at: record.recorded_at,
            received_at: record.received_at,
            order: Some(record.insert_order),
        }
    }
}

impl From<marmot_app::ReceivedMessage> for Message {
    fn from(message: marmot_app::ReceivedMessage) -> Self {
        Self {
            group: GroupKey(hex::encode(message.group_id.as_slice())),
            event: EventKey(message.message_id_hex),
            sender: PubKey(message.sender),
            sender_name: message.sender_display_name,
            kind: message.kind,
            content: message.plaintext,
            tags: message.tags,
            recorded_at: message.recorded_at,
            received_at: message.received_at,
            order: None,
        }
    }
}

/// MDK could not be read now, for a passing failure: what it was asked for is asked again later,
/// and nothing is taken as read meanwhile. Not "not there", which is an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotNow;

/// What a message is for the conversation.
pub enum Incoming {
    /// A chat message.
    Heard(Heard),
    /// An answer or a control.
    Asked(HeardRequest),
}

/// The agent's account in MDK.
#[derive(Clone)]
pub struct Marmot {
    /// MDK's runtime.
    pub runtime: MarmotAppRuntime,
    /// MDK's app, for what the runtime does not offer.
    pub app: MarmotApp,
    /// The account's label in its home.
    pub label: String,
    /// The agent's public key.
    pub agent: PubKey,
    /// Faults put in the way of sends: none in use. For the tests of what the core does when MDK
    /// fails, as the simulator will put them (`docs/design/verification.md`).
    pub faults: Faults,
}

/// Faults the next sends, and the next readings of where a group is, or of where its messages end,
/// meet, one each, oldest first.
#[derive(Clone, Default)]
pub struct Faults {
    next: Arc<Mutex<VecDeque<Fault>>>,
    /// The tokens of sends MDK keeps queued ([`Fault::Held`]), and until when.
    held: Arc<Mutex<HashMap<String, std::time::Instant>>>,
}

/// What a send meets.
pub enum Fault {
    /// The send goes out, and MDK's reply is lost, as when the wait for it runs out.
    LoseReply,
    /// MDK refuses the send with this error, before it takes it.
    Refuse(AppError),
    /// The send meets what MDK says of a try it took and dropped before it went out, as when its
    /// engine fails it. Nothing is handed to MDK; the core tries again under a new token.
    Reject,
    /// MDK cannot say where a group is, the next time a message is read: a passing failure, as
    /// when the group's state cannot be read for a moment.
    PlaceFails,
    /// MDK cannot give what it stored for a group, the next time the core reads it.
    StoredFails,
    /// The relays cannot be read for the agent's profile, the next time the core reads it.
    ProfileFails,
    /// The relays that answer hold no profile for the agent, the next time the core reads it.
    ProfileMissing,
    /// The agent's profile is read, and the next publish of its name fails.
    NameFails,
    /// MDK takes the send and keeps it queued this long before its engine takes it, as when the
    /// engine is held up: until then MDK says it is queued.
    Held(Duration),
}

impl Faults {
    /// The next send meets `fault`.
    pub fn push(&self, fault: Fault) {
        self.next.lock().expect("never poisoned").push_back(fault);
    }

    /// Whether every fault pushed has been met.
    pub fn is_empty(&self) -> bool {
        self.next.lock().expect("never poisoned").is_empty()
    }

    /// The oldest fault for a send, one that is none of the others.
    fn next_send(&self) -> Option<Fault> {
        self.next(|f| {
            !matches!(
                f,
                Fault::PlaceFails
                    | Fault::StoredFails
                    | Fault::ProfileFails
                    | Fault::ProfileMissing
                    | Fault::NameFails
            )
        })
    }

    /// Keeps the send under `token` queued for `long`, from its first try.
    fn hold(&self, token: &str, long: Duration) {
        let mut held = self.held.lock().expect("never poisoned");
        let until = std::time::Instant::now() + long;
        held.entry(token.to_string()).or_insert(until);
    }

    /// Whether the send under `token` is kept queued now.
    fn held(&self, token: &str) -> bool {
        let held = self.held.lock().expect("never poisoned");
        held.get(token)
            .is_some_and(|until| *until > std::time::Instant::now())
    }

    /// The oldest fault `which` picks.
    fn next(&self, which: impl Fn(&Fault) -> bool) -> Option<Fault> {
        let mut faults = self.next.lock().expect("never poisoned");
        let at = faults.iter().position(which)?;
        faults.remove(at)
    }
}

impl Marmot {
    /// What a message is for the conversation, if anything: a chat message or an answer or a
    /// control for this agent, from someone else, in a group the agent has accepted. [`NotNow`]
    /// when MDK could not say where its group is, or what it quotes, for a passing failure: the
    /// message is read again later, never taken as one that is not for the agent.
    pub async fn incoming(&self, message: Message) -> Result<Option<Incoming>, NotNow> {
        if message.sender == self.agent {
            return Ok(None);
        }
        let is_request = message.kind == REQUEST_KIND;
        if message.kind != CHAT && !is_request {
            return Ok(None);
        }
        let Some(chat) = self.place(&message.group).await? else {
            return Ok(None);
        };
        let author_name = self.name(&message.sender, message.sender_name.clone());
        if is_request {
            let Some((target, asked)) = carrier::read_request(&self.agent, &message.tags) else {
                return Ok(None);
            };
            return Ok(Some(Incoming::Asked(HeardRequest {
                chat,
                event: message.event,
                author: message.sender,
                author_name,
                age_secs: now().saturating_sub(message.recorded_at),
                target,
                asked,
            })));
        }
        let mentions_agent = message
            .tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "p" && t[1] == self.agent.as_str());
        let reply_to = self.quoted(&message)?;
        Ok(Some(Incoming::Heard(Heard {
            chat,
            event: message.event,
            author: message.sender,
            author_name,
            text: without_mention(&message.content, &self.agent),
            reply_to,
            mentions_agent,
            attachments: Vec::new(),
        })))
    }

    /// Where a group is: whether it is a direct chat, and how many members it has. `None` for a
    /// group the agent has not accepted, or one MDK no longer has; [`NotNow`] when MDK could not
    /// be read.
    async fn place(&self, group: &GroupKey) -> Result<Option<Where>, NotNow> {
        let Some(id) = group_id(group) else {
            return Ok(None);
        };
        let groups = self.app.groups(&self.label).map_err(|_| NotNow)?;
        let Some(record) = groups
            .into_iter()
            .find(|g| g.group_id_hex == group.as_str())
        else {
            return Ok(None);
        };
        if record.pending_confirmation || record.archived {
            return Ok(None);
        }
        if self
            .faults
            .next(|f| matches!(f, Fault::PlaceFails))
            .is_some()
        {
            return Err(NotNow);
        }
        let state = match self.runtime.group_mls_state(&self.label, &id).await {
            Ok(state) => state,
            // Gone for good: there is no one to answer.
            Err(AppError::UnknownGroup(_) | AppError::GroupRemoved(_)) => return Ok(None),
            Err(_) => return Err(NotNow),
        };
        Ok(Some(Where {
            group: group.clone(),
            name: None,
            // MDK marks a direct chat: no name and two members. A named group of two is a group.
            direct: record.direct_member_ids_hex.is_some(),
            members: state.member_count,
        }))
    }

    /// The two accounts in `group` now, as MDK keeps them for a direct chat: no name and two
    /// accounts, however many devices each has in it. `None` for a named group, one of more or
    /// fewer accounts, or one MDK does not have; [`NotNow`] when MDK could not be read.
    pub fn two_people(&self, group: &GroupKey) -> Result<Option<Vec<PubKey>>, NotNow> {
        let record = (self.app)
            .group(&self.label, group.as_str())
            .map_err(|_| NotNow)?;
        Ok(record
            .and_then(|r| r.direct_member_ids_hex)
            .map(|ids| ids.into_iter().map(PubKey).collect()))
    }

    fn name(&self, person: &PubKey, given: Option<String>) -> String {
        given
            .or_else(|| self.runtime.display_name_for_account_id(person.as_str()))
            .unwrap_or_else(|| "Someone".into())
    }

    /// The message a chat message replies to: the first `e` tag, read from storage. `None` when
    /// it replies to nothing, or to a message not stored here; [`NotNow`] when storage could not be
    /// read.
    fn quoted(&self, message: &Message) -> Result<Option<HeardQuote>, NotNow> {
        let Some(target) = (message.tags.iter())
            .find(|t| t.len() >= 2 && t[0] == "e")
            .map(|t| t[1].clone())
        else {
            return Ok(None);
        };
        let record = self
            .runtime
            .message_by_id(&self.label, message.group.as_str(), &target)
            .map_err(|_| NotNow)?;
        let Some(record) = record else {
            return Ok(None);
        };
        let author = PubKey(record.sender.clone());
        Ok(Some(HeardQuote {
            event: EventKey(target),
            author_name: self.name(&author, None),
            author,
            text: record.plaintext,
        }))
    }

    /// Sends something to a group under `token`, and gives back the id of the event it became.
    /// MDK keeps the send under [`Token::mdk`] before it answers (its `submit_*` calls), and a
    /// send tried again under the same token, after an answer that was lost or a restart, gives
    /// back the same event and sends nothing: what went out goes out once. The event comes back
    /// once MDK has taken the send into its engine, which delivers it from there. MDK is asked
    /// once: while it still has the send queued the answer is [`SendFailed::Pending`], and the
    /// core asks again later by trying it again under the same token, with nothing held up
    /// meanwhile. MDK may drop it before its engine takes it ([`SendFailed::Rejected`]), and then
    /// nothing of it went out.
    pub async fn send(
        &self,
        group: &GroupKey,
        what: &Sending,
        token: &Token,
    ) -> Result<EventKey, SendFailed> {
        let id = group_id(group).ok_or(SendFailed::Refused { gone: false })?;
        let fault = self.faults.next_send();
        match fault {
            Some(Fault::Refuse(error)) => return Err(send_failed(&error)),
            Some(Fault::Reject) => return Err(SendFailed::Rejected),
            Some(Fault::Held(long)) => self.faults.hold(&token.mdk, long),
            _ => {}
        }
        if let Some(tried) = token.tried_at
            && self.may_have_let_go(group, tried)?
        {
            match self.status(&id, &token.mdk) {
                // MDK still keeps it: tried again, it is answered from what MDK keeps.
                Ok(Some(_)) => {}
                Ok(None) => return Err(SendFailed::Forgotten),
                Err(_) => return Err(SendFailed::Unknown),
            }
        }
        let taken = match self.submit(&id, what, token).await {
            Ok(Taken::Kept(event)) => event,
            // Words an older core sent, without a token: their edit goes as it went then. Sent
            // again, it shows the same text, and adds nothing anyone sees.
            Ok(Taken::Sent(event)) => return Ok(event),
            Err(error) => {
                return Err(match self.status(&id, &token.mdk) {
                    // A try MDK dropped is refused if tried again; that is the drop, not a
                    // refusal.
                    Ok(Some(LocalSendStatus::Rejected)) => SendFailed::Rejected,
                    // MDK answers a token it took with what it made of it, before it looks at
                    // the group: so it never took this one, and the group is going or gone.
                    Ok(None) if group_ended(&error) => SendFailed::Refused { gone: false },
                    // Nor this edit: it checks the token before the first event the edit names.
                    Ok(None) if token.edits.is_some() && first_gone(&error) => {
                        SendFailed::FirstGone
                    }
                    // Nor these words: it places a thread message before it keeps it.
                    Ok(None) if in_thread(what) && thread_gone(&error) => SendFailed::ThreadGone,
                    _ => send_failed(&error),
                });
            }
        };
        match self.status(&id, &token.mdk) {
            Ok(Some(LocalSendStatus::EngineOwned | LocalSendStatus::Completed(_))) => {}
            Ok(Some(LocalSendStatus::Rejected)) => return Err(SendFailed::Rejected),
            // Queued, or not to be read for a moment: MDK has it, and goes on with it.
            Ok(Some(LocalSendStatus::Queued) | None) | Err(_) => return Err(SendFailed::Pending),
        }
        if let Some(Fault::LoseReply) = fault {
            return Err(SendFailed::Unknown);
        }
        Ok(taken)
    }

    /// Hands a send to MDK, which keeps it under `token` and gives back the event it will be.
    async fn submit(&self, id: &GroupId, what: &Sending, token: &Token) -> Result<Taken, AppError> {
        let label = &self.label;
        let mdk = token.mdk.clone();
        let accepted = match what {
            Sending::Item { item, .. } => {
                let out = carrier::item(item).map_err(|_| too_large())?;
                if out.edits().is_none() {
                    (self.runtime)
                        .submit_custom_event(label, id, out.kind, out.tags, out.content, mdk)
                        .await?
                } else {
                    // A later version is MDK's edit of the first event, which MDK names from the
                    // token that event went under; the core sent every first event under one.
                    let original = token.edits.clone().ok_or_else(|| {
                        AppError::InvalidAppMessagePayload("no token for the first event".into())
                    })?;
                    let tags = out.tags_but_target();
                    (self.runtime)
                        .submit_tagged_edit_for_local_send(
                            label,
                            id,
                            original,
                            out.content,
                            tags,
                            mdk,
                        )
                        .await?
                }
            }
            Sending::Words {
                edit: Some(target),
                text,
                ..
            } => {
                let text = carrier::fit_words(text, &[]).map_err(|_| too_large())?;
                let Some(original) = token.edits.clone() else {
                    let summary = (self.runtime)
                        .edit_message(label, id, target.as_str(), &text)
                        .await?;
                    let event = summary.message_ids.into_iter().next();
                    return event
                        .map(|e| Taken::Sent(EventKey(e)))
                        .ok_or(AppError::UnknownGroup(String::new()));
                };
                (self.runtime)
                    .submit_tagged_edit_for_local_send(label, id, original, text, Vec::new(), mdk)
                    .await?
            }
            Sending::Words {
                text,
                turn,
                remark,
                settles,
                thread,
                ..
            } => {
                let tags = carrier::words_tags(turn.as_ref(), *remark, settles);
                // Sized with the thread's tags too, which MDK adds to a thread message.
                let sized: Vec<Vec<String>> = (tags.iter().cloned())
                    .chain(thread.iter().flat_map(|t| carrier::thread_tags(t, true)))
                    .collect();
                let text = carrier::fit_words(text, &sized).map_err(|_| too_large())?;
                match thread.as_ref().and_then(|t| t.parent.as_ref()) {
                    // A subagent's words are a thread message under its `task` item, which MDK
                    // tags from that item's place (D-122).
                    Some(parent) => {
                        let parent = parent.as_str().to_string();
                        (self.runtime)
                            .submit_thread_message(label, id, parent, text, Vec::new(), tags, mdk)
                            .await?
                    }
                    None => {
                        (self.runtime)
                            .submit_tagged_text(label, id, text, tags, mdk)
                            .await?
                    }
                }
            }
        };
        Ok(Taken::Kept(EventKey(accepted.message_id_hex)))
    }

    /// Whether MDK may have let go of a token first tried at `tried` (seconds since 1970). MDK
    /// forgets a token with the message it made, and a group's messages disappear once they are
    /// as old as the group says; a message is never older than its first try. An error when MDK
    /// cannot say for a moment.
    fn may_have_let_go(&self, group: &GroupKey, tried: u64) -> Result<bool, SendFailed> {
        let record = (self.app)
            .group(&self.label, group.as_str())
            .map_err(|_| SendFailed::Unknown)?;
        let keep = record.map_or(0, |r| r.message_retention.disappearing_message_secs);
        let now = SystemTime::now().duration_since(UNIX_EPOCH);
        let now = now.map_or(0, |d| d.as_secs());
        Ok(keep > 0 && now.saturating_sub(tried) >= keep)
    }

    /// What MDK says became of the send it keeps under `token`.
    fn status(&self, id: &GroupId, token: &str) -> Result<Option<LocalSendStatus>, AppError> {
        if self.faults.held(token) {
            return Ok(Some(LocalSendStatus::Queued));
        }
        (self.runtime).local_send_status(&self.label, id, token)
    }

    /// Sends a file to a group as an encrypted attachment, tagged with its turn like the agent's
    /// words (D-022, D-090), and gives back the id of the chat message it became.
    pub async fn send_file(
        &self,
        group: &GroupKey,
        file: &Outgoing,
        turn: Option<&TurnId>,
    ) -> Result<EventKey, AppError> {
        let id = group_id(group).ok_or_else(|| AppError::UnknownGroup(String::new()))?;
        let request = MediaUploadRequest {
            attachments: vec![MediaUploadAttachmentRequest {
                file_name: file.name.clone(),
                media_type: file.media_type.clone(),
                plaintext: file.bytes.clone(),
                dim: None,
                thumbhash: None,
            }],
            caption: None,
            send: true,
            blossom_server: None,
            message_tags: carrier::words_tags(turn, false, &[]),
        };
        let uploaded = self.runtime.upload_media(&self.label, &id, request).await?;
        uploaded
            .sent
            .and_then(|sent| sent.message_ids.into_iter().next())
            .map(EventKey)
            .ok_or(AppError::UnknownGroup(String::new()))
    }

    /// The invitations waiting for a decision, each with who sent it.
    pub fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)> {
        self.app
            .pending_group_invites(&self.label)
            .unwrap_or_default()
            .into_iter()
            .map(|invite| {
                let group = GroupKey(hex::encode(invite.group_id.as_slice()));
                let welcomer = invite.welcomer.map(|w| PubKey(hex::encode(w.as_slice())));
                (group, welcomer)
            })
            .collect()
    }

    /// Accepts or declines an invitation, trying again while MDK is busy catching up.
    pub async fn decide(&self, group: &GroupKey, accept: bool) -> Result<(), AppError> {
        let id = group_id(group).ok_or_else(|| AppError::UnknownGroup(String::new()))?;
        for _ in 0..50 {
            let result = if accept {
                self.runtime
                    .accept_group_invite(&self.label, &id)
                    .await
                    .map(drop)
            } else {
                self.runtime
                    .decline_group_invite(&self.label, &id)
                    .await
                    .map(drop)
            };
            match result {
                Err(AppError::AccountWorkerBusy) => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await
                }
                other => return other,
            }
        }
        Err(AppError::AccountWorkerBusy)
    }

    /// The groups the agent has accepted; [`NotNow`] when MDK could not list them, which is not
    /// an account with none.
    pub fn groups(&self) -> Result<Vec<GroupKey>, NotNow> {
        Ok((self.accepted()?)
            .map(|g| GroupKey(g.group_id_hex))
            .collect())
    }

    /// MDK's record of each group the agent has accepted, for [`Marmot::groups`] and
    /// [`Marmot::listed`] alike.
    fn accepted(&self) -> Result<impl Iterator<Item = marmot_app::AppGroupRecord>, NotNow> {
        let groups = self.app.groups(&self.label).map_err(|_| NotNow)?;
        Ok((groups.into_iter()).filter(|g| !g.pending_confirmation && !g.archived))
    }

    /// Each group the agent has accepted, as `marmot-agent groups` shows it to `owner`: its id, its
    /// name, whether it is the owner's two-person chat and whether `activity` lists it (D-136).
    /// [`NotNow`] when MDK could not list them.
    pub fn listed(&self, owner: &PubKey, activity: &FullActivity) -> Result<Vec<Listed>, NotNow> {
        Ok((self.accepted()?)
            .map(|g| {
                let two: Option<Vec<PubKey>> =
                    (g.direct_member_ids_hex).map(|ids| ids.into_iter().map(PubKey).collect());
                let group = GroupKey(g.group_id_hex);
                Listed {
                    owners_chat: owners_chat(&self.agent, owner, two.as_deref()),
                    full_activity: activity.includes(&group),
                    name: g.profile.name,
                    group,
                }
            })
            .collect())
    }

    /// A page of what was stored for a group after `after`, in storage order: at most `limit`
    /// rows, MDK's own paging forward in the order messages arrived here (its
    /// `messages_after_insert_order`), so a message that arrived late with an earlier time is read
    /// in its place. The page holds the chat messages, answers and controls others sent; its last
    /// place counts the rows the core does not read too, such as the agent's own.
    pub fn stored(&self, group: &GroupKey, after: i64, limit: usize) -> Result<Page, NotNow> {
        if (self.faults)
            .next(|f| matches!(f, Fault::StoredFails))
            .is_some()
        {
            return Err(NotNow);
        }
        let query = AppMessageQuery {
            group_id_hex: Some(group.as_str().to_string()),
            kinds: Some(vec![CHAT, REQUEST_KIND]),
            limit: Some(limit),
        };
        let rows = (self.runtime)
            .messages_after_insert_order(&self.label, query, after)
            .map_err(|_| NotNow)?;
        let last = rows.last().map(|r| r.insert_order);
        let more = rows.len() >= limit;
        let messages = (rows.into_iter())
            .filter(|r| r.direction == "received" && !r.invalidated)
            .map(Message::from)
            .collect();
        Ok(Page {
            messages,
            last,
            more,
        })
    }
}

/// One page of what a group stored, in storage order.
#[derive(Debug, Default)]
pub struct Page {
    /// The chat messages, answers and controls others sent, in storage order.
    pub messages: Vec<Message>,
    /// The place of the last row of the page, whether or not it is one of `messages`: the next
    /// page starts after it. `None` when nothing was stored after the place asked for.
    pub last: Option<i64>,
    /// Whether the page was full, so more may follow it.
    pub more: bool,
}

/// What MDK gave back for a send.
enum Taken {
    /// MDK keeps it under its token, as this event.
    Kept(EventKey),
    /// It went out without a token, as this event.
    Sent(EventKey),
}

/// What a send that cannot fit in 64 KB fails with, before it reaches MDK: a refusal, which drops
/// it with a notice (S7).
fn too_large() -> AppError {
    AppError::InvalidAppMessagePayload("over the size limit".into())
}

/// The group's id, as MDK takes it.
fn group_id(group: &GroupKey) -> Option<GroupId> {
    hex::decode(group.as_str()).ok().map(GroupId::new)
}

/// The text without the mention of the agent that addressed it, which the harness need not read.
fn without_mention(text: &str, agent: &PubKey) -> String {
    let Ok(npub) = marmot_app::npub_for_account_id(agent.as_str()) else {
        return text.to_string();
    };
    let mut text = text.replace(&format!("nostr:{npub}"), "");
    text = text.replace(&format!("@{npub}"), "");
    text.trim().to_string()
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What the run loop needs from the Marmot side, so the core can run on MDK, as [`Marmot`] does,
/// or on a stand-in that a simulator controls (`docs/design/verification.md`). Each method does
/// what [`Marmot`]'s method of the same name does; MDK's errors are reduced to what the core
/// tells apart.
pub trait Side: Clone + Send + Sync + 'static {
    /// Where live events come from.
    type Events: Send;

    /// The agent's public key.
    fn agent(&self) -> &PubKey;

    /// The next live event. Dropping the wait loses nothing.
    fn next(&self, events: &mut Self::Events) -> impl Future<Output = Live> + Send;

    /// Publishes the agent's name in its profile, keeping the rest of it, unless the name is
    /// already right. [`NotNow`] when the profile could not be read, so nothing was published
    /// over it, or the name could not be published: the core tries again later.
    fn publish_name(
        &self,
        name: &str,
        relays: &[String],
    ) -> impl Future<Output = Result<(), NotNow>> + Send;

    /// What a message is for the conversation, if anything; [`NotNow`] when that could not be
    /// told for a passing failure.
    fn incoming(
        &self,
        message: Message,
    ) -> impl Future<Output = Result<Option<Incoming>, NotNow>> + Send;

    /// The invitations waiting for a decision, each with who sent it.
    fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)>;

    /// Accepts or declines an invitation; whether that was done.
    fn decide(&self, group: &GroupKey, accept: bool) -> impl Future<Output = bool> + Send;

    /// The groups the agent has accepted; [`NotNow`] when they could not be listed.
    fn groups(&self) -> Result<Vec<GroupKey>, NotNow>;

    /// The two accounts in `group` now, when MDK marks it a two-person chat: no name and two
    /// accounts. `None` for any other group; [`NotNow`] when MDK could not say.
    fn two_people(&self, group: &GroupKey) -> Result<Option<Vec<PubKey>>, NotNow>;

    /// A page of what was stored for a group after `after`, in storage order: at most `limit`
    /// rows, of which the page holds what others sent.
    fn stored(&self, group: &GroupKey, after: i64, limit: usize) -> Result<Page, NotNow>;

    /// Sends something to a group under `token`, and gives back the id of the event it became.
    /// A send tried again under the same token gives back the same event and sends nothing.
    fn send(
        &self,
        group: &GroupKey,
        what: &Sending,
        token: &Token,
    ) -> impl Future<Output = Result<EventKey, SendFailed>> + Send;

    /// Sends a file to a group, tagged with its turn.
    fn send_file(
        &self,
        group: &GroupKey,
        file: &Outgoing,
        turn: Option<&TurnId>,
    ) -> impl Future<Output = Result<EventKey, FileFailed>> + Send;

    /// Reads a file the agent asks to send from the folders it may come from, before it is sent:
    /// what [`Folders::read`] does, on a thread that may block. A stand-in may take its time, as a
    /// slow disk does.
    fn read_file(&self, folders: &Folders, path: &Path) -> Result<Outgoing, Refusal> {
        folders.read(path)
    }
}

/// Live events, and when they were subscribed to: what a group the core has no read mark for
/// received by then is its history (L3).
pub struct Subscribed<E> {
    /// The events, from the moment they were subscribed to.
    pub events: E,
    /// That moment, by this machine's clock in whole seconds, as a message's `received_at` is.
    pub since: u64,
}

/// Subscribes to MDK's live events, noting when. A core subscribes before MDK starts, so it
/// misses nothing MDK announces while it starts, and before its harness starts, so what arrives
/// while a slow harness starts is not taken as history.
pub fn subscribe(
    runtime: &MarmotAppRuntime,
) -> Subscribed<tokio::sync::broadcast::Receiver<MarmotAppEvent>> {
    let events = runtime.subscribe();
    Subscribed {
        events,
        since: now(),
    }
}

/// A live event, as the core takes it.
pub enum Live {
    /// A message arrived in one of the agent's groups: its group is read from its mark. MDK may
    /// announce a message again, or not in the order it stored them (its `MessageReceived`).
    Message(Message),
    /// The agent joined a group, or one of its groups changed: invitations are looked at again.
    Groups,
    /// Something the core does not act on.
    Other,
    /// The core fell behind and missed events: it reads what was stored.
    Lagged,
    /// No more events will come.
    Closed,
}

/// What a failed send means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendFailed {
    /// MDK did not take it, and may later: it is tried again.
    Busy,
    /// MDK refused it, and will again.
    Refused {
        /// It was an edit of a message that is gone.
        gone: bool,
    },
    /// MDK may have taken it, and the answer was lost or is not known yet: it is tried again
    /// under the same token, which MDK answers with the same event, sending nothing twice.
    Unknown,
    /// MDK took it, and dropped it before it went out: nothing of it went. MDK refuses its token
    /// from now on, so it is tried again under a new one.
    Rejected,
    /// MDK keeps it under its token, queued, and its engine has not taken it yet: it is tried
    /// again soon under the same token, which MDK answers from what it keeps.
    Pending,
    /// It was an edit, and MDK no longer has the event it edits, nor the token that event went
    /// under: the group's messages disappeared since it went. MDK did not take this edit. A
    /// later version of an item then goes out as a new first event, and the agent's words as a
    /// new message.
    FirstGone,
    /// It was a subagent's words, a thread message under its `task` item, and MDK no longer has
    /// that item, or the thread's root, to place it by: the group's messages disappeared since it
    /// went. MDK did not take it. The words then go in the main chat, as the agent's words.
    ThreadGone,
    /// It was first tried so long ago that its group's messages have disappeared since, and
    /// MDK no longer knows its token: it may have gone out, and tried again it could go out
    /// twice. It is given up, and the chat told it may not have been delivered.
    Forgotten,
}

/// What a failed file send means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFailed {
    /// MDK's account was busy and did not take it: it may be offered again.
    Busy,
    /// Anything else, which may have come after MDK took it.
    Failed,
}

/// Whether MDK refused a send because its group is being disbanded, or because MDK has no such
/// group: for good, unlike other failures to read its storage, which pass.
fn group_ended(error: &AppError) -> bool {
    matches!(
        error,
        AppError::GroupDisbanding(_)
            | AppError::Storage(cgka_traits::storage::StorageError::NotFound)
    )
}

/// Whether MDK refused an edit because it no longer has the first event it names, nor the token
/// that event went under: MDK forgets a token with its event once the group's messages disappear.
/// Not an event someone deleted, or one that never reached the group, which MDK refuses as an
/// edit target ([`AppError::InvalidEditTarget`]).
fn first_gone(error: &AppError) -> bool {
    matches!(
        error,
        AppError::InvalidAppMessagePayload(why)
            if why == "original local send was not found"
                || why == "original local message is unavailable"
    )
}

/// Whether `what` is words to go in a subagent's thread, as a new thread message.
fn in_thread(what: &Sending) -> bool {
    matches!(what, Sending::Words { edit: None, thread: Some(t), .. } if t.parent.is_some())
}

/// Whether MDK refused a thread message because it does not have the message it answers, or the
/// thread's root, to place it by (`thread_placement`, MDK `029e057`).
fn thread_gone(error: &AppError) -> bool {
    matches!(
        error,
        AppError::InvalidAppMessagePayload(why)
            if why == "thread parent is unavailable" || why == "thread root is unavailable"
    )
}

/// What MDK's error says of a send: whether MDK took it.
fn send_failed(error: &AppError) -> SendFailed {
    match error {
        AppError::AccountWorkerBusy
        | AppError::RuntimeBusy
        | AppError::AccountSessionBusy
        | AppError::GroupCatchingUp(_)
        | AppError::GroupCommitPending(_) => SendFailed::Busy,
        AppError::InvalidEditTarget => SendFailed::Refused { gone: true },
        // MDK spaces out the edits of one message a second apart, at most half a minute ahead:
        // the edit is taken once the ones before it have gone.
        AppError::InvalidAppMessagePayload(why) if why.starts_with("pending edit rate limit") => {
            SendFailed::Busy
        }
        // MDK makes an event's id from its kind, tags, content and second, and keeps the id of
        // every send it took, even one it dropped: the same bytes under another token in that
        // second are the same event. MDK took nothing, and in the next second they are another.
        AppError::InvalidAppMessagePayload(why)
            if why.starts_with("message identity collision") =>
        {
            SendFailed::Busy
        }
        AppError::InvalidAppMessagePayload(_)
        | AppError::EventTooLarge { .. }
        | AppError::UnknownGroup(_)
        | AppError::GroupRemoved(_)
        | AppError::UserBlocked => SendFailed::Refused { gone: false },
        _ => SendFailed::Unknown,
    }
}

impl Side for Marmot {
    type Events = tokio::sync::broadcast::Receiver<MarmotAppEvent>;

    fn agent(&self) -> &PubKey {
        &self.agent
    }

    async fn next(&self, events: &mut Self::Events) -> Live {
        use tokio::sync::broadcast::error::RecvError;
        match events.recv().await {
            Ok(MarmotAppEvent::MessageReceived(received))
                if received.account_id_hex == self.agent.as_str() =>
            {
                Live::Message(Message::from(received.message))
            }
            Ok(
                MarmotAppEvent::GroupJoined { account_id_hex, .. }
                | MarmotAppEvent::GroupEvent(marmot_app::RuntimeGroupEvent {
                    account_id_hex, ..
                }),
            ) if account_id_hex == self.agent.as_str() => Live::Groups,
            Ok(_) => Live::Other,
            Err(RecvError::Lagged(_)) => Live::Lagged,
            Err(RecvError::Closed) => Live::Closed,
        }
    }

    async fn publish_name(&self, name: &str, relays: &[String]) -> Result<(), NotNow> {
        let relays = (relays.iter())
            .map(|r| cgka_traits::TransportEndpoint(r.clone()))
            .collect();
        let read = (self.faults).next(|f| matches!(f, Fault::ProfileFails | Fault::ProfileMissing));
        let current = match read {
            Some(Fault::ProfileFails) => Err(()),
            Some(_) => Ok(None),
            None => self
                .runtime
                .fetch_current_user_profile_for_account_id(self.agent.as_str(), relays)
                .await
                .map_err(drop),
        };
        // MDK's publisher replaces the profile's about, picture, NIP-05 and lightning address
        // with those of the profile it is given, so the name is published only over a profile
        // read here. A read that fails, or finds none on the relays that answered, is no
        // profile: MDK's read says `None` so that a caller does not publish a partial one over
        // what it could not confirm. The agent's first profile is published when its identity
        // is made, so one that has none to read waits until the relays give it.
        let Ok(Some(current)) = current else {
            return Err(NotNow);
        };
        if current.name.as_deref() == Some(name) && current.display_name.as_deref() == Some(name) {
            return Ok(());
        }
        let profile = marmot_app::UserProfileMetadata {
            name: Some(name.to_string()),
            display_name: Some(name.to_string()),
            ..current
        };
        let published = match (self.faults).next(|f| matches!(f, Fault::NameFails)) {
            Some(_) => Err(()),
            None => (self.runtime)
                .publish_user_profile_using_account_relays(&self.label, profile)
                .await
                .map(drop)
                .map_err(drop),
        };
        if published.is_err() {
            eprintln!("marmot-agent: the agent's name could not be published; trying again later");
            return Err(NotNow);
        }
        Ok(())
    }

    fn incoming(
        &self,
        message: Message,
    ) -> impl Future<Output = Result<Option<Incoming>, NotNow>> + Send {
        Marmot::incoming(self, message)
    }

    fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)> {
        Marmot::invitations(self)
    }

    async fn decide(&self, group: &GroupKey, accept: bool) -> bool {
        Marmot::decide(self, group, accept).await.is_ok()
    }

    fn groups(&self) -> Result<Vec<GroupKey>, NotNow> {
        Marmot::groups(self)
    }

    fn two_people(&self, group: &GroupKey) -> Result<Option<Vec<PubKey>>, NotNow> {
        Marmot::two_people(self, group)
    }

    fn stored(&self, group: &GroupKey, after: i64, limit: usize) -> Result<Page, NotNow> {
        Marmot::stored(self, group, after, limit)
    }

    async fn send(
        &self,
        group: &GroupKey,
        what: &Sending,
        token: &Token,
    ) -> Result<EventKey, SendFailed> {
        Marmot::send(self, group, what, token).await
    }

    async fn send_file(
        &self,
        group: &GroupKey,
        file: &Outgoing,
        turn: Option<&TurnId>,
    ) -> Result<EventKey, FileFailed> {
        Marmot::send_file(self, group, file, turn)
            .await
            .map_err(|error| match error {
                AppError::AccountWorkerBusy => FileFailed::Busy,
                _ => FileFailed::Failed,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MDK's words, at the revision the core links (`marmot-app`'s `local_submissions.rs`), for a
    /// send under a new token whose event is one MDK already keeps: the same kind, tags, content
    /// and second.
    const COLLISION: &str = "message identity collision: an identical event already exists; \
                             submission was not accepted";

    // The same words in the same second meet the event MDK made of them: MDK took nothing, and in
    // the next second they are another event, so it is tried again, and not refused (S1).
    #[test]
    fn a_send_that_meets_an_event_mdk_keeps_is_busy_and_not_refused() {
        let collision = AppError::InvalidAppMessagePayload(COLLISION.into());
        assert_eq!(send_failed(&collision), SendFailed::Busy);
        let unreadable =
            AppError::InvalidAppMessagePayload("missing local submission payload".into());
        assert_eq!(
            send_failed(&unreadable),
            SendFailed::Refused { gone: false },
            "any other payload MDK will not take is refused"
        );
    }
}
