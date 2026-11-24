//! The properties, checked from outside the core: from what the simulated MDK stored and was
//! refused, and from what the scripted harness was handed and emitted. Nothing here reads the
//! core's state. The ids are those of `docs/design/verification.md`.

use crate::schedule::What;
use crate::world::{CHAT, COMMENT, EDIT, Row, World};
use marmot_agent::EventKey;
use marmot_agent::carrier;
use marmot_agent::conversation::EFFECT_LIMIT_SECS;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A property that does not hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// Its id, such as `S3`.
    pub property: String,
    /// What was seen, in the simulator's own names.
    pub detail: String,
}

fn fail(property: &str, detail: String) -> Failure {
    Failure {
        property: property.into(),
        detail,
    }
}

/// One version of an item the agent sent.
struct Version<'a> {
    row: &'a Row,
    id: String,
    version: u64,
    first: Option<EventKey>,
    state: State,
    seq: u64,
    turn: bool,
    started_by: Option<EventKey>,
    notice: Option<String>,
}

fn versions(world: &World) -> Vec<Version<'_>> {
    (world.rows.iter())
        .filter(|r| r.from_agent)
        .filter_map(|row| {
            let read = carrier::read_item(row.kind, &row.content, &row.tags)?;
            let header = read.item.event.header();
            Some(Version {
                row,
                id: header.item.as_str().to_string(),
                version: read.item.version,
                first: read.item.first.clone(),
                state: header.state,
                seq: header.seq,
                turn: matches!(read.item.event.body(), Body::Turn(_)),
                started_by: read.item.started_by.clone(),
                notice: match read.item.event.body() {
                    Body::Notice(n) => Some(n.reason.clone().unwrap_or_default()),
                    _ => None,
                },
            })
        })
        .collect()
}

/// What each event the agent sent settles: (the row, the requests).
fn settling(world: &World) -> Vec<(&Row, Vec<EventKey>)> {
    (world.rows.iter())
        .filter(|r| r.from_agent)
        .filter_map(|row| {
            let settles: Vec<EventKey> =
                if let Some(read) = carrier::read_item(row.kind, &row.content, &row.tags) {
                    read.item.settles
                } else if row.kind == CHAT || row.kind == COMMENT {
                    (row.tags.iter())
                        .filter(|t| t.len() >= 2 && t[0] == "settles")
                        .map(|t| EventKey(t[1].clone()))
                        .collect()
                } else {
                    Vec::new()
                };
            (!settles.is_empty()).then_some((row, settles))
        })
        .collect()
}

fn final_state(state: State) -> bool {
    !matches!(state, State::Running | State::Waiting)
}

/// The message a person posted with this event.
fn msg_of(world: &World, event: &EventKey) -> Option<usize> {
    (world.msgs.iter()).position(|m| m.as_ref().is_some_and(|m| m.event == *event))
}

/// The safety properties, after a step: S1 (at most once, and by the right event), S3, S4, S5,
/// S10 and S12. The first that fails.
pub fn safety(world: &World) -> Option<Failure> {
    s1(world)
        .or_else(|| s3(world))
        .or_else(|| s4(world))
        .or_else(|| s5(world))
        .or_else(|| s10(world))
        .or_else(|| s12(world))
        .or_else(|| s14(world))
}

/// The earliest the core's `no_effect` net may settle a request after the harness took it, in
/// milliseconds. The harness takes a request as the driver answers the core, which is when the
/// core sets its deadline; it waits [`EFFECT_LIMIT_SECS`] from there on a clock of whole seconds,
/// so it may fire up to a second early, and later by up to its 10-second tick. An effect shown
/// before this was shown in time.
pub const NET_EARLIEST_MS: u64 = (EFFECT_LIMIT_SECS - 1) * 1000;

