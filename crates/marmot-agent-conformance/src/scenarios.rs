//! The scenarios every driver must pass. Each takes a fresh subject, drives it as the core would,
//! and returns the first property broken. The checker looks after every step; a scenario adds
//! only what it alone can tell, such as that a named session was taken up.

use crate::check::{Violation, broke, mark};
use crate::fake::{Heard, Knobs, Progress, file_for};
use crate::run::{Run, Subject, turn_ended, waiting};
use marmot_agent_driver::{ChatId, Driver, Emitted, MessageId, Output, Reason, Refusal, SendFile};
use marmot_agent_events::{Body, State};
use std::time::Duration;

/// The one list of the scenarios every driver must pass. It calls the macro it is given with what
/// it was given, then `;` and every scenario's name: a test file generates one test for each,
/// and the suite's own [`ALL`] and [`run`] are made from it, so a scenario added here is run by
/// every driver that runs them all, and must be named by every driver that runs only some.
///
/// ```ignore
/// macro_rules! conformance {
///     ($make:expr; $($name:ident),*) => { $( #[tokio::test] async fn $name() { ... } )* };
/// }
/// marmot_agent_conformance::with_scenarios!(conformance!(OnFake::default()));
/// ```
#[macro_export]
macro_rules! with_scenarios {
    ($then:ident ! ($($given:tt)*)) => {
        $then!(
            $($given)*;
            one_turn,
            two_chats,
            answered_once,
            answer_after_death,
            answer_after_interrupt,
            dies_mid_turn,
            dies_with_requests_pending,
            dropped_waits,
            slow_and_out_of_order,
            partial_frames,
            never_answers,
            stops_reading,
            late_replies,
            restart,
            new_session,
            folder,
            send_file,
            settings
        );
    };
}

macro_rules! registry {
    (; $($name:ident),*) => {
        /// Every scenario, by name, from [`with_scenarios`](crate::with_scenarios).
        pub const ALL: &[&str] = &[$(stringify!($name)),*];

        /// Runs the scenario called `name`.
        pub async fn run<S: Subject>(name: &str, subject: S) -> Result<(), Violation> {
            match name {
                $(stringify!($name) => $name(subject).await,)*
                other => panic!("no scenario called {other}"),
            }
        }
    };
}

crate::with_scenarios!(registry!());

