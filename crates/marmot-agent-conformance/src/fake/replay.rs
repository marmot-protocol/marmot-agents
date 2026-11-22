//! A fake that replays a recording's native messages.
//!
//! A recording is the messages that went each way in one scripted session
//! (`fixtures/recordings/<harness>/*.jsonl`). The replay waits for the driver to write each
//! recorded outgoing message, matched by its kind (its method, or its Pi type, or a reply), and
//! answers it with the recorded incoming messages that followed, up to the next outgoing one. The
//! driver's answer to a request of the harness's own, such as an approval, must be the recorded
//! one: to the same request, with the same decision, or nothing more is played. A
//! recorded reply is given the id of the driver's request it answers, since the driver numbers its
//! own requests. What the recording does not hold, such as the lists of models Codex's driver asks
//! for on a new chat, goes to a fallback fake.

use super::{Heard, Outbox, Protocol, is_reply, key};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// How far a replay got, for a test to see: how many recorded messages are still to be played,
/// and how many answers it turned away as not the recorded one.
#[derive(Debug, Default)]
pub struct Progress {
    left: AtomicUsize,
    refused: AtomicUsize,
}

impl Progress {
    /// Recorded messages not yet played.
    pub fn left(&self) -> usize {
        self.left.load(Ordering::SeqCst)
    }

    /// Answers to the harness's own requests that were not the recorded one.
    pub fn refused(&self) -> usize {
        self.refused.load(Ordering::SeqCst)
    }
}

/// Replays one recording, with a fallback for what it does not hold.
pub struct Replay<P> {
    lines: Vec<(bool, Value)>,
    at: usize,
    /// The driver's id for each recorded request id, as text.
    ids: HashMap<String, Value>,
    fallback: P,
    /// How far it got, for a test to see that all was played.
    progress: Arc<Progress>,
}

impl<P: Protocol> Replay<P> {
    /// A replay of a recording's text. Lines the recorder wrote about itself are left out.
    pub fn new(recording: &str, fallback: P) -> Self {
        let lines: Vec<(bool, Value)> = recording
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|r| r["msg"].get("_recorder").is_none())
            .map(|r| (r["dir"] == "out", r["msg"].clone()))
            .collect();
        let progress = Arc::new(Progress {
            left: AtomicUsize::new(lines.len()),
            refused: AtomicUsize::new(0),
        });
        Self {
            lines,
            at: 0,
            ids: HashMap::new(),
            fallback,
            progress,
        }
    }

    /// Counts how far it got in `progress`, which a test keeps: a replay that played nothing
    /// passes nothing.
    pub fn counting(mut self, progress: Arc<Progress>) -> Self {
        progress.left.store(self.lines.len(), Ordering::SeqCst);
        self.progress = progress;
        self
    }
}

impl<P: Protocol> Protocol for Replay<P> {
    fn hear(&mut self, frame: &Value, out: &mut Outbox, heard: &mut Vec<Heard>) {
        let next = (self.at..self.lines.len()).find(|&i| self.lines[i].0);
        let kind = next.filter(|&i| key(&self.lines[i].1) == key(frame));
        let matched = kind.filter(|&i| {
            let recorded = &self.lines[i].1;
            !answers_harness(recorded) || same(recorded, frame)
        });
        let Some(at) = matched else {
            if kind.is_some() {
                // The answer the recording waits for, given otherwise: nothing more is played.
                self.progress.refused.fetch_add(1, Ordering::SeqCst);
            }
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
        let left = self.lines.len().saturating_sub(self.at);
        self.progress.left.store(left, Ordering::SeqCst);
    }

    fn ended(&mut self) {
        self.fallback.ended();
    }

    fn waker(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.fallback.waker()
    }

    fn woken(&mut self, out: &mut Outbox, heard: &mut Vec<Heard>) {
        self.fallback.woken(out, heard);
    }
}

/// Whether a message from the driver answers a request of the harness's own: a JSON-RPC reply, or
/// Pi's answer to an extension's dialog.
fn answers_harness(message: &Value) -> bool {
    is_reply(message) || message["type"] == "extension_ui_response"
}

/// Whether the driver's answer is the recorded one: every field the recording has, the request's
/// id and the decision among them, is the same.
fn same(recorded: &Value, frame: &Value) -> bool {
    recorded
        .as_object()
        .is_some_and(|fields| fields.iter().all(|(k, v)| frame.get(k) == Some(v)))
}
