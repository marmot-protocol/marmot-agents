//! Who may prompt and who may approve (D-010, D-032, D-039, D-047).

use crate::keys::PubKey;
use marmot_agent_driver::{ApprovalCommand, Position};
use marmot_agent_events::body;
use serde::{Deserialize, Serialize};

/// Who holds a right. The owner always does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Who {
    /// The owner alone, the default (D-010).
    #[default]
    Owner,
    /// The owner and these people.
    People(Vec<PubKey>),
    /// Everyone in the chat.
    Everyone,
}

/// The two rights the owner hands out (D-032).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rights {
    /// Who owns the agent.
    pub owner: PubKey,
    /// Who may prompt, use controls and answer questions (D-039).
    pub prompt: Who,
    /// Who may answer approvals and change what the agent may do without asking (D-032).
    pub approve: Who,
}

/// One of the two rights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Right {
    /// To prompt.
    Prompt,
    /// To approve.
    Approve,
}

impl Rights {
    /// Whether `person` holds `right`.
    pub fn allows(&self, person: &PubKey, right: Right) -> bool {
        if *person == self.owner {
            return true;
        }
        match self.who(right) {
            Who::Owner => false,
            Who::People(people) => people.contains(person),
            Who::Everyone => true,
        }
    }

    /// Whether the owner listed `person` by key for `right`: the only way another agent holds one
    /// (D-175).
    pub fn lists(&self, person: &PubKey, right: Right) -> bool {
        matches!(self.who(right), Who::People(people) if people.contains(person))
    }

    /// Who of `members`, a chat's, holds each right, as the chat's session item shows it (D-176):
    /// everyone, or the owner and the people listed, those of them in the chat alone, the owner
    /// first.
    pub fn shown(&self, members: &[PubKey]) -> body::Rights {
        let holders = |right| match self.who(right) {
            Who::Everyone => body::Holders::Everyone(body::Everyone::Everyone),
            Who::Owner => body::Holders::Members(self.present(&[], members)),
            Who::People(people) => body::Holders::Members(self.present(people, members)),
        };
        body::Rights {
            prompt: holders(Right::Prompt),
            approve: holders(Right::Approve),
        }
    }

    /// The owner and `people`, once each, in that order, those of them among `members`.
    fn present(&self, people: &[PubKey], members: &[PubKey]) -> Vec<String> {
        let mut present: Vec<String> = Vec::new();
        for person in std::iter::once(&self.owner).chain(people) {
            if members.contains(person) && !present.iter().any(|p| p == person.as_str()) {
                present.push(person.as_str().to_string());
            }
        }
        present
    }

    fn who(&self, right: Right) -> &Who {
        match right {
            Right::Prompt => &self.prompt,
            Right::Approve => &self.approve,
        }
    }
}

/// The right a message needs: to approve when it carries one of the harness's typed commands
/// that approve or widen what the agent may do, where that command counts (D-047), and to prompt
/// otherwise. The match reads a message as any harness may read it, and when the harnesses differ
/// it takes every reading, so it only ever asks for the right to approve more often (D-150):
///
/// - A harness reads a command in any case, so the match compares in each way one lowercases:
///   ASCII only, Unicode's full lowercase (Python's `str.lower`, JavaScript's `toLowerCase`), and
///   a case folding (Python's `str.casefold`, a JavaScript expression's `/iu`).
/// - A command that counts at the start counts after anything a harness skips as space (Python's
///   `str.strip`, JavaScript's `trim`, Rust's `trim`), and at the start of any line, by any
///   harness's line breaks, since a harness may drop lines before it, as Hermes drops lines naming
///   attachments.
/// - A command counts with a bot's name or punctuation after it (`/yolo@bot`, `/yolo.`), and ends
///   at anything that cannot continue a command's name; one whose name ends in such a mark
///   counts whatever follows it.
pub fn needed_for(text: &str, commands: &[ApprovalCommand]) -> Right {
    if commands.is_empty() {
        return Right::Prompt;
    }
    let found = readings(text).iter().enumerate().any(|(n, text)| {
        commands.iter().any(|c| {
            let command = &readings(&c.command)[n];
            match c.counts {
                Position::Start => text
                    .split(is_line_break)
                    .any(|line| is_word_at(line.trim_start_matches(is_space), 0, command)),
                Position::Anywhere => text
                    .match_indices(command.as_str())
                    .any(|(at, _)| is_word_at(text, at, command)),
            }
        })
    });
    if found { Right::Approve } else { Right::Prompt }
}

