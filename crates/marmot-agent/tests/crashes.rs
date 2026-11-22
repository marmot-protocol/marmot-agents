//! The core across a crash and a failed save, end to end through MDK: what reaches the driver,
//! what is sent, and what is settled once the core starts again (S1, S3, S4).

mod common;

use common::{Chat, World, first, within};
use marmot_agent::carrier::Asked;
use marmot_agent::marmot::Fault;
use marmot_agent::run::{Kept, Store};
use marmot_agent::{Conversation, EventKey, GroupKey, Heard, PubKey, Sending, Where};
use marmot_agent_driver::{
    Answer, Capabilities, ChatId, Control, Driver, Emitted, FilesIn, Harness, MidTurn, Output,
    Prompt, Refusal, Sessions,
};
use marmot_agent_events::body::{
    Approval, Body, Choice, Effect, Message, NoticeKind, StartedBy, Turn,
};
use marmot_agent_events::{AgentEvent, Header, State};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// What the stand-in harness was handed, and the chat it was prompted in.
#[derive(Default)]
struct Seen {
    prompts: Vec<String>,
    answers: usize,
    chat: Option<ChatId>,
}

/// A stand-in harness: it takes every prompt and emits what the test hands it. With `hang`, an
/// answer reaches it and is never taken, as a harness that is slow or stuck.
struct Stub {
    events: mpsc::UnboundedReceiver<Emitted>,
    seen: Arc<Mutex<Seen>>,
    hang: bool,
}

fn stub(hang: bool) -> (Stub, mpsc::UnboundedSender<Emitted>, Arc<Mutex<Seen>>) {
    let (send, events) = mpsc::unbounded_channel();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let stub = Stub {
        events,
        seen: seen.clone(),
        hang,
    };
    (stub, send, seen)
}

impl Driver for Stub {
    fn harness(&self) -> Harness {
        Harness {
            name: "stub".into(),
            version: None,
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: true,
            mid_turn: MidTurn::Queue,
            sessions: Sessions::Core,
            resume: false,
            approvals: true,
            files_in: FilesIn::None,
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let mut seen = self.seen.lock().unwrap();
        seen.prompts.push(prompt.text);
        seen.chat = Some(prompt.chat.id);
        Ok(())
    }

    async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
        self.seen.lock().unwrap().answers += 1;
        if self.hang {
            std::future::pending::<()>().await;
        }
        Ok(())
    }

    async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
        Ok(())
    }

    async fn next_event(&mut self) -> Option<Output> {
        self.events.recv().await.map(Output::from)
    }
}

fn event(item: &str, state: State, body: Body) -> AgentEvent {
    let header = Header {
        harness: "stub".into(),
        session: Some("s1".into()),
        turn: Some("t1".into()),
        item: item.into(),
        seq: 1,
        state,
        title: item.into(),
        text: format!("{item} text"),
    };
    AgentEvent::new(header, body, None).unwrap()
}

fn approval() -> Body {
    Body::Approval(Approval {
        subject: None,
        choices: vec![Choice {
            id: "allow".into(),
            label: "Allow once".into(),
            effect: Effect::Allow,
        }],
        answer: None,
    })
}

fn turn() -> Body {
    Body::Turn(Turn {
        started_by: StartedBy::User,
        duration_ms: None,
        usage: None,
    })
}

/// A direct chat with the agent, and the core's id for it, learnt from a first prompt.
async fn a_chat<'a>(world: &'a World, seen: &Arc<Mutex<Seen>>) -> (Chat<'a>, ChatId) {
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let hex = world.joined(&group).await;
    let chat = Chat {
        world,
        group,
        hex,
        turn: 30,
    };
    chat.say("hello").await;
    let id = within(30, || seen.lock().unwrap().chat.clone()).await;
    (chat, id)
}

/// Waits for the item a request is settled by, and gives it back.
async fn settled_by(chat: &Chat<'_>, request: &EventKey, seconds: u64) -> AgentEvent {
    within(seconds, || {
        let items = chat.items().into_iter().map(|(_, r)| r);
        items
            .into_iter()
            .find(|r| r.item.settles.contains(request))
            .map(|r| r.item.event)
    })
    .await
}

/// The state as a save would write it, as JSON, to look into.
fn json(kept: &Kept) -> serde_json::Value {
    serde_json::to_value(kept).unwrap()
}

