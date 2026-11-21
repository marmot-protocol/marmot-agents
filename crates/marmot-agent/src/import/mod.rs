//! `marmot-agent import`: takes over one of today's connector installs (D-089).
//!
//! The agent keeps its account home where it is, so it keeps its identity, its keys, its
//! encryption state and every group it is in. The import reads the old install without changing
//! it and makes a plan: which connector it is, its one identity, its relays, and how its
//! allow-lists become the owner, who may prompt, who may approve and who may invite (D-010). Then
//! it stops and disables the old services, keeping their files; takes the home's runtime lease,
//! so nothing else runs on it; backs up the whole home, since MDK upgrades its storage forward
//! only and the core has not opened it yet; and writes the settings (D-073). Nothing here opens
//! the store.
//!
//! The rule throughout: the new settings never let anyone do more than the old install let them.
//! Where the old lists disagree, only what they all allow is kept; where what the old install
//! allowed cannot be read for certain, it counts as allowing nobody; and where the owner is not
//! the one person the old install let both invite and prompt the agent, it must be named.
//!
//! - [`services`]: finding, reading and stopping the old services.
//! - [`backup`]: the checked, owner-only copy of the home.

pub mod backup;
pub mod services;

use crate::config::Config;
use crate::keys::PubKey;
use crate::rights::Who;
use marmot_account::AccountHome;
use serde::Deserialize;
use services::{Role, Service, Services};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The White Noise relays `wn-agent` uses when it is given none.
const DEFAULT_RELAYS: [&str; 2] = [
    "wss://relay.eu.whitenoise.chat",
    "wss://relay.us.whitenoise.chat",
];

/// One of today's connectors, as MDK's installers set them up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connector {
    /// `wn-claude`, a terminal harness.
    Claude,
    /// `wn-codex`, a terminal harness.
    Codex,
    /// `wn-opencode`, a terminal harness.
    OpenCode,
    /// `wn-pi`, a terminal harness.
    Pi,
    /// `wn-goose`, a terminal harness.
    Goose,
    /// The Hermes platform plugin.
    Hermes,
    /// The OpenClaw channel plugin.
    OpenClaw,
    /// `wn-agent` alone, for a client of its own socket.
    Custom,
}

const CONNECTORS: [Connector; 8] = [
    Connector::Claude,
    Connector::Codex,
    Connector::OpenCode,
    Connector::Pi,
    Connector::Goose,
    Connector::Hermes,
    Connector::OpenClaw,
    Connector::Custom,
];

impl Connector {
    /// The name `--harness` takes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::Goose => "goose",
            Self::Hermes => "hermes",
            Self::OpenClaw => "openclaw",
            Self::Custom => "custom",
        }
    }

    /// The name people know it by.
    pub fn title(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
            Self::Goose => "Goose",
            Self::Hermes => "Hermes",
            Self::OpenClaw => "OpenClaw",
            Self::Custom => "wn-agent for a custom client",
        }
    }

    /// The connector `--harness` names.
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        let name = if name == "standalone" {
            "custom"
        } else {
            &name
        };
        CONNECTORS.into_iter().find(|c| c.name() == name)
    }

    /// The home's folder name the installer picks by default.
    fn folder(self) -> &'static str {
        match self {
            Self::OpenCode => "harnesses",
            Self::Custom => "standalone",
            other => other.name(),
        }
    }

    /// The account label the installer picks by default.
    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude-harness-agent",
            Self::Codex => "codex-harness-agent",
            Self::OpenCode => "terminal-harness-agent",
            Self::Pi => "pi-harness-agent",
            Self::Goose => "goose-harness-agent",
            Self::Hermes => "hermes-agent",
            Self::OpenClaw => "openclaw-agent",
            Self::Custom => "standalone-agent",
        }
    }

    /// The prefix of a terminal harness's own settings, such as `WN_CODEX`.
    fn env_prefix(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("WN_CLAUDE"),
            Self::Codex => Some("WN_CODEX"),
            Self::OpenCode => Some("WN_OPENCODE"),
            Self::Pi => Some("WN_PI"),
            Self::Goose => Some("WN_GOOSE"),
            _ => None,
        }
    }

    /// The harness program a terminal harness runs by default.
    fn program(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            other => other.name(),
        }
    }
}

/// Where the import looks for what lives outside the home.
#[derive(Clone)]
pub struct Places {
    /// The person's own folder: the working folder by default, and `%h` in a systemd unit.
    pub user_home: PathBuf,
    /// Where systemd user units are.
    pub systemd_units: PathBuf,
    /// Where LaunchAgents are.
    pub launch_agents: PathBuf,
    /// Hermes's home, whose `.env` holds its allow-list.
    pub hermes_home: PathBuf,
    /// OpenClaw's settings file, which holds its allow-list.
    pub openclaw_config: PathBuf,
}

impl Places {
    /// The places for the user running the import, from `HOME`, `XDG_CONFIG_HOME`,
    /// `HERMES_HOME` and `OPENCLAW_HOME` as the installers read them.
    pub fn of_this_user() -> Result<Self, String> {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let home = PathBuf::from(var("HOME").ok_or("HOME is not set")?);
        let config = var("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
        let openclaw = var("OPENCLAW_HOME").map_or_else(|| home.clone(), PathBuf::from);
        Ok(Self {
            systemd_units: config.join("systemd/user"),
            launch_agents: home.join("Library/LaunchAgents"),
            hermes_home: var("HERMES_HOME").map_or_else(|| home.join(".hermes"), PathBuf::from),
            openclaw_config: openclaw.join(".openclaw/openclaw.json"),
            user_home: home,
        })
    }
}

/// What the person asked for.
#[derive(Clone)]
pub struct Options {
    /// The old account home.
    pub home: PathBuf,
    /// The connector, when the home does not say.
    pub harness: Option<String>,
    /// The owner, when the old install does not establish one.
    pub owner: Option<String>,
    /// The harness's working folder; the person's own folder by default.
    pub cwd: Option<String>,
    /// Where the settings go; `marmot-agent.toml` in the home by default.
    pub settings: Option<PathBuf>,
    /// Services that name the home but do not run the old connector's programs, which the
    /// person has said to leave running.
    pub leave_running: Vec<String>,
    /// Where to look outside the home.
    pub places: Places,
    /// The time, which names the backup.
    pub now: SystemTime,
}

/// What the import will do.
pub struct Plan {
    /// The connector.
    pub connector: Connector,
    /// How the connector was told.
    pub found_by: &'static str,
    /// The home, which stays where it is.
    pub home: PathBuf,
    /// The agent's own public key.
    pub agent: PubKey,
    /// Who owns it.
    pub owner: PubKey,
    /// Who may prompt.
    pub prompt: Who,
    /// Who may approve.
    pub approve: Who,
    /// Who, besides the owner, may invite it.
    pub invite: Vec<PubKey>,
    /// Its relays.
    pub relays: Vec<String>,
    /// Whether it used relays on this machine.
    pub allow_loopback_relays: bool,
    /// The folders the agent may send files from: those the old connector allowed.
    pub files_from: Vec<String>,
    /// The harness's section of the settings.
    pub section: String,
    /// The old connector's services, to stop and disable.
    pub services: Vec<Service>,
    /// Other services that name the home, which the person said to leave running.
    pub left_running: Vec<Service>,
    /// The old connector's socket, which must be dead before the backup.
    pub old_socket: PathBuf,
    /// Where the backup goes.
    pub backup: PathBuf,
    /// Where the settings go.
    pub settings: PathBuf,
    /// The settings, as they will be written.
    pub settings_text: String,
    /// What was carried over, in words.
    pub mapped: Vec<String>,
    /// What was not, in words.
    pub not_mapped: Vec<String>,
    /// What the person does after the import.
    pub next: Vec<String>,
}

/// Keys, relay addresses, paths and the settings stay out of logs (D-071): only the shape is shown.
impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("connector", &self.connector)
            .field("relays", &self.relays.len())
            .field("invite", &self.invite.len())
            .field("files_from", &self.files_from.len())
            .field("services", &self.services)
            .finish_non_exhaustive()
    }
}

/// The invite policy, as `wn-agent` reads it from its allow-list record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InvitePolicy {
    Deny,
    #[default]
    Allowlist,
    #[serde(alias = "any-authenticated-direct")]
    AnyAuthenticatedDirect,
    #[serde(alias = "any-authenticated")]
    AnyAuthenticated,
}

/// The old welcomer allow-list, as `wn-agent` keeps it in `dev/agent-allowlist/<account>.json`.
#[derive(Deserialize)]
struct AllowlistRecord {
    account_id_hex: String,
    #[serde(default)]
    invite_policy: InvitePolicy,
    welcomer_account_ids_hex: Vec<String>,
}

/// Who the old install let do something.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Allowed {
    /// These people, and nobody else; none at all when empty.
    People(BTreeSet<PubKey>),
    /// Anyone.
    Everyone,
}

