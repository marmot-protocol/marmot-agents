//! The scenarios every driver must pass. Each takes a fresh subject, drives it as the core would,
//! and returns the first property broken. The checker looks after every step; a scenario adds
//! only what it alone can tell, such as that a named session was taken up.

use crate::check::{Violation, broke};
use crate::fake::{Heard, Knobs};
use crate::run::{Run, Subject, turn_ended, waiting};
use marmot_agent_driver::{ChatId, Emitted, MessageId, Output};
use marmot_agent_events::{Body, State};
use std::time::Duration;

/// Every scenario, by name, for a test to run one at a time.
pub const ALL: [&str; 15] = [
    "one_turn",
    "two_chats",
    "answered_once",
    "answer_after_death",
    "dies_mid_turn",
    "dies_with_requests_pending",
    "dropped_waits",
    "slow_and_out_of_order",
    "partial_frames",
    "never_answers",
    "stops_reading",
    "late_replies",
    "restart",
    "new_session",
    "settings",
];

/// Runs the scenario called `name`.
pub async fn run<S: Subject>(name: &str, subject: S) -> Result<(), Violation> {
    match name {
        "one_turn" => one_turn(subject).await,
        "two_chats" => two_chats(subject).await,
        "answered_once" => answered_once(subject).await,
        "answer_after_death" => answer_after_death(subject).await,
        "dies_mid_turn" => dies_mid_turn(subject).await,
        "dies_with_requests_pending" => dies_with_requests_pending(subject).await,
        "dropped_waits" => dropped_waits(subject).await,
        "slow_and_out_of_order" => slow_and_out_of_order(subject).await,
        "partial_frames" => partial_frames(subject).await,
        "never_answers" => never_answers(subject).await,
        "stops_reading" => stops_reading(subject).await,
        "late_replies" => late_replies(subject).await,
        "restart" => restart(subject).await,
        "new_session" => new_session(subject).await,
        "settings" => settings(subject).await,
        other => panic!("no scenario called {other}"),
    }
}

/// A turn of the chat's that responds to `message`.
fn turn_of(message: &MessageId) -> impl Fn(&Emitted) -> bool + '_ {
    move |e| matches!(e.event.body(), Body::Turn(_)) && e.prompts.contains(message)
}

fn ended_turn_of(message: &MessageId) -> impl Fn(&Emitted) -> bool + '_ {
    move |e| turn_ended(e) && e.prompts.contains(message)
}

fn of_chat<'a>(chat: &'a str, f: impl Fn(&Emitted) -> bool + 'a) -> impl Fn(&Emitted) -> bool + 'a {
    move |e| e.chat.as_ref().is_some_and(|c| c.0 == chat) && f(e)
}

fn taken<T>(result: Result<T, marmot_agent_driver::Refusal>, what: &str) -> Result<T, Violation> {
    result.map_err(|r| broke("L3", format!("{what} was refused: {r:?}")))
}

/// One prompt, answered in words.
pub async fn one_turn<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let m = taken(run.prompt("c-1", "hello").await?, "the prompt")?;
    run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    run.end().await
}

/// The body of the two-chat scenarios: chat 1 runs a command until interrupted, while chat 2's
/// harness asks for approval, which is given; then chat 1 is interrupted. `lost` names what a
/// wait that runs out breaks.
async fn two_chats_with<S: Subject>(run: &mut Run<S>, lost: &'static str) -> Result<(), Violation> {
    let sleeping = taken(run.prompt("c-1", "#sleep").await?, "chat 1's prompt")?;
    let asking = taken(run.prompt("c-2", "#ask").await?, "chat 2's prompt")?;
    let asked = run
        .wait(lost, "chat 2's approval", of_chat("c-2", waiting))
        .await?;
    let turn = run.wait(lost, "chat 1's turn", turn_of(&sleeping)).await?;
    let turn = turn.event.header().turn.clone().expect("a turn has an id");
    taken(run.answer(&asked, true).await?.1, "the answer")?;
    run.wait(lost, "chat 2's turn's end", ended_turn_of(&asking))
        .await?;
    taken(run.interrupt("c-1", &turn).await?.1, "the interrupt")?;
    run.wait(lost, "chat 1's turn's end", ended_turn_of(&sleeping))
        .await?;
    run.end().await
}

