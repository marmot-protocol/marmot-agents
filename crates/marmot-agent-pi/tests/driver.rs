//! The transport, against a stand-in for Pi (`fake_pi.py`, which speaks the records the
//! recordings show): which chat each event belongs to, the prompts a turn responds to, the
//! request each answer and control settles, and what happens when Pi stops or a session is not
//! the chat's (D-074). Each control is settled exactly once (D-028): by its effect, by an error
//! when Pi refuses it or an extension cancels it, and by an error when Pi stops before acting.

use marmot_agent_driver::{
    Action, Answer, Chat, ChatId, Control, Driver, Emitted, FilesIn, Given, MidTurn, Output,
    Person, Prompt, Reason, Refusal, Reply, SendFile, Sessions,
};
use marmot_agent_events::body::{Body, StartedBy};
use marmot_agent_events::{SessionId, State, TurnId};
use marmot_agent_pi::{Pi, Setup};
use std::path::Path;
use std::time::Duration;

async fn start() -> (Pi, tempfile::TempDir) {
    let work = tempfile::tempdir().unwrap();
    (start_in(work.path(), &[]).await, work)
}

/// A driver whose fakes run in `work`, with `extra` arguments.
async fn start_in(work: &Path, extra: &[&str]) -> Pi {
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_pi.py");
    let mut args = vec![fake.display().to_string()];
    args.extend(extra.iter().map(|a| a.to_string()));
    let setup = Setup {
        program: "python3".into(),
        args,
        cwd: work.to_path_buf(),
    };
    Pi::start(setup).await.expect("the fake starts")
}

/// Whether the last fake started in `work` has gone, within five seconds.
async fn gone(work: &Path) -> bool {
    let pid = std::fs::read_to_string(work.join(".fake-pid")).unwrap();
    let proc = Path::new("/proc").join(pid.trim());
    for _ in 0..50 {
        if !proc.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn person() -> Person {
    Person {
        id: "u-1".into(),
        name: "Alice".into(),
    }
}

fn prompt(chat: &str, message: &str, text: &str) -> Prompt {
    Prompt {
        message: message.into(),
        text: text.into(),
        chat: Chat {
            id: chat.into(),
            group: false,
            name: None,
            folder: None,
        },
        sender: person(),
        reply_to: None,
        attachments: Vec::new(),
        session: None,
    }
}

/// The next event, within ten seconds.
async fn next(pi: &mut Pi) -> Emitted {
    tokio::time::timeout(Duration::from_secs(10), pi.next_event())
        .await
        .expect("an event in time")
        .and_then(Output::into_event)
        .expect("the driver goes on")
}

/// Events until one passes `found`, which is returned.
async fn until(pi: &mut Pi, found: impl Fn(&Emitted) -> bool) -> Emitted {
    loop {
        let event = next(pi).await;
        if found(&event) {
            return event;
        }
    }
}

fn is_turn(e: &Emitted, state: State) -> bool {
    matches!(e.event.body(), Body::Turn(_)) && e.event.header().state == state
}

fn session_of(e: &Emitted) -> SessionId {
    e.event.header().session.clone().expect("a session")
}

fn set(request: &str, session: &SessionId, setting: &str, value: &str) -> Control {
    Control {
        request: request.into(),
        by: person(),
        action: Action::Set {
            session: session.clone(),
            setting: setting.into(),
            value: value.into(),
        },
    }
}

fn is_error(e: &Emitted) -> bool {
    matches!(e.event.body(), Body::Error(_))
}

#[tokio::test]
async fn a_prompt_opens_a_session_and_a_turn_that_names_it() {
    let (mut pi, _work) = start().await;
    assert_eq!(pi.harness().version.as_deref(), Some("0.0.0-fake"));
    let caps = pi.capabilities();
    assert!(caps.interrupt && caps.resume && !caps.approvals);
    assert_eq!(caps.mid_turn, MidTurn::Fold);
    assert_eq!(caps.sessions, Sessions::Core);
    assert_eq!(caps.files_in, FilesIn::None);

    pi.prompt(prompt("c-1", "m-1", "hello")).await.unwrap();
    let session = next(&mut pi).await;
    assert!(matches!(session.event.body(), Body::Session(_)));
    assert_eq!(session.chat, Some(ChatId::from("c-1")));
    let started = until(&mut pi, |e| is_turn(e, State::Running)).await;
    assert_eq!(started.prompts, vec!["m-1".into()]);
    let said = next(&mut pi).await;
    assert_eq!(said.event.header().text, "ok: hello");
    let ended = until(&mut pi, |e| is_turn(e, State::Done)).await;
    assert_eq!(ended.prompts, vec!["m-1".into()]);
    assert_eq!(session_of(&ended), session_of(&session));
}

#[tokio::test]
async fn an_interrupt_is_settled_by_the_end_of_its_turn() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "sleep")).await.unwrap();
    let started = until(&mut pi, |e| is_turn(e, State::Running)).await;
    until(&mut pi, |e| matches!(e.event.body(), Body::Tool(_))).await;
    let turn: TurnId = started.event.header().turn.clone().unwrap();
    let interrupt = Control {
        request: "r-1".into(),
        by: person(),
        action: Action::Interrupt { turn },
    };
    pi.control(interrupt).await.unwrap();
    let tool = until(&mut pi, |e| matches!(e.event.body(), Body::Tool(_))).await;
    assert_eq!(tool.event.header().state, State::Cancelled);
    let ended = until(&mut pi, |e| matches!(e.event.body(), Body::Turn(_))).await;
    assert_eq!(ended.event.header().state, State::Cancelled);
    assert_eq!(ended.settles, vec!["r-1".into()]);
}

