//! The core end to end through MDK: a person and the agent, two accounts on MDK's test relay, and
//! the core's loop between the agent's account and a scripted stand-in for a harness. What the
//! person sees is read back from the person's own storage.

mod common;

use common::{Echo, World, eventually};
use marmot_agent::GroupKey;
use marmot_agent::run::Store;
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use marmot_app::MarmotAppEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// A person who owns the agent invites it to a direct chat and writes to it. The core accepts the
// invitation, hands the message to the harness as a prompt, and the person receives the agent's
// words as a chat message and its turn as an item that names the message it answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_writes_to_the_agent_and_reads_its_answer() {
    let world = World::new("name = \"Echo\"\n[codex]\ncwd = \"/unused\"\n").await;
    let prompts = Arc::new(AtomicUsize::new(0));
    world.runtime.start().await.unwrap();
    let before = marmot_app::UserProfileMetadata {
        name: Some("Old name".into()),
        about: Some("kept".into()),
        ..marmot_app::UserProfileMetadata::default()
    };
    world
        .runtime
        .publish_user_profile_using_account_relays(&world.marmot.label, before)
        .await
        .unwrap();
    let core = world.start(Echo::new(prompts.clone()));
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let group_hex = world.joined(&group).await;
    world
        .runtime
        .send_message(&world.person, &group, b"hello".to_vec())
        .await
        .unwrap();

    let words = world.words(&group_hex, "echo: hello").await;
    let relays = vec![cgka_traits::TransportEndpoint(world.relay.clone())];
    let profile = world
        .runtime
        .fetch_current_user_profile_for_account_id(&world.agent, relays)
        .await
        .unwrap()
        .expect("a profile on the relay");
    assert_eq!(
        profile.name.as_deref(),
        Some("Echo"),
        "the name from the settings"
    );
    assert_eq!(
        profile.about.as_deref(),
        Some("kept"),
        "the rest of the profile kept"
    );
    assert!(words.tags.iter().any(|t| t[0] == "turn"));
    let turn = eventually(|| {
        let items = world.items(&group_hex).into_iter().map(|(_, read)| read);
        items
            .filter(|read| matches!(read.item.event.body(), Body::Turn(_)))
            .find(|read| read.item.event.header().state == State::Done)
    })
    .await;
    assert_eq!(turn.item.version, 2);
    assert!(
        turn.item.first.is_some(),
        "the later version names the first"
    );
    assert!(turn.item.started_by.is_some(), "the turn names its message");

    core.stop().await;
    world.runtime.shutdown().await;
}

// In a named group of two the agent answers only when addressed, as in any group (D-075); and a
// restart reads what MDK stored while the core was stopped, once (D-037, MDK #3551).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_named_group_of_two_is_a_group_and_a_restart_repeats_nothing() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let prompts = Arc::new(AtomicUsize::new(0));
    world.runtime.start().await.unwrap();
    let core = world.start(Echo::new(prompts.clone()));
    let named = world
        .runtime
        .create_group(
            &world.person,
            "Team",
            std::slice::from_ref(&world.agent),
            None,
        )
        .await
        .unwrap()
        .operation
        .expect("the group is made");
    world.joined(&named).await;
    let direct = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let direct_hex = world.joined(&direct).await;

    world
        .runtime
        .send_message(&world.person, &named, b"lunch, anyone?".to_vec())
        .await
        .unwrap();
    world
        .runtime
        .send_message(&world.person, &direct, b"first".to_vec())
        .await
        .unwrap();
    world.words(&direct_hex, "echo: first").await;
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "the group message was not a prompt"
    );

    core.stop().await;
    let group = GroupKey::from(direct_hex.as_str());
    let mark = Store::new(world.state.path()).load().unwrap().read[&group];
    let mut arrivals = world.runtime.subscribe();
    world
        .runtime
        .send_message(&world.person, &direct, b"while away".to_vec())
        .await
        .unwrap();
    // MDK can store the row before it broadcasts the notification. Observe both while the
    // core is stopped so its next subscription can only recover the message from storage.
    let gap_id = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let MarmotAppEvent::MessageReceived(received) = arrivals.recv().await.unwrap()
                && received.account_id_hex == world.agent
                && received.message.plaintext == "while away"
            {
                break received.message.message_id_hex;
            }
        }
    })
    .await
    .expect("MDK broadcast the gap message to the recipient");
    drop(arrivals);
    assert!(
        world
            .marmot
            .stored(&group, mark, 1000)
            .unwrap()
            .messages
            .iter()
            .any(|m| m.event.as_str() == gap_id),
        "the recipient stored the gap message after the saved mark"
    );

    let core = world.start(Echo::new(prompts.clone()));
    world.words(&direct_hex, "echo: while away").await;
    core.stop().await;

    let core = world.start(Echo::new(prompts.clone()));
    world
        .runtime
        .send_message(&world.person, &direct, b"second".to_vec())
        .await
        .unwrap();
    world.words(&direct_hex, "echo: second").await;
    core.stop().await;
    assert_eq!(prompts.load(Ordering::SeqCst), 3, "each prompt heard once");
    world.runtime.shutdown().await;
}

