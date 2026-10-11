//! Comparing a measurement against its baseline, with the PRD's KR2 thresholds.
//!
//! KR2 (PRD §4): DC operating point within 10%, AC peak frequency within 20%.
//! Those are the defaults; a caller may override them, and the output always says
//! which tolerance was actually applied (see `design.md` D5).

use crate::hal::{Measurement, Quantity};
use crate::plan::{TestPlan, TestStep};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Default DC tolerance from KR2 (percent).
pub const KR2_DC_PCT: f64 = 10.0;
/// Default AC peak-frequency tolerance from KR2 (percent).
pub const KR2_AC_FREQ_PCT: f64 = 20.0;

/// Which class of quantity a step belongs to, for tolerance selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantityClass {
    /// DC levels: voltages and currents.
    Dc,
    /// Frequencies (the "AC peak frequency" of KR2).
    AcFrequency,
    /// Anything else.
    Other,
}

/// Classify a quantity for tolerance purposes.
pub fn classify(q: Quantity) -> QuantityClass {
    match q {
        Quantity::VoltageDc | Quantity::CurrentDc | Quantity::Resistance
        | Quantity::Continuity => QuantityClass::Dc,
        Quantity::Frequency => QuantityClass::AcFrequency,
    }
}

/// Where the tolerance that judged a step came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToleranceSource {
    /// The PRD's KR2 default for the quantity class.
    Kr2Default,
    /// An explicit override supplied by the caller.
    Override,
    /// The plan step stated its own tolerance.
    StepTolerance,
}

impl ToleranceSource {
    pub fn describe(self) -> &'static str {
        match self {
            ToleranceSource::Kr2Default => "默认 KR2 阈值",
            ToleranceSource::Override => "覆盖阈值",
            ToleranceSource::StepTolerance => "步骤自带容差",
        }
    }
}

/// The tolerance policy applied to a run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TolerancePolicy {
    pub dc_pct: f64,
    pub ac_freq_pct: f64,
    pub source: ToleranceSource,
}

impl Default for TolerancePolicy {
    fn default() -> Self {
        Self::kr2_default()
    }
}

impl TolerancePolicy {
    /// The KR2 defaults.
    pub fn kr2_default() -> Self {
        Self {
            dc_pct: KR2_DC_PCT,
            ac_freq_pct: KR2_AC_FREQ_PCT,
            source: ToleranceSource::Kr2Default,
        }
    }

    /// An explicit override (recorded as such).
    pub fn override_with(dc_pct: f64, ac_freq_pct: f64) -> Self {
        Self {
            dc_pct,
            ac_freq_pct,
            source: ToleranceSource::Override,
        }
    }

    /// Tolerance for a class, ignoring any step-level tolerance.
    pub fn for_class(&self, class: QuantityClass) -> f64 {
        match class {
            QuantityClass::Dc | QuantityClass::Other => self.dc_pct,
            QuantityClass::AcFrequency => self.ac_freq_pct,
        }
    }

    /// Resolve the tolerance for a step: an explicit step tolerance wins,
    /// otherwise the policy's class threshold applies.
    pub fn resolve(&self, step: &TestStep) -> (f64, ToleranceSource) {
        if step.expected.tol_pct > 0.0 {
            (step.expected.tol_pct, ToleranceSource::StepTolerance)
        } else {
            (self.for_class(classify(step.quantity)), self.source)
        }
    }
}

/// The outcome for one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CompareVerdict {
    Pass,
    Fail,
    /// No baseline or no measurement: not counted as a pass.
    NotComparable,
    /// Refused by a safety gate (or a missing instrument).
    Rejected,
}

impl CompareVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompareVerdict::Pass => "PASS",
            CompareVerdict::Fail => "FAIL",
            CompareVerdict::NotComparable => "NOT_COMPARABLE",
            CompareVerdict::Rejected => "REJECTED",
        }
    }
}

/// One step's comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub step: usize,
    pub target: String,
    pub quantity: Quantity,
    pub expected: f64,
    pub measured: Option<f64>,
    pub unit: String,
    pub tolerance_pct: f64,
    pub tolerance_source: ToleranceSource,
    pub deviation_abs: Option<f64>,
    pub deviation_pct: Option<f64>,
    pub verdict: CompareVerdict,
    /// Why it failed / was rejected / could not be compared.
    pub analysis: Option<String>,
    /// Baseline provenance (simulation or user).
    pub baseline: String,
    /// Measurement provenance (backend/device), including synthetic marking.
    pub measurement_source: Option<String>,
    pub synthetic: Option<bool>,
}

/// Tolerance check on a relative deviation.
fn within(deviation_pct: f64, tolerance_pct: f64) -> bool {
    deviation_pct.is_finite() && deviation_pct.abs() <= tolerance_pct
}

