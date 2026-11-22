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
            .stored(&group, mark)
            .unwrap()
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

// The agent's words that went out are found in what MDK stored by all they carry, so a send whose
// outcome was not known is not sent twice; words that differ only in what they settle, or in
// their phase, are not taken for them (S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_agents_words_are_found_by_all_they_carry() {
    use marmot_agent::{EventKey, GroupKey, Sending};
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
    let words = |remark: bool, settles: &[&str]| Sending::Words {
        key: "w".into(),
        text: "the words".into(),
        turn: Some("t1".into()),
        remark,
        edit: None,
        settles: settles.iter().map(|s| EventKey::from(*s)).collect(),
    };
    let sent = words(true, &["e-r1", "e-r2"]);
    let went = world.marmot.send(&group, &sent, "w@1").await.unwrap();
    let found = |what: &Sending| world.marmot.find_sent(&group, what, |_| false).unwrap();
    assert_eq!(found(&sent), Some(went));
    assert_eq!(found(&words(true, &["e-r1"])), None);
    assert_eq!(found(&words(false, &["e-r1", "e-r2"])), None);
    world.runtime.shutdown().await;
}
