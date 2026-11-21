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
use marmot_agent_events::body::{Body, Notice, NoticeKind, Question};
use marmot_agent_events::{AgentEvent, Header, ItemId, Native, SessionId, State, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    /// How large the event is: the whole event as a relay gets it, with its id, author, time and
    /// signature, and the content and tags escaped as JSON. The core cannot see the event MDK
    /// builds, so this stands in for it, rounding each part up.
    pub fn size(&self) -> usize {
        event_size(self.kind, &self.content, &self.tags)
    }
}

/// The size of an event with these parts, as [`Outgoing::size`] measures it.
fn event_size(kind: u64, content: &str, tags: &[Vec<String>]) -> usize {
    #[derive(Serialize)]
    struct Event<'a> {
        id: &'a str,
        pubkey: &'a str,
        created_at: u64,
        kind: u64,
        tags: &'a [Vec<String>],
        content: &'a str,
        sig: &'a str,
    }
    let hex = "0".repeat(128);
    let event = Event {
        id: &hex[..64],
        pubkey: &hex[..64],
        created_at: u64::MAX,
        kind,
        tags,
        content,
        sig: &hex,
    };
    serde_json::to_string(&event).map_or(usize::MAX, |e| e.len())
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
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeftOut {
    /// The size of the native message, when it was left out.
    pub native: Option<usize>,
    /// The full length of the output, when it was cut.
    pub output: Option<usize>,
    /// How many of an edit's diffs were cut. A diff is a tool's output too: Codex puts a file
    /// edit's whole diff there.
    pub diffs: usize,
    /// Anything else cut, by the kind of field, with the full length of all that was cut of that
    /// kind: `text` for the plain rendering, `title`, `command`, `error`, `input`, `plan`,
    /// `steps`, `choices`, `questions` and `description`. A list thinned in its middle, with how
    /// many it held: `entries` or `changes`. A session's commands whose descriptions were
    /// dropped: `descriptions`, with how many. An item replaced by a stand-in: `item`, with the
    /// size of the event it stands in for. The names are provisional.
    pub fields: Vec<(String, usize)>,
}

/// An item read back from an event.
#[derive(Clone, PartialEq)]
pub struct Read {
    /// The item, as sent.
    pub item: Item,
    /// What the core left out of it.
    pub left_out: LeftOut,
}

/// Writes one version of an item as an event no larger than the limit (D-026, D-057): fitted by
/// [`fit`], or, when what is never cut is over the limit alone, a short stand-in in its place,
/// the same item at the same version, naming its first event and settling what it settles.
/// Nothing over the limit is ever sent: when even the stand-in is over it, as for a version that
/// settles very many requests or whose ids are very long, this fails, and the send is refused.
/// The conversation never queues a version that settles more than [`SETTLES_MAX`], so its own
/// stand-ins fit.
pub fn item(item: &Item) -> Result<Outgoing, TooLarge> {
    fit(item).or_else(|TooLarge| {
        let out = stand_in(item);
        if out.size() <= LIMIT {
            Ok(out)
        } else {
            Err(TooLarge)
        }
    })
}

/// The most requests one event settles. Each takes about 80 bytes, so this many take about 16 KB,
/// and an event that names them still fits; the conversation spreads more over several events.
pub const SETTLES_MAX: usize = 200;

/// What is never cut in an item is over the limit alone.
#[derive(Debug)]
pub struct TooLarge;

