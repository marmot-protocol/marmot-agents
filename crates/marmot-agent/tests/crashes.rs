//! The core across a crash, a failed save and a passing failure of MDK, end to end through MDK:
//! what reaches the driver, what is sent, and what is settled once the core starts again (S1, S3,
//! S4, S10, L3).

mod common;

use common::{Chat, World, first, within};
use marmot_agent::carrier::Asked;
use marmot_agent::marmot::{self, Fault, Subscribed, now};
use marmot_agent::run::{Core, Kept, Store};
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
            subagents: false,
            stop_subagent: false,
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
        subagent: None,
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

/// Seconds since 1970, as the core keeps when a send was first tried.
fn unix_now() -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    now.unwrap().as_secs()
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
    let first = send_queued(&world, &mut talk, &GroupKey(chat.hex.clone())).await;
    talk.emitted(emitted(State::Done));
    store
        .save(&Kept {
            conversation: talk.saved().clone(),
            read: kept.read,
            history: kept.history,
        })
        .unwrap();

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    let read = within(20, || {
        let items = chat.items().into_iter().map(|(_, r)| r);
        items
            .into_iter()
            .find(|r| r.item.event.header().item.as_str() == "t" && r.item.version > 1)
    })
    .await;
    assert_eq!(read.item.version, 2);
    assert_eq!(read.item.first, first);
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

// A message MDK cannot place for a moment, as when its group's state cannot be read, is not skipped:
// it is read again once the failure passes, and reaches the driver once, before the message after
// it (S10, L3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_that_cannot_be_read_for_a_moment_is_read_after() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, _) = a_chat(&world, &seen).await;
    world.marmot.faults.push(Fault::PlaceFails);
    chat.say("second").await;
    chat.say("third").await;
    let third = |seen: &Seen| seen.prompts.iter().any(|p| p == "third");
    within(30, || third(&seen.lock().unwrap()).then_some(())).await;
    // A message read again, and anything read twice, would come by now.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let prompts = seen.lock().unwrap().prompts.clone();
    assert_eq!(prompts, ["hello", "second", "third"]);
    core.stop().await;
    world.runtime.shutdown().await;
}

// A group the core has never seen, as on an imported account's first run, starts after what
// arrived before the core subscribed. When MDK cannot give what it stored for a while, a message
// that arrives meanwhile is still read, once, and what was there before is not (L3), even when
// MDK announces a message that arrived before the core subscribed only after: it stores a sync's
// messages and announces them at the end of the sync, so a core that subscribes in between hears
// one live.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_that_arrives_while_a_new_group_cannot_be_read_is_read() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, _) = a_chat(&world, &seen).await;
    core.stop().await;
    // Listening before MDK announces the message, so the core hears it live when it starts,
    // though it arrived before the core subscribed.
    let events = world.runtime.subscribe();
    chat.say("while away").await;
    let group = GroupKey(chat.hex.clone());
    let stored = || (world.marmot.stored(&group, 0, 1000).ok()).filter(|p| p.messages.len() == 2);
    within(30, || stored().map(|_| ())).await;
    let live = Subscribed {
        events,
        since: now(),
    };
    // The core has never seen the group, and MDK cannot give what it stored for about six seconds.
    let store = Store::new(world.state.path());
    let mut kept = store.load().unwrap();
    kept.read.remove(&group);
    store.save(&kept).unwrap();
    for _ in 0..6 {
        world.marmot.faults.push(Fault::StoredFails);
    }
    let (driver, _events, seen) = stub(false);
    let since = live.since;
    let core = world.start_on(driver, live);
    // MDK gives the time a message arrived in whole seconds, so only one that arrived in a later
    // second than the core subscribed is known to have come after it.
    within(5, || (now() >= since + 2).then_some(())).await;
    chat.say("new").await;
    within(30, || seen.lock().unwrap().prompts.first().cloned()).await;
    // A message read twice, or history read as a prompt, would come by now.
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert_eq!(seen.lock().unwrap().prompts, ["new"]);
    core.stop().await;
    world.runtime.shutdown().await;
}

// A prompt that arrives in a group the core has no mark for, while the harness is still starting,
// is read: the core starts when it subscribes to MDK, before it starts the harness, not once the
// harness is up (L3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_that_arrives_while_the_harness_starts_is_read() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, _) = a_chat(&world, &seen).await;
    core.stop().await;
    let group = GroupKey(chat.hex.clone());
    let store = Store::new(world.state.path());
    let mut kept = store.load().unwrap();
    kept.read.remove(&group);
    store.save(&kept).unwrap();
    let live = marmot::subscribe(&world.runtime);
    let subscribed = live.since;
    // The harness is slow to start. MDK gives the time a message arrived in whole seconds, so
    // the prompt comes in a later second than the core subscribed.
    within(5, || (now() >= subscribed + 2).then_some(())).await;
    chat.say("new").await;
    let stored = || (world.marmot.stored(&group, 0, 1000).ok()).filter(|p| p.messages.len() == 2);
    within(30, || stored().map(|_| ())).await;
    within(5, || (now() >= subscribed + 4).then_some(())).await;
    let (driver, _events, seen) = stub(false);
    let core = world.start_on(driver, live);
    within(30, || seen.lock().unwrap().prompts.first().cloned()).await;
    // A message read twice, or history read as a prompt, would come by now.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(seen.lock().unwrap().prompts, ["new"]);
    core.stop().await;
    world.runtime.shutdown().await;
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

