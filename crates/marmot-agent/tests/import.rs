//! Taking over one of today's connector installs (D-089). Today's connector itself, MDK's
//! `agent-connector` that is `wn-agent`, makes the home and joins a person's chat there, and the
//! files its installer writes are put around it. The import backs the home up and writes the settings, and the core then
//! runs on the home in place: the same identity, in the same chat, without being invited again.

mod common;

use agent_control::{AgentControlEnvelope, AgentControlRequest, AgentControlResponse};
use common::{Echo, eventually, within};
use marmot_account::AccountHome;
use marmot_agent::PubKey;
use marmot_agent::config::Config;
use marmot_agent::import::services::{Loaded, Service, Services};
use marmot_agent::import::{self, Connector, Options, Places};
use marmot_agent::marmot::{self, Marmot};
use marmot_agent::rights::Who;
use marmot_agent::run::{Core, Store};
use marmot_app::{
    AccountSetupRequest, AppMessageQuery, AppMessageRecord, MarmotApp, MarmotAppConfig,
    MarmotAppRuntime,
};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

/// The service managers, as the tests see them: each service stopped and disabled is noted.
/// There is no systemd to ask, so the units' files are read.
#[derive(Default)]
struct Noted(Vec<String>);

impl Services for Noted {
    fn stop_and_disable(&mut self, service: &Service) -> Result<(), String> {
        self.0.push(service.name.clone());
        Ok(())
    }

    fn systemd_unit(&mut self, _: &str) -> Option<Result<Loaded, String>> {
        None
    }
}

fn places(root: &Path) -> Places {
    Places {
        user_home: root.to_path_buf(),
        systemd_units: root.join(".config/systemd/user"),
        launch_agents: root.join("Library/LaunchAgents"),
        hermes_home: root.join(".hermes"),
        openclaw_config: root.join(".openclaw/openclaw.json"),
    }
}

/// What the Codex installer writes around the home besides what `wn-agent` keeps in it: the
/// harness's settings, and the two systemd user units.
fn installed_codex(root: &Path, home: &Path, agent: &str, person: &str, relay: &str) {
    let h = home.display();
    std::fs::write(
        home.join("dev/wn-codex.env"),
        format!(
            "MARMOT_HOME={h}\nMARMOT_AGENT_SOCKET={h}/dev/wn-agent.sock\n\
             WN_CODEX_ACCOUNT_ID_HEX={agent}\nWN_CODEX_ALLOWED_SENDERS_HEX={person}\n\
             WN_CODEX_BIN=codex\nMARMOT_HARNESS_EXECUTION_PROFILE=inherit\n\
             WN_CODEX_TIMEOUT_SECS=3600\n"
        ),
    )
    .unwrap();
    let units = root.join(".config/systemd/user");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::write(
        units.join("wn-agent-codex.service"),
        format!(
            "[Unit]\nDescription=Marmot wn-agent connector\n\n[Service]\n\
             ExecStart=\"/x/bin/wn-agent\" \"--home\" \"{h}\" \"--socket\" \"{h}/dev/wn-agent.sock\" \
             \"--relay\" \"{relay}\" \"--allow-loopback-relays\"\nRestart=always\n\n\
             [Install]\nWantedBy=default.target\n"
        ),
    )
    .unwrap();
    std::fs::write(
        units.join("wn-codex.service"),
        format!(
            "[Unit]\nRequires=wn-agent-codex.service\n\n[Service]\n\
             Environment=\"MARMOT_HOME={h}\"\n\
             Environment=\"WN_CODEX_ALLOWED_SENDERS_HEX={person}\"\n\
             Environment=\"WN_CODEX_TIMEOUT_SECS=3600\"\n\
             ExecStart=\"/x/bin/wn-codex\"\n"
        ),
    )
    .unwrap();
}

/// An app on a home, as `wn-agent` and the test client open one, with only the test relay.
fn local_app(home: &Path, relay: &str) -> MarmotApp {
    MarmotApp::try_with_relays_and_account_home_and_config(
        home,
        vec![relay.to_string()],
        AccountHome::open(home),
        MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true),
    )
    .unwrap()
}

fn setup(relay: &str) -> AccountSetupRequest {
    let endpoint = cgka_traits::TransportEndpoint(relay.to_string());
    AccountSetupRequest {
        default_relays: vec![endpoint.clone()],
        bootstrap_relays: vec![endpoint],
        publish_initial_key_package: true,
        ..AccountSetupRequest::default()
    }
}

