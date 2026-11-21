//! Which right a message needs, for the typed commands a harness lists (D-032, D-047).

use marmot_agent::rights::{Right, needed_for};
use marmot_agent_driver::{ApprovalCommand, Position};

// A command that counts at the start counts in any case, with a bot's name or punctuation after
// it, and at the start of any line, since a harness may drop lines before it (Hermes drops lines
// naming attachments); a longer word or a command later in a line does not.
#[test]
fn a_command_at_the_start_counts_in_every_form_a_harness_reads() {
    let commands = [ApprovalCommand {
        command: "/yolo".into(),
        counts: Position::Start,
    }];
    for text in [
        "/yolo",
        "  /YOLO on",
        "/Yolo@agent_bot",
        "/yolo.",
        "[file: notes.txt]\n/yolo",
    ] {
        assert_eq!(needed_for(text, &commands), Right::Approve, "{text:?}");
    }
    for text in ["/yolomode", "what does /yolo do?", "hello"] {
        assert_eq!(needed_for(text, &commands), Right::Prompt, "{text:?}");
    }
    assert_eq!(needed_for("/yolo", &[]), Right::Prompt);
}