/// The tool `k` in its final state, with a native message, which only a chat sent the agent's
/// full activity is sent (D-136).
fn a_tool() -> AgentEvent {
    let native = marmot_agent_events::Native {
        kind: "item/completed".into(),
        payload: serde_json::value::RawValue::from_string(r#"{"command":"ls"}"#.into()).unwrap(),
    };
    let header = event("k", State::Done, turn()).header().clone();
    let tool: Body =
        serde_json::from_str(r#"{"type":"tool","body":{"kind":"command","name":"shell"}}"#)
            .expect("a tool");
    AgentEvent::new(header, tool, Some(native)).unwrap()
}

/// A third person is added to the owner's two-person chat, and MDK says so.
async fn someone_joins(world: &World, chat: &Chat<'_>) {
    let third = world.someone_else().await;
    world
        .runtime
        .invite_members(&world.person, &chat.group, std::slice::from_ref(&third))
        .await
        .unwrap();
    let group = GroupKey(chat.hex.clone());
    within(15, || {
        matches!(world.marmot.two_people(&group), Ok(None)).then_some(())
    })
    .await;
}

/// The state the core left, as a conversation to add to, which knows the chat is the owner's.
fn conversation_of(world: &World, kept: &Kept, group: &GroupKey) -> Conversation {
    let mut talk = Conversation::new(
        PubKey(world.agent.clone()),
        "stub",
        world.config.rights(),
        Stub::caps(),
        kept.conversation.clone(),
    );
    let owners = [PubKey(world.agent.clone()), PubKey(world.person.clone())];
    talk.members_now(group, Some(&owners));
    talk
}

/// Saves `talk` in place of the state's conversation.
fn save_conversation(store: &Store, kept: Kept, talk: &Conversation) {
    store
        .save(&Kept {
            conversation: talk.saved().clone(),
            ..kept
        })
        .unwrap();
}

// A send MDK was handed while the chat was the owner's, and still keeps, is not handed to MDK again
// after someone joins the chat: its outcome is learnt by asking MDK only. Handed again, it could go
// out as new, to the chat as it is now, were MDK to let go of its token in between (#194); and
// tried again as the chat is sent now, without its native message, MDK would refuse its token as
// bound to another submission, and a notice would settle again what MDK still posted (D-136, H1 of
// ActivityHoldBack.tla). It goes on to the core's record as MDK was handed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_mdk_keeps_is_not_handed_to_it_again_after_someone_joins() {
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
    let state = || json(&store.load().unwrap());
    let taken = within(20, || {
        let pending = state()["conversation"]["pending"].as_object().cloned();
        pending.and_then(|p| p.keys().next().cloned())
    })
    .await;

    // The tool, done, with its native message, settling the request: MDK takes it, and keeps it
    // queued while someone is added to the chat. Then MDK's engine has it, as for a send whose
    // publish failed: the core goes on with the event MDK names, which it kept.
    world.marmot.faults.push(Fault::Owned {
        queued: Duration::from_secs(20),
        owned: Duration::from_secs(3600),
    });
    events
        .send(Emitted {
            event: a_tool(),
            chat: Some(id.clone()),
            prompts: Vec::new(),
            settles: vec![marmot_agent_driver::RequestId(taken)],
        })
        .unwrap();
    // The tool's event, as MDK named it when it took it, saved before someone joins.
    let tried = |state: serde_json::Value| {
        (state["conversation"]["outbox"].as_array()).is_some_and(|queue| {
            (queue.iter()).any(|q| q["event"].is_string() && q["what"]["Item"].is_object())
        })
    };
    within(20, || tried(state()).then_some(())).await;
    someone_joins(&world, &chat).await;
    // Were the tool handed to MDK again, this would meet it, and it would be given up.
    world.marmot.faults.push(Fault::Refuse(
        marmot_app::AppError::InvalidAppMessagePayload("handed again".into()),
    ));

    // The core takes the event MDK named when it took it, handing nothing again.
    let queued = || (state()["conversation"]["outbox"].as_array()).map(Vec::len);
    within(40, || (queued() == Some(0)).then_some(())).await;
    within(10, || (!versions(&chat, "k").is_empty()).then_some(())).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let settling: Vec<AgentEvent> = (chat.items().into_iter())
        .filter(|(_, r)| r.item.settles.contains(&request))
        .map(|(_, r)| r.item.event)
        .collect();
    assert_eq!(settling.len(), 1, "settled once");
    assert_eq!(settling[0].header().item.as_str(), "k", "by the tool");
    assert!(settling[0].native().is_some(), "as MDK was handed it");
    assert!(
        (chat.items().iter()).all(|(_, r)| refused(&r.item.event).is_none()),
        "nothing was given up"
    );
    assert!(!world.marmot.faults.is_empty(), "not handed to MDK again");
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send of the agent's full activity that MDK was handed while the chat was the owner's, so long
// ago that the group's messages have disappeared since, and whose token MDK no longer knows, may
// have gone out. After someone joins the chat, the chat is told so, as D-109 says: it is not taken
// for a send MDK never took and held back (#182).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_mdk_no_longer_knows_is_told_as_not_known_after_someone_joins() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;

    // As the core left it, had it handed the tool to MDK two minutes ago and stopped before it
    // learnt what became of it. MDK has no record of it now.
    let group = GroupKey(chat.hex.clone());
    let store = Store::new(world.state.path());
    let kept = store.load().unwrap();
    let mut talk = conversation_of(&world, &kept, &group);
    talk.emitted(Emitted {
        event: a_tool(),
        chat: Some(id),
        prompts: Vec::new(),
        settles: Vec::new(),
    });
    let (token, _) = talk.next_send(&group).unwrap();
    talk.attempting(&token.send, unix_now() - 120);
    save_conversation(&store, kept, &talk);

    // The group's messages last a minute, and someone joins.
    (world.runtime)
        .update_message_retention(&world.person, &chat.group, 60)
        .await
        .unwrap();
    let marmot = &world.marmot;
    within(30, || {
        let record = marmot.app.group(&marmot.label, &chat.hex).ok().flatten()?;
        (record.message_retention.disappearing_message_secs == 60).then_some(())
    })
    .await;
    someone_joins(&world, &chat).await;

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    let told = within(30, || {
        (chat.items().into_iter())
            .map(|(_, r)| r.item.event)
            .find(|e| matches!(e.body(), Body::Notice(_)))
    })
    .await;
    assert!(
        told.header().text.contains("may not have been delivered"),
        "told as not known: {}",
        told.header().text
    );
    world.nothing_queued().await;
    assert!(versions(&chat, "k").is_empty(), "the tool is not sent");
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send of the agent's full activity whose event the core saved when MDK took it, and whose token
// MDK no longer knows, was taken by MDK and let go of since, as under a disappearing time that was
// lengthened after: it may have gone out. After someone joins the chat, the chat is told so, as
// D-109 says, however long the group's messages last now: it is not taken for a send MDK never
// took and held back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_whose_event_was_saved_is_told_as_not_known_when_mdk_no_longer_knows_it() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;

    // As the core left it, had MDK taken the tool a moment ago and named its event, which the core
    // saved, and let go of it since. MDK has no record of it now.
    let group = GroupKey(chat.hex.clone());
    let store = Store::new(world.state.path());
    let kept = store.load().unwrap();
    let mut talk = conversation_of(&world, &kept, &group);
    talk.emitted(Emitted {
        event: a_tool(),
        chat: Some(id),
        prompts: Vec::new(),
        settles: Vec::new(),
    });
    let (token, _) = talk.next_send(&group).unwrap();
    talk.attempting(&token.send, unix_now());
    talk.took(&token.send, EventKey("ab".repeat(32)));
    save_conversation(&store, kept, &talk);
    someone_joins(&world, &chat).await;

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    let told = within(30, || {
        (chat.items().into_iter())
            .map(|(_, r)| r.item.event)
            .find(|e| matches!(e.body(), Body::Notice(_)))
    })
    .await;
    assert!(
        told.header().text.contains("may not have been delivered"),
        "told as not known: {}",
        told.header().text
    );
    world.nothing_queued().await;
    assert!(versions(&chat, "k").is_empty(), "the tool is not sent");
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send an older core queued and handed to MDK, which noted no form for it (`bare`), was handed
// over in full: the older core sent the native message by who was in the chat at each try. After
// someone joins the chat, before the newer core starts, it is not tried again changed, without
// its native message, which MDK would refuse as bound to another submission, giving it up with a
// notice: MDK is asked what became of it, and it goes on as MDK took it (#181).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_an_older_core_handed_over_goes_on_as_mdk_took_it_after_someone_joins() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;

    // As an older core left it: the turn's first version, with its native message, handed to and
    // taken by MDK in the owner's chat, and the core stopped before it learnt so.
    let group = GroupKey(chat.hex.clone());
    let store = Store::new(world.state.path());
    let kept = store.load().unwrap();
    let mut talk = conversation_of(&world, &kept, &group);
    let native = marmot_agent_events::Native {
        kind: "turn/started".into(),
        payload: serde_json::value::RawValue::from_string(r#"{"id":"t1"}"#.into()).unwrap(),
    };
    let running = event("t", State::Running, turn());
    let running = AgentEvent::new(running.header().clone(), turn(), Some(native)).unwrap();
    talk.emitted(Emitted {
        event: running,
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    });
    let (token, what) = talk.next_send(&group).unwrap();
    talk.attempting(&token.send, unix_now());
    let went = world.send(&group, &what, &token).await.unwrap();
    let mut old = json(&Kept {
        conversation: talk.saved().clone(),
        ..Kept::default()
    });
    for queued in old["conversation"]["outbox"].as_array_mut().unwrap() {
        queued.as_object_mut().unwrap().remove("bare");
    }
    let old: Kept = serde_json::from_value(old).unwrap();
    save_conversation(
        &store,
        kept,
        &Conversation::new(
            PubKey(world.agent.clone()),
            "stub",
            world.config.rights(),
            Stub::caps(),
            old.conversation,
        ),
    );
    someone_joins(&world, &chat).await;

    let (driver, events, _) = stub(false);
    let core = world.start(driver);
    world.nothing_queued().await;
    events
        .send(Emitted {
            event: event("t", State::Done, turn()),
            chat: Some(id),
            prompts: Vec::new(),
            settles: Vec::new(),
        })
        .unwrap();
    within(30, || {
        versions(&chat, "t").iter().find(|v| v.1 == 2).cloned()
    })
    .await;
    let versions = versions(&chat, "t");
    let firsts: Vec<&EventKey> = (versions.iter())
        .filter(|v| v.1 == 1)
        .map(|v| &v.0)
        .collect();
    assert_eq!(
        firsts,
        vec![&went],
        "the first version went out once, as MDK took it"
    );
    let second = versions.iter().find(|v| v.1 == 2).unwrap();
    assert_eq!(second.2.as_ref(), Some(&went), "and the next names it");
    assert!(
        (chat.items().iter()).all(|(_, r)| refused(&r.item.event).is_none()),
        "nothing was given up"
    );
    core.stop().await;
    world.runtime.shutdown().await;
}

/// A tool MDK was handed while the chat was the owner's, whose answer the core lost to a stop, and
/// which MDK's engine has for good, in a group whose messages last `keep` seconds (or for ever, for
/// 0), which someone joined since: the core is started on that state, and the outbox empties, the
/// tool having gone out once, as MDK took it, and nothing handed to MDK again. If `queued`, MDK
/// keeps it queued first, until the core has saved the event MDK names. What the chat was told.
async fn engine_has_it(keep: u64, queued: bool) -> Vec<String> {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;
    if keep > 0 {
        (world.runtime)
            .update_message_retention(&world.person, &chat.group, keep)
            .await
            .unwrap();
        let marmot = &world.marmot;
        within(30, || {
            let record = marmot.app.group(&marmot.label, &chat.hex).ok().flatten()?;
            (record.message_retention.disappearing_message_secs == keep).then_some(())
        })
        .await;
    }
    someone_joins(&world, &chat).await;

    let group = GroupKey(chat.hex.clone());
    let store = Store::new(world.state.path());
    let kept = store.load().unwrap();
    let mut talk = conversation_of(&world, &kept, &group);
    talk.emitted(Emitted {
        event: a_tool(),
        chat: Some(id),
        prompts: Vec::new(),
        settles: Vec::new(),
    });
    let (token, what) = talk.next_send(&group).unwrap();
    talk.attempting(&token.send, unix_now());
    world.marmot.faults.push(Fault::Owned {
        queued: if queued {
            Duration::from_secs(3600)
        } else {
            Duration::ZERO
        },
        owned: Duration::from_secs(3600),
    });
    let went = match world.marmot.send(&group, &what, &token).await {
        Ok(event) | Err(marmot_agent::marmot::SendFailed::Pending(event)) => event,
        Err(failed) => panic!("MDK took it: {failed:?}"),
    };
    save_conversation(&store, kept, &talk);
    // Were the tool handed to MDK again, this would meet it, and it would be given up.
    world.marmot.faults.push(Fault::Refuse(
        marmot_app::AppError::InvalidAppMessagePayload("handed again".into()),
    ));

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    if queued {
        within(10, || {
            let state = json(&store.load().unwrap());
            let queue = state["conversation"]["outbox"].as_array().cloned()?;
            (queue.len() == 1 && queue[0]["event"].as_str() == Some(went.as_str())).then_some(())
        })
        .await;
        world.marmot.faults.engine_takes(&token.mdk);
    }
    within(20, || {
        let state = json(&store.load().unwrap());
        (state["conversation"]["outbox"].as_array())
            .is_some_and(|q| q.is_empty())
            .then_some(())
    })
    .await;
    // The person's side receives the post a moment later.
    within(10, || (!versions(&chat, "k").is_empty()).then_some(())).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let told: Vec<String> = (chat.items().into_iter())
        .filter(|(_, r)| matches!(r.item.event.body(), Body::Notice(_)))
        .map(|(_, r)| r.item.event.header().text.clone())
        .collect();
    let firsts: Vec<EventKey> = (versions(&chat, "k").into_iter()).map(|v| v.0).collect();
    assert_eq!(firsts, vec![went], "the tool went out once, as MDK took it");
    assert!(!world.marmot.faults.is_empty(), "not handed to MDK again");
    core.stop().await;
    world.runtime.shutdown().await;
    told
}

// A send of the agent's full activity that MDK was handed while the chat was the owner's, whose
// answer the core lost, and which MDK's engine keeps for good, goes on with the event MDK names
// (mdk#3712), handed to MDK no second time, and is not told as one that may not have been
// delivered, which would settle again what it settled (S1, H1). Before MDK named it, this held up
// the chat for good in a group whose messages disappear within a minute (D-152's known limit).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_mdks_engine_keeps_goes_on_with_the_event_mdk_names() {
    let told = engine_has_it(60, false).await;
    assert!(told.is_empty(), "nothing is told of it: {told:?}");
}

// The same in a group whose messages never disappear, where the core used to hand the send to MDK
// again to learn its event (D-152's one path left, `hand_again`), here first kept queued: the core
// saves the event MDK names meanwhile, so the send is known to have been taken should MDK later
// let go of its token (#203).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_mdks_engine_keeps_is_never_handed_again() {
    let told = engine_has_it(0, true).await;
    assert!(told.is_empty(), "nothing is told of it: {told:?}");
}