/// What picks the save that fails, once a test has set it.
type Armed = Arc<Mutex<Option<Box<dyn Fn(&serde_json::Value) -> bool + Send>>>>;

/// A store whose saves fail from the first one whose state the armed check matches: the fault is
/// put at the write a test means, not at whichever write comes next. The flag says whether it
/// has failed.
fn armed_store(world: &World) -> (Store, Armed, Arc<AtomicBool>) {
    let armed: Armed = Arc::default();
    let check = armed.clone();
    let (store, failed) = Store::failing_when(world.state.path(), move |kept| {
        (check.lock().unwrap().as_ref()).is_some_and(|when| when(&json(kept)))
    });
    (store, armed, failed)
}

/// Whether a state queues a send whose token starts with `prefix`.
fn queues(state: &serde_json::Value, prefix: &str) -> bool {
    (state["conversation"]["outbox"].as_array()).is_some_and(|queue| {
        (queue.iter()).any(|q| q["token"].as_str().is_some_and(|t| t.starts_with(prefix)))
    })
}

fn refused(event: &AgentEvent) -> Option<String> {
    match event.body() {
        Body::Notice(n) if n.kind == NoticeKind::Refused => n.reason.clone(),
        _ => None,
    }
}

// An answer the core handed to the driver is on disk as pending before the driver gets it: a
// crash while the driver holds it leaves it to be settled, as failed, when the core starts again,
// never pending for good (S1, D-070).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_the_driver_held_at_a_crash_is_settled_after() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(true);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    events
        .send(Emitted {
            event: event("ap", State::Waiting, approval()),
            chat: Some(id),
            prompts: Vec::new(),
            settles: Vec::new(),
        })
        .unwrap();
    let (target, read) = within(30, || {
        chat.latest()
            .into_iter()
            .find(|(_, r)| matches!(r.item.event.body(), Body::Approval(_)))
    })
    .await;
    let target = first(&target, &read);
    let choice = Asked::Choice {
        choice: "allow".into(),
    };
    let request = chat.ask(Some(&target), &choice, "Allow once").await;
    within(30, || (seen.lock().unwrap().answers == 1).then_some(())).await;
    core.crash().await;

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    let settled = settled_by(&chat, &request, 20).await;
    assert_eq!(refused(&settled).as_deref(), Some("failed"));
    core.stop().await;
    world.runtime.shutdown().await;
}

// A message is marked read on disk before it reaches the driver. When that mark cannot be written,
// the prompt does not reach the driver at all, and the core stops: a crash after it would hand
// the driver the same prompt again (S4, D-037).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prompt_whose_read_mark_cannot_be_saved_is_not_delivered() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let (store, armed, failed) = armed_store(&world);
    let core = world.start_with(driver, store);
    let (chat, _) = a_chat(&world, &seen).await;
    // The save that fails is the one that marks "second" read: past the mark "hello" left.
    let group = GroupKey(chat.hex.clone());
    let kept = Store::new(world.state.path()).load().unwrap();
    let mark = kept.read[&group];
    *armed.lock().unwrap() = Some(Box::new(move |state| {
        state["read"][group.as_str()].as_i64() > Some(mark)
    }));
    chat.say("second").await;
    let stopped = core.stops_within(20).await;
    assert_eq!(seen.lock().unwrap().prompts, vec!["hello".to_string()]);
    assert!(stopped, "the core stops when it cannot save");
    assert!(failed.load(SeqCst), "the save that failed was the mark's");
    assert!(core.ended().await.is_err(), "it says it could not save");
    world.runtime.shutdown().await;
}

// Nothing goes to the chat before the core has saved that it means to send it, with its version
// and what it settles: when that cannot be saved, nothing is sent and the core stops (S1, S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_sent_that_was_not_saved_first() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let (store, armed, failed) = armed_store(&world);
    let core = world.start_with(driver, store);
    let (chat, id) = a_chat(&world, &seen).await;
    // The save that fails is the one that queues the turn's first version.
    let first_version = format!("{}/t@1", id.0);
    *armed.lock().unwrap() = Some(Box::new(move |state| queues(state, &first_version)));
    events
        .send(Emitted {
            event: event("t", State::Running, turn()),
            chat: Some(id),
            prompts: Vec::new(),
            settles: Vec::new(),
        })
        .unwrap();
    let stopped = core.stops_within(20).await;
    assert!(chat.items().is_empty(), "nothing was sent");
    assert!(stopped, "the core stops when it cannot save");
    assert!(failed.load(SeqCst), "the save that failed was the send's");
    assert!(core.ended().await.is_err(), "it says it could not save");
    world.runtime.shutdown().await;
}

