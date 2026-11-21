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
//! A systemd unit is read as systemd merges it: its `[Service]` section, then its drop-ins
//! (`<unit>.d/*.conf`, and `service.d/*.conf` for every service) in name order, with the files
//! `EnvironmentFile=` names, whose variables win over `Environment=`.

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
    /// Its environment, as it ran.
    pub env: Vec<(String, String)>,
    /// Whether a file its environment needs could not be read, so the environment above is not
    /// all of it, and the service could not have started as it stands.
    pub env_unreadable: bool,
}

/// Paths, keys and relay addresses stay out of logs (D-071): only the shape is shown.
impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service")
            .field("manager", &self.manager)
            .field("role", &self.role)
            .field("args", &self.args.len())
            .field("env", &self.env.len())
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

/// Stops and disables a service, leaving its file where it is. The seam the tests replace.
pub trait Services {
    /// Stops `service` and keeps it from starting again, at login or otherwise. Succeeds only
    /// once the service is known to be stopped.
    fn stop_and_disable(&mut self, service: &Service) -> Result<(), String>;
}

/// The machine's own service managers: `systemctl --user` and `launchctl`.
pub struct System;

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
/// they are stopped before the connector they need. `user_home` is what `%h` stands for.
pub fn find(
    home: &Path,
    systemd_units: &Path,
    launch_agents: &Path,
    user_home: &Path,
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
            let read = match manager {
                Manager::Systemd => read_unit(&file, user_home),
                Manager::Launchd => fs::read_to_string(&file)
                    .ok()
                    .and_then(|text| read_plist(&file, &text)),
            };
            if let Some(service) = read.filter(|s| runs_on(s, home)) {
                found.push(service);
            }
        }
    }
    found.sort_by_key(|s| (s.role == Role::Connector, s.name.clone()));
    found
}

fn runs_on(service: &Service, home: &Path) -> bool {
    let named = service
        .values_of("--home")
        .into_iter()
        .chain(service.env("MARMOT_HOME").map(str::to_string));
    named.into_iter().any(|h| same_place(Path::new(&h), home))
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

/// What a unit's `[Service]` sections say, merged in order.
#[derive(Default)]
struct UnitSettings {
    exec_start: Option<String>,
    environment: Vec<String>,
    environment_files: Vec<String>,
}

impl UnitSettings {
    fn read(&mut self, text: &str) {
        let mut in_service = false;
        for line in text.lines() {
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
            let value = value.trim();
            // An empty assignment resets the list, as systemd does.
            match key.trim() {
                "ExecStart" if value.is_empty() => self.exec_start = None,
                "ExecStart" => self.exec_start = Some(value.to_string()),
                "Environment" if value.is_empty() => self.environment.clear(),
                "Environment" => self.environment.push(value.to_string()),
                "EnvironmentFile" if value.is_empty() => self.environment_files.clear(),
                "EnvironmentFile" => self.environment_files.push(value.to_string()),
                _ => {}
            }
        }
    }
}

/// The drop-in files for a unit: `service.d/*.conf` and `<unit>.d/*.conf` beside it, in the
/// order systemd applies them, by file name, a unit's own winning a tie.
fn drop_ins(file: &Path) -> Vec<PathBuf> {
    let (Some(folder), Some(name)) = (file.parent(), file.file_name()) else {
        return Vec::new();
    };
    let mut by_name = std::collections::BTreeMap::new();
    for dir in [
        folder.join("service.d"),
        folder.join(format!("{}.d", name.to_string_lossy())),
    ] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "conf") {
                by_name.insert(entry.file_name(), path);
            }
        }
    }
    by_name.into_values().collect()
}

/// `%h` is the person's own folder and `%%` a percent sign; other specifiers are left as they are.
fn specifiers(text: &str, user_home: &Path) -> String {
    text.replace("%%", "\u{0}")
        .replace("%h", &user_home.to_string_lossy())
        .replace('\u{0}', "%")
}