/// Two chats at once: each event, prompt and answer stays in its own chat (S5), and each request
/// is settled once (S1).
pub async fn two_chats<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    two_chats_with(&mut run, "L2").await
}

/// An answer is settled once, and the same item answered again is refused, not settled again.
/// Both chats are answered, one allowing and one refusing, so an answer that reached the other
/// chat's request would be heard with the wrong choice (S5).
pub async fn answered_once<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let one = taken(run.prompt("c-1", "#ask").await?, "chat 1's prompt")?;
    let two = taken(run.prompt("c-2", "#quiz").await?, "chat 2's prompt")?;
    let asked_one = run
        .wait("L2", "chat 1's approval", of_chat("c-1", waiting))
        .await?;
    let asked_two = run
        .wait("L2", "chat 2's question", of_chat("c-2", waiting))
        .await?;
    taken(run.answer(&asked_two, false).await?.1, "chat 2's answer")?;
    taken(run.answer(&asked_one, true).await?.1, "chat 1's answer")?;
    run.wait("L2", "chat 1's turn's end", ended_turn_of(&one))
        .await?;
    run.wait("L2", "chat 2's turn's end", ended_turn_of(&two))
        .await?;
    let (_, again) = run.answer(&asked_one, true).await?;
    if again.is_ok() {
        return Err(broke("S1", "an item answered twice took the second answer"));
    }
    run.end().await
}

/// The harness dies while its approval waits, and the driver, not yet told, is given the answer:
/// it cannot hand the answer over, so it must not take it, nor show it as given (S11, S1). The
/// approval is closed unanswered instead, and the core settles the answer as failed.
pub async fn answer_after_death<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    taken(run.prompt("c-1", "#ask").await?, "the prompt")?;
    let asked = run
        .wait("L2", "the approval", of_chat("c-1", waiting))
        .await?;
    run.kill("c-1");
    let fake = run.subject.fake().clone();
    if !fake.wait_heard(run.patience, |_| fake.deaths() > 0).await {
        return Err(broke("L2", "the harness did not die"));
    }
    // A harness run as a program goes a moment after its output closes.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (request, result) = run.answer(&asked, true).await?;
    run.drain(Duration::from_millis(200)).await;
    let settled = run.seen.iter().any(|e| e.settles.contains(&request));
    if result.is_ok() && settled {
        return Err(broke(
            "S11",
            "an answer the harness could not have read was taken and shown as given",
        ));
    }
    run.checkpoint().await?;
    run.check.closed_after_stop();
    run.ok()
}

/// The harness dies in the middle of a turn: every item closes (S2), and nothing the driver
/// makes for the stop settles anything. A driver with a harness for each chat starts it again
/// for the next prompt, in the chat's own session (S12).
pub async fn dies_mid_turn<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let m = taken(run.prompt("c-1", "#die").await?, "the prompt")?;
    run.wait("L2", "the turn", turn_of(&m)).await?;
    run.wait(
        "S2",
        "the turn's end, after the harness died",
        ended_turn_of(&m),
    )
    .await?;
    run.checkpoint().await?;
    run.check.closed_after_stop();
    run.ok()?;
    if !run.subject.shared_harness() {
        let again = taken(run.prompt("c-1", "hello").await?, "the next prompt")?;
        run.wait("L2", "the next turn's end", ended_turn_of(&again))
            .await?;
    }
    run.end().await
}