/// Compare one step against its measurement.
pub fn compare(
    step: &TestStep,
    measured: Option<&Measurement>,
    policy: &TolerancePolicy,
) -> Comparison {
    let (tol, tol_source) = policy.resolve(step);
    let baseline = step.expected.source.describe();
    let mut out = Comparison {
        step: step.index,
        target: step.target.clone(),
        quantity: step.quantity,
        expected: step.expected.value,
        measured: None,
        unit: if step.expected.unit.is_empty() {
            step.quantity.unit().to_string()
        } else {
            step.expected.unit.clone()
        },
        tolerance_pct: tol,
        tolerance_source: tol_source,
        deviation_abs: None,
        deviation_pct: None,
        verdict: CompareVerdict::NotComparable,
        analysis: None,
        baseline,
        measurement_source: None,
        synthetic: None,
    };

    let Some(m) = measured else {
        out.analysis = Some(format!(
            "没有取得 {} 的实测值，无法判定（预期 {} {}，依据 {}）。",
            step.target, step.expected.value, out.unit, out.baseline
        ));
        return out;
    };

    out.measured = Some(m.value);
    out.measurement_source = Some(m.source_label());
    out.synthetic = Some(m.synthetic);

    let abs = m.value - step.expected.value;
    out.deviation_abs = Some(abs);
    if step.expected.value != 0.0 {
        let pct = abs / step.expected.value * 100.0;
        out.deviation_pct = Some(pct);
        if within(pct, tol) {
            out.verdict = CompareVerdict::Pass;
        } else {
            out.verdict = CompareVerdict::Fail;
            out.analysis = Some(fail_attribution(
                &step.target,
                m.value,
                step.expected.value,
                pct,
                tol,
                &out.unit,
                m.synthetic,
            ));
        }
    } else {
        // A zero baseline cannot be judged with a *relative* tolerance. Rather
        // than inventing an absolute threshold, report the absolute deviation and
        // refuse to call it pass or fail.
        out.verdict = CompareVerdict::NotComparable;
        out.analysis = Some(format!(
            "预期基准为 0，实测 {} {}（绝对偏差 {} {}）。基准为零时相对容差没有意义，\
             因此不判定；请提供非零基准，或为该步骤改用绝对判据。",
            m.value, out.unit, abs, out.unit
        ));
    }
    out
}

fn fail_attribution(
    target: &str,
    measured: f64,
    expected: f64,
    deviation_pct: f64,
    tolerance_pct: f64,
    unit: &str,
    synthetic: bool,
) -> String {
    let mut s = format!(
        "{target} 超出容差：实测 {measured} {unit}，预期 {expected} {unit}，\
         偏差 {deviation_pct:+.2}%（容差 ±{tolerance_pct:.2}%）。可能原因：\
         元件取值/容差与设计不符、焊接或接触不良、拓扑与设计不一致、\
         仿真模型偏差、或测量条件（负载/激励/探针位置）与仿真不同。"
    );
    if synthetic {
        s.push_str(" 注意：本次实测数据来自合成源（非真实板卡），偏差只反映注入的误差。");
    }
    s
}

