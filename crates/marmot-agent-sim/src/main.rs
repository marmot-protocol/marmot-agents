//! `marmot-agent-sim`: runs simulator campaigns, replays a failure capsule, and runs or shows a
//! schedule. See the crate's README.

use marmot_agent_sim::{BugSet, Capsule, Family, RunName, Schedule, campaign, generate, run};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "\
usage:
  marmot-agent-sim run [--family NAME|gate|all] [--seed N] [--from I] [--runs N] [--out DIR]
                       [--shrink RUNS] [--bug NAME]...
  marmot-agent-sim replay CAPSULE
  marmot-agent-sim schedule FILE
  marmot-agent-sim show FAMILY SEED INDEX

families: calm crash sends live driver mixed (the gate), disorder late
bugs (with the `bugs` feature): save_after_send retry_ambiguous_with_new_token settles_only_from_items";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run_campaigns(&args[1..]),
        Some("replay") => replay(&args[1..]),
        Some("schedule") => run_file(&args[1..]),
        Some("show") => show(&args[1..]),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(2)
        }
    }
}

fn number<T: std::str::FromStr>(value: Option<&String>, what: &str) -> Result<T, String> {
    value
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("{what} needs a number\n{USAGE}"))
}

fn run_campaigns(args: &[String]) -> Result<bool, String> {
    let mut families: Vec<Family> = Family::GATE.to_vec();
    let (mut seed, mut from, mut runs, mut shrink) = (0_u64, 0_u32, 100_u32, 200_usize);
    let mut out: Option<PathBuf> = None;
    let mut bugs = BugSet::default();
    let mut at = 0;
    while at < args.len() {
        let value = args.get(at + 1);
        match args[at].as_str() {
            "--family" => {
                let name = value.ok_or(USAGE)?;
                families = match name.as_str() {
                    "gate" => Family::GATE.to_vec(),
                    "all" => Family::ALL.to_vec(),
                    name => vec![Family::named(name).ok_or_else(|| format!("no family {name}"))?],
                };
            }
            "--seed" => seed = number(value, "--seed")?,
            "--from" => from = number(value, "--from")?,
            "--runs" => runs = number(value, "--runs")?,
            "--shrink" => shrink = number(value, "--shrink")?,
            "--out" => out = Some(PathBuf::from(value.ok_or(USAGE)?)),
            "--bug" => match value.map(String::as_str) {
                Some("save_after_send") => bugs.save_after_send = true,
                Some("retry_ambiguous_with_new_token") => {
                    bugs.retry_ambiguous_with_new_token = true
                }
                Some("settles_only_from_items") => bugs.settles_only_from_items = true,
                _ => return Err(format!("no such bug\n{USAGE}")),
            },
            _ => return Err(USAGE.into()),
        }
        at += 2;
    }
    let mut passed = true;
    for family in families {
        let started = Instant::now();
        let done = campaign(family, seed, from, runs, bugs, out.as_deref(), shrink)?;
        println!(
            "{family}: {} runs, {} steps, {} virtual s, {} failed, {:.1} s",
            done.runs,
            done.steps,
            done.virtual_ms / 1000,
            done.failures.len(),
            started.elapsed().as_secs_f64()
        );
        let seen: Vec<String> = (done.seen.iter())
            .map(|(what, n)| format!("{what} {n}"))
            .collect();
        println!("  seen: {}", seen.join(", "));
        for (name, failed, path) in &done.failures {
            passed = false;
            println!(
                "  {name}: {} {} (step {:?}){}",
                failed.failure.property,
                failed.failure.detail,
                failed.step,
                path.as_ref()
                    .map(|p| format!(", capsule {}", p.display()))
                    .unwrap_or_default()
            );
        }
        for unwritten in &done.unwritten {
            passed = false;
            println!("  {unwritten}");
        }
    }
    Ok(passed)
}

fn replay(args: &[String]) -> Result<bool, String> {
    let path = PathBuf::from(args.first().ok_or(USAGE)?);
    let capsule = Capsule::read(&path)?;
    match capsule.replay() {
        Ok(report) => {
            let failed = report.failure.expect("a capsule fails");
            println!(
                "replayed exactly: {} {} (step {:?}), {} steps in the schedule",
                failed.failure.property,
                failed.failure.detail,
                failed.step,
                capsule.schedule.steps.len()
            );
            for entry in &report.trace {
                println!(
                    "{:>9} {:>5} {}",
                    entry.ms,
                    entry.step.map_or("-".into(), |s| s.to_string()),
                    entry.what
                );
            }
            Ok(true)
        }
        Err(why) => {
            println!("{why}");
            Ok(false)
        }
    }
}

fn run_file(args: &[String]) -> Result<bool, String> {
    let path = PathBuf::from(args.first().ok_or(USAGE)?);
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read the schedule: {e}"))?;
    let schedule: Schedule =
        serde_json::from_slice(&bytes).map_err(|e| format!("not a schedule: {e}"))?;
    let report = run(&schedule);
    for entry in &report.trace {
        println!(
            "{:>9} {:>5} {}",
            entry.ms,
            entry.step.map_or("-".into(), |s| s.to_string()),
            entry.what
        );
    }
    match &report.failure {
        Some(failed) => {
            println!(
                "failed: {} {} (step {:?})",
                failed.failure.property, failed.failure.detail, failed.step
            );
            Ok(false)
        }
        None => {
            println!("passed");
            Ok(true)
        }
    }
}

fn show(args: &[String]) -> Result<bool, String> {
    let family = args
        .first()
        .and_then(|f| Family::named(f))
        .ok_or_else(|| USAGE.to_string())?;
    let seed = number(args.get(1), "SEED")?;
    let index = number(args.get(2), "INDEX")?;
    let schedule = generate(&RunName::new(family, seed, index));
    println!(
        "{}",
        serde_json::to_string_pretty(&schedule).map_err(|e| e.to_string())?
    );
    Ok(true)
}
