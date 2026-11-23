//! The old connector's services: found by the home they run on, read for their relays and
//! settings as they ran, and stopped and disabled without being removed (D-089).
//!
//! Today's installers set up a systemd user unit on Linux and a LaunchAgent on macOS for
//! `wn-agent` (`wn-agent-codex.service`, `org.marmot.wn-agent.codex` and so on), and for a
//! terminal harness a second one for the harness (`wn-codex.service`, `org.marmot.wn-codex`).
//! The names can be changed when installing, so a service is found by what it runs on: `--home`
//! on its command line or `MARMOT_HOME` in its environment. Only a service that runs one of the
//! old connector's own programs is stopped; anything else that names the home is reported.
//!
//! A systemd unit is read from systemd itself whenever it can be asked: `busctl --user` gives the
//! unit as systemd has it loaded, with every drop-in (prefix drop-ins such as
//! `wn-agent-.service.d` too), continued line, specifier and reset applied, and each argument
//! whole. `systemctl show` would do as well but joins the arguments with spaces, so a path with a
//! space in it could not be told from two arguments. Only when there is no systemd to ask (no
//! user manager, as in the tests) are the unit's files read here, as systemd merges them; what
//! that reading cannot settle, such as a specifier other than `%h`, makes the service doubtful.
//! Either way, a command line with `$` references, which systemd fills in only as it starts the
//! service, an environment file named by a pattern, and one that cannot be read, make it doubtful
//! too. A doubtful service on the home stops the import: it never guesses how a service ran.
//!
//! An environment file is read as systemd reads it, not as a shell would ([`environment_file`]).
//! A name it sets in a way systemd has read otherwise by version is unsure: a sender list, which
//! then lets only the owner prompt, with the line named to check (D-135); any other setting the
//! import reads makes the service doubtful.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// How a service is run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    /// A systemd user unit.
    Systemd,
    /// A LaunchAgent.
    Launchd,
}

/// What a service runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// `wn-agent`, which holds the account.
    Connector,
    /// One of the old terminal harnesses, such as `wn-codex`.
    Harness,
    /// Something else that names the home, such as a backup job: never stopped by the import.
    Other,
}

/// The terminal harness programs today's installers set up.
pub const HARNESS_PROGRAMS: [&str; 5] =
    ["wn-claude", "wn-codex", "wn-opencode", "wn-pi", "wn-goose"];

/// One of the old connector's services.
#[derive(Clone, PartialEq, Eq)]
pub struct Service {
    /// How it is run.
    pub manager: Manager,
    /// The unit's name, such as `wn-agent-codex.service`, or the LaunchAgent's label.
    pub name: String,
    /// The file that defines it, which is kept.
    pub file: PathBuf,
    /// What it runs.
    pub role: Role,
    /// Its program's arguments, the program first.
    pub args: Vec<String>,
    /// Its environment, as it ran: one value for each name, `UnsetEnvironment=` applied.
    pub env: Vec<(String, String)>,
    /// Why how it ran cannot be read for certain, when it cannot, in words.
    pub doubt: Option<String>,
    /// Names its environment files set in a way systemd reads otherwise by version, so what
    /// they held as it ran is not known.
    pub unsure: Vec<Unsure>,
}

/// A name an environment file sets in a way systemd reads otherwise by version.
#[derive(Clone, PartialEq, Eq)]
pub struct Unsure {
    /// Its name, such as `WN_CODEX_ALLOWED_SENDERS_HEX`.
    pub name: String,
    /// The file.
    pub file: String,
    /// The line or lines that set it, as written.
    pub written: String,
}

/// Whether `name` holds who may write to an old harness or plugin. Such a name read unsurely
/// lets only the owner prompt; any other the import reads makes the service doubtful.
pub fn is_sender_list(name: &str) -> bool {
    name.ends_with("_ALLOWED_SENDERS_HEX")
        || [
            "WN_OPENCODE_ADMIN_HEX",
            "MARMOT_ALLOWED_USERS",
            "MARMOT_ALLOW_ALL_USERS",
        ]
        .contains(&name)
}

/// What an `EnvironmentFile=` sets.
pub struct EnvironmentFile {
    /// Each name and its value, as systemd sets them, in the order each first appears.
    pub vars: Vec<(String, String)>,
    /// Names systemd may set otherwise, each with its lines as written, in the same order.
    pub unsure: Vec<(String, String)>,
}

/// Reads an `EnvironmentFile=` as systemd reads it (`parse_env_file_internal` in systemd's
/// `env-file.c`, as in 254 and later): one assignment may run over several lines, inside quotes
/// or after a backslash; quotes inside a bare value are kept; `#` and `;` start a comment only
/// at a line's start; a line whose name is not a name, such as `export A=1`, sets nothing.
///
/// A name is unsure, and its value left out, where an older systemd read it otherwise: a line
/// after a comment ending in a backslash, which systemd before 254 took as comment too, and a
/// backslash in double quotes before anything but `"`, `\` or the line's end, which older ones
/// took off. So is the name a line such as `export A=1` meant, which systemd leaves out.
pub fn environment_file(text: &str) -> EnvironmentFile {
    let now = final_values(text, false);
    let before = final_values(text, true);
    let mut names: Vec<&String> = Vec::new();
    for name in now.iter().chain(&before).map(|(n, _)| n) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    fn find<'a>(list: &'a [(String, Final)], name: &str) -> Option<&'a Final> {
        list.iter().find(|(n, _)| n == name).map(|(_, f)| f)
    }
    let mut read = EnvironmentFile {
        vars: Vec::new(),
        unsure: Vec::new(),
    };
    for name in names {
        let (a, b) = (find(&now, name), find(&before, name));
        fn sure(f: Option<&Final>) -> Option<&Option<String>> {
            f.and_then(|f| (!f.versioned).then_some(&f.value))
        }
        match (sure(a), sure(b)) {
            (Some(Some(x)), Some(Some(y))) if x == y => {
                read.vars.push((name.clone(), x.clone()));
            }
            _ => {
                let (start, end) = a.or(b).map(|f| f.span).unwrap_or_default();
                read.unsure
                    .push((name.clone(), text[start..end].to_string()));
            }
        }
    }
    read
}

/// The last assignment of a name in one reading.
struct Final {
    /// Its value; none for a line that meant the name but is not one, such as `export A=1`.
    value: Option<String>,
    /// Whether an older systemd read its value otherwise.
    versioned: bool,
    /// Where it is in the text.
    span: (usize, usize),
}