#[tokio::test]
async fn a_message_sent_during_a_turn_joins_it() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "ask first")).await.unwrap();
    let question = until(&mut pi, |e| matches!(e.event.body(), Body::Question(_))).await;
    pi.prompt(prompt("c-1", "m-2", "also this")).await.unwrap();
    let answer = Answer {
        request: "r-1".into(),
        item: question.event.header().item.clone(),
        by: person(),
        given: Given::Questions(vec![Reply {
            question: "q0".into(),
            options: vec!["o0".into()],
            text: None,
        }]),
    };
    pi.answer(answer.clone()).await.unwrap();
    let answered = until(&mut pi, |e| matches!(e.event.body(), Body::Question(_))).await;
    assert_eq!(answered.event.header().state, State::Done);
    assert_eq!(answered.settles, vec!["r-1".into()]);
    let said = until(&mut pi, |e| matches!(e.event.body(), Body::Message(_))).await;
    assert_eq!(said.event.header().text, "allowed and also this");
    let ended = until(&mut pi, |e| matches!(e.event.body(), Body::Turn(_))).await;
    assert_eq!(ended.prompts, vec!["m-1".into(), "m-2".into()]);
    // The question has been answered: a second answer is refused.
    let again = pi.answer(answer).await.unwrap_err();
    assert_eq!(again.reason, Reason::AlreadyAnswered);
}

