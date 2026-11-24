//! The simulated MDK, as the core sees it through [`Side`]: what the real [`Marmot`] does, over
//! the world's storage, with the faults the schedule armed.
//!
//! [`Marmot`]: marmot_agent::marmot::Marmot

use crate::schedule::{Outcome, ReadStep};
use crate::world::{CHAT, COMMENT, EDIT, Kept, Landing, RefusedSend, World, describe};
use marmot_agent::carrier::{self, REQUEST_KIND};
use marmot_agent::files::Outgoing;
use marmot_agent::marmot::{FileFailed, Incoming, Live, Message, NotNow, Page, SendFailed, Side};
use marmot_agent::run::REJECTED_TRIES;
use marmot_agent::{EventKey, GroupKey, Heard, HeardRequest, PubKey, Sending, Token, Where};
use marmot_agent_events::TurnId;
use marmot_agent_events::body::{Body, NoticeKind};
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
    /// A page of what MDK stored for a chat after `after`: at most `limit` rows of its chat
    /// messages and requests, the agent's own too, in storage order, as MDK's
    /// `messages_after_insert_order` gives them. The page holds what others sent.
    fn page(world: &World, chat: usize, after: i64, limit: usize) -> Page {
        let group = world.chats[chat].group.clone();
        let mut rows: Vec<&crate::world::Row> = (world.rows.iter())
            .filter(|r| r.chat == chat && r.order > after)
            .filter(|r| r.kind == CHAT || r.kind == REQUEST_KIND)
            .collect();
        rows.sort_by_key(|r| r.order);
        rows.truncate(limit);
        let messages = (rows.iter())
            .filter(|r| !r.from_agent)
            .map(|r| Message {
                group: group.clone(),
                event: r.event.clone(),
                sender: world.owner.clone(),
                sender_name: Some("Owner".into()),
                kind: r.kind,
                content: r.content.clone(),
                tags: r.tags.clone(),
                recorded_at: r.recorded_at,
                received_at: World::secs_at(r.ms),
                order: Some(r.order),
            })
            .collect();
        Page {
            messages,
            last: rows.last().map(|r| r.order),
            more: rows.len() >= limit,
        }
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

    async fn publish_name(&self, _name: &str, _relays: &[String]) -> Result<(), NotNow> {
        Ok(())
    }

    async fn incoming(&self, message: Message) -> Result<Option<Incoming>, NotNow> {
        let mut world = lock(&self.world);
        if message.sender == world.agent {
            return Ok(None);
        }
        let Some(chat) = world.chat_of(&message.group) else {
            return Ok(None);
        };
        if !world.chats[chat].accepted {
            return Ok(None);
        }
        if world.read_fails(ReadStep::Place) {
            return Err(NotNow);
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
            let Some((target, asked)) = carrier::read_request(&world.agent, &message.tags) else {
                return Ok(None);
            };
            // The core hands on what this request becomes before it reads another.
            world.reading = (world.msgs.iter())
                .position(|m| m.as_ref().is_some_and(|m| m.event == message.event));
            return Ok(Some(Incoming::Asked(HeardRequest {
                chat: place,
                event: message.event,
                author: message.sender,
                author_name: "Owner".into(),
                age_secs: world.secs().saturating_sub(message.recorded_at),
                target,
                asked,
            })));
        }
        if message.kind != CHAT {
            return Ok(None);
        }
        Ok(Some(Incoming::Heard(Heard {
            chat: place,
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

    fn groups(&self) -> Result<Vec<GroupKey>, NotNow> {
        let mut world = lock(&self.world);
        if world.read_fails(ReadStep::Groups) {
            return Err(NotNow);
        }
        Ok((world.chats.iter())
            .filter(|c| c.accepted)
            .map(|c| c.group.clone())
            .collect())
    }

    /// Every chat of a simulation is the owner's two-person chat with the agent.
    fn two_people(&self, group: &GroupKey) -> Result<Option<Vec<PubKey>>, NotNow> {
        let world = lock(&self.world);
        Ok(world
            .chat_of(group)
            .map(|_| vec![world.agent.clone(), world.owner.clone()]))
    }

    fn stored(&self, group: &GroupKey, after: i64, limit: usize) -> Result<Page, NotNow> {
        let mut world = lock(&self.world);
        world.land();
        let Some(chat) = world.chat_of(group) else {
            return Ok(Page::default());
        };
        if world.read_fails(ReadStep::Stored) {
            return Err(NotNow);
        }
        let page = Self::page(&world, chat, after, limit);
        let now = world.ms();
        for message in &page.messages {
            if let Some(m) = (world.msgs.iter_mut().flatten()).find(|m| m.event == message.event) {
                m.read_ms = Some(now);
            }
        }
        // A read that finds nothing, as most of the tick's do, is not noted.
        if !page.messages.is_empty() {
            world.note(format!("CatchUp(c{chat}, {} stored)", page.messages.len()));
        }
        Ok(page)
    }

    async fn send(
        &self,
        group: &GroupKey,
        what: &Sending,
        token: &Token,
    ) -> Result<EventKey, SendFailed> {
        let (result, crash) = {
            let mut world = lock(&self.world);
            world.land();
            if world.effect("Send") {
                (Err(SendFailed::Busy), true)
            } else {
                send(&mut world, group, what, token)
            }
        };
        if crash {
            std::future::pending::<()>().await;
        }
        result
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
/// outcome the schedule armed for it. The second value says the core crashes now. MDK keeps each
/// send it took under its token ([`Token::mdk`]), as its `submit_*` calls do: a send tried again
/// under a token it keeps meets no armed outcome, and is answered from what became of the first:
/// the same event once it went out, an unknown outcome while it is still to land, and its drop
/// when MDK dropped it. A token it keeps for other bytes is refused.
fn send(
    world: &mut World,
    group: &GroupKey,
    what: &Sending,
    token: &Token,
) -> (Result<EventKey, SendFailed>, bool) {
    let Some(chat) = world.chat_of(group) else {
        return (Err(SendFailed::Refused { gone: false }), false);
    };
    // What MDK would store, as the real Marmot side builds it.
    let built = match what {
        Sending::Item { item, .. } => carrier::item(item).map(|out| {
            let edits = out.edits().map(EventKey::from);
            (out.kind, out.content, out.tags, edits)
        }),
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
            thread,
            ..
        } => {
            let mut tags = carrier::words_tags(turn.as_ref(), *remark, settles);
            // A subagent's words are MDK's thread message, tagged from its parent (D-122).
            let kind = match thread {
                Some(thread) => {
                    tags.extend(carrier::thread_tags(thread, true));
                    tags.retain(|t| t[0] != "p");
                    COMMENT
                }
                None => CHAT,
            };
            carrier::fit_words(text, &tags).map(|text| (kind, text, tags, None))
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
    let seq = match what {
        Sending::Item { item, .. } => Some(item.event.header().seq),
        Sending::Words { .. } => None,
    };
    let (state, notice) = match what {
        Sending::Item { item, .. } => (
            Some(item.event.header().state),
            match item.event.body() {
                Body::Notice(n) if matches!(n.kind, NoticeKind::Refused | NoticeKind::Warning) => {
                    n.reason.clone()
                }
                _ => None,
            },
        ),
        Sending::Words { .. } => (None, None),
    };
    let dropped = RefusedSend {
        chat,
        item,
        settles,
        words,
        state,
        seq,
        notice,
    };
    let Ok((kind, content, tags, edits)) = built else {
        world.refused.push(dropped);
        world.note(format!("Send(c{chat}): Refused, too large"));
        return (Err(SendFailed::Refused { gone: false }), false);
    };
    if let Some(why) = misshapen(what, kind, &tags, token) {
        let why = format!("{why} in c{chat}");
        world.note(format!("Send(c{chat}): Refused, {why}"));
        world.misshapen.push(why);
        world.refused.push(dropped);
        return (Err(SendFailed::Refused { gone: false }), false);
    }
    // MDK edits the event the edit's original token went under, and places an item's `e` tag
    // itself (its `submit_tagged_edit_for_local_send`): it never reads the target the core names.
    let (tags, edits) = match (edits, &token.edits) {
        (Some(named), Some(original)) => {
            let Some(kept) = world.tokens.get(&(chat, original.clone())) else {
                let why = format!("an edit under a token MDK does not keep in c{chat}");
                world.note(format!("Send(c{chat}): Refused, {why}"));
                world.misshapen.push(why);
                world.refused.push(dropped);
                return (Err(SendFailed::Refused { gone: false }), false);
            };
            let target = kept.event.clone();
            let tags = if carrier::edit_target(&tags) == Some(named.as_str()) {
                let mut tags = carrier::but_target(&tags);
                tags.push(vec!["e".into(), target.as_str().into()]);
                tags
            } else {
                tags
            };
            (tags, Some(target))
        }
        (edits, _) => (tags, edits),
    };
    let request = (kind, content.clone(), tags.clone(), edits.clone());
    if let Some(kept) = world.tokens.get(&(chat, token.mdk.clone())).cloned() {
        return (answer_again(world, chat, &kept, &request, dropped), false);
    }
    let row = world.store_sent(chat, kind, content, tags, edits);
    let described = describe(&row);
    let event = row.event.clone();
    let outcome = world.outcomes.pop_front();
    // What MDK keeps under the token: every outcome but those where it never had the send.
    if !matches!(
        outcome,
        Some(Outcome::Busy | Outcome::Ambiguous | Outcome::Refused)
    ) {
        let rejected = outcome == Some(Outcome::Rejected);
        let kept = Kept {
            request,
            event,
            rejected,
        };
        world.tokens.insert((chat, token.mdk.clone()), kept);
    }
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
            world.refused.push(dropped);
            world.note(format!("Send({described}): Refused"));
            (Err(SendFailed::Refused { gone: false }), false)
        }
        Some(Outcome::Rejected) => {
            world.note(format!("Send({described}): Rejected"));
            (Err(dropped_try(world, &token.send, dropped)), false)
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

/// MDK's answer to a send tried again under a token it keeps: `request` is what it carries now.
fn answer_again(
    world: &mut World,
    chat: usize,
    kept: &Kept,
    request: &crate::world::Request,
    dropped: RefusedSend,
) -> Result<EventKey, SendFailed> {
    if kept.request != *request {
        // Never made by the core: a token names one send. The core drops it as refused.
        world.refused.push(dropped);
        world.note(format!("Send(c{chat}): Refused, token taken"));
        return Err(SendFailed::Refused { gone: false });
    }
    if kept.rejected {
        world.note(format!("Send(c{chat}): Rejected again"));
        return Err(SendFailed::Rejected);
    }
    if (world.rows.iter()).any(|r| r.event == kept.event) {
        world.note(format!("Send(c{chat}): Sent before, same token"));
        return Ok(kept.event.clone());
    }
    world.note(format!("Send(c{chat}): still to land, same token"));
    Err(SendFailed::Unknown)
}

/// MDK dropped a try of the send named `send`: the core tries it again under a new token, until
/// it has dropped [`REJECTED_TRIES`] of them, when the core gives it up as refused, and it is
/// recorded among the sends dropped.
fn dropped_try(world: &mut World, send: &str, dropped: RefusedSend) -> SendFailed {
    let chat = dropped.chat;
    let tries = world.drops.entry((chat, send.to_string())).or_default();
    *tries += 1;
    if *tries >= REJECTED_TRIES {
        world.note(format!("Send(c{chat}): given up"));
        world.refused.push(dropped);
    }
    SendFailed::Rejected
}

/// Why MDK would refuse an item the core sends, as its own checks do (its `submit_custom_event`
/// for a first version, and `submit_tagged_edit_for_local_send` for a later one): tags MDK does
/// not read as an item, or a later version with no token for its first event to name. `None`
/// for anything else.
fn misshapen(what: &Sending, kind: u64, tags: &[Vec<String>], token: &Token) -> Option<String> {
    let Sending::Item { item, .. } = what else {
        return None;
    };
    let name = format!("{} v{}", item.event.header().item.as_str(), item.version);
    let edit = kind == EDIT;
    // A first event is checked as it is sent; an edit without the `e` MDK places itself, as the
    // real Marmot side hands it over.
    let own = if edit {
        carrier::but_target(tags)
    } else {
        tags.to_vec()
    };
    let checked = if edit {
        cgka_traits::agent_events::validate_agent_item_edit_tags(&own)
    } else {
        cgka_traits::agent_events::validate_agent_item_tags(&own)
    };
    if let Err(error) = checked {
        return Some(format!("{name}: {error}"));
    }
    if edit != item.first.is_some() || (edit && token.edits.is_none()) {
        return Some(format!("{name}: not an edit of its first event"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use marmot_agent::carrier::Item;
    use marmot_agent_events::body::{StartedBy, Turn};
    use marmot_agent_events::{AgentEvent, Header, State};

    fn version(first: Option<&str>) -> Sending {
        let header = Header {
            harness: "sim".into(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            subagent: None,
            item: "t1".into(),
            seq: 1,
            state: State::Running,
            title: "Turn".into(),
            text: "turn".into(),
        };
        let body = Body::Turn(Turn {
            started_by: StartedBy::User,
            duration_ms: None,
            usage: None,
        });
        Sending::Item {
            key: "t1".into(),
            item: Box::new(Item {
                event: AgentEvent::new(header, body, None).expect("no secret"),
                version: if first.is_some() { 2 } else { 1 },
                first: first.map(EventKey::from),
                started_by: None,
                settles: Vec::new(),
                thread: None,
            }),
        }
    }

    // MDK refuses a first event that carries an `e` tag, which is an edit's target for MDK to
    // place; an edit's own `e` is MDK's, and is not checked as the item's.
    #[test]
    fn a_first_event_is_checked_with_every_tag_it_carries() {
        let token = |edits: Option<&str>| Token {
            send: "a".into(),
            mdk: "a".into(),
            edits: edits.map(Into::into),
            tried_at: None,
        };
        let first = version(None);
        let Sending::Item { item, .. } = &first else {
            unreachable!()
        };
        let out = carrier::item(item).expect("fits");
        assert_eq!(misshapen(&first, out.kind, &out.tags, &token(None)), None);
        let mut tags = out.tags.clone();
        tags.push(vec!["e".into(), "0".repeat(64)]);
        assert!(misshapen(&first, out.kind, &tags, &token(None)).is_some());

        let edit = version(Some(&"1".repeat(64)));
        let Sending::Item { item, .. } = &edit else {
            unreachable!()
        };
        let out = carrier::item(item).expect("fits");
        assert_eq!(
            misshapen(&edit, out.kind, &out.tags, &token(Some("a"))),
            None
        );
    }
}
