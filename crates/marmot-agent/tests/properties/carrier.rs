//! S7 and the carrier (D-026, D-057, D-076): every version of an item the core writes is at most
//! 64 KB, is one MDK reads, and reads back as the item sent, whole when nothing had to be cut.

use crate::events::{self, hex64};
use cgka_traits::agent_events::{AgentItemState, validate_agent_item_tags};
use marmot_agent::carrier::{
    self, EDIT_KIND, ITEM_KIND, Item, LIMIT, LeftOut, SETTLES_MAX, Thread,
};
use marmot_agent::{EventKey, PubKey};
use marmot_agent_events::{AgentEvent, State};
use proptest::prelude::*;

fn item() -> impl Strategy<Value = Item> {
    (
        events::event(),
        prop_oneof![1..4u64, 1..u64::from(u32::MAX)],
        prop::option::of(hex64()),
        prop::option::of(hex64()),
        prop_oneof![
            4 => prop::collection::vec(hex64(), 0..4),
            1 => prop::collection::vec(hex64(), 0..=SETTLES_MAX),
        ],
        // A subagent's item carries NIP-22's thread tags, which count against the limit too (D-121).
        prop::option::of((hex64(), prop::option::of(hex64()), hex64())),
    )
        .prop_map(
            |(event, version, first, started_by, settles, thread)| Item {
                event,
                version,
                first: first.map(|f| EventKey::from(f.as_str())),
                started_by: started_by.map(|s| EventKey::from(s.as_str())),
                settles: settles.iter().map(|s| EventKey::from(s.as_str())).collect(),
                thread: thread.map(|(root, parent, agent)| Thread {
                    root: EventKey::from(root.as_str()),
                    parent: parent.map(|p| EventKey::from(p.as_str())),
                    agent: PubKey::from(agent.as_str()),
                }),
            },
        )
}

/// The ids the core never cuts, which a stand-in keeps: the harness, the item, its session and
/// turn. When they alone come near the limit, nothing can be sent.
fn ids_size(event: &AgentEvent) -> usize {
    let h = event.header();
    let escaped = |s: &str| serde_json::to_string(s).map_or(0, |t| t.len());
    escaped(&h.harness)
        + escaped(h.item.as_str())
        + h.session.as_ref().map_or(0, |s| escaped(s.as_str()))
        + h.turn.as_ref().map_or(0, |t| escaped(t.as_str()))
}

fn mdk_state(state: State) -> AgentItemState {
    match state {
        State::Running => AgentItemState::Running,
        State::Waiting => AgentItemState::Waiting,
        State::Done => AgentItemState::Done,
        State::Failed => AgentItemState::Failed,
        State::Declined => AgentItemState::Declined,
        State::Cancelled => AgentItemState::Cancelled,
    }
}

proptest! {
    /// S7: what the core writes of an item fits, MDK reads it as the same item at the same
    /// version, and the core reads it back with every id and what it settles; with nothing cut,
    /// it reads back whole, native message and all. Only ids too large to fit alone stop it.
    #[test]
    fn s7_an_item_fits_and_reads_back(item in item()) {
        let written = carrier::item(&item);
        let Ok(out) = written else {
            prop_assert!(
                ids_size(&item.event) * 2 + item.settles.len() * 80 > LIMIT / 2,
                "only ids too large for the limit stop an item"
            );
            return Ok(());
        };
        prop_assert!(out.size() <= LIMIT, "{} bytes", out.size());
        prop_assert_eq!(out.kind, if item.first.is_some() { EDIT_KIND } else { ITEM_KIND });
        prop_assert_eq!(out.edits(), item.first.as_ref().map(EventKey::as_str));

        // MDK takes it, and reads what the core meant.
        let mdk = validate_agent_item_tags(&out.tags_but_target());
        prop_assert!(mdk.is_ok(), "MDK refuses it: {:?}", mdk.err());
        let mdk = mdk.unwrap();
        let header = item.event.header();
        prop_assert_eq!(mdk.version, item.version);
        prop_assert_eq!(mdk.state, mdk_state(header.state));
        prop_assert_eq!(&mdk.item_id, header.item.as_str());
        prop_assert_eq!(mdk.seq, header.seq);
        let settles: Vec<&str> = item.settles.iter().map(EventKey::as_str).collect();
        prop_assert_eq!(&mdk.settles, &settles);
        prop_assert_eq!(
            mdk.started_by_message_id_hex.as_deref(),
            item.started_by.as_ref().map(EventKey::as_str)
        );

        // The core reads back what it wrote.
        let read = carrier::read_item(out.kind, &out.content, &out.tags);
        prop_assert!(read.is_some(), "the core cannot read what it wrote");
        let read = read.unwrap();
        prop_assert_eq!(read.item.version, item.version);
        prop_assert_eq!(&read.item.first, &item.first);
        prop_assert_eq!(&read.item.started_by, &item.started_by);
        prop_assert_eq!(&read.item.settles, &item.settles);
        let back = read.item.event.header();
        prop_assert_eq!(&back.harness, &header.harness);
        prop_assert_eq!(&back.item, &header.item);
        prop_assert_eq!(&back.session, &header.session);
        prop_assert_eq!(&back.turn, &header.turn);
        prop_assert_eq!(back.seq, header.seq);
        prop_assert_eq!(back.state, header.state);
        if read.left_out == LeftOut::default() {
            // An edit repeats only the thread's root, not its parent (D-133); a first event with no
            // parent of its own names the root as its parent, and reads back so.
            let mut expected = item.clone();
            if let Some(thread) = expected.thread.as_mut() {
                thread.parent = match expected.first {
                    Some(_) => None,
                    None => Some(thread.parent.clone().unwrap_or_else(|| thread.root.clone())),
                };
            }
            prop_assert!(read.item == expected, "an item nothing was cut of reads back whole");
        } else {
            prop_assert!(out.size() > 0);
            prop_assert!(
                read.item.event.native().is_none(),
                "a cut item never carries a native message"
            );
        }
    }

    /// S7 for the agent's words: what is left fits with its tags, and is the words' start and end
    /// around a marker, or the words whole.
    #[test]
    fn s7_words_fit(pump in events::pump(), tail in events::pump(), settles in prop::collection::vec(hex64(), 0..40)) {
        let text = format!("{}{}", pump.text(), tail.text());
        let tags = carrier::words_tags(None, false, &settles.iter().map(|s| EventKey::from(s.as_str())).collect::<Vec<_>>());
        let Ok(fitted) = carrier::fit_words(&text, &tags) else {
            prop_assert!(false, "words with {} tags did not fit", tags.len());
            return Ok(());
        };
        if fitted != text {
            let at = fitted.find("\n… cut: ");
            prop_assert!(at.is_some(), "a cut is marked");
            let at = at.unwrap();
            let end = fitted[at + 1..].find(" …\n").map(|e| at + 1 + e + " …\n".len());
            prop_assert!(end.is_some());
            let (head, tail) = (&fitted[..at], &fitted[end.unwrap()..]);
            prop_assert!(text.starts_with(head) && text.ends_with(tail));
            prop_assert!(head.len() + tail.len() < text.len());
        }
        let size = carrier::Outgoing { kind: 9, content: fitted, tags }.size();
        prop_assert!(size <= LIMIT, "{} bytes", size);
    }
}