/// For a driver that runs only some scenarios: those it runs and those it leaves out, with a
/// reason, between them name every scenario, once. Gives back what is wrong.
pub fn covered(run: &[&str], left_out: &[&str]) -> Result<(), String> {
    let mut wrong = Vec::new();
    for name in ALL {
        match (run.contains(name), left_out.contains(name)) {
            (true, true) => wrong.push(format!("{name} is both run and left out")),
            (false, false) => wrong.push(format!("{name} is neither run nor left out")),
            _ => {}
        }
    }
    for name in run.iter().chain(left_out) {
        if !ALL.contains(name) {
            wrong.push(format!("{name} is no scenario"));
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(wrong.join("; "))
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
/// it cannot hand the answer over, so it must refuse it (S11), and the approval is closed
/// unanswered (S2). The core settles the answer as failed.
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
    let (_, result) = run.answer(&asked, true).await?;
    if result.is_ok() {
        return Err(broke(
            "S11",
            "an answer to a harness that had died was taken",
        ));
    }
    run.drain(Duration::from_millis(200)).await;
    run.checkpoint().await?;
    run.check.closed_after_stop();
    run.ok()?;
    // Closed, and with no answer shown: nobody's answer reached the harness.
    let item = asked.event.header().item.clone();
    let last = (run.seen.iter().rev())
        .find(|e| e.event.header().item == item)
        .expect("the approval was seen");
    let answered = match last.event.body() {
        Body::Approval(a) => a.answer.is_some(),
        Body::Question(q) => q.answer().is_some(),
        _ => false,
    };
    if waiting(last) || answered {
        return Err(broke(
            "S2",
            "the approval was not closed unanswered after the harness died",
        ));
    }
    Ok(())
}

/// A turn is interrupted while its approval waits: the harness drops what it asked, and the
/// approval closes with the turn. An answer given after that must be refused, and must never
/// reach the harness (S11): an answer reaches the harness only while its item waits.
pub async fn answer_after_interrupt<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let m = taken(run.prompt("c-1", "#ask").await?, "the prompt")?;
    let asked = run
        .wait("L2", "the approval", of_chat("c-1", waiting))
        .await?;
    let turn = run.wait("L2", "the turn", turn_of(&m)).await?;
    let turn = turn.event.header().turn.clone().expect("a turn has an id");
    taken(run.interrupt("c-1", &turn).await?.1, "the interrupt")?;
    run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    let item = asked.event.header().item.clone();
    let last = (run.seen.iter().rev())
        .find(|e| e.event.header().item == item)
        .expect("the approval was seen");
    if waiting(last) {
        return Err(broke(
            "S2",
            "the approval still waits after its turn was interrupted",
        ));
    }
    let (_, late) = run.answer(&asked, true).await?;
    if late.is_ok() {
        return Err(broke(
            "S11",
            "an answer to an approval closed with its interrupted turn was taken",
        ));
    }
    run.drain(Duration::from_millis(200)).await;
    run.end().await
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
    let deaths = fake.deaths();
    fake.set_knobs(Knobs {
        die_after: Some(fake.frames() + 1),
        ..Knobs::default()
    });
    // The driver may take the interrupt or refuse it once the harness has gone, but it must write
    // it: a harness that never read it never died, and nothing below would be checked.
    let _ = run.interrupt("c-1", &turn).await?;
    if !fake
        .wait_heard(run.patience, |_| fake.deaths() > deaths)
        .await
    {
        return Err(broke(
            "L2",
            "the harness did not die on the interrupt, so nothing was checked",
        ));
    }
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
///
/// With a harness for each chat, only the first chat's stops reading. The second chat's goes on
/// serving it, without being started again, while the first is stuck and after; and the first
/// chat's next prompt is taken, by a harness started again for it.
pub async fn stops_reading<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let apart = !run.subject.shared_harness();
    let first = taken(run.prompt("c-1", "hello").await?, "the first prompt")?;
    run.wait("L2", "the first turn's end", ended_turn_of(&first))
        .await?;
    let mut deaf_only = None;
    if apart {
        let other = taken(run.prompt("c-2", "hello").await?, "chat 2's first prompt")?;
        run.wait("L2", "chat 2's first turn's end", ended_turn_of(&other))
            .await?;
        deaf_only = run.subject.tag_of(&ChatId::from("c-1"));
        if deaf_only.is_none() {
            return Err(broke("L2", "chat 1's harness has no tag"));
        }
    }
    let fake = run.subject.fake().clone();
    let opened = |heard: &[Heard]| {
        (heard.iter())
            .filter(|h| matches!(h, Heard::Opened { .. }))
            .count()
    };
    let opened_before = opened(&fake.heard());
    fake.set_knobs(Knobs {
        deaf: true,
        deaf_only,
        ..Knobs::default()
    });
    let stuck = "#stuck";
    let refused = run
        .prompt("c-1", &longer_than_a_pipe(stuck))
        .await?
        .is_err();
    if apart {
        // Chat 1's harness is still deaf; chat 2's serves on.
        let other = taken(run.prompt("c-2", "again").await?, "chat 2's next prompt")?;
        run.wait("L2", "chat 2's next turn's end", ended_turn_of(&other))
            .await?;
        if opened(&fake.heard()) != opened_before {
            return Err(broke(
                "L2",
                "a harness was started again for a chat whose harness kept reading",
            ));
        }
    }
    // The harness reads again.
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
    if apart {
        // Chat 1's next prompt takes its own session up again, in a new harness.
        let session = (run.check.session_of(&ChatId::from("c-1")).cloned())
            .ok_or_else(|| broke("L2", "chat 1's session was never shown"))?;
        let before = fake.heard().len();
        let again = taken(run.prompt("c-1", "hello").await?, "chat 1's next prompt")?;
        run.wait("L2", "chat 1's next turn's end", ended_turn_of(&again))
            .await?;
        let taken_up = fake.heard()[before..]
            .iter()
            .any(|h| matches!(h, Heard::Opened { session: s, resumed: true, .. } if *s == session));
        if !taken_up {
            return Err(broke(
                "S12",
                "chat 1's session was not taken up again after its harness stopped reading",
            ));
        }
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
        Heard::Prompt {
            session,
            text,
            refused: false,
        } if text.contains(&format!(" {} ", more.0)) => Some(session.clone()),
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
        .any(|h| matches!(h, Heard::Opened { session: s, resumed: true, .. } if s == session));
    if !taken_up {
        return Err(broke("S12", "the session a prompt named was not taken up"));
    }
    Ok(())
}

/// A new session for a chat: settled once, by the new session (S1); the next prompt goes to it
/// (S12), and the session a prompt named before it is not taken up again (S12); and another
/// chat naming the old session is refused (S5).
pub async fn new_session<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let first = taken(run.prompt("c-1", "hello").await?, "the first prompt")?;
    run.wait("L2", "the first turn's end", ended_turn_of(&first))
        .await?;
    let chat = ChatId::from("c-1");
    let old = (run.check.session_of(&chat).cloned())
        .ok_or_else(|| broke("L2", "the chat's session was never shown"))?;
    // A prompt naming the chat's own session, as the core names it (D-065): it may take it up,
    // but that leaves no later prompt free to.
    let named = taken(
        run.prompt_in("c-1", "still here", Some(&old)).await?,
        "a prompt naming the chat's session",
    )?;
    run.wait("L2", "that turn's end", ended_turn_of(&named))
        .await?;
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
        Heard::Prompt {
            session,
            text,
            refused: false,
        } if text.contains(&format!(" {} ", next.0)) => Some(session.clone()),
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

/// The folder the last session `session` opened in, as the harness heard it.
fn opened_in(heard: &[Heard], session: &str) -> Option<String> {
    heard.iter().rev().find_map(|h| match h {
        Heard::Opened {
            session: s, folder, ..
        } if s == session => folder.clone(),
        _ => None,
    })
}

/// The session a prompt went to, by its message id.
fn prompted_in(heard: &[Heard], message: &MessageId) -> Option<String> {
    heard.iter().rev().find_map(|h| match h {
        Heard::Prompt {
            session,
            text,
            refused: false,
        } if text.contains(&format!(" {} ", message.0)) => Some(session.clone()),
        _ => None,
    })
}

/// A chat moves the harness to another folder (D-101). A chat that chose none works in the
/// driver's own folder; a new session in another folder, settled by the new session, opens there,
/// and the chat's next prompt goes to it; a chat with no session yet may move too; another chat
/// stays where it was; and after a restart the moved chat's session is taken up in its folder. A
/// driver may refuse none of it.
pub async fn folder<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let own = run
        .driver
        .folder()
        .ok_or_else(|| broke("D-101", "a coding driver states no folder of its own"))?;
    let own = own.to_string_lossy().into_owned();
    let other = run.subject.other_folder();
    let first = taken(run.prompt("c-1", "hello").await?, "the first prompt")?;
    run.wait("L2", "the first turn's end", ended_turn_of(&first))
        .await?;
    let chat = ChatId::from("c-1");
    let old = (run.check.session_of(&chat).cloned())
        .ok_or_else(|| broke("L2", "the chat's session was never shown"))?;
    let heard = run.subject.fake().heard();
    if opened_in(&heard, &old).as_deref() != Some(own.as_str()) {
        return Err(broke("D-101", "a chat that chose none worked elsewhere"));
    }

    let (request, result) = run.new_session_in("c-1", Some(&old), &other).await?;
    taken(result, "a new session in another folder")?;
    let fresh = run
        .wait("L1", "the new session", |e| {
            matches!(e.event.body(), Body::Session(_))
                && e.event.header().state == State::Running
                && (e.event.header().session.as_ref()).is_some_and(|s| s.as_str() != old)
        })
        .await?;
    let new = fresh.event.header().session.clone().expect("a session");
    run.checkpoint().await?;
    if !run.seen.iter().any(|e| e.settles.contains(&request)) {
        return Err(broke("S1", "the new session was never settled"));
    }
    let next = taken(run.prompt("c-1", "where now").await?, "the next prompt")?;
    run.wait("L2", "the next turn's end", ended_turn_of(&next))
        .await?;
    run.checkpoint().await?;
    let heard = run.subject.fake().heard();
    let other_text = other.to_string_lossy().into_owned();
    if prompted_in(&heard, &next).as_deref() != Some(new.as_str()) {
        return Err(broke(
            "S12",
            "the prompt after the move went to another session",
        ));
    }
    if opened_in(&heard, new.as_str()).as_deref() != Some(other_text.as_str()) {
        return Err(broke(
            "D-101",
            "the new session did not open in the chat's folder",
        ));
    }

    // A chat with no session yet moves, and its first session opens there.
    let (_, result) = run.new_session_in("c-2", None, &other).await?;
    taken(
        result,
        "a new session in another folder, for a chat with none",
    )?;
    let second = taken(run.prompt("c-2", "hello").await?, "chat 2's prompt")?;
    run.wait("L2", "chat 2's turn's end", ended_turn_of(&second))
        .await?;
    // A third chat stays in the driver's own folder.
    let third = taken(run.prompt("c-3", "hello").await?, "chat 3's prompt")?;
    run.wait("L2", "chat 3's turn's end", ended_turn_of(&third))
        .await?;
    run.checkpoint().await?;
    let heard = run.subject.fake().heard();
    for (message, want, what) in [
        (
            &second,
            &other_text,
            "chat 2's session did not open in its folder",
        ),
        (&third, &own, "a chat that did not move worked elsewhere"),
    ] {
        let session = prompted_in(&heard, message);
        let folder = session.and_then(|s| opened_in(&heard, &s));
        if folder.as_ref() != Some(want) {
            return Err(broke("D-101", what));
        }
    }

    // After a restart the moved chat's session is taken up, in its folder.
    run.restart().await?;
    let again = taken(
        run.prompt_in("c-1", "still there", Some(new.as_str()))
            .await?,
        "a prompt naming the moved session",
    )?;
    run.wait("L2", "the resumed turn's end", ended_turn_of(&again))
        .await?;
    run.checkpoint().await?;
    let heard = run.subject.fake().heard();
    if opened_in(&heard, new.as_str()).as_deref() != Some(other_text.as_str()) {
        return Err(broke("D-101", "the moved session was taken up elsewhere"));
    }
    run.end().await
}

/// The agent sends a file with the driver's own tool (D-090). Each request must name the chat whose
/// harness asked (S5), with the relative path made whole from the folder that chat works in
/// (D-101), and what the core says of it must reach that harness, and only it, as the tool's
/// result.
///
/// Two chats open sessions in the driver's folder, chat 2's last: an agent that keeps one tool
/// server for every session in a folder (OpenCode) then reaches the driver through chat 2's, and
/// only the call itself names chat 1's session. Chat 1's file is sent. While it waits for the
/// core's word, chat 1 prompts again: the prompt must return at once, taken or refused, since the
/// core cannot give the word until it does (S11). Then chat 2 moves to another folder, and two
/// files are asked for at once, chat 2's there and chat 1's, each waiting while the other is asked
/// for, and answered the other way round: chat 1's sent, chat 2's refused, and the core's words
/// for the refusal must reach chat 2's new session. A driver that gives its harness no such tool
/// must hand over no file.
pub async fn send_file<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    if !run.subject.sends_files() {
        let asked = taken(run.prompt("c-1", "#file").await?, "the prompt")?;
        run.wait("L2", "the turn's end", ended_turn_of(&asked))
            .await?;
        run.checkpoint().await?;
        if !run.files.is_empty() {
            return Err(broke(
                "D-090",
                "a driver that gives no file tool handed over a file",
            ));
        }
        return run.end().await;
    }
    let own = run
        .driver
        .folder()
        .ok_or_else(|| broke("D-101", "a coding driver states no folder of its own"))?;
    for chat in ["c-1", "c-2"] {
        let first = taken(run.prompt(chat, "hello").await?, "a first prompt")?;
        run.wait("L2", "a first turn's end", ended_turn_of(&first))
            .await?;
    }

    // One file, sent, with a prompt while it waits.
    let asked = taken(run.prompt("c-1", "#file").await?, "chat 1's prompt")?;
    let file = asks("c-1", &own, &asked, &mut run).await?;
    let started = tokio::time::Instant::now();
    let _ = run.prompt("c-1", "hello").await?;
    if started.elapsed() >= HELD {
        return Err(broke(
            "S11",
            format!("a prompt while a file's result was to come was held {HELD:?} or more"),
        ));
    }
    run.file_sent(&file, Ok(())).await?;
    told(&mut run, 1, true).await?;
    run.wait("L2", "the file's turn's end", ended_turn_of(&asked))
        .await?;

    // Chat 2 moves to another folder.
    let chat = ChatId::from("c-2");
    let old = (run.check.session_of(&chat).cloned())
        .ok_or_else(|| broke("L2", "chat 2's session was never shown"))?;
    let other = run.subject.other_folder();
    let (_, result) = run.new_session_in("c-2", Some(&old), &other).await?;
    taken(result, "chat 2's new session")?;
    run.wait("L1", "chat 2's new session", |e| {
        e.chat.as_ref() == Some(&chat)
            && matches!(e.event.body(), Body::Session(_))
            && e.event.header().state == State::Running
            && (e.event.header().session.as_ref()).is_some_and(|s| s.as_str() != old)
    })
    .await?;

    // Two at once, answered the other way round.
    let two = taken(run.prompt("c-2", "#file").await?, "chat 2's prompt")?;
    let second = asks("c-2", &other, &two, &mut run).await?;
    let one = taken(run.prompt("c-1", "#file").await?, "chat 1's second prompt")?;
    let third = asks("c-1", &own, &one, &mut run).await?;
    if second.id == third.id {
        return Err(broke("D-090", "two waiting requests share an id"));
    }
    run.file_sent(&third, Ok(())).await?;
    told(&mut run, 2, true).await?;
    let refused = Refusal {
        reason: Reason::NotAllowed,
        detail: Some("the file is not in a folder files may be sent from.".into()),
    };
    run.file_sent(&second, Err(refused)).await?;
    told(&mut run, 3, false).await?;
    for message in [&two, &one] {
        run.wait("L2", "a file's turn's end", ended_turn_of(message))
            .await?;
    }
    run.checkpoint().await?;
    if run.check.files_unheard() > 0 {
        return Err(broke(
            "S5",
            "a file's result reached another chat's session",
        ));
    }
    let heard = run.subject.fake().heard();
    let session = prompted_in(&heard, &two);
    let said = heard.iter().find_map(|h| match h {
        Heard::FileResult {
            session: s, text, ..
        } if Some(s) == session.as_ref() => Some(text),
        _ => None,
    });
    if !said.is_some_and(|t| t.contains("not in a folder files may be sent from")) {
        return Err(broke(
            "D-090",
            "the core's words for a refusal did not reach the agent",
        ));
    }
    run.end().await
}