// A version the core saved that it meant to send, and had not marked as sent when it stopped, is
// sent when it starts again: the same version, naming the item's first event, not a new row and
// not a version after it (S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_version_saved_but_not_sent_goes_out_after_a_restart() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, _) = a_chat(&world, &seen).await;
    core.stop().await;

    // As the core left it, had it stopped after saving the turn's second version and before
    // sending it.
    let store = Store::new(world.state.path());
    let kept = store.load().unwrap();
    let mut talk = Conversation::new(
        PubKey(world.agent.clone()),
        "stub",
        world.config.rights(),
        Stub::caps(),
        kept.conversation,
    );
    let place = Where {
        group: GroupKey(chat.hex.clone()),
        name: None,
        direct: true,
        members: 2,
    };
    talk.heard(Heard {
        chat: place,
        event: EventKey::from("e-again"),
        author: PubKey(world.person.clone()),
        author_name: "Person".into(),
        text: "again".into(),
        reply_to: None,
        mentions_agent: false,
        attachments: Vec::new(),
    });
    let id = seen.lock().unwrap().chat.clone().unwrap();
    let emitted = |state| Emitted {
        event: event("t", state, turn()),
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    };
    talk.emitted(emitted(State::Running));
    talk.sent(&format!("{}/t", id.0), EventKey::from("e-first"));
    talk.emitted(emitted(State::Done));
    store
        .save(&Kept {
            conversation: talk.saved().clone(),
            read: kept.read,
        })
        .unwrap();

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    let read = within(20, || {
        let items = chat.items().into_iter().map(|(_, r)| r);
        items
            .into_iter()
            .find(|r| r.item.event.header().item.as_str() == "t")
    })
    .await;
    assert_eq!(read.item.version, 2);
    assert_eq!(read.item.first, Some(EventKey::from("e-first")));
    assert_eq!(read.item.event.header().state, State::Done);
    core.stop().await;
    world.runtime.shutdown().await;
}

// An answer is marked read on disk only together with what the core made of it: a request pending
// for the driver, or a refusal queued. A crash after the mark and before the request was saved
// would otherwise leave an answer that is never settled, since the mark says it was handled
// (S1, D-070). The save right after the one that holds the request fails here, as a crash right
// after that one would end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_is_marked_read_only_with_its_request() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let (store, armed, failed) = armed_store(&world);
    let core = world.start_with(driver, store);
    let (chat, id) = a_chat(&world, &seen).await;
    events
        .send(Emitted {
            event: event("ap", State::Waiting, approval()),
            chat: Some(id),
            prompts: Vec::new(),
            settles: Vec::new(),
        })
        .unwrap();
    let (target, read) = within(30, || {
        chat.latest()
            .into_iter()
            .find(|(_, r)| matches!(r.item.event.body(), Body::Approval(_)))
    })
    .await;
    let target = first(&target, &read);
    // The first save that marks the answer read must hold its request too, or it fails, and so
    // does the one after it.
    let group = GroupKey(chat.hex.clone());
    let mark = Store::new(world.state.path()).load().unwrap().read[&group];
    let marked = AtomicBool::new(false);
    *armed.lock().unwrap() = Some(Box::new(move |state| {
        if marked.load(SeqCst) {
            return true;
        }
        if state["read"][group.as_str()].as_i64() <= Some(mark) {
            return false;
        }
        marked.store(true, SeqCst);
        let pending = state["conversation"]["pending"].as_object();
        pending.is_none_or(|p| p.is_empty())
    }));
    let choice = Asked::Choice {
        choice: "allow".into(),
    };
    let request = chat.ask(Some(&target), &choice, "Allow once").await;
    assert!(
        core.stops_within(20).await,
        "the core stops when it cannot save"
    );
    assert!(failed.load(SeqCst));
    assert!(core.ended().await.is_err(), "it says it could not save");

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    let settled = settled_by(&chat, &request, 20).await;
    assert_eq!(refused(&settled).as_deref(), Some("failed"));
    core.stop().await;
    world.runtime.shutdown().await;
}