fn chat_messages(runtime: &MarmotAppRuntime, account: &str, group: &str) -> Vec<AppMessageRecord> {
    let query = AppMessageQuery {
        group_id_hex: Some(group.to_string()),
        kinds: Some(vec![9]),
        limit: None,
    };
    runtime
        .messages_with_query(account, query)
        .unwrap_or_default()
}

/// Today's connector, `wn-agent`, as its own program: MDK's `agent-connector` at the revision
/// the core uses, serving its socket on the home as `wn-agent --home <home> --relay <relay>
/// --allow-loopback-relays` does. The test runs itself again with this test alone and the home
/// and relay in its environment, and stops it as systemd stops a service. It does nothing when
/// run on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run by the import test, as the old connector"]
async fn old_connector() {
    let (Some(home), Some(relay)) = (
        std::env::var_os("OLD_CONNECTOR_HOME"),
        std::env::var("OLD_CONNECTOR_RELAY").ok(),
    ) else {
        return;
    };
    storage_sqlite::relax_storage_for_test_process();
    let mut config = agent_connector::AgentConnectorConfig::new(&home);
    config.relays = vec![relay];
    config.allow_loopback_relays = true;
    agent_connector::serve_socket(config).await.unwrap();
}

/// Why a request to the old connector has no answer.
#[derive(Debug)]
enum Unanswered {
    /// Nothing listens on the socket yet: the request was never sent.
    NotListening,
    /// It was sent, and no answer came in time: what it asked may or may not have been done.
    NoAnswer,
}

/// One request to the old connector's socket, as its clients make them, and its answer, waited
/// for up to 30 seconds.
async fn ask(
    socket: &Path,
    request: AgentControlRequest,
) -> Result<AgentControlResponse, Unanswered> {
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|_| Unanswered::NotListening)?;
    let (read, mut write) = stream.into_split();
    let envelope = AgentControlEnvelope::request(Some("1".into()), request);
    let exchange = async {
        agent_control::write_frame(&mut write, &envelope)
            .await
            .ok()?;
        let mut read = tokio::io::BufReader::new(read);
        let answer: AgentControlEnvelope<AgentControlResponse> =
            agent_control::read_envelope(&mut read).await.ok()??;
        Some(answer.payload)
    };
    tokio::time::timeout(Duration::from_secs(30), exchange)
        .await
        .ok()
        .flatten()
        .ok_or(Unanswered::NoAnswer)
}