/// Reads a systemd unit as systemd merges it with its drop-ins and environment files.
fn read_unit(file: &Path, user_home: &Path) -> Option<Service> {
    let mut settings = UnitSettings::default();
    settings.read(&fs::read_to_string(file).ok()?);
    for drop_in in drop_ins(file) {
        if let Ok(text) = fs::read_to_string(&drop_in) {
            settings.read(&text);
        }
    }
    let command = settings.exec_start?;
    // A leading `-`, `@`, `+`, `!` or `:` changes how systemd runs it, not what.
    let command = command.trim_start_matches(['-', '@', '+', '!', ':']);
    let args: Vec<String> = words(command)
        .into_iter()
        .map(|w| specifiers(&w, user_home))
        .collect();
    if args.is_empty() {
        return None;
    }
    let mut env = Vec::new();
    for vars in &settings.environment {
        for var in words(vars) {
            if let Some((k, v)) = var.split_once('=') {
                env.push((k.to_string(), specifiers(v, user_home)));
            }
        }
    }
    let mut env_unreadable = false;
    for named in &settings.environment_files {
        let (optional, path) = match named.strip_prefix('-') {
            Some(path) => (true, path),
            None => (false, named.as_str()),
        };
        match fs::read_to_string(specifiers(path, user_home)) {
            Ok(text) => env.extend(crate::import::env_file(&text)),
            Err(_) if optional => {}
            Err(_) => env_unreadable = true,
        }
    }
    Some(Service {
        manager: Manager::Systemd,
        name: file.file_name()?.to_string_lossy().into_owned(),
        file: file.to_path_buf(),
        role: role_of(&args),
        args,
        env,
        env_unreadable,
    })
}

/// Splits a command line as the installers write one: words in double quotes, with `\\`, `\"`,
/// `$$` and `%%` inside (systemd's quoting), in single quotes, or bare with backslashes (bash's
/// `%q`).
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
                            '$' | '%' if chars.peek() == Some(&c) => {
                                chars.next();
                                word.push(c);
                            }
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
        env_unreadable: false,
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

    #[test]
    fn words_reads_both_quotings() {
        assert_eq!(
            words(r#""/bin/wn-agent" "--home" "/h/a \"b\" $$x %%y" --relay ws://r"#),
            [
                "/bin/wn-agent",
                "--home",
                "/h/a \"b\" $x %y",
                "--relay",
                "ws://r"
            ]
        );
        assert_eq!(
            words(r"/bin/wn-agent --home /h/with\ space 'single quoted'"),
            ["/bin/wn-agent", "--home", "/h/with space", "single quoted"]
        );
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

        let found = find(&home, &units, &agents, root.path());
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
        fs::create_dir_all(units.join("wn-codex.service.d")).unwrap();
        fs::create_dir_all(units.join("service.d")).unwrap();
        fs::write(
            units.join("wn-codex.service"),
            "[Unit]
Environment=NOT=service
[Service]
Environment=A=unit B=unit
             ExecStart=/x/wn-codex --old
",
        )
        .unwrap();
        fs::write(
            root.path().join("vars.env"),
            "B=file
",
        )
        .unwrap();
        // Applied by file name: `10-` from `service.d` first, then the unit's own `20-`.
        fs::write(
            units.join("service.d/10-all.conf"),
            "[Service]
Environment=C=all
",
        )
        .unwrap();
        fs::write(
            units.join("wn-codex.service.d/20-mine.conf"),
            "[Service]
ExecStart=
ExecStart=/x/wn-codex --new
EnvironmentFile=%h/vars.env
             EnvironmentFile=-%h/missing.env
",
        )
        .unwrap();
        let unit = read_unit(&units.join("wn-codex.service"), root.path()).unwrap();
        assert_eq!(unit.args, ["/x/wn-codex", "--new"]);
        assert_eq!(unit.role, Role::Harness);
        assert_eq!(unit.env("A"), Some("unit"));
        assert_eq!(unit.env("B"), Some("file"), "an environment file wins");
        assert_eq!(unit.env("C"), Some("all"));
        assert_eq!(unit.env("NOT"), None, "only [Service] counts");
        assert!(!unit.env_unreadable, "a missing optional file is fine");
        fs::write(
            units.join("wn-codex.service.d/30-broken.conf"),
            "[Service]
EnvironmentFile=%h/missing.env
",
        )
        .unwrap();
        let unit = read_unit(&units.join("wn-codex.service"), root.path()).unwrap();
        assert!(unit.env_unreadable);
        // A reset empties the list.
        fs::write(
            units.join("wn-codex.service.d/40-reset.conf"),
            "[Service]
Environment=
EnvironmentFile=
",
        )
        .unwrap();
        let unit = read_unit(&units.join("wn-codex.service"), root.path()).unwrap();
        assert!(unit.env.is_empty());
    }
}
