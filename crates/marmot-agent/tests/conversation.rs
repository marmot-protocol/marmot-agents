//! The core's rules about people, sessions and items, with no MDK and no harness: what arrives
//! in a group, and what the driver does, are handed to the conversation, and the acts it answers
//! with are checked.

use marmot_agent::carrier::{self, Asked, AskedReply, Item};
use marmot_agent::chat_folder::FolderRule;
use marmot_agent::conversation::EFFECT_LIMIT_SECS;
use marmot_agent::{
    Act, Conversation, EventKey, GroupKey, Heard, HeardQuote, HeardRequest, PubKey, Rights, Saved,
    Sending, Where, Who,
};
use marmot_agent_driver::{
    Action, ApprovalCommand, Capabilities, ChatId, Emitted, FilesIn, Given, MidTurn, Position,
    Prompt, Reason, Refusal, Sessions,
};
use marmot_agent_events::body::{
    self, Approval, Body, Choice, Effect, Message, NoticeKind, Setting, StartedBy, Task, Turn,
};
use marmot_agent_events::{AgentEvent, Header, SessionId, State};
use std::path::PathBuf;

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

/// What the acts queued to send, each with its group, as it would be sent now: read from the
/// queue, the one place what is sent is kept. Other acts are passed over.
fn sends(talk: &Conversation, acts: Vec<Act>) -> Vec<(GroupKey, Sending)> {
    let groups: Vec<GroupKey> = (acts.into_iter())
        .filter_map(|act| match act {
            Act::Send { group } => Some(group),
            _ => None,
        })
        .collect();
    let mut taken: Vec<(GroupKey, usize)> = Vec::new();
    let mut found = Vec::new();
    for group in &groups {
        let queued = talk.queued(group);
        let wanted = groups.iter().filter(|g| *g == group).count();
        let used = match taken.iter_mut().find(|(g, _)| g == group) {
            Some((_, used)) => used,
            None => {
                taken.push((group.clone(), 0));
                &mut taken.last_mut().unwrap().1
            }
        };
        let (_, what) = queued[queued.len() - wanted + *used].clone();
        *used += 1;
        found.push((group.clone(), what));
    }
    found
}

/// Every item the acts queued.
fn items_of(talk: &Conversation, acts: Vec<Act>) -> Vec<Item> {
    (sends(talk, acts).into_iter())
        .map(|(_, what)| match what {
            Sending::Item { item, .. } => *item,
            other => panic!("{other:?}"),
        })
        .collect()
}

/// The one item `act` sent, with the group it went to.
fn the_item(
    talk: &mut Conversation,
    act: impl FnOnce(&mut Conversation) -> Vec<Act>,
) -> (GroupKey, String, Item) {
    let acts = act(talk);
    assert_eq!(acts.len(), 1, "not one act: {acts:?}");
    match sends(talk, acts).pop() {
        Some((group, Sending::Item { key, item })) => (group, key, *item),
        other => panic!("not an item: {other:?}"),
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
    let (_, key, item) = the_item(talk, |t| t.emitted(emitted(chat, event)));
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
    let (group_key, _, item) = the_item(&mut talk, |t| t.heard(mention));
    assert_eq!(group_key, GroupKey::from("g-group"));
    assert!(is_refused(&item, "not_allowed"));
    assert_eq!(item.started_by, Some(EventKey::from("e1")));
}

// A typed command that approves needs the right to approve, wherever it counts (D-032, D-047).
#[test]
fn a_typed_approval_needs_the_right_to_approve() {
    let mut talk = conversation(Saved::default());
    let typed = heard(direct(), "e1", FRIEND, "fine, /approve it");
    assert!(is_refused(
        &the_item(&mut talk, |t| t.heard(typed)).2,
        "not_allowed"
    ));
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
            is_refused(&the_item(&mut talk, |t| t.heard(typed)).2, "not_allowed"),
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
    assert!(is_refused(
        &the_item(&mut after_restart, |_| refused).2,
        "busy"
    ));
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

// A prompt whose answer was lost with the connection may have reached the harness: the chat is
// told it may not have been delivered, and it is not sent again (D-037, D-129). One refused is
// told it was not.
#[test]
fn a_prompt_whose_answer_was_lost_may_have_been_delivered() {
    let (mut talk, prompt) = started();
    let lost = Refusal {
        unanswered: true,
        ..Refusal::new(Reason::Failed)
    };
    let told = talk.prompted(&prompt, Err(lost));
    let item = the_item(&mut talk, |_| told).2;
    assert!(is_refused(&item, "failed"));
    assert!(
        item.event
            .header()
            .text
            .starts_with("Your message may not have been passed to the agent"),
        "{}",
        item.event.header().text
    );
    let again = the_prompt(talk.heard(heard(direct(), "e2", OWNER, "and again")));
    let told = talk.prompted(&again, Err(Refusal::new(Reason::Failed)));
    let item = the_item(&mut talk, |_| told).2;
    assert!(
        item.event
            .header()
            .text
            .starts_with("Your message was not passed to the agent")
    );
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
// version, as MDK's edit of it (D-025, D-112).
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
    let (_, _, second) = the_item(&mut talk, |t| {
        t.emitted(emitted(&chat, event("cmd", State::Done, turn())))
    });
    assert_eq!(second.version, 2);
    assert_eq!(second.first, Some(EventKey::from("e-cmd")));
}

// A turn names the message that started it.
#[test]
fn a_turn_names_the_message_that_started_it() {
    let (mut talk, prompt) = started();
    let mut started_turn = emitted(&prompt.chat.id.0, event("t1", State::Running, turn()));
    started_turn.prompts = vec![prompt.message.clone()];
    let (_, _, item) = the_item(&mut talk, |t| t.emitted(started_turn));
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
    let Some((
        _,
        Sending::Words {
            key, text, edit, ..
        },
    )) = sends(&talk, acts).pop()
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
    let (_, _, item) = the_item(&mut talk, |t| t.emitted(done));
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
    let (_, _, item) = the_item(&mut talk, |t| t.asked(asked));
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
        &the_item(&mut talk, |t| t.asked(unknown.clone())).2,
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
    let (_, _, item) = the_item(&mut talk, |t| {
        t.requested(&control.request, Err(Refusal::new(Reason::Busy)), 0)
    });
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
    let (_, _, item) = the_item(&mut talk, |t| t.asked(answer));
    assert!(is_refused(&item, "not_supported"));
    assert!(!item.event.header().text.contains("hunter2"));
}

// A question of several parts, answered from the chat by someone who may prompt, reaches the
// driver as one answer to the item, with each part's choices and text as given, in order; a secret
// part is taken in a two-person chat (D-041). A second answer to it is refused (D-028).
#[test]
fn a_question_of_several_parts_is_answered_at_once() {
    let (mut talk, prompt) = started();
    let part = |id: &str, options: &[&str], secret: bool| body::Asked {
        id: id.into(),
        header: None,
        text: format!("{id}?"),
        options: (options.iter())
            .map(|o| body::Offered {
                id: (*o).into(),
                label: o.to_uppercase(),
                description: None,
            })
            .collect(),
        multiple: options.len() > 1,
        allow_text: true,
        secret,
    };
    let parts = vec![
        part("colour", &["red", "blue", "green"], false),
        part("name", &[], false),
        part("token", &[], true),
    ];
    let question = body::Question::new(body::AnswerVia::Request, None, parts).unwrap();
    let waiting = event("q", State::Waiting, Body::Question(question));
    sent(&mut talk, &prompt.chat.id.0, waiting, "e-q");
    let reply = |question: &str, options: &[&str], text: Option<&str>| AskedReply {
        question: question.into(),
        options: options.iter().map(|o| (*o).into()).collect(),
        text: text.map(Into::into),
    };
    let answer = |event: &str| {
        request(
            direct(),
            event,
            FRIEND,
            Some("e-q"),
            Asked::Questions {
                questions: vec![
                    reply("colour", &["red", "green"], None),
                    reply("name", &[], Some("Ada")),
                    reply("token", &[], Some("s3cret")),
                ],
            },
        )
    };
    let Ok([Act::Answer(given)]) = <[Act; 1]>::try_from(talk.asked(answer("r1"))) else {
        panic!("not one answer")
    };
    assert_eq!(given.item.as_str(), "q");
    let Given::Questions(replies) = &given.given else {
        panic!("not answers to questions")
    };
    let replies: Vec<_> = (replies.iter())
        .map(|r| (r.question.as_str(), r.options.clone(), r.text.as_deref()))
        .collect();
    assert_eq!(
        replies,
        vec![
            ("colour", vec!["red".to_string(), "green".to_string()], None),
            ("name", Vec::new(), Some("Ada")),
            ("token", Vec::new(), Some("s3cret")),
        ]
    );
    assert!(talk.requested(&given.request, Ok(()), 0).is_empty());
    let (_, _, again) = the_item(&mut talk, |t| t.asked(answer("r2")));
    assert!(is_refused(&again, "already_answered"));
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
        &the_item(&mut talk, |t| t.asked(set("r1", "sandbox"))).2,
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
    let items = items_of(&talk, acts);
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
    let acts = restarted.recover();
    let items = items_of(&restarted, acts);
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

// An event id, as MDK reads one: 64 hex characters.
fn hex_id(n: u8) -> EventKey {
    EventKey(format!("{n:02x}").repeat(32))
}

// An item read back from its event is the item that was sent. Its first version is kind 1201;
// a later one is MDK's edit, kind 1009, whose one `e` tag names the first event (D-025, D-076),
// and the message that started the turn is `started_by`, MDK's name for it (D-112).
#[test]
fn an_item_survives_the_carrier() {
    let first = Item {
        event: event("cmd", State::Running, turn()),
        version: 1,
        first: None,
        started_by: Some(hex_id(2)),
        settles: vec![hex_id(3)],
    };
    let later = Item {
        event: event("cmd", State::Done, turn()),
        version: 3,
        first: Some(hex_id(1)),
        ..first.clone()
    };
    for (item, kind) in [(&first, carrier::ITEM_KIND), (&later, carrier::EDIT_KIND)] {
        let out = carrier::item(item).unwrap();
        assert_eq!(out.kind, kind);
        let named: Vec<_> = (out.tags.iter()).filter(|t| t[0] == "e").collect();
        assert_eq!(
            named.len(),
            usize::from(item.first.is_some()),
            "one `e`, on an edit"
        );
        assert_eq!(out.edits(), item.first.as_ref().map(|f| f.as_str()));
        let started = (out.tags.iter()).find(|t| t[0] == "started_by");
        assert_eq!(started.map(|t| t[1].as_str()), Some(hex_id(2).as_str()));
        let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
        assert!(read.item == *item);
        assert_eq!(read.left_out, carrier::LeftOut::default());
    }
    // Neither the agent's words nor an edit of them is an item, nor an item of another kind.
    let out = carrier::item(&first).unwrap();
    assert!(carrier::read_item(9, &out.content, &out.tags).is_none());
    assert!(carrier::read_item(carrier::EDIT_KIND, "words", &[]).is_none());
}

// Every event the carrier writes is one MDK reads as it sends it: an item's first version, a
// later one with MDK's own `e` tag taken off, the stand-in for one too large, and an answer or
// a control (D-076).
#[test]
fn mdk_reads_every_event_the_carrier_writes() {
    use cgka_traits::agent_events::{AgentRequest, validate_agent_item_tags};
    let first = Item {
        event: event("call", State::Waiting, approval()),
        version: 1,
        first: None,
        started_by: Some(hex_id(2)),
        settles: vec![hex_id(3)],
    };
    let huge = Body::Approval(body::Approval {
        subject: None,
        choices: (0..2000)
            .map(|n| Choice {
                id: format!("choice-{n}-{}", "x".repeat(40)),
                label: "Allow".into(),
                effect: Effect::Allow,
            })
            .collect(),
        answer: None,
    });
    let cases = [
        first.clone(),
        Item {
            version: 2,
            first: Some(hex_id(1)),
            ..first.clone()
        },
        Item {
            event: event("call", State::Waiting, huge),
            ..first.clone()
        },
    ];
    for (item, kind) in cases.iter().zip(["approval", "approval", "notice"]) {
        let out = carrier::item(item).unwrap();
        let read = validate_agent_item_tags(&out.tags_but_target()).unwrap();
        assert_eq!(read.version, item.version);
        assert_eq!(read.item_type, kind);
        assert_eq!(
            read.alerts(),
            kind == "approval",
            "a waiting approval alerts (D-024)"
        );
        assert_eq!(read.settles, vec![hex_id(3).0]);
        assert_eq!(read.started_by_message_id_hex, Some(hex_id(2).0));
    }
    let stand_in = carrier::item(&cases[2]).unwrap();
    assert!(
        stand_in
            .tags
            .iter()
            .any(|t| t[0] == "cut" && t[1] == "item")
    );
    let agent = PubKey("ab".repeat(32));
    let out = carrier::request(&agent, Some(&hex_id(1)), &Asked::Interrupt, "Stop");
    assert_eq!(out.kind, carrier::REQUEST_KIND);
    let read = AgentRequest::from_tags(&out.tags).unwrap();
    assert_eq!(read.agent_pubkey_hex, agent.0);
    assert_eq!(read.item_message_id_hex, Some(hex_id(1).0));
    assert_eq!(read.action, "interrupt");
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
    let out = carrier::item(&item).unwrap();
    assert!(out.size() <= carrier::LIMIT, "{}", out.size());
    let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
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
        the_item(&mut talk, |t| t
            .emitted(emitted(&chat, event("t1", State::Done, turn()))))
        .2
        .version,
        2
    );
    let late = request(direct(), "r1", OWNER, Some("e-t1"), Asked::NewSession);
    assert!(is_refused(
        &the_item(&mut talk, |t| t.asked(late)).2,
        "nothing_to_do"
    ));
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
    assert!(is_refused(
        &the_item(&mut talk, |t| t.asked(answer)).2,
        "not_waiting"
    ));
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
    let out = carrier::item(&item).unwrap();
    assert!(out.size() <= carrier::LIMIT, "{}", out.size());
    let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
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
        &the_item(&mut talk, |t| t.asked(allow("r2"))).2,
        "already_answered"
    ));
}