/// Writes one version of an item as an event, cutting it if it would be over the limit, in the
/// order D-057 sets and then on (each step only while the event is still over the limit): the
/// native message is left out; then a tool's output, and an edit's diffs, which are output too,
/// are cut, keeping their start and their end; then a session's commands lose their descriptions
/// and keep their names and hints (D-045); then the largest remaining text is cut the same way:
/// the plain rendering, the title, a tool's command, error and input, a plan's text and steps, an
/// approval's choices, a question's words, a task's description. Ids, names, hints, paths and the
/// values a person's answer names are never cut. Only when no text is left to cut are a plan's
/// steps, or an edit's changes, thinned in their middle. What was cut is said in a few `cut`
/// tags, one for each kind of field, never one for each step.
pub fn fit(item: &Item) -> Result<Outgoing, TooLarge> {
    let whole = write_item(item, &LeftOut::default());
    if whole.size() <= LIMIT {
        return Ok(whole);
    }
    let mut fitting = Fitting::new(item);
    let mut slimmer = item.clone();
    loop {
        slimmer.event = fitting.event();
        let left_out = fitting.left_out();
        let out = write_item(&slimmer, &left_out);
        let over = out.size().saturating_sub(LIMIT);
        if over == 0 {
            return Ok(out);
        }
        // Escaping in JSON can make text count for more than its length, and the cut tags add a
        // little, so cut more than is over.
        let more = over * 2 + 256;
        if !fitting.cut(more, true)
            && !fitting.drop_descriptions()
            && !fitting.cut(more, false)
            && !fitting.thin()
        {
            return Err(TooLarge);
        }
    }
}

/// The stand-in for an item that cannot fit: a warning notice in its place, with the item's ids,
/// state, version, first event and what it settles, and a `cut` tag `item` with the size of the
/// event it stands in for. The tag names are provisional.
fn stand_in(item: &Item) -> Outgoing {
    let header = Header {
        title: "Too large to show".into(),
        text: "This was too large to send to the chat.".into(),
        ..item.event.header().clone()
    };
    let body = Body::Notice(Notice {
        kind: NoticeKind::Warning,
        duration_ms: None,
        reason: Some("too_large".into()),
    });
    let slim = Item {
        event: AgentEvent::new(header, body, None).expect("a notice repeats no secret"),
        ..item.clone()
    };
    let size = write_item(item, &LeftOut::default()).size();
    let left_out = LeftOut {
        fields: vec![("item".into(), size)],
        ..LeftOut::default()
    };
    write_item(&slim, &left_out)
}

/// A string that may be cut, by where it is.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Spot {
    Text,
    Title,
    Command,
    Output,
    Error,
    Diff(usize),
    /// A string in a tool's input, the harness's own JSON, by its JSON pointer.
    Input(String),
    PlanText,
    Step(usize),
    Choice(usize),
    QuestionHeader(usize),
    QuestionText(usize),
    Description,
}

impl Spot {
    /// Whether it is a tool's output, which D-057 cuts before any other text. A diff is a tool's
    /// output too: Codex puts a file edit's whole diff there.
    fn is_output(&self) -> bool {
        matches!(self, Self::Output | Self::Diff(_))
    }

    /// The kind of field, as a `cut` tag names it.
    fn kind(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Title => "title",
            Self::Command => "command",
            Self::Output => "output",
            Self::Error => "error",
            Self::Diff(_) => "diffs",
            Self::Input(_) => "input",
            Self::PlanText => "plan",
            Self::Step(_) => "steps",
            Self::Choice(_) => "choices",
            Self::QuestionHeader(_) | Self::QuestionText(_) => "questions",
            Self::Description => "description",
        }
    }
}

/// An item being cut to fit.
struct Fitting {
    header: Header,
    body: Body,
    /// A question's parts, when the item is one: they are cut here, and the question made again.
    question: Option<QuestionParts>,
    /// The size of the native message, left out.
    native: Option<usize>,
    /// The full length of each string cut, the first time it was cut.
    full: std::collections::HashMap<Spot, usize>,
    /// For each list thinned, how many it held; and how many descriptions were dropped.
    counts: Vec<(&'static str, usize)>,
}

struct QuestionParts {
    via: marmot_agent_events::body::AnswerVia,
    subject: Option<ItemId>,
    questions: Vec<marmot_agent_events::body::Asked>,
    answer: Option<Vec<marmot_agent_events::body::Answered>>,
}

impl Fitting {
    fn new(item: &Item) -> Self {
        let body = item.event.body().clone();
        let question = match &body {
            Body::Question(q) => Some(QuestionParts {
                via: q.answer_via(),
                subject: q.subject().cloned(),
                questions: q.questions().to_vec(),
                answer: q.answer().map(<[_]>::to_vec),
            }),
            _ => None,
        };
        Self {
            header: item.event.header().clone(),
            body,
            question,
            native: item.event.native().map(|n| n.payload.get().len()),
            full: std::collections::HashMap::new(),
            counts: Vec::new(),
        }
    }

