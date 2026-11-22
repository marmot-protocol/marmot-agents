//! A run is reproducible: the same schedule gives the same trace, a capsule replays exactly, and
//! the shrinker cuts a failing schedule down to one that fails the same way.

use marmot_agent_sim::{Capsule, Family, RunName, generate, run, shrink};

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

#[test]
fn a_name_always_makes_the_same_schedule() {
    let name = RunName::new(Family::Mixed, 7, 3);
    assert_eq!(generate(&name), generate(&name));
    assert_ne!(
        generate(&name),
        generate(&RunName::new(Family::Mixed, 7, 4))
    );
}

// The accepted limit (`verification.md`, "What a restart still cannot recover"): a send whose
// outcome is not known and that lands only after the core looked for it goes out twice. With the
// assumption that a send settles within the core's wait switched off (the `late` family), the
// simulator finds the duplicate; the capsule replays it exactly and shrinks.
#[test]
fn the_accepted_limit_is_found_when_its_assumption_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let found = (0..60).find_map(|index| {
        let schedule = generate(&RunName::new(Family::Late, 0, index));
        let report = run(&schedule);
        report.failure.is_some().then_some((schedule, report))
    });
    let (schedule, report) = found.expect("a late landing makes a duplicate within 60 runs");
    let property = report.failure.as_ref().unwrap().failure.property.clone();
    assert!(["S1", "S3"].contains(&property.as_str()), "{property}");
    let (small, small_report) = shrink(&schedule, &report, 300);
    assert!(small.steps.len() < schedule.steps.len());
    let capsule = Capsule::new(small, &small_report).unwrap();
    let path = capsule.write(dir.path()).unwrap();
    let read = Capsule::read(&path).unwrap();
    read.replay().expect("the capsule replays exactly");
    assert_eq!(read.failed.failure.property, property);
}