// Every version of a turn names the message that started it, the closing one too.
#[test]
fn a_closed_turn_still_names_its_message() {
    let (mut talk, prompt) = started();
    let mut started_turn = emitted(&prompt.chat.id.0, event("t1", State::Running, turn()));
    started_turn.prompts = vec![prompt.message.clone()];
    let (_, key, _) = the_item(&mut talk, |t| t.emitted(started_turn));
    talk.sent(&key, EventKey::from("e-t1"));
    let (_, _, closed) = the_item(&mut talk, |t| t.ended());
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
    let remark = |talk: &mut Conversation, event| {
        let acts = talk.emitted(emitted(&chat, event));
        match sends(talk, acts).pop() {
            Some((_, Sending::Words { remark, .. })) => remark,
            other => panic!("no words: {other:?}"),
        }
    };
    let commentary = Some(body::Phase::Commentary);
    assert!(remark(&mut talk, said("m1", commentary)));
    let answer = Some(body::Phase::Final);
    assert!(!remark(&mut talk, said("m2", answer)));
    assert!(!remark(&mut talk, said("m3", None)));
    let turn = marmot_agent_events::TurnId::from("t1");
    let tag = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
    assert_eq!(
        carrier::words_tags(Some(&turn), true, &[]),
        vec![tag("turn", "t1"), tag("phase", "commentary")]
    );
    assert_eq!(
        carrier::words_tags(Some(&turn), false, &[]),
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
        let (_, _, item) = the_item(&mut talk, |t| t.overdue(1_120));
        assert!(is_refused(&item, "no_effect"));
        settled.extend(item.settles);
    }
    settled.sort();
    assert_eq!(settled, vec![EventKey::from("r1"), EventKey::from("r2")]);
    assert!(talk.overdue(5_000).is_empty(), "each settled once");
}

// Someone without the right a request needs learns nothing about the item it names: whether the
// item exists, is waiting, was answered or has finished, they are told only `not_allowed` (S6).
#[test]
fn a_request_without_the_right_learns_nothing_of_the_item() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("ap", State::Waiting, approval()),
        "e-ap",
    );
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    talk.emitted(emitted(&chat, event("t1", State::Done, turn())));
    let allow = Asked::Choice {
        choice: "allow".into(),
    };
    assert!(matches!(
        talk.asked(request(direct(), "r0", OWNER, Some("e-ap"), allow.clone()))
            .as_slice(),
        [Act::Answer(_)]
    ));
    let cases = [
        // Unknown item.
        (STRANGER, Some("e-nothing"), Asked::Interrupt),
        // Already answered, and the friend may only prompt.
        (FRIEND, Some("e-ap"), allow.clone()),
        (STRANGER, Some("e-ap"), allow),
        // Finished.
        (STRANGER, Some("e-t1"), Asked::NewSession),
        // Nothing to interrupt.
        (STRANGER, None, Asked::Interrupt),
    ];
    for (n, (author, target, asked)) in cases.into_iter().enumerate() {
        let event = format!("r{}", n + 1);
        let (_, _, item) = the_item(&mut talk, |t| {
            t.asked(request(direct(), &event, author, target, asked))
        });
        assert!(is_refused(&item, "not_allowed"), "case {n}");
        assert_eq!(item.settles, vec![EventKey::from(event.as_str())]);
    }
}

fn words(state: State) -> AgentEvent {
    event("w1", state, Body::Message(Message::default()))
}

/// The one chat message `act` sent, with the requests it settles.
fn the_words(
    talk: &mut Conversation,
    act: impl FnOnce(&mut Conversation) -> Vec<Act>,
) -> (String, Vec<EventKey>) {
    let acts = act(talk);
    assert_eq!(acts.len(), 1, "not one act: {acts:?}");
    match sends(talk, acts).pop() {
        Some((_, Sending::Words { text, settles, .. })) => (text, settles),
        other => panic!("not words: {other:?}"),
    }
}

// An event of any type can show a request's effect: the agent's words settle what they name, as a
// tag on the chat message, and the request is not settled again when the driver stops (S1, D-028).
// Words that can only go out as an edit cannot carry a tag, so the turn's next version, queued
// right after the edit, settles what they name: by the effect, not as `no_effect` or failed.
#[test]
fn the_agents_words_settle_what_they_name() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let take = |talk: &mut Conversation, event: &str| {
        let interrupt = request(direct(), event, OWNER, Some("e-t1"), Asked::Interrupt);
        let Some(Act::Control(control)) = talk.asked(interrupt).into_iter().next() else {
            panic!("no control")
        };
        assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
        control
    };
    // Still running: nothing is sent yet, and the request waits for the words that go out.
    let first = take(&mut talk, "r1");
    let mut running = emitted(&chat, words(State::Running));
    running.settles = vec![first.request.clone()];
    assert!(talk.emitted(running).is_empty());
    let (text, settles) = the_words(&mut talk, |t| t.emitted(emitted(&chat, words(State::Done))));
    assert_eq!(text, "the plain rendering");
    assert_eq!(settles, vec![EventKey::from("r1")]);
    talk.sent("c-1/w1", EventKey::from("e-w1"));

    // An edit: the turn's next version settles the request, after the edit.
    let second = take(&mut talk, "r2");
    let mut changed = emitted(&chat, words(State::Done));
    changed.event = AgentEvent::new(
        Header {
            text: "other words".into(),
            ..changed.event.header().clone()
        },
        changed.event.body().clone(),
        None,
    )
    .unwrap();
    changed.settles = vec![second.request.clone()];
    let acts = talk.emitted(changed);
    let mut queued = sends(&talk, acts).into_iter();
    assert!(
        matches!(queued.next(), Some((_, Sending::Words { edit: Some(_), settles, .. }))
            if settles.is_empty()),
        "the edit first"
    );
    let settled: Vec<(String, u64, Vec<EventKey>)> = queued
        .filter_map(|(_, what)| match what {
            Sending::Item { item, .. } => Some((
                item.event.header().item.as_str().to_string(),
                item.version,
                item.settles,
            )),
            Sending::Words { .. } => None,
        })
        .collect();
    assert_eq!(settled, vec![("t1".into(), 2, vec![EventKey::from("r2")])]);
    let acts = talk.ended();
    let failed: Vec<EventKey> = (items_of(&talk, acts).into_iter())
        .filter(|item| is_refused(item, "failed"))
        .flat_map(|item| item.settles)
        .collect();
    assert!(failed.is_empty(), "each was settled once: {failed:?}");
}

// A driver's event settles only requests from its own chat: one that names another chat's
// request, by mistake, settles nothing there, whether it is an item or the agent's words (S1, S5).
#[test]
fn an_event_settles_only_its_own_chats_requests() {
    let (mut talk, prompt) = started();
    let a = prompt.chat.id.0.clone();
    let mut mention = heard(group(), "e-team", OWNER, "hi");
    mention.mentions_agent = true;
    let b = the_prompt(talk.heard(mention)).chat.id.0;
    sent(&mut talk, &a, event("t1", State::Running, turn()), "e-t1");
    let interrupt = request(direct(), "r1", OWNER, Some("e-t1"), Asked::Interrupt);
    let Some(Act::Control(control)) = talk.asked(interrupt).into_iter().next() else {
        panic!("no control")
    };
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());

    let mut said = emitted(&b, words(State::Done));
    said.settles = vec![control.request.clone()];
    let (_, settles) = the_words(&mut talk, |t| t.emitted(said));
    assert!(settles.is_empty(), "nothing of chat A's settled");
    let mut item = emitted(&b, event("tb", State::Running, turn()));
    item.settles = vec![control.request.clone()];
    let (group, _, sent_item) = the_item(&mut talk, |t| t.emitted(item));
    assert_eq!(group, GroupKey::from("g-group"));
    assert!(sent_item.settles.is_empty(), "nothing of chat A's settled");

    let acts = talk.ended();
    let failed: Vec<(GroupKey, Vec<EventKey>)> = (sends(&talk, acts).into_iter())
        .filter_map(|(group, what)| match what {
            Sending::Item { item, .. } if is_refused(&item, "failed") => {
                Some((group, item.settles))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        failed,
        vec![(GroupKey::from("g-direct"), vec![EventKey::from("r1")])],
        "still pending in chat A, settled there when the driver stops"
    );
}

// An answer the driver took whose item stops waiting without naming it had no effect: it is
// settled then, with `no_effect`, and not left pending until the driver stops (S1, D-087).
#[test]
fn an_answer_is_settled_when_its_item_stops_waiting() {
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
    let Some(Act::Answer(answer)) = talk.asked(asked).into_iter().next() else {
        panic!("no answer")
    };
    assert!(talk.requested(&answer.request, Ok(()), 0).is_empty());
    // Still waiting: nothing yet.
    let (_, _, item) = the_item(&mut talk, |t| {
        t.emitted(emitted(&chat, event("ap", State::Waiting, approval())))
    });
    assert!(item.settles.is_empty());

    let acts = talk.emitted(emitted(&chat, event("ap", State::Cancelled, approval())));
    let items = items_of(&talk, acts);
    assert_eq!(items.len(), 2, "the item, then the notice");
    assert_eq!(items[0].event.header().state, State::Cancelled);
    assert!(is_refused(&items[1], "no_effect"));
    assert_eq!(items[1].settles, vec![EventKey::from("r1")]);
    assert!(talk.ended().is_empty(), "nothing left to settle");
}

/// Finishes `count` items of their own in the chat, each sent once.
fn finish_many(talk: &mut Conversation, chat: &str, count: usize) {
    for n in 0..count {
        let item = format!("done-{n}");
        let (_, key, _) = the_item(talk, |t| {
            t.emitted(emitted(chat, event(&item, State::Done, turn())))
        });
        talk.sent(&key, EventKey::from(format!("e-{item}").as_str()));
    }
}

/// Marks every send queued for the direct chat as gone, so no record is kept for want of it.
fn ack_all(talk: &mut Conversation) {
    let mut n = 0;
    while let Some((_, what)) = talk.next_send(&direct().group) {
        let key = match what {
            Sending::Item { key, .. } | Sending::Words { key, .. } => key,
        };
        n += 1;
        talk.sent(&key, EventKey::from(format!("e-ack-{n}").as_str()));
    }
}

/// The names of the items the conversation keeps a record of.
fn kept_items(talk: &Conversation) -> Vec<String> {
    let saved = serde_json::to_value(talk.saved()).unwrap();
    saved["items"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

// The core forgets finished items after a while, but never one that is open again, nor the chat's
// session, which a restart closes and the next prompt resumes: their next versions go on from the
// last, naming the first event, and never start a new row at version 1 (S2, S3). Every send is
// acknowledged before the pressure, so only those rules keep them.
#[test]
fn an_item_that_opens_again_is_not_forgotten() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let session = |state| event("s", state, Body::Session(session_item(false)));
    sent(&mut talk, &chat, session(State::Running), "e-s");
    sent(
        &mut talk,
        &chat,
        event("task", State::Running, turn()),
        "e-task",
    );
    talk.emitted(emitted(&chat, event("task", State::Done, turn())));
    let (_, _, again) = the_item(&mut talk, |t| {
        t.emitted(emitted(&chat, event("task", State::Running, turn())))
    });
    assert_eq!(again.version, 3);
    ack_all(&mut talk);
    // Open again, it is kept however many items finish after it: more than the core keeps, and
    // more than it remembers the place of.
    finish_many(&mut talk, &chat, 5000);
    let (_, _, task) = the_item(&mut talk, |t| {
        t.emitted(emitted(&chat, event("task", State::Done, turn())))
    });
    assert_eq!(
        (task.version, task.first),
        (4, Some(EventKey::from("e-task")))
    );

    // A stop closes the session; it stays the chat's to resume (D-070).
    let mut talk = conversation(talk.saved().clone());
    let _ = talk.ended();
    ack_all(&mut talk);
    finish_many(&mut talk, &chat, 5000);
    let (_, _, resumed) = the_item(&mut talk, |t| {
        t.emitted(emitted(&chat, session(State::Running)))
    });
    assert_eq!(resumed.first, Some(EventKey::from("e-s")));
    assert_eq!(resumed.version, 3);
}

// An item the core has forgotten whose events come again goes on from where it was, for as long
// as the core remembers where that was, rather than starting a new row (S3).
#[test]
fn a_forgotten_item_goes_on_from_its_last_version() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("old", State::Running, turn()),
        "e-old",
    );
    talk.emitted(emitted(&chat, event("old", State::Done, turn())));
    ack_all(&mut talk);
    finish_many(&mut talk, &chat, 3000);
    let old = format!("{chat}/old");
    assert!(!kept_items(&talk).contains(&old), "forgotten");
    let (_, _, late) = the_item(&mut talk, |t| {
        t.emitted(emitted(&chat, event("old", State::Done, turn())))
    });
    assert_eq!(
        (late.version, late.first),
        (3, Some(EventKey::from("e-old")))
    );
}