#[tokio::test]
async fn settings_and_a_new_session_are_settled_by_the_session_item() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "hello")).await.unwrap();
    let first = until(&mut pi, |e| is_turn(e, State::Done)).await;
    let old = session_of(&first);

    let set = Control {
        request: "r-1".into(),
        by: person(),
        action: Action::Set {
            session: old.clone(),
            setting: "thinking".into(),
            value: "high".into(),
        },
    };
    pi.control(set).await.unwrap();
    let edit = until(&mut pi, |e| !e.settles.is_empty()).await;
    assert!(matches!(edit.event.body(), Body::Session(_)));
    assert!(edit.event.header().text.contains("Thinking: high"));

    // Pi refuses a model it does not have: the refusal settles the request.
    let refused = Control {
        request: "r-2".into(),
        by: person(),
        action: Action::Set {
            session: old.clone(),
            setting: "model".into(),
            value: "fake/m9".into(),
        },
    };
    pi.control(refused).await.unwrap();
    let error = until(&mut pi, |e| !e.settles.is_empty()).await;
    assert!(matches!(error.event.body(), Body::Error(_)));
    assert_eq!(error.settles, vec!["r-2".into()]);

    let fresh = Control {
        request: "r-3".into(),
        by: person(),
        action: Action::NewSession {
            chat: prompt("c-1", "m-0", "").chat,
            session: Some(old.clone()),
        },
    };
    pi.control(fresh).await.unwrap();
    let ended = next(&mut pi).await;
    assert_eq!(ended.event.header().state, State::Done);
    assert_eq!(session_of(&ended), old);
    let started = next(&mut pi).await;
    assert!(matches!(started.event.body(), Body::Session(_)));
    assert_ne!(session_of(&started), old);
    assert_eq!(started.settles, vec!["r-3".into()]);
}

#[tokio::test]
async fn a_pi_that_stops_closes_its_turn_and_the_next_prompt_resumes_the_session() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "die")).await.unwrap();
    let started = until(&mut pi, |e| is_turn(e, State::Running)).await;
    let tool = until(&mut pi, |e| matches!(e.event.body(), Body::Tool(_))).await;
    assert_eq!(tool.event.header().state, State::Running);
    let closed = until(&mut pi, |e| matches!(e.event.body(), Body::Tool(_))).await;
    assert_eq!(closed.event.header().state, State::Cancelled);
    assert!(closed.event.native().is_none());
    let ended = next(&mut pi).await;
    assert!(is_turn(&ended, State::Cancelled));
    assert_eq!(ended.prompts, vec!["m-1".into()]);

    pi.prompt(prompt("c-1", "m-2", "again")).await.unwrap();
    let resumed = until(&mut pi, |e| is_turn(e, State::Running)).await;
    assert_eq!(session_of(&resumed), session_of(&started));
    assert_eq!(resumed.prompts, vec!["m-2".into()]);
}

#[tokio::test]
async fn a_session_is_its_chats_alone() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "hello")).await.unwrap();
    let done = until(&mut pi, |e| is_turn(e, State::Done)).await;
    let mut other = prompt("c-2", "m-2", "hello");
    other.session = Some(session_of(&done));
    let refused = pi.prompt(other).await.unwrap_err();
    assert_eq!(refused.reason, Reason::Invalid);

    // A slash command an extension handles starts no turn, and the next prompt's turn does not
    // claim it.
    pi.prompt(prompt("c-1", "m-4", "/fake")).await.unwrap();
    pi.prompt(prompt("c-1", "m-5", "after")).await.unwrap();
    let started = until(&mut pi, |e| is_turn(e, State::Running)).await;
    assert_eq!(started.prompts, vec!["m-5".into()]);
}

#[tokio::test]
async fn a_session_that_held_messages_and_is_gone_is_not_made_again() {
    let (mut pi, work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "die")).await.unwrap();
    let ended = until(&mut pi, |e| is_turn(e, State::Cancelled)).await;
    let session = session_of(&ended);
    std::fs::remove_file(work.path().join(".fake-sessions").join(session.as_str())).unwrap();
    let refused = pi.prompt(prompt("c-1", "m-2", "again")).await.unwrap_err();
    assert_eq!(refused.reason, Reason::Failed);
}

