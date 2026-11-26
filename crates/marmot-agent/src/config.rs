//! The standing settings, one TOML file the core reads at start (D-073).
//!
//! ```toml
//! home = "/home/me/.local/share/marmot-agent/codex"   # the account home
//! name = "Codex"                       # the agent's name, as people see it
//! relays = ["wss://relay.example"]
//! owner = "<the owner's public key, in hex or as an npub>"
//! prompt = "owner"                     # or "everyone", or { people = ["<hex>", ...] }
//! approve = "owner"                    # the same choices
//! invite = ["<hex>"]                   # besides the owner, who may add the agent to a group
//! white_noise = true                   # false: only the relays above, for a local try
//! allow_loopback_relays = false        # true: relays on this machine may be used
//! files_from = ["/home/me/work/out"]   # where the agent may send files from; by default the
//!                                      # chat's folder, and none for a harness with no folder
//! send_files_from_agent_home = false   # true: files inside the account home may be sent too
//! folders = ["~/projects"]             # the folders a chat may move a coding harness to, and any
//!                                      # folder inside them; by default the harness's working
//!                                      # folder. `~` is the home: listing it allows any folder
//!                                      # in the home (D-101)
//! full_activity = ["<group id>"]       # the groups also sent the agent's reasoning, tools,
//!                                      # plans, tasks and notices, or "all"; by default only
//!                                      # the owner's two-person chat with it (D-136).
//!                                      # `marmot-agent groups <settings>`, with the agent
//!                                      # stopped, prints each group's id
//!
//! [codex]                              # Codex, built in; or, for a connector on the socket,
//! cwd = "/home/me/work"                # an empty [connector] section in its place
//! sandbox = "workspace-write"          # or "read-only", "danger-full-access"
//! approval_policy = "on-request"       # or "untrusted", "on-failure", "never"
//! ```
//!
//! An agent over ACP, built in, takes an `[acp]` section in place of `[codex]`:
//!
//! ```toml
//! [acp]
//! name = "opencode"                    # what events call the agent until it names itself
//! command = ["opencode", "acp"]        # or ["cursor-agent", "acp"], ["agy", "acp"], or Goose's container wrapper
//! cwd = "/home/me/work"
//! ```
//!
//! Pi, built in, takes a `[pi]` section in place of `[codex]`:
//!
//! ```toml
//! [pi]
//! cwd = "/home/me/work"
//! program = "pi"                       # or a script that runs Pi elsewhere, such as tools/pi/pi.sh
//! args = []                            # arguments for that script, before Pi's own
//! ```

use crate::activity::FullActivity;
use crate::keys::{PubKey, public_key_hex};
use crate::rights::{Rights, Who};
use marmot_agent_codex::{ApprovalPolicy, Sandbox, Setup};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The core's settings.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The account home: the agent's keys and encrypted storage.
    pub home: PathBuf,
    /// The agent's name, as people in its chats see it. Published in its profile each time the
    /// core starts, the rest of the profile kept; without one, the profile is left as it is.
    #[serde(default)]
    pub name: Option<String>,
    /// The relays the account uses.
    pub relays: Vec<String>,
    /// Who owns the agent.
    pub owner: PubKey,
    /// Who may prompt. The owner alone by default (D-010).
    #[serde(default)]
    pub prompt: Who,
    /// Who may approve. The owner alone by default (D-010).
    #[serde(default)]
    pub approve: Who,
    /// Who, besides the owner, may add the agent to a group.
    #[serde(default)]
    pub invite: Vec<PubKey>,
    /// Whether relays on this machine may be used, for trying the core against a local relay.
    #[serde(default)]
    pub allow_loopback_relays: bool,
    /// Whether to use White Noise's own relays and directories as well as `relays`. On by
    /// default; off, the core uses only the relays listed, as a try against a local relay should.
    #[serde(default = "yes")]
    pub white_noise: bool,
    /// The folders the agent may send files from (D-090). By default the chat's folder (D-101),
    /// and none for a harness that states no folder. A folder listed here may not be the account home, hold it or lie inside it: the
    /// core refuses to start on one, unless `send_files_from_agent_home` is set.
    #[serde(default)]
    pub files_from: Option<Vec<PathBuf>>,
    /// Whether the agent may send files inside the account home, which holds its keys, when an
    /// allowed folder holds it (D-099). Off by default: the core refuses every file whose real
    /// place is inside the home, wherever the allowed folders are.
    #[serde(default)]
    pub send_files_from_agent_home: bool,
    /// The folders a chat may move a coding harness to, each with every folder inside it (D-101).
    /// By default the harness's own working folder. A `~` at the start stands for the home, so
    /// listing `~` allows any folder in it; every other entry is a whole path. The agent's account
    /// home is never a chat's folder, whatever is listed.
    #[serde(default)]
    pub folders: Option<Vec<PathBuf>>,
    /// Which groups are sent the agent's full activity besides the owner's two-person chat with
    /// it: those listed, or every chat with `"all"`. None by default (D-136).
    #[serde(default)]
    pub full_activity: FullActivity,
    /// Codex, when the agent is Codex, built in.
    #[serde(default)]
    pub codex: Option<CodexConfig>,
    /// Pi, when the agent is Pi, built in.
    #[serde(default)]
    pub pi: Option<PiConfig>,
    /// A connector on the socket, such as the Claude Code driver, when the agent is not Codex.
    #[serde(default)]
    pub connector: Option<ConnectorConfig>,
    /// An agent over ACP, built in: OpenCode, Goose, Cursor or Antigravity.
    #[serde(default)]
    pub acp: Option<AcpConfig>,
}

