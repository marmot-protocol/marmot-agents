//! The driver against a stand-in for `codex app-server`: a task that reads what the driver writes
//! and answers as a script says, in the shapes the recordings hold. The test against a real Codex
//! is at the end, and runs only when asked for.

use marmot_agent_codex::{Codex, Sandbox, Setup};
use marmot_agent_driver::{
    Action, Answer, Chat, Control, Driver, Emitted, Given, Output, Person, Prompt, Reason,
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
            folder: None,
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
        let event = codex
            .next_event()
            .await
            .and_then(Output::into_event)
            .expect("an event");
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
    let answered = codex
        .next_event()
        .await
        .and_then(Output::into_event)
        .unwrap();
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
    let second = fake.heard.recv().await.unwrap();
    assert_eq!(
        second["method"], "collaborationMode/list",
        "and what was queued with it"
    );
    fake.say(json!({"id": first["id"], "result": {"data": []}}))
        .await;
    fake.say(json!({"id": second["id"], "result": {"data": []}}))
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

// Codex can announce a turn before it replies to the request that started it: 7 of the 37
// recorded turns came that way. The turn still names its prompt.
#[tokio::test]
async fn a_turn_announced_before_its_reply_still_names_its_prompt() {
    let (mut codex, mut fake) = connect().await;
    let driver = tokio::spawn(async move {
        codex.prompt(prompt("m-1", "hello")).await.unwrap();
        codex
    });
    fake.answer("thread/start", json!({"thread": {"id": "th1"}}))
        .await;
    let start = fake.hear().await;
    assert_eq!(start["method"], "turn/start");
    fake.say(turn("turn/started", "inProgress")).await; // before the reply
    fake.say(json!({"id": start["id"], "result": {"turn": {"id": "t1"}}}))
        .await;
    fake.say(turn("turn/completed", "completed")).await;
    let mut codex = driver.await.unwrap();
    let events = until(&mut codex, is_turn(State::Done)).await;
    for turn in events
        .iter()
        .filter(|e| matches!(e.event.body(), Body::Turn(_)))
    {
        assert_eq!(turn.prompts, ["m-1".into()]);
    }
}

fn new_session(request: &str) -> Control {
    Control {
        request: request.into(),
        by: alice(),
        action: Action::NewSession {
            chat: prompt("m-0", "").chat,
            session: None,
        },
    }
}

// A new session asked for by a control is the same as one a prompt starts: what its thread says
// before Codex's reply is kept, and a prompt sent meanwhile uses it rather than starting another.
#[tokio::test]
async fn a_new_session_from_a_control_is_kept_and_used_by_the_next_prompt() {
    let (mut codex, mut fake) = connect().await;
    let driver = tokio::spawn(async move {
        codex.control(new_session("r-8")).await.unwrap();
        codex.prompt(prompt("m-1", "hello")).await.unwrap();
        codex
    });
    let start = fake.hear().await;
    assert_eq!(start["method"], "thread/start");
    fake.say(settings_updated("m9", "low")).await; // before the reply
    fake.say(json!({"id": start["id"], "result": {"thread": {"id": "th1"}, "model": "m1"}}))
        .await;
    let next = fake
        .answer("turn/start", json!({"turn": {"id": "t1"}}))
        .await;
    assert_eq!(next["params"]["threadId"], "th1", "one session, not two");
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
    let settled: Vec<_> = events.iter().flat_map(|e| e.settles.clone()).collect();
    assert_eq!(settled, ["r-8".into()]);
}

// A session belongs to one chat. Another chat asking to resume it is refused before anything is
// sent, and the session's activity still goes to its own chat.
#[tokio::test]
async fn a_chat_cannot_take_another_chats_session() {
    let (mut codex, mut fake) = running().await;
    let mut other = prompt("m-9", "let me in");
    other.chat.id = "c-2".into();
    other.session = Some("th1".into());
    assert_eq!(
        codex.prompt(other).await.unwrap_err().reason,
        Reason::Invalid
    );
    fake.say(turn("turn/completed", "completed")).await;
    let events = until(&mut codex, is_turn(State::Done)).await;
    assert!(events.iter().all(|e| e.chat == Some("c-1".into())));
}

/// How a [`Breakable`] writer fails.
const WHOLE: u8 = 0;
const ON_WRITE: u8 = 1;
const ON_FLUSH: u8 = 2;

/// A writer that fails once it is told to, as Codex's input does when Codex has gone: either at
/// once, or after taking some bytes, when they are flushed.
struct Breakable {
    inner: WriteHalf<DuplexStream>,
    broken: std::sync::Arc<std::sync::atomic::AtomicU8>,
    taken: bool,
}

impl Breakable {
    fn mode(&self) -> u8 {
        self.broken.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl tokio::io::AsyncWrite for Breakable {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.mode() {
            WHOLE => std::pin::Pin::new(&mut self.inner).poll_write(cx, buf),
            ON_WRITE => {
                let gone = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
                std::task::Poll::Ready(Err(gone))
            }
            _ => {
                self.taken = true; // taken, never delivered
                std::task::Poll::Ready(Ok(buf.len()))
            }
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.mode() == ON_FLUSH && self.taken {
            let gone = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
            return std::task::Poll::Ready(Err(gone));
        }
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

// An answer that never reached Codex is not shown as given: when Codex has gone, the approval is
// closed unanswered, and nothing settles the answer. Codex can be found gone when the answer is
// written, or only when it is flushed.
#[tokio::test]
async fn an_answer_codex_never_received_is_not_shown_as_given() {
    never_received(ON_WRITE).await;
    never_received(ON_FLUSH).await;
}

async fn never_received(failure: u8) {
    let (driver_side, fake_side) = duplex(1 << 16);
    let (from_driver, to_driver) = tokio::io::split(fake_side);
    let (driver_reads, driver_writes) = tokio::io::split(driver_side);
    let broken = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(WHOLE));
    let writer = Breakable {
        inner: driver_writes,
        broken: broken.clone(),
        taken: false,
    };
    let (tx, heard) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut lines = BufReader::new(from_driver).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(serde_json::from_str::<Value>(&line).unwrap());
        }
    });
    let mut fake = Fake { heard, to_driver };
    let codex = tokio::spawn(Codex::connect(driver_reads, writer, Setup::new("/w")));
    let hello = fake.heard.recv().await.unwrap();
    fake.say(json!({"id": hello["id"], "result": {"userAgent": "codex_cli_rs/0.169.0"}}))
        .await;
    let mut codex = codex.await.unwrap().unwrap();
    let driver = tokio::spawn(async move {
        codex.prompt(prompt("m-1", "hello")).await.unwrap();
        codex
    });
    fake.answer("thread/start", json!({"thread": {"id": "th1"}}))
        .await;
    fake.answer("turn/start", json!({"turn": {"id": "t1"}}))
        .await;
    let mut codex = driver.await.unwrap();
    fake.say(turn("turn/started", "inProgress")).await;
    fake.say(json!({"id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "ls"}})).await;
    let asked = until(&mut codex, |e| matches!(e.event.body(), Body::Approval(_))).await;
    let item = asked.last().unwrap().event.header().item.clone();
    broken.store(failure, std::sync::atomic::Ordering::SeqCst);
    let allow = Answer {
        request: "r-9".into(),
        item: item.clone(),
        by: alice(),
        given: Given::Choice("c0".into()),
    };
    assert_eq!(
        codex.answer(allow).await.unwrap_err().reason,
        Reason::Failed
    );
    let mut rest = Vec::new();
    while let Some(event) = codex.next_event().await.and_then(Output::into_event) {
        rest.push(event);
    }
    assert!(
        rest.iter().all(|e| e.settles.is_empty()),
        "nothing settles the answer"
    );
    let closed = rest
        .iter()
        .find(|e| e.event.header().item == item)
        .expect("the approval closed");
    assert_eq!(closed.event.header().state, State::Cancelled);
}

/// Starts a session for chat `chat` with thread `thread`, and lets its turn finish.
async fn idle(codex: Codex, fake: &mut Fake, chat: &str, thread: &str) -> Codex {
    let mut asked = prompt("m-1", "hello");
    asked.chat.id = chat.into();
    let driver = tokio::spawn(async move {
        let mut codex = codex;
        codex.prompt(asked).await.unwrap();
        codex
    });
    let turn_id = format!("{thread}-t");
    fake.answer("thread/start", json!({"thread": {"id": thread}}))
        .await;
    fake.answer("turn/start", json!({"turn": {"id": turn_id}}))
        .await;
    let mut codex = driver.await.unwrap();
    let mark = |method: &str, status: &str| json!({"method": method, "params": {"threadId": thread, "turn": {"id": turn_id, "items": [], "status": status, "error": null}}});
    fake.say(mark("turn/started", "inProgress")).await;
    fake.say(mark("turn/completed", "completed")).await;
    until(&mut codex, is_turn(State::Done)).await;
    codex
}

fn new_session_of(chat: &str, session: Option<&str>, request: &str) -> Control {
    let mut target = prompt("m-0", "").chat;
    target.id = chat.into();
    Control {
        request: request.into(),
        by: alice(),
        action: Action::NewSession {
            chat: target,
            session: session.map(Into::into),
        },
    }
}

// A session stays its chat's after the chat moves on to a new one: another chat cannot resume it.
#[tokio::test]
async fn a_chats_old_session_stays_its_own() {
    let (codex, mut fake) = connect().await;
    let mut codex = idle(codex, &mut fake, "c-1", "th1").await;
    codex
        .control(new_session_of("c-1", Some("th1"), "r-10"))
        .await
        .unwrap();
    fake.answer("thread/start", json!({"thread": {"id": "th2"}}))
        .await;
    until(&mut codex, |e| !e.settles.is_empty()).await;
    let mut other = prompt("m-9", "let me in");
    other.chat.id = "c-2".into();
    other.session = Some("th1".into());
    assert_eq!(
        codex.prompt(other).await.unwrap_err().reason,
        Reason::Invalid
    );
}

// After a new session is asked for, the next prompt names none (D-065) and goes to the new one,
// even before the core has heard of it.
#[tokio::test]
async fn a_prompt_after_a_new_session_goes_to_the_new_one() {
    let (codex, mut fake) = connect().await;
    let mut codex = idle(codex, &mut fake, "c-1", "th1").await;
    let driver = tokio::spawn(async move {
        codex
            .control(new_session_of("c-1", Some("th1"), "r-11"))
            .await
            .unwrap();
        codex.prompt(prompt("m-2", "carry on")).await.unwrap();
        codex
    });
    fake.answer("thread/start", json!({"thread": {"id": "th2"}}))
        .await;
    let next = fake
        .answer("turn/start", json!({"turn": {"id": "t2"}}))
        .await;
    assert_eq!(next["params"]["threadId"], "th2");
    driver.await.unwrap();
}

// A prompt naming a session resumes it (D-065): after a restart, the chat's session is taken up
// again before the turn starts.
#[tokio::test]
async fn a_prompt_naming_a_session_resumes_it() {
    let (mut codex, mut fake) = connect().await;
    let driver = tokio::spawn(async move {
        let mut again = prompt("m-1", "where were we");
        again.session = Some("th0".into());
        codex.prompt(again).await.unwrap();
        codex
    });
    let resume = fake
        .answer("thread/resume", json!({"thread": {"id": "th0"}}))
        .await;
    assert_eq!(resume["params"]["threadId"], "th0");
    let next = fake
        .answer("turn/start", json!({"turn": {"id": "t1"}}))
        .await;
    assert_eq!(next["params"]["threadId"], "th0");
    driver.await.unwrap();
}

// While two chats pick sessions at once, what one session's thread said before its reply reaches
// it before what it said after.
#[tokio::test]
async fn early_messages_keep_their_order_while_another_chat_picks() {
    let (mut codex, mut fake) = connect().await;
    let driver = tokio::spawn(async move {
        codex
            .control(new_session_of("c-1", None, "r-12"))
            .await
            .unwrap();
        let mut other = prompt("m-1", "hello");
        other.chat.id = "c-2".into();
        codex.prompt(other).await.unwrap();
        codex
    });
    let first = fake.hear().await;
    let second = fake.hear().await;
    assert_eq!(first["method"], "thread/start");
    assert_eq!(second["method"], "thread/start");
    let updated = |model: &str| json!({"method": "thread/settings/updated", "params": {"threadId": "tha", "threadSettings": {"model": model}}});
    fake.say(updated("m9")).await; // before tha's reply
    fake.say(json!({"id": first["id"], "result": {"thread": {"id": "tha"}, "model": "m1"}}))
        .await;
    fake.say(updated("m8")).await; // after it, while c-2 still waits
    fake.say(json!({"id": second["id"], "result": {"thread": {"id": "thb"}}}))
        .await;
    fake.answer("turn/start", json!({"turn": {"id": "tb"}}))
        .await;
    let mut codex = driver.await.unwrap();
    let mut models = Vec::new();
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), codex.next_event()).await
        && let Some(event) = event.into_event()
    {
        if let Body::Session(s) = event.event.body()
            && event.chat == Some("c-1".into())
        {
            let model = s
                .settings
                .iter()
                .find(|x| x.id == "model")
                .and_then(|x| x.value.clone());
            models.push(model.unwrap_or_default());
        }
    }
    // Both arrive, in the order Codex sent them.
    let at = |model: &str| models.iter().position(|m| m == model);
    assert!(at("m9").is_some() && at("m9") < at("m8"), "{models:?}");
    assert_eq!(models.last().map(String::as_str), Some("m8"), "{models:?}");
}

// Codex stopping is not an interrupt working: the turn the driver closes then settles nothing, and
// the core settles the interrupt as failed (D-058).
#[tokio::test]
async fn a_turn_closed_because_codex_stopped_does_not_settle_an_interrupt() {
    let (mut codex, mut fake) = running().await;
    until(&mut codex, is_turn(State::Running)).await;
    let stop = Control {
        request: "r-13".into(),
        by: alice(),
        action: Action::Interrupt { turn: "t1".into() },
    };
    codex.control(stop).await.unwrap();
    assert_eq!(fake.hear().await["method"], "turn/interrupt");
    fake.to_driver.shutdown().await.unwrap();
    let mut rest = Vec::new();
    while let Some(event) = codex.next_event().await.and_then(Output::into_event) {
        rest.push(event);
    }
    assert!(rest.iter().any(|e| matches!(e.event.body(), Body::Turn(_))));
    assert!(rest.iter().all(|e| e.settles.is_empty()));
}

// When Codex refuses to start, the error says so with Codex's code and none of its words, which
// can hold a path; an error may be logged (D-071). The setup leaves out its working folder too.
#[tokio::test]
async fn a_refused_start_and_the_setup_print_no_path() {
    let (driver_side, fake_side) = duplex(1 << 16);
    let (from_driver, mut to_driver) = tokio::io::split(fake_side);
    let (driver_reads, driver_writes) = tokio::io::split(driver_side);
    let setup = Setup::new("/home/someone/private");
    assert!(!format!("{setup:?}").contains("private"));
    let codex = tokio::spawn(Codex::connect(driver_reads, driver_writes, setup));
    let mut lines = BufReader::new(from_driver).lines();
    let hello: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    let refusal = json!({"id": hello["id"], "error": {"code": -32600, "message": "bad config in /home/someone/private/config.toml"}});
    to_driver
        .write_all(format!("{refusal}\n").as_bytes())
        .await
        .unwrap();
    let error = codex.await.unwrap().err().expect("refused");
    for printed in [format!("{error:?}"), error.to_string()] {
        assert!(!printed.contains("private"), "{printed}");
        assert!(printed.contains("-32600"), "{printed}");
    }
}

// -- subagents (D-117)

/// A subagent's turn, under the person's turn `t1`.
fn sub_turn(method: &str, thread: &str, turn: &str, status: &str) -> Value {
    json!({"method": method, "params": {"threadId": thread, "turn": {"id": turn,
        "rootTurnId": "t1", "items": [], "status": status, "error": null}}})
}

fn is_task(state: State) -> impl Fn(&Emitted) -> bool {
    move |e| matches!(e.event.body(), Body::Task(_)) && e.event.header().state == state
}

// From Codex 0.171.0 a subagent's thread can be announced with the thread that started it
// (`parentThreadId`) and no item naming it: it is shown in the chat of its parent all the same.
#[tokio::test]
async fn a_thread_started_under_a_chats_thread_is_its_subagent() {
    let (mut codex, mut fake) = running().await;
    until(&mut codex, is_turn(State::Running)).await;
    fake.say(
        json!({"method": "thread/started", "params": {"thread": {"id": "sub1",
        "parentThreadId": "th1"}}}),
    )
    .await;
    fake.say(sub_turn("turn/started", "sub1", "st1", "inProgress"))
        .await;
    // The item that names it comes after its turn has started: the task takes it as its subject.
    fake.say(started_by("a1", "sub1")).await;
    fake.say(json!({"method": "item/completed", "params": {"threadId": "sub1", "turnId": "st1",
        "item": {"type": "agentMessage", "id": "w1", "text": "found it", "phase": "final_answer"}}}))
        .await;
    fake.say(sub_turn("turn/completed", "sub1", "st1", "completed"))
        .await;
    let events = until(&mut codex, is_task(State::Done)).await;
    let tasks: Vec<_> = (events.iter())
        .filter_map(|e| match e.event.body() {
            Body::Task(t) => Some((e.event.header().state, t.subject.clone())),
            _ => None,
        })
        .collect();
    let a1 = Some("a1".into());
    assert_eq!(tasks.first(), Some(&(State::Running, None)), "{tasks:?}");
    assert!(
        tasks.contains(&(State::Running, a1.clone())),
        "the open task is given its subject: {tasks:?}"
    );
    assert_eq!(tasks.last(), Some(&(State::Done, a1)), "{tasks:?}");
    for e in &events {
        let header = e.event.header();
        assert_eq!(e.chat, Some("c-1".into()));
        assert_eq!(header.session.as_ref().map(|s| s.as_str()), Some("th1"));
        assert_eq!(header.turn.as_ref().map(|t| t.as_str()), Some("t1"));
    }
    let said = events.iter().any(|e| e.event.header().text == "found it");
    assert!(said, "the subagent's words reach the chat");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.event.body(), Body::Session(_))),
        "a subagent's thread is no session of the chat"
    );
    assert_eq!(codex.unrouted(), 0);
}

