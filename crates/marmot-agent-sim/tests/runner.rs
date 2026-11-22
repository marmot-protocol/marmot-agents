//! The runner's own checks fail a run (`verification.md`: "A check must be shown to catch its
//! bug"): a core whose task panics or ends when it should not, and a harness whose work never
//! ends, each made by hand ([`Faults`]), fail the run with its trace; without the fault the same
//! schedule passes.

use marmot_agent_sim::{
    Action, BugSet, Family, Faults, Outcome, Posted, Report, RunName, Schedule, What, run,
};

/// One chat, one prompt that reaches the harness, and then `then`.
fn one_prompt(then: &[Action], faults: Faults) -> Schedule {
    let mut steps = vec![
        Action::Invite { chat: 0 },
        Action::Store { msg: 0 },
        Action::Hear { msg: 0 },
    ];
    steps.extend_from_slice(then);
    Schedule {
        name: RunName::new(Family::Calm, 0, 0),
        chats: 1,
        bugs: BugSet::default(),
        faults,
        messages: vec![Posted {
            chat: 0,
            what: What::Prompt { approval: false },
        }],
        steps,
    }
}

fn property(report: &Report) -> Option<(String, Option<usize>)> {
    (report.failure.as_ref()).map(|f| (f.failure.property.clone(), f.step))
}

#[test]
fn the_schedules_pass_without_their_fault() {
    for then in [&[][..], &[Action::Stop, Action::Restart][..]] {
        let report = run(&one_prompt(then, Faults::default()));
        assert_eq!(property(&report), None, "{:?}", report.failure);
        assert!(report.stats.delivered > 0);
    }
}

// The old runner noted the panic as the core going down, and passed the run once everything was
// closed.
#[test]
fn a_core_that_panics_as_it_stops_at_the_end_fails() {
    let faults = Faults {
        panic_when_stopped: true,
        ..Faults::default()
    };
    let report = run(&one_prompt(&[], faults));
    assert_eq!(property(&report), Some(("panic".into(), None)));
    assert!(report.trace.iter().any(|t| t.what == "Down(panicked)"));
}

// The old runner restarted the core in the fair part and passed the run.
#[test]
fn a_core_that_panics_mid_run_fails_at_that_step() {
    let faults = Faults {
        panic_when_stopped: true,
        ..Faults::default()
    };
    let report = run(&one_prompt(&[Action::Stop, Action::Restart], faults));
    assert_eq!(property(&report), Some(("panic".into(), Some(3))));
}

// The old runner found nothing left by L1 to L3 at the bound, returned no failure, and skipped
// the last stop.
#[test]
fn harness_work_left_at_the_bound_fails() {
    let faults = Faults {
        harness_stalls: true,
        ..Faults::default()
    };
    let report = run(&one_prompt(&[], faults));
    let failed = report
        .failure
        .expect("work left at the bound fails the run");
    assert_eq!(failed.failure.property, "L2");
    assert!(failed.failure.detail.contains("harness"), "{failed:?}");
    // The last stop still ran.
    assert!(report.trace.iter().rev().any(|t| t.what == "Stop"));
}

// Review round 2. The old `Hang` queued behind the moves already armed: after a `Refuse`, the
// schedule's prompt met the refusal, the crash that follows a `Hang` killed a core that held
// nothing, and the restart threw the hang away unused.
#[test]
fn a_hang_after_a_queued_refusal_still_hangs_the_next_request() {
    let schedule = one_prompt_after(
        &[Action::Refuse, Action::Hang],
        &[Action::Crash { after: 0 }],
    );
    let report = run(&schedule);
    assert_eq!(property(&report), None, "{:?}", report.failure);
    let hung = (report.trace.iter()).any(|t| t.what == "Deliver(prompt m0), hangs");
    assert!(hung, "{:#?}", report.trace);
}

// Review round 2. The old fair part ended once everything was settled, with a send still to land:
// a send armed to land 90 s late went out again after the core looked for it, the turn ended, and
// the run passed before the first copy landed as the duplicate the `late` family looks for.
#[test]
fn the_fair_part_waits_for_a_late_send_to_land() {
    let schedule = Schedule {
        name: RunName::new(Family::Late, 0, 0),
        ..one_prompt_after(
            &[Action::Send {
                outcome: Outcome::Late { ms: 90_000 },
            }],
            &[Action::Emit {
                pick: 0,
                force: false,
            }],
        )
    };
    let report = run(&schedule);
    let landed = (report.trace.iter()).any(|t| t.what.starts_with("Send(landed late"));
    assert!(landed, "{:#?}", report.trace);
    assert_eq!(
        report.failure.map(|f| f.failure.property),
        Some("S3".into())
    );
}

/// One chat, `before`, then one prompt stored and heard, then `then`.
fn one_prompt_after(before: &[Action], then: &[Action]) -> Schedule {
    let mut steps = vec![Action::Invite { chat: 0 }];
    steps.extend_from_slice(before);
    steps.extend([Action::Store { msg: 0 }, Action::Hear { msg: 0 }]);
    steps.extend_from_slice(then);
    Schedule {
        steps,
        ..one_prompt(&[], Faults::default())
    }
}
