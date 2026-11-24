//! The starting corpus of each target, made from what the repository holds: the socket's examples
//! (`fixtures/socket/examples.json`), every recording of Codex, the ACP agents and Pi, the drivers'
//! expected events as the core writes them, and settings and state files the core takes. Nothing
//! binary is committed: the nightly job writes these out with `examples/seeds.rs` before it
//! fuzzes, and `tests/seeds.rs` runs every check over them.

use marmot_agent::carrier::{self, Asked, AskedReply, Item};
use marmot_agent::{EventKey, PubKey};
use marmot_agent_events::AgentEvent;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A fuzz target's check.
pub type Check = fn(&[u8]);

/// Every target, with its check, by the name cargo-fuzz knows it by.
pub const TARGETS: [(&str, Check); 11] = [
    ("socket_lines", crate::socket_lines),
    ("socket_hello", crate::socket_hello),
    ("socket_connector", crate::socket_connector),
    ("carrier_request", crate::carrier_request),
    ("carrier_item", crate::carrier_item),
    ("settings", crate::settings),
    ("saved_state", crate::saved_state),
    ("codex", crate::codex),
    ("acp", crate::acp),
    ("pi", crate::pi),
    ("import", crate::import),
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// The seeds of the target named `target`.
pub fn of(target: &str) -> Vec<Vec<u8>> {
    match target {
        "socket_lines" => socket_messages(None)
            .into_iter()
            .map(|m| {
                let mut seed = vec![63, 7, 3, 31];
                seed.extend(m.into_bytes());
                seed.push(b'\n');
                seed
            })
            .collect(),
        "socket_hello" => socket_messages(Some("hello"))
            .into_iter()
            .chain([crate::hello().trim_end().to_string()])
            .map(String::into_bytes)
            // A raw newline inside the token. The check takes every newline out to make one
            // line, so what it checks is that line, not these bytes.
            .chain([crate::hello()
                .trim_end()
                .replacen(crate::TOKEN, &crate::TOKEN.replacen("0123", "0123\n", 1), 1)
                .into_bytes()])
            .collect(),
        "socket_connector" => {
            let all = socket_messages(None).join("\n") + "\n";
            let mut seeds: Vec<Vec<u8>> = (socket_messages(None).into_iter())
                .map(|m| (m + "\n").into_bytes())
                .collect();
            seeds.push(all.into_bytes());
            seeds
        }
        "carrier_request" => requests(),
        "carrier_item" => items(),
        "settings" => settings(),
        "saved_state" => vec![b"{}".to_vec(), saved()],
        "codex" => recordings(&["recordings/codex"]),
        "acp" => recordings(&[
            "recordings/acp/opencode",
            "recordings/acp/goose",
            "recordings/acp/cursor",
        ]),
        "pi" => recordings(&["recordings/pi"]),
        "import" => vec![
            b"MARMOT_ALLOWED_USERS=a,b\nexport MARMOT_ALLOW_ALL_USERS='true'\n# a note\nA=$'it\\'s'\n"
                .to_vec(),
            b"Environment=\"A=1\" B=2\nA=\"one \\\n two\"\n;comment\nexec /usr/bin/wn-agent --home \"/x y\"\n"
                .to_vec(),
        ],
        _ => Vec::new(),
    }
}

/// The socket's example messages from a connector, as the lines it writes, only those whose method
/// is `method` when one is given. A hello carries the fuzz targets' token.
fn socket_messages(method: Option<&str>) -> Vec<String> {
    let text = std::fs::read_to_string(fixtures().join("socket/examples.json")).expect("examples");
    let examples: Value = serde_json::from_str(&text).expect("JSON");
    (examples["examples"].as_array().expect("a list").iter())
        .filter(|e| e["from"] == "connector")
        .map(|e| e["message"].clone())
        .filter(|m| method.is_none_or(|method| m["method"] == method))
        .map(|mut m| {
            if m["method"] == "hello" {
                m["params"]["token"] = json!(crate::TOKEN);
            }
            m.to_string()
        })
        .collect()
}

/// Every event the drivers' expected files hold.
fn expected_events() -> Vec<AgentEvent> {
    let mut events = Vec::new();
    for folder in [
        "recordings/codex/expected",
        "recordings/claude/expected",
        "recordings/pi/expected",
        "recordings/acp/opencode/expected",
        "recordings/hermes/expected",
        "recordings/openclaw/expected",
    ] {
        for file in jsonl(&fixtures().join(folder)) {
            let text = std::fs::read_to_string(file).expect("a file");
            events.extend(text.lines().filter_map(|l| serde_json::from_str(l).ok()));
        }
    }
    events
}

fn jsonl(folder: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = (std::fs::read_dir(folder).expect("a folder"))
        .map(|e| e.expect("an entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    files
}

fn parts(kind: u64, content: &str, tags: &[Vec<String>]) -> Vec<u8> {
    json!({"kind": kind, "content": content, "tags": tags})
        .to_string()
        .into_bytes()
}

/// An event id, from a number.
fn id(n: u64) -> EventKey {
    EventKey(format!("{n:064x}"))
}

/// Each expected event as the core writes it, a first version and a later one.
fn items() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for (n, event) in expected_events().into_iter().enumerate() {
        let n = n as u64;
        for (version, first) in [(1, None), (2, Some(id(n)))] {
            let item = Item {
                event: event.clone(),
                version,
                first,
                started_by: Some(id(n + 1)),
                settles: vec![id(n + 2)],
                thread: None,
            };
            if let Ok(out) = carrier::item(&item) {
                seeds.push(parts(out.kind, &out.content, &out.tags));
            }
        }
    }
    seeds
}

/// An answer or a control of each action, as an app writes it, about an item and not.
fn requests() -> Vec<Vec<u8>> {
    let agent = PubKey::from(crate::AGENT);
    let asked = [
        Asked::Choice {
            choice: "once".into(),
        },
        Asked::Questions {
            questions: vec![AskedReply {
                question: "q".into(),
                options: vec!["a".into()],
                text: Some("t".into()),
            }],
        },
        Asked::Interrupt,
        Asked::NewSession,
        Asked::Set {
            setting: "model".into(),
            value: "m".into(),
        },
        Asked::Own {
            id: "compact".into(),
        },
    ];
    let mut seeds = Vec::new();
    for asked in &asked {
        for target in [None, Some(id(7))] {
            let out = carrier::request(&agent, target.as_ref(), asked, "words");
            seeds.push(parts(out.kind, &out.content, &out.tags));
        }
    }
    seeds
}

/// Settings of each kind the core takes.
fn settings() -> Vec<Vec<u8>> {
    let owner = crate::AGENT;
    let npub = "npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg";
    [
        format!(
            "home = \"/h\"\nrelays = [\"wss://relay.example\"]\nowner = \"{owner}\"\n\
             prompt = \"everyone\"\napprove = {{ people = [\"{npub}\"] }}\ninvite = [\"{owner}\"]\n\
             [codex]\ncwd = \"/w\"\nsandbox = \"workspace-write\"\napproval_policy = \"on-request\"\n"
        ),
        format!(
            "home = \"/h\"\nrelays = []\nowner = \"{}\"\nfull_activity = \"all\"\nfolders = [\"~/p\"]\n[connector]\n",
            owner.to_uppercase()
        ),
        format!(
            "home = \"/h\"\nrelays = []\nowner = \"{npub}\"\n[acp]\nname = \"opencode\"\ncommand = [\"opencode\", \"acp\"]\ncwd = \"/w\"\n"
        ),
        format!(
            "home = \"/h\"\nrelays = []\nowner = \"{owner}\"\n[pi]\ncwd = \"/w\"\nprogram = \"pi\"\nargs = []\n"
        ),
    ]
    .into_iter()
    .map(String::into_bytes)
    .collect()
}

/// A state file with a chat, an item, and a send queued.
fn saved() -> Vec<u8> {
    use marmot_agent::{Conversation, GroupKey, Heard, Rights, Where, Who};
    use marmot_agent_driver::{Capabilities, ChatId, Emitted, FilesIn, MidTurn, Sessions};
    let rights = Rights {
        owner: PubKey::from("owner"),
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
    };
    let saved = marmot_agent::Saved::default();
    let mut talk = Conversation::new(PubKey::from("agent"), "codex", rights, capabilities, saved);
    let acts = talk.heard(Heard {
        chat: Where {
            group: GroupKey::from("g"),
            name: None,
            direct: true,
            members: 2,
        },
        event: id(1),
        author: PubKey::from("owner"),
        author_name: "Owner".into(),
        text: "hello".into(),
        reply_to: None,
        mentions_agent: false,
        attachments: Vec::new(),
    });
    let chat = acts.into_iter().find_map(|a| match a {
        marmot_agent::Act::Prompt(p) => Some(p.chat.id),
        _ => None,
    });
    if let (Some(chat), Some(event)) = (chat, expected_events().into_iter().next()) {
        talk.emitted(Emitted {
            event,
            chat: Some(ChatId(chat.0)),
            prompts: Vec::new(),
            settles: Vec::new(),
        });
    }
    serde_json::to_vec(talk.saved()).expect("writes")
}

/// Each recording in `folders`, as the translator targets read one: a line for each message,
/// its first byte `1` for a message from the harness and `0` for one to it.
fn recordings(folders: &[&str]) -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for folder in folders {
        for file in jsonl(&fixtures().join(folder)) {
            let text = std::fs::read_to_string(file).expect("a recording");
            let mut seed = Vec::new();
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                let line: Value = serde_json::from_str(line).expect("a recorded line");
                if line["msg"].get("_recorder").is_some() {
                    continue;
                }
                seed.push(if line["dir"] == "in" { b'1' } else { b'0' });
                seed.extend(line["msg"].to_string().into_bytes());
                seed.push(b'\n');
            }
            seeds.push(seed);
        }
    }
    seeds
}
