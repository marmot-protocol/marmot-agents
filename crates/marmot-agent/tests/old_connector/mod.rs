//! What the import tests share (D-089): today's connector, MDK's `agent-connector` that is
//! `wn-agent`, run as its own program on an account home; a person who invites it and talks to it
//! there; the files today's installers write around the home; and the service managers as the
//! tests see them.
//!
//! A test file that uses it declares the old connector as a test of its own, which this module
//! runs by re-running the test binary with that test alone:
//!
//! ```ignore
//! #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
//! #[ignore = "run by the import test, as the old connector"]
//! async fn old_connector() {
//!     old_connector::serve().await;
//! }
//! ```

#![allow(dead_code)]

use agent_control::{AgentControlEnvelope, AgentControlRequest, AgentControlResponse};
use cgka_traits::GroupId;
use marmot_account::AccountHome;
use marmot_agent::import::Places;
use marmot_agent::import::services::{Loaded, Service, Services};
use marmot_app::{
    AccountSetupRequest, AppMessageQuery, AppMessageRecord, MarmotApp, MarmotAppConfig,
    MarmotAppRuntime,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The service managers, as the tests see them: each service stopped and disabled is noted.
/// There is no systemd to ask, so the units' files are read.
#[derive(Default)]
pub struct Noted(pub Vec<String>);

impl Services for Noted {
    fn stop_and_disable(&mut self, service: &Service) -> Result<(), String> {
        self.0.push(service.name.clone());
        Ok(())
    }

    fn systemd_unit(&mut self, _: &str) -> Option<Result<Loaded, String>> {
        None
    }
}

/// Where the import looks outside the home, all under `root`, which stands for the person's
/// own folder.
pub fn places(root: &Path) -> Places {
    Places {
        user_home: root.to_path_buf(),
        systemd_units: root.join(".config/systemd/user"),
        launch_agents: root.join("Library/LaunchAgents"),
        hermes_home: root.join(".hermes"),
        openclaw_config: root.join(".openclaw/openclaw.json"),
    }
}

/// An app on a home, as `wn-agent` and the test client open one, with only the test relay.
pub fn local_app(home: &Path, relay: &str) -> MarmotApp {
    marmot_agent::marmot::start_sqlite();
    MarmotApp::try_with_relays_and_account_home_and_config(
        home,
        vec![relay.to_string()],
        AccountHome::open(home),
        MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true),
    )
    .unwrap()
}

pub fn setup(relay: &str) -> AccountSetupRequest {
    let endpoint = cgka_traits::TransportEndpoint(relay.to_string());
    AccountSetupRequest {
        default_relays: vec![endpoint.clone()],
        bootstrap_relays: vec![endpoint],
        publish_initial_key_package: true,
        ..AccountSetupRequest::default()
    }
}

