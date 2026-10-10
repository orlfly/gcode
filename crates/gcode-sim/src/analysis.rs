//! Metric extraction and plain-language verdicts.
//!
//! Everything here is a pure function over waveform columns: no processes, no
//! files. That is what makes the analysis layer testable without ngspice
//! (`design.md` D6) and what keeps the PASS/FAIL verdict derived from simulation
//! data rather than from a language model (`sim-analysis` spec).

use crate::runner::Waveform;
use std::collections::BTreeMap;

/// Magnitude of a complex response in dB.
fn db(x: f64) -> f64 {
    20.0 * x.max(1e-30).log10()
}

/// Linear magnitude ratio of two columns, sampled at the same index.
pub fn ratio(out: &[f64], inp: &[f64]) -> Vec<f64> {
    out.iter()
        .zip(inp)
        .map(|(o, i)| if *i == 0.0 { f64::NAN } else { o / i })
        .collect()
}

/// Gain in dB at the lowest swept frequency (the "passband" value for the
/// low-pass/high-pass shapes used in the catalog).
pub fn low_freq_gain_db(w: &Waveform, out_col: &str, in_col: &str) -> Option<f64> {
    let o = w.column(out_col)?;
    let i = w.column(in_col)?;
    let r = ratio(&o, &i);
    r.first().filter(|v| v.is_finite()).map(|v| db(*v))
}

/// Linear gain at the lowest swept frequency.
pub fn low_freq_gain_linear(w: &Waveform, out_col: &str, in_col: &str) -> Option<f64> {
    let o = w.column(out_col)?;
    let i = w.column(in_col)?;
    let r = ratio(&o, &i);
    r.first().filter(|v| v.is_finite()).copied()
}

/// Frequency where the response falls 3 dB below its passband value.
///
/// Interpolates in log-frequency, which is where the sweep is uniform.
pub fn cutoff_3db(w: &Waveform, out_col: &str, in_col: &str) -> Option<f64> {
    cutoff_3db_shape(w, out_col, in_col, ResponseShape::Lowpass)
}

/// Which side of the response is the passband.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseShape {
    /// Passband at low frequency (first sample is the reference).
    Lowpass,
    /// Passband at high frequency (the maximum is the reference).
    Highpass,
}

/// Shape-aware -3 dB cutoff.
pub fn cutoff_3db_shape(
    w: &Waveform,
    out_col: &str,
    in_col: &str,
    shape: ResponseShape,
) -> Option<f64> {
    let f = w.column(w.scale_name()?)?;
    let o = w.column(out_col)?;
    let i = w.column(in_col)?;
    let h: Vec<f64> = ratio(&o, &i).into_iter().map(db).collect();
    let reference = match shape {
        ResponseShape::Lowpass => *h.first().filter(|v| v.is_finite())?,
        ResponseShape::Highpass => h
            .iter()
            .cloned()
            .filter(|v| v.is_finite())
            .fold(f64::NEG_INFINITY, f64::max),
    };
    if !reference.is_finite() {
        return None;
    }
    let target = reference - 3.0;

    for k in 1..h.len() {
        if !h[k].is_finite() || !h[k - 1].is_finite() {
            continue;
        }
        let crossing = match shape {
            ResponseShape::Lowpass => h[k] <= target && h[k - 1] > target,
            ResponseShape::Highpass => h[k] >= target && h[k - 1] < target,
        };
        if crossing {
            // Interpolate in log10(f).
            let (x0, x1) = (f[k - 1].log10(), f[k].log10());
            let (y0, y1) = (h[k - 1], h[k]);
            if (y1 - y0).abs() < f64::EPSILON {
                return Some(f[k]);
            }
            let t = (target - y0) / (y1 - y0);
            return Some(10f64.powf(x0 + t * (x1 - x0)));
        }
    }
    None
}

/// Minimum value of a column.
pub fn min_value(w: &Waveform, col: &str) -> Option<f64> {
    let v = w.column(col)?;
    v.iter().cloned().fold(None, |acc: Option<f64>, x| {
        Some(acc.map_or(x, |a: f64| a.min(x)))
    })
}

/// Maximum value of a column.
pub fn max_value(w: &Waveform, col: &str) -> Option<f64> {
    let v = w.column(col)?;
    v.iter().cloned().fold(None, |acc: Option<f64>, x| {
        Some(acc.map_or(x, |a: f64| a.max(x)))
    })
}