/// S1: every answer and control is settled at most once, in its own chat (S5), and not by the
/// core's `no_effect` net when the harness showed its effect in time.
fn s1(world: &World) -> Option<Failure> {
    let mut count: BTreeMap<EventKey, usize> = BTreeMap::new();
    for (row, settles) in settling(world) {
        for request in settles {
            let Some(msg) = msg_of(world, &request) else {
                return Some(fail(
                    "S1",
                    format!("c{} settles an event no one posted", row.chat),
                ));
            };
            let m = world.msgs[msg].as_ref().expect("posted");
            if !m.posted.is_request() {
                return Some(fail(
                    "S1",
                    format!("m{msg} is settled, and is not a request"),
                ));
            }
            if usize::from(m.posted.chat) != row.chat {
                return Some(fail(
                    "S5",
                    format!("m{msg} of c{} is settled in c{}", m.posted.chat, row.chat),
                ));
            }
            let n = count.entry(request).or_default();
            *n += 1;
            if *n > 1 {
                return Some(fail("S1", format!("m{msg} is settled {n} times")));
            }
        }
    }
    // Settled by the net although the harness named it before the net could fire.
    let net: BTreeSet<EventKey> = (versions(world).into_iter())
        .filter(|v| v.notice.as_deref() == Some("no_effect"))
        .flat_map(|v| {
            carrier::read_item(v.row.kind, &v.row.content, &v.row.tags)
                .map(|r| r.item.settles)
                .unwrap_or_default()
        })
        .collect();
    let h = &world.harness;
    for (request, (msg, taken)) in &h.requests {
        let named = (h.emissions.iter())
            .any(|e| e.settles.contains(request) && e.ms < taken + NET_EARLIEST_MS);
        let event = world.msgs[*msg].as_ref().map(|m| m.event.clone());
        if named && event.is_some_and(|e| net.contains(&e)) {
            return Some(fail(
                "S1",
                format!("m{msg} is settled as having no effect, though the harness showed it"),
            ));
        }
    }
    None
}

/// S3: an item's versions rise by one, skipping only versions MDK refused, and every later
/// version names its first event; the agent's words go out as a new message once.
fn s3(world: &World) -> Option<Failure> {
    if let Some(why) = world.misshapen.first() {
        return Some(fail("S3", format!("MDK could not take {why}")));
    }
    let refused: BTreeSet<(usize, String, u64)> = (world.refused.iter())
        .filter_map(|r| r.item.clone().map(|(id, v)| (r.chat, id, v)))
        .collect();
    let mut seen: BTreeMap<(usize, String), (EventKey, u64)> = BTreeMap::new();
    for v in versions(world) {
        let key = (v.row.chat, v.id.clone());
        match seen.get_mut(&key) {
            None => {
                if v.first.is_some() {
                    return Some(fail(
                        "S3",
                        format!(
                            "c{} {} v{} names a first event none was sent",
                            key.0, v.id, v.version
                        ),
                    ));
                }
                for missing in 1..v.version {
                    if !refused.contains(&(key.0, v.id.clone(), missing)) {
                        return Some(fail(
                            "S3",
                            format!(
                                "c{} {} v{} is the first stored, skipping v{missing}",
                                key.0, v.id, v.version
                            ),
                        ));
                    }
                }
                seen.insert(key, (v.row.event.clone(), v.version));
            }
            Some((first, last)) => {
                if v.version <= *last {
                    return Some(fail(
                        "S3",
                        format!("c{} {} v{} follows v{}", key.0, v.id, v.version, last),
                    ));
                }
                if v.first.as_ref() != Some(first) {
                    return Some(fail(
                        "S3",
                        format!(
                            "c{} {} v{} does not name its first event",
                            key.0, v.id, v.version
                        ),
                    ));
                }
                for missing in *last + 1..v.version {
                    if !refused.contains(&(key.0, v.id.clone(), missing)) {
                        return Some(fail(
                            "S3",
                            format!(
                                "c{} {} v{} follows v{}, skipping v{missing}",
                                key.0, v.id, v.version, last
                            ),
                        ));
                    }
                }
                *last = v.version;
            }
        }
    }
    let mut words: BTreeSet<(usize, &str)> = BTreeSet::new();
    // A subagent's words are a thread comment, a new message too (D-122).
    let new = |r: &&Row| r.from_agent && (r.kind == CHAT || r.kind == COMMENT);
    for row in world.rows.iter().filter(new) {
        if !words.insert((row.chat, row.content.as_str())) {
            return Some(fail(
                "S3",
                format!(
                    "c{} the agent's words {} went out twice as new messages",
                    row.chat, row.content
                ),
            ));
        }
    }
    None
}

