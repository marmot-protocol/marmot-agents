//! Each oracle catches its bug (`verification.md`: "A check must be shown to catch its bug"):
//! worlds made by hand as a faulty core would leave them, which the oracles before review round 1
//! passed, each fail the property now; and the same world made right passes.

use marmot_agent::EventKey;
use marmot_agent::carrier::{self, Item};
use marmot_agent_events::body::{Approval, Body, Notice, NoticeKind, Turn};
use marmot_agent_events::{AgentEvent, Header, State};
use marmot_agent_sim::oracle::{self, Failure, NET_EARLIEST_MS};
use marmot_agent_sim::world::{CHAT, Delivery, EDIT, Emission, RefusedSend, World};
use marmot_agent_sim::{Posted, What};

/// A world whose chats the agent joined, with `posted` posted and stored.
fn world(chats: u8, posted: &[Posted]) -> World {
    let mut world = World::new(chats, posted.to_vec(), true);
    for chat in 0..usize::from(chats) {
        world.invite(chat);
        world.accepted(chat);
    }
    for msg in 0..posted.len() {
        world.store(msg);
    }
    world
}

fn prompt(chat: u8) -> Posted {
    Posted {
        chat,
        what: What::Prompt { approval: false },
    }
}

fn set(chat: u8) -> Posted {
    Posted {
        chat,
        what: What::Set,
    }
}

fn event(msg: usize, world: &World) -> EventKey {
    world.msgs[msg].as_ref().expect("posted").event.clone()
}

fn header(item: &str, state: State) -> Header {
    Header {
        harness: "sim".into(),
        session: None,
        turn: None,
        subagent: None,
        item: item.into(),
        seq: 1,
        state,
        title: item.into(),
        text: String::new(),
    }
}

fn notice(reason: &str) -> Body {
    Body::Notice(Notice {
        kind: NoticeKind::Refused,
        duration_ms: None,
        reason: Some(reason.into()),
    })
}

fn turn() -> Body {
    Body::Turn(Turn {
        started_by: marmot_agent_events::body::StartedBy::User,
        duration_ms: None,
        usage: None,
    })
}

fn item(id: &str, version: u64, state: State, body: Body, settles: Vec<EventKey>) -> Item {
    Item {
        event: AgentEvent::new(header(id, state), body, None).expect("no secret"),
        version,
        first: None,
        started_by: None,
        settles,
        thread: None,
    }
}

/// The agent's item, stored in `chat`; gives its event.
fn sent(world: &mut World, chat: usize, item: &Item) -> EventKey {
    let out = carrier::item(item).expect("fits");
    let row = world.store_sent(chat, out.kind, out.content, out.tags, None);
    world.keep(row)
}

/// The agent's words, stored in `chat`, editing `edits` when given; gives their event.
fn said(world: &mut World, chat: usize, text: &str, edits: Option<EventKey>) -> EventKey {
    let kind = if edits.is_some() { EDIT } else { CHAT };
    let row = world.store_sent(chat, kind, text.into(), Vec::new(), edits);
    world.keep(row)
}

/// A send of `item` in `chat` that MDK refused for good.
fn refused(world: &mut World, chat: usize, item: &Item) {
    let notice = match item.event.body() {
        Body::Notice(n) => n.reason.clone(),
        _ => None,
    };
    world.refused.push(RefusedSend {
        chat,
        item: Some((item.event.header().item.as_str().into(), item.version)),
        settles: item.settles.clone(),
        words: None,
        state: Some(item.event.header().state),
        seq: Some(item.event.header().seq),
        notice,
    });
}

/// The harness was handed the prompt `msg`, under the core's chat id `chat`.
fn handed(world: &mut World, msg: usize, chat: &str) {
    let stored = world.last_order();
    world.harness.deliveries.push(Delivery {
        life: 1,
        ms: 0,
        stored,
        msg: Some(msg),
        kind: "prompt",
        chat: Some(chat.into()),
        session: None,
        chat_taken: false,
        item: None,
    });
}

/// The harness emitted `item` in the core's chat `chat` at `ms`.
fn emitted(world: &mut World, ms: u64, chat: &str, item: &str, words: Option<&str>) -> Emission {
    let e = Emission {
        life: 1,
        ms,
        chat: chat.into(),
        item: item.into(),
        seq: 1,
        settles: Vec::new(),
        session: None,
        state: State::Done,
        words: words.map(str::to_string),
    };
    world.harness.emissions.push(e.clone());
    e
}