/// Peak absolute value of a column.
pub fn peak(w: &Waveform, col: &str) -> Option<f64> {
    let v = w.column(col)?;
    v.iter().cloned().fold(None, |acc: Option<f64>, x| {
        Some(acc.map_or(x.abs(), |a: f64| a.max(x.abs())))
    })
}

/// Steady-state ripple (max - min) over the last `fraction` of a transient.
pub fn ripple_pp(w: &Waveform, col: &str, fraction: f64) -> Option<f64> {
    let v = w.column(col)?;
    let start = ((v.len() as f64) * (1.0 - fraction)).floor() as usize;
    let window = v.get(start.min(v.len().saturating_sub(1))..)?;
    if window.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for x in window {
        lo = lo.min(*x);
        hi = hi.max(*x);
    }
    Some(hi - lo)
}

/// 10%→90% rise time of a monotonic step in a column.
pub fn rise_time(w: &Waveform, col: &str) -> Option<f64> {
    let t = w.column(w.scale_name()?)?;
    let v = w.column(col)?;
    if v.len() < 3 {
        return None;
    }
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for x in &v {
        lo = lo.min(*x);
        hi = hi.max(*x);
    }
    if !(hi - lo).is_finite() || (hi - lo).abs() < 1e-12 {
        return None;
    }
    let lo_t = lo + 0.1 * (hi - lo);
    let hi_t = lo + 0.9 * (hi - lo);
    let cross = |level: f64| -> Option<f64> {
        for k in 1..v.len() {
            if v[k - 1] < level && v[k] >= level {
                let t0 = t[k - 1];
                let t1 = t[k];
                let frac = (level - v[k - 1]) / (v[k] - v[k - 1]);
                return Some(t0 + frac * (t1 - t0));
            }
        }
        None
    };
    let t10 = cross(lo_t)?;
    let t90 = cross(hi_t)?;
    Some((t90 - t10).max(0.0))
}

/// Approximate phase margin: `180 + angle(out/in)` at the unity-gain crossing.
///
/// ngspice's `vp()` returns **radians** (verified against a known RC phase), so
/// the difference is converted to degrees here.
///
/// Only meaningful for a closed-loop amplifier whose loop is dominated by the
/// modelled pole; callers must treat it as an approximation.
pub fn phase_margin_deg(w: &Waveform, out_col: &str, in_col: &str) -> Option<f64> {
    let o = w.column(out_col)?;
    let i = w.column(in_col)?;
    let r = ratio(&o, &i);
    // Phase of out/in is the difference of the two phase columns when present.
    let out_phase = w.column(&format!("vp({})", strip_v(out_col)))?;
    let in_phase = w.column(&format!("vp({})", strip_v(in_col)))?;
    for k in 1..r.len() {
        if !r[k].is_finite() || !r[k - 1].is_finite() {
            continue;
        }
        if r[k - 1] > 1.0 && r[k] <= 1.0 {
            let phase = out_phase[k] - in_phase[k];
            return Some(180.0 + phase.to_degrees());
        }
    }
    None
}

fn strip_v(col: &str) -> String {
    col.strip_prefix("vm(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(col)
        .to_string()
}

// ---------- verdicts ----------

/// An expected metric value with a tolerance.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Expectation {
    /// Metric key (see [`METRIC_INFO`]).
    pub metric: String,
    pub expected: f64,
    /// Tolerance as a percentage of `expected`.
    pub tol_pct: f64,
    pub unit: String,
    /// How the expected value was derived (shown to the user).
    pub rationale: String,
}

impl Expectation {
    pub fn new(
        metric: &str,
        expected: f64,
        tol_pct: f64,
        unit: &str,
        rationale: &str,
    ) -> Self {
        Self {
            metric: metric.into(),
            expected,
            tol_pct,
            unit: unit.into(),
            rationale: rationale.into(),
        }
    }
}

/// One metric compared against its expectation.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub metric: String,
    pub label: String,
    pub expected: f64,
    pub measured: Option<f64>,
    pub unit: String,
    pub tol_pct: f64,
    pub deviation_abs: Option<f64>,
    pub deviation_pct: Option<f64>,
    pub passed: bool,
    pub rationale: String,
    pub explanation: String,
    pub suggestion: Option<String>,
}

/// Overall verdict for one analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub passed: bool,
    pub checks: Vec<CheckResult>,
}

impl Verdict {
    /// Number of checks that passed.
    pub fn passed_count(&self) -> usize {
        self.checks.iter().filter(|c| c.passed).count()
    }

