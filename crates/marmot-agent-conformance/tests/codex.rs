//! The Codex driver against the conformance suite, on the fake `codex app-server`.

use marmot_agent_codex::{Codex, Setup};
use marmot_agent_conformance::fake::{
    Fake, Pending, Progress, Protocol, Replay, Tap, codex, injector,
};
use marmot_agent_conformance::faults::{self, Fault};
use marmot_agent_conformance::run::{turn_ended, waiting};
use marmot_agent_conformance::{Run, Subject, scenarios, subagents};
use marmot_agent_driver::ChatId;
use marmot_agent_events::State;
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

    fn sends_files(&self) -> bool {
        true
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

// Codex starts subagents: each scenario for them, one test each. This is the one list of them: a
// scenario added to `src/subagents.rs` is added here too.
macro_rules! subagent_scenarios {
    ($($name:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                let found = subagents::$name(OnFake::default()).await;
                found.unwrap_or_else(|v| panic!("{v}"));
            }
        )*
    };
}

subagent_scenarios!(
    subagent_in_its_parents_chat,
    subagent_speaks_first,
    nested_subagents,
    subagent_asks_for_approval,
    stop_stops_every_subagent,
    stop_after_the_turn_has_ended,
    stop_stops_nested_subagents,
    dies_with_subagents_running,
    new_session_with_subagents_running,
    restart_with_subagents_running,
);

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

// Every recording of subagents that one prompt makes, its approvals allowed as recorded, played
// back through the driver: every recorded message is played, nothing is routed nowhere but
// Codex's own bookkeeping, and the chat shows each subagent as one `task` item, with all it did
// naming that item, as the fake's do (D-117, D-121).
#[tokio::test]
async fn replays_subagent_recordings() {
    for (name, count) in [
        ("subagent.jsonl", 1),
        ("subagent_v1.jsonl", 1),
        ("subagent_nested.jsonl", 2),
        ("subagent_approval.jsonl", 1),
    ] {
        let progress = std::sync::Arc::new(Progress::default());
        let subject = OnFake {
            recording: Some(recording(&format!("subagents/{name}"))),
            progress: progress.clone(),
            ..OnFake::default()
        };
        let mut run = Run::start(subject).await;
        let m = run.prompt("c-1", "as recorded").await.unwrap().unwrap();
        loop {
            let e = (run.until("L2", "the recorded turn's end", |e| {
                waiting(e) || (turn_ended(e) && e.prompts.contains(&m))
            }))
            .await
            .unwrap_or_else(|v| panic!("{name}: {v}"));
            if !waiting(&e) {
                break;
            }
            run.answer(&e, true).await.unwrap().1.unwrap();
        }
        run.drain(std::time::Duration::from_millis(100)).await;
        run.check.all_settled();
        run.ok().unwrap_or_else(|v| panic!("{name}: {v}"));
        assert_eq!(progress.left(), 0, "{name}: recorded messages never played");
        let found = subagents::shown_flat(&run.seen, "c-1", count, State::Done);
        found.unwrap_or_else(|v| panic!("{name}: {v}"));
        if count == 2 {
            let found = subagents::nested_once(&run.seen, "c-1");
            found.unwrap_or_else(|v| panic!("{name}: {v}"));
        }
        // What is routed nowhere is what names no thread, after Codex's greeting: the account,
        // rate limits and remote control. (The recorder's moderation notes name a redacted
        // thread, and are held as a thread not yet named.)
        let text = recording(&format!("subagents/{name}"));
        let threadless = (text.lines())
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .skip_while(|r| r["msg"]["id"] != 1 || r["dir"] != "in")
            .filter(|r| r["dir"] == "in" && r["msg"].get("method").is_some())
            .filter(|r| {
                let p = &r["msg"]["params"];
                p["threadId"].is_null() && p["thread"]["id"].is_null()
            })
            .count() as u64;
        assert_eq!(run.driver.unrouted(), threadless, "{name}");
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
