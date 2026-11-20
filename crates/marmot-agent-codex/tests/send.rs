//! What the driver sends, checked against what the recorded client sent.
//!
//! Each recording is replayed. Before each message the client sent, the driver builds its own from
//! the same intent: the same text, the same choice, the same setting. The two must be the same
//! JSON, apart from what the recorder chose and the driver does not send, which each case names.

use marmot_agent_codex::{
    ApprovalPolicy, Direction, Sandbox, Setup, Translator, collaboration_mode_list, initialize,
    initialized, model_list,
};
use marmot_agent_driver::{Action, Answer, Attachment, Chat, Given, Person, Prompt, Reason, Reply};
use marmot_agent_events::{AgentEvent, Body, State};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Line<'a> {
    dir: String,
    #[serde(borrow)]
    msg: &'a RawValue,
}

fn recordings() -> Vec<PathBuf> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/recordings/codex");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)
        .expect("the recordings folder")
        .map(|entry| entry.expect("a file").path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    paths.sort();
    paths
}

fn alice() -> Person {
    Person {
        id: "u-1".into(),
        name: "Alice".into(),
    }
}

fn prompt(text: &str, group: bool) -> Prompt {
    Prompt {
        message: "m-1".into(),
        text: text.into(),
        chat: Chat {
            id: "c-1".into(),
            group,
            name: None,
        },
        sender: alice(),
        reply_to: None,
        attachments: Vec::new(),
        session: None,
    }
}

fn setup(params: &Value, fallback: Option<&Setup>) -> Setup {
    let word = |key: &str| params.get(key).and_then(Value::as_str);
    let mut setup = fallback.cloned().unwrap_or_else(|| Setup::new(""));
    setup.cwd = word("cwd").expect("a working folder").to_string();
    if let Some(sandbox) = word("sandbox") {
        setup.sandbox = match sandbox {
            "read-only" => Sandbox::ReadOnly,
            "workspace-write" => Sandbox::WorkspaceWrite,
            "danger-full-access" => Sandbox::DangerFullAccess,
            other => panic!("a sandbox {other}"),
        };
    }
    if let Some(policy) = word("approvalPolicy") {
        setup.approval_policy = match policy {
            "untrusted" => ApprovalPolicy::Untrusted,
            "on-failure" => ApprovalPolicy::OnFailure,
            "on-request" => ApprovalPolicy::OnRequest,
            "never" => ApprovalPolicy::Never,
            other => panic!("an approval policy {other}"),
        };
    }
    setup
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("JSON")
}

fn drop_keys(mut value: Value, keys: &[&str]) -> Value {
    if let Some(params) = value.get_mut("params").and_then(Value::as_object_mut) {
        for key in keys {
            params.remove(*key);
        }
    }
    value
}

/// Whether everything in `part` is in `whole`, with the same value.
fn within(part: &Value, whole: &Value) -> bool {
    match (part, whole) {
        (Value::Object(p), Value::Object(w)) => p
            .iter()
            .all(|(k, v)| w.get(k).is_some_and(|x| within(v, x))),
        _ => part == whole,
    }
}

/// The item waiting for the reply to Codex's request `id`, from the events so far.
fn waiting_for<'a>(events: &'a [AgentEvent], id: &Value) -> Option<&'a AgentEvent> {
    let suffix = format!(":{id}");
    events.iter().rev().find(|e| {
        e.header().item.as_str().ends_with(&suffix) && matches!(e.header().state, State::Waiting)
    })
}

fn answer(item: &AgentEvent, given: Given) -> Answer {
    Answer {
        request: "r-1".into(),
        item: item.header().item.clone(),
        by: alice(),
        given,
    }
}