#[tokio::test]
async fn a_session_that_never_held_a_message_is_made_again_after_a_restart() {
    let work = tempfile::tempdir().unwrap();
    let mut pi = start_in(work.path(), &[]).await;
    let fresh = Control {
        request: "r-1".into(),
        by: person(),
        action: Action::NewSession {
            chat: prompt("c-1", "m-0", "").chat,
            session: None,
        },
    };
    pi.control(fresh).await.unwrap();
    let started = until(&mut pi, |e| !e.settles.is_empty()).await;
    assert!(matches!(started.event.body(), Body::Session(_)));
    let session = session_of(&started);
    drop(pi); // the core restarts before anyone writes; Pi wrote no file for the session

    let mut pi = start_in(work.path(), &[]).await;
    let mut resume = prompt("c-1", "m-1", "hello");
    resume.session = Some(session.clone());
    pi.prompt(resume).await.unwrap();
    let done = until(&mut pi, |e| is_turn(e, State::Done)).await;
    assert_eq!(session_of(&done), session);
    assert_eq!(done.prompts, vec!["m-1".into()]);
}

#[tokio::test]
async fn a_question_asked_as_pi_starts_fails_the_start_and_stops_pi() {
    let work = tempfile::tempdir().unwrap();
    let mut pi = start_in(work.path(), &["--ask-at-start"]).await;
    let asked = pi.prompt(prompt("c-1", "m-1", "hello"));
    let refused = tokio::time::timeout(Duration::from_secs(5), asked)
        .await
        .expect("refused at once")
        .unwrap_err();
    assert_eq!(refused.reason, Reason::Failed);
    assert!(gone(work.path()).await, "the Pi that asked is stopped");
    // The question went nowhere: nobody could answer it.
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(200), pi.next_event()).await
        && let Some(event) = event.into_event()
    {
        assert!(!matches!(event.event.body(), Body::Question(_)));
    }
}

#[tokio::test]
async fn a_pi_that_comes_up_in_another_session_is_stopped() {
    let work = tempfile::tempdir().unwrap();
    let mut pi = start_in(work.path(), &[]).await;
    let mut resume = prompt("c-1", "m-1", "hello");
    resume.session = Some("other-1".into());
    let refused = pi.prompt(resume).await.unwrap_err();
    assert_eq!(refused.reason, Reason::Failed);
    assert!(
        gone(work.path()).await,
        "the Pi in the wrong session is stopped"
    );
}

#[tokio::test]
async fn a_failed_resume_leaves_the_chats_session_as_it_was() {
    let (mut pi, work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "hello")).await.unwrap();
    let done = until(&mut pi, |e| is_turn(e, State::Done)).await;
    let session = session_of(&done);

    // The Pi started for this comes up in another session: it is stopped before the translator
    // reads its state, so the chat's session item is neither closed nor replaced.
    let mut resume = prompt("c-1", "m-2", "hello");
    resume.session = Some("other-1".into());
    let refused = pi.prompt(resume).await.unwrap_err();
    assert_eq!(refused.reason, Reason::Failed);
    assert!(
        gone(work.path()).await,
        "the Pi in the wrong session is stopped"
    );
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(200), pi.next_event()).await
        && let Some(event) = event.into_event()
    {
        assert!(
            !matches!(event.event.body(), Body::Session(_)),
            "the session item is left alone"
        );
    }

    pi.prompt(prompt("c-1", "m-3", "again")).await.unwrap();
    let resumed = until(&mut pi, |e| is_turn(e, State::Running)).await;
    assert_eq!(session_of(&resumed), session);
}

#[tokio::test]
async fn an_answer_that_cannot_reach_pi_is_refused_and_never_shown_as_given() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "ask hangup")).await.unwrap();
    let question = until(&mut pi, |e| matches!(e.event.body(), Body::Question(_))).await;
    assert_eq!(question.event.header().state, State::Waiting);
    let answer = Answer {
        request: "r-1".into(),
        item: question.event.header().item.clone(),
        by: person(),
        given: Given::Questions(vec![Reply {
            question: "q0".into(),
            options: vec!["o0".into()],
            text: None,
        }]),
    };
    let refused = pi.answer(answer).await.unwrap_err();
    assert_eq!(refused.reason, Reason::Failed);
    // Pi exits: the question closes unanswered, and nothing settles the refused answer.
    let closed = until(&mut pi, |e| matches!(e.event.body(), Body::Question(_))).await;
    assert_eq!(closed.event.header().state, State::Cancelled);
    assert!(closed.settles.is_empty());
    let ended = until(&mut pi, |e| matches!(e.event.body(), Body::Turn(_))).await;
    assert_eq!(ended.event.header().state, State::Cancelled);
    assert!(ended.settles.is_empty());
}

