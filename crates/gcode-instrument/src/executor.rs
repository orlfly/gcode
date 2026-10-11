//! The executor: runs a test plan safely and produces a report.
//!
//! Responsibilities, in order of importance:
//!
//! 1. **De-energise on every exit path.** Success, fuse, rejection, or an
//!    unexpected error all end with the supply output off.
//! 2. **Fuse.** A measured current above twice the step's expected current cuts
//!    the output immediately and stops the run.
//! 3. **Explain refusals.** A step that cannot run (no supply, no confirmation,
//!    over the limits) is recorded as `REJECTED` with a reason, never skipped
//!    silently.
//!
//! The executor talks only to [`Instrument`], so the whole thing is testable with
//! the virtual backend.

use crate::compare::TolerancePolicy;
use crate::hal::{Instrument, MeasureRequest, Measurement, Quantity, SourceRequest};
use crate::plan::{TestPlan, TestStep};
use crate::report::TestReport;
use crate::safety::{ConfirmToken, fuse_tripped};
use std::collections::BTreeMap;

/// Runs plans against a meter (and optionally a programmable supply).
pub struct Executor<'a> {
    meter: &'a dyn Instrument,
    supply: Option<&'a dyn Instrument>,
    policy: TolerancePolicy,
}

impl<'a> Executor<'a> {
    pub fn new(meter: &'a dyn Instrument, supply: Option<&'a dyn Instrument>) -> Self {
        Self {
            meter,
            supply,
            policy: TolerancePolicy::kr2_default(),
        }
    }

    pub fn with_policy(mut self, policy: TolerancePolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn policy(&self) -> &TolerancePolicy {
        &self.policy
    }

    /// The instrument used to energise the board: an explicit supply if given,
    /// otherwise the meter itself when it is source-capable (a virtual bench or
    /// an SMU can do both).
    fn effective_supply(&self) -> Option<&'a dyn Instrument> {
        match self.supply {
            Some(s) => Some(s),
            None if self.meter.capabilities().can_source => Some(self.meter),
            None => None,
        }
    }