/// Each name's last assignment, in the order each first appears. `before_254` takes a line after
/// a comment ending in a backslash as comment too.
fn final_values(text: &str, before_254: bool) -> Vec<(String, Final)> {
    let mut out: Vec<(String, Final)> = Vec::new();
    let mut put = |name: &str, last: Final| match out.iter_mut().find(|(n, _)| n == name) {
        Some((_, f)) => *f = last,
        None => out.push((name.to_string(), last)),
    };
    for entry in entries(text, before_254) {
        if valid_name(&entry.key) {
            put(
                &entry.key,
                Final {
                    value: Some(entry.value),
                    versioned: entry.versioned,
                    span: entry.span,
                },
            );
        } else if let Some(meant) = entry
            .key
            .split_whitespace()
            .last()
            .filter(|w| valid_name(w))
        {
            put(
                meant,
                Final {
                    value: None,
                    versioned: true,
                    span: entry.span,
                },
            );
        }
    }
    out
}

/// A name as systemd takes one: letters, digits and `_`, not starting with a digit.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// One assignment as read.
struct Entry {
    key: String,
    value: String,
    versioned: bool,
    span: (usize, usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    PreKey,
    Key,
    PreValue,
    Value,
    ValueEscape,
    Single,
    Double,
    DoubleEscape,
    Comment,
    CommentEscape,
}

/// systemd's reading, state for state. Whitespace is space, tab and both line ends, and a line
/// ends at `\n` or `\r`.
fn entries(text: &str, before_254: bool) -> Vec<Entry> {
    use State::*;
    let space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r');
    let newline = |c: char| matches!(c, '\n' | '\r');
    let mut out = Vec::new();
    let mut state = PreKey;
    let (mut key, mut value) = (String::new(), String::new());
    // Where trailing whitespace starts, to be cut off.
    let (mut key_space, mut value_space): (Option<usize>, Option<usize>) = (None, None);
    let mut versioned = false;
    let mut start = 0;
    let mut push = |key: &mut String,
                    key_space: Option<usize>,
                    value: &mut String,
                    value_space: Option<usize>,
                    versioned: bool,
                    span: (usize, usize)| {
        if let Some(at) = key_space {
            key.truncate(at);
        }
        if let Some(at) = value_space {
            value.truncate(at);
        }
        out.push(Entry {
            key: std::mem::take(key),
            value: std::mem::take(value),
            versioned,
            span,
        });
    };
    for (i, c) in text.char_indices() {
        match state {
            PreKey => {
                if c == '#' || c == ';' {
                    state = Comment;
                } else if !space(c) {
                    state = Key;
                    key_space = None;
                    key.clear();
                    value.clear();
                    versioned = false;
                    start = i;
                    key.push(c);
                }
            }
            Key => {
                if newline(c) {
                    state = PreKey;
                    key.clear();
                } else if c == '=' {
                    state = PreValue;
                    value_space = None;
                } else {
                    if !space(c) {
                        key_space = None;
                    } else if key_space.is_none() {
                        key_space = Some(key.len());
                    }
                    key.push(c);
                }
            }
            PreValue => {
                if newline(c) {
                    state = PreKey;
                    push(&mut key, key_space, &mut value, None, versioned, (start, i));
                } else if c == '\'' {
                    state = Single;
                } else if c == '"' {
                    state = Double;
                } else if c == '\\' {
                    state = ValueEscape;
                } else if !space(c) {
                    state = Value;
                    value.push(c);
                }
            }
            Value => {
                if newline(c) {
                    state = PreKey;
                    push(
                        &mut key,
                        key_space,
                        &mut value,
                        value_space,
                        versioned,
                        (start, i),
                    );
                } else if c == '\\' {
                    state = ValueEscape;
                    value_space = None;
                } else {
                    if !space(c) {
                        value_space = None;
                    } else if value_space.is_none() {
                        value_space = Some(value.len());
                    }
                    value.push(c);
                }
            }
            ValueEscape => {
                state = Value;
                // A backslash before a line's end joins the lines.
                if !newline(c) {
                    value.push(c);
                }
            }
            Single => {
                if c == '\'' {
                    state = PreValue;
                } else {
                    value.push(c);
                }
            }
            Double => {
                if c == '"' {
                    state = PreValue;
                } else if c == '\\' {
                    state = DoubleEscape;
                } else {
                    value.push(c);
                }
            }
            DoubleEscape => {
                state = Double;
                if matches!(c, '"' | '\\' | '`' | '$') {
                    value.push(c);
                } else if c != '\n' {
                    value.push('\\');
                    value.push(c);
                }
                if !matches!(c, '"' | '\\' | '\n') {
                    versioned = true;
                }
            }
            Comment => {
                if c == '\\' {
                    state = CommentEscape;
                } else if newline(c) {
                    state = PreKey;
                }
            }
            CommentEscape => {
                state = if newline(c) && !before_254 {
                    PreKey
                } else {
                    Comment
                };
            }
        }
    }
    match state {
        Value => push(
            &mut key,
            key_space,
            &mut value,
            value_space,
            versioned,
            (start, text.len()),
        ),
        PreValue | ValueEscape | Single | Double | DoubleEscape => {
            push(
                &mut key,
                key_space,
                &mut value,
                None,
                versioned,
                (start, text.len()),
            );
        }
        _ => {}
    }
    out
}

/// Paths, keys and relay addresses stay out of logs (D-071): only the shape is shown.
impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service")
            .field("manager", &self.manager)
            .field("role", &self.role)
            .field("args", &self.args.len())
            .field("env", &self.env.len())
            .field("doubt", &self.doubt.is_some())
            .field("unsure", &self.unsure.len())
            .finish_non_exhaustive()
    }
}

impl Service {
    /// The value an argument such as `--relay` takes, each time it is given.
    pub fn values_of(&self, flag: &str) -> Vec<String> {
        let mut values = Vec::new();
        let mut args = self.args.iter();
        while let Some(arg) = args.next() {
            if arg == flag {
                if let Some(value) = args.next() {
                    values.push(value.clone());
                }
            } else if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
                values.push(value.to_string());
            }
        }
        values
    }

    /// Whether a flag such as `--allow-loopback-relays` is given.
    pub fn has(&self, flag: &str) -> bool {
        self.args.iter().any(|a| a == flag)
    }

    /// A variable in its environment.
    pub fn env(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The file name of the program it runs, such as `wn-codex`.
    pub fn program(&self) -> Option<String> {
        program_name(&self.args)
    }
}