/// The chat's text messages, as `account` has them.
pub fn chat_messages(
    runtime: &MarmotAppRuntime,
    account: &str,
    group: &str,
) -> Vec<AppMessageRecord> {
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
/// --allow-loopback-relays` does. Run by the calling binary's `old_connector` test, with the home
/// and relay in its environment, and stopped as systemd stops a service. It does nothing when
/// run on its own.
pub async fn serve() {
    let (Some(home), Some(relay)) = (
        std::env::var_os("OLD_CONNECTOR_HOME"),
        std::env::var("OLD_CONNECTOR_RELAY").ok(),
    ) else {
        return;
    };
    storage_sqlite::relax_storage_for_test_process();
    marmot_agent::marmot::start_sqlite();
    let mut config = agent_connector::AgentConnectorConfig::new(&home);
    config.relays = vec![relay];
    config.allow_loopback_relays = true;
    agent_connector::serve_socket(config).await.unwrap();
}

/// Why a request to the old connector has no answer.
#[derive(Debug)]
pub enum Unanswered {
    /// Nothing listens on the socket yet: the request was never sent.
    NotListening,
    /// It was sent, and no answer came in time: what it asked may or may not have been done.
    NoAnswer,
}

/// One request to the old connector's socket, as its clients make them, and its answer, waited
/// for up to 30 seconds.
pub async fn ask(
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
pub async fn timeline(socket: &Path, agent: &str, group: &str) -> Option<Vec<String>> {
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

/// What the person wrote to the old connector before the move.
pub const BEFORE: &str = "before the move";

/// An install of today's connector that a person has invited and talked to, now stopped.
pub struct Invited {
    /// The person's own folder: the home, the units and the person's own home are in it.
    pub root: tempfile::TempDir,
    /// The old account home.
    pub home: PathBuf,
    /// The test relay's address.
    pub url: String,
    pub person_runtime: MarmotAppRuntime,
    pub person: String,
    /// The agent's key, which the old connector made.
    pub agent: String,
    pub group: GroupId,
    pub group_hex: String,
    /// The test relay, which stops once this is dropped: keep it as long as the test runs.
    pub relay: nostr_relay_builder::MockRelay,
}

/// Today's connector on a home in `folder` under a fresh person's folder, as the installer's
/// bootstrap sets it up: it makes the identity under `label`, publishes a key package and lets
/// the person invite it. The person starts a chat with it, it accepts and stores what the person
/// writes there ([`BEFORE`]), and then it is stopped as systemd stops it. The calling binary's
/// `old_connector` test is what runs it.
pub async fn invited(folder: &str, label: &str) -> Invited {
    storage_sqlite::relax_storage_for_test_process();
    marmot_agent::marmot::start_sqlite();
    let relay = nostr_relay_builder::MockRelay::run().await.unwrap();
    let url = relay.url().await.to_string();
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join(folder);
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
        label: Some(label.into()),
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
    // The person writes, and writes again if the old connector has not stored it after a
    // while, as MDK can take some seconds to settle a chat it has just joined. Then every copy
    // sent is waited for, so the old connector is stopped having read all the person wrote: a
    // copy still on its way would reach the core after the move, and the core rightly takes
    // what reaches it after it started as a prompt (D-111).
    let copies = async || {
        let texts = timeline(&socket, &agent, &group_hex)
            .await
            .unwrap_or_default();
        texts.iter().filter(|t| *t == BEFORE).count()
    };
    let (mut sent, mut stored) = (0, 0);
    for _ in 0..20 {
        person_runtime
            .send_message(&person, &group, BEFORE.as_bytes().to_vec())
            .await
            .unwrap();
        sent += 1;
        for _ in 0..6 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            stored = copies().await;
            if stored > 0 {
                break;
            }
        }
        if stored > 0 {
            break;
        }
    }
    assert!(stored > 0, "the old connector stored the person's message");
    for _ in 0..120 {
        if stored == sent {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        stored = copies().await;
    }
    assert_eq!(
        stored, sent,
        "the old connector stored every copy of the person's message it was sent"
    );
    // Stopped at once: with MDK at the revision the core uses, a wn-agent stopped in the moment
    // after a message reached it no longer leaves the home deaf to that chat (MDK #3602).
    // Stopped as systemd stops it.
    let pid = connector.id().unwrap().to_string();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(killed.success());
    for _ in 0..300 {
        if connector.try_wait().ok().flatten().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        connector.try_wait().ok().flatten().is_some(),
        "the old connector stopped within 30 seconds"
    );
    Invited {
        root,
        home: old,
        url,
        person_runtime,
        person,
        agent,
        group,
        group_hex,
        relay,
    }
}

/// One of today's terminal harnesses, as `install-terminal-harness-marmot.sh` names what it sets
/// up for it (`MARMOT_TERMINAL_HARNESS`).
#[derive(Clone, Copy, Debug)]
pub struct Terminal {
    /// The installer's harness kind: `wn-<kind>` is the harness program and its service.
    pub kind: &'static str,
    /// The prefix of the harness's own settings, such as `WN_CLAUDE`.
    pub prefix: &'static str,
    /// The home's folder under the person's own, by default.
    pub folder: &'static str,
    /// The account label, by default.
    pub label: &'static str,
    /// The `wn-agent` service, by default.
    pub agent_service: &'static str,
}

pub const CLAUDE: Terminal = Terminal {
    kind: "claude",
    prefix: "WN_CLAUDE",
    folder: ".marmot-agents/claude",
    label: "claude-harness-agent",
    agent_service: "wn-agent-claude",
};

pub const OPENCODE: Terminal = Terminal {
    kind: "opencode",
    prefix: "WN_OPENCODE",
    folder: ".marmot-agents/harnesses",
    label: "terminal-harness-agent",
    agent_service: "wn-agent-harnesses",
};

pub const PI: Terminal = Terminal {
    kind: "pi",
    prefix: "WN_PI",
    folder: ".marmot-agents/pi",
    label: "pi-harness-agent",
    agent_service: "wn-agent-pi",
};

pub const GOOSE: Terminal = Terminal {
    kind: "goose",
    prefix: "WN_GOOSE",
    folder: ".marmot-agents/goose",
    label: "goose-harness-agent",
    agent_service: "wn-agent-goose",
};

/// What `install-terminal-harness-marmot.sh` writes around the home besides what `wn-agent` keeps
/// in it, with its defaults, as MDK has it at the revision the core uses: the harness's settings
/// in `dev/wn-<kind>.env`, and the two systemd user units, `wn-agent` with the relay, and the
/// harness with the same settings in `Environment=` lines. `bin` is the harness's program, which
/// the installer writes as the whole path it found. The `wn-agent` unit also lets relays on this
/// machine be used, which the test relay needs.
pub fn installed(
    terminal: Terminal,
    root: &Path,
    home: &Path,
    agent: &str,
    person: &str,
    relay: &str,
    bin: &Path,
) {
    let h = home.display();
    let p = terminal.prefix;
    let bin = bin.display();
    let settings = [
        format!("MARMOT_HOME={h}"),
        format!("MARMOT_AGENT_SOCKET={h}/dev/wn-agent.sock"),
        format!("{p}_ACCOUNT_ID_HEX={agent}"),
        format!("{p}_ALLOWED_SENDERS_HEX={person}"),
        format!("{p}_BIN={bin}"),
        "MARMOT_HARNESS_EXECUTION_PROFILE=inherit".into(),
        format!("{p}_TIMEOUT_SECS=3600"),
        format!("{p}_IDLE_TIMEOUT_SECS=120"),
        format!("{p}_REQUEST_TIMEOUT_SECS=30"),
        format!("{p}_MAX_REPLY_BYTES=30000"),
        format!("{p}_MAX_PENDING_PER_GROUP=4"),
    ];
    let env_file = home.join(format!("dev/wn-{}.env", terminal.kind));
    std::fs::write(&env_file, settings.join("\n") + "\n").unwrap();
    std::fs::set_permissions(&env_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let units = root.join(".config/systemd/user");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::write(
        units.join(format!("{}.service", terminal.agent_service)),
        format!(
            "[Unit]\nDescription=Marmot wn-agent connector\nAfter=network-online.target\n\n\
             [Service]\n\
             ExecStart=\"{r}/.local/bin/wn-agent\" \"--home\" \"{h}\" \"--socket\" \
             \"{h}/dev/wn-agent.sock\" \"--relay\" \"{relay}\" \"--allow-loopback-relays\"\n\
             Restart=always\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
            r = root.display(),
        ),
    )
    .unwrap();
    let environment: String = settings
        .iter()
        .map(|s| format!("Environment=\"{s}\"\n"))
        .collect();
    std::fs::write(
        units.join(format!("wn-{}.service", terminal.kind)),
        format!(
            "[Unit]\nDescription=Marmot wn-{k} harness\nAfter={a}.service\nRequires={a}.service\n\n\
             [Service]\n{environment}ExecStart=\"{r}/.local/bin/wn-{k}\"\n\
             Restart=always\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
            k = terminal.kind,
            a = terminal.agent_service,
            r = root.display(),
        ),
    )
    .unwrap();
}