    /// Drops a session's commands' descriptions, keeping their names and hints (D-045). Whether
    /// any was dropped.
    fn drop_descriptions(&mut self) -> bool {
        let Body::Session(session) = &mut self.body else {
            return false;
        };
        let dropped = (session.commands.iter_mut())
            .filter_map(|c| c.description.take())
            .count();
        if dropped > 0 {
            self.counts.push(("descriptions", dropped));
        }
        dropped > 0
    }

    /// The event as it stands, with no native message.
    fn event(&self) -> AgentEvent {
        let body = match &self.question {
            Some(q) => {
                let question = Question::new(q.via, q.subject.clone(), q.questions.clone())
                    .and_then(|made| match &q.answer {
                        Some(answer) => made.answered(answer.clone()),
                        None => Ok(made),
                    });
                match question {
                    Ok(question) => Body::Question(question),
                    // Never: only words are cut, and ids are kept.
                    Err(_) => self.body.clone(),
                }
            }
            None => self.body.clone(),
        };
        AgentEvent::new(self.header.clone(), body, None)
            .expect("an event with no native message repeats no secret")
    }

    /// What was left out and cut, in a few tags: one for each kind of field, with the full
    /// length of all that was cut of that kind, and one for each list thinned, with how many it
    /// held. The output's full length, and how many diffs were cut, keep their own tags.
    fn left_out(&self) -> LeftOut {
        let mut left_out = LeftOut {
            native: self.native,
            ..LeftOut::default()
        };
        let mut kinds: Vec<(String, usize)> = Vec::new();
        for (spot, full) in &self.full {
            match spot {
                Spot::Output => left_out.output = Some(*full),
                Spot::Diff(_) => left_out.diffs += 1,
                _ => match kinds.iter_mut().find(|(k, _)| k == spot.kind()) {
                    Some((_, total)) => *total += full,
                    None => kinds.push((spot.kind().into(), *full)),
                },
            }
        }
        kinds.sort();
        left_out.fields = kinds;
        left_out
            .fields
            .extend(self.counts.iter().map(|(k, n)| ((*k).into(), *n)));
        left_out
    }

    /// Every string that may be cut, with its length.
    fn spots(&self) -> Vec<(Spot, usize)> {
        let mut spots = vec![
            (Spot::Text, self.header.text.len()),
            (Spot::Title, self.header.title.len()),
        ];
        let mut some = |spot: Spot, text: Option<&String>| {
            if let Some(text) = text {
                spots.push((spot, text.len()));
            }
        };
        match &self.body {
            Body::Tool(tool) => {
                some(Spot::Command, tool.command.as_ref());
                some(Spot::Output, tool.output.as_ref());
                some(Spot::Error, tool.error.as_ref());
                for (at, change) in tool.changes.iter().enumerate() {
                    some(Spot::Diff(at), Some(&change.diff));
                }
                if let Some(input) = &tool.input {
                    let mut leaves = Vec::new();
                    leaves_of(input, &mut String::new(), &mut leaves);
                    for (pointer, length) in leaves {
                        spots.push((Spot::Input(pointer), length));
                    }
                }
            }
            Body::Plan(plan) => {
                some(Spot::PlanText, plan.text.as_ref());
                for (at, entry) in plan.entries.iter().enumerate() {
                    some(Spot::Step(at), Some(&entry.text));
                }
            }
            Body::Approval(approval) => {
                for (at, choice) in approval.choices.iter().enumerate() {
                    some(Spot::Choice(at), Some(&choice.label));
                }
            }
            Body::Task(task) => some(Spot::Description, task.description.as_ref()),
            _ => {}
        }
        if let Some(q) = &self.question {
            for (at, asked) in q.questions.iter().enumerate() {
                spots.push((Spot::QuestionText(at), asked.text.len()));
                if let Some(header) = &asked.header {
                    spots.push((Spot::QuestionHeader(at), header.len()));
                }
            }
        }
        spots
    }