/// The versions of the item `item` in the chat, oldest first, each with its own event.
fn versions(chat: &Chat<'_>, item: &str) -> Vec<(EventKey, u64, Option<EventKey>)> {
    (chat.items().into_iter())
        .filter(|(_, r)| r.item.event.header().item.as_str() == item)
        .map(|(event, r)| (event, r.item.version, r.item.first))
        .collect()
}

// A send whose reply from MDK was lost may have gone out. The core looks in what MDK stored before
// it sends it again, and finds it: the version goes out once, and the next names it (S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_whose_reply_was_lost_is_not_sent_twice() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    world.marmot.faults.push(Fault::LoseReply);
    let emit = |state| Emitted {
        event: event("t", state, turn()),
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    };
    events.send(emit(State::Running)).unwrap();
    within(20, || (!versions(&chat, "t").is_empty()).then_some(())).await;
    events.send(emit(State::Done)).unwrap();
    within(30, || {
        versions(&chat, "t").iter().find(|v| v.1 == 2).cloned()
    })
    .await;
    let versions = versions(&chat, "t");
    let firsts: Vec<&EventKey> = (versions.iter())
        .filter(|v| v.1 == 1)
        .map(|v| &v.0)
        .collect();
    assert_eq!(firsts.len(), 1, "the first version went out once");
    let second = versions.iter().find(|v| v.1 == 2).unwrap();
    assert_eq!(second.2.as_ref(), Some(firsts[0]));
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send that went out just before a crash, before the core marked it sent, is found in what MDK
// stored when the core starts again, and is not sent a second time (S3). With `lookup_fails`, MDK
// cannot be asked the first time the core looks: that is not knowing, never "not there".
async fn went_out_before_a_crash(lookup_fails: bool) {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;

    // As the core left it, had it crashed after MDK sent the turn's first version and before the
    // core saved that it went.
    let store = Store::new(world.state.path());
    let kept = store.load().unwrap();
    let mut talk = Conversation::new(
        PubKey(world.agent.clone()),
        "stub",
        world.config.rights(),
        Stub::caps(),
        kept.conversation,
    );
    talk.emitted(Emitted {
        event: event("t", State::Running, turn()),
        chat: Some(id),
        prompts: Vec::new(),
        settles: Vec::new(),
    });
    let group = GroupKey(chat.hex.clone());
    let (token, what) = talk.next_send(&group).unwrap();
    talk.attempting(&token);
    let went = world.marmot.send(&group, &what, &token).await.unwrap();
    // A second send of the same event in the same second would be the same event, so a later one.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    store
        .save(&Kept {
            conversation: talk.saved().clone(),
            read: kept.read,
        })
        .unwrap();

    // Starting again closes the open turn: its second version names the first that went out.
    if lookup_fails {
        world.marmot.faults.push(Fault::LookupFails);
    }
    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    within(20, || {
        versions(&chat, "t").iter().find(|v| v.1 == 2).cloned()
    })
    .await;
    let versions = versions(&chat, "t");
    let firsts: Vec<&EventKey> = (versions.iter())
        .filter(|v| v.1 == 1)
        .map(|v| &v.0)
        .collect();
    assert_eq!(firsts, vec![&went], "the first version went out once");
    let second = versions.iter().find(|v| v.1 == 2).unwrap();
    assert_eq!(second.2.as_ref(), Some(&went));
    core.stop().await;
    world.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_that_went_out_before_a_crash_is_not_sent_again() {
    went_out_before_a_crash(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_is_not_sent_again_when_mdk_cannot_be_asked() {
    went_out_before_a_crash(true).await;
}

// A send whose outcome is never known is given up in the end, but only after the core has looked
// for its last try in what MDK stored: one that went out is taken as sent, and the chat is not
// also told it failed (S1, S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_is_looked_for_before_it_is_given_up() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let store = Store::new(world.state.path());
    let core = world.core(driver, store);
    let core = world.run(core.unknown_sends(2, Duration::from_millis(300)));
    let (chat, id) = a_chat(&world, &seen).await;
    // The first try fails before MDK has it, without saying so; the last goes out, and MDK's
    // reply is lost.
    let timed_out = marmot_app::AppError::AccountWorkerResponseTimedOut;
    world.marmot.faults.push(Fault::Refuse(timed_out));
    world.marmot.faults.push(Fault::LoseReply);
    let emit = |state| Emitted {
        event: event("t", state, turn()),
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    };
    events.send(emit(State::Running)).unwrap();
    events.send(emit(State::Done)).unwrap();
    within(30, || {
        versions(&chat, "t").iter().find(|v| v.1 == 2).cloned()
    })
    .await;
    let versions = versions(&chat, "t");
    let firsts: Vec<&EventKey> = (versions.iter())
        .filter(|v| v.1 == 1)
        .map(|v| &v.0)
        .collect();
    assert_eq!(firsts.len(), 1, "the first version went out once");
    let second = versions.iter().find(|v| v.1 == 2).unwrap();
    assert_eq!(second.2.as_ref(), Some(firsts[0]), "and the next names it");
    let failed = (chat.items().into_iter()).filter(|(_, r)| refused(&r.item.event).is_some());
    assert_eq!(failed.count(), 0, "nothing said to have failed");
    core.stop().await;
    world.runtime.shutdown().await;
}

// A group the agent accepts is read from its start, and the mark that says so is on disk before
// the agent accepts: a save that fails stops the core before it accepts, and a message sent to the
// group meanwhile still reaches the driver once the core starts again, never skipped as history.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_group_is_read_from_its_start_after_a_failed_save() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, _) = stub(false);
    // The save that fails is the first that marks a group to be read from its start.
    let (store, failed) = Store::failing_when(world.state.path(), |kept| {
        kept.read.values().any(|&m| m == 0)
    });
    let core = world.start_with(driver, store);
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    assert!(
        core.stops_within(20).await,
        "the core stops when it cannot save"
    );
    assert!(failed.load(SeqCst));
    assert!(core.ended().await.is_err());
    world
        .runtime
        .send_message(&world.person, &group, b"hello".to_vec())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;

    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let prompts = || seen.lock().unwrap().prompts.clone();
    within(20, || (prompts() == ["hello"]).then_some(())).await;
    core.stop().await;
    world.runtime.shutdown().await;
}

/// What the agent sent to a chat, of one kind, as its own storage has it: what the core handed
/// MDK, whether or not the person has it yet.
fn sent_by_core(chat: &Chat<'_>, kind: u64) -> Vec<marmot_app::AppMessageRecord> {
    let query = marmot_app::AppMessageQuery {
        group_id_hex: Some(chat.hex.clone()),
        kinds: Some(vec![kind]),
        limit: None,
    };
    let marmot = &chat.world.marmot;
    (marmot
        .runtime
        .messages_with_query(&marmot.label, query)
        .unwrap())
    .into_iter()
    .filter(|m| m.direction == "sent")
    .collect()
}

/// The agent's words in a chat, as the core sent them.
fn words_in(chat: &Chat<'_>) -> Vec<String> {
    (sent_by_core(chat, 9).into_iter())
        .map(|m| m.plaintext)
        .collect()
}

/// Whether the core sent a version of the item `item` to a chat.
fn sent_item(chat: &Chat<'_>, item: &str) -> bool {
    sent_by_core(chat, marmot_agent::carrier::ITEM_KIND)
        .iter()
        .filter_map(|m| marmot_agent::carrier::read_item(&m.plaintext, &m.tags))
        .any(|r| r.item.event.header().item.as_str() == item)
}

// A send MDK will not take does not hold up the chat, nor any other: an edit of words that were
// removed goes out as a new message, and what comes after it goes out too, while another chat
// goes on meanwhile. What the core sent is read from the agent's own storage: whether the person
// has it soon after is MDK's matter, and with a second group made it sometimes does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_mdk_refuses_does_not_hold_up_the_chat() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    let said = |text: &str, chat: &ChatId| {
        let header = Header {
            text: text.into(),
            ..event("w", State::Done, Body::Message(Message::default()))
                .header()
                .clone()
        };
        Emitted {
            event: AgentEvent::new(header, Body::Message(Message::default()), None).unwrap(),
            chat: Some(chat.clone()),
            prompts: Vec::new(),
            settles: Vec::new(),
        }
    };
    events.send(said("first words", &id)).unwrap();
    let sent = world.words(&chat.hex, "first words").await;
    world
        .runtime
        .delete_message(&world.agent, &chat.group, &sent.message_id_hex)
        .await
        .unwrap();
    let team = world
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
    let team_hex = world.joined(&team).await;
    // A message to a newly made group sometimes does not arrive, which is MDK's matter: it is sent
    // again until the agent has it.
    let mut other_id = None;
    for _ in 0..5 {
        let mention = vec![vec!["p".to_string(), world.agent.clone()]];
        world
            .runtime
            .send_tagged_text(&world.person, &team, "hello team".into(), mention)
            .await
            .unwrap();
        for _ in 0..100 {
            other_id = seen.lock().unwrap().chat.clone().filter(|c| *c != id);
            if other_id.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if other_id.is_some() {
            break;
        }
    }
    let other_id = other_id.expect("the agent was prompted in the group");
    let other = Chat {
        world: &world,
        group: team,
        hex: team_hex,
        turn: 30,
    };

    tokio::time::sleep(Duration::from_secs(1)).await;
    events.send(said("second words", &id)).unwrap();
    events
        .send(Emitted {
            event: event("t", State::Running, turn()),
            chat: Some(id.clone()),
            prompts: Vec::new(),
            settles: Vec::new(),
        })
        .unwrap();
    events
        .send(Emitted {
            event: event("o", State::Running, turn()),
            chat: Some(other_id.clone()),
            prompts: Vec::new(),
            settles: Vec::new(),
        })
        .unwrap();
    within(30, || sent_item(&other, "o").then_some(())).await;
    within(30, || {
        words_in(&chat)
            .iter()
            .any(|w| w == "second words")
            .then_some(())
    })
    .await;
    within(30, || sent_item(&chat, "t").then_some(())).await;
    core.stop().await;
    world.runtime.shutdown().await;
}

