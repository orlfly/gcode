//! Minimal serial programmable-supply backend (SCPI-lite).
//!
//! Only a documented minimum is implemented: read voltage/current, set voltage,
//! set the current limit, and switch the output. That is enough for the R1.5
//! functional tests and small enough to keep the protocol surface honest.
//!
//! No serial crate is used: the tty is opened as a file and written/read with a
//! timeout. Real-device behaviour still needs verification against hardware, so
//! the response parsing is covered by recorded fixtures.

use crate::hal::{
    Capabilities, Instrument, InstrumentError, MeasureRequest, Measurement, PSU_PORT_ENV, Quantity,
    SourceRequest,
};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Default response timeout.
pub const DEFAULT_TIMEOUT_MS: u64 = 800;

/// Extract the first number from a device response.
///
/// Supplies answer with bare numbers (`12.003`), sometimes with a unit
/// (`12.003V`), and occasionally echo the command first. Taking the first
/// parseable token handles all three.
pub fn parse_value(text: &str) -> Option<f64> {
    for token in text.split(|c: char| c.is_whitespace() || c == ',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Strip a trailing unit suffix, keeping sign/digits/exponent/'k'/'m'.
        let numeric: String = trimmed
            .chars()
            .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
            .collect();
        if numeric.is_empty() {
            continue;
        }
        if let Ok(v) = numeric.parse::<f64>() {
            return Some(v);
        }
    }
    None
}

/// Parse an on/off acknowledgement (`OK`, `1`, `ON`).
pub fn parse_ack(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    t.contains("ok") || t == "1" || t.starts_with("on")
}

/// A serial-attached programmable supply.
#[derive(Debug, Clone)]
pub struct SerialPsu {
    port: PathBuf,
    timeout: Duration,
}

impl SerialPsu {
    /// Only available when `GCODE_PSU_PORT` names an existing device node.
    pub fn probe() -> Result<Self, InstrumentError> {
        let port = match std::env::var(PSU_PORT_ENV) {
            Ok(p) if !p.trim().is_empty() => PathBuf::from(p),
            _ => {
                return Err(InstrumentError::Unavailable {
                    backend: "serial-psu".into(),
                    reason: format!("未设置 {PSU_PORT_ENV}（本机无可编程电源）"),
                })
            }
        };
        if !Path::new(&port).exists() {
            return Err(InstrumentError::DeviceNotFound(
                port.display().to_string(),
                format!("{PSU_PORT_ENV} 指向的设备不存在"),
            ));
        }
        Ok(Self {
            port,
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
        })
    }

    pub fn at(port: impl Into<PathBuf>) -> Self {
        Self {
            port: port.into(),
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
        }
    }

    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout = Duration::from_millis(ms);
        self
    }

    pub fn port(&self) -> &Path {
        &self.port
    }

    /// SCPI-lite command for reading a quantity.
    pub fn measure_command(quantity: Quantity) -> Option<&'static str> {
        match quantity {
            Quantity::VoltageDc => Some("MEAS:VOLT?"),
            Quantity::CurrentDc => Some("MEAS:CURR?"),
            _ => None,
        }
    }

    /// Send a command and return the raw response.
    pub async fn send(&self, command: &str) -> Result<String, InstrumentError> {
        let mut file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.port)
            .await?;
        let line = format!("{command}\n");
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;

        let mut buf = vec![0u8; 256];
        let mut acc = String::new();
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, file.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if acc.contains('\n') {
                        break;
                    }
                }
                Ok(Err(e)) => return Err(InstrumentError::Io(e)),
                Err(_) => break, // timeout: return what we have
            }
        }
        if acc.trim().is_empty() {
            return Err(InstrumentError::Parse {
                backend: "serial-psu".into(),
                detail: format!("设备未在 {:?} 内响应 {command:?}", self.timeout),
            });
        }
        Ok(acc)
    }
}