// A long chat is not throttled by the test relay: MDK's mock relay takes 60 events a minute from
// each connection by default, and a scenario that sends more must not stall on it, as MDK's own
// harness relay does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_chat_is_not_throttled_by_the_test_relay() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let prompts = Arc::new(AtomicUsize::new(0));
    world.runtime.start().await.unwrap();
    let core = world.start(Echo::new(prompts.clone()));
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let group_hex = world.joined(&group).await;
    // Each prompt makes three events: the turn, the words, and the turn again, 150 in all. On the
    // throttled relay this took about three minutes; without the limit, a few seconds.
    let chat = async {
        for n in 0..50 {
            let text = format!("n{n}");
            world
                .runtime
                .send_message(&world.person, &group, text.into_bytes())
                .await
                .unwrap();
            world.words(&group_hex, &format!("echo: n{n}")).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(60), chat)
        .await
        .expect("throttled: the chat took over a minute");
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send tried again under the token MDK keeps it under gives back the event it made and sends
// nothing, however long after: the agent's words go out once, with every tag the core gives them.
// A token MDK holds for one send is refused for another, never taken for it (S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_tried_again_under_its_token_goes_out_once() {
    use marmot_agent::marmot::SendFailed;
    use marmot_agent::{EventKey, GroupKey, Sending, Token};
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let core = world.start(Echo::new(Arc::new(AtomicUsize::new(0))));
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let group = GroupKey(world.joined(&group).await);
    core.stop().await;
    let words = |text: &str| Sending::Words {
        key: "w".into(),
        text: text.into(),
        turn: Some("t1".into()),
        remark: true,
        edit: None,
        settles: vec![EventKey::from("e-r1")],
    };
    let token = Token {
        send: "w@1".into(),
        mdk: "a token".into(),
        edits: None,
        tried_at: None,
    };
    let went = world.send(&group, &words("the words"), &token).await;
    let went = went.unwrap();
    // A second send of the same event in the same second would be the same event, so a later one.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let again = world.send(&group, &words("the words"), &token).await;
    assert_eq!(again, Ok(went.clone()), "the same event");
    let other = world.send(&group, &words("other words"), &token).await;
    assert_eq!(other, Err(SendFailed::Refused { gone: false }));
    let query = marmot_app::AppMessageQuery {
        group_id_hex: Some(group.as_str().to_string()),
        kinds: Some(vec![9]),
        limit: None,
    };
    let sent = (world.marmot.runtime)
        .messages_with_query(&world.marmot.label, query)
        .unwrap();
    let sent: Vec<_> = sent.into_iter().filter(|m| m.direction == "sent").collect();
    assert_eq!(sent.len(), 1, "sent once");
    assert_eq!(sent[0].message_id_hex, went.as_str());
    let tag = |name: &str| {
        (sent[0].tags.iter())
            .find(|t| t[0] == name)
            .map(|t| t[1].clone())
    };
    assert_eq!(tag("turn").as_deref(), Some("t1"));
    assert_eq!(tag("phase").as_deref(), Some("commentary"));
    assert_eq!(tag("settles").as_deref(), Some("e-r1"));
    world.runtime.shutdown().await;
}

