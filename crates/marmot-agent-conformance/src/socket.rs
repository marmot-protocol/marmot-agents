//! The scenarios for a connector on the socket (`docs/design/socket-protocol.md`): one whose harness
//! runs inside it, and outlives its connection to the core. Hermes and OpenClaw connect again when
//! the connection drops, and their gateway goes on; Claude Code's connector ends, and the next is
//! started. What the next connection is handed must be only what is new (D-058, D-070, D-129,
//! D-139). A built-in driver has no such connection, and runs none of these.
//!
//! [`Subject::start`] stands for the connection: a restart drops the one there is, and takes the
//! next. [`Knobs::drop_after`] drops it after the harness has a request and before the connector
//! answers it.

use crate::check::{Violation, broke, mark};
use crate::fake::{Heard, Knobs};
use crate::run::{Run, Subject, waiting};
use crate::scenarios::{ended_turn_of, of_chat, taken};

/// The one list of the socket scenarios, as [`with_scenarios`](crate::with_scenarios) is of the
/// others: it calls the macro it is given with what it was given, then `;` and every name.
#[macro_export]
macro_rules! with_socket_scenarios {
    ($then:ident ! ($($given:tt)*)) => {
        $then!($($given)*; reconnect, dropped_after_delivery);
    };
}

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

/// The connection drops once the harness has a prompt, and before the connector answers the core,
/// which then counts the prompt as perhaps not delivered and never sends it again (D-129). Then the
/// same for an answer. The connector connects again: the harness had each once (S4, S11), nothing
/// the first connection was handed is settled on the next (S1), and the chat's next prompt is
/// answered.
pub async fn dropped_after_delivery<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let fake = run.subject.fake().clone();
    fake.set_knobs(Knobs {
        drop_after: Some(fake.frames() + 1),
        ..Knobs::default()
    });
    let _ = run.prompt("c-1", "dropped").await?;
    let delivered = fake
        .wait_heard(run.patience, |h| prompt_heard(h, "c-1", "dropped"))
        .await;
    fake.set_knobs(Knobs::default());
    if !delivered {
        return Err(broke(
            "L3",
            "the harness never had the prompt the connection dropped after, so nothing was checked",
        ));
    }
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
    fake.set_knobs(Knobs {
        drop_after: Some(fake.frames() + 1),
        ..Knobs::default()
    });
    let (_, result) = run.answer(&asked, true).await?;
    if result.is_err() {
        // Refused as failed, as the connection went, it reached the harness all the same: the
        // check is told to expect it, as for one taken.
        let chat = asked.chat.clone().expect("an item names its chat");
        run.check.expect_answer(&chat, Some(true), None);
    }
    let delivered = fake.wait_heard(run.patience, |h| answers(h) > before).await;
    fake.set_knobs(Knobs::default());
    if !delivered {
        return Err(broke(
            "L3",
            "the harness never had the answer the connection dropped after, so nothing was checked",
        ));
    }
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
