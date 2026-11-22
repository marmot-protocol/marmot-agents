//! Running a schedule: the real core, started and killed and started again, with the simulated
//! MDK and the scripted driver, all in one thread on tokio's paused clock. Nothing waits on the
//! real clock or the network, and the core's loop takes what is ready in turn, so the same
//! schedule always gives the same trace.

use crate::driver::SimDriver;
use crate::oracle::{self, Failure};
use crate::schedule::{Action, Schedule};
use crate::side::{Shared, SimEvents, SimSide, lock};
use crate::world::{Move, Trace, World};
use marmot_agent::config::Config;
use marmot_agent::run::{Core, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

/// How long, on the virtual clock, the fair part at the end may take to settle everything.
pub const LIVENESS_BOUND: Duration = Duration::from_secs(900);

/// How a run went.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    /// The property that failed, and at which step; `None` when all held.
    pub failure: Option<Failed>,
    /// What happened.
    pub trace: Vec<Trace>,
    /// Counts, for the campaign's summary.
    pub stats: Stats,
}

/// A property that failed, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failed {
    /// The property.
    #[serde(flatten)]
    pub failure: Failure,
    /// The step after which it was found; `None` in the fair part at the end.
    pub step: Option<usize>,
}

/// What a run did.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    /// Steps run.
    pub steps: usize,
    /// Times the core went down: crashes, failed saves, stops, a driver that ended.
    pub downs: u32,
    /// Times it started.
    pub starts: u32,
    /// Events the agent sent that MDK stored.
    pub sent: usize,
    /// Requests and prompts handed to the harness.
    pub delivered: usize,
    /// Milliseconds on the virtual clock.
    pub virtual_ms: u64,
}

/// A core that is running.
struct Running {
    task: JoinHandle<Result<(), String>>,
    stop: Option<oneshot::Sender<()>>,
}

/// One run.
struct Sim<'a> {
    schedule: &'a Schedule,
    world: Shared,
    notify: Arc<Notify>,
    dir: tempfile::TempDir,
    config: Config,
    core: Option<Running>,
    stats: Stats,
}

/// Runs a schedule from the start, on a fresh runtime with a paused clock.
pub fn run(schedule: &Schedule) -> Report {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("a runtime");
    runtime.block_on(async { Sim::new(schedule).run().await })
}

/// A folder for the core's state file: in memory where the system has one, since every save is
/// flushed to disk.
fn state_dir() -> tempfile::TempDir {
    let shm = std::path::Path::new("/dev/shm");
    if shm.is_dir()
        && let Ok(dir) = tempfile::Builder::new()
            .prefix("marmot-agent-sim")
            .tempdir_in(shm)
    {
        return dir;
    }
    tempfile::Builder::new()
        .prefix("marmot-agent-sim")
        .tempdir()
        .expect("a temporary folder")
}

impl<'a> Sim<'a> {
    fn new(schedule: &'a Schedule) -> Self {
        let world = World::new(
            schedule.chats,
            schedule.messages.clone(),
            schedule.name.family.within_contract(),
        );
        let settings = format!(
            "home = \"/unused\"\nrelays = [\"wss://relay.invalid\"]\nowner = \"{}\"\n[connector]\n",
            world.owner.as_str()
        );
        Self {
            schedule,
            world: Arc::new(Mutex::new(world)),
            notify: Arc::new(Notify::new()),
            dir: state_dir(),
            config: Config::parse(&settings).expect("the simulator's settings"),
            core: None,
            stats: Stats::default(),
        }
    }

    async fn run(mut self) -> Report {
        self.start();
        self.settle().await;
        for (at, action) in self.schedule.steps.iter().enumerate() {
            lock(&self.world).step = Some(at);
            self.apply(*action).await;
            self.settle().await;
            self.stats.steps = at + 1;
            let found = oracle::safety(&lock(&self.world));
            if let Some(failure) = found {
                return self.report(Some(Failed {
                    failure,
                    step: Some(at),
                }));
            }
        }
        lock(&self.world).step = None;
        let failure = self.fair().await;
        self.report(failure.map(|failure| Failed {
            failure,
            step: None,
        }))
    }