    fn text_mut(&mut self, spot: &Spot) -> Option<&mut String> {
        if let Some(q) = &mut self.question {
            match spot {
                Spot::QuestionText(at) => return q.questions.get_mut(*at).map(|a| &mut a.text),
                Spot::QuestionHeader(at) => {
                    return q.questions.get_mut(*at).and_then(|a| a.header.as_mut());
                }
                _ => {}
            }
        }
        match (spot, &mut self.body) {
            (Spot::Text, _) => Some(&mut self.header.text),
            (Spot::Title, _) => Some(&mut self.header.title),
            (Spot::Command, Body::Tool(t)) => t.command.as_mut(),
            (Spot::Output, Body::Tool(t)) => t.output.as_mut(),
            (Spot::Error, Body::Tool(t)) => t.error.as_mut(),
            (Spot::Diff(at), Body::Tool(t)) => t.changes.get_mut(*at).map(|c| &mut c.diff),
            (Spot::Input(pointer), Body::Tool(t)) => match t.input.as_mut()?.pointer_mut(pointer) {
                Some(Value::String(text)) => Some(text),
                _ => None,
            },
            (Spot::PlanText, Body::Plan(p)) => p.text.as_mut(),
            (Spot::Step(at), Body::Plan(p)) => p.entries.get_mut(*at).map(|e| &mut e.text),
            (Spot::Choice(at), Body::Approval(a)) => a.choices.get_mut(*at).map(|c| &mut c.label),
            (Spot::Description, Body::Task(t)) => t.description.as_mut(),
            _ => None,
        }
    }

    /// Cuts the largest strings first until about `over` bytes are saved, as JSON counts them:
    /// only a tool's output when `output`, else any. Whether anything was cut.
    fn cut(&mut self, over: usize, output: bool) -> bool {
        let mut spots: Vec<(Spot, usize)> = (self.spots().into_iter())
            .filter(|(spot, length)| *length > WORTH_CUTTING && (!output || spot.is_output()))
            .collect();
        spots.sort_by_key(|(_, length)| std::cmp::Reverse(*length));
        let mut left = over;
        let mut any = false;
        for (spot, length) in spots {
            if left == 0 {
                break;
            }
            let full = *self.full.get(&spot).unwrap_or(&length);
            let Some(text) = self.text_mut(&spot) else {
                continue;
            };
            let before = escaped(text);
            let shorter = cut(text, left, full);
            if shorter.len() + 16 > text.len() {
                continue;
            }
            *text = shorter;
            left = left.saturating_sub(before.saturating_sub(escaped(text)));
            self.full.entry(spot).or_insert(full);
            any = true;
        }
        any
    }

