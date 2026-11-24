//! The checks the fuzz targets make, one for each place the core reads input from outside
//! (`docs/design/verification.md`, "7. Fuzzing"). Each takes the bytes libFuzzer made and panics
//! when a property breaks; not panicking on any input is the least of them. The targets in
//! `fuzz_targets/` only call these, and `tests/seeds.rs` runs them over every seed on the stable
//! compiler, so a check that no longer builds or no longer passes its seeds fails a pull request.
//!
//! | Target | Reads | Checks besides not failing |
//! | --- | --- | --- |
//! | `socket_lines` | the socket's line reader, in parts of any size | the lines are those written, none over the limit, no more than the limit held (S11, D-033) |
//! | `socket_hello` | a connector's first line | a hello the core takes has this agent's token and protocol |
//! | `socket_connector` | everything a connector writes after its hello | every event comes with the connector's harness and a `seq` that rises in its session (D-059) |
//! | `carrier_request` | a kind 1202's tags | the core acts only on what MDK reads alike: agent, item and action (S6, D-140) |
//! | `carrier_item` | a kind 1201 or an edit | an item the core reads, written again, fits, MDK reads it, and it reads back the same (S7) |
//! | `settings` | the settings file | every key the settings take is lowercase hex, as MDK gives keys (S6) |
//! | `saved_state` | the state file | one the core reads it writes back the same, and starts on |
//! | `codex`, `acp`, `pi` | each built-in driver's harness messages | every native message is the message read or a whole value of one of its fields (D-007), `seq` rises (D-059) |
//! | `import` | today's connectors' env files and unit lines | none |

pub mod seeds;

use cgka_traits::agent_events::{AgentItem, AgentRequest};
use marmot_agent::carrier::{self, Asked};
use marmot_agent::config::Config;
use marmot_agent::socket::lines::Splitter;
use marmot_agent::socket::{PROTOCOL, SocketDriver};
use marmot_agent::{Conversation, EventKey, PubKey, Rights, Saved, Who};
use marmot_agent_conformance::native::Arrived;
use marmot_agent_driver::{Capabilities, Driver, FilesIn, MidTurn, Output, Sessions};
use marmot_agent_events::AgentEvent;
use serde::Deserialize;
use std::collections::HashMap;

/// The agent's key, as the core holds it.
pub const AGENT: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
/// The connector's token.
pub const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The socket's line reader, with a small limit so libFuzzer reaches it: the first byte is the
/// limit, the next few the sizes of the parts the bytes arrive in, the rest the bytes.
pub fn socket_lines(data: &[u8]) {
    let Some((&max, rest)) = data.split_first() else {
        return;
    };
    let max = usize::from(max % 64);
    let (sizes, bytes) = rest.split_at(rest.len().min(4));
    let mut sizes = sizes.iter().map(|s| usize::from(*s % 32) + 1).cycle();
    let mut splitter = Splitter::new(max);
    let mut lines = Vec::new();
    let mut left = bytes;
    while !left.is_empty() {
        let size = sizes.next().unwrap_or(1).min(left.len());
        let mut part = &left[..size];
        while !part.is_empty() {
            let (used, line) = splitter.take(part);
            assert!(used > 0 && used <= part.len());
            assert!(splitter.held() <= max);
            lines.extend(line);
            part = &part[used..];
        }
        left = &left[size..];
    }
    let mut want: Vec<Vec<u8>> = bytes.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
    want.pop();
    want.retain(|l| l.len() <= max);
    assert_eq!(lines, want);
}

/// Runs `core` on the core's side of a fresh socket pair, in a runtime of its own, while the
/// connector's side writes `bytes`, reads all it is sent, and closes.
fn on_socket<T>(bytes: Vec<u8>, core: impl AsyncFnOnce(tokio::net::UnixStream) -> T) -> T {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    runtime.block_on(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (ours, theirs) = tokio::net::UnixStream::pair().expect("a socket pair");
        let connector = tokio::spawn(async move {
            let (mut read, mut write) = theirs.into_split();
            let drain = tokio::spawn(async move {
                let mut sink = Vec::new();
                let _ = read.read_to_end(&mut sink).await;
            });
            let _ = write.write_all(&bytes).await;
            let _ = write.shutdown().await;
            let _ = drain.await;
        });
        let out = core(ours).await;
        connector.abort();
        out
    })
}

/// A connector's first line, as the hello: the bytes with every newline taken out. A hello the
/// core takes names this agent's token and the protocol.
pub fn socket_hello(data: &[u8]) {
    let mut line = data.to_vec();
    line.retain(|&b| b != b'\n');
    let mut written = line.clone();
    written.push(b'\n');
    let taken = on_socket(written, async |stream| {
        SocketDriver::greet(stream, TOKEN, "Agent").await.is_ok()
    });
    if taken {
        let hello: serde_json::Value = serde_json::from_slice(&line).expect("a hello taken reads");
        assert_eq!(hello["params"]["token"], TOKEN);
        assert_eq!(hello["params"]["protocol"], PROTOCOL);
    }
}