/// The parent's item that starts a subagent in `thread`, on Codex's newer runtime.
fn started_by(item: &str, thread: &str) -> Value {
    let started = json!({"type": "subAgentActivity", "id": item, "kind": "started",
        "agentThreadId": thread, "agentPath": "/root/a"});
    json!({"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1",
        "item": started}})
}

fn stop_t1() -> Control {
    Control {
        request: "r1".into(),
        by: alice(),
        action: Action::Interrupt { turn: "t1".into() },
    }
}

fn settles_r1(e: &Emitted) -> bool {
    e.settles.contains(&"r1".into())
}

/// The next message the driver writes, or a failure if it writes none in a few seconds.
async fn next_heard(fake: &mut Fake) -> Value {
    let heard = tokio::time::timeout(std::time::Duration::from_secs(5), fake.hear()).await;
    heard.expect("the driver wrote")
}

/// A turn running with a subagent `sub1` running turn `st1` under it, and a Stop of the turn sent
/// and taken: the interrupts it sent, in order, unanswered.
async fn stopping_with_a_subagent() -> (Codex, Fake, Value, Value) {
    let (mut codex, mut fake) = running().await;
    fake.say(started_by("a1", "sub1")).await;
    fake.say(sub_turn("turn/started", "sub1", "st1", "inProgress"))
        .await;
    until(&mut codex, is_task(State::Running)).await;
    let driver = tokio::spawn(async move {
        codex.control(stop_t1()).await.unwrap();
        codex
    });
    let first = fake.hear().await;
    let second = fake.hear().await;
    assert_eq!(first["params"]["threadId"], "th1", "the parent first");
    assert_eq!(second["params"]["threadId"], "sub1");
    (driver.await.unwrap(), fake, first, second)
}

