//! The driver against a stand-in for `codex app-server`: a task that reads what the driver writes
//! and answers as a script says, in the shapes the recordings hold. The test against a real Codex
//! is at the end, and runs only when asked for.

use marmot_agent_codex::{Codex, Sandbox, Setup};
use marmot_agent_driver::{
    Action, Answer, Chat, Control, Driver, Emitted, Given, Person, Prompt, Reason,
};
use marmot_agent_events::{Body, State};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, WriteHalf, duplex};
use tokio::sync::mpsc;

/// The stand-in: everything the driver writes arrives on `heard`; `say` writes to the driver.
struct Fake {
    heard: mpsc::UnboundedReceiver<Value>,
    to_driver: WriteHalf<DuplexStream>,
}

impl Fake {
    async fn say(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.to_driver.write_all(line.as_bytes()).await.unwrap();
    }

    /// The next message the driver wrote that is not one of `skip`.
    async fn hear(&mut self) -> Value {
        loop {
            let message = self.heard.recv().await.expect("the driver wrote");
            let method = message["method"].as_str().unwrap_or_default();
            if !matches!(
                method,
                "model/list" | "collaborationMode/list" | "initialized"
            ) {
                return message;
            }
            if method != "initialized" {
                self.say(json!({"id": message["id"], "result": {"data": []}}))
                    .await;
            }
        }
    }

    /// Answers the driver's next request, which must be `method`, with `result`.
    async fn answer(&mut self, method: &str, result: Value) -> Value {
        let request = self.hear().await;
        assert_eq!(request["method"], method, "{request}");
        self.say(json!({"id": request["id"], "result": result}))
            .await;
        request
    }
}

async fn connect() -> (Codex, Fake) {
    let (driver_side, fake_side) = duplex(1 << 16);
    let (from_driver, to_driver) = tokio::io::split(fake_side);
    let (driver_reads, driver_writes) = tokio::io::split(driver_side);
    let (tx, heard) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut lines = BufReader::new(from_driver).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(serde_json::from_str::<Value>(&line).unwrap());
        }
    });
    let mut fake = Fake { heard, to_driver };
    let codex = tokio::spawn(Codex::connect(
        driver_reads,
        driver_writes,
        Setup::new("/w"),
    ));
    let hello = fake.heard.recv().await.unwrap();
    assert_eq!(hello["method"], "initialize");
    let agent = "codex_cli_rs/0.169.0 (Linux; x86_64)";
    fake.say(json!({"id": hello["id"], "result": {"userAgent": agent}}))
        .await;
    (codex.await.unwrap().expect("a greeting"), fake)
}

fn alice() -> Person {
    Person {
        id: "u-1".into(),
        name: "Alice".into(),
    }
}

fn prompt(message: &str, text: &str) -> Prompt {
    Prompt {
        message: message.into(),
        text: text.into(),
        chat: Chat {
            id: "c-1".into(),
            group: false,
            name: None,
        },
        sender: alice(),
        reply_to: None,
        attachments: Vec::new(),
        session: None,
    }
}

fn turn(method: &str, status: &str) -> Value {
    json!({"method": method, "params": {"threadId": "th1", "turn": {"id": "t1", "items": [], "status": status, "error": null}}})
}

/// Events until one matches, with that one last.
async fn until(codex: &mut Codex, done: impl Fn(&Emitted) -> bool) -> Vec<Emitted> {
    let mut events = Vec::new();
    loop {
        let event = codex.next_event().await.expect("an event");
        let stop = done(&event);
        events.push(event);
        if stop {
            return events;
        }
    }
}

fn is_turn(state: State) -> impl Fn(&Emitted) -> bool {
    move |e| matches!(e.event.body(), Body::Turn(_)) && e.event.header().state == state
}