// A session goes on across connections, and so do its numbers (D-059): the number a session
// reached is kept after the items that showed it are forgotten, so a connector's next connection
// numbers that session's events after it.
#[test]
fn a_sessions_last_number_outlives_its_items() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let header = Header {
        harness: "codex".into(),
        session: Some("earlier".into()),
        turn: Some("t0".into()),
        item: "early".into(),
        seq: 40,
        state: State::Done,
        title: "A title".into(),
        text: "the plain rendering".into(),
    };
    sent(
        &mut talk,
        &chat,
        AgentEvent::new(header, turn(), None).unwrap(),
        "e-early",
    );
    ack_all(&mut talk);
    finish_many(&mut talk, &chat, 3000);
    assert!(
        !kept_items(&talk).contains(&format!("{chat}/early")),
        "forgotten"
    );
    let talk = conversation(talk.saved().clone());
    assert_eq!(talk.saved().last_numbers().get("earlier"), Some(&40));
}

// A state file from before the core kept each session's number apart from its items still has
// those numbers in its items: they are taken up when the conversation starts, so forgetting the
// items afterwards loses none of them (D-059).
#[test]
fn a_state_file_from_before_keeps_its_items_numbers() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let header = Header {
        harness: "codex".into(),
        session: Some("earlier".into()),
        turn: Some("t0".into()),
        item: "early".into(),
        seq: 40,
        state: State::Done,
        title: "A title".into(),
        text: "the plain rendering".into(),
    };
    sent(
        &mut talk,
        &chat,
        AgentEvent::new(header, turn(), None).unwrap(),
        "e-early",
    );
    ack_all(&mut talk);
    let mut before = serde_json::to_value(talk.saved()).unwrap();
    before.as_object_mut().unwrap().remove("numbers");
    let mut talk = conversation(serde_json::from_value(before).unwrap());
    finish_many(&mut talk, &chat, 3000);
    assert!(
        !kept_items(&talk).contains(&format!("{chat}/early")),
        "forgotten"
    );
    assert_eq!(talk.saved().last_numbers().get("earlier"), Some(&40));
}

// Finished items kept only because their last version waited to be sent are forgotten once it
// has gone, without waiting for another item to finish.
#[test]
fn finished_items_are_forgotten_once_their_sends_have_gone() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    for n in 0..2100 {
        let item = format!("done-{n}");
        talk.emitted(emitted(&chat, event(&item, State::Done, turn())));
    }
    assert!(kept_items(&talk).len() >= 2100, "kept while queued");
    ack_all(&mut talk);
    assert!(
        kept_items(&talk).len() <= 2048,
        "{}",
        kept_items(&talk).len()
    );
}

// ------------------------------------------------------------------ every item fits (S7)

/// A small generator of test values, the same on every run.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }

    /// Text of about `bytes` bytes, with letters of one to four bytes and characters JSON
    /// escapes, the control ones to six bytes each.
    fn text(&mut self, bytes: usize) -> String {
        let mix = ["a", "é", "€", "𝄞", "\"", "\\", "\n", "\u{1}", " ", "z"];
        let mut text = String::new();
        while text.len() < bytes {
            text.push_str(mix[self.below(mix.len() as u64)]);
        }
        text
    }

    /// Text that is mostly short and sometimes very long.
    fn long(&mut self) -> String {
        let bytes = self.size();
        self.text(bytes)
    }

    /// Mostly short, sometimes very long.
    fn size(&mut self) -> usize {
        match self.below(4) {
            0 => self.below(40),
            1 => self.below(2_000),
            _ => 10_000 + self.below(150_000),
        }
    }
}

/// An item of some type, with long text in its plain rendering, its title and its fields.
fn a_large_item(dice: &mut Dice) -> Item {
    let body = match dice.below(6) {
        0 => Body::Reasoning(body::Reasoning {}),
        1 => Body::Tool(body::Tool {
            kind: body::ToolKind::Command,
            name: dice.text(10),
            command: Some(dice.long()),
            cwd: None,
            exit_code: None,
            duration_ms: None,
            output: Some(dice.long()),
            changes: (0..dice.below(4))
                .map(|_| body::Change {
                    path: dice.text(20),
                    diff: dice.long(),
                    change: None,
                })
                .collect(),
            query: None,
            path: None,
            input: Some(serde_json::json!({ "args": [dice.long(), 3] })),
            error: Some(dice.long()),
        }),
        2 => Body::Plan(body::Plan {
            text: Some(dice.long()),
            entries: (0..dice.below(6_000))
                .map(|n| body::PlanEntry {
                    text: format!("step {n}"),
                    state: body::PlanEntryState::Pending,
                })
                .collect(),
        }),
        3 => Body::Approval(Approval {
            subject: None,
            choices: vec![Choice {
                id: "allow".into(),
                label: dice.long(),
                effect: Effect::Allow,
            }],
            answer: None,
        }),
        4 => Body::Question(
            body::Question::new(
                body::AnswerVia::Request,
                None,
                vec![body::Asked {
                    id: "q1".into(),
                    header: Some(dice.long()),
                    text: dice.long(),
                    options: Vec::new(),
                    multiple: false,
                    allow_text: true,
                    secret: false,
                }],
            )
            .unwrap(),
        ),
        _ => Body::Task(body::Task {
            kind: "subagent".into(),
            description: Some(dice.long()),
            subject: None,
        }),
    };
    let header = Header {
        harness: "codex".into(),
        session: Some("th1".into()),
        turn: Some("t1".into()),
        item: "big".into(),
        seq: 7,
        state: State::Running,
        title: dice.long(),
        text: dice.long(),
    };
    let native = (dice.below(2) == 0).then(|| marmot_agent_events::Native {
        kind: "item".into(),
        payload: serde_json::value::to_raw_value(&dice.long()).unwrap(),
    });
    Item {
        event: AgentEvent::new(header, body, native).unwrap(),
        version: 2,
        first: Some(EventKey::from("e-first")),
        started_by: None,
        settles: vec![EventKey::from("r1")],
    }
}

/// The size of the event a relay gets, counted here on its own: id, author, time, signature,
/// and the content and tags escaped as JSON.
fn on_the_wire(out: &carrier::Outgoing) -> usize {
    serde_json::json!({
        "id": "0".repeat(64), "pubkey": "0".repeat(64), "created_at": u64::MAX,
        "kind": out.kind, "tags": out.tags, "content": out.content, "sig": "0".repeat(128),
    })
    .to_string()
    .len()
}

// Every item fits in 64 KB, whatever holds its bulk: the plain rendering, the title, a tool's
// output, input, error or diffs, a plan's text or its many steps, an approval's or a question's
// words. Measured as the whole event, escaped; what was cut is named, ids are never cut, and the
// item reads back (S7, D-026, D-057).
#[test]
fn every_item_is_cut_to_fit() {
    let mut dice = Dice(0x5eed_1234_abcd_0042);
    for case in 0..150 {
        let item = a_large_item(&mut dice);
        let out = carrier::item(&item).unwrap();
        assert!(
            on_the_wire(&out) <= carrier::LIMIT,
            "case {case}: {}",
            on_the_wire(&out)
        );
        let read = carrier::read_item(out.kind, &out.content, &out.tags).expect("reads back");
        let (sent, got) = (item.event.header(), read.item.event.header());
        assert_eq!(
            (got.item.as_str(), got.seq, got.state),
            ("big", sent.seq, sent.state)
        );
        assert_eq!((read.item.version, &read.item.settles), (2, &item.settles));
        if let (Body::Approval(a), Body::Approval(b)) = (item.event.body(), read.item.event.body())
        {
            assert_eq!(a.choices[0].id, b.choices[0].id, "case {case}");
        }
        let was_cut = got != sent || read.item.event.body() != item.event.body();
        assert!(
            !was_cut
                || read.left_out.output.is_some()
                || read.left_out.diffs > 0
                || !read.left_out.fields.is_empty(),
            "case {case}: cut without saying so"
        );
    }
}

// The agent's words, a chat message, fit too, cut in the middle on a character's boundary.
#[test]
fn the_agents_words_are_cut_to_fit() {
    let mut dice = Dice(7);
    for _ in 0..50 {
        let text = dice.long();
        let tags = carrier::words_tags(None, false, &[]);
        let fitted = carrier::fit_words(&text, &tags).unwrap();
        let out = carrier::Outgoing {
            kind: 9,
            content: fitted.clone(),
            tags,
        };
        assert!(on_the_wire(&out) <= carrier::LIMIT);
        if fitted != text {
            assert!(fitted.contains("cut:"));
        }
    }
}

/// What is queued next for the direct chat, as it would be sent now: the item's key, version,
/// first event and what it settles.
type Next = (String, u64, Option<EventKey>, Vec<EventKey>);

fn queued(talk: &Conversation) -> Option<Next> {
    match talk.next_send(&direct().group)? {
        (_, Sending::Item { key, item }) => Some((key, item.version, item.first, item.settles)),
        (_, other) => panic!("{other:?}"),
    }
}

// A send keeps its place until it has gone out, however often it fails: a later version of the
// item waits behind it and, once it has gone, names its first event, never going out as if it
// were new (S3). After a restart the queue is as it was, in order (S1).
#[test]
fn a_send_keeps_its_place_until_it_goes() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    // The approval's first version is queued and has not gone out.
    the_item(&mut talk, |t| {
        t.emitted(emitted(&chat, event("ap", State::Waiting, approval())))
    });
    // A request for an item not yet sent is refused as unknown; its notice queues behind.
    let asked = request(direct(), "r1", OWNER, Some("e-not-yet"), Asked::Interrupt);
    the_item(&mut talk, |t| t.asked(asked));
    the_item(&mut talk, |t| {
        t.emitted(emitted(&chat, event("ap", State::Declined, approval())))
    });

    let ap = "c-1/ap".to_string();
    assert_eq!(queued(&talk), Some((ap.clone(), 1, None, Vec::new())));
    let mut talk = conversation(talk.saved().clone());
    assert_eq!(
        queued(&talk),
        Some((ap.clone(), 1, None, Vec::new())),
        "kept"
    );
    assert!(talk.recover().is_empty(), "nothing open, nothing pending");
    talk.sent(&ap, EventKey::from("e-ap"));
    let (notice, version, _, settles) = queued(&talk).unwrap();
    assert_eq!((version, settles), (1, vec![EventKey::from("r1")]));
    talk.sent(&notice, EventKey::from("e-notice"));
    assert_eq!(
        queued(&talk),
        Some((ap.clone(), 2, Some(EventKey::from("e-ap")), Vec::new())),
        "the later version names the first"
    );
    talk.sent(&ap, EventKey::from("e-ap2"));
    assert_eq!(queued(&talk), None);
}

// A version that settles a request is queued with it: after a restart before it went out, it is
// sent as it was, and the request is not also settled as failed (S1).
#[test]
fn a_queued_version_settles_its_request_after_a_restart() {
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
    let Some(Act::Answer(answer)) = talk.asked(asked).into_iter().next() else {
        panic!("no answer")
    };
    assert!(talk.requested(&answer.request, Ok(()), 0).is_empty());
    let mut done = emitted(&chat, event("ap", State::Done, approval()));
    done.settles = vec![answer.request.clone()];
    the_item(&mut talk, |t| t.emitted(done));

    let mut restarted = conversation(talk.saved().clone());
    assert!(restarted.recover().is_empty(), "not settled again");
    assert_eq!(
        queued(&restarted),
        Some((
            "c-1/ap".into(),
            2,
            Some(EventKey::from("e-ap")),
            vec![EventKey::from("r1")]
        ))
    );
}

/// A session item with `count` slash commands, each with a name, a hint and a description of
/// these lengths.
fn commands_item(count: usize, name: usize, hint: usize, description: usize) -> Item {
    let session = body::Session {
        harness_version: "1".into(),
        settings: Vec::new(),
        actions: Vec::new(),
        commands: (0..count)
            .map(|n| body::Command {
                name: format!("c{n:05}{}", "n".repeat(name.saturating_sub(6))),
                description: Some("d".repeat(description)),
                hint: "h".repeat(hint),
            })
            .collect(),
    };
    Item {
        event: event("s", State::Running, Body::Session(session)),
        version: 2,
        first: Some(EventKey::from("e-first")),
        started_by: None,
        settles: vec![EventKey::from("r1")],
    }
}

/// How many `cut` and `left_out` tags an event carries.
fn cut_tags(out: &carrier::Outgoing) -> usize {
    (out.tags.iter())
        .filter(|t| t[0] == "cut" || t[0] == "left_out")
        .count()
}

// A session over the limit drops its commands' descriptions first, and keeps every command with
// its name and hint (D-045): many short descriptions are dropped, never the commands (S7).
#[test]
fn a_session_keeps_every_command_name_and_hint() {
    let item = commands_item(800, 12, 8, 40);
    let out = carrier::item(&item).unwrap();
    assert!(on_the_wire(&out) <= carrier::LIMIT, "{}", on_the_wire(&out));
    let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
    let (Body::Session(sent), Body::Session(got)) = (item.event.body(), read.item.event.body())
    else {
        panic!("not a session")
    };
    assert_eq!(
        got.commands.len(),
        sent.commands.len(),
        "no command dropped"
    );
    for (a, b) in sent.commands.iter().zip(&got.commands) {
        assert_eq!((&a.name, &a.hint), (&b.name, &b.hint));
    }
    assert!(cut_tags(&out) <= 8, "{} cut tags", cut_tags(&out));
}

