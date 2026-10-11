//! In-process virtual instrument.
//!
//! Deterministic by construction and honest about itself: every reading is marked
//! `synthetic`, so it can never be mistaken for a real board measurement. It
//! exists so the plan → safety → execute → compare → report chain is testable
//! without hardware, and so error injection (a systematic offset) can be used to
//! exercise the KR2 verdict both ways.

use crate::hal::{
    Capabilities, Instrument, InstrumentError, MeasureRequest, Measurement, Quantity,
    SourceRequest,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// A programmable virtual instrument.
#[derive(Default)]
pub struct VirtualInstrument {
    readings: BTreeMap<Quantity, f64>,
    /// Deterministic pseudo-noise amplitude, as a percentage of the nominal value.
    noise_pct: f64,
    /// Systematic error, as a percentage of the nominal value.
    error_pct: f64,
    /// Device label reported in measurements.
    device: String,
    /// Requests that reached this backend (used by tests to prove gating).
    pub sources: Mutex<Vec<String>>,
}

impl VirtualInstrument {
    /// An instrument with no readings configured (measuring anything fails).
    pub fn new() -> Self {
        Self {
            device: "virtual".into(),
            ..Default::default()
        }
    }

    pub fn with_reading(mut self, quantity: Quantity, value: f64) -> Self {
        self.readings.insert(quantity, value);
        self
    }

    /// Inject a systematic error, e.g. `6.0` makes every reading 6% high.
    pub fn with_error_percent(mut self, pct: f64) -> Self {
        self.error_pct = pct;
        self
    }

    /// Inject reproducible noise of the given amplitude.
    pub fn with_noise_percent(mut self, pct: f64) -> Self {
        self.noise_pct = pct;
        self
    }

    pub fn with_device(mut self, device: &str) -> Self {
        self.device = device.into();
        self
    }

    pub fn set_reading(&mut self, quantity: Quantity, value: f64) {
        self.readings.insert(quantity, value);
    }

    /// The value this instrument will report for a quantity, before reduction.
    pub fn reading(&self, quantity: Quantity) -> Option<f64> {
        self.readings.get(&quantity).copied()
    }

    /// Deterministic noise in [-1, 1] derived from the quantity and value.
    fn noise(&self, quantity: Quantity, nominal: f64) -> f64 {
        if self.noise_pct == 0.0 {
            return 0.0;
        }
        // A tiny integer hash keeps this reproducible run to run (unlike RNGs).
        let seed = (quantity as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ nominal.to_bits().wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let x = seed ^ (seed >> 33);
        let unit = ((x % 2001) as f64 - 1000.0) / 1000.0;
        unit * self.noise_pct / 100.0 * nominal
    }
}

#[async_trait]
impl Instrument for VirtualInstrument {
    fn id(&self) -> String {
        format!("virtual:{}", self.device)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            quantities: vec![
                Quantity::VoltageDc,
                Quantity::CurrentDc,
                Quantity::Resistance,
                Quantity::Continuity,
                Quantity::Frequency,
            ],
            max_voltage: crate::safety::MAX_VOLTAGE_V,
            max_current: crate::safety::MAX_CURRENT_A,
            can_source: true,
        }
    }

    async fn measure(&self, req: &MeasureRequest) -> Result<Measurement, InstrumentError> {
        let nominal = self.reading(req.quantity).ok_or_else(|| {
            InstrumentError::Unavailable {
                backend: self.id(),
                reason: format!("未配置 {:?} 的虚拟读数", req.quantity),
            }
        })?;
        let value = nominal * (1.0 + self.error_pct / 100.0) + self.noise(req.quantity, nominal);
        Ok(Measurement::new(
            req.quantity,
            value,
            "virtual",
            &self.device,
            true,
        ))
    }

    async fn source(
        &self,
        req: &SourceRequest,
        _auth: Option<&crate::safety::ConfirmToken>,
    ) -> Result<(), InstrumentError> {
        // Record what reached the backend so tests can prove the gate ran first.
        self.sources.lock().unwrap().push(format!(
            "V={:?} I={:?} OUT={:?}",
            req.voltage, req.current_limit, req.output
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn readings_are_deterministic_and_marked_synthetic() {
        let vi = VirtualInstrument::new().with_reading(Quantity::VoltageDc, 3.3);
        let a = vi.measure(&MeasureRequest::new(Quantity::VoltageDc)).await.unwrap();
        let b = vi.measure(&MeasureRequest::new(Quantity::VoltageDc)).await.unwrap();
        assert_eq!(a.value, b.value);
        assert!(a.synthetic, "virtual data must be flagged");
        assert_eq!(a.backend, "virtual");
    }

    #[tokio::test]
    async fn error_injection_shifts_the_reading() {
        let vi = VirtualInstrument::new()
            .with_reading(Quantity::VoltageDc, 10.0)
            .with_error_percent(15.0);
        let m = vi.measure(&MeasureRequest::new(Quantity::VoltageDc)).await.unwrap();
        assert!((m.value - 11.5).abs() < 1e-9, "got {}", m.value);
    }

    #[tokio::test]
    async fn noise_is_reproducible() {
        let vi = VirtualInstrument::new()
            .with_reading(Quantity::CurrentDc, 0.1)
            .with_noise_percent(3.0);
        let a = vi.measure(&MeasureRequest::new(Quantity::CurrentDc)).await.unwrap();
        let b = vi.measure(&MeasureRequest::new(Quantity::CurrentDc)).await.unwrap();
        assert_eq!(a.value, b.value);
        assert!((a.value - 0.1).abs() < 0.004);
    }

    #[tokio::test]
    async fn unconfigured_quantity_is_an_error_not_a_zero() {
        let vi = VirtualInstrument::new();
        assert!(vi.measure(&MeasureRequest::new(Quantity::Frequency)).await.is_err());
    }

    #[tokio::test]
    async fn source_calls_are_recorded() {
        let vi = VirtualInstrument::new();
        vi.source(&SourceRequest::set_voltage(3.3), None).await.unwrap();
        assert_eq!(vi.sources.lock().unwrap().len(), 1);
    }
}
