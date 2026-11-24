//! S3 and the outbox (D-025, D-109, D-112), on the conversation alone: over any order of the
//! driver's events, sends that go, that MDK refuses or drops, and restarts from the state file,
//! an item's versions rise by one, skipping only versions that were refused, and every later
//! version names the item's first event; a send keeps one name across its tries and restarts,
//! no two sends share one, and the token MDK keeps it under changes only when MDK dropped a try.
//!
//! The simulator checks S3 for the whole core, against a simulated MDK; this checks the
//! conversation's own part of it after every step, with no MDK at all, so a failure points at
//! the conversation.

use marmot_agent::{
    Act, Conversation, EventKey, GroupKey, Heard, PubKey, Rights, Saved, Sending, Where, Who,
};
use marmot_agent_driver::{Capabilities, ChatId, Emitted, FilesIn, MidTurn, Sessions};
use marmot_agent_events::body::{Body, Message, Plan, StartedBy, Turn};
use marmot_agent_events::{AgentEvent, Header, State};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

const AGENT: &str = "agent-key";
const OWNER: &str = "owner-key";

#[derive(Clone, Debug)]
enum Step {
    /// The driver's next event for item `item` (a plan, a turn, or the agent's words), in `state`.
    Emit { item: u8, state: State, words: bool },
    /// The next send of the group goes out.
    Send,
    /// MDK refuses the next send for good.
    Refuse,
    /// MDK took the next send and dropped it: it is tried again under a new token.
    Drop,
    /// The core stops and starts again from its state file.
    Restart,
}

fn state() -> impl Strategy<Value = State> {
    prop_oneof![
        3 => Just(State::Running),
        2 => Just(State::Done),
        1 => Just(State::Failed),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => (0..4u8, state(), any::<bool>()).prop_map(|(item, state, words)| Step::Emit { item, state, words }),
        6 => Just(Step::Send),
        1 => Just(Step::Refuse),
        1 => Just(Step::Drop),
        1 => Just(Step::Restart),
    ]
}

fn conversation(saved: Saved) -> Conversation {
    let capabilities = Capabilities {
        interrupt: true,
        mid_turn: MidTurn::Fold,
        sessions: Sessions::Core,
        resume: true,
        approvals: true,
        files_in: FilesIn::Images,
        approval_commands: Vec::new(),
    };
    let rights = Rights {
        owner: PubKey::from(OWNER),
        prompt: Who::Owner,
        approve: Who::Owner,
    };
    Conversation::new(PubKey::from(AGENT), "codex", rights, capabilities, saved)
}

fn direct() -> Where {
    Where {
        group: GroupKey::from("g-direct"),
        name: None,
        direct: true,
        members: 2,
    }
}

fn event(item: u8, words: bool, state: State, seq: u64) -> AgentEvent {
    let (id, body) = match (item, words) {
        (0, _) => (
            "turn".to_string(),
            Body::Turn(Turn {
                started_by: StartedBy::User,
                duration_ms: None,
                usage: None,
            }),
        ),
        (n, true) => (
            format!("words{n}"),
            Body::Message(Message {
                phase: None,
                parent: None,
            }),
        ),
        (n, false) => (
            format!("plan{n}"),
            Body::Plan(Plan {
                text: Some(format!("step {seq}")),
                entries: Vec::new(),
            }),
        ),
    };
    let header = Header {
        harness: "codex".into(),
        session: Some("th1".into()),
        turn: Some("t1".into()),
        item: id.as_str().into(),
        seq,
        state,
        title: "A title".into(),
        text: format!("text {seq}"),
    };
    AgentEvent::new(header, body, None).expect("no secret")
}

/// What has been seen of the sends, to check each step against.
#[derive(Default)]
struct Seen {
    /// For each item, the versions sent or refused, and the first event it went out as.
    versions: BTreeMap<String, BTreeSet<u64>>,
    firsts: BTreeMap<String, EventKey>,
    /// Each send's name, by what it sends (an item's key and version, or the words' key and the
    /// order they were queued in), and the tokens MDK was handed for each name.
    names: BTreeMap<String, (String, u64)>,
    tokens: BTreeMap<String, String>,
    next_event: u64,
}

fn group() -> GroupKey {
    direct().group
}