/// The driver and the stand-in, with a session started and a turn running for prompt `m-1`.
async fn running() -> (Codex, Fake) {
    let (mut codex, mut fake) = connect().await;
    let driver = tokio::spawn(async move {
        codex.prompt(prompt("m-1", "hello")).await.unwrap();
        codex
    });
    let thread = json!({"thread": {"id": "th1", "cliVersion": "0.169.0"}, "model": "m1", "reasoningEffort": "low"});
    fake.answer("thread/start", thread).await;
    fake.answer("turn/start", json!({"turn": {"id": "t1"}}))
        .await;
    fake.say(turn("turn/started", "inProgress")).await;
    (driver.await.unwrap(), fake)
}

#[tokio::test]
async fn a_prompt_starts_a_session_and_a_turn_that_names_it() {
    let (mut codex, mut fake) = running().await;
    assert_eq!(codex.harness().version.as_deref(), Some("0.169.0"));
    fake.say(json!({"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1", "item": {"type": "agentMessage", "id": "a1", "text": "Hi.", "phase": "final_answer"}}})).await;
    fake.say(turn("turn/completed", "completed")).await;
    let events = until(&mut codex, is_turn(State::Done)).await;
    assert!(events.iter().all(|e| e.chat == Some("c-1".into())));
    let session = &events[0];
    assert!(matches!(session.event.body(), Body::Session(_)));
    for turn in events
        .iter()
        .filter(|e| matches!(e.event.body(), Body::Turn(_)))
    {
        assert_eq!(
            turn.prompts,
            ["m-1".into()],
            "every event of the turn names its prompt"
        );
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e.event.body(), Body::Message(_)))
    );
}

#[tokio::test]
async fn a_prompt_during_a_turn_joins_it() {
    let (mut codex, mut fake) = running().await;
    let driver = tokio::spawn(async move {
        codex.prompt(prompt("m-2", "and this")).await.unwrap();
        codex
    });
    let steer = fake.answer("turn/steer", json!({"turnId": "t1"})).await;
    assert_eq!(steer["params"]["expectedTurnId"], "t1");
    let mut codex = driver.await.unwrap();
    fake.say(turn("turn/completed", "completed")).await;
    let events = until(&mut codex, is_turn(State::Done)).await;
    assert_eq!(events.last().unwrap().prompts, ["m-1".into(), "m-2".into()]);
}

#[tokio::test]
async fn a_prompt_waits_while_codex_has_yet_to_start_the_turn_before_it() {
    let (mut codex, mut fake) = connect().await;
    let driver = tokio::spawn(async move {
        codex.prompt(prompt("m-1", "hello")).await.unwrap();
        // The turn has been asked for and has not started: this waits, then joins it.
        codex.prompt(prompt("m-2", "and this")).await.unwrap();
        codex
    });
    fake.answer("thread/start", json!({"thread": {"id": "th1"}}))
        .await;
    fake.answer("turn/start", json!({"turn": {"id": "t1"}}))
        .await;
    fake.say(turn("turn/started", "inProgress")).await;
    fake.answer("turn/steer", json!({"turnId": "t1"})).await;
    driver.await.unwrap();
}

