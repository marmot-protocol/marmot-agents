//! The Codex driver against the conformance suite, on the fake `codex app-server`.

use marmot_agent_codex::{Codex, Setup};
use marmot_agent_conformance::fake::{
    Fake, Pending, Progress, Protocol, Replay, Tap, codex, injector,
};
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
    /// For a replay: how far it got.
    progress: std::sync::Arc<Progress>,
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
            Some(text) => Box::new(Replay::new(text, codex).counting(self.progress.clone())),
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

// One test for each scenario, made from the suite's one list of them.
macro_rules! conformance {
    ($make:expr; $($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let found = scenarios::$name($make).await;
                found.unwrap_or_else(|v| panic!("{v}"));
            }
        )*
    };
}

marmot_agent_conformance::with_scenarios!(conformance!(OnFake::default()));

#[tokio::test]
async fn dies_at_every_step() {
    let found = scenarios::dies_at_every_step(OnFake::default).await;
    found.unwrap_or_else(|v| panic!("{v}"));
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
        let progress = std::sync::Arc::new(Progress::default());
        let subject = OnFake {
            recording: Some(recording(name)),
            progress: progress.clone(),
            ..OnFake::default()
        };
        let found = scenarios::replay(subject, "as recorded").await;
        found.unwrap_or_else(|v| panic!("{name}: {v}"));
        let left = progress.left();
        assert_eq!(left, 0, "{name}: {left} recorded messages never played");
    }
}

// Each deliberate fault is caught by the scenario meant for it, and as the property it breaks, but
// those only a harness for each chat can show (Pi's test runs them).
#[tokio::test]
async fn every_fault_is_caught() {
    let mut missed = Vec::new();
    for fault in Fault::ALL
        .into_iter()
        .filter(|f| !f.needs_a_harness_per_chat())
    {
        let subject = OnFake::default();
        let inject = injector(&subject.pending);
        let found = faults::caught(subject, fault, Some(inject)).await;
        if !fault.caught_as(&found) {
            missed.push(format!("{fault:?}: {found:?}"));
        }
    }
    assert!(
        missed.is_empty(),
        "not caught as they should be: {missed:#?}"
    );
}

// A replay plays on only when the driver answers as recorded: refused, the recording that allowed
// stops at the approval, and the turn never ends.
#[tokio::test]
async fn a_replay_stops_on_another_answer() {
    let progress = std::sync::Arc::new(Progress::default());
    let subject = OnFake {
        recording: Some(recording("approval_allowed.jsonl")),
        progress: progress.clone(),
        ..OnFake::default()
    };
    let found = scenarios::replay_refused(subject, "as recorded", &progress).await;
    found.unwrap_or_else(|wrong| panic!("{wrong}"));
}
