//! The driver conformance suite: layer 2 of the verification plan
//! (`docs/design/verification.md`, "A conformance suite for every driver").
//!
//! One set of scenarios, written once against the [`Driver`](marmot_agent_driver::Driver) trait,
//! that every driver must pass. A [`Subject`](run::Subject) is a driver on a scriptable fake
//! harness; [`Run`](run::Run) drives it as the core does; the [`Checker`](check::Checker) looks at
//! every step and names the property a failure breaks (S1, S2, S4, S5, S11, S12, S13, and `seq`).
//! The fakes ([`fake`]) speak each harness's own protocol, grown from the stand-ins the drivers'
//! tests had, and can answer late or never, out of order, in pieces, die at any step, serve two
//! chats, restart, and replay a recording. [`faults`] puts bugs in on purpose, so a test shows
//! each check catches its own. [`native`] is D-007's check of a translator. [`subagents`] holds
//! the scenarios for a driver whose harness starts subagents, and [`socket`] those for a connector
//! on the socket, whose connection to the core can drop while its harness goes on.
//!
//! The drivers are run against it in this crate's tests: `tests/codex.rs`, `tests/acp.rs`,
//! `tests/pi.rs`, each with the deliberate faults it shows; the fakes themselves in
//! `tests/fake.rs`. See `README.md` beside this crate.

pub mod check;
pub mod fake;
pub mod faults;
pub mod native;
pub mod run;
pub mod scenarios;
pub mod socket;
pub mod subagents;

pub use check::{Checker, Violation};
pub use run::{Run, Subject};
