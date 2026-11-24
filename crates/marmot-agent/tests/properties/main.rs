//! Property tests of the core's pure parts (`docs/design/verification.md`, "6. Property tests"):
//! what it reads from a group, what it writes to one, who may do what, and what it prints.
//!
//! Each property names the id the page gives it. The cases are generated with proptest; how many
//! comes from `PROPTEST_CASES` (proptest's default of 256 when it is not set), which the nightly
//! run raises. A failing case is kept in `tests/proptest-regressions/`, in the file named after its
//! module, and committed, so it runs first from then on.

mod carrier;
mod conversation;
mod debug;
mod events;
mod requests;
mod rights;
