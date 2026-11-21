//! The Pi driver.
//!
//! It speaks `pi --mode rpc`, Pi's JSON lines over standard input and output, reading Pi's
//! records with no library between (see `docs/design/drivers.md`), and turns what Pi says into
//! agent events.
//!
//! It has two halves. [`Translator`] reads the records of one Pi and makes the events, and builds
//! the reply to an extension's question. It does no input or output, and is tested by replaying
//! the recordings in `fixtures/recordings/pi` against the events the prototype makes for each.
//! [`Pi`] is the transport: it runs one Pi for each chat, feeds each chat's translator, and
//! implements the driver trait.

mod driver;
mod translate;

pub use driver::{Pi, Setup};
pub use translate::{Direction, NOT_SENT, Translator};
