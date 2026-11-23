//! What the generator makes, and the campaign that runs it.

use marmot_agent_sim::{
    Action, BugSet, Family, Faults, Posted, RunName, Schedule, What, campaign, generate, run,
};
use std::collections::BTreeSet;

// Review round 2. The old generator kept a message on its list still to be heard after it was
// heard, so a later step spent on hearing it again did nothing. Since MDK #3606, within its
// contract, a message heard live brings only itself, once it is stored, and a notice of it again
// is a step of its own (`Reannounce`): no `Hear` or `Ask` names a message twice, nor one not yet
// stored.
#[test]
fn no_step_hears_a_message_twice_or_before_it_is_stored() {
    for family in [Family::Live, Family::Mixed] {
        for index in 0..300 {
            let schedule = generate(&RunName::new(family, 0, index));
            let mut stored: BTreeSet<usize> = BTreeSet::new();
            let mut heard: BTreeSet<usize> = BTreeSet::new();
            for (at, step) in schedule.steps.iter().enumerate() {
                match *step {
                    Action::Store { msg } => {
                        stored.insert(msg);
                    }
                    Action::Hear { msg } | Action::Ask { msg } => {
                        let name = &schedule.name;
                        assert!(
                            stored.contains(&msg),
                            "{name}: step {at} hears m{msg} unstored"
                        );
                        assert!(heard.insert(msg), "{name}: step {at} hears m{msg} again");
                    }
                    _ => {}
                }
            }
        }
    }
}

// Review round 2. The old campaign added the first index and the count in 32 bits: past the last
// index it panicked, or ran nothing and reported no failure.
#[test]
fn a_campaign_past_the_last_index_is_refused() {
    let none = BugSet::default();
    let past = campaign(Family::Calm, 0, u32::MAX, 2, none, None, 0);
    assert!(past.is_err());
    let last = campaign(Family::Calm, 0, u32::MAX, 1, none, None, 0).expect("the last index");
    assert_eq!(last.runs, 1);
}

// Review round 2. The old campaign dropped the error when it could not write a schedule, and ran
// it anyway: one that then hung left nothing behind.
#[test]
fn a_schedule_that_cannot_be_written_stops_the_campaign_before_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a-file");
    std::fs::write(&file, b"").unwrap();
    let error = campaign(Family::Calm, 0, 0, 1, BugSet::default(), Some(&file), 0)
        .expect_err("a file is no folder");
    assert!(error.contains("calm/v1/0/0"), "{error}");
}

/// One chat and one prompt, stored and heard, with `messages` and `steps` in place of those.
fn by_hand(chats: u8, messages: Vec<Posted>, steps: Vec<Action>) -> Schedule {
    Schedule {
        name: RunName::new(Family::Calm, 0, 0),
        chats,
        bugs: BugSet::default(),
        faults: Faults::default(),
        messages,
        steps,
    }
}

fn prompt(chat: u8) -> Posted {
    Posted {
        chat,
        what: What::Prompt { approval: false },
    }
}

/// The property a run failed, the step and the detail.
fn failed(schedule: &Schedule) -> Option<(String, Option<usize>, String)> {
    let report = run(schedule);
    report
        .failure
        .map(|f| (f.failure.property, f.step, f.failure.detail))
}

// #148. The old world skipped a step that named a message or a chat the schedule does not have,
// and the oracle skipped the message: a schedule written by hand with a wrong number passed
// without doing what it was written to check.
#[test]
fn a_step_naming_a_message_the_schedule_lacks_fails_the_run_before_it_starts() {
    let schedule = by_hand(
        1,
        vec![prompt(0)],
        vec![
            Action::Invite { chat: 0 },
            Action::Store { msg: 0 },
            Action::Hear { msg: 1 },
        ],
    );
    let report = run(&schedule);
    let failure = report.failure.expect("the run fails");
    assert_eq!(
        (failure.failure.property.as_str(), failure.step),
        ("schedule", Some(2))
    );
    assert!(
        failure.failure.detail.contains("m1"),
        "{}",
        failure.failure.detail
    );
    assert!(report.trace.is_empty(), "{:?}", report.trace);
    assert_eq!(report.stats.steps, 0);
}

#[test]
fn a_message_to_a_chat_the_schedule_lacks_fails_the_run() {
    let schedule = by_hand(
        1,
        vec![prompt(0), prompt(1)],
        vec![Action::Invite { chat: 0 }, Action::Store { msg: 0 }],
    );
    let (property, step, detail) = failed(&schedule).expect("the run fails");
    assert_eq!((property.as_str(), step), ("schedule", None));
    assert!(detail.contains("m1") && detail.contains("c1"), "{detail}");
}

#[test]
fn an_invitation_to_a_chat_the_schedule_lacks_fails_the_run() {
    let schedule = by_hand(1, vec![], vec![Action::Invite { chat: 1 }]);
    let (property, step, detail) = failed(&schedule).expect("the run fails");
    assert_eq!((property.as_str(), step), ("schedule", Some(0)));
    assert!(detail.contains("c1"), "{detail}");
}

// Review of #148. `Hear` and `Ask` both hear the message they name, whose kind decides what it
// is, so a step that names a prompt `Ask`, or an answer `Hear`, still does what it was written to:
// only a missing chat or message is skipped, and only that fails the check.
#[test]
fn a_step_that_hears_a_message_by_either_name_is_valid() {
    let answer = Posted {
        chat: 0,
        what: What::Answer,
    };
    for (messages, step) in [
        (vec![prompt(0)], Action::Ask { msg: 0 }),
        (vec![answer], Action::Hear { msg: 0 }),
    ] {
        let schedule = by_hand(
            1,
            messages,
            vec![Action::Invite { chat: 0 }, Action::Store { msg: 0 }, step],
        );
        assert_eq!(schedule.check(), Ok(()), "{step:?}");
    }
}

#[test]
fn a_schedule_written_by_hand_with_good_references_passes() {
    let schedule = by_hand(
        1,
        vec![prompt(0)],
        vec![
            Action::Invite { chat: 0 },
            Action::Store { msg: 0 },
            Action::Hear { msg: 0 },
        ],
    );
    assert_eq!(schedule.check(), Ok(()));
    assert_eq!(failed(&schedule), None);
}

// Every generated schedule names only what it has, so the check changes no seeded run.
#[test]
fn every_generated_schedule_names_only_what_it_has() {
    for family in Family::ALL {
        for index in 0..500 {
            let schedule = generate(&RunName::new(family, 0, index));
            assert_eq!(schedule.check(), Ok(()), "{}", schedule.name);
        }
    }
}