    /// Thins a plan's steps, or else an edit's changes, in their middle, keeping the start and
    /// the end. Whether anything was thinned. A session's commands are never thinned (D-045).
    fn thin(&mut self) -> bool {
        let (kind, length) = match &self.body {
            Body::Plan(p) if p.entries.len() > 2 => ("entries", p.entries.len()),
            Body::Tool(t) if t.changes.len() > 2 => ("changes", t.changes.len()),
            _ => return false,
        };
        if !self.counts.iter().any(|(k, _)| *k == kind) {
            self.counts.push((kind, length));
        }
        let gone = length / 2;
        let from = (length - gone) / 2;
        match &mut self.body {
            Body::Plan(p) => drop(p.entries.drain(from..from + gone)),
            Body::Tool(t) => drop(t.changes.drain(from..from + gone)),
            _ => {}
        }
        // What was noted of the steps or changes taken out goes with them; those after the gap
        // moved up, and what was noted of them moves with them.
        let place = |at: usize| match at {
            _ if at < from => Some(at),
            _ if at < from + gone => None,
            _ => Some(at - gone),
        };
        self.full = (std::mem::take(&mut self.full).into_iter())
            .filter_map(|(spot, full)| {
                let spot = match spot {
                    Spot::Step(at) => Spot::Step(place(at)?),
                    Spot::Diff(at) => Spot::Diff(place(at)?),
                    other => other,
                };
                Some((spot, full))
            })
            .collect();
        true
    }
}

/// Every string in a tool's input, with its JSON pointer and length.
fn leaves_of(value: &Value, pointer: &mut String, leaves: &mut Vec<(String, usize)>) {
    match value {
        Value::String(text) => leaves.push((pointer.clone(), text.len())),
        Value::Array(items) => {
            for (at, item) in items.iter().enumerate() {
                let length = pointer.len();
                pointer.push_str(&format!("/{at}"));
                leaves_of(item, pointer, leaves);
                pointer.truncate(length);
            }
        }
        Value::Object(map) => {
            for (name, item) in map {
                let length = pointer.len();
                pointer.push('/');
                pointer.push_str(&name.replace('~', "~0").replace('/', "~1"));
                leaves_of(item, pointer, leaves);
                pointer.truncate(length);
            }
        }
        _ => {}
    }
}

/// How long a string is as JSON writes it.
fn escaped(text: &str) -> usize {
    serde_json::to_string(text).map_or(text.len(), |t| t.len())
}

/// Text shorter than this is not cut: what a cut leaves is about this long.
const WORTH_CUTTING: usize = MARKER_ROOM * 2;

/// The agent's words, cut in their middle if the chat message they go in would be over the limit
/// with `tags`, as an item's text is (D-057). Fails when cutting cannot make it fit, as with very
/// many `settles` tags: nothing over the limit is ever sent.
pub fn fit_words(text: &str, tags: &[Vec<String>]) -> Result<String, TooLarge> {
    let mut text = text.to_string();
    loop {
        // Room for what MDK adds to an edit, which names the message it replaces.
        let over = (event_size(9, &text, tags) + 256).saturating_sub(LIMIT);
        if over == 0 {
            return Ok(text);
        }
        let shorter = cut(&text, over * 2, text.len());
        if text.len() <= WORTH_CUTTING || shorter.len() >= text.len() {
            return Err(TooLarge);
        }
        text = shorter;
    }
}

/// Room left for the marker that says where text was cut.
const MARKER_ROOM: usize = 64;

/// The event with another body and no native message: what the core sends when it leaves the
/// native message out, cuts the output, or closes an item itself.
pub fn rebuild(event: &AgentEvent, body: Body) -> AgentEvent {
    AgentEvent::new(event.header().clone(), body, None)
        .expect("an event with no native message repeats no secret")
}

/// What is left of `text` after cutting about `over` bytes from its middle, keeping the start and
/// the end, with a marker where it was cut that gives its `full` length. At least `MARKER_ROOM`
/// bytes of it are kept.
fn cut(text: &str, over: usize, full: usize) -> String {
    let marker = format!("\n… cut: {full} bytes in all …\n");
    let keep = (text.len().saturating_sub(over + marker.len())).max(MARKER_ROOM);
    if keep >= text.len() {
        return text.to_string();
    }
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

fn write_item(item: &Item, left_out: &LeftOut) -> Outgoing {
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
    for (name, size) in &left_out.fields {
        tags.push(tag(["cut", name, &size.to_string()]));
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
            fields: tags
                .iter()
                .filter(|t| t.len() == 3 && t[0] == "cut" && t[1] != "output" && t[1] != "diffs")
                .filter_map(|t| Some((t[1].clone(), t[2].parse().ok()?)))
                .collect(),
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

/// The tags of the agent's words, a chat message (D-022): the turn they belong to, for a remark
/// made while the agent works the phase that keeps it from alerting (D-078), and the answers and
/// controls whose effect they show, as an item names them (D-028).
pub fn words_tags(turn: Option<&TurnId>, remark: bool, settles: &[EventKey]) -> Vec<Vec<String>> {
    let mut tags: Vec<Vec<String>> = turn
        .map(|t| tag(["turn", t.as_str()]))
        .into_iter()
        .collect();
    if remark {
        tags.push(tag(["phase", "commentary"]));
    }
    for request in settles {
        tags.push(tag(["settles", request.as_str()]));
    }
    tags
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