/// Compare a whole plan against the measurements collected for it.
pub fn compare_all(
    plan: &TestPlan,
    measurements: &BTreeMap<usize, Measurement>,
    rejected: &BTreeMap<usize, String>,
    policy: &TolerancePolicy,
) -> Vec<Comparison> {
    plan.steps
        .iter()
        .map(|step| {
            if let Some(reason) = rejected.get(&step.index) {
                let mut c = compare(step, None, policy);
                c.verdict = CompareVerdict::Rejected;
                c.analysis = Some(reason.clone());
                c
            } else {
                compare(step, measurements.get(&step.index), policy)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Expected, TestStep};

    fn step(quantity: Quantity, expected: f64, tol_pct: f64) -> TestStep {
        TestStep::new(
            1,
            "target",
            quantity,
            Expected::from_user(expected, tol_pct, quantity.unit(), "test"),
        )
    }

    fn measurement(quantity: Quantity, value: f64) -> Measurement {
        Measurement::new(quantity, value, "virtual", "t", true)
    }

    #[test]
    fn classification_matches_the_kr2_split() {
        assert_eq!(classify(Quantity::VoltageDc), QuantityClass::Dc);
        assert_eq!(classify(Quantity::Frequency), QuantityClass::AcFrequency);
    }

    #[test]
    fn default_policy_uses_kr2_thresholds() {
        let p = TolerancePolicy::kr2_default();
        assert_eq!(p.for_class(QuantityClass::Dc), 10.0);
        assert_eq!(p.for_class(QuantityClass::AcFrequency), 20.0);
        assert_eq!(p.source, ToleranceSource::Kr2Default);
        assert_eq!(p.source.describe(), "默认 KR2 阈值");
    }

    #[test]
    fn override_is_recorded_as_such() {
        let p = TolerancePolicy::override_with(5.0, 15.0);
        assert_eq!(p.for_class(QuantityClass::Dc), 5.0);
        assert_eq!(p.source, ToleranceSource::Override);
        assert_eq!(p.source.describe(), "覆盖阈值");
    }

    #[test]
    fn step_tolerance_takes_precedence() {
        let p = TolerancePolicy::kr2_default();
        let s = step(Quantity::VoltageDc, 3.3, 2.0);
        assert_eq!(p.resolve(&s), (2.0, ToleranceSource::StepTolerance));
    }

    #[test]
    fn zero_step_tolerance_falls_back_to_the_policy() {
        let p = TolerancePolicy::kr2_default();
        let s = step(Quantity::Frequency, 1000.0, 0.0);
        assert_eq!(p.resolve(&s), (20.0, ToleranceSource::Kr2Default));
    }

    #[test]
    fn pass_inside_tolerance() {
        let p = TolerancePolicy::kr2_default();
        let s = step(Quantity::VoltageDc, 3.3, 10.0);
        let m = measurement(Quantity::VoltageDc, 3.31);
        let c = compare(&s, Some(&m), &p);
        assert_eq!(c.verdict, CompareVerdict::Pass);
        assert!((c.deviation_pct.unwrap() - 0.303).abs() < 0.01);
        assert!(c.analysis.is_none());
        assert_eq!(c.tolerance_source, ToleranceSource::StepTolerance);
        assert_eq!(c.synthetic, Some(true));
    }

    #[test]
    fn fail_outside_tolerance_with_attribution() {
        let p = TolerancePolicy::kr2_default();
        let s = step(Quantity::VoltageDc, 3.3, 10.0);
        let m = measurement(Quantity::VoltageDc, 3.8);
        let c = compare(&s, Some(&m), &p);
        assert_eq!(c.verdict, CompareVerdict::Fail);
        let a = c.analysis.unwrap();
        assert!(a.contains("超出容差"));
        assert!(a.contains("合成源"), "synthetic data must be flagged in the analysis");
    }

    #[test]
    fn dc_15_percent_fails_under_kr2_default() {
        let p = TolerancePolicy::kr2_default();
        let s = step(Quantity::VoltageDc, 10.0, 0.0);
        let m = measurement(Quantity::VoltageDc, 11.5);
        let c = compare(&s, Some(&m), &p);
        assert_eq!(c.verdict, CompareVerdict::Fail);
        assert_eq!(c.tolerance_pct, 10.0);
        assert_eq!(c.tolerance_source, ToleranceSource::Kr2Default);
    }

    #[test]
    fn ac_frequency_25_percent_fails_by_default_but_passes_when_overridden() {
        let s = step(Quantity::Frequency, 1000.0, 0.0);
        let m = measurement(Quantity::Frequency, 1250.0);
        let default = compare(&s, Some(&m), &TolerancePolicy::kr2_default());
        assert_eq!(default.verdict, CompareVerdict::Fail);
        assert_eq!(default.tolerance_pct, 20.0);

        let loose = compare(&s, Some(&m), &TolerancePolicy::override_with(10.0, 30.0));
        assert_eq!(loose.verdict, CompareVerdict::Pass);
        assert_eq!(loose.tolerance_source, ToleranceSource::Override);
    }

    #[test]
    fn missing_measurement_is_not_comparable() {
        let p = TolerancePolicy::kr2_default();
        let s = step(Quantity::VoltageDc, 3.3, 10.0);
        let c = compare(&s, None, &p);
        assert_eq!(c.verdict, CompareVerdict::NotComparable);
        assert!(c.analysis.unwrap().contains("没有取得"));
    }

    #[test]
    fn zero_baseline_does_not_produce_a_ridiculous_percentage() {
        let p = TolerancePolicy::kr2_default();
        let s = step(Quantity::CurrentDc, 0.0, 10.0);
        let m = measurement(Quantity::CurrentDc, 0.001);
        let c = compare(&s, Some(&m), &p);
        assert!(c.deviation_pct.is_none(), "no percentage against a zero baseline");
        assert_eq!(c.deviation_abs, Some(0.001));
        // Refusing to judge is better than inventing an absolute threshold.
        assert_eq!(c.verdict, CompareVerdict::NotComparable);
        assert!(c.analysis.unwrap().contains("不判定"));
    }

    #[test]
    fn rejected_steps_carry_the_reason_and_never_pass() {
        let p = TolerancePolicy::kr2_default();
        let plan = TestPlan::new("tp", vec![step(Quantity::VoltageDc, 3.3, 10.0)]).unwrap();
        let mut rejected = BTreeMap::new();
        rejected.insert(1, "已拒绝：本次会话未启用源输出".to_string());
        let cs = compare_all(&plan, &BTreeMap::new(), &rejected, &p);
        assert_eq!(cs[0].verdict, CompareVerdict::Rejected);
        assert!(cs[0].analysis.as_ref().unwrap().contains("已拒绝"));
    }
}