// A plan of thousands of steps fits, and what was cut is said in a few tags, not one a step (S7).
#[test]
fn a_plan_of_thousands_of_steps_fits_with_few_cut_tags() {
    let plan = Body::Plan(body::Plan {
        text: Some("the plan".into()),
        entries: (0..4000)
            .map(|n| body::PlanEntry {
                text: format!("step {n} {}", "s".repeat(200)),
                state: body::PlanEntryState::Pending,
            })
            .collect(),
    });
    let item = Item {
        event: event("p", State::Running, plan),
        version: 1,
        first: None,
        started_by: None,
        settles: Vec::new(),
    };
    let started = std::time::Instant::now();
    let out = carrier::item(&item).unwrap();
    assert!(on_the_wire(&out) <= carrier::LIMIT, "{}", on_the_wire(&out));
    assert!(cut_tags(&out) <= 8, "{} cut tags", cut_tags(&out));
    assert!(started.elapsed().as_secs() < 5, "fitting took too long");
    let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
    assert!(matches!(read.item.event.body(), Body::Plan(_)));
}

// When what is never cut is over the limit alone, as a session with thousands of long command
// names, nothing over the limit is sent: a short stand-in goes instead, the same item at the same
// version, naming its first event and settling what the version settled (S1, S3, S7).
#[test]
fn an_item_that_cannot_fit_is_sent_as_a_stand_in() {
    let item = commands_item(3000, 40, 20, 0);
    let out = carrier::item(&item).unwrap();
    assert!(on_the_wire(&out) <= carrier::LIMIT, "{}", on_the_wire(&out));
    let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
    assert!(
        matches!(read.item.event.body(), Body::Notice(_)),
        "a stand-in, not a session with commands missing"
    );
    let got = read.item.event.header();
    assert_eq!((got.item.as_str(), got.state), ("s", State::Running));
    assert_eq!(read.item.version, item.version);
    assert_eq!(read.item.first, item.first);
    assert_eq!(read.item.settles, item.settles);
}

/// `count` requests' events, as many as an event may settle.
fn many_settles(count: usize) -> Vec<EventKey> {
    (0..count)
        .map(|n| EventKey::from(format!("{n:064x}").as_str()))
        .collect()
}

// An item that settles very many requests is never sent over the limit, the stand-in included:
// what is never cut, the settles among it, cannot make it over 64 KB (S7).
#[test]
fn an_item_that_settles_very_many_is_never_over_the_limit() {
    let item = Item {
        event: event("t", State::Done, turn()),
        version: 2,
        first: Some(EventKey::from("e-first")),
        started_by: None,
        settles: many_settles(3000),
    };
    if let Ok(out) = carrier::item(&item) {
        assert!(on_the_wire(&out) <= carrier::LIMIT, "{}", on_the_wire(&out));
    }
    // As many as one version settles always fit, in a stand-in at worst.
    let item = Item {
        settles: many_settles(carrier::SETTLES_MAX),
        ..commands_item(3000, 40, 20, 0)
    };
    let out = carrier::item(&item).expect("fits");
    assert!(on_the_wire(&out) <= carrier::LIMIT, "{}", on_the_wire(&out));
    assert_eq!(
        carrier::read_item(out.kind, &out.content, &out.tags)
            .unwrap()
            .item
            .settles
            .len(),
        carrier::SETTLES_MAX
    );
}

// The agent's words with very many settles are never handed back over the limit (S7).
#[test]
fn words_that_settle_very_many_are_never_over_the_limit() {
    let tags = carrier::words_tags(None, false, &many_settles(3000));
    if let Ok(fitted) = carrier::fit_words("hello", &tags) {
        let out = carrier::Outgoing {
            kind: 9,
            content: fitted,
            tags,
        };
        assert!(on_the_wire(&out) <= carrier::LIMIT, "{}", on_the_wire(&out));
    }
}

// A session whose native message alone takes it over the limit loses only that: its commands
// keep their descriptions, which drop only when the item is still too large without it (D-057).
#[test]
fn a_session_keeps_its_descriptions_when_leaving_out_the_native_message_is_enough() {
    let mut item = commands_item(20, 12, 8, 40);
    let native = marmot_agent_events::Native {
        kind: "session".into(),
        payload: serde_json::value::RawValue::from_string(format!(
            "{{\"x\":\"{}\"}}",
            "n".repeat(80_000)
        ))
        .unwrap(),
    };
    item.event = AgentEvent::new(
        item.event.header().clone(),
        item.event.body().clone(),
        Some(native),
    )
    .unwrap();
    let out = carrier::item(&item).unwrap();
    let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
    assert!(read.left_out.native.is_some());
    let Body::Session(got) = read.item.event.body() else {
        panic!("not a session")
    };
    assert!(
        got.commands.iter().all(|c| c.description.is_some()),
        "descriptions kept"
    );
    assert!(
        read.left_out.fields.is_empty(),
        "{:?}",
        read.left_out.fields
    );
}

// A plan thinned in its middle still says, for the steps it kept, the full length of what was cut
// of them: the steps after the gap moved up, and what was noted of them moves with them (S7).
#[test]
fn a_thinned_plan_counts_what_was_cut_of_every_step_it_kept() {
    let plan = Body::Plan(body::Plan {
        text: None,
        entries: (0..4000)
            .map(|n| body::PlanEntry {
                text: format!("{n:05}{}", "s".repeat(295)),
                state: body::PlanEntryState::Pending,
            })
            .collect(),
    });
    let item = Item {
        event: event("p", State::Running, plan),
        version: 1,
        first: None,
        started_by: None,
        settles: Vec::new(),
    };
    let out = carrier::item(&item).unwrap();
    let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
    let Body::Plan(got) = read.item.event.body() else {
        panic!("not a plan")
    };
    let cut = (got.entries.iter())
        .filter(|e| e.text.contains("… cut: 300 bytes in all …"))
        .count();
    assert!(cut > 0);
    let said = (read.left_out.fields.iter())
        .find(|(kind, _)| kind == "steps")
        .map(|(_, total)| *total);
    assert_eq!(said, Some(cut * 300));
}

// The output is cut first, before any other text, as D-057 orders: a long command stays whole
// when cutting the output is enough.
#[test]
fn the_output_is_cut_before_other_text() {
    let command = "c".repeat(50_000);
    let tool = Body::Tool(body::Tool {
        kind: body::ToolKind::Command,
        name: "shell".into(),
        command: Some(command.clone()),
        cwd: None,
        exit_code: Some(0),
        duration_ms: None,
        output: Some("o".repeat(30_000)),
        changes: Vec::new(),
        query: None,
        path: None,
        input: None,
        error: None,
    });
    let item = Item {
        event: event("cmd", State::Done, tool),
        version: 1,
        first: None,
        started_by: None,
        settles: Vec::new(),
    };
    let out = carrier::item(&item).unwrap();
    assert!(on_the_wire(&out) <= carrier::LIMIT);
    let read = carrier::read_item(out.kind, &out.content, &out.tags).unwrap();
    let Body::Tool(got) = read.item.event.body() else {
        panic!("not a tool")
    };
    assert_eq!(got.command.as_ref(), Some(&command), "the command is whole");
    assert_eq!(read.left_out.output, Some(30_000));
}

/// An interrupt of the turn whose first event is `e-t1`, from the owner as `event`, that the
/// driver took.
fn interrupt_taken(talk: &mut Conversation, event: &str) -> marmot_agent_driver::RequestId {
    let interrupt = request(direct(), event, OWNER, Some("e-t1"), Asked::Interrupt);
    let Some(Act::Control(control)) = talk.asked(interrupt).into_iter().next() else {
        panic!("no control")
    };
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    control.request
}

/// Every request a stop settles as failed.
fn failed_at_the_end(talk: &mut Conversation) -> Vec<EventKey> {
    let acts = talk.ended();
    (items_of(talk, acts).into_iter())
        .filter(|item| is_refused(item, "failed"))
        .flat_map(|item| item.settles)
        .collect()
}

// An event that settles more requests than one event can carry settles them over as many
// versions as it takes, each the same, numbered on, and each fitting in 64 KB: none is lost and
// none is sent over the limit (S1, S3, S7).
#[test]
fn an_event_that_settles_very_many_spreads_them_over_versions() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let requests: Vec<_> = (0..450)
        .map(|n| interrupt_taken(&mut talk, &format!("r{n}")))
        .collect();
    let mut done = emitted(&chat, event("t1", State::Done, turn()));
    done.settles = requests.clone();
    let acts = talk.emitted(done);
    let items = items_of(&talk, acts);
    let versions: Vec<u64> = items.iter().map(|i| i.version).collect();
    assert_eq!(versions, vec![2, 3, 4]);
    let mut settled = Vec::new();
    for item in &items {
        assert!(item.settles.len() <= carrier::SETTLES_MAX);
        let out = carrier::item(item).expect("fits");
        assert!(on_the_wire(&out) <= carrier::LIMIT);
        settled.extend(item.settles.clone());
    }
    settled.sort();
    settled.dedup();
    assert_eq!(settled.len(), 450);
    assert!(failed_at_the_end(&mut talk).is_empty());
}

// The agent's words carry at most what MDK takes in a chat message, 64 tags of 16 KB in all,
// with their turn and phase; the turn's next versions settle the rest.
#[test]
fn words_that_settle_very_many_leave_the_rest_to_the_turn() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let requests: Vec<_> = (0..450)
        .map(|n| interrupt_taken(&mut talk, &format!("r{n}")))
        .collect();
    let mut said = emitted(&chat, words(State::Done));
    said.settles = requests;
    let acts = talk.emitted(said);
    let mut settled = Vec::new();
    let mut words = 0;
    for (_, what) in sends(&talk, acts) {
        let settles = match what {
            Sending::Words {
                settles,
                turn,
                remark,
                ..
            } => {
                // MDK takes a chat message with at most 64 tags, of at most 16 KB in all.
                let tags = carrier::words_tags(turn.as_ref(), remark, &settles);
                let bytes: usize = tags.iter().flatten().map(String::len).sum();
                assert!(
                    tags.len() <= 64 && bytes <= 16 * 1024,
                    "MDK takes the words"
                );
                words = settles.len();
                settles
            }
            Sending::Item { item, .. } => item.settles,
        };
        assert!(settles.len() <= carrier::SETTLES_MAX);
        settled.extend(settles);
    }
    assert!(words > 0, "the words settle what they can");
    settled.sort();
    settled.dedup();
    assert_eq!(settled.len(), 450);
    assert!(failed_at_the_end(&mut talk).is_empty());
}

// What fits on the words is sized by what their `settles` tags carry, each request's event id,
// not by the core's shorter key for it: with a long turn, fewer fit, and the turn settles the rest.
#[test]
fn words_settle_what_fits_by_the_requests_events() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let long = "t".repeat(12 * 1024);
    let in_long_turn = |event: AgentEvent| {
        let mut header = event.header().clone();
        header.turn = Some(long.as_str().into());
        AgentEvent::new(header, event.body().clone(), None).unwrap()
    };
    sent(
        &mut talk,
        &chat,
        in_long_turn(event("t1", State::Running, turn())),
        "e-t1",
    );
    let requests: Vec<_> = (0..60)
        .map(|n| interrupt_taken(&mut talk, &format!("{n:064x}")))
        .collect();
    let mut said = emitted(&chat, in_long_turn(words(State::Done)));
    said.settles = requests;
    let acts = talk.emitted(said);
    let mut settled = Vec::new();
    let mut words = 0;
    for (_, what) in sends(&talk, acts) {
        let settles = match what {
            Sending::Words {
                settles,
                turn,
                remark,
                ..
            } => {
                let tags = carrier::words_tags(turn.as_ref(), remark, &settles);
                let bytes: usize = tags.iter().flatten().map(String::len).sum();
                assert!(
                    tags.len() <= 64 && bytes <= 16 * 1024,
                    "MDK takes the words: {} tags, {bytes} bytes",
                    tags.len()
                );
                words = settles.len();
                settles
            }
            Sending::Item { item, .. } => item.settles,
        };
        settled.extend(settles);
    }
    assert!(
        words > 0 && words < 60,
        "the words settle what fits: {words}"
    );
    settled.sort();
    settled.dedup();
    assert_eq!(settled.len(), 60);
    assert!(failed_at_the_end(&mut talk).is_empty());
}

// Words that come again unchanged, and final, still settle what they name: the turn's next
// version carries it, and it is not left to be settled as failed (S1, D-028).
#[test]
fn final_words_that_do_not_change_still_settle_what_they_name() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    the_words(&mut talk, |t| t.emitted(emitted(&chat, words(State::Done))));
    talk.sent(&format!("{chat}/w1"), EventKey::from("e-w1"));
    let request = interrupt_taken(&mut talk, "r1");
    let mut again = emitted(&chat, words(State::Done));
    again.settles = vec![request];
    let acts = talk.emitted(again);
    let items = items_of(&talk, acts);
    assert_eq!(items.len(), 1, "the turn's next version");
    assert_eq!(
        (items[0].event.header().item.as_str(), &items[0].settles),
        ("t1", &vec![EventKey::from("r1")])
    );
    assert!(failed_at_the_end(&mut talk).is_empty());
}