/// A good hello, then whatever the connector writes. Every event the core takes from it carries
/// the connector's harness and the next `seq` of its session.
pub fn socket_connector(data: &[u8]) {
    let mut write = hello().into_bytes();
    write.extend_from_slice(data);
    let outputs = on_socket(write, async |stream| {
        let mut driver = SocketDriver::greet(stream, TOKEN, "Agent")
            .await
            .expect("the hello is good");
        let mut outputs = Vec::new();
        let limit = tokio::time::Duration::from_secs(5);
        while let Ok(Some(output)) = tokio::time::timeout(limit, driver.next_event()).await {
            outputs.push(output);
        }
        outputs
    });
    let mut seq: HashMap<String, u64> = HashMap::new();
    for output in outputs {
        let Output::Event(emitted) = output else {
            continue;
        };
        let header = emitted.event.header();
        assert_eq!(header.harness, "fuzzed");
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        let last = seq.entry(session.unwrap_or_default()).or_insert(0);
        assert_eq!(
            header.seq,
            *last + 1,
            "the core numbers each session's events"
        );
        *last = header.seq;
    }
}

/// The hello `socket_connector` opens with.
pub fn hello() -> String {
    let hello = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "hello",
        "params": {
            "protocol": PROTOCOL,
            "token": TOKEN,
            "harness": {"name": "fuzzed"},
            "capabilities": {
                "interrupt": true,
                "mid_turn": "restart",
                "sessions": "core",
                "resume": true,
                "approvals": true,
                "files_in": "images",
                "approval_commands": [],
            },
        },
    });
    format!("{hello}\n")
}

/// An event's parts, as JSON: what MDK hands the core.
#[derive(Deserialize)]
pub struct Parts {
    #[serde(default)]
    pub kind: u64,
    #[serde(default)]
    pub content: String,
    pub tags: Vec<Vec<String>>,
}

fn parts(data: &[u8]) -> Option<Parts> {
    serde_json::from_slice(data).ok()
}

/// What an action is called in a request's `details`.
fn action(asked: &Asked) -> &'static str {
    match asked {
        Asked::Choice { .. } => "choice",
        Asked::Questions { .. } => "questions",
        Asked::Interrupt => "interrupt",
        Asked::NewSession => "new_session",
        Asked::Set { .. } => "set",
        Asked::Own { .. } => "own",
    }
}

/// A kind 1202's tags: the core acts only on what MDK reads the same way, and hears everything
/// MDK reads for this agent.
pub fn carrier_request(data: &[u8]) {
    let Some(parts) = parts(data) else { return };
    let agent = PubKey::from(AGENT);
    let core = carrier::read_request(&agent, &parts.tags);
    let mdk = AgentRequest::from_tags(&parts.tags);
    if let Some((target, Ok(asked))) = &core {
        let mdk = mdk.as_ref().expect("MDK reads what the core acts on");
        assert_eq!(mdk.agent_pubkey_hex, AGENT);
        assert_eq!(
            mdk.item_message_id_hex.as_deref(),
            target.as_ref().map(EventKey::as_str)
        );
        assert_eq!(mdk.action, action(asked));
    }
    if mdk.is_ok_and(|m| m.agent_pubkey_hex == AGENT) {
        assert!(
            core.is_some(),
            "a request MDK reads for this agent is heard"
        );
    }
}

/// A kind 1201, or an edit of one: an item the core reads, written again, fits, MDK reads it as
/// the same item, and the core reads it back with the same ids.
pub fn carrier_item(data: &[u8]) {
    let Some(parts) = parts(data) else { return };
    let Some(read) = carrier::read_item(parts.kind, &parts.content, &parts.tags) else {
        return;
    };
    let item = read.item;
    let Ok(out) = carrier::item(&item) else {
        return;
    };
    assert!(out.size() <= carrier::LIMIT);
    let header = item.event.header();
    // MDK reads only an item whose ids are there and whose numbers start at 1: those the core
    // makes. One read from elsewhere may lack them.
    let shaped = !header.harness.is_empty()
        && !header.item.as_str().is_empty()
        && header.seq > 0
        && item.version > 0
        && i64::try_from(item.version).is_ok()
        && i64::try_from(header.seq).is_ok()
        && (item.settles.iter().chain(&item.started_by)).all(|e| is_event_id(e.as_str()));
    if shaped {
        let mdk =
            AgentItem::from_tags(&out.tags_but_target()).expect("MDK reads what the core writes");
        assert_eq!(mdk.version, item.version);
        assert_eq!(mdk.item_id, header.item.as_str());
        assert_eq!(mdk.seq, header.seq);
    }
    let back = carrier::read_item(out.kind, &out.content, &out.tags).expect("reads back");
    assert_eq!(back.item.version, item.version);
    assert_eq!(back.item.first, item.first);
    assert_eq!(back.item.settles, item.settles);
    assert_eq!(back.item.event.header().item, header.item);
    assert_eq!(back.item.event.header().state, header.state);
}