impl Allowed {
    fn nobody() -> Self {
        Self::People(BTreeSet::new())
    }

    /// What both allow.
    fn and(&self, other: &Allowed) -> Allowed {
        match (self, other) {
            (Self::Everyone, x) | (x, Self::Everyone) => x.clone(),
            (Self::People(a), Self::People(b)) => {
                Self::People(a.intersection(b).cloned().collect())
            }
        }
    }

    fn lets(&self, person: &PubKey) -> bool {
        match self {
            Self::Everyone => true,
            Self::People(people) => people.contains(person),
        }
    }
}

/// Who the old install let write to the agent, and where that was read.
struct Senders {
    allowed: Allowed,
    /// The places it was read from, in words; empty when it could not be established.
    from: Vec<String>,
}

/// Reads the old install, changing nothing, and works out what the import will do. `services`
/// is asked how the old services ran.
pub fn plan(options: &Options, services: &mut dyn Services) -> Result<Plan, String> {
    let home = &options.home;
    if !home.is_dir() {
        return Err("the old home is not a folder".into());
    }
    if !home.is_absolute() {
        return Err("give the old home as an absolute path".into());
    }
    let accounts = AccountHome::open(home)
        .accounts()
        .map_err(|_| "the home's accounts could not be read")?;
    let signing: Vec<_> = accounts.into_iter().filter(|a| a.local_signing).collect();
    let account = match signing.as_slice() {
        [one] => one.clone(),
        [] => {
            return Err("the home holds no identity of its own; is it a connector's home?".into());
        }
        several => {
            return Err(format!(
                "the home holds {} identities, and the core runs every identity in its home. Each \
                 identity is its own program (D-072), so a home with several cannot be taken \
                 over in place; nothing was changed",
                several.len()
            ));
        }
    };
    let agent = PubKey(account.account_id_hex.clone());
    let places = &options.places;
    let found = services::find(
        home,
        &places.systemd_units,
        &places.launch_agents,
        &places.user_home,
        services,
    );
    if let Some((name, doubt)) = found
        .iter()
        .filter(|s| s.role != Role::Other)
        .find_map(|s| Some((&s.name, s.doubt.as_ref()?)))
    {
        return Err(format!(
            "{name} runs the old connector on this home, but how it ran cannot be read for              certain: {doubt}. Write its settings out plainly in its unit (no `$` references or              patterns, every environment file readable), run `systemctl --user daemon-reload` and              restart it, then run the import again; or remove it if it is not in use. Nothing              was changed"
        ));
    }
    let (others, services): (Vec<Service>, Vec<Service>) =
        found.into_iter().partition(|s| s.role == Role::Other);
    let unnamed: Vec<&str> = others
        .iter()
        .filter(|s| !options.leave_running.contains(&s.name))
        .map(|s| s.name.as_str())
        .collect();
    if !unnamed.is_empty() {
        return Err(format!(
            "these services name the home but do not run the old connector's programs, so the \
             import does not stop them: {}. If they must stop, stop them yourself; if they may \
             keep running, say so with `--leave-running <name>` for each. Nothing was changed",
            unnamed.join(", ")
        ));
    }
    if let Some(name) = options
        .leave_running
        .iter()
        .find(|n| !others.iter().any(|s| &s.name == *n))
    {
        return Err(format!(
            "`--leave-running {name}` names no other service found on this home"
        ));
    }
    let connectors: Vec<&Service> = services
        .iter()
        .filter(|s| s.role == Role::Connector)
        .collect();
    if connectors.len() > 1 {
        return Err(format!(
            "{} wn-agent services run on this home, so which settings it ran with is not clear; \
             remove the ones not in use and run the import again. Nothing was changed",
            connectors.len()
        ));
    }
    let connector_service = connectors.first().copied();
    let (connector, found_by) = detect(options, &account.label, &services)?;

    let mut mapped = Vec::new();
    let mut not_mapped = Vec::new();

    // Who may invite, and who may write to the agent.
    let (policy, welcomers) = read_allowlist(home, &agent, &mut not_mapped);
    let senders = read_senders(connector, home, places, &services, &mut not_mapped);
    let env = harness_env(connector, home, &services);

    // The owner prompts, approves and invites, so only someone the old install let both invite
    // and prompt the agent becomes the owner unasked.
    let may_invite = match policy {
        InvitePolicy::Allowlist => Allowed::People(welcomers.clone()),
        InvitePolicy::AnyAuthenticated => Allowed::Everyone,
        // Anyone, but into a chat of two only, which the core has no setting for.
        InvitePolicy::Deny | InvitePolicy::AnyAuthenticatedDirect => Allowed::nobody(),
    };
    let owner = match &options.owner {
        Some(given) => {
            let owner = parse_key(given).ok_or("`--owner` is not a public key")?;
            mapped.push("the owner: `--owner`".into());
            if !may_invite.lets(&owner) {
                not_mapped.push(
                    "the owner named with `--owner` may now invite the agent, which the old \
                     install did not let them do"
                        .into(),
                );
            }
            if !senders.allowed.lets(&owner) {
                not_mapped.push(
                    "the owner named with `--owner` may now prompt the agent and answer its \
                     questions, which the old install did not let them do"
                        .into(),
                );
            }
            owner
        }
        None => match may_invite.and(&senders.allowed) {
            Allowed::People(people) => {
                let count = people.len();
                let mut people = people.into_iter();
                match (people.next(), people.next()) {
                    (Some(one), None) => {
                        mapped.push(
                            "the owner: the one person the old install let both invite and \
                             prompt the agent"
                                .into(),
                        );
                        one
                    }
                    _ => {
                        return Err(format!(
                            "the old install let {count} people both invite and prompt the \
                             agent, so who owns it is not clear; name the owner with `--owner \
                             <npub or hex>`. Nothing was changed"
                        ));
                    }
                }
            }
            Allowed::Everyone => {
                return Err(
                    "the old install let anyone invite and prompt the agent, so who owns it is \
                     not clear; name the owner with `--owner <npub or hex>`. Nothing was changed"
                        .into(),
                );
            }
        },
    };

    let others_than_owner = |people: &BTreeSet<PubKey>| -> Vec<PubKey> {
        people.iter().filter(|p| **p != owner).cloned().collect()
    };
    let invite = match policy {
        InvitePolicy::Allowlist => others_than_owner(&welcomers),
        InvitePolicy::AnyAuthenticated => {
            not_mapped.push(
                "the invite policy let anyone invite the agent; the core has no such setting, so \
                 only the owner and the people on the old allow-list may invite it"
                    .into(),
            );
            others_than_owner(&welcomers)
        }
        InvitePolicy::AnyAuthenticatedDirect => {
            not_mapped.push(
                "the invite policy let anyone invite the agent into a chat of two; the core has \
                 no such setting, so only the owner may invite it"
                    .into(),
            );
            Vec::new()
        }
        InvitePolicy::Deny => {
            not_mapped.push("invites were refused from everyone; only the owner may invite".into());
            Vec::new()
        }
    };
    mapped.push(format!(
        "who may invite: the owner and {} other people from the old allow-list",
        invite.len()
    ));

    let prompt = match &senders.allowed {
        Allowed::Everyone => {
            mapped.push(format!(
                "who may prompt: everyone in the chat, as {} allowed",
                senders.from.join(" and ")
            ));
            Who::Everyone
        }
        Allowed::People(people) => {
            let people = others_than_owner(people);
            if !senders.from.is_empty() {
                mapped.push(format!(
                    "who may prompt: the owner and {} other people, as {} allowed",
                    people.len(),
                    senders.from.join(" and ")
                ));
            }
            if people.is_empty() {
                Who::Owner
            } else {
                Who::People(people)
            }
        }
    };
    not_mapped.push(approve_note(connector).into());

    // Relays, as the old service ran.
    let (relays, allow_loopback_relays) = match connector_service {
        Some(service) => {
            // wn-agent splits each `--relay` at commas.
            let mut relays: Vec<String> = service
                .values_of("--relay")
                .iter()
                .flat_map(|r| csv(r))
                .collect();
            let mut from = "the old service's command line";
            if relays.is_empty()
                && let Some(list) = service
                    .env("MARMOT_RELAYS")
                    .or_else(|| service.env("MARMOT_RELAY"))
            {
                relays = csv(list);
                from = "the old service's environment";
            }
            if relays.is_empty() && !service.has("--no-default-relays") {
                relays = DEFAULT_RELAYS.map(String::from).to_vec();
                from = "wn-agent's defaults, as the old service named none";
            }
            mapped.push(format!("the relays: {}, from {from}", relays.len()));
            (relays, service.has("--allow-loopback-relays"))
        }
        None => {
            not_mapped.push(
                "no service runs on this home, so its relays could not be read; the White \
                 Noise relays wn-agent uses by default are set"
                    .into(),
            );
            (DEFAULT_RELAYS.map(String::from).to_vec(), false)
        }
    };
    if relays.is_empty() {
        return Err(
            "the old service ran with no relays (`--no-default-relays` and none named), so it              was offline; the core needs relays. Nothing was changed"
                .into(),
        );
    }

    let backup = backup::place_for(home, options.now);
    let files_from = files_from(
        connector_service,
        home,
        &backup,
        &mut mapped,
        &mut not_mapped,
    );

    let cwd = options
        .cwd
        .clone()
        .unwrap_or_else(|| options.places.user_home.to_string_lossy().into_owned());
    let section = section(connector, &cwd, &env, &mut mapped, &mut not_mapped);

    let old_socket = connector_service
        .and_then(|s| s.values_of("--socket").pop())
        .map_or_else(|| home.join("dev/wn-agent.sock"), PathBuf::from);
    let settings = options
        .settings
        .clone()
        .unwrap_or_else(|| home.join("marmot-agent.toml"));
    if std::fs::symlink_metadata(&settings).is_ok() {
        return Err(format!(
            "the settings file {} is already there; move it away or give another with \
             `--settings`",
            settings.display()
        ));
    }
    if std::fs::symlink_metadata(&backup).is_ok() {
        return Err("the backup folder is already there".into());
    }

    let mut plan = Plan {
        connector,
        found_by,
        home: home.clone(),
        agent,
        owner,
        prompt,
        approve: Who::Owner,
        invite,
        relays,
        allow_loopback_relays,
        files_from,
        section,
        services,
        left_running: others,
        old_socket,
        backup,
        settings,
        settings_text: String::new(),
        mapped,
        not_mapped,
        next: Vec::new(),
    };
    plan.settings_text = settings_text(&plan, options.now);
    Config::parse(&plan.settings_text).map_err(|e| format!("the settings made are wrong: {e}"))?;
    plan.next = next_steps(&plan, &cwd);
    for partial in backup::partials_of(home) {
        plan.next.push(format!(
            "remove {}, a backup an earlier import did not finish",
            partial.display()
        ));
    }
    Ok(plan)
}

