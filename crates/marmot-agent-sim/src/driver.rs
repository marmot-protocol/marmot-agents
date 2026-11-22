//! The driver in a simulation: it hands every request to the world's scripted harness, which
//! decides what to do with it, and hands the core what the harness emits.

use crate::side::{Shared, lock};
use crate::world::Decision;
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
    /// What the harness emits.
    pub events: mpsc::UnboundedReceiver<Output>,
}

impl SimDriver {
    async fn answer_with(decision: Decision) -> Result<(), Refusal> {
        match decision {
            Decision::Now(result) => result,
            Decision::Slow(ms, result) => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                result
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
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        let decision = lock(&self.world).prompt(self.life, &prompt);
        Self::answer_with(decision).await
    }

    async fn answer(&mut self, answer: Answer) -> Result<(), Refusal> {
        let decision = lock(&self.world).answer(self.life, &answer);
        Self::answer_with(decision).await
    }

    async fn control(&mut self, control: Control) -> Result<(), Refusal> {
        let decision = lock(&self.world).control(self.life, &control);
        Self::answer_with(decision).await
    }

    async fn next_event(&mut self) -> Option<Output> {
        self.events.recv().await
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