/// The texts of the chat as the old connector has stored them, once it is in the chat.
async fn timeline(socket: &Path, agent: &str, group: &str) -> Option<Vec<String>> {
    let request = AgentControlRequest::TimelineList {
        account_id_hex: agent.to_string(),
        group_id_hex: group.to_string(),
        before: None,
        after: None,
        before_inclusive: false,
        limit: Some(50),
    };
    match ask(socket, request).await.ok()? {
        AgentControlResponse::TimelinePage { messages, .. } => {
            Some(messages.into_iter().filter_map(|m| m.text).collect())
        }
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_imported_codex_install_keeps_its_identity_and_its_chat() {
    imported(false).await;
}

// As above, and then the person writes in the chat again, and the agent answers. MDK at the
// revision the core uses delivers nothing sent to that chat once the core has started on the
// home: in 7 runs of 7, waiting up to 300 s, with a pause of 5 s or 15 s before the old connector
// stopped, and with the core before 2026-11-22 too. A message sent in the very second the core
// started reaches it, some 20 s late, but that second is history to the core.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "MDK #3602: an imported home hears nothing sent after the core starts"]
async fn an_imported_codex_install_answers_in_its_chat() {
    imported(true).await;
}

/// Imports a Codex install of the old connector and runs the core on its home. With `answers`,
/// the person writes in the chat after the move, and the agent answers.
async fn imported(answers: bool) {
    storage_sqlite::relax_storage_for_test_process();
    let relay = nostr_relay_builder::MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join(".marmot-agents/codex");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o700)).unwrap();

    // The person, in a home of their own.
    let person_runtime = MarmotAppRuntime::new(local_app(&root.path().join("person"), &url));
    let person = person_runtime
        .create_identity(setup(&url))
        .await
        .unwrap()
        .account
        .account_id_hex;
    person_runtime.start().await.unwrap();

    // Today's connector runs on the home. As the installer's bootstrap does over its socket, it
    // makes the identity, publishes a key package and lets the person invite it.
    let mut connector = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "old_connector", "--ignored", "--nocapture"])
        .env("OLD_CONNECTOR_HOME", &old)
        .env("OLD_CONNECTOR_RELAY", &url)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let socket = old.join("dev/wn-agent.sock");
    let create = || AgentControlRequest::AccountCreate {
        label: Some("codex-harness-agent".into()),
        publish_key_package: true,
    };
    let mut agent = None;
    for _ in 0..600 {
        match ask(&socket, create()).await {
            Ok(AgentControlResponse::AccountCreated { account }) => {
                agent = Some(account.account_id_hex);
                break;
            }
            // Not listening yet, or refused outright: nothing was made, so it may be asked again.
            Ok(_) | Err(Unanswered::NotListening) => {}
            // Sent and not answered: an identity may have been made, and a second would make the
            // home one the core refuses. Never asked again.
            Err(Unanswered::NoAnswer) => panic!("the old connector did not answer AccountCreate"),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let agent = agent.expect("the old connector made its identity");
    let allow = AgentControlRequest::AllowlistAdd {
        account_id_hex: agent.clone(),
        welcomer_account_id_hex: person.clone(),
    };
    assert!(matches!(
        ask(&socket, allow).await,
        Ok(AgentControlResponse::Allowlist { .. })
    ));

    // The person starts a chat with it; it accepts, as the person is on its allow-list, and
    // stores what the person writes there.
    let group = person_runtime
        .start_direct_conversation(&person, &agent)
        .await
        .unwrap();
    let group_hex = hex::encode(group.as_slice());
    let mut joined = false;
    for _ in 0..120 {
        if timeline(&socket, &agent, &group_hex).await.is_some() {
            joined = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(joined, "the old connector joined the chat");
    let mut stored = false;
    for _ in 0..20 {
        person_runtime
            .send_message(&person, &group, b"before the move".to_vec())
            .await
            .unwrap();
        for _ in 0..6 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let texts = timeline(&socket, &agent, &group_hex)
                .await
                .unwrap_or_default();
            stored = texts.iter().any(|t| t == "before the move");
            if stored {
                break;
            }
        }
        if stored {
            break;
        }
    }
    assert!(stored, "the old connector stored the person's message");
    // A wn-agent stopped in the moment after a message arrives leaves the home deaf to that chat
    // for the next program: without this pause the core never hears the person (every run, with
    // MDK at the revision the core uses). One that has run for a while, as an installed one has,
    // does not.
    tokio::time::sleep(Duration::from_secs(5)).await;
    // Stopped as systemd stops it.
    let pid = connector.id().unwrap().to_string();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(killed.success());
    within(30, || connector.try_wait().ok().flatten().map(|_| ())).await;
    installed_codex(root.path(), &old, &agent, &person, &url);

    // The plan reads the install and changes nothing.
    let work = root.path().join("work");
    let options = Options {
        home: old.clone(),
        harness: None,
        owner: None,
        cwd: Some(work.to_string_lossy().into_owned()),
        settings: None,
        leave_running: Vec::new(),
        places: places(root.path()),
        now: std::time::SystemTime::now(),
    };
    let plan = import::plan(&options, &mut Noted::default()).unwrap();
    assert_eq!(plan.connector, Connector::Codex);
    assert_eq!(plan.owner.as_str(), person, "the one person allowed");
    assert_eq!(plan.prompt, Who::Owner, "the owner was the only sender");
    assert!(plan.invite.is_empty());
    assert!(!plan.backup.exists() && !plan.settings.exists());
    let mut printed = Vec::new();
    plan.print(&mut printed).unwrap();
    let printed = String::from_utf8(printed).unwrap();
    assert!(
        !printed.contains(&person) && !printed.contains(&url),
        "keys and relays are counted, not shown"
    );
    assert!(printed.contains("time limits"), "what did not map is said");

    // Carried out: the services stopped, harness first, the home backed up, the settings written.
    let mut services = Noted::default();
    let mut said = Vec::new();
    import::carry_out(&options, &plan, &mut services, &mut said).unwrap();
    assert_eq!(services.0, ["wn-codex.service", "wn-agent-codex.service"]);
    let units = root.path().join(".config/systemd/user");
    assert!(units.join("wn-agent-codex.service").exists(), "kept");
    // The backup was checked against the home before the settings were written beside it.
    let said = String::from_utf8(said).unwrap();
    assert!(said.contains("checked against the home"), "{said}");
    assert!(!plan.backup.join("marmot-agent.toml").exists());
    let accounts = AccountHome::open(&plan.backup).accounts().unwrap();
    assert_eq!(accounts[0].account_id_hex, agent);
    let mode = std::fs::metadata(&plan.backup)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    let mode = std::fs::metadata(&plan.settings)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let mut config = Config::read(&plan.settings).unwrap();
    assert_eq!(config.home, old, "the home, in place");
    assert_eq!(config.relays, std::slice::from_ref(&url), "the old relays");
    assert!(config.allow_loopback_relays);
    assert_eq!(config.owner.as_str(), person);
    let codex = config.codex.as_ref().expect("a Codex section");
    assert_eq!(codex.cwd, work.to_string_lossy());
    assert_eq!(codex.program, "codex");
    assert!(
        config.file_folders().is_empty(),
        "the old connector sent no files"
    );
    // A second import of the same home is refused: its settings are there.
    assert!(import::plan(&options, &mut Noted::default()).is_err());

    // The core runs on the home in place, as the same identity, already in the chat. White
    // Noise's own relays are out of reach in a test.
    config.white_noise = false;
    let (home, app, runtime) = marmot::open(&config).unwrap();
    let account = marmot::identity(&home).unwrap();
    assert_eq!(account.account_id_hex, agent, "the same identity");
    let live = marmot::subscribe(&runtime);
    let since = live.since;
    runtime.start().await.unwrap();
    let marmot = Marmot {
        runtime: runtime.clone(),
        app,
        label: account.label,
        agent: PubKey(account.account_id_hex),
        faults: Default::default(),
    };
    assert!(
        marmot
            .groups()
            .unwrap()
            .iter()
            .any(|g| g.as_str() == group_hex),
        "still in the chat"
    );
    let prompts = Arc::new(AtomicUsize::new(0));
    let core = Core::new(
        marmot,
        Echo::new(prompts.clone()),
        &config,
        Store::new(&config.home),
    )
    .unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(core.run(live, async {
        let _ = stopped.await;
    }));

    // On its first start the core reads nothing already stored as a prompt: it marks where
    // each chat stands. Once it has, nobody invites it again: the person writes in the same
    // chat, and the agent answers.
    let state = config.home.join("marmot-agent-state.json");
    eventually(|| {
        let kept = std::fs::read_to_string(&state).ok()?;
        kept.contains(&group_hex).then_some(())
    })
    .await;
    if answers {
        // MDK gives the time a message arrived in whole seconds: one that arrived in the second
        // the core subscribed in is history.
        within(5, || (marmot::now() > since).then_some(())).await;
        person_runtime
            .send_message(&person, &group, b"after the move".to_vec())
            .await
            .unwrap();
        // MDK can take some seconds to settle a chat after a program first opens it.
        within(90, || {
            let said = chat_messages(&person_runtime, &person, &group_hex);
            said.iter()
                .any(|m| m.sender == agent && m.plaintext == "echo: after the move")
                .then_some(())
        })
        .await;
    } else {
        // MDK can take some seconds to settle a chat after a program first opens it, and
        // announces what it fetched then: history announced late would be read by now.
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    let said = chat_messages(&person_runtime, &person, &group_hex);
    assert!(
        !said.iter().any(|m| m.plaintext == "echo: before the move"),
        "what the old connector read is not a prompt"
    );
    assert_eq!(prompts.load(Ordering::SeqCst), usize::from(answers));

    let _ = stop.send(());
    task.await.unwrap().unwrap();
    runtime.shutdown().await;
    person_runtime.shutdown().await;
}

/// An option where a value belongs is refused before anything is read or changed: once,
/// `--settings --dry-run` wrote the settings to a file called `--dry-run` and carried the import
/// out.
#[test]
fn an_option_given_as_a_value_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("codex");
    std::fs::create_dir_all(&home).unwrap();
    let owner = AccountHome::open(root.path().join("person"))
        .create_account("person")
        .unwrap()
        .account_id_hex;
    AccountHome::open(&home)
        .create_account("codex-harness-agent")
        .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_marmot-agent"))
        .args(["import", &home.to_string_lossy(), "--owner", &owner])
        .args(["--settings", "--dry-run"])
        .env("HOME", root.path())
        .env_remove("XDG_CONFIG_HOME")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("`--settings` needs a value"), "{said}");
    let left: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let mut left = left;
    left.sort();
    assert_eq!(left, ["codex", "person"], "no backup and no settings");
}