// Codex refusing to stop a subagent that then runs on: once the rest have ended the Stop is
// settled once, and as failed, by the refusal, not by the turn's end, which would look like it
// worked.
#[tokio::test]
async fn a_stop_codex_refuses_for_a_running_subagent_fails() {
    let (mut codex, mut fake, first, second) = stopping_with_a_subagent().await;
    fake.say(json!({"id": second["id"], "error": {"code": -32600, "message": "no"}}))
        .await;
    fake.say(json!({"id": first["id"], "result": {}})).await;
    fake.say(turn("turn/completed", "interrupted")).await;
    let events = until(&mut codex, settles_r1).await;
    let settler = events.last().unwrap();
    assert!(
        matches!(settler.event.body(), Body::Error(_)),
        "settled by the refusal: {settler:?}"
    );
    assert_eq!(settler.event.header().state, State::Failed);
    assert!(
        events.iter().any(is_turn(State::Cancelled)),
        "the turn ended first"
    );
    fake.say(sub_turn("turn/completed", "sub1", "st1", "completed"))
        .await;
    let rest = until(&mut codex, is_task(State::Done)).await;
    assert!(!rest.iter().any(settles_r1), "settled once");
}

// Codex refusing to stop a subagent that ends anyway before the rest: the Stop is settled once,
// by the end of the last turn it waited for.
#[tokio::test]
async fn a_stop_codex_refuses_for_a_subagent_that_ends_waits_for_the_rest() {
    let (mut codex, mut fake, first, second) = stopping_with_a_subagent().await;
    fake.say(json!({"id": second["id"], "error": {"code": -32600, "message": "no"}}))
        .await;
    fake.say(sub_turn("turn/completed", "sub1", "st1", "completed"))
        .await;
    fake.say(json!({"id": first["id"], "result": {}})).await;
    fake.say(turn("turn/completed", "interrupted")).await;
    let events = until(&mut codex, is_turn(State::Cancelled)).await;
    let settling: Vec<&Emitted> = events.iter().filter(|e| settles_r1(e)).collect();
    assert_eq!(settling.len(), 1, "settled once");
    assert!(matches!(settling[0].event.body(), Body::Turn(_)));
}

