//! The ACP driver against the conformance suite, on a fake agent shaped like each recorded one:
//! OpenCode, Goose and Cursor.

use marmot_agent_acp::Acp;
use marmot_agent_conformance::fake::acp::{self, Flavor};
use marmot_agent_conformance::fake::{Fake, Progress, Protocol, Replay, Tap};
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
    /// For a replay: how far it got.
    progress: std::sync::Arc<Progress>,
}

fn subject(flavor: Flavor) -> OnFake {
    OnFake {
        flavor,
        fake: Fake::new(),
        store: acp::Shared::default(),
        recording: None,
        progress: Default::default(),
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
            Some(text) => Box::new(Replay::new(text, agent).counting(self.progress.clone())),
            None => Box::new(agent),
        };
        self.fake.serve(fake_reads, fake_writes, protocol, None);
        let tap = Tap::new(driver_writes, Default::default());
        let name = format!("{:?}", self.flavor).to_lowercase();
        let mut acp = Acp::connect(driver_reads, tap, "/work/demo", &name)
            .await
            .expect("the fake greets the driver");
        // File sending is switched on, as the settings do with `send_files = true`.
        let served = acp.serve_files().await.expect("the driver serves its tool");
        assert!(served, "every flavour takes MCP servers over HTTP");
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

    fn sends_files(&self) -> bool {
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

// The scenarios a flavour runs, one test each, with the crash sweep. A flavour that runs only some
// names those it leaves out; a check that the two lists name every scenario in the suite's one list,
// once, is made from the same names.
macro_rules! conformance {
    ($flavor:ident: run: [$($name:ident),* $(,)?]; left_out: [$($out:ident),* $(,)?]) => {
        #[allow(non_snake_case)]
        mod $flavor {
            use super::*;
            $(
                #[tokio::test]
                async fn $name() {
                    let found = scenarios::$name(subject(Flavor::$flavor)).await;
                    found.unwrap_or_else(|v| panic!("{v}"));
                }
            )*

            #[tokio::test]
            async fn dies_at_every_step() {
                let found = scenarios::dies_at_every_step(|| subject(Flavor::$flavor)).await;
                found.unwrap_or_else(|v| panic!("{v}"));
            }

            #[test]
            fn every_scenario_is_run_or_left_out() {
                $(let _ = scenarios::$out::<OnFake>;)*
                let found =
                    scenarios::covered(&[$(stringify!($name)),*], &[$(stringify!($out)),*]);
                found.unwrap_or_else(|wrong| panic!("{wrong}"));
            }
        }
    };
    // Every scenario, from the suite's one list.
    ($flavor:ident; $($name:ident),* $(,)?) => {
        conformance!($flavor: run: [$($name),*]; left_out: []);
    };
}

marmot_agent_conformance::with_scenarios!(conformance!(OpenCode));

// Goose and Cursor differ from OpenCode in how a session is taken up again, in Goose's runs of its
// own, in Cursor's to-do list, and in how each keeps the driver's tool server and names the session
// that calls it: they run the scenarios that reach those, and the answer to an agent that has gone. The rest go through the same code as OpenCode's.
conformance!(Goose:
    run: [two_chats, answered_once, answer_after_death, dies_mid_turn, restart, new_session, folder, send_file];
    left_out: [
        one_turn,
        dies_with_requests_pending,
        dropped_waits,
        slow_and_out_of_order,
        partial_frames,
        never_answers,
        stops_reading,
        late_replies,
        settings,
    ]
);

conformance!(Cursor:
    run: [two_chats, answered_once, answer_after_death, restart, send_file];
    left_out: [
        one_turn,
        dies_mid_turn,
        dies_with_requests_pending,
        dropped_waits,
        slow_and_out_of_order,
        partial_frames,
        never_answers,
        stops_reading,
        late_replies,
        new_session,
        folder,
        settings,
    ]
);

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
            let progress = std::sync::Arc::new(Progress::default());
            let subject = OnFake {
                recording: Some(recording(folder, name)),
                progress: progress.clone(),
                ..subject(flavor)
            };
            let found = scenarios::replay(subject, "as recorded").await;
            found.unwrap_or_else(|v| panic!("{folder}/{name}: {v}"));
            let left = progress.left();
            assert_eq!(
                left, 0,
                "{folder}/{name}: {left} recorded messages never played"
            );
        }
    }
}

// A prompt handed to the agent twice is caught. The driver restarts a running turn for a prompt
// that comes during it (`mid_turn: restart`), so the agent takes both copies; one it refused would
// count as well (`tests/fake.rs`).
#[tokio::test]
async fn a_prompt_twice_is_caught() {
    use marmot_agent_conformance::faults::{self, Fault};
    let fault = Fault::PromptTwice;
    let found = faults::caught(subject(Flavor::OpenCode), fault, None).await;
    assert!(
        fault.caught_as(&found),
        "not caught as it should be: {found:?}"
    );
}

// A replay plays on only when the driver answers as recorded: refused, the recording that allowed
// stops at the approval, and the turn never ends.
#[tokio::test]
async fn a_replay_stops_on_another_answer() {
    for (flavor, folder) in [
        (Flavor::OpenCode, "opencode"),
        (Flavor::Goose, "goose"),
        (Flavor::Cursor, "cursor"),
    ] {
        let progress = std::sync::Arc::new(Progress::default());
        let subject = OnFake {
            recording: Some(recording(folder, "approval_allowed.jsonl")),
            progress: progress.clone(),
            ..subject(flavor)
        };
        let found = scenarios::replay_refused(subject, "as recorded", &progress).await;
        found.unwrap_or_else(|wrong| panic!("{folder}: {wrong}"));
    }
}

// A file handed over for the wrong chat, from the wrong folder, or a result that never reaches the
// agent, is caught on every flavour: each keeps the driver's tool server and names the calling
// session its own way.
#[tokio::test]
async fn the_file_faults_are_caught() {
    use marmot_agent_conformance::faults::{self, Fault};
    for flavor in [Flavor::OpenCode, Flavor::Goose, Flavor::Cursor] {
        for fault in [
            Fault::FileWrongChat,
            Fault::FileToWrongChat,
            Fault::FileOwnFolder,
            Fault::FileUnanswered,
            Fault::LoseFileResult,
        ] {
            let found = faults::caught(subject(flavor), fault, None).await;
            assert!(
                fault.caught_as(&found),
                "{flavor:?}, {fault:?}: not caught as it should be: {found:?}"
            );
        }
    }
}
