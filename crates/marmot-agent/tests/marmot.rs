//! The core end to end through MDK: a person and the agent, two accounts on MDK's test relay, and
//! the core's loop between the agent's account and a scripted stand-in for a harness. What the
//! person sees is read back from the person's own storage.

mod common;

use common::{Echo, World, eventually};
use marmot_agent::groups::Listed;
use marmot_agent::marmot::Fault;
use marmot_agent::run::Store;
use marmot_agent::{FullActivity, GroupKey, PubKey};
use marmot_agent_events::State;
use marmot_agent_events::body::Body;
use marmot_app::MarmotAppEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// #139. MDK's publisher replaces the about, picture, NIP-05 and lightning address of the agent's
// profile with those of the profile it is given, so the core publishes the agent's name only over
// a profile it has read: when the read fails, finds no profile, or the publish fails, the profile
// on the relay stays as it was, and the core tries again later, publishing the name with the rest
// kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_profile_that_cannot_be_read_is_not_published_over() {
    the_name_waits_out(Fault::ProfileFails).await;
}

// MDK's read says `None` when the relays hold no fresh profile, so that a caller does not publish
// a partial one over what it could not confirm: the core waits for a read that finds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_that_finds_no_profile_publishes_nothing_over_it() {
    the_name_waits_out(Fault::ProfileMissing).await;
}

// The read works and the publish fails: it is tried again later, from a new read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_whose_publish_failed_is_published_on_a_later_try() {
    the_name_waits_out(Fault::NameFails).await;
}

/// Publishes a full profile for the agent under another name, starts the core with `fault`
/// waiting for its first try, and checks that the relay shows that profile unchanged once the
/// fault is met, and the agent's name with the rest kept after a later try.
async fn the_name_waits_out(fault: Fault) {
    let world = World::new("name = \"Echo\"\n[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let before = marmot_app::UserProfileMetadata {
        name: Some("Old name".into()),
        about: Some("kept".into()),
        picture: Some("https://example.invalid/agent.png".into()),
        nip05: Some("agent@example.invalid".into()),
        lud16: Some("agent@example.invalid".into()),
        ..marmot_app::UserProfileMetadata::default()
    };
    world
        .runtime
        .publish_user_profile_using_account_relays(&world.marmot.label, before.clone())
        .await
        .unwrap();
    // A second on, so the core's profile is newer than this one.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    world.marmot.faults.push(fault);
    let core = world.start(Echo::new(Arc::default()));
    // The first try meets the fault, and the next is 10 s later.
    eventually(|| world.marmot.faults.is_empty().then_some(())).await;
    let kept = agent_profile(&world).await;
    let as_published = marmot_app::UserProfileMetadata {
        created_at: 0,
        source_relays: Vec::new(),
        ..kept.clone()
    };
    assert_eq!(as_published, before, "nothing published over it");
    let mut profile = kept;
    for _ in 0..60 {
        if profile.name.as_deref() == Some("Echo") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        profile = agent_profile(&world).await;
    }
    assert_eq!(
        profile.name.as_deref(),
        Some("Echo"),
        "named on a later try"
    );
    assert_eq!(profile.display_name.as_deref(), Some("Echo"));
    assert_eq!(
        (profile.about, profile.picture, profile.nip05, profile.lud16),
        (before.about, before.picture, before.nip05, before.lud16),
        "the rest of the profile kept"
    );
    core.stop().await;
    world.runtime.shutdown().await;
}

/// The agent's profile as the relay holds it.
async fn agent_profile(world: &World) -> marmot_app::UserProfileMetadata {
    let relays = vec![cgka_traits::TransportEndpoint(world.relay.clone())];
    (world.runtime)
        .fetch_current_user_profile_for_account_id(&world.agent, relays)
        .await
        .unwrap()
        .expect("a profile on the relay")
}

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

// The test relay accepts more than nostr-relay-builder's default 60 events a minute on one
// connection (`common::relay`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_test_relay_takes_more_than_60_events_a_minute_on_one_connection() {
    use futures_util::{SinkExt, StreamExt};
    use nostr_relay_builder::prelude::{ClientMessage, EventBuilder, JsonUtil, Keys, RelayMessage};
    use tokio_tungstenite::tungstenite::Message;

    let relay = common::relay().await;
    let url = relay.url().await.to_string();
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let keys = Keys::generate();
    for n in 0..150 {
        let event = EventBuilder::text_note(format!("n{n}"))
            .sign_with_keys(&keys)
            .unwrap();
        let frame = Message::Text(ClientMessage::event(event.clone()).as_json().into());
        ws.send(frame).await.unwrap();
        let reply = loop {
            match ws.next().await.expect("relay replied").unwrap() {
                Message::Text(text) => match RelayMessage::from_json(text.as_bytes()).unwrap() {
                    ok @ RelayMessage::Ok { .. } => break ok,
                    _ => continue,
                },
                Message::Ping(_) | Message::Pong(_) => continue,
                other => panic!("unexpected frame: {other:?}"),
            }
        };
        assert!(
            matches!(
                &reply,
                RelayMessage::Ok { event_id, status: true, .. } if *event_id == event.id
            ),
            "relay rejected event {n}: {reply:?}"
        );
    }
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
        thread: None,
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
            subagent: None,
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
                thread: None,
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
        thread: None,
    };
    world
        .send(&group, &remark, &token("w", None))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let again = world.send(&group, &later, &token("v2", Some("v1"))).await;
    assert_eq!(again, Ok(edit.clone()), "the same edit");

    let kinds = |kind: u64| sent_by_core(&world, &group, kind);
    // Beside the session item the core sent the chat when it joined (D-175).
    let approvals = (kinds(ITEM_KIND).into_iter())
        .filter(|m| m.plaintext != "No session has started yet.")
        .count();
    assert_eq!(approvals, 1);
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