fn is_event_id(text: &str) -> bool {
    text.len() == 64 && (text.bytes()).all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The settings file: every key it takes is lowercase hex, so it matches the keys MDK gives.
pub fn settings(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(config) = Config::parse(text) else {
        return;
    };
    let rights = config.rights();
    let mut keys = vec![rights.owner.clone()];
    for who in [&rights.prompt, &rights.approve] {
        if let Who::People(people) = who {
            keys.extend(people.iter().cloned());
        }
    }
    for key in keys {
        assert!(
            is_event_id(key.as_str()),
            "a key the settings take is lowercase hex"
        );
    }
    let _ = config.setup();
    let _ = config.pi_setup();
    let _ = config.file_folders();
}

/// The state file: one the core reads, it writes back as it read it, and starts on, closing what
/// was left open and naming each queued send.
pub fn saved_state(data: &[u8]) {
    let Ok(saved) = serde_json::from_slice::<Saved>(data) else {
        return;
    };
    let again: Saved = serde_json::from_str(&serde_json::to_string(&saved).expect("writes"))
        .expect("reads what it wrote");
    assert!(
        again == saved,
        "the state file reads back as it was written"
    );
    let rights = Rights {
        owner: PubKey::from(AGENT),
        prompt: Who::Owner,
        approve: Who::Owner,
    };
    let capabilities = Capabilities {
        interrupt: true,
        mid_turn: MidTurn::Fold,
        sessions: Sessions::Core,
        resume: true,
        approvals: true,
        files_in: FilesIn::Images,
        approval_commands: Vec::new(),
        subagents: false,
        stop_subagent: false,
    };
    let mut talk = Conversation::new(PubKey::from(AGENT), "codex", rights, capabilities, saved);
    talk.recover();
    for group in talk.queued_groups() {
        let _ = talk.queued(&group);
        let _ = talk.next_send(&group);
    }
}

/// Lines of a harness's messages, each with which way it went: the first byte of a line says,
/// odd (as `1`) for a message from the harness, even (as `0`) for one to it.
fn harness_lines(data: &[u8]) -> Vec<(bool, &str)> {
    (data.split(|&b| b == b'\n'))
        .filter_map(|line| {
            let (&way, text) = line.split_first()?;
            Some((way % 2 == 1, std::str::from_utf8(text).ok()?))
        })
        .collect()
}

/// What a translator made, checked: every native message arrived, as
/// [`Arrived`](marmot_agent_conformance::native::Arrived) checks (D-007), and `seq` rises in each
/// session.
fn translated(
    arrived: &Arrived,
    events: &[AgentEvent],
    stopped: bool,
    seq: &mut HashMap<String, u64>,
) {
    for event in events {
        if let Err(why) = arrived.check(event, stopped) {
            panic!("{why}");
        }
        let header = event.header();
        let session = header.session.as_ref().map(|s| s.as_str().to_string());
        let last = seq.entry(session.unwrap_or_default()).or_insert(0);
        assert!(header.seq > *last, "seq rises in a session");
        *last = header.seq;
    }
}

macro_rules! translator_check {
    ($name:ident, $doc:literal, $make:expr, $crate_:ident) => {
        #[doc = $doc]
        pub fn $name(data: &[u8]) {
            let mut translator = $make;
            let mut arrived = Arrived::default();
            let mut seq = HashMap::new();
            for (from_harness, text) in harness_lines(data) {
                arrived.push(text);
                let way = if from_harness {
                    $crate_::Direction::In
                } else {
                    $crate_::Direction::Out
                };
                let events = translator.feed(way, text);
                translated(&arrived, &events, false, &mut seq);
            }
            let events = translator.harness_stopped();
            translated(&arrived, &events, true, &mut seq);
        }
    };
}

translator_check!(
    codex,
    "What Codex writes and is written, through the Codex driver's translator.",
    marmot_agent_codex::Translator::new(),
    marmot_agent_codex
);
translator_check!(
    acp,
    "What an ACP agent writes and is written, through the ACP driver's translator.",
    marmot_agent_acp::Translator::named("acp"),
    marmot_agent_acp
);
translator_check!(
    pi,
    "What Pi writes and is written, through the Pi driver's translator.",
    marmot_agent_pi::Translator::new(),
    marmot_agent_pi
);

/// Today's connectors' files, as the import reads them: env files, as the installers and Hermes
/// write them and as systemd reads them, and a unit's command line.
pub fn import(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = marmot_agent::import::env_file(text);
    let _ = marmot_agent::import::services::environment_file(text);
    for line in text.lines() {
        let _ = marmot_agent::import::services::words(line);
    }
}