/// S4: a prompt reaches the harness at most once, across every crash.
fn s4(world: &World) -> Option<Failure> {
    let mut seen = BTreeSet::new();
    for d in (world.harness.deliveries.iter()).filter(|d| d.kind == "prompt") {
        if let Some(msg) = d.msg
            && !seen.insert(msg)
        {
            return Some(fail("S4", format!("m{msg} reached the harness twice")));
        }
    }
    None
}

/// The chat each of the core's chat ids stands for, learnt from the prompts the harness got.
fn chat_ids(world: &World) -> Result<BTreeMap<String, usize>, Failure> {
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    for d in &world.harness.deliveries {
        let (Some(msg), Some(chat)) = (d.msg, &d.chat) else {
            continue;
        };
        let Some(m) = world.msgs.get(msg).and_then(|m| m.as_ref()) else {
            continue;
        };
        let real = usize::from(m.posted.chat);
        match ids.get(chat) {
            Some(&known) if known != real => {
                return Err(fail(
                    "S5",
                    format!("one chat id was handed m{msg} of c{real} and a message of c{known}"),
                ));
            }
            _ => {
                ids.insert(chat.clone(), real);
            }
        }
    }
    let mut back = BTreeMap::new();
    for (id, chat) in &ids {
        if let Some(other) = back.insert(*chat, id) {
            let _ = other;
            return Err(fail("S5", format!("c{chat} has two chat ids")));
        }
    }
    Ok(ids)
}

/// S5: a request, an item and the agent's words reach only their own chat, and an edit of the
/// agent's words edits their own message there. A subagent's words are a thread comment, and a
/// later version of them MDK's plain edit of it (D-142).
fn s5(world: &World) -> Option<Failure> {
    let ids = match chat_ids(world) {
        Ok(ids) => ids,
        Err(failure) => return Some(failure),
    };
    let mut item_chat: BTreeMap<&str, usize> = BTreeMap::new();
    for e in &world.harness.emissions {
        if let Some(&chat) = ids.get(&e.chat) {
            item_chat.insert(e.item.as_str(), chat);
        }
    }
    for v in versions(world) {
        if let Some(&chat) = item_chat.get(v.id.as_str())
            && chat != v.row.chat
        {
            return Some(fail(
                "S5",
                format!("{} of c{chat} was sent to c{}", v.id, v.row.chat),
            ));
        }
    }
    // Each text of the agent's words comes from one emission, of one item, in one chat.
    let mut said: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for e in &world.harness.emissions {
        if let (Some(text), Some(&chat)) = (&e.words, ids.get(&e.chat)) {
            said.insert(text.as_str(), (chat, e.item.as_str()));
        }
    }
    let words = (world.rows.iter()).filter(|r| {
        let item = carrier::read_item(r.kind, &r.content, &r.tags).is_some();
        r.from_agent && [CHAT, COMMENT, EDIT].contains(&r.kind) && !item
    });
    for row in words {
        let Some(&(chat, item)) = said.get(row.content.as_str()) else {
            continue;
        };
        if row.chat != chat {
            return Some(fail(
                "S5",
                format!("the words of {item} of c{chat} were sent to c{}", row.chat),
            ));
        }
        if row.kind == EDIT {
            let target =
                (row.edits.as_ref()).and_then(|t| world.rows.iter().find(|r| r.event == *t));
            let own = target.is_some_and(|t| {
                t.from_agent
                    && (t.kind == CHAT || t.kind == COMMENT)
                    && t.chat == chat
                    && said
                        .get(t.content.as_str())
                        .is_some_and(|(_, i)| *i == item)
            });
            if !own {
                return Some(fail(
                    "S5",
                    format!("an edit of the words of {item} in c{chat} edits another message"),
                ));
            }
        }
    }
    None
}

/// S10: what reaches the harness from each chat comes in storage order, each once.
fn s10(world: &World) -> Option<Failure> {
    let mut last: BTreeMap<usize, (i64, usize)> = BTreeMap::new();
    for d in &world.harness.deliveries {
        let Some(msg) = d.msg else { continue };
        let Some(m) = world.msgs.get(msg).and_then(|m| m.as_ref()) else {
            continue;
        };
        let Some(order) = m.stored else { continue };
        let chat = usize::from(m.posted.chat);
        if let Some(&(before, prev)) = last.get(&chat)
            && order <= before
        {
            return Some(fail(
                "S10",
                format!("c{chat}: m{msg} reached the harness after m{prev}, stored after it"),
            ));
        }
        last.insert(chat, (order, msg));
    }
    None
}

