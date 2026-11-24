//! Every built-in driver's translator over its recordings changed at random
//! (`docs/design/verification.md`, "6. Property tests"): messages repeated, dropped, swapped with
//! the next, stripped of their `params` and `result`, and the harness dying after any of them
//! (nothing of a harness that died reaches the translator after; the recordings' own restarts
//! stay). After each, with the harness stopped at the end:
//!
//! - D-007: every native message arrived, unchanged: the message the translator was reading, or
//!   the whole value of one of its fields (a streamed one, a list of such values of the chunks that
//!   arrived), as `marmot_agent_conformance::native` checks.
//! - SEQ: `seq` rises within a session (D-059, point 4).
//! - S2: every item of the session's turns is closed. A session's own item outlives its harness.
//! - Nothing reopens an item once it ended: no later event of it is running or waiting.
//!
//! The last two are checked when messages were only repeated or the harness died: a harness writes
//! its messages in order on one stream, which loses none, so a message dropped, an item's end
//! swapped before its start, or a message without the field that carries what it says, is not a
//! case a driver meets. Those are checked for the first two,
//! and for not failing: a translator reads whatever arrives.
//!
//! The case count comes from `PROPTEST_CASES`; a failing case is kept in
//! `translators.proptest-regressions` beside this file.

use marmot_agent_conformance::native::Arrived;
use marmot_agent_events::{AgentEvent, Body, State};
use proptest::prelude::*;
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Harness {
    Codex,
    Acp(&'static str),
    Pi,
}

/// One line of a recording: a message and which way it went, or a mark the recorder left.
#[derive(Clone, Debug)]
enum Line {
    Message { into: bool, text: String },
    Restarted,
    PiVersion(Option<String>),
}

struct Recording {
    harness: Harness,
    name: String,
    lines: Vec<Line>,
}

fn recordings() -> &'static [Recording] {
    static ALL: OnceLock<Vec<Recording>> = OnceLock::new();
    ALL.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recordings");
        let mut all = Vec::new();
        for (harness, folder) in [
            (Harness::Codex, root.join("codex")),
            (Harness::Acp("opencode"), root.join("acp/opencode")),
            (Harness::Acp("goose"), root.join("acp/goose")),
            (Harness::Acp("cursor"), root.join("acp/cursor")),
            (Harness::Pi, root.join("pi")),
        ] {
            for path in files(&folder) {
                all.push(read(harness, &path));
            }
        }
        assert!(all.len() > 50, "the recordings are there");
        all
    })
}

fn files(folder: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)
        .expect("a recordings folder")
        .map(|entry| entry.expect("a file").path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    paths.sort();
    paths
}

fn read(harness: Harness, path: &Path) -> Recording {
    #[derive(serde::Deserialize)]
    struct Recorded<'a> {
        dir: String,
        #[serde(borrow)]
        msg: &'a RawValue,
    }
    let text = std::fs::read_to_string(path).expect("a recording");
    let mut lines = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let recorded: Recorded<'_> = serde_json::from_str(line).expect("a recorded line");
        let message = recorded.msg.get();
        let value: serde_json::Value = serde_json::from_str(message).expect("JSON");
        if let Some(note) = value.get("_recorder").and_then(|n| n.as_str()) {
            if note == "pi --version" {
                let version = value.get("version").and_then(|v| v.as_str());
                lines.push(Line::PiVersion(version.map(str::to_string)));
            } else if note.contains("new one started") {
                lines.push(Line::Restarted);
            }
            continue;
        }
        lines.push(Line::Message {
            into: recorded.dir == "in",
            text: message.to_string(),
        });
    }
    Recording {
        harness,
        name: path.file_stem().expect("a name").to_string_lossy().into(),
        lines,
    }
}

