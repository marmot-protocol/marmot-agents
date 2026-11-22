//! Running a schedule: the real core, started and killed and started again, with the simulated
//! MDK and the scripted driver, all in one thread on tokio's paused clock. Nothing waits on the
//! real clock or the network, and the core's loop takes what is ready in turn, so the same
//! schedule always gives the same trace.

use crate::driver::SimDriver;
use crate::oracle::{self, Failure};

fn fail(property: &str, detail: &str) -> Failure {
    Failure {
        property: property.into(),
        detail: detail.into(),
    }
}
use crate::schedule::{Action, Schedule};
use crate::side::{Shared, SimEvents, SimSide, lock};
use crate::world::{Move, Trace, World};
use marmot_agent::config::Config;
use marmot_agent::marmot::Subscribed;
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
    /// How the core's task ended when it should not have: a panic, or an end no fault explains.
    broken: Option<Failure>,
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
        let mut world = world;
        world.faults = schedule.faults;
        Self {
            schedule,
            world: Arc::new(Mutex::new(world)),
            notify: Arc::new(Notify::new()),
            dir: state_dir(),
            config: Config::parse(&settings).expect("the simulator's settings"),
            core: None,
            stats: Stats::default(),
            broken: None,
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
            if let Some(failure) = self.check() {
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
            world.stopping = false;
            world.unsaved = false;
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
                world.unsaved = true;
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
                retry_ambiguous_with_new_token: bugs.retry_ambiguous_with_new_token,
                settles_only_from_items: bugs.settles_only_from_items,
            })
        };
        #[cfg(not(feature = "bugs"))]
        assert!(
            !self.schedule.bugs.any(),
            "this schedule puts bugs back: build with the `bugs` feature"
        );
        let (stop, stopped) = oneshot::channel::<()>();
        // Subscribed as it starts: the simulated harness starts at once.
        let since = lock(&self.world).secs();
        let live = Subscribed {
            events: SimEvents,
            since,
        };
        let task = tokio::spawn(core.run(live, async {
            let _ = stopped.await;
        }));
        self.core = Some(Running {
            task,
            stop: Some(stop),
        });
        self.notify.notify_one();
    }

    /// What has gone wrong so far: the core's task ended as it should not have, or a safety
    /// property fails.
    fn check(&mut self) -> Option<Failure> {
        self.broken
            .take()
            .or_else(|| oracle::safety(&lock(&self.world)))
    }

    /// Lets the core take what is ready, until it waits for something new: the clock moves on
    /// by a millisecond only once nothing in the runtime can go on. A core that reached a crash
    /// is killed, and one that ended is noted down. An end is expected only after a clean stop
    /// or the driver's end (`Ok`), or a failed save (an error); any other, and every panic, is
    /// kept as the run's failure.
    async fn settle(&mut self) {
        if !lock(&self.world).live.is_empty() {
            self.notify.notify_one();
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
        // A send that lands late lands at its time, whether or not the core looks.
        lock(&self.world).land();
        let crashed = std::mem::take(&mut lock(&self.world).crashed);
        if crashed {
            self.kill("crashed").await;
            return;
        }
        if self.core.as_ref().is_some_and(|c| c.task.is_finished()) {
            let running = self.core.take().expect("running");
            let stopped = running.stop.is_none();
            let ended = running.task.await;
            let (driver_ended, unsaved) = {
                let world = lock(&self.world);
                (world.harness.out.is_none(), world.unsaved)
            };
            let (how, broken) = match ended {
                Ok(Ok(())) => (
                    "Down(stopped)",
                    (!stopped && !driver_ended).then(|| {
                        fail(
                            "exit",
                            "the core ended with no stop asked and its driver running",
                        )
                    }),
                ),
                Ok(Err(_)) => (
                    "Down(could not save)",
                    (!unsaved).then(|| fail("exit", "the core ended unsaved with no save failing")),
                ),
                Err(error) => {
                    let detail = match error.try_into_panic() {
                        Ok(payload) => (payload.downcast_ref::<&str>().map(|s| s.to_string()))
                            .or_else(|| payload.downcast_ref::<String>().cloned())
                            .unwrap_or_default(),
                        Err(_) => "cancelled".into(),
                    };
                    let detail = format!("the core's task panicked: {detail}");
                    ("Down(panicked)", Some(fail("panic", &detail)))
                }
            };
            self.down(how);
            if self.broken.is_none() {
                self.broken = broken;
            }
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

    /// Asks the running core to stop cleanly (`Stop`).
    fn stop(&mut self) {
        if let Some(stop) = self.core.as_mut().and_then(|c| c.stop.take()) {
            let mut world = lock(&self.world);
            world.stopping = true;
            world.note("Stop");
            drop(world);
            let _ = stop.send(());
        }
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
            // The very next request hangs, ahead of any move still queued: the crash that follows
            // it in a schedule then always meets a harness that hangs.
            Action::Hang => lock(&self.world).harness.moves.push_front(Move::Hang),
            Action::Ignore => lock(&self.world).harness.moves.push_back(Move::Ignore),
            Action::DriverStops => lock(&self.world).driver_stops(),
            Action::Send { outcome } => lock(&self.world).outcomes.push_back(outcome),
            Action::ReadFails { at } => lock(&self.world).read_errors.push_back(at),
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
                self.stop();
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
            world.read_errors.clear();
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
        if let Some(failure) = self.broken.take() {
            return Some(failure);
        }
        if self.core.is_none() {
            self.start();
            self.settle().await;
        }
        let began = tokio::time::Instant::now();
        // What is still waiting when the bound passes. The last stop runs anyway.
        let mut left_at_bound = None;
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
            if let Some(failure) = self.check() {
                return Some(failure);
            }
            if self.core.is_none() {
                self.start();
            }
            self.settle().await;
            if let Some(failure) = self.check() {
                return Some(failure);
            }
            let left = oracle::liveness(&lock(&self.world));
            let (work, landing) = {
                let world = lock(&self.world);
                (world.has_work(), !world.landings.is_empty())
            };
            let waited = began.elapsed();
            // Done only within the bound, on the virtual clock, and with nothing still to land
            // (a late send would be a duplicate if the core sent it again under a new token).
            if left.is_empty() && !work && !landing && waited <= LIVENESS_BOUND {
                break;
            }
            if waited >= LIVENESS_BOUND {
                let first = (left.into_iter().next()).unwrap_or_else(|| {
                    if work {
                        fail("L2", "the harness still has work")
                    } else if landing {
                        fail("L2", "a send is still to land")
                    } else {
                        fail("L2", "the run did not settle")
                    }
                });
                left_at_bound = Some(Failure {
                    property: first.property,
                    detail: format!("{} within {} s", first.detail, LIVENESS_BOUND.as_secs()),
                });
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        // The last stop: the core ends cleanly, and everything is closed and sent.
        if self.core.is_none() {
            self.start();
            self.settle().await;
        }
        self.stop();
        for _ in 0..10 {
            self.settle().await;
            if self.core.is_none() {
                break;
            }
        }
        if self.core.is_some() && self.broken.is_none() {
            self.broken = Some(fail("exit", "the core did not end after the last stop"));
        }
        let failure = self.check();
        let world = lock(&self.world);
        left_at_bound
            .or(failure)
            .or_else(|| oracle::after_stop(&world))
    }
}