// A send keeps the token MDK knows it by: the same each time it is tried, across a restart, a new
// one only once MDK dropped a try of it, and another for each send and each state file. An edit of
// the agent's words names the token their first message went under (S3).
#[test]
fn a_send_keeps_its_token_until_mdk_drops_a_try() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let group = direct().group;
    talk.emitted(emitted(&chat, words(State::Done)));
    let (first, _) = talk.next_send(&group).unwrap();
    assert!(first.mdk.len() <= 128, "MDK takes at most 128 bytes");
    let again = conversation(talk.saved().clone());
    assert_eq!(
        again.next_send(&group).unwrap().0,
        first,
        "the same after a restart"
    );
    // A core whose state file was lost names its sends the same again, under other tokens.
    let (mut lost, _) = started();
    lost.emitted(emitted(&chat, words(State::Done)));
    let (anew, _) = lost.next_send(&group).unwrap();
    assert_eq!(anew.send, first.send);
    assert_ne!(anew.mdk, first.mdk, "another state file, other tokens");

    assert_eq!(talk.rejected(&first.send), 1);
    let (retry, _) = talk.next_send(&group).unwrap();
    assert_eq!(retry.send, first.send, "the same send");
    assert_ne!(retry.mdk, first.mdk, "under a new token");
    talk.sent(&format!("{chat}/w1"), EventKey::from("e-w1"));

    let mut changed = emitted(&chat, words(State::Done));
    changed.event = AgentEvent::new(
        Header {
            text: "other words".into(),
            ..changed.event.header().clone()
        },
        changed.event.body().clone(),
        None,
    )
    .unwrap();
    talk.emitted(changed);
    let (edit, what) = talk.next_send(&group).unwrap();
    assert!(matches!(what, Sending::Words { edit: Some(_), .. }));
    assert_eq!(
        edit.edits.as_ref(),
        Some(&retry.mdk),
        "the edit names the first message"
    );
    assert_ne!(edit.mdk, retry.mdk, "and is a send of its own");
}

// An item's later version is MDK's edit of its first event, which names the token that event
// went under, as an edit of the agent's words does (D-025, D-109).
#[test]
fn a_later_version_names_the_token_its_first_event_went_under() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let group = direct().group;
    talk.emitted(emitted(&chat, event("cmd", State::Running, turn())));
    let (first, what) = talk.next_send(&group).unwrap();
    assert!(matches!(&what, Sending::Item { item, .. } if item.first.is_none()));
    assert_eq!(first.edits, None, "a first event edits nothing");
    talk.sent(&format!("{chat}/cmd"), hex_id(1));
    talk.emitted(emitted(&chat, event("cmd", State::Done, turn())));
    let (later, what) = talk.next_send(&group).unwrap();
    let Sending::Item { item, .. } = what else {
        panic!("not an item")
    };
    assert_eq!((item.version, item.first), (2, Some(hex_id(1))));
    assert_eq!(
        later.edits.as_ref(),
        Some(&first.mdk),
        "the edit names the first event"
    );
    assert_ne!(later.mdk, first.mdk, "and is a send of its own");
}

// In a chat whose messages disappear, an item's first event can be gone while the item goes on,
// and MDK refuses an edit of it. The version goes out as a new first event, under the same name,
// and the versions after it edit that one; the item's version goes on rising. Kept across a
// restart, so it is saved before it is sent. The agent's words whose message is gone go out as
// a new message.
#[test]
fn a_version_whose_first_event_is_gone_goes_out_as_a_new_first_event() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let group = direct().group;
    talk.emitted(emitted(&chat, event("cmd", State::Running, turn())));
    let _ = talk.next_send(&group).unwrap();
    talk.sent(&format!("{chat}/cmd"), hex_id(1));
    let mut running = event("cmd", State::Running, turn());
    running = AgentEvent::new(
        Header {
            text: "still running".into(),
            ..running.header().clone()
        },
        turn(),
        None,
    )
    .unwrap();
    talk.emitted(emitted(&chat, running));
    let (edit, what) = talk.next_send(&group).unwrap();
    assert!(matches!(&what, Sending::Item { item, .. } if item.first == Some(hex_id(1))));

    talk.first_gone(&edit.send);
    let mut talk = conversation(talk.saved().clone());
    let (anew, what) = talk.next_send(&group).unwrap();
    let Sending::Item { item, .. } = what else {
        panic!("not an item")
    };
    assert_eq!((item.version, &item.first, &anew.edits), (2, &None, &None));
    assert_eq!(carrier::item(&item).unwrap().kind, carrier::ITEM_KIND);
    assert_eq!(anew.send, edit.send, "the same send");
    talk.sent(&format!("{chat}/cmd"), hex_id(2));

    talk.emitted(emitted(&chat, event("cmd", State::Done, turn())));
    let (last, what) = talk.next_send(&group).unwrap();
    let Sending::Item { item, .. } = what else {
        panic!("not an item")
    };
    assert_eq!((item.version, item.first), (3, Some(hex_id(2))));
    assert_eq!(
        last.edits.as_ref(),
        Some(&anew.mdk),
        "names the new first event"
    );

    talk.sent(&format!("{chat}/cmd"), hex_id(3));
    talk.emitted(emitted(&chat, words(State::Running)));
    talk.sent(&format!("{chat}/w1"), hex_id(4));
    let mut changed = emitted(&chat, words(State::Done));
    changed.event = AgentEvent::new(
        Header {
            text: "other words".into(),
            ..changed.event.header().clone()
        },
        changed.event.body().clone(),
        None,
    )
    .unwrap();
    talk.emitted(changed);
    let (edit, what) = talk.next_send(&group).unwrap();
    assert!(matches!(what, Sending::Words { edit: Some(_), .. }));
    talk.first_gone(&edit.send);
    let (_, what) = talk.next_send(&group).unwrap();
    assert!(
        matches!(what, Sending::Words { edit: None, .. }),
        "a new message"
    );
}

// A state file from before kind 1201, whose items' first events went out under a temporary kind
// no edit applies to (D-069): an item's next version goes out as a new first event, of kind 1201,
// and its version goes on rising. The agent's words were chat messages then too, and are still
// edited.
#[test]
fn an_item_from_before_kind_1201_goes_on_as_a_new_first_event() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let group = direct().group;
    sent(
        &mut talk,
        &chat,
        event("cmd", State::Running, turn()),
        "e-old",
    );
    talk.emitted(emitted(&chat, words(State::Running)));
    talk.sent(&format!("{chat}/w1"), EventKey::from("e-words"));
    let mut before = serde_json::to_value(talk.saved()).unwrap();
    before.as_object_mut().unwrap().remove("kind_1201");
    let mut talk = conversation(serde_json::from_value(before).unwrap());

    let mut running = event("cmd", State::Running, turn());
    running = AgentEvent::new(
        Header {
            text: "still running".into(),
            ..running.header().clone()
        },
        turn(),
        None,
    )
    .unwrap();
    talk.emitted(emitted(&chat, running));
    let (token, what) = talk.next_send(&group).unwrap();
    let Sending::Item { item, .. } = what else {
        panic!("not an item")
    };
    assert_eq!((item.version, &item.first, &token.edits), (2, &None, &None));
    assert_eq!(carrier::item(&item).unwrap().kind, carrier::ITEM_KIND);
    talk.sent(&format!("{chat}/cmd"), hex_id(5));

    let mut changed = emitted(&chat, words(State::Done));
    changed.event = AgentEvent::new(
        Header {
            text: "other words".into(),
            ..changed.event.header().clone()
        },
        changed.event.body().clone(),
        None,
    )
    .unwrap();
    talk.emitted(changed);
    let (_, what) = talk.next_send(&group).unwrap();
    let edits = EventKey::from("e-words");
    assert!(matches!(what, Sending::Words { edit: Some(e), .. } if e == edits));
    talk.sent(&format!("{chat}/w1"), edits);

    // Taken up once: after a restart, the new first event is the one the next version names.
    let mut talk = conversation(talk.saved().clone());
    talk.emitted(emitted(&chat, event("cmd", State::Done, turn())));
    let (_, what) = talk.next_send(&group).unwrap();
    assert!(matches!(what, Sending::Item { item, .. } if item.first == Some(hex_id(5))));
}

// A notice of the core's own has a place in its session from 1, as MDK reads an item (D-059):
// the last its chat showed, so it sorts after what came before it.
#[test]
fn a_notice_of_the_cores_own_has_a_place_mdk_takes() {
    let mut talk = conversation(Saved::default());
    let mut mention = heard(group(), "e1", STRANGER, "delete everything");
    mention.mentions_agent = true;
    let (_, _, item) = the_item(&mut talk, |t| t.heard(mention));
    assert!(is_refused(&item, "not_allowed"));
    assert_eq!(item.event.header().seq, 1, "with nothing shown before it");

    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let shown = AgentEvent::new(
        Header {
            seq: 7,
            ..event("cmd", State::Running, turn()).header().clone()
        },
        turn(),
        None,
    )
    .unwrap();
    sent(&mut talk, &chat, shown, "e-cmd");
    let typed = heard(direct(), "e2", STRANGER, "hello");
    let (_, _, item) = the_item(&mut talk, |t| t.heard(typed));
    assert!(is_refused(&item, "not_allowed"));
    assert_eq!(item.event.header().seq, 7);
}

// When MDK refuses an edit of the agent's words, the turn's version queued to settle what the
// edit showed settles nothing: those requests are settled as failed, as the edit failed (S1).
#[test]
fn a_refused_edit_leaves_its_turn_version_settling_nothing() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    the_words(&mut talk, |t| t.emitted(emitted(&chat, words(State::Done))));
    talk.sent(&format!("{chat}/w1"), EventKey::from("e-w1"));
    let request = interrupt_taken(&mut talk, "r2");
    let mut changed = emitted(&chat, words(State::Done));
    changed.event = AgentEvent::new(
        Header {
            text: "other words".into(),
            ..changed.event.header().clone()
        },
        changed.event.body().clone(),
        None,
    )
    .unwrap();
    changed.settles = vec![request];
    talk.emitted(changed);
    let queued = talk.queued(&direct().group);
    let edit = (queued.iter())
        .find(|(_, what)| matches!(what, Sending::Words { edit: Some(_), .. }))
        .map(|(token, _)| token.clone())
        .expect("the edit is queued");
    talk.refused(&edit.send, false);
    let mut failed = Vec::new();
    for (_, what) in talk.queued(&direct().group) {
        let Sending::Item { item, .. } = what else {
            panic!("the edit is gone")
        };
        if is_refused(&item, "failed") {
            failed.extend(item.settles);
        } else {
            assert!(item.settles.is_empty(), "the turn shows nothing");
        }
    }
    assert_eq!(failed, vec![EventKey::from("r2")]);
}

// In a group MDK refuses everything for, as one that is gone, a dropped send is followed by what
// stands in for it, once: the item's close of the core's own, and a `failed` notice in place of
// one that settled a request. Those are the last; when they are dropped too, nothing takes their
// place, so nothing loops (S2, L1).
#[test]
fn what_stands_in_for_a_dropped_send_is_the_last() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let request = interrupt_taken(&mut talk, "r1");
    let mut done = emitted(&chat, event("t1", State::Done, turn()));
    done.settles = vec![request];
    talk.emitted(done);
    let group = direct().group;
    let mut dropped = Vec::new();
    while let Some((token, what)) = talk.next_send(&group) {
        assert!(dropped.len() < 10, "it loops");
        let Sending::Item { item, .. } = what else {
            panic!("no words were queued")
        };
        let header = item.event.header();
        let reason = match item.event.body() {
            Body::Notice(n) => n.reason.clone(),
            _ => None,
        };
        dropped.push((
            header.item.as_str().starts_with("core-"),
            item.version,
            header.state,
            reason,
            item.settles.clone(),
        ));
        talk.refused(&token.send, false);
    }
    let r1 = vec![EventKey::from("r1")];
    let failed = Some("failed".to_string());
    assert_eq!(
        dropped,
        vec![
            (false, 2, State::Done, None, r1.clone()),
            // The item's close, as small as it can be, and the notice that tells of the drop.
            (
                false,
                3,
                State::Done,
                Some("not_delivered".into()),
                Vec::new()
            ),
            (true, 1, State::Done, failed.clone(), r1.clone()),
            // In place of that notice, which settled r1: the last.
            (true, 1, State::Done, failed, r1),
        ]
    );
    assert!(talk.queued(&group).is_empty());
}

/// Sends everything queued for the direct chat as MDK would, with the real carrier: what it
/// cannot fit in 64 KB is refused, as `marmot.rs` refuses it. Gives what went out, in order.
fn deliver_all(talk: &mut Conversation) -> Vec<Item> {
    let group = direct().group;
    let mut out = Vec::new();
    let mut tries = 0;
    while let Some((token, what)) = talk.next_send(&group) {
        tries += 1;
        assert!(tries < 20, "it loops");
        let Sending::Item { key, item } = what else {
            panic!("no words were queued")
        };
        if carrier::item(&item).is_ok() {
            talk.sent(&key, EventKey::from(format!("e-sent-{tries}").as_str()));
            out.push(*item);
        } else {
            talk.refused(&token.send, false);
        }
    }
    out
}

fn reason(item: &Item) -> Option<String> {
    match item.event.body() {
        Body::Notice(n) => n.reason.clone(),
        _ => None,
    }
}

