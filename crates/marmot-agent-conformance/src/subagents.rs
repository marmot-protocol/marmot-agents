//! Scenarios for a driver whose harness starts subagents (D-117). A subagent's activity is shown
//! flat in the chat of the turn that started it: what started it is a `tool` item of kind
//! `subagent`, its work a `task` item of kind `subagent` whose subject is that tool item, and its
//! words a message whose `parent` is that tool item. Stop stops the agent and every subagent it
//! started, and is settled once, when the last of them has ended (D-028, D-029). Every subagent
//! item closes when the harness stops (D-070).
//!
//! Only a driver whose fake starts subagents runs these: Codex's, today. The fake reads the words
//! of [`Spawn`](crate::fake) in its prompt: `#spawn`, `#spawn1` (the older runtime, whose subagent
//! speaks first), `#nest` and `#outlive`, and the prompt's other words say what the subagent does.

use crate::check::{Violation, broke};
use crate::fake::Heard;
use crate::run::{Run, Subject, turn_ended, waiting};
use marmot_agent_driver::{Emitted, MessageId, RequestId};
use marmot_agent_events::body::ToolKind;
use marmot_agent_events::{Body, State};
use std::collections::HashMap;
use std::time::Duration;

const FLAT: &str = "D-117";

fn taken<T>(result: Result<T, marmot_agent_driver::Refusal>, what: &str) -> Result<T, Violation> {
    result.map_err(|r| broke("L3", format!("{what} was refused: {r:?}")))
}

fn in_chat<'a>(chat: &'a str, f: impl Fn(&Emitted) -> bool + 'a) -> impl Fn(&Emitted) -> bool + 'a {
    move |e| e.chat.as_ref().is_some_and(|c| c.0 == chat) && f(e)
}

fn ended_turn_of(message: &MessageId) -> impl Fn(&Emitted) -> bool + '_ {
    move |e| turn_ended(e) && e.prompts.contains(message)
}

/// A subagent's task item, in a state that has ended or not.
fn task(open: bool) -> impl Fn(&Emitted) -> bool {
    move |e| {
        let running = matches!(e.event.header().state, State::Running | State::Waiting);
        matches!(e.event.body(), Body::Task(t) if t.kind == "subagent") && running == open
    }
}

/// A command, still running.
fn command_running(e: &Emitted) -> bool {
    matches!(e.event.body(), Body::Tool(t) if t.kind == ToolKind::Command)
        && e.event.header().state == State::Running
}

/// The last version of each item of `chat`, by its id, in the order each first came.
fn items<'a>(seen: &'a [Emitted], chat: &str) -> Vec<&'a Emitted> {
    let mut at: HashMap<&str, usize> = HashMap::new();
    let mut last: Vec<&Emitted> = Vec::new();
    for e in seen
        .iter()
        .filter(|e| e.chat.as_ref().is_some_and(|c| c.0 == chat))
    {
        let id = e.event.header().item.as_str();
        match at.get(id) {
            Some(&n) => last[n] = e,
            None => {
                at.insert(id, last.len());
                last.push(e);
            }
        }
    }
    last
}

/// What a chat shows of its subagents, from the last version of each item: the tool items that
/// started them or gave them work, each subagent's tasks, by the tool item they name, and each
/// message that names a parent, by that parent.
struct Shown {
    tools: Vec<String>,
    tasks: Vec<(Option<String>, State)>,
    words: Vec<(String, String)>,
}

fn shown(seen: &[Emitted], chat: &str) -> Shown {
    let mut shown = Shown {
        tools: Vec::new(),
        tasks: Vec::new(),
        words: Vec::new(),
    };
    for e in items(seen, chat) {
        let header = e.event.header();
        match e.event.body() {
            Body::Tool(t) if t.kind == ToolKind::Subagent => {
                shown.tools.push(header.item.as_str().to_string());
            }
            Body::Task(t) if t.kind == "subagent" => {
                let subject = t.subject.as_ref().map(|s| s.as_str().to_string());
                shown.tasks.push((subject, header.state));
            }
            Body::Message(m) => {
                if let Some(parent) = &m.parent {
                    let parent = parent.as_str().to_string();
                    shown.words.push((parent, header.text.clone()));
                }
            }
            _ => {}
        }
    }
    shown
}

