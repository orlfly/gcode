//! ngspice driver: locate the binary, run a deck, retrieve waveforms.
//!
//! Waveforms are retrieved through ngspice's `wrdata` (configured by
//! `netlist::generate` to emit a single scale column plus a header), so we never
//! scrape terminal tables. Columns are addressed **by header name**, which is
//! what makes `.op` work: its first column is an unnamed scale placeholder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

/// Environment override for the ngspice executable.
pub const NGSPICE_ENV: &str = "GCODE_NGSPICE";
/// Minimum ngspice major version we are willing to drive.
pub const MIN_MAJOR_VERSION: u32 = 30;

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("ngspice 未找到：请安装 ngspice（或设置 {NGSPICE_ENV} 指向可执行文件）。\
             无需 root 的安装方式：apt-get download ngspice && dpkg -x ngspice_*.deb <dir>，\
             然后设置 GCODE_NGSPICE=<dir>/usr/bin/ngspice")]
    NotFound,
    #[error("无法解析 ngspice 版本（输出：{0:?}）")]
    VersionUnreadable(String),
    #[error("ngspice 版本过低：{found}（需要 >= {min}）")]
    TooOld { found: String, min: u32 },
    #[error("ngspice 执行失败（退出码 {code}）：{detail}")]
    Failed { code: i32, detail: String },
    #[error("仿真未产出波形数据：{detail}")]
    NoWaveform { detail: String },
    #[error("波形文件解析失败：{0}")]
    Parse(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// A probed ngspice installation.
#[derive(Debug, Clone, PartialEq)]
pub struct Ngspice {
    pub path: PathBuf,
    pub version: String,
    pub major: u32,
}

fn parse_major(raw: &str) -> Option<u32> {
    // Banner looks like: "** ngspice-42 : Circuit level simulation program"
    for token in raw.split(|c: char| c.is_whitespace() || c == ':' || c == '*') {
        if let Some(rest) = token.strip_prefix("ngspice-") {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(v) = digits.parse() {
                return Some(v);
            }
        }
    }
    None
}

impl Ngspice {
    /// Locate ngspice via `GCODE_NGSPICE` then `PATH`.
    pub async fn probe() -> Result<Self, RunnerError> {
        Self::probe_with(MIN_MAJOR_VERSION).await
    }

    pub async fn probe_with(min_major: u32) -> Result<Self, RunnerError> {
        let path = std::env::var_os(NGSPICE_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ngspice"));
        Self::probe_at_with(&path, min_major).await
    }

    /// Probe an explicit binary path (no environment lookup).
    ///
    /// Used by tests and by callers that resolve the binary themselves, so they
    /// never have to mutate process-global environment state.
    pub async fn probe_at(path: &Path) -> Result<Self, RunnerError> {
        Self::probe_at_with(path, MIN_MAJOR_VERSION).await
    }

    async fn probe_at_with(path: &Path, min_major: u32) -> Result<Self, RunnerError> {
        let output = Command::new(path)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|_| RunnerError::NotFound)?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let major = parse_major(&combined)
            .ok_or_else(|| RunnerError::VersionUnreadable(combined.clone()))?;
        if major < min_major {
            return Err(RunnerError::TooOld {
                found: format!("ngspice-{major}"),
                min: min_major,
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            version: format!("ngspice-{major}"),
            major,
        })
    }

    /// Run `deck_text` in `workdir` and read back `wrdata_file`.
    ///
    /// `deck_text` must have been produced with
    /// `DeckOptions { wrdata_file, .. }` set to the same relative name.
    pub async fn run(
        &self,
        workdir: &Path,
        deck_text: &str,
        wrdata_file: &str,
    ) -> Result<Waveform, RunnerError> {
        std::fs::create_dir_all(workdir)?;
        let deck_path = workdir.join("sim.cir");
        tokio::fs::write(&deck_path, deck_text).await?;
        // Remove any stale output so a missing file is a real signal.
        let data_path = workdir.join(wrdata_file);
        let _ = tokio::fs::remove_file(&data_path).await;

        let output = Command::new(&self.path)
            .arg("-b")
            .arg("sim.cir")
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|_| RunnerError::NotFound)?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let raw = format!("{stdout}{stderr}");
        let code = output.status.code().unwrap_or(-1);

        // ngspice exits 0 even for some analysis failures, so the waveform file
        // (and known error markers) decide success.
        let text = match tokio::fs::read_to_string(&data_path).await {
            Ok(t) if !t.trim().is_empty() => t,
            _ => {
                return Err(RunnerError::NoWaveform {
                    detail: summarize(&raw, code),
                })
            }
        };

        if let Some(marker) = first_error_marker(&raw) {
            return Err(RunnerError::Failed {
                code,
                detail: format!("{marker}\n{}", summarize(&raw, code)),
            });
        }

        let waveform = Waveform::parse(&text)?;
        Ok(Waveform {
            raw_output: raw,
            ..waveform
        })
    }
}

/// Detect ngspice failure markers in its console output.
fn first_error_marker(output: &str) -> Option<String> {
    const MARKERS: &[&str] = &[
        "Error:",
        "error:",
        "singular matrix",
        "no convergence",
        "doAnalyses:",
        "no such device",
        "unknown device",
        "aborted",
    ];
    output
        .lines()
        .find(|l| MARKERS.iter().any(|m| l.contains(m)))
        .map(|l| l.trim().to_string())
}

/// Keep a bounded, useful slice of ngspice's output for diagnostics.
fn summarize(output: &str, code: i32) -> String {
    let interesting: Vec<&str> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("error") || l.contains("warning") || l.contains("convergence")
                || l.contains("singular") || l.contains("fail")
        })
        .take(8)
        .collect();
    if interesting.is_empty() {
        let tail: Vec<&str> = output
            .lines()
            .filter(|l| !l.trim().is_empty())
            .rev()
            .take(5)
            .collect();
        format!("退出码 {code}；输出尾部：{}", tail.join(" | "))
    } else {
        format!("退出码 {code}；{}", interesting.join(" | "))
    }
}