/// The folders the agent may send files from: the old connector's `--media-allowed-root`
/// folders, never the working folder by default, and never one that is, holds or lies inside
/// the account home or its backup (D-090). Each is found as `wn-agent` found it: its `..` taken
/// away by the words first, then its links followed; one `wn-agent` could not find is left out.
fn files_from(
    service: Option<&Service>,
    home: &Path,
    backup: &Path,
    mapped: &mut Vec<String>,
    not_mapped: &mut Vec<String>,
) -> Vec<String> {
    let roots = service.map_or_else(Vec::new, |s| s.values_of("--media-allowed-root"));
    let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let home = resolve(home);
    let backup = match (backup.parent(), backup.file_name()) {
        (Some(parent), Some(name)) => resolve(parent).join(name),
        _ => backup.to_path_buf(),
    };
    let mut kept = Vec::new();
    let mut refused = 0;
    for root in &roots {
        let path = Path::new(root);
        if !path.is_absolute() {
            refused += 1;
            continue;
        }
        // `link/../out` is `out` beside the link, not beside where the link leads.
        let Ok(path) = std::fs::canonicalize(lexical(path)) else {
            refused += 1;
            continue;
        };
        let overlaps = |other: &Path| path.starts_with(other) || other.starts_with(&path);
        if overlaps(&home) || overlaps(&backup) {
            refused += 1;
            continue;
        }
        let root = path.to_string_lossy().into_owned();
        if !kept.contains(&root) {
            kept.push(root);
        }
    }
    if kept.is_empty() && refused == 0 {
        mapped.push(
            "the folders the agent may send files from: none, as the old connector allowed none"
                .into(),
        );
    } else {
        mapped.push(format!(
            "the folders the agent may send files from: {}, the old connector's \
             `--media-allowed-root` folders",
            kept.len()
        ));
    }
    if refused > 0 {
        not_mapped.push(format!(
            "{refused} of the old connector's media folders were left out, being relative, not \
             there, or being, holding or lying inside the account home or its backup"
        ));
    }
    kept
}

/// A path with `.` and `..` taken away by the words alone, as `wn-agent` takes them away before
/// it follows any link.
fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Which connector the home belongs to, and how that was told.
fn detect(
    options: &Options,
    label: &str,
    services: &[Service],
) -> Result<(Connector, &'static str), String> {
    if let Some(name) = &options.harness {
        let connector = Connector::from_name(name).ok_or(
            "`--harness` takes claude, codex, opencode, pi, goose, hermes, openclaw or custom",
        )?;
        return Ok((connector, "`--harness`"));
    }
    let home = &options.home;
    let by_env: Vec<Connector> = CONNECTORS
        .into_iter()
        .filter(|c| c.env_prefix().is_some())
        .filter(|c| home.join(format!("dev/wn-{}.env", c.name())).is_file())
        .collect();
    if let [one] = by_env.as_slice() {
        return Ok((*one, "the harness's settings in the home"));
    }
    let by_service: Vec<Connector> = CONNECTORS
        .into_iter()
        .filter(|c| c.env_prefix().is_some())
        .filter(|c| services.iter().any(|s| runs_harness(s, *c)))
        .collect();
    if let [one] = by_service.as_slice() {
        return Ok((*one, "the harness's service"));
    }
    if let Some(c) = CONNECTORS.into_iter().find(|c| c.label() == label) {
        return Ok((c, "the account's label"));
    }
    let folder = home.file_name().map(|f| f.to_string_lossy().into_owned());
    if let Some(c) = CONNECTORS
        .into_iter()
        .find(|c| Some(c.folder()) == folder.as_deref())
    {
        return Ok((c, "the home's folder name"));
    }
    Err("which connector this home belongs to could not be told; name it with `--harness`".into())
}

fn runs_harness(service: &Service, connector: Connector) -> bool {
    service.role == Role::Harness
        && service.program().as_deref() == Some(format!("wn-{}", connector.name()).as_str())
}

/// The invite policy and the welcomers, read as `wn-agent` reads them.
fn read_allowlist(
    home: &Path,
    agent: &PubKey,
    not_mapped: &mut Vec<String>,
) -> (InvitePolicy, BTreeSet<PubKey>) {
    let path = home
        .join("dev/agent-allowlist")
        .join(format!("{}.json", agent.as_str()));
    let record = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<AllowlistRecord>(&bytes).ok(),
        Err(_) => {
            not_mapped.push("the home has no invite allow-list, so nobody could invite".into());
            return (InvitePolicy::Allowlist, BTreeSet::new());
        }
    };
    // wn-agent reads a damaged record, or one for another account, as an empty list.
    let Some(record) = record.filter(|r| r.account_id_hex == agent.as_str()) else {
        not_mapped.push(
            "the invite allow-list is damaged, so wn-agent let nobody invite, and nor does the \
             import"
                .into(),
        );
        return (InvitePolicy::Allowlist, BTreeSet::new());
    };
    // wn-agent compares each entry with the welcomer's key in lowercase hex, so an entry in any
    // other form, such as an npub, never let anyone in.
    let mut people = BTreeSet::new();
    let mut unmatched = 0;
    for entry in &record.welcomer_account_ids_hex {
        match parse_key(entry) {
            Some(key) if key.as_str() == entry => {
                people.insert(key);
            }
            _ => unmatched += 1,
        }
    }
    if unmatched > 0 {
        not_mapped.push(format!(
            "{unmatched} entries of the invite allow-list are not keys in lowercase hex, so \
             wn-agent never matched them, and they were left out"
        ));
    }
    (record.invite_policy, people)
}

fn parse_key(value: &str) -> Option<PubKey> {
    AccountHome::account_id_for_public_key(value.trim())
        .ok()
        .map(PubKey)
}

/// A key as the old harnesses and the OpenClaw plugin take one: 64 hex digits, in either case.
fn strict_hex(value: &str) -> Option<PubKey> {
    let value = value.trim().to_ascii_lowercase();
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| parse_key(&value))
        .flatten()
}

fn csv(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
        .collect()
}

/// A terminal harness's own settings, as its service ran it, or its env file in the home when
/// no service runs it.
fn harness_env(connector: Connector, home: &Path, services: &[Service]) -> Vec<(String, String)> {
    if connector.env_prefix().is_none() {
        return Vec::new();
    }
    if let Some(service) = services.iter().find(|s| runs_harness(s, connector)) {
        return service.env.clone();
    }
    let file = home.join(format!("dev/wn-{}.env", connector.name()));
    std::fs::read_to_string(file)
        .map(|text| env_file(&text))
        .unwrap_or_default()
}

fn lookup<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Reads `KEY=value` lines as the installers and Hermes write them, values quoted or not.
pub fn env_file(text: &str) -> Vec<(String, String)> {
    let mut vars = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let value = match value.strip_prefix("$'").and_then(|v| v.strip_suffix('\'')) {
            Some(inner) => inner.replace("\\'", "'").replace("\\\\", "\\"),
            None => services::words(value).join(" "),
        };
        vars.push((key.trim().to_string(), value));
    }
    vars
}

