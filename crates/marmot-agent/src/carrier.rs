//! How an agent item, an answer and a control travel in a Marmot group, for now.
//!
//! Until MDK takes the new shape of kinds 1201 and 1202, an item travels under [`ITEM_KIND`] and
//! an answer or a control under [`REQUEST_KIND`], kinds MDK does not reserve. An item's later
//! versions are new events that name its first event and carry a higher version, where the real
//! carrier will use edits; a reader keeps the highest version of each item (D-069). The parts of
//! an event follow the carrier page (`docs/design/event-carrier.md`): the plain rendering in the
//! content, everything else in tags. The tag names are provisional.
//!
//! The core applies the 64 KB limit here, because only here is an item's final size known
//! (D-057).

use crate::keys::{EventKey, PubKey};
use marmot_agent_events::body::{Body, Tool};
use marmot_agent_events::{AgentEvent, Header, ItemId, Native, SessionId, State, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The kind an agent item travels under until MDK takes kind 1201.
pub const ITEM_KIND: u64 = 41201;
/// The kind an answer or a control travels under until MDK takes kind 1202.
pub const REQUEST_KIND: u64 = 41202;
/// The largest an agent item may be (D-026).
pub const LIMIT: usize = 64 * 1024;

/// An event ready for MDK to send: its kind, content and tags.
#[derive(Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// The kind.
    pub kind: u64,
    /// The content.
    pub content: String,
    /// The tags.
    pub tags: Vec<Vec<String>>,
}

impl Outgoing {
    /// How large the event's parts are, as near as the core can tell before MDK builds it.
    pub fn size(&self) -> usize {
        self.content.len() + serde_json::to_string(&self.tags).map_or(0, |t| t.len())
    }
}

/// Prints the kind and the size, and nothing the event holds (D-071).
impl std::fmt::Debug for Outgoing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outgoing")
            .field("kind", &self.kind)
            .field("size", &self.size())
            .finish_non_exhaustive()
    }
}

/// One version of an item, as it is sent.
#[derive(Clone, PartialEq)]
pub struct Item {
    /// The event, as the driver made it.
    pub event: AgentEvent,
    /// The item's version, from 1.
    pub version: u64,
    /// The item's first event, on every version after the first that was sent.
    pub first: Option<EventKey>,
    /// The message that started the turn, on a turn item.
    pub started_by: Option<EventKey>,
    /// The answers and controls whose effect this version shows (D-028).
    pub settles: Vec<EventKey>,
}

/// What the core left out of an item to keep it under the limit (D-057).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeftOut {
    /// The size of the native message, when it was left out.
    pub native: Option<usize>,
    /// The full length of the output, when it was cut.
    pub output: Option<usize>,
    /// How many of an edit's diffs were cut. A diff is a tool's output too: Codex puts a file
    /// edit's whole diff there.
    pub diffs: usize,
}

/// An item read back from an event.
#[derive(Clone, PartialEq)]
pub struct Read {
    /// The item, as sent.
    pub item: Item,
    /// What the core left out of it.
    pub left_out: LeftOut,
}

/// Writes one version of an item as an event, leaving out the native message and then cutting
/// the output if it would be over the limit (D-057). The output is a tool's output and each of
/// an edit's diffs; the largest is cut first, keeping its start and its end, until the event fits.
/// An item over the limit with nothing left to cut is sent as it is, for a relay to take or not.
pub fn item(item: &Item) -> Outgoing {
    let whole = write_item(item, LeftOut::default());
    if whole.size() <= LIMIT {
        return whole;
    }
    let mut left_out = LeftOut {
        native: item.event.native().map(|n| n.payload.get().len()),
        ..LeftOut::default()
    };
    let mut body = item.event.body().clone();
    let mut slimmer = item.clone();
    loop {
        slimmer.event = rebuild(&item.event, body.clone());
        let out = write_item(&slimmer, left_out);
        let over = out.size().saturating_sub(LIMIT);
        let Body::Tool(tool) = &mut body else {
            return out;
        };
        if over == 0 || !cut_largest(tool, over, &mut left_out) {
            return out;
        }
    }
}

