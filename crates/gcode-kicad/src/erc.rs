//! ERC bridge: run `kicad-cli sch erc`, parse the JSON report, interpret it in
//! plain language, and drive the generate → check → revise → regenerate loop.

use crate::netlist::NetlistIr;
use crate::schematic::{generate_with_report, Layout};
use serde::Deserialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

/// Environment override for the `kicad-cli` executable.
pub const KICAD_CLI_ENV: &str = "GCODE_KICAD_CLI";
/// Environment override for the minimum acceptable KiCad version.
pub const MIN_VERSION_ENV: &str = "GCODE_KICAD_MIN_VERSION";
/// Minimum KiCad version ERC is allowed on (PRD targets KiCad 10).
pub const DEFAULT_MIN_VERSION: &str = "10.0.0";

#[derive(Debug, thiserror::Error)]
pub enum ErcError {
    #[error("kicad-cli not found (install kicad or set {KICAD_CLI_ENV})")]
    CliNotFound,
    #[error("kicad-cli exited with {code}: {stderr}")]
    CliFailed { code: i32, stderr: String },
    #[error("failed to parse ERC report: {0}")]
    Parse(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ErcReport {
    #[serde(default)]
    pub violations: Vec<ErcViolation>,
    #[serde(default)]
    pub unconnected_items: Vec<serde_json::Value>,
    #[serde(default)]
    pub schematic_parity: Vec<serde_json::Value>,
    /// Present in real kicad-cli output, e.g. "8.0.1".
    #[serde(default)]
    pub kicad_version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ErcViolation {
    #[serde(default)]
    pub severity: String,
    /// KiCad writes the key as `type`; the field is `type_` in Rust.
    #[serde(default, rename = "type")]
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
        self.violations
            .iter()
            .filter(|v| v.severity == "error")
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity != "error")
            .count()
    }

    /// Count of violations that matter for the verdict (errors only).
    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    pub fn verdict(&self) -> &'static str {
        if self.is_clean() {
            "PASS"
        } else {
            "FAIL"
        }
    }

    pub fn summary(&self) -> String {
        if self.is_clean() {
            return "ERC: PASS (0 issues)".into();
        }
        format!(
            "ERC: {} ({} errors, {} warnings, {} unconnected, {} parity)",
            self.verdict(),
            self.error_count(),
            self.warning_count(),
            self.unconnected_items.len(),
            self.schematic_parity.len()
        )
    }