#[tokio::test]
async fn one_compaction_at_a_time_settled_by_its_own_end() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "hello")).await.unwrap();
    let done = until(&mut pi, |e| is_turn(e, State::Done)).await;
    let session = session_of(&done);
    let compact = |request: &str| Control {
        request: request.into(),
        by: person(),
        action: Action::Own {
            session: session.clone(),
            id: "compact".into(),
        },
    };
    pi.control(compact("r-1")).await.unwrap();
    // A second press while the first runs is refused: Pi would abort the first.
    let again = pi.control(compact("r-2")).await.unwrap_err();
    assert_eq!(again.reason, Reason::Busy);
    // A compaction Pi starts of its own accord meanwhile settles nothing.
    pi.prompt(prompt("c-1", "m-2", "auto")).await.unwrap();
    let mut automatic = 0;
    let settled = loop {
        let event = next(&mut pi).await;
        let compaction = event
            .event
            .header()
            .item
            .as_str()
            .starts_with("compaction:");
        if !event.settles.is_empty() {
            break event;
        }
        if compaction && event.event.header().state == State::Done {
            automatic += 1;
        }
    };
    assert_eq!(automatic, 1, "the automatic compaction ended first");
    assert!(matches!(settled.event.body(), Body::Notice(_)));
    assert_eq!(settled.event.header().state, State::Done);
    assert_eq!(settled.settles, vec!["r-1".into()]);
    // Once it has ended, another may be asked for.
    pi.control(compact("r-3")).await.unwrap();
    let settled = until(&mut pi, |e| !e.settles.is_empty()).await;
    assert_eq!(settled.settles, vec!["r-3".into()]);
}

#[tokio::test]
async fn a_prompt_sent_as_a_turn_ends_starts_the_next_turn() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "hello")).await.unwrap();
    until(&mut pi, |e| is_turn(e, State::Running)).await;
    // Pi finishes the turn and says so; the driver has not read that yet when the next prompt
    // goes.
    tokio::time::sleep(Duration::from_millis(500)).await;
    pi.prompt(prompt("c-1", "m-2", "again")).await.unwrap();
    let ended = until(&mut pi, |e| is_turn(e, State::Done)).await;
    assert_eq!(ended.prompts, vec!["m-1".into()]);
    let next = until(&mut pi, |e| is_turn(e, State::Running)).await;
    assert_eq!(next.prompts, vec!["m-2".into()]);
    let Body::Turn(turn) = next.event.body() else {
        unreachable!()
    };
    assert_eq!(turn.started_by, StartedBy::User);
    let ended = until(&mut pi, |e| is_turn(e, State::Done)).await;
    assert_eq!(ended.prompts, vec!["m-2".into()]);
}

#[tokio::test]
async fn a_command_that_asks_first_does_not_hold_up_the_prompt() {
    let (mut pi, _work) = start().await;
    // Pi acknowledges this command only once its question is answered, which the core can send
    // only after the prompt has returned.
    let asked = tokio::time::timeout(
        Duration::from_secs(5),
        pi.prompt(prompt("c-1", "m-1", "/ask")),
    );
    asked.await.expect("the prompt returns").unwrap();
    let question = until(&mut pi, |e| matches!(e.event.body(), Body::Question(_))).await;
    assert_eq!(question.event.header().state, State::Waiting);
    let answer = Answer {
        request: "r-1".into(),
        item: question.event.header().item.clone(),
        by: person(),
        given: Given::Questions(vec![Reply {
            question: "q0".into(),
            options: vec!["o0".into()],
            text: None,
        }]),
    };
    pi.answer(answer).await.unwrap();
    let answered = until(&mut pi, |e| matches!(e.event.body(), Body::Question(_))).await;
    assert_eq!(answered.settles, vec!["r-1".into()]);
    // The command started no turn, and the next prompt's turn does not claim it.
    pi.prompt(prompt("c-1", "m-2", "after")).await.unwrap();
    let started = until(&mut pi, |e| is_turn(e, State::Running)).await;
    assert_eq!(started.prompts, vec!["m-2".into()]);
}