/// Every event of `chat` is in the chat's one session: a subagent's own thread is never shown as
/// a session of the chat.
fn one_session(seen: &[Emitted], chat: &str) -> Result<(), Violation> {
    let sessions: std::collections::HashSet<String> = (seen.iter())
        .filter(|e| e.chat.as_ref().is_some_and(|c| c.0 == chat))
        .filter_map(|e| e.event.header().session.as_ref())
        .map(|s| s.as_str().to_string())
        .collect();
    if sessions.len() > 1 {
        let detail = format!("chat {chat} shows {} sessions", sessions.len());
        return Err(broke(FLAT, detail));
    }
    Ok(())
}

fn flat(run: &Run<impl Subject>, chat: &str, count: usize, state: State) -> Result<(), Violation> {
    shown_flat(&run.seen, chat, count, state)
}

/// What `seen` shows in `chat`: `count` subagents, each started by a `subagent` tool item, each
/// task naming one, each having spoken with its tool item as parent, and every task ended as
/// `state`, all in the chat's one session.
pub fn shown_flat(
    seen: &[Emitted],
    chat: &str,
    count: usize,
    state: State,
) -> Result<(), Violation> {
    subagents_shown(seen, chat, count, state)?;
    one_session(seen, chat)
}

fn subagents_shown(
    seen: &[Emitted],
    chat: &str,
    count: usize,
    state: State,
) -> Result<(), Violation> {
    let s = shown(seen, chat);
    let fail = |detail: String| Err(broke(FLAT, detail));
    if s.tasks.len() != count {
        return fail(format!(
            "{} subagent tasks shown, not {count}",
            s.tasks.len()
        ));
    }
    for (subject, ended) in &s.tasks {
        let Some(subject) = subject else {
            return fail("a subagent's task names no item that started it".into());
        };
        if !s.tools.contains(subject) {
            return fail(format!(
                "a task names {subject}, which is no subagent tool item"
            ));
        }
        if *ended != state {
            return fail(format!("a subagent's task ended {ended:?}, not {state:?}"));
        }
        if state == State::Done && !s.words.iter().any(|(parent, _)| parent == subject) {
            return fail(format!(
                "the subagent started by {subject} said nothing in the chat"
            ));
        }
    }
    Ok(())
}

/// A subagent's work reaches the chat of the turn that started it, and only that chat, while
/// another chat is served.
pub async fn subagent_in_its_parents_chat<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let one = taken(run.prompt("c-1", "#spawn").await?, "chat 1's prompt")?;
    let two = taken(run.prompt("c-2", "hello").await?, "chat 2's prompt")?;
    run.wait("L2", "chat 1's turn's end", ended_turn_of(&one))
        .await?;
    run.wait("L2", "chat 2's turn's end", ended_turn_of(&two))
        .await?;
    run.checkpoint().await?;
    flat(&run, "c-1", 1, State::Done)?;
    if !shown(&run.seen, "c-2").tasks.is_empty() {
        return Err(broke("S5", "a subagent of chat 1 shown in chat 2"));
    }
    run.end().await
}

/// A subagent whose turn starts before the item that names it is complete (Codex's older
/// runtime): what it says first is held, not lost, and shown once the item names it.
pub async fn subagent_speaks_first<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let m = taken(run.prompt("c-1", "#spawn1").await?, "the prompt")?;
    run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    run.checkpoint().await?;
    flat(&run, "c-1", 1, State::Done)?;
    run.end().await
}

