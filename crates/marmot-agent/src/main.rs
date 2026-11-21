//! `marmot-agent`: one agent's member of Marmot groups, driving Codex or Pi built in, or a
//! connector on the socket (D-072).
//!
//! ```text
//! marmot-agent init <settings.toml>   make the agent's identity, and print its public key
//! marmot-agent run <settings.toml>    run until stopped
//! ```

use marmot_account::AccountHome;
use marmot_agent::PubKey;
use marmot_agent::config::Config;
use marmot_agent::marmot::Marmot;
use marmot_agent::run::{Core, Store, serve};
use marmot_agent::socket::Endpoint;
use marmot_agent_acp::Acp;
use marmot_agent_codex::Codex;
use marmot_agent_pi::Pi;
use marmot_app::{AccountSetupRequest, MarmotApp, MarmotAppConfig, MarmotAppRuntime};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(command), Some(path)) = (args.get(1), args.get(2)) else {
        return usage();
    };
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
        _ => return usage(),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => fail(&why),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: marmot-agent init|run <settings.toml>");
    ExitCode::from(2)
}

fn fail(why: &str) -> ExitCode {
    eprintln!("marmot-agent: {why}");
    ExitCode::FAILURE
}

fn open(config: &Config) -> Result<(AccountHome, MarmotApp, MarmotAppRuntime), String> {
    let home = AccountHome::open(&config.home);
    let base = if config.white_noise {
        MarmotAppConfig::white_noise()
    } else {
        MarmotAppConfig::default()
    };
    let settings = base.with_allow_loopback_relay_endpoints(config.allow_loopback_relays);
    let app = MarmotApp::try_with_relays_and_account_home_and_config(
        &config.home,
        config.relays.clone(),
        home.clone(),
        settings,
    )
    .map_err(|_| "the account home could not be opened; is another core running on it?")?;
    let runtime = MarmotAppRuntime::new(app.clone());
    Ok((home, app, runtime))
}

/// Makes the agent's identity in its home: keys, relay lists and a key package, so it can be
/// invited.
async fn init(config: &Config) -> Result<(), String> {
    let (home, _, runtime) = open(config)?;
    if home
        .accounts()
        .is_ok_and(|a| a.iter().any(|a| a.local_signing))
    {
        return Err("the home already has an identity".into());
    }
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

async fn run(config: &Config) -> Result<(), String> {
    let (home, app, runtime) = open(config)?;
    let accounts = home
        .accounts()
        .map_err(|_| "the account home could not be read")?;
    let mut signing = accounts.into_iter().filter(|a| a.local_signing);
    let (Some(account), None) = (signing.next(), signing.next()) else {
        return Err(
            "the home must hold exactly one identity; run `marmot-agent init` first".into(),
        );
    };
    // Subscribe before starting, so what MDK delivers while it starts is not missed.
    let events = runtime.subscribe();
    runtime
        .start()
        .await
        .map_err(|_| "MDK could not start the account")?;
    let marmot = Marmot {
        runtime: runtime.clone(),
        app,
        label: account.label,
        agent: PubKey(account.account_id_hex),
        media_server: None,
    };
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    match (&config.codex, &config.acp, config.pi_setup()) {
        (Some(codex), _, _) => {
            let setup = config.setup().map_err(|e| e.to_string())?;
            let driver = Codex::start(&codex.program, setup)
                .await
                .map_err(|_| "Codex could not be started")?;
            let core = Core::new(marmot, driver, config, Store::new(&config.home))?;
            core.run(events, stop).await;
        }
        (None, Some(acp), _) => {
            let driver = Acp::start(&acp.command, &acp.cwd, &acp.name)
                .await
                .map_err(|_| "the ACP agent could not be started")?;
            let core = Core::new(marmot, driver, config, Store::new(&config.home))?;
            core.run(events, stop).await;
        }
        (None, None, Some(setup)) => {
            let driver = Pi::start(setup)
                .await
                .map_err(|_| "Pi could not be started")?;
            let core = Core::new(marmot, driver, config, Store::new(&config.home))?;
            core.run(events, stop).await;
        }
        (None, None, None) => {
            drop(events);
            let endpoint = Endpoint::in_home(&config.home);
            let (listener, token) = endpoint
                .listen()
                .map_err(|_| "the socket could not be opened")?;
            serve(marmot, config, &config.home, listener, token, stop).await?;
        }
    }
    runtime.shutdown().await;
    Ok(())
}