// An item's later version reaches the person as MDK's edit of its first event (D-025): MDK's row
// for the item holds its latest version, and the version tried again under its token goes out
// once. A waiting approval alerts the person; its closing version, and the agent's remark during
// a turn, do not (D-024, D-078).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_items_later_version_is_mdks_edit_of_its_first_event() {
    use marmot_agent::carrier::{self, EDIT_KIND, ITEM_KIND, Item};
    use marmot_agent::{Sending, Token};
    use marmot_agent_events::body::{Approval, Choice, Effect};
    use marmot_agent_events::{AgentEvent, Header};
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    (world.marmot.app)
        .set_local_notifications_enabled(&world.person, true)
        .unwrap();
    let mut alerts = world.runtime.subscribe_notifications().unwrap();
    let approval = |state: State, version: u64, first: Option<marmot_agent::EventKey>| {
        let header = Header {
            harness: "codex".into(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            item: "call1".into(),
            seq: 3,
            state,
            title: "Allow this command?".into(),
            text: "Allow `ls`?".into(),
        };
        let body = Body::Approval(Approval {
            subject: None,
            choices: vec![Choice {
                id: "allow".into(),
                label: "Allow once".into(),
                effect: Effect::Allow,
            }],
            answer: None,
        });
        Sending::Item {
            key: "call1".into(),
            item: Box::new(Item {
                event: AgentEvent::new(header, body, None).unwrap(),
                version,
                first,
                started_by: None,
                settles: Vec::new(),
            }),
        }
    };
    let token = |mdk: &str, edits: Option<&str>| Token {
        send: mdk.into(),
        mdk: mdk.into(),
        edits: edits.map(Into::into),
        tried_at: None,
    };
    let first = (world.send(
        &group,
        &approval(State::Waiting, 1, None),
        &token("v1", None),
    ))
    .await
    .unwrap();
    let alert = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let update = alerts.recv().await.expect("notifications");
            if update.account_id_hex == world.person {
                return update;
            }
        }
    })
    .await
    .expect("the waiting approval alerts the person");
    assert_eq!(alert.message_id_hex.as_deref(), Some(first.as_str()));

    let later = approval(State::Done, 2, Some(first.clone()));
    let edit = world.send(&group, &later, &token("v2", Some("v1"))).await;
    let edit = edit.unwrap();
    assert_ne!(edit, first, "an event of its own");
    let remark = Sending::Words {
        key: "w".into(),
        text: "Looking around".into(),
        turn: Some("t1".into()),
        remark: true,
        edit: None,
        settles: Vec::new(),
    };
    world
        .send(&group, &remark, &token("w", None))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let again = world.send(&group, &later, &token("v2", Some("v1"))).await;
    assert_eq!(again, Ok(edit.clone()), "the same edit");

    let kinds = |kind: u64| sent_by_core(&world, &group, kind);
    assert_eq!(kinds(ITEM_KIND).len(), 1);
    let edits = kinds(EDIT_KIND);
    assert_eq!(edits.len(), 1, "the later version went out once");
    let named: Vec<_> = (edits[0].tags.iter()).filter(|t| t[0] == "e").collect();
    assert_eq!(named, vec![&vec!["e".to_string(), first.0.clone()]]);
    let row = eventually(|| {
        let row = (world.runtime)
            .timeline_message(&world.person, group.as_str(), first.as_str())
            .unwrap()?;
        (row.agent_item.as_ref()?.version == 2).then_some(row)
    })
    .await;
    assert_eq!(row.kind, ITEM_KIND, "one row, at the first event");
    let read = carrier::read_item(row.kind, &row.plaintext, &row.tags).unwrap();
    assert_eq!(read.item.event.header().state, State::Done);
    let quiet = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let update = alerts.recv().await.expect("notifications");
            if update.account_id_hex == world.person {
                return update;
            }
        }
    });
    assert!(quiet.await.is_err(), "nothing more alerts");
    world.runtime.shutdown().await;
}