/// Every way the driver could answer the item, as the text it would send.
fn replies(translator: &Translator, item: &AgentEvent, recorded: &Value) -> Vec<String> {
    match item.body() {
        Body::Approval(approval) => approval
            .choices
            .iter()
            .filter_map(|c| {
                translator
                    .reply(&answer(item, Given::Choice(c.id.clone())))
                    .ok()
            })
            .collect(),
        Body::Question(question) => {
            // Codex's own questions are answered with labels; turn them back into options.
            let said = &recorded["result"]["answers"];
            let replies = question
                .questions()
                .iter()
                .map(|q| {
                    let labels: Vec<&str> = said[&q.id]["answers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    let options = (q.options.iter())
                        .filter(|o| labels.contains(&o.label.as_str()))
                        .map(|o| o.id.clone())
                        .collect();
                    Reply {
                        question: q.id.clone(),
                        options,
                        text: None,
                    }
                })
                .collect();
            let given = Given::Questions(replies);
            translator.reply(&answer(item, given)).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

#[test]
fn the_driver_sends_what_the_recorded_client_sent() {
    let mut checked: BTreeMap<String, usize> = BTreeMap::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    for path in recordings() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).expect("a recording");
        let mut translator = Translator::new();
        let mut events: Vec<AgentEvent> = Vec::new();
        let mut started: Option<Setup> = None;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let line: Line<'_> = serde_json::from_str(line).expect("a recorded line");
            let message = line.msg.get();
            let recorded = json(message);
            if let Some(note) = recorded.get("_recorder").and_then(Value::as_str) {
                if note.contains("new one started") {
                    events.extend(translator.harness_stopped());
                }
                continue;
            }
            let direction = if line.dir == "out" {
                Direction::Out
            } else {
                Direction::In
            };
            if direction == Direction::Out {
                let id = recorded.get("id").and_then(Value::as_i64).unwrap_or(-1);
                let params = recorded.get("params").cloned().unwrap_or(json!({}));
                let method = recorded.get("method").and_then(Value::as_str);
                let session = translator_session(&events);
                // (what the driver builds, what the recorder sent less what the driver leaves out)
                let pair: Option<(Value, Value)> = match method {
                    Some("initialize") => Some((
                        drop_keys(json(&initialize(id)), &["clientInfo"]),
                        drop_keys(recorded.clone(), &["clientInfo"]),
                    )),
                    Some("initialized") => Some((json(&initialized()), recorded.clone())),
                    Some("model/list") => Some((json(&model_list(id)), recorded.clone())),
                    Some("collaborationMode/list") => {
                        Some((json(&collaboration_mode_list(id)), recorded.clone()))
                    }
                    Some("thread/start") => {
                        let setup = setup(&params, None);
                        let built = translator.pick(id, None, &setup).expect("a new session");
                        started = Some(setup);
                        // The recorder's sessions were throwaway; the driver's are kept (D-063).
                        // `config` added the recorder's demo MCP server.
                        let mut expected = drop_keys(recorded.clone(), &["config"]);
                        expected["params"]["ephemeral"] = json!(false);
                        Some((json(&built), expected))
                    }
                    Some("thread/resume") => {
                        // The driver sends the session's setup again on every resume (rule 10).
                        // Two recordings resumed without it, and the sandbox came back wider.
                        let setup = setup(&params, started.as_ref());
                        let thread = params["threadId"].as_str().unwrap();
                        let built = translator.pick(id, Some(thread), &setup).expect("a resume");
                        let mut expected = recorded.clone();
                        let resent = json(&built)["params"].clone();
                        for key in ["approvalPolicy", "sandbox"] {
                            expected["params"][key] = resent[key].clone();
                        }
                        Some((json(&built), expected))
                    }
                    Some(start @ ("turn/start" | "turn/steer")) => {
                        let input = params["input"].as_array().unwrap();
                        let kind = |k: &'static str| input.iter().filter(move |i| i["type"] == k);
                        let said = kind("text").next().unwrap()["text"].as_str().unwrap();
                        let mut asked = prompt(said, false);
                        for image in kind("localImage") {
                            asked.attachments.push(Attachment {
                                path: image["path"].as_str().unwrap().into(),
                                name: "image".into(),
                                media_type: "image/png".into(),
                            });
                        }
                        let built = translator.prompt(id, &asked).expect("a prompt");
                        assert_eq!(
                            json(&built)["method"],
                            start,
                            "{name}: a prompt during a turn steers it"
                        );
                        // The recorder set the model, mode, effort and summary on each turn; the
                        // driver changes settings with their own request. It also put an image
                        // before the text; the driver puts the text first.
                        let leave = ["effort", "model", "collaborationMode", "summary"];
                        let mut expected = drop_keys(recorded.clone(), &leave);
                        let ordered: Vec<Value> =
                            kind("text").chain(kind("localImage")).cloned().collect();
                        expected["params"]["input"] = ordered.into();
                        Some((json(&built), expected))
                    }
                    Some("turn/interrupt") => {
                        let turn = params["turnId"].as_str().unwrap();
                        let action = Action::Interrupt { turn: turn.into() };
                        let built = translator.control(id, &action, &Setup::new("/w"));
                        Some((json(&built.expect("an interrupt")), recorded.clone()))
                    }
                    Some("thread/compact/start") => {
                        let action = Action::Own {
                            session: session.clone().expect("a session").into(),
                            id: "compact".into(),
                        };
                        let built = translator.control(id, &action, &Setup::new("/w"));
                        Some((json(&built.expect("a compaction")), recorded.clone()))
                    }
                    Some("thread/settings/update") => {
                        // The driver changes one setting a request; the recorder sent two at once,
                        // and one more field than the session item shows. Each must be within it.
                        let shown = [
                            ("model", "model", None),
                            ("approvalPolicy", "approval_policy", None),
                            ("sandboxPolicy", "sandbox", Some("type")),
                        ];
                        for (key, setting, inner) in shown {
                            let Some(value) = params.get(key) else {
                                continue;
                            };
                            let value = inner.map_or(value, |k| &value[k]).as_str().unwrap();
                            let action = Action::Set {
                                session: session.clone().expect("a session").into(),
                                setting: setting.into(),
                                value: value.into(),
                            };
                            let built = translator.control(id, &action, &Setup::new("/w"));
                            let built = json(&built.expect("a setting"));
                            assert!(
                                within(&built, &recorded),
                                "{name}: {built} is not within {recorded}"
                            );
                        }
                        *checked.entry("thread/settings/update".into()).or_default() += 1;
                        None
                    }
                    Some(other) => {
                        // `turn/settings/update` needs a feature that is off, and Codex refused it;
                        // `thread/unsubscribe` was the recorder's own.
                        *skipped.entry(other.to_string()).or_default() += 1;
                        None
                    }
                    None => {
                        let item = waiting_for(&events, &recorded["id"])
                            .unwrap_or_else(|| panic!("{name}: a reply to nothing waiting"));
                        let built = replies(&translator, item, &recorded);
                        assert!(
                            built.iter().any(|b| json(b) == recorded),
                            "{name}: no answer to {} gives {recorded}; the driver gives {built:?}",
                            item.header().item.as_str()
                        );
                        *checked.entry("a reply".into()).or_default() += 1;
                        None
                    }
                };
                if let Some((built, expected)) = pair {
                    assert_eq!(built, expected, "{name}");
                    *checked.entry(method.unwrap().to_string()).or_default() += 1;
                }
            }
            events.extend(translator.feed(direction, message));
        }
    }
    // Every kind the recorded client sent, and how often: the counts change only with the
    // recordings.
    let expected = [
        ("a reply", 7),
        ("collaborationMode/list", 1),
        ("initialize", 31),
        ("initialized", 31),
        ("model/list", 2),
        ("thread/compact/start", 1),
        ("thread/resume", 3),
        ("thread/settings/update", 2),
        ("thread/start", 29),
        ("turn/interrupt", 2),
        ("turn/start", 36),
        ("turn/steer", 1),
    ];
    let checked: Vec<(&str, usize)> = checked.iter().map(|(k, n)| (k.as_str(), *n)).collect();
    assert_eq!(checked, expected);
    let skipped: Vec<(&str, usize)> = skipped.iter().map(|(k, n)| (k.as_str(), *n)).collect();
    assert_eq!(
        skipped,
        [("thread/unsubscribe", 1), ("turn/settings/update", 1)]
    );
}

/// The session the events so far are in.
fn translator_session(events: &[AgentEvent]) -> Option<String> {
    let last = events.iter().rev().find(|e| e.header().session.is_some())?;
    last.header()
        .session
        .as_ref()
        .map(|s| s.as_str().to_string())
}

// Codex offers some decisions as objects. Every recorded command approval offers one, and no
// recorded reply chose it, so the replay above would pass a driver that sent a plain word. The
// reply must carry the whole object Codex offered, under the id of the request it answers.
#[test]
fn a_decision_codex_offers_as_an_object_is_sent_back_whole() {
    let request = r#"{"id": 3, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "python3 greet.py", "availableDecisions": ["accept", {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["python3", "greet.py"]}}, "cancel"]}}"#;
    let mut translator = Translator::new();
    let events = translator.feed(Direction::In, request);
    let Body::Approval(approval) = events[0].body() else {
        panic!("an approval");
    };
    let choice = (approval.choices.iter())
        .find(|c| c.label == "Allow, and don't ask again for this command")
        .expect("the object's choice");
    let reply = translator
        .reply(&answer(&events[0], Given::Choice(choice.id.clone())))
        .expect("a reply");
    let offered = json(request)["params"]["availableDecisions"][1].clone();
    assert_eq!(
        json(&reply),
        json!({"id": 3, "result": {"decision": offered}})
    );
}

#[test]
fn a_message_names_its_sender_in_a_group_and_quotes_what_it_replies_to() {
    let mut translator = Translator::new();
    translator.feed(
        Direction::Out,
        r#"{"id": 1, "method": "thread/start", "params": {}}"#,
    );
    translator.feed(
        Direction::In,
        r#"{"id": 1, "result": {"thread": {"id": "th1"}}}"#,
    );
    let text = |prompt: &Prompt| {
        let built = json(&translator.prompt(2, prompt).expect("a prompt"));
        built["params"]["input"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(text(&prompt("run the tests", false)), "run the tests");
    assert_eq!(text(&prompt("run the tests", true)), "Alice: run the tests");
    let mut reply = prompt("yes, that one", true);
    reply.reply_to = Some(marmot_agent_driver::Quoted {
        message: "m-0".into(),
        text: "Which file?\nThe first or the second?".into(),
        author: Person {
            id: "u-2".into(),
            name: "Bob".into(),
        },
    });
    assert_eq!(
        text(&reply),
        "> Bob: Which file?\n> The first or the second?\n\nAlice: yes, that one"
    );
    // A slash command goes exactly as typed (D-031).
    assert_eq!(text(&prompt("/compact", true)), "/compact");

    let mut image = prompt("what is this?", false);
    image.attachments.push(Attachment {
        path: "/tmp/a.png".into(),
        name: "a.png".into(),
        media_type: "image/png".into(),
    });
    let built = json(&translator.prompt(2, &image).unwrap());
    assert_eq!(
        built["params"]["input"][1],
        json!({"type": "localImage", "path": "/tmp/a.png"})
    );
    image.attachments[0].media_type = "application/pdf".into();
    let refused = translator.prompt(2, &image).unwrap_err();
    assert_eq!(refused.reason, Reason::NotSupported);
}

#[test]
fn a_session_is_replaced_only_while_idle_and_once_at_a_time() {
    let setup = Setup::new("/w");
    let mut translator = Translator::new();
    let start = translator.pick(1, None, &setup).unwrap();
    translator.feed(Direction::Out, &start);
    // Its reply has not come: a second pick waits.
    assert_eq!(
        translator.pick(2, None, &setup).unwrap_err().reason,
        Reason::Busy
    );
    translator.feed(
        Direction::In,
        r#"{"id": 1, "result": {"thread": {"id": "th1"}}}"#,
    );
    translator.feed(
        Direction::In,
        r#"{"method": "turn/started", "params": {"threadId": "th1", "turn": {"id": "t1", "items": [], "status": "inProgress", "error": null}}}"#,
    );
    // A turn is running: the session is not idle.
    assert_eq!(
        translator.pick(3, Some("th0"), &setup).unwrap_err().reason,
        Reason::Busy
    );
}

// No recording holds these replies; their shapes are from Codex's schema (0.169.0).
#[test]
fn a_permission_grant_and_a_form_are_answered_in_codexs_shapes() {
    let asked = r#"{"id": 9, "method": "item/permissions/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "call9", "cwd": "/w", "reason": "needs the network", "permissions": {"network": {"enabled": true}}}}"#;
    let mut translator = Translator::new();
    let events = translator.feed(Direction::In, asked);
    let reply = |choice: &str| {
        let given = Given::Choice(choice.into());
        json(&translator.reply(&answer(&events[0], given)).unwrap())["result"].clone()
    };
    let wanted = json!({"network": {"enabled": true}});
    assert_eq!(reply("c0"), json!({"permissions": wanted, "scope": "turn"}));
    assert_eq!(
        reply("c1"),
        json!({"permissions": wanted, "scope": "session"})
    );
    assert_eq!(reply("c2"), json!({"permissions": {}}));
    let wrong = translator.reply(&answer(&events[0], Given::Choice("c9".into())));
    assert_eq!(wrong.unwrap_err().reason, Reason::Invalid);

    let form = r#"{"id": 4, "method": "mcpServer/elicitation/request", "params": {"threadId": "th1", "turnId": "t1", "serverName": "demo", "mode": "form", "message": "Your details", "requestedSchema": {"type": "object", "properties": {"city": {"type": "string", "title": "City"}}}}}"#;
    let mut translator = Translator::new();
    let events = translator.feed(Direction::In, form);
    let given = Given::Questions(vec![Reply {
        question: "city".into(),
        options: Vec::new(),
        text: Some("Lisbon".into()),
    }]);
    let built = json(&translator.reply(&answer(&events[0], given)).unwrap());
    assert_eq!(
        built,
        json!({"id": 4, "result": {"action": "accept", "content": {"city": "Lisbon"}}})
    );
}