/// Cuts the largest of a tool's output and diffs by at least `over` bytes, and notes it. `false`
/// when nothing is left that a cut would shorten.
fn cut_largest(tool: &mut Tool, over: usize, left_out: &mut LeftOut) -> bool {
    // Escaping in JSON can make text count for more than its length, so cut twice as much.
    let over = over * 2;
    let output = tool.output.as_ref().map_or(0, String::len);
    let diff = tool
        .changes
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| c.diff.len())
        .map(|(at, c)| (at, c.diff.len()));
    let worth_cutting = MARKER_ROOM * 2;
    match diff {
        Some((at, length)) if length > output && length > worth_cutting => {
            let change = &mut tool.changes[at];
            change.diff = cut(&change.diff, over);
            left_out.diffs += 1;
        }
        _ if output > worth_cutting => {
            let text = tool.output.take().unwrap_or_default();
            left_out.output.get_or_insert(text.len());
            tool.output = Some(cut(&text, over));
        }
        _ => return false,
    }
    true
}

/// Room left for the marker that says where text was cut.
const MARKER_ROOM: usize = 64;

/// The event with another body and no native message: what the core sends when it leaves the
/// native message out, cuts the output, or closes an item itself.
pub fn rebuild(event: &AgentEvent, body: Body) -> AgentEvent {
    AgentEvent::new(event.header().clone(), body, None)
        .expect("an event with no native message repeats no secret")
}

/// What is left of `text` after cutting at least `over` bytes from its middle, keeping the start
/// and the end, with a marker where it was cut.
fn cut(text: &str, over: usize) -> String {
    let marker = format!("\n… cut: {} bytes in all …\n", text.len());
    let keep = text.len().saturating_sub(over + marker.len() + MARKER_ROOM);
    let mut head = keep / 2;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - keep / 2;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{marker}{}", &text[..head], &text[tail..])
}

/// The type's own fields, as one tag of JSON: `{"type": ..., "body": ...}`.
#[derive(Serialize, Deserialize)]
struct Fields(Body);

fn write_item(item: &Item, left_out: LeftOut) -> Outgoing {
    let header = item.event.header();
    let mut tags = vec![
        tag(["v", &item.version.to_string()]),
        tag(["state", word(&header.state)]),
        tag(["title", &header.title]),
        tag(["harness", &header.harness]),
        tag(["item_id", header.item.as_str()]),
        tag(["seq", &header.seq.to_string()]),
        tag(["fields", &json(&Fields(item.event.body().clone()))]),
    ];
    if let Some(first) = &item.first {
        tags.push(tag(["item", first.as_str()]));
    }
    if let Some(session) = &header.session {
        tags.push(tag(["session", session.as_str()]));
    }
    if let Some(turn) = &header.turn {
        tags.push(tag(["turn", turn.as_str()]));
    }
    if let Some(started_by) = &item.started_by {
        tags.push(tag(["e", started_by.as_str()]));
    }
    for request in &item.settles {
        tags.push(tag(["settles", request.as_str()]));
    }
    if let Some(native) = item.event.native() {
        tags.push(tag(["native", &native.kind, native.payload.get()]));
    }
    if let Some(size) = left_out.native {
        tags.push(tag(["left_out", "native", &size.to_string()]));
    }
    if let Some(length) = left_out.output {
        tags.push(tag(["cut", "output", &length.to_string()]));
    }
    if left_out.diffs > 0 {
        tags.push(tag(["cut", "diffs", &left_out.diffs.to_string()]));
    }
    Outgoing {
        kind: ITEM_KIND,
        content: header.text.clone(),
        tags,
    }
}

