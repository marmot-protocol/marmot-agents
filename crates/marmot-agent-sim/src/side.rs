//! The simulated MDK, as the core sees it through [`Side`]: what the real [`Marmot`] does, over
//! the world's storage, with the faults the schedule armed.
//!
//! [`Marmot`]: marmot_agent::marmot::Marmot

use crate::schedule::Outcome;
use crate::world::{CHAT, EDIT, Landing, RefusedSend, World, describe};
use marmot_agent::carrier::{self, ITEM_KIND, REQUEST_KIND};
use marmot_agent::files::Outgoing;
use marmot_agent::marmot::{FileFailed, Incoming, Live, LookupFailed, Message, SendFailed, Side};
use marmot_agent::{EventKey, GroupKey, Heard, HeardRequest, PubKey, Sending, Where};
use marmot_agent_events::TurnId;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// The world, shared by the simulator, the core's Marmot side and its driver.
pub type Shared = Arc<Mutex<World>>;

/// Locks the world. Nothing holds it across a wait, so it is never poisoned but by a panic,
/// which ends the run anyway.
pub fn lock(world: &Shared) -> std::sync::MutexGuard<'_, World> {
    world
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The core's Marmot side in a simulation.
#[derive(Clone)]
pub struct SimSide {
    /// The world.
    pub world: Shared,
    /// Rung when a live event is waiting.
    pub notify: Arc<Notify>,
    /// The agent.
    pub agent: PubKey,
}

/// Where a core's live events come from: the world's queue, while that core runs.
pub struct SimEvents;

impl SimSide {
    fn row_messages(world: &World, chat: usize, after: i64) -> Vec<Message> {
        let group = world.chats[chat].group.clone();
        let mut rows: Vec<&crate::world::Row> = (world.rows.iter())
            .filter(|r| r.chat == chat && !r.from_agent && r.order > after)
            .filter(|r| r.kind == CHAT || r.kind == REQUEST_KIND)
            .collect();
        rows.sort_by_key(|r| r.order);
        rows.into_iter()
            .map(|r| Message {
                group: group.clone(),
                event: r.event.clone(),
                sender: world.owner.clone(),
                sender_name: Some("Owner".into()),
                kind: r.kind,
                content: r.content.clone(),
                tags: r.tags.clone(),
                recorded_at: r.recorded_at,
                order: Some(r.order),
            })
            .collect()
    }
}

impl Side for SimSide {
    type Events = SimEvents;

    fn agent(&self) -> &PubKey {
        &self.agent
    }

    async fn next(&self, _events: &mut SimEvents) -> Live {
        loop {
            if let Some(live) = lock(&self.world).live.pop_front() {
                return live;
            }
            self.notify.notified().await;
        }
    }

    async fn publish_name(&self, _name: &str, _relays: &[String]) {}

    async fn incoming(&self, message: Message) -> Option<Incoming> {
        let mut world = lock(&self.world);
        if message.sender == world.agent {
            return None;
        }
        let chat = world.chat_of(&message.group)?;
        if !world.chats[chat].accepted {
            return None;
        }
        // When the core last read it, for the oracle that knows a late request is dropped.
        let now = world.ms();
        if let Some(m) = (world.msgs.iter_mut().flatten()).find(|m| m.event == message.event) {
            m.read_ms = Some(now);
        }
        let place = Where {
            group: message.group.clone(),
            name: None,
            direct: true,
            members: 2,
        };
        if message.kind == REQUEST_KIND {
            let (target, asked) = carrier::read_request(&world.agent, &message.tags)?;
            return Some(Incoming::Asked(HeardRequest {
                chat: place,
                event: message.event,
                author: message.sender,
                author_name: "Owner".into(),
                age_secs: world.secs().saturating_sub(message.recorded_at),
                target,
                asked,
            }));
        }
        if message.kind != CHAT {
            return None;
        }
        Some(Incoming::Heard(Heard {
            chat: place,
            event: message.event,
            author: message.sender,
            author_name: "Owner".into(),
            text: message.content,
            reply_to: None,
            mentions_agent: false,
            attachments: Vec::new(),
        }))
    }

    fn order_of(&self, message: &Message) -> Option<i64> {
        if message.order.is_some() {
            return message.order;
        }
        let mut world = lock(&self.world);
        world.land();
        (world.rows.iter())
            .find(|r| r.event == message.event)
            .map(|r| r.order)
    }

