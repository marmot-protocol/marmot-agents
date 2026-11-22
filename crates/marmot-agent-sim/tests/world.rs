//! The simulated world behaves as the harness and MDK it stands for, so the oracles judge what a
//! real run would show. Each test is a world driven by hand, as the core would drive it; each
//! names what the world did before review round 2 and the faulty run that let pass.

use marmot_agent::carrier::{self, Asked, Item};
use marmot_agent_driver::{Action, Chat, ChatId, Control, Person, PersonId, RequestId};
use marmot_agent_events::body::{Approval, Body, Choice, Effect, Notice, NoticeKind};
use marmot_agent_events::{AgentEvent, Header, State};
use marmot_agent_sim::oracle;
use marmot_agent_sim::world::{Decision, Handed, Landing, Move, World};
use marmot_agent_sim::{Posted, What};
use std::time::Duration;

/// Runs `test` on a paused clock, as a run does.
fn paused(test: impl AsyncFnOnce()) {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("a runtime")
        .block_on(test());
}

/// A world with one chat the agent joined, `posted` posted and stored, and a harness running.
fn world(posted: &[What]) -> World {
    let posted: Vec<Posted> = (posted.iter())
        .map(|&what| Posted { chat: 0, what })
        .collect();
    let mut world = World::new(1, posted.clone(), true);
    world.invite(0);
    world.accepted(0);
    for msg in 0..posted.len() {
        world.store(msg);
    }
    let (_, events) = world.new_life();
    // Kept open for the run: the harness's events go nowhere.
    std::mem::forget(events);
    world
}

fn control(request: &str, action: Action) -> Control {
    Control {
        request: RequestId(request.into()),
        by: Person {
            id: PersonId("p-1".into()),
            name: "Owner".into(),
        },
        action,
    }
}

fn new_session(request: &str, session: Option<&str>) -> Control {
    let chat = Chat {
        id: ChatId("c-1".into()),
        group: false,
        name: None,
    };
    let session = session.map(Into::into);
    control(request, Action::NewSession { chat, session })
}

/// The core reads request message `msg` and hands the harness what it became, as
/// `Core::handle` does: the harness meets `request`.
fn hand(world: &mut World, msg: usize, request: Handed<'_>) -> Decision {
    world.reading = Some(msg);
    world.handed(world.harness.life, request)
}

fn event(world: &World, msg: usize) -> marmot_agent::EventKey {
    world.msgs[msg].as_ref().expect("posted").event.clone()
}

fn header(item: &str, state: State) -> Header {
    Header {
        harness: "sim".into(),
        session: None,
        turn: None,
        item: item.into(),
        seq: 1,
        state,
        title: item.into(),
        text: String::new(),
    }
}

// The old harness noted a request as taken, and acted on it, when it was delivered, even under a
// `Slow` move: it could show the effect before the driver answered the core, and the oracle
// counted the `no_effect` window from the delivery. A core whose net fired 125 s after a setting
// was delivered, 95 s after the driver acknowledged it, while the harness showed its effect at
// 125 s, passed S1.
#[test]
fn the_net_window_starts_when_the_driver_acknowledges() {
    paused(async || {
        let mut w = world(&[What::Prompt { approval: false }, What::Set]);
        w.harness
            .sessions_started
            .push(("c-1".into(), "s1-1".into()));
        w.harness.moves.push_back(Move::Slow(30_000));
        // Message 1 is a setting whose effect the agent's words show (its number is 1 in 3).
        let set = control(
            "r-1",
            Action::Set {
                session: "s1-1".into(),
                setting: "mode".into(),
                value: "v1".into(),
            },
        );
        let Decision::Later(ms, take) = hand(&mut w, 1, Handed::Control(&set)) else {
            panic!("a slow move holds the request");
        };
        assert_eq!(ms, 30_000);
        // Not yet taken: the harness has nothing of it to show.
        w.emit(0, false);
        assert!(w.harness.emissions.is_empty());
        assert!(w.harness.requests.is_empty());
        tokio::time::advance(Duration::from_millis(ms)).await;
        assert!(w.take(w.harness.life, Handed::Control(&set), take).is_ok());
        tokio::time::advance(Duration::from_secs(95)).await;
        w.emit(0, false);
        assert_eq!(w.harness.emissions.len(), 1, "the setting's effect");
        // The core's net settles it now, before its time from the acknowledgement.
        let net = Item {
            event: AgentEvent::new(
                header("core-1", State::Done),
                Body::Notice(Notice {
                    kind: NoticeKind::Refused,
                    duration_ms: None,
                    reason: Some("no_effect".into()),
                }),
                None,
            )
            .expect("no secret"),
            version: 1,
            first: None,
            started_by: None,
            settles: vec![event(&w, 1)],
        };
        let out = carrier::item(&net).expect("fits");
        let row = w.store_sent(0, out.kind, out.content, out.tags, None);
        w.keep(row);
        let failure = oracle::safety(&w).map(|f| f.property);
        assert_eq!(failure, Some("S1".into()));
    });
}