// A subagent whose turn has started, but whose start the driver has not read when the Stop
// comes, is stopped too once its start is read, and the Stop waits for it.
#[tokio::test]
async fn a_stop_also_stops_a_subagent_whose_start_was_still_unread() {
    let (mut codex, mut fake) = running().await;
    fake.say(started_by("a1", "sub1")).await;
    until(&mut codex, |e| matches!(e.event.body(), Body::Tool(_))).await;
    // Codex has started the subagent's turn; the driver has not read it yet.
    fake.say(sub_turn("turn/started", "sub1", "st1", "inProgress"))
        .await;
    let driver = tokio::spawn(async move {
        codex.control(stop_t1()).await.unwrap();
        codex
    });
    let first = fake.hear().await;
    assert_eq!(first["params"]["threadId"], "th1");
    let mut codex = driver.await.unwrap();
    fake.say(json!({"id": first["id"], "result": {}})).await;
    let reading = tokio::spawn(async move {
        let events = until(&mut codex, is_task(State::Cancelled)).await;
        (codex, events)
    });
    let second = next_heard(&mut fake).await;
    assert_eq!(second["method"], "turn/interrupt");
    assert_eq!(second["params"]["threadId"], "sub1");
    assert_eq!(second["params"]["turnId"], "st1");
    fake.say(json!({"id": second["id"], "result": {}})).await;
    fake.say(turn("turn/completed", "interrupted")).await;
    fake.say(sub_turn("turn/completed", "sub1", "st1", "interrupted"))
        .await;
    let (_, events) = reading.await.unwrap();
    let settling: Vec<&Emitted> = events.iter().filter(|e| settles_r1(e)).collect();
    assert_eq!(settling.len(), 1, "settled once");
    assert!(
        matches!(settling[0].event.body(), Body::Task(_)),
        "settled by the last to end"
    );
}