/// What the core kept when it stopped, and the conversation taken up from it, to change as the
/// core would have before a crash.
fn taken_up(world: &World) -> (Store, Kept, Conversation) {
    let store = Store::new(world.state.path());
    let kept = store.load().unwrap();
    let talk = Conversation::new(
        PubKey(world.agent.clone()),
        "stub",
        world.config.rights(),
        Stub::caps(),
        kept.conversation.clone(),
    );
    (store, kept, talk)
}

/// The agent's words `text`, done, in the turn `t1`, settling `settles`.
fn words_done(text: &str, chat: &ChatId, settles: Vec<marmot_agent_driver::RequestId>) -> Emitted {
    let header = Header {
        text: text.into(),
        ..event("w", State::Done, Body::Message(Message::default()))
            .header()
            .clone()
    };
    Emitted {
        event: AgentEvent::new(header, Body::Message(Message::default()), None).unwrap(),
        chat: Some(chat.clone()),
        prompts: Vec::new(),
        settles,
    }
}

/// The same words as the agent's said earlier in the turn `t1`, settling nothing.
fn said_before(text: &str) -> Sending {
    Sending::Words {
        key: "earlier".into(),
        text: text.into(),
        turn: Some("t1".into()),
        remark: false,
        edit: None,
        settles: Vec::new(),
    }
}