fn property(failure: Option<Failure>) -> Option<String> {
    failure.map(|f| f.property)
}

fn properties(failures: Vec<Failure>) -> Vec<String> {
    failures.into_iter().map(|f| f.property).collect()
}

// Finding 5. The old window was 30 s: a core that fired the net at 30 s, while the harness showed
// the effect at 60 s, passed.
#[test]
fn a_net_that_fires_before_its_time_fails_s1() {
    let make = |shown_at: u64| {
        let mut w = world(1, &[set(0)]);
        let request = event(0, &w);
        w.harness.requests.insert("r0".into(), (0, 0));
        let mut e = emitted(&mut w, shown_at, "A", "sess-s1", None);
        e.settles = vec!["r0".into()];
        *w.harness.emissions.last_mut().unwrap() = e;
        let net = item("core-1", 1, State::Done, notice("no_effect"), vec![request]);
        sent(&mut w, 0, &net);
        w
    };
    assert_eq!(property(oracle::safety(&make(60_000))), Some("S1".into()));
    // Shown after the net could first fire: a fair race, not a failure.
    assert_eq!(property(oracle::safety(&make(NET_EARLIEST_MS))), None);
}

// Finding 4. The old S5 checked items only: words sent to another chat, or an edit of another
// message, passed.
#[test]
fn words_in_the_wrong_chat_fail_s5() {
    let base = || {
        let mut w = world(2, &[prompt(0), prompt(1)]);
        handed(&mut w, 0, "A");
        handed(&mut w, 1, "B");
        emitted(&mut w, 10, "A", "words-t1-2", Some("w-t1-2"));
        w
    };
    let mut right = base();
    said(&mut right, 0, "w-t1-2", None);
    assert_eq!(property(oracle::safety(&right)), None);

    let mut wrong = base();
    said(&mut wrong, 1, "w-t1-2", None);
    assert_eq!(property(oracle::safety(&wrong)), Some("S5".into()));

    // An edit of the words, in their chat, that edits another message of the agent's there.
    let mut edit = base();
    emitted(&mut edit, 20, "A", "words-t1-4", Some("w-t1-4"));
    emitted(&mut edit, 30, "A", "words-t1-2", Some("w-t1-2 more"));
    said(&mut edit, 0, "w-t1-2", None);
    let other = said(&mut edit, 0, "w-t1-4", None);
    said(&mut edit, 0, "w-t1-2 more", Some(other));
    assert_eq!(property(oracle::safety(&edit)), Some("S5".into()));

    let mut own = base();
    emitted(&mut own, 30, "A", "words-t1-2", Some("w-t1-2 more"));
    let first = said(&mut own, 0, "w-t1-2", None);
    said(&mut own, 0, "w-t1-2 more", Some(first));
    assert_eq!(property(oracle::safety(&own)), None);
}

// Finding 3, requests. The old L1 let off a request when any refused send had settled it: a core
// that answered a refused version with no `failed` notice passed.
#[test]
fn a_request_left_after_a_refused_version_fails_l1() {
    let mut w = world(2, &[set(0)]);
    let request = event(0, &w);
    let version = item(
        "sess-s1",
        2,
        State::Running,
        Body::Approval(no_choice()),
        vec![request.clone()],
    );
    refused(&mut w, 0, &version);
    assert_eq!(properties(oracle::liveness(&w)), vec!["L1"]);
    // Its `failed` notice refused too: the request is still to be settled, by the notice the core
    // sends in its place. It used to be let off, by design, until the TLA+ model showed it left
    // the request settled nowhere (L1).
    let notice_settling =
        |id: &str| item(id, 1, State::Done, notice("failed"), vec![request.clone()]);
    let failed = notice_settling("core-1");
    refused(&mut w, 0, &failed);
    assert_eq!(properties(oracle::liveness(&w)), vec!["L1"]);
    // The same notice refused again, as when the core tried it again after a crash before the
    // first refusal was saved: still one notice, with nothing yet in its place. And a notice in
    // another chat that names the request is not one of its own. Review round 1 of
    // `dropped-sends`: the oracle counted refusals, so either let the request off.
    refused(&mut w, 0, &failed);
    refused(&mut w, 1, &notice_settling("core-9"));
    assert_eq!(properties(oracle::liveness(&w)), vec!["L1"]);
    // The notice in its place refused too: the last, so let off, as in a group that is gone.
    refused(&mut w, 0, &notice_settling("core-2"));
    assert_eq!(properties(oracle::liveness(&w)), Vec::<String>::new());
}

