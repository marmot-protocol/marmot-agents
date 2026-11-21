//! The core's rules about people, sessions and items, with no MDK and no harness: what arrives
//! in a group, and what the driver does, are handed to the conversation, and the acts it answers
//! with are checked.

use marmot_agent::carrier::{self, Asked, AskedReply, Item};
use marmot_agent::{
    Act, Conversation, EventKey, GroupKey, Heard, HeardQuote, HeardRequest, PubKey, Rights, Saved,
    Sending, Where, Who,
};
use marmot_agent_driver::{
    Action, ApprovalCommand, Capabilities, ChatId, Emitted, FilesIn, Given, MidTurn, Position,
    Prompt, Reason, Refusal, Sessions,
};
use marmot_agent_events::body::{
    self, Approval, Body, Choice, Effect, Message, NoticeKind, Setting, StartedBy, Turn,
};
use marmot_agent_events::{AgentEvent, Header, SessionId, State};

const AGENT: &str = "agent-key";
const OWNER: &str = "owner-key";
const FRIEND: &str = "friend-key";
const STRANGER: &str = "stranger-key";

fn capabilities() -> Capabilities {
    Capabilities {
        interrupt: true,
        mid_turn: MidTurn::Fold,
        sessions: Sessions::Core,
        resume: true,
        approvals: true,
        files_in: FilesIn::Images,
        approval_commands: vec![ApprovalCommand {
            command: "/approve".into(),
            counts: Position::Anywhere,
        }],
    }
}

fn rights() -> Rights {
    Rights {
        owner: PubKey::from(OWNER),
        prompt: Who::People(vec![PubKey::from(FRIEND)]),
        approve: Who::Owner,
    }
}

fn conversation(saved: Saved) -> Conversation {
    Conversation::new(
        PubKey::from(AGENT),
        "codex",
        rights(),
        capabilities(),
        saved,
    )
}

fn direct() -> Where {
    Where {
        group: GroupKey::from("g-direct"),
        name: None,
        direct: true,
        members: 2,
    }
}

fn group() -> Where {
    Where {
        group: GroupKey::from("g-group"),
        name: Some("Team".into()),
        direct: false,
        members: 4,
    }
}

fn heard(chat: Where, event: &str, author: &str, text: &str) -> Heard {
    Heard {
        chat,
        event: EventKey::from(event),
        author: PubKey::from(author),
        author_name: format!("{author} name"),
        text: text.into(),
        reply_to: None,
        mentions_agent: false,
        attachments: Vec::new(),
    }
}

fn request(
    chat: Where,
    event: &str,
    author: &str,
    target: Option<&str>,
    asked: Asked,
) -> HeardRequest {
    HeardRequest {
        chat,
        event: EventKey::from(event),
        author: PubKey::from(author),
        author_name: format!("{author} name"),
        age_secs: 1,
        target: target.map(EventKey::from),
        asked,
    }
}

fn the_prompt(acts: Vec<Act>) -> Prompt {
    match <[Act; 1]>::try_from(acts) {
        Ok([Act::Prompt(prompt)]) => prompt,
        Ok([other]) => panic!("not a prompt: {other:?}"),
        Err(acts) => panic!("not one act: {acts:?}"),
    }
}

/// The one item sent, with the group it went to.
fn the_item(acts: Vec<Act>) -> (GroupKey, String, Item) {
    match <[Act; 1]>::try_from(acts) {
        Ok(
            [
                Act::Send {
                    group,
                    what: Sending::Item { key, item },
                },
            ],
        ) => (group, key, *item),
        Ok([other]) => panic!("not an item: {other:?}"),
        Err(acts) => panic!("not one act: {acts:?}"),
    }
}

fn is_refused(item: &Item, reason: &str) -> bool {
    matches!(item.event.body(), Body::Notice(n)
        if n.kind == NoticeKind::Refused && n.reason.as_deref() == Some(reason))
}

fn event(item: &str, state: State, body: Body) -> AgentEvent {
    let header = Header {
        harness: "codex".into(),
        session: Some("th1".into()),
        turn: Some("t1".into()),
        item: item.into(),
        seq: 1,
        state,
        title: "A title".into(),
        text: "the plain rendering".into(),
    };
    AgentEvent::new(header, body, None).unwrap()
}

fn turn() -> Body {
    Body::Turn(Turn {
        started_by: StartedBy::User,
        duration_ms: None,
        usage: None,
    })
}