// Each translator in a chat numbers the items it makes up from 0; two subagents' warnings and
// errors are still four items, not two.
#[tokio::test]
async fn two_subagents_warnings_and_errors_are_items_of_their_own() {
    let (mut codex, mut fake) = running().await;
    fake.say(started_by("a1", "sub1")).await;
    fake.say(started_by("a2", "sub2")).await;
    for thread in ["sub1", "sub2"] {
        fake.say(json!({"method": "warning", "params": {"threadId": thread, "message": "w"}}))
            .await;
        fake.say(json!({"method": "error", "params": {"threadId": thread,
            "error": {"message": "e"}, "willRetry": false}}))
            .await;
    }
    let mut items = std::collections::HashSet::new();
    let mut count = 0;
    while count < 4 {
        let e = until(&mut codex, |e| {
            matches!(e.event.body(), Body::Notice(_) | Body::Error(_))
        })
        .await;
        items.insert(e.last().unwrap().event.header().item.as_str().to_string());
        count += 1;
    }
    assert_eq!(items.len(), 4, "{items:?}");
}

// Threads named by one item, which spoke before it, have what they said shown in the order it
// came, not thread by thread.
#[tokio::test]
async fn what_threads_said_before_they_were_named_keeps_its_order() {
    let (mut codex, mut fake) = running().await;
    let said = |thread: &str, id: &str| {
        json!({"method": "item/completed", "params": {"threadId": thread, "turnId": "x",
            "item": {"type": "agentMessage", "id": id, "text": id, "phase": "final_answer"}}})
    };
    fake.say(said("subB", "b1")).await;
    fake.say(said("subA", "a1")).await;
    fake.say(said("subB", "b2")).await;
    fake.say(said("subA", "a2")).await;
    let spawn = json!({"type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent",
        "status": "completed", "receiverThreadIds": ["subA", "subB"], "prompt": "look"});
    fake.say(
        json!({"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1",
        "item": spawn}}),
    )
    .await;
    let mut words = Vec::new();
    while words.len() < 4 {
        let e = until(&mut codex, |e| matches!(e.event.body(), Body::Message(_))).await;
        words.push(e.last().unwrap().event.header().text.clone());
    }
    assert_eq!(words, ["b1", "a1", "b2", "a2"]);
}