// A send an older core handed to MDK without its native message, in a chat that was not the
// owner's, and which noted no form for it (`bare`), is not handed to MDK again in full while MDK's
// engine has it: MDK would refuse the token as bound to another submission, the core would give the
// send up, and what stands in for it could settle again what MDK still posts. It goes on with the
// event MDK names, though no one joined or left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_of_unknown_form_is_never_handed_again() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;
    someone_joins(&world, &chat).await;

    // As an older core left it: the turn's first version handed to and taken by MDK without its
    // native message, as the chat of three is sent, and the core stopped before it learnt so.
    let group = GroupKey(chat.hex.clone());
    let store = Store::new(world.state.path());
    let kept = store.load().unwrap();
    let mut talk = Conversation::new(
        PubKey(world.agent.clone()),
        "stub",
        world.config.rights(),
        Stub::caps(),
        kept.conversation.clone(),
    );
    talk.members_now(&group, None);
    let native = marmot_agent_events::Native {
        kind: "turn/started".into(),
        payload: serde_json::value::RawValue::from_string(r#"{"id":"t1"}"#.into()).unwrap(),
    };
    let running = event("t", State::Running, turn());
    let running = AgentEvent::new(running.header().clone(), turn(), Some(native)).unwrap();
    talk.emitted(Emitted {
        event: running,
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    });
    let (token, what) = talk.next_send(&group).unwrap();
    talk.attempting(&token.send, unix_now());
    world.marmot.faults.push(Fault::Owned {
        queued: Duration::ZERO,
        owned: Duration::from_secs(3600),
    });
    let went = world.send(&group, &what, &token).await.unwrap();
    let mut old = json(&Kept {
        conversation: talk.saved().clone(),
        ..Kept::default()
    });
    for queued in old["conversation"]["outbox"].as_array_mut().unwrap() {
        queued.as_object_mut().unwrap().remove("bare");
    }
    let old: Kept = serde_json::from_value(old).unwrap();
    save_conversation(
        &store,
        kept,
        &Conversation::new(
            PubKey(world.agent.clone()),
            "stub",
            world.config.rights(),
            Stub::caps(),
            old.conversation,
        ),
    );

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    within(20, || {
        let state = json(&store.load().unwrap());
        (state["conversation"]["outbox"].as_array())
            .is_some_and(|q| q.is_empty())
            .then_some(())
    })
    .await;
    // The person's side receives the post a moment later.
    within(10, || (!versions(&chat, "t").is_empty()).then_some(())).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        (chat.items().iter()).all(|(_, r)| refused(&r.item.event).is_none()),
        "nothing was given up"
    );
    let firsts: Vec<EventKey> = (versions(&chat, "t").into_iter())
        .filter(|v| v.1 == 1)
        .map(|v| v.0)
        .collect();
    assert_eq!(
        firsts,
        vec![went],
        "the first version went out once, as MDK took it"
    );
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send that went out just before a crash, before the core marked it sent, is tried again when
// the core starts, under the token it went under: MDK gives back the event it made, sends nothing,
// and the send is not made a second time (S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_that_went_out_before_a_crash_is_not_sent_again() {
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
    talk.attempting(&token.send, unix_now());
    let went = world.send(&group, &what, &token).await.unwrap();
    // A second send of the same event in the same second would be the same event, so a later one.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    store
        .save(&Kept {
            conversation: talk.saved().clone(),
            read: kept.read,
            history: kept.history,
        })
        .unwrap();

    // Starting again closes the open turn: its second version names the first that went out.
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
    let failed = (chat.items().into_iter()).filter(|(_, r)| refused(&r.item.event).is_some());
    assert_eq!(failed.count(), 0, "nothing said to have failed");
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send MDK took and dropped before it went out sent nothing: it is tried again under a new
// token, goes out once, and the chat is not told it failed (S1, S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_mdk_drops_is_tried_again_under_a_new_token() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    world.marmot.faults.push(Fault::Reject);
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
    use marmot_agent::carrier::{EDIT_KIND, ITEM_KIND};
    // An item's later versions are MDK's edits of its first event.
    let kinds = if kind == ITEM_KIND {
        vec![ITEM_KIND, EDIT_KIND]
    } else {
        vec![kind]
    };
    let query = marmot_app::AppMessageQuery {
        group_id_hex: Some(chat.hex.clone()),
        kinds: Some(kinds),
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
        .filter_map(|m| marmot_agent::carrier::read_item(m.kind, &m.plaintext, &m.tags))
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
        thread: None,
    }
}