// The old harness left an interrupt's and a new session's message unknown, so S10 skipped them:
// a core that handed the harness the same new session twice passed.
#[test]
fn a_control_handed_twice_fails_s10() {
    paused(async || {
        let mut w = world(&[What::NewSession]);
        let first = new_session("r-1", None);
        assert!(matches!(
            hand(&mut w, 0, Handed::Control(&first)),
            Decision::Now(Ok(()))
        ));
        assert_eq!(oracle::safety(&w), None);
        let again = new_session("r-2", None);
        let _ = hand(&mut w, 0, Handed::Control(&again));
        let failure = oracle::safety(&w).map(|f| f.property);
        assert_eq!(failure, Some("S10".into()));
    });
}

// The old harness chose the session a new session ends when it took the request: two taken before
// either showed its effect both ended the first session, and the second replaced the session the
// first started without ending it.
#[test]
fn a_second_new_session_ends_the_one_the_first_started() {
    paused(async || {
        let mut w = world(&[What::NewSession, What::NewSession]);
        let _ = hand(&mut w, 0, Handed::Control(&new_session("r-1", Some("s0"))));
        let _ = hand(&mut w, 1, Handed::Control(&new_session("r-2", Some("s0"))));
        w.emit(0, false);
        w.emit(0, false);
        let shown: Vec<(String, State)> = (w.harness.emissions.iter())
            .map(|e| (e.item.clone(), e.state))
            .collect();
        let started = shown[1].0.clone();
        assert_eq!(shown[0], ("sess-s0".to_string(), State::Done));
        assert_eq!(shown[1].1, State::Running);
        assert_eq!(shown[2], (started, State::Done), "{shown:?}");
        assert_eq!(shown[3].1, State::Running);
        assert_eq!(shown.len(), 4);
    });
}

// The old person answered with `allow-N`, a choice the approval does not offer (D-028), which the
// harness read its message from.
#[test]
fn an_answer_chooses_what_the_approval_offers() {
    paused(async || {
        let w = world(&[What::Answer]);
        let m = w.msgs[0].as_ref().expect("posted");
        let asked = carrier::read_request(&w.agent, &m.tags).map(|(_, asked)| asked);
        assert!(
            matches!(&asked, Some(Asked::Choice { choice }) if choice == "allow"),
            "{asked:?}"
        );
    });
}

// The old person read the chat without the sends due to land by then, when no core had looked
// since: an answer posted after an approval landed named no target.
#[test]
fn an_answer_sees_an_approval_that_landed_while_no_core_looked() {
    paused(async || {
        let mut w = world(&[What::Prompt { approval: true }]);
        w.posted.push(Posted {
            chat: 0,
            what: What::Answer,
        });
        w.msgs.push(None);
        let approval = Item {
            event: AgentEvent::new(
                header("appr-t1-2", State::Waiting),
                Body::Approval(Approval {
                    subject: None,
                    choices: vec![Choice {
                        id: "allow".into(),
                        label: "Allow".into(),
                        effect: Effect::Allow,
                    }],
                    answer: None,
                }),
                None,
            )
            .expect("no secret"),
            version: 1,
            first: None,
            started_by: None,
            settles: Vec::new(),
        };
        let out = carrier::item(&approval).expect("fits");
        let row = w.store_sent(0, out.kind, out.content, out.tags, None);
        let landed = row.event.clone();
        let at = tokio::time::Instant::now() + Duration::from_secs(10);
        w.landings.push(Landing { at, row });
        tokio::time::advance(Duration::from_secs(20)).await;
        // The person posts the answer now.
        let tags = w.post(1).expect("posted").tags.clone();
        let target = carrier::read_request(&w.agent, &tags).and_then(|(t, _)| t);
        assert_eq!(target, Some(landed));
    });
}
