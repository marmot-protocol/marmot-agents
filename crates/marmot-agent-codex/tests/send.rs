//! What the driver sends, checked against what the recorded client sent.
//!
//! Each recording is replayed. Before each message the client sent, the driver builds its own from
//! the same intent: the same text, the same choice, the same setting. The two must be the same
//! JSON, apart from what the recorder chose and the driver does not send, which each case names.

use marmot_agent_codex::{
    ApprovalPolicy, Direction, Sandbox, Setup, Translator, collaboration_mode_list, initialize,
    initialized, model_list, send_file_tool, tool_reply,
};
use marmot_agent_driver::{
    Action, Answer, Attachment, Chat, Given, Person, Prompt, Reason, Refusal, Reply,
};
use marmot_agent_events::body::Effect;
use marmot_agent_events::{AgentEvent, Body, State};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
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
            folder: None,
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
        let mut started_params = json!({});
        // Codex's calls of the driver's own tool, by Codex's id for them.
        let mut tool_calls: HashSet<i64> = HashSet::new();
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
            if direction == Direction::In && recorded["method"] == "item/tool/call" {
                tool_calls.insert(recorded["id"].as_i64().expect("a numbered call"));
            }
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
                        started_params = params.clone();
                        // The recorder's sessions were throwaway; the driver's are kept (D-063).
                        // `config` added the recorder's demo MCP server. Every session the
                        // driver starts has its own tool (D-090); only one recording gave it.
                        let mut expected = drop_keys(recorded.clone(), &["config"]);
                        expected["params"]["ephemeral"] = json!(false);
                        if expected["params"].get("dynamicTools").is_none() {
                            expected["params"]["dynamicTools"] = json!([send_file_tool()]);
                        }
                        Some((json(&built), expected))
                    }
                    Some("thread/resume") => {
                        // The driver sends the session's setup again on every resume (rule 10).
                        // Two recordings resumed without it, and the sandbox came back wider.
                        let setup = setup(&params, started.as_ref());
                        let thread = params["threadId"].as_str().unwrap();
                        let built = translator.pick(id, Some(thread), &setup).expect("a resume");
                        // Expected: what the recording resent, or else what it started with.
                        let mut expected = recorded.clone();
                        for key in ["approvalPolicy", "sandbox"] {
                            if expected["params"].get(key).is_none() {
                                expected["params"][key] = started_params[key].clone();
                            }
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
                        // and the sandbox with a field the session item does not show. Each setting
                        // is built alone and must be exactly the recorded one.
                        let shown = [
                            ("model", "model", None),
                            ("approvalPolicy", "approval_policy", None),
                            ("sandboxPolicy", "sandbox", Some("type")),
                        ];
                        for (key, setting, inner) in shown {
                            let Some(value) = params.get(key) else {
                                continue;
                            };
                            let value = inner.map_or(value, |k| &value[k]).clone();
                            let action = Action::Set {
                                session: session.clone().expect("a session").into(),
                                setting: setting.into(),
                                value: value.as_str().unwrap().into(),
                            };
                            let built = translator.control(id, &action, &Setup::new("/w"));
                            let recorded_value = match inner {
                                Some(k) => json!({ k: value }),
                                None => value,
                            };
                            let expected = json!({"id": id, "method": "thread/settings/update", "params": {"threadId": params["threadId"], key: recorded_value}});
                            assert_eq!(json(&built.expect("a setting")), expected, "{name}");
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
                    None if recorded["id"]
                        .as_i64()
                        .is_some_and(|id| tool_calls.remove(&id)) =>
                    {
                        // The driver's reply to a call of its own tool: what it says is the
                        // core's words, not the recorder's, so only whether it worked must match.
                        let sent = recorded["result"]["success"] == true;
                        let result = if sent {
                            Ok(())
                        } else {
                            Err(Refusal::new(Reason::NotAllowed))
                        };
                        let built = json(&tool_reply(&recorded["id"], &result));
                        assert_eq!(built["id"], recorded["id"], "{name}");
                        assert_eq!(built["result"]["success"], sent, "{name}");
                        assert_eq!(built["result"]["contentItems"][0]["type"], "inputText");
                        *checked.entry("a tool reply".into()).or_default() += 1;
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
        ("a tool reply", 3),
        ("collaborationMode/list", 1),
        ("initialize", 33),
        ("initialized", 33),
        ("model/list", 2),
        ("thread/compact/start", 1),
        ("thread/resume", 4),
        ("thread/settings/update", 2),
        ("thread/start", 30),
        ("turn/interrupt", 2),
        ("turn/start", 38),
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

// No recording holds this reply; its shape is from Codex's schema (0.169.0).
#[test]
fn a_permission_grant_is_answered_in_codexs_shape() {
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
}

#[test]
fn a_session_starts_and_resumes_narrow_unless_configured() {
    // D-061: writes only inside the working folder, and Codex asks before anything outside it.
    let setup = Setup::new("/w");
    let start = json(&Translator::new().pick(1, None, &setup).unwrap());
    let resume = json(&Translator::new().pick(1, Some("th1"), &setup).unwrap());
    for built in [&start, &resume] {
        assert_eq!(built["params"]["sandbox"], "workspace-write");
        assert_eq!(built["params"]["approvalPolicy"], "on-request");
        assert_eq!(built["params"]["cwd"], "/w");
    }
    assert_eq!(start["params"]["ephemeral"], false, "D-063");
}

fn started(translator: &mut Translator) {
    translator.feed(
        Direction::Out,
        r#"{"id": 1, "method": "thread/start", "params": {}}"#,
    );
    translator.feed(
        Direction::In,
        r#"{"id": 1, "result": {"thread": {"id": "th1"}}}"#,
    );
}

#[test]
fn a_turn_asked_for_and_not_started_keeps_the_session() {
    let setup = Setup::new("/w");
    let mut translator = Translator::new();
    started(&mut translator);
    let turn = translator.prompt(2, &prompt("hello", false)).unwrap();
    translator.feed(Direction::Out, &turn);
    assert_eq!(
        translator.pick(3, None, &setup).unwrap_err().reason,
        Reason::Busy
    );
}

#[test]
fn a_prompt_needs_its_own_session() {
    let mut translator = Translator::new();
    let refused = translator.prompt(2, &prompt("hello", false)).unwrap_err();
    assert_eq!(refused.reason, Reason::Busy, "no session has been picked");
    started(&mut translator);
    let mut elsewhere = prompt("hello", false);
    elsewhere.session = Some("th0".into());
    let refused = translator.prompt(2, &elsewhere).unwrap_err();
    assert_eq!(
        refused.reason,
        Reason::UnknownItem,
        "the driver resumes th0 first"
    );
}

#[test]
fn an_approval_closed_at_the_turns_end_takes_no_answer() {
    let mut translator = Translator::new();
    let asked = translator.feed(
        Direction::In,
        r#"{"id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "ls"}}"#,
    );
    translator.feed(
        Direction::In,
        r#"{"method": "turn/completed", "params": {"threadId": "th1", "turn": {"id": "t1", "items": [], "status": "interrupted", "error": null}}}"#,
    );
    let late = translator.reply(&answer(&asked[0], Given::Choice("c0".into())));
    assert_eq!(late.unwrap_err().reason, Reason::NotWaiting);
}

// Codex resolves a request itself, as it does on an interrupt (`approval_unanswered`: the turn
// completes, and the request is marked resolved), and says so with `serverRequest/resolved`. An
// answer after that would reach a Codex that no longer waits for it, whatever came first.
#[test]
fn an_approval_codex_resolved_takes_no_answer() {
    let mut translator = Translator::new();
    let asked = translator.feed(
        Direction::In,
        r#"{"id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "ls"}}"#,
    );
    let resolved = translator.feed(
        Direction::In,
        r#"{"method": "serverRequest/resolved", "params": {"threadId": "th1", "requestId": 0}}"#,
    );
    assert!(resolved.is_empty(), "nothing new reaches the chat");
    let late = translator.reply(&answer(&asked[0], Given::Choice("c0".into())));
    assert_eq!(late.unwrap_err().reason, Reason::NotWaiting);
}

fn turn_started(translator: &mut Translator) {
    translator.feed(
        Direction::In,
        r#"{"method": "turn/started", "params": {"threadId": "th1", "turn": {"id": "t1", "items": [], "status": "inProgress", "error": null}}}"#,
    );
}

// While a request that starts a turn or picks the session waits for Codex, only replies and
// interrupts are built; everything else is busy until Codex answers.
#[test]
fn nothing_but_an_interrupt_is_built_while_codex_has_yet_to_answer() {
    let setup = Setup::new("/w");
    let compact = Action::Own {
        session: "th1".into(),
        id: "compact".into(),
    };
    let model = Action::Set {
        session: "th1".into(),
        setting: "model".into(),
        value: "m2".into(),
    };
    let busy = |translator: &Translator| {
        for refused in [
            translator.prompt(9, &prompt("and this", false)),
            translator.pick(9, None, &setup),
            translator.control(9, &compact, &setup),
            translator.control(9, &model, &setup),
        ] {
            assert_eq!(refused.unwrap_err().reason, Reason::Busy);
        }
    };

    let mut translator = Translator::new();
    started(&mut translator);
    let first = translator.prompt(2, &prompt("hello", false)).unwrap();
    translator.feed(Direction::Out, &first);
    busy(&translator); // the turn was asked for and has not started
    turn_started(&mut translator);
    let second = json(&translator.prompt(3, &prompt("and this", false)).unwrap());
    assert_eq!(
        second["method"], "turn/steer",
        "once it runs, a message joins it"
    );

    let mut translator = Translator::new();
    started(&mut translator);
    let compaction = translator.control(2, &compact, &setup).unwrap();
    translator.feed(Direction::Out, &compaction);
    busy(&translator); // a compaction starts a turn too

    let mut translator = Translator::new();
    started(&mut translator);
    let pick = translator.pick(2, None, &setup).unwrap();
    translator.feed(Direction::Out, &pick);
    busy(&translator); // a new session is on its way
}

#[test]
fn a_second_answer_is_told_it_was_already_answered() {
    let mut translator = Translator::new();
    let asked = translator.feed(
        Direction::In,
        r#"{"id": 0, "method": "item/commandExecution/requestApproval", "params": {"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "ls"}}"#,
    );
    let allow = answer(&asked[0], Given::Choice("c0".into()));
    let reply = translator.reply(&allow).unwrap();
    translator.feed(Direction::Out, &reply);
    assert_eq!(
        translator.reply(&allow).unwrap_err().reason,
        Reason::AlreadyAnswered
    );
}

#[test]
fn an_mcp_form_is_declined_at_once() {
    let form = r#"{"id": 5, "method": "mcpServer/elicitation/request", "params": {"threadId": "th1", "turnId": "t1", "serverName": "demo", "mode": "form", "message": "Upload", "requestedSchema": {"type": "object", "properties": {"city": {"type": "string"}}}}}"#;
    let mut translator = Translator::new();
    let events = translator.feed(Direction::In, form);
    assert!(
        matches!(events[0].body(), Body::Notice(_)),
        "it is shown, not asked"
    );
    let replies = translator.take_replies();
    assert_eq!(
        replies.iter().map(|r| json(r)).collect::<Vec<_>>(),
        [json!({"id": 5, "result": {"action": "decline", "content": null}})]
    );
    assert!(translator.take_replies().is_empty());

    // A decline still waiting to be sent is dropped when Codex stops: nothing is left to answer.
    translator.feed(Direction::In, form);
    translator.harness_stopped();
    assert!(translator.take_replies().is_empty());
}

fn command_approval(decisions: &str) -> String {
    format!(
        r#"{{"id": 0, "method": "item/commandExecution/requestApproval", "params": {{"threadId": "th1", "turnId": "t1", "itemId": "c", "command": "curl example.org", "availableDecisions": {decisions}}}}}"#
    )
}

// A choice says what it does only when the driver knows the decision: a lasting network rule allows
// or denies by its own action, and a decision of a kind the driver does not know is not offered.
#[test]
fn a_choice_does_what_its_decision_does() {
    let decisions = r#"["accept", {"applyNetworkPolicyAmendment": {"network_policy_amendment": {"action": "allow", "host": "example.org"}}}, {"applyNetworkPolicyAmendment": {"network_policy_amendment": {"action": "deny", "host": "example.org"}}}, {"somethingNew": {}}, "cancel"]"#;
    let mut translator = Translator::new();
    let events = translator.feed(Direction::In, &command_approval(decisions));
    let Body::Approval(approval) = events[0].body() else {
        panic!("an approval");
    };
    let shown: Vec<(&str, &str, Effect)> = (approval.choices.iter())
        .map(|c| (c.id.as_str(), c.label.as_str(), c.effect))
        .collect();
    assert_eq!(
        shown,
        [
            ("c0", "Allow once", Effect::Allow),
            ("c1", "Always allow example.org", Effect::Allow),
            ("c2", "Always deny example.org", Effect::Deny),
            ("c4", "Deny and stop", Effect::Deny),
            ("c5", "Deny", Effect::Deny),
        ]
    );
    // The decision not offered cannot be chosen.
    let unknown = translator.reply(&answer(&events[0], Given::Choice("c3".into())));
    assert_eq!(unknown.unwrap_err().reason, Reason::Invalid);
    let allow = translator
        .reply(&answer(&events[0], Given::Choice("c1".into())))
        .unwrap();
    let offered = json(&command_approval(decisions))["params"]["availableDecisions"][1].clone();
    assert_eq!(json(&allow)["result"], json!({"decision": offered}));
}

#[test]
fn a_setting_is_waited_for_until_codex_answers_and_holds_from_then() {
    let setup = Setup::new("/w");
    let set = |setting: &str, value: &str| Action::Set {
        session: "th1".into(),
        setting: setting.into(),
        value: value.into(),
    };
    let mut translator = Translator::new();
    translator.feed(
        Direction::Out,
        r#"{"id": 1, "method": "thread/start", "params": {}}"#,
    );
    translator.feed(
        Direction::In,
        r#"{"id": 1, "result": {"thread": {"id": "th1"}, "model": "m1", "reasoningEffort": "low"}}"#,
    );
    let change = translator.control(2, &set("model", "m2"), &setup).unwrap();
    translator.feed(Direction::Out, &change);
    // Codex has not answered: what it runs on is not known.
    let refused = translator
        .control(3, &set("mode", "plan"), &setup)
        .unwrap_err();
    assert_eq!(refused.reason, Reason::Busy);
    // Codex took it. A mode built now carries the new model, though Codex has not reported it.
    translator.feed(Direction::In, r#"{"id": 2, "result": {}}"#);
    let mode = json(&translator.control(3, &set("mode", "plan"), &setup).unwrap());
    let settings = &mode["params"]["collaborationMode"]["settings"];
    assert_eq!(
        (
            settings["model"].as_str(),
            settings["reasoning_effort"].as_str()
        ),
        (Some("m2"), Some("low"))
    );

    // A refused change is not waited for, and does not hold.
    let change = translator.control(4, &set("model", "m3"), &setup).unwrap();
    translator.feed(Direction::Out, &change);
    translator.feed(
        Direction::In,
        r#"{"id": 4, "error": {"code": -32600, "message": "no"}}"#,
    );
    let mode = json(&translator.control(5, &set("mode", "plan"), &setup).unwrap());
    assert_eq!(
        mode["params"]["collaborationMode"]["settings"]["model"],
        "m2"
    );
}

#[test]
fn a_new_session_asked_for_an_old_one_is_out_of_date() {
    let setup = Setup::new("/w");
    let new_session = |session: Option<&str>| Action::NewSession {
        chat: Chat {
            id: "c-1".into(),
            group: false,
            name: None,
            folder: None,
        },
        session: session.map(Into::into),
    };
    let mut translator = Translator::new();
    // A driver started again has no thread yet, and the core names the chat's saved one: with no
    // thread running there is nothing for the request to be out of date with (D-101).
    assert!(
        Translator::new()
            .control(1, &new_session(Some("th-saved")), &setup)
            .is_ok()
    );
    assert!(translator.control(1, &new_session(None), &setup).is_ok());
    started(&mut translator);
    let stale = translator
        .control(2, &new_session(Some("th0")), &setup)
        .unwrap_err();
    assert_eq!(stale.reason, Reason::UnknownItem);
    let none = translator
        .control(2, &new_session(None), &setup)
        .unwrap_err();
    assert_eq!(
        none.reason,
        Reason::UnknownItem,
        "the chat has a session now"
    );
    assert!(
        translator
            .control(2, &new_session(Some("th1")), &setup)
            .is_ok()
    );
}