/// S14: a subagent's item goes in its thread or, failing that, in the main chat, and never moves
/// (D-121, D-133, D-142). An item's first event that carries NIP-22's tags names, as its parent,
/// the first event of its subagent's `task` item in the same chat, stored before it, and as its
/// root that item's root, or that item when it is the root. An item of a subagent's goes in the
/// main chat only when that `task` item had not gone out to the chat before it. Each edit of an
/// item carries the root its first event carries, and none when that carries none.
fn s14(world: &World) -> Option<Failure> {
    let one = |row: &Row, name: &str| {
        (row.tags.iter())
            .find(|t| t.len() >= 2 && t[0] == name)
            .map(|t| t[1].clone())
    };
    let items: Vec<(&Row, carrier::Read)> = (world.rows.iter())
        .filter(|r| r.from_agent)
        .filter_map(|r| Some((r, carrier::read_item(r.kind, &r.content, &r.tags)?)))
        .collect();
    let mut firsts: BTreeMap<(usize, String), &Row> = BTreeMap::new();
    for (row, read) in items.iter().filter(|(r, _)| r.kind == carrier::ITEM_KIND) {
        let id = read.item.event.header().item.as_str().to_string();
        firsts.entry((row.chat, id)).or_insert(row);
    }
    for (row, read) in &items {
        let (root, parent) = (one(row, "E"), one(row, "e"));
        let name = read.item.event.header().item.as_str();
        if row.kind == EDIT {
            let first = (world.rows.iter()).find(|r| Some(&r.event) == row.edits.as_ref());
            if let Some(first) = first
                && one(first, "E") != root
            {
                return Some(fail("S14", format!("an edit of {name} moved it")));
            }
            continue;
        }
        // Its subagent's `task` item in this chat, if it went out before it: the thread it goes in.
        let task = (read.item.event.header().subagent.as_ref())
            .and_then(|t| firsts.get(&(row.chat, t.as_str().to_string())))
            .filter(|task| task.order < row.order);
        let Some(root) = root else {
            if parent.is_some() {
                return Some(fail("S14", format!("{name} names a parent and no root")));
            }
            if task.is_some() {
                return Some(fail(
                    "S14",
                    format!("{name} is in the main chat, though its thread was there"),
                ));
            }
            continue;
        };
        let Some(task) = task else {
            return Some(fail(
                "S14",
                format!("{name} is in a thread of no subagent's"),
            ));
        };
        let wanted_root = one(task, "E").unwrap_or_else(|| task.event.as_str().to_string());
        if parent.as_deref() != Some(task.event.as_str()) || root != wanted_root {
            return Some(fail(
                "S14",
                format!("{name} is in another subagent's thread"),
            ));
        }
    }
    None
}

/// S12: a prompt names a session only to resume it: one the chat had, that has not ended, on the
/// first prompt of a driver's life.
fn s12(world: &World) -> Option<Failure> {
    // Each session item's versions, as the chat saw them, in storage order.
    let sessions: Vec<(String, i64, State)> = (versions(world).into_iter())
        .filter(|v| v.id.starts_with("sess-"))
        .map(|v| (v.id["sess-".len()..].to_string(), v.row.order, v.state))
        .collect();
    let h = &world.harness;
    for d in (h.deliveries.iter()).filter(|d| d.kind == "prompt") {
        let Some(session) = &d.session else { continue };
        let what = format!("m{}", d.msg.map_or("?".into(), |m| m.to_string()));
        if d.chat_taken {
            return Some(fail(
                "S12",
                format!("{what} names a session though the driver already took the chat"),
            ));
        }
        let had = (h.sessions_started.iter())
            .any(|(chat, s)| Some(chat) == d.chat.as_ref() && s == session);
        if !had {
            return Some(fail(
                "S12",
                format!("{what} names a session the chat never had"),
            ));
        }
        // As the chat last saw the session before the prompt: by storage order, which tells
        // apart what happened in the same millisecond.
        let last = (sessions.iter()).rfind(|(s, order, _)| s == session && *order <= d.stored);
        if last.is_some_and(|(_, _, state)| *state == State::Done) {
            return Some(fail("S12", format!("{what} names a session that ended")));
        }
    }
    None
}

