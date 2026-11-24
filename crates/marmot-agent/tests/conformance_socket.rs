//! The socket connectors against the driver conformance suite (`crates/marmot-agent-conformance`,
//! "the socket connectors"): the Hermes plugin (`python/marmot_hermes`), the Claude Code driver
//! (`packages/claude-code`) and the OpenClaw plugin (`packages/openclaw`), each the shipped code on
//! the core's real socket, as [`SocketDriver`], with a scripted harness inside it in place of the real
//! one. No model, no key and no network.
//!
//! Each connector is started from its package's conformance entry (`python/tests/conformance_harness.py`,
//! `packages/*/test/conformance-harness.ts`), which reports on its standard output what its harness
//! heard and each call the connector made on it, and ends when its standard input closes, as when
//! the test goes. The connection runs
//! through a relay of the test's, which holds the connector's responses back when a scenario says,
//! keeps what was in flight, and drops the connection on the scenario's word, so a drop after
//! delivery and before the response is one. A restart drops the connection, as the core's restart
//! does: Hermes and OpenClaw connect again, as they do; Claude Code's connector ends with its
//! connection, and the next is started, as the core starts it.
//!
//! It needs Python 3.14 and Node 24, with `npm ci` run in both packages, so it runs in CI's own job
//! for it, not with the core's other tests:
//!
//! ```sh
//! cargo test -p marmot-agent --test conformance_socket -- --ignored
//! ```

use marmot_agent::socket::{Endpoint, SocketDriver};
use marmot_agent_conformance::fake::{Dropped, Fake, Heard, Unanswered};
use marmot_agent_conformance::{Subject, scenarios, socket};
use marmot_agent_driver::ChatId;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, Command};

/// How long a connector has to take or refuse a request: shorter than the suite's patience, so a
/// connector that never answers is given up before the suite blames the wait.
const ANSWER_WITHIN: Duration = Duration::from_secs(2);
/// How long a connector has to connect, from its start or after a connection dropped.
const CONNECT_WITHIN: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
enum Connector {
    Hermes,
    ClaudeCode,
    OpenClaw,
}

impl Connector {
    /// The program that runs the connector on its scripted harness, and the folder it runs in.
    fn program(self) -> (Command, PathBuf) {
        let top = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (folder, mut command) = match self {
            Self::Hermes => {
                let mut c = Command::new("python3");
                c.args(["-u", "tests/conformance_harness.py"]);
                (top.join("python"), c)
            }
            Self::ClaudeCode => {
                let mut c = Command::new("node");
                c.arg("test/conformance-harness.ts");
                (top.join("packages/claude-code"), c)
            }
            Self::OpenClaw => {
                let mut c = Command::new("node");
                c.arg("test/conformance-harness.ts");
                (top.join("packages/openclaw"), c)
            }
        };
        command.current_dir(&folder);
        (command, folder)
    }

    /// Whether the connector ends when its connection does, so the next connection is a new
    /// connector. Hermes and OpenClaw run in their gateway, which goes on, and connect again.
    fn ends_with_connection(self) -> bool {
        matches!(self, Self::ClaudeCode)
    }
}

/// The connector's process, while it runs.
struct Process {
    child: Child,
    /// Held open: the connector's entry ends when it closes.
    _input: tokio::process::ChildStdin,
    /// Cleared once its output closes.
    alive: Arc<AtomicBool>,
    /// Set when the suite ends it on purpose: it ending then is no death.
    expected: Arc<AtomicBool>,
}

/// One connector, on its scripted harness, as the suite's subject.
struct OnSocket {
    connector: Connector,
    fake: Fake,
    home: tempfile::TempDir,
    /// Where the scripted harness keeps what outlives its process, as a harness keeps its sessions.
    state: tempfile::TempDir,
    process: Option<Process>,
    listener: Option<UnixListener>,
    /// Both ends of the relay of the connection there is, so a restart drops it.
    current: Option<Ends>,
    /// How long the relay waits, once it has held a response back, before it says so, and so before
    /// a scenario drops the connection: none but in the test that shows a drop too late to decide
    /// the request is caught.
    withheld_late: Duration,
}