/// The harness dies just after it reads an interrupt, before acting on it, while another chat's
/// answer has been settled. The turn the driver closes for the stop must not settle the interrupt
/// (S1); every item closes (S2).
pub async fn dies_with_requests_pending<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let sleeping = taken(run.prompt("c-1", "#sleep").await?, "chat 1's prompt")?;
    let asking = taken(run.prompt("c-2", "#ask").await?, "chat 2's prompt")?;
    let asked = run
        .wait("L2", "chat 2's approval", of_chat("c-2", waiting))
        .await?;
    taken(run.answer(&asked, true).await?.1, "the answer")?;
    run.wait("L2", "chat 2's turn's end", ended_turn_of(&asking))
        .await?;
    let turn = run.wait("L2", "chat 1's turn", turn_of(&sleeping)).await?;
    let turn = turn.event.header().turn.clone().expect("a turn has an id");
    let fake = run.subject.fake().clone();
    fake.set_knobs(Knobs {
        die_after: Some(fake.frames() + 1),
        ..Knobs::default()
    });
    let _ = run.interrupt("c-1", &turn).await?;
    run.wait(
        "S2",
        "chat 1's turn's end, after the harness died",
        ended_turn_of(&sleeping),
    )
    .await?;
    run.checkpoint().await?;
    run.check.closed_after_stop();
    run.end().await
}

/// The two-chat scenario while the core drops every wait for an event at the first point it
/// would wait, and the harness writes every frame in pieces (S11: nothing lost, nothing halved).
pub async fn dropped_waits<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    run.subject.fake().set_knobs(Knobs {
        partial: true,
        ..Knobs::default()
    });
    run.dropping = true;
    run.patience = Duration::from_secs(10);
    two_chats_with(&mut run, "S11").await
}

/// The two-chat scenario with replies late, and after what follows from them, and overtaking
/// one another; for a harness that keeps its order, with everything it says slow.
pub async fn slow_and_out_of_order<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let knobs = if run.subject.reorders() {
        Knobs {
            late: Some(Duration::from_millis(15)),
            reorder: true,
            ..Knobs::default()
        }
    } else {
        Knobs {
            slow: Some(Duration::from_millis(5)),
            ..Knobs::default()
        }
    };
    run.subject.fake().set_knobs(knobs);
    two_chats_with(&mut run, "L2").await
}

/// The two-chat scenario with every frame from the harness written in pieces.
pub async fn partial_frames<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    run.subject.fake().set_knobs(Knobs {
        partial: true,
        ..Knobs::default()
    });
    two_chats_with(&mut run, "L2").await
}

/// The harness never answers the request a prompt waits on: the prompt still returns, taken or
/// refused, within the suite's patience (S11, D-066: a harness that never answers is a driver
/// that stopped working, and ending it is the remedy). A refused prompt stays refused: when the
/// harness answers at last, nothing in the chat follows from it.
pub async fn never_answers<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let never = run.subject.never().to_string();
    run.subject.fake().set_knobs(Knobs {
        never: vec![never],
        ..Knobs::default()
    });
    run.patience = Duration::from_secs(6);
    run.loose = true; // the harness heard it, and will not say so
    let refused = run.prompt("c-1", "hello").await?.is_err();
    run.drain(Duration::from_millis(200)).await;
    // The harness answers, late.
    let before = run.seen.len();
    run.subject.fake().set_knobs(Knobs::default());
    tokio::time::sleep(Duration::from_millis(300)).await;
    run.drain(Duration::from_millis(100)).await;
    if refused && run.seen.len() > before {
        return Err(broke(
            "S11",
            "a reply that came after the prompt was refused reached the chat",
        ));
    }
    run.end().await
}

/// What a prompt says to be longer than any pipe between the driver and the harness, so that
/// writing it fills the pipe.
fn longer_than_a_pipe(words: &str) -> String {
    format!("{words} {}", "x".repeat(1 << 20))
}

/// The harness stops reading its input, keeping it open, while a prompt too long for the pipe is
/// written: the prompt still returns, within the suite's patience (S11, D-066). Refused, it stays
/// refused: when the harness reads again, it never reads the prompt.
pub async fn stops_reading<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let first = taken(run.prompt("c-1", "hello").await?, "the first prompt")?;
    run.wait("L2", "the first turn's end", ended_turn_of(&first))
        .await?;
    run.subject.fake().set_knobs(Knobs {
        deaf: true,
        ..Knobs::default()
    });
    let stuck = "#stuck";
    let refused = run
        .prompt("c-1", &longer_than_a_pipe(stuck))
        .await?
        .is_err();
    // The harness reads again.
    let fake = run.subject.fake().clone();
    fake.set_knobs(Knobs::default());
    let read = |heard: &[Heard]| {
        (heard.iter()).any(|h| matches!(h, Heard::Prompt { text, .. } if text.contains(stuck)))
    };
    if refused && fake.wait_heard(Duration::from_secs(1), read).await {
        return Err(broke(
            "S11",
            "a prompt the driver refused reached the harness once it read again",
        ));
    }
    run.end().await
}