/// The token the words [`said_before`] went under: another send's.
fn earlier_token() -> marmot_agent::Token {
    marmot_agent::Token {
        send: "earlier".into(),
        mdk: "earlier".into(),
        edits: None,
        tried_at: None,
    }
}

/// How many times the core sent the agent's words `text` to a chat.
fn times_said(chat: &Chat<'_>, text: &str) -> usize {
    words_in(chat).iter().filter(|w| *w == text).count()
}

// A send the core had not tried when it stopped is sent when the core starts again, under its own
// token, so an older message just like it is never taken for it (S1, S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_never_tried_is_sent_and_not_taken_for_an_older_one() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;

    let group = GroupKey(chat.hex.clone());
    let text = "the same words";
    world
        .send(&group, &said_before(text), &earlier_token())
        .await
        .unwrap();
    // A second send of the same event in the same second would be the same event.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (store, kept, mut talk) = taken_up(&world);
    talk.emitted(words_done(text, &id, Vec::new()));
    let read = kept.read;
    let conversation = talk.saved().clone();
    store
        .save(&Kept {
            conversation,
            read,
            ..Kept::default()
        })
        .unwrap();

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    within(20, || (times_said(&chat, text) == 2).then_some(())).await;
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send the core tried, and that may have gone out, is tried again under its own token: an older
// message with the same words and turn, under another, is not taken for words that settle a
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
        .send(&group, &said_before(text), &earlier_token())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (store, kept, mut talk) = taken_up(&world);
    let request_id = marmot_agent_driver::RequestId(taken);
    talk.emitted(words_done(text, &id, vec![request_id]));
    let (token, what) = talk.next_send(&group).unwrap();
    assert!(matches!(what, Sending::Words { .. }));
    // Tried just before the crash, and it did not go out.
    talk.attempting(&token.send, unix_now());
    let read = kept.read;
    let conversation = talk.saved().clone();
    store
        .save(&Kept {
            conversation,
            read,
            ..Kept::default()
        })
        .unwrap();

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

