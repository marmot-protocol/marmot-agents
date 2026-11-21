//! Every event the prototype expects of a driver (`fixtures/recordings/*/expected`, written by
//! `tools/prototype/translate.py --expected`) must be one these types read. A driver on the socket
//! is tested against those files in its own language; this is what tells it that the core will
//! take what it sends.

use marmot_agent_events::AgentEvent;
use std::path::Path;

#[test]
fn every_expected_event_is_one_the_core_reads() {
    let recordings = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recordings");
    let mut read = 0;
    for harness in std::fs::read_dir(&recordings).expect("the recordings folder") {
        let expected = harness.expect("a folder").path().join("expected");
        let Ok(files) = std::fs::read_dir(&expected) else {
            continue;
        };
        for file in files {
            let path = file.expect("a file").path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("an expected file");
            for (n, line) in text
                .lines()
                .enumerate()
                .filter(|(_, l)| !l.trim().is_empty())
            {
                if let Err(e) = serde_json::from_str::<AgentEvent>(line) {
                    panic!("{}:{}: {e}", path.display(), n + 1);
                }
                read += 1;
            }
        }
    }
    assert!(read > 0, "no expected events found");
}
