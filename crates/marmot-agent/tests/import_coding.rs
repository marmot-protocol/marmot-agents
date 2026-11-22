//! Moving today's coding connectors over without being invited again (D-089, M4): Claude Code,
//! OpenCode, Pi and Goose, each as `install-terminal-harness-marmot.sh` sets it up. Today's
//! connector itself, MDK's `agent-connector` that is `wn-agent`, makes the home, and a person
//! invites it and writes to it there; the files the installer writes are put around it. Then it
//! is stopped as systemd stops it, the import backs the home up and writes the settings, and the
//! core program, `marmot-agent run` on those settings, takes the home over in place: the person's
//! next message reaches the harness and is answered in the same chat, by the same agent key, and
//! nothing written before the move is read as a prompt.
//!
//! The core runs with the harness's own driver, from the settings the import wrote, on a
//! stand-in for the harness, since each real one needs a model or an account:
//!
//! - OpenCode and Goose: the ACP driver, on `stand_in_acp.py`, an agent over ACP that answers
//!   "ok: " and the prompt;
//! - Pi: the Pi driver, on its own tests' `fake_pi.py`, which answers the same way;
//! - Claude Code: the core serves its socket, and a stand-in for the Claude Code driver
//!   (`packages/claude-code`) connects to it as that driver does, with its hello, and answers
//!   the same way. The real driver needs Node, its packages and a signed-in Claude Code, and
//!   runs in `claude.rs`.
//!
//! Each stand-in sits where the installer found the harness's program, which is what the
//! import carries over.

mod common;
mod old_connector;