/// Waveform data as returned by `wrdata` with `wr_singlescale` + `wr_vecnames`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Waveform {
    /// Column names exactly as ngspice wrote them.
    pub columns: Vec<String>,
    /// Row-major values; every row has `columns.len()` entries.
    pub data: Vec<Vec<f64>>,
    /// ngspice console output, kept for diagnostics.
    pub raw_output: String,
}

impl Waveform {
    /// Parse the `wrdata` text format.
    pub fn parse(text: &str) -> Result<Self, RunnerError> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header = lines
            .next()
            .ok_or_else(|| RunnerError::Parse("empty waveform file".into()))?;
        let columns: Vec<String> = header.split_whitespace().map(str::to_string).collect();
        if columns.is_empty() {
            return Err(RunnerError::Parse("waveform header has no columns".into()));
        }
        let mut data = Vec::new();
        for (i, line) in lines.enumerate() {
            let values: Vec<f64> = line
                .split_whitespace()
                .map(|t| {
                    t.parse::<f64>()
                        .map_err(|_| RunnerError::Parse(format!("row {}: bad number {t:?}", i + 1)))
                })
                .collect::<Result<_, _>>()?;
            if values.len() != columns.len() {
                return Err(RunnerError::Parse(format!(
                    "row {}: expected {} values, got {}",
                    i + 1,
                    columns.len(),
                    values.len()
                )));
            }
            data.push(values);
        }
        if data.is_empty() {
            return Err(RunnerError::Parse("waveform has a header but no rows".into()));
        }
        Ok(Self {
            columns,
            data,
            raw_output: String::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Index of a column by its ngspice name (exact match).
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name)
    }

    /// All values of a named column.
    pub fn column(&self, name: &str) -> Option<Vec<f64>> {
        let idx = self.column_index(name)?;
        Some(self.data.iter().map(|r| r[idx]).collect())
    }

