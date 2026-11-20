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