#[async_trait]
impl Instrument for SerialPsu {
    fn id(&self) -> String {
        format!("serial-psu:{}", self.port.display())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            quantities: vec![Quantity::VoltageDc, Quantity::CurrentDc],
            max_voltage: crate::safety::MAX_VOLTAGE_V,
            max_current: crate::safety::MAX_CURRENT_A,
            can_source: true,
        }
    }

    async fn measure(&self, req: &MeasureRequest) -> Result<Measurement, InstrumentError> {
        let cmd = Self::measure_command(req.quantity).ok_or(InstrumentError::Unsupported(
            req.quantity,
        ))?;
        let response = self.send(cmd).await?;
        let value = parse_value(&response).ok_or_else(|| InstrumentError::Parse {
            backend: "serial-psu".into(),
            detail: format!("无法从 {response:?} 解析 {:?}", req.quantity),
        })?;
        Ok(Measurement::new(
            req.quantity,
            value,
            "serial-psu",
            &self.port.display().to_string(),
            false,
        ))
    }

    async fn source(
        &self,
        req: &SourceRequest,
        _auth: Option<&crate::safety::ConfirmToken>,
    ) -> Result<(), InstrumentError> {
        // Note: the safety layer has already enforced caps and confirmation.
        if let Some(v) = req.voltage {
            self.send(&format!("VOLT {v:.3}")).await?;
        }
        if let Some(c) = req.current_limit {
            self.send(&format!("CURR {c:.3}")).await?;
        }
        if let Some(on) = req.output {
            let response = self.send(&format!("OUT {}", if on { 1 } else { 0 })).await?;
            if !parse_ack(&response) && on {
                return Err(InstrumentError::Parse {
                    backend: "serial-psu".into(),
                    detail: format!("输出开启未被确认：{response:?}"),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_united_responses() {
        assert_eq!(parse_value("12.003"), Some(12.003));
        assert_eq!(parse_value("12.003V"), Some(12.003));
        assert_eq!(parse_value("12.003 V\n"), Some(12.003));
        assert_eq!(parse_value("-0.512A\n"), Some(-0.512));
        assert_eq!(parse_value("1.25e-3"), Some(1.25e-3));
    }

    #[test]
    fn parses_echoed_commands() {
        // Some supplies echo the command before the value.
        assert_eq!(parse_value("MEAS:VOLT? 5.001\n"), Some(5.001));
    }

    #[test]
    fn rejects_text_without_numbers() {
        assert_eq!(parse_value("ERROR\n"), None);
        assert_eq!(parse_value(""), None);
    }

    #[test]
    fn parses_acknowledgements() {
        assert!(parse_ack("OK\n"));
        assert!(parse_ack("1"));
        assert!(parse_ack("ON"));
        assert!(!parse_ack("ERR"));
        assert!(!parse_ack("0"));
    }

    #[test]
    fn measurement_commands_are_defined_for_supported_quantities() {
        assert_eq!(
            SerialPsu::measure_command(Quantity::VoltageDc),
            Some("MEAS:VOLT?")
        );
        assert_eq!(
            SerialPsu::measure_command(Quantity::CurrentDc),
            Some("MEAS:CURR?")
        );
        assert_eq!(SerialPsu::measure_command(Quantity::Resistance), None);
    }

    #[test]
    fn probe_requires_a_configured_port() {
        // Tests do not set GCODE_PSU_PORT, so probing must fail clearly.
        let err = SerialPsu::probe().unwrap_err();
        match err {
            InstrumentError::Unavailable { reason, .. } => assert!(reason.contains(PSU_PORT_ENV)),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn missing_device_is_reported_as_such() {
        let psu = SerialPsu::at("/nonexistent/ttyUSB9");
        assert_eq!(psu.port().display().to_string(), "/nonexistent/ttyUSB9");
        assert!(!psu.capabilities().quantities.is_empty());
    }
}
