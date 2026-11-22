//! A run is reproducible: the same schedule gives the same trace, a capsule replays exactly, and
//! the shrinker cuts a failing schedule down to one that fails the same way.

use marmot_agent_sim::{
    Action, BugSet, Capsule, Family, Faults, Posted, ReadStep, RunName, Schedule, What, generate,
    run, shrink,
};

#[test]
fn the_same_schedule_gives_the_same_trace() {
    for family in Family::GATE {
        for index in 0..3 {
            let schedule = generate(&RunName::new(family, 0, index));
            let first = run(&schedule);
            let second = run(&schedule);
            assert_eq!(first.trace, second.trace, "{}", schedule.name);
            assert!(
                first.trace.len() > schedule.steps.len(),
                "{}",
                schedule.name
            );
        }
    }
}

// Two groups that MDK cannot read in the same catch-up wait until the same moment on the virtual
// clock. They are read again in the same order every time, or a capsule would not replay.
#[test]
fn groups_read_again_at_the_same_moment_give_the_same_trace() {
    let prompt = |chat| Posted {
        chat,
        what: What::Prompt { approval: false },
    };
    let fails = Action::ReadFails {
        at: ReadStep::Stored,
    };
    let schedule = Schedule {
        name: RunName::new(Family::Live, 0, 0),
        chats: 2,
        bugs: BugSet::default(),
        faults: Faults::default(),
        messages: vec![prompt(0), prompt(1)],
        steps: vec![
            Action::Invite { chat: 0 },
            Action::Invite { chat: 1 },
            fails,
            fails,
            Action::Store { msg: 0 },
            Action::Store { msg: 1 },
            Action::Lag,
        ],
    };
    let first = run(&schedule);
    assert!(first.failure.is_none(), "{:?}", first.failure);
    let failed = (first.trace.iter()).filter(|t| t.what == "Stored: Error");
    assert_eq!(failed.count(), 2, "both groups wait");
    for _ in 0..16 {
        assert_eq!(run(&schedule).trace, first.trace);
    }
}

#[test]
fn a_name_always_makes_the_same_schedule() {
    let name = RunName::new(Family::Mixed, 7, 3);
    assert_eq!(generate(&name), generate(&name));
    assert_ne!(
        generate(&name),
        generate(&RunName::new(Family::Mixed, 7, 4))
    );
}

// A failure shrinks and leaves a capsule that replays it exactly. The schedule is fixed, so this
// tests the shrinker and the capsule only; that the simulator finds the bug is
// `bugs.rs` `mark_from_live_is_found`. With the bug `mark_from_live` put back, a prompt heard
// live moves the mark past an answer stored before it, which is never read (L1). The first
// message, the ticks and the step after the prompt play no part.
#[cfg(feature = "bugs")]
#[test]
fn a_failure_is_shrunk_and_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = |approval| Posted {
        chat: 0,
        what: What::Prompt { approval },
    };
    let answer = Posted {
        chat: 0,
        what: What::Answer,
    };
    let schedule = Schedule {
        name: RunName::new(Family::Live, 0, 0),
        chats: 1,
        bugs: BugSet {
            mark_from_live: true,
            ..BugSet::default()
        },
        faults: Faults::default(),
        messages: vec![prompt(false), answer, prompt(true)],
        steps: vec![
            Action::Invite { chat: 0 },
            Action::Tick { ms: 50 },
            Action::Store { msg: 1 },
            Action::Tick { ms: 50 },
            Action::Hear { msg: 2 },
            Action::Tick { ms: 1000 },
        ],
    };
    let report = run(&schedule);
    let property = (report.failure.as_ref())
        .expect("mark_from_live loses the answer")
        .failure
        .property
        .clone();
    assert_eq!(property, "L1");
    let (small, small_report) = shrink(&schedule, &report, 300);
    assert!(small.steps.len() < schedule.steps.len());
    assert!(
        small.messages.len() < schedule.messages.len(),
        "the messages no step names are dropped"
    );
    let capsule = Capsule::new(small, &small_report).unwrap();
    let path = capsule.write(dir.path()).unwrap();
    let read = Capsule::read(&path).unwrap();
    read.replay().expect("the capsule replays exactly");
    assert_eq!(read.failed.failure.property, property);
}
