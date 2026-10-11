//! Test plan IR: the structured, reviewable form of a measurement intent.
//!
//! Every step carries an [`Expected`] value **with its source**, so an agent can
//! never invent a pass/fail criterion: expectations come either from a simulation
//! baseline (`gcode-sim`) or from an explicit user input (`design.md` D4).

use crate::hal::{Quantity, Reduce};
use serde::{Deserialize, Serialize};

/// Where an expected value came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Baseline {
    /// Taken from a simulation result.
    Simulation {
        circuit: String,
        node: String,
        analysis: String,
    },
    /// Supplied by the user.
    User,
}

impl Baseline {
    /// One-line description used in reports.
    pub fn describe(&self) -> String {
        match self {
            Baseline::Simulation {
                circuit,
                node,
                analysis,
            } => format!("仿真基准（{circuit} / {node} / {analysis}）"),
            Baseline::User => "用户输入".into(),
        }
    }
}

/// An expected value, its tolerance, and where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expected {
    pub value: f64,
    /// Relative tolerance in percent.
    pub tol_pct: f64,
    pub unit: String,
    pub source: Baseline,
    pub rationale: String,
}

impl Expected {
    pub fn from_simulation(
        circuit: &str,
        node: &str,
        analysis: &str,
        value: f64,
        tol_pct: f64,
        unit: &str,
        rationale: &str,
    ) -> Self {
        Self {
            value,
            tol_pct,
            unit: unit.into(),
            source: Baseline::Simulation {
                circuit: circuit.into(),
                node: node.into(),
                analysis: analysis.into(),
            },
            rationale: rationale.into(),
        }
    }

    pub fn from_user(value: f64, tol_pct: f64, unit: &str, rationale: &str) -> Self {
        Self {
            value,
            tol_pct,
            unit: unit.into(),
            source: Baseline::User,
            rationale: rationale.into(),
        }
    }

    /// Absolute tolerance in the expected unit.
    pub fn tol_absolute(&self) -> f64 {
        (self.value.abs() * self.tol_pct / 100.0).abs()
    }
}

/// Supply settings required to energise the board for a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupplySetting {
    pub voltage: f64,
    pub current_limit: f64,
    /// Expected current, used by the executor's fuse check.
    pub expected_current: Option<f64>,
}

/// One measurement step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestStep {
    /// 1-based ordinal, used in the report.
    pub index: usize,
    /// Human target name, e.g. "3V3 rail".
    pub target: String,
    pub quantity: Quantity,
    pub channel: Option<String>,
    pub samples: Option<u32>,
    #[serde(default)]
    pub reduce: Reduce,
    pub expected: Expected,
    /// Present when the step needs the board powered.
    pub supply: Option<SupplySetting>,
}

impl TestStep {
    pub fn new(index: usize, target: &str, quantity: Quantity, expected: Expected) -> Self {
        Self {
            index,
            target: target.into(),
            quantity,
            channel: None,
            samples: None,
            reduce: Reduce::default(),
            expected,
            supply: None,
        }
    }

    pub fn on_channel(mut self, channel: &str) -> Self {
        self.channel = Some(channel.into());
        self
    }

    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = Some(samples);
        self
    }

    pub fn with_reduce(mut self, reduce: Reduce) -> Self {
        self.reduce = reduce;
        self
    }

    pub fn powered(mut self, voltage: f64, current_limit: f64) -> Self {
        self.supply = Some(SupplySetting {
            voltage,
            current_limit,
            expected_current: None,
        });
        self
    }

    pub fn with_expected_current(mut self, amps: f64) -> Self {
        if let Some(s) = &mut self.supply {
            s.expected_current = Some(amps);
        }
        self
    }

    /// Whether this step needs the board energised.
    pub fn needs_power(&self) -> bool {
        self.supply.is_some()
    }
}

/// A complete test plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestPlan {
    pub plan_id: String,
    pub steps: Vec<TestStep>,
    /// Things the planner deliberately did not include, and why.
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PlanError {
    #[error("测试计划没有任何步骤")]
    Empty,
    #[error("步骤序号重复：{0}")]
    DuplicateIndex(usize),
    #[error("步骤 {index} （{target}）没有预期值来源")]
    MissingExpected { index: usize, target: String },
    #[error("步骤 {index} （{target}）需要带电但没有电源设置")]
    PoweredWithoutSupply { index: usize, target: String },
    #[error("未知电路 {0}")]
    UnknownCircuit(String),
    #[error("电路 {circuit} 没有名为 {recipe} 的仿真配方（可用：{available}）")]
    UnknownRecipe {
        circuit: String,
        recipe: String,
        available: String,
    },
    #[error("JSON 解析失败：{0}")]
    Json(String),
}

