//! The gate: a fixed set of seeds for every family that must pass. `MARMOT_AGENT_SIM_RUNS`
//! sets how many runs each family makes (the pull-request job sets the normal count; the
//! nightly one more, from another seed with `MARMOT_AGENT_SIM_SEED`).

use marmot_agent_sim::{BugSet, Family, campaign};
use std::path::PathBuf;

fn runs() -> u32 {
    std::env::var("MARMOT_AGENT_SIM_RUNS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(20)
}

fn seed() -> u64 {
    std::env::var("MARMOT_AGENT_SIM_SEED")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Where schedules and capsules go: kept after the test for a failure to be replayed.
fn out() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("marmot-agent-sim")
}

/// Runs the family's seeds, which must all pass, and checks that each of `expected` happened in
/// them: a family whose faults stopped happening would pass empty.
fn gate(family: Family, expected: &[&str]) {
    let done = campaign(
        family,
        seed(),
        0,
        runs(),
        BugSet::default(),
        Some(&out()),
        300,
    )
    .expect("the campaign runs");
    // The failures first: a run that stopped short may be why a fault was not seen.
    let failures: Vec<String> = (done.failures.iter())
        .map(|(name, failed, path)| {
            format!(
                "{name}: {} {} (step {:?}), capsule {:?}",
                failed.failure.property, failed.failure.detail, failed.step, path
            )
        })
        .chain(done.unwritten.iter().cloned())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(done.runs, runs());
    for what in expected {
        assert!(
            done.seen.get(what).copied().unwrap_or(0) > 0,
            "{family}: no {what} in {} runs; seen {:?}",
            done.runs,
            done.seen
        );
    }
}

#[test]
fn calm() {
    gate(Family::Calm, &["harness takes"]);
}

#[test]
fn crash() {
    gate(
        Family::Crash,
        &[
            "crash before a save",
            "crash before a send",
            "crash before a delivery",
            "crash just after a send went",
            "save fails",
            "lookup found",
            "live event lost while down",
            "catch-up that reads something",
        ],
    );
}

#[test]
fn sends() {
    gate(
        Family::Sends,
        &[
            "send busy",
            "send refused",
            "send ambiguous, not gone",
            "send ambiguous, gone",
            "lookup found",
            "lookup absent",
            "lookup error",
        ],
    );
}

#[test]
fn live() {
    gate(Family::Live, &["lag", "catch-up that reads something"]);
}

#[test]
fn driver() {
    gate(
        Family::Driver,
        &[
            "harness refuses",
            "harness slow",
            "harness hangs",
            "driver stops",
        ],
    );
}

#[test]
fn mixed() {
    gate(
        Family::Mixed,
        &[
            "crash before a save",
            "send ambiguous, gone",
            "lag",
            "driver stops",
        ],
    );
}