/// The terminal harness's list, parsed as it parses it: every entry 64 hex digits, at least
/// one, or the harness does not start and nobody may write to it.
fn harness_senders(raw: Option<&str>) -> Result<BTreeSet<PubKey>, &'static str> {
    let raw = raw.ok_or("names no senders")?;
    let mut people = BTreeSet::new();
    for entry in raw.split(',').filter(|e| !e.trim().is_empty()) {
        people.insert(strict_hex(entry).ok_or("holds an entry that is not 64 hex digits")?);
    }
    if people.is_empty() {
        return Err("names no senders");
    }
    Ok(people)
}

/// OpenClaw's `senderPolicy`, parsed as the old plugin parses it: any fault denies everyone.
fn openclaw_policy(policy: &serde_json::Value) -> Result<Allowed, &'static str> {
    let Some(fields) = policy.as_object() else {
        return Err("is not an object");
    };
    if fields
        .keys()
        .any(|k| k != "allowedUsers" && k != "allowAll")
    {
        return Err("has a field the plugin does not know");
    }
    let allow_all = match fields.get("allowAll") {
        None => false,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(_) => return Err("has an `allowAll` that is not true or false"),
    };
    let mut people = BTreeSet::new();
    match fields.get("allowedUsers") {
        None => {}
        Some(serde_json::Value::Array(entries)) => {
            for entry in entries {
                let key = entry.as_str().and_then(strict_hex);
                people.insert(key.ok_or("lists an entry that is not 64 hex digits")?);
            }
        }
        Some(_) => return Err("has an `allowedUsers` that is not a list"),
    }
    if allow_all {
        Ok(Allowed::Everyone)
    } else if people.is_empty() {
        Err("lists nobody")
    } else {
        Ok(Allowed::People(people))
    }
}

/// Hermes's `.env` allow-list. Any entry that is not a key leaves it unclear what Hermes did.
fn hermes_policy(env: &[(String, String)]) -> Result<Allowed, &'static str> {
    let everyone =
        match lookup(env, "MARMOT_ALLOW_ALL_USERS").map(|v| v.trim().to_ascii_lowercase()) {
            None => false,
            Some(v) if ["1", "true", "yes", "on"].contains(&v.as_str()) => true,
            Some(v) if ["0", "false", "no", "off"].contains(&v.as_str()) => false,
            Some(_) => return Err("has a `MARMOT_ALLOW_ALL_USERS` that is not true or false"),
        };
    let mut people = BTreeSet::new();
    for entry in csv(lookup(env, "MARMOT_ALLOWED_USERS").unwrap_or_default()) {
        people.insert(parse_key(&entry).ok_or("lists an entry that is not a public key")?);
    }
    Ok(if everyone {
        Allowed::Everyone
    } else {
        Allowed::People(people)
    })
}

/// Who the old install let write to the agent. What cannot be read for certain allows nobody.
fn read_senders(
    connector: Connector,
    home: &Path,
    places: &Places,
    services: &[Service],
    not_mapped: &mut Vec<String>,
) -> Senders {
    let mut allowed = Allowed::Everyone;
    let mut from = Vec::new();
    let unclear = |why: String, not_mapped: &mut Vec<String>| {
        not_mapped.push(format!("{why}; only the owner may prompt now"));
        Allowed::nobody()
    };
    match connector {
        Connector::Custom => {
            allowed = unclear(
                "a custom client decided for itself who could prompt".into(),
                not_mapped,
            );
        }
        Connector::Hermes => {
            let config =
                std::fs::read_to_string(places.hermes_home.join("config.yaml")).unwrap_or_default();
            let env = std::fs::read_to_string(places.hermes_home.join(".env"))
                .map(|text| env_file(&text))
                .unwrap_or_default();
            let named = ["MARMOT_ALLOWED_USERS", "MARMOT_ALLOW_ALL_USERS"]
                .iter()
                .any(|k| lookup(&env, k).is_some());
            if ["allowed_users", "allow_all_users"]
                .iter()
                .any(|k| config.contains(k))
            {
                allowed = unclear(
                    "Hermes's config.yaml has an allow-list of its own, which Hermes combines \
                     with its `.env` in ways the import does not follow"
                        .into(),
                    not_mapped,
                );
            } else if !named {
                allowed = unclear(
                    "Hermes's allow-list was not found in its `.env`, and one set in its \
                     service's environment is not read"
                        .into(),
                    not_mapped,
                );
            } else {
                match hermes_policy(&env) {
                    Ok(policy) => {
                        allowed = policy;
                        from.push("Hermes's `.env`".into());
                    }
                    Err(why) => {
                        allowed = unclear(format!("Hermes's allow-list {why}"), not_mapped);
                    }
                }
            }
        }
        Connector::OpenClaw => {
            let config = std::fs::read_to_string(&places.openclaw_config)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
            let policy = config
                .as_ref()
                .and_then(|c| c.pointer("/channels/marmot"))
                .and_then(|m| m.as_object())
                .and_then(|m| m.get("senderPolicy"));
            match policy.map(openclaw_policy) {
                Some(Ok(policy)) => {
                    allowed = policy;
                    from.push("OpenClaw's sender policy".into());
                }
                Some(Err(why)) => {
                    allowed = unclear(
                        format!(
                            "OpenClaw's sender policy {why}, so the old plugin let nobody write"
                        ),
                        not_mapped,
                    );
                }
                None => {
                    allowed = unclear(
                        "OpenClaw's sender policy was not found in its settings, and \
                         `MARMOT_ALLOWED_USERS` in the gateway's environment is not read"
                            .into(),
                        not_mapped,
                    );
                }
            }
        }
        terminal => {
            let prefix = terminal.env_prefix().unwrap_or_default();
            let name = format!("{prefix}_ALLOWED_SENDERS_HEX");
            let legacy = (terminal == Connector::OpenCode).then_some("WN_OPENCODE_ADMIN_HEX");
            let read = |env: &[(String, String)]| {
                harness_senders(lookup(env, &name).or_else(|| legacy.and_then(|l| lookup(env, l))))
            };
            // Every place the harness's settings are kept: each service that runs it, as it
            // ran, and the env file in the home. Only who all of them allow is kept.
            let mut sources: Vec<(String, Result<BTreeSet<PubKey>, &'static str>)> = Vec::new();
            // A service whose environment could not be read in full has stopped the plan.
            for service in services.iter().filter(|s| runs_harness(s, terminal)) {
                sources.push((format!("the service {}", service.name), read(&service.env)));
            }
            let file = home.join(format!("dev/wn-{}.env", terminal.name()));
            if let Ok(text) = std::fs::read_to_string(&file) {
                sources.push(("the harness's env file".into(), read(&env_file(&text))));
            }
            if sources.is_empty() {
                allowed = unclear(
                    "the harness's list of senders was not found".into(),
                    not_mapped,
                );
            }
            for (place, read) in sources {
                match read {
                    Ok(people) => {
                        allowed = allowed.and(&Allowed::People(people));
                        from.push(format!("`{name}` in {place}"));
                    }
                    Err(why) => {
                        allowed = unclear(
                            format!("`{name}` in {place} {why}, so the harness could not start"),
                            not_mapped,
                        );
                    }
                }
            }
            if from.len() > 1 {
                not_mapped.push(
                    "the harness's list of senders is kept in several places; only the people on \
                     all of them may prompt"
                        .into(),
                );
            }
        }
    }
    if allowed == Allowed::nobody() {
        from.clear();
    }
    Senders { allowed, from }
}

fn approve_note(connector: Connector) -> &'static str {
    match connector {
        Connector::Hermes | Connector::OpenClaw => {
            "who may approve: the owner alone (D-010), where the old plugin let the people \
             allowed to write answer approvals; widen `approve` in the settings to keep that"
        }
        Connector::Custom => "who may approve: the owner alone (D-010)",
        _ => {
            "who may approve: the owner alone (D-010); the old harness could not be asked for \
             approval at all"
        }
    }
}

fn quote(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

fn list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| quote(i)).collect();
    format!("[{}]", quoted.join(", "))
}

fn who(who: &Who) -> String {
    match who {
        Who::Owner => quote("owner"),
        Who::Everyone => quote("everyone"),
        Who::People(people) => {
            let people: Vec<String> = people.iter().map(|p| p.0.clone()).collect();
            format!("{{ people = {} }}", list(&people))
        }
    }
}