#[tokio::test]
async fn a_setting_given_the_value_it_has_is_settled_by_the_session_item() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "hello")).await.unwrap();
    let done = until(&mut pi, |e| is_turn(e, State::Done)).await;
    let session = session_of(&done);
    pi.control(set("r-1", &session, "thinking", "low"))
        .await
        .unwrap();
    let edit = until(&mut pi, |e| !e.settles.is_empty()).await;
    assert!(matches!(edit.event.body(), Body::Session(_)));
    assert_eq!(edit.settles, vec!["r-1".into()]);
    pi.control(set("r-2", &session, "model", "fake/m1"))
        .await
        .unwrap();
    let edit = until(&mut pi, |e| !e.settles.is_empty()).await;
    assert!(matches!(edit.event.body(), Body::Session(_)));
    assert_eq!(edit.settles, vec!["r-2".into()]);
}

#[tokio::test]
async fn a_new_session_an_extension_cancels_is_settled_by_an_error() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "/veto")).await.unwrap();
    pi.prompt(prompt("c-1", "m-2", "hello")).await.unwrap();
    let done = until(&mut pi, |e| is_turn(e, State::Done)).await;
    let old = session_of(&done);
    let fresh = Control {
        request: "r-1".into(),
        by: person(),
        action: Action::NewSession {
            chat: prompt("c-1", "m-0", "").chat,
            session: Some(old.clone()),
        },
    };
    pi.control(fresh).await.unwrap();
    let error = until(&mut pi, |e| !e.settles.is_empty()).await;
    assert!(is_error(&error));
    assert_eq!(error.settles, vec!["r-1".into()]);
    // The session is unchanged, and the next turn is in it.
    pi.prompt(prompt("c-1", "m-3", "again")).await.unwrap();
    let next = until(&mut pi, |e| is_turn(e, State::Done)).await;
    assert_eq!(session_of(&next), old);
    assert!(next.settles.is_empty());
}

#[tokio::test]
async fn an_interrupt_pi_stops_before_acting_on_has_failed() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "sleep crash"))
        .await
        .unwrap();
    let started = until(&mut pi, |e| is_turn(e, State::Running)).await;
    until(&mut pi, |e| matches!(e.event.body(), Body::Tool(_))).await;
    let interrupt = Control {
        request: "r-1".into(),
        by: person(),
        action: Action::Interrupt {
            turn: started.event.header().turn.clone().unwrap(),
        },
    };
    pi.control(interrupt).await.unwrap();
    // The driver closes the turn Pi left open, and that settles nothing: Pi never showed the
    // interrupt's effect. An error says it failed.
    let ended = until(&mut pi, |e| matches!(e.event.body(), Body::Turn(_))).await;
    assert_eq!(ended.event.header().state, State::Cancelled);
    assert!(ended.settles.is_empty());
    let error = next(&mut pi).await;
    assert!(is_error(&error));
    assert_eq!(error.event.header().state, State::Failed);
    assert_eq!(error.settles, vec!["r-1".into()]);
}

