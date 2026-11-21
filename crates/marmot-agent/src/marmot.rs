//! The Marmot side: what arrives in the agent's groups, turned into what the conversation takes,
//! and what the conversation sends, handed to MDK (D-002).
//!
//! MDK is used as it is today, so an agent item, an answer and a control travel under the
//! temporary kinds of [`carrier`] (D-069). The agent's words are chat messages carrying their
//! turn (D-022).

use crate::carrier::{self, REQUEST_KIND};
use crate::conversation::{Heard, HeardQuote, HeardRequest, Sending, Where};
use crate::files::Outgoing;
use crate::keys::{EventKey, GroupKey, PubKey};
use cgka_traits::GroupId;
use marmot_agent_events::TurnId;
use marmot_app::{
    AppError, AppMessageQuery, AppMessageRecord, MarmotApp, MarmotAppRuntime,
    MediaUploadAttachmentRequest, MediaUploadRequest,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// The kind of a chat message.
const CHAT: u64 = 9;

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
    /// Its place in this machine's storage, which only grows: the order messages arrived in here,
    /// whatever their senders' clocks say. Known for a stored message; a live one is looked up.
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
            order: None,
        }
    }
}

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
    /// A Blossom server to upload files to, in place of the group's own, for a test on this
    /// machine. `None` uses the group's.
    pub media_server: Option<String>,
}

impl Marmot {
    /// What a message is for the conversation, if anything: a chat message or an answer or a
    /// control for this agent, from someone else, in a group the agent has accepted.
    pub async fn incoming(&self, message: Message) -> Option<Incoming> {
        if message.sender == self.agent {
            return None;
        }
        let is_request = message.kind == REQUEST_KIND;
        if message.kind != CHAT && !is_request {
            return None;
        }
        let chat = self.place(&message.group).await?;
        let author_name = self.name(&message.sender, message.sender_name.clone());
        if is_request {
            let (target, asked) = carrier::read_request(&self.agent, &message.tags)?;
            return Some(Incoming::Asked(HeardRequest {
                chat,
                event: message.event,
                author: message.sender,
                author_name,
                age_secs: now().saturating_sub(message.recorded_at),
                target,
                asked,
            }));
        }
        let mentions_agent = message
            .tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "p" && t[1] == self.agent.as_str());
        let reply_to = self.quoted(&message).await;
        Some(Incoming::Heard(Heard {
            chat,
            event: message.event,
            author: message.sender,
            author_name,
            text: without_mention(&message.content, &self.agent),
            reply_to,
            mentions_agent,
            attachments: Vec::new(),
        }))
    }

    /// Where a group is: whether it is a direct chat, and how many members it has. `None` for a
    /// group the agent has not accepted.
    async fn place(&self, group: &GroupKey) -> Option<Where> {
        let id = group_id(group)?;
        let record = self
            .app
            .groups(&self.label)
            .ok()?
            .into_iter()
            .find(|g| g.group_id_hex == group.as_str())?;
        if record.pending_confirmation || record.archived {
            return None;
        }
        let state = self.runtime.group_mls_state(&self.label, &id).await.ok()?;
        Some(Where {
            group: group.clone(),
            name: None,
            // MDK marks a direct chat: no name and two members. A named group of two is a group.
            direct: record.direct_member_ids_hex.is_some(),
            members: state.member_count,
        })
    }

    fn name(&self, person: &PubKey, given: Option<String>) -> String {
        given
            .or_else(|| self.runtime.display_name_for_account_id(person.as_str()))
            .unwrap_or_else(|| "Someone".into())
    }

    /// The message a chat message replies to: the first `e` tag, read from storage.
    async fn quoted(&self, message: &Message) -> Option<HeardQuote> {
        let target = message
            .tags
            .iter()
            .find(|t| t.len() >= 2 && t[0] == "e")
            .map(|t| t[1].clone())?;
        let record = self
            .runtime
            .message_by_id(&self.label, message.group.as_str(), &target)
            .ok()??;
        let author = PubKey(record.sender.clone());
        Some(HeardQuote {
            event: EventKey(target),
            author_name: self.name(&author, None),
            author,
            text: record.plaintext,
        })
    }

    /// Sends something to a group, and gives back the id of the event it became.
    pub async fn send(&self, group: &GroupKey, what: &Sending) -> Result<EventKey, AppError> {
        let id = group_id(group).ok_or_else(|| AppError::UnknownGroup(String::new()))?;
        let summary = match what {
            Sending::Item { item, .. } => {
                let out = carrier::item(item);
                self.runtime
                    .send_custom_event(&self.label, &id, out.kind, out.tags, out.content)
                    .await?
            }
            Sending::Words {
                edit: Some(target),
                text,
                ..
            } => {
                self.runtime
                    .edit_message(&self.label, &id, target.as_str(), text)
                    .await?
            }
            Sending::Words {
                text, turn, remark, ..
            } => {
                let tags = carrier::words_tags(turn.as_ref(), *remark);
                self.runtime
                    .send_tagged_text(&self.label, &id, text.clone(), tags)
                    .await?
            }
        };
        summary
            .message_ids
            .into_iter()
            .next()
            .map(EventKey)
            .ok_or(AppError::UnknownGroup(String::new()))
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
            blossom_server: self.media_server.clone(),
            message_tags: carrier::words_tags(turn, false),
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

    /// The groups the agent has accepted.
    pub fn groups(&self) -> Vec<GroupKey> {
        self.app
            .groups(&self.label)
            .unwrap_or_default()
            .into_iter()
            .filter(|g| !g.pending_confirmation && !g.archived)
            .map(|g| GroupKey(g.group_id_hex))
            .collect()
    }

    /// The chat messages, answers and controls others sent to a group that arrived here after
    /// `after` in storage order, in that order, however many there are.
    pub fn stored(&self, group: &GroupKey, after: i64) -> Vec<Message> {
        let mut rows: Vec<AppMessageRecord> = self
            .stored_all(group)
            .into_iter()
            .filter(|r| r.insert_order > after && r.direction == "received" && !r.invalidated)
            .collect();
        rows.sort_by_key(|r| r.insert_order);
        rows.into_iter().map(Message::from).collect()
    }

    /// The latest place in storage order for a group: where a group the core has never seen
    /// starts, so its history is not read as prompts.
    pub fn latest(&self, group: &GroupKey) -> i64 {
        self.stored_all(group)
            .iter()
            .map(|r| r.insert_order)
            .max()
            .unwrap_or(0)
    }

    /// A message's place in storage order. `None` when it is not stored, as for one MDK removed.
    pub fn order_of(&self, message: &Message) -> Option<i64> {
        if message.order.is_some() {
            return message.order;
        }
        self.runtime
            .message_by_id(&self.label, message.group.as_str(), message.event.as_str())
            .ok()
            .flatten()
            .map(|r| r.insert_order)
    }

    /// Every chat message, answer and control stored for the group. Not only the most recent: MDK
    /// picks those by the time their senders gave, not by when they arrived here, so a message that
    /// arrived late with an earlier time could fall outside the window and never be read.
    fn stored_all(&self, group: &GroupKey) -> Vec<AppMessageRecord> {
        let query = AppMessageQuery {
            group_id_hex: Some(group.as_str().to_string()),
            kinds: Some(vec![CHAT, REQUEST_KIND]),
            limit: None,
        };
        self.runtime
            .messages_with_query(&self.label, query)
            .unwrap_or_default()
    }
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
