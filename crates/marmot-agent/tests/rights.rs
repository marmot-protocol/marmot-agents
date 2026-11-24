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

// A command counts as a harness reads it, and when the harnesses read it differently, as any of
// them does (D-150): after anything Python or JavaScript skips as space where Rust does not, and
// in any case as Unicode lowercases or folds it, so the core asks for the right to approve at
// least as often as the harness would run the command.
#[test]
fn a_command_counts_as_any_harness_reads_it() {
    let start = |command: &str| ApprovalCommand {
        command: command.into(),
        counts: Position::Start,
    };
    let commands = [start("/approve"), start("/skills"), start("/bash")];
    for text in [
        // Python's `str.lstrip` skips the information separators; JavaScript's `trim` the byte
        // order mark.
        "\u{1c}/approve",
        "\u{1d}/approve",
        "\u{1e}/approve",
        "\u{1f}/approve",
        "\u{feff}/approve",
        "\u{1f}\u{feff} /approve",
        // Python's and JavaScript's lowercase turn the Kelvin sign into a k.
        "/s\u{212a}ills",
        "/S\u{212a}ILLS on",
        // A case folding turns the long s into an s, and the ligature into its letters.
        "/\u{17f}kills",
        "/ba\u{17f}h ls",
        // A line any harness breaks at.
        "hello\u{2028}/approve",
        "hello\u{1e}/approve",
    ] {
        assert_eq!(needed_for(text, &commands), Right::Approve, "{text:?}");
    }
    let anywhere = [ApprovalCommand {
        command: "/fiddle".into(),
        counts: Position::Anywhere,
    }];
    assert_eq!(needed_for("ok /\u{fb01}ddle", &anywhere), Right::Approve);
    let sharp = [start("/class")];
    for text in ["/cla\u{df}", "/cla\u{1e9e}"] {
        assert_eq!(needed_for(text, &sharp), Right::Approve, "{text:?}");
    }
    for text in [
        "/approved",
        "say\u{a0}/approve",
        "/skills_x",
        "\u{200b}x/approve",
    ] {
        assert_eq!(needed_for(text, &commands), Right::Prompt, "{text:?}");
    }
}

// A command whose name ends in what cannot continue a name, such as a shorthand of one mark,
// counts whatever follows it, as a harness runs the rest of the line after it; a name that ends
// in a letter still needs its end.
#[test]
fn a_command_that_ends_in_a_mark_counts_whatever_follows() {
    let commands = [
        ApprovalCommand {
            command: "!".into(),
            counts: Position::Start,
        },
        ApprovalCommand {
            command: "/bash".into(),
            counts: Position::Start,
        },
    ];
    for text in [
        "!",
        "!ls",
        " !ls -la",
        "\u{3000}!config",
        "! ls",
        "hi\n!pwd",
    ] {
        assert_eq!(needed_for(text, &commands), Right::Approve, "{text:?}");
    }
    for text in ["/bashful", "hello!", "say !ls"] {
        assert_eq!(needed_for(text, &commands), Right::Prompt, "{text:?}");
    }
}