/// Reads an item back from an event's content and tags. `None` when it is not one.
pub fn read_item(content: &str, tags: &[Vec<String>]) -> Option<Read> {
    let one = |name: &str| value(tags, name);
    let Fields(body) = serde_json::from_str(one("fields")?).ok()?;
    let header = Header {
        harness: one("harness")?.to_string(),
        session: one("session").map(SessionId::from),
        turn: one("turn").map(TurnId::from),
        item: ItemId::from(one("item_id")?),
        seq: one("seq")?.parse().ok()?,
        state: serde_json::from_value(serde_json::Value::String(one("state")?.into())).ok()?,
        title: one("title")?.to_string(),
        text: content.to_string(),
    };
    let native = tags
        .iter()
        .find(|t| t.first().map(String::as_str) == Some("native") && t.len() == 3)
        .and_then(|t| {
            let payload = RawValue::from_string(t[2].clone()).ok()?;
            Some(Native {
                kind: t[1].clone(),
                payload,
            })
        });
    let event = AgentEvent::new(header, body, native).ok()?;
    let sized = |what: &str| {
        tags.iter()
            .find(|t| t.len() == 3 && t[1] == what && (t[0] == "left_out" || t[0] == "cut"))
            .and_then(|t| t[2].parse().ok())
    };
    Some(Read {
        item: Item {
            event,
            version: one("v")?.parse().ok()?,
            first: one("item").map(EventKey::from),
            started_by: one("e").map(EventKey::from),
            settles: values(tags, "settles").map(EventKey::from).collect(),
        },
        left_out: LeftOut {
            native: sized("native"),
            output: sized("output"),
            diffs: sized("diffs").unwrap_or(0),
        },
    })
}

/// What a person's app asks of an agent: an answer or a control (D-027).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Asked {
    /// An answer to an approval: the id of one choice.
    Choice {
        /// The choice's id.
        choice: String,
    },
    /// An answer to a question item.
    Questions {
        /// One for each question answered.
        questions: Vec<AskedReply>,
    },
    /// Stop the running turn.
    Interrupt,
    /// End the chat's session and start another.
    NewSession,
    /// Change one setting of the session.
    Set {
        /// The setting's id.
        setting: String,
        /// The new value.
        value: String,
    },
    /// One of the harness's own actions.
    Own {
        /// The action's id.
        id: String,
    },
}

/// The answer to one question.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskedReply {
    /// The question's id.
    pub question: String,
    /// The ids of the options chosen.
    #[serde(default)]
    pub options: Vec<String>,
    /// What the person typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Names what is asked, and nothing of its values (D-071).
impl std::fmt::Debug for Asked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Choice { .. } => "Asked::Choice",
            Self::Questions { .. } => "Asked::Questions",
            Self::Interrupt => "Asked::Interrupt",
            Self::NewSession => "Asked::NewSession",
            Self::Set { .. } => "Asked::Set",
            Self::Own { .. } => "Asked::Own",
        })
    }
}

/// An answer or a control, as a person's app sends it: for `agent`, about `target` (an item's
/// first event), with a plain rendering such as "Allow once".
pub fn request(agent: &PubKey, target: Option<&EventKey>, asked: &Asked, words: &str) -> Outgoing {
    let mut tags = vec![tag(["p", agent.as_str()]), tag(["details", &json(asked)])];
    if let Some(target) = target {
        tags.push(tag(["e", target.as_str()]));
    }
    Outgoing {
        kind: REQUEST_KIND,
        content: words.to_string(),
        tags,
    }
}

/// An answer or a control read back: what it is about and what it asks. `None` when it is not one,
/// or is for another agent (D-027).
pub fn read_request(agent: &PubKey, tags: &[Vec<String>]) -> Option<(Option<EventKey>, Asked)> {
    if value(tags, "p")? != agent.as_str() {
        return None;
    }
    let asked = serde_json::from_str(value(tags, "details")?).ok()?;
    Some((value(tags, "e").map(EventKey::from), asked))
}

/// The tags of the agent's words, a chat message (D-022): the turn it belongs to.
pub fn words_tags(turn: Option<&TurnId>) -> Vec<Vec<String>> {
    turn.map(|t| vec![tag(["turn", t.as_str()])])
        .unwrap_or_default()
}

fn tag<const N: usize>(parts: [&str; N]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("the core's own types serialize")
}

fn word(state: &State) -> &'static str {
    match state {
        State::Running => "running",
        State::Waiting => "waiting",
        State::Done => "done",
        State::Failed => "failed",
        State::Declined => "declined",
        State::Cancelled => "cancelled",
    }
}

fn value<'a>(tags: &'a [Vec<String>], name: &str) -> Option<&'a str> {
    values(tags, name).next()
}

fn values<'a>(tags: &'a [Vec<String>], name: &str) -> impl Iterator<Item = &'a str> {
    let name = name.to_string();
    tags.iter()
        .filter(move |t| t.len() >= 2 && t[0] == name)
        .map(|t| t[1].as_str())
}