/// A direct chat between the person and the agent, with the core stopped once the agent is in it
/// and has sent the chat its session item (D-175).
async fn a_quiet_chat(world: &World) -> GroupKey {
    world.runtime.start().await.unwrap();
    let core = world.start(Echo::new(Arc::new(AtomicUsize::new(0))));
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let group = GroupKey(world.joined(&group).await);
    let item = marmot_agent::carrier::ITEM_KIND;
    eventually(|| (!sent_by_core(world, &group, item).is_empty()).then_some(())).await;
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
        thread: None,
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
    // The second try must start in the second MDK dated the first, which a busy machine can make
    // it miss: then it is another event, which MDK takes and drops too. The disband goes through
    // a moment later, and MDK refuses every send to the group after, so the case is tried again
    // with a new person and agent. So it is too when the disband went through before either send
    // was looked at: MDK refused it for good, as it should, and the case was not tried.
    for n in 0.. {
        assert!(n < 10, "two sends in one second, in ten tries");
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
        let before = sent_by_core(&world, &group, 9);
        let tried = world.send(&group, &words, &first).await;
        if disbanded(&world, &group) {
            world.runtime.shutdown().await;
            continue;
        }
        let dated = dated_since(&world, &group, &before);
        let again = marmot_agent::Token {
            mdk: "token a, again".into(),
            ..first.clone()
        };
        let started = unix_second();
        let tried_again = world.send(&group, &words, &again).await;
        let met =
            !disbanded(&world, &group) && met_the_first(&world, &group, &again, dated, started);
        world.runtime.shutdown().await;
        if !met {
            continue;
        }
        assert_eq!(tried, Err(SendFailed::Rejected), "taken, and dropped");
        assert_eq!(
            tried_again,
            Err(SendFailed::Busy),
            "busy until the next second"
        );
        break;
    }
}

/// Whether MDK's disband of `group` has gone through: MDK refuses every send to it from then on,
/// whatever the token, and keeps no record of one. MDK marks the group so in the same change that
/// ends it.
fn disbanded(world: &World, group: &GroupKey) -> bool {
    let record = (world.marmot.app).group(&world.marmot.label, group.as_str());
    record
        .expect("MDK reads its group")
        .is_none_or(|g| g.disbanded)
}

/// This machine's clock, in whole seconds, as MDK dates an event.
fn unix_second() -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    now.unwrap().as_secs()
}

/// The second MDK dated the one event the agent made in `group` since it kept `before`: what
/// MDK recorded, not this machine's clock around the send.
fn dated_since(world: &World, group: &GroupKey, before: &[marmot_app::AppMessageRecord]) -> u64 {
    let new: Vec<_> = (sent_by_core(world, group, 9).into_iter())
        .filter(|m| before.iter().all(|b| b.message_id_hex != m.message_id_hex))
        .collect();
    assert_eq!(new.len(), 1, "one event made of the send");
    new[0].recorded_at
}

/// Whether a send under `token`, started at `started`, met the event MDK dated `dated`: it
/// started in that second, and MDK took nothing under its token, which MDK says itself, apart
/// from what the core made of its answer. If MDK took it, it was another event, of a later
/// second; if it started later, it could not be the same event. Either way the case was not
/// tried, and says nothing.
fn met_the_first(
    world: &World,
    group: &GroupKey,
    token: &marmot_agent::Token,
    dated: u64,
    started: u64,
) -> bool {
    let status =
        (world.marmot.runtime).local_send_status(&world.marmot.label, &group_id(group), &token.mdk);
    let took = status.expect("MDK says what it has of a token").is_some();
    let of_that_second = sent_by_core(world, group, 9)
        .iter()
        .filter(|m| m.recorded_at == dated)
        .count();
    assert_eq!(
        of_that_second, 1,
        "MDK makes the same words one event in a second"
    );
    started == dated && !took
}

// Two sends of the same words, with the same tags, in one second would be one event to MDK: the
// second waits for the next second, and both go out (S1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_words_twice_in_one_second_both_go_out() {
    use marmot_agent::marmot::SendFailed;
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    // The second send must start in the second MDK dated the first, which a busy machine can
    // make it miss: then they are two events, and are tried again in a later second under tokens
    // of their own.
    for n in 0.. {
        assert!(n < 5, "two sends in one second, in five tries");
        let before = sent_by_core(&world, &group, 9);
        let (a, first) = words_named(&format!("a{n}"));
        let (b, second) = words_named(&format!("b{n}"));
        at_a_new_second().await;
        let went = world.send(&group, &a, &first).await.unwrap();
        let dated = dated_since(&world, &group, &before);
        let started = unix_second();
        let tried = world.send(&group, &b, &second).await;
        if !met_the_first(&world, &group, &second, dated, started) {
            continue;
        }
        assert_eq!(tried, Err(SendFailed::Busy), "busy until the next second");
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let also = world.send(&group, &b, &second).await.unwrap();
        assert_ne!(went, also, "two events");
        let sent = sent_by_core(&world, &group, 9).len();
        assert_eq!(sent, before.len() + 2, "both went out");
        break;
    }
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

