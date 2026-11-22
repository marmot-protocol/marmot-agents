//! Failure capsules, replay and shrinking, as MDK's simulator keeps them: a run that fails
//! leaves a file with its schedule, its trace and the property that failed; replaying the file
//! runs the same schedule and must give the same trace; the shrinker cuts the schedule down to
//! a short one that still fails the same property.

use crate::schedule::{Action, Schedule};
use crate::sim::{self, Failed, Report};
use crate::world::Trace;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a failed run leaves.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capsule {
    /// The capsule format's version.
    pub format: u32,
    /// The schedule, cut down when the shrinker ran.
    pub schedule: Schedule,
    /// The property that failed, and when.
    pub failed: Failed,
    /// What happened, step by step.
    pub trace: Vec<Trace>,
    /// The schedule as it was generated, before shrinking, when it was shrunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<Schedule>,
}

/// The capsule format's version.
pub const FORMAT: u32 = 1;

impl Capsule {
    /// The capsule for a failed run.
    pub fn new(schedule: Schedule, report: &Report) -> Option<Self> {
        Some(Self {
            format: FORMAT,
            schedule,
            failed: report.failure.clone()?,
            trace: report.trace.clone(),
            original: None,
        })
    }

    /// Writes the capsule to `dir`, and gives the file's path.
    pub fn write(&self, dir: &Path) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.capsule.json", self.schedule.name.file_stem()));
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, json)?;
        Ok(path)
    }

    /// Reads a capsule.
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read the capsule: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("not a capsule: {e}"))
    }

    /// Runs the capsule's schedule again. `Ok` when it fails the same property at the same step
    /// with the same trace; otherwise what differs.
    pub fn replay(&self) -> Result<Report, String> {
        let report = sim::run(&self.schedule);
        if report.failure.as_ref() != Some(&self.failed) {
            return Err(format!(
                "the replay ended differently: {:?}, not {:?}",
                report.failure, self.failed
            ));
        }
        if let Some(at) = (0..report.trace.len().max(self.trace.len()))
            .find(|&i| report.trace.get(i) != self.trace.get(i))
        {
            return Err(format!(
                "the replay's trace differs at entry {at}: {:?}, not {:?}",
                report.trace.get(at),
                self.trace.get(at)
            ));
        }
        Ok(report)
    }
}

/// Writes a schedule to `dir` before it runs, so a run that hangs or panics leaves it behind.
pub fn write_schedule(schedule: &Schedule, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.schedule.json", schedule.name.file_stem()));
    let json = serde_json::to_vec_pretty(schedule).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Cuts a failing schedule down, greedily: it drops runs of steps, halving their length down to
/// one, and keeps each cut after which the run still fails the same property; then it shortens
/// the clock's steps; then it drops each message no step names any more, numbering the rest
/// again, when the run still fails without it (the fair part at the end posts every message, so
/// one no step names may still matter). At most `budget` runs. Gives the shortest failing
/// schedule found, with its report.
pub fn shrink(schedule: &Schedule, report: &Report, budget: usize) -> (Schedule, Report) {
    let Some(failed) = &report.failure else {
        return (schedule.clone(), report.clone());
    };
    let property = failed.failure.property.clone();
    let mut best = schedule.clone();
    let mut best_report = report.clone();
    let mut runs = 0;
    let still_fails = |candidate: &Schedule, runs: &mut usize| -> Option<Report> {
        *runs += 1;
        let report = sim::run(candidate);
        let same = report
            .failure
            .as_ref()
            .is_some_and(|f| f.failure.property == property);
        same.then_some(report)
    };
    // Steps after the failing one never ran.
    if let Some(step) = failed.step {
        best.steps.truncate(step + 1);
    }
    let mut chunk = best.steps.len().div_ceil(2).max(1);
    while runs < budget {
        let mut cut_any = false;
        let mut at = 0;
        while at < best.steps.len() && runs < budget {
            let mut candidate = best.clone();
            let end = (at + chunk).min(candidate.steps.len());
            candidate.steps.drain(at..end);
            match still_fails(&candidate, &mut runs) {
                Some(report) => {
                    best = candidate;
                    best_report = report;
                    cut_any = true;
                }
                None => at += chunk,
            }
        }
        if chunk == 1 && !cut_any {
            break;
        }
        chunk = (chunk / 2).max(1);
    }
    // Make the remaining clock steps as short as still fails.
    for at in 0..best.steps.len() {
        if runs >= budget {
            break;
        }
        if let Action::Tick { ms } = best.steps[at] {
            for shorter in [1, ms / 10, ms / 2] {
                if shorter == 0 || shorter >= ms || runs >= budget {
                    continue;
                }
                let mut candidate = best.clone();
                candidate.steps[at] = Action::Tick { ms: shorter };
                if let Some(report) = still_fails(&candidate, &mut runs) {
                    best = candidate;
                    best_report = report;
                    break;
                }
            }
        }
    }
    // Drop the messages no step names, the newest first.
    for msg in (0..best.messages.len()).rev() {
        if runs >= budget {
            break;
        }
        if let Some(candidate) = without_message(&best, msg)
            && let Some(report) = still_fails(&candidate, &mut runs)
        {
            best = candidate;
            best_report = report;
        }
    }
    (best, best_report)
}

/// The message a step names.
fn named(action: &Action) -> Option<usize> {
    match action {
        Action::Store { msg }
        | Action::Hear { msg }
        | Action::Ask { msg }
        | Action::Reannounce { msg } => Some(*msg),
        _ => None,
    }
}

/// The schedule without message `msg`, the later ones numbered one less; `None` when a step names
/// it.
fn without_message(schedule: &Schedule, msg: usize) -> Option<Schedule> {
    if schedule.steps.iter().any(|a| named(a) == Some(msg)) {
        return None;
    }
    let mut out = schedule.clone();
    out.messages.remove(msg);
    for step in &mut out.steps {
        if let Action::Store { msg: m }
        | Action::Hear { msg: m }
        | Action::Ask { msg: m }
        | Action::Reannounce { msg: m } = step
            && *m > msg
        {
            *m -= 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{Posted, What};

    #[test]
    fn a_message_no_step_names_is_dropped_and_the_rest_numbered_again() {
        let posted = |chat| Posted {
            chat,
            what: What::Prompt { approval: false },
        };
        let mut schedule = crate::generate(&crate::RunName::new(crate::Family::Calm, 0, 0));
        schedule.messages = vec![posted(0), posted(0), posted(0)];
        schedule.steps = vec![Action::Store { msg: 0 }, Action::Hear { msg: 2 }];
        assert_eq!(without_message(&schedule, 0), None);
        let cut = without_message(&schedule, 1).expect("no step names m1");
        assert_eq!(cut.messages.len(), 2);
        assert_eq!(
            cut.steps,
            vec![Action::Store { msg: 0 }, Action::Hear { msg: 1 }]
        );
    }
}
