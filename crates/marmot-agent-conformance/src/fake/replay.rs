//! A fake that replays a recording's native messages.
//!
//! A recording is the messages that went each way in one scripted session
//! (`fixtures/recordings/<harness>/*.jsonl`). The replay waits for the driver to write each
//! recorded outgoing message, matched by its kind (its method, or its Pi type, or a reply), and
//! answers it with the recorded incoming messages that followed, up to the next outgoing one. A
//! recorded reply is given the id of the driver's request it answers, since the driver numbers its
//! own requests. What the recording does not hold, such as the lists of models Codex's driver asks
//! for on a new chat, goes to a fallback fake.

use super::{Heard, Outbox, Protocol, is_reply, key};
use serde_json::Value;
use std::collections::HashMap;

/// Replays one recording, with a fallback for what it does not hold.
pub struct Replay<P> {
    lines: Vec<(bool, Value)>,
    at: usize,
    /// The driver's id for each recorded request id, as text.
    ids: HashMap<String, Value>,
    fallback: P,
}

impl<P: Protocol> Replay<P> {
    /// A replay of a recording's text. Lines the recorder wrote about itself are left out.
    pub fn new(recording: &str, fallback: P) -> Self {
        let lines = recording
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|r| r["msg"].get("_recorder").is_none())
            .map(|r| (r["dir"] == "out", r["msg"].clone()))
            .collect();
        Self {
            lines,
            at: 0,
            ids: HashMap::new(),
            fallback,
        }
    }

    /// Whether every recorded message has been played.
    pub fn done(&self) -> bool {
        self.at >= self.lines.len()
    }
}

impl<P: Protocol> Protocol for Replay<P> {
    fn hear(&mut self, frame: &Value, out: &mut Outbox, heard: &mut Vec<Heard>) {
        let next = (self.at..self.lines.len()).find(|&i| self.lines[i].0);
        let matched = next.filter(|&i| key(&self.lines[i].1) == key(frame));
        let Some(at) = matched else {
            self.fallback.hear(frame, out, heard);
            return;
        };
        let recorded = &self.lines[at].1;
        if !is_reply(recorded)
            && let (Some(theirs), Some(ours)) = (recorded.get("id"), frame.get("id"))
        {
            self.ids.insert(theirs.to_string(), ours.clone());
        }
        self.at = at + 1;
        while let Some((false, message)) = self.lines.get(self.at) {
            let mut message = message.clone();
            self.at += 1;
            if is_reply(&message) {
                let id = message["id"].to_string();
                if let Some(ours) = self.ids.get(&id) {
                    message["id"] = ours.clone();
                }
                out.reply(message);
            } else {
                out.say(message);
            }
        }
    }

    fn ended(&mut self) {
        self.fallback.ended();
    }
}