    /// Machine-readable, stable summary used by tools and tests.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "verdict": self.verdict(),
            "errors": self.error_count(),
            "warnings": self.warning_count(),
            "unconnected": self.unconnected_items.len(),
            "parity": self.schematic_parity.len(),
            "violations": self.violations.iter().map(|v| serde_json::json!({
                "severity": v.severity,
                "type": v.type_,
                "description": v.description,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Locate the kicad-cli binary (overridable via `GCODE_KICAD_CLI`).
pub fn kicad_cli_path() -> PathBuf {
    std::env::var_os(KICAD_CLI_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("kicad-cli"))
}

/// A probed `kicad-cli` installation.
#[derive(Debug, Clone, PartialEq)]
pub struct KicadToolchain {
    pub path: PathBuf,
    pub version: String,
    pub major: u32,
    pub minor: u32,
    pub required: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolchainError {
    #[error("kicad-cli 未找到：请安装 KiCad（或设置 {KICAD_CLI_ENV} 指向可执行文件）")]
    NotFound,
    #[error("无法读取 kicad-cli 版本（输出：{0:?}）")]
    VersionUnreadable(String),
    #[error("kicad-cli 版本过低：当前 {found}，ERC 需要 >= {required}。请升级 KiCad（或设置 {MIN_VERSION_ENV} 覆盖最低版本）")]
    TooOld { found: String, required: String },
}

fn parse_version(s: &str) -> Option<(u32, u32)> {
    let first = s.split_whitespace().last().unwrap_or(s);
    let mut it = first.trim().split('.');
    let major = it.next()?.trim().parse::<u32>().ok()?;
    let minor = it.next().unwrap_or("0").trim().parse::<u32>().unwrap_or(0);
    Some((major, minor))
}

impl KicadToolchain {
    /// Probe using the default minimum version.
    pub async fn probe() -> Result<Self, ToolchainError> {
        let required =
            std::env::var(MIN_VERSION_ENV).unwrap_or_else(|_| DEFAULT_MIN_VERSION.to_string());
        Self::probe_with(&required).await
    }

    /// Probe and enforce a specific minimum version.
    pub async fn probe_with(required: &str) -> Result<Self, ToolchainError> {
        let path = kicad_cli_path();
        let output = Command::new(&path)
            .arg("version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ToolchainError::NotFound,
                _ => ToolchainError::NotFound,
            })?;
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let (major, minor) =
            parse_version(&raw).ok_or_else(|| ToolchainError::VersionUnreadable(raw.clone()))?;
        let tc = KicadToolchain {
            path,
            version: raw,
            major,
            minor,
            required: required.to_string(),
        };
        tc.ensure_at_least(required)?;
        Ok(tc)
    }

    /// Enforce a minimum version.
    pub fn ensure_at_least(&self, required: &str) -> Result<(), ToolchainError> {
        let (r_major, r_minor) =
            parse_version(required).unwrap_or((DEFAULT_MIN_VERSION_PARTS.0, DEFAULT_MIN_VERSION_PARTS.1));
        if (self.major, self.minor) < (r_major, r_minor) {
            return Err(ToolchainError::TooOld {
                found: self.version.clone(),
                required: required.to_string(),
            });
        }
        Ok(())
    }

    /// Gate for running ERC.
    pub fn ensure_erc_allowed(&self) -> Result<(), ToolchainError> {
        self.ensure_at_least(&self.required)
    }
}

const DEFAULT_MIN_VERSION_PARTS: (u32, u32) = (10, 0);

/// Run ERC on a `.kicad_sch`, writing a JSON report next to it.
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
    let report: ErcReport =
        serde_json::from_slice(&raw).map_err(|e| ErcError::Parse(e.to_string()))?;
    Ok(report)
}

/// Parse the raw JSON text of an ERC report (for tests and cached reports).
pub fn parse_report(json: &str) -> Result<ErcReport, ErcError> {
    serde_json::from_str(json).map_err(|e| ErcError::Parse(e.to_string()))
}

/// Round-trip helper: read a `.kicad_sch` back into `Sexpr` (used by the agent
/// loop to inspect what was actually generated).
pub fn read_schematic(schematic: &Path) -> Result<crate::sexpr::Sexpr, crate::sexpr::ParseError> {
    let text =
        std::fs::read_to_string(schematic).map_err(|_| crate::sexpr::ParseError::Eof(0))?;
    let docs = crate::sexpr::Sexpr::parse(&text)?;
    docs.into_iter()
        .next()
        .ok_or(crate::sexpr::ParseError::Eof(0))
}

// ---------- interpretation ----------

/// One violation, explained in plain language with an actionable fix.
#[derive(Debug, Clone, PartialEq)]
pub struct InterpretedViolation {
    pub severity: String,
    pub kind: String,
    pub description: String,
    /// Chinese explanation of what this means.
    pub explanation: String,
    /// Concrete next action for the user/agent.
    pub fix: String,
    /// Component/net references gleaned from the report, if any.
    pub involved: Vec<String>,
    /// True when the explanation came from the generic fallback.
    pub unknown_kind: bool,
}

/// Plain-language interpretation of a whole report.
#[derive(Debug, Clone, PartialEq)]
pub struct Interpretation {
    pub verdict: &'static str,
    pub items: Vec<InterpretedViolation>,
    /// Residual high-level notes (unconnected items without a type).
    pub notes: Vec<String>,
}

impl Interpretation {
    /// Human-readable rendering, suitable for the agent's context.
    pub fn render(&self) -> String {
        let mut out = format!(
            "ERC 判定：{} （{} 条问题）\n",
            self.verdict,
            self.items.len()
        );
        for (i, v) in self.items.iter().enumerate() {
            out.push_str(&format!(
                "\n{}. [{}] {} — {}\n   解释：{}\n   建议：{}",
                i + 1,
                v.severity,
                v.kind,
                v.description,
                v.explanation,
                v.fix
            ));
            if !v.involved.is_empty() {
                out.push_str(&format!("\n   涉及：{}", v.involved.join(", ")));
            }
            if v.unknown_kind {
                out.push_str("\n   （未收录的违规类型，以上为通用建议）");
            }
        }
        for n in &self.notes {
            out.push_str(&format!("\n- {n}"));
        }
        out
    }
}

/// Known ERC violation types → (explanation, fix).
///
/// Keys are the `type` strings KiCad writes into the ERC JSON report.
pub const KNOWN_VIOLATIONS: &[(&str, &str, &str)] = &[
    (
        "power_pin_not_driven",
        "某个网络的电源输入引脚没有任何驱动源，KiCad 无法确认该电源轨电压",
        "在该网络加一个 power:PWR_FLAG（power_out 引脚），或把它接到稳压器/电源的输出脚",
    ),
    (
        "input_pin_not_driven",
        "输入引脚没有任何驱动源，信号可能悬空",
        "把该引脚接到信号源、上拉/下拉电阻或输出引脚",
    ),
    (
        "pin_not_connected",
        "有引脚完全没有连接",
        "连接该引脚，或如果确实不用则放置 no-connect 标记",
    ),
    (
        "pin_to_pin",
        "两个引脚电气类型冲突（例如两个输出脚接在一起，或输出接电源）",
        "检查该网络上的引脚连接，移除冲突的驱动源或加限流/隔离",
    ),
    (
        "duplicate_pin_numbers",
        "符号内部有重复的引脚编号",
        "修正符号定义中的引脚编号，或改用正确的库符号",
    ),
    (
        "label_dangling",
        "标签悬空，没有连到任何引脚或导线",
        "把标签移到引脚连接点上，或删除多余标签",
    ),
    (
        "global_label_dangling",
        "全局标签悬空，没有连到任何引脚或导线",
        "把全局标签移到引脚连接点上，或删除多余标签",
    ),
    (
        "wire_dangling",
        "导线端点悬空",
        "把导线连到引脚或另一个导线端点，或删除该导线",
    ),
    (
        "no_connect_dangling",
        "no-connect 标记没有落在任何引脚上",
        "把 no-connect 移到未使用引脚上，或删除它",
    ),
    (
        "unannotated",
        "元件位号未标注（仍是 R? 之类）",
        "给所有元件编号（R1、C1、U1…），位号必须唯一",
    ),
    (
        "missing_unit",
        "多单元符号还有单元没有放置到原理图上",
        "放置剩余单元，或改用单运放等单单元符号",
    ),
    (
        "missing_units",
        "多单元符号还有单元没有放置到原理图上",
        "放置剩余单元，或改用单运放等单单元符号",
    ),
    (
        "lib_symbol_issues",
        "符号定义本身有问题（引脚或图形缺失）",
        "检查库符号定义，或换用标准库中的符号",
    ),
    (
        "endpoint_off_grid",
        "导线/引脚端点没有对齐栅格，容易导致以为连上其实没连",
        "把端点对齐到 1.27mm 栅格",
    ),
    (
        "four_way_junction",
        "四点十字交叉，连接关系有歧义",
        "把交叉点拆成两个 T 型连接，避免十字直接相连",
    ),
    (
        "similar_labels",
        "存在拼写相似的网络标签，可能是打错字",
        "检查这些标签名是否本应是同一个网络",
    ),
    (
        "different_unit_net",
        "同一符号的不同单元被接到了不同网络",
        "确认多单元符号的连接是否符合设计意图",
    ),
    (
        "footprint_link_issues",
        "封装链接有问题（封装库缺失或不匹配）",
        "为该元件指定一个存在的封装，或先忽略（不影响电气正确性）",
    ),
    (
        "simulation_model_issue",
        "仿真模型有问题",
        "检查该元件的 SPICE 模型/仿真参数（R0 不做仿真，可先忽略）",
    ),
    (
        "net_not_bus_member",
        "网络名不是任何总线的成员",
        "检查总线定义或该网络的命名",
    ),
    (
        "unresolved_variable",
        "文本变量无法解析",
        "检查文本中的 ${...} 变量名",
    ),
];

/// Explain one violation.
pub fn interpret_violation(v: &ErcViolation) -> InterpretedViolation {
    let kind = v.type_.clone().unwrap_or_else(|| "unknown".into());
    let involved = collect_involved(v);
    match KNOWN_VIOLATIONS.iter().find(|(k, _, _)| *k == kind) {
        Some((_, explanation, fix)) => InterpretedViolation {
            severity: if v.severity.is_empty() {
                "error".into()
            } else {
                v.severity.clone()
            },
            kind,
            description: v.description.clone(),
            explanation: (*explanation).to_string(),
            fix: (*fix).to_string(),
            involved,
            unknown_kind: false,
        },
        None => InterpretedViolation {
            severity: if v.severity.is_empty() {
                "error".into()
            } else {
                v.severity.clone()
            },
            kind,
            description: v.description.clone(),
            explanation: format!(
                "KiCad 报出的问题：{}（该类型尚未收录到解释表）",
                v.description
            ),
            fix: "先在 KiCad 打开原理图定位高亮处；若涉及电源脚，检查是否需要 PWR_FLAG".into(),
            involved,
            unknown_kind: true,
        },
    }
}

/// Pull component/net references out of a violation's `items` array.
fn collect_involved(v: &ErcViolation) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(s) => {
                if !s.is_empty() && !out.contains(s) {
                    out.push(s.clone());
                }
            }
            serde_json::Value::Object(map) => {
                for k in ["description", "ref", "pin", "pin_name", "uuid"] {
                    if let Some(serde_json::Value::String(s)) = map.get(k) {
                        if !s.is_empty() && !out.contains(s) {
                            out.push(s.clone());
                        }
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for i in items {
                    walk(i, out);
                }
            }
            _ => {}
        }
    }
    walk(&serde_json::Value::Array(v.items.clone()), &mut out);
    out.truncate(6);
    out
}

/// Interpret a full ERC report.
pub fn interpret_report(report: &ErcReport) -> Interpretation {
    let items: Vec<InterpretedViolation> =
        report.violations.iter().map(interpret_violation).collect();
    let mut notes = Vec::new();
    if !report.unconnected_items.is_empty() {
        notes.push(format!(
            "另有 {} 项未连接（unconnected items）：通常是引脚没接线，请补连接或加 no-connect 标记",
            report.unconnected_items.len()
        ));
    }
    if !report.schematic_parity.is_empty() {
        notes.push(format!(
            "另有 {} 项原理图/PCB 一致性差异（schematic parity）",
            report.schematic_parity.len()
        ));
    }
    Interpretation {
        verdict: report.verdict(),
        items,
        notes,
    }
}

// ---------- self-check loop ----------

/// Something that can run ERC on a schematic file.
#[async_trait::async_trait]
pub trait ErcRunner: Send + Sync {
    async fn run(&self, schematic: &Path) -> Result<ErcReport, ErcError>;
}

/// Runner backed by a probed `kicad-cli`.
pub struct KicadCliRunner {
    pub toolchain: KicadToolchain,
}

impl KicadCliRunner {
    pub async fn new() -> Result<Self, ToolchainError> {
        Ok(Self {
            toolchain: KicadToolchain::probe().await?,
        })
    }
}

#[async_trait::async_trait]
impl ErcRunner for KicadCliRunner {
    async fn run(&self, schematic: &Path) -> Result<ErcReport, ErcError> {
        self.toolchain
            .ensure_erc_allowed()
            .map_err(|e| ErcError::CliFailed {
                code: -1,
                stderr: e.to_string(),
            })?;
        run_erc(schematic).await
    }
}

/// Runner returning a scripted sequence of reports (tests).
pub struct ScriptedRunner {
    script: std::sync::Mutex<VecDeque<ErcReport>>,
    pub calls: std::sync::Mutex<usize>,
}

impl ScriptedRunner {
    pub fn new(reports: Vec<ErcReport>) -> Self {
        Self {
            script: std::sync::Mutex::new(reports.into()),
            calls: std::sync::Mutex::new(0),
        }
    }

    pub fn call_count(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl ErcRunner for ScriptedRunner {
    async fn run(&self, _schematic: &Path) -> Result<ErcReport, ErcError> {
        *self.calls.lock().unwrap() += 1;
        Ok(self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default())
    }
}

/// Outcome of a generate → ERC → revise loop.
#[derive(Debug, Clone)]
pub struct SelfCheckOutcome {
    /// Number of ERC runs performed.
    pub rounds: usize,
    pub passed: bool,
    pub final_report: ErcReport,
    /// Remaining problem kinds when the loop gave up.
    pub residual: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SelfCheckError {
    #[error("netlist invalid: {0}")]
    InvalidIr(#[from] crate::netlist::IrError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("erc failed: {0}")]
    Erc(#[from] ErcError),
}

/// Generate, check, revise, regenerate — bounded by `max_rounds`.
///
/// `revise` receives the failing report and the round number; returning a new
/// netlist retries, returning `None` stops the loop. PASS ends it immediately.
pub async fn self_check<R, F>(
    dir: &Path,
    initial: NetlistIr,
    layout: Layout,
    max_rounds: usize,
    runner: &R,
    mut revise: F,
) -> Result<SelfCheckOutcome, SelfCheckError>
where
    R: ErcRunner,
    F: FnMut(&ErcReport, usize) -> Option<NetlistIr>,
{
    let sch = dir.join("schematic.kicad_sch");
    std::fs::create_dir_all(dir)?;
    let mut ir = initial;
    let mut warnings = Vec::new();
    let mut rounds = 0usize;

    loop {
        rounds += 1;
        let report = generate_with_report(&ir, layout)?;
        warnings.extend(report.warnings.clone());
        tokio::fs::write(&sch, &report.content).await?;

        let erc = runner.run(&sch).await?;
        if erc.is_clean() {
            return Ok(SelfCheckOutcome {
                rounds,
                passed: true,
                final_report: erc,
                residual: Vec::new(),
                warnings,
            });
        }
        let residual: Vec<String> = erc
            .violations
            .iter()
            .map(|v| v.type_.clone().unwrap_or_else(|| "unknown".into()))
            .collect();

        if rounds >= max_rounds {
            return Ok(SelfCheckOutcome {
                rounds,
                passed: false,
                final_report: erc,
                residual,
                warnings,
            });
        }
        match revise(&erc, rounds) {
            Some(next) => ir = next,
            None => {
                return Ok(SelfCheckOutcome {
                    rounds,
                    passed: false,
                    final_report: erc,
                    residual,
                    warnings,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clean_report() {
        let report =
            parse_report(r#"{"violations":[],"unconnected_items":[],"schematic_parity":[]}"#)
                .unwrap();
        assert!(report.is_clean());
        assert!(report.summary().contains("PASS"));
        assert_eq!(report.verdict(), "PASS");
    }

    #[test]
    fn parse_violation_report_reads_the_type_key() {
        // Real kicad-cli writes "type" (not "type_"); regression guard.
        let json = r#"{
            "kicad_version": "8.0.1",
            "violations": [
                {"severity": "error", "type": "power_pin_not_driven",
                 "description": "Input power pin not driven by any Output Power pins",
                 "items": [{"description": "Symbol #PWR01 [GND]"}]},
                {"severity": "warning", "type": "unannotated", "description": "x", "items": []}
            ],
            "unconnected_items": [{"a": 1}]
        }"#;
        let report = parse_report(json).unwrap();
        assert!(!report.is_clean());
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.warning_count(), 1);
        assert_eq!(report.violations[0].type_.as_deref(), Some("power_pin_not_driven"));
        assert_eq!(report.kicad_version.as_deref(), Some("8.0.1"));
        assert_eq!(report.verdict(), "FAIL");
    }

    #[test]
    fn interprets_known_violation() {
        let v = ErcViolation {
            severity: "error".into(),
            type_: Some("power_pin_not_driven".into()),
            description: "Input power pin not driven".into(),
            items: vec![serde_json::json!({"description": "Symbol #PWR01 [GND] Pin 1"})],
        };
        let i = interpret_violation(&v);
        assert!(!i.unknown_kind);
        assert!(i.fix.contains("PWR_FLAG"));
        assert!(i.explanation.contains("电源"));
        assert!(i.involved.iter().any(|s| s.contains("#PWR01")));
    }

    #[test]
    fn interprets_unknown_violation_with_fallback() {
        let v = ErcViolation {
            severity: "warning".into(),
            type_: Some("brand_new_check".into()),
            description: "something new".into(),
            items: vec![],
        };
        let i = interpret_violation(&v);
        assert!(i.unknown_kind);
        assert!(i.explanation.contains("something new"));
        assert!(!i.fix.is_empty());
    }

    #[test]
    fn violation_without_type_is_handled() {
        let v = ErcViolation {
            severity: "error".into(),
            type_: None,
            description: "no type".into(),
            items: vec![],
        };
        let i = interpret_violation(&v);
        assert_eq!(i.kind, "unknown");
        assert!(i.unknown_kind);
    }

    #[test]
    fn report_renders_all_items() {
        let report = ErcReport {
            violations: vec![
                ErcViolation {
                    severity: "error".into(),
                    type_: Some("power_pin_not_driven".into()),
                    description: "a".into(),
                    items: vec![],
                },
                ErcViolation {
                    severity: "warning".into(),
                    type_: Some("footprint_link_issues".into()),
                    description: "b".into(),
                    items: vec![],
                },
            ],
            unconnected_items: vec![serde_json::json!({})],
            ..Default::default()
        };
        let text = interpret_report(&report).render();
        assert!(text.contains("FAIL"));
        assert!(text.contains("PWR_FLAG"));
        assert!(text.contains("未连接"));
    }

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("7.0.11"), Some((7, 0)));
        assert_eq!(parse_version("8.0.1"), Some((8, 0)));
        assert_eq!(parse_version("kicad-cli 10.0.4"), Some((10, 0)));
        assert_eq!(parse_version("10"), Some((10, 0)));
        assert_eq!(parse_version("garbage"), None);
    }

    #[test]
    fn version_gate_rejects_old_toolchain() {
        let tc = KicadToolchain {
            path: PathBuf::from("kicad-cli"),
            version: "7.0.11".into(),
            major: 7,
            minor: 0,
            required: "10.0.0".into(),
        };
        let err = tc.ensure_erc_allowed().unwrap_err();
        assert!(matches!(err, ToolchainError::TooOld { .. }));
        assert!(err.to_string().contains("7.0.11"));
        assert!(err.to_string().contains("10.0.0"));
    }

    #[test]
    fn version_gate_accepts_current_toolchain() {
        let tc = KicadToolchain {
            path: PathBuf::from("kicad-cli"),
            version: "10.0.4".into(),
            major: 10,
            minor: 0,
            required: "10.0.0".into(),
        };
        assert!(tc.ensure_erc_allowed().is_ok());
    }

    fn failing_then_clean() -> Vec<ErcReport> {
        vec![
            ErcReport {
                violations: vec![ErcViolation {
                    severity: "error".into(),
                    type_: Some("power_pin_not_driven".into()),
                    description: "not driven".into(),
                    items: vec![],
                }],
                ..Default::default()
            },
            ErcReport::default(),
        ]
    }

    #[tokio::test]
    async fn self_check_passes_after_one_revision() {
        let dir = std::env::temp_dir().join(format!("gcode-selfcheck-{}", std::process::id()));
        let runner = ScriptedRunner::new(failing_then_clean());
        let ir = crate::catalog::by_id("voltage-divider").unwrap().ir;
        let mut revisions = 0;
        let out = self_check(&dir, ir, Layout::default(), 5, &runner, |_, _| {
            revisions += 1;
            Some(crate::catalog::by_id("voltage-divider").unwrap().ir)
        })
        .await
        .unwrap();
        assert!(out.passed);
        assert_eq!(out.rounds, 2, "one failure then one pass");
        assert_eq!(revisions, 1);
        assert_eq!(runner.call_count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn self_check_stops_at_the_round_limit() {
        let dir = std::env::temp_dir().join(format!("gcode-selfcheck-lim-{}", std::process::id()));
        let mut script = Vec::new();
        for _ in 0..10 {
            script.push(ErcReport {
                violations: vec![ErcViolation {
                    severity: "error".into(),
                    type_: Some("pin_not_connected".into()),
                    description: "x".into(),
                    items: vec![],
                }],
                ..Default::default()
            });
        }
        let runner = ScriptedRunner::new(script);
        let ir = crate::catalog::by_id("voltage-divider").unwrap().ir;
        let out = self_check(&dir, ir, Layout::default(), 3, &runner, |_, _| {
            Some(crate::catalog::by_id("voltage-divider").unwrap().ir)
        })
        .await
        .unwrap();
        assert!(!out.passed);
        assert_eq!(out.rounds, 3, "must stop at the limit");
        assert!(!out.residual.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn self_check_stops_when_reviser_gives_up() {
        let dir = std::env::temp_dir().join(format!("gcode-selfcheck-null-{}", std::process::id()));
        let runner = ScriptedRunner::new(failing_then_clean());
        let ir = crate::catalog::by_id("voltage-divider").unwrap().ir;
        let out = self_check(&dir, ir, Layout::default(), 5, &runner, |_, _| None)
            .await
            .unwrap();
        assert!(!out.passed);
        assert_eq!(out.rounds, 1);
        assert_eq!(runner.call_count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