/// `text` lowercased in each way a harness may: ASCII only (Rust's `to_ascii_lowercase`),
/// Unicode's full lowercase, and folded. The fold is the lowercase of the uppercase of the
/// lowercase, which gives what Python's `str.casefold` gives wherever that is plain ASCII
/// (`ß` and `ẞ` as `ss`, `ſ` as `s`, ligatures as their letters), and folds a little more than
/// it (a dotless `ı` as `i`), which only asks more often.
fn readings(text: &str) -> [String; 3] {
    let lower = text.to_lowercase();
    let folded = lower.to_uppercase().to_lowercase();
    [text.to_ascii_lowercase(), lower, folded]
}

/// Whether a harness may skip `c` as space before a command: Rust's whitespace, and what Python's
/// `str.isspace` (the information separators) and JavaScript's `trim` (the byte order mark) add.
fn is_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}' | '\u{feff}')
}

/// Whether `c` ends a line for some harness: Python's `str.splitlines` breaks at each of these,
/// which take in JavaScript's and Rust's.
fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{b}'
            | '\u{c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// Whether `command` stands at `at` in `text` as a command of its own: what follows cannot
/// continue its name (a letter, a digit, `_` or `-`), so `/approve`, `/approve.` and
/// `/approve@bot` count and `/approved` does not. A name that ends in what cannot continue a name,
/// such as a shorthand of one mark, counts whatever follows it, as a harness runs the rest of the
/// line after it. Before it may be anything, since a command can follow punctuation.
fn is_word_at(text: &str, at: usize, command: &str) -> bool {
    let continues = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    text[at..].starts_with(command)
        && (command.chars().next_back().is_some_and(|c| !continues(c))
            || text[at + command.len()..]
                .chars()
                .next()
                .is_none_or(|c| !continues(c)))
}

/// Bounded proofs (`docs/design/verification.md`, "5. Bounded proofs with Kani"), run with
/// `cargo kani -p marmot-agent`. Keys are three, every one of them in every place: what the rights
/// check asks of a key is only whether it is another, and three keys show every such case. At most
/// 2 people are listed: every input of those sizes is checked.
#[cfg(kani)]
mod proofs {
    use super::*;

    /// One of three keys. Kani cannot reason in bounded memory about keys of symbolic text in a
    /// list: it ran out of memory with keys of up to 3 bytes.
    fn key() -> PubKey {
        PubKey::from(match kani::any::<u8>() % 3 {
            0 => "o",
            1 => "a",
            _ => "b",
        })
    }

    fn who() -> Who {
        match kani::any::<u8>() % 3 {
            0 => Who::Owner,
            1 => Who::Everyone,
            _ => {
                let mut people = vec![key()];
                if kani::any() {
                    people.push(key());
                }
                Who::People(people)
            }
        }
    }

    fn right() -> Right {
        if kani::any() {
            Right::Prompt
        } else {
            Right::Approve
        }
    }

    /// S6: the owner holds both rights, whatever the settings give.
    #[kani::proof]
    #[kani::unwind(5)]
    fn s6_the_owner_always_passes() {
        let rights = Rights {
            owner: key(),
            prompt: who(),
            approve: who(),
        };
        assert!(rights.allows(&rights.owner.clone(), right()));
    }

    /// S6: nobody else is the owner: someone who is not holds a right only where the settings
    /// give it them, by name or to everyone.
    #[kani::proof]
    #[kani::unwind(5)]
    fn s6_nobody_else_is_the_owner() {
        let rights = Rights {
            owner: key(),
            prompt: who(),
            approve: who(),
        };
        let other = key();
        kani::assume(other != rights.owner);
        let right = right();
        let given = match right {
            Right::Prompt => &rights.prompt,
            Right::Approve => &rights.approve,
        };
        let expected = match given {
            Who::Owner => false,
            Who::Everyone => true,
            Who::People(people) => people.contains(&other),
        };
        assert_eq!(rights.allows(&other, right), expected);
    }

    // Typed commands (D-047) have no proof here: Kani on `needed_for` over every text of 4 bytes
    // ran past half an hour and 7 GB, in its lowercasing and line splitting. The property tests
    // (`tests/properties/rights.rs`) own it.
}
