//! The properties, checked from outside the core: from what the simulated MDK stored and was
//! refused, and from what the scripted harness was handed and emitted. Nothing here reads the
//! core's state. The ids are those of `docs/design/verification.md`.

use crate::schedule::What;
use crate::world::{CHAT, EDIT, Row, World};
use marmot_agent::EventKey;
use marmot_agent::carrier::{self, ITEM_KIND};
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
    turn: bool,
    started_by: Option<EventKey>,
    notice: Option<String>,
}

fn versions(world: &World) -> Vec<Version<'_>> {
    (world.rows.iter())
        .filter(|r| r.from_agent && r.kind == ITEM_KIND)
        .filter_map(|row| {
            let read = carrier::read_item(&row.content, &row.tags)?;
            let header = read.item.event.header();
            Some(Version {
                row,
                id: header.item.as_str().to_string(),
                version: read.item.version,
                first: read.item.first.clone(),
                state: header.state,
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
            let settles: Vec<EventKey> = if row.kind == ITEM_KIND {
                carrier::read_item(&row.content, &row.tags)?.item.settles
            } else if row.kind == CHAT {
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
}

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
    // Settled by the net although the harness named it, soon after taking it.
    let net: BTreeSet<EventKey> = (versions(world).into_iter())
        .filter(|v| v.notice.as_deref() == Some("no_effect"))
        .flat_map(|v| {
            carrier::read_item(&v.row.content, &v.row.tags)
                .map(|r| r.item.settles)
                .unwrap_or_default()
        })
        .collect();
    let h = &world.harness;
    for (request, (msg, taken)) in &h.requests {
        let named =
            (h.emissions.iter()).any(|e| e.settles.contains(request) && e.ms <= taken + 30_000);
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
    for row in (world.rows.iter()).filter(|r| r.from_agent && r.kind == CHAT) {
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

/// S5: a request and an item reach only their own chat.
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

/// S12: a prompt names a session only to resume it: one the chat had, that has not ended, on the
/// first prompt of a driver's life.
fn s12(world: &World) -> Option<Failure> {
    // Each session item's versions, as the chat saw them, oldest first.
    let sessions: Vec<(String, u64, State)> = (versions(world).into_iter())
        .filter(|v| v.id.starts_with("sess-"))
        .map(|v| (v.id["sess-".len()..].to_string(), v.row.ms, v.state))
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
        // As the chat last saw the session before the prompt.
        let last = (sessions.iter()).rfind(|(s, ms, _)| s == session && *ms <= d.ms);
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
    // settled; a notice it refuses is dropped, so nothing loops, and what that notice settled
    // stays unsettled, by design (`Conversation::refused`).
    let refused: BTreeSet<&EventKey> = world.refused.iter().flat_map(|r| &r.settles).collect();
    let versions = versions(world);
    let about: BTreeSet<&EventKey> = versions
        .iter()
        .filter_map(|v| v.started_by.as_ref())
        .collect();
    for (msg, m) in world.msgs.iter().enumerate() {
        let Some(m) = m else { continue };
        if m.stored.is_none() {
            continue;
        }
        if m.posted.is_request() {
            if settled.get(&m.event).copied().unwrap_or(0) == 1 || refused.contains(&m.event) {
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
    // Everything the running driver emitted reaches the chat in the state it last gave (L2): a
    // turn ends where people see it, and nothing waits in the outbox for good.
    let refused: BTreeSet<&str> = (world.refused.iter())
        .filter_map(|r| r.item.as_ref().map(|(id, _)| id.as_str()))
        .collect();
    if world.harness.out.is_some() {
        let mut emitted: BTreeMap<&str, &crate::world::Emission> = BTreeMap::new();
        for e in (world.harness.emissions.iter()).filter(|e| e.life == world.harness.life) {
            emitted.insert(e.item.as_str(), e);
        }
        for (item, e) in emitted {
            if refused.contains(item) {
                continue;
            }
            let reached = match &e.words {
                Some(text) if world.refused.iter().any(|r| r.words.as_ref() == Some(text)) => true,
                Some(text) => (world.rows.iter()).any(|r| {
                    r.from_agent && r.content == *text && (r.kind == CHAT || r.kind == EDIT)
                }),
                None => (versions.iter())
                    .rfind(|v| v.id == item)
                    .is_some_and(|v| v.state == e.state),
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

/// The last version of each item, leaving out an item MDK refused a version of: the core gives
/// up on that version with a notice, by design, and sends nothing after it, so the chat may show
/// the item as it was (S2 and L2 assume MDK takes what it does not refuse for good).
fn last_versions<'a>(
    world: &World,
    versions: &'a [Version<'a>],
) -> BTreeMap<(usize, String), &'a Version<'a>> {
    let refused: BTreeSet<(usize, &str)> = (world.refused.iter())
        .filter_map(|r| r.item.as_ref().map(|(id, _)| (r.chat, id.as_str())))
        .collect();
    let mut last = BTreeMap::new();
    for v in versions {
        if !refused.contains(&(v.row.chat, v.id.as_str())) {
            last.insert((v.row.chat, v.id.clone()), v);
        }
    }
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
