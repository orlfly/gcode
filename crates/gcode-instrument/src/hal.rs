//! Measurement instrument hardware abstraction layer.
//!
//! Everything above this layer (test plans, comparison, reports) talks only to
//! [`Instrument`]. Backends hide whether the numbers come from a real bench DMM,
//! a sigrok-compatible device, a serial programmable supply, or an in-process
//! virtual instrument.
//!
//! Every [`Measurement`] carries its provenance (`backend`, `device`,
//! `synthetic`) so synthetic data can never be presented as a real board
//! measurement (see `design.md` D2).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Environment override for the `sigrok-cli` executable.
pub const SIGROK_CLI_ENV: &str = "GCODE_SIGROK_CLI";
/// Environment variable naming the serial port of a programmable supply.
pub const PSU_PORT_ENV: &str = "GCODE_PSU_PORT";
/// Enables `source()` (energising hardware). Off by default by design.
pub const ALLOW_SOURCE_ENV: &str = "GCODE_ALLOW_SOURCE";

/// A measurable quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Quantity {
    VoltageDc,
    CurrentDc,
    Resistance,
    Continuity,
    Frequency,
}

impl Quantity {
    /// SI unit for the quantity.
    pub fn unit(self) -> &'static str {
        match self {
            Quantity::VoltageDc => "V",
            Quantity::CurrentDc => "A",
            Quantity::Resistance => "ohm",
            Quantity::Continuity => "ohm",
            Quantity::Frequency => "Hz",
        }
    }

    /// Chinese label used in reports.
    pub fn label_zh(self) -> &'static str {
        match self {
            Quantity::VoltageDc => "直流电压",
            Quantity::CurrentDc => "直流电流",
            Quantity::Resistance => "电阻",
            Quantity::Continuity => "通断（电阻）",
            Quantity::Frequency => "频率",
        }
    }

    /// Parse from a sigrok unit/type pair, e.g. ("V", "DC").
    pub fn from_unit(unit: &str, kind: &str) -> Option<Self> {
        match (unit, kind) {
            ("V", "DC") => Some(Quantity::VoltageDc),
            ("A", "DC") => Some(Quantity::CurrentDc),
            ("Ohm", _) | ("ohm", _) => Some(Quantity::Resistance),
            ("Hz", _) => Some(Quantity::Frequency),
            _ => None,
        }
    }
}

/// What an instrument can do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub quantities: Vec<Quantity>,
    /// Hard ceiling the device itself is rated for (informational; the safety
    /// layer enforces the project limits independently).
    pub max_voltage: f64,
    pub max_current: f64,
    pub can_source: bool,
}

impl Capabilities {
    pub fn supports(&self, q: Quantity) -> bool {
        self.quantities.contains(&q)
    }
}

/// How a backend should reduce a series of samples into one reading.
///
/// A bench DMM gives a single number, but a sigrok acquisition returns a series;
/// what "the measurement" means depends on the intent (average level vs
/// amplitude), so it is part of the request rather than guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Reduce {
    /// Arithmetic mean (default; a DMM-like reading).
    #[default]
    Mean,
    /// Largest magnitude in the window (amplitude of a square/sine).
    Peak,
    /// Final sample (settled value of a step).
    Last,
}

/// A measurement request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasureRequest {
    pub quantity: Quantity,
    /// Backend-specific channel, e.g. a sigrok channel name (`A0`).
    pub channel: Option<String>,
    /// How many samples to acquire (backend-specific).
    pub samples: Option<u32>,
    /// How to reduce the acquired series.
    #[serde(default)]
    pub reduce: Reduce,
}

impl MeasureRequest {
    pub fn new(quantity: Quantity) -> Self {
        Self {
            quantity,
            channel: None,
            samples: None,
            reduce: Reduce::default(),
        }
    }

    pub fn on_channel(quantity: Quantity, channel: &str) -> Self {
        Self {
            quantity,
            channel: Some(channel.to_string()),
            samples: None,
            reduce: Reduce::default(),
        }
    }

    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = Some(samples);
        self
    }

    pub fn with_reduce(mut self, reduce: Reduce) -> Self {
        self.reduce = reduce;
        self
    }
}

/// One measurement, with provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub quantity: Quantity,
    pub value: f64,
    pub unit: String,
    /// Backend id, e.g. `virtual`, `sigrok`, `serial-psu`.
    pub backend: String,
    /// Device id, e.g. `demo`, `/dev/ttyUSB0`.
    pub device: String,
    /// True when the value did not come from a physical device (virtual backend,
    /// or a synthetic source such as sigrok's `demo` driver).
    pub synthetic: bool,
    /// Unix milliseconds; part of the machine-readable record.
    pub timestamp_ms: u128,
}

