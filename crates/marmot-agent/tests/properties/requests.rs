//! S6 and D-140, differentially: a kind 1202 the core reads and acts on is one MDK reads the same
//! way, so a person's app, which reads it with MDK, shows the press the core acted on. Built from
//! tags in every order, repeated, short, or holding what an app would never write.

use crate::events::hex64;
use cgka_traits::agent_events::AgentRequest;
use marmot_agent::carrier::{self, Asked, AskedReply};
use marmot_agent::{EventKey, PubKey};
use proptest::prelude::*;

const AGENT: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const OTHER: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";

fn word() -> impl Strategy<Value = String> {
    prop_oneof!["[a-z]{1,6}", Just(String::new()), "\\PC{0,4}"]
}

fn asked() -> impl Strategy<Value = Asked> {
    prop_oneof![
        word().prop_map(|choice| Asked::Choice { choice }),
        prop::collection::vec(
            (
                word(),
                prop::collection::vec(word(), 0..3),
                prop::option::of(word())
            ),
            0..3
        )
        .prop_map(|q| Asked::Questions {
            questions: q
                .into_iter()
                .map(|(question, options, text)| AskedReply {
                    question,
                    options,
                    text
                })
                .collect()
        }),
        Just(Asked::Interrupt),
        Just(Asked::NewSession),
        (word(), word()).prop_map(|(setting, value)| Asked::Set { setting, value }),
        word().prop_map(|id| Asked::Own { id }),
    ]
}

/// The `details` of a request: what an app writes, or what it would not.
fn details() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => asked().prop_map(|a| serde_json::to_string(&a).expect("serializes")),
        // Its action, twice; another field, twice; or the action of one and the fields of another.
        1 => (asked(), asked()).prop_map(|(a, b)| {
            let a = serde_json::to_string(&a).expect("serializes");
            let b = serde_json::to_string(&b).expect("serializes");
            format!("{},{}", &a[..a.len() - 1], &b[1..])
        }),
        1 => Just(r#"{"action":"rewind"}"#.to_string()),
        1 => Just(r#"{"action":""}"#.to_string()),
        1 => "\\PC{0,12}",
    ]
}

fn tag() -> impl Strategy<Value = Vec<String>> {
    let t = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    prop_oneof![
        4 => Just(t(&["p", AGENT])),
        1 => Just(t(&["p", OTHER])),
        1 => Just(t(&["p"])),
        2 => hex64().prop_map(move |e| t(&["e", &e])),
        1 => Just(t(&["e"])),
        4 => details().prop_map(move |d| t(&["details", &d])),
        1 => Just(t(&["details"])),
        1 => ("[a-z]{1,3}", "\\PC{0,8}").prop_map(move |(n, v)| t(&[&n, &v])),
        1 => Just(Vec::new()),
    ]
}

/// What an action is called in a request's `details`.
fn action(asked: &Asked) -> &'static str {
    match asked {
        Asked::Choice { .. } => "choice",
        Asked::Questions { .. } => "questions",
        Asked::Interrupt => "interrupt",
        Asked::NewSession => "new_session",
        Asked::Set { .. } => "set",
        Asked::Own { .. } => "own",
    }
}

proptest! {
    /// S6, D-140: what the core acts on is what MDK reads, for this agent, about the same item,
    /// asking the same action. And every request MDK reads for this agent the core hears, so it
    /// is settled.
    #[test]
    fn s6_the_core_acts_only_on_what_mdk_reads_alike(tags in prop::collection::vec(tag(), 0..6)) {
        let agent = PubKey::from(AGENT);
        let core = carrier::read_request(&agent, &tags);
        let mdk = AgentRequest::from_tags(&tags);
        if let Some((target, Ok(asked))) = &core {
            prop_assert!(mdk.is_ok(), "the core acts on what MDK refuses: {:?}", mdk.as_ref().err());
            let mdk = mdk.as_ref().unwrap();
            prop_assert_eq!(&mdk.agent_pubkey_hex, AGENT);
            prop_assert_eq!(mdk.item_message_id_hex.as_deref(), target.as_ref().map(EventKey::as_str));
            prop_assert_eq!(mdk.action.as_str(), action(asked));
        }
        if let Ok(mdk) = &mdk && mdk.agent_pubkey_hex == AGENT {
            prop_assert!(core.is_some(), "a request MDK reads for this agent is not heard");
        }
    }
}

/// The cases the property found, kept as examples: a tag given twice is never read as one of
/// its values, as a field given twice in `details` is not, and an `e` that is no event id names
/// no item (D-140). Before, the core read the first
/// of each, so it acted on a press every app refuses to show.
#[test]
fn a_tag_given_twice_is_not_acted_on() {
    let agent = PubKey::from(AGENT);
    let t = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    let allow = r#"{"action":"choice","choice":"allow"}"#;
    let deny = r#"{"action":"choice","choice":"deny"}"#;
    let e1 = "e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1";
    let e2 = "e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2";
    for tags in [
        vec![
            t(&["p", AGENT]),
            t(&["details", allow]),
            t(&["details", deny]),
        ],
        vec![
            t(&["p", AGENT]),
            t(&["details", allow]),
            t(&["e", e1]),
            t(&["e", e2]),
        ],
        vec![t(&["p", AGENT]), t(&["details"]), t(&["details", allow])],
    ] {
        let read = carrier::read_request(&agent, &tags);
        assert!(
            matches!(read, Some((_, Err(carrier::Unreadable::Invalid)))),
            "{tags:?} is read as {read:?}"
        );
        assert!(AgentRequest::from_tags(&tags).is_err());
    }
    // An `e` that is not an event id names no item MDK can read.
    for tags in [
        vec![t(&["p", AGENT]), t(&["e"]), t(&["details", allow])],
        vec![t(&["p", AGENT]), t(&["e", "es"]), t(&["details", allow])],
        vec![
            t(&["p", AGENT]),
            t(&["e", &e1.to_uppercase()]),
            t(&["details", allow]),
        ],
    ] {
        let read = carrier::read_request(&agent, &tags);
        assert!(
            matches!(read, Some((None, Err(carrier::Unreadable::Invalid)))),
            "{tags:?} is read as {read:?}"
        );
        assert!(AgentRequest::from_tags(&tags).is_err());
    }
    // Naming the agent twice, or another agent too, names no one agent: it is not for this one.
    for tags in [
        vec![t(&["p", AGENT]), t(&["p", OTHER]), t(&["details", allow])],
        vec![t(&["p", OTHER]), t(&["p", AGENT]), t(&["details", allow])],
        vec![t(&["p", AGENT]), t(&["p"]), t(&["details", allow])],
    ] {
        assert_eq!(carrier::read_request(&agent, &tags), None, "{tags:?}");
        assert!(AgentRequest::from_tags(&tags).is_err());
    }
}