/// Every reply from the harness comes late, but well within the driver's patience: a prompt in a
/// new chat, which waits for the harness more than once, is still taken, and its turn ends. For a
/// harness that keeps its order, everything it says is slow instead.
pub async fn late_replies<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let knobs = if run.subject.reorders() {
        Knobs {
            late: Some(Duration::from_millis(500)),
            ..Knobs::default()
        }
    } else {
        Knobs {
            slow: Some(Duration::from_millis(100)),
            ..Knobs::default()
        }
    };
    run.subject.fake().set_knobs(knobs);
    run.patience = Duration::from_secs(10);
    let m = taken(run.prompt("c-1", "hello").await?, "the prompt")?;
    run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    run.end().await
}

/// The driver restarts, as the core's restart drops it: the next prompt names the chat's session,
/// which is taken up (D-065), and no item id from before is used again (S13). A prompt that names
/// none goes to the same session (S12). A restart in the middle of a turn is survived the same way.
pub async fn restart<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let first = taken(run.prompt("c-1", "hello").await?, "the first prompt")?;
    run.wait("L2", "the first turn's end", ended_turn_of(&first))
        .await?;
    let chat = ChatId::from("c-1");
    let session = (run.check.session_of(&chat).cloned())
        .ok_or_else(|| broke("L2", "the chat's session was never shown"))?;
    run.restart().await?;
    resumes(&mut run, &session).await?;
    let more = taken(
        run.prompt("c-1", "and more").await?,
        "a prompt naming no session",
    )?;
    run.wait("L2", "that turn's end", ended_turn_of(&more))
        .await?;
    run.checkpoint().await?;
    let heard = run.subject.fake().heard();
    let went = heard.iter().rev().find_map(|h| match h {
        Heard::Prompt { session, text } if text.contains(&format!(" {} ", more.0)) => {
            Some(session.clone())
        }
        _ => None,
    });
    if went.as_ref() != Some(&session) {
        return Err(broke(
            "S12",
            "a prompt naming no session left the chat's session",
        ));
    }
    // Again, with a turn running when the driver goes.
    let sleeping = taken(run.prompt("c-1", "#sleep").await?, "a long prompt")?;
    run.wait("L2", "the long turn", turn_of(&sleeping)).await?;
    run.restart().await?;
    resumes(&mut run, &session).await?;
    run.end().await
}

/// A prompt naming `session` takes it up, and its turn ends.
async fn resumes<S: Subject>(run: &mut Run<S>, session: &str) -> Result<(), Violation> {
    let before = run.subject.fake().heard().len();
    let again = taken(
        run.prompt_in("c-1", "where were we", Some(session)).await?,
        "a prompt naming the chat's session",
    )?;
    run.wait("L2", "the resumed turn's end", ended_turn_of(&again))
        .await?;
    let heard = run.subject.fake().heard();
    let taken_up = heard[before..]
        .iter()
        .any(|h| matches!(h, Heard::Opened { session: s, resumed: true } if s == session));
    if !taken_up {
        return Err(broke("S12", "the session a prompt named was not taken up"));
    }
    Ok(())
}

