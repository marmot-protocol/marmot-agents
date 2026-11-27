//! `marmot-agent`: one agent's member of Marmot groups, driving Codex or Pi built in, or a
//! connector on the socket (D-072).
//!
//! ```text
//! marmot-agent init <settings.toml>   make the agent's identity, and print its public key
//! marmot-agent run <settings.toml>    run until stopped
//! marmot-agent groups <settings.toml> print the groups the agent is in, with their ids, for
//!                                     `full_activity`; with the agent stopped (D-136)
//! marmot-agent key <hex or npub>      print a public key in hex, or fail (for the install scripts)
//! marmot-agent import <old home> [--harness <name>] [--owner <key>] [--cwd <folder>]
//!                     [--settings <file>] [--leave-running <service>]... [--dry-run]
//!                                     take over one of today's connector installs (D-089)
//! ```

use marmot_account::AccountHome;
use marmot_agent::PubKey;
use marmot_agent::config::Config;
use marmot_agent::import::{self, Options, Places};
use marmot_agent::marmot::{self, Marmot};
use marmot_agent::run::{Core, Store, serve};
use marmot_agent::socket::Endpoint;
use marmot_agent_acp::Acp;
use marmot_agent_codex::Codex;
use marmot_agent_pi::Pi;
use marmot_app::AccountSetupRequest;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(command), Some(path)) = (args.get(1), args.get(2)) else {
        return usage();
    };
    if command == "key" {
        return match marmot_agent::keys::public_key_hex(path) {
            Some(hex) => {
                println!("{hex}");
                ExitCode::SUCCESS
            }
            None => fail("not a public key in hex or as an npub"),
        };
    }
    if command == "import" {
        return match import(path, &args[3..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(why) => fail(&why),
        };
    }
    let config = match Config::read(Path::new(path)) {
        Ok(config) => config,
        Err(error) => return fail(&error.to_string()),
    };
    // MDK's unoptimised OpenMLS code needs more stack than Rust's default.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(4 * 1024 * 1024)
        .build();
    let Ok(runtime) = runtime else {
        return fail("the async runtime could not start");
    };
    let outcome = match command.as_str() {
        "init" => runtime.block_on(init(&config)),
        "run" => runtime.block_on(run(&config)),
        "groups" => runtime.block_on(groups(&config)),
        _ => return usage(),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => fail(&why),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: marmot-agent init|run|groups <settings.toml>, or marmot-agent key <hex or npub>"
    );
    eprintln!(
        "       marmot-agent import <old home> [--harness <name>] [--owner <key>] \
         [--cwd <folder>] [--settings <file>] [--leave-running <service>]... [--dry-run]"
    );
    ExitCode::from(2)
}

fn fail(why: &str) -> ExitCode {
    eprintln!("marmot-agent: {why}");
    ExitCode::FAILURE
}

/// Makes the agent's identity in its home: keys, relay lists and a key package, so it can be
/// invited.
async fn init(config: &Config) -> Result<(), String> {
    // Read from MDK's account records alone, before the store is opened: a newer core moves a store
    // forward as it opens it, out of reach of the older core an upgrade may put back (D-159).
    if AccountHome::open(&config.home)
        .accounts()
        .is_ok_and(|a| a.iter().any(|a| a.local_signing))
    {
        return Err("the home already has an identity".into());
    }
    let (_, _, runtime) = marmot::open(config)?;
    let relays: Vec<cgka_traits::TransportEndpoint> = config
        .relays
        .iter()
        .map(|r| cgka_traits::TransportEndpoint(r.clone()))
        .collect();
    let setup = AccountSetupRequest {
        default_relays: relays.clone(),
        bootstrap_relays: relays,
        publish_initial_key_package: true,
        ..AccountSetupRequest::default()
    };
    let made = runtime
        .create_identity(setup)
        .await
        .map_err(|_| "the identity could not be made")?;
    runtime.shutdown().await;
    // The agent's own public key, for its owner to invite it.
    println!("{}", made.account.account_id_hex);
    Ok(())
}

/// Prints each group the agent is in, for the owner to choose the ids `full_activity` lists
/// (D-136): to the terminal only, as a group's id is never logged (D-071). It opens the home
/// without starting MDK, so nothing is fetched or sent, and only while the agent is stopped.
async fn groups(config: &Config) -> Result<(), String> {
    let (home, app, runtime) = marmot::open(config)?;
    let account = marmot::identity(&home)?;
    let marmot = Marmot {
        runtime: runtime.clone(),
        app,
        label: account.label,
        agent: PubKey(account.account_id_hex),
        faults: Default::default(),
    };
    let listed = marmot.listed(&config.owner, &config.full_activity);
    runtime.shutdown().await;
    let listed = listed.map_err(|_| "the agent's groups could not be read")?;
    print!("{}", marmot_agent::groups::table(&listed));
    Ok(())
}