impl TestPlan {
    /// Build a plan, validating it immediately.
    pub fn new(plan_id: &str, steps: Vec<TestStep>) -> Result<Self, PlanError> {
        let plan = Self {
            plan_id: plan_id.into(),
            steps,
            notes: Vec::new(),
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn with_note(mut self, note: &str) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Structural validation: non-empty, unique ordinals, powered steps have a
    /// supply. A step cannot exist without an `Expected` (enforced by the type),
    /// which is how "no invented pass criteria" is guaranteed.
    pub fn validate(&self) -> Result<(), PlanError> {
        if self.steps.is_empty() {
            return Err(PlanError::Empty);
        }
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.steps {
            if !seen.insert(s.index) {
                return Err(PlanError::DuplicateIndex(s.index));
            }
        }
        for s in &self.steps {
            if s.expected.unit.trim().is_empty() {
                return Err(PlanError::MissingExpected {
                    index: s.index,
                    target: s.target.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, PlanError> {
        serde_json::to_string_pretty(self).map_err(|e| PlanError::Json(e.to_string()))
    }

    pub fn from_json(text: &str) -> Result<Self, PlanError> {
        let plan: Self = serde_json::from_str(text).map_err(|e| PlanError::Json(e.to_string()))?;
        plan.validate()?;
        Ok(plan)
    }

    /// Steps that need the board energised.
    pub fn powered_steps(&self) -> Vec<&TestStep> {
        self.steps.iter().filter(|s| s.needs_power()).collect()
    }

    /// Build a plan from a catalog circuit's simulation recipe.
    ///
    /// The recipe's expectations become the baselines. Metrics that cannot be
    /// measured with the available instruments (a gain ratio, for instance) are
    /// **not** turned into steps; they are recorded in `notes` instead of being
    /// silently dropped or faked.
    pub fn from_recipe(circuit_id: &str, recipe_name: Option<&str>) -> Result<Self, PlanError> {
        let entry = gcode_kicad::catalog::by_id(circuit_id)
            .ok_or_else(|| PlanError::UnknownCircuit(circuit_id.to_string()))?;
        let mut candidates = gcode_sim::recipes::by_circuit(circuit_id);
        if let Some(name) = recipe_name {
            candidates.retain(|r| r.name == name);
        }
        let recipe = candidates.first().ok_or_else(|| PlanError::UnknownRecipe {
            circuit: circuit_id.to_string(),
            recipe: recipe_name.unwrap_or("(any)").into(),
            available: gcode_sim::recipes::by_circuit(circuit_id)
                .iter()
                .map(|r| r.name)
                .collect::<Vec<_>>()
                .join(", "),
        })?;

        let mut steps = Vec::new();
        let mut notes = vec![format!(
            "计划基于仿真配方「{}」（分析 {}），预期值来源为仿真基准。",
            recipe.name,
            recipe.analysis.kind.tag()
        )];
        let mut index = 1;

        // A DC operating point becomes a voltage measurement on the probed node.
        // (Current is measurable too, but the recipe probes voltage.)
        for e in &recipe.expectations {
            let (quantity, unit) = match e.metric.as_str() {
                "dc_voltage" => (Quantity::VoltageDc, "V"),
                "fc_3db" => (Quantity::Frequency, "Hz"),
                other => {
                    notes.push(format!(
                        "指标 {other}（预期 {} {}）无法用当前仪器直接测量，已跳过。",
                        e.expected, e.unit
                    ));
                    continue;
                }
            };
            let expected = Expected::from_simulation(
                entry.id,
                recipe.output,
                recipe.analysis.kind.tag(),
                e.expected,
                e.tol_pct,
                unit,
                &e.rationale,
            );
            steps.push(TestStep::new(index, recipe.output, quantity, expected));
            index += 1;
        }

        // Frequency can only be measured on a powered, oscillating node; voltage
        // measurements need the rail up. Either way the board must be energised.
        for s in &mut steps {
            let (v, i) = match s.quantity {
                Quantity::Frequency => (5.0, 0.1),
                _ => (5.0, 0.2),
            };
            s.supply = Some(SupplySetting {
                voltage: v,
                current_limit: i,
                expected_current: Some(i / 2.0),
            });
        }

        let mut plan = Self::new(&format!("tp-{circuit_id}"), steps)?;
        plan.notes = notes;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voltage_step(index: usize) -> TestStep {
        TestStep::new(
            index,
            "3V3 rail",
            Quantity::VoltageDc,
            Expected::from_user(3.3, 5.0, "V", "用户要求 3.3V"),
        )
    }

    #[test]
    fn plan_round_trips_through_json() {
        let plan = TestPlan::new("tp-1", vec![voltage_step(1)]).unwrap();
        let json = plan.to_json().unwrap();
        let back = TestPlan::from_json(&json).unwrap();
        assert_eq!(plan, back);
    }

    #[test]
    fn empty_plan_is_rejected() {
        assert_eq!(TestPlan::new("tp-1", vec![]).unwrap_err(), PlanError::Empty);
    }

    #[test]
    fn duplicate_ordinals_are_rejected() {
        let plan = TestPlan::new("tp-1", vec![voltage_step(1), voltage_step(1)]);
        assert_eq!(plan.unwrap_err(), PlanError::DuplicateIndex(1));
    }

    #[test]
    fn expected_value_carries_its_source() {
        let user = Expected::from_user(3.3, 5.0, "V", "r");
        assert_eq!(user.source, Baseline::User);
        assert_eq!(user.tol_absolute(), 0.165);

        let sim = Expected::from_simulation("voltage-divider", "VOUT", "op", 2.449, 1.0, "V", "r");
        match &sim.source {
            Baseline::Simulation { circuit, node, .. } => {
                assert_eq!(circuit, "voltage-divider");
                assert_eq!(node, "VOUT");
            }
            other => panic!("expected simulation baseline, got {other:?}"),
        }
        assert!(sim.source.describe().contains("仿真基准"));
    }

    #[test]
    fn powered_steps_are_identifiable() {
        let passive = voltage_step(1);
        assert!(!passive.needs_power());
        let powered = voltage_step(2).powered(5.0, 0.2).with_expected_current(0.05);
        assert!(powered.needs_power());
        assert_eq!(powered.supply.unwrap().expected_current, Some(0.05));
    }

    #[test]
    fn plan_from_a_recipe_uses_simulation_baselines() {
        let plan = TestPlan::from_recipe("voltage-divider", None).unwrap();
        assert!(!plan.steps.is_empty());
        let step = &plan.steps[0];
        assert_eq!(step.quantity, Quantity::VoltageDc);
        assert!(matches!(step.expected.source, Baseline::Simulation { .. }));
        assert!(step.needs_power(), "verifying a rail requires energising");
        assert!(plan.notes.iter().any(|n| n.contains("仿真基准")));
    }

    #[test]
    fn plan_from_an_ac_recipe_maps_cutoff_to_frequency() {
        let plan = TestPlan::from_recipe("rc-lowpass", None).unwrap();
        assert!(
            plan.steps.iter().any(|s| s.quantity == Quantity::Frequency),
            "cutoff frequency must become a frequency step"
        );
        // The dimensionless passband gain cannot be measured with a DMM.
        assert!(
            plan.notes.iter().any(|n| n.contains("无法用当前仪器")),
            "unmeasurable metrics must be recorded as notes: {:?}",
            plan.notes
        );
    }

    #[test]
    fn unknown_circuit_and_recipe_are_errors() {
        assert!(matches!(
            TestPlan::from_recipe("nope", None),
            Err(PlanError::UnknownCircuit(_))
        ));
        assert!(matches!(
            TestPlan::from_recipe("rc-lowpass", Some("no such recipe")),
            Err(PlanError::UnknownRecipe { .. })
        ));
    }

    #[test]
    fn json_without_a_unit_is_rejected() {
        let json = r#"{"plan_id":"x","steps":[{"index":1,"target":"t","quantity":"VoltageDc",
            "channel":null,"samples":null,"reduce":"Mean","supply":null,
            "expected":{"value":1.0,"tol_pct":1.0,"unit":"","source":{"kind":"User"},"rationale":"r"}}]}"#;
        assert!(matches!(
            TestPlan::from_json(json),
            Err(PlanError::MissingExpected { .. })
        ));
    }
}