impl Measurement {
    pub fn new(
        quantity: Quantity,
        value: f64,
        backend: &str,
        device: &str,
        synthetic: bool,
    ) -> Self {
        Self {
            quantity,
            value,
            unit: quantity.unit().to_string(),
            backend: backend.to_string(),
            device: device.to_string(),
            synthetic,
            timestamp_ms: now_ms(),
        }
    }

    /// Human-facing source description used in reports.
    pub fn source_label(&self) -> String {
        if self.synthetic {
            format!("{}（合成数据，来源 {}:{})", "非真实测量", self.backend, self.device)
        } else {
            format!("{}:{}", self.backend, self.device)
        }
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// A request to energise or adjust a source output.
///
/// The user-confirmation token is **not** part of this struct: it is passed
/// separately as an authorisation argument to `Instrument::source`, because it
/// authorises the call rather than describing the desired output.
#[derive(Debug)]
pub struct SourceRequest {
    /// Target voltage in volts.
    pub voltage: Option<f64>,
    /// Current limit in amps.
    pub current_limit: Option<f64>,
    /// Turn the output on/off.
    pub output: Option<bool>,
}

impl SourceRequest {
    pub fn set_voltage(voltage: f64) -> Self {
        Self {
            voltage: Some(voltage),
            current_limit: None,
            output: None,
        }
    }

    pub fn with_current_limit(mut self, amps: f64) -> Self {
        self.current_limit = Some(amps);
        self
    }

    pub fn output_on(mut self, on: bool) -> Self {
        self.output = Some(on);
        self
    }

    /// Whether this request could put energy into the board.
    ///
    /// Turning the output on, or setting a non-zero target voltage, does.
    /// Turning it off, or setting 0 V with the output off, does not — otherwise
    /// the executor could not de-energise without asking for a token.
    pub fn energises(&self) -> bool {
        if self.output == Some(true) {
            return true;
        }
        if self.voltage.is_some_and(|v| v != 0.0) {
            return true;
        }
        self.current_limit.is_some() && self.output != Some(false)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstrumentError {
    #[error("后端 {backend} 不可用：{reason}")]
    Unavailable { backend: String, reason: String },
    #[error("设备 {0} 不存在：{1}")]
    DeviceNotFound(String, String),
    #[error("后端输出无法解析（{backend}）：{detail}")]
    Parse { backend: String, detail: String },
    #[error("仪器不支持测量 {0:?}")]
    Unsupported(Quantity),
    #[error("测量读数超出预期范围：{0}")]
    OutOfRange(String),
    /// A safety gate refused the operation.
    #[error("已拒绝：{reason}（{hint}）")]
    Rejected { reason: String, hint: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// A measurement instrument.
#[async_trait]
pub trait Instrument: Send + Sync {
    /// Stable backend/device id, e.g. `virtual`, `sigrok:demo`.
    fn id(&self) -> String;
    fn capabilities(&self) -> Capabilities;
    async fn measure(&self, req: &MeasureRequest) -> Result<Measurement, InstrumentError>;
    /// Energise/adjust an output. Backends without source support must reject.
    ///
    /// `auth` carries the user's confirmation for anything that energises
    /// hardware; the safety layer requires it and backends may ignore it.
    async fn source(
        &self,
        req: &SourceRequest,
        auth: Option<&super::safety::ConfirmToken>,
    ) -> Result<(), InstrumentError> {
        let _ = (req, auth);
        Err(InstrumentError::Unavailable {
            backend: self.id(),
            reason: "该后端不支持源输出".into(),
        })
    }
}

/// Whether a backend can be used right now, and why not if it cannot.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendStatus {
    pub backend: &'static str,
    pub available: bool,
    pub reason: String,
}

impl BackendStatus {
    /// One-line description for tools and reports.
    pub fn describe(&self) -> String {
        format!(
            "{}{}：{}",
            self.backend,
            if self.available { "（可用）" } else { "（不可用）" },
            self.reason
        )
    }
}

/// Path to the sigrok CLI, honouring `GCODE_SIGROK_CLI`.
pub fn sigrok_cli_path() -> PathBuf {
    std::env::var_os(SIGROK_CLI_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("sigrok-cli"))
}

/// Whether energising hardware is permitted at all (default: no).
pub fn source_enabled() -> bool {
    matches!(
        std::env::var(ALLOW_SOURCE_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Probe every backend and report availability with reasons.
pub async fn probe_backends() -> Vec<BackendStatus> {
    let mut out = vec![BackendStatus {
        backend: "virtual",
        available: true,
        reason: "进程内虚拟仪器（合成数据，用于验证与 CI）".into(),
    }];

    // sigrok
    let sigrok = super::instruments::sigrok::SigrokInstrument::probe().await;
    out.push(match sigrok {
        Ok(s) => BackendStatus {
            backend: "sigrok",
            available: true,
            reason: format!("sigrok-cli 可用；设备 {}", s.device_label()),
        },
        Err(e) => BackendStatus {
            backend: "sigrok",
            available: false,
            reason: e.to_string(),
        },
    });

    // serial programmable supply
    out.push(match std::env::var(PSU_PORT_ENV) {
        Ok(port) if Path::new(&port).exists() => BackendStatus {
            backend: "serial-psu",
            available: true,
            reason: format!("串口 {port} 已配置"),
        },
        Ok(port) => BackendStatus {
            backend: "serial-psu",
            available: false,
            reason: format!("{PSU_PORT_ENV}={port} 但该设备不存在"),
        },
        Err(_) => BackendStatus {
            backend: "serial-psu",
            available: false,
            reason: format!("未设置 {PSU_PORT_ENV}（无可编程电源）"),
        },
    });

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities_carry_units() {
        assert_eq!(Quantity::VoltageDc.unit(), "V");
        assert_eq!(Quantity::Resistance.unit(), "ohm");
    }

    #[test]
    fn parses_sigrok_units() {
        assert_eq!(
            Quantity::from_unit("V", "DC"),
            Some(Quantity::VoltageDc)
        );
        assert_eq!(
            Quantity::from_unit("A", "DC"),
            Some(Quantity::CurrentDc)
        );
        assert_eq!(Quantity::from_unit("V", "AC"), None);
        assert_eq!(Quantity::from_unit("s", "time"), None);
    }

    #[test]
    fn measurement_reports_its_provenance() {
        let real = Measurement::new(Quantity::VoltageDc, 3.3, "serial-psu", "/dev/ttyUSB0", false);
        assert!(!real.synthetic);
        assert!(real.source_label().contains("/dev/ttyUSB0"));
        assert!(!real.source_label().contains("非真实"));

        let synth = Measurement::new(Quantity::VoltageDc, 3.3, "sigrok", "demo", true);
        assert!(synth.synthetic);
        assert!(synth.source_label().contains("非真实"));
    }

    #[test]
    fn source_request_knows_when_it_energises() {
        assert!(SourceRequest::set_voltage(3.3).energises());
        assert!(SourceRequest::set_voltage(0.0).output_on(true).energises());
        assert!(SourceRequest::set_voltage(5.0).with_current_limit(0.2).energises());
        // De-energising must not demand confirmation, or the run could never
        // power the board back down.
        assert!(!SourceRequest {
            voltage: None,
            current_limit: None,
            output: Some(false),
        }
        .energises());
        assert!(!SourceRequest::set_voltage(0.0).output_on(false).energises());
    }

    #[test]
    fn reduce_defaults_to_the_mean() {
        assert_eq!(MeasureRequest::new(Quantity::VoltageDc).reduce, Reduce::Mean);
        assert_eq!(
            MeasureRequest::new(Quantity::VoltageDc).with_reduce(Reduce::Peak).reduce,
            Reduce::Peak
        );
    }

    #[test]
    fn capabilities_membership() {
        let caps = Capabilities {
            quantities: vec![Quantity::VoltageDc, Quantity::CurrentDc],
            max_voltage: 48.0,
            max_current: 5.0,
            can_source: false,
        };
        assert!(caps.supports(Quantity::VoltageDc));
        assert!(!caps.supports(Quantity::Resistance));
    }

    #[test]
    fn source_flag_defaults_off_and_env_can_enable_it() {
        // Held so no other test observes a half-set flag.
        let _guard = crate::test_support::env_lock();
        // SAFETY: the env lock above serialises all flag mutations.
        unsafe { std::env::remove_var(ALLOW_SOURCE_ENV) };
        assert!(!source_enabled(), "source must be off by default");

        unsafe { std::env::set_var(ALLOW_SOURCE_ENV, "1") };
        assert!(source_enabled());
        unsafe { std::env::set_var(ALLOW_SOURCE_ENV, "true") };
        assert!(source_enabled());
        unsafe { std::env::set_var(ALLOW_SOURCE_ENV, "0") };
        assert!(!source_enabled(), "only 1/true/yes enable it");

        unsafe { std::env::remove_var(ALLOW_SOURCE_ENV) };
    }
}