/// A subagent that starts one of its own: both are shown in the chat, the inner one named by the
/// item of the outer one that started it.
pub async fn nested_subagents<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let m = taken(run.prompt("c-1", "#nest").await?, "the prompt")?;
    run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    run.checkpoint().await?;
    flat(&run, "c-1", 2, State::Done)?;
    run.end().await
}

/// A subagent asks for approval: it waits in the chat, is answered there, and the answer reaches
/// the subagent.
pub async fn subagent_asks_for_approval<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let m = taken(run.prompt("c-1", "#spawn #ask").await?, "the prompt")?;
    let asked = run
        .wait("L2", "the subagent's approval", in_chat("c-1", waiting))
        .await?;
    taken(run.answer(&asked, true).await?.1, "the answer")?;
    run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    run.checkpoint().await?;
    flat(&run, "c-1", 1, State::Done)?;
    run.end().await
}

/// What Stop must do for a tree of `count` subagents started by `words`: the interrupt names the
/// agent's turn, which `turn_first` says ends before it is asked to stop or not, and it stops
/// every subagent, settled once, by the last to end, after which nothing is left working.
async fn stop<S: Subject>(
    run: &mut Run<S>,
    words: &str,
    count: usize,
    turn_first: bool,
) -> Result<(), Violation> {
    let m = taken(run.prompt("c-1", words).await?, "the prompt")?;
    let working = run
        .wait(
            "L2",
            "the subagents' command",
            in_chat("c-1", command_running),
        )
        .await?;
    let turn = working.event.header().turn.clone();
    let turn = turn.ok_or_else(|| broke(FLAT, "a subagent's command names no turn"))?;
    if turn_first {
        run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    }
    let before = interrupts_heard(&run.subject.fake().heard());
    let (request, result) = run.interrupt("c-1", &turn).await?;
    taken(result, "the interrupt")?;
    let settled = run
        .wait("L2", "the stop's settling", |e| {
            e.settles.contains(&request)
        })
        .await?;
    run.drain(Duration::from_millis(100)).await;
    run.checkpoint().await?;
    let heard = interrupts_heard(&run.subject.fake().heard()) - before;
    let wanted = count + usize::from(!turn_first);
    if heard != wanted {
        return Err(broke(
            FLAT,
            format!("{heard} turns were interrupted, not {wanted}"),
        ));
    }
    settled_by_the_last(&run.seen, &request, &settled)?;
    flat(run, "c-1", count, State::Cancelled)?;
    Ok(())
}

fn interrupts_heard(heard: &[Heard]) -> usize {
    (heard.iter())
        .filter(|h| matches!(h, Heard::Control { what, .. } if what == "interrupt"))
        .count()
}

/// The stop is settled by the end of the turn or subagent that ended last: nothing ends after it.
fn settled_by_the_last(
    seen: &[Emitted],
    request: &RequestId,
    settled: &Emitted,
) -> Result<(), Violation> {
    let at = (seen.iter())
        .position(|e| e.settles.contains(request))
        .expect("settled");
    let ends = |e: &Emitted| {
        matches!(e.event.body(), Body::Turn(_) | Body::Task(_))
            && !matches!(e.event.header().state, State::Running | State::Waiting)
    };
    if !ends(settled) {
        return Err(broke(
            FLAT,
            "a stop settled by something that is not an end",
        ));
    }
    if seen[at + 1..].iter().any(ends) {
        return Err(broke(FLAT, "a stop settled before the last subagent ended"));
    }
    Ok(())
}

/// Stop while the agent waits for its subagent: the agent's turn and the subagent both stop,
/// though the harness stops only the turn it is told to.
pub async fn stop_stops_every_subagent<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    stop(&mut run, "#spawn #sleep", 1, false).await?;
    run.end().await
}

/// Stop after the agent's turn has ended, while a subagent it started works on: the interrupt
/// names that turn, and stops the subagent.
pub async fn stop_after_the_turn_has_ended<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    stop(&mut run, "#outlive #sleep", 1, true).await?;
    run.end().await
}