    /// Execute the plan and return a report.
    pub async fn run(&self, plan: &TestPlan, auth: Option<&ConfirmToken>) -> TestReport {
        let mut events: Vec<String> = Vec::new();
        let mut measurements: BTreeMap<usize, Measurement> = BTreeMap::new();
        let mut rejected: BTreeMap<usize, String> = BTreeMap::new();
        let mut energised: Vec<&TestStep> = Vec::new();
        let mut fused = false;

        events.push(format!(
            "测量仪器 {}（{}）",
            self.meter.id(),
            if self.meter.capabilities().can_source {
                "也可作为源"
            } else {
                "仅测量"
            }
        ));
        match self.effective_supply() {
            Some(s) => events.push(format!("上电通道 {}", s.id())),
            None => events.push("没有可上电的通道：需要上电的步骤将被拒绝".into()),
        }
        if auth.is_none() {
            events.push("本次运行未携带用户确认令牌：带电步骤将被拒绝".into());
        }

        for step in &plan.steps {
            if fused {
                rejected.insert(
                    step.index,
                    "熔断后已终止后续步骤（请在排除过流原因后重新取得用户确认再试）".to_string(),
                );
                continue;
            }

            // Steps that need power: energise first, and refuse if we cannot.
            if step.needs_power() {
                let Some(supply) = self.effective_supply() else {
                    rejected.insert(
                        step.index,
                        "该步骤需要给板子上电，但没有可编程电源（设置 GCODE_PSU_PORT）".to_string(),
                    );
                    continue;
                };
                let setting = step.supply.as_ref().expect("needs_power implies supply");
                let req = SourceRequest::set_voltage(setting.voltage)
                    .with_current_limit(setting.current_limit)
                    .output_on(true);
                // Defence in depth: re-authorise here even if the caller handed
                // us an unwrapped backend.
                if let Err(v) = crate::safety::authorise(&req, auth) {
                    rejected.insert(
                        step.index,
                        format!("上电被拒绝：{v}（{}）", v.hint()),
                    );
                    continue;
                }
                if let Err(e) = supply.source(&req, auth).await {
                    rejected.insert(step.index, format!("上电被拒绝：{e}"));
                    continue;
                }
                events.push(format!(
                    "步骤 {}：已上电 {}V / 限流 {}A",
                    step.index, setting.voltage, setting.current_limit
                ));
                energised.push(step);

                // Fuse: read current and compare with the expectation.
                if let Some(expected_current) = setting.expected_current {
                    if self
                        .meter
                        .capabilities()
                        .supports(Quantity::CurrentDc)
                    {
                        match self.measure_step(step, Quantity::CurrentDc).await {
                            Ok(m) => {
                                events.push(format!(
                                    "步骤 {}：实测电流 {} A（预期 {} A，熔断阈值 {} A）",
                                    step.index,
                                    m.value,
                                    expected_current,
                                    expected_current * crate::safety::FUSE_MULTIPLE
                                ));
                                if fuse_tripped(m.value, expected_current) {
                                    fused = true;
                                    // Cut the power before anything else.
                                    let off = SourceRequest {
                                        voltage: None,
                                        current_limit: None,
                                        output: Some(false),
                                    };
                                    let off_result = supply.source(&off, None).await;
                                    let note = match &off_result {
                                        Ok(()) => "",
                                        Err(_) => "（断电请求失败，请人工断开！）",
                                    };
                                    events.push(format!(
                                        "⚠️ 熔断：电流超过预期 {} 倍，已切断输出{note}",
                                        crate::safety::FUSE_MULTIPLE
                                    ));
                                    if let Err(e) = &off_result {
                                        events.push(format!("断电失败详情：{e}"));
                                    }
                                    rejected.insert(
                                        step.index,
                                        format!(
                                            "熔断：实测电流 {} A 超过预期 {} A 的 {} 倍",
                                            m.value,
                                            expected_current,
                                            crate::safety::FUSE_MULTIPLE
                                        ),
                                    );
                                    continue;
                                }
                            }
                            Err(e) => {
                                events.push(format!(
                                    "步骤 {}：无法读取电流（{e}），跳过熔断检查",
                                    step.index
                                ));
                            }
                        }
                    }
                }
            }

            // The actual measurement of this step.
            match self.measure_step(step, step.quantity).await {
                Ok(m) => {
                    measurements.insert(step.index, m);
                }
                Err(e) => {
                    rejected.insert(step.index, format!("测量失败：{e}"));
                }
            }
        }

        // Always leave the supply off.
        if let Some(supply) = self.effective_supply() {
            if !energised.is_empty() || fused {
                let off = SourceRequest {
                    voltage: None,
                    current_limit: None,
                    output: Some(false),
                };
                match supply.source(&off, None).await {
                    Ok(()) => events.push("运行结束：已关闭电源输出".into()),
                    Err(e) => events.push(format!("运行结束但关闭输出失败：{e}（请人工检查）")),
                }
            }
        }

        TestReport::from_run(plan, &measurements, &rejected, &self.policy, events)
    }