use common::within;
use marmot_account::AccountHome;
use marmot_agent::config::Config;
use marmot_agent::import::{self, Connector, Options};
use marmot_agent::rights::Who;
use marmot_agent::socket::PROTOCOL;
use old_connector::{
    BEFORE, CLAUDE, GOOSE, Invited, Noted, OPENCODE, PI, Terminal, chat_messages, installed,
    invited, places,
};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Today's connector, as its own program: run by `old_connector::invited`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run by the import test, as the old connector"]
async fn old_connector() {
    old_connector::serve().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_imported_claude_code_install_answers_in_its_chat() {
    moved(CLAUDE, Connector::Claude).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_imported_opencode_install_answers_in_its_chat() {
    moved(OPENCODE, Connector::OpenCode).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_imported_pi_install_answers_in_its_chat() {
    moved(PI, Connector::Pi).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_imported_goose_install_answers_in_its_chat() {
    moved(GOOSE, Connector::Goose).await;
}

/// What the person writes after the move.
const AFTER: &str = "after the move";

/// The harness's program where the installer found it, in the person's own `~/.local/bin`: a
/// stand-in for the harness.
fn stand_in(root: &Path, connector: Connector) -> PathBuf {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let acp = tests.join("stand_in_acp.py");
    let pi = tests.join("../../marmot-agent-pi/tests/fake_pi.py");
    let quoted = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', r"'\''"));
    let (name, body) = match connector {
        Connector::OpenCode => (
            "opencode",
            format!("exec python3 {} opencode \"$@\"", quoted(&acp)),
        ),
        Connector::Goose => (
            "goose",
            format!("exec python3 {} goose \"$@\"", quoted(&acp)),
        ),
        Connector::Pi => ("pi", format!("exec python3 {} \"$@\"", quoted(&pi))),
        // After the move the Claude Code driver runs Claude Code, never the core.
        Connector::Claude => ("claude", "exit 1".into()),
        other => panic!("{other:?} is not a coding connector"),
    };
    let bin = root.join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join(name);
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

/// The prompts the stand-in harness heard, in the working folder, oldest first.
fn heard(work: &Path, connector: Connector) -> Vec<String> {
    let file = match connector {
        Connector::Pi => ".fake-prompts",
        _ => ".stand-in-prompts",
    };
    let text = std::fs::read_to_string(work.join(file)).unwrap_or_default();
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// A stand-in for the Claude Code driver: it finds the core's socket and token in the agent's
/// home, as `node src/main.ts --home <home> --cwd <work>` does, gives the hello that driver gives,
/// and answers each prompt "ok: " and its text, in one turn. It takes every other request and
/// does nothing with it. Each prompt's text is added to `heard`.
async fn claude_code_stand_in(home: PathBuf, work: PathBuf, heard: Arc<Mutex<Vec<String>>>) {
    let socket = home.join("socket/marmot-agent.sock");
    let token = within(60, || {
        let token = std::fs::read_to_string(home.join("connector.token")).ok()?;
        socket.exists().then_some(token)
    })
    .await;
    let stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    async fn send(write: &mut tokio::net::unix::OwnedWriteHalf, message: Value) {
        let line = format!("{message}\n");
        write.write_all(line.as_bytes()).await.unwrap();
    }
    send(
        &mut write,
        json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {
            "protocol": PROTOCOL,
            "connector": {"name": "marmot-agent-claude-code", "version": "0.0.0-stand-in"},
            "harness": {"name": "claude-code"},
            "token": token.trim(),
            // The Claude Code driver's own (`CAPABILITIES` in `src/connector.ts`).
            "capabilities": {
                "interrupt": true, "mid_turn": "restart", "sessions": "core", "resume": true,
                "approvals": true, "files_in": "none", "approval_commands": [],
            },
            "folder": work,
        }}),
    )
    .await;
    let welcome: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert!(welcome.get("error").is_none(), "the core took the hello");
    let mut turns = 0;
    while let Ok(Some(line)) = lines.next_line().await {
        let request: Value = serde_json::from_str(&line).unwrap();
        if request.get("id").is_some() {
            send(
                &mut write,
                json!({"jsonrpc": "2.0", "id": request["id"], "result": {}}),
            )
            .await;
        }
        if request["method"] != "prompt" {
            continue;
        }
        turns += 1;
        let params = &request["params"];
        let text = params["text"].as_str().unwrap_or_default().to_string();
        heard.lock().unwrap().push(text.clone());
        let event = |item: String, kind: &str, state: &str, text: String, body: Value| {
            json!({"jsonrpc": "2.0", "method": "event", "params": {
                "item": item, "type": kind, "state": state, "title": kind, "text": text,
                "body": body, "session": "s1", "turn": format!("t{turns}"),
                "chat": params["chat"]["id"], "prompts": [params["message"]],
            }})
        };
        let turn = json!({"started_by": "user"});
        let (id, words) = (format!("turn{turns}"), format!("ok: {text}"));
        let running = event(
            id.clone(),
            "turn",
            "running",
            "Working".into(),
            turn.clone(),
        );
        send(&mut write, running).await;
        let said = event(format!("msg{turns}"), "message", "done", words, json!({}));
        send(&mut write, said).await;
        send(&mut write, event(id, "turn", "done", "Done".into(), turn)).await;
    }
}

/// Moves an install of `terminal` over, and checks the agent answers in its chat afterwards.
async fn moved(terminal: Terminal, connector: Connector) {
    let Invited {
        root,
        home,
        url,
        person_runtime,
        person,
        agent,
        group,
        group_hex,
        relay: _relay,
    } = invited(terminal.folder, terminal.label).await;
    let program = stand_in(root.path(), connector);
    installed(
        terminal,
        root.path(),
        &home,
        &agent,
        &person,
        &url,
        &program,
    );

    // The plan reads the install and changes nothing.
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let options = Options {
        home: home.clone(),
        harness: None,
        owner: None,
        cwd: Some(work.to_string_lossy().into_owned()),
        settings: None,
        leave_running: Vec::new(),
        places: places(root.path()),
        now: std::time::SystemTime::now(),
    };
    let plan = import::plan(&options, &mut Noted::default()).unwrap();
    assert_eq!(plan.connector, connector);
    assert_eq!(plan.found_by, "the harness's settings in the home");
    assert_eq!(plan.agent.as_str(), agent);
    assert_eq!(plan.owner.as_str(), person, "the one person allowed");
    assert_eq!(plan.prompt, Who::Owner, "the owner was the only sender");
    assert_eq!(plan.approve, Who::Owner);
    assert!(plan.invite.is_empty());
    assert_eq!(plan.relays, std::slice::from_ref(&url), "the old relays");
    assert!(!plan.backup.exists() && !plan.settings.exists());
    let mut printed = Vec::new();
    plan.print(&mut printed).unwrap();
    let printed = String::from_utf8(printed).unwrap();
    assert!(
        printed.contains(&format!("A {} install", connector.title())),
        "{printed}"
    );
    assert!(
        !printed.contains(&person) && !printed.contains(&agent) && !printed.contains(&url),
        "keys and relays are counted, not shown"
    );
    // What did not map is said.
    assert!(printed.contains("time limits"), "{printed}");
    assert!(printed.contains("`/<path>`"), "{printed}");
    // The program the installer found is carried over where the harness's section takes one.
    let (carried, not_carried) = printed.split_once("Not carried over").unwrap();
    if connector == Connector::Claude {
        assert!(not_carried.contains("`WN_CLAUDE_BIN`"), "{printed}");
    } else {
        assert!(carried.contains("the harness's program"), "{printed}");
    }

    // Carried out: the services stopped, harness first, the home backed up, the settings written.
    let mut services = Noted::default();
    let mut said = Vec::new();
    import::carry_out(&options, &plan, &mut services, &mut said).unwrap();
    let harness_service = format!("wn-{}.service", terminal.kind);
    let agent_service = format!("{}.service", terminal.agent_service);
    assert_eq!(services.0, [harness_service.as_str(), &agent_service]);
    let units = root.path().join(".config/systemd/user");
    assert!(units.join(&harness_service).exists(), "kept");
    assert!(units.join(&agent_service).exists(), "kept");
    let said = String::from_utf8(said).unwrap();
    assert!(said.contains("checked against the home"), "{said}");
    // And it says how to start the core, and for Claude Code its driver too.
    assert!(
        said.contains("start the agent: `marmot-agent run "),
        "{said}"
    );
    if connector == Connector::Claude {
        assert!(said.contains("start the Claude Code driver"), "{said}");
    }
    assert!(!plan.backup.join("marmot-agent.toml").exists());
    let accounts = AccountHome::open(&plan.backup).accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].account_id_hex, agent);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&plan.backup), 0o700);
    assert_eq!(mode(&plan.settings), 0o600);
    assert_eq!(plan.settings, home.join("marmot-agent.toml"));
    let config = Config::read(&plan.settings).unwrap();
    assert_eq!(config.home, home, "the home, in place");
    assert_eq!(config.relays, std::slice::from_ref(&url));
    assert!(config.allow_loopback_relays);
    assert_eq!(config.owner.as_str(), person);
    assert!(
        config.file_folders().is_empty(),
        "the old connector sent no files"
    );
    assert!(config.codex.is_none());
    let program = program.to_string_lossy().into_owned();
    let cwd = work.to_string_lossy().into_owned();
    match connector {
        Connector::OpenCode | Connector::Goose => {
            let acp = config.acp.as_ref().expect("an [acp] section");
            assert_eq!(acp.name, connector.name());
            assert_eq!(acp.command, [program.clone(), "acp".into()]);
            assert_eq!(acp.cwd, cwd);
            assert!(config.pi.is_none() && config.connector.is_none());
        }
        Connector::Pi => {
            let pi = config.pi.as_ref().expect("a [pi] section");
            assert_eq!(pi.program, program);
            assert!(pi.args.is_empty());
            assert_eq!(pi.cwd, cwd);
            assert!(config.acp.is_none() && config.connector.is_none());
        }
        _ => {
            assert!(config.connector.is_some(), "a [connector] section");
            assert!(config.acp.is_none() && config.pi.is_none());
        }
    }
    // A second import of the same home is refused: its settings are there.
    assert!(import::plan(&options, &mut Noted::default()).is_err());

    // The core runs on the settings as the import wrote them, as the import says to start it.
    // White Noise's own relays are out of reach in a test, and are never used from one.
    let text = std::fs::read_to_string(&plan.settings).unwrap();
    std::fs::write(&plan.settings, format!("white_noise = false\n{text}")).unwrap();
    let mut core = tokio::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .arg("run")
        .arg(&plan.settings)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let claude_heard = Arc::new(Mutex::new(Vec::new()));
    let driver = (connector == Connector::Claude).then(|| {
        tokio::spawn(claude_code_stand_in(
            home.clone(),
            work.clone(),
            claude_heard.clone(),
        ))
    });
    let heard_now = || match connector {
        Connector::Claude => claude_heard.lock().unwrap().clone(),
        _ => heard(&work, connector),
    };

    // On its first start the core reads nothing already stored as a prompt: it marks where each
    // chat stands. Once it has, nobody invites it again: the person writes in the same chat, and
    // the agent answers, from the same key.
    let state = home.join("marmot-agent-state.json");
    within(60, || {
        let kept = std::fs::read_to_string(&state).ok()?;
        kept.contains(&group_hex).then_some(())
    })
    .await;
    // MDK gives the time a message arrived in whole seconds: one that arrived in the second the
    // core subscribed in is history.
    tokio::time::sleep(Duration::from_secs(2)).await;
    person_runtime
        .send_message(&person, &group, AFTER.as_bytes().to_vec())
        .await
        .unwrap();
    let answer = format!("ok: {AFTER}");
    // MDK can take some seconds to settle a chat after a program first opens it.
    let mut answered = false;
    for _ in 0..900 {
        let said = chat_messages(&person_runtime, &person, &group_hex);
        answered = said
            .iter()
            .any(|m| m.sender == agent && m.plaintext == answer);
        if answered {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        answered,
        "no answer within 90 seconds; the harness heard {} prompts, the core {}, and the \
         person has {} messages from the agent",
        heard_now().len(),
        match core.try_wait() {
            Ok(Some(status)) => format!("ended: {status}"),
            _ => "runs".into(),
        },
        chat_messages(&person_runtime, &person, &group_hex)
            .iter()
            .filter(|m| m.sender == agent)
            .count(),
    );
    // History announced late would be read by now.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        heard_now(),
        [AFTER],
        "the harness heard the one message after the move, and nothing from before"
    );
    let said = chat_messages(&person_runtime, &person, &group_hex);
    assert!(
        said.iter()
            .filter(|m| m.sender != person)
            .all(|m| m.sender == agent && !m.plaintext.contains(BEFORE)),
        "only the agent answers, and never to what the old connector read"
    );

    // Stopped as systemd stops it.
    let pid = core.id().unwrap().to_string();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(killed.success());
    let status = within(30, || core.try_wait().ok().flatten()).await;
    assert!(status.success(), "the core stops cleanly: {status}");
    if let Some(driver) = driver {
        // The core closed the socket as it stopped, which ends the stand-in.
        tokio::time::timeout(Duration::from_secs(10), driver)
            .await
            .expect("the stand-in driver ends with the core")
            .unwrap();
    }
    person_runtime.shutdown().await;
}