    fn report(self, failure: Option<Failed>) -> Report {
        let world = lock(&self.world);
        let mut stats = self.stats;
        stats.sent = world.rows.iter().filter(|r| r.from_agent).count();
        stats.delivered = world.harness.deliveries.len();
        stats.virtual_ms = world.ms();
        Report {
            failure,
            trace: world.trace.clone(),
            stats,
        }
    }

    /// Starts the core from its saved state, with a new driver (`Restart`).
    fn start(&mut self) {
        let (life, events) = {
            let mut world = lock(&self.world);
            world.live.clear();
            world.listening = true;
            let (life, events) = world.new_life();
            world.note(format!("Restart(life {life})"));
            (life, events)
        };
        self.stats.starts += 1;
        let agent = lock(&self.world).agent.clone();
        let side = SimSide {
            world: self.world.clone(),
            notify: self.notify.clone(),
            agent,
        };
        let driver = SimDriver {
            world: self.world.clone(),
            life,
            events,
        };
        let saves = self.world.clone();
        let (store, _) = Store::failing_when(self.dir.path(), move |_| {
            let mut world = lock(&saves);
            if world.save_fails {
                world.save_fails = false;
                world.note("SaveFails");
                return true;
            }
            if world.effect("Save") {
                return true;
            }
            world.note("Save");
            false
        });
        let core = Core::new(side, driver, &self.config, store).expect("the core starts");
        #[cfg(feature = "bugs")]
        let core = {
            let bugs = self.schedule.bugs;
            core.with_bugs(marmot_agent::run::Bugs {
                save_after_send: bugs.save_after_send,
                retry_ambiguous_blindly: bugs.retry_ambiguous_blindly,
                settles_only_from_items: bugs.settles_only_from_items,
            })
        };
        #[cfg(not(feature = "bugs"))]
        assert!(
            !self.schedule.bugs.any(),
            "this schedule puts bugs back: build with the `bugs` feature"
        );
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(core.run(SimEvents, async {
            let _ = stopped.await;
        }));
        self.core = Some(Running {
            task,
            stop: Some(stop),
        });
        self.notify.notify_one();
    }