/// How many times the core sent the agent's words `text` to a chat.
fn times_said(chat: &Chat<'_>, text: &str) -> usize {
    words_in(chat).iter().filter(|w| *w == text).count()
}

// A send the core had not tried when it stopped cannot have gone out: it is sent when the core
// starts again, not looked for, so an older message just like it is never taken for it (S1, S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_never_tried_is_sent_and_not_looked_for() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;

    let group = GroupKey(chat.hex.clone());
    let text = "the same words";
    world
        .marmot
        .send(&group, &said_before(text), "x")
        .await
        .unwrap();
    // A second send of the same event in the same second would be the same event.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (store, kept, mut talk) = taken_up(&world);
    talk.emitted(words_done(text, &id, Vec::new()));
    let read = kept.read;
    let conversation = talk.saved().clone();
    store.save(&Kept { conversation, read }).unwrap();

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    within(20, || (times_said(&chat, text) == 2).then_some(())).await;
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send the core tried, and that may have gone out, is looked for by all it carries: an older
// message with the same words and turn that settles nothing is not taken for words that settle a
// request, which are sent, and settle it (S1, S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_tried_is_not_taken_for_an_older_one_like_it() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    events
        .send(Emitted {
            event: event("t", State::Running, turn()),
            chat: Some(id.clone()),
            prompts: Vec::new(),
            settles: Vec::new(),
        })
        .unwrap();
    let (target, read) = within(30, || {
        let items = chat.latest().into_iter();
        items
            .into_iter()
            .find(|(_, r)| r.item.event.header().item.as_str() == "t")
    })
    .await;
    let target = first(&target, &read);
    let request = chat.ask(Some(&target), &Asked::Interrupt, "Stop").await;
    let store = Store::new(world.state.path());
    let pending = |kept: Kept| {
        let state = json(&kept);
        let pending = state["conversation"]["pending"].as_object().cloned();
        pending.and_then(|p| p.keys().next().cloned())
    };
    let taken = within(20, || pending(store.load().unwrap())).await;
    core.crash().await;

    let group = GroupKey(chat.hex.clone());
    let text = "the same words";
    world
        .marmot
        .send(&group, &said_before(text), "x")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (store, kept, mut talk) = taken_up(&world);
    let request_id = marmot_agent_driver::RequestId(taken);
    talk.emitted(words_done(text, &id, vec![request_id]));
    let (token, what) = talk.next_send(&group).unwrap();
    assert!(matches!(what, Sending::Words { .. }));
    // Tried just before the crash, and it did not go out.
    talk.attempting(&token);
    let read = kept.read;
    let conversation = talk.saved().clone();
    store.save(&Kept { conversation, read }).unwrap();

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    within(20, || (times_said(&chat, text) == 2).then_some(())).await;
    let settling = sent_by_core(&chat, 9).into_iter().filter(|m| {
        (m.tags.iter()).any(|t| t.len() >= 2 && t[0] == "settles" && t[1] == request.as_str())
    });
    assert_eq!(settling.count(), 1, "the words that settle went out once");
    core.stop().await;
    world.runtime.shutdown().await;
}