// An edit of the agent's words that went out just before a crash, before the core saved that it
// went, is not sent again when the core starts: MDK keeps the token the send was tried under, and
// a send tried again under it gives back the event it made and sends nothing (S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_that_went_out_before_a_crash_is_not_sent_again() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    events
        .send(words_done("first words", &id, Vec::new()))
        .unwrap();
    within(20, || (times_said(&chat, "first words") == 1).then_some(())).await;
    core.stop().await;

    // As the core left it, had it crashed after MDK sent the edit and before it saved that it
    // went.
    let group = GroupKey(chat.hex.clone());
    let (store, kept, mut talk) = taken_up(&world);
    talk.emitted(words_done("second words", &id, Vec::new()));
    let (token, what) = talk.next_send(&group).unwrap();
    assert!(matches!(what, Sending::Words { edit: Some(_), .. }));
    talk.attempting(&token.send, unix_now());
    world.send(&group, &what, &token).await.unwrap();
    // The same edit made again in the same second would be the same event, so a later one.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let read = kept.read;
    let conversation = talk.saved().clone();
    store
        .save(&Kept {
            conversation,
            read,
            ..Kept::default()
        })
        .unwrap();

    let (driver, _events, _) = stub(false);
    let core = world.start(driver);
    let sent = |kept: Kept| (json(&kept)["conversation"]["outbox"].as_array()).map(Vec::len);
    within(20, || {
        (sent(store.load().unwrap()) == Some(0)).then_some(())
    })
    .await;
    // Edits of the agent's words, not the later versions of items, which are edits too.
    let edits = (sent_by_core(&chat, marmot_agent::carrier::EDIT_KIND).into_iter())
        .filter(|m| marmot_agent::carrier::read_item(m.kind, &m.plaintext, &m.tags).is_none());
    assert_eq!(edits.count(), 1, "the edit went out once");
    core.stop().await;
    world.runtime.shutdown().await;
}