    /// Lets the core take what is ready, until it waits for something new: the clock moves on
    /// by a millisecond only once nothing in the runtime can go on. A core that reached a crash
    /// is killed, and one that ended is noted down.
    async fn settle(&mut self) {
        if !lock(&self.world).live.is_empty() {
            self.notify.notify_one();
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
        let crashed = std::mem::take(&mut lock(&self.world).crashed);
        if crashed {
            self.kill("crashed").await;
            return;
        }
        if self.core.as_ref().is_some_and(|c| c.task.is_finished()) {
            let running = self.core.take().expect("running");
            let ended = running.task.await;
            let how = match ended {
                Ok(Ok(())) => "Down(stopped)",
                Ok(Err(_)) => "Down(could not save)",
                Err(_) => "Down(panicked)",
            };
            self.down(how);
        }
    }

    /// Kills the core at once: nothing more is saved or sent (`Crash`).
    async fn kill(&mut self, why: &str) {
        if let Some(running) = self.core.take() {
            running.task.abort();
            let _ = running.task.await;
            self.down(&format!("Down({why})"));
        }
    }

    fn down(&mut self, how: &str) {
        self.stats.downs += 1;
        let mut world = lock(&self.world);
        world.listening = false;
        world.live.clear();
        world.harness.out = None;
        world.note(how);
    }

    async fn apply(&mut self, action: Action) {
        let up = self.core.is_some();
        match action {
            Action::Invite { chat } => lock(&self.world).invite(usize::from(chat)),
            Action::Store { msg } => lock(&self.world).store(msg),
            Action::Hear { msg } | Action::Ask { msg } => lock(&self.world).hear(msg),
            Action::Lag => lock(&self.world).lag(),
            Action::Emit { pick, force } => lock(&self.world).emit(pick, force),
            Action::Refuse => lock(&self.world).harness.moves.push_back(Move::Refuse),
            Action::Slow { ms } => lock(&self.world).harness.moves.push_back(Move::Slow(ms)),
            Action::Hang => lock(&self.world).harness.moves.push_back(Move::Hang),
            Action::Ignore => lock(&self.world).harness.moves.push_back(Move::Ignore),
            Action::DriverStops => lock(&self.world).driver_stops(),
            Action::Send { outcome } => lock(&self.world).outcomes.push_back(outcome),
            Action::Lookup => lock(&self.world).lookup_errors += 1,
            Action::SaveFails => lock(&self.world).save_fails = true,
            Action::Crash { after: 0 } => {
                if up {
                    lock(&self.world).note("Crash");
                    self.kill("crashed").await;
                }
            }
            Action::Crash { after } => lock(&self.world).crash_after = Some(after),
            Action::Tick { ms } => {
                lock(&self.world).note(format!("Tick({ms} ms)"));
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            Action::Restart => {
                if !up {
                    self.start();
                }
            }
            Action::Stop => {
                if let Some(stop) = self.core.as_mut().and_then(|c| c.stop.take()) {
                    lock(&self.world).note("Stop");
                    let _ = stop.send(());
                }
            }
        }
    }

    /// The fair part: faults stop, everything posted is stored and heard, the harness finishes
    /// its work, and time passes, until every property of liveness holds or the bound passes.
    /// Then the core stops cleanly, and nothing may be left open.
    async fn fair(&mut self) -> Option<Failure> {
        {
            let mut world = lock(&self.world);
            world.note("Fair");
            world.outcomes.clear();
            world.lookup_errors = 0;
            world.save_fails = false;
            world.crash_after = None;
            world.harness.moves.clear();
        }
        self.settle().await;
        // A harness that never answers breaks the fairness the end assumes: it is restarted
        // once, as a person would.
        if lock(&self.world).harness.hung {
            lock(&self.world).note("Crash(the harness hangs)");
            self.kill("crashed").await;
        }
        if self.core.is_none() {
            self.start();
            self.settle().await;
        }
        let mut waited = Duration::ZERO;
        let posted = self.schedule.messages.len();
        loop {
            {
                let mut world = lock(&self.world);
                for chat in 0..world.chats.len() {
                    world.invite(chat);
                }
                for msg in 0..posted {
                    world.store(msg);
                }
                // In storage order, so the fair part adds no disorder of its own.
                let mut stored: Vec<usize> = (0..posted).collect();
                stored.sort_by_key(|&m| world.msgs[m].as_ref().and_then(|m| m.stored));
                for msg in stored {
                    world.hear(msg);
                }
            }
            for _ in 0..64 {
                if !lock(&self.world).has_work() {
                    break;
                }
                lock(&self.world).emit(0, true);
                self.settle().await;
            }
            if self.core.is_none() {
                self.start();
            }
            self.settle().await;
            if let Some(failure) = oracle::safety(&lock(&self.world)) {
                return Some(failure);
            }
            let left = oracle::liveness(&lock(&self.world));
            if left.is_empty() && !lock(&self.world).has_work() {
                break;
            }
            if waited >= LIVENESS_BOUND {
                let first = left.into_iter().next()?;
                return Some(Failure {
                    property: first.property,
                    detail: format!("{} within {} s", first.detail, LIVENESS_BOUND.as_secs()),
                });
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            waited += Duration::from_secs(5);
        }
        // The last stop: everything is closed and sent.
        if let Some(stop) = self.core.as_mut().and_then(|c| c.stop.take()) {
            lock(&self.world).note("Stop");
            let _ = stop.send(());
        }
        for _ in 0..10 {
            self.settle().await;
            if self.core.is_none() {
                break;
            }
        }
        let world = lock(&self.world);
        oracle::safety(&world).or_else(|| oracle::after_stop(&world))
    }
}
