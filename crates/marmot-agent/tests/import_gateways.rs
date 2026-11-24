//! Moving one of today's gateway installs over (M4's "done when", D-089): Hermes and OpenClaw, each
//! with the old connector's plugin in the real gateway, in the repository's gateway image. Today's
//! connector itself, MDK's `agent-connector` that is `wn-agent`, at the revision the core uses,
//! makes the home and serves its socket there; the old plugin is built from MDK's sources at that
//! revision, as MDK's release workflow builds it, and put into the gateway and set up as MDK's
//! installer does. A person invites the agent and has a message answered. Then the move, as
//! `docs/move.md` says: `wn-agent` stopped as systemd stops it, the import, our installer with
//! `--replace-plugin` against the same gateway, the core started with the launcher the installer
//! wrote, and the gateway restarted. The person's next message, sent once, is answered in the
//! same chat, by the same agent key, in the turn that message started, and no request the gateway
//! makes to its model after the move holds anything said before it.
//!
//! What stands in: the gateway's model is the model double (`tools/model_double`), scripted, on the
//! loopback address, which answers each request by what the person's words in it say, so no model
//! key is needed and nothing leaves the machine; the service manager is the one the other import
//! tests use (`old_connector::Noted`), which notes what the import stops (no systemd is touched),
//! and the core runs without a service (`--no-service`), started with the launcher the installer
//! writes. The gateway runs in its container with the test's folder mounted at its own path, so the
//! installer, on this machine, runs the gateway's own commands (`hermes`, `openclaw`) in that
//! container, on the same files.
//!
//! It needs Docker, and the network to build the images and the old plugin's packages the first
//! time. A gateway's home with its runtime takes some hundreds of megabytes in the temporary
//! folder: set `TMPDIR` to another one where that is small, with a short path, since the agent's
//! sockets are in it. The gateways run on this machine's network, and OpenClaw's on a port the test
//! finds free and then lets go of, so run one scenario at a time. They run only when asked for:
//!
//! ```sh
//! cargo test -p marmot-agent --test import_gateways -- --ignored --nocapture --test-threads=1
//! ```

mod common;
mod model_double;
mod old_connector;

use agent_control::{AgentControlRequest, AgentControlResponse};
use common::within;
use marmot_account::AccountHome;
use marmot_agent::EventKey;
use marmot_agent::carrier::{self, Asked, EDIT_KIND, ITEM_KIND};
use marmot_agent::import::{self, Connector, Options};
use marmot_agent::rights::Who;
use marmot_agent_events::body::Body;
use marmot_app::{AppMessageQuery, MarmotAppRuntime};
use model_double::Double;
use old_connector::{BEFORE, Noted, Unanswered, ask, chat_messages, local_app, places, setup};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Std;
use std::time::Duration;
use tokio::process::{Child, Command};

/// What the person writes after the move; before it, they write `old_connector::BEFORE`.
const AFTER: &str = "after the move";
/// What the scripted model answers to each.
const BEFORE_ANSWER: &str = "first answer";
const AFTER_ANSWER: &str = "second answer";
const OTHER_ANSWER: &str = "some other answer";

/// How long a gateway may take to answer, from its start: it loads its plugins and runtime first.
const ANSWER: u64 = 300;

/// Today's connector, as its own program: run by `Install::new`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run by the gateway import tests, as the old connector"]
async fn old_connector() {
    old_connector::serve().await;
}

/// The scripted model: the model double (`tools/model_double`), an OpenAI-compatible endpoint on
/// the loopback address that answers every chat completion by what the person's prompt in it
/// says, streamed or not, and keeps the whole body of each request it was sent, oldest first.
struct Model {
    url: String,
    double: Double,
}

impl Model {
    async fn serve() -> Self {
        let answer = |when: &str, text: &str| json!({"when": when, "replies": [[{"text": text}]], "repeat": true});
        let script = json!({
            "prompts": [answer(AFTER, AFTER_ANSWER), answer(BEFORE, BEFORE_ANSWER), answer("", OTHER_ANSWER)],
            "side": [{"text": OTHER_ANSWER}],
        });
        let double = Double::scripted_keeping_bodies(&script).await;
        Self {
            url: format!("{}/v1", double.url),
            double,
        }
    }

    async fn requests(&self) -> usize {
        self.double.log().await.len()
    }

    /// The bodies of the requests the model was sent after the first `from`.
    async fn since(&self, from: usize) -> Vec<String> {
        self.double.bodies().await[from..].to_vec()
    }
}

/// The repository's top folder.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// MDK's sources at the revision the core uses, where Cargo checked them out:
/// `$CARGO_HOME/git/checkouts/mdk-<hash>/<the revision's first seven digits>`.
fn mdk() -> PathBuf {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let line = (manifest.lines())
        .find(|l| l.starts_with("agent-connector"))
        .expect("agent-connector is a dependency");
    let rev = line.split("rev = \"").nth(1).unwrap();
    let short = &rev[..7];
    let cargo = std::env::var_os("CARGO_HOME").map_or_else(
        || PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo"),
        PathBuf::from,
    );
    let checkouts = cargo.join("git/checkouts");
    (std::fs::read_dir(&checkouts).unwrap())
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("mdk-"))
        .map(|e| e.path().join(short))
        .find(|p| p.join("crates/agent-connector/Cargo.toml").exists())
        .expect("MDK's sources, checked out by Cargo")
}