    /// Human-readable rendering (Chinese) for the agent and the user.
    pub fn render(&self) -> String {
        let mut out = format!(
            "仿真判定：{}（{}/{} 项达标）\n",
            if self.passed { "PASS" } else { "FAIL" },
            self.passed_count(),
            self.checks.len()
        );
        for c in &self.checks {
            out.push_str(&format!(
                "\n- [{}] {} = {}\n    预期 {} {}（容差 ±{}%）\n    依据：{}",
                if c.passed { "PASS" } else { "FAIL" },
                c.label,
                c.measured
                    .map(|m| format!("{} {}", fmt(m), c.unit))
                    .unwrap_or_else(|| "无数据".into()),
                fmt(c.expected),
                c.unit,
                fmt(c.tol_pct),
                c.rationale
            ));
            if let (Some(a), Some(p)) = (c.deviation_abs, c.deviation_pct) {
                out.push_str(&format!(
                    "\n    偏差：{} {}（{:.2}%）",
                    fmt(a),
                    c.unit,
                    p
                ));
            }
            out.push_str(&format!("\n    说明：{}", c.explanation));
            if let Some(s) = &c.suggestion {
                out.push_str(&format!("\n    建议：{s}"));
            }
        }
        out
    }

    /// Machine-readable form.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "verdict": if self.passed { "PASS" } else { "FAIL" },
            "passed": self.passed_count(),
            "total": self.checks.len(),
            "checks": self.checks.iter().map(|c| serde_json::json!({
                "metric": c.metric,
                "label": c.label,
                "expected": c.expected,
                "measured": c.measured,
                "unit": c.unit,
                "tol_pct": c.tol_pct,
                "deviation_abs": c.deviation_abs,
                "deviation_pct": c.deviation_pct,
                "passed": c.passed,
                "explanation": c.explanation,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Metric key → (Chinese label, physical meaning, common failure causes).
pub const METRIC_INFO: &[(&str, &str, &str, &str)] = &[
    (
        "dc_voltage",
        "直流电压",
        "该节点在直流工作点的电压",
        "元件取值与设计不符、拓扑接错、模型直流工作点偏移",
    ),
    (
        "gain_linear",
        "低频增益（倍数）",
        "通带内输出与输入幅度之比",
        "反馈电阻取值不符、运放模型开环增益不足、输入端接错",
    ),
    (
        "gain_db",
        "低频增益（dB）",
        "通带内输出与输入幅度之比（dB）",
        "反馈网络取值不符、模型开环增益不足",
    ),
    (
        "fc_3db",
        "截止频率（-3dB）",
        "幅频响应相对通带下降 3dB 的频率",
        "R/C 取值与设计不符、元件容差、拓扑接错",
    ),
    (
        "phase_margin_deg",
        "相位裕度（近似）",
        "单位增益穿越处的相位余量，衡量稳定性",
        "主极点位置不当、反馈过深、模型简化",
    ),
    (
        "rise_time_s",
        "上升时间（10%→90%）",
        "阶跃响应的上升快慢，与带宽成反比",
        "R/C 取值不符、驱动能力不足、模型简化",
    ),
    (
        "ripple_pp_v",
        "纹波（峰峰值）",
        "稳态区间内电压的波动幅度",
        "滤波不足、负载变化、激励频率偏高",
    ),
    (
        "peak_v",
        "峰值",
        "波形绝对值的最大值",
        "增益过高、偏置不对、激励过大",
    ),
    (
        "min_v",
        "最小值",
        "波形在分析区间内的最小值",
        "偏置不对、开关未导通、激励或负载不符",
    ),
    (
        "max_v",
        "最大值",
        "波形在分析区间内的最大值",
        "偏置不对、上拉不足、激励过大",
    ),
];

fn metric_info(metric: &str) -> (&'static str, &'static str, &'static str) {
    METRIC_INFO
        .iter()
        .find(|(k, _, _, _)| *k == metric)
        .map(|(_, label, meaning, causes)| (*label, *meaning, *causes))
        .unwrap_or((
            "未收录指标",
            "该指标的物理含义尚未收录到解释表",
            "元件容差、模型精度或拓扑接错",
        ))
}

/// Compare measured metrics against expectations.
///
/// A metric with no measurement never passes: a missing simulation result must
/// not be reported as success.
pub fn evaluate(
    measured: &BTreeMap<String, f64>,
    expectations: &[Expectation],
) -> Verdict {
    let mut checks = Vec::new();
    for e in expectations {
        let (label, meaning, causes) = metric_info(&e.metric);
        let value = measured.get(&e.metric).copied().filter(|v| v.is_finite());
        let (dev_abs, dev_pct, passed) = match value {
            Some(v) => {
                let abs = v - e.expected;
                let pct = if e.expected == 0.0 {
                    f64::NAN
                } else {
                    abs / e.expected * 100.0
                };
                let ok = pct.is_finite() && pct.abs() <= e.tol_pct;
                (Some(abs), Some(pct), ok)
            }
            None => (None, None, false),
        };
        let (explanation, suggestion) = match value {
            None => (
                format!("{label}：仿真未产出该指标（{meaning}），无法判定。"),
                Some("确认探针节点正确、分析类型与扫描范围合适，并检查仿真是否成功。".to_string()),
            ),
            Some(v) if passed => (
                format!(
                    "{label}：{meaning}。实测 {} {} 落在预期 {} {} 的 ±{}% 内。",
                    fmt(v),
                    e.unit,
                    fmt(e.expected),
                    e.unit,
                    fmt(e.tol_pct)
                ),
                None,
            ),
            Some(v) => (
                format!(
                    "{label}：{meaning}。实测 {} {} 偏离预期 {} {} 超过 ±{}%。",
                    fmt(v),
                    e.unit,
                    fmt(e.expected),
                    e.unit,
                    fmt(e.tol_pct)
                ),
                Some(format!(
                    "优先排查：{causes}。可先核对元件取值是否与预期推导一致，再看拓扑连接。"
                )),
            ),
        };
        checks.push(CheckResult {
            metric: e.metric.clone(),
            label: label.to_string(),
            expected: e.expected,
            measured: value,
            unit: e.unit.clone(),
            tol_pct: e.tol_pct,
            deviation_abs: dev_abs,
            deviation_pct: dev_pct,
            passed,
            rationale: e.rationale.clone(),
            explanation,
            suggestion,
        });
    }
    Verdict {
        passed: checks.iter().all(|c| c.passed) && !checks.is_empty(),
        checks,
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

/// Which standard metric set to extract for an analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricSet {
    /// DC operating point: node voltages.
    Dc,
    /// AC sweep: gain and shape-dependent cutoff.
    Ac { shape: ResponseShape },
    /// Transient: extremes and rise time.
    Tran,
}

/// Extract the standard metrics for an analysis from a waveform.
///
/// `out_col`/`in_col` are ngspice column names (`v(OUT)`, `vm(OUT)`, ...).
/// Missing data simply omits the key, which the verdict layer treats as a
/// failure rather than a pass.
pub fn extract_metrics(
    set: MetricSet,
    w: &Waveform,
    out_col: &str,
    in_col: &str,
) -> BTreeMap<String, f64> {
    let mut m = BTreeMap::new();
    match set {
        MetricSet::Dc => {
            if let Some(v) = w.last_value(out_col).filter(|v| v.is_finite()) {
                m.insert("dc_voltage".into(), v);
            }
        }
        MetricSet::Ac { shape } => {
            if let Some(g) = low_freq_gain_linear(w, out_col, in_col).filter(|v| v.is_finite()) {
                m.insert("gain_linear".into(), g);
                m.insert("gain_db".into(), db(g));
            }
            if let Some(fc) = cutoff_3db_shape(w, out_col, in_col, shape).filter(|v| v.is_finite()) {
                m.insert("fc_3db".into(), fc);
            }
        }
        MetricSet::Tran => {
            if let Some(v) = min_value(w, out_col).filter(|v| v.is_finite()) {
                m.insert("min_v".into(), v);
            }
            if let Some(v) = max_value(w, out_col).filter(|v| v.is_finite()) {
                m.insert("max_v".into(), v);
            }
            if let Some(v) = peak(w, out_col).filter(|v| v.is_finite()) {
                m.insert("peak_v".into(), v);
            }
            if let Some(v) = rise_time(w, out_col).filter(|v| v.is_finite()) {
                m.insert("rise_time_s".into(), v);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(columns: &[&str], data: Vec<Vec<f64>>) -> Waveform {
        Waveform {
            columns: columns.iter().map(|s| s.to_string()).collect(),
            data,
            raw_output: String::new(),
        }
    }

    /// First-order low pass sampled at the analytic -3 dB point, driven by a
    /// unit-magnitude input column (so the transfer ratio is `vm(OUT)/vm(IN)`).
    fn rc_lowpass_wave() -> Waveform {
        let f: [f64; 6] = [10.0, 100.0, 1000.0, 1591.549, 10000.0, 100000.0];
        let rows: Vec<Vec<f64>> = f
            .iter()
            .map(|freq| {
                let fc: f64 = 1591.5494309189535;
                let h = 1.0 / (1.0 + (*freq / fc).powi(2)).sqrt();
                let phase = -(*freq / fc).atan();
                vec![*freq, 1.0, 0.0, h, phase]
            })
            .collect();
        wave(&["frequency", "vm(IN)", "vp(IN)", "vm(OUT)", "vp(OUT)"], rows)
    }

    #[test]
    fn extracts_low_frequency_gain() {
        let w = rc_lowpass_wave();
        let g = low_freq_gain_linear(&w, "vm(OUT)", "vm(IN)").unwrap();
        assert!((g - 1.0).abs() < 1e-3, "gain {g}");
    }

    #[test]
    fn finds_the_3db_cutoff() {
        let w = rc_lowpass_wave();
        let fc = cutoff_3db(&w, "vm(OUT)", "vm(IN)").unwrap();
        assert!(
            (fc - 1591.55).abs() / 1591.55 < 0.01,
            "expected ~1591.5 Hz, got {fc}"
        );
    }

    #[test]
    fn cutoff_absent_when_response_never_drops() {
        let w = wave(
            &["frequency", "vm(OUT)"],
            vec![vec![10.0, 1.0], vec![100.0, 1.0], vec![1000.0, 1.0]],
        );
        assert!(cutoff_3db(&w, "vm(OUT)", "vm(OUT)").is_none());
    }

    #[test]
    fn cutoff_absent_for_a_flat_transfer() {
        // out == in everywhere: no -3 dB point exists.
        let w = wave(
            &["frequency", "vm(IN)", "vm(OUT)"],
            vec![vec![10.0, 1.0, 1.0], vec![1000.0, 1.0, 1.0]],
        );
        assert!(cutoff_3db(&w, "vm(OUT)", "vm(IN)").is_none());
    }

    #[test]
    fn measures_rise_time_of_a_step() {
        // Exponential step with tau = 1 ms: t10->t90 = tau*ln(9) ~ 2.197 ms.
        let tau = 1e-3;
        let mut rows = Vec::new();
        for k in 0..200 {
            let t = k as f64 * 5e-5;
            let v = 1.0 - (-t / tau).exp();
            rows.push(vec![t, v]);
        }
        let w = wave(&["time", "v(OUT)"], rows);
        let tr = rise_time(&w, "v(OUT)").unwrap();
        let expected = tau * 9f64.ln();
        assert!(
            (tr - expected).abs() / expected < 0.02,
            "expected {expected}, got {tr}"
        );
    }

    #[test]
    fn measures_ripple_in_the_tail() {
        let mut rows = Vec::new();
        for k in 0..100 {
            let t = k as f64 * 1e-5;
            // settles to 5 V with 20 mV of ripple after t > 0.5 ms
            let v = if t < 5e-4 { t * 1e4 } else { 5.0 + 0.01 * (k as f64).sin() };
            rows.push(vec![t, v]);
        }
        let w = wave(&["time", "v(OUT)"], rows);
        let r = ripple_pp(&w, "v(OUT)", 0.4).unwrap();
        assert!(r > 0.0 && r < 0.03, "ripple {r}");
    }

    #[test]
    fn phase_margin_at_unity_crossing() {
        // Gain 10 with a pole at 1 kHz: unity crossing ~10 kHz, phase ~ -84 deg.
        let f: [f64; 4] = [100.0, 1000.0, 10000.0, 100000.0];
        let fc: f64 = 1000.0;
        let rows: Vec<Vec<f64>> = f
            .iter()
            .map(|freq| {
                let mag = 10.0 / (1.0 + (*freq / fc).powi(2)).sqrt();
                let ph = -(*freq / fc).atan();
                vec![*freq, 1.0, 0.0, mag, ph]
            })
            .collect();
        let w = wave(
            &["frequency", "vm(IN)", "vp(IN)", "vm(OUT)", "vp(OUT)"],
            rows,
        );
        let pm = phase_margin_deg(&w, "vm(OUT)", "vm(IN)").unwrap();
        // Crossing between 10k and 100k; phase there is between -84 and -89 deg.
        assert!(pm > 80.0 && pm < 100.0, "phase margin {pm}");
    }

    #[test]
    fn verdict_passes_within_tolerance() {
        let mut m = BTreeMap::new();
        m.insert("gain_linear".to_string(), 10.05);
        let v = evaluate(&m, &[Expectation::new("gain_linear", 10.1, 5.0, "x", "1+R2/R1")]);
        assert!(v.passed);
        assert_eq!(v.passed_count(), 1);
        assert!(v.render().contains("PASS"));
        assert!(v.render().contains("1+R2/R1"));
    }

    #[test]
    fn verdict_fails_outside_tolerance_and_explains() {
        let mut m = BTreeMap::new();
        m.insert("gain_linear".to_string(), 6.2);
        let v = evaluate(&m, &[Expectation::new("gain_linear", 10.1, 5.0, "x", "1+R2/R1")]);
        assert!(!v.passed);
        let c = &v.checks[0];
        assert!((c.deviation_pct.unwrap() + 38.6).abs() < 0.5);
        assert!(c.suggestion.is_some());
        assert!(v.render().contains("FAIL"));
    }

    #[test]
    fn missing_measurement_never_passes() {
        let m = BTreeMap::new();
        let v = evaluate(&m, &[Expectation::new("fc_3db", 1591.0, 5.0, "Hz", "1/(2 pi R C)")]);
        assert!(!v.passed);
        assert!(v.checks[0].measured.is_none());
        assert!(v.checks[0].suggestion.is_some());
    }

    #[test]
    fn empty_expectations_is_not_a_pass() {
        let v = evaluate(&BTreeMap::new(), &[]);
        assert!(!v.passed, "an empty check list must not report success");
    }

    #[test]
    fn metric_set_dc_reads_the_operating_point() {
        let w = wave(
            &["in", "v(out)"],
            vec![vec![12.0, 2.44897959]],
        );
        let m = extract_metrics(MetricSet::Dc, &w, "v(out)", "v(out)");
        assert!((m["dc_voltage"] - 2.44897959).abs() < 1e-9);
    }

    #[test]
    fn metric_set_ac_reads_gain_and_cutoff() {
        let w = rc_lowpass_wave();
        let m = extract_metrics(
            MetricSet::Ac {
                shape: ResponseShape::Lowpass,
            },
            &w,
            "vm(OUT)",
            "vm(IN)",
        );
        assert!((m["gain_linear"] - 1.0).abs() < 1e-3);
        assert!((m["fc_3db"] - 1591.55).abs() / 1591.55 < 0.01);
    }

    #[test]
    fn metric_set_ac_highpass_shape() {
        // Rising response: -3 dB above the low-frequency floor.
        let rows = vec![
            vec![10.0, 1.0, 0.0, 0.01, -1.5708],
            vec![100.0, 1.0, 0.0, 0.1, -1.4661],
            vec![1591.5, 1.0, 0.0, 0.7071, -0.7854],
            vec![10000.0, 1.0, 0.0, 0.99, -0.0995],
        ];
        let w = wave(
            &["frequency", "vm(IN)", "vp(IN)", "vm(OUT)", "vp(OUT)"],
            rows,
        );
        let m = extract_metrics(
            MetricSet::Ac {
                shape: ResponseShape::Highpass,
            },
            &w,
            "vm(OUT)",
            "vm(IN)",
        );
        assert!(
            (m["fc_3db"] - 1591.5).abs() / 1591.5 < 0.02,
            "got {}",
            m["fc_3db"]
        );
    }

    #[test]
    fn metric_set_tran_reads_extremes() {
        let rows = vec![
            vec![0.0, 0.0],
            vec![1e-3, 4.9],
            vec![2e-3, 5.05],
            vec![3e-3, 5.0],
        ];
        let w = wave(&["time", "v(out)"], rows);
        let m = extract_metrics(MetricSet::Tran, &w, "v(out)", "v(out)");
        assert_eq!(m["min_v"], 0.0);
        assert_eq!(m["max_v"], 5.05);
        assert_eq!(m["peak_v"], 5.05);
        assert!(m.contains_key("rise_time_s"));
    }

    #[test]
    fn json_verdict_is_machine_readable() {
        let mut m = BTreeMap::new();
        m.insert("dc_voltage".to_string(), 2.449);
        let v = evaluate(&m, &[Expectation::new("dc_voltage", 2.449, 1.0, "V", "12*10/49")]);
        let j = v.to_json();
        assert_eq!(j["verdict"], "PASS");
        assert_eq!(j["checks"][0]["metric"], "dc_voltage");
    }
}