fn approval() -> Body {
    Body::Approval(Approval {
        subject: None,
        choices: vec![Choice {
            id: "allow".into(),
            label: "Allow once".into(),
            effect: Effect::Allow,
        }],
        answer: None,
    })
}

fn emitted(chat: &str, event: AgentEvent) -> Emitted {
    Emitted {
        event,
        chat: Some(ChatId(chat.into())),
        prompts: Vec::new(),
        settles: Vec::new(),
    }
}

/// A conversation that has prompted once in the direct chat, so the chat has an id.
fn started() -> (Conversation, Prompt) {
    let mut talk = conversation(Saved::default());
    let prompt = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")));
    (talk, prompt)
}

/// Sends an item for the direct chat and records that it went out as `first`.
fn sent(talk: &mut Conversation, chat: &str, event: AgentEvent, first: &str) -> Item {
    let (_, key, item) = the_item(talk.emitted(emitted(chat, event)));
    talk.sent(&key, EventKey::from(first));
    item
}

// ------------------------------------------------------------------------------ who answers

// In a direct chat every message is for the agent, and the driver sees the core's ids, never a
// group's id or a public key (D-036, D-075).
#[test]
fn a_direct_message_is_a_prompt_with_the_cores_ids() {
    let (_, prompt) = started();
    assert_eq!(prompt.text, "hello");
    assert!(!prompt.chat.group);
    for id in [&prompt.chat.id.0, &prompt.sender.id.0, &prompt.message.0] {
        assert!(
            !id.contains("key") && !id.contains("g-") && id != "e1",
            "{id}"
        );
    }
    assert_eq!(prompt.sender.name, "owner-key name");
}

// In a group the agent answers a message that mentions it or replies to it, and nothing else
// (D-075).
#[test]
fn in_a_group_only_a_mention_or_a_reply_is_for_the_agent() {
    let mut talk = conversation(Saved::default());
    assert!(talk.heard(heard(group(), "e1", OWNER, "lunch?")).is_empty());

    let mut mention = heard(group(), "e2", OWNER, "run the tests");
    mention.mentions_agent = true;
    assert!(the_prompt(talk.heard(mention)).chat.group);

    let mut reply = heard(group(), "e3", OWNER, "and the docs");
    reply.reply_to = Some(HeardQuote {
        event: EventKey::from("e0"),
        author: PubKey::from(AGENT),
        author_name: "Codex".into(),
        text: "Done.".into(),
    });
    let prompt = the_prompt(talk.heard(reply));
    assert_eq!(prompt.reply_to.unwrap().text, "Done.");

    let mut to_someone_else = heard(group(), "e4", OWNER, "thanks");
    to_someone_else.reply_to = Some(HeardQuote {
        event: EventKey::from("e9"),
        author: PubKey::from(FRIEND),
        author_name: "Friend".into(),
        text: "here".into(),
    });
    assert!(talk.heard(to_someone_else).is_empty());
}

// The agent's own messages, and a message delivered twice, act once at most.
#[test]
fn the_agents_own_message_and_a_repeat_do_nothing() {
    let (mut talk, _) = started();
    assert!(talk.heard(heard(direct(), "e1", OWNER, "hello")).is_empty());
    assert!(
        talk.heard(heard(direct(), "e2", AGENT, "I said"))
            .is_empty()
    );
}

// ------------------------------------------------------------------------------------ rights

// Someone without the right to prompt is told so, in a notice about their message (D-010).
#[test]
fn someone_who_may_not_prompt_is_told() {
    let mut talk = conversation(Saved::default());
    let mut mention = heard(group(), "e1", STRANGER, "delete everything");
    mention.mentions_agent = true;
    let (group_key, _, item) = the_item(talk.heard(mention));
    assert_eq!(group_key, GroupKey::from("g-group"));
    assert!(is_refused(&item, "not_allowed"));
    assert_eq!(item.started_by, Some(EventKey::from("e1")));
}