/// The sockets of one connection, by its relay: the connector's, and the one towards the core.
/// Each relay has its own, so one that ends late closes only its own connection, never the next.
type Ends = Arc<Mutex<Vec<std::os::unix::net::UnixStream>>>;

/// A request of the core's on its way, as the relay keeps it until its response passes.
struct Pending {
    id: Value,
    method: String,
    /// The prompt's message, or the answer's or the control's request.
    key: String,
    /// The prompt's chat.
    chat: Option<String>,
    withheld: bool,
    shown: bool,
}

impl OnSocket {
    fn new(connector: Connector) -> Self {
        Self {
            connector,
            fake: Fake::new(),
            home: tempfile::Builder::new().prefix("mac-").tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
            process: None,
            listener: None,
            current: None,
            withheld_late: Duration::ZERO,
        }
    }

    fn running(&self) -> bool {
        (self.process.as_ref()).is_some_and(|p| p.alive.load(Ordering::SeqCst))
    }

    /// Drops a connection, as the core's end going.
    fn drop_connection(ends: &Ends) {
        for stream in ends.lock().unwrap().drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Carries the connection between the connector and the core, line by line, until either end
    /// goes or the scenario drops it: holding the connector's responses back while the knob says,
    /// and noting which events showed a request's effect (D-147).
    fn relay(
        fake: Fake,
        connector: std::os::unix::net::UnixStream,
        core: std::os::unix::net::UnixStream,
        ends: Ends,
        withheld_late: Duration,
    ) {
        let pending: Arc<Mutex<Vec<Pending>>> = Arc::default();
        let stream = |s: std::os::unix::net::UnixStream| {
            s.set_nonblocking(true).unwrap();
            UnixStream::from_std(s).unwrap()
        };
        let (from_connector, mut to_connector) = stream(connector).into_split();
        let (from_core, mut to_core) = stream(core).into_split();
        let mut drops = fake.drops();
        drops.borrow_and_update();
        let (fake_up, pending_up, ends_up) = (fake.clone(), pending.clone(), ends.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(from_connector).lines();
            loop {
                let line = tokio::select! {
                    line = lines.next_line() => line,
                    _ = drops.changed() => {
                        let in_flight = pending_up.lock().unwrap().iter().map(|p| Unanswered {
                            method: p.method.clone(),
                            withheld: p.withheld,
                            shown: p.shown,
                        }).collect();
                        fake_up.record_dropped(Dropped { unanswered: in_flight });
                        break;
                    }
                };
                let Ok(Some(line)) = line else { break };
                let message: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                if message.get("method").is_none() && message.get("id").is_some() {
                    let withheld = {
                        let mut pending = pending_up.lock().unwrap();
                        let at = pending.iter().position(|p| p.id == message["id"]);
                        match at {
                            Some(at) if fake_up.knobs().withhold_responses => {
                                pending[at].withheld = true;
                                true
                            }
                            Some(at) => {
                                pending.remove(at);
                                false
                            }
                            None => false,
                        }
                    };
                    if withheld {
                        tokio::time::sleep(withheld_late).await;
                        fake_up.record_withheld();
                        continue;
                    }
                }
                if message["method"] == "event" {
                    let event = &message["params"];
                    let names = |field: &str, key: &str| {
                        event[field]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|v| v == key))
                    };
                    for p in pending_up.lock().unwrap().iter_mut() {
                        p.shown |= match p.method.as_str() {
                            "prompt" => {
                                event["type"] == "turn"
                                    && event["chat"].as_str() == p.chat.as_deref()
                                    && names("prompts", &p.key)
                            }
                            _ => names("settles", &p.key),
                        };
                    }
                }
                if to_core
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Self::drop_connection(&ends_up);
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(from_core).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let message: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                let method = message["method"].as_str().unwrap_or_default();
                if matches!(method, "prompt" | "answer" | "control") {
                    let params = &message["params"];
                    let key = if method == "prompt" {
                        &params["message"]
                    } else {
                        &params["request"]
                    };
                    pending.lock().unwrap().push(Pending {
                        id: message["id"].clone(),
                        method: method.to_string(),
                        key: key.as_str().unwrap_or_default().to_string(),
                        chat: params["chat"]["id"].as_str().map(String::from),
                        withheld: false,
                        shown: false,
                    });
                }
                if to_connector
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Self::drop_connection(&ends);
        });
    }

    /// Starts the connector, and the task that reads what its harness reports.
    fn spawn(&mut self) {
        let (mut command, _) = self.connector.program();
        command
            .env("MARMOT_AGENT_HOME", self.home.path())
            .env("MARMOT_CONFORMANCE_STATE", self.state.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .expect("the connector's conformance entry starts");
        let stdout = child.stdout.take().expect("its output");
        let input = child.stdin.take().expect("its input");
        let alive = Arc::new(AtomicBool::new(true));
        let expected = Arc::new(AtomicBool::new(false));
        let fake = self.fake.clone();
        let (still, ended) = (alive.clone(), expected.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    panic!("the connector's entry wrote a line that is not JSON: {line}");
                };
                if let Some(heard) = message.get("heard") {
                    fake.record_heard(heard_of(heard));
                } else if message.get("frame").is_some() {
                    fake.record_frame(None);
                } else {
                    panic!("the connector's entry wrote a line the suite does not know: {line}");
                }
            }
            still.store(false, Ordering::SeqCst);
            if !ended.load(Ordering::SeqCst) {
                fake.record_death(None);
            }
        });
        self.process = Some(Process {
            child,
            _input: input,
            alive,
            expected,
        });
    }
}