/// The file `asked` asks for in `chat`, working in `folder`: it must be handed over for that chat
/// (S5), its relative path made whole from that folder (D-101).
async fn asks<S: Subject>(
    chat: &str,
    folder: &std::path::Path,
    asked: &MessageId,
    run: &mut Run<S>,
) -> Result<SendFile, Violation> {
    let file = run.next_file("L2", "the file the agent asked for").await?;
    run.check.file_asked(&file, &ChatId::from(chat));
    run.ok()?;
    let want = folder.join(file_for(&format!("{} {} #file", mark(chat), asked.0)));
    if file.path != want {
        return Err(broke(
            "D-101",
            "a relative path was not made whole from the chat's folder",
        ));
    }
    Ok(file)
}

/// Waits until the harness has heard `count` results of its tool to send a file, the last of them
/// `sent` or not. The driver hands a result on as it returns: the harness hears it without the
/// driver being polled again (D-090).
async fn told<S: Subject>(run: &mut Run<S>, count: usize, sent: bool) -> Result<(), Violation> {
    let results = |heard: &[Heard]| {
        (heard.iter())
            .filter(|h| matches!(h, Heard::FileResult { .. }))
            .count()
    };
    let fake = run.subject.fake().clone();
    if !fake.wait_heard(run.patience, |h| results(h) >= count).await {
        let how = if sent { "sent" } else { "refused" };
        return Err(broke(
            "D-090",
            format!("the harness never heard that a file was {how}"),
        ));
    }
    Ok(())
}