    fn invitations(&self) -> Vec<(GroupKey, Option<PubKey>)> {
        let world = lock(&self.world);
        (world.chats.iter())
            .filter(|c| c.invited && !c.accepted)
            .map(|c| (c.group.clone(), Some(world.owner.clone())))
            .collect()
    }

    async fn decide(&self, group: &GroupKey, accept: bool) -> bool {
        let crash = {
            let mut world = lock(&self.world);
            let crash = world.effect("Accept");
            if !crash && let Some(chat) = world.chat_of(group) {
                if accept {
                    world.accepted(chat);
                } else {
                    world.chats[chat].invited = false;
                }
            }
            crash
        };
        if crash {
            std::future::pending::<()>().await;
        }
        true
    }

    fn groups(&self) -> Vec<GroupKey> {
        let world = lock(&self.world);
        (world.chats.iter())
            .filter(|c| c.accepted)
            .map(|c| c.group.clone())
            .collect()
    }

    fn latest(&self, group: &GroupKey) -> i64 {
        let mut world = lock(&self.world);
        world.land();
        let Some(chat) = world.chat_of(group) else {
            return 0;
        };
        (world.rows.iter())
            .filter(|r| r.chat == chat && (r.kind == CHAT || r.kind == REQUEST_KIND))
            .map(|r| r.order)
            .max()
            .unwrap_or(0)
    }

    fn stored(&self, group: &GroupKey, after: i64) -> Vec<Message> {
        let mut world = lock(&self.world);
        world.land();
        let Some(chat) = world.chat_of(group) else {
            return Vec::new();
        };
        let messages = Self::row_messages(&world, chat, after);
        let now = world.ms();
        for message in &messages {
            if let Some(m) = (world.msgs.iter_mut().flatten()).find(|m| m.event == message.event) {
                m.read_ms = Some(now);
            }
        }
        world.note(format!("CatchUp(c{chat}, {} stored)", messages.len()));
        messages
    }

    async fn send(
        &self,
        group: &GroupKey,
        what: &Sending,
        _token: &str,
    ) -> Result<EventKey, SendFailed> {
        let (result, crash) = {
            let mut world = lock(&self.world);
            world.land();
            if world.effect("Send") {
                (Err(SendFailed::Busy), true)
            } else {
                send(&mut world, group, what)
            }
        };
        if crash {
            std::future::pending::<()>().await;
        }
        result
    }

    fn find_sent(
        &self,
        group: &GroupKey,
        what: &Sending,
        taken: &dyn Fn(&EventKey) -> bool,
    ) -> Result<Option<EventKey>, LookupFailed> {
        let mut world = lock(&self.world);
        world.land();
        if world.lookup_errors > 0 {
            world.lookup_errors -= 1;
            world.note("Lookup: Error");
            return Err(LookupFailed);
        }
        let Some(chat) = world.chat_of(group) else {
            return Ok(None);
        };
        let found = find(&world, chat, what, taken);
        world.note(match found {
            Some(_) => "Lookup: Found",
            None => "Lookup: Absent",
        });
        Ok(found)
    }

    async fn send_file(
        &self,
        _group: &GroupKey,
        _file: &Outgoing,
        _turn: Option<&TurnId>,
    ) -> Result<EventKey, FileFailed> {
        // The scripted driver sends no files.
        Err(FileFailed::Failed)
    }
}