/// A systemd unit as systemd has it loaded, or as its files say when systemd cannot be asked.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Loaded {
    /// `loaded` when systemd has the unit; otherwise `not-found`, `masked`, `bad-setting` and so on.
    pub state: String,
    /// Whether its files changed since systemd read them.
    pub needs_reload: bool,
    /// The commands `ExecStart=` runs, each as its program and then its arguments.
    pub exec_start: Vec<Vec<String>>,
    /// `Environment=`, one `NAME=value` each, in order.
    pub environment: Vec<String>,
    /// `EnvironmentFile=`: each path, and whether it may be missing.
    pub environment_files: Vec<(String, bool)>,
    /// `UnsetEnvironment=`: names, or `NAME=value` to unset only that value.
    pub unset_environment: Vec<String>,
}

/// Shows the shape only (D-071).
impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("state", &self.state)
            .field("needs_reload", &self.needs_reload)
            .field("exec_start", &self.exec_start.len())
            .finish_non_exhaustive()
    }
}

/// Reads and stops services: the seam the tests replace.
pub trait Services {
    /// Stops `service` and keeps it from starting again, at login or otherwise, leaving its file
    /// where it is. Succeeds only once the service is known to be stopped.
    fn stop_and_disable(&mut self, service: &Service) -> Result<(), String>;

    /// The user unit `name` as systemd has it loaded; `None` when there is no systemd to ask, and
    /// the unit's files are read instead.
    fn systemd_unit(&mut self, name: &str) -> Option<Result<Loaded, String>>;
}

/// The machine's own service managers: `systemctl --user`, `busctl --user` and `launchctl`.
#[derive(Default)]
pub struct System {
    /// Whether the user's systemd answers, once asked.
    systemd: Option<bool>,
}

const SYSTEMD: &str = "org.freedesktop.systemd1";

impl Services for System {
    fn stop_and_disable(&mut self, service: &Service) -> Result<(), String> {
        match service.manager {
            Manager::Systemd => {
                run("systemctl", &["--user", "disable", "--now", &service.name])?;
                let state = output(
                    "systemctl",
                    &[
                        "--user",
                        "show",
                        "-p",
                        "ActiveState",
                        "--value",
                        &service.name,
                    ],
                )?;
                match state.trim() {
                    "inactive" | "failed" => Ok(()),
                    _ => Err("systemd does not report it stopped".into()),
                }
            }
            Manager::Launchd => {
                let uid = user_id()?;
                let target = format!("gui/{uid}/{}", service.name);
                // Disabled first, so nothing starts it again between the two steps.
                run("launchctl", &["disable", &target])?;
                // `bootout` fails when the job is not loaded, which is what is wanted; so the
                // job's absence, not the exit status, is what counts.
                let _ = run("launchctl", &["bootout", &target]);
                if run("launchctl", &["print", &target]).is_ok() {
                    return Err("launchd still has it loaded".into());
                }
                Ok(())
            }
        }
    }

    fn systemd_unit(&mut self, name: &str) -> Option<Result<Loaded, String>> {
        let answers = *self.systemd.get_or_insert_with(|| {
            busctl(&[
                "get-property",
                SYSTEMD,
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager",
                "Version",
            ])
            .is_ok()
        });
        answers.then(|| ask_systemd(name))
    }
}

fn busctl(args: &[&str]) -> Result<String, String> {
    let mut all = vec!["--user", "--json=short"];
    all.extend_from_slice(args);
    output("busctl", &all)
}

/// Asks systemd for the unit `name`, loading it if it is not loaded.
fn ask_systemd(name: &str) -> Result<Loaded, String> {
    let path = busctl(&[
        "call",
        SYSTEMD,
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "LoadUnit",
        "s",
        name,
    ])?;
    let path = json_lines(&path)?
        .first()
        .and_then(|v| v.pointer("/data/0"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or("systemd gave no unit")?;
    let unit = busctl(&[
        "get-property",
        SYSTEMD,
        &path,
        "org.freedesktop.systemd1.Unit",
        "LoadState",
        "NeedDaemonReload",
    ])?;
    let loaded = unit.contains("\"loaded\"");
    let service = if loaded {
        busctl(&[
            "get-property",
            SYSTEMD,
            &path,
            "org.freedesktop.systemd1.Service",
            "ExecStart",
            "Environment",
            "EnvironmentFiles",
            "UnsetEnvironment",
        ])?
    } else {
        String::new()
    };
    read_busctl(&unit, &service)
}

fn json_lines(text: &str) -> Result<Vec<serde_json::Value>, String> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|_| "systemd's answer could not be read".into()))
        .collect()
}