// A send of the agent's full activity that MDK was handed while the chat was the owner's goes on,
// after someone joins, only as MDK says when asked (`outcome`): once its engine took it, MDK names
// the event the send became, which is the event MDK gave back when it took it, and names it too
// while it keeps the send queued or its engine has it (mdk#3712). MDK lets go of a token with the
// message it made, once the group's messages disappear (D-136, D-109).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mdk_says_what_became_of_a_send() {
    use marmot_agent::marmot::{Outcome, Side};
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    (world.runtime)
        .update_message_retention(&world.person, &group_id(&group), 3600)
        .await
        .unwrap();
    let keep = || {
        let record = world.marmot.app.group(&world.marmot.label, group.as_str());
        record
            .ok()
            .flatten()
            .map(|r| r.message_retention.disappearing_message_secs)
    };
    eventually(|| (keep() == Some(3600)).then_some(())).await;
    let (words, token) = words_named("kept");
    let went = world.send(&group, &words, &token).await.unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let now = now.unwrap().as_secs();
    let tried = |ago: u64| marmot_agent::Token {
        tried_at: Some(now - ago),
        ..token.clone()
    };
    eventually(|| {
        (world.marmot.outcome(&group, &tried(0)) == Ok(Outcome::Went(went.clone()))).then_some(())
    })
    .await;
    let (_, never) = words_named("never handed");
    assert_eq!(world.marmot.outcome(&group, &never), Ok(Outcome::NotTaken));
    let forgotten = marmot_agent::Token {
        tried_at: Some(now - 7200),
        ..never
    };
    assert_eq!(
        world.marmot.outcome(&group, &forgotten),
        Ok(Outcome::Forgotten),
        "the group's messages have disappeared since it was first tried"
    );

    // MDK keeps one queued for a while, then its engine has it: MDK names its event throughout.
    world.marmot.faults.push(Fault::Owned {
        queued: std::time::Duration::from_secs(5),
        owned: std::time::Duration::from_secs(3600),
    });
    let (mut words, held) = words_named("held");
    if let marmot_agent::Sending::Words { text, .. } = &mut words {
        *text = "other words".into();
    }
    let pending = world.marmot.send(&group, &words, &held).await;
    let Err(marmot_agent::marmot::SendFailed::Pending(event)) = pending else {
        panic!("MDK keeps it queued: {pending:?}");
    };
    assert_eq!(
        world.marmot.outcome(&group, &held),
        Ok(Outcome::Queued(event.clone()))
    );
    eventually(|| {
        (world.marmot.outcome(&group, &held) == Ok(Outcome::Went(event.clone()))).then_some(())
    })
    .await;
    world.runtime.shutdown().await;
}

// In a chat whose messages disappear, an item's first event can be gone while the item goes on.
// MDK then refuses an edit of it, as it no longer has the event nor the token it went under, and
// says so: the core is told the first event is gone, apart from any other refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_of_a_first_event_that_disappeared_is_told_apart() {
    use marmot_agent::carrier::Item;
    use marmot_agent::marmot::SendFailed;
    use marmot_agent::{Sending, Token};
    use marmot_agent_events::body::{StartedBy, Turn};
    use marmot_agent_events::{AgentEvent, Header};
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    (world.runtime)
        .update_message_retention(&world.person, &group_id(&group), 60)
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
    let session = |version: u64, first: Option<marmot_agent::EventKey>| {
        let header = Header {
            harness: "codex".into(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            subagent: None,
            item: "t1".into(),
            seq: version,
            state: State::Running,
            title: "Turn".into(),
            text: format!("version {version}"),
        };
        let body = Body::Turn(Turn {
            started_by: StartedBy::User,
            duration_ms: None,
            usage: None,
        });
        Sending::Item {
            key: "t1".into(),
            item: Box::new(Item {
                event: AgentEvent::new(header, body, None).unwrap(),
                version,
                first,
                started_by: None,
                settles: Vec::new(),
                thread: None,
            }),
        }
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let now = now.unwrap().as_secs();
    let token = |mdk: &str, edits: Option<&str>| Token {
        send: mdk.into(),
        mdk: mdk.into(),
        edits: edits.map(Into::into),
        tried_at: Some(now),
    };
    let first = (world.send(&group, &session(1, None), &token("v1", None)))
        .await
        .unwrap();
    // The first event's time comes: MDK's sweep takes the clock it is given.
    let later = (now + 3600) * 1000;
    let mut gone = false;
    for _ in 0..100 {
        (world.marmot.runtime)
            .sweep_expired_retention(&world.marmot.label, later)
            .await
            .unwrap();
        gone = (world.marmot.runtime)
            .timeline_message(&world.marmot.label, group.as_str(), first.as_str())
            .unwrap()
            .is_none();
        if gone {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(gone, "the first event disappeared");
    let edit = session(2, Some(first.clone()));
    let sent = world.send(&group, &edit, &token("v2", Some("v1"))).await;
    assert_eq!(sent, Err(SendFailed::FirstGone));
    world.runtime.shutdown().await;
}

// The agent's words carry as many `settles` as MDK takes in a chat message, with their turn and
// phase, and no more: the numbers the core keeps for MDK's limits, which MDK does not export, are
// MDK's. A long turn leaves room for fewer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_agents_words_settle_as_many_as_mdk_takes() {
    use marmot_agent::marmot::SendFailed;
    use marmot_agent::{EventKey, Sending, Token, carrier};
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    let requests: Vec<EventKey> = (0..300)
        .map(|n| EventKey::from(format!("{n:064x}").as_str()))
        .collect();
    for (name, turn) in [("short", "t1".to_string()), ("long", "t".repeat(12 * 1024))] {
        let turn = Some(marmot_agent_events::TurnId(turn));
        let fit = carrier::words_settles_fit(turn.as_ref(), true, &requests);
        assert!(fit > 0 && fit < carrier::WORDS_TAGS_MAX, "{name}: {fit}");
        for (count, takes) in [(fit, true), (fit + 1, false)] {
            let words = Sending::Words {
                key: format!("{name}{count}"),
                text: format!("words {name} {count}"),
                turn: turn.clone(),
                remark: true,
                edit: None,
                settles: requests[..count].to_vec(),
                thread: None,
            };
            let token = Token {
                send: format!("{name}{count}"),
                mdk: format!("token {name} {count}"),
                edits: None,
                tried_at: None,
            };
            let sent = world.send(&group, &words, &token).await;
            if takes {
                assert!(sent.is_ok(), "{name}: {count} taken: {sent:?}");
            } else {
                assert_eq!(
                    sent,
                    Err(SendFailed::Refused { gone: false }),
                    "{name}: {count}"
                );
            }
        }
    }
    world.runtime.shutdown().await;
}

/// A stand-in harness that keeps every prompt it is handed, and says nothing.
struct Listener {
    prompts: Arc<std::sync::Mutex<Vec<marmot_agent_driver::Prompt>>>,
}

impl marmot_agent_driver::Driver for Listener {
    fn harness(&self) -> marmot_agent_driver::Harness {
        marmot_agent_driver::Harness {
            name: "listener".into(),
            version: None,
        }
    }

    fn capabilities(&self) -> marmot_agent_driver::Capabilities {
        marmot_agent_driver::Capabilities {
            interrupt: false,
            mid_turn: marmot_agent_driver::MidTurn::Queue,
            sessions: marmot_agent_driver::Sessions::Core,
            resume: false,
            approvals: false,
            files_in: marmot_agent_driver::FilesIn::None,
            subagents: false,
            stop_subagent: false,
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(
        &mut self,
        prompt: marmot_agent_driver::Prompt,
    ) -> Result<(), marmot_agent_driver::Refusal> {
        self.prompts.lock().unwrap().push(prompt);
        Ok(())
    }

    async fn answer(
        &mut self,
        _answer: marmot_agent_driver::Answer,
    ) -> Result<(), marmot_agent_driver::Refusal> {
        Ok(())
    }

    async fn control(
        &mut self,
        _control: marmot_agent_driver::Control,
    ) -> Result<(), marmot_agent_driver::Refusal> {
        Ok(())
    }

    async fn next_event(&mut self) -> Option<marmot_agent_driver::Output> {
        std::future::pending().await
    }
}

// In a group, a reply to the agent's message is for the agent (D-075): the core reads the message
// it quotes from MDK's storage, and hands the harness the quote's text and author as the core's
// own ids, never an event id or a key (D-036). A message in the group that addresses nobody is not
// a prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_to_the_agent_in_a_group_quotes_what_it_replies_to() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let prompts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let core = world.start(Listener {
        prompts: prompts.clone(),
    });
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
    let group = GroupKey(world.joined(&named).await);
    let (mut words, token) = words_named("a");
    if let marmot_agent::Sending::Words { text, .. } = &mut words {
        *text = "the agent's earlier words".into();
    }
    let agents = world.send(&group, &words, &token).await.unwrap();
    eventually(|| {
        (world.runtime)
            .timeline_message(&world.person, group.as_str(), agents.as_str())
            .unwrap()
    })
    .await;

    world
        .runtime
        .send_message(&world.person, &named, b"lunch, anyone?".to_vec())
        .await
        .unwrap();
    world
        .runtime
        .reply_to_message(&world.person, &named, agents.as_str(), "about that")
        .await
        .unwrap();
    let prompt = eventually(|| prompts.lock().unwrap().first().cloned()).await;
    assert_eq!(prompt.text, "about that");
    let quoted = prompt.reply_to.expect("the quote");
    assert_eq!(quoted.text, "the agent's earlier words");
    assert_ne!(quoted.author.id, prompt.sender.id, "the agent wrote it");
    assert_ne!(quoted.message.0, agents.0, "the core's own id");
    assert!(!quoted.author.id.0.contains(&world.agent), "not a key");
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(prompts.lock().unwrap().len(), 1, "only the reply");
    core.stop().await;
    world.runtime.shutdown().await;
}

// An edit whose token names no edits, as the core gives one for words an older core sent without
// a token, goes out as MDK's plain edit of the message: the person sees the new text in the
// message's place.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_without_an_edits_token_is_mdks_plain_edit() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    let group = a_quiet_chat(&world).await;
    let (words, token) = words_named("a");
    let first = world.send(&group, &words, &token).await.unwrap();
    let edit = marmot_agent::Sending::Words {
        key: "a".into(),
        text: "the words, changed".into(),
        turn: Some("t1".into()),
        remark: true,
        edit: Some(first.clone()),
        settles: Vec::new(),
        thread: None,
    };
    let older = marmot_agent::Token {
        send: "a/2".into(),
        mdk: "token a/2".into(),
        edits: None,
        tried_at: None,
    };
    let edited = world.send(&group, &edit, &older).await.unwrap();
    assert_ne!(edited, first, "an event of its own");
    eventually(|| {
        let row = (world.runtime)
            .timeline_message(&world.person, group.as_str(), first.as_str())
            .unwrap()?;
        (row.plaintext == "the words, changed").then_some(row)
    })
    .await;
    world.runtime.shutdown().await;
}

