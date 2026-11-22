//! The Codex driver against the conformance suite, on the fake `codex app-server`.

use marmot_agent_codex::{Codex, Setup};
use marmot_agent_conformance::fake::{Fake, Pending, Protocol, Replay, Tap, codex, injector};
use marmot_agent_conformance::faults::{self, Fault};
use marmot_agent_conformance::{Subject, scenarios};
use marmot_agent_driver::ChatId;
use std::path::Path;

/// The pipe between driver and harness holds this many bytes each way, so a long message is
/// written in several steps and a dropped wait can fall in the middle of one.
const PIPE: usize = 256;

#[derive(Default)]
struct OnFake {
    fake: Fake,
    store: codex::Shared,
    recording: Option<String>,
    /// For a replay: how much of the recording is left to play.
    left: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pending: Pending,
}

impl Subject for OnFake {
    type Driver = Codex;

    async fn start(&mut self) -> Codex {
        let (driver_side, fake_side) = tokio::io::duplex(PIPE);
        let (fake_reads, fake_writes) = tokio::io::split(fake_side);
        let (driver_reads, driver_writes) = tokio::io::split(driver_side);
        let codex = codex::Codex::new(self.store.clone());
        let protocol: Box<dyn Protocol> = match &self.recording {
            Some(text) => Box::new(Replay::new(text, codex).counting(self.left.clone())),
            None => Box::new(codex),
        };
        self.fake.serve(fake_reads, fake_writes, protocol, None);
        let tap = Tap::new(driver_writes, self.pending.clone());
        let mut codex = Codex::connect(driver_reads, tap, Setup::new("/work/demo"))
            .await
            .expect("the fake greets the driver");
        // The suite waits five seconds for a request; the driver gives up on Codex sooner.
        codex.set_patience(std::time::Duration::from_secs(2));
        codex
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
        "thread/start"
    }

    fn loose(&self) -> bool {
        self.recording.is_some()
    }
}

macro_rules! conformance {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let found = scenarios::run(stringify!($name), OnFake::default()).await;
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
    stops_reading,
    late_replies,
    restart,
    new_session,
    settings,
);

#[test]
fn every_scenario_is_run() {
    let here = include_str!("codex.rs");
    for name in scenarios::ALL {
        let listed = here.contains(&format!("    {name},\n"));
        let alone = here.contains(&format!("async fn {name}()"));
        assert!(listed || alone, "{name} is not run");
    }
}

#[tokio::test]
async fn dies_at_every_step() {
    for step in 1..=12 {
        let found = scenarios::dies_at_step(OnFake::default(), step).await;
        found.unwrap_or_else(|v| panic!("dying at step {step}: {v}"));
    }
}

fn recording(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/recordings/codex")
        .join(name);
    std::fs::read_to_string(path).expect("the recording")
}

#[tokio::test]
async fn replays_recordings() {
    for name in ["simple.jsonl", "approval_allowed.jsonl", "command.jsonl"] {
        let left = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
        let subject = OnFake {
            recording: Some(recording(name)),
            left: left.clone(),
            ..OnFake::default()
        };
        let found = scenarios::replay(subject, "as recorded").await;
        found.unwrap_or_else(|v| panic!("{name}: {v}"));
        let left = left.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(left, 0, "{name}: {left} recorded messages never played");
    }
}

// Each deliberate fault is caught by the scenario meant for it, and as the property it breaks.
#[tokio::test]
async fn every_fault_is_caught() {
    let mut missed = Vec::new();
    for fault in Fault::ALL {
        let subject = OnFake::default();
        let inject = injector(&subject.pending);
        match faults::caught(subject, fault, Some(inject)).await {
            Err(v) if v.property == fault.breaks() => {}
            other => missed.push(format!("{fault:?}: {other:?}")),
        }
    }
    assert!(
        missed.is_empty(),
        "not caught as they should be: {missed:#?}"
    );
}