/// A change to a recording, at a place given as any number (taken modulo its length).
#[derive(Clone, Debug)]
enum Change {
    Repeat(usize),
    Drop(usize),
    SwapWithNext(usize),
    DiesAt(usize),
    /// A message without its `params` or `result`: not one a harness or the driver writes, but
    /// one a translator must read without making up the field.
    Bare(usize),
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        any::<usize>().prop_map(Change::Repeat),
        any::<usize>().prop_map(Change::Drop),
        any::<usize>().prop_map(Change::SwapWithNext),
        any::<usize>().prop_map(Change::DiesAt),
        any::<usize>().prop_map(Change::Bare),
    ]
}

fn changed(lines: &[Line], changes: &[Change]) -> Vec<Line> {
    let mut lines = lines.to_vec();
    for change in changes {
        if lines.is_empty() {
            break;
        }
        let n = lines.len();
        match *change {
            Change::Repeat(at) => {
                let line = lines[at % n].clone();
                lines.insert(at % n, line);
            }
            Change::Drop(at) => drop(lines.remove(at % n)),
            Change::SwapWithNext(at) if n > 1 => lines.swap(at % (n - 1), at % (n - 1) + 1),
            Change::SwapWithNext(_) => {}
            Change::DiesAt(at) => lines.truncate(at % (n + 1)),
            Change::Bare(at) => {
                if let Line::Message { text, .. } = &mut lines[at % n]
                    && let Ok(serde_json::Value::Object(mut message)) = serde_json::from_str(text)
                {
                    message.remove("params");
                    message.remove("result");
                    *text = serde_json::Value::Object(message).to_string();
                }
            }
        }
    }
    lines
}

/// The three translators, fed alike.
enum Translator {
    Codex(Box<marmot_agent_codex::Translator>),
    Acp(Box<marmot_agent_acp::Translator>),
    Pi(Box<marmot_agent_pi::Translator>),
}

impl Translator {
    fn new(harness: Harness) -> Self {
        match harness {
            Harness::Codex => Self::Codex(Box::new(marmot_agent_codex::Translator::new())),
            Harness::Acp(name) => Self::Acp(Box::new(marmot_agent_acp::Translator::named(name))),
            Harness::Pi => Self::Pi(Box::new(marmot_agent_pi::Translator::new())),
        }
    }

    fn feed(&mut self, into: bool, text: &str) -> Vec<AgentEvent> {
        match self {
            Self::Codex(t) => t.feed(
                if into {
                    marmot_agent_codex::Direction::In
                } else {
                    marmot_agent_codex::Direction::Out
                },
                text,
            ),
            Self::Acp(t) => t.feed(
                if into {
                    marmot_agent_acp::Direction::In
                } else {
                    marmot_agent_acp::Direction::Out
                },
                text,
            ),
            Self::Pi(t) => t.feed(
                if into {
                    marmot_agent_pi::Direction::In
                } else {
                    marmot_agent_pi::Direction::Out
                },
                text,
            ),
        }
    }

    fn harness_stopped(&mut self) -> Vec<AgentEvent> {
        match self {
            Self::Codex(t) => t.harness_stopped(),
            Self::Acp(t) => t.harness_stopped(),
            Self::Pi(t) => t.harness_stopped(),
        }
    }
}

fn is_open(state: State) -> bool {
    matches!(state, State::Running | State::Waiting)
}

/// What the checks keep of the events so far.
#[derive(Default)]
struct Checked {
    /// Whether every item must close, and none reopen once it ended.
    strict: bool,
    arrived: Arrived,
    /// Each item's last state, by its id; a session's own item apart.
    items: BTreeMap<String, (State, bool)>,
    seq: BTreeMap<String, u64>,
}