/// How long a prompt may take while a file's result is to come: far less than any driver's
/// patience with its harness.
const HELD: Duration = Duration::from_secs(1);

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

/// The prompt the crash sweep runs: a turn that asks for approval, which is given.
const SWEPT: &str = "#ask";

/// Takes what comes until the driver ends or has nothing more, answering every approval.
async fn answering<S: Subject>(run: &mut Run<S>) -> Result<(), Violation> {
    loop {
        match tokio::time::timeout(Duration::from_millis(300), run.next()).await {
            Ok(Some(Output::Event(e))) if waiting(&e) => {
                let _ = run.answer(&e, true).await?;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return Ok(()),
        }
    }
}

/// How many frames the driver writes to the harness from its start to the end of the swept
/// turn, when nothing fails: the steps at which the sweep can make the harness die.
pub async fn reachable_steps<S: Subject>(subject: S) -> Result<usize, Violation> {
    let mut run = Run::start(subject).await;
    let start = run.subject.fake().frames();
    let m = taken(run.prompt("c-1", SWEPT).await?, "the swept prompt")?;
    loop {
        let e = run
            .until("L2", "the swept turn's end", |e| {
                waiting(e) || (turn_ended(e) && e.prompts.contains(&m))
            })
            .await?;
        if !waiting(&e) {
            break;
        }
        taken(run.answer(&e, true).await?.1, "the answer")?;
    }
    run.end().await?;
    Ok(run.subject.fake().frames() - start)
}

/// The harness dies on reading its `step`-th frame after the driver started, in a turn that asks
/// for approval: it must die, and then every request is settled once, every item closes (S1,
/// S2), and nothing panics.
pub async fn dies_at_step<S: Subject>(subject: S, step: usize) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let fake = run.subject.fake().clone();
    fake.set_knobs(Knobs {
        die_after: Some(fake.frames() + step),
        ..Knobs::default()
    });
    run.patience = Duration::from_secs(3);
    if run.prompt("c-1", SWEPT).await?.is_ok() {
        answering(&mut run).await?;
    }
    run.drain(Duration::from_millis(150)).await;
    // A step the driver never reaches checks nothing.
    if !fake.wait_heard(run.patience, |_| fake.deaths() > 0).await {
        return Err(broke(
            "L2",
            format!("the harness never died at step {step}, so nothing was checked"),
        ));
    }
    run.checkpoint().await?;
    run.check.closed_after_stop();
    run.end().await
}