/// Waits until just after a whole second starts, so what follows at once falls in that second.
async fn at_a_new_second() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let into = now.unwrap().subsec_millis() as u64;
    tokio::time::sleep(std::time::Duration::from_millis(1000 - into + 20)).await;
}

/// What the agent sent to `group`, of kind `kind`, as MDK keeps it for the agent.
fn sent_by_core(world: &World, group: &GroupKey, kind: u64) -> Vec<marmot_app::AppMessageRecord> {
    let query = marmot_app::AppMessageQuery {
        group_id_hex: Some(group.as_str().to_string()),
        kinds: Some(vec![kind]),
        limit: None,
    };
    let sent = (world.marmot.runtime)
        .messages_with_query(&world.marmot.label, query)
        .unwrap();
    sent.into_iter().filter(|m| m.direction == "sent").collect()
}

/// A direct chat between the person and the agent, with the core stopped once the agent is in it.
async fn a_quiet_chat(world: &World) -> GroupKey {
    world.runtime.start().await.unwrap();
    let core = world.start(Echo::new(Arc::new(AtomicUsize::new(0))));
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let group = GroupKey(world.joined(&group).await);
    core.stop().await;
    group
}

/// The group's id, as MDK takes it.
fn group_id(group: &GroupKey) -> cgka_traits::GroupId {
    cgka_traits::GroupId::new(hex::decode(group.as_str()).unwrap())
}

/// The agent's own words in `group`, as the core sends them, with a key and a token named `name`.
fn words_named(name: &str) -> (marmot_agent::Sending, marmot_agent::Token) {
    let words = marmot_agent::Sending::Words {
        key: name.into(),
        text: "the same words".into(),
        turn: Some("t1".into()),
        remark: true,
        edit: None,
        settles: Vec::new(),
    };
    let token = marmot_agent::Token {
        send: name.into(),
        mdk: format!("token {name}"),
        edits: None,
        tried_at: None,
    };
    (words, token)
}

// MDK makes an event's id from its kind, tags, content and second, and keeps the id of a send it
// took and dropped. The same words tried again in that second under a new token meet that id:
// MDK is busy for the rest of the second, which is no refusal (S1, S3). While the agent's own
// disband of a group is under way, MDK takes each send to it and then drops it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_send_tried_again_in_the_same_second_is_not_refused() {
    use marmot_agent::marmot::SendFailed;
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    let id = group_id(&group);
    let agent = &world.marmot.label;
    world
        .runtime
        .enable_group_disbanding(agent, &id)
        .await
        .unwrap();
    at_a_new_second().await;
    world.runtime.disband_group(agent, &id).await.unwrap();
    let (words, first) = words_named("a");
    let tried = world.send(&group, &words, &first).await;
    assert_eq!(tried, Err(SendFailed::Rejected), "taken, and dropped");
    let again = marmot_agent::Token {
        mdk: "token a, again".into(),
        ..first
    };
    let tried = world.send(&group, &words, &again).await;
    assert_eq!(tried, Err(SendFailed::Busy), "busy until the next second");
    world.runtime.shutdown().await;
}