/// The liveness properties, at the end of the fair part: L1, L2, L3, and S2 for what an earlier
/// life of the driver left. Empty when all hold.
pub fn liveness(world: &World) -> Vec<Failure> {
    let mut failures = Vec::new();
    let mut settled: BTreeMap<EventKey, usize> = BTreeMap::new();
    for (_, settles) in settling(world) {
        for request in settles {
            *settled.entry(request).or_default() += 1;
        }
    }
    // A send MDK refuses for good is answered by a `failed` notice that settles what it
    // settled, and a notice it refuses by one in its place, which is the last: when MDK refuses
    // that too, the core takes the group as one MDK refuses everything for, as a gone one, and
    // stops there, so nothing loops (`Conversation::refused`). Only then is a request let off:
    // when two notices of its own chat that settled it were refused, the notice and the one in
    // its place, each known by its id: one notice refused twice, as when the core tried it again
    // after a crash took back the first refusal, is still one, with nothing yet in its place.
    let mut refused_notices: BTreeMap<(usize, &EventKey), BTreeSet<&str>> = BTreeMap::new();
    for r in (world.refused.iter()).filter(|r| r.notice.is_some()) {
        let Some((id, _)) = &r.item else { continue };
        for request in &r.settles {
            (refused_notices.entry((r.chat, request)).or_default()).insert(id.as_str());
        }
    }
    let versions = versions(world);
    let about: BTreeSet<&EventKey> = versions
        .iter()
        .filter_map(|v| v.started_by.as_ref())
        .collect();
    // The fair part invites the agent to every chat and stores everything posted: a chat never
    // joined leaves its messages waiting for good.
    for (chat, c) in world.chats.iter().enumerate() {
        if !c.accepted {
            failures.push(fail("L3", format!("c{chat} is never joined")));
        }
    }
    for (msg, m) in world.msgs.iter().enumerate() {
        let Some(m) = m else { continue };
        if m.stored.is_none() {
            let property = if m.posted.is_request() { "L1" } else { "L3" };
            failures.push(fail(
                property,
                format!("m{msg} is never stored: c{} is not joined", m.posted.chat),
            ));
            continue;
        }
        if m.posted.is_request() {
            let chat = usize::from(m.posted.chat);
            let chain_ended =
                (refused_notices.get(&(chat, &m.event))).is_some_and(|n| n.len() >= 2);
            if settled.get(&m.event).copied().unwrap_or(0) == 1 || chain_ended {
                continue;
            }
            // A request that names nothing and was older than five minutes when the core
            // read it is dropped, by design (D-028): also one read in time, whose reading a crash
            // undid, and read again late.
            let names = m.tags.iter().any(|t| t.len() >= 2 && t[0] == "e");
            let late = m.read_ms.is_some_and(|read| {
                (1_000_000 + read / 1000).saturating_sub(m.recorded_at)
                    > marmot_agent::conversation::LATE_SECS
            });
            if !names && late {
                continue;
            }
            let what = match m.posted.what {
                What::Answer => "answer",
                What::Interrupt => "interrupt",
                What::NewSession => "new session",
                What::Set => "setting",
                What::Prompt { .. } => "prompt",
            };
            let read = m.read_ms.map_or("never read".to_string(), |ms| {
                format!("last read at {ms} ms")
            });
            failures.push(fail(
                "L1",
                format!("m{msg} ({what}) is never settled, {read}"),
            ));
        } else {
            let delivered = (world.harness.deliveries.iter()).any(|d| d.msg == Some(msg));
            let lost = world.harness.lost.contains(&msg);
            if !delivered && !lost && !about.contains(&m.event) {
                failures.push(fail("L3", format!("m{msg} is never handled")));
            }
        }
    }
    let last = last_versions(world, &versions);
    for ((chat, id), v) in &last {
        if v.turn && !final_state(v.state) {
            failures.push(fail("L2", format!("c{chat} {id} never ends")));
        }
    }
    // The latest thing the running driver emitted of each item reaches the chat (L2): a turn
    // ends where people see it, and nothing waits in the outbox for good; unless MDK refused the
    // very version that carried it while the item was open. A closing version MDK refused is
    // followed by the core's own close of the item, which keeps its `seq`, unless the chat
    // already showed the item closed, by the version stored before it: the core leaves that close
    // as it is (`close_undelivered`). Each emission is known by its `seq`, which the core keeps.
    let closed_before = |r: &crate::world::RefusedSend| {
        let Some((id, version)) = &r.item else {
            return false;
        };
        (versions.iter())
            .filter(|v| v.row.chat == r.chat && v.id == *id && v.version < *version)
            .max_by_key(|v| v.version)
            .is_some_and(|v| final_state(v.state))
    };
    let refused: BTreeSet<(&str, u64)> = (world.refused.iter())
        .filter(|r| !r.state.is_some_and(final_state) || closed_by_core(r) || closed_before(r))
        .filter_map(|r| Some((r.item.as_ref()?.0.as_str(), r.seq?)))
        .collect();
    if world.harness.out.is_some() {
        let mut emitted: BTreeMap<&str, &crate::world::Emission> = BTreeMap::new();
        for e in (world.harness.emissions.iter()).filter(|e| e.life == world.harness.life) {
            emitted.insert(e.item.as_str(), e);
        }
        for (item, e) in emitted {
            if refused.contains(&(item, e.seq)) {
                continue;
            }
            let reached = match &e.words {
                Some(text) if world.refused.iter().any(|r| r.words.as_ref() == Some(text)) => true,
                Some(text) => (world.rows.iter()).any(|r| {
                    r.from_agent && r.content == *text && [CHAT, COMMENT, EDIT].contains(&r.kind)
                }),
                None => (versions.iter()).any(|v| v.id == item && v.seq == e.seq),
            };
            if !reached {
                failures.push(fail(
                    "L2",
                    format!(
                        "{item} never reaches the chat as {}",
                        crate::world::state_word(e.state)
                    ),
                ));
            }
        }
    }
    // What an earlier life of the driver left open is closed (S2).
    let h = &world.harness;
    let mut last_life: BTreeMap<&str, u32> = BTreeMap::new();
    for e in &h.emissions {
        last_life.insert(e.item.as_str(), e.life);
    }
    for ((chat, id), v) in &last {
        let earlier = last_life
            .get(id.as_str())
            .is_some_and(|life| *life < h.life || h.out.is_none());
        if earlier && !final_state(v.state) {
            failures.push(fail(
                "S2",
                format!("c{chat} {id}, left by an earlier driver, is still open"),
            ));
        }
    }
    failures
}

