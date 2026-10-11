//! Backend that drives a real `sigrok-cli`.
//!
//! Only three surfaces of sigrok are relied on, all long-stable:
//! `--version`, `--scan`, and the per-line reading output
//! `<channel>: <value> <unit> <type>` (e.g. `A0: -10.0000 V DC`).
//!
//! Errors are classified deliberately: a missing CLI, a missing device and an
//! unparseable response need different fixes, so they are different variants.

use crate::hal::{
    sigrok_cli_path, Capabilities, Instrument, InstrumentError, MeasureRequest, Measurement,
    Quantity, Reduce,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

/// sigrok driver ids whose output is synthesised rather than measured.
pub const SYNTHETIC_DRIVERS: &[&str] = &["demo"];

/// A device found by `sigrok-cli --scan`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedDevice {
    pub id: String,
    pub description: String,
}

impl ScannedDevice {
    /// Whether this device generates synthetic data.
    pub fn synthetic(&self) -> bool {
        SYNTHETIC_DRIVERS.contains(&self.id.as_str())
    }
}

/// An instrument backed by `sigrok-cli`.
#[derive(Debug, Clone)]
pub struct SigrokInstrument {
    cli: PathBuf,
    device: ScannedDevice,
    default_channel: String,
    version: String,
}

impl SigrokInstrument {
    /// Parse `sigrok-cli --scan` output.
    ///
    /// Lines look like `<id> - <description>`; the header line and the
    /// "No devices found" case are handled explicitly.
    pub fn parse_scan(text: &str) -> Result<Vec<ScannedDevice>, InstrumentError> {
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.ends_with(':') || line.starts_with("The following") {
                continue;
            }
            if line.to_ascii_lowercase().contains("no devices found") {
                return Err(InstrumentError::DeviceNotFound(
                    "（无）".into(),
                    "sigrok-cli 未发现任何设备；请连接 DMM/逻辑分析仪并确认权限".into(),
                ));
            }
            match line.split_once(" - ") {
                Some((id, desc)) => out.push(ScannedDevice {
                    id: id.trim().to_string(),
                    description: desc.trim().to_string(),
                }),
                None => out.push(ScannedDevice {
                    id: line.to_string(),
                    description: String::new(),
                }),
            }
        }
        Ok(out)
    }

    /// Parse per-line readings: `<channel>: <value> <unit> <type>`.
    pub fn parse_readings(
        text: &str,
        channel: &str,
    ) -> Result<Vec<(f64, String, String)>, InstrumentError> {
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((ch, rest)) = line.split_once(':') else {
                continue;
            };
            if ch.trim() != channel {
                continue;
            }
            let mut parts = rest.split_whitespace();
            let (Some(value), Some(unit)) = (parts.next(), parts.next()) else {
                return Err(InstrumentError::Parse {
                    backend: "sigrok".into(),
                    detail: format!("读数行缺少数值或单位：{line:?}"),
                });
            };
            let value: f64 = value.parse().map_err(|_| InstrumentError::Parse {
                backend: "sigrok".into(),
                detail: format!("数值无法解析：{value:?}"),
            })?;
            let kind = parts.next().unwrap_or("").to_string();
            out.push((value, unit.to_string(), kind));
        }
        if out.is_empty() {
            return Err(InstrumentError::Parse {
                backend: "sigrok".into(),
                detail: format!("没有通道 {channel} 的读数（原始输出前 200 字符：{:?}）", &text.chars().take(200).collect::<String>()),
            });
        }
        Ok(out)
    }

    /// Reduce a reading series according to the request.
    pub fn reduce(series: &[f64], reduce: Reduce) -> f64 {
        if series.is_empty() {
            return f64::NAN;
        }
        match reduce {
            Reduce::Mean => series.iter().sum::<f64>() / series.len() as f64,
            Reduce::Peak => series
                .iter()
                .fold(0.0f64, |acc, v| acc.max(v.abs())),
            Reduce::Last => *series.last().unwrap_or(&f64::NAN),
        }
    }

    /// Probe the CLI and pick a device.
    ///
    /// A real device is preferred over a synthetic one: if both are present we
    /// must not silently report `demo` data as if it were measured.
    pub async fn probe() -> Result<Self, InstrumentError> {
        let cli = sigrok_cli_path();
        let version = Command::new(&cli)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|_| InstrumentError::Unavailable {
                backend: "sigrok".into(),
                reason: format!(
                    "找不到 sigrok-cli（可设置 {} 指向可执行文件）",
                    crate::hal::SIGROK_CLI_ENV
                ),
            })?;
        if !version.status.success() {
            return Err(InstrumentError::Unavailable {
                backend: "sigrok".into(),
                reason: format!(
                    "sigrok-cli 执行失败：{}",
                    String::from_utf8_lossy(&version.stderr).trim()
                ),
            });
        }
        let version_text = String::from_utf8_lossy(&version.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();

        let scan = Command::new(&cli)
            .arg("--scan")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        let scan_text = format!(
            "{}{}",
            String::from_utf8_lossy(&scan.stdout),
            String::from_utf8_lossy(&scan.stderr)
        );
        let mut devices = Self::parse_scan(&scan_text)?;
        if devices.is_empty() {
            return Err(InstrumentError::DeviceNotFound(
                "（无）".into(),
                "sigrok-cli 未发现任何设备".into(),
            ));
        }
        devices.sort_by_key(|d| d.synthetic()); // real devices first
        let device = devices.remove(0);

        Ok(Self {
            cli,
            device,
            default_channel: "A0".into(),
            version: version_text,
        })
    }

    /// Use a specific driver/channel instead of the probed default.
    pub async fn with_driver(
        driver: &str,
        channel: &str,
        description: &str,
    ) -> Result<Self, InstrumentError> {
        let mut s = Self::probe().await.or_else(|_| {
            // Allow explicit drivers even when --scan cannot find them.
            Ok::<Self, InstrumentError>(Self {
                cli: sigrok_cli_path(),
                device: ScannedDevice {
                    id: driver.to_string(),
                    description: description.to_string(),
                },
                default_channel: channel.to_string(),
                version: String::new(),
            })
        })?;
        s.device = ScannedDevice {
            id: driver.to_string(),
            description: description.to_string(),
        };
        s.default_channel = channel.to_string();
        Ok(s)
    }

    pub fn device_label(&self) -> String {
        if self.device.synthetic() {
            format!("{}（sigrok 合成设备，非真实测量）", self.device.id)
        } else {
            self.device.id.clone()
        }
    }

    pub fn device(&self) -> &ScannedDevice {
        &self.device
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

#[async_trait]
impl Instrument for SigrokInstrument {
    fn id(&self) -> String {
        format!("sigrok:{}", self.device.id)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            quantities: vec![
                Quantity::VoltageDc,
                Quantity::CurrentDc,
                Quantity::Resistance,
                Quantity::Frequency,
            ],
            max_voltage: crate::safety::MAX_VOLTAGE_V,
            max_current: crate::safety::MAX_CURRENT_A,
            can_source: false,
        }
    }

    async fn measure(&self, req: &MeasureRequest) -> Result<Measurement, InstrumentError> {
        let channel = req
            .channel
            .clone()
            .unwrap_or_else(|| self.default_channel.clone());
        let samples = req.samples.unwrap_or(8);

        let output = Command::new(&self.cli)
            .args(["-d", &self.device.id, "--channels", &channel, "--samples"])
            .arg(samples.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|_| InstrumentError::Unavailable {
                backend: "sigrok".into(),
                reason: format!("找不到 {}", self.cli.display()),
            })?;

        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let lowered = text.to_ascii_lowercase();
        if lowered.contains("no device") || lowered.contains("not found") {
            return Err(InstrumentError::DeviceNotFound(
                self.device.id.clone(),
                "sigrok-cli 找不到该设备；请确认已连接且驱动可用".into(),
            ));
        }

        let readings = Self::parse_readings(&text, &channel)?;
        let values: Vec<f64> = readings.iter().map(|(v, _, _)| *v).collect();
        let (_, unit, kind) = readings[0].clone();
        let value = Self::reduce(&values, req.reduce);

        // Trust the device's own unit when it disagrees with the request.
        let quantity = Quantity::from_unit(&unit, &kind).unwrap_or(req.quantity);
        let mut m = Measurement::new(
            quantity,
            value,
            "sigrok",
            &self.device.id,
            self.device.synthetic(),
        );
        m.unit = unit;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scan_output() {
        let text = "The following devices were found:\ndemo - Demo device with 13 channels: D0 D1 A0\nuni-t-ut61e-1.2 - UNI-T UT61E\n";
        let devices = SigrokInstrument::parse_scan(text).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, "demo");
        assert!(devices[0].synthetic());
        assert_eq!(devices[1].id, "uni-t-ut61e-1.2");
        assert!(!devices[1].synthetic());
    }

    #[test]
    fn no_devices_is_a_device_error() {
        let text = "No devices found.\n";
        assert!(matches!(
            SigrokInstrument::parse_scan(text),
            Err(InstrumentError::DeviceNotFound(..))
        ));
    }

    #[test]
    fn parses_reading_lines() {
        // Shape captured from a real `sigrok-cli -d demo --channels A0` run.
        let text = "A0: -10.0000 V DC\nA0: 10.0000 V DC\nA0: 10.0000 V DC\n";
        let r = SigrokInstrument::parse_readings(text, "A0").unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].0, -10.0);
        assert_eq!(r[0].1, "V");
        assert_eq!(r[0].2, "DC");
    }

    #[test]
    fn other_channels_are_ignored_and_missing_channel_is_an_error() {
        let text = "D0: 1 D0\nD1: 0 D0\n";
        assert!(matches!(
            SigrokInstrument::parse_readings(text, "A0"),
            Err(InstrumentError::Parse { .. })
        ));
        let r = SigrokInstrument::parse_readings(text, "D0").unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, "D0");
    }

    #[test]
    fn unparseable_value_is_a_parse_error() {
        let text = "A0: abc V DC\n";
        assert!(matches!(
            SigrokInstrument::parse_readings(text, "A0"),
            Err(InstrumentError::Parse { .. })
        ));
    }

    #[test]
    fn reduction_modes() {
        let series = [-10.0, 10.0, 10.0, 10.0];
        assert!((SigrokInstrument::reduce(&series, Reduce::Mean) - 5.0).abs() < 1e-9);
        assert!((SigrokInstrument::reduce(&series, Reduce::Peak) - 10.0).abs() < 1e-9);
        assert!((SigrokInstrument::reduce(&series, Reduce::Last) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn demo_device_labels_itself_as_not_a_real_measurement() {
        let s = SigrokInstrument {
            cli: "sigrok-cli".into(),
            device: ScannedDevice {
                id: "demo".into(),
                description: String::new(),
            },
            default_channel: "A0".into(),
            version: String::new(),
        };
        assert!(s.device_label().contains("非真实测量"));
    }

    #[test]
    fn sigrok_backend_cannot_source() {
        let s = SigrokInstrument {
            cli: "sigrok-cli".into(),
            device: ScannedDevice {
                id: "demo".into(),
                description: String::new(),
            },
            default_channel: "A0".into(),
            version: String::new(),
        };
        assert!(!s.capabilities().can_source);
    }
}
