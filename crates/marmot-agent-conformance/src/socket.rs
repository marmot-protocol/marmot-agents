//! The scenarios for a connector on the socket (`docs/design/socket-protocol.md`): one whose harness
//! runs inside it, and outlives its connection to the core. Hermes and OpenClaw connect again when
//! the connection drops, and their gateway goes on; Claude Code's connector ends, and the next is
//! started. What the next connection is handed must be only what is new (D-058, D-070, D-129,
//! D-139). A built-in driver has no such connection, and runs none of these.
//!
//! [`Subject::start`] stands for the connection: a restart drops the one there is, and takes the
//! next. [`Fake::drop_connection`] drops it where a scenario says, and
//! [`Knobs::withhold_responses`] keeps the connector's responses from the core until then, so a
//! request is in flight when it drops.

use crate::check::{Violation, broke, mark};
use crate::fake::{Fake, Heard, Knobs, Unanswered};
use crate::run::{Run, Subject, waiting};
use crate::scenarios::{ended_turn_of, of_chat, taken};
use std::future::Future;
use std::time::Duration;

/// The connection drops while an approval waits, and the connector connects again, as when the
/// core restarts. Nothing from the first connection is settled on the next (S1) or shown again
/// there (S13), an answer to the approval the first one asked is refused (S11), and a chat's next
/// prompt is answered.
pub async fn reconnect<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let first = taken(run.prompt("c-1", "hello").await?, "chat 1's prompt")?;
    run.wait("L2", "chat 1's turn's end", ended_turn_of(&first))
        .await?;
    taken(run.prompt("c-2", "#ask").await?, "chat 2's prompt")?;
    let asked = run
        .wait("L2", "chat 2's approval", of_chat("c-2", waiting))
        .await?;
    run.restart().await?;
    let (_, late) = run.answer(&asked, true).await?;
    if late.is_ok() {
        return Err(broke(
            "S11",
            "an answer to an approval the connection before asked was taken",
        ));
    }
    let next = taken(
        run.prompt("c-1", "and again").await?,
        "chat 1's next prompt",
    )?;
    run.wait("L2", "that turn's end", ended_turn_of(&next))
        .await?;
    run.end().await
}

/// Whether the harness heard a prompt of `chat` with `words`.
fn prompt_heard(heard: &[Heard], chat: &str, words: &str) -> bool {
    heard.iter().any(|h| {
        matches!(h, Heard::Prompt { text, .. } if text.starts_with(&mark(chat)) && text.ends_with(words))
    })
}

/// Runs `request` with the connector's responses held back on the socket, and drops the connection
/// once the harness has had the request (`delivered`) and the connector has responded: a disconnect
/// after delivery and before the response reaches the core. Gives what `request` gave and the
/// request as the drop found it, which must be the one request in flight, its response held back,
/// and still waited on when the connection began to drop: otherwise the drop did not happen where
/// it was meant to, and nothing was checked. A request given up before then timed out, which the
/// core settles as it settles a drop (D-147), so its end shows nothing of the drop.
async fn dropped_during<T>(
    fake: &Fake,
    patience: Duration,
    what: &str,
    delivered: impl Fn(&[Heard]) -> bool,
    request: impl Future<Output = Result<T, Violation>>,
) -> Result<(T, Unanswered), Violation> {
    let (withheld, drops) = (fake.withheld(), fake.dropped().len());
    fake.set_knobs(Knobs {
        withhold_responses: true,
        ..Knobs::default()
    });
    let dropping = async {
        let heard = fake.wait_heard(patience, &delivered).await;
        let deadline = tokio::time::Instant::now() + patience;
        while fake.withheld() == withheld && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        fake.drop_connection();
        // What carries the connection keeps what was in flight as it drops it.
        while fake.dropped().len() == drops && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        heard
    };
    let request = async {
        let out = request.await;
        (out, fake.dropped().len() > drops)
    };
    let ((out, after_the_drop), heard) = tokio::join!(request, dropping);
    fake.set_knobs(Knobs::default());
    let out = out?;
    if !heard {
        return Err(broke(
            "L3",
            format!(
                "the harness never had the {what} the connection dropped after, so nothing was checked"
            ),
        ));
    }
    if !after_the_drop {
        return Err(broke(
            "L3",
            format!(
                "the {what} ended before the connection dropped, given up as a timeout, \
                 so nothing was checked"
            ),
        ));
    }
    let dropped = fake.dropped();
    match dropped.get(drops).map(|d| d.unanswered.as_slice()) {
        Some([one]) if one.method == what && one.withheld => Ok((out, one.clone())),
        found => Err(broke(
            "L3",
            format!(
                "the connection was to drop with the {what} alone in flight, its response held back, \
                 and it dropped with {found:?}, so nothing was checked"
            ),
        )),
    }
}

/// A request the connection dropped before its response was taken exactly when an event the core
/// read before the drop showed it was (D-147, D-129).
fn taken_as_shown(what: &str, taken: bool, dropped: &Unanswered) -> Result<(), Violation> {
    match (taken, dropped.shown) {
        (true, false) => Err(broke(
            "S1",
            format!(
                "the {what} the connection dropped before its response was taken, though no event showed it"
            ),
        )),
        (false, true) => Err(broke(
            "S1",
            format!(
                "the {what} the connection dropped before its response was refused, though an event showed it taken"
            ),
        )),
        _ => Ok(()),
    }
}

/// The connection drops once the harness has a prompt and the connector has responded, and before
/// the response reaches the core, which then counts the prompt as perhaps not delivered, unless a
/// turn of the chat already named it, and never sends it again (D-129, D-147). Then the same for an
/// answer. The connector connects again: the harness had each once (S4, S11), nothing the first
/// connection was handed is settled on the next (S1), and the chat's next prompt is answered.
pub async fn dropped_after_delivery<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let fake = run.subject.fake().clone();
    let patience = run.patience;
    let (prompted, dropped) = dropped_during(
        &fake,
        patience,
        "prompt",
        |h| prompt_heard(h, "c-1", "dropped"),
        run.prompt("c-1", "dropped"),
    )
    .await?;
    taken_as_shown("prompt", prompted.is_ok(), &dropped)?;
    run.restart().await?;
    let next = taken(run.prompt("c-1", "and again").await?, "the next prompt")?;
    run.wait("L2", "the next turn's end", ended_turn_of(&next))
        .await?;

    // An answer, the same way.
    taken(run.prompt("c-2", "#ask").await?, "chat 2's prompt")?;
    let asked = run
        .wait("L2", "chat 2's approval", of_chat("c-2", waiting))
        .await?;
    let answers = |h: &[Heard]| {
        h.iter()
            .filter(|h| matches!(h, Heard::Answer { .. }))
            .count()
    };
    let before = answers(&fake.heard());
    let ((_, result), dropped) = dropped_during(
        &fake,
        patience,
        "answer",
        |h| answers(h) > before,
        run.answer(&asked, true),
    )
    .await?;
    if result.is_err() {
        // Refused as failed, as the connection went, it reached the harness all the same: the
        // check is told to expect it, as for one taken.
        let chat = asked.chat.clone().expect("an item names its chat");
        run.check.expect_answer(&chat, Some(true), None);
    }
    taken_as_shown("answer", result.is_ok(), &dropped)?;
    run.restart().await?;
    let last = taken(run.prompt("c-1", "once more").await?, "the last prompt")?;
    run.wait("L2", "the last turn's end", ended_turn_of(&last))
        .await?;
    run.end().await?;
    if answers(&fake.heard()) != before + 1 {
        return Err(broke("S11", "an answer reached the harness more than once"));
    }
    Ok(())
}