/// A new session for a chat: settled once, by the new session (S1); the next prompt goes to it
/// (S12); and another chat naming the old session is refused (S5).
pub async fn new_session<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let first = taken(run.prompt("c-1", "hello").await?, "the first prompt")?;
    run.wait("L2", "the first turn's end", ended_turn_of(&first))
        .await?;
    let chat = ChatId::from("c-1");
    let old = (run.check.session_of(&chat).cloned())
        .ok_or_else(|| broke("L2", "the chat's session was never shown"))?;
    let (request, result) = run.new_session("c-1", Some(&old)).await?;
    taken(result, "the new session")?;
    let fresh = run
        .wait("L1", "the new session", |e| {
            matches!(e.event.body(), Body::Session(_))
                && e.event.header().state == State::Running
                && e.event
                    .header()
                    .session
                    .as_ref()
                    .is_some_and(|s| s.as_str() != old)
        })
        .await?;
    let new = fresh.event.header().session.clone().expect("a session");
    run.checkpoint().await?;
    let settled = run.seen.iter().any(|e| e.settles.contains(&request));
    if !settled {
        return Err(broke("S1", "the new session was never settled"));
    }
    let next = taken(run.prompt("c-1", "hello again").await?, "the next prompt")?;
    run.wait("L2", "the next turn's end", ended_turn_of(&next))
        .await?;
    run.checkpoint().await?;
    let heard = run.subject.fake().heard();
    let went = heard.iter().rev().find_map(|h| match h {
        Heard::Prompt { session, text } if text.contains(&format!(" {} ", next.0)) => {
            Some(session.clone())
        }
        _ => None,
    });
    if went.as_deref() != Some(new.as_str()) {
        return Err(broke(
            "S12",
            "the prompt after a new session went to another",
        ));
    }
    if run.prompt_in("c-2", "let me in", Some(&old)).await?.is_ok() {
        return Err(broke("S5", "a chat took up another chat's session"));
    }
    run.end().await
}

/// A setting changed: the control is settled once (S1).
pub async fn settings<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let first = taken(run.prompt("c-1", "hello").await?, "the prompt")?;
    run.wait("L2", "the turn's end", ended_turn_of(&first))
        .await?;
    let chat = ChatId::from("c-1");
    let session = (run.check.session_of(&chat).cloned())
        .ok_or_else(|| broke("L2", "the chat's session was never shown"))?;
    let (request, result) = run.set("c-1", &session).await?;
    taken(result, "the setting")?;
    let deadline = tokio::time::Instant::now() + run.patience;
    while !run.seen.iter().any(|e| e.settles.contains(&request)) {
        if tokio::time::Instant::now() > deadline {
            return Err(broke("L1", "the setting was never settled"));
        }
        run.drain(Duration::from_millis(20)).await;
    }
    run.end().await
}

/// The harness dies on reading its `step`-th frame after the driver started, in a turn that asks
/// for approval: whatever the step, every request is settled once, every item closes (S1, S2),
/// and nothing panics.
pub async fn dies_at_step<S: Subject>(subject: S, step: usize) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let fake = run.subject.fake().clone();
    fake.set_knobs(Knobs {
        die_after: Some(fake.frames() + step),
        ..Knobs::default()
    });
    run.patience = Duration::from_secs(3);
    if run.prompt("c-1", "#ask").await?.is_ok() {
        // Whatever comes, until the driver ends or has nothing more: an approval is answered.
        loop {
            match tokio::time::timeout(Duration::from_millis(300), run.next()).await {
                Ok(Some(Output::Event(e))) if waiting(&e) => {
                    let _ = run.answer(&e, true).await?;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
    }
    run.drain(Duration::from_millis(150)).await;
    run.checkpoint().await?;
    if fake.deaths() > 0 {
        run.check.closed_after_stop();
    }
    run.end().await
}

/// Replays a recording through the driver: one prompt, every approval allowed, until its turn
/// ends. The subject's fake is a [`Replay`](crate::fake::Replay).
pub async fn replay<S: Subject>(subject: S, words: &str) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let m = taken(run.prompt("c-1", words).await?, "the prompt")?;
    loop {
        let e = run
            .until("L2", "the recorded turn's end", |e| {
                waiting(e) || (turn_ended(e) && e.prompts.contains(&m))
            })
            .await?;
        if !waiting(&e) {
            break;
        }
        taken(run.answer(&e, true).await?.1, "the answer")?;
    }
    run.drain(Duration::from_millis(100)).await;
    run.check.all_settled();
    run.ok()
}