/// The harness's section of the settings.
fn section(
    connector: Connector,
    cwd: &str,
    env: &[(String, String)],
    mapped: &mut Vec<String>,
    not_mapped: &mut Vec<String>,
) -> String {
    let program = connector.env_prefix().and_then(|prefix| {
        lookup(env, &format!("{prefix}_BIN"))
            .filter(|p| !p.is_empty() && *p != connector.program())
            .map(String::from)
    });
    if program.is_some() {
        mapped.push("the harness's program".into());
    }
    let program = program.unwrap_or_else(|| connector.program().to_string());
    if let Some(profile) =
        lookup(env, "MARMOT_HARNESS_EXECUTION_PROFILE").filter(|p| !p.is_empty() && *p != "inherit")
    {
        not_mapped.push(format!(
            "the execution profile `{profile}`: the core uses the harness's own safe defaults \
             and asks in the chat, so set the harness section by hand to give it more"
        ));
    }
    if let Some(prefix) = connector.env_prefix() {
        let limits: Vec<&str> = [
            "TIMEOUT_SECS",
            "IDLE_TIMEOUT_SECS",
            "REQUEST_TIMEOUT_SECS",
            "MAX_REPLY_BYTES",
            "MAX_PENDING_PER_GROUP",
        ]
        .into_iter()
        .filter(|l| lookup(env, &format!("{prefix}_{l}")).is_some())
        .collect();
        if !limits.is_empty() {
            not_mapped.push(
                "the harness's time limits and reply limits: the core has no such settings".into(),
            );
        }
        not_mapped.push(
            "the folder each chat chose with `/<path>`: the core works in one folder, `cwd`".into(),
        );
    }
    match connector {
        Connector::Codex => {
            let mut text = "[codex]\n".to_string();
            if program != "codex" {
                text += &format!("program = {}\n", quote(&program));
            }
            text + &format!("cwd = {}\n", quote(cwd))
        }
        Connector::Pi => {
            let mut text = "[pi]\n".to_string();
            if program != "pi" {
                text += &format!("program = {}\n", quote(&program));
            }
            text + &format!("cwd = {}\n", quote(cwd))
        }
        Connector::OpenCode | Connector::Goose => {
            let command = vec![program, "acp".to_string()];
            format!(
                "[acp]\nname = {}\ncommand = {}\ncwd = {}\n",
                quote(connector.name()),
                list(&command),
                quote(cwd)
            )
        }
        Connector::Claude | Connector::Hermes | Connector::OpenClaw | Connector::Custom => {
            "[connector]\n".to_string()
        }
    }
}

/// The settings file's text.
fn settings_text(plan: &Plan, now: SystemTime) -> String {
    let invite: Vec<String> = plan.invite.iter().map(|p| p.0.clone()).collect();
    let mut text = format!(
        "# Imported by `marmot-agent import` from a {} install, at {}.\n\
         # The home before the import is backed up in {}.\n\
         home = {}\n\
         relays = {}\n\
         owner = {}\n\
         prompt = {}\n\
         approve = {}\n\
         invite = {}\n\
         # Only the folders the old connector let files be sent from; never the working folder.\n\
         files_from = {}\n",
        plan.connector.title(),
        backup::utc_stamp(now),
        plan.backup.display(),
        quote(&plan.home.to_string_lossy()),
        list(&plan.relays),
        quote(plan.owner.as_str()),
        who(&plan.prompt),
        who(&plan.approve),
        list(&invite),
        list(&plan.files_from),
    );
    if plan.allow_loopback_relays {
        text += "allow_loopback_relays = true\n";
    }
    text + "\n" + &plan.section
}