// A state file that has sends handed to MDK but no salt was written by a core from before MDK
// kept tokens, which sent without one: under a token now, such a send could go out a second time.
// No such core was released, so the core does not look those sends up: it refuses to start, and
// says why (S3).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_state_with_sends_tried_before_tokens_is_refused() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;
    let group = GroupKey(chat.hex.clone());
    let (store, kept, mut talk) = taken_up(&world);
    talk.emitted(words_done("the words", &id, Vec::new()));
    let (token, _) = talk.next_send(&group).unwrap();
    talk.attempting(&token.send, unix_now());
    let read = kept.read;
    let conversation = talk.saved().clone();
    store
        .save(&Kept {
            conversation,
            read,
            ..Kept::default()
        })
        .unwrap();
    // As an older core left it: no salt.
    let mut file = json(&store.load().unwrap());
    file["conversation"].as_object_mut().unwrap().remove("salt");
    let path = world.state.path().join("marmot-agent-state.json");
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

    let (driver, _events, _) = stub(false);
    let started = Core::new(world.marmot.clone(), driver, &world.config, store);
    let Err(why) = started else {
        panic!("the core starts on a state from before tokens");
    };
    assert!(why.contains("older"), "says why: {why}");
    world.runtime.shutdown().await;
}

// A state file from before kind 1201 with an item handed to MDK: MDK keeps its token bound to the
// old event, so tried again in the new shape it is refused, and the chat would be told of a
// failure that may not have been one. No such core was released, so the core refuses to start,
// and says why. An item queued and never handed to MDK is taken up (D-112).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_state_with_an_item_tried_before_kind_1201_is_refused() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    core.stop().await;
    let group = GroupKey(chat.hex.clone());
    let path = world.state.path().join("marmot-agent-state.json");
    let before_1201 = |attempted: bool| {
        let (store, kept, mut talk) = taken_up(&world);
        talk.emitted(Emitted {
            event: event("t", State::Done, turn()),
            chat: Some(id.clone()),
            prompts: Vec::new(),
            settles: Vec::new(),
        });
        let (token, _) = talk.next_send(&group).unwrap();
        if attempted {
            talk.attempting(&token.send, unix_now());
        }
        let read = kept.read;
        let conversation = talk.saved().clone();
        store
            .save(&Kept {
                conversation,
                read,
                ..Kept::default()
            })
            .unwrap();
        let mut file = json(&store.load().unwrap());
        file["conversation"]
            .as_object_mut()
            .unwrap()
            .remove("kind_1201");
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        store
    };

    let store = before_1201(false);
    let (driver, _events, _) = stub(false);
    let started = Core::new(world.marmot.clone(), driver, &world.config, store);
    assert!(started.is_ok(), "an item never tried is taken up");

    let store = before_1201(true);
    let (driver, _events, _) = stub(false);
    let started = Core::new(world.marmot.clone(), driver, &world.config, store);
    let Err(why) = started else {
        panic!("the core starts on an item tried before kind 1201");
    };
    assert!(why.contains("kind 1201"), "says why: {why}");
    world.runtime.shutdown().await;
}

