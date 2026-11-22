//! What the generator makes, and the campaign that runs it.

use marmot_agent_sim::{Action, BugSet, Family, RunName, campaign, generate};
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