// An item whose close went out stays closed as it went out: a later version, also final, that
// is dropped (one of the versions that spread what an event settles) is not followed by the
// core's close, which would show "not delivered" over the close the chat has. What that version
// settled is settled by the `failed` notice (S1, S2).
#[test]
fn a_final_version_dropped_after_a_delivered_close_leaves_the_close() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let requests: Vec<_> = (0..450)
        .map(|n| interrupt_taken(&mut talk, &format!("r{n}")))
        .collect();
    let mut done = emitted(&chat, event("t1", State::Done, turn()));
    done.settles = requests.clone();
    talk.emitted(done);
    let group = direct().group;
    // Versions 2 and 3 go out, each closing the item; version 4 is refused.
    let mut carried = Vec::new();
    for n in 2..=4 {
        let Some((token, Sending::Item { key, item })) = talk.next_send(&group) else {
            panic!("no version")
        };
        assert_eq!(item.version, n);
        if n < 4 {
            talk.sent(&key, EventKey::from(format!("e-v{n}").as_str()));
        } else {
            carried = item.settles.clone();
            talk.refused(&token.send, false);
        }
    }
    assert_eq!(carried.len(), 50);
    let queued = talk.queued(&group);
    let mut settled = Vec::new();
    for (_, what) in queued {
        let Sending::Item { item, .. } = what else {
            panic!("no words were queued")
        };
        assert!(
            item.event.header().item.as_str().starts_with("core-"),
            "only the `failed` notice follows, not v{} of {}",
            item.version,
            item.event.header().item.as_str()
        );
        assert_eq!(reason(&item).as_deref(), Some("failed"));
        settled.extend(item.settles);
    }
    assert_eq!(settled, carried);
}

// A `failed` notice that tells of a dropped version, and settles nothing, is replaced once when
// it is dropped too, so the chat is told of the gap; what replaces it is the last (S3).
#[test]
fn a_dropped_notice_that_tells_of_a_gap_is_replaced_once() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    talk.emitted(emitted(&chat, event("t1", State::Done, turn())));
    let group = direct().group;
    let mut dropped = Vec::new();
    while let Some((token, what)) = talk.next_send(&group) {
        assert!(dropped.len() < 10, "it loops");
        let Sending::Item { key, item } = what else {
            panic!("no words were queued")
        };
        let core = item.event.header().item.as_str().starts_with("core-");
        dropped.push((core, item.version, reason(&item), item.settles.clone()));
        if reason(&item).as_deref() == Some("not_delivered") {
            talk.sent(&key, EventKey::from("e-close"));
        } else {
            talk.refused(&token.send, false);
        }
    }
    let failed = Some("failed".to_string());
    assert_eq!(
        dropped,
        vec![
            (false, 2, None, Vec::new()),
            (false, 3, Some("not_delivered".into()), Vec::new()),
            (true, 1, failed.clone(), Vec::new()),
            // In place of the notice that told of version 2: the last.
            (true, 1, failed, Vec::new()),
        ]
    );
}

// What stands in for a dropped send does not inherit the size that got it dropped: a session id
// over 64 KB, which the driver chose, makes every version of its item too large, and the core's
// own notices would carry it too. The `failed` notices leave the session out, and the core's
// close of the item leaves out its session and turn, keeping its id, version and final state, so
// the chat is told and the item is closed (S2, S3).
#[test]
fn what_stands_in_for_a_dropped_send_fits_when_the_session_id_does_not() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let huge = "s".repeat(70_000);
    let session = |state| {
        let header = Header {
            harness: "codex".into(),
            session: Some(huge.as_str().into()),
            turn: Some("t1".into()),
            item: "sess".into(),
            seq: 1,
            state,
            title: "Session".into(),
            text: "the plain rendering".into(),
        };
        AgentEvent::new(header, Body::Session(session_item(false)), None).unwrap()
    };
    talk.emitted(emitted(&chat, session(State::Running)));
    let first = deliver_all(&mut talk);
    assert_eq!(
        (first.iter().map(reason)).collect::<Vec<_>>(),
        vec![Some("failed".to_string())],
        "the chat is told of the dropped version"
    );
    assert_eq!(first[0].event.header().session, None);
    talk.emitted(emitted(&chat, session(State::Done)));
    let then = deliver_all(&mut talk);
    let close = (then.iter())
        .find(|i| i.event.header().item.as_str() == "sess")
        .expect("the item is closed in the chat");
    assert_eq!(reason(close).as_deref(), Some("not_delivered"));
    assert_eq!(close.version, 3);
    assert_eq!(close.event.header().state, State::Done);
    assert_eq!(close.event.header().session, None);
    assert_eq!(close.event.header().turn, None);
    assert!(then.iter().any(|i| reason(i).as_deref() == Some("failed")));
}

// ---------------------------------------------------------------------------------- folders

/// A throwaway layout for a chat's folder (D-101): the install folder `work`, holding `a`, a file,
/// a link to `a`, a link out of it, a link to nothing outside, a link to nothing inside, and the
/// agent's account home; and `outside`, beside it.
struct Layout {
    _dir: tempfile::TempDir,
    work: PathBuf,
    outside: PathBuf,
    home: PathBuf,
}

fn layout() -> Layout {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let work = root.join("work");
    let outside = root.join("outside");
    let home = work.join("agent-home");
    for folder in [&work, &outside, &home, &work.join("a").join("deeper")] {
        std::fs::create_dir_all(folder).unwrap();
    }
    std::fs::write(work.join("notes.txt"), "not a folder").unwrap();
    std::os::unix::fs::symlink(work.join("a"), work.join("to-a")).unwrap();
    std::os::unix::fs::symlink(&outside, work.join("to-outside")).unwrap();
    std::os::unix::fs::symlink(outside.join("nope"), work.join("to-nowhere")).unwrap();
    std::os::unix::fs::symlink(work.join("gone"), work.join("to-gone")).unwrap();
    Layout {
        _dir: dir,
        work,
        outside,
        home,
    }
}

/// A conversation whose driver works in `work` by default, with the owner listing `listed`.
fn moving(layout: &Layout, listed: Option<Vec<PathBuf>>, saved: Saved) -> Conversation {
    let rule = FolderRule::new(&layout.work, listed.as_deref(), &layout.home);
    conversation(saved).with_folders(rule)
}

fn set_folder(event: &str, author: &str, target: Option<&str>, value: &str) -> HeardRequest {
    request(
        direct(),
        event,
        author,
        target,
        Asked::Set {
            setting: "folder".into(),
            value: value.into(),
        },
    )
}

/// The folder setting a session item shows.
fn folder_shown(item: &Item) -> Option<Setting> {
    let Body::Session(session) = item.event.body() else {
        panic!("not a session item")
    };
    (session.settings.iter())
        .find(|s| s.id == "folder")
        .cloned()
}

/// The new session a control asks for, and the folder it starts in.
fn the_new_session(acts: Vec<Act>) -> (marmot_agent_driver::Control, Option<SessionId>, PathBuf) {
    match <[Act; 1]>::try_from(acts) {
        Ok([Act::Control(control)]) => match &control.action {
            Action::NewSession { chat, session } => {
                let (session, folder) = (session.clone(), chat.folder.clone());
                (control, session, folder.expect("a folder"))
            }
            other => panic!("not a new session: {other:?}"),
        },
        other => panic!("not one control: {other:?}"),
    }
}

// A chat moves its harness to another folder from its next session (D-101): the session item
// shows the folder, marked as needing the right to approve; setting it starts a new session in
// the folder, resolved; it is the chat's once the driver has taken that new session, and every
// prompt after names it; the new session shows it, and the old one, ending, keeps its own.
#[test]
fn a_chat_moves_its_folder_from_its_next_session() {
    let place = layout();
    let mut talk = moving(&place, None, Saved::default());
    let first = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")));
    assert_eq!(first.chat.folder, None, "the driver's own folder");
    let chat = first.chat.id.0.clone();
    let session = |item: &str, session: &str, state| {
        let mut header = event(item, state, Body::Session(session_item(false)))
            .header()
            .clone();
        header.session = Some(session.into());
        AgentEvent::new(header, Body::Session(session_item(false)), None).unwrap()
    };
    let shown = sent(
        &mut talk,
        &chat,
        session("s1", "th1", State::Running),
        "es1",
    );
    let setting = folder_shown(&shown).expect("the folder setting");
    assert_eq!(setting.value, Some(place.work.display().to_string()));
    assert!(setting.approve, "it needs the right to approve");
    assert_eq!(setting.options, None, "any folder the owner allows");
    // The harness's own settings stay as they were.
    let Body::Session(body) = shown.event.body() else {
        panic!()
    };
    assert!(body.settings.iter().any(|s| s.id == "sandbox"));

    let asked = talk.asked(set_folder("r1", OWNER, Some("es1"), "a/deeper/.."));
    let (control, ends, folder) = the_new_session(asked);
    assert_eq!(folder, place.work.join("a"));
    assert_eq!(
        ends,
        Some(SessionId::from("th1")),
        "the chat's session ends"
    );
    let before = the_prompt(talk.heard(heard(direct(), "e2", OWNER, "meanwhile")));
    assert_eq!(before.chat.folder, None, "not the chat's until taken");
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    let after = the_prompt(talk.heard(heard(direct(), "e3", OWNER, "now")));
    assert_eq!(after.chat.folder, Some(place.work.join("a")));

    let ended = sent(&mut talk, &chat, session("s1", "th1", State::Done), "es1b");
    assert_eq!(
        folder_shown(&ended).and_then(|s| s.value),
        Some(place.work.display().to_string()),
        "a session keeps the folder it started in"
    );
    let mut fresh = emitted(&chat, session("s2", "th2", State::Running));
    fresh.settles = vec![control.request.clone()];
    let (_, _, item) = the_item(&mut talk, |t| t.emitted(fresh));
    assert_eq!(item.settles, vec![EventKey::from("r1")]);
    assert_eq!(
        folder_shown(&item).and_then(|s| s.value),
        Some(place.work.join("a").display().to_string())
    );
    assert_eq!(
        talk.chat_folder(&ChatId(chat)),
        Some(place.work.join("a")),
        "a file it sends comes from there"
    );
}

// Only someone with the right to approve moves a chat, since a folder changes what the agent can
// touch (D-101, D-043): someone who may only prompt is refused, with or without a session item.
#[test]
fn only_someone_who_may_approve_moves_a_chat() {
    let place = layout();
    let mut talk = moving(&place, None, Saved::default());
    let _ = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")));
    let (_, _, refused) = the_item(&mut talk, |t| t.asked(set_folder("r1", FRIEND, None, "a")));
    assert!(is_refused(&refused, "not_allowed"));
    let (_, _, refused) = the_item(&mut talk, |t| {
        t.asked(set_folder("r2", STRANGER, None, "/nowhere"))
    });
    assert!(is_refused(&refused, "not_allowed"));
}

// A chat that has had no session yet may choose its folder: the new session ends none.
#[test]
fn a_chat_with_no_session_yet_chooses_its_folder() {
    let place = layout();
    let mut talk = moving(&place, None, Saved::default());
    let path = place.work.join("a").display().to_string();
    let (control, ends, folder) = the_new_session(talk.asked(set_folder("r1", OWNER, None, &path)));
    assert_eq!((ends, folder), (None, place.work.join("a")));
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    let prompt = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")));
    assert_eq!(prompt.chat.folder, Some(place.work.join("a")));
}

// A folder is resolved, links followed and `..` taken away, and must lie inside one the owner
// listed: one outside, or that leads outside through a link or a `..`, is refused as not allowed;
// so is the account home, which holds the agent's keys; one that is not there, or is a file, is
// refused as invalid, but only once it is known to be inside, so a chat cannot learn what exists
// elsewhere. Nothing is handed to the driver, and the refusal names no path (D-071).
#[test]
fn a_folder_outside_the_owners_list_is_refused() {
    let place = layout();
    let mut talk = moving(&place, None, Saved::default());
    let _ = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")));
    let outside = place.outside.display().to_string();
    let home = place.home.display().to_string();
    let deep_home = place
        .home
        .join("..")
        .join("agent-home")
        .display()
        .to_string();
    let cases = [
        (outside.as_str(), "not_allowed"),
        ("..", "not_allowed"),
        ("a/../../outside", "not_allowed"),
        ("to-outside", "not_allowed"),
        ("/", "not_allowed"),
        (home.as_str(), "not_allowed"),
        (deep_home.as_str(), "not_allowed"),
        ("../no-such-folder", "not_allowed"),
        // A link out says the same whether what it leads to is there or not.
        ("to-nowhere", "not_allowed"),
        ("to-nowhere/deeper", "not_allowed"),
        ("to-outside/nope", "not_allowed"),
        ("no-such-folder", "invalid"),
        ("to-gone", "invalid"),
        ("notes.txt", "invalid"),
        ("", "invalid"),
    ];
    for (n, (value, reason)) in cases.into_iter().enumerate() {
        let (_, _, refused) = the_item(&mut talk, |t| {
            t.asked(set_folder(&format!("r{n}"), OWNER, None, value))
        });
        assert!(
            is_refused(&refused, reason),
            "{value:?}: {:?}",
            refused.event.body()
        );
        let text = &refused.event.header().text;
        assert!(
            !text.contains('/'),
            "{value:?}: the refusal names a path: {text}"
        );
    }
    // A link that stays inside is followed.
    let (_, _, folder) = the_new_session(talk.asked(set_folder("ok", OWNER, None, "to-a")));
    assert_eq!(folder, place.work.join("a"));
}