/// What a scripted harness reports, as the suite's [`Heard`].
fn heard_of(heard: &Value) -> Heard {
    let string = |v: &Value| v.as_str().map(String::from);
    if let Some(h) = heard.get("prompt") {
        return Heard::Prompt {
            session: string(&h["session"]).unwrap_or_default(),
            text: string(&h["text"]).unwrap_or_default(),
            refused: h["refused"] == true,
        };
    }
    if let Some(h) = heard.get("answer") {
        return Heard::Answer {
            session: string(&h["session"]).unwrap_or_default(),
            allow: h["allow"].as_bool(),
            text: string(&h["text"]),
        };
    }
    if let Some(h) = heard.get("opened") {
        return Heard::Opened {
            session: string(&h["session"]).unwrap_or_default(),
            resumed: h["resumed"] == true,
            folder: string(&h["folder"]),
        };
    }
    if let Some(h) = heard.get("control") {
        return Heard::Control {
            session: string(&h["session"]),
            what: string(&h["what"]).unwrap_or_default(),
        };
    }
    panic!("the connector's entry reported something the suite does not know: {heard}");
}

impl Subject for OnSocket {
    type Driver = SocketDriver;

    async fn start(&mut self) -> SocketDriver {
        // A restart: the core's end of the connection goes, and so does its socket, which the next
        // core binds again. A connector that ends with its connection is let end.
        if let Some(process) = &self.process
            && self.connector.ends_with_connection()
        {
            process.expected.store(true, Ordering::SeqCst);
        }
        if let Some(ends) = self.current.take() {
            Self::drop_connection(&ends);
        }
        self.listener = None;
        if self.connector.ends_with_connection() {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while self.running() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let (listener, token) = Endpoint::in_home(self.home.path())
            .listen()
            .expect("the socket opens");
        if !self.running() {
            self.spawn();
        }
        let (stream, _) = tokio::time::timeout(CONNECT_WITHIN, listener.accept())
            .await
            .unwrap_or_else(|_| panic!("{:?} did not connect", self.connector))
            .expect("the connection is taken");
        self.listener = Some(listener);
        let connector = stream
            .into_std()
            .expect("the connection, as a plain socket");
        let (core, relayed) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
        let handles = [&connector, &relayed].map(|s| s.try_clone().expect("a second handle on it"));
        let ends: Ends = Arc::new(Mutex::new(handles.into()));
        self.current = Some(ends.clone());
        Self::relay(
            self.fake.clone(),
            connector,
            relayed,
            ends,
            self.withheld_late,
        );
        core.set_nonblocking(true).unwrap();
        let stream = UnixStream::from_std(core).expect("the core's end, on the runtime");
        SocketDriver::greet(stream, &token, "Agent")
            .await
            .unwrap_or_else(|e| panic!("{:?} gave no good hello: {e}", self.connector))
            .answer_within(ANSWER_WITHIN)
    }

    fn fake(&self) -> &Fake {
        &self.fake
    }

    fn kill(&mut self, _chat: &ChatId) {
        if let Some(process) = &mut self.process {
            let _ = process.child.start_kill();
        }
    }

    fn shared_harness(&self) -> bool {
        true
    }

    fn setting(&self) -> (String, String) {
        ("model".into(), "scripted-two".into())
    }

    fn never(&self) -> &'static str {
        "handle_message"
    }
}