/// How an agent over ACP runs.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcpConfig {
    /// What events call the agent until it names itself, such as `opencode`.
    pub name: String,
    /// The program that speaks ACP on its standard input and output, and its arguments.
    pub command: Vec<String>,
    /// The working folder.
    pub cwd: String,
    /// Whether the agent gets the driver's `send_file` tool (D-090), from an MCP server the driver
    /// serves on the loopback address (D-110). On unless set to `false`.
    #[serde(default = "on")]
    pub send_files: bool,
}

/// What `send_files` is when the settings leave it out.
fn on() -> bool {
    true
}

/// How Pi runs. Each chat gets a Pi of its own, started as `program args... --mode rpc` in `cwd`.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PiConfig {
    /// The program, `pi` by default, or a script that runs Pi elsewhere, such as in a container.
    #[serde(default = "pi")]
    pub program: String,
    /// Arguments for the program, before Pi's own.
    #[serde(default)]
    pub args: Vec<String>,
    /// The working folder.
    pub cwd: String,
}

/// A connector that drives the harness from a separate program, over the socket (D-003). It
/// needs no settings of its own here: it connects with the token the core writes.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorConfig {}

/// How Codex runs (D-061).
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexConfig {
    /// The program, `codex` by default.
    #[serde(default = "codex")]
    pub program: String,
    /// The working folder.
    pub cwd: String,
    /// The sandbox: `workspace-write` by default.
    #[serde(default)]
    pub sandbox: Option<String>,
    /// When Codex asks: `on-request` by default.
    #[serde(default)]
    pub approval_policy: Option<String>,
}

/// A folder from the settings as a whole path: one that starts with `~` is taken from `home`; any
/// other must be whole already.
fn whole(folder: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let mut parts = folder.components();
    match parts.next() {
        Some(std::path::Component::Normal(first)) if first == "~" => {
            let home = home.filter(|h| h.is_absolute())?;
            Some(home.join(parts.as_path()))
        }
        _ if folder.is_absolute() => Some(folder.to_path_buf()),
        _ => None,
    }
}

fn codex() -> String {
    "codex".into()
}

fn pi() -> String {
    "pi".into()
}

fn yes() -> bool {
    true
}

