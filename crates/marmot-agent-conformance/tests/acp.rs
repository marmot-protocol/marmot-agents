//! The ACP driver against the conformance suite, on a fake agent shaped like each recorded one:
//! OpenCode, Goose and Cursor.

use marmot_agent_acp::Acp;
use marmot_agent_conformance::fake::acp::{self, Flavor};
use marmot_agent_conformance::fake::{Fake, Protocol, Replay, Tap};
use marmot_agent_conformance::{Subject, scenarios};
use marmot_agent_driver::ChatId;
use std::path::Path;
use std::time::Duration;

/// The pipe between driver and harness holds this many bytes each way.
const PIPE: usize = 256;

struct OnFake {
    flavor: Flavor,
    fake: Fake,
    store: acp::Shared,
    recording: Option<String>,
}

fn subject(flavor: Flavor) -> OnFake {
    OnFake {
        flavor,
        fake: Fake::new(),
        store: acp::Shared::default(),
        recording: None,
    }
}

impl Subject for OnFake {
    type Driver = Acp;

    async fn start(&mut self) -> Acp {
        let (driver_side, fake_side) = tokio::io::duplex(PIPE);
        let (fake_reads, fake_writes) = tokio::io::split(fake_side);
        let (driver_reads, driver_writes) = tokio::io::split(driver_side);
        let agent = acp::Acp::new(self.flavor, self.store.clone());
        let protocol: Box<dyn Protocol> = match &self.recording {
            Some(text) => Box::new(Replay::new(text, agent)),
            None => Box::new(agent),
        };
        self.fake.serve(fake_reads, fake_writes, protocol, None);
        let tap = Tap::new(driver_writes, Default::default());
        let name = format!("{:?}", self.flavor).to_lowercase();
        let mut acp = Acp::connect(driver_reads, tap, "/work/demo", &name)
            .await
            .expect("the fake greets the driver");
        // The suite waits five seconds for a request; the driver gives up on the agent sooner.
        acp.set_patience(Duration::from_secs(2));
        acp
    }

    fn fake(&self) -> &Fake {
        &self.fake
    }

    fn kill(&mut self, _chat: &ChatId) {
        self.fake.kill();
    }

    fn shared_harness(&self) -> bool {
        true
    }

    fn setting(&self) -> (String, String) {
        ("model".into(), "m2".into())
    }

    fn never(&self) -> &'static str {
        "session/new"
    }

    fn loose(&self) -> bool {
        self.recording.is_some()
    }
}

macro_rules! conformance {
    ($flavor:ident: $($name:ident),* $(,)?) => {
        #[allow(non_snake_case)]
        mod $flavor {
            use super::*;
            $(
                #[tokio::test]
                async fn $name() {
                    let found = scenarios::run(stringify!($name), subject(Flavor::$flavor)).await;
                    found.unwrap_or_else(|v| panic!("{v}"));
                }
            )*

            #[tokio::test]
            async fn dies_at_every_step() {
                for step in 1..=10 {
                    let found = scenarios::dies_at_step(subject(Flavor::$flavor), step).await;
                    found.unwrap_or_else(|v| panic!("dying at step {step}: {v}"));
                }
            }
        }
    };
}

conformance!(OpenCode:
    one_turn,
    two_chats,
    answered_once,
    dies_mid_turn,
    dies_with_requests_pending,
    dropped_waits,
    slow_and_out_of_order,
    partial_frames,
    never_answers,
    restart,
    new_session,
    settings,
);

// Goose and Cursor differ from OpenCode in how a session is taken up again, in Goose's runs of its
// own, and in Cursor's to-do list: the scenarios that reach those.
conformance!(Goose:
    two_chats,
    answered_once,
    dies_mid_turn,
    restart,
    new_session,
);

conformance!(Cursor:
    two_chats,
    answered_once,
    restart,
);

// Found a bug in the driver: an answer written after the agent had gone was shown as given.
#[tokio::test]
async fn answer_after_death() {
    for flavor in [Flavor::OpenCode, Flavor::Goose, Flavor::Cursor] {
        let found = scenarios::run("answer_after_death", subject(flavor)).await;
        found.unwrap_or_else(|v| panic!("{flavor:?}: {v}"));
    }
}

#[test]
fn every_scenario_is_run() {
    let here = include_str!("acp.rs");
    for name in scenarios::ALL {
        let listed = here.contains(&format!("    {name},\n"));
        let alone = here.contains(&format!("async fn {name}()"));
        assert!(listed || alone, "{name} is not run");
    }
}

fn recording(folder: &str, name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/recordings/acp")
        .join(folder)
        .join(name);
    std::fs::read_to_string(path).expect("the recording")
}

#[tokio::test]
async fn replays_recordings() {
    for (flavor, folder) in [
        (Flavor::OpenCode, "opencode"),
        (Flavor::Goose, "goose"),
        (Flavor::Cursor, "cursor"),
    ] {
        for name in ["simple.jsonl", "approval_allowed.jsonl"] {
            let subject = OnFake {
                recording: Some(recording(folder, name)),
                ..subject(flavor)
            };
            let found = scenarios::replay(subject, "as recorded").await;
            found.unwrap_or_else(|v| panic!("{folder}/{name}: {v}"));
        }
    }
}
