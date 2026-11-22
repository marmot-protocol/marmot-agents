//! The fakes themselves: what the checks rely on them to report.

use marmot_agent_conformance::Checker;
use marmot_agent_conformance::fake::acp::{self, Flavor};
use marmot_agent_conformance::fake::{Fake, Heard, Knobs, Outbox, Protocol, codex};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A harness that says a great deal as it starts, and nothing after.
struct Chatty;

impl Protocol for Chatty {
    fn start(&mut self, out: &mut Outbox, _heard: &mut Vec<Heard>) {
        for n in 0..200 {
            out.say(json!({"method": "note", "params": {"n": n, "pad": "x".repeat(100)}}));
        }
    }

    fn hear(&mut self, _frame: &Value, _out: &mut Outbox, _heard: &mut Vec<Heard>) {}
}

/// A harness that hears and says nothing.
struct Silent;

impl Protocol for Silent {
    fn hear(&mut self, _frame: &Value, _out: &mut Outbox, _heard: &mut Vec<Heard>) {}
}

// A driver that stops reading leaves the harness blocked in a write. A kill still ends it: the death
// is counted, and the driver's end of the pipe reaches its end.
#[tokio::test]
async fn a_kill_ends_a_harness_the_driver_stopped_reading() {
    let fake = Fake::new();
    let (mut driver, harness) = tokio::io::duplex(256);
    let (reads, writes) = tokio::io::split(harness);
    fake.serve(reads, writes, Chatty, None);
    // The harness fills the pipe, and blocks.
    tokio::time::sleep(Duration::from_millis(50)).await;
    fake.kill();
    let died = fake
        .wait_heard(Duration::from_secs(2), |_| fake.deaths() == 1)
        .await;
    assert!(died, "a harness blocked in a write did not die when killed");
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(2), driver.read_to_end(&mut rest)).await;
    assert!(read.is_ok(), "the driver's end never reached its end");
}

// A death counts as a harness dying reading what was written for a chat only when it is that chat's
// harness that read and died.
#[tokio::test]
async fn a_death_counts_only_for_its_own_harness() {
    let fake = Fake::new();
    let mut ends = Vec::new();
    for tag in ["a", "b"] {
        let (driver, harness) = tokio::io::duplex(256);
        let (reads, writes) = tokio::io::split(harness);
        fake.serve(reads, writes, Silent, Some(tag.to_string()));
        ends.push(driver);
    }
    let mark = fake.mark();
    // "a" reads its line; "b" dies reading the second line read in all.
    fake.set_knobs(Knobs {
        die_after: Some(2),
        ..Knobs::default()
    });
    ends[0].write_all(b"{\"n\": 1}\n").await.expect("a writes");
    let read = fake
        .wait_heard(Duration::from_secs(2), |_| fake.frames() == 1)
        .await;
    assert!(read, "the first harness never read its line");
    ends[1].write_all(b"{\"n\": 2}\n").await.expect("b writes");
    let died = fake
        .wait_heard(Duration::from_secs(2), |_| fake.deaths() == 1)
        .await;
    assert!(died, "the second harness never died");
    assert!(
        !fake.died_reading(mark, |tag| tag == Some("a")),
        "the first harness read, and the second died, but that is no death of the first"
    );
    assert!(
        fake.frames() > 0 && fake.deaths() > 0,
        "what the old check took as the first harness dying reading: any line read, any death"
    );
    assert!(fake.died_reading(mark, |tag| tag == Some("b")));
    assert!(
        !fake.died_reading(fake.mark(), |_| true),
        "a death before the mark counted"
    );
}

/// Whether the checker counts what was heard against S4.
fn counted_twice(heard: &[Heard]) -> bool {
    let mut check = Checker::new();
    check.heard(heard, &[]);
    (check.violations().iter()).any(|v| v.property == "S4")
}

fn refused(heard: &[Heard]) -> bool {
    (heard.iter()).any(|h| matches!(h, Heard::Prompt { refused: true, .. }))
}

// A copy of a prompt that an ACP agent refuses, because its session is busy, reached it all the
// same, and S4 counts it.
#[test]
fn a_refused_copy_of_a_prompt_reaches_an_acp_agent() {
    for flavor in [Flavor::OpenCode, Flavor::Goose, Flavor::Cursor] {
        let mut agent = acp::Acp::new(flavor, acp::Shared::default());
        let (mut out, mut heard) = (Outbox::default(), Vec::new());
        let new = json!({"jsonrpc": "2.0", "id": 1, "method": "session/new", "params": {}});
        agent.hear(&new, &mut out, &mut heard);
        let Some(Heard::Opened { session, .. }) = heard.first().cloned() else {
            panic!("no session opened");
        };
        let prompt = |id: u64| {
            json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt", "params": {
                "sessionId": session, "prompt": [{"type": "text", "text": "<<c-1>> m-1 #sleep"}]}})
        };
        agent.hear(&prompt(2), &mut out, &mut heard);
        agent.hear(&prompt(3), &mut out, &mut heard);
        assert!(refused(&heard), "{flavor:?}: the copy was not refused");
        assert!(
            counted_twice(&heard),
            "{flavor:?}: a refused copy was not counted"
        );
    }
}

// A copy of a prompt that Codex refuses, because its turn is running or the copy names another,
// reached it all the same, and S4 counts it.
#[test]
fn a_refused_copy_of_a_prompt_reaches_codex() {
    for copy in ["turn/start", "turn/steer"] {
        let mut codex = codex::Codex::new(codex::Shared::default());
        let (mut out, mut heard) = (Outbox::default(), Vec::new());
        let start = json!({"id": 1, "method": "thread/start", "params": {}});
        codex.hear(&start, &mut out, &mut heard);
        let Some(Heard::Opened { session, .. }) = heard.first().cloned() else {
            panic!("no thread opened");
        };
        let input = json!([{"type": "text", "text": "<<c-1>> m-1 #sleep"}]);
        let first = json!({"id": 2, "method": "turn/start", "params": {"threadId": session, "input": input}});
        codex.hear(&first, &mut out, &mut heard);
        let again = json!({"id": 3, "method": copy, "params": {"threadId": session, "input": input,
            "expectedTurnId": "not-the-turn"}});
        codex.hear(&again, &mut out, &mut heard);
        assert!(refused(&heard), "{copy}: the copy was not refused");
        assert!(
            counted_twice(&heard),
            "{copy}: a refused copy was not counted"
        );
    }
}
