//! ERC bridge: run `kicad-cli sch erc` and parse its JSON report.

use crate::sexpr::Sexpr;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum ErcError {
    #[error("kicad-cli not found (install kicad or set GCODE_KICAD_CLI)")]
    CliNotFound,
    #[error("kicad-cli exited with {code}: {stderr}")]
    CliFailed { code: i32, stderr: String },
    #[error("failed to parse ERC report: {0}")]
    Parse(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErcReport {
    #[serde(default)]
    pub violations: Vec<ErcViolation>,
    #[serde(default)]
    pub unconnected_items: Vec<serde_json::Value>,
    #[serde(default)]
    pub schematic_parity: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErcViolation {
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub type_: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
}

impl ErcReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
            && self.unconnected_items.is_empty()
            && self.schematic_parity.is_empty()
    }

    pub fn error_count(&self) -> usize {
        self.violations.iter().filter(|v| v.severity == "error").count()
    }

    pub fn summary(&self) -> String {
        if self.is_clean() {
            return "ERC: PASS (0 issues)".into();
        }
        format!(
            "ERC: {} violations ({} errors), {} unconnected, {} parity",
            self.violations.len(),
            self.error_count(),
            self.unconnected_items.len(),
            self.schematic_parity.len()
        )
    }
}

/// Locate the kicad-cli binary (overridable via GCODE_KICAD_CLI).
pub fn kicad_cli_path() -> PathBuf {
    std::env::var_os("GCODE_KICAD_CLI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("kicad-cli"))
}

/// Run ERC on a .kicad_sch, writing a JSON report next to it.
pub async fn run_erc(schematic: &Path) -> Result<ErcReport, ErcError> {
    let report_path = schematic.with_extension("erc.json");
    let output = Command::new(kicad_cli_path())
        .args([
            "sch",
            "erc",
            "--severity-error",
            "--severity-warning",
            "--format",
            "json",
            "--output",
        ])
        .arg(&report_path)
        .arg(schematic)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ErcError::CliNotFound,
            _ => ErcError::Io(e),
        })?;

    if !output.status.success() {
        return Err(ErcError::CliFailed {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let raw = tokio::fs::read(&report_path).await.map_err(ErcError::Io)?;
    let report: ErcReport = serde_json::from_slice(&raw)
        .map_err(|e| ErcError::Parse(e.to_string()))?;
    Ok(report)
}

/// Parse the raw JSON text of an ERC report (for tests and cached reports).
pub fn parse_report(json: &str) -> Result<ErcReport, ErcError> {
    serde_json::from_str(json).map_err(|e| ErcError::Parse(e.to_string()))
}

/// Round-trip helper: read a .kicad_sch back into `Sexpr` (used by the agent
/// loop to inspect what was actually generated).
pub fn read_schematic(schematic: &Path) -> Result<Sexpr, super::sexpr::ParseError> {
    let text = std::fs::read_to_string(schematic)
        .map_err(|_| super::sexpr::ParseError::Eof(0))?;
    let docs = Sexpr::parse(&text)?;
    docs.into_iter()
        .next()
        .ok_or(super::sexpr::ParseError::Eof(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clean_report() {
        let report = parse_report(r#"{"violations":[],"unconnected_items":[],"schematic_parity":[]}"#).unwrap();
        assert!(report.is_clean());
        assert!(report.summary().contains("PASS"));
    }

    #[test]
    fn parse_violation_report() {
        let json = r#"{
            "violations": [
                {"severity": "error", "type": "pin_to_pin", "description": "shorted pins", "items": []},
                {"severity": "warning", "type": "unannotated", "description": "x", "items": []}
            ],
            "unconnected_items": [{"a": 1}]
        }"#;
        let report = parse_report(json).unwrap();
        assert!(!report.is_clean());
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.violations.len(), 2);
    }
}