// A typed command that approves needs the right to approve, wherever it counts (D-032, D-047).
#[test]
fn a_typed_approval_needs_the_right_to_approve() {
    let mut talk = conversation(Saved::default());
    let typed = heard(direct(), "e1", FRIEND, "fine, /approve it");
    assert!(is_refused(&the_item(talk.heard(typed)).2, "not_allowed"));
    let mentioned = heard(direct(), "e2", FRIEND, "what does /approved mean?");
    the_prompt(talk.heard(mentioned));
    let owner = heard(direct(), "e3", OWNER, "/approve");
    the_prompt(talk.heard(owner));
    // A harness reads a command in any case, and with a bot's name or punctuation after it, so
    // those forms need the right too (D-047).
    for (event, text) in [
        ("e4", "/APPROVE"),
        ("e5", "ok /Approve@agent_bot"),
        ("e6", "/approve."),
    ] {
        let typed = heard(direct(), event, FRIEND, text);
        assert!(
            is_refused(&the_item(talk.heard(typed)).2, "not_allowed"),
            "{text}"
        );
    }
}

// --------------------------------------------------------------------------------- sessions

// A prompt names the chat's saved session until the driver has taken one for the chat, and
// then names none (D-065).
#[test]
fn the_saved_session_is_named_until_the_driver_takes_a_prompt() {
    let (mut talk, first) = started();
    assert_eq!(first.session, None, "no session was saved");
    let session = event("s", State::Running, Body::Session(session_item(false)));
    sent(&mut talk, &first.chat.id.0, session, "es");

    let mut after_restart = conversation(talk.saved().clone());
    let again = the_prompt(after_restart.heard(heard(direct(), "e2", OWNER, "where were we")));
    assert_eq!(again.session, Some(SessionId::from("th1")));
    let refused = after_restart.prompted(&again, Err(Refusal::new(Reason::Busy)));
    assert!(is_refused(&the_item(refused).2, "busy"));
    let retry = the_prompt(after_restart.heard(heard(direct(), "e3", OWNER, "where were we?")));
    assert_eq!(
        retry.session,
        Some(SessionId::from("th1")),
        "a refusal took nothing"
    );
    assert!(after_restart.prompted(&retry, Ok(())).is_empty());
    let next = the_prompt(after_restart.heard(heard(direct(), "e4", OWNER, "go on")));
    assert_eq!(next.session, None);
}

fn session_item(approve_sandbox: bool) -> body::Session {
    body::Session {
        harness_version: "0.169.0".into(),
        settings: vec![Setting {
            id: "sandbox".into(),
            label: "Sandbox".into(),
            value: Some("workspace-write".into()),
            options: None,
            approve: approve_sandbox,
        }],
        actions: Vec::new(),
        commands: Vec::new(),
    }
}

// ------------------------------------------------------------------------------------- items

// An item's first version is sent as it is; each later one names the first and has a higher
// version, until MDK takes edits (D-069).
#[test]
fn an_items_later_versions_name_its_first() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let first = sent(
        &mut talk,
        &chat,
        event("cmd", State::Running, turn()),
        "e-cmd",
    );
    assert_eq!((first.version, first.first.clone()), (1, None));
    let (_, _, second) = the_item(talk.emitted(emitted(&chat, event("cmd", State::Done, turn()))));
    assert_eq!(second.version, 2);
    assert_eq!(second.first, Some(EventKey::from("e-cmd")));
}

// A turn names the message that started it.
#[test]
fn a_turn_names_the_message_that_started_it() {
    let (mut talk, prompt) = started();
    let mut started_turn = emitted(&prompt.chat.id.0, event("t1", State::Running, turn()));
    started_turn.prompts = vec![prompt.message.clone()];
    let (_, _, item) = the_item(talk.emitted(started_turn));
    assert_eq!(item.started_by, Some(EventKey::from("e1")));
}

// The agent's words are a chat message; the same words again send nothing; changed words
// replace the message (D-022).
#[test]
fn the_agents_words_are_a_chat_message() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let words = |state| event("msg", state, Body::Message(Message::default()));
    assert!(
        talk.emitted(emitted(&chat, words(State::Running)))
            .is_empty()
    );
    let acts = talk.emitted(emitted(&chat, words(State::Done)));
    let Some(Act::Send {
        what: Sending::Words {
            key, text, edit, ..
        },
        ..
    }) = acts.into_iter().next()
    else {
        panic!("no words")
    };
    assert_eq!((text.as_str(), edit), ("the plain rendering", None));
    talk.sent(&key, EventKey::from("e-words"));
    assert!(talk.emitted(emitted(&chat, words(State::Done))).is_empty());
}

// ------------------------------------------------------------------------ answers, controls

