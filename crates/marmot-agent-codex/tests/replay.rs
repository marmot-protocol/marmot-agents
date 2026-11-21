//! Every Codex recording, replayed through the translator, must give the events the prototype
//! gives for it (`fixtures/recordings/codex/expected`, written by
//! `tools/prototype/translate.py --expected`). The two were written apart, from the same design;
//! where they disagree, the recording and the event model decide which is wrong.

use marmot_agent_codex::{Direction, Translator};
use marmot_agent_events::AgentEvent;
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::path::{Path, PathBuf};

/// One line of a recording: which way the message went, and the message as the exact text it was.
#[derive(Deserialize)]
struct Line<'a> {
    dir: String,
    #[serde(borrow)]
    msg: &'a RawValue,
}

/// What the recorder wrote between messages, such as that it restarted Codex.
#[derive(Deserialize)]
struct Marker {
    #[serde(rename = "_recorder")]
    recorder: Option<String>,
}

fn recordings() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recordings/codex")
}

fn files(folder: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)
        .expect("the recordings folder")
        .map(|entry| entry.expect("a file").path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    paths.sort();
    paths
}

/// The events the translator gives for one recording.
fn replay(path: &Path) -> Vec<AgentEvent> {
    let text = std::fs::read_to_string(path).expect("a recording");
    let mut translator = Translator::new();
    let mut events = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let line: Line<'_> = serde_json::from_str(line).expect("a recorded line");
        let message = line.msg.get();
        if let Ok(Marker {
            recorder: Some(note),
        }) = serde_json::from_str(message)
        {
            if note.contains("new one started") {
                events.extend(translator.harness_stopped());
            }
            continue;
        }
        let direction = match line.dir.as_str() {
            "in" => Direction::In,
            "out" => Direction::Out,
            other => panic!("{}: a line going {other}", path.display()),
        };
        let made = translator.feed(direction, message);
        for event in &made {
            if let Some(native) = event.native() {
                assert!(
                    message.contains(native.payload.get()),
                    "{}: the native message of item {} is not text that arrived",
                    path.display(),
                    event.header().item.as_str()
                );
            }
        }
        events.extend(made);
    }
    assert_eq!(translator.unreadable(), 0, "{}", path.display());
    // Each recording is one chat: every message is for the session it follows.
    assert_eq!(translator.other_threads(), 0, "{}", path.display());
    assert!(
        translator.unknown().is_empty(),
        "{}: no rule for {:?}",
        path.display(),
        translator.unknown()
    );
    events
}

fn expected(path: &Path) -> Vec<Value> {
    let file = path
        .parent()
        .expect("a folder")
        .join("expected")
        .join(path.file_name().expect("a name"));
    std::fs::read_to_string(&file)
        .unwrap_or_else(|_| panic!("{} is missing", file.display()))
        .lines()
        .map(|line| serde_json::from_str(line).expect("an expected event"))
        .collect()
}

#[test]
fn every_recording_gives_the_prototypes_events() {
    let paths = files(&recordings());
    assert_eq!(paths.len(), 29, "the Codex recordings");
    let mut differ = Vec::new();
    for path in &paths {
        let made: Vec<Value> = replay(path)
            .iter()
            .map(|e| serde_json::to_value(e).expect("an event as JSON"))
            .collect();
        let want = expected(path);
        let name = path.file_stem().expect("a name").to_string_lossy();
        if let Some(at) = (0..made.len().max(want.len())).find(|&n| made.get(n) != want.get(n)) {
            differ.push(format!(
                "{name}, event {at} of {} (expected {}):\n  driver:    {}\n  prototype: {}",
                made.len(),
                want.len(),
                made.get(at).map_or("none".into(), Value::to_string),
                want.get(at).map_or("none".into(), Value::to_string),
            ));
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}

#[test]
fn the_expected_events_are_what_the_types_write() {
    for path in files(&recordings()) {
        for (at, want) in expected(&path).into_iter().enumerate() {
            let event: AgentEvent = serde_json::from_value(want.clone())
                .unwrap_or_else(|e| panic!("{}, event {at}: {e}", path.display()));
            let back = serde_json::to_value(&event).expect("an event as JSON");
            assert_eq!(back, want, "{}, event {at}", path.display());
        }
    }
}