// Finding 3, closing. The old S2 let off an item when any version of it was refused: a core that
// never closed an item after MDK refused one of its open versions passed.
#[test]
fn an_item_left_open_after_a_refused_open_version_fails_s2() {
    let make = || {
        let mut w = world(1, &[]);
        let first = item("turn-t1-2", 1, State::Running, turn(), Vec::new());
        sent(&mut w, 0, &first);
        let second = item("turn-t1-2", 2, State::Running, turn(), Vec::new());
        refused(&mut w, 0, &second);
        w
    };
    let mut w = make();
    assert_eq!(property(oracle::after_stop(&w)), Some("S2".into()));
    // Its closing version refused: the item is still open in the chat until the core closes it
    // with a version of its own. It used to be let off, by design, until the TLA+ model showed
    // nothing ever closed it (S2).
    let closing = item("turn-t1-2", 3, State::Done, turn(), Vec::new());
    refused(&mut w, 0, &closing);
    assert_eq!(property(oracle::after_stop(&w)), Some("S2".into()));
    let mut close = item(
        "turn-t1-2",
        4,
        State::Done,
        Body::Notice(Notice {
            kind: NoticeKind::Warning,
            duration_ms: None,
            reason: Some("not_delivered".into()),
        }),
        Vec::new(),
    );
    close.first = Some(first_event(&w));
    sent(&mut w, 0, &close);
    assert_eq!(property(oracle::after_stop(&w)), None);
    assert_eq!(property(oracle::safety(&w)), None);
    // The core's close refused too: the last, so let off, as in a group that is gone.
    let mut w = make();
    refused(&mut w, 0, &closing);
    refused(&mut w, 0, &close);
    assert_eq!(property(oracle::after_stop(&w)), None);
    // Until the item opens again, newer than that close, and is left open. Review round 1 of
    // `dropped-sends`: the refused close let the item off for good.
    let mut again = item("turn-t1-2", 5, State::Running, turn(), Vec::new());
    again.first = Some(first_event(&w));
    sent(&mut w, 0, &again);
    assert_eq!(property(oracle::after_stop(&w)), Some("S2".into()));
}

/// The first event the agent stored.
fn first_event(world: &World) -> EventKey {
    (world.rows.iter())
        .find(|r| r.from_agent)
        .expect("one was stored")
        .event
        .clone()
}

// Finding 2. The old L1 and L3 skipped a message never stored: a core that never joined a chat
// passed with everything posted there waiting.
#[test]
fn a_chat_never_joined_fails_liveness() {
    let mut w = World::new(1, vec![prompt(0), set(0)], true);
    w.invite(0);
    w.store(0);
    w.store(1);
    let found = properties(oracle::liveness(&w));
    assert_eq!(found, vec!["L3", "L3", "L1"], "{found:?}");
    w.accepted(0);
    handed(&mut w, 0, "A");
    let request = event(1, &w);
    let failed = item("core-1", 1, State::Done, notice("failed"), vec![request]);
    sent(&mut w, 0, &failed);
    assert_eq!(properties(oracle::liveness(&w)), Vec::<String>::new());
}

fn no_choice() -> Approval {
    Approval {
        subject: None,
        choices: Vec::new(),
        answer: None,
    }
}

// Review round 2, S3. The old check of an item's first stored version looked only at `first`: a
// core that sent v3 as the item's first event, with v1 and v2 never stored nor refused, passed.
#[test]
fn a_first_stored_version_that_skips_versions_fails_s3() {
    let v = |n: u64| item("turn-t1-2", n, State::Running, turn(), Vec::new());
    let mut skipped = world(1, &[]);
    sent(&mut skipped, 0, &v(3));
    assert_eq!(property(oracle::safety(&skipped)), Some("S3".into()));
    // v1 and v2 refused: what MDK refused may be skipped.
    let mut refused_first = world(1, &[]);
    refused(&mut refused_first, 0, &v(1));
    refused(&mut refused_first, 0, &v(2));
    sent(&mut refused_first, 0, &v(3));
    assert_eq!(property(oracle::safety(&refused_first)), None);
    // Only v1 refused: v2 is still skipped.
    let mut one_refused = world(1, &[]);
    refused(&mut one_refused, 0, &v(1));
    sent(&mut one_refused, 0, &v(3));
    assert_eq!(property(oracle::safety(&one_refused)), Some("S3".into()));
    let mut first = world(1, &[]);
    sent(&mut first, 0, &v(1));
    assert_eq!(property(oracle::safety(&first)), None);
}