// In a chat whose messages disappear, an item's first event can be gone while the item goes on:
// its next version goes out as a new first event, of kind 1201, and the chat is not told of a
// failure, as nothing failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_item_whose_first_event_disappeared_goes_on_as_a_new_one() {
    use marmot_agent::carrier::{ITEM_KIND, read_item};
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    (world.runtime)
        .update_message_retention(&world.person, &chat.group, 60)
        .await
        .unwrap();
    let marmot = &world.marmot;
    within(30, || {
        let record = marmot.app.group(&marmot.label, &chat.hex).ok().flatten()?;
        (record.message_retention.disappearing_message_secs == 60).then_some(())
    })
    .await;
    let emit = |state: State| Emitted {
        event: event("t", state, turn()),
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    };
    let firsts = || -> Vec<(String, u64)> {
        (sent_by_core(&chat, ITEM_KIND).into_iter())
            .filter(|m| m.kind == ITEM_KIND)
            .filter_map(|m| {
                let read = read_item(m.kind, &m.plaintext, &m.tags)?;
                let header = read.item.event.header();
                (header.item.as_str() == "t").then_some((m.message_id_hex, read.item.version))
            })
            .collect()
    };
    events.send(emit(State::Running)).unwrap();
    let first = within(20, || firsts().into_iter().next()).await;
    // Only once MDK is done with it: a send MDK still has queued could go out after the sweep.
    world.nothing_queued().await;
    // The first event's time comes: MDK's sweep takes the clock it is given.
    let later = (unix_now() + 3600) * 1000;
    let mut gone = false;
    for _ in 0..100 {
        (marmot.runtime)
            .sweep_expired_retention(&marmot.label, later)
            .await
            .unwrap();
        gone = (marmot.runtime)
            .timeline_message(&marmot.label, &chat.hex, &first.0)
            .unwrap()
            .is_none();
        if gone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(gone, "the first event disappeared");

    events.send(emit(State::Done)).unwrap();
    let anew = within(20, || {
        (firsts().into_iter()).find(|(_, version)| *version == 2)
    })
    .await;
    assert_ne!(anew.0, first.0, "a new first event");
    let failed = (sent_by_core(&chat, ITEM_KIND).into_iter())
        .filter_map(|m| read_item(m.kind, &m.plaintext, &m.tags))
        .filter(|r| refused(&r.item.event).is_some())
        .count();
    assert_eq!(failed, 0, "nothing failed");
    core.stop().await;
    world.runtime.shutdown().await;
}

// A send MDK keeps queued, and has not yet taken into its engine, does not hold up the core: the
// core asks after it again later, and meanwhile reads what the chat says and hands it on. Once
// MDK takes the send, it goes out once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_mdk_keeps_queued_holds_up_nothing_else() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let core = world.start(driver);
    let (chat, id) = a_chat(&world, &seen).await;
    world
        .marmot
        .faults
        .push(Fault::Held(Duration::from_secs(8)));
    let emit = Emitted {
        event: event("t", State::Done, turn()),
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    };
    events.send(emit).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    chat.say("meanwhile").await;
    let prompts = || seen.lock().unwrap().prompts.clone();
    within(4, || {
        prompts().iter().any(|p| p == "meanwhile").then_some(())
    })
    .await;
    let store = Store::new(world.state.path());
    let queued =
        || (json(&store.load().unwrap())["conversation"]["outbox"].as_array()).map(Vec::len);
    assert_eq!(queued(), Some(1), "still queued");
    within(20, || (queued() == Some(0)).then_some(())).await;
    within(10, || (!versions(&chat, "t").is_empty()).then_some(())).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(versions(&chat, "t").len(), 1, "went out once");
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
    let open = |talk: &mut Conversation, chat: &ChatId, item: &str| {
        talk.emitted(Emitted {
            event: event(item, State::Running, turn()),
            chat: Some(chat.clone()),
            prompts: Vec::new(),
            settles: Vec::new(),
        });
    };
    for n in 0..20 {
        open(&mut talk, &id, &format!("a{n:02}"));
    }
    send_queued(&world, &mut talk, &GroupKey(chat.hex.clone())).await;
    open(&mut talk, &team_id, "b");
    send_queued(&world, &mut talk, &GroupKey(team_hex.clone())).await;
    let read = kept.read;
    let conversation = talk.saved().clone();
    store
        .save(&Kept {
            conversation,
            read,
            ..Kept::default()
        })
        .unwrap();

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
                let read = marmot_agent::carrier::read_item(m.kind, &m.plaintext, &m.tags);
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

/// Sends what `talk` has queued for `group` through MDK, as the core does, and notes each as sent;
/// gives the last event. An item's later version is MDK's edit of its first event, so that event
/// must be one MDK sent.
async fn send_queued(world: &World, talk: &mut Conversation, group: &GroupKey) -> Option<EventKey> {
    let mut last = None;
    while let Some((token, what)) = talk.next_send(group) {
        let event = world.send(group, &what, &token).await.unwrap();
        let (Sending::Item { key, .. } | Sending::Words { key, .. }) = &what;
        talk.sent(key, event.clone());
        last = Some(event);
    }
    last
}

impl Stub {
    fn caps() -> Capabilities {
        stub(false).0.capabilities()
    }
}