/// Whether a refused send was the core's own close of an item whose closing version was
/// dropped: the last thing sent for it.
fn closed_by_core(refused: &crate::world::RefusedSend) -> bool {
    refused.item.is_some() && refused.notice.as_deref() == Some("not_delivered")
}

/// The last version of each item, as the chat last saw it. A closing version MDK refused is
/// followed by the core's own close, so the item does not stay open (S2, L2); only when MDK
/// refused that close too is the item left out: the close is the last, so nothing loops, and the
/// core takes the group as one MDK refuses everything for, as a gone one. That holds only while
/// the refused close is the item's newest: a version stored after it, as when the item opens
/// again, counts as any.
fn last_versions<'a>(
    world: &World,
    versions: &'a [Version<'a>],
) -> BTreeMap<(usize, String), &'a Version<'a>> {
    let mut refused: BTreeMap<(usize, &str), u64> = BTreeMap::new();
    for r in (world.refused.iter()).filter(|r| closed_by_core(r)) {
        if let Some((id, version)) = &r.item {
            let newest = refused.entry((r.chat, id.as_str())).or_default();
            *newest = (*newest).max(*version);
        }
    }
    let mut last: BTreeMap<(usize, String), &'a Version<'a>> = BTreeMap::new();
    for v in versions {
        last.insert((v.row.chat, v.id.clone()), v);
    }
    last.retain(|(chat, id), v| {
        (refused.get(&(*chat, id.as_str()))).is_none_or(|close| v.version > *close)
    });
    last
}

/// S2, after the last clean stop: nothing is left open.
pub fn after_stop(world: &World) -> Option<Failure> {
    let versions = versions(world);
    let last = last_versions(world, &versions);
    (last.into_iter())
        .find(|(_, v)| !final_state(v.state))
        .map(|((chat, id), _)| fail("S2", format!("c{chat} {id} is open after the core stopped")))
}