// Review round 2, S12. The old check took the session's state from the versions stored in or
// before the prompt's millisecond: a prompt handed over before the session's end was stored, in
// the same millisecond, failed though it named a session still running. Storage order decides.
#[test]
fn a_session_ended_in_the_same_millisecond_counts_only_if_stored_before() {
    let make = |close_first: bool| {
        let mut w = world(1, &[prompt(0)]);
        w.harness.sessions_started.push(("A".into(), "s1-1".into()));
        let session = |state| item("sess-s1-1", 1, state, Body::Approval(no_choice()), vec![]);
        let first = sent(&mut w, 0, &session(State::Running));
        let mut done = session(State::Done);
        done.version = 2;
        done.first = Some(first);
        if close_first {
            sent(&mut w, 0, &done);
        }
        handed(&mut w, 0, "A");
        w.harness.deliveries.last_mut().unwrap().session = Some("s1-1".into());
        if !close_first {
            sent(&mut w, 0, &done);
        }
        // Everything in the same millisecond.
        assert!(w.rows.iter().all(|r| r.ms == w.rows[0].ms));
        w
    };
    assert_eq!(property(oracle::safety(&make(true))), Some("S12".into()));
    assert_eq!(property(oracle::safety(&make(false))), None);
}

// Review round 2, L2. The old check found the item's last stored version and compared its state:
// a core that dropped the second of two `running` versions passed, since the first was stored as
// `running` too. Each emission is known by its `seq`.
#[test]
fn a_dropped_later_version_in_the_same_state_fails_l2() {
    let make = |second_stored: bool| {
        let mut w = world(1, &[prompt(0)]);
        let (_, events) = w.new_life();
        handed(&mut w, 0, "A");
        for seq in [1, 2] {
            let mut e = emitted(&mut w, seq, "A", "sess-s1-1", None);
            e.seq = seq;
            e.state = State::Running;
            *w.harness.emissions.last_mut().unwrap() = e;
        }
        let v1 = item(
            "sess-s1-1",
            1,
            State::Running,
            Body::Approval(no_choice()),
            vec![],
        );
        sent(&mut w, 0, &with_seq(v1, 1));
        let mut v2 = with_seq(
            item(
                "sess-s1-1",
                2,
                State::Running,
                Body::Approval(no_choice()),
                vec![],
            ),
            2,
        );
        v2.first = Some(w.rows.last().unwrap().event.clone());
        if second_stored {
            sent(&mut w, 0, &v2);
        }
        (w, events, v2)
    };
    let (dropped, _events, _) = make(false);
    let found = properties(oracle::liveness(&dropped));
    assert_eq!(found, vec!["L2"], "{found:?}");
    let (kept, _events, _) = make(true);
    assert_eq!(properties(oracle::liveness(&kept)), Vec::<String>::new());
    // MDK refused the first version: that does not let off the second.
    let (mut first_refused, _events, _) = make(false);
    let v1 = with_seq(
        item(
            "sess-s1-1",
            1,
            State::Running,
            Body::Approval(no_choice()),
            vec![],
        ),
        1,
    );
    refused(&mut first_refused, 0, &v1);
    assert_eq!(properties(oracle::liveness(&first_refused)), vec!["L2"]);
    // MDK refused the second: let off.
    let (mut second_refused, _events, v2) = make(false);
    refused(&mut second_refused, 0, &v2);
    assert_eq!(
        properties(oracle::liveness(&second_refused)),
        Vec::<String>::new()
    );
}

fn with_seq(item: Item, seq: u64) -> Item {
    let mut header = item.event.header().clone();
    header.seq = seq;
    Item {
        event: AgentEvent::new(header, item.event.body().clone(), None).expect("no secret"),
        ..item
    }
}