/// Reads what `busctl --json=short get-property` printed: `LoadState` and `NeedDaemonReload`
/// from the unit, then `ExecStart`, `Environment`, `EnvironmentFiles` and `UnsetEnvironment`
/// from the service, one JSON value a line, in the order asked.
pub fn read_busctl(unit: &str, service: &str) -> Result<Loaded, String> {
    let wrong = || "systemd's answer could not be read".to_string();
    let unit = json_lines(unit)?;
    let [state, reload] = unit.as_slice() else {
        return Err(wrong());
    };
    let mut loaded = Loaded {
        state: state["data"].as_str().ok_or_else(wrong)?.to_string(),
        needs_reload: reload["data"].as_bool().ok_or_else(wrong)?,
        ..Loaded::default()
    };
    if loaded.state != "loaded" {
        return Ok(loaded);
    }
    let service = json_lines(service)?;
    let [exec, environment, files, unset] = service.as_slice() else {
        return Err(wrong());
    };
    let strings = |value: &serde_json::Value| -> Option<Vec<String>> {
        value
            .as_array()?
            .iter()
            .map(|s| s.as_str().map(String::from))
            .collect()
    };
    for command in exec["data"].as_array().ok_or_else(wrong)? {
        // (path, argv, ignore errors, times..., pid, code, status): the program is the path,
        // which `@` may set apart from what it is told its name is.
        let path = command[0].as_str().ok_or_else(wrong)?;
        let argv = strings(&command[1]).ok_or_else(wrong)?;
        let mut args = vec![path.to_string()];
        args.extend(argv.into_iter().skip(1));
        loaded.exec_start.push(args);
    }
    loaded.environment = strings(&environment["data"]).ok_or_else(wrong)?;
    for file in files["data"].as_array().ok_or_else(wrong)? {
        let path = file[0].as_str().ok_or_else(wrong)?;
        let optional = file[1].as_bool().ok_or_else(wrong)?;
        loaded.environment_files.push((path.to_string(), optional));
    }
    loaded.unset_environment = strings(&unset["data"]).ok_or_else(wrong)?;
    Ok(loaded)
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("`{program}` could not be run ({})", e.kind()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{program} {}` failed", args.join(" ")))
    }
}

fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("`{program}` could not be run ({})", e.kind()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("`{program} {}` failed", args.join(" ")))
    }
}

fn user_id() -> Result<String, String> {
    let id = output("id", &["-u"]).map_err(|_| "the user id could not be read")?;
    let id = id.trim().to_string();
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err("the user id could not be read".into())
    }
}

/// The services in `systemd_units` and `launch_agents` that run on `home`, harnesses first, so
/// they are stopped before the connector they need. `user_home` is what `%h` stands for when a
/// unit's files are read.
pub fn find(
    home: &Path,
    systemd_units: &Path,
    launch_agents: &Path,
    user_home: &Path,
    services: &mut dyn Services,
) -> Vec<Service> {
    let mut found = Vec::new();
    for (folder, manager, extension) in [
        (systemd_units, Manager::Systemd, "service"),
        (launch_agents, Manager::Launchd, "plist"),
    ] {
        let Ok(entries) = fs::read_dir(folder) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == extension) && p.is_file())
            .collect();
        files.sort();
        for file in files {
            let views = match manager {
                Manager::Systemd => systemd_service(&file, user_home, services),
                Manager::Launchd => fs::read_to_string(&file)
                    .ok()
                    .and_then(|text| read_plist(&file, &text))
                    .into_iter()
                    .collect(),
            };
            if let Some(service) = views.into_iter().find(|s| runs_on(s, home)) {
                found.push(service);
            }
        }
    }
    found.sort_by_key(|s| (s.role == Role::Connector, s.name.clone()));
    found
}

/// A systemd unit as it ran: as systemd has it, or as its files say when systemd cannot be
/// asked. When systemd and the files may disagree, both, each marked doubtful, so the unit is
/// found on the home by either.
fn systemd_service(file: &Path, user_home: &Path, services: &mut dyn Services) -> Vec<Service> {
    let Some(name) = file.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Vec::new();
    };
    let from_files = read_unit_files(file, user_home);
    let doubted = |why: String| -> Vec<Service> {
        from_files
            .iter()
            .filter_map(|(loaded, doubts)| {
                let mut doubts = doubts.clone();
                doubts.push(why.clone());
                built(&name, file, loaded, doubts)
            })
            .collect()
    };
    match services.systemd_unit(&name) {
        None => from_files
            .iter()
            .filter_map(|(loaded, doubts)| built(&name, file, loaded, doubts.clone()))
            .collect(),
        Some(Ok(loaded)) if loaded.state == "loaded" && !loaded.needs_reload => {
            built(&name, file, &loaded, Vec::new())
                .into_iter()
                .collect()
        }
        Some(Ok(loaded)) if loaded.state == "loaded" => {
            let why = "its files changed since systemd read them, so it may run otherwise when \
                       next started; run `systemctl --user daemon-reload` and restart it"
                .to_string();
            let mut views: Vec<Service> = built(&name, file, &loaded, vec![why.clone()])
                .into_iter()
                .collect();
            views.extend(doubted(why));
            views
        }
        Some(Ok(loaded)) => doubted(format!(
            "systemd reports it as {}, not as its files say",
            loaded.state
        )),
        Some(Err(why)) => doubted(format!("systemd could not be asked about it ({why})")),
    }
}

/// Puts a service together from a unit as loaded: its command, and its environment with the
/// files it names read and `UnsetEnvironment=` applied.
fn built(name: &str, file: &Path, loaded: &Loaded, mut doubts: Vec<String>) -> Option<Service> {
    let args = match loaded.exec_start.as_slice() {
        [] => return None,
        [one] => one.clone(),
        [first, ..] => {
            doubts.push("it runs several commands".into());
            first.clone()
        }
    };
    if args.is_empty() {
        return None;
    }
    if args.iter().any(|a| a.contains('$')) {
        doubts.push(
            "its command line has `$` references, which systemd fills in only as it starts it"
                .into(),
        );
    }
    let mut env = Vec::new();
    let mut unsure = Vec::new();
    for assignment in &loaded.environment {
        if let Some((k, v)) = assignment.split_once('=') {
            set(&mut env, k, v);
        }
    }
    // An environment file's variables win over `Environment=`.
    for (path, optional) in &loaded.environment_files {
        if path.contains(['*', '?', '[']) {
            doubts.push("it names an environment file by a pattern".into());
            continue;
        }
        match fs::read_to_string(path) {
            Ok(text) => {
                let read = environment_file(&text);
                for (k, v) in read.vars {
                    set(&mut env, &k, &v);
                    unsure.retain(|u: &Unsure| u.name != k);
                }
                for (name, written) in read.unsure {
                    env.retain(|(k, _)| *k != name);
                    unsure.retain(|u| u.name != name);
                    unsure.push(Unsure {
                        name,
                        file: path.clone(),
                        written,
                    });
                }
            }
            Err(e) if *optional && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => doubts.push("it needs an environment file that cannot be read".into()),
        }
    }
    for unset in &loaded.unset_environment {
        match unset.split_once('=') {
            Some((k, v)) => env.retain(|(name, value)| !(name == k && value == v)),
            None => {
                env.retain(|(name, _)| name != unset);
                unsure.retain(|u| &u.name != unset);
            }
        }
    }
    // Only the sender lists, which then let only the owner prompt, may be unsure.
    for u in &unsure {
        if !is_sender_list(&u.name) && (u.name.starts_with("MARMOT_") || u.name.starts_with("WN_"))
        {
            doubts.push(format!(
                "its environment file {} sets `{}` in a way systemd reads otherwise by version",
                u.file, u.name
            ));
        }
    }
    doubts.dedup();
    Some(Service {
        manager: Manager::Systemd,
        name: name.to_string(),
        file: file.to_path_buf(),
        role: role_of(&args),
        args,
        env,
        doubt: (!doubts.is_empty()).then(|| doubts.join("; ")),
        unsure,
    })
}

/// Sets `name` to `value`, replacing a value set before.
fn set(env: &mut Vec<(String, String)>, name: &str, value: &str) {
    match env.iter_mut().find(|(k, _)| k == name) {
        Some((_, v)) => *v = value.to_string(),
        None => env.push((name.to_string(), value.to_string())),
    }
}

/// Whether a service runs on `home`. One of the old connector's programs whose home is a `$`
/// reference may, and is taken to.
fn runs_on(service: &Service, home: &Path) -> bool {
    let named: Vec<String> = service
        .values_of("--home")
        .into_iter()
        .chain(service.env("MARMOT_HOME").map(str::to_string))
        .collect();
    let unsure = service.unsure.iter().any(|u| u.name == "MARMOT_HOME");
    named.iter().any(|h| same_place(Path::new(h), home))
        || (service.role != Role::Other && (unsure || named.iter().any(|h| h.contains('$'))))
}

fn same_place(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn program_name(args: &[String]) -> Option<String> {
    args.first()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

fn role_of(args: &[String]) -> Role {
    match program_name(args).as_deref() {
        Some("wn-agent") => Role::Connector,
        Some(p) if HARNESS_PROGRAMS.contains(&p) => Role::Harness,
        _ => Role::Other,
    }
}

/// The logical lines of a unit file: a line ending in `\` goes on with the next, and comment
/// lines within one are skipped, as systemd reads them.
fn logical_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut pending: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if pending.is_some() && (line.starts_with('#') || line.starts_with(';')) {
            continue;
        }
        let (part, goes_on) = match line.strip_suffix('\\') {
            Some(part) => (part, true),
            None => (line, false),
        };
        let joined = match pending.take() {
            Some(mut before) => {
                before.push(' ');
                before.push_str(part);
                before
            }
            None => part.to_string(),
        };
        if goes_on {
            pending = Some(joined);
        } else {
            lines.push(joined);
        }
    }
    lines.extend(pending);
    lines
}

/// Reads the `[Service]` sections of a unit's files into `loaded`, in order, as systemd merges
/// them; what cannot be read for certain goes into `doubts`.
fn read_sections(text: &str, user_home: &Path, loaded: &mut Loaded, doubts: &mut Vec<String>) {
    let mut in_service = false;
    for line in logical_lines(text) {
        let line = line.trim();
        if line.starts_with('[') {
            in_service = line == "[Service]";
            continue;
        }
        if !in_service || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if ![
            "ExecStart",
            "Environment",
            "EnvironmentFile",
            "UnsetEnvironment",
        ]
        .contains(&key)
        {
            continue;
        }
        // An empty assignment resets the list, as systemd does.
        if value.is_empty() {
            match key {
                "ExecStart" => loaded.exec_start.clear(),
                "Environment" => loaded.environment.clear(),
                "EnvironmentFile" => loaded.environment_files.clear(),
                _ => loaded.unset_environment.clear(),
            }
            continue;
        }
        // Specifiers are filled in first, then the words are taken apart. One that is not
        // filled in here is left as written, so the service is still found, and doubted.
        let value = specifiers(value, user_home).unwrap_or_else(|| {
            doubts.push("it uses a systemd specifier other than `%h`".into());
            value.to_string()
        });
        match key {
            "ExecStart" => {
                // A leading `-`, `@`, `+`, `!`, `:` or `|` changes how systemd runs it, not what;
                // after `@` the second word is only the name the program is told it has.
                let prefix_len = value.len()
                    - value
                        .trim_start_matches(['-', '@', '+', '!', ':', '|'])
                        .len();
                let (prefix, command) = value.split_at(prefix_len);
                let mut args = words(command);
                if prefix.contains('@') && args.len() > 1 {
                    args.remove(1);
                }
                loaded.exec_start.push(args);
            }
            "Environment" => loaded.environment.extend(words(&value)),
            "EnvironmentFile" => {
                let (optional, path) = match value.strip_prefix('-') {
                    Some(path) => (true, path),
                    None => (false, value.as_str()),
                };
                loaded.environment_files.push((path.to_string(), optional));
            }
            _ => loaded.unset_environment.extend(words(&value)),
        }
    }
}

/// The drop-in files for a unit, in the order systemd applies them: by file name, from
/// `<unit>.d`, the prefix folders such as `wn-agent-.service.d`, and `service.d` beside it, the
/// more specific folder winning a tie.
fn drop_ins(file: &Path) -> Vec<PathBuf> {
    let (Some(folder), Some(name)) = (file.parent(), file.file_name()) else {
        return Vec::new();
    };
    let name = name.to_string_lossy();
    let stem = name.strip_suffix(".service").unwrap_or(&name);
    let mut dirs = vec![folder.join(format!("{name}.d"))];
    for (at, _) in stem.rmatch_indices('-') {
        dirs.push(folder.join(format!("{}-.service.d", &stem[..at])));
    }
    dirs.push(folder.join("service.d"));
    let mut by_name = std::collections::BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "conf") {
                by_name.entry(entry.file_name()).or_insert(path);
            }
        }
    }
    by_name.into_values().collect()
}

/// `%h` is the person's own folder and `%%` a percent sign; any other specifier is not filled in
/// here, and gives nothing.
fn specifiers(text: &str, user_home: &Path) -> Option<String> {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('h') => out.push_str(&user_home.to_string_lossy()),
            _ => return None,
        }
    }
    Some(out)
}

/// Reads a systemd unit's files as systemd merges them with its drop-ins, for when systemd
/// cannot be asked; with what could not be read for certain.
fn read_unit_files(file: &Path, user_home: &Path) -> Option<(Loaded, Vec<String>)> {
    let mut loaded = Loaded {
        state: "loaded".into(),
        ..Loaded::default()
    };
    let mut doubts = Vec::new();
    read_sections(
        &fs::read_to_string(file).ok()?,
        user_home,
        &mut loaded,
        &mut doubts,
    );
    for drop_in in drop_ins(file) {
        match fs::read_to_string(&drop_in) {
            Ok(text) => read_sections(&text, user_home, &mut loaded, &mut doubts),
            Err(_) => doubts.push("one of its drop-ins cannot be read".into()),
        }
    }
    Some((loaded, doubts))
}

/// Splits a command line as the installers write one: words in double quotes, with `\\` and `\"`
/// inside (systemd's quoting), in single quotes, or bare with backslashes (bash's `%q`). A `$` is
/// kept as it is: what it stands for is not known here.
pub fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return words;
        }
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            chars.next();
            match c {
                '"' => {
                    while let Some(c) = chars.next() {
                        match c {
                            '"' => break,
                            '\\' => word.extend(chars.next()),
                            c => word.push(c),
                        }
                    }
                }
                '\'' => {
                    for c in chars.by_ref() {
                        if c == '\'' {
                            break;
                        }
                        word.push(c);
                    }
                }
                '\\' => word.extend(chars.next()),
                c => word.push(c),
            }
        }
        words.push(word);
    }
}