// The owner's list replaces the install folder as the limit; listing a folder allows every folder
// inside it, so listing `~` lifts the limit within the home (D-101).
#[test]
fn the_owners_list_sets_the_limit() {
    let place = layout();
    let mut talk = moving(&place, Some(vec![place.outside.clone()]), Saved::default());
    let _ = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")));
    let (_, _, refused) = the_item(&mut talk, |t| t.asked(set_folder("r1", OWNER, None, "a")));
    assert!(
        is_refused(&refused, "not_allowed"),
        "the install folder is not listed"
    );
    let outside = place.outside.display().to_string();
    let (_, _, folder) = the_new_session(talk.asked(set_folder("r2", OWNER, None, &outside)));
    assert_eq!(folder, place.outside);
}

// A new session the driver refuses leaves the chat's folder as it was, and the chat is told; one
// still pending when the core stops is settled as failed when it starts again, and moves nothing
// (D-070).
#[test]
fn a_move_that_does_not_happen_leaves_the_folder() {
    let place = layout();
    let mut talk = moving(&place, None, Saved::default());
    let _ = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")));
    let (control, _, _) = the_new_session(talk.asked(set_folder("r1", OWNER, None, "a")));
    let refused = talk.requested(&control.request, Err(Refusal::new(Reason::Busy)), 0);
    assert!(is_refused(&the_item(&mut talk, |_| refused).2, "busy"));
    let prompt = the_prompt(talk.heard(heard(direct(), "e2", OWNER, "still here?")));
    assert_eq!(prompt.chat.folder, None);

    let _ = the_new_session(talk.asked(set_folder("r2", OWNER, None, "a")));
    let mut restarted = moving(&place, None, talk.saved().clone());
    let recovered = restarted.recover();
    let settled = items_of(&restarted, recovered);
    assert!(settled.iter().any(|i| is_refused(i, "failed")));
    let prompt = the_prompt(restarted.heard(heard(direct(), "e3", OWNER, "and now?")));
    assert_eq!(prompt.chat.folder, None);
}

/// A session item for `session`.
fn session_event(item: &str, session: &str, state: State) -> AgentEvent {
    let mut header = event(item, state, Body::Session(session_item(false)))
        .header()
        .clone();
    header.session = Some(session.into());
    AgentEvent::new(header, Body::Session(session_item(false)), None).unwrap()
}

/// A conversation whose direct chat has moved from `th1` to the folder `a`, where its session is
/// `th2`.
fn moved(place: &Layout) -> (Conversation, ChatId) {
    let mut talk = moving(place, None, Saved::default());
    let chat = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")))
        .chat
        .id;
    let _ = sent(
        &mut talk,
        &chat.0,
        session_event("s1", "th1", State::Running),
        "es1",
    );
    let (control, _, _) = the_new_session(talk.asked(set_folder("r1", OWNER, None, "a")));
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    let mut fresh = emitted(&chat.0, session_event("s2", "th2", State::Running));
    fresh.settles = vec![control.request];
    let _ = the_item(&mut talk, |t| t.emitted(fresh));
    (talk, chat)
}

// A chat's folder is kept across a restart, with its session there: the next core's prompts name
// both (D-101).
#[test]
fn a_chats_folder_survives_a_restart() {
    let place = layout();
    let (talk, chat) = moved(&place);
    assert_eq!(talk.chosen_folder(&chat), Some(place.work.join("a")));
    let mut restarted = moving(&place, None, talk.saved().clone());
    let prompt = the_prompt(restarted.heard(heard(direct(), "e2", OWNER, "again")));
    assert_eq!(
        (prompt.chat.folder, prompt.session),
        (Some(place.work.join("a")), Some(SessionId::from("th2")))
    );
}

// The driver taking a new session is not the harness starting it: a harness can still refuse it.
// Until the new session shows, the chat keeps its folder and its session, together, and a core
// stopped meanwhile starts again with both; a move that never shows leaves them (D-101).
#[test]
fn a_move_is_the_chats_only_once_its_session_shows() {
    let place = layout();
    let mut talk = moving(&place, None, Saved::default());
    let chat = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")))
        .chat
        .id;
    let _ = sent(
        &mut talk,
        &chat.0,
        session_event("s1", "th1", State::Running),
        "es1",
    );
    let (control, _, _) = the_new_session(talk.asked(set_folder("r1", OWNER, None, "a")));
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    assert_eq!(talk.chosen_folder(&chat), None);
    assert_eq!(talk.chat_folder(&chat), Some(place.work.clone()));

    let mut restarted = moving(&place, None, talk.saved().clone());
    let _ = restarted.recover();
    let prompt = the_prompt(restarted.heard(heard(direct(), "e2", OWNER, "again")));
    assert_eq!(
        (prompt.chat.folder, prompt.session),
        (None, Some(SessionId::from("th1")))
    );

    // The harness refused it, and showed nothing: it is settled for want of an effect.
    let settled = talk.overdue(u64::MAX / 2);
    assert!(!settled.is_empty());
    assert_eq!(talk.chosen_folder(&chat), None);
    let prompt = the_prompt(talk.heard(heard(direct(), "e3", OWNER, "still here?")));
    assert_eq!(prompt.chat.folder, None);
}

// A move whose new session shows only after the core settled it for want of an effect (D-087) is
// still the chat's then: the harness works in the new folder, so the chat's folder, which its
// files come from, and its session item, show that folder, and not the one it left. The effect
// settles nothing more. Only the running driver can show it, so nothing of it is saved.
#[test]
fn a_move_whose_session_shows_late_is_still_the_chats() {
    let place = layout();
    let mut talk = moving(&place, None, Saved::default());
    let chat = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")))
        .chat
        .id;
    let _ = sent(
        &mut talk,
        &chat.0,
        session_event("s1", "th1", State::Running),
        "es1",
    );
    let (control, _, _) = the_new_session(talk.asked(set_folder("r1", OWNER, None, "a")));
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    let (_, _, overdue) = the_item(&mut talk, |t| t.overdue(EFFECT_LIMIT_SECS));
    assert!(is_refused(&overdue, "no_effect"));
    assert_eq!(talk.chosen_folder(&chat), None, "not moved yet");

    let mut late = emitted(&chat.0, session_event("s2", "th2", State::Running));
    late.settles = vec![control.request.clone()];
    let (_, _, item) = the_item(&mut talk, |t| t.emitted(late));
    assert!(item.settles.is_empty(), "settled already");
    assert_eq!(
        folder_shown(&item).and_then(|s| s.value),
        Some(place.work.join("a").display().to_string()),
    );
    assert_eq!(talk.chosen_folder(&chat), Some(place.work.join("a")));
    let prompt = the_prompt(talk.heard(heard(direct(), "e2", OWNER, "where now?")));
    assert_eq!(prompt.chat.folder, Some(place.work.join("a")));
}

// A new session that names no folder starts where the chat works, and is the chat's there when it
// shows, even after a late move's session (D-087, D-101): a driver that hears of the late move
// while it starts the plain one shows the moved session first and the plain one after, and the
// chat then works where the plain one runs, not in the folder the late move named. So for a chat
// in the driver's own folder and for one that chose a folder.
#[test]
fn a_new_session_after_a_late_move_keeps_its_own_folder() {
    let place = layout();
    let start = |chose: bool| {
        if chose {
            let (talk, chat) = moved(&place);
            (talk, chat, Some(place.work.join("a")), "deeper")
        } else {
            let mut talk = moving(&place, None, Saved::default());
            let chat = the_prompt(talk.heard(heard(direct(), "e1", OWNER, "hello")))
                .chat
                .id;
            let _ = sent(
                &mut talk,
                &chat.0,
                session_event("s1", "th1", State::Running),
                "es1",
            );
            (talk, chat, None, "a")
        }
    };
    for chose in [false, true] {
        let (mut talk, chat, before, to) = start(chose);
        let (moving, _, _) = the_new_session(talk.asked(set_folder("r-move", OWNER, None, to)));
        assert!(talk.requested(&moving.request, Ok(()), 0).is_empty());
        let (_, _, overdue) = the_item(&mut talk, |t| t.overdue(EFFECT_LIMIT_SECS));
        assert!(is_refused(&overdue, "no_effect"));

        let plain = request(direct(), "r-new", OWNER, None, Asked::NewSession);
        let plain = match <[Act; 1]>::try_from(talk.asked(plain)) {
            Ok([Act::Control(control)]) => control,
            other => panic!("not one control: {other:?}"),
        };
        let Action::NewSession {
            chat: sent_chat, ..
        } = &plain.action
        else {
            panic!("not a new session")
        };
        assert_eq!(sent_chat.folder, before, "chose: {chose}");
        assert!(
            talk.requested(&plain.request, Ok(()), EFFECT_LIMIT_SECS)
                .is_empty()
        );

        let mut late = emitted(&chat.0, session_event("s-late", "th-late", State::Running));
        late.settles = vec![moving.request.clone()];
        let _ = the_item(&mut talk, |t| t.emitted(late));
        let mut fresh = emitted(&chat.0, session_event("s-new", "th-new", State::Running));
        fresh.settles = vec![plain.request.clone()];
        let (_, _, item) = the_item(&mut talk, |t| t.emitted(fresh));
        let runs_in = before.clone().unwrap_or_else(|| place.work.clone());
        assert_eq!(
            folder_shown(&item).and_then(|s| s.value),
            Some(runs_in.display().to_string()),
            "chose: {chose}"
        );
        assert_eq!(talk.chosen_folder(&chat), before, "chose: {chose}");
        let prompt = the_prompt(talk.heard(heard(direct(), "e9", OWNER, "where now?")));
        assert_eq!(prompt.chat.folder, before, "chose: {chose}");
    }
}

// A saved folder is checked again when the core starts: one the owner no longer allows is the
// chat's no more, and the chat's session there is not taken up again, so its next prompt starts
// a session in the driver's own folder (D-101).
#[test]
fn a_saved_folder_the_owner_no_longer_allows_is_dropped() {
    let place = layout();
    let (talk, chat) = moved(&place);
    let mut narrowed = moving(
        &place,
        Some(vec![place.outside.clone()]),
        talk.saved().clone(),
    );
    assert_eq!(narrowed.chosen_folder(&chat), None);
    let prompt = the_prompt(narrowed.heard(heard(direct(), "e2", OWNER, "and now?")));
    assert_eq!((prompt.chat.folder, prompt.session), (None, None));
}

// A harness that keeps its own workspace, such as a gateway, states no folder: the core adds no
// folder setting to its session item, and a `set` of a setting named `folder` goes to the driver
// as any setting does.
#[test]
fn a_harness_with_its_own_workspace_gets_no_folder_setting() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let session = event("s", State::Running, Body::Session(session_item(false)));
    let shown = sent(&mut talk, &chat, session, "es");
    assert_eq!(folder_shown(&shown), None);
    let acts = talk.asked(set_folder("r1", FRIEND, Some("es"), "/anywhere"));
    assert!(
        matches!(acts.as_slice(), [Act::Control(c)] if matches!(&c.action, Action::Set { setting, .. } if setting == "folder"))
    );
}

// A send is first tried at a time kept with it, for as long as its token stands: a try MDK
// dropped makes a new token, tried afresh. One given up without knowing whether it went out is
// dropped like a refused one, with what stands in for it, and the chat's notice says it may not
// have been delivered, not that it was not (S1, S3).
#[test]
fn a_send_given_up_unknown_says_it_may_not_have_gone() {
    let (mut talk, _) = started();
    let asked = request(direct(), "r1", OWNER, Some("e-not-yet"), Asked::Interrupt);
    the_item(&mut talk, |t| t.asked(asked));
    let (token, _) = talk.next_send(&direct().group).unwrap();
    assert_eq!(token.tried_at, None);
    assert!(talk.attempting(&token.send, 100), "the first try is saved");
    assert!(!talk.attempting(&token.send, 200), "a later one is not");
    let (tried, _) = talk.next_send(&direct().group).unwrap();
    assert_eq!((tried.tried_at, &tried.mdk), (Some(100), &token.mdk));
    talk.rejected(&token.send);
    let (again, _) = talk.next_send(&direct().group).unwrap();
    assert_eq!(again.tried_at, None, "a new token is tried afresh");
    assert_ne!(again.mdk, token.mdk);

    talk.lost(&token.send);
    let Some((_, Sending::Item { item, .. })) = talk.next_send(&direct().group) else {
        panic!("a notice stands in");
    };
    assert_eq!(item.settles, vec![EventKey::from("r1")]);
    assert!(is_refused(&item, "failed"));
    assert!(
        item.event
            .header()
            .text
            .contains("may not have been delivered")
    );
}

// ------------------------------------------------------------------------------- subagents

fn subagent_task() -> Body {
    Body::Task(Task {
        kind: "subagent".into(),
        description: Some("/root/a".into()),
        subject: Some("a1".into()),
    })
}

// A subagent's work, shown in its parent's chat, is the chat's like any item: a core that stopped
// while it ran closes it when it starts again (D-070, D-117).
#[test]
fn a_restart_closes_a_subagents_open_items() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(&mut talk, &chat, event("t1", State::Done, turn()), "e-t1");
    sent(
        &mut talk,
        &chat,
        event("task:st1", State::Running, subagent_task()),
        "e-task",
    );
    let saved = talk.saved().clone();

    let mut restarted = conversation(saved);
    let acts = restarted.recover();
    let items = items_of(&restarted, acts);
    assert!(
        items
            .iter()
            .any(|i| i.first == Some(EventKey::from("e-task"))
                && i.event.header().state == State::Cancelled),
        "the subagent's task closed"
    );
}