// Two sends of the same words, with the same tags, in one second would be one event to MDK: the
// second waits for the next second, and both go out (S1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_words_twice_in_one_second_both_go_out() {
    use marmot_agent::marmot::SendFailed;
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    let ((a, first), (b, second)) = (words_named("a"), words_named("b"));
    at_a_new_second().await;
    let went = world.send(&group, &a, &first).await.unwrap();
    let tried = world.send(&group, &b, &second).await;
    assert_eq!(tried, Err(SendFailed::Busy), "busy until the next second");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let also = world.send(&group, &b, &second).await.unwrap();
    assert_ne!(went, also, "two events");
    assert_eq!(sent_by_core(&world, &group, 9).len(), 2, "both went out");
    world.runtime.shutdown().await;
}

// A send to a group that is disbanded, or that MDK does not have, is refused for good, so the
// core drops it with its notice and the chat's later sends are not held up behind it for ever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_to_a_group_disbanded_or_unknown_is_refused() {
    use marmot_agent::marmot::SendFailed;
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    let (words, _) = words_named("a");
    let unknown = GroupKey("ab".repeat(16));
    let tried = world.send(&unknown, &words, &words_named("a").1).await;
    assert_eq!(tried, Err(SendFailed::Refused { gone: false }), "unknown");

    let id = group_id(&group);
    world
        .runtime
        .enable_group_disbanding(&world.person, &id)
        .await
        .unwrap();
    world
        .runtime
        .disband_group(&world.person, &id)
        .await
        .unwrap();
    // MDK takes sends a while longer, until the disband has gone through; a send it took and
    // dropped is refused under its token after, so each try has one of its own.
    for n in 0.. {
        assert!(n < 300, "refused within 30 seconds");
        let token = marmot_agent::Token {
            send: "a".into(),
            mdk: format!("token {n}"),
            edits: None,
            tried_at: None,
        };
        let tried = world.send(&group, &words, &token).await;
        if tried == Err(SendFailed::Refused { gone: false }) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    world.runtime.shutdown().await;
}

// MDK lets go of a token with the message it made, once a group's messages disappear. A send
// first tried longer ago than they last, whose token MDK no longer knows, may have gone out: it
// is not sent again, and is given up as not known. One whose token MDK still keeps is answered
// from it; and in a group whose messages stay, a send goes however long ago it was first tried.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_older_than_its_groups_messages_is_not_sent_again() {
    use marmot_agent::marmot::SendFailed;
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let now = now.unwrap().as_secs();
    let tried = |name: &str, ago: Option<u64>| {
        let (mut words, mut token) = words_named(name);
        if let marmot_agent::Sending::Words { text, .. } = &mut words {
            *text = format!("words {name}");
        }
        token.tried_at = ago.map(|ago| now - ago);
        (words, token)
    };
    let (words, token) = tried("long ago", Some(1_000_000));
    assert!(
        world.send(&group, &words, &token).await.is_ok(),
        "messages stay"
    );

    let id = group_id(&group);
    (world.runtime)
        .update_message_retention(&world.person, &id, 60)
        .await
        .unwrap();
    let keep = || {
        let record = world.marmot.app.group(&world.marmot.label, group.as_str());
        record
            .ok()
            .flatten()
            .map(|r| r.message_retention.disappearing_message_secs)
    };
    eventually(|| (keep() == Some(60)).then_some(())).await;
    let (words, token) = tried("forgotten", Some(120));
    let sent = world.send(&group, &words, &token).await;
    assert_eq!(sent, Err(SendFailed::Forgotten), "not sent again");
    let (words, token) = tried("kept", None);
    let went = world.send(&group, &words, &token).await.unwrap();
    let token = marmot_agent::Token {
        tried_at: Some(now - 120),
        ..token
    };
    assert_eq!(
        world.send(&group, &words, &token).await,
        Ok(went),
        "answered from its token"
    );
    let texts: Vec<_> = (sent_by_core(&world, &group, 9).into_iter())
        .map(|m| m.plaintext)
        .collect();
    assert!(!texts.contains(&"words forgotten".to_string()));
    world.runtime.shutdown().await;
}
