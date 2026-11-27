//! D-007's check of a translator: every native message an event carries arrived, unchanged. A
//! native message is, byte for byte, the whole of the message it came from or the whole value of
//! one of that message's fields (`params`, `result`, `error`), and a streamed one is a list of
//! such values. So the check compares it with those values of the message it came from: the one
//! the translator was reading when it made the event. Text found anywhere in what arrived is not
//! enough, or a payload a translator made up, such as a `{}` for a field a message lacks, would
//! pass whenever another message held an empty object.
//!
//! The translators' property (`tests/translators.rs`) and the fuzz targets of the built-in drivers
//! (`crates/marmot-agent-fuzz`) both use it.

use marmot_agent_events::AgentEvent;
use serde::de::{Deserialize, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;

/// Whether `kind` is one of the ACP streamed-chunk native kinds whose payload is a JSON array of
/// arrived chunk payloads rather than a single arrived message/field value.
pub fn is_streamed_kind(kind: &str) -> bool {
    matches!(
        kind,
        "session/update [agent_message_chunk]" | "session/update [agent_thought_chunk]"
    )
}

/// The messages a translator was fed, in order, each as the values a native message may be.
#[derive(Default)]
pub struct Arrived {
    messages: Vec<Vec<String>>,
}

impl Arrived {
    /// A message the translator is fed next, either way.
    pub fn push(&mut self, text: &str) {
        let mut values = Vec::new();
        if let Ok(whole) = serde_json::from_str::<&RawValue>(text) {
            values.push(whole.get().to_string());
        }
        if let Ok(Fields(fields)) = serde_json::from_str::<Fields>(text) {
            values.extend(fields);
        }
        self.messages.push(values);
    }

    /// Checks an event the translator made: a native message is one of the values of the
    /// message it was reading, the last one pushed, or of any message before when the harness
    /// had stopped (`stopped`), when what it still held comes out. Each chunk of a streamed one
    /// is a value of a message that arrived.
    pub fn check(&self, event: &AgentEvent, stopped: bool) -> Result<(), String> {
        let Some(native) = event.native() else {
            return Ok(());
        };
        let id = event.header().item.as_str();
        let kind = &native.kind;
        if is_streamed_kind(kind) {
            let chunks: Vec<&RawValue> =
                serde_json::from_str(native.payload.get()).map_err(|_| {
                    format!("item {id}: a streamed native message ({kind}) is not a list")
                })?;
            for chunk in chunks {
                if !self.messages.iter().flatten().any(|v| v == chunk.get()) {
                    return Err(format!(
                        "D-007: a chunk of item {id}'s native message ({kind}) is not a whole value of a message that arrived"
                    ));
                }
            }
            return Ok(());
        }
        let from: &[Vec<String>] = if stopped {
            &self.messages
        } else {
            self.messages
                .last()
                .map(std::slice::from_ref)
                .unwrap_or_default()
        };
        if from.iter().flatten().any(|v| v == native.payload.get()) {
            Ok(())
        } else {
            Err(format!(
                "D-007: item {id}'s native message ({kind}) is not the message it came from, nor the whole value of one of its fields"
            ))
        }
    }
}

/// The value of each field of a JSON object, as it was written, a field given twice included.
struct Fields(Vec<String>);

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Each;
        impl<'de> Visitor<'de> for Each {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut values = Vec::new();
                while let Some((IgnoredAny, value)) =
                    map.next_entry::<IgnoredAny, Box<RawValue>>()?
                {
                    values.push(value.get().to_string());
                }
                Ok(Fields(values))
            }
        }
        deserializer.deserialize_map(Each)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_native_kind(kind: &str, payload: &str) -> AgentEvent {
        let kind_json = serde_json::to_string(kind).expect("kind json");
        let event = format!(
            r#"{{"harness": "h", "item": "notice:1", "seq": 1, "type": "notice", "state": "done", "title": "t", "text": "", "body": {{"kind": "warning"}}, "native": {{"kind": {kind_json}, "payload": {payload}}}}}"#
        );
        serde_json::from_str(&event).expect("an event")
    }

    fn with_native(payload: &str) -> AgentEvent {
        with_native_kind("warning", payload)
    }

    #[test]
    fn non_streamed_kind_ending_in_chunk_bracket_is_not_treated_as_chunk_list() {
        // Regression for #242: a dynamic command/method name ending in `_chunk]`
        // (e.g. Pi's `response to get__chunk]`) carries a single native payload, not
        // an ACP streamed chunk list.
        let mut arrived = Arrived::default();
        let msg = r#"{"id":"4","type":"response","command":"get__chunk]","message":"/mcp"}"#;
        arrived.push(msg);
        assert_eq!(
            arrived.check(&with_native_kind("response to get__chunk]", msg), false),
            Ok(())
        );
        // True ACP streamed kinds still require a JSON list of arrived chunks.
        for streamed in [
            "session/update [agent_message_chunk]",
            "session/update [agent_thought_chunk]",
        ] {
            assert!(
                arrived
                    .check(&with_native_kind(streamed, msg), false)
                    .is_err(),
                "{streamed} must still reject a non-list payload"
            );
            assert_eq!(
                arrived.check(&with_native_kind(streamed, &format!("[{msg}]")), false),
                Ok(()),
                "{streamed} must accept a list of arrived chunks"
            );
        }
    }

    #[test]
    fn a_native_message_is_the_message_read_or_a_value_of_one_of_its_fields() {
        let mut arrived = Arrived::default();
        arrived.push(r#"{"method":"warning","params":{"message":"m"},"params":{"a": 1}}"#);
        for payload in [
            r#"{"method":"warning","params":{"message":"m"},"params":{"a": 1}}"#,
            r#"{"message":"m"}"#,
            r#"{"a": 1}"#,
        ] {
            assert_eq!(
                arrived.check(&with_native(payload), false),
                Ok(()),
                "{payload}"
            );
        }
        // Part of a value, or the same value written otherwise, did not arrive.
        for payload in [r#""m""#, r#"{"a":1}"#] {
            assert!(
                arrived.check(&with_native(payload), false).is_err(),
                "{payload}"
            );
        }
    }

    // A translator that reads a missing field as `{}` makes up a native message, though another
    // message held an empty object: text search over all that arrived let it through.
    #[test]
    fn a_native_message_made_up_is_caught_though_its_text_arrived_elsewhere() {
        let mut arrived = Arrived::default();
        arrived.push(r#"{"id":2,"method":"model/list","params":{}}"#);
        arrived.push(r#"{"id":2}"#);
        assert!(arrived.check(&with_native("{}"), false).is_err());
        // Once the harness stopped, what the translator held may come from any message.
        assert_eq!(arrived.check(&with_native("{}"), true), Ok(()));
    }
}