proptest! {
    #[test]
    fn s3_versions_rise_and_a_send_keeps_its_name(steps in prop::collection::vec(step(), 1..60)) {
        let mut talk = conversation(Saved::default());
        let heard = Heard {
            chat: direct(),
            event: EventKey::from("e1"),
            author: PubKey::from(OWNER),
            author_name: "Owner".into(),
            text: "hello".into(),
            reply_to: None,
            mentions_agent: false,
            attachments: Vec::new(),
        };
        let chat = match talk.heard(heard).pop() {
            Some(Act::Prompt(prompt)) => prompt.chat.id,
            other => panic!("not a prompt: {other:?}"),
        };
        let mut seen = Seen::default();
        let mut seq = 0;
        let mut dropped: BTreeMap<String, u32> = BTreeMap::new();
        for step in steps {
            match step {
                Step::Emit { item, state, words } => {
                    seq += 1;
                    talk.emitted(Emitted {
                        event: event(item, words, state, seq),
                        chat: Some(ChatId(chat.0.clone())),
                        prompts: Vec::new(),
                        settles: Vec::new(),
                    });
                }
                Step::Send | Step::Refuse | Step::Drop => {
                    let Some((token, what)) = talk.next_send(&group()) else { continue };
                    let key = match &what { Sending::Item { key, .. } | Sending::Words { key, .. } => key.clone() };
                    match step {
                        Step::Send => {
                            if let Sending::Item { item, .. } = &what {
                                check_version(&mut seen, &key, item.version)?;
                                match seen.firsts.get(&key) {
                                    Some(first) => prop_assert_eq!(item.first.as_ref(), Some(first), "{} names its first event", key),
                                    None => prop_assert!(item.first.is_none(), "{} names a first event none was sent", key),
                                }
                            }
                            if let Sending::Words { edit, .. } = &what {
                                prop_assert_eq!(edit.as_ref(), seen.firsts.get(&key), "words edit their first message");
                            }
                            seen.next_event += 1;
                            let event = EventKey::from(format!("{:064x}", seen.next_event).as_str());
                            seen.firsts.entry(key.clone()).or_insert_with(|| event.clone());
                            talk.sent(&key, event);
                            seen.tokens.remove(&token.send);
                        }
                        Step::Refuse => {
                            if let Sending::Item { item, .. } = &what {
                                check_version(&mut seen, &key, item.version)?;
                            }
                            talk.refused(&token.send, false);
                            seen.tokens.remove(&token.send);
                        }
                        _ => {
                            talk.attempting(&token.send, 1);
                            let n = talk.rejected(&token.send);
                            let before = dropped.insert(token.send.clone(), n).unwrap_or(0);
                            prop_assert_eq!(n, before + 1, "each drop is counted once");
                            // The next try is under a new token.
                            let (again, _) = talk.next_send(&group()).expect("still queued");
                            prop_assert_eq!(&again.send, &token.send);
                            prop_assert_ne!(&again.mdk, &token.mdk, "a dropped try's token is not used again");
                            seen.tokens.insert(again.send.clone(), again.mdk.clone());
                        }
                    }
                }
                Step::Restart => {
                    let saved: Saved = serde_json::from_str(
                        &serde_json::to_string(talk.saved()).expect("saves"),
                    ).expect("reads");
                    let before = talk.queued(&group());
                    talk = conversation(saved);
                    let after = talk.queued(&group());
                    // What was queued is queued as it was, under the same names and tokens.
                    prop_assert_eq!(before.len(), after.len());
                    for ((a, x), (b, y)) in before.iter().zip(&after) {
                        prop_assert_eq!(&a.send, &b.send);
                        prop_assert_eq!(&a.mdk, &b.mdk);
                        prop_assert_eq!(&a.edits, &b.edits);
                        prop_assert!(x == y, "a queued send is the same after a restart");
                    }
                    talk.recover();
                }
            }
            // After every step: in the queue, each item's versions rise, each send has its own
            // name and token, and a name kept from before still sends what it sent.
            let queued = talk.queued(&group());
            let mut last: BTreeMap<String, u64> = BTreeMap::new();
            let mut names = BTreeSet::new();
            let mut tokens = BTreeSet::new();
            for (n, (token, what)) in queued.iter().enumerate() {
                prop_assert!(names.insert(token.send.clone()), "two queued sends share a name");
                prop_assert!(tokens.insert(token.mdk.clone()), "two queued sends share a token");
                let (key, version) = match what {
                    Sending::Item { key, item } => (key.clone(), item.version),
                    Sending::Words { key, .. } => (key.clone(), n as u64),
                };
                if let Sending::Item { .. } = what {
                    if let Some(before) = last.insert(key.clone(), version) {
                        prop_assert!(version > before, "{} v{} is queued after v{}", key, version, before);
                    }
                    if let Some(max) = seen.versions.get(&key).and_then(|v| v.last()) {
                        prop_assert!(version > *max, "{} v{} is queued after v{} went", key, version, max);
                    }
                    match seen.names.get(&token.send) {
                        Some((k, v)) => prop_assert!(k == &key && *v == version, "a name sends something else"),
                        None => { seen.names.insert(token.send.clone(), (key.clone(), version)); }
                    }
                }
                if let Some(mdk) = seen.tokens.get(&token.send) {
                    prop_assert_eq!(mdk, &token.mdk, "a send's token changed with no drop");
                }
                seen.tokens.insert(token.send.clone(), token.mdk.clone());
            }
        }
    }
}

/// An item's next version is one more than the last that was sent or refused.
fn check_version(seen: &mut Seen, key: &str, version: u64) -> Result<(), TestCaseError> {
    let versions = seen.versions.entry(key.to_string()).or_default();
    let expected = versions.last().map_or(1, |v| v + 1);
    prop_assert_eq!(version, expected, "{} goes out at v{}", key, version);
    versions.insert(version);
    Ok(())
}