// Review round 1 of `dropped-sends`. A closing version dropped when the chat already shows the
// item closed (here by the core's close at a restart) is not followed by the core's own close,
// so it does not show "not delivered" over the close the chat has; the old L2 asked for that
// close all the same. Found by the simulator (`sends`, seed 3). One dropped while the chat shows
// the item open still fails L2 without the core's close.
#[test]
fn a_closing_version_dropped_after_a_close_is_let_off_l2() {
    let make = |closed_first: bool| {
        let mut w = world(1, &[prompt(0)]);
        let (_, events) = w.new_life();
        handed(&mut w, 0, "A");
        let mut e = emitted(&mut w, 2, "A", "sess-s1-1", None);
        e.seq = 2;
        *w.harness.emissions.last_mut().unwrap() = e;
        let v1 = item(
            "sess-s1-1",
            1,
            State::Running,
            Body::Approval(no_choice()),
            vec![],
        );
        sent(&mut w, 0, &v1);
        let mut version = 2;
        if closed_first {
            let mut closed = item(
                "sess-s1-1",
                2,
                State::Cancelled,
                Body::Approval(no_choice()),
                vec![],
            );
            closed.first = Some(first_event(&w));
            sent(&mut w, 0, &closed);
            version = 3;
        }
        let done = with_seq(
            item(
                "sess-s1-1",
                version,
                State::Done,
                Body::Approval(no_choice()),
                vec![],
            ),
            2,
        );
        refused(&mut w, 0, &done);
        (w, events)
    };
    let (open, _events) = make(false);
    assert_eq!(properties(oracle::liveness(&open)), vec!["L2"]);
    let (closed, _events) = make(true);
    assert_eq!(properties(oracle::liveness(&closed)), Vec::<String>::new());
}

// S14: a subagent's item in a thread is under its own subagent's `task` item; one under another
// subagent's fails (D-121, D-142).
#[test]
fn an_item_in_another_subagents_thread_fails_s14() {
    let task = |id: &str| {
        let body = Body::Task(marmot_agent_events::body::Task {
            kind: "subagent".into(),
            description: None,
            subject: None,
        });
        item(id, 1, State::Running, body, vec![])
    };
    let under = |task: &str, place: &EventKey| {
        let mut child = item("subcmd-t", 1, State::Running, turn(), vec![]);
        let header = Header {
            subagent: Some(task.into()),
            ..child.event.header().clone()
        };
        child.event = AgentEvent::new(header, child.event.body().clone(), None).unwrap();
        child.thread = Some(marmot_agent::carrier::Thread {
            root: place.clone(),
            parent: Some(place.clone()),
            agent: marmot_agent::PubKey::from("agent"),
        });
        child
    };
    let make = |right: bool| {
        let mut w = world(1, &[]);
        let own = sent(&mut w, 0, &task("sub-t"));
        let other = sent(&mut w, 0, &task("sub-u"));
        sent(
            &mut w,
            0,
            &under("sub-t", if right { &own } else { &other }),
        );
        w
    };
    assert_eq!(property(oracle::safety(&make(true))), None);
    assert_eq!(property(oracle::safety(&make(false))), Some("S14".into()));
}

// S14: a subagent's item goes in the main chat only when its subagent's `task` item has not gone
// out to that chat before it; with that item there, an item of the subagent's in the main chat
// fails (D-142).
#[test]
fn an_item_in_the_main_chat_whose_task_item_went_out_fails_s14() {
    let task = || {
        let body = Body::Task(marmot_agent_events::body::Task {
            kind: "subagent".into(),
            description: None,
            subject: None,
        });
        item("sub-t", 1, State::Running, body, vec![])
    };
    let of_subagent = || {
        let mut child = item("subcmd-t", 1, State::Running, turn(), vec![]);
        let header = Header {
            subagent: Some("sub-t".into()),
            ..child.event.header().clone()
        };
        child.event = AgentEvent::new(header, child.event.body().clone(), None).unwrap();
        child
    };
    // Its task item never went to this chat: the main chat is its place.
    let mut w = world(2, &[]);
    sent(&mut w, 1, &task());
    sent(&mut w, 0, &of_subagent());
    assert_eq!(property(oracle::safety(&w)), None);
    // It did, before it.
    let mut w = world(1, &[]);
    sent(&mut w, 0, &task());
    sent(&mut w, 0, &of_subagent());
    assert_eq!(property(oracle::safety(&w)), Some("S14".into()));
}
