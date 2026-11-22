//! The Pi driver against the conformance suite. Pi's driver starts a program for each chat; here
//! that program is the relay, and each chat's Pi is a fake in this test program.

use marmot_agent_conformance::fake::pi::Server;
use marmot_agent_conformance::fake::{Fake, Progress};
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
        let mut pi = Pi::start(Setup { program, args, cwd })
            .await
            .expect("the relay answers --version");
        // The suite waits five seconds for a request; the driver gives up on a Pi sooner.
        pi.set_patience(std::time::Duration::from_secs(2));
        pi
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

    fn sends_files(&self) -> bool {
        true
    }

    fn loose(&self) -> bool {
        self.replay
    }

    // Pi's driver starts each chat's Pi in the chat's folder, so it must be on disk.
    fn other_folder(&self) -> std::path::PathBuf {
        let (_, _, cwd) = self.server.program();
        let other = cwd.join("elsewhere");
        std::fs::create_dir_all(&other).expect("a folder to move to");
        other
    }

    fn chat_of(&self, tag: &str) -> Option<ChatId> {
        self.server.chat_of(tag).map(ChatId)
    }

    fn tag_of(&self, chat: &ChatId) -> Option<String> {
        self.server.tag_of(&chat.0)
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

marmot_agent_conformance::with_scenarios!(conformance!(subject()));

#[tokio::test]
async fn dies_at_every_step() {
    let found = scenarios::dies_at_every_step(subject).await;
    found.unwrap_or_else(|v| panic!("{v}"));
}

// The faults that touch what differs with a harness for each chat: settling when one chat's Pi
// stops, closing what it left, and keeping chats apart, files included.
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
        Fault::SwallowPrompt,
        Fault::FileWrongChat,
        Fault::FileOwnFolder,
        Fault::FileUnanswered,
    ] {
        let found = faults::caught(subject(), fault, None).await;
        if !fault.caught_as(&found) {
            missed.push(format!("{fault:?}: {found:?}"));
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
        let progress = std::sync::Arc::new(Progress::default());
        let server =
            Server::replaying(&fake, &recording, progress.clone()).expect("the fake listens");
        let subject = OnFake {
            fake,
            server,
            replay: true,
        };
        let found = scenarios::replay(subject, "as recorded").await;
        found.unwrap_or_else(|v| panic!("{name}: {v}"));
        let left = progress.left();
        assert_eq!(left, 0, "{name}: {left} recorded messages never played");
    }
}

// A replay plays on only when the driver answers as recorded: answered the other way, the recording
// stops at the dialog, and the turn never ends.
#[tokio::test]
async fn a_replay_stops_on_another_answer() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/recordings/pi/gate_allowed.jsonl");
    let recording = std::fs::read_to_string(path).expect("the recording");
    let fake = Fake::new();
    let progress = std::sync::Arc::new(Progress::default());
    let server = Server::replaying(&fake, &recording, progress.clone()).expect("the fake listens");
    let subject = OnFake {
        fake,
        server,
        replay: true,
    };
    let found = scenarios::replay_refused(subject, "as recorded", &progress).await;
    found.unwrap_or_else(|wrong| panic!("{wrong}"));
}