async fn run(config: &Config) -> Result<(), String> {
    // Listen for SIGTERM first, so a service manager's stop during start-up is heard too, and
    // ends start-up: a harness that is slow to start is not waited for.
    let mut stop = Box::pin(stopped());
    let (home, app, runtime) = marmot::open(config)?;
    let account = marmot::identity(&home)?;
    // Subscribe before starting, so what MDK delivers while it starts is not missed, and before
    // the harness starts, so what arrives while it starts is not taken as history.
    let live = marmot::subscribe(&runtime);
    let Some(started) = unless_stopped(&mut stop, runtime.start()).await else {
        runtime.shutdown().await;
        return Ok(());
    };
    started.map_err(|_| "MDK could not start the account")?;
    let marmot = Marmot {
        runtime: runtime.clone(),
        app,
        label: account.label,
        agent: PubKey(account.account_id_hex),
        faults: Default::default(),
    };
    // A harness stopped while it starts is killed: each driver's child is killed when dropped.
    let ran = match (&config.codex, &config.acp, config.pi_setup()) {
        (Some(codex), _, _) => {
            let setup = config.setup().map_err(|e| e.to_string())?;
            match unless_stopped(&mut stop, Codex::start(&codex.program, setup)).await {
                Some(driver) => {
                    let driver = driver.map_err(|_| "Codex could not be started")?;
                    let core = Core::new(marmot, driver, config, Store::new(&config.home))?;
                    ready();
                    core.run(live, stop).await
                }
                None => Ok(()),
            }
        }
        (None, Some(acp), _) => {
            let starting = async {
                let mut driver = Acp::start(&acp.command, &acp.cwd, &acp.name).await?;
                // Unless the settings switch it off: the server listens on the loopback address (D-110).
                if acp.send_files {
                    driver.serve_files().await?;
                }
                Ok::<_, std::io::Error>(driver)
            };
            match unless_stopped(&mut stop, starting).await {
                Some(driver) => {
                    let driver = driver.map_err(|_| "the ACP agent could not be started")?;
                    let core = Core::new(marmot, driver, config, Store::new(&config.home))?;
                    ready();
                    core.run(live, stop).await
                }
                None => Ok(()),
            }
        }
        (None, None, Some(setup)) => match unless_stopped(&mut stop, Pi::start(setup)).await {
            Some(driver) => {
                let driver = driver.map_err(|_| "Pi could not be started")?;
                let core = Core::new(marmot, driver, config, Store::new(&config.home))?;
                ready();
                core.run(live, stop).await
            }
            None => Ok(()),
        },
        (None, None, None) => {
            let since = live.since;
            drop(live);
            let endpoint = Endpoint::in_home(&config.home);
            let (listener, token) = endpoint
                .listen()
                .map_err(|_| "the socket could not be opened")?;
            // A connector can connect from now on: the core is ready, whether or not one has.
            ready();
            serve(marmot, config, &config.home, listener, token, since, stop).await
        }
    };
    runtime.shutdown().await;
    ran
}

/// Tells the service manager, if there is one, that the store is open and the driver started
/// (D-178). A core that cannot say so runs on: systemd then counts its start as failed once it has
/// waited, and the installer puts back what it changed.
fn ready() {
    if let Err(why) = marmot_agent::ready::tell() {
        eprintln!("marmot-agent: {why}");
    }
}

/// What `work` gives, or nothing when the core is asked to stop first; `work` is then dropped.
async fn unless_stopped<T>(
    stop: &mut (impl std::future::Future<Output = ()> + Unpin),
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        done = work => Some(done),
        () = stop => None,
    }
}

/// Ends when the core is asked to stop: Ctrl-C in a terminal, or SIGTERM, which is how systemd
/// and launchd stop a service. Both are listened for from the call, not from the first poll.
fn stopped() -> impl std::future::Future<Output = ()> {
    use tokio::signal::unix::{SignalKind, signal};
    let term = signal(SignalKind::terminate()).ok();
    let interrupt = signal(SignalKind::interrupt()).ok();
    async move {
        let next = |s: Option<tokio::signal::unix::Signal>| async move {
            match s {
                Some(mut s) => {
                    s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = next(term) => {}
            _ = next(interrupt) => {}
        }
    }
}

/// Takes over one of today's connector installs: plans, prints the plan, and carries it out
/// unless `--dry-run` is given.
fn import(home: &str, rest: &[String]) -> Result<(), String> {
    let mut options = Options {
        home: std::path::absolute(home).map_err(|_| "the old home's path could not be read")?,
        harness: None,
        owner: None,
        cwd: None,
        settings: None,
        leave_running: Vec::new(),
        places: Places::of_this_user()?,
        now: std::time::SystemTime::now(),
    };
    let mut dry_run = false;
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        // An option where a value belongs is a mistake, never the value: `--settings --dry-run`
        // must not write the settings to `--dry-run` and carry the import out.
        let mut value = || match rest.next() {
            Some(value) if !value.starts_with('-') => Ok(value.clone()),
            _ => Err(format!("`{flag}` needs a value; nothing was changed")),
        };
        match flag.as_str() {
            "--harness" => options.harness = Some(value()?),
            "--owner" => options.owner = Some(value()?),
            "--cwd" => options.cwd = Some(value()?),
            "--settings" => {
                // Whole, so the commands printed afterwards work from any folder.
                let settings = std::path::absolute(value()?)
                    .map_err(|_| "the settings' path could not be read")?;
                options.settings = Some(settings);
            }
            "--leave-running" => options.leave_running.push(value()?),
            "--dry-run" => dry_run = true,
            _ => return Err(format!("`{flag}` is not an option of import")),
        }
    }
    let mut services = import::services::System::default();
    let plan = import::plan(&options, &mut services)?;
    let mut out = std::io::stdout();
    plan.print(&mut out)
        .map_err(|_| "the plan could not be printed")?;
    if dry_run {
        println!("\nDry run: nothing was changed.");
        return Ok(());
    }
    println!();
    import::carry_out(&options, &plan, &mut services, &mut out)
}