/// What the simulated MDK does with a send: what it stores, and what it answers, with the
/// outcome the schedule armed for it. The second value says the core crashes now.
fn send(
    world: &mut World,
    group: &GroupKey,
    what: &Sending,
) -> (Result<EventKey, SendFailed>, bool) {
    let Some(chat) = world.chat_of(group) else {
        return (Err(SendFailed::Refused { gone: false }), false);
    };
    // What MDK would store, as the real Marmot side builds it.
    let built = match what {
        Sending::Item { item, .. } => {
            carrier::item(item).map(|out| (out.kind, out.content, out.tags, None))
        }
        Sending::Words {
            edit: Some(target),
            text,
            ..
        } => {
            carrier::fit_words(text, &[]).map(|text| (EDIT, text, Vec::new(), Some(target.clone())))
        }
        Sending::Words {
            text,
            turn,
            remark,
            settles,
            ..
        } => {
            let tags = carrier::words_tags(turn.as_ref(), *remark, settles);
            carrier::fit_words(text, &tags).map(|text| (CHAT, text, tags, None))
        }
    };
    let item = match what {
        Sending::Item { item, .. } => {
            Some((item.event.header().item.as_str().to_string(), item.version))
        }
        Sending::Words { .. } => None,
    };
    let words = match what {
        Sending::Words { text, .. } => Some(text.clone()),
        Sending::Item { .. } => None,
    };
    let settles = match what {
        Sending::Item { item, .. } => item.settles.clone(),
        Sending::Words { settles, .. } => settles.clone(),
    };
    let Ok((kind, content, tags, edits)) = built else {
        world.refused.push(RefusedSend {
            chat,
            item,
            settles,
            words,
        });
        world.note(format!("Send(c{chat}): Refused, too large"));
        return (Err(SendFailed::Refused { gone: false }), false);
    };
    let row = world.store_sent(chat, kind, content, tags, edits);
    let described = describe(&row);
    let outcome = world.outcomes.pop_front();
    match outcome {
        None => {
            let event = world.keep(row);
            world.note(format!("Send({described}): Sent"));
            (Ok(event), false)
        }
        Some(Outcome::Busy) => {
            world.note(format!("Send({described}): Busy"));
            (Err(SendFailed::Busy), false)
        }
        Some(Outcome::Refused) => {
            world.refused.push(RefusedSend {
                chat,
                item,
                settles,
                words,
            });
            world.note(format!("Send({described}): Refused"));
            (Err(SendFailed::Refused { gone: false }), false)
        }
        Some(Outcome::Ambiguous) => {
            world.note(format!("Send({described}): Ambiguous, not sent"));
            (Err(SendFailed::Unknown), false)
        }
        Some(Outcome::AmbiguousLanded) => {
            world.keep(row);
            world.note(format!("Send({described}): Ambiguous, sent"));
            (Err(SendFailed::Unknown), false)
        }
        Some(Outcome::CrashAfter) => {
            world.keep(row);
            world.crashed = true;
            world.note(format!("Send({described}): Sent, then Crash"));
            (Err(SendFailed::Unknown), true)
        }
        Some(Outcome::Late { ms }) => {
            let at = tokio::time::Instant::now() + Duration::from_millis(ms);
            world.landings.push(Landing { at, row });
            world.note(format!("Send({described}): Ambiguous, lands in {ms} ms"));
            (Err(SendFailed::Unknown), false)
        }
    }
}

/// The event a send became, as the real Marmot side looks for it: an item by its id, its
/// version and its first event; the agent's words by their text and their tags, among those not
/// already known as the first of something; an edit never.
fn find(
    world: &World,
    chat: usize,
    what: &Sending,
    taken: &dyn Fn(&EventKey) -> bool,
) -> Option<EventKey> {
    let tag = |tags: &[Vec<String>], name: &str| -> Option<String> {
        (tags.iter())
            .find(|t| t.len() >= 2 && t[0] == name)
            .map(|t| t[1].clone())
    };
    let ours = (world.rows.iter()).filter(|r| r.chat == chat && r.from_agent);
    match what {
        Sending::Item { item, .. } => {
            let id = item.event.header().item.as_str().to_string();
            let version = item.version.to_string();
            let first = item.first.as_ref().map(|f| f.as_str().to_string());
            ours.filter(|r| r.kind == ITEM_KIND)
                .filter(|r| {
                    tag(&r.tags, "item_id").as_deref() == Some(id.as_str())
                        && tag(&r.tags, "v").as_deref() == Some(version.as_str())
                        && tag(&r.tags, "item") == first
                })
                .map(|r| r.event.clone())
                .find(|e| !taken(e))
        }
        Sending::Words { edit: Some(_), .. } => None,
        Sending::Words {
            text,
            turn,
            remark,
            settles,
            ..
        } => {
            let tags = carrier::words_tags(turn.as_ref(), *remark, settles);
            let text = carrier::fit_words(text, &tags).ok()?;
            let named = |tags: &[Vec<String>], name: &str| -> Vec<Vec<String>> {
                (tags.iter())
                    .filter(|t| t.first().map(String::as_str) == Some(name))
                    .cloned()
                    .collect()
            };
            let wanted = [
                named(&tags, "turn"),
                named(&tags, "phase"),
                named(&tags, "settles"),
            ];
            ours.filter(|r| r.kind == CHAT && r.content == text)
                .filter(|r| {
                    [
                        named(&r.tags, "turn"),
                        named(&r.tags, "phase"),
                        named(&r.tags, "settles"),
                    ] == wanted
                })
                .map(|r| r.event.clone())
                .find(|e| !taken(e))
        }
    }
}
