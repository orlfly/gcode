//! Concrete instrument backends.
//!
//! - [`virtual_instrument`]: in-process, deterministic, error-injectable. Used by
//!   the executor tests and by anyone who needs a repeatable bench.
//! - [`sigrok`]: drives a real `sigrok-cli`.
//! - [`serial_psu`]: minimal SCPI-lite programmable supply over a tty.

pub mod serial_psu;
pub mod sigrok;
pub mod virtual_instrument;

pub use serial_psu::SerialPsu;
pub use sigrok::SigrokInstrument;
pub use virtual_instrument::VirtualInstrument;