/// Runs a program to its end, and stops the test if it fails, saying what it printed.
fn run(command: &mut Std, what: &str) -> String {
    let out = command.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{what} failed:\n{said}");
    said
}

/// The user and group ids, as the gateway images are built for them.
fn ids() -> (String, String) {
    let id = |flag| run(Std::new("id").arg(flag), "id").trim().to_string();
    (id("-u"), id("-g"))
}

/// A gateway's container, removed when dropped.
struct Container(String, Child);

impl Container {
    /// Stops the gateway as its service manager does, letting it end by itself, and removes it.
    async fn stop(mut self) {
        let _ = Std::new("docker")
            .args(["stop", "-t", "60", &self.0])
            .output();
        let _ = self.1.wait().await;
        let _ = Std::new("docker").args(["rm", "-f", &self.0]).output();
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Std::new("docker").args(["rm", "-f", &self.0]).output();
    }
}

/// Everything about one install of today's connector, and the person who uses it.
struct Install {
    root: tempfile::TempDir,
    /// The old connector's account home.
    home: PathBuf,
    relay: nostr_relay_builder::LocalRelay,
    url: String,
    person_runtime: MarmotAppRuntime,
    person: String,
    agent: String,
    connector: Child,
}

impl Install {
    /// Today's connector on a new home, `~/.marmot-agents/<name>`, with an identity made and the
    /// person allowed to invite it, as the installer's bootstrap does over its socket.
    async fn new(name: &str, label: &str) -> Self {
        storage_sqlite::relax_storage_for_test_process();
        marmot_agent::marmot::start_sqlite();
        let relay = common::relay().await;
        let url = relay.url().await.to_string();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join(".marmot-agents").join(name);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();

        let person_runtime = MarmotAppRuntime::new(local_app(&root.path().join("person"), &url));
        let person = person_runtime
            .create_identity(setup(&url))
            .await
            .unwrap()
            .account
            .account_id_hex;
        person_runtime.start().await.unwrap();

        let connector = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "old_connector", "--ignored", "--nocapture"])
            .env("OLD_CONNECTOR_HOME", &home)
            .env("OLD_CONNECTOR_RELAY", &url)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let socket = home.join("dev/wn-agent.sock");
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
                Err(Unanswered::NotListening) => {}
                // Asked again, a second identity could be made, which the core refuses.
                other => panic!("the old connector did not make the identity: {other:?}"),
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
        Self {
            root,
            home,
            relay,
            url,
            person_runtime,
            person,
            agent,
            connector,
        }
    }

    fn root(&self) -> &Path {
        self.root.path()
    }

    /// The `wn-agent` service MDK's installer writes for this home, as `install-<harness>-marmot.sh`
    /// writes it: the program, the home, the socket, the outbound media folder in the home, and the
    /// relays. The relay is on this machine, which the old connector was told it may use.
    fn write_unit(&self, name: &str) {
        let h = self.home.display();
        let units = self.root().join(".config/systemd/user");
        std::fs::create_dir_all(&units).unwrap();
        let media = self.home.join("dev/outbound-media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::set_permissions(&media, std::fs::Permissions::from_mode(0o700)).unwrap();
        let program = self.root().join(".local/bin/wn-agent");
        std::fs::write(
            units.join(format!("wn-agent-{name}.service")),
            format!(
                "[Unit]\nDescription=Marmot wn-agent connector\nAfter=network-online.target\n\n\
                 [Service]\nExecStart={} --home {h} --socket {h}/dev/wn-agent.sock \
                 --media-allowed-root {} --relay {} --allow-loopback-relays\n\
                 Restart=always\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
                program.display(),
                media.display(),
                self.url,
            ),
        )
        .unwrap();
    }

    /// The person starts a chat with the agent, which the old connector accepts, as the person may
    /// invite it. Gives back the chat, and its id in hex.
    async fn chat(&self) -> (cgka_traits::GroupId, String) {
        let group = self
            .person_runtime
            .start_direct_conversation(&self.person, &self.agent)
            .await
            .unwrap();
        let group_hex = hex::encode(group.as_slice());
        let socket = self.home.join("dev/wn-agent.sock");
        let mut ok = false;
        for _ in 0..120 {
            if old_connector::timeline(&socket, &self.agent, &group_hex)
                .await
                .is_some()
            {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert!(ok, "the old connector joined the chat");
        (group, group_hex)
    }

    /// The agent's messages in the chat that say `text`.
    fn answers(&self, group_hex: &str, text: &str) -> usize {
        chat_messages(&self.person_runtime, &self.person, group_hex)
            .iter()
            .filter(|m| m.sender == self.agent && m.plaintext.contains(text))
            .count()
    }

    /// Through the old plugin: the person writes `text`, again every 30 seconds while no answer
    /// came, until the agent answers with `answer`, from its own key. The old connector can miss a
    /// message sent as it settles a new chat, and OpenClaw's old plugin first asks whether to
    /// publish a name for the agent, and takes the next message as that name.
    async fn exchange_before(
        &self,
        group: &cgka_traits::GroupId,
        group_hex: &str,
        text: &str,
        answer: &str,
    ) {
        for _ in 0..(ANSWER / 30) {
            self.person_runtime
                .send_message(&self.person, group, text.as_bytes().to_vec())
                .await
                .unwrap();
            for _ in 0..60 {
                if self.answers(group_hex, answer) > 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        self.show_log();
        panic!("the agent did not answer {text:?}");
    }

    /// Through our plugin: the person writes `text` once, and the agent answers it with `answer`,
    /// from its own key, in the turn that message started. Gives the turn.
    async fn exchange_after(
        &self,
        group: &cgka_traits::GroupId,
        group_hex: &str,
        text: &str,
        answer: &str,
    ) -> String {
        let sent = self
            .person_runtime
            .send_message(&self.person, group, text.as_bytes().to_vec())
            .await
            .unwrap();
        let prompt = EventKey(sent.message_ids[0].clone());
        for _ in 0..ANSWER * 2 {
            if let Some(turn) = self.answered_in(group_hex, &prompt, answer) {
                return turn;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        self.show_log();
        panic!("the agent did not answer {text:?}, sent once");
    }

    /// The turn that `prompt` started in which the agent said `answer`, if there is one yet: a
    /// turn item names the message that started it, and the agent's words name their turn.
    fn answered_in(&self, group_hex: &str, prompt: &EventKey, answer: &str) -> Option<String> {
        let sent = |kind: u64| {
            let query = AppMessageQuery {
                group_id_hex: Some(group_hex.to_string()),
                kinds: Some(vec![kind]),
                limit: None,
            };
            (self.person_runtime.messages_with_query(&self.person, query))
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.sender == self.agent)
        };
        let turns: Vec<String> = sent(ITEM_KIND)
            .filter_map(|m| carrier::read_item(ITEM_KIND, &m.plaintext, &m.tags))
            .filter(|r| matches!(r.item.event.body(), Body::Turn(_)))
            .filter(|r| r.item.started_by.as_ref() == Some(prompt))
            .filter_map(|r| {
                r.item
                    .event
                    .header()
                    .turn
                    .as_ref()
                    .map(|t| t.as_str().to_string())
            })
            .collect();
        sent(9)
            .filter(|m| m.plaintext.contains(answer))
            .find_map(|m| {
                let turn = m.tags.iter().find(|t| t.len() >= 2 && t[0] == "turn")?;
                turns.contains(&turn[1]).then(|| turn[1].clone())
            })
    }

    /// Sets the session's model from the chat, as an app's menu does: a `set` of `model` to
    /// `value`, kind 1202 (D-029). Waits for what settles it, and checks that it is the session's
    /// item, showing `shown` as the model, never a notice saying why it had no effect (D-028).
    async fn set_model(
        &self,
        group: &cgka_traits::GroupId,
        group_hex: &str,
        value: &str,
        shown: &str,
    ) {
        let asked = Asked::Set {
            setting: "model".into(),
            value: value.into(),
        };
        let out = carrier::request(
            &marmot_agent::PubKey(self.agent.clone()),
            None,
            &asked,
            value,
        );
        let sent = (self.person_runtime)
            .send_custom_event(&self.person, group, out.kind, out.tags, out.content)
            .await
            .unwrap();
        let request = EventKey(sent.message_ids[0].clone());
        for _ in 0..ANSWER * 2 {
            let query = AppMessageQuery {
                group_id_hex: Some(group_hex.to_string()),
                kinds: Some(vec![ITEM_KIND, EDIT_KIND]),
                limit: None,
            };
            let settled = (self.person_runtime.messages_with_query(&self.person, query))
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.sender == self.agent)
                .filter_map(|m| carrier::read_item(m.kind, &m.plaintext, &m.tags))
                .find(|r| r.item.settles.contains(&request));
            if let Some(read) = settled {
                let event = &read.item.event;
                let Body::Session(session) = event.body() else {
                    self.show_log();
                    panic!("{value:?} was not taken: {}", event.header().text);
                };
                let model = session.settings.iter().find(|s| s.id == "model");
                assert_eq!(
                    model.and_then(|m| m.value.as_deref()),
                    Some(shown),
                    "the session shows the model after {value:?}"
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        self.show_log();
        panic!("the agent never settled the model's {value:?}");
    }

    /// Stops the old connector as systemd stops it, at once: with MDK at the revision the core
    /// uses, a `wn-agent` stopped in the moment after a message reached it no longer leaves the
    /// home deaf to that chat (MDK #3602).
    async fn stop_connector(&mut self) {
        let pid = self.connector.id().unwrap().to_string();
        let killed = Std::new("kill").args(["-TERM", &pid]).status().unwrap();
        assert!(killed.success());
        tokio::time::timeout(Duration::from_secs(30), self.connector.wait())
            .await
            .expect("the old connector stopped")
            .unwrap();
    }

    /// The import, as `docs/move.md` says for a gateway: the settings go in the agent's folder,
    /// where the installer looks for them. Checks the plan and what was written, and gives back the
    /// settings' path.
    fn import(&self, connector: Connector, unit: &str) -> PathBuf {
        let agent_folder = self
            .root()
            .join(".local/share/marmot-agent/agents")
            .join(connector.name());
        std::fs::create_dir_all(&agent_folder).unwrap();
        let options = Options {
            home: self.home.clone(),
            harness: None,
            owner: None,
            cwd: None,
            settings: Some(agent_folder.join("settings.toml")),
            leave_running: Vec::new(),
            places: places(self.root()),
            now: std::time::SystemTime::now(),
        };
        let plan = import::plan(&options, &mut Noted::default()).unwrap();
        let mut printed = Vec::new();
        plan.print(&mut printed).unwrap();
        let printed = String::from_utf8(printed).unwrap();
        println!("{printed}");
        assert_eq!(plan.connector, connector);
        assert_eq!(plan.owner.as_str(), self.person, "the one person allowed");
        assert_eq!(plan.prompt, Who::Owner, "the owner was the only sender");
        assert!(plan.invite.is_empty());
        assert!(!plan.backup.exists() && !plan.settings.exists());
        assert!(
            !printed.contains(&self.person) && !printed.contains(&self.url),
            "keys and relays are counted, not shown"
        );

        let mut services = Noted::default();
        let mut said = Vec::new();
        import::carry_out(&options, &plan, &mut services, &mut said).unwrap();
        let said = String::from_utf8(said).unwrap();
        println!("{said}");
        assert_eq!(
            services.0,
            [unit],
            "only wn-agent: the gateway is the person's"
        );
        assert!(said.contains("checked against the home"), "{said}");
        let accounts = AccountHome::open(&plan.backup).accounts().unwrap();
        assert_eq!(accounts[0].account_id_hex, self.agent, "the backup");
        let mode = std::fs::metadata(&plan.settings)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let config = marmot_agent::config::Config::read(&plan.settings).unwrap();
        assert_eq!(config.home, self.home, "the home, in place");
        assert_eq!(
            config.relays,
            std::slice::from_ref(&self.url),
            "the old relays"
        );
        assert!(config.allow_loopback_relays);
        assert_eq!(config.owner.as_str(), self.person);
        assert!(config.connector.is_some(), "a connector on the socket");
        assert!(
            config.file_folders().is_empty(),
            "the old outbound media folder is in the home, which no file is sent from"
        );
        plan.settings.clone()
    }

    /// Runs our installer for `harness` as `docs/move.md` says, with `--replace-plugin`, on this
    /// machine as the person: its home is the test's folder, and the gateway's own command, which
    /// runs in the gateway's container, is first on its PATH. Gives back what it said.
    fn install(&self, harness: &str) -> String {
        let path = format!(
            "{}:{}",
            self.root().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let script = repository().join(format!("install/{harness}.sh"));
        let said = run(
            Std::new(script)
                .args(["--yes", "--no-service", "--replace-plugin", "--binary"])
                .arg(env!("CARGO_BIN_EXE_marmot-agent"))
                .env("HOME", self.root())
                .env("PATH", path)
                .env_remove("XDG_DATA_HOME")
                .env_remove("XDG_CONFIG_HOME")
                .env_remove("HERMES_HOME")
                .env_remove("OPENCLAW_HOME")
                .env_remove("OPENCLAW_STATE_DIR")
                .current_dir(self.root()),
            &format!("install/{harness}.sh --replace-plugin"),
        );
        println!("{said}");
        let agent_folder = self.agent_folder(harness);
        assert!(
            !agent_folder.join("home").exists(),
            "no new identity: the agent keeps the one in its old home"
        );
        assert!(
            said.contains("keeps the identity it was imported with"),
            "{said}"
        );
        said
    }

    fn agent_folder(&self, harness: &str) -> PathBuf {
        self.root()
            .join(".local/share/marmot-agent/agents")
            .join(harness)
    }

    /// Starts the core as the installer's launcher runs it, on the imported settings. White
    /// Noise's own relays are out of reach in a test, so the settings are told to use only the
    /// old relay first. Waits until it serves its socket in the old home.
    async fn start_core(&self, harness: &str, settings: &Path) -> Child {
        let text = std::fs::read_to_string(settings).unwrap();
        let text = text.replacen(
            "allow_loopback_relays = true\n",
            "allow_loopback_relays = true\nwhite_noise = false\n",
            1,
        );
        assert!(text.contains("white_noise = false"), "{text}");
        std::fs::write(settings, text).unwrap();
        let core = Command::new(self.agent_folder(harness).join("start"))
            .env("HOME", self.root())
            .kill_on_drop(true)
            .spawn()
            .expect("the launcher starts the core");
        let socket = self.home.join("socket/marmot-agent.sock");
        within(60, || {
            (socket.exists() && self.home.join("connector.token").exists()).then_some(())
        })
        .await;
        core
    }

    /// Waits until the core has marked where `group_hex` stands, which it does on its first start
    /// for every chat already there, reading none of it as a prompt.
    async fn marked(&self, group_hex: &str) {
        let state = self.home.join("marmot-agent-state.json");
        for _ in 0..ANSWER * 2 {
            let kept = std::fs::read_to_string(&state).unwrap_or_default();
            if kept.contains(group_hex) {
                // MDK gives the time a message arrived in whole seconds: one that arrives in the
                // second the core subscribed in is history to it.
                tokio::time::sleep(Duration::from_secs(2)).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        self.show_log();
        panic!("the core did not mark where the chat stands");
    }

    /// Checks that the gateway asked its model about the message after the move, and that none of
    /// the requests it made to its model since `moved` holds what was said before the move: the
    /// person's words or the agent's answer.
    async fn nothing_from_before(&self, model: &Model, moved: usize) {
        let after = model.since(moved).await;
        assert!(
            after.iter().any(|b| b.contains(AFTER)),
            "the gateway asked its model about the message after the move"
        );
        for (i, body) in after.iter().enumerate() {
            for old in [BEFORE, BEFORE_ANSWER] {
                assert!(
                    !body.contains(old),
                    "request {i} after the move holds {old:?}: something from before reached the model"
                );
            }
        }
    }

    /// Prints the plugin's own log lines, which name what happened by kind only (D-071), when a
    /// step fails.
    fn show_log(&self) {
        if let Ok(log) = std::fs::read_to_string(self.root().join("plugin.log")) {
            println!("the plugin's log:\n{log}");
        }
    }

    async fn end(mut self) {
        let _ = self.connector.kill().await;
        self.person_runtime.shutdown().await;
        drop(self.relay);
    }
}

/// The repository's Hermes image, built for this user as `python/hermes/run.sh` builds it, when
/// it is not there.
fn hermes_image() -> String {
    let (uid, gid) = ids();
    let image = format!("marmot-agents-hermes-{uid}-{gid}");
    let there = Std::new("docker")
        .args(["image", "inspect", &image])
        .output()
        .expect("docker runs");
    if !there.status.success() {
        run(
            Std::new("docker")
                .args(["build", "--build-arg"])
                .arg(format!("UID={uid}"))
                .arg("--build-arg")
                .arg(format!("GID={gid}"))
                .arg("-f")
                .arg(repository().join("python/hermes/Dockerfile"))
                .arg("-t")
                .arg(&image)
                .arg(repository().join("python")),
            "building the Hermes image",
        );
    }
    image
}

/// A gateway's image, run with the test's folder at its own path and on this machine's network,
/// so the gateway reaches the scripted model on the loopback address.
struct Image {
    image: String,
    /// The gateway's container's name.
    name: String,
    /// `docker run`'s options, before the image.
    options: Vec<String>,
}

impl Image {
    fn new(image: &str, root: &Path, env: &[String], name: &str) -> Self {
        let mut options: Vec<String> = ["--init", "--network", "host", "--log-driver", "none"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // As this user, who owns the test's folder.
        let (uid, gid) = ids();
        options.extend(["--user".into(), format!("{uid}:{gid}")]);
        options.push("--mount".into());
        options.push(format!("type=bind,source={0},target={0}", root.display()));
        options.extend(["-w".into(), root.display().to_string()]);
        for e in env {
            options.extend(["-e".into(), e.clone()]);
        }
        Self {
            image: image.to_string(),
            name: name.to_string(),
            options,
        }
    }

    /// `docker run` with `more` options, of `program` and its arguments. The image's own settings
    /// for its plugin (`MARMOT_AGENT_HOME`, which would point our plugin elsewhere, and
    /// `MARMOT_ALLOW_ALL_USERS`) are taken out first: a person's own gateway has neither.
    fn run(&self, more: &[&str], program: &[&str]) -> Vec<String> {
        let mut args = vec!["run".to_string()];
        args.extend(self.options.iter().cloned());
        args.extend(more.iter().map(|s| s.to_string()));
        args.extend([
            "--entrypoint".into(),
            "/usr/bin/env".into(),
            self.image.clone(),
        ]);
        args.extend(["-u", "MARMOT_AGENT_HOME", "-u", "MARMOT_ALLOW_ALL_USERS"].map(String::from));
        args.extend(program.iter().map(|s| s.to_string()));
        args
    }

    /// Writes `bin/<program>` in the test's folder: the gateway's command, run in its image, as the
    /// person runs it on their machine. While the gateway runs, the command sees its processes, as
    /// on one machine: OpenClaw's commands check that the gateway holding its state is alive.
    fn wrapper(&self, root: &Path, program: &str) {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let args = self.run(&["--rm", "-i"], &[program]);
        let quoted: Vec<String> = args[1..].iter().map(|a| format!("'{a}'")).collect();
        let name = &self.name;
        let script = format!(
            "#!/bin/sh\npid=\nif [ \"$(docker inspect -f '{{{{.State.Running}}}}' '{name}' 2>/dev/null)\" = true ]; \
             then pid=--pid=container:'{name}'; fi\nexec docker run $pid {} \"$@\"\n",
            quoted.join(" ")
        );
        let file = bin.join(program);
        std::fs::write(&file, script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Starts the gateway, `program`, in its container, removed when the result is dropped.
    /// Of the gateway's output, only the plugins' own lines, which name what happened by kind
    /// (D-071), are kept, in `plugin.log` in the test's folder, to show when a step fails.
    fn start(&self, program: &[&str], log: &Path) -> Container {
        let name = self.name.as_str();
        let mut child = Command::new("docker")
            .args(self.run(&["--name", name], program))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("docker runs the gateway");
        let out = child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);
        let err = child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);
        for stream in [out, err].into_iter().flatten() {
            let log = log.to_path_buf();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(at) = line.find("marmot-agent-openclaw: ") {
                        let mut file = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&log)
                            .unwrap();
                        let _ = std::io::Write::write_all(
                            &mut file,
                            format!("{}\n", &line[at..]).as_bytes(),
                        );
                    }
                }
            });
        }
        Container(name.to_string(), child)
    }
}

/// The Hermes plugin of today's connector, as MDK's release workflow bundles it
/// (`.github/workflows/wn-agent-binaries.yml`, `hermes-marmot-plugin`: the same files, with the
/// version in `plugin.yaml` left as the sources have it), put where MDK's installer puts it.
fn old_hermes_plugin(plugin: &Path) {
    let mdk = mdk();
    let from = mdk.join("integrations/hermes/marmot");
    std::fs::create_dir_all(plugin).unwrap();
    let manifest = std::fs::read_to_string(from.join("plugin.yaml")).unwrap();
    std::fs::write(plugin.join("plugin.yaml"), manifest).unwrap();
    for file in [
        "__init__.py",
        "adapter.py",
        "agent_control.py",
        "ambient_context.py",
        "diagnostics.py",
        "doctor.py",
        "inbound_spool.py",
        "README.md",
    ] {
        std::fs::copy(from.join(file), plugin.join(file)).unwrap();
    }
    std::fs::copy(
        mdk.join("scripts/hermes_marmot_configure_gateway.py"),
        plugin.join("configure_gateway.py"),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Docker, and the network to build the image the first time"]
async fn a_hermes_install_moves_over() {
    let image = hermes_image();
    let model = Model::serve().await;
    let mut install = Install::new("hermes", "hermes-agent").await;
    let root = install.root().to_path_buf();
    let hermes_home = root.join(".hermes");
    std::fs::create_dir_all(&hermes_home).unwrap();
    let env = [
        format!("HERMES_HOME={}", hermes_home.display()),
        "SCRIPTED_MODEL_KEY=none".into(),
        // Our plugin's own log lines, by kind only (D-071), shown when a step fails.
        format!("MARMOT_LOG={}", root.join("plugin.log").display()),
    ];
    let name = format!("marmot-import-hermes-{}", std::process::id());
    let gateway = Image::new(&image, &root, &env, &name);
    gateway.wrapper(&root, "hermes");
    let hermes =
        |args: &[&str], what: &str| run(Std::new(root.join("bin/hermes")).args(args), what);

    // The person's Hermes, set up with its model.
    std::fs::write(
        hermes_home.join("config.yaml"),
        format!(
            "model:\n  default: scripted\n  provider: custom\n  base_url: {}\n  \
             key_env: SCRIPTED_MODEL_KEY\n  context_length: 128000\n",
            model.url
        ),
    )
    .unwrap();

    // Today's installer: wn-agent's service, the plugin, enabled with Hermes's own command, and
    // the gateway set up by the plugin's configure_gateway.py with the installer's arguments. The
    // person is let in to invite the agent (--allow-welcomer) and to write to it (--allow-user).
    install.write_unit("hermes");
    let plugin = hermes_home.join("plugins/marmot");
    old_hermes_plugin(&plugin);
    hermes(
        &["plugins", "enable", "marmot"],
        "hermes plugins enable marmot",
    );
    // MDK's configure_gateway.py needs PyYAML for a config.yaml that holds a list, as Hermes's
    // plugins list is, and Hermes does not bring it: the installer runs it with the python3 of the
    // machine, so here with one that has it.
    let h = hermes_home.display();
    let configure = format!(
        "pip install -q --disable-pip-version-check --no-cache-dir --target {r}/pyyaml pyyaml==6.0.3 && \
         PYTHONPATH={r}/pyyaml /usr/local/bin/python3 {p}/configure_gateway.py --home {h} \
         --agent-home {a} --socket-path {a}/dev/wn-agent.sock --account-id-hex {agent} \
         --tool-progress off --interim-messages 0 --long-running-notifications 0 \
         --busy-ack-detail 0 --configure-env --welcomer-allowlist {person} --allow-user {person}",
        r = root.display(),
        p = plugin.display(),
        a = install.home.display(),
        agent = install.agent,
        person = install.person,
    );
    println!(
        "{}",
        run(
            Std::new("docker").args(gateway.run(&["--rm"], &["sh", "-c", &configure])),
            "the old plugin's configure_gateway.py"
        )
    );

    // The gateway runs with the old plugin, and the person has a message answered through it.
    let old_gateway = gateway.start(&["hermes", "gateway", "run"], &root.join("plugin.log"));
    let (group, group_hex) = install.chat().await;
    install
        .exchange_before(&group, &group_hex, BEFORE, BEFORE_ANSWER)
        .await;
    let answered_before = install.answers(&group_hex, BEFORE_ANSWER);
    println!(
        "answered through the old plugin, {} model requests",
        model.requests().await
    );

    // The move. The gateway keeps running meanwhile, as it does on the person's machine.
    install.stop_connector().await;
    let settings = install.import(Connector::Hermes, "wn-agent-hermes.service");
    install.install("hermes");
    let kept = std::fs::read_dir(install.agent_folder("hermes").join("backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("marmot.")
        })
        .expect("the old plugin is kept aside");
    assert!(
        std::fs::read_to_string(kept.join("plugin.yaml"))
            .unwrap()
            .contains("wn-agent control socket"),
        "the old plugin's"
    );
    assert!(
        std::fs::read_to_string(plugin.join("plugin.yaml"))
            .unwrap()
            .contains("marmot-agent's local socket"),
        "ours, in its place"
    );
    let pointed = hermes(
        &["config", "get", "platforms.marmot.home"],
        "hermes config get",
    );
    assert_eq!(pointed.trim(), install.home.display().to_string());

    let mut core = install.start_core("hermes", &settings).await;
    // The person's gateway is restarted, as the installer says, and loads our plugin.
    old_gateway.stop().await;
    let moved = model.requests().await;
    let _new_gateway = gateway.start(&["hermes", "gateway", "run"], &root.join("plugin.log"));
    install.marked(&group_hex).await;

    install
        .exchange_after(&group, &group_hex, AFTER, AFTER_ANSWER)
        .await;
    install.nothing_from_before(&model, moved).await;
    assert_eq!(install.answers(&group_hex, BEFORE_ANSWER), answered_before);

    // The model switched from the chat, and back. The plugin types `/model` for Hermes and tells
    // from its reply that it was taken, by Hermes's own catalog of replies (`agent.i18n.t`),
    // which Hermes does not publish for plugins: this is what fails when that changes.
    install
        .set_model(&group, &group_hex, "scripted-two", "scripted-two")
        .await;
    install
        .set_model(&group, &group_hex, "reset", "scripted")
        .await;

    let _ = core.kill().await;
    install.end().await;
}

/// The repository's OpenClaw image, built as `packages/openclaw/run.sh` builds it, when it is not
/// there.
fn openclaw_image() -> String {
    let image = "marmot-agent-openclaw".to_string();
    let there = Std::new("docker")
        .args(["image", "inspect", &image])
        .output()
        .expect("docker runs");
    if !there.status.success() {
        let package = repository().join("packages/openclaw");
        run(
            Std::new("docker")
                .arg("build")
                .arg("-f")
                .arg(package.join("container/Dockerfile"))
                .arg("-t")
                .arg(&image)
                .arg(&package),
            "building the OpenClaw image",
        );
    }
    image
}

/// The OpenClaw plugin of today's connector, packed as MDK's release workflow packs it
/// (`openclaw-marmot-plugin`: `pnpm install --frozen-lockfile`, built, `pnpm pack`), from MDK's
/// sources at the revision the core uses, in a Node container. Kept in the build's temporary
/// folder, by revision, for the next run.
fn old_openclaw_plugin() -> PathBuf {
    let mdk = mdk();
    let revision = mdk.file_name().unwrap().to_string_lossy().into_owned();
    let kept = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("openclaw-marmot-{revision}"));
    let packed = kept.join("openclaw-marmot-plugin.tgz");
    if packed.exists() {
        return packed;
    }
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("src");
    run(
        Std::new("cp")
            .arg("-R")
            .arg(mdk.join("integrations/openclaw/marmot"))
            .arg(&source),
        "copying the old plugin's sources",
    );
    let (uid, gid) = ids();
    let build = "npm install -g --prefix /w/pnpm pnpm@10.13.1 >/dev/null && cd /w/src && \
                 /w/pnpm/bin/pnpm install --frozen-lockfile >/dev/null && /w/pnpm/bin/pnpm build >/dev/null && \
                 /w/pnpm/bin/pnpm pack --pack-destination /w >/dev/null";
    run(
        Std::new("docker")
            .args(["run", "--rm", "--log-driver", "none", "--user"])
            .arg(format!("{uid}:{gid}"))
            .args([
                "-e",
                "HOME=/w/home",
                "-e",
                "npm_config_cache=/w/home/.npm",
                "--mount",
            ])
            .arg(format!(
                "type=bind,source={},target=/w",
                work.path().display()
            ))
            .args(["node:24.22.1-bookworm-slim", "sh", "-c", build]),
        "packing the old OpenClaw plugin",
    );
    let tgz = (std::fs::read_dir(work.path()).unwrap())
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "tgz"))
        .expect("pnpm pack made the plugin's archive");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::copy(tgz, &packed).unwrap();
    packed
}

/// Changes OpenClaw's settings file as `change` says.
fn edit_json(file: &Path, change: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    change(&mut value);
    std::fs::write(file, serde_json::to_string_pretty(&value).unwrap() + "\n").unwrap();
}

/// A port on the loopback address nothing listens on now, for the gateway's own. It is let go of
/// before the gateway takes it, so it is one the system does not hand out by itself, below its
/// range of ports for the other end of a connection (`ip_local_port_range`): only a program that
/// asks for it by number can take it meanwhile, as a second scenario run beside this one could.
fn free_port() -> u16 {
    let lowest = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .ok()
        .and_then(|r| r.split_whitespace().next()?.parse::<u16>().ok())
        .unwrap_or(32768);
    let start = 20000 + (std::process::id() % 5000) as u16;
    (start..lowest)
        .chain(20000..start)
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .expect("a free port below the system's own range")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs Docker, and the network to build the image and the old plugin the first time"]
async fn an_openclaw_install_moves_over() {
    let image = openclaw_image();
    let old_plugin = old_openclaw_plugin();
    let model = Model::serve().await;
    let mut install = Install::new("openclaw", "openclaw-agent").await;
    let root = install.root().to_path_buf();
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let env = [
        format!("HOME={}", root.display()),
        format!("OPENCLAW_HOME={}", root.display()),
        "SCRIPTED_MODEL_KEY=none".into(),
    ];
    let name = format!("marmot-import-openclaw-{}", std::process::id());
    let gateway = Image::new(&image, &root, &env, &name);
    gateway.wrapper(&root, "openclaw");
    let openclaw =
        |args: &[&str], what: &str| run(Std::new(root.join("bin/openclaw")).args(args), what);

    // The person's OpenClaw, set up, with its model, its own port on this machine and the agent's
    // workspace.
    openclaw(
        &[
            "onboard",
            "--non-interactive",
            "--accept-risk",
            "--mode",
            "local",
            "--flow",
            "quickstart",
            "--suppress-gateway-token-output",
            "--no-install-daemon",
            "--skip-channels",
            "--skip-skills",
            "--skip-ui",
            "--skip-search",
            "--skip-hooks",
            "--skip-health",
            "--skip-bootstrap",
            "--auth-choice",
            "skip",
        ],
        "openclaw onboard",
    );
    let config = root.join(".openclaw/openclaw.json");
    let port = free_port();
    edit_json(&config, |c| {
        c["models"] = json!({"providers": {"scripted": {
            "baseUrl": model.url, "apiKey": "${SCRIPTED_MODEL_KEY}", "api": "openai-completions",
            "models": [{"id": "scripted", "name": "scripted", "reasoning": false, "input": ["text"],
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                "contextWindow": 128000, "maxTokens": 8192}],
        }}});
        c["agents"]["defaults"]["model"] = json!({"primary": "scripted/scripted"});
        c["agents"]["defaults"]["workspace"] = json!(work);
        if let Some(entries) = c["agents"]["entries"].as_object_mut() {
            for entry in entries.values_mut() {
                entry["workspace"] = json!(work);
            }
        }
        c["gateway"]["port"] = json!(port);
    });

    // Today's installer: wn-agent's service, the plugin installed and enabled with OpenClaw's own
    // commands, and the channel pointed at the home, as the installer's own lines set it, with
    // the person let in to invite the agent. The installer leaves who may write to the person,
    // who sets the sender policy to themself.
    install.write_unit("openclaw");
    // Downloaded, as the installer downloads the release's archive, to a folder of its own.
    let download = root.join("download/openclaw-marmot-plugin.tgz");
    std::fs::create_dir_all(download.parent().unwrap()).unwrap();
    std::fs::copy(&old_plugin, &download).unwrap();
    let packed = download.display().to_string();
    openclaw(
        &[
            "plugins",
            "install",
            "--force",
            "--accept-capabilities",
            &packed,
        ],
        "installing the old plugin",
    );
    openclaw(&["plugins", "enable", "marmot"], "enabling the old plugin");
    let home = install.home.display().to_string();
    let (agent, person) = (install.agent.clone(), install.person.clone());
    edit_json(&config, |c| {
        let existing = c["channels"]["marmot"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut channel = serde_json::Map::from_iter(existing);
        channel.insert("enabled".into(), json!(true));
        channel.insert("home".into(), json!(home));
        channel.insert(
            "socketPath".into(),
            json!(format!("{home}/dev/wn-agent.sock")),
        );
        channel.insert("accountIdHex".into(), json!(agent));
        channel.insert("dm".into(), json!({"allowFrom": [person]}));
        channel.insert("senderPolicy".into(), json!({"allowedUsers": [person]}));
        if !c["channels"].is_object() {
            c["channels"] = json!({});
        }
        c["channels"]["marmot"] = Value::Object(channel);
    });

    // The gateway runs with the old plugin, and the person has a message answered through it.
    let old_gateway = gateway.start(&["openclaw", "gateway", "run"], &root.join("plugin.log"));
    let (group, group_hex) = install.chat().await;
    install
        .exchange_before(&group, &group_hex, BEFORE, BEFORE_ANSWER)
        .await;
    let answered_before = install.answers(&group_hex, BEFORE_ANSWER);
    println!(
        "answered through the old plugin, {} model requests",
        model.requests().await
    );

    // The move. The gateway keeps running meanwhile, as it does on the person's machine.
    install.stop_connector().await;
    let settings = install.import(Connector::OpenClaw, "wn-agent-openclaw.service");
    install.install("openclaw");
    let kept = std::fs::read_dir(install.agent_folder("openclaw").join("backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("marmot.")
        })
        .expect("the old plugin is kept aside");
    let package = |folder: &Path| {
        let text = std::fs::read_to_string(folder.join("package.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["name"].clone()
    };
    assert_eq!(
        package(&kept),
        "@marmot-protocol/openclaw-marmot",
        "the old plugin's"
    );
    let channel = openclaw(
        &["config", "get", "channels.marmot", "--json"],
        "openclaw config get",
    );
    let channel: Value = serde_json::from_str(&channel).unwrap();
    assert_eq!(channel, json!({"enabled": true, "home": install.home}));

    let mut core = install.start_core("openclaw", &settings).await;
    // The person's gateway is restarted, as the installer says, and loads our plugin.
    old_gateway.stop().await;
    let moved = model.requests().await;
    let _new_gateway = gateway.start(&["openclaw", "gateway", "run"], &root.join("plugin.log"));
    install.marked(&group_hex).await;

    install
        .exchange_after(&group, &group_hex, AFTER, AFTER_ANSWER)
        .await;
    install.nothing_from_before(&model, moved).await;
    assert_eq!(install.answers(&group_hex, BEFORE_ANSWER), answered_before);

    let _ = core.kill().await;
    install.end().await;
}