// An answer from someone with the right reaches the driver for the exact item; the event that
// shows its effect settles it, once (D-028).
#[test]
fn an_answer_is_settled_once_by_its_effect() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("ap", State::Waiting, approval()),
        "e-ap",
    );
    let asked = request(
        direct(),
        "r1",
        OWNER,
        Some("e-ap"),
        Asked::Choice {
            choice: "allow".into(),
        },
    );
    let acts = talk.asked(asked);
    let Some(Act::Answer(answer)) = acts.into_iter().next() else {
        panic!("no answer")
    };
    assert_eq!(answer.item.as_str(), "ap");
    assert_eq!(answer.given, Given::Choice("allow".into()));
    assert!(talk.requested(&answer.request, Ok(()), 0).is_empty());
    // An answer is never settled for want of an effect: it keeps its place (D-087).
    assert!(talk.overdue(10_000).is_empty());

    let mut done = emitted(&chat, event("ap", State::Done, approval()));
    done.settles = vec![answer.request.clone()];
    let (_, _, item) = the_item(talk.emitted(done));
    assert_eq!(item.settles, vec![EventKey::from("r1")]);
    assert!(
        talk.requested(&answer.request, Err(Refusal::new(Reason::Failed)), 0)
            .is_empty()
    );
}

// Answering an approval needs the right to approve; the refusal settles the request.
#[test]
fn an_approval_from_someone_who_may_only_prompt_is_refused() {
    let (mut talk, prompt) = started();
    sent(
        &mut talk,
        &prompt.chat.id.0,
        event("ap", State::Waiting, approval()),
        "e-ap",
    );
    let asked = request(
        direct(),
        "r1",
        FRIEND,
        Some("e-ap"),
        Asked::Choice {
            choice: "allow".into(),
        },
    );
    let (_, _, item) = the_item(talk.asked(asked));
    assert!(is_refused(&item, "not_allowed"));
    assert_eq!(item.settles, vec![EventKey::from("r1")]);
}

// A request about an item the agent never sent is refused; one delivered twice acts once; a
// late one that points at nothing is dropped (D-028).
#[test]
fn unknown_repeated_and_late_requests() {
    let (mut talk, _) = started();
    let unknown = request(direct(), "r1", OWNER, Some("e-nothing"), Asked::Interrupt);
    assert!(is_refused(
        &the_item(talk.asked(unknown.clone())).2,
        "unknown_item"
    ));
    assert!(talk.asked(unknown).is_empty());
    let mut late = request(direct(), "r2", OWNER, None, Asked::NewSession);
    late.age_secs = 6 * 60;
    assert!(talk.asked(late).is_empty());
}

// A driver's refusal settles the request with a notice that says why.
#[test]
fn a_refused_control_is_settled_by_a_notice() {
    let (mut talk, _) = started();
    let fresh = request(direct(), "r1", OWNER, None, Asked::NewSession);
    let Some(Act::Control(control)) = talk.asked(fresh).into_iter().next() else {
        panic!("no control")
    };
    assert!(matches!(
        control.action,
        Action::NewSession { session: None, .. }
    ));
    let (_, _, item) =
        the_item(talk.requested(&control.request, Err(Refusal::new(Reason::Busy)), 0));
    assert!(is_refused(&item, "busy"));
    assert_eq!(item.settles, vec![EventKey::from("r1")]);
}

// A secret answer is taken only in a two-person chat (D-041).
#[test]
fn a_secret_answer_is_refused_in_a_group() {
    let mut talk = conversation(Saved::default());
    let mut mention = heard(group(), "e1", OWNER, "log in");
    mention.mentions_agent = true;
    let prompt = the_prompt(talk.heard(mention));
    let asked = body::Asked {
        id: "pw".into(),
        header: None,
        text: "Password?".into(),
        options: Vec::new(),
        multiple: false,
        allow_text: true,
        secret: true,
    };
    let question = body::Question::new(body::AnswerVia::Request, None, vec![asked]).unwrap();
    let waiting = event("q", State::Waiting, Body::Question(question));
    sent(&mut talk, &prompt.chat.id.0, waiting, "e-q");
    let reply = AskedReply {
        question: "pw".into(),
        options: Vec::new(),
        text: Some("hunter2".into()),
    };
    let answer = request(
        group(),
        "r1",
        OWNER,
        Some("e-q"),
        Asked::Questions {
            questions: vec![reply],
        },
    );
    let (_, _, item) = the_item(talk.asked(answer));
    assert!(is_refused(&item, "not_supported"));
    assert!(!item.event.header().text.contains("hunter2"));
}