/// Why the settings could not be used. Prints no path and no value (D-071).
#[derive(Debug)]
pub enum ConfigError {
    /// The file could not be read.
    Unreadable(std::io::ErrorKind),
    /// The file is not valid TOML, or not these settings.
    Invalid,
    /// A setting has a value it does not take.
    Value(&'static str),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(kind) => write!(f, "the settings file could not be read ({kind})"),
            Self::Invalid => f.write_str("the settings file is not valid"),
            Self::Value(setting) => {
                write!(f, "the setting `{setting}` has a value it does not take")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Reads the settings from `path`.
    pub fn read(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Unreadable(e.kind()))?;
        Self::parse(&text)
    }

    /// Reads the settings from their text.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut config: Self = toml::from_str(text).map_err(|_| ConfigError::Invalid)?;
        // Every key as MDK gives it, lowercase hex, so the owner and the people listed are
        // recognised; anything else is refused here, not left to match nobody (S6).
        let read = |key: &mut PubKey, setting| match public_key_hex(key.as_str()) {
            Some(hex) => {
                *key = PubKey(hex);
                Ok(())
            }
            None => Err(ConfigError::Value(setting)),
        };
        read(&mut config.owner, "owner")?;
        for key in &mut config.invite {
            read(key, "invite")?;
        }
        for (who, setting) in [
            (&mut config.prompt, "prompt"),
            (&mut config.approve, "approve"),
        ] {
            if let Who::People(people) = who {
                for key in people {
                    read(key, setting)?;
                }
            }
        }
        if config.name.as_ref().is_some_and(|n| n.trim().is_empty()) {
            return Err(ConfigError::Value("name"));
        }
        let drivers = [
            config.codex.is_some(),
            config.connector.is_some(),
            config.acp.is_some(),
            config.pi.is_some(),
        ];
        if drivers.iter().filter(|d| **d).count() != 1 {
            return Err(ConfigError::Value(
                "codex, acp, pi or connector, one of them",
            ));
        }
        if let Some(acp) = &config.acp {
            if acp.command.is_empty() {
                return Err(ConfigError::Value("acp.command"));
            }
            if acp.name.trim().is_empty() {
                return Err(ConfigError::Value("acp.name"));
            }
        }
        if config.codex.is_some() {
            config.setup()?;
        }
        if let Some(folders) = &mut config.folders {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            for folder in folders {
                *folder = whole(folder, home.as_deref()).ok_or(ConfigError::Value("folders"))?;
            }
        }
        Ok(config)
    }

    /// The folders the agent may send files from: those listed, or else the harness's working
    /// folder (D-090).
    pub fn file_folders(&self) -> Vec<PathBuf> {
        if let Some(listed) = &self.files_from {
            return listed.clone();
        }
        let cwd = (self.codex.as_ref().map(|c| &c.cwd))
            .or(self.acp.as_ref().map(|a| &a.cwd))
            .or(self.pi.as_ref().map(|p| &p.cwd));
        cwd.map(PathBuf::from).into_iter().collect()
    }

    /// Who may do what.
    pub fn rights(&self) -> Rights {
        Rights {
            owner: self.owner.clone(),
            prompt: self.prompt.clone(),
            approve: self.approve.clone(),
        }
    }

    /// Whether `person` may add the agent to a group: the owner, and the people listed.
    pub fn may_invite(&self, person: &PubKey) -> bool {
        *person == self.owner || self.invite.contains(person)
    }

    /// Pi's setup, when the agent is Pi.
    pub fn pi_setup(&self) -> Option<marmot_agent_pi::Setup> {
        let pi = self.pi.as_ref()?;
        Some(marmot_agent_pi::Setup {
            program: pi.program.clone(),
            args: pi.args.clone(),
            cwd: pi.cwd.clone().into(),
        })
    }

    /// Codex's setup, when the agent is Codex.
    pub fn setup(&self) -> Result<Setup, ConfigError> {
        let codex = self.codex.as_ref().ok_or(ConfigError::Value("codex"))?;
        let mut setup = Setup::new(codex.cwd.clone());
        if let Some(word) = &codex.sandbox {
            setup.sandbox = match word.as_str() {
                "read-only" => Sandbox::ReadOnly,
                "workspace-write" => Sandbox::WorkspaceWrite,
                "danger-full-access" => Sandbox::DangerFullAccess,
                _ => return Err(ConfigError::Value("codex.sandbox")),
            };
        }
        if let Some(word) = &codex.approval_policy {
            setup.approval_policy = match word.as_str() {
                "untrusted" => ApprovalPolicy::Untrusted,
                "on-failure" => ApprovalPolicy::OnFailure,
                "on-request" => ApprovalPolicy::OnRequest,
                "never" => ApprovalPolicy::Never,
                _ => return Err(ConfigError::Value("codex.approval_policy")),
            };
        }
        Ok(setup)
    }
}