// An invitation from someone who may not add the agent is declined: the agent never joins the
// group, and the invitation waits no more (D-010).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invitation_from_someone_not_allowed_is_declined() {
    let mut world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.config.owner = marmot_agent::PubKey("ab".repeat(32));
    world.runtime.start().await.unwrap();
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let group = GroupKey(hex::encode(group.as_slice()));
    eventually(|| {
        let waiting = world.marmot.invitations();
        waiting.iter().any(|(g, _)| *g == group).then_some(())
    })
    .await;
    let core = world.start(Echo::new(Arc::new(AtomicUsize::new(0))));
    eventually(|| world.marmot.invitations().is_empty().then_some(())).await;
    assert!(
        !world.marmot.groups().unwrap().contains(&group),
        "not joined"
    );
    core.stop().await;
    world.runtime.shutdown().await;
}

// `marmot-agent groups` lists each group the agent is in: its id, its name, whether it is the
// owner's two-person chat, which is sent the agent's full activity as it is, and whether
// `full_activity` lists it (D-136).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_groups_the_agent_is_in_are_listed() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let core = world.start(Echo::new(Arc::new(AtomicUsize::new(0))));
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
    let named = GroupKey(world.joined(&named).await);
    let direct = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let direct = GroupKey(world.joined(&direct).await);
    core.stop().await;

    let owner = &world.config.owner;
    let listed = |owner: &PubKey, activity: &FullActivity| {
        let mut listed = world.marmot.listed(owner, activity).unwrap();
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        listed
    };
    let row = |group: &GroupKey, name: &str, owners_chat, full_activity| Listed {
        group: group.clone(),
        name: name.into(),
        owners_chat,
        full_activity,
    };
    assert_eq!(
        listed(owner, &FullActivity::Groups(vec![named.clone()])),
        vec![
            row(&direct, "", true, false),
            row(&named, "Team", false, true),
        ]
    );
    assert_eq!(
        listed(owner, &FullActivity::All),
        vec![
            row(&direct, "", true, true),
            row(&named, "Team", false, true)
        ]
    );
    // A named group of two is not the owner's two-person chat, and neither is a direct chat
    // with someone who is not the owner.
    assert_eq!(
        listed(&PubKey("ab".repeat(32)), &FullActivity::default()),
        vec![
            row(&direct, "", false, false),
            row(&named, "Team", false, false),
        ]
    );
    world.runtime.shutdown().await;
}

