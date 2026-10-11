//! gcode-instrument: measurement HAL for the Gcode agent.
//!
//! Layers:
//! - [`hal`]: instrument abstraction, capabilities, backends registry
//! - [`safety`]: hard limits, user-confirmation token, fuse policy
//! - [`plan`]: test plan IR with explicit baseline provenance
//! - [`compare`]: sim-vs-measured deviation and the KR2 verdict
//! - [`report`]: TestReport IR (PRD §7.3.5) and readable rendering
//! - [`executor`]: runs a plan through the safety gate, with fuse and power-off
//! - [`instruments`]: concrete backends (`virtual`, `sigrok`, `serial-psu`)
//!
//! Honest scope (R1.5): the chain is verified against real `sigrok-cli` software
//! and real acquisition paths, but this machine has **no bench instrument**, so
//! no KR2 conclusion about a physical board is claimed. Every reading carries its
//! provenance, and synthetic sources are marked as such.

pub mod compare;
pub mod executor;
pub mod hal;
pub mod instruments;
pub mod plan;
pub mod report;
pub mod safety;

pub use compare::{
    compare, compare_all, Comparison, CompareVerdict, TolerancePolicy, ToleranceSource,
};
pub use executor::Executor;
pub use hal::{
    probe_backends, BackendStatus, Capabilities, Instrument, InstrumentError, MeasureRequest,
    Measurement, Quantity, Reduce, SourceRequest,
};
pub use plan::{Baseline, Expected, PlanError, SupplySetting, TestPlan, TestStep};
pub use report::{ReportItem, TestReport};
pub use safety::{ConfirmToken, SafetyViolation, MAX_CURRENT_A, MAX_VOLTAGE_V};

/// Serialises tests that touch process-global environment state
/// (`GCODE_ALLOW_SOURCE`). Production code never uses this.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Acquire the global environment lock. Hold it for the whole test.
    pub fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        // A panicking test must not poison the lock for the others.
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
