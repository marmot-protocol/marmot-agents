//! S8, D-071: what the core and the drivers print for debugging holds nothing anyone said or
//! chose, and no key or id the harness or MDK made. Every string an event holds is replaced by a
//! marker, and so is every value a request carries; no marker may appear in what is printed.
//! Only what D-071 lets through stays: the core's own ids, and the ids of settings and actions
//! the harness published.

use crate::events;
use marmot_agent::carrier::{self, Asked, AskedReply, Item};
use marmot_agent::{EventKey, GroupKey, PubKey, Sending};
use marmot_agent_driver::{
    Action, Answer, Attachment, Chat, ChatId, Control, Emitted, Given, MessageId, Person, PersonId,
    Prompt, Quoted, Reason, Refusal, Reply, RequestId,
};
use marmot_agent_events::{AgentEvent, ItemId, SessionId, TurnId};
use proptest::prelude::*;
use serde_json::Value;
use std::path::PathBuf;

const MARK: &str = "zqxMARKxqz";

/// A marker, numbered, so a failure shows which value was printed.
fn mark(n: usize) -> String {
    format!("{MARK}{n}")
}

/// The event with every string it holds that can be changed, a marker: each in turn, kept only
/// when the event still reads (an item's state and type are words from a list).
fn marked(event: &AgentEvent) -> AgentEvent {
    let mut value = serde_json::to_value(event).expect("serializes");
    let mut n = 0;
    let mut strings = Vec::new();
    collect(&value, &mut String::new(), &mut strings);
    // The harness's name and a native message's kind are kinds, which D-071 lets through.
    for pointer in strings
        .into_iter()
        .filter(|p| p != "/harness" && p != "/native/kind")
    {
        let before = value.clone();
        n += 1;
        *value.pointer_mut(&pointer).expect("found") = Value::String(mark(n));
        if serde_json::from_value::<AgentEvent>(value.clone()).is_err() {
            value = before;
        }
    }
    serde_json::from_value(value).expect("kept only when it reads")
}

fn collect(value: &Value, at: &mut String, out: &mut Vec<String>) {
    let mut go = |name: String, item: &Value| {
        let length = at.len();
        at.push('/');
        at.push_str(&name.replace('~', "~0").replace('/', "~1"));
        collect(item, at, out);
        at.truncate(length);
    };
    match value {
        Value::String(_) => out.push(at.clone()),
        Value::Array(items) => (items.iter().enumerate()).for_each(|(n, i)| go(n.to_string(), i)),
        Value::Object(map) => map.iter().for_each(|(k, v)| go(k.clone(), v)),
        _ => {}
    }
}

fn assert_quiet(what: &str, printed: String) -> Result<(), TestCaseError> {
    prop_assert!(
        !printed.contains(MARK),
        "{} prints a value: {}",
        what,
        printed
    );
    Ok(())
}

fn person(n: usize) -> Person {
    Person {
        id: PersonId::from("p1"),
        name: mark(n),
    }
}

fn chat(n: usize) -> Chat {
    Chat {
        id: ChatId::from("c1"),
        group: true,
        name: Some(mark(n)),
        folder: Some(PathBuf::from(mark(n + 1))),
    }
}

