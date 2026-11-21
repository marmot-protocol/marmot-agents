//! Every ACP recording, replayed through the translator, must give the events the prototype gives
//! for it (`expected/` beside the recordings, written by `tools/prototype/translate.py --expected`).
//! The two were written apart, from the same design; where they disagree, the recording and the
//! event model decide which is wrong.

use marmot_agent_acp::{Direction, Translator};
use marmot_agent_events::{AgentEvent, Body, State};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One line of a recording: which way the message went, and the message as the exact text it was.
#[derive(Deserialize)]
struct Line<'a> {
    dir: String,
    #[serde(borrow)]
    msg: &'a RawValue,
}

/// What the recorder wrote between messages, such as that it restarted the agent.
#[derive(Deserialize)]
struct Marker {
    #[serde(rename = "_recorder")]
    recorder: Option<String>,
}

/// Each agent recorded over ACP, and how many recordings it has.
const AGENTS: &[(&str, usize)] = &[("opencode", 12), ("goose", 12), ("cursor", 11)];

fn folder(agent: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/recordings/acp")
        .join(agent)
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
    // The driver's name for the agent is its folder's: Cursor never names itself.
    let agent = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or("acp");
    let mut translator = Translator::named(agent);
    let mut events = Vec::new();
    let mut arrived = String::new(); // every message so far, to find each native payload in
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
        arrived.push_str(message);
        arrived.push('\n');
        let made = translator.feed(direction, message);
        for event in &made {
            let Some(native) = event.native() else {
                continue;
            };
            // A streamed message's native message is the list of its chunks' payloads.
            let parts: Vec<&RawValue> = if native.kind.ends_with("_chunk]") {
                serde_json::from_str(native.payload.get()).expect("a list of chunks")
            } else {
                vec![&*native.payload]
            };
            for part in parts {
                assert!(
                    arrived.contains(part.get()),
                    "{}: the native message of item {} is not text that arrived",
                    path.display(),
                    event.header().item.as_str()
                );
            }
        }
        events.extend(made);
    }
    assert_eq!(translator.unreadable(), 0, "{}", path.display());
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
    let mut differ = Vec::new();
    for (agent, count) in AGENTS {
        let paths = files(&folder(agent));
        assert_eq!(paths.len(), *count, "the {agent} recordings");
        for path in &paths {
            let made: Vec<Value> = replay(path)
                .iter()
                .map(|e| serde_json::to_value(e).expect("an event as JSON"))
                .collect();
            let want = expected(path);
            let name = path.file_stem().expect("a name").to_string_lossy();
            if let Some(at) = (0..made.len().max(want.len())).find(|&n| made.get(n) != want.get(n))
            {
                differ.push(format!(
                    "{agent}/{name}, event {at} of {} (expected {}):\n  driver:    {}\n  prototype: {}",
                    made.len(),
                    want.len(),
                    made.get(at).map_or("none".into(), Value::to_string),
                    want.get(at).map_or("none".into(), Value::to_string),
                ));
            }
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}

#[test]
fn the_expected_events_are_what_the_types_write() {
    for (agent, _) in AGENTS {
        for path in files(&folder(agent)) {
            for (at, want) in expected(&path).into_iter().enumerate() {
                let event: AgentEvent = serde_json::from_value(want.clone())
                    .unwrap_or_else(|e| panic!("{}, event {at}: {e}", path.display()));
                let back = serde_json::to_value(&event).expect("an event as JSON");
                assert_eq!(back, want, "{}, event {at}", path.display());
            }
        }
    }
}

/// OpenCode's question tool and the form it asks with are one question item, and no tool item
/// (rule 4 of the event model). Its free-text field is the question's own answer, not a second
/// question.
#[test]
fn a_question_is_one_item() {
    let events = replay(&folder("opencode").join("question.jsonl"));
    let questions: Vec<&AgentEvent> = events
        .iter()
        .filter(|e| matches!(e.body(), Body::Question(_)))
        .collect();
    let items: HashSet<&str> = questions.iter().map(|e| e.header().item.as_str()).collect();
    assert_eq!(items.len(), 1, "one question item");
    let Body::Question(q) = questions[0].body() else {
        unreachable!()
    };
    assert_eq!(q.questions().len(), 1, "one question");
    assert!(q.questions()[0].allow_text, "with a free-text answer");
    assert!(
        !events.iter().any(|e| matches!(e.body(), Body::Tool(_))),
        "the question tool is not a tool item"
    );
}

/// A command that exits with an error is failed, with its exit code, though ACP calls it completed.
#[test]
fn a_failing_command_is_failed_with_its_exit_code() {
    let events = replay(&folder("opencode").join("failing_command.jsonl"));
    let last = events
        .iter()
        .rev()
        .find_map(|e| match e.body() {
            Body::Tool(t) => Some((e.header().state, t.exit_code)),
            _ => None,
        })
        .expect("a tool item");
    assert_eq!(last, (State::Failed, Some(1)));
}