#[tokio::test]
async fn an_answer_reaches_codex_and_its_event_settles_it() {
    let (mut codex, mut fake) = running().await;
    fake.say(json!({"id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "ls", "availableDecisions": ["accept", "cancel"]}})).await;
    let asked = until(&mut codex, |e| matches!(e.event.body(), Body::Approval(_))).await;
    let item = asked.last().unwrap().event.header().item.clone();
    let allow = Answer {
        request: "r-1".into(),
        item,
        by: alice(),
        given: Given::Choice("c0".into()),
    };
    codex.answer(allow.clone()).await.unwrap();
    let reply = fake.heard.recv().await.unwrap();
    assert_eq!(reply, json!({"id": 0, "result": {"decision": "accept"}}));
    let answered = codex.next_event().await.unwrap();
    assert_eq!(answered.settles, ["r-1".into()]);
    assert_eq!(answered.event.header().state, State::Done);
    let again = codex.answer(allow).await.unwrap_err();
    assert_eq!(again.reason, Reason::AlreadyAnswered);
}

#[tokio::test]
async fn an_interrupt_is_settled_when_the_turn_ends() {
    let (mut codex, mut fake) = running().await;
    // The core knows a turn from its events.
    until(&mut codex, is_turn(State::Running)).await;
    let stop = Control {
        request: "r-2".into(),
        by: alice(),
        action: Action::Interrupt { turn: "t1".into() },
    };
    codex.control(stop).await.unwrap();
    fake.answer("turn/interrupt", json!({})).await;
    fake.say(turn("turn/completed", "interrupted")).await;
    let events = until(&mut codex, is_turn(State::Cancelled)).await;
    assert_eq!(events.last().unwrap().settles, ["r-2".into()]);
}

#[tokio::test]
async fn a_control_codex_refuses_is_settled_by_the_refusal() {
    let (mut codex, mut fake) = running().await;
    let set = Control {
        request: "r-3".into(),
        by: alice(),
        action: Action::Set {
            session: "th1".into(),
            setting: "model".into(),
            value: "m2".into(),
        },
    };
    let driver = tokio::spawn(async move {
        codex.control(set).await.unwrap();
        codex
    });
    let request = fake.hear().await;
    assert_eq!(request["method"], "thread/settings/update");
    let mut codex = driver.await.unwrap();
    fake.say(json!({"id": request["id"], "error": {"code": -32600, "message": "no"}}))
        .await;
    let events = until(&mut codex, |e| matches!(e.event.body(), Body::Error(_))).await;
    assert_eq!(events.last().unwrap().settles, ["r-3".into()]);
}

#[tokio::test]
async fn when_codex_stops_its_turn_is_closed_and_no_more_events_come() {
    let (mut codex, mut fake) = running().await;
    fake.to_driver.shutdown().await.unwrap(); // Codex's output ends
    let events = until(&mut codex, is_turn(State::Cancelled)).await;
    assert_eq!(events.last().unwrap().prompts, ["m-1".into()]);
    assert!(codex.next_event().await.is_none());
    let refused = codex
        .prompt(prompt("m-3", "still there?"))
        .await
        .unwrap_err();
    assert_eq!(refused.reason, Reason::Failed);
}

/// Against a real Codex, signed in on this machine. It uses a little of the account's usage, so
/// it runs only when asked for: `cargo test -p marmot-agent-codex --test driver -- --ignored`.
#[tokio::test]
#[ignore = "needs a signed-in Codex; run with --ignored"]
async fn a_real_codex_answers_a_prompt() {
    let folder = std::env::temp_dir().join(format!("marmot-agent-codex-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let mut setup = Setup::new(folder.to_str().unwrap());
    setup.sandbox = Sandbox::ReadOnly;
    let mut codex = Codex::start("codex", setup)
        .await
        .expect("codex app-server");
    codex
        .prompt(prompt("m-1", "Reply with exactly: hello from codex"))
        .await
        .expect("a prompt");
    let events = until(&mut codex, |e| {
        matches!(e.event.body(), Body::Turn(_)) && e.event.header().state != State::Running
    })
    .await;
    let finished = events.last().unwrap();
    assert_eq!(finished.event.header().state, State::Done, "{events:?}");
    assert_eq!(finished.prompts, ["m-1".into()]);
    let said = events.iter().any(|e| {
        matches!(e.event.body(), Body::Message(_))
            && e.event
                .header()
                .text
                .to_lowercase()
                .contains("hello from codex")
    });
    assert!(said, "{events:?}");
    std::fs::remove_dir_all(&folder).ok();
}

// A new session needs the turn to end, and the turn waits for an approval only a person can give:
// the driver says busy at once, rather than waiting for something the core cannot then send.
#[tokio::test]
async fn a_new_session_during_a_waiting_turn_is_refused_so_the_approval_can_be_answered() {
    let (mut codex, mut fake) = running().await;
    fake.say(json!({"id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "ls"}})).await;
    let asked = until(&mut codex, |e| matches!(e.event.body(), Body::Approval(_))).await;
    let item = asked.last().unwrap().event.header().item.clone();
    let fresh = Control {
        request: "r-4".into(),
        by: alice(),
        action: Action::NewSession {
            chat: prompt("m-0", "").chat,
            session: Some("th1".into()),
        },
    };
    assert_eq!(codex.control(fresh).await.unwrap_err().reason, Reason::Busy);
    let allow = Answer {
        request: "r-5".into(),
        item,
        by: alice(),
        given: Given::Choice("c0".into()),
    };
    codex.answer(allow).await.unwrap();
    assert_eq!(
        fake.heard.recv().await.unwrap()["result"]["decision"],
        "accept"
    );
}

/// A writer that takes only `allowed` more bytes, then waits until given more.
struct Choke {
    inner: WriteHalf<DuplexStream>,
    allowed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl tokio::io::AsyncWrite for Choke {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::sync::atomic::Ordering;
        let allowed = self.allowed.load(Ordering::SeqCst);
        if allowed == 0 {
            cx.waker().wake_by_ref(); // checked again when polled
            return std::task::Poll::Pending;
        }
        let n = buf.len().min(allowed);
        let polled = std::pin::Pin::new(&mut self.inner).poll_write(cx, &buf[..n]);
        if let std::task::Poll::Ready(Ok(written)) = polled {
            self.allowed.fetch_sub(written, Ordering::SeqCst);
        }
        polled
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

// The core drops a request's future to do something else. A message cut short in the middle is
// finished, whole, before the next one, so Codex never reads half a line.
#[tokio::test]
async fn a_message_cut_short_is_finished_before_the_next() {
    use std::sync::atomic::Ordering;
    let (driver_side, fake_side) = duplex(1 << 16);
    let (from_driver, to_driver) = tokio::io::split(fake_side);
    let (driver_reads, driver_writes) = tokio::io::split(driver_side);
    let allowed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX / 2));
    let choke = Choke {
        inner: driver_writes,
        allowed: allowed.clone(),
    };
    let (tx, heard) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut lines = BufReader::new(from_driver).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(serde_json::from_str::<Value>(&line).expect("whole lines only"));
        }
    });
    let mut fake = Fake { heard, to_driver };
    let codex = tokio::spawn(Codex::connect(driver_reads, choke, Setup::new("/w")));
    let hello = fake.heard.recv().await.unwrap();
    fake.say(json!({"id": hello["id"], "result": {"userAgent": "codex_cli_rs/0.169.0"}}))
        .await;
    let mut codex = codex.await.unwrap().unwrap();
    assert_eq!(fake.heard.recv().await.unwrap()["method"], "initialized");

    allowed.store(10, Ordering::SeqCst); // the next message gets ten bytes out, then stalls
    let cut = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        codex.prompt(prompt("m-1", "hi")),
    );
    assert!(
        cut.await.is_err(),
        "the write stalls and the core gives up on it"
    );
    allowed.store(usize::MAX / 2, Ordering::SeqCst);
    let driver = tokio::spawn(async move {
        codex.prompt(prompt("m-1", "hi")).await.unwrap();
        codex
    });
    let first = fake.heard.recv().await.unwrap();
    assert_eq!(
        first["method"], "model/list",
        "the cut message arrives whole, first"
    );
    fake.say(json!({"id": first["id"], "result": {"data": []}}))
        .await;
    fake.answer("thread/start", json!({"thread": {"id": "th1"}}))
        .await;
    fake.answer("turn/start", json!({"turn": {"id": "t1"}}))
        .await;
    driver.await.unwrap();
}

fn set_model(request: &str, value: &str) -> Control {
    Control {
        request: request.into(),
        by: alice(),
        action: Action::Set {
            session: "th1".into(),
            setting: "model".into(),
            value: value.into(),
        },
    }
}

fn settings_updated(model: &str, effort: &str) -> Value {
    json!({"method": "thread/settings/updated", "params": {"threadId": "th1", "threadSettings": {"model": model, "effort": effort}}})
}

// Codex restates a new session's settings a moment after it starts. That is not the change a
// person asked for, and does not settle it; the edit that shows the new value does.
#[tokio::test]
async fn a_setting_is_settled_by_the_edit_that_shows_its_value() {
    let (mut codex, mut fake) = running().await;
    // Codex's own restatement of the new session, still unread: it edits the session item, and
    // still shows the old model.
    fake.say(settings_updated("m1", "medium")).await;
    let driver = tokio::spawn(async move {
        codex.control(set_model("r-6", "m2")).await.unwrap();
        codex
    });
    let change = fake.answer("thread/settings/update", json!({})).await;
    assert_eq!(change["params"]["model"], "m2");
    let mut codex = driver.await.unwrap();
    fake.say(settings_updated("m2", "medium")).await;
    let events = until(&mut codex, |e| !e.settles.is_empty()).await;
    let settled = events.last().unwrap();
    assert_eq!(settled.settles, ["r-6".into()]);
    let Body::Session(session) = settled.event.body() else {
        panic!("the session item");
    };
    let model = session.settings.iter().find(|s| s.id == "model").unwrap();
    assert_eq!(model.value.as_deref(), Some("m2"));
}

// A prompt joins a turn only if Codex takes it.
#[tokio::test]
async fn a_prompt_codex_refuses_to_add_to_a_turn_is_not_counted_in_it() {
    let (mut codex, mut fake) = running().await;
    let driver = tokio::spawn(async move {
        codex.prompt(prompt("m-2", "and this")).await.unwrap();
        codex
    });
    let steer = fake.hear().await;
    assert_eq!(steer["method"], "turn/steer");
    let mut codex = driver.await.unwrap();
    fake.say(turn("turn/completed", "completed")).await;
    fake.say(json!({"id": steer["id"], "error": {"code": -32600, "message": "no active turn"}}))
        .await;
    let events = until(&mut codex, is_turn(State::Done)).await;
    assert_eq!(events.last().unwrap().prompts, ["m-1".into()]);
}

// A turn that ends before Codex handles the interrupt settles it; Codex's later refusal does not
// settle it again.
#[tokio::test]
async fn an_interrupt_is_settled_once() {
    let (mut codex, mut fake) = running().await;
    until(&mut codex, is_turn(State::Running)).await;
    let stop = Control {
        request: "r-7".into(),
        by: alice(),
        action: Action::Interrupt { turn: "t1".into() },
    };
    codex.control(stop).await.unwrap();
    let interrupt = fake.hear().await;
    fake.say(turn("turn/completed", "completed")).await;
    fake.say(
        json!({"id": interrupt["id"], "error": {"code": -32600, "message": "no active turn"}}),
    )
    .await;
    fake.say(json!({"method": "warning", "params": {"threadId": "th1", "message": "after"}}))
        .await;
    let events = until(&mut codex, |e| matches!(e.event.body(), Body::Notice(_))).await;
    let settled: Vec<_> = events.iter().flat_map(|e| e.settles.clone()).collect();
    assert_eq!(settled, ["r-7".into()]);
}

// A session's thread can speak before the reply that starts it; what it says is kept for it.
#[tokio::test]
async fn what_a_new_thread_says_before_its_reply_is_kept() {
    let (mut codex, mut fake) = connect().await;
    let driver = tokio::spawn(async move {
        codex.prompt(prompt("m-1", "hello")).await.unwrap();
        codex
    });
    let start = fake.hear().await;
    assert_eq!(start["method"], "thread/start");
    fake.say(settings_updated("m9", "low")).await; // before the reply
    fake.say(json!({"id": start["id"], "result": {"thread": {"id": "th1"}, "model": "m1"}}))
        .await;
    fake.answer("turn/start", json!({"turn": {"id": "t1"}}))
        .await;
    let mut codex = driver.await.unwrap();
    assert_eq!(codex.unrouted(), 0);
    let events = until(&mut codex, |e| {
        let Body::Session(s) = e.event.body() else {
            return false;
        };
        s.settings
            .iter()
            .any(|x| x.id == "model" && x.value.as_deref() == Some("m9"))
    })
    .await;
    assert!(!events.is_empty());
}