/// The crash sweep: the harness dies at each step the swept turn reaches, one run each.
pub async fn dies_at_every_step<S: Subject>(subject: impl Fn() -> S) -> Result<(), Violation> {
    let steps = reachable_steps(subject()).await?;
    if steps == 0 {
        return Err(broke("L2", "the driver wrote nothing to the harness"));
    }
    for step in 1..=steps {
        dies_at_step(subject(), step)
            .await
            .map_err(|v| broke(v.property, format!("step {step} of {steps}: {}", v.detail)))?;
    }
    Ok(())
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

/// Replays a recording that allowed its first approval, refusing it instead, to show the replay
/// checks what the driver answers. Gives back what is wrong, if the test shows nothing: the prompt
/// must be taken and the approval asked, as recorded; the answer must be taken, reach the replay
/// and be turned away there as not the recorded one; and nothing more of the recording may play,
/// so the turn never ends. `progress` is the replay's own count.
pub async fn replay_refused<S: Subject>(
    subject: S,
    words: &str,
    progress: &Progress,
) -> Result<(), String> {
    let mut run = Run::start(subject).await;
    run.patience = Duration::from_secs(2);
    let found = async {
        let m = taken(run.prompt("c-1", words).await?, "the prompt")?;
        let asked = run
            .until("L2", "the recorded approval", |e| {
                waiting(e) || (turn_ended(e) && e.prompts.contains(&m))
            })
            .await?;
        Ok::<_, Violation>((m, asked))
    }
    .await;
    let (m, asked) = found.map_err(|v| format!("before the approval: {v}"))?;
    if !waiting(&asked) {
        return Err("the recorded turn ended before its approval".into());
    }
    let (left, refused) = (progress.left(), progress.refused());
    let (_, result) = (run.answer(&asked, false).await).map_err(|v| format!("the answer: {v}"))?;
    result.map_err(|r| format!("the answer was refused: {r:?}"))?;
    if progress.refused() <= refused {
        return Err("the replay never turned the other answer away".into());
    }
    let ended = run
        .until("L2", "the recorded turn's end", ended_turn_of(&m))
        .await;
    match ended {
        Ok(_) => return Err("the turn ended after the other answer".into()),
        Err(v) if v.property == "L2" && v.detail.ends_with("never came") => {}
        Err(v) => return Err(format!("after the other answer: {v}")),
    }
    if progress.left() != left {
        let played = left.saturating_sub(progress.left());
        return Err(format!(
            "{played} recorded messages played after the other answer"
        ));
    }
    Ok(())
}