/// `text` as one word for a POSIX shell: as it is when that is safe, else in single quotes.
fn shell(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./:=@%+,-".contains(&b));
    if plain {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn shell_path(path: &Path) -> String {
    shell(&path.to_string_lossy())
}

fn next_steps(plan: &Plan, cwd: &str) -> Vec<String> {
    let run = format!("marmot-agent run {}", shell_path(&plan.settings));
    let mut next = vec![format!("start the agent: `{run}`")];
    match plan.connector {
        Connector::Claude => next.push(format!(
            "start the Claude Code driver: `node packages/claude-code/src/main.ts --home {} --cwd {}`",
            shell_path(&plan.home),
            shell(cwd)
        )),
        Connector::Hermes => next.push(
            "put the new Hermes plugin (python/marmot_hermes) in Hermes in place of the old \
             one, and restart Hermes"
                .into(),
        ),
        Connector::OpenClaw => next.push(
            "put the new OpenClaw plugin (packages/openclaw) in OpenClaw in place of the old \
             one, and restart OpenClaw"
                .into(),
        ),
        Connector::Custom => next.push(
            "a custom client of wn-agent's socket does not work with the core, which speaks \
             another protocol on another socket"
                .into(),
        ),
        _ => {}
    }
    next
}

impl Plan {
    /// Prints what the import found and will do. Keys and relay addresses are counted, not
    /// shown; they are in the settings file.
    pub fn print(&self, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            out,
            "A {} install, told by {}.",
            self.connector.title(),
            self.found_by
        )?;
        writeln!(
            out,
            "Its one identity and its home stay where they are: {}",
            self.home.display()
        )?;
        writeln!(out, "\nCarried over:")?;
        for line in &self.mapped {
            writeln!(out, "  - {line}")?;
        }
        writeln!(out, "\nNot carried over, or changed:")?;
        for line in &self.not_mapped {
            writeln!(out, "  - {line}")?;
        }
        writeln!(out, "\nThe import will:")?;
        if self.services.is_empty() {
            writeln!(
                out,
                "  - find no service of the old connector running on this home; if it runs some \
                 other way, stop it first"
            )?;
        }
        for service in &self.services {
            writeln!(
                out,
                "  - stop and disable {}, keeping its file {}",
                service.name,
                service.file.display()
            )?;
        }
        for service in &self.left_running {
            writeln!(out, "  - leave {} running, as asked", service.name)?;
        }
        writeln!(
            out,
            "  - take the home's runtime lease, and stop if anything else holds it"
        )?;
        writeln!(
            out,
            "  - back up the whole home to {}, owner-only, and check the copy",
            self.backup.display()
        )?;
        writeln!(out, "  - write the settings to {}", self.settings.display())?;
        Ok(())
    }
}

/// Carries out the plan made from `options`: stops and disables the old services, takes the
/// home's runtime lease, reads the old install again and checks it still makes the same plan,
/// backs up the home and writes the settings, saying what it did as it goes. Stops at the first
/// step that fails, and says what was done.
pub fn carry_out(
    options: &Options,
    plan: &Plan,
    services: &mut dyn Services,
    out: &mut dyn Write,
) -> Result<(), String> {
    let say = |out: &mut dyn Write, line: String| {
        let _ = writeln!(out, "{line}");
    };
    let mut stopped = Vec::new();
    for service in &plan.services {
        if let Err(why) = services.stop_and_disable(service) {
            // It may be stopped or disabled in part, so it is started again with the rest.
            let mut touched = stopped.clone();
            touched.push(service);
            return Err(format!(
                "{} could not be stopped and disabled: {why}; it may be stopped or disabled in \
                 part. {}To run the old install again, {}. Nothing was copied or written",
                service.name,
                already(&stopped),
                start_again(touched)
            ));
        }
        say(
            out,
            format!(
                "Stopped and disabled {}; its file is kept: {}",
                service.name,
                service.file.display()
            ),
        );
        stopped.push(service);
    }
    // Held until the settings are written: nothing that takes the lease, wn-agent or the core,
    // can open the home meanwhile.
    let _lease = match marmot_app::MarmotRootRuntimeLease::try_acquire(&plan.home) {
        Ok(lease) => lease,
        Err(marmot_app::AppError::RuntimeBusy) => {
            return Err(format!(
                "another program still holds the home's runtime lease, so it is in use. {}Stop \
                 it and run the import again; nothing was copied or written",
                stopped_so_far(&stopped)
            ));
        }
        Err(_) => {
            return Err(format!(
                "the home's runtime lease could not be taken. {}Nothing was copied or written",
                stopped_so_far(&stopped)
            ));
        }
    };
    if std::os::unix::net::UnixStream::connect(&plan.old_socket).is_ok() {
        return Err(format!(
            "something still serves the old connector's socket, so the home is in use. {}Stop \
             it and run the import again; nothing was copied or written",
            stopped_so_far(&stopped)
        ));
    }
    // The old programs could change what they allow as they stop, such as the welcomers wn-agent
    // keeps in the home; read with them stopped and the home held, it must make the same plan.
    let again = self::plan(options, services).map_err(|why| {
        format!(
            "read again with the old services stopped, the install does not make a plan: {why}. \
             {}Nothing was copied or written",
            stopped_so_far(&stopped)
        )
    })?;
    if again.settings_text != plan.settings_text || again.services != plan.services {
        return Err(format!(
            "the old install changed while its services stopped, so it no longer makes the plan \
             shown. {}Run the import again to see the plan it makes now; nothing was copied or \
             written",
            stopped_so_far(&stopped)
        ));
    }
    let copied = backup::take(&plan.home, &plan.backup).map_err(|failed| match failed {
        backup::Failed::NotMade(e) => format!(
            "the backup failed ({e}), and what was copied is removed. {}Nothing was written",
            stopped_so_far(&stopped)
        ),
        backup::Failed::Unconfirmed(e) => format!(
            "the backup is in place in {}, but whether it reached the disk could not be \
             confirmed ({e}). {}The settings were not written; run the import again, which \
             takes a new backup",
            plan.backup.display(),
            stopped_so_far(&stopped)
        ),
    })?;
    say(
        out,
        format!(
            "Backed up the home to {}: {} files, {} bytes, checked against the home",
            plan.backup.display(),
            copied.files,
            copied.bytes
        ),
    );
    backup::publish_file(&plan.settings, plan.settings_text.as_bytes()).map_err(|failed| {
        match failed {
            backup::Failed::NotMade(e) => format!(
                "the settings could not be written ({}); the backup is kept in {}. {}",
                e.kind(),
                plan.backup.display(),
                stopped_so_far(&stopped)
            ),
            backup::Failed::Unconfirmed(e) => format!(
                "the settings are written to {}, but whether they reached the disk could not be \
                 confirmed ({}); run `sync` before starting the core. The backup is in {}",
                plan.settings.display(),
                e.kind(),
                plan.backup.display()
            ),
        }
    })?;
    say(
        out,
        format!("Wrote the settings to {}", plan.settings.display()),
    );
    say(out, "\nNext:".into());
    for line in &plan.next {
        say(out, format!("  - {line}"));
    }
    say(
        out,
        format!(
            "\nTo go back before the core has run: delete the settings file, and {}. Once it \
             has run, MDK has upgraded the store: stop the core, put {} in place of the home, \
             and {0}. Do that only if the agent has done nothing since, because an older store \
             can reuse keys or run a prompt twice.",
            start_again(plan.services.iter().collect()),
            plan.backup.display()
        ),
    );
    Ok(())
}

/// How to start the given services again, the connector first.
fn start_again(services: Vec<&Service>) -> String {
    let again: Vec<String> = services
        .iter()
        .rev()
        .map(|s| match s.manager {
            services::Manager::Systemd => {
                format!("`systemctl --user enable --now {}`", shell(&s.name))
            }
            services::Manager::Launchd => format!(
                "`launchctl enable gui/$(id -u)/{0}` and `launchctl bootstrap gui/$(id -u) {1}`",
                shell(&s.name),
                shell_path(&s.file)
            ),
        })
        .collect();
    if again.is_empty() {
        "start the old connector again".to_string()
    } else {
        format!("start the old services again: {}", again.join(", then "))
    }
}

/// The services already stopped and disabled, as a sentence; nothing when there are none.
fn already(stopped: &[&Service]) -> String {
    if stopped.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = stopped.iter().map(|s| s.name.as_str()).collect();
    format!("Already stopped and disabled: {}. ", names.join(", "))
}

fn stopped_so_far(stopped: &[&Service]) -> String {
    if stopped.is_empty() {
        return String::new();
    }
    format!(
        "{}To run the old install again, {}. ",
        already(stopped),
        start_again(stopped.to_vec())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home with identities made by `labels`, as `wn-agent`'s account creation makes them.
    fn home_with(root: &Path, folder: &str, labels: &[&str]) -> (PathBuf, Vec<String>) {
        let home = root.join(folder);
        std::fs::create_dir_all(&home).unwrap();
        let accounts = AccountHome::open(&home);
        let ids = labels
            .iter()
            .map(|l| accounts.create_account(l).unwrap().account_id_hex)
            .collect();
        (home, ids)
    }

    /// Public keys that are real points, from a scratch home.
    fn people(root: &Path, n: usize) -> Vec<String> {
        let labels: Vec<String> = (0..n).map(|i| format!("p{i}")).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        home_with(root, "people", &labels).1
    }

    fn options(root: &Path, home: &Path) -> Options {
        Options {
            home: home.to_path_buf(),
            harness: None,
            owner: None,
            cwd: None,
            settings: None,
            leave_running: Vec::new(),
            places: Places {
                user_home: root.to_path_buf(),
                systemd_units: root.join("units"),
                launch_agents: root.join("agents"),
                hermes_home: root.join(".hermes"),
                openclaw_config: root.join(".openclaw/openclaw.json"),
            },
            now: SystemTime::UNIX_EPOCH,
        }
    }

    fn welcomers(home: &Path, agent: &str, policy: &str, people: &[String]) {
        let dir = home.join("dev/agent-allowlist");
        std::fs::create_dir_all(&dir).unwrap();
        let record = serde_json::json!({
            "account_id_hex": agent,
            "invite_policy": policy,
            "welcomer_account_ids_hex": people,
        });
        std::fs::write(dir.join(format!("{agent}.json")), record.to_string()).unwrap();
    }

    /// The Codex harness's env file in the home, letting `senders` write.
    fn codex_env(home: &Path, senders: &[String]) {
        std::fs::create_dir_all(home.join("dev")).unwrap();
        std::fs::write(
            home.join("dev/wn-codex.env"),
            format!("WN_CODEX_ALLOWED_SENDERS_HEX={}\n", senders.join(",")),
        )
        .unwrap();
    }

    /// A unit in the test's systemd folder.
    fn unit(root: &Path, name: &str, text: &str) {
        let path = root.join("units").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// The plan, with no systemd to ask: the units' files are read.
    fn plan(options: &Options) -> Result<Plan, String> {
        super::plan(options, &mut Noted(Vec::new()))
    }

    fn plan_err(options: &Options) -> String {
        plan(options).unwrap_err()
    }

    #[test]
    fn a_home_with_several_identities_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let (home, _) = home_with(root.path(), "codex", &["a", "b"]);
        let why = plan_err(&options(root.path(), &home));
        assert!(why.contains("2 identities"), "{why}");
        let (empty, _) = home_with(root.path(), "pi", &[]);
        assert!(plan(&options(root.path(), &empty)).is_err());
    }

    #[test]
    fn the_owner_is_the_one_person_who_could_invite_and_prompt() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 3);
        // p0 may invite and prompt, p1 may only invite, p2 may only prompt.
        welcomers(&home, &ids[0], "allowlist", &p[..2]);
        codex_env(&home, &[p[0].clone(), p[2].clone()]);
        let plan = plan(&options(root.path(), &home)).unwrap();
        assert_eq!(plan.connector, Connector::Codex);
        assert_eq!(plan.owner.as_str(), p[0]);
        assert_eq!(plan.invite, [PubKey(p[1].clone())]);
        assert_eq!(plan.prompt, Who::People(vec![PubKey(p[2].clone())]));
        assert_eq!(plan.approve, Who::Owner);
        // No service: the relays wn-agent uses by default, and it is said.
        assert_eq!(plan.relays, DEFAULT_RELAYS);
        assert!(plan.not_mapped.iter().any(|l| l.contains("no service")));
        // No media folders: no files, and never the working folder.
        assert!(plan.files_from.is_empty());
        let config = Config::parse(&plan.settings_text).unwrap();
        assert!(config.file_folders().is_empty());
    }

    #[test]
    fn a_sole_welcomer_who_could_not_prompt_is_not_the_owner() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 2);
        welcomers(&home, &ids[0], "allowlist", &p[..1]);
        codex_env(&home, &p[1..]);
        assert!(plan_err(&options(root.path(), &home)).contains("0 people"));

        // Named, the owner gains what the old install did not give them, and the plan says so.
        let mut named = options(root.path(), &home);
        named.owner = Some(marmot_app::npub_for_account_id(&p[0]).unwrap());
        let plan = plan(&named).unwrap();
        assert_eq!(plan.owner.as_str(), p[0]);
        assert_eq!(plan.prompt, Who::People(vec![PubKey(p[1].clone())]));
        assert!(plan.not_mapped.iter().any(|l| l.contains("may now prompt")));
    }

    #[test]
    fn under_deny_nobody_becomes_the_owner_unasked() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 1);
        welcomers(&home, &ids[0], "deny", &[]);
        codex_env(&home, &p);
        assert!(plan_err(&options(root.path(), &home)).contains("--owner"));
        let mut named = options(root.path(), &home);
        named.owner = Some(p[0].clone());
        let plan = plan(&named).unwrap();
        assert!(plan.invite.is_empty());
        assert!(plan.not_mapped.iter().any(|l| l.contains("may now invite")));
    }

    #[test]
    fn one_key_in_hex_and_as_an_npub_is_one_person() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "hermes", &["hermes-agent"]);
        let p = people(root.path(), 1);
        welcomers(&home, &ids[0], "allowlist", &p);
        let npub = marmot_app::npub_for_account_id(&p[0]).unwrap();
        std::fs::create_dir_all(root.path().join(".hermes")).unwrap();
        std::fs::write(
            root.path().join(".hermes/.env"),
            format!("MARMOT_ALLOWED_USERS={},{npub}\n", p[0]),
        )
        .unwrap();
        let plan = plan(&options(root.path(), &home)).unwrap();
        assert_eq!(plan.owner.as_str(), p[0]);
        assert_eq!(plan.prompt, Who::Owner);
    }

    #[test]
    fn a_drop_in_that_narrows_the_senders_is_read() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 2);
        welcomers(&home, &ids[0], "allowlist", &p[..1]);
        let h = home.display();
        // The unit lets both write; a drop-in and an environment file narrow it to p0.
        unit(
            root.path(),
            "wn-codex.service",
            &format!(
                "[Service]\nEnvironment=\"MARMOT_HOME={h}\"\n\
                 Environment=\"WN_CODEX_ALLOWED_SENDERS_HEX={},{}\"\nExecStart=/x/wn-codex\n",
                p[0], p[1]
            ),
        );
        std::fs::write(
            root.path().join("narrow.env"),
            format!("WN_CODEX_ALLOWED_SENDERS_HEX={}\n", p[0]),
        )
        .unwrap();
        unit(
            root.path(),
            "wn-codex.service.d/narrow.conf",
            "[Service]\nEnvironmentFile=%h/narrow.env\n",
        );
        let plan = plan(&options(root.path(), &home)).unwrap();
        assert_eq!(plan.owner.as_str(), p[0]);
        assert_eq!(
            plan.prompt,
            Who::Owner,
            "only the owner, as the drop-in said"
        );

        // The env file in the home and the service disagree: only who both allow is kept.
        codex_env(&home, &p[1..]);
        assert!(plan_err(&options(root.path(), &home)).contains("0 people"));
    }

    #[test]
    fn a_harness_list_with_a_bad_entry_lets_nobody_write() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 1);
        welcomers(&home, &ids[0], "allowlist", &p);
        // The old harness takes hex only, and refuses to start on anything else.
        let npub = marmot_app::npub_for_account_id(&p[0]).unwrap();
        codex_env(&home, &[p[0].clone(), npub]);
        assert!(plan_err(&options(root.path(), &home)).contains("--owner"));
    }

    #[test]
    fn hermes_lets_everyone_write_and_an_open_invite_policy_is_narrowed() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "hermes", &["hermes-agent"]);
        let p = people(root.path(), 2);
        welcomers(&home, &ids[0], "any_authenticated", &p[..1]);
        std::fs::create_dir_all(root.path().join(".hermes")).unwrap();
        std::fs::write(
            root.path().join(".hermes/.env"),
            format!(
                "OTHER=1\nexport MARMOT_ALLOWED_USERS=\"{}\"\nMARMOT_ALLOW_ALL_USERS=true\n",
                p[1]
            ),
        )
        .unwrap();
        let mut named = options(root.path(), &home);
        named.owner = Some(p[0].clone());
        let plan = plan(&named).unwrap();
        assert_eq!(plan.connector, Connector::Hermes);
        assert_eq!(plan.found_by, "the account's label");
        assert_eq!(plan.prompt, Who::Everyone);
        assert!(plan.invite.is_empty());
        assert!(
            plan.not_mapped
                .iter()
                .any(|l| l.contains("let anyone invite"))
        );
        assert!(plan.settings_text.ends_with("[connector]\n"));
        // Anyone could invite and write, so the owner is not clear unasked.
        assert!(plan_err(&options(root.path(), &home)).contains("anyone"));
    }

    #[test]
    fn openclaw_senders_are_read_as_the_old_plugin_reads_them() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "openclaw", &["some-label"]);
        let p = people(root.path(), 3);
        welcomers(&home, &ids[0], "allowlist", &p[..1]);
        let write = |policy: serde_json::Value| {
            let config =
                serde_json::json!({ "channels": { "marmot": { "senderPolicy": policy } } });
            std::fs::create_dir_all(root.path().join(".openclaw")).unwrap();
            std::fs::write(
                root.path().join(".openclaw/openclaw.json"),
                config.to_string(),
            )
            .unwrap();
        };
        write(serde_json::json!({ "allowedUsers": [p[0], p[1].to_uppercase(), p[2]] }));
        let plan = plan(&options(root.path(), &home)).unwrap();
        assert_eq!(plan.connector, Connector::OpenClaw);
        assert_eq!(plan.found_by, "the home's folder name");
        assert_eq!(plan.owner.as_str(), p[0]);
        let mut others = vec![PubKey(p[1].clone()), PubKey(p[2].clone())];
        others.sort();
        assert_eq!(plan.prompt, Who::People(others));

        // The old plugin needs raw hex: an npub, even beside `allowAll`, voids the whole policy.
        let npub = marmot_app::npub_for_account_id(&p[2]).unwrap();
        write(serde_json::json!({ "allowedUsers": [p[0], npub], "allowAll": true }));
        assert!(plan_err(&options(root.path(), &home)).contains("--owner"));
        let mut named = options(root.path(), &home);
        named.owner = Some(p[0].clone());
        let voided = self::plan(&named).unwrap();
        assert_eq!(voided.prompt, Who::Owner);
        assert!(
            voided
                .not_mapped
                .iter()
                .any(|l| l.contains("let nobody write"))
        );
    }

    #[test]
    fn files_come_only_from_the_old_media_folders_away_from_the_home() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 1);
        welcomers(&home, &ids[0], "allowlist", &p);
        codex_env(&home, &p);
        let out = root.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let h = home.display();
        let (r, o) = (root.path().display(), out.display());
        unit(
            root.path(),
            "wn-agent-codex.service",
            &format!(
                "[Service]\nExecStart=/x/wn-agent --home {h} --relay wss://a,wss://b \
                 --media-allowed-root {o} --media-allowed-root {r} --media-allowed-root {h}/dev \
                 --media-allowed-root rel\n"
            ),
        );
        let plan = plan(&options(root.path(), &home)).unwrap();
        assert_eq!(plan.relays, ["wss://a", "wss://b"]);
        let out = std::fs::canonicalize(&out).unwrap();
        assert_eq!(plan.files_from, [out.to_string_lossy()]);
        assert!(
            plan.not_mapped
                .iter()
                .any(|l| l.starts_with("3 of the old"))
        );
        let config = Config::parse(&plan.settings_text).unwrap();
        assert_eq!(config.file_folders(), std::slice::from_ref(&out));

        // wn-agent takes `..` away by the words before it follows a link: `link/../out` is the
        // `out` beside the link, never one beside where the link leads, and `link/../codex/dev`
        // is inside the home wherever the link leads.
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("sub")).unwrap();
        std::fs::create_dir_all(elsewhere.join("out")).unwrap();
        std::fs::create_dir_all(elsewhere.join("codex/dev")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("sub"), root.path().join("link")).unwrap();
        std::fs::remove_dir_all(home.join("dev/agent-allowlist")).ok();
        welcomers(&home, &ids[0], "allowlist", &p);
        unit(
            root.path(),
            "wn-agent-codex.service",
            &format!(
                "[Service]\nExecStart=/x/wn-agent --home {h} --relay wss://a \
                 --media-allowed-root {r}/link/../out --media-allowed-root {r}/link/../codex/dev \
                 --media-allowed-root {r}/link/../missing\n"
            ),
        );
        let plan = self::plan(&options(root.path(), &home)).unwrap();
        assert_eq!(plan.files_from, [out.to_string_lossy()]);
        assert!(
            plan.not_mapped
                .iter()
                .any(|l| l.starts_with("2 of the old"))
        );
    }

    /// A service of the old connector whose settings systemd fills in only as it starts it, or
    /// that cannot be read in full, stops the plan, saying what to do.
    #[test]
    fn a_doubtful_service_stops_the_plan() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 1);
        welcomers(&home, &ids[0], "allowlist", &p);
        codex_env(&home, &p);
        let h = home.display();
        for text in [
            format!("ExecStart=/x/wn-agent --home {h} --media-allowed-root ${{MEDIA}}"),
            "ExecStart=/x/wn-agent --home ${HOME_DIR}".to_string(),
            format!("Environment=MARMOT_HOME={h}\nEnvironmentFile=%h/*.env\nExecStart=/x/wn-codex"),
            format!(
                "Environment=MARMOT_HOME={h}\nEnvironmentFile=%h/gone.env\nExecStart=/x/wn-codex"
            ),
        ] {
            unit(root.path(), "wn-x.service", &format!("[Service]\n{text}\n"));
            let why = plan_err(&options(root.path(), &home));
            assert!(
                why.contains("wn-x.service") && why.contains("daemon-reload"),
                "{text}: {why}"
            );
        }
    }

    #[test]
    fn an_offline_connector_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 1);
        welcomers(&home, &ids[0], "allowlist", &p);
        codex_env(&home, &p);
        let h = home.display();
        unit(
            root.path(),
            "wn-agent-codex.service",
            &format!("[Service]\nExecStart=/x/wn-agent --home {h} --no-default-relays\n"),
        );
        assert!(plan_err(&options(root.path(), &home)).contains("no relays"));
        // `MARMOT_RELAY`, the older name, still names relays.
        unit(
            root.path(),
            "wn-agent-codex.service",
            &format!(
                "[Service]\nEnvironment=MARMOT_RELAY=wss://old\n\
                 ExecStart=/x/wn-agent --home {h} --no-default-relays\n"
            ),
        );
        assert_eq!(
            plan(&options(root.path(), &home)).unwrap().relays,
            ["wss://old"]
        );
    }

    #[test]
    fn printed_commands_quote_their_paths() {
        assert_eq!(shell("/a/b-c_d.toml"), "/a/b-c_d.toml");
        assert_eq!(shell("/my agents/it's"), "'/my agents/it'\\''s'");
        assert_eq!(shell(""), "''");
        let root = tempfile::tempdir().unwrap();
        let (home, _) = home_with(root.path(), "my claude", &["claude-harness-agent"]);
        let mut asked = options(root.path(), &home);
        asked.owner = Some(people(root.path(), 1).remove(0));
        asked.cwd = Some("/w $x".into());
        let plan = plan(&asked).unwrap();
        let h = home.display();
        assert!(
            plan.next[0].ends_with(&format!("run '{h}/marmot-agent.toml'`")),
            "{:?}",
            plan.next
        );
        assert!(
            plan.next[1].contains(&format!("--home '{h}' --cwd '/w $x'")),
            "{:?}",
            plan.next
        );
    }

    #[test]
    fn a_service_that_is_not_the_old_connector_is_never_stopped_unasked() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let p = people(root.path(), 1);
        welcomers(&home, &ids[0], "allowlist", &p);
        codex_env(&home, &p);
        let h = home.display();
        unit(
            root.path(),
            "nightly-backup.service",
            &format!("[Service]\nExecStart=/usr/bin/restic backup --home {h}\n"),
        );
        unit(
            root.path(),
            "my-agent.service",
            &format!("[Service]\nExecStart=/opt/bin/wn-agent --home {h}\n"),
        );
        let why = plan_err(&options(root.path(), &home));
        assert!(why.contains("nightly-backup.service"), "{why}");
        let mut asked = options(root.path(), &home);
        asked.leave_running = vec!["nightly-backup.service".into()];
        let plan = plan(&asked).unwrap();
        let stopped: Vec<_> = plan.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            stopped,
            ["my-agent.service"],
            "a renamed unit of wn-agent is stopped"
        );
        assert_eq!(plan.left_running.len(), 1);
        asked.leave_running.push("nothing.service".into());
        assert!(plan_err(&asked).contains("names no other service"));
    }

    #[test]
    fn a_terminal_harness_becomes_its_section() {
        let root = tempfile::tempdir().unwrap();
        let (home, _) = home_with(root.path(), "elsewhere", &["x"]);
        let mut goose = options(root.path(), &home);
        goose.harness = Some("goose".into());
        goose.owner = Some(people(root.path(), 1).remove(0));
        goose.cwd = Some("/w".into());
        let plan = plan(&goose).unwrap();
        assert!(
            plan.settings_text.ends_with(
                "[acp]\nname = \"goose\"\ncommand = [\"goose\", \"acp\"]\ncwd = \"/w\"\n"
            ),
            "{}",
            plan.settings_text
        );
        let shown = format!("{plan:?}");
        assert!(
            !shown.contains(plan.owner.as_str()) && !shown.contains("/w"),
            "{shown}"
        );
        let mut unknown = options(root.path(), &home);
        unknown.owner = goose.owner.clone();
        assert!(plan_err(&unknown).contains("--harness"));
    }

    /// The services the tests stop: none really, each noted. There is no systemd to ask.
    struct Noted(Vec<String>);

    impl Services for Noted {
        fn stop_and_disable(&mut self, service: &Service) -> Result<(), String> {
            self.0.push(service.name.clone());
            Ok(())
        }

        fn systemd_unit(&mut self, _: &str) -> Option<Result<services::Loaded, String>> {
            None
        }
    }

    #[test]
    fn a_home_another_program_holds_is_not_copied() {
        let root = tempfile::tempdir().unwrap();
        let (home, _) = home_with(root.path(), "codex", &["codex-harness-agent"]);
        let mut asked = options(root.path(), &home);
        asked.owner = Some(people(root.path(), 1).remove(0));
        let plan = plan(&asked).unwrap();
        // Held as wn-agent or a core holds it, with no socket to find it by.
        let held = marmot_app::MarmotRootRuntimeLease::try_acquire(&home).unwrap();
        let why = carry_out(&asked, &plan, &mut Noted(Vec::new()), &mut Vec::new()).unwrap_err();
        assert!(why.contains("runtime lease"), "{why}");
        assert!(!plan.backup.exists() && !plan.settings.exists());
        assert!(backup::partials_of(&home).is_empty());

        drop(held);
        carry_out(&asked, &plan, &mut Noted(Vec::new()), &mut Vec::new()).unwrap();
        assert!(plan.backup.is_dir() && plan.settings.is_file());
        // Released once done, for the core.
        drop(marmot_app::MarmotRootRuntimeLease::try_acquire(&home).unwrap());
    }

    /// Stops services as told, and as the first one stops, changes what the install allows, as
    /// wn-agent could on its way down.
    struct Changing {
        stopped: Vec<String>,
        fail: Option<&'static str>,
        on_stop: Option<Box<dyn FnMut()>>,
    }

    impl Services for Changing {
        fn stop_and_disable(&mut self, service: &Service) -> Result<(), String> {
            if self.fail == Some(service.name.as_str()) {
                return Err("systemd does not report it stopped".into());
            }
            self.stopped.push(service.name.clone());
            if let Some(mut change) = self.on_stop.take() {
                change();
            }
            Ok(())
        }

        fn systemd_unit(&mut self, _: &str) -> Option<Result<services::Loaded, String>> {
            None
        }
    }

    /// A Codex install with both its services, for carrying out.
    fn installed(root: &Path) -> (PathBuf, Vec<String>, Vec<String>) {
        let (home, ids) = home_with(root, "codex", &["codex-harness-agent"]);
        let p = people(root, 2);
        welcomers(&home, &ids[0], "allowlist", &p[..1]);
        codex_env(&home, &p[..1]);
        let h = home.display();
        unit(
            root,
            "wn-agent-codex.service",
            &format!("[Service]\nExecStart=/x/wn-agent --home {h} --relay wss://a\n"),
        );
        unit(
            root,
            "wn-codex.service",
            &format!(
                "[Service]\nEnvironment=MARMOT_HOME={h}\n\
                 Environment=WN_CODEX_ALLOWED_SENDERS_HEX={}\nExecStart=/x/wn-codex\n",
                p[0]
            ),
        );
        (home, ids, p)
    }

    #[test]
    fn an_install_that_changes_as_it_stops_is_not_carried_out() {
        let root = tempfile::tempdir().unwrap();
        let (home, ids, p) = installed(root.path());
        let asked = options(root.path(), &home);
        let plan = plan(&asked).unwrap();
        let (home2, agent, person) = (home.clone(), ids[0].clone(), p[1].clone());
        let mut services = Changing {
            stopped: Vec::new(),
            fail: None,
            on_stop: Some(Box::new(move || {
                welcomers(&home2, &agent, "allowlist", std::slice::from_ref(&person));
            })),
        };
        let why = carry_out(&asked, &plan, &mut services, &mut Vec::new()).unwrap_err();
        assert!(why.contains("enable --now wn-agent-codex.service"), "{why}");
        assert!(why.contains("Nothing was copied"), "{why}");
        assert!(!plan.backup.exists() && !plan.settings.exists());

        // Unchanged, it goes ahead.
        welcomers(&home, &ids[0], "allowlist", &p[..1]);
        let mut services = Changing {
            stopped: Vec::new(),
            fail: None,
            on_stop: None,
        };
        carry_out(&asked, &plan, &mut services, &mut Vec::new()).unwrap();
        assert!(plan.settings.is_file());
    }

    /// A service that may be stopped or disabled in part is started again with the rest.
    #[test]
    fn a_service_that_fails_to_stop_is_in_the_restart_steps() {
        let root = tempfile::tempdir().unwrap();
        let (home, _, _) = installed(root.path());
        let asked = options(root.path(), &home);
        let plan = plan(&asked).unwrap();
        let mut services = Changing {
            stopped: Vec::new(),
            fail: Some("wn-agent-codex.service"),
            on_stop: None,
        };
        let why = carry_out(&asked, &plan, &mut services, &mut Vec::new()).unwrap_err();
        assert!(
            why.contains(
                "`systemctl --user enable --now wn-agent-codex.service`, then \
                 `systemctl --user enable --now wn-codex.service`"
            ),
            "{why}"
        );
        assert!(
            why.contains("Already stopped and disabled: wn-codex.service"),
            "{why}"
        );
    }

    #[test]
    fn env_files_are_read_as_written() {
        let vars = env_file("# c\nexport A=1\nB='two words'\nC=/a\\ b\nD=$'x\\'y'\n\nE=\"q\"\n");
        let get = |k: &str| lookup(&vars, k).map(String::from);
        assert_eq!(get("A").as_deref(), Some("1"));
        assert_eq!(get("B").as_deref(), Some("two words"));
        assert_eq!(get("C").as_deref(), Some("/a b"));
        assert_eq!(get("D").as_deref(), Some("x'y"));
        assert_eq!(get("E").as_deref(), Some("q"));
    }
}