    async fn measure_step(
        &self,
        step: &TestStep,
        quantity: Quantity,
    ) -> Result<Measurement, crate::hal::InstrumentError> {
        let mut req = MeasureRequest::new(quantity).with_reduce(step.reduce);
        if let Some(ch) = &step.channel {
            req.channel = Some(ch.clone());
        }
        req.samples = step.samples;
        self.meter.measure(&req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::{Capabilities, InstrumentError};
    use crate::instruments::VirtualInstrument;
    use crate::plan::{Expected, TestPlan, TestStep};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Enables the flag for a test. Every caller must hold the env lock so that
    /// parallel tests never observe a half-set flag.
    fn enable_source() {
        // SAFETY: callers hold crate::test_support::env_lock().
        unsafe { std::env::set_var(crate::hal::ALLOW_SOURCE_ENV, "1") };
    }
    fn disable_source() {
        unsafe { std::env::remove_var(crate::hal::ALLOW_SOURCE_ENV) };
    }

    /// A meter whose current reading is fixed, to exercise the fuse.
    struct FixedMeter {
        voltage: f64,
        current: f64,
    }

    #[async_trait]
    impl Instrument for FixedMeter {
        fn id(&self) -> String {
            "fixed".into()
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                quantities: vec![Quantity::VoltageDc, Quantity::CurrentDc],
                max_voltage: 48.0,
                max_current: 5.0,
                can_source: false,
            }
        }
        async fn measure(&self, req: &MeasureRequest) -> Result<Measurement, InstrumentError> {
            let v = match req.quantity {
                Quantity::VoltageDc => self.voltage,
                Quantity::CurrentDc => self.current,
                _ => {
                    return Err(InstrumentError::Unsupported(req.quantity));
                }
            };
            Ok(Measurement::new(req.quantity, v, "fixed", "bench", false))
        }
    }

    /// A meter that fails on a chosen quantity, to exercise error paths.
    struct FailingMeter;

    #[async_trait]
    impl Instrument for FailingMeter {
        fn id(&self) -> String {
            "failing".into()
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                quantities: vec![Quantity::VoltageDc],
                max_voltage: 48.0,
                max_current: 5.0,
                can_source: false,
            }
        }
        async fn measure(&self, _req: &MeasureRequest) -> Result<Measurement, InstrumentError> {
            Err(InstrumentError::Unavailable {
                backend: "failing".into(),
                reason: "设备离线".into(),
            })
        }
    }

    /// Records source commands so power-off can be asserted.
    #[derive(Default)]
    struct SpySupply {
        log: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Instrument for SpySupply {
        fn id(&self) -> String {
            "spy".into()
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                quantities: vec![],
                max_voltage: 48.0,
                max_current: 5.0,
                can_source: true,
            }
        }
        async fn measure(&self, _req: &MeasureRequest) -> Result<Measurement, InstrumentError> {
            Err(InstrumentError::Unsupported(Quantity::VoltageDc))
        }
        async fn source(
            &self,
            req: &SourceRequest,
            _auth: Option<&ConfirmToken>,
        ) -> Result<(), InstrumentError> {
            self.log.lock().unwrap().push(format!(
                "V={:?} I={:?} OUT={:?}",
                req.voltage, req.current_limit, req.output
            ));
            Ok(())
        }
    }

