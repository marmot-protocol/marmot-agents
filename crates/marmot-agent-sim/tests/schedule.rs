//! What the generator makes, and the campaign that runs it.

use marmot_agent_sim::{Action, BugSet, Family, RunName, campaign, generate};
use std::collections::BTreeSet;

// Review round 2. The old generator kept a message on its list still to be heard after a later
// message of its chat was heard live, which (within MDK's contract) hears every message stored
// before it: a later step spent on hearing it again did nothing.
#[test]
fn no_step_hears_again_what_a_live_message_already_brought() {
    for family in [Family::Live, Family::Mixed] {
        for index in 0..300 {
            let schedule = generate(&RunName::new(family, 0, index));
            let chat = |msg: usize| schedule.messages[msg].chat;
            let mut stored: Vec<usize> = Vec::new();
            let mut heard: BTreeSet<usize> = BTreeSet::new();
            for (at, step) in schedule.steps.iter().enumerate() {
                match *step {
                    Action::Store { msg } => stored.push(msg),
                    Action::Hear { msg } | Action::Ask { msg } => {
                        assert!(
                            !heard.contains(&msg),
                            "{}: step {at} hears m{msg} again",
                            schedule.name
                        );
                        let upto = stored.iter().position(|&m| m == msg).expect("stored");
                        for &m in &stored[..=upto] {
                            if chat(m) == chat(msg) {
                                heard.insert(m);
                            }
                        }
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
