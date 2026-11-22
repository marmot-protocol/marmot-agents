//! `marmot-agent-sim`: a deterministic simulator of the core (`docs/design/verification.md`,
//! layer 3).
//!
//! It runs the real core, [`marmot_agent::run::Core`], with a simulated MDK behind the core's
//! [`marmot_agent::marmot::Side`] and a scripted harness behind a driver, on tokio's paused
//! clock, from a seed. A run is named by its family, the family's version, a seed and an index
//! ([`RunName`]); the name makes a [`Schedule`] of steps named as the shared vocabulary names
//! them. The safety properties are checked after every step and the liveness ones at the end,
//! from what the simulated MDK and the harness saw ([`oracle`]). A failure leaves a
//! [`Capsule`] that replays exactly, and [`shrink`] cuts it down.
//!
//! - [`schedule`]: families, names, steps, and the generator.
//! - [`world`]: the simulated MDK, the people, the scripted harness and the armed faults.
//! - [`side`] and [`driver`]: the core's view of them.
//! - [`oracle`]: the properties.
//! - [`sim`]: running a schedule.
//! - [`capsule`]: failure capsules, replay and shrinking.

pub mod capsule;
pub mod driver;
pub mod oracle;
pub mod rng;
pub mod schedule;
pub mod side;
pub mod sim;
pub mod world;

pub use capsule::{Capsule, shrink, write_schedule};
pub use schedule::{
    Action, BugSet, Family, Faults, Outcome, Posted, ReadStep, RunName, Schedule, What, generate,
};
pub use sim::{Report, run};

use std::collections::BTreeMap;
use std::path::Path;

/// How a campaign went.
#[derive(Debug, Default)]
pub struct Campaign {
    /// Runs made.
    pub runs: u32,
    /// The runs that failed, each with its capsule's path when one was written.
    pub failures: Vec<(RunName, sim::Failed, Option<std::path::PathBuf>)>,
    /// Virtual milliseconds, over all runs.
    pub virtual_ms: u64,
    /// Steps, over all runs.
    pub steps: usize,
    /// How often each kind of event the trace names happened, over all runs ([`MARKS`]): to
    /// show a family's faults did happen, and its checks did not pass empty.
    pub seen: BTreeMap<&'static str, u32>,
    /// Capsules of failed runs that could not be written, each with the run and the reason.
    pub unwritten: Vec<String>,
}

/// The kinds of event a campaign counts, each with what marks it in a trace entry.
pub const MARKS: [(&str, &str); 25] = [
    ("harness takes", "), Take"),
    ("crash before a save", "Crash(before Save)"),
    ("crash before a send", "Crash(before Send)"),
    ("crash before a delivery", "Crash(before Deliver)"),
    ("crash before an accept", "Crash(before Accept)"),
    ("crash at once", "Crash"),
    ("crash just after a send went", "Sent, then Crash"),
    ("save fails", "SaveFails"),
    ("send busy", ": Busy"),
    ("send refused", ": Refused"),
    ("send ambiguous, not gone", "Ambiguous, not sent"),
    ("send ambiguous, gone", "Ambiguous, sent"),
    ("lookup found", "Lookup: Found"),
    ("lookup absent", "Lookup: Absent"),
    ("lookup error", "Lookup: Error"),
    ("place error", "Place: Error"),
    ("order error", "Order: Error"),
    ("stored error", "Stored: Error"),
    ("groups error", "Groups: Error"),
    ("lag", "Lag("),
    ("harness refuses", "), Refuse"),
    ("harness slow", " ms"),
    ("harness hangs", ", hangs"),
    ("driver stops", "DriverStops"),
    ("live event lost while down", "lost: no core"),
];

/// Counts what a trace shows, by [`MARKS`].
fn tally(seen: &mut BTreeMap<&'static str, u32>, trace: &[world::Trace]) {
    for entry in trace {
        for (name, mark) in MARKS {
            let found = match name {
                "crash at once" => entry.what == "Crash",
                "harness slow" => entry.what.starts_with("Deliver(") && entry.what.ends_with(" ms"),
                _ => entry.what.contains(mark),
            };
            if found {
                *seen.entry(name).or_default() += 1;
            }
        }
        if entry.what.starts_with("CatchUp(") && !entry.what.contains(", 0 stored") {
            *seen.entry("catch-up that reads something").or_default() += 1;
        }
    }
}

/// Runs `count` runs of `family`'s campaign `seed`, from index `first`, with `bugs` put back.
/// Each schedule is written to `out` before it runs, when given, and each failure leaves a
/// capsule there, shrunk within `shrink_budget` runs. An error, before anything runs, for a range
/// that goes past the last index; and, before that run, for a schedule that cannot be written,
/// since a run that then hung would leave nothing behind. A capsule that cannot be written is
/// noted in [`Campaign::unwritten`].
pub fn campaign(
    family: Family,
    seed: u64,
    first: u32,
    count: u32,
    bugs: BugSet,
    out: Option<&Path>,
    shrink_budget: usize,
) -> Result<Campaign, String> {
    let last = u64::from(first) + u64::from(count);
    if last > u64::from(u32::MAX) + 1 {
        return Err(format!(
            "runs {first} to {} go past the last index, {}",
            last - 1,
            u32::MAX
        ));
    }
    let mut campaign = Campaign::default();
    for index in (u64::from(first)..last).map(|i| u32::try_from(i).expect("checked above")) {
        let name = RunName::new(family, seed, index);
        let mut schedule = generate(&name);
        schedule.bugs = bugs;
        if let Some(out) = out {
            write_schedule(&schedule, out).map_err(|e| {
                format!(
                    "{name}: cannot write its schedule to {}: {e}",
                    out.display()
                )
            })?;
        }
        let report = run(&schedule);
        campaign.runs += 1;
        campaign.virtual_ms += report.stats.virtual_ms;
        campaign.steps += report.stats.steps;
        tally(&mut campaign.seen, &report.trace);
        if let Some(failed) = report.failure.clone() {
            let (small, small_report) = shrink(&schedule, &report, shrink_budget);
            let mut path = None;
            if let Some(out) = out
                && let Some(mut capsule) = Capsule::new(small, &small_report)
            {
                capsule.original = Some(schedule.clone());
                match capsule.write(out) {
                    Ok(written) => path = Some(written),
                    Err(e) => campaign.unwritten.push(format!(
                        "{name}: cannot write its capsule to {}: {e}",
                        out.display()
                    )),
                }
            }
            campaign.failures.push((name, failed, path));
        } else if let Some(out) = out {
            let _ = std::fs::remove_file(out.join(format!("{}.schedule.json", name.file_stem())));
        }
    }
    Ok(campaign)
}