// A setting the session item marks needs the right to approve; others need the right to
// prompt (D-043).
#[test]
fn a_marked_setting_needs_the_right_to_approve() {
    let (mut talk, prompt) = started();
    let session = event("s", State::Running, Body::Session(session_item(true)));
    sent(&mut talk, &prompt.chat.id.0, session, "es");
    let set = |event, setting: &str| {
        request(
            direct(),
            event,
            FRIEND,
            Some("es"),
            Asked::Set {
                setting: setting.into(),
                value: "x".into(),
            },
        )
    };
    assert!(is_refused(
        &the_item(talk.asked(set("r1", "sandbox"))).2,
        "not_allowed"
    ));
    let acts = talk.asked(set("r2", "model"));
    assert!(
        matches!(acts.as_slice(), [Act::Control(c)] if matches!(&c.action, Action::Set { setting, .. } if setting == "model"))
    );
}

// --------------------------------------------------------------------------------- stopping

// A stopped driver leaves nothing open: items close as interrupted and requests settle as
// failed (D-058, D-070). After a restart, the items a stopped core left open close too.
#[test]
fn a_stop_closes_items_and_settles_requests() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let interrupt = request(direct(), "r1", OWNER, None, Asked::Interrupt);
    let Some(Act::Control(control)) = talk.asked(interrupt).into_iter().next() else {
        panic!("no control")
    };
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    let saved = talk.saved().clone();

    let acts = talk.ended();
    let items: Vec<Item> = acts
        .into_iter()
        .map(|a| match a {
            Act::Send {
                what: Sending::Item { item, .. },
                ..
            } => *item,
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(
        items
            .iter()
            .any(|i| i.event.header().state == State::Cancelled
                && i.first == Some(EventKey::from("e-t1")))
    );
    assert!(
        items
            .iter()
            .any(|i| is_refused(i, "failed") && i.settles == vec![EventKey::from("r1")])
    );

    let mut restarted = conversation(saved);
    let items: Vec<Item> = restarted
        .recover()
        .into_iter()
        .map(|a| match a {
            Act::Send {
                what: Sending::Item { item, .. },
                ..
            } => *item,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(items.len(), 2, "the turn closed and the interrupt settled");
    assert!(
        items
            .iter()
            .any(|i| i.event.header().state == State::Cancelled)
    );
    assert!(
        items
            .iter()
            .any(|i| is_refused(i, "failed") && i.settles == vec![EventKey::from("r1")])
    );
    assert!(restarted.recover().is_empty());
}

// ----------------------------------------------------------------------------------- carrier

// An item read back from its event is the item that was sent.
#[test]
fn an_item_survives_the_carrier() {
    let item = Item {
        event: event("cmd", State::Done, turn()),
        version: 3,
        first: Some(EventKey::from("e-first")),
        started_by: Some(EventKey::from("e-prompt")),
        settles: vec![EventKey::from("r1")],
    };
    let out = carrier::item(&item);
    assert_eq!(out.kind, carrier::ITEM_KIND);
    let read = carrier::read_item(&out.content, &out.tags).unwrap();
    assert!(read.item == item);
    assert_eq!(read.left_out, carrier::LeftOut::default());
}

// Over 64 KB, the native message is left out first, then the output is cut, keeping its start
// and its end, and the event says what was done (D-057).
#[test]
fn an_item_over_the_limit_loses_its_native_message_then_output() {
    let output = format!("START{}END", "x".repeat(100_000));
    let tool = Body::Tool(body::Tool {
        kind: body::ToolKind::Command,
        name: "shell".into(),
        command: Some("cat big".into()),
        cwd: None,
        exit_code: Some(0),
        duration_ms: None,
        output: Some(output.clone()),
        changes: Vec::new(),
        query: None,
        path: None,
        input: None,
        error: None,
    });
    let native = marmot_agent_events::Native {
        kind: "item/completed".into(),
        payload: serde_json::value::RawValue::from_string(format!("{{\"o\":\"{output}\"}}"))
            .unwrap(),
    };
    let header = event("cmd", State::Done, turn()).header().clone();
    let item = Item {
        event: AgentEvent::new(header, tool, Some(native)).unwrap(),
        version: 1,
        first: None,
        started_by: None,
        settles: Vec::new(),
    };
    let out = carrier::item(&item);
    assert!(out.size() <= carrier::LIMIT, "{}", out.size());
    let read = carrier::read_item(&out.content, &out.tags).unwrap();
    assert!(read.item.event.native().is_none());
    assert_eq!(read.left_out.output, Some(output.len()));
    assert!(read.left_out.native.is_some());
    let Body::Tool(cut) = read.item.event.body() else {
        panic!()
    };
    let kept = cut.output.as_deref().unwrap();
    assert!(kept.starts_with("START") && kept.ends_with("END"));
}

// An answer or a control is read only by the agent it names (D-027).
#[test]
fn a_request_is_read_only_by_its_agent() {
    let agent = PubKey::from(AGENT);
    let asked = Asked::Set {
        setting: "model".into(),
        value: "m2".into(),
    };
    let out = carrier::request(&agent, Some(&EventKey::from("es")), &asked, "Model: m2");
    assert_eq!(out.kind, carrier::REQUEST_KIND);
    let (target, read) = carrier::read_request(&agent, &out.tags).unwrap();
    assert_eq!(target, Some(EventKey::from("es")));
    assert!(read == asked);
    assert!(carrier::read_request(&PubKey::from("other"), &out.tags).is_none());
}

// A request that points at an item acts only while that item is live: a new session aimed at a
// finished turn reaches no driver (D-028).
#[test]
fn a_request_about_a_finished_item_does_nothing() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    assert_eq!(
        the_item(talk.emitted(emitted(&chat, event("t1", State::Done, turn()))))
            .2
            .version,
        2
    );
    let late = request(direct(), "r1", OWNER, Some("e-t1"), Asked::NewSession);
    assert!(is_refused(&the_item(talk.asked(late)).2, "nothing_to_do"));
    sent(
        &mut talk,
        &chat,
        event("ap", State::Waiting, approval()),
        "e-ap",
    );
    talk.emitted(emitted(&chat, event("ap", State::Declined, approval())));
    let answer = request(
        direct(),
        "r2",
        OWNER,
        Some("e-ap"),
        Asked::Choice {
            choice: "allow".into(),
        },
    );
    assert!(is_refused(&the_item(talk.asked(answer)).2, "not_waiting"));
}

// Codex puts a file edit's whole diff in the item, so a large diff is cut like large output,
// and text that JSON escapes heavily still ends under the limit (D-057).
#[test]
fn a_large_diff_and_escaped_output_are_cut_to_fit() {
    let tool = Body::Tool(body::Tool {
        kind: body::ToolKind::Edit,
        name: "apply_patch".into(),
        command: None,
        cwd: None,
        exit_code: None,
        duration_ms: None,
        output: Some("\"\n".repeat(30_000)),
        changes: vec![body::Change {
            path: "big.txt".into(),
            diff: format!("+{}", "line\n".repeat(30_000)),
            change: None,
        }],
        query: None,
        path: None,
        input: None,
        error: None,
    });
    let item = Item {
        event: event("edit", State::Done, tool),
        version: 1,
        first: None,
        started_by: None,
        settles: Vec::new(),
    };
    let out = carrier::item(&item);
    assert!(out.size() <= carrier::LIMIT, "{}", out.size());
    let read = carrier::read_item(&out.content, &out.tags).unwrap();
    assert_eq!(read.left_out.diffs, 1);
    assert!(read.left_out.output.is_some());
}

// The first answer wins: a second one to the same item, while the first is with the driver, is
// refused as already answered (D-028).
#[test]
fn a_second_answer_is_refused() {
    let (mut talk, prompt) = started();
    sent(
        &mut talk,
        &prompt.chat.id.0,
        event("ap", State::Waiting, approval()),
        "e-ap",
    );
    let allow = |event| {
        request(
            direct(),
            event,
            OWNER,
            Some("e-ap"),
            Asked::Choice {
                choice: "allow".into(),
            },
        )
    };
    assert!(matches!(
        talk.asked(allow("r1")).as_slice(),
        [Act::Answer(_)]
    ));
    assert!(is_refused(
        &the_item(talk.asked(allow("r2"))).2,
        "already_answered"
    ));
}

// Every version of a turn names the message that started it, the closing one too.
#[test]
fn a_closed_turn_still_names_its_message() {
    let (mut talk, prompt) = started();
    let mut started_turn = emitted(&prompt.chat.id.0, event("t1", State::Running, turn()));
    started_turn.prompts = vec![prompt.message.clone()];
    let (_, key, _) = the_item(talk.emitted(started_turn));
    talk.sent(&key, EventKey::from("e-t1"));
    let (_, _, closed) = the_item(talk.ended());
    assert_eq!(closed.started_by, Some(EventKey::from("e1")));
}

// A remark the harness makes while it works is marked so it does not alert; the answer, and
// words the harness did not mark, are not; the turn tag stays either way (D-078).
#[test]
fn a_remark_is_marked_and_the_answer_is_not() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let said = |item: &str, phase| {
        let message = Message {
            phase,
            parent: None,
        };
        event(item, State::Done, Body::Message(message))
    };
    let remark = |acts: Vec<Act>| match acts.into_iter().next() {
        Some(Act::Send {
            what: Sending::Words { remark, .. },
            ..
        }) => remark,
        other => panic!("no words: {other:?}"),
    };
    let commentary = Some(body::Phase::Commentary);
    assert!(remark(talk.emitted(emitted(&chat, said("m1", commentary)))));
    let answer = Some(body::Phase::Final);
    assert!(!remark(talk.emitted(emitted(&chat, said("m2", answer)))));
    assert!(!remark(talk.emitted(emitted(&chat, said("m3", None)))));
    let turn = marmot_agent_events::TurnId::from("t1");
    let tag = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
    assert_eq!(
        carrier::words_tags(Some(&turn), true),
        vec![tag("turn", "t1"), tag("phase", "commentary")]
    );
    assert_eq!(
        carrier::words_tags(Some(&turn), false),
        vec![tag("turn", "t1")]
    );
}

// A session that has ended, as a new session ends the one before, is not resumed after a restart;
// one the core closed because the driver stopped still is (D-065, D-070).
#[test]
fn an_ended_session_is_not_resumed_and_a_stopped_one_is() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let session = |state| event("s", state, Body::Session(session_item(false)));
    sent(&mut talk, &chat, session(State::Running), "es");
    let mut stopped = conversation(talk.saved().clone());
    let _ = stopped.ended();
    let resumed = the_prompt(conversation(stopped.saved().clone()).heard(heard(
        direct(),
        "e2",
        OWNER,
        "go on",
    )));
    assert_eq!(
        resumed.session,
        Some(SessionId::from("th1")),
        "a stopped session is resumed"
    );

    talk.emitted(emitted(&chat, session(State::Done)));
    let fresh =
        the_prompt(conversation(talk.saved().clone()).heard(heard(direct(), "e3", OWNER, "new")));
    assert_eq!(fresh.session, None, "an ended session is not");
}