impl Checked {
    fn event(&mut self, event: &AgentEvent, stopped: bool) -> Result<(), String> {
        let header = event.header();
        let id = header.item.as_str().to_string();
        let session_item = matches!(event.body(), Body::Session(_));
        self.arrived.check(event, stopped)?;
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        let last = self.seq.entry(session.unwrap_or_default()).or_insert(0);
        if header.seq <= *last {
            return Err(format!(
                "SEQ: item {id} has seq {} after {}",
                header.seq, *last
            ));
        }
        *last = header.seq;
        if let Some((before, _)) = self.items.get(&id)
            && self.strict
            && !session_item
            && !is_open(*before)
            && is_open(header.state)
        {
            return Err(format!(
                "item {id} is {:?} after it ended {:?}",
                header.state, before
            ));
        }
        self.items.insert(id, (header.state, session_item));
        Ok(())
    }

    fn all_closed(&self) -> Result<(), String> {
        for (id, (state, session_item)) in &self.items {
            if is_open(*state) && !session_item {
                return Err(format!(
                    "S2: item {id} is left {state:?} after the harness stopped"
                ));
            }
        }
        Ok(())
    }
}

fn run(recording: &Recording, lines: &[Line], strict: bool) -> Result<(), String> {
    let mut translator = Translator::new(recording.harness);
    let mut checked = Checked {
        strict,
        ..Checked::default()
    };
    for line in lines {
        let (events, stopped) = match line {
            Line::Message { into, text } => {
                checked.arrived.push(text);
                (translator.feed(*into, text), false)
            }
            Line::Restarted => (translator.harness_stopped(), true),
            Line::PiVersion(version) => {
                if let Translator::Pi(t) = &mut translator {
                    t.set_version(version.clone());
                }
                continue;
            }
        };
        for event in &events {
            checked.event(event, stopped)?;
        }
    }
    for event in &translator.harness_stopped() {
        checked.event(event, true)?;
    }
    if strict { checked.all_closed() } else { Ok(()) }
}

#[test]
fn every_recording_as_recorded_keeps_the_properties() {
    let mut failed = Vec::new();
    for recording in recordings() {
        if let Err(why) = run(recording, &recording.lines, true) {
            failed.push(format!("{:?} {}: {why}", recording.harness, recording.name));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

// A reply with no `result` is not one Codex writes, nor one the driver does. Read as if its
// result were `{}`, a list arriving once the session had started made a session event whose
// native message, `{}`, never arrived (D-007), though `{}` had, as the params of two requests;
// and an answer to an approval request made the approval's.
#[test]
fn a_codex_reply_with_no_result_makes_up_no_native_message() {
    let message = |into: bool, text: &str| Line::Message {
        into,
        text: text.to_string(),
    };
    let list = Recording {
        harness: Harness::Codex,
        name: "a list with no result".into(),
        lines: vec![
            message(false, r#"{"id":1,"method":"thread/start","params":{}}"#),
            message(true, r#"{"id":1,"result":{"thread":{"id":"th1"}}}"#),
            message(false, r#"{"id":2,"method":"model/list","params":{}}"#),
            message(true, r#"{"id":2}"#),
        ],
    };
    assert_eq!(run(&list, &list.lines, false), Ok(()));
    let recorded = (recordings().iter())
        .find(|r| r.harness == Harness::Codex && r.name == "approval_allowed")
        .expect("a recording of an approval");
    let asked = (recorded.lines.iter())
        .position(|l| matches!(l, Line::Message { text, .. } if text.contains("requestApproval")))
        .expect("the approval request");
    let mut lines = recorded.lines[..=asked].to_vec();
    lines.push(message(false, r#"{"id":0}"#));
    assert_eq!(run(recorded, &lines, false), Ok(()));
}

proptest! {
    #[test]
    fn every_recording_changed_keeps_the_properties(
        at in 0..recordings().len(),
        changes in prop::collection::vec(change(), 1..6),
    ) {
        let recording = &recordings()[at];
        let lines = changed(&recording.lines, &changes);
        let strict = (changes.iter()).all(|c| matches!(c, Change::Repeat(_) | Change::DiesAt(_)));
        if let Err(why) = run(recording, &lines, strict) {
            prop_assert!(false, "{:?} {}: {}", recording.harness, recording.name, why);
        }
    }
}