/// What makes the next send fail for good: MDK refuses it in a group that stays live, or, with
/// `give_up`, MDK takes it and drops it before it goes out, so the core gives it up after its one
/// try. The core is started with one try for a send MDK drops.
///
/// A fault is met by the next send the core makes, whatever it is, so it is armed only once the
/// core has nothing queued: else a send still queued, which the core asks MDK about again, would
/// meet it in place of the one the test means.
async fn dropped_next(world: &World, give_up: bool) {
    world.nothing_queued().await;
    let fault = if give_up {
        Fault::Reject
    } else {
        Fault::Refuse(marmot_app::AppError::InvalidAppMessagePayload(
            "refused".into(),
        ))
    };
    world.marmot.faults.push(fault);
}

/// An item whose closing version MDK refuses, or that the core gives up, is closed in the chat
/// all the same, by a later version of the core's own; the chat is told in a `failed` notice
/// (S2, L2). Found by the TLA+ model, whose cases `give-up-closes-item` and `refuse-closes-item`
/// now check the fix.
async fn closing_version_dropped(give_up: bool) {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let core = world.core(driver, Store::new(world.state.path()));
    let core = world.run(core.rejected_sends(1));
    let (chat, id) = a_chat(&world, &seen).await;
    let emit = |state| Emitted {
        event: event("t", state, turn()),
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    };
    events.send(emit(State::Running)).unwrap();
    within(20, || (!versions(&chat, "t").is_empty()).then_some(())).await;
    dropped_next(&world, give_up).await;
    events.send(emit(State::Done)).unwrap();
    let (_, read) = within(30, || {
        (chat.items().into_iter())
            .find(|(_, r)| r.item.event.header().item.as_str() == "t" && r.item.version > 2)
    })
    .await;
    let first = versions(&chat, "t")
        .into_iter()
        .find(|v| v.1 == 1)
        .map(|v| v.0);
    assert!(
        versions(&chat, "t").iter().all(|v| v.1 != 2),
        "the closing version never went out"
    );
    assert_eq!(read.item.version, 3, "the close follows on");
    assert_eq!(read.item.first, first, "and names the item's first event");
    assert_eq!(read.item.event.header().state, State::Done);
    assert!(read.item.event.native().is_none());
    assert!(read.item.settles.is_empty());
    within(20, || {
        (chat.items().into_iter())
            .any(|(_, r)| refused(&r.item.event).as_deref() == Some("failed"))
            .then_some(())
    })
    .await;
    core.stop().await;
    world.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_item_whose_closing_version_mdk_refuses_is_closed_after_all() {
    closing_version_dropped(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_item_whose_closing_version_is_given_up_is_closed_after_all() {
    closing_version_dropped(true).await;
}

// The last try MDK may drop is saved with what it leads to, in one write: the send given up and
// what stands in for it (D-097). A state that counts the last drop while the send is still queued
// is never written, so a crash cannot leave the send to be tried once more than its tries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_last_dropped_try_is_saved_with_its_give_up() {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, events, seen) = stub(false);
    let (store, failed) = Store::failing_when(world.state.path(), |kept| {
        let kept = json(kept);
        let outbox = kept["conversation"]["outbox"].as_array().cloned();
        (outbox.unwrap_or_default().iter()).any(|q| q["rejected"].as_u64() >= Some(1))
    });
    let core = world.run(world.core(driver, store).rejected_sends(1));
    let (chat, id) = a_chat(&world, &seen).await;
    world.marmot.faults.push(Fault::Reject);
    let emit = Emitted {
        event: event("t", State::Done, turn()),
        chat: Some(id.clone()),
        prompts: Vec::new(),
        settles: Vec::new(),
    };
    events.send(emit).unwrap();
    within(20, || {
        (chat.items().into_iter())
            .any(|(_, r)| refused(&r.item.event).as_deref() == Some("failed"))
            .then_some(())
    })
    .await;
    assert!(
        !failed.load(SeqCst),
        "no state counts the last drop of a queued send"
    );
    core.stop().await;
    world.runtime.shutdown().await;
}

/// A notice that settles a request, which MDK refuses or the core gives up, is followed by a
/// `failed` notice that settles the request in its place (L1). Found by the TLA+ model, whose
/// cases `give-up-settles` and `refuse-settles` now check the fix.
async fn settling_notice_dropped(give_up: bool) {
    let world = World::new("[codex]\ncwd = \"/unused\"\n").await;
    world.runtime.start().await.unwrap();
    let (driver, _events, seen) = stub(false);
    let core = world.core(driver, Store::new(world.state.path()));
    let core = world.run(core.rejected_sends(1));
    let (chat, _) = a_chat(&world, &seen).await;
    // A message of the person's, which is no item: an interrupt that names it is refused as for
    // an unknown item, in a notice that settles it.
    let message = chat.say("not an item").await;
    within(20, || {
        (seen.lock().unwrap().prompts.len() == 2).then_some(())
    })
    .await;
    dropped_next(&world, give_up).await;
    let request = chat.ask(Some(&message), &Asked::Interrupt, "Stop").await;
    let settled = settled_by(&chat, &request, 30).await;
    assert_eq!(refused(&settled).as_deref(), Some("failed"));
    let settling = (chat.items().into_iter()).filter(|(_, r)| r.item.settles.contains(&request));
    assert_eq!(settling.count(), 1, "settled once");
    core.stop().await;
    world.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_whose_notice_mdk_refuses_is_settled_after_all() {
    settling_notice_dropped(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_whose_notice_is_given_up_is_settled_after_all() {
    settling_notice_dropped(true).await;
}
