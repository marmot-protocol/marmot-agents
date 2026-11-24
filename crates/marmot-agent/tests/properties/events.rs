//! Agent events to start from: every event the drivers' expected files hold, for every harness and
//! every item type, each changed at random: strings in it made long, or of characters JSON
//! escapes or that take several bytes, and lists in it made long. So every field of every type
//! is reached without a generator written for each.

use marmot_agent_events::AgentEvent;
use proptest::prelude::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Every event in `fixtures/recordings/**/expected`, as JSON.
pub fn pool() -> &'static [Value] {
    static POOL: OnceLock<Vec<Value>> = OnceLock::new();
    POOL.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recordings");
        let mut files = Vec::new();
        expected_files(&root, &mut files);
        files.sort();
        let mut events = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("an expected file");
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                let value: Value = serde_json::from_str(line).expect("an expected event");
                if serde_json::from_value::<AgentEvent>(value.clone()).is_ok() {
                    events.push(value);
                }
            }
        }
        assert!(events.len() > 100, "the expected files hold the events");
        events
    })
}

fn expected_files(folder: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(folder).expect("a folder") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            expected_files(&path, files);
        } else if path.parent().and_then(Path::file_name) == Some("expected".as_ref())
            && path.extension().is_some_and(|e| e == "jsonl")
        {
            files.push(path);
        }
    }
}

/// A string to put in place of one in an event: one character, or a few, many times over.
#[derive(Clone, Debug)]
pub struct Pump {
    pub unit: &'static str,
    pub times: usize,
}

impl Pump {
    pub fn text(&self) -> String {
        self.unit.repeat(self.times)
    }
}

/// Characters a cut must not split, that JSON escapes, or that are plain.
const UNITS: [&str; 9] = ["a", "é", "€", "😀", "\"", "\\", "\n", "\u{1}", "ab\u{301}"];

pub fn pump() -> impl Strategy<Value = Pump> {
    let times = prop_oneof![
        3 => 0..200usize,
        2 => 200..20_000usize,
        2 => 20_000..90_000usize,
    ];
    (0..UNITS.len(), times).prop_map(|(unit, times)| Pump {
        unit: UNITS[unit],
        times,
    })
}

/// One change to an event: a string (by its place among the event's strings) replaced, or a list
/// (by its place among the event's lists) made longer by repeating its first entry.
#[derive(Clone, Debug)]
pub enum Change {
    Text(usize, Pump),
    Longer(usize, usize),
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        4 => (any::<usize>(), pump()).prop_map(|(at, pump)| Change::Text(at, pump)),
        1 => (any::<usize>(), 1..400usize).prop_map(|(at, n)| Change::Longer(at, n)),
    ]
}

/// An event from the pool, with a few changes made to it, as long as it still reads as an event
/// and is no larger than [`MOST`].
pub fn event() -> impl Strategy<Value = AgentEvent> {
    (0..pool().len(), prop::collection::vec(change(), 0..5)).prop_map(|(at, changes)| {
        let mut value = pool()[at].clone();
        for change in changes {
            let before = value.clone();
            apply(&mut value, &change);
            if size(&value) > MOST || serde_json::from_value::<AgentEvent>(value.clone()).is_err() {
                value = before;
            }
        }
        serde_json::from_value(value).expect("kept only when it reads")
    })
}

fn apply(value: &mut Value, change: &Change) {
    match change {
        Change::Text(at, pump) => {
            let mut strings = Vec::new();
            pointers(value, &mut String::new(), &mut strings, &|v| v.is_string());
            if strings.is_empty() {
                return;
            }
            let pointer = &strings[at % strings.len()];
            // The fields that name the item stay readable: a driver never sends them empty.
            let text = pump.text();
            let text = if text.is_empty() && is_id(pointer) {
                "x".into()
            } else {
                text
            };
            *value.pointer_mut(pointer).expect("found") = Value::String(text);
        }
        Change::Longer(at, n) => {
            let mut lists = Vec::new();
            pointers(value, &mut String::new(), &mut lists, &|v| {
                v.as_array().is_some_and(|a| !a.is_empty())
            });
            if lists.is_empty() {
                return;
            }
            let pointer = &lists[at % lists.len()];
            let room = MOST.saturating_sub(size(value));
            let list = value.pointer_mut(pointer).expect("found");
            let first = list[0].clone();
            // Only as many copies as fit, each with its comma: copying them first could take far
            // more memory than the runner has.
            let n = (*n).min(room / (size(&first) + 1));
            list.as_array_mut()
                .expect("a list")
                .extend(std::iter::repeat_n(first, n));
        }
    }
}

fn is_id(pointer: &str) -> bool {
    matches!(pointer, "/harness" | "/item" | "/session" | "/turn")
}

/// The JSON pointer of every value in `value` that `keep` keeps, outside a native message.
fn pointers(value: &Value, at: &mut String, out: &mut Vec<String>, keep: &dyn Fn(&Value) -> bool) {
    if keep(value) {
        out.push(at.clone());
    }
    let top = at.is_empty();
    let mut go = |name: String, item: &Value| {
        let length = at.len();
        at.push('/');
        at.push_str(&name.replace('~', "~0").replace('/', "~1"));
        pointers(item, at, out, keep);
        at.truncate(length);
    };
    match value {
        Value::Array(items) => {
            for (n, item) in items.iter().enumerate() {
                go(n.to_string(), item);
            }
        }
        Value::Object(map) => {
            for (name, item) in map {
                if top && name == "native" {
                    continue;
                }
                go(name.clone(), item);
            }
        }
        _ => {}
    }
}

/// An event id as MDK writes one: 64 lowercase hex digits.
pub fn hex64() -> impl Strategy<Value = String> {
    "[0-9a-f]{64}"
}

/// The most an event made here may take, written as JSON: room for many items over the 64 KB a
/// carrier takes, and not so much that the cases fill a runner's memory.
pub const MOST: usize = 2 << 20;

fn size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(0, |v| v.len())
}

/// No change makes an event larger than [`MOST`]. Repeating a list of long entries would: an
/// option of 360 KB repeated 400 times, in a question repeated 400 times, is 57 GB.
#[test]
fn no_change_makes_an_event_larger_than_the_most() {
    let mut value = serde_json::json!({"questions": [{"options": ["x"]}]});
    let pump = Pump {
        unit: "a",
        times: 20_000,
    };
    apply(&mut value, &Change::Text(0, pump));
    // The lists in order: the questions, then the first question's options.
    apply(&mut value, &Change::Longer(1, 399));
    assert!(size(&value) <= MOST, "{} bytes", size(&value));
    apply(&mut value, &Change::Longer(0, 399));
    assert!(size(&value) <= MOST, "{} bytes", size(&value));
}
