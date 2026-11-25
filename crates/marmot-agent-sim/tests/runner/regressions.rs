//! Runs the nightly campaign found failing, cut down to the steps that matter and kept here so
//! every test run covers them. Each names the run it came from and the issue.

use marmot_agent_sim::{
    Action, BugSet, Family, Faults, Posted, Report, RunName, Schedule, What, run,
};

fn property(report: &Report) -> Option<(String, String)> {
    (report.failure.as_ref()).map(|f| (f.failure.property.clone(), f.failure.detail.clone()))
}

fn schedule(messages: Vec<Posted>, steps: Vec<Action>) -> Schedule {
    Schedule {
        name: RunName::new(Family::Driver, 0, 0),
        chats: 1,
        bugs: BugSet::default(),
        faults: Faults::default(),
        messages,
        steps,
    }
}

const PROMPT: Posted = Posted {
    chat: 0,
    what: What::Prompt { approval: false },
};

const SET: Posted = Posted {
    chat: 0,
    what: What::Set,
};

// `driver/v2/45539714087/3755` (#198). The harness showed a setting's effect in time, while the
// core waited on the driver to take a slow request; by the time the driver answered, the net's
// deadline had passed, and the core's loop took its tick before the driver's event, which was
// already waiting: it settled the setting as having no effect, and then sent the effect (S1).
#[test]
fn an_effect_shown_while_the_core_waits_on_the_driver_is_not_settled_by_the_net() {
    // m1 is shown by the session's item (`take_control`, its message number modulo 3).
    let messages = vec![PROMPT, SET, PROMPT];
    // The core's clock ticks every 10 s, and each tick is a turn of its loop, which changes which
    // of the things it waits for it looks at first. So each wait, 10 s apart, comes out of the slow
    // request at another place in that turn, the tick before the driver's event or after it.
    for wait in (55_000..=115_000).step_by(10_000) {
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
            // The next request holds the core for 70 s, past the net's deadline.
            Action::Slow { ms: 70_000 },
            Action::Store { msg: 2 },
            Action::Hear { msg: 2 },
            // Meanwhile, before the net could fire, the harness shows the setting's effect: the
            // second of the two things it has to emit, the turn and the effect.
            Action::Emit {
                pick: 1,
                force: false,
            },
        ];
        let emit = steps.len() - 1;
        let report = run(&schedule(messages.clone(), steps));
        let shown = (report.trace.iter()).any(|t| {
            t.step == Some(emit) && t.what.starts_with("Emit(sess-") && t.what.contains("settles")
        });
        assert!(
            shown,
            "wait {wait}: the effect is not shown in time\n{:#?}",
            report.trace
        );
        assert_eq!(property(&report), None, "wait {wait}\n{:#?}", report.trace);
    }
}
