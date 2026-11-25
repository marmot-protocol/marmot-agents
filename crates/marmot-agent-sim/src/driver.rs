//! The driver in a simulation: it hands every request to the world's scripted harness, which
//! decides what to do with it, and hands the core what the harness emits.

use crate::side::{Shared, lock};
use crate::world::{Decision, Handed};
use marmot_agent_driver::{
    Answer, Capabilities, Control, Driver, FilesIn, Harness, MidTurn, Output, Prompt, Refusal,
    Sessions,
};
use std::time::Duration;
use tokio::sync::mpsc;

/// A driver whose harness is the world's script.
pub struct SimDriver {
    /// The world.
    pub world: Shared,
    /// The harness's life this driver belongs to.
    pub life: u32,
    /// What the harness emits, each with when it did: the driver's boundary, where an output
    /// reaches the core.
    pub events: mpsc::UnboundedReceiver<(std::time::Instant, Output)>,
    /// When the output handed over last was emitted.
    pub arrived: Option<std::time::Instant>,
}

impl SimDriver {
    /// Hands the harness a request, and answers as it decides: at once, after a `Slow` move's
    /// time (when the harness takes it, and only then acts on it), or never.
    async fn hand(&self, request: Handed<'_>) -> Result<(), Refusal> {
        let decision = lock(&self.world).handed(self.life, request);
        match decision {
            Decision::Now(result) => result,
            Decision::Later(ms, take) => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                lock(&self.world).take(self.life, request, take)
            }
            Decision::Crash | Decision::Hang => std::future::pending().await,
        }
    }
}

impl Driver for SimDriver {
    fn harness(&self) -> Harness {
        Harness {
            name: "sim".into(),
            version: None,
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: true,
            mid_turn: MidTurn::Queue,
            sessions: Sessions::Core,
            resume: true,
            approvals: true,
            files_in: FilesIn::None,
            subagents: true,
            stop_subagent: false,
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        self.hand(Handed::Prompt(&prompt)).await
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        self.hand(Handed::Answer(&answer)).await
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        self.hand(Handed::Control(&control)).await
    }

    async fn next_event(&mut self) -> Option<Output> {
        let (at, output) = self.events.recv().await?;
        self.arrived = Some(at);
        Some(output)
    }

    fn arrived(&self) -> Option<std::time::Instant> {
        self.arrived
    }
}

impl Drop for SimDriver {
    /// A core dropped as its task ends after a clean stop panics there, when the schedule makes
    /// that fault by hand ([`crate::schedule::Faults::panic_when_stopped`]).
    fn drop(&mut self) {
        let panics = {
            let world = lock(&self.world);
            world.faults.panic_when_stopped && world.stopping && world.harness.life == self.life
        };
        if panics && !std::thread::panicking() {
            panic!("a fault made by hand: the core panics as it stops");
        }
    }
}