proptest! {
    /// An event, the item it goes out as, what is queued to send and what a driver hands the
    /// core print none of its values.
    #[test]
    fn s8_an_event_prints_nothing_it_holds(at in 0..events::pool().len()) {
        // The events as the expected files hold them: what matters is that every string is
        // marked, not how long it is, and marking reads the event again for each of its strings.
        let event: AgentEvent = serde_json::from_value(events::pool()[at].clone()).expect("an event");
        let event = marked(&event);
        assert_quiet("AgentEvent", format!("{event:?}"))?;
        assert_quiet("Header", format!("{:?}", event.header()))?;
        assert_quiet("Body", format!("{:?}", event.body()))?;
        assert_quiet("Native", format!("{:?}", event.native()))?;
        let item = Item {
            event: event.clone(),
            version: 2,
            first: Some(EventKey::from(mark(1).as_str())),
            started_by: Some(EventKey::from(mark(2).as_str())),
            settles: vec![EventKey::from(mark(3).as_str())],
        };
        assert_quiet("Item", format!("{item:?}"))?;
        assert_quiet("Outgoing", format!("{:?}", carrier::item(&item).ok()))?;
        let sending = Sending::Item { key: mark(4), item: Box::new(item) };
        assert_quiet("Sending", format!("{sending:?}"))?;
        let words = Sending::Words {
            key: mark(5),
            text: mark(6),
            turn: Some(TurnId::from(mark(7))),
            remark: true,
            edit: Some(EventKey::from(mark(8).as_str())),
            settles: vec![EventKey::from(mark(9).as_str())],
        };
        assert_quiet("Sending", format!("{words:?}"))?;
        let emitted = Emitted {
            event,
            chat: Some(ChatId::from("c1")),
            prompts: vec![MessageId::from("m1")],
            settles: vec![RequestId::from("r1")],
        };
        assert_quiet("Emitted", format!("{emitted:?}"))?;
    }
}

/// What a person sends, as the core hands it to a driver, and an answer or a control as the
/// core reads it, print none of their values.
#[test]
fn s8_requests_print_nothing_they_carry() -> Result<(), TestCaseError> {
    let prompt = Prompt {
        message: MessageId::from("m1"),
        text: mark(1),
        chat: chat(2),
        sender: person(4),
        reply_to: Some(Quoted {
            message: MessageId::from("m0"),
            text: mark(5),
            author: person(6),
        }),
        attachments: vec![Attachment {
            path: PathBuf::from(mark(7)),
            name: mark(8),
            media_type: mark(9),
        }],
        session: Some(SessionId::from(mark(10))),
    };
    assert_quiet("Prompt", format!("{prompt:?}"))?;
    for given in [
        Given::Choice(mark(11)),
        Given::Questions(vec![Reply {
            question: mark(12),
            options: vec![mark(13)],
            text: Some(mark(14)),
        }]),
    ] {
        let answer = Answer {
            request: RequestId::from("r1"),
            item: ItemId::from(mark(15)),
            by: person(16),
            given,
        };
        assert_quiet("Answer", format!("{answer:?}"))?;
    }
    for action in [
        Action::Interrupt {
            turn: TurnId::from(mark(17)),
        },
        Action::NewSession {
            chat: chat(18),
            session: Some(SessionId::from(mark(20))),
        },
        // A setting's id, and an action's, are the harness's published names, which D-071 lets
        // through; the value chosen is not.
        Action::Set {
            session: SessionId::from(mark(21)),
            setting: "model".into(),
            value: mark(22),
        },
        Action::Own {
            session: SessionId::from(mark(23)),
            id: "compact".into(),
        },
    ] {
        let control = Control {
            request: RequestId::from("r2"),
            by: person(24),
            action,
        };
        assert_quiet("Control", format!("{control:?}"))?;
    }
    let refusal = Refusal {
        reason: Reason::Invalid,
        detail: Some(mark(25)),
        unanswered: false,
    };
    assert_quiet("Refusal", format!("{refusal:?}"))?;
    for asked in [
        Asked::Choice { choice: mark(26) },
        Asked::Questions {
            questions: vec![AskedReply {
                question: mark(27),
                options: vec![mark(28)],
                text: Some(mark(29)),
            }],
        },
        Asked::Set {
            setting: mark(30),
            value: mark(31),
        },
        Asked::Own { id: mark(32) },
    ] {
        assert_quiet("Asked", format!("{asked:?}"))?;
        let out = carrier::request(
            &PubKey::from(mark(33).as_str()),
            Some(&EventKey::from(mark(34).as_str())),
            &asked,
            &mark(35),
        );
        assert_quiet("Outgoing", format!("{out:?}"))?;
    }
    assert_quiet(
        "keys",
        format!(
            "{:?} {:?} {:?}",
            GroupKey::from(mark(36).as_str()),
            EventKey::from(mark(37).as_str()),
            PubKey::from(mark(38).as_str())
        ),
    )?;
    Ok(())
}
