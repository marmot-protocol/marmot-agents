//! S6: who may prompt and approve (D-010, D-032), the right a typed command needs (D-047), and the
//! keys the settings name them by.

use marmot_agent::keys::public_key_hex;
use marmot_agent::rights::{Right, needed_for};
use marmot_agent::{PubKey, Rights, Who};
use marmot_agent_driver::{ApprovalCommand, Position};
use proptest::prelude::*;

fn key() -> impl Strategy<Value = PubKey> {
    "[0-9a-f]{64}".prop_map(|k| PubKey::from(k.as_str()))
}

fn who() -> impl Strategy<Value = Who> {
    prop_oneof![
        Just(Who::Owner),
        Just(Who::Everyone),
        prop::collection::vec(key(), 0..4).prop_map(Who::People),
    ]
}

fn right() -> impl Strategy<Value = Right> {
    prop_oneof![Just(Right::Prompt), Just(Right::Approve)]
}

/// Text around a command: words, spaces, new lines, punctuation, letters of other scripts.
fn filler() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just(" ".to_string()),
            Just("\n".to_string()),
            Just("\r\n".to_string()),
            Just("\t".to_string()),
            Just(".".to_string()),
            "[a-zA-Z]{1,5}",
            "\\PC{1,3}",
        ],
        0..6,
    )
    .prop_map(|parts| parts.concat())
}

/// A command as a person may type it: in any case.
fn typed(command: &str) -> impl Strategy<Value = String> {
    let command = command.to_string();
    prop::collection::vec(any::<bool>(), command.len()).prop_map(move |upper| {
        (command.chars().zip(upper))
            .map(|(c, up)| if up { c.to_ascii_uppercase() } else { c })
            .collect()
    })
}

/// What may follow a command that still makes it one: nothing, a space, punctuation, a bot's
/// name.
fn after() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "", " ", " now", ".", "!", "@bot", "\n", "\r\n", ",", "?",
    ])
}

fn position() -> impl Strategy<Value = Position> {
    prop_oneof![Just(Position::Start), Just(Position::Anywhere)]
}

const COMMAND: &str = "/yolo";

proptest! {
    /// The owner always holds both rights, whatever the settings say; nobody else holds one the
    /// settings do not give.
    #[test]
    fn s6_the_owner_always_passes_and_nobody_else_is_the_owner(
        owner in key(), other in key(), prompt in who(), approve in who(), right in right()
    ) {
        let rights = Rights { owner: owner.clone(), prompt, approve };
        prop_assert!(rights.allows(&owner, right));
        if other != owner {
            let who = match right {
                Right::Prompt => &rights.prompt,
                Right::Approve => &rights.approve,
            };
            let given = match who {
                Who::Owner => false,
                Who::Everyone => true,
                Who::People(people) => people.contains(&other),
            };
            prop_assert_eq!(rights.allows(&other, right), given);
        }
    }

    /// S6, D-047: a command at the start of any line needs the right to approve, typed in any case,
    /// after any space, and whatever follows it that cannot go on with its name; where it counts
    /// anywhere, after anything too.
    #[test]
    fn s6_a_command_at_the_start_of_a_line_needs_approval(
        before in filler(), lead in "[ \t]{0,3}", typed in typed(COMMAND), after in after(),
        rest in filler(), counts in position()
    ) {
        // Nothing after it but more text would make it a longer word.
        let rest = if after.is_empty() { String::new() } else { rest };
        let text = if before.is_empty() || before.ends_with('\n') {
            format!("{before}{lead}{typed}{after}{rest}")
        } else {
            format!("{before}\n{lead}{typed}{after}{rest}")
        };
        let commands = [ApprovalCommand { command: COMMAND.into(), counts }];
        prop_assert_eq!(needed_for(&text, &commands), Right::Approve, "{:?}", text);
    }

    /// Where a command counts anywhere, it needs approval after anything.
    #[test]
    fn s6_a_command_anywhere_needs_approval(
        before in filler(), typed in typed(COMMAND), after in after(), rest in filler()
    ) {
        let rest = if after.is_empty() { String::new() } else { rest };
        let text = format!("{before}{typed}{after}{rest}");
        let commands = [ApprovalCommand { command: COMMAND.into(), counts: Position::Anywhere }];
        prop_assert_eq!(needed_for(&text, &commands), Right::Approve, "{:?}", text);
    }

    /// Text that does not hold the command, in any case, needs only the right to prompt; and more
    /// lines after a command never take the need to approve away.
    #[test]
    fn s6_more_text_never_lowers_the_right(
        text in filler(), more in filler(), counts in position()
    ) {
        let commands = [ApprovalCommand { command: COMMAND.into(), counts }];
        let need = needed_for(&text, &commands);
        if !text.to_ascii_lowercase().contains(COMMAND) {
            prop_assert_eq!(need, Right::Prompt);
        }
        if need == Right::Approve {
            prop_assert_eq!(needed_for(&format!("{text}\n{more}"), &commands), Right::Approve);
        }
    }

    /// A key in the settings, in hex in either case, reads as the lowercase hex MDK uses; as an
    /// npub, as the key it encodes; anything else that is not one of those is refused, never read
    /// as some other key.
    #[test]
    fn s6_a_key_reads_as_itself(bytes in prop::array::uniform32(any::<u8>()), upper in any::<bool>(), junk in "\\PC{0,70}") {
        let hex = hex::encode(bytes);
        let typed = if upper { hex.to_ascii_uppercase() } else { hex.clone() };
        prop_assert_eq!(public_key_hex(&typed), Some(hex.clone()));
        let hrp = bech32::Hrp::parse("npub").expect("an hrp");
        let npub = bech32::encode::<bech32::Bech32>(hrp, &bytes).expect("encodes");
        prop_assert_eq!(public_key_hex(&npub), Some(hex.clone()));
        if let Some(read) = public_key_hex(&junk) {
            prop_assert_eq!(read.len(), 64);
            prop_assert!(read.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            prop_assert_eq!(&read, &junk.trim().to_ascii_lowercase());
        }
    }
}