// The command itself, on a home `marmot-agent init` made: it opens the home without the core and
// prints, to the terminal, that the agent is in no groups yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_groups_command_reads_the_agents_home() {
    let relay = common::relay().await;
    let url = relay.url().await.to_string();
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.toml");
    std::fs::write(
        &settings,
        format!(
            "home = {home:?}\nrelays = [{url:?}]\nowner = \"{owner}\"\nwhite_noise = false\n\
             allow_loopback_relays = true\n\n[connector]\n",
            home = dir.path().join("agent").display().to_string(),
            owner = "ab".repeat(32),
        ),
    )
    .unwrap();
    let agent = |command: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
            .arg(command)
            .arg(&settings)
            .output()
            .unwrap()
    };
    assert!(
        agent("init").status.success(),
        "the agent's identity is made"
    );
    let listed = agent("groups");
    assert!(listed.status.success(), "{listed:?}");
    assert_eq!(
        String::from_utf8(listed.stdout).unwrap(),
        "The agent is in no groups.\n"
    );
}

// `marmot-agent init` on a home that holds an identity, as MDK's account records say, refuses before
// it opens the home's store, which a newer core would move forward where an older one cannot open
// it (D-159, D-173). A home whose first `init` was stopped before it made an identity is not refused
// (#215): the installer leaves that rule to the core.
#[test]
fn init_refuses_a_home_with_an_identity_before_it_opens_its_store() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("agent");
    let record = home.join("accounts/agent/account.json");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    std::fs::write(
        &record,
        format!(
            "{{\"label\":\"agent\",\"account_id_hex\":\"{}\",\"local_signing\":true}}",
            "ab".repeat(32)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&record, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let settings = dir.path().join("settings.toml");
    std::fs::write(
        &settings,
        format!(
            "home = {home:?}\nrelays = []\nowner = \"{owner}\"\n\n[connector]\n",
            home = home.display().to_string(),
            owner = "cd".repeat(32),
        ),
    )
    .unwrap();
    let init = std::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .arg("init")
        .arg(&settings)
        .output()
        .unwrap();
    assert!(!init.status.success());
    assert!(
        String::from_utf8_lossy(&init.stderr).contains("the home already has an identity"),
        "{init:?}"
    );
    let found: Vec<_> = walk(&home)
        .into_iter()
        .map(|p| p.strip_prefix(&home).unwrap().display().to_string())
        .collect();
    assert_eq!(
        found,
        ["accounts", "accounts/agent", "accounts/agent/account.json"],
        "nothing else in the home was made or opened"
    );
}

/// Every path under a folder, sorted.
fn walk(folder: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(walk(&path));
        }
        found.push(path);
    }
    found.sort();
    found
}

/// A stand-in for a harness whose subagents the test scripts: each prompt is handed to the test,
/// and what the test hands back is what the driver says (D-121).
struct Scripted {
    events: tokio::sync::mpsc::UnboundedReceiver<marmot_agent_driver::Emitted>,
    prompts: tokio::sync::mpsc::UnboundedSender<marmot_agent_driver::Prompt>,
}

impl marmot_agent_driver::Driver for Scripted {
    fn harness(&self) -> marmot_agent_driver::Harness {
        marmot_agent_driver::Harness {
            name: "scripted".into(),
            version: None,
        }
    }