// One test for each scenario a connector runs, for each connector. A scenario a connector leaves
// out is named, with the reason, in `crates/marmot-agent-conformance/README.md`.
macro_rules! conformance {
    ($module:ident, $connector:expr; $($name:ident),* $(,)?) => {
        mod $module {
            use super::*;
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                #[ignore = "needs Python 3.14 and Node 24, with npm ci run in both packages: CI's Conformance (socket) job"]
                async fn $name() {
                    let found = run!($name, OnSocket::new($connector));
                    found.unwrap_or_else(|v| panic!("{v}"));
                }
            )*
        }
    };
}

/// A scenario, from the suite's list or the socket's.
macro_rules! run {
    (reconnect, $subject:expr) => {
        socket::reconnect($subject).await
    };
    (dropped_after_delivery, $subject:expr) => {
        socket::dropped_after_delivery($subject).await
    };
    ($name:ident, $subject:expr) => {
        scenarios::$name($subject).await
    };
}

// Chat isolation (S5), each request settled once (S1), what the next connection is handed (S1, S11,
// S13) and a connection that drops after delivery and before the response (S4, S11), for each.
conformance!(hermes, Connector::Hermes; one_turn, two_chats, answered_once, reconnect, dropped_after_delivery);
conformance!(claude_code, Connector::ClaudeCode; one_turn, two_chats, answered_once, restart, reconnect, dropped_after_delivery);
conformance!(openclaw, Connector::OpenClaw; one_turn, two_chats, answered_once, reconnect);

// OpenClaw's plugin sends the event that settles an answer before its response to it. When the
// connection drops between the two, the event reached the core, and the answer was taken, settled
// once by the event (D-147): refused as failed, it would be settled twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Python 3.14 and Node 24, with npm ci run in both packages: CI's Conformance (socket) job"]
async fn openclaw_dropped_after_delivery() {
    let subject = OnSocket::new(Connector::OpenClaw);
    let fake = subject.fake.clone();
    let found = socket::dropped_after_delivery(subject).await;
    found.unwrap_or_else(|v| panic!("{v}"));
    let answer = (fake.dropped().into_iter())
        .flat_map(|d| d.unanswered)
        .find(|u| u.method == "answer");
    assert!(
        answer.as_ref().is_some_and(|a| a.shown),
        "the event that settles the answer reached the core before the connection dropped: {answer:?}"
    );
}

// A drop that comes only once the request has been given up decides nothing: the core refuses a
// request on a timeout as on a drop (D-147), so the scenario cannot tell which it saw, and fails.
// The relay says it held the response back only after the request's deadline, as on a machine that
// stalls there, so the scenario drops the connection too late.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Python 3.14 and Node 24, with npm ci run in both packages: CI's Conformance (socket) job"]
async fn a_drop_after_the_request_was_given_up_checks_nothing() {
    let mut subject = OnSocket::new(Connector::Hermes);
    subject.withheld_late = ANSWER_WITHIN + Duration::from_secs(1);
    let found = socket::dropped_after_delivery(subject).await;
    let violation = found.expect_err("a drop after the deadline is caught");
    assert_eq!(violation.property, "L3", "{violation}");
    assert!(
        violation.detail.contains("before the connection dropped"),
        "{violation}"
    );
}
