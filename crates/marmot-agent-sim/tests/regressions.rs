//! Runs the nightly campaign found failing, cut down to the steps that matter and kept here so
//! every test run covers them. Each names the run it came from and the issue.

use marmot_agent_sim::{
    Action, BugSet, Family, Faults, Posted, Report, RunName, Schedule, What, run,
};

fn property(report: &Report) -> Option<(String, String)> {
    (report.failure.as_ref()).map(|f| (f.failure.property.clone(), f.failure.detail.clone()))
}

// `driver/v2/45539714087/3755` (#198). The harness showed a setting's effect in time, while the
// core waited on the driver to take a slow request; by the time the driver answered, the net's
// deadline had passed, and the core's loop took its tick before the driver's event, which was
// already waiting: it settled the setting as having no effect, and then sent the effect (S1).
#[test]
fn an_effect_shown_while_the_core_waits_on_the_driver_is_not_settled_by_the_net() {
    let messages = vec![
        Posted {
            chat: 0,
            what: What::Prompt { approval: false },
        },
        // m1: shown by the session's item (`take_control`, its message number modulo 3).
        Posted {
            chat: 0,
            what: What::Set,
        },
        Posted {
            chat: 0,
            what: What::Prompt { approval: false },
        },
    ];
    // Each schedule waits a different time before the slow request, so the tick and the driver's
    // event are ready together at each place in the loop's turn.
    for wait in [110_000, 111_000, 112_000, 113_000, 114_000, 115_000] {
        let steps = vec![
            Action::Invite { chat: 0 },
            Action::Store { msg: 0 },
            Action::Hear { msg: 0 },
            // The session starts, so the setting has a session to change.
            Action::Emit {
                pick: 0,
                force: false,
            },
            Action::Store { msg: 1 },
            Action::Ask { msg: 1 },
            Action::Tick { ms: wait },
            // The next request holds the core for 20 s, past the net's deadline.
            Action::Slow { ms: 20_000 },
            Action::Store { msg: 2 },
            Action::Hear { msg: 2 },
            // Meanwhile, before the net could fire, the harness shows the setting's effect: the
            // second of the two things it has to emit, the turn and the effect.
            Action::Emit {
                pick: 1,
                force: false,
            },
        ];
        let schedule = Schedule {
            name: RunName::new(Family::Driver, 0, 0),
            chats: 1,
            bugs: BugSet::default(),
            faults: Faults::default(),
            messages: messages.clone(),
            steps,
        };
        let report = run(&schedule);
        let effect = (report.trace.iter()).any(|t| t.what.contains("settles"));
        assert!(effect, "wait {wait}: no effect shown\n{:#?}", report.trace);
        assert_eq!(property(&report), None, "wait {wait}\n{:#?}", report.trace);
    }
}