    fn capabilities(&self) -> marmot_agent_driver::Capabilities {
        marmot_agent_driver::Capabilities {
            interrupt: false,
            mid_turn: marmot_agent_driver::MidTurn::Queue,
            sessions: marmot_agent_driver::Sessions::Core,
            resume: false,
            approvals: false,
            files_in: marmot_agent_driver::FilesIn::None,
            subagents: true,
            stop_subagent: false,
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(
        &mut self,
        prompt: marmot_agent_driver::Prompt,
    ) -> Result<(), marmot_agent_driver::Refusal> {
        let _ = self.prompts.send(prompt);
        Ok(())
    }

    async fn answer(
        &mut self,
        _answer: marmot_agent_driver::Answer,
    ) -> Result<(), marmot_agent_driver::Refusal> {
        Ok(())
    }

    async fn control(
        &mut self,
        _control: marmot_agent_driver::Control,
    ) -> Result<(), marmot_agent_driver::Refusal> {
        Ok(())
    }

    async fn next_event(&mut self) -> Option<marmot_agent_driver::Output> {
        self.events
            .recv()
            .await
            .map(marmot_agent_driver::Output::from)
    }
}

/// A group, "Team", of the person and the agent, whose driver, [`Scripted`], the test speaks for:
/// the agent was prompted there once, and [`Team::emit`] says what the driver says in that prompt's
/// turn, `t1` of session `s1`.
struct Team {
    world: World,
    core: common::Running,
    say: tokio::sync::mpsc::UnboundedSender<marmot_agent_driver::Emitted>,
    prompt: marmot_agent_driver::Prompt,
    group: cgka_traits::GroupId,
    hex: String,
    seq: u64,
}

impl Team {
    async fn new() -> Self {
        let mut world = World::new("[codex]\ncwd = \"/unused\"\n").await;
        world.config.full_activity = FullActivity::All;
        world.runtime.start().await.unwrap();
        let (say, events) = tokio::sync::mpsc::unbounded_channel();
        let (prompts, mut prompted) = tokio::sync::mpsc::unbounded_channel();
        let core = world.start(Scripted { events, prompts });
        let group = (world.runtime)
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
        let hex = world.joined(&group).await;
        // A message to a newly made group sometimes does not arrive, which is MDK's matter: it is
        // sent again until the agent has it.
        let mut prompt = None;
        for _ in 0..5 {
            let mention = vec![vec!["p".to_string(), world.agent.clone()]];
            (world.runtime)
                .send_tagged_text(&world.person, &group, "split it up".into(), mention)
                .await
                .unwrap();
            let waited = tokio::time::timeout(std::time::Duration::from_secs(10), prompted.recv());
            if let Ok(Some(got)) = waited.await {
                prompt = Some(got);
                break;
            }
        }
        let prompt = prompt.expect("the agent was prompted in the group");
        Self {
            world,
            core,
            say,
            prompt,
            group,
            hex,
            seq: 0,
        }
    }

    /// The driver says `item` is in `state`, as the work of the subagent whose `task` item is `of`.
    fn emit(&mut self, item: &str, of: Option<&str>, state: State, body: Body, text: &str) {
        use marmot_agent_events::{AgentEvent, Header};
        self.seq += 1;
        let header = Header {
            harness: "scripted".into(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            subagent: of.map(Into::into),
            item: item.into(),
            seq: self.seq,
            state,
            title: item.into(),
            text: text.into(),
        };
        let prompts = match body {
            Body::Turn(_) => vec![self.prompt.message.clone()],
            _ => Vec::new(),
        };
        let event = AgentEvent::new(header, body, None).unwrap();
        (self.say)
            .send(marmot_agent_driver::Emitted {
                event,
                chat: Some(self.prompt.chat.id.clone()),
                prompts,
                settles: Vec::new(),
            })
            .unwrap();
    }

    /// What `account`'s app has of the group now.
    fn timeline(&self, account: &str) -> Vec<marmot_app::TimelineMessageRecord> {
        let query = marmot_app::TimelineMessageQuery {
            group_id_hex: Some(self.hex.clone()),
            ..Default::default()
        };
        (self.world.runtime)
            .timeline_messages_with_query(account, query)
            .map(|page| page.messages)
            .unwrap_or_default()
    }

    /// `account`'s row of the item `item`.
    fn row_of(&self, account: &str, item: &str) -> Option<marmot_app::TimelineMessageRecord> {
        (self.timeline(account).into_iter()).find(|m| {
            m.kind == marmot_agent::carrier::ITEM_KIND
                && m.agent_item.as_ref().is_some_and(|i| i.item_id == item)
        })
    }

    /// The agent's words in `account`'s app: its chat messages and thread comments.
    fn words(&self, account: &str) -> Vec<marmot_app::TimelineMessageRecord> {
        (self.timeline(account).into_iter())
            .filter(|m| m.sender == self.world.agent && matches!(m.kind, 9 | 1111))
            .collect()
    }

    async fn stop(self) {
        self.core.stop().await;
        self.world.runtime.shutdown().await;
    }
}

/// Where a row is in a thread: its root and its parent.
fn place(row: &marmot_app::TimelineMessageRecord) -> Option<(String, String)> {
    let t = row.thread.as_ref()?;
    Some((
        t.root_message_id_hex.clone(),
        t.parent_message_id_hex.clone(),
    ))
}

fn turn_body() -> Body {
    use marmot_agent_events::body::{StartedBy, Turn};
    Body::Turn(Turn {
        started_by: StartedBy::User,
        duration_ms: None,
        usage: None,
    })
}

fn task_body() -> Body {
    Body::Task(marmot_agent_events::body::Task {
        kind: "subagent".into(),
        description: Some("look around".into()),
        subject: None,
    })
}

fn command_body() -> Body {
    use marmot_agent_events::body::{Tool, ToolKind};
    Body::Tool(Tool {
        kind: ToolKind::Command,
        name: "shell".into(),
        command: Some("ls".into()),
        cwd: None,
        exit_code: None,
        duration_ms: None,
        output: None,
        changes: Vec::new(),
        query: None,
        path: None,
        input: None,
        error: None,
    })
}

fn words_body() -> Body {
    Body::Message(marmot_agent_events::body::Message::default())
}

// A subagent's work reaches a person in its thread, through MDK (D-121, D-122): the subagent's
// `task` item is a row in the main chat, everything it does is in a thread under it, a nested
// subagent's work in the same tree under its own `task` item, and its words are a thread message
// that does not alert. Someone added to the chat while the subagents work sees each item from its
// next edit, in its thread, as having started earlier (D-124, D-133).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subagents_work_reaches_a_person_in_its_thread() {
    let mut team = Team::new().await;
    let world = &team.world;
    let setup = marmot_app::AccountSetupRequest {
        default_relays: vec![cgka_traits::TransportEndpoint(world.relay.clone())],
        bootstrap_relays: vec![cgka_traits::TransportEndpoint(world.relay.clone())],
        publish_initial_key_package: true,
        ..marmot_app::AccountSetupRequest::default()
    };
    let late = (world.runtime.create_identity(setup).await.unwrap())
        .account
        .account_id_hex;
    (world.marmot.app)
        .set_local_notifications_enabled(&world.person, true)
        .unwrap();
    let mut alerts = world.runtime.subscribe_notifications().unwrap();
    let person = world.person.clone();
    team.emit("turn", None, State::Running, turn_body(), "Working");
    team.emit("task:a", None, State::Running, task_body(), "look around");
    team.emit(
        "cmd:a",
        Some("task:a"),
        State::Running,
        command_body(),
        "ls",
    );
    team.emit(
        "w:a",
        Some("task:a"),
        State::Done,
        words_body(),
        "found two files",
    );
    team.emit(
        "task:b",
        Some("task:a"),
        State::Running,
        task_body(),
        "look deeper",
    );
    team.emit(
        "cmd:b",
        Some("task:b"),
        State::Running,
        command_body(),
        "ls -R",
    );

    // The person's app has each item as the agent sent it, in its place.
    let rows: Vec<marmot_app::TimelineMessageRecord> = eventually(|| {
        let rows: Vec<_> = ["task:a", "cmd:a", "task:b", "cmd:b"]
            .iter()
            .map(|item| team.row_of(&person, item))
            .collect::<Option<_>>()?;
        Some(rows)
    })
    .await;
    let [task_a, cmd_a, task_b, cmd_b] = <[_; 4]>::try_from(rows).ok().unwrap();
    let a = task_a.message_id_hex.clone();
    let b = task_b.message_id_hex.clone();
    assert_eq!(
        place(&task_a),
        None,
        "the top-level task item is in the main chat"
    );
    assert_eq!(place(&cmd_a), Some((a.clone(), a.clone())));
    assert_eq!(
        place(&task_b),
        Some((a.clone(), a.clone())),
        "in the outer thread"
    );
    assert_eq!(
        place(&cmd_b),
        Some((a.clone(), b.clone())),
        "under the nested task item"
    );
    // The subagent's words are a thread message under its task item.
    let said = eventually(|| {
        (team.timeline(&person).into_iter()).find(|m| m.plaintext == "found two files")
    })
    .await;
    assert_eq!(said.kind, 1111);
    assert_eq!(place(&said), Some((a.clone(), a.clone())));
    // The agent's words in its own thread alert nobody (D-122).
    let alerted = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let update = alerts.recv().await.expect("notifications");
            if update.message_id_hex.as_deref() == Some(said.message_id_hex.as_str()) {
                return update;
            }
        }
    });
    assert!(alerted.await.is_err(), "the subagent's words alerted");

    // Someone joins while the subagents work, and gets what comes after.
    let world = &team.world;
    (world.runtime)
        .invite_members(&world.person, &team.group, std::slice::from_ref(&late))
        .await
        .unwrap();
    let mut accepted = false;
    for _ in 0..300 {
        if world
            .runtime
            .accept_group_invite(&late, &team.group)
            .await
            .is_ok()
        {
            accepted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(accepted, "the late joiner accepted the invitation");
    // Its copy of the group must have the agent's next edits: wait for a message of the person's.
    (world.runtime)
        .send_message(&world.person, &team.group, b"welcome".to_vec())
        .await
        .unwrap();
    eventually(|| {
        (team.timeline(&late).iter())
            .any(|m| m.plaintext == "welcome")
            .then_some(())
    })
    .await;
    team.emit(
        "cmd:b",
        Some("task:b"),
        State::Done,
        command_body(),
        "ls -R",
    );
    team.emit(
        "task:b",
        Some("task:a"),
        State::Done,
        task_body(),
        "look deeper",
    );
    team.emit("cmd:a", Some("task:a"), State::Done, command_body(), "ls");
    team.emit("task:a", None, State::Done, task_body(), "look around");
    team.emit("turn", None, State::Done, turn_body(), "Done");
    let seen: Vec<marmot_app::TimelineMessageRecord> = eventually(|| {
        let rows: Vec<_> = ["task:a", "cmd:a", "task:b", "cmd:b"]
            .iter()
            .map(|item| team.row_of(&late, item))
            .collect::<Option<_>>()?;
        rows.iter()
            .all(|r| r.agent_item.as_ref().is_some_and(|i| i.version == 2))
            .then_some(rows)
    })
    .await;
    for row in &seen {
        assert!(row.agent_item_first_event_missing, "shown from its edit");
        assert_eq!(row.message_id_hex.len(), 64);
    }
    let [task_a, cmd_a, task_b, cmd_b] = <[_; 4]>::try_from(seen).ok().unwrap();
    assert_eq!(
        task_a.message_id_hex, a,
        "the row is the item's, at its first event's id"
    );
    assert_eq!(place(&task_a), None);
    // An edit names its root alone: the late joiner has each item in the outer thread.
    for row in [&cmd_a, &task_b, &cmd_b] {
        assert_eq!(place(row).map(|p| p.0), Some(a.clone()), "in the thread");
    }
    team.stop().await;
}

// A later version of a subagent's words, which are a thread message, reaches the person as MDK's
// edit of that message: MDK keeps no edit of a thread message under a token, and takes its plain
// edit (D-142). The words went out once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_version_of_a_subagents_words_is_mdks_edit_of_its_thread_message() {
    let mut team = Team::new().await;
    let person = team.world.person.clone();
    team.emit("turn", None, State::Running, turn_body(), "Working");
    team.emit("task:a", None, State::Running, task_body(), "look around");
    team.emit(
        "w:a",
        Some("task:a"),
        State::Done,
        words_body(),
        "found two files",
    );
    let first =
        eventually(|| (team.words(&person).into_iter()).find(|m| m.plaintext == "found two files"))
            .await;
    assert_eq!(first.kind, 1111);
    team.emit(
        "w:a",
        Some("task:a"),
        State::Done,
        words_body(),
        "found three files",
    );
    eventually(|| {
        let words = team.words(&person);
        (words.len() == 1 && words[0].plaintext == "found three files").then_some(())
    })
    .await;
    let words = team.words(&person);
    assert_eq!(
        words[0].message_id_hex, first.message_id_hex,
        "edited in place"
    );
    assert_eq!(place(&words[0]), place(&first), "in its thread");
    team.stop().await;
}

