//! Test report IR, matching PRD §7.3.5.
//!
//! The required shape is `plan_id` / `items[step, target, expected{value,tol},
//! measured, unit, verdict, analysis?]` / `summary`. Provenance is added as
//! *extra* fields (source, synthetic, deviation) so a reader can always tell
//! whether a number came from a real instrument or a synthetic source.

use crate::compare::{compare_all, Comparison, TolerancePolicy};
use crate::hal::Measurement;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The `expected` object of a report item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedValue {
    pub value: f64,
    /// Absolute tolerance in `unit` (PRD writes it this way).
    pub tol: f64,
    /// Relative tolerance in percent, kept for traceability.
    pub tol_pct: f64,
    pub unit: String,
    /// Human description of where the baseline came from.
    pub source: String,
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportItem {
    pub step: usize,
    pub target: String,
    pub expected: ExpectedValue,
    pub measured: Option<f64>,
    pub unit: String,
    pub verdict: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis: Option<String>,
    // ---- extras beyond the PRD schema ----
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deviation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deviation_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurement_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    pub tolerance_pct: f64,
    pub tolerance_source: String,
}

impl From<&Comparison> for ReportItem {
    fn from(c: &Comparison) -> Self {
        let tol = c.expected.abs() * c.tolerance_pct / 100.0;
        Self {
            step: c.step,
            target: c.target.clone(),
            expected: ExpectedValue {
                value: c.expected,
                tol,
                tol_pct: c.tolerance_pct,
                unit: c.unit.clone(),
                source: c.baseline.clone(),
            },
            measured: c.measured,
            unit: c.unit.clone(),
            verdict: c.verdict.as_str().to_string(),
            analysis: c.analysis.clone(),
            deviation: c.deviation_abs,
            deviation_pct: c.deviation_pct,
            measurement_source: c.measurement_source.clone(),
            synthetic: c.synthetic,
            tolerance_pct: c.tolerance_pct,
            tolerance_source: c.tolerance_source.describe().to_string(),
        }
    }
}

/// A complete test report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestReport {
    pub plan_id: String,
    pub items: Vec<ReportItem>,
    pub summary: String,
    // ---- extras beyond the PRD schema ----
    pub passed: usize,
    pub failed: usize,
    pub rejected: usize,
    pub not_comparable: usize,
    pub overall: String,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl TestReport {
    /// Build a report from comparisons plus run events.
    pub fn build(
        plan_id: &str,
        comparisons: &[Comparison],
        events: Vec<String>,
        notes: Vec<String>,
    ) -> Self {
        let items: Vec<ReportItem> = comparisons.iter().map(ReportItem::from).collect();
        let passed = items.iter().filter(|i| i.verdict == "PASS").count();
        let failed = items.iter().filter(|i| i.verdict == "FAIL").count();
        let rejected = items.iter().filter(|i| i.verdict == "REJECTED").count();
        let not_comparable = items.iter().filter(|i| i.verdict == "NOT_COMPARABLE").count();
        let total = items.len();
        let overall = if total == 0 {
            "未执行"
        } else if passed == total {
            "PASS"
        } else {
            "FAIL"
        };
        let mut summary = format!("{passed}/{total} PASS");
        if failed > 0 {
            summary.push_str(&format!("，{failed} FAIL 已归因"));
        }
        if rejected > 0 {
            summary.push_str(&format!("，{rejected} 被安全策略拒绝"));
        }
        if not_comparable > 0 {
            summary.push_str(&format!("，{not_comparable} 不可比"));
        }

        Self {
            plan_id: plan_id.into(),
            items,
            summary,
            passed,
            failed,
            rejected,
            not_comparable,
            overall: overall.into(),
            events,
            notes,
        }
    }

    /// Convenience: compare and build in one step.
    pub fn from_run(
        plan: &crate::plan::TestPlan,
        measurements: &BTreeMap<usize, Measurement>,
        rejected: &BTreeMap<usize, String>,
        policy: &TolerancePolicy,
        events: Vec<String>,
    ) -> Self {
        let comparisons = compare_all(plan, measurements, rejected, policy);
        Self::build(&plan.plan_id, &comparisons, events, plan.notes.clone())
    }

    /// Whether any item is synthetic (i.e. not a real board measurement).
    pub fn any_synthetic(&self) -> bool {
        self.items.iter().any(|i| i.synthetic == Some(true))
    }

    /// Distinct measurement sources seen in this report.
    pub fn sources(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .items
            .iter()
            .filter_map(|i| i.measurement_source.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Chinese, human-readable rendering.
    pub fn render(&self) -> String {
        let mut out = format!(
            "测试报告 {}：{}（{}）\n",
            self.plan_id, self.overall, self.summary
        );
        for i in &self.items {
            out.push_str(&format!(
                "\n- [{}] 步骤 {} · {}\n    实测 {} {}\n    预期 {} {}（±{}%，{}）\n    依据 {}",
                i.verdict,
                i.step,
                i.target,
                i.measured
                    .map(|m| fmt(m))
                    .unwrap_or_else(|| "—".into()),
                i.unit,
                fmt(i.expected.value),
                i.unit,
                fmt(i.tolerance_pct),
                i.tolerance_source,
                i.expected.source
            ));
            if let Some(d) = i.deviation_pct {
                out.push_str(&format!("\n    偏差 {d:+.2}%"));
            } else if let Some(d) = i.deviation {
                out.push_str(&format!("\n    偏差 {} {}", fmt(d), i.unit));
            }
            if let Some(src) = &i.measurement_source {
                out.push_str(&format!("\n    数据来源 {src}"));
            }
            if let Some(a) = &i.analysis {
                out.push_str(&format!("\n    说明 {a}"));
            }
        }
        if !self.events.is_empty() {
            out.push_str("\n事件：\n");
            for e in &self.events {
                out.push_str(&format!("- {e}\n"));
            }
        }
        if !self.notes.is_empty() {
            out.push_str("\n计划说明：\n");
            for n in &self.notes {
                out.push_str(&format!("- {n}\n"));
            }
        }
        if self.any_synthetic() {
            out.push_str(
                "\n⚠️ 本报告包含**合成数据**（虚拟仪器或 sigrok demo 设备），\
                 不能作为真实板卡的测量结论。\n",
            );
        }
        out
    }

    /// Write JSON and the readable text next to each other.
    pub fn write(&self, dir: &Path, stem: &str) -> std::io::Result<(std::path::PathBuf, std::path::PathBuf)> {
        std::fs::create_dir_all(dir)?;
        let json = dir.join(format!("{stem}.report.json"));
        let txt = dir.join(format!("{stem}.report.txt"));
        std::fs::write(&json, self.to_json())?;
        std::fs::write(&txt, self.render())?;
        Ok((json, txt))
    }
}

fn fmt(v: f64) -> String {
    if !v.is_finite() {
        return "n/a".into();
    }
    let a = v.abs();
    if a != 0.0 && (a < 1e-3 || a >= 1e6) {
        return format!("{v:.4e}");
    }
    let mut s = format!("{v:.6}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::TolerancePolicy;
    use crate::hal::Quantity;
    use crate::plan::{Expected, TestPlan, TestStep};

    fn plan() -> TestPlan {
        TestPlan::new(
            "tp-001",
            vec![
                TestStep::new(
                    1,
                    "3V3 rail",
                    Quantity::VoltageDc,
                    Expected::from_user(3.3, 1.0, "V", "用户要求"),
                ),
                TestStep::new(
                    2,
                    "LED PWM freq",
                    Quantity::Frequency,
                    Expected::from_user(1000.0, 5.0, "Hz", "用户要求"),
                ),
                TestStep::new(
                    3,
                    "opamp gain",
                    Quantity::VoltageDc,
                    Expected::from_user(10.0, 0.5, "V", "用户要求"),
                ),
            ],
        )
        .unwrap()
    }

    fn report() -> TestReport {
        let p = plan();
        let mut measurements = BTreeMap::new();
        measurements.insert(
            1,
            Measurement::new(Quantity::VoltageDc, 3.31, "virtual", "bench", true),
        );
        measurements.insert(
            2,
            Measurement::new(Quantity::Frequency, 987.0, "virtual", "bench", true),
        );
        measurements.insert(
            3,
            Measurement::new(Quantity::VoltageDc, 6.2, "virtual", "bench", true),
        );
        TestReport::from_run(
            &p,
            &measurements,
            &BTreeMap::new(),
            &TolerancePolicy::kr2_default(),
            vec!["已上电 5V".into()],
        )
    }

    #[test]
    fn report_matches_the_prd_item_shape() {
        let r = report();
        assert_eq!(r.plan_id, "tp-001");
        let json: serde_json::Value = serde_json::from_str(&r.to_json()).unwrap();
        let item = &json["items"][0];
        assert!(item["step"].is_number());
        assert!(item["target"].is_string());
        assert!(item["expected"]["value"].is_number());
        assert!(item["expected"]["tol"].is_number());
        assert!(item["measured"].is_number());
        assert!(item["unit"].is_string());
        assert!(item["verdict"].is_string());
        assert!(json["summary"].is_string());
    }

    #[test]
    fn passing_item_has_no_analysis_and_failing_item_does() {
        let r = report();
        assert_eq!(r.items[0].verdict, "PASS");
        assert!(r.items[0].analysis.is_none());
        assert_eq!(r.items[2].verdict, "FAIL");
        assert!(r.items[2].analysis.is_some());
    }

    #[test]
    fn summary_counts_passes_and_failures() {
        let r = report();
        assert_eq!(r.passed, 2);
        assert_eq!(r.failed, 1);
        assert_eq!(r.overall, "FAIL");
        assert!(r.summary.contains("2/3 PASS"));
        assert!(r.summary.contains("1 FAIL"));
    }

    #[test]
    fn tolerance_source_is_recorded_per_item() {
        let r = report();
        assert_eq!(r.items[0].tolerance_source, "步骤自带容差");
        assert!((r.items[0].expected.tol - 0.033).abs() < 1e-9);
    }

    #[test]
    fn synthetic_data_is_surfaced_prominently() {
        let r = report();
        assert!(r.any_synthetic());
        let text = r.render();
        assert!(text.contains("合成数据"));
        assert!(text.contains("不能作为真实板卡的测量结论"));
        assert!(r.sources().iter().any(|s| s.contains("非真实")));
    }

    #[test]
    fn render_includes_the_key_numbers() {
        let text = report().render();
        assert!(text.contains("3V3 rail"));
        assert!(text.contains("3.31"));
        assert!(text.contains("LED PWM freq"));
        assert!(text.contains("987"));
        assert!(text.contains("已上电 5V"));
    }

    #[test]
    fn rendering_and_json_are_deterministic() {
        assert_eq!(report().render(), report().render());
        assert_eq!(report().to_json(), report().to_json());
    }

    #[test]
    fn rejected_items_count_separately() {
        let p = plan();
        let mut rejected = BTreeMap::new();
        rejected.insert(1, "已拒绝：未启用源输出".to_string());
        let r = TestReport::from_run(&p, &BTreeMap::new(), &rejected, &TolerancePolicy::kr2_default(), vec![]);
        assert_eq!(r.rejected, 1);
        assert_eq!(r.not_comparable, 2);
        assert_eq!(r.passed, 0);
        assert!(r.summary.contains("被安全策略拒绝"));
    }
}