// A chat's session replaced while a subagent of it runs: Codex does not stop the subagent with
// the session, so the driver does (D-117). A subagent the old one was still starting will name
// no thread a chat has, so a later request for a thread never seen is refused at once, not held
// until the driver starts again.
#[tokio::test]
async fn a_new_session_stops_the_old_ones_subagents() {
    let (mut codex, mut fake) = running().await;
    fake.say(started_by("a1", "sub1")).await;
    fake.say(sub_turn("turn/started", "sub1", "st1", "inProgress"))
        .await;
    // The subagent starts one of its own, on Codex's older runtime, which names its thread only
    // when the item completes.
    let nested = json!({"type": "collabAgentToolCall", "id": "c9", "tool": "spawnAgent",
        "status": "inProgress", "receiverThreadIds": [], "prompt": "look"});
    fake.say(
        json!({"method": "item/started", "params": {"threadId": "sub1", "turnId": "st1",
        "item": nested}}),
    )
    .await;
    fake.say(turn("turn/completed", "completed")).await;
    until(&mut codex, is_turn(State::Done)).await;
    let fresh = Control {
        request: "r2".into(),
        by: alice(),
        action: Action::NewSession {
            chat: prompt("m-0", "").chat,
            session: Some("th1".into()),
        },
    };
    let driver = tokio::spawn(async move {
        codex.control(fresh).await.unwrap();
        codex
    });
    let start = fake.hear().await;
    assert_eq!(start["method"], "thread/start");
    let mut codex = driver.await.unwrap();
    fake.say(json!({"id": start["id"], "result": {"thread": {"id": "th2"}, "model": "m1"}}))
        .await;
    let reading = tokio::spawn(async move {
        let events = until(&mut codex, |_| false).await;
        (codex, events)
    });
    let interrupt = next_heard(&mut fake).await;
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["threadId"], "sub1");
    assert_eq!(interrupt["params"]["turnId"], "st1");
    fake.say(json!({"id": 12, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th-other", "turnId": "t9", "itemId": "c", "command": "ls"}})).await;
    let reply = reply_to(&mut fake, 12).await;
    reading.abort();
    let reply = reply.expect("a request for a thread never seen was held after the session went");
    assert_eq!(reply["error"], no_chat_has("th-other"));
}

/// The driver's reply to Codex's request `id`, if it wrote one within half a second.
async fn reply_to(fake: &mut Fake, id: i64) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while let Ok(Some(message)) = tokio::time::timeout_at(deadline, fake.heard.recv()).await {
        if message["id"] == id && message.get("method").is_none() {
            return Some(message);
        }
    }
    None
}

/// What the driver wrote up to the moment it had handled every line said before this: a request
/// of a kind it has no rule for, in the chat's own thread, is refused as it is read, and the
/// driver reads in order. Without the refusal itself.
async fn written_so_far(fake: &mut Fake) -> Vec<Value> {
    fake.say(json!({"id": 999, "method": "item/something/new", "params": {"threadId": "th1", "turnId": "t1"}}))
        .await;
    let mut written = Vec::new();
    loop {
        let message = next_heard(fake).await;
        if message["id"] == 999 && message.get("method").is_none() {
            return written;
        }
        written.push(message);
    }
}

/// Whether the driver replied to Codex's request `id` among `written`.
fn replied(written: &[Value], id: i64) -> bool {
    (written.iter()).any(|m| m["id"] == id && m.get("method").is_none())
}

/// The error a request for a thread no chat has is refused with.
fn no_chat_has(thread: &str) -> Value {
    json!({"code": -32602, "message": "Invalid params", "data": {"threadId": thread}})
}

/// Codex waits for a reply to each request it makes, and treats an error as a refusal (an approval
/// denied, a question with no answers), so one no chat is asked about is refused at once, as a
/// call of a tool the driver does not have is. Found by the `DriverTransport` model
/// (`formal/tla`, case `codex-stray`): an approval for a thread no chat has was counted as
/// unrouted and never answered.
#[tokio::test]
async fn a_request_for_a_thread_no_chat_has_is_answered() {
    let (mut codex, mut fake) = running().await;
    let reading = tokio::spawn(async move {
        until(&mut codex, is_turn(State::Done)).await;
        codex
    });
    fake.say(json!({"id": 5, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th-other", "turnId": "t9", "itemId": "c", "command": "ls", "availableDecisions": ["accept", "cancel"]}})).await;
    let reply = reply_to(&mut fake, 5).await;
    let reply = reply.expect("the request for a thread no chat has was never answered");
    assert_eq!(reply["error"], no_chat_has("th-other"));
    fake.say(turn("turn/completed", "completed")).await;
    let codex = reading.await.unwrap();
    assert_eq!(codex.unrouted(), 1);
}

/// A request for a thread the chat has left, when its session was replaced, is refused at once
/// too: nothing will name it again.
#[tokio::test]
async fn a_request_for_a_thread_the_chat_left_is_answered() {
    let (mut codex, mut fake) = running().await;
    fake.say(turn("turn/completed", "completed")).await;
    until(&mut codex, is_turn(State::Done)).await;
    let fresh = Control {
        request: "r2".into(),
        by: alice(),
        action: Action::NewSession {
            chat: prompt("m-0", "").chat,
            session: Some("th1".into()),
        },
    };
    let driver = tokio::spawn(async move {
        codex.control(fresh).await.unwrap();
        codex
    });
    let start = fake.hear().await;
    assert_eq!(start["method"], "thread/start");
    let mut codex = driver.await.unwrap();
    fake.say(json!({"id": start["id"], "result": {"thread": {"id": "th2"}, "model": "m1"}}))
        .await;
    let reading = tokio::spawn(async move { until(&mut codex, |_| false).await });
    fake.say(json!({"id": 7, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "ls"}})).await;
    let reply = reply_to(&mut fake, 7).await;
    reading.abort();
    let reply = reply.expect("the request for the thread the chat left was never answered");
    assert_eq!(reply["error"], no_chat_has("th1"));
}

/// While a subagent is being started (Codex's older runtime names its thread only when the
/// `spawnAgent` item completes), a request for a thread not known yet is held: it may be the
/// subagent's. Once the spawn is done, the subagent's request is the chat's, and one for a thread
/// still unknown is refused.
#[tokio::test]
async fn a_request_held_while_a_subagent_starts_is_answered_once_it_has() {
    let (mut codex, mut fake) = running().await;
    let (shown, mut asked) = mpsc::unbounded_channel();
    let reading = tokio::spawn(async move {
        until(&mut codex, |e| {
            let waits = matches!(e.event.body(), Body::Approval(_))
                && e.event.header().state == State::Waiting;
            if waits {
                shown
                    .send(e.event.header().item.as_str().to_string())
                    .unwrap();
            }
            false
        })
        .await
    });
    let spawn = |status: &str, threads: Value| {
        let item = json!({"type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent",
            "status": status, "receiverThreadIds": threads, "prompt": "look"});
        let method = if status == "completed" {
            "item/completed"
        } else {
            "item/started"
        };
        json!({"method": method, "params": {"threadId": "th1", "turnId": "t1", "item": item}})
    };
    fake.say(spawn("inProgress", json!([]))).await;
    let asks = |id: i64, thread: &str| json!({"id": id, "method": "item/commandExecution/requestApproval", "params": {"threadId": thread, "turnId": "st1", "itemId": "c", "command": "ls"}});
    fake.say(asks(8, "subA")).await;
    fake.say(asks(9, "th-other")).await;
    let written = written_so_far(&mut fake).await;
    assert!(!replied(&written, 9), "held while the spawn is under way");
    fake.say(spawn("completed", json!(["subA"]))).await;
    let reply = reply_to(&mut fake, 9).await;
    let reply = reply.expect("the request still for no chat was never answered");
    assert_eq!(reply["error"], no_chat_has("th-other"));
    let item = asked
        .recv()
        .await
        .expect("the subagent's request waits in the chat");
    assert_eq!(item, "approval:st1:8");
    assert!(asked.try_recv().is_err(), "and only it");
    reading.abort();
}

/// A request held while a subagent starts, which Codex then says is resolved, needs no reply:
/// Codex waits for it no longer. It is not refused once the spawn is done.
#[tokio::test]
async fn a_held_request_codex_resolves_is_not_refused() {
    let (mut codex, mut fake) = running().await;
    let reading = tokio::spawn(async move { until(&mut codex, |_| false).await });
    let spawn = |method: &str, status: &str| {
        let item = json!({"type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent",
            "status": status, "receiverThreadIds": ["subA"], "prompt": "look"});
        json!({"method": method, "params": {"threadId": "th1", "turnId": "t1", "item": item}})
    };
    fake.say(spawn("item/started", "inProgress")).await;
    fake.say(json!({"id": 9, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th-other", "turnId": "t9", "itemId": "c", "command": "ls"}})).await;
    fake.say(json!({"method": "serverRequest/resolved", "params": {"threadId": "th-other", "requestId": 9}}))
        .await;
    fake.say(spawn("item/completed", "completed")).await;
    let written = written_so_far(&mut fake).await;
    reading.abort();
    assert!(
        !replied(&written, 9),
        "a resolved request was refused: {written:?}"
    );
}

/// The error a request the driver cannot show is refused with.
fn cannot_show() -> Value {
    json!({"code": -32602, "message": "Invalid params"})
}

/// A question the driver cannot show, such as one with two questions of one id, is refused at
/// once, as a form the chat cannot show is declined. Found by the `DriverTransport` model
/// (`formal/tla`, case `codex-unshown`): the driver left it unanswered, and the turn waited for
/// it until someone interrupted.
#[tokio::test]
async fn a_question_the_driver_cannot_show_is_answered() {
    let (mut codex, mut fake) = running().await;
    let reading = tokio::spawn(async move { until(&mut codex, is_turn(State::Done)).await });
    fake.say(json!({"id": 6, "method": "item/tool/requestUserInput", "params": {"threadId": "th1", "turnId": "t1", "itemId": "call1", "questions": [
        {"id": "q", "header": "One", "question": "First?", "isOther": true, "isSecret": false, "options": []},
        {"id": "q", "header": "Two", "question": "Second?", "isOther": true, "isSecret": false, "options": []}]}})).await;
    let reply = reply_to(&mut fake, 6).await;
    let reply = reply.expect("the question the driver cannot show was never answered");
    assert_eq!(reply["error"], cannot_show());
    fake.say(turn("turn/completed", "completed")).await;
    reading.await.unwrap();
}

/// So is an approval whose params the driver cannot read, and a request of a kind it has no rule
/// for, as an unknown method.
#[tokio::test]
async fn a_request_the_driver_cannot_read_is_answered() {
    let (mut codex, mut fake) = running().await;
    let reading = tokio::spawn(async move { until(&mut codex, is_turn(State::Done)).await });
    fake.say(json!({"id": 10, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": ["not", "text"]}})).await;
    let reply = reply_to(&mut fake, 10).await;
    let reply = reply.expect("the approval the driver cannot read was never answered");
    assert_eq!(reply["error"], cannot_show());
    fake.say(json!({"id": 11, "method": "item/something/new", "params": {"threadId": "th1", "turnId": "t1"}})).await;
    let reply = reply_to(&mut fake, 11).await;
    let reply = reply.expect("the request of an unknown kind was never answered");
    assert_eq!(
        reply["error"],
        json!({"code": -32601, "message": "Method not found"})
    );
    fake.say(turn("turn/completed", "completed")).await;
    reading.await.unwrap();

/// A turn running with subagent `sub1` running turn `st1` under it, then the turn ended: the
/// subagent outlives it.
async fn outliving() -> (Codex, Fake) {
    let (mut codex, mut fake) = running().await;
    fake.say(started_by("a1", "sub1")).await;
    fake.say(sub_turn("turn/started", "sub1", "st1", "inProgress"))
        .await;
    fake.say(turn("turn/completed", "completed")).await;
    until(&mut codex, is_turn(State::Done)).await;
    (codex, fake)
}

// What the parent's thread says of a subagent after the parent's turn has ended (recorded in
// `subagent_interrupt`) is shown under the turn it names, not under none.
#[tokio::test]
async fn a_subagents_end_after_its_parents_turn_keeps_that_turn() {
    let (mut codex, mut fake) = outliving().await;
    let ended = json!({"type": "subAgentActivity", "id": "a2", "kind": "completed",
        "agentThreadId": "sub1", "agentPath": "/root/a"});
    fake.say(
        json!({"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1",
        "item": ended}}),
    )
    .await;
    let events = until(&mut codex, |e| e.event.header().item.as_str() == "a2").await;
    let header = events.last().unwrap().event.header();
    assert_eq!(header.turn.as_ref().map(|t| t.as_str()), Some("t1"));
}

// Work given to a subagent while its turn runs is for its next turn: the running turn's words
// still name what it is working for.
#[tokio::test]
async fn work_given_during_a_subagents_turn_is_for_its_next() {
    let (mut codex, mut fake) = running().await;
    fake.say(started_by("a1", "sub1")).await;
    fake.say(sub_turn("turn/started", "sub1", "st1", "inProgress"))
        .await;
    let more = json!({"type": "subAgentActivity", "id": "a2", "kind": "interacted",
        "agentThreadId": "sub1", "agentPath": "/root/a"});
    fake.say(
        json!({"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1",
        "item": more}}),
    )
    .await;
    fake.say(
        json!({"method": "item/completed", "params": {"threadId": "sub1", "turnId": "st1",
        "item": {"type": "agentMessage", "id": "w1", "text": "first", "phase": "final_answer"}}}),
    )
    .await;
    fake.say(sub_turn("turn/completed", "sub1", "st1", "completed"))
        .await;
    fake.say(sub_turn("turn/started", "sub1", "st2", "inProgress"))
        .await;
    let events = until(&mut codex, |e| e.event.header().item.as_str() == "task:st2").await;
    let parent = (events.iter()).find_map(|e| match e.event.body() {
        Body::Message(m) if e.event.header().item.as_str() == "w1" => Some(m.parent.clone()),
        _ => None,
    });
    assert_eq!(parent, Some(Some("a1".into())), "the running turn's words");
    let subject = |item: &str| {
        (events.iter().rev()).find_map(|e| match e.event.body() {
            Body::Task(t) if e.event.header().item.as_str() == item => Some(t.subject.clone()),
            _ => None,
        })
    };
    assert_eq!(subject("task:st1"), Some(Some("a1".into())));
    assert_eq!(subject("task:st2"), Some(Some("a2".into())));
}

// A tool for subagents that failed is not titled as having worked.
#[tokio::test]
async fn a_failed_spawn_is_not_shown_as_started() {
    let (mut codex, mut fake) = running().await;
    let spawn = json!({"type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent",
        "status": "failed", "receiverThreadIds": [], "prompt": "look"});
    fake.say(
        json!({"method": "item/completed", "params": {"threadId": "th1", "turnId": "t1",
        "item": spawn}}),
    )
    .await;
    let events = until(&mut codex, |e| e.event.header().item.as_str() == "c1").await;
    let header = events.last().unwrap().event.header();
    assert_eq!(header.state, State::Failed);
    assert_ne!(header.title, "Started a subagent");
}