// After the agent's turn has ended, a Stop that names no item stops what still works beside it,
// such as a subagent: the turn its task names (D-029, D-117). With nothing working, nothing is
// sent to the driver.
#[test]
fn a_stop_after_the_turn_stops_what_still_works() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    sent(&mut talk, &chat, event("t1", State::Done, turn()), "e-t1");
    sent(
        &mut talk,
        &chat,
        event("task:st1", State::Running, subagent_task()),
        "e-task",
    );
    let stop = request(direct(), "r1", OWNER, None, Asked::Interrupt);
    let acts = talk.asked(stop);
    assert!(
        matches!(acts.as_slice(), [Act::Control(c)]
            if matches!(&c.action, Action::Interrupt { turn } if turn.as_str() == "t1")),
        "{acts:?}"
    );

    sent(
        &mut talk,
        &chat,
        event("task:st1", State::Cancelled, subagent_task()),
        "e-task2",
    );
    let stop = request(direct(), "r2", OWNER, None, Asked::Interrupt);
    let acts = talk.asked(stop);
    assert!(
        !acts.iter().any(|a| matches!(a, Act::Control(_))),
        "{acts:?}"
    );
}

// ------------------------------------------------------------------------- what a chat is sent

/// Each item type, with whether a chat without full activity is sent it (D-136): the agent's
/// words, the turn's start and end, what it asks and the errors go to every chat; its reasoning,
/// tools, plans, tasks, notices and a subagent's words go only where the owner sees everything.
fn every_type() -> Vec<(AgentEvent, bool)> {
    let tool = Body::Tool(body::Tool {
        kind: body::ToolKind::Command,
        name: "shell".into(),
        command: Some("cat notes".into()),
        cwd: None,
        exit_code: None,
        duration_ms: None,
        output: None,
        changes: Vec::new(),
        query: None,
        path: None,
        input: None,
        error: None,
    });
    let asked = body::Asked {
        id: "q1".into(),
        header: None,
        text: "Which one?".into(),
        options: Vec::new(),
        multiple: false,
        allow_text: true,
        secret: false,
    };
    let question = body::Question::new(body::AnswerVia::Request, None, vec![asked]).unwrap();
    let notice = Body::Notice(body::Notice {
        kind: NoticeKind::Warning,
        duration_ms: None,
        reason: None,
    });
    let subagent_words = Body::Message(Message {
        phase: None,
        parent: Some("a1".into()),
    });
    vec![
        (event("t1", State::Running, turn()), true),
        (
            event("w1", State::Done, Body::Message(Message::default())),
            true,
        ),
        (event("ap", State::Waiting, approval()), true),
        (event("q", State::Waiting, Body::Question(question)), true),
        (
            event("err", State::Done, Body::Error(body::Error::default())),
            true,
        ),
        (
            event("s", State::Running, Body::Session(session_item(false))),
            true,
        ),
        (
            event("r", State::Done, Body::Reasoning(body::Reasoning {})),
            false,
        ),
        (event("cmd", State::Running, tool), false),
        (
            event("p", State::Running, Body::Plan(body::Plan::default())),
            false,
        ),
        (event("task:st1", State::Running, subagent_task()), false),
        (event("n", State::Done, notice), false),
        (event("sw", State::Done, subagent_words), false),
    ]
}

/// A conversation the owner has prompted in the group, and the group chat's id.
fn prompted_in_group(mut talk: Conversation) -> (Conversation, String) {
    let mut mention = heard(group(), "e-g1", OWNER, "look into it");
    mention.mentions_agent = true;
    let prompt = the_prompt(talk.heard(mention));
    (talk, prompt.chat.id.0)
}

/// Whether the event, emitted for `chat`, queued something to send.
fn goes_out(talk: &mut Conversation, chat: &str, event: AgentEvent) -> bool {
    let acts = talk.emitted(emitted(chat, event));
    !sends(talk, acts).is_empty()
}

// A group is sent the agent's words and what people there need to answer it, and none of its
// activity, unless the owner lists the group (D-136). A subagent's approvals and questions are
// the agent's own, so they reach the group; its tasks and its words do not.
#[test]
fn a_group_is_sent_the_agents_words_and_requests_not_its_activity() {
    let (mut talk, chat) = prompted_in_group(conversation(Saved::default()));
    for (event, shown) in every_type() {
        let name = event.header().item.as_str().to_string();
        assert_eq!(goes_out(&mut talk, &chat, event), shown, "{name}");
    }
}

// A group the owner lists is sent everything; another is not (D-136).
#[test]
fn a_group_the_owner_lists_is_sent_everything() {
    let listed = marmot_agent::FullActivity::Groups(vec![GroupKey::from("g-group")]);
    let (mut talk, chat) =
        prompted_in_group(conversation(Saved::default()).with_activity(listed.clone()));
    for (event, _) in every_type() {
        let name = event.header().item.as_str().to_string();
        assert!(goes_out(&mut talk, &chat, event), "{name}");
    }

    let other = marmot_agent::FullActivity::Groups(vec![GroupKey::from("g-other")]);
    let (mut talk, chat) = prompted_in_group(conversation(Saved::default()).with_activity(other));
    let tool = every_type()
        .into_iter()
        .find(|(e, _)| e.header().item.as_str() == "cmd");
    assert!(!goes_out(&mut talk, &chat, tool.unwrap().0));

    let (mut talk, chat) = prompted_in_group(
        conversation(Saved::default()).with_activity(marmot_agent::FullActivity::All),
    );
    for (event, _) in every_type() {
        let name = event.header().item.as_str().to_string();
        assert!(goes_out(&mut talk, &chat, event), "all: {name}");
    }
}

// The owner's two-person chat with the agent is sent everything. Someone else's two-person chat
// with it is not, nor the owner's once a third person is in it (D-136).
#[test]
fn only_the_owners_two_person_chat_is_sent_everything_by_default() {
    let (mut talk, prompt) = started();
    for (event, _) in every_type() {
        let name = event.header().item.as_str().to_string();
        assert!(goes_out(&mut talk, &prompt.chat.id.0, event), "{name}");
    }

    let mut talk = conversation(Saved::default());
    let friends = Where {
        group: GroupKey::from("g-friend"),
        ..direct()
    };
    let prompt = the_prompt(talk.heard(heard(friends, "e1", FRIEND, "hello")));
    for (event, shown) in every_type() {
        let name = event.header().item.as_str().to_string();
        assert_eq!(
            goes_out(&mut talk, &prompt.chat.id.0, event),
            shown,
            "{name}"
        );
    }

    let (mut talk, prompt) = started();
    let three = Where {
        direct: false,
        members: 3,
        ..direct()
    };
    let mut mention = heard(three, "e2", OWNER, "and now?");
    mention.mentions_agent = true;
    the_prompt(talk.heard(mention));
    let reasoning = event("r2", State::Done, Body::Reasoning(body::Reasoning {}));
    assert!(!goes_out(&mut talk, &prompt.chat.id.0, reasoning));
}

// What an item a group is not sent would have settled is settled by the turn's next version, so
// the person who asked is not told that nothing came of it (D-028, D-136).
#[test]
fn what_an_item_not_sent_settles_is_settled_by_its_turn() {
    let (mut talk, chat) = prompted_in_group(conversation(Saved::default()));
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let interrupt = request(group(), "r1", OWNER, Some("e-t1"), Asked::Interrupt);
    let Some(Act::Control(control)) = talk.asked(interrupt).into_iter().next() else {
        panic!("no control")
    };
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    let (tool, _) = every_type().swap_remove(7);
    let mut hidden = emitted(&chat, tool);
    hidden.settles = vec![control.request.clone()];
    let acts = talk.emitted(hidden);
    let items = items_of(&talk, acts);
    assert_eq!(items.len(), 1, "one version of the turn");
    assert!(matches!(items[0].event.body(), Body::Turn(_)));
    assert_eq!(items[0].first, Some(EventKey::from("e-t1")));
    assert_eq!(items[0].settles, vec![EventKey::from("r1")]);
}

// A Stop in a group stops a subagent the group is not shown, as in any chat (D-117, D-136).
#[test]
fn a_stop_in_a_group_stops_a_subagent_it_is_not_shown() {
    let (mut talk, chat) = prompted_in_group(conversation(Saved::default()));
    sent(&mut talk, &chat, event("t1", State::Done, turn()), "e-t1");
    let task = event("task:st1", State::Running, subagent_task());
    assert!(!goes_out(&mut talk, &chat, task));
    let stop = request(group(), "r1", OWNER, None, Asked::Interrupt);
    let acts = talk.asked(stop);
    assert!(
        matches!(acts.as_slice(), [Act::Control(c)]
            if matches!(&c.action, Action::Interrupt { turn } if turn.as_str() == "t1")),
        "{acts:?}"
    );
}

/// The keys of the items queued for `group`, in order.
fn queued_keys(talk: &Conversation, group: &GroupKey) -> Vec<String> {
    (talk.queued(group).into_iter())
        .filter_map(|(_, what)| match what {
            Sending::Item { key, .. } => Some(key),
            Sending::Words { .. } => None,
        })
        .collect()
}

/// An interrupt of the turn whose first event is `e-t1`, from the owner in `chat` as `event`,
/// that the driver took.
fn interrupt_taken_in(
    talk: &mut Conversation,
    chat: Where,
    event: &str,
) -> marmot_agent_driver::RequestId {
    let interrupt = request(chat, event, OWNER, Some("e-t1"), Asked::Interrupt);
    let Some(Act::Control(control)) = talk.asked(interrupt).into_iter().next() else {
        panic!("no control")
    };
    assert!(talk.requested(&control.request, Ok(()), 0).is_empty());
    control.request
}

// Whether a chat is the owner's two-person chat is read from MDK's members at each send, not
// from the last message: a third person added during a turn, with no message since, gets none
// of the agent's activity, not even what was queued before they came, and what that settled is
// settled by the turn. The same after a restart, and in someone else's two-person chat (D-136).
#[test]
fn the_owners_chat_is_read_from_its_members_at_each_send() {
    let (mut talk, prompt) = started();
    let chat = prompt.chat.id.0.clone();
    let g = direct().group;
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let request = interrupt_taken(&mut talk, "r1");
    let (tool, _) = every_type().swap_remove(7);
    let mut queued_tool = emitted(&chat, tool.clone());
    queued_tool.settles = vec![request];
    let acts = talk.emitted(queued_tool);
    assert!(!sends(&talk, acts).is_empty());
    let both = [PubKey::from(AGENT), PubKey::from(OWNER)];
    talk.members_now(&g, Some(&both));
    assert_eq!(queued_keys(&talk, &g), vec![format!("{chat}/cmd")]);

    // A third person is added: MDK no longer marks it a two-person chat.
    talk.members_now(&g, None);
    let keys = queued_keys(&talk, &g);
    assert_eq!(keys, vec![format!("{chat}/t1")], "the tool is held back");
    let Some((_, Sending::Item { item, .. })) = talk.next_send(&g) else {
        panic!("no turn")
    };
    assert_eq!(
        item.settles,
        vec![EventKey::from("r1")],
        "settled by the turn"
    );
    let reasoning = event("r2", State::Done, Body::Reasoning(body::Reasoning {}));
    assert!(!goes_out(&mut talk, &chat, reasoning));
    let done = event(
        "cmd",
        State::Done,
        every_type().swap_remove(7).0.body().clone(),
    );
    assert!(!goes_out(&mut talk, &chat, done), "a later version too");

    // The owner alone with the agent again: its activity goes again.
    talk.members_now(&g, Some(&both));
    let reasoning = event("r3", State::Done, Body::Reasoning(body::Reasoning {}));
    assert!(goes_out(&mut talk, &chat, reasoning));

    // Queued before a restart, checked when it goes after: here, someone else's chat now.
    let saved = talk.saved().clone();
    let mut again = conversation(saved);
    assert!(queued_keys(&again, &g).contains(&format!("{chat}/r3")));
    let theirs = [PubKey::from(AGENT), PubKey::from(FRIEND)];
    again.members_now(&g, Some(&theirs));
    assert!(!queued_keys(&again, &g).contains(&format!("{chat}/r3")));
}

// A notice that settles a control is how the harness says what came of it, such as a refused
// new session or a failed compaction, so a group is sent it; a notice that settles nothing is
// not (D-028, D-136).
#[test]
fn a_notice_that_settles_a_control_reaches_a_group() {
    let (mut talk, chat) = prompted_in_group(conversation(Saved::default()));
    sent(
        &mut talk,
        &chat,
        event("t1", State::Running, turn()),
        "e-t1",
    );
    let request = interrupt_taken_in(&mut talk, group(), "r1");
    let refusal = Body::Notice(body::Notice {
        kind: NoticeKind::Warning,
        duration_ms: None,
        reason: None,
    });
    let mut notice = emitted(&chat, event("n1", State::Done, refusal.clone()));
    notice.settles = vec![request];
    let acts = talk.emitted(notice);
    let items = items_of(&talk, acts);
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0].event.body(), Body::Notice(_)));
    assert_eq!(items[0].settles, vec![EventKey::from("r1")]);

    // Queued so, it still goes when the members are read at its send.
    talk.members_now(&group().group, None);
    assert!(queued_keys(&talk, &group().group).contains(&format!("{chat}/n1")));

    let alone = emitted(&chat, event("n2", State::Done, refusal));
    assert!(!goes_out(&mut talk, &chat, alone.event));
}