#[tokio::test]
async fn a_setting_pi_stops_before_acting_on_has_failed_and_stays_failed() {
    let (mut pi, _work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "hello")).await.unwrap();
    let done = until(&mut pi, |e| is_turn(e, State::Done)).await;
    let session = session_of(&done);
    pi.control(set("r-1", &session, "thinking", "crash"))
        .await
        .unwrap();
    let error = until(&mut pi, |e| !e.settles.is_empty()).await;
    assert!(is_error(&error));
    assert_eq!(error.settles, vec!["r-1".into()]);
    // A new Pi resumes the session; nothing it shows settles the failed control again.
    pi.prompt(prompt("c-1", "m-2", "again")).await.unwrap();
    loop {
        let event = next(&mut pi).await;
        assert!(event.settles.is_empty(), "settled once only");
        if is_turn(&event, State::Done) {
            break;
        }
    }
}

/// Events until the driver asks to send a file, which it gives back. None of them may show the
/// tool's dialog as a question: it is the driver's own request, not the chat's to answer.
async fn file_request(pi: &mut Pi) -> SendFile {
    loop {
        let output = tokio::time::timeout(Duration::from_secs(10), pi.next_event())
            .await
            .expect("an output in time")
            .expect("the driver goes on");
        match output {
            Output::SendFile(file) => return file,
            Output::Event(e) => assert!(
                !matches!(e.event.body(), Body::Question(_)),
                "the tool's dialog was shown as a question"
            ),
        }
    }
}

/// The answers the fake's `send_file` tool got, in order.
fn file_replies(work: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(work.join(".fake-file-replies")).unwrap_or_default();
    (text.lines().map(|l| serde_json::from_str(l).unwrap())).collect()
}

// Pi gets the driver's `send_file` tool from its extension (D-090). The call reaches the core as a
// request for the chat that made it, with a relative path made whole from the folder Pi works in,
// and what the core says goes back to Pi as the tool's result.
#[tokio::test]
async fn a_file_the_agent_sends_goes_to_the_core_and_the_result_back_to_pi() {
    let (mut pi, work) = start().await;
    pi.prompt(prompt("c-1", "m-1", "file:out/notes.txt"))
        .await
        .unwrap();
    let file = file_request(&mut pi).await;
    assert_eq!(file.chat, ChatId::from("c-1"));
    assert_eq!(file.path, work.path().join("out/notes.txt"));
    pi.file_sent(file.id, Ok(())).await;
    until(&mut pi, |e| is_turn(e, State::Done)).await;
    assert_eq!(
        file_replies(work.path()),
        vec![serde_json::json!({"sent": true, "text": "Sent."})]
    );

    // A refusal goes back with the core's words, which name no path; an absolute path stays as
    // the agent gave it.
    pi.prompt(prompt("c-1", "m-2", "file:/elsewhere/notes.txt"))
        .await
        .unwrap();
    let file = file_request(&mut pi).await;
    assert_eq!(file.path, Path::new("/elsewhere/notes.txt"));
    let refused = Refusal {
        reason: Reason::NotAllowed,
        detail: Some("the file is not in a folder the settings allow.".into()),
    };
    pi.file_sent(file.id, Err(refused)).await;
    until(&mut pi, |e| is_turn(e, State::Done)).await;
    let text = "Not sent: the file is not in a folder the settings allow.";
    assert_eq!(
        file_replies(work.path())[1],
        serde_json::json!({"sent": false, "text": text})
    );
}

// A chat that moved works in its own folder (D-101): a relative path is made whole from there,
// not from the driver's.
#[tokio::test]
async fn a_relative_path_is_made_whole_from_the_chats_folder() {
    let (mut pi, work) = start().await;
    let moved = work.path().join("moved");
    std::fs::create_dir(&moved).unwrap();
    let mut asked = prompt("c-1", "m-1", "file:notes.txt");
    asked.chat.folder = Some(moved.clone());
    pi.prompt(asked).await.unwrap();
    let file = file_request(&mut pi).await;
    assert_eq!(file.path, moved.join("notes.txt"));
    pi.file_sent(file.id, Ok(())).await;
    until(&mut pi, |e| is_turn(e, State::Done)).await;
    assert_eq!(file_replies(&moved).len(), 1, "the chat's own Pi heard it");
}
