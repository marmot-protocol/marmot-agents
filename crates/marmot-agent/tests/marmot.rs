//! The core end to end through MDK: a person and the agent, two accounts on MDK's test relay, and
//! the core's loop between the agent's account and a scripted stand-in for a harness. What the
//! person sees is read back from the person's own storage.

mod common;

use common::{Echo, World, eventually};
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
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
// restart reads nothing again that the core had read before it stopped (D-037).
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
    let core = world.start(Echo::new(prompts.clone()));
    world
        .runtime
        .send_message(&world.person, &direct, b"second".to_vec())
        .await
        .unwrap();
    world.words(&direct_hex, "echo: second").await;
    assert_eq!(prompts.load(Ordering::SeqCst), 2, "nothing read twice");

    core.stop().await;
    world.runtime.shutdown().await;
}
