//! The simulator finds each bug fixed before, put back (`verification.md`: "A check must be
//! shown to catch its bug"). Each is found within a bound of runs of the family that exercises
//! it, for the property its fix restored, and its capsule replays exactly.

#![cfg(feature = "bugs")]

use marmot_agent_sim::{BugSet, Capsule, Family, RunName, generate, run, shrink};

/// Runs `family` with `bugs` until a run fails; gives the property and the run's index.
fn found(family: Family, bugs: BugSet, within: u32) -> (String, u32) {
    for index in 0..within {
        let mut schedule = generate(&RunName::new(family, 0, index));
        schedule.bugs = bugs;
        let report = run(&schedule);
        if let Some(failed) = &report.failure {
            let (small, small_report) = shrink(&schedule, &report, 100);
            let capsule = Capsule::new(small, &small_report).expect("it fails");
            capsule.replay().expect("the capsule replays exactly");
            return (failed.failure.property.clone(), index);
        }
    }
    panic!("not found within {within} runs of {family}");
}

#[test]
fn save_after_send_is_found() {
    let bugs = BugSet {
        save_after_send: true,
        ..BugSet::default()
    };
    let (property, index) = found(Family::Crash, bugs, 40);
    assert!(
        ["S1", "S2", "S3", "L1", "L2"].contains(&property.as_str()),
        "{property} at {index}"
    );
}

#[test]
fn retry_ambiguous_blindly_is_found() {
    let bugs = BugSet {
        retry_ambiguous_blindly: true,
        ..BugSet::default()
    };
    let (property, index) = found(Family::Sends, bugs, 40);
    assert!(
        ["S1", "S3"].contains(&property.as_str()),
        "{property} at {index}"
    );
}

#[test]
fn settles_only_from_items_is_found() {
    let bugs = BugSet {
        settles_only_from_items: true,
        ..BugSet::default()
    };
    let (property, index) = found(Family::Calm, bugs, 40);
    assert_eq!(property, "S1", "at {index}");
}
