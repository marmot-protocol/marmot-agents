//! Every expected event a driver is tested against (each `expected` folder under
//! `fixtures/recordings`, written by `tools/prototype/translate.py --expected`) is one the item
//! types read, with nothing passed over: read and written again, it is the same JSON. A connector's
//! events reach the core as JSON, and the core passes over an event it cannot read, so an expected
//! event the types could not read would test a driver against events the chat never sees.

use marmot_agent_events::AgentEvent;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Every `expected` folder under `dir`, at any depth, as `acp/opencode/expected` is.
fn expected_folders(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("an entry").path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|n| n == "expected") {
            found.push(path);
        } else {
            expected_folders(&path, found);
        }
    }
}

#[test]
fn every_expected_event_is_read_whole() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recordings");
    let mut folders = Vec::new();
    expected_folders(&root, &mut folders);
    folders.sort();
    let mut read = 0;
    for folder in &folders {
        let mut paths: Vec<_> = std::fs::read_dir(folder)
            .expect("the expected events")
            .map(|entry| entry.expect("a file").path())
            .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        paths.sort();
        assert!(
            !paths.is_empty(),
            "no expected events in {}",
            folder.display()
        );
        for path in paths {
            let text = std::fs::read_to_string(&path).expect("an expected file");
            for (n, line) in text
                .lines()
                .enumerate()
                .filter(|(_, l)| !l.trim().is_empty())
            {
                let at = format!("{}:{}", path.display(), n + 1);
                let event: AgentEvent = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("{at}: the item types cannot read it: {e}"));
                let again: Value = serde_json::to_value(&event).expect("written again");
                let was: Value = serde_json::from_str(line).expect("JSON");
                // OpenClaw's carry fields the types do not have, which the core drops (issue #68).
                if !path.components().any(|c| c.as_os_str() == "openclaw") {
                    assert_eq!(again, was, "{at}: read and written again, it differs");
                }
                read += 1;
            }
        }
    }
    assert!(
        folders.len() >= 7,
        "expected events for every driven harness"
    );
    assert!(read > 0);
}