/// Stop with a subagent that has started one of its own: all three stop.
pub async fn stop_stops_nested_subagents<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    stop(&mut run, "#nest #sleep", 2, false).await?;
    run.end().await
}

/// The harness dies while a subagent works: every item of the chat closes, the subagent's
/// included (S2).
pub async fn dies_with_subagents_running<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    taken(run.prompt("c-1", "#nest #sleep").await?, "the prompt")?;
    run.wait(
        "L2",
        "the subagent's command",
        in_chat("c-1", command_running),
    )
    .await?;
    run.kill("c-1");
    let fake = run.subject.fake().clone();
    if !fake.wait_heard(run.patience, |_| fake.deaths() > 0).await {
        return Err(broke("L2", "the harness did not die"));
    }
    run.drain(Duration::from_millis(200)).await;
    run.checkpoint().await?;
    run.check.closed_after_stop();
    run.ok()?;
    flat(&run, "c-1", 2, State::Cancelled)?;
    run.end().await
}

/// A new session while a subagent of the old one works, after the agent's turn has ended: the old
/// session has ended, its subagent is stopped, since the harness does not stop it with the
/// session, and every item of its subagents closes (D-070, D-117).
pub async fn new_session_with_subagents_running<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    let m = taken(run.prompt("c-1", "#outlive #sleep").await?, "the prompt")?;
    run.wait(
        "L2",
        "the subagent's command",
        in_chat("c-1", command_running),
    )
    .await?;
    run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    let session = (run.seen.iter())
        .find_map(|e| e.event.header().session.clone())
        .ok_or_else(|| broke(FLAT, "no session shown"))?;
    let before = interrupts_heard(&run.subject.fake().heard());
    let (request, result) = run.new_session("c-1", Some(session.as_str())).await?;
    taken(result, "the new session")?;
    run.wait("L2", "the new session", |e| e.settles.contains(&request))
        .await?;
    run.drain(Duration::from_millis(100)).await;
    run.checkpoint().await?;
    let heard = interrupts_heard(&run.subject.fake().heard()) - before;
    if heard != 1 {
        return Err(broke(
            FLAT,
            format!("{heard} subagents were stopped with their session, not 1"),
        ));
    }
    let open: Vec<String> = (items(&run.seen, "c-1").into_iter())
        .filter(|e| !matches!(e.event.body(), Body::Session(_)))
        .filter(|e| matches!(e.event.header().state, State::Running | State::Waiting))
        .map(|e| e.event.header().item.as_str().to_string())
        .collect();
    if !open.is_empty() {
        let detail = format!("left open when the session ended: {}", open.join(", "));
        return Err(broke("D-070", detail));
    }
    // The new session is the chat's second: only the subagent is checked.
    subagents_shown(&run.seen, "c-1", 1, State::Cancelled)?;
    run.end().await
}

/// A restart while subagents work: the new driver takes the chat's session up for its next
/// prompt, and shows nothing more of the old subagents (the core closes what the old driver left
/// open, D-070).
pub async fn restart_with_subagents_running<S: Subject>(subject: S) -> Result<(), Violation> {
    let mut run = Run::start(subject).await;
    taken(run.prompt("c-1", "#spawn #sleep").await?, "the prompt")?;
    run.wait(
        "L2",
        "the subagent's command",
        in_chat("c-1", command_running),
    )
    .await?;
    let session = (run.seen.iter())
        .find_map(|e| e.event.header().session.clone())
        .ok_or_else(|| broke(FLAT, "no session shown"))?;
    run.restart().await?;
    let m = run
        .prompt_in("c-1", "hello", Some(session.as_str()))
        .await?;
    let m = taken(m, "the prompt after the restart")?;
    run.wait("L2", "the turn's end", ended_turn_of(&m)).await?;
    run.checkpoint().await?;
    if run.seen.iter().any(task(true)) || run.seen.iter().any(task(false)) {
        return Err(broke(FLAT, "an old subagent shown after the restart"));
    }
    run.end().await
}