    /// Name of the independent variable: the first column that is not a probe.
    ///
    /// ngspice writes the scale first (`frequency`, `time`, or a placeholder for
    /// `.op`), so the caller can rely on this being column 0.
    pub fn scale_name(&self) -> Option<&str> {
        self.columns.first().map(String::as_str)
    }

    /// The independent variable's values.
    pub fn scale(&self) -> Option<Vec<f64>> {
        self.column(self.scale_name()?)
    }

    /// A named column's final value (convenient for `.op`).
    pub fn last_value(&self, name: &str) -> Option<f64> {
        let idx = self.column_index(name)?;
        self.data.last().map(|r| r[idx])
    }

    /// Map of column name -> last value.
    pub fn final_values(&self) -> BTreeMap<String, f64> {
        let mut out = BTreeMap::new();
        if let Some(last) = self.data.last() {
            for (i, c) in self.columns.iter().enumerate() {
                out.insert(c.clone(), last[i]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ac_waveform() {
        let text = " frequency       vm(OUT)         vp(OUT)        \n\
                     1.00000000e+02  9.98031905e-01 -6.27493650e-02 \n\
                     3.16227766e+02  9.80826659e-01 -1.96137326e-01 \n";
        let w = Waveform::parse(text).unwrap();
        assert_eq!(w.columns, vec!["frequency", "vm(OUT)", "vp(OUT)"]);
        assert_eq!(w.len(), 2);
        assert_eq!(w.scale_name(), Some("frequency"));
        let f = w.column("frequency").unwrap();
        assert!((f[0] - 100.0).abs() < 1e-9);
        let m = w.column("vm(OUT)").unwrap();
        assert!((m[1] - 0.980826659).abs() < 1e-9);
    }

    #[test]
    fn parses_op_waveform_with_placeholder_scale() {
        // Real ngspice .op output: the first column is an unnamed scale slot.
        let text = " in              v(out)          v(in)          \n\
                     1.20000000e+01  2.44897959e+00  1.20000000e+01 \n";
        let w = Waveform::parse(text).unwrap();
        assert_eq!(w.columns, vec!["in", "v(out)", "v(in)"]);
        assert_eq!(w.last_value("v(out)"), Some(2.44897959));
        assert_eq!(w.last_value("v(in)"), Some(12.0));
        assert_eq!(w.scale_name(), Some("in"));
    }

    #[test]
    fn rejects_ragged_rows() {
        let text = "time v(out)\n0.0 1.0\n0.1\n";
        assert!(matches!(Waveform::parse(text), Err(RunnerError::Parse(_))));
    }

    #[test]
    fn rejects_empty_waveform() {
        assert!(matches!(Waveform::parse(""), Err(RunnerError::Parse(_))));
        assert!(matches!(
            Waveform::parse("time v(out)\n"),
            Err(RunnerError::Parse(_))
        ));
    }

    #[test]
    fn version_parsing() {
        assert_eq!(
            parse_major("******\n** ngspice-42 : Circuit level simulation program\n"),
            Some(42)
        );
        assert_eq!(parse_major("ngspice-30"), Some(30));
        assert_eq!(parse_major("no version here"), None);
    }

    #[test]
    fn error_markers_are_detected() {
        assert!(first_error_marker("Doing analysis...\nError: no such device\n").is_some());
        assert!(first_error_marker("singular matrix\n").is_some());
        assert!(first_error_marker("No. of Data Rows : 41\n").is_none());
    }

    #[tokio::test]
    async fn probe_reports_missing_binary_clearly() {
        // No environment mutation: probe an explicit bogus path.
        let err = Ngspice::probe_at(Path::new("/nonexistent/ngspice-binary"))
            .await
            .unwrap_err();
        assert!(matches!(err, RunnerError::NotFound));
        assert!(err.to_string().contains(NGSPICE_ENV));
    }
}