// One chat's backlog does not hold up another: a chat sends a few events before the core turns
// to the next, however many one piece of work queues for it, as when a start closes many open
// items at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chats_backlog_does_not_hold_up_another() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    let team = world
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
    let team_hex = world.joined(&team).await;
    core.stop().await;

    // As the core left it: twenty items open in the direct chat, then one in the team's.
    let (store, kept, mut talk) = taken_up(&world);
    let place = Where {
        group: GroupKey(team_hex.clone()),
        name: None,
        direct: false,
        members: 2,
    };
    let Some(marmot_agent::Act::Prompt(prompt)) = talk
        .heard(Heard {
            chat: place,
            event: EventKey::from("e-team"),
            author: PubKey(world.person.clone()),
            author_name: "Person".into(),
            text: "hi".into(),
            reply_to: None,
            mentions_agent: true,
            attachments: Vec::new(),
        })
        .into_iter()
        .next()
    else {
        panic!("no prompt")
    };
    let team_id = prompt.chat.id;
    assert!(
        format!("{}/", id.0) < format!("{}/", team_id.0),
        "closed last"
    );
    let mut open = |chat: &ChatId, item: &str, n: usize| {
        talk.emitted(Emitted {
            event: event(item, State::Running, turn()),
            chat: Some(chat.clone()),
            prompts: Vec::new(),
            settles: Vec::new(),
        });
        let first = EventKey::from(format!("{n:064x}").as_str());
        talk.sent(&format!("{}/{item}", chat.0), first);
    };
    for n in 0..20 {
        open(&id, &format!("a{n:02}"), n);
    }
    open(&team_id, "b", 99);
    let read = kept.read;
    let conversation = talk.saved().clone();
    store.save(&Kept { conversation, read }).unwrap();

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    let other = Chat {
        world: &world,
        group: team,
        hex: team_hex,
        turn: 30,
    };
    let closes = |chat: &Chat<'_>| -> Vec<i64> {
        (sent_by_core(chat, marmot_agent::carrier::ITEM_KIND).into_iter())
            .filter(|m| {
                let read = marmot_agent::carrier::read_item(&m.plaintext, &m.tags);
                read.is_some_and(|r| r.item.event.header().state == State::Cancelled)
            })
            .map(|m| m.insert_order)
            .collect()
    };
    within(30, || {
        (closes(&chat).len() == 20 && closes(&other).len() == 1).then_some(())
    })
    .await;
    let last_direct = closes(&chat).into_iter().max().unwrap();
    assert!(
        closes(&other)[0] < last_direct,
        "the team's close went before the direct chat's backlog was done"
    );
    core.stop().await;
    world.runtime.shutdown().await;
}

impl Stub {
    fn caps() -> Capabilities {
        stub(false).0.capabilities()
    }
}