// A control the driver took but whose effect never shows is settled by a notice two minutes after
// it was taken, one at a time so each is saved before the next; one whose effect showed is left
// alone, and an answer keeps its place so a second answer is still refused (D-087, D-028).
#[test]
fn a_control_whose_effect_never_shows_is_settled_in_the_end() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let session = event("s1", State::Running, Body::Session(session_item(false)));
    sent(&mut talk, &chat, session, "es");
    let set = |event: &str| {
        request(
            direct(),
            event,
            OWNER,
            Some("es"),
            Asked::Set {
                setting: "model".into(),
                value: "fast".into(),
            },
        )
    };
    let take = |talk: &mut Conversation, event: &str| {
        let Some(Act::Control(control)) = talk.asked(set(event)).into_iter().next() else {
            panic!("no control")
        };
        assert!(talk.requested(&control.request, Ok(()), 1_000).is_empty());
        control
    };
    take(&mut talk, "r1");
    take(&mut talk, "r2");
    let seen = take(&mut talk, "r3");
    let mut effect = emitted(
        &chat,
        event("s1", State::Running, Body::Session(session_item(false))),
    );
    effect.settles = vec![seen.request.clone()];
    talk.emitted(effect);

    assert!(talk.overdue(1_119).is_empty(), "not yet");
    let mut settled = Vec::new();
    for _ in 0..2 {
        let (_, _, item) = the_item(talk.overdue(1_120));
        assert!(is_refused(&item, "no_effect"));
        settled.extend(item.settles);
    }
    settled.sort();
    assert_eq!(settled, vec![EventKey::from("r1"), EventKey::from("r2")]);
    assert!(talk.overdue(5_000).is_empty(), "each settled once");
}