// In a chat whose messages disappear, a subagent's `task` item can be gone when the subagent
// speaks: MDK has nothing to place the thread message by, and does not take it. The words go in
// the main chat as the agent's words, and stay there: a later version is MDK's edit of that chat
// message. Nothing is lost, and the chat is told of no failure (D-142).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subagents_words_after_its_task_item_disappeared_go_in_the_main_chat() {
    use marmot_agent::carrier::read_item;
    let mut team = Team::new().await;
    let person = team.world.person.clone();
    (team.world.runtime)
        .update_message_retention(&person, &team.group, 60)
        .await
        .unwrap();
    let marmot = team.world.marmot.clone();
    let hex = team.hex.clone();
    eventually(|| {
        let record = marmot.app.group(&marmot.label, &hex).ok().flatten()?;
        (record.message_retention.disappearing_message_secs == 60).then_some(())
    })
    .await;
    team.emit("turn", None, State::Running, turn_body(), "Working");
    team.emit("task:a", None, State::Running, task_body(), "look around");
    let task = eventually(|| team.row_of(&person, "task:a")).await;
    // Only once MDK is done with it: a send MDK still has queued could go out again after the
    // sweep deletes its token (#96).
    team.world.nothing_queued().await;
    // The task item's time comes, for the agent: MDK's sweep takes the clock it is given.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let later = (now.unwrap().as_secs() + 3600) * 1000;
    let mut gone = false;
    for _ in 0..100 {
        (marmot.runtime)
            .sweep_expired_retention(&marmot.label, later)
            .await
            .unwrap();
        gone = (marmot.runtime)
            .timeline_message(&marmot.label, &hex, &task.message_id_hex)
            .unwrap()
            .is_none();
        if gone {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(gone, "the task item disappeared");

    team.emit(
        "w:a",
        Some("task:a"),
        State::Done,
        words_body(),
        "late words",
    );
    let said =
        eventually(|| (team.words(&person).into_iter()).find(|m| m.plaintext == "late words"))
            .await;
    assert_eq!((said.kind, place(&said)), (9, None), "in the main chat");
    team.emit(
        "w:a",
        Some("task:a"),
        State::Done,
        words_body(),
        "late words, again",
    );
    eventually(|| {
        let words = team.words(&person);
        (words.len() == 1 && words[0].plaintext == "late words, again").then_some(())
    })
    .await;
    let failed = (team.timeline(&person).into_iter())
        .filter(|m| m.sender == team.world.agent)
        .filter_map(|m| read_item(m.kind, &m.plaintext, &m.tags))
        .filter(|r| matches!(r.item.event.body(), Body::Notice(_)))
        .count();
    assert_eq!(failed, 0, "nothing failed");
    team.stop().await;
}

// D-175 and D-176 through MDK: a group the agent joins is sent its session item before anyone
// prompts, naming no session, with who may approve read from MDK's member list; a restart sends it
// nothing again. A member whose own session item MDK delivers is an agent: its mention is no
// prompt, though everyone may prompt, while the person's is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_joined_group_is_sent_its_rights_and_another_agent_is_not_prompted_by() {
    use marmot_agent::carrier::Item;
    use marmot_agent::{Sending, Token};
    use marmot_agent_events::body::{Holders, Session};
    use marmot_agent_events::{AgentEvent, Header};
    let world = World::new("prompt = \"everyone\"\n[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let other = world.someone_else().await;
    let prompts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let core = world.start(Listener {
        prompts: prompts.clone(),
    });
    let named = (world.runtime)
        .create_group(
            &world.person,
            "Team",
            &[world.agent.clone(), other.clone()],
            None,
        )
        .await
        .unwrap()
        .operation
        .expect("the group is made");
    let hex = world.joined(&named).await;
    let session = |items: Vec<(marmot_agent::EventKey, marmot_agent::carrier::Read)>| {
        (items.into_iter()).find_map(|(_, read)| match read.item.event.body() {
            Body::Session(s) if read.item.event.header().session.is_none() => Some(s.clone()),
            _ => None,
        })
    };
    let shown = eventually(|| session(world.items(&hex))).await;
    let rights = shown.rights.expect("rights shown");
    assert!(matches!(rights.prompt, Holders::Everyone(_)));
    assert!(matches!(&rights.approve, Holders::Members(keys) if *keys == [world.person.clone()]));
    assert_eq!((shown.label, shown.owner), (None, None));

    // The other member's own session item, as its core would send it.
    let theirs = marmot_agent::marmot::Marmot {
        label: other.clone(),
        agent: PubKey(other.clone()),
        ..world.marmot.clone()
    };
    let header = Header {
        harness: "codex".into(),
        session: None,
        turn: None,
        subagent: None,
        item: "core-1".into(),
        seq: 1,
        state: State::Done,
        title: "Session".into(),
        text: "No session has started yet.".into(),
    };
    let body = Body::Session(Session {
        harness_version: "1".into(),
        ..Session::default()
    });
    let item = Sending::Item {
        key: "theirs".into(),
        item: Box::new(Item {
            event: AgentEvent::new(header, body, None).unwrap(),
            version: 1,
            first: None,
            started_by: None,
            settles: Vec::new(),
            thread: None,
        }),
    };
    let token = Token {
        send: "theirs".into(),
        mdk: "theirs".into(),
        edits: None,
        tried_at: None,
    };
    let group = GroupKey(hex.clone());
    eventually(|| {
        (theirs.invitations().iter())
            .any(|(g, _)| *g == group)
            .then_some(())
    })
    .await;
    theirs.decide(&group, true).await.unwrap();
    let mut sent = None;
    for _ in 0..3000 {
        match theirs.send(&group, &item, &token).await {
            Err(marmot_agent::marmot::SendFailed::Pending(_)) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            done => {
                sent = Some(done);
                break;
            }
        }
    }
    sent.expect("MDK took the send").expect("it went out");
    let mention = |text: &str| {
        (
            text.to_string(),
            vec![vec!["p".to_string(), world.agent.clone()]],
        )
    };
    let (text, tags) = mention("you, do this");
    (world.runtime)
        .send_tagged_text(&other, &named, text, tags)
        .await
        .unwrap();
    let (text, tags) = mention("and you, this");
    (world.runtime)
        .send_tagged_text(&world.person, &named, text, tags)
        .await
        .unwrap();
    let prompt = eventually(|| prompts.lock().unwrap().first().cloned()).await;
    assert_eq!(prompt.text, "and you, this");
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(
        prompts.lock().unwrap().len(),
        1,
        "the other agent's mention is no prompt"
    );
    core.stop().await;

    let core = world.start(Listener {
        prompts: prompts.clone(),
    });
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    core.stop().await;
    assert_eq!(
        world
            .sent_by_agent(&hex, marmot_agent::carrier::ITEM_KIND)
            .len(),
        1,
        "one session item, sent once and never edited"
    );
    world.runtime.shutdown().await;
}