/// Reads a LaunchAgent's label, `ProgramArguments` and `EnvironmentVariables`.
fn read_plist(file: &Path, text: &str) -> Option<Service> {
    let tokens = plist_tokens(text);
    let mut label = None;
    let mut args = Vec::new();
    let mut env = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Key(k) if k == "Label" => {
                if let Some(Token::String(s)) = tokens.get(i + 1) {
                    label = Some(s.clone());
                }
            }
            Token::Key(k) if k == "ProgramArguments" => {
                let mut j = i + 1;
                while let Some(Token::String(s)) = tokens.get(j) {
                    args.push(s.clone());
                    j += 1;
                }
                i = j;
                continue;
            }
            Token::Key(k) if k == "EnvironmentVariables" => {
                let mut j = i + 1;
                while let (Some(Token::Key(k)), Some(Token::String(v))) =
                    (tokens.get(j), tokens.get(j + 1))
                {
                    env.push((k.clone(), v.clone()));
                    j += 2;
                }
                i = j;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if args.is_empty() {
        return None;
    }
    let name = label.or_else(|| Some(file.file_stem()?.to_string_lossy().into_owned()))?;
    Some(Service {
        manager: Manager::Launchd,
        name,
        file: file.to_path_buf(),
        role: role_of(&args),
        args,
        env,
        doubt: None,
        unsure: Vec::new(),
    })
}

enum Token {
    Key(String),
    String(String),
}

/// The `<key>` and `<string>` elements of a property list, in order; the rest is skipped.
fn plist_tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('>') else { break };
        let tag = &rest[..close];
        rest = &rest[close + 1..];
        let make: fn(String) -> Token = match tag {
            "key" => Token::Key,
            "string" => Token::String,
            _ => continue,
        };
        let end = format!("</{tag}>");
        let Some(stop) = rest.find(&end) else { break };
        tokens.push(make(unescape_xml(&rest[..stop])));
        rest = &rest[stop + end.len()..];
    }
    tokens
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No systemd to ask: the unit's files are read.
    struct Offline;

    /// A systemd that has every unit loaded as given.
    struct Answers(Option<Result<Loaded, String>>);

    impl Services for Offline {
        fn stop_and_disable(&mut self, _: &Service) -> Result<(), String> {
            unreachable!()
        }
        fn systemd_unit(&mut self, _: &str) -> Option<Result<Loaded, String>> {
            None
        }
    }

    impl Services for Answers {
        fn stop_and_disable(&mut self, _: &Service) -> Result<(), String> {
            unreachable!()
        }
        fn systemd_unit(&mut self, _: &str) -> Option<Result<Loaded, String>> {
            self.0.clone()
        }
    }

    fn read(file: &Path, user_home: &Path, services: &mut dyn Services) -> Service {
        systemd_service(file, user_home, services)
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn words_reads_both_quotings() {
        assert_eq!(
            words(r#""/bin/wn-agent" "--home" "/h/a \"b\" $x" --relay ws://r"#),
            [
                "/bin/wn-agent",
                "--home",
                "/h/a \"b\" $x",
                "--relay",
                "ws://r"
            ]
        );
        assert_eq!(
            words(r"/bin/wn-agent --home /h/with\ space 'single quoted'"),
            ["/bin/wn-agent", "--home", "/h/with space", "single quoted"]
        );
    }

    /// The cases as systemd 257 read them, run with `systemd-run --user -p EnvironmentFile=`.
    #[test]
    fn an_environment_file_is_read_as_systemd_reads_it() {
        let text = concat!(
            "A=a\"b\"c\n",           // quotes inside a bare value are kept
            "B=\"x\nC=y\"\n",        // a quoted value goes on over the line's end
            "D=d # not a comment\n", // `#` starts a comment only at a line's start
            "; I=i\n",               // `;` starts one too
            "export E=e\n",          // not a name: systemd leaves it out
            "F=f\\\ng\n",            // a backslash at the end goes on to the next line
            "G = \"q\" r \n",        // spaces around `=` and between parts go
            "H='s\\n'\"t\\\"u\"\n",  // single quotes keep a backslash; double take it off `"`
            "  J=j  \n",             // spaces around a line go
            "K=\"l1\\\nl2\"\n",      // a line goes on inside double quotes too
            "L\n",                   // no `=`: nothing
            "M=\n",                  // an empty value
            "N=1\r\n",               // CR ends a line
            "N=2\n",                 // the last one counts
            "O=\"open",              // unclosed at the end: kept as it is
        );
        let read = environment_file(text);
        let vars: Vec<(&str, &str)> = read
            .vars
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            vars,
            [
                ("A", "a\"b\"c"),
                ("B", "x\nC=y"),
                ("D", "d # not a comment"),
                ("F", "fg"),
                ("G", "qr"),
                ("H", "s\\nt\"u"),
                ("J", "j"),
                ("K", "l1l2"),
                ("M", ""),
                ("N", "2"),
                ("O", "open"),
            ]
        );
        // `export E` is not a name, so systemd sets nothing; whoever wrote it meant `E`.
        assert_eq!(read.unsure.len(), 1);
        assert_eq!(read.unsure[0].0, "E");
        assert!(read.unsure[0].1.contains("export E=e"));
    }

    /// systemd before 254 went on from a comment ending in a backslash into the next line, and
    /// older ones took a backslash in double quotes off other characters than today's do. A name
    /// such a line sets is unsure, with the line as written.
    #[test]
    fn a_line_systemd_reads_otherwise_by_version_is_unsure() {
        let read = environment_file("# a comment \\\nW=1\nX=\"a\\$b\"\nY=plain\n");
        let names: Vec<&str> = read.unsure.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["W", "X"]);
        assert!(read.unsure[0].1.contains("W=1"), "{:?}", read.unsure[0]);
        assert!(
            read.unsure[1].1.contains("X=\"a\\$b\""),
            "{:?}",
            read.unsure[1]
        );
        assert!(read.vars.contains(&("Y".into(), "plain".into())));
        // An escaped backslash before the line's end does not go on.
        let read = environment_file("# a comment \\\\\nW=1\n");
        assert!(read.unsure.is_empty());
    }

    /// A service whose environment file sets a sender list unsurely is not doubtful: the list
    /// is unsure, and the import lets only the owner prompt. Any other setting the import reads
    /// makes it doubtful.
    #[test]
    fn an_unsure_line_marks_its_name() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("wn-codex.service");
        let vars = root.path().join("vars.env");
        fs::write(
            &file,
            format!(
                "[Service]\nEnvironment=WN_CODEX_ALLOWED_SENDERS_HEX=aa\nEnvironmentFile={}\nExecStart=/x/wn-codex\n",
                vars.display()
            ),
        )
        .unwrap();
        fs::write(&vars, "# old \\\nWN_CODEX_ALLOWED_SENDERS_HEX=bb\n").unwrap();
        let unit = read(&file, root.path(), &mut Offline);
        assert_eq!(unit.doubt, None);
        assert_eq!(unit.env("WN_CODEX_ALLOWED_SENDERS_HEX"), None);
        assert_eq!(unit.unsure.len(), 1);
        assert_eq!(unit.unsure[0].name, "WN_CODEX_ALLOWED_SENDERS_HEX");
        assert!(unit.unsure[0].written.contains("=bb"));
        // A later file that sets it plainly settles it.
        let again = root.path().join("again.env");
        fs::write(&again, "WN_CODEX_ALLOWED_SENDERS_HEX=cc\n").unwrap();
        let text = fs::read_to_string(&file).unwrap().replace(
            "ExecStart",
            &format!("EnvironmentFile={}\nExecStart", again.display()),
        );
        fs::write(&file, text).unwrap();
        let unit = read(&file, root.path(), &mut Offline);
        assert!(unit.unsure.is_empty());
        assert_eq!(unit.env("WN_CODEX_ALLOWED_SENDERS_HEX"), Some("cc"));
        // The relays are not a sender list: the service is doubtful.
        fs::write(&vars, "export MARMOT_RELAYS=ws://a\n").unwrap();
        fs::write(&again, "").unwrap();
        let unit = read(&file, root.path(), &mut Offline);
        assert!(
            unit.doubt
                .as_deref()
                .is_some_and(|d| d.contains("MARMOT_RELAYS")),
            "{:?}",
            unit.doubt
        );
        // Nor is a name the import never reads.
        fs::write(&vars, "export OTHER=1\n").unwrap();
        assert_eq!(read(&file, root.path(), &mut Offline).doubt, None);
    }

    #[test]
    fn services_on_the_home_are_found_harness_first() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        fs::create_dir(&home).unwrap();
        let units = root.path().join("units");
        let agents = root.path().join("agents");
        fs::create_dir(&units).unwrap();
        fs::create_dir(&agents).unwrap();
        let h = home.display();
        fs::write(
            units.join("wn-agent-codex.service"),
            format!(
                "[Service]\nExecStart=\"/x/wn-agent\" \"--home\" \"{h}\" \"--socket\" \"{h}/dev/wn-agent.sock\" \"--relay\" \"ws://one\" \"--relay\" \"ws://two\"\n"
            ),
        )
        .unwrap();
        fs::write(
            units.join("wn-codex.service"),
            format!(
                "[Service]\nEnvironment=\"MARMOT_HOME={h}\"\nEnvironment=\"WN_CODEX_ALLOWED_SENDERS_HEX=aa,bb\"\nExecStart=\"/x/wn-codex\"\n"
            ),
        )
        .unwrap();
        fs::write(
            units.join("wn-agent-other.service"),
            "[Service]\nExecStart=\"/x/wn-agent\" \"--home\" \"/elsewhere\"\n",
        )
        .unwrap();
        fs::write(
            agents.join("org.marmot.wn-agent.codex.plist"),
            format!(
                "<plist><dict><key>Label</key><string>org.marmot.wn-agent.codex</string>\
                 <key>ProgramArguments</key><array><string>/x/wn-agent</string><string>--home</string>\
                 <string>{h}</string><string>--relay</string><string>ws://a&amp;b</string></array></dict></plist>"
            ),
        )
        .unwrap();

        let found = find(&home, &units, &agents, root.path(), &mut Offline);
        let names: Vec<_> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "wn-codex.service",
                "org.marmot.wn-agent.codex",
                "wn-agent-codex.service"
            ]
        );
        assert_eq!(found[0].role, Role::Harness);
        assert_eq!(found[0].env("WN_CODEX_ALLOWED_SENDERS_HEX"), Some("aa,bb"));
        assert_eq!(found[1].manager, Manager::Launchd);
        assert_eq!(found[1].values_of("--relay"), ["ws://a&b"]);
        assert_eq!(found[2].role, Role::Connector);
        assert_eq!(found[2].values_of("--relay"), ["ws://one", "ws://two"]);
        assert!(found.iter().all(|s| s.doubt.is_none()));
        let shown = format!("{:?}", found[2]);
        assert!(
            !shown.contains("ws://") && !shown.contains(&h.to_string()),
            "{shown}"
        );
    }

    #[test]
    fn a_unit_is_read_as_systemd_merges_it() {
        let root = tempfile::tempdir().unwrap();
        let units = root.path().join("units");
        for dir in [
            "wn-codex.service.d",
            "wn-.service.d",
            "service.d",
            "wn-agent-.service.d",
        ] {
            fs::create_dir_all(units.join(dir)).unwrap();
        }
        // A line that goes on, with a comment inside it, as systemd reads one.
        fs::write(
            units.join("wn-codex.service"),
            "[Unit]
Environment=NOT=service
[Service]
Environment=A=unit B=unit \\
# a comment within
  D=unit
             ExecStart=/x/wn-codex \\
  --old
",
        )
        .unwrap();
        fs::write(root.path().join("vars.env"), "B=file\nE=file\n").unwrap();
        // Applied by file name across the folders: `05-` from `wn-.service.d`, a prefix folder,
        // `10-` from `service.d`, then the unit's own `20-`. The `20-` in `service.d` is
        // overridden by the unit's own, and `wn-agent-.service.d` is not this unit's.
        fs::write(
            units.join("wn-.service.d/05-prefix.conf"),
            "[Service]\nEnvironment=F=prefix\n",
        )
        .unwrap();
        fs::write(
            units.join("service.d/10-all.conf"),
            "[Service]\nEnvironment=C=all\n",
        )
        .unwrap();
        fs::write(
            units.join("service.d/20-mine.conf"),
            "[Service]\nEnvironment=C=overridden\n",
        )
        .unwrap();
        fs::write(
            units.join("wn-agent-.service.d/10-agent.conf"),
            "[Service]\nEnvironment=G=agent\n",
        )
        .unwrap();
        fs::write(
            units.join("wn-codex.service.d/20-mine.conf"),
            "[Service]
ExecStart=
ExecStart=/x/wn-codex --new
EnvironmentFile=%h/vars.env
             EnvironmentFile=-%h/missing.env
UnsetEnvironment=D E=other
UnsetEnvironment=E=file
",
        )
        .unwrap();
        let file = units.join("wn-codex.service");
        let unit = read(&file, root.path(), &mut Offline);
        assert_eq!(unit.args, ["/x/wn-codex", "--new"]);
        assert_eq!(unit.role, Role::Harness);
        assert_eq!(unit.env("A"), Some("unit"));
        assert_eq!(unit.env("B"), Some("file"), "an environment file wins");
        assert_eq!(unit.env("C"), Some("all"));
        assert_eq!(unit.env("D"), None, "unset by name");
        assert_eq!(unit.env("E"), None, "unset by its value");
        assert_eq!(unit.env("F"), Some("prefix"));
        assert_eq!(unit.env("G"), None, "another prefix's drop-in");
        assert_eq!(unit.env("NOT"), None, "only [Service] counts");
        assert_eq!(unit.doubt, None, "a missing optional file is fine");

        // A reset empties the list.
        fs::write(
            units.join("wn-codex.service.d/40-reset.conf"),
            "[Service]\nEnvironment=\nEnvironmentFile=\n",
        )
        .unwrap();
        let unit = read(&file, root.path(), &mut Offline);
        assert!(unit.env.is_empty());
    }

    /// What the files cannot settle, and what systemd fills in only as it starts a service, make
    /// it doubtful, whether systemd is asked or the files are read.
    #[test]
    fn what_cannot_be_read_for_certain_makes_a_service_doubtful() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("wn-agent-codex.service");
        let cases = [
            (
                "ExecStart=/x/wn-agent --home /h --media-allowed-root ${MEDIA}",
                "`$` references",
            ),
            (
                "ExecStart=/x/wn-agent --home /h --relay $R",
                "`$` references",
            ),
            (
                "ExecStart=/x/wn-agent --home /h\nEnvironmentFile=/etc/wn/*.env",
                "a pattern",
            ),
            (
                "ExecStart=/x/wn-agent --home /h\nEnvironmentFile=-/nowhere/x.env\nEnvironmentFile=/nowhere/y.env",
                "cannot be read",
            ),
            ("ExecStart=/x/wn-agent --home /h/%u", "specifier"),
        ];
        for (text, why) in cases {
            fs::write(&file, format!("[Service]\n{text}\n")).unwrap();
            let doubt = read(&file, root.path(), &mut Offline).doubt;
            assert!(
                doubt.as_deref().is_some_and(|d| d.contains(why)),
                "{text}: {doubt:?}"
            );
        }

        // The same from systemd, which never fills in `$` before the start either.
        let loaded = |exec: &[&str], files: &[(&str, bool)]| Loaded {
            state: "loaded".into(),
            exec_start: vec![exec.iter().map(|s| s.to_string()).collect()],
            environment_files: files.iter().map(|(p, o)| (p.to_string(), *o)).collect(),
            ..Loaded::default()
        };
        let asked = |l: Loaded| read(&file, root.path(), &mut Answers(Some(Ok(l)))).doubt;
        let media = loaded(
            &[
                "/x/wn-agent",
                "--home",
                "/h",
                "--media-allowed-root",
                "${MEDIA}",
            ],
            &[],
        );
        assert!(asked(media).unwrap().contains("`$` references"));
        let pattern = loaded(&["/x/wn-agent"], &[("/etc/wn/*.env", true)]);
        assert!(asked(pattern).unwrap().contains("a pattern"));
        let gone = loaded(&["/x/wn-agent"], &[("/nowhere/y.env", false)]);
        assert!(asked(gone).unwrap().contains("cannot be read"));
        let plain = loaded(
            &["/x/wn-agent", "--home", "/h"],
            &[("/nowhere/y.env", true)],
        );
        assert_eq!(asked(plain), None);
    }

    /// systemd's own view counts over the files; where systemd and the files may differ, the
    /// service is doubtful.
    #[test]
    fn systemd_is_asked_before_the_files_are_read() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        fs::create_dir(&home).unwrap();
        let file = root.path().join("wn-agent-codex.service");
        let h = home.to_string_lossy().into_owned();
        // The file names another home; systemd, with a drop-in the files here do not show,
        // runs this one.
        fs::write(
            &file,
            "[Service]\nExecStart=/x/wn-agent --home /elsewhere\n",
        )
        .unwrap();
        let loaded = Loaded {
            state: "loaded".into(),
            exec_start: vec![vec!["/x/wn-agent".into(), "--home".into(), h.clone()]],
            environment: vec!["MARMOT_RELAYS=ws://a b".into(), "GONE=1".into()],
            unset_environment: vec!["GONE".into()],
            ..Loaded::default()
        };
        let mut systemd = Answers(Some(Ok(loaded.clone())));
        let found = find(
            &home,
            root.path(),
            &root.path().join("none"),
            root.path(),
            &mut systemd,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].env("MARMOT_RELAYS"), Some("ws://a b"));
        assert_eq!(found[0].env("GONE"), None);
        assert_eq!(found[0].doubt, None);

        // Changed since systemd read it: found by either view, and doubtful.
        let mut stale = Answers(Some(Ok(Loaded {
            needs_reload: true,
            ..loaded
        })));
        let found = find(
            &home,
            root.path(),
            &root.path().join("none"),
            root.path(),
            &mut stale,
        );
        assert!(found[0].doubt.as_deref().unwrap().contains("daemon-reload"));
        fs::write(
            &file,
            format!("[Service]\nExecStart=/x/wn-agent --home {h}\n"),
        )
        .unwrap();
        for answer in [
            Ok(Loaded {
                state: "masked".into(),
                ..Loaded::default()
            }),
            Err("no answer".into()),
        ] {
            let found = find(
                &home,
                root.path(),
                &root.path().join("none"),
                root.path(),
                &mut Answers(Some(answer)),
            );
            assert!(found[0].doubt.is_some());
        }
    }

    #[test]
    fn what_busctl_prints_is_read() {
        // As systemd 257 printed it, for a unit run with arguments that hold spaces and `$`.
        let unit = "{\"type\":\"s\",\"data\":\"loaded\"}\n{\"type\":\"b\",\"data\":false}\n";
        let service = concat!(
            "{\"type\":\"a(sasbttttuii)\",\"data\":[[\"/bin/true\",[\"true\",\"a b\",\"$X\"],false,0,0,0,0,0,0,0]]}\n",
            "{\"type\":\"as\",\"data\":[\"A=x y\"]}\n",
            "{\"type\":\"a(sb)\",\"data\":[[\"/n/x y.env\",true],[\"/n/z.env\",false]]}\n",
            "{\"type\":\"as\",\"data\":[\"FOO\"]}\n",
        );
        let loaded = read_busctl(unit, service).unwrap();
        assert_eq!(loaded.exec_start, [["/bin/true", "a b", "$X"]]);
        assert_eq!(loaded.environment, ["A=x y"]);
        assert_eq!(
            loaded.environment_files,
            [
                ("/n/x y.env".to_string(), true),
                ("/n/z.env".to_string(), false)
            ]
        );
        assert_eq!(loaded.unset_environment, ["FOO"]);
        let gone = read_busctl(
            "{\"type\":\"s\",\"data\":\"not-found\"}\n{\"type\":\"b\",\"data\":false}\n",
            "",
        )
        .unwrap();
        assert_eq!(gone.state, "not-found");
        assert!(read_busctl("not json", "").is_err());
    }
}
