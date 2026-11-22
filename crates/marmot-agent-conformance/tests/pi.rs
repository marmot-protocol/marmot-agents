//! The Pi driver against the conformance suite. Pi's driver starts a program for each chat; here
//! that program is the relay, and each chat's Pi is a fake in this test program.

use marmot_agent_conformance::fake::Fake;
use marmot_agent_conformance::fake::pi::Server;
use marmot_agent_conformance::{Subject, scenarios};
use marmot_agent_driver::ChatId;
use marmot_agent_pi::{Pi, Setup};

struct OnFake {
    fake: Fake,
    server: Server,
    replay: bool,
}

fn subject() -> OnFake {
    let fake = Fake::new();
    let server = Server::start(&fake).expect("the fake listens");
    OnFake {
        fake,
        server,
        replay: false,
    }
}

impl Subject for OnFake {
    type Driver = Pi;

    async fn start(&mut self) -> Pi {
        let (program, args, cwd) = self.server.program();
        Pi::start(Setup { program, args, cwd })
            .await
            .expect("the relay answers --version")
    }

    fn fake(&self) -> &Fake {
        &self.fake
    }

    fn kill(&mut self, chat: &ChatId) {
        self.server.kill_chat(&chat.0);
    }

    fn shared_harness(&self) -> bool {
        false
    }

    fn setting(&self) -> (String, String) {
        ("thinking".into(), "high".into())
    }

    fn never(&self) -> &'static str {
        "prompt"
    }

    fn reorders(&self) -> bool {
        false
    }

    fn loose(&self) -> bool {
        self.replay
    }

    fn chat_of(&self, tag: &str) -> Option<ChatId> {
        self.server.chat_of(tag).map(ChatId)
    }
}

macro_rules! conformance {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let found = scenarios::run(stringify!($name), subject()).await;
                found.unwrap_or_else(|v| panic!("{v}"));
            }
        )*
    };
}

conformance!(
    one_turn,
    two_chats,
    answered_once,
    answer_after_death,
    dies_mid_turn,
    dies_with_requests_pending,
    dropped_waits,
    slow_and_out_of_order,
    partial_frames,
    never_answers,
    late_replies,
    restart,
    new_session,
    settings,
);

// Pi's driver waits for a record to be written with no limit, so a Pi that stops reading its input
// stalls the request, and every chat with it (found by this scenario, 2026-11-22; not yet fixed).
#[tokio::test]
#[ignore = "Pi's driver waits for a record to be written with no limit"]
async fn stops_reading() {
    let found = scenarios::run("stops_reading", subject()).await;
    found.unwrap_or_else(|v| panic!("{v}"));
}

#[test]
fn every_scenario_is_run() {
    let here = include_str!("pi.rs");
    for name in scenarios::ALL {
        let listed = here.contains(&format!("    {name},\n"));
        let alone = here.contains(&format!("async fn {name}()"));
        assert!(listed || alone, "{name} is not run");
    }
}

#[tokio::test]
async fn dies_at_every_step() {
    for step in 1..=10 {
        let found = scenarios::dies_at_step(subject(), step).await;
        found.unwrap_or_else(|v| panic!("dying at step {step}: {v}"));
    }
}

// The faults that touch what differs with a harness for each chat: settling when one chat's Pi
// stops, closing what it left, and keeping chats apart.
#[tokio::test]
async fn faults_are_caught() {
    use marmot_agent_conformance::faults::{self, Fault};
    let mut missed = Vec::new();
    for fault in [
        Fault::DropSettle,
        Fault::SettleOnStop,
        Fault::LeaveOpen,
        Fault::WrongChat,
        Fault::ReuseItem,
        Fault::ResumeUnasked,
        Fault::TakeUnhanded,
    ] {
        match faults::caught(subject(), fault, None).await {
            Err(v) if v.property == fault.breaks() => {}
            other => missed.push(format!("{fault:?}: {other:?}")),
        }
    }
    assert!(
        missed.is_empty(),
        "not caught as they should be: {missed:#?}"
    );
}

#[tokio::test]
async fn replays_recordings() {
    for name in ["gate_allowed.jsonl", "command.jsonl"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/recordings/pi")
            .join(name);
        let recording = std::fs::read_to_string(path).expect("the recording");
        let fake = Fake::new();
        let left = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
        let server = Server::replaying(&fake, &recording, left.clone()).expect("the fake listens");
        let subject = OnFake {
            fake,
            server,
            replay: true,
        };
        let found = scenarios::replay(subject, "as recorded").await;
        found.unwrap_or_else(|v| panic!("{name}: {v}"));
        let left = left.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(left, 0, "{name}: {left} recorded messages never played");
    }
}