    fn rail_plan(expected: f64, expected_current: f64) -> TestPlan {
        TestPlan::new(
            "tp-rail",
            vec![
                TestStep::new(
                    1,
                    "3V3 rail",
                    Quantity::VoltageDc,
                    Expected::from_user(expected, 5.0, "V", "用户要求 3.3V"),
                )
                .powered(5.0, 0.2)
                .with_expected_current(expected_current),
            ],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn happy_path_passes_and_powers_off() {
        let _guard = crate::test_support::env_lock();
        enable_source();
        let meter = FixedMeter {
            voltage: 3.31,
            current: 0.01,
        };
        let supply = SpySupply::default();
        let exec = Executor::new(&meter, Some(&supply));
        let token = ConfirmToken::mint(true).unwrap();
        let report = exec.run(&rail_plan(3.3, 0.05), Some(&token)).await;
        disable_source();

        assert_eq!(report.overall, "PASS");
        assert_eq!(report.passed, 1);
        let log = supply.log.lock().unwrap();
        assert!(log.first().unwrap().contains("OUT=Some(true)"), "{log:?}");
        assert!(
            log.last().unwrap().contains("OUT=Some(false)"),
            "the run must end with the supply off: {log:?}"
        );
    }

    #[tokio::test]
    async fn fuse_trips_and_cuts_power_and_stops_the_run() {
        let _guard = crate::test_support::env_lock();
        enable_source();
        let meter = FixedMeter {
            voltage: 3.3,
            current: 0.2, // 4x the expected 0.05 A
        };
        let supply = SpySupply::default();
        let exec = Executor::new(&meter, Some(&supply));
        let token = ConfirmToken::mint(true).unwrap();
        let report = exec.run(&rail_plan(3.3, 0.05), Some(&token)).await;
        disable_source();

        assert_eq!(report.rejected, 1);
        assert_eq!(report.passed, 0);
        assert!(report.events.iter().any(|e| e.contains("熔断")));
        let log = supply.log.lock().unwrap();
        assert!(
            log.iter().any(|l| l.contains("OUT=Some(false)")),
            "power must be cut on a fuse event: {log:?}"
        );
    }

    #[tokio::test]
    async fn fuse_does_not_trip_inside_the_threshold() {
        let _guard = crate::test_support::env_lock();
        enable_source();
        let meter = FixedMeter {
            voltage: 3.3,
            current: 0.1, // exactly 2x: not above
        };
        let supply = SpySupply::default();
        let exec = Executor::new(&meter, Some(&supply));
        let token = ConfirmToken::mint(true).unwrap();
        let report = exec.run(&rail_plan(3.3, 0.05), Some(&token)).await;
        disable_source();

        assert_eq!(report.rejected, 0);
        assert_eq!(report.passed, 1);
        // The informational line mentions the fuse *threshold*, so look for the
        // actual trip marker.
        assert!(!report.events.iter().any(|e| e.contains("⚠️ 熔断")));
    }

    #[tokio::test]
    async fn without_a_token_powered_steps_are_rejected_with_reasons() {
        let _guard = crate::test_support::env_lock();
        enable_source();
        let meter = FixedMeter {
            voltage: 3.3,
            current: 0.01,
        };
        let supply = SpySupply::default();
        let exec = Executor::new(&meter, Some(&supply));
        let report = exec.run(&rail_plan(3.3, 0.05), None).await;
        disable_source();

        assert_eq!(report.rejected, 1);
        assert_eq!(report.passed, 0);
        assert!(supply.log.lock().unwrap().is_empty(), "nothing may be energised");
        let item = &report.items[0];
        assert!(item.analysis.as_ref().unwrap().contains("拒绝"));
    }

    #[tokio::test]
    async fn without_a_supply_powered_steps_are_rejected() {
        // FixedMeter cannot source, and no supply is given.
        let meter = FixedMeter {
            voltage: 3.3,
            current: 0.01,
        };
        let exec = Executor::new(&meter, None);
        let report = exec.run(&rail_plan(3.3, 0.05), None).await;
        assert_eq!(report.rejected, 1);
        assert!(report.items[0]
            .analysis
            .as_ref()
            .unwrap()
            .contains("需要给板子上电"));
    }

    #[tokio::test]
    async fn measurement_failure_is_reported_not_silently_skipped() {
        let exec = Executor::new(&FailingMeter, None);
        let plan = TestPlan::new(
            "tp-fail",
            vec![TestStep::new(
                1,
                "3V3 rail",
                Quantity::VoltageDc,
                Expected::from_user(3.3, 5.0, "V", "用户要求"),
            )],
        )
        .unwrap();
        let report = exec.run(&plan, None).await;
        assert_eq!(report.rejected, 1);
        assert!(report.items[0].analysis.as_ref().unwrap().contains("测量失败"));
    }

    #[tokio::test]
    async fn passive_plan_needs_no_power_and_no_token() {
        let meter = VirtualInstrument::new().with_reading(Quantity::Resistance, 220.0);
        let exec = Executor::new(&meter, None);
        let plan = TestPlan::new(
            "tp-r",
            vec![TestStep::new(
                1,
                "R1",
                Quantity::Resistance,
                Expected::from_user(220.0, 5.0, "ohm", "标称 220R"),
            )],
        )
        .unwrap();
        let report = exec.run(&plan, None).await;
        assert_eq!(report.passed, 1);
        assert_eq!(report.overall, "PASS");
    }

    #[tokio::test]
    async fn the_report_always_ends_with_the_supply_off_even_on_failure() {
        let _guard = crate::test_support::env_lock();
        enable_source();
        let meter = FixedMeter {
            voltage: 3.3,
            current: 0.5,
        };
        let supply = SpySupply::default();
        let exec = Executor::new(&meter, Some(&supply));
        let token = ConfirmToken::mint(true).unwrap();
        let _ = exec.run(&rail_plan(3.3, 0.05), Some(&token)).await;
        disable_source();
        let log = supply.log.lock().unwrap();
        assert_eq!(
            log.last().unwrap().contains("OUT=Some(false)"),
            true,
            "last command must be power-off: {log:?}"
        );
    }
}
