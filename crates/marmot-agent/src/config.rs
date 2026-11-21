//! The standing settings, one TOML file the core reads at start (D-073).
//!
//! ```toml
//! home = "/home/me/.local/share/marmot-agent/codex"   # the account home
//! name = "Codex"                       # the agent's name, as people see it
//! relays = ["wss://relay.example"]
//! owner = "<the owner's public key, in hex>"
//! prompt = "owner"                     # or "everyone", or { people = ["<hex>", ...] }
//! approve = "owner"                    # the same choices
//! invite = ["<hex>"]                   # besides the owner, who may add the agent to a group
//! white_noise = true                   # false: only the relays above, for a local try
//! allow_loopback_relays = false        # true: relays on this machine may be used
//!
//! [codex]
//! cwd = "/home/me/work"
//! sandbox = "workspace-write"          # or "read-only", "danger-full-access"
//! approval_policy = "on-request"       # or "untrusted", "on-failure", "never"
//! ```

use crate::keys::PubKey;
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
    /// Codex.
    pub codex: CodexConfig,
}

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

fn codex() -> String {
    "codex".into()
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
        let config: Self = toml::from_str(text).map_err(|_| ConfigError::Invalid)?;
        if config.name.as_ref().is_some_and(|n| n.trim().is_empty()) {
            return Err(ConfigError::Value("name"));
        }
        config.setup()?;
        Ok(config)
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

    /// Codex's setup.
    pub fn setup(&self) -> Result<Setup, ConfigError> {
        let mut setup = Setup::new(self.codex.cwd.clone());
        if let Some(word) = &self.codex.sandbox {
            setup.sandbox = match word.as_str() {
                "read-only" => Sandbox::ReadOnly,
                "workspace-write" => Sandbox::WorkspaceWrite,
                "danger-full-access" => Sandbox::DangerFullAccess,
                _ => return Err(ConfigError::Value("codex.sandbox")),
            };
        }
        if let Some(word) = &self.codex.approval_policy {
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
