//! Simulation recipes for the R0 circuit catalog, and the run pipeline.
//!
//! A recipe pairs a circuit with an analysis, the excitation, the node to
//! measure, and **analytically derived** expectations with tolerances. The
//! acceptance test compares simulation against those derivations — it never
//! derives an expectation from a previous simulation run (`sim-catalog` spec).

use crate::analysis::{self, Expectation, MetricSet, ResponseShape, Verdict};
use crate::artifacts;
use crate::netlist::{self, Analysis, AnalysisKind, DeckOptions, Stimulus};
use crate::runner::{Ngspice, RunnerError, Waveform};
use gcode_kicad::NetlistIr;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A reproducible simulation recipe for one circuit.
#[derive(Debug, Clone)]
pub struct SimRecipe {
    pub circuit_id: &'static str,
    /// Human name, e.g. "dc operating point".
    pub name: &'static str,
    pub analysis: Analysis,
    /// IR net whose waveform is measured.
    pub output: &'static str,
    /// IR net of the excitation, when a ratio is needed (AC gain).
    pub input: Option<&'static str>,
    pub expectations: Vec<Expectation>,
    /// What the expectations are based on (shown in reports).
    pub basis: &'static str,
}

impl SimRecipe {
    fn metric_set(&self, shape: ResponseShape) -> MetricSet {
        match self.analysis.kind {
            AnalysisKind::Op => MetricSet::Dc,
            AnalysisKind::Ac { .. } => MetricSet::Ac { shape },
            AnalysisKind::Tran { .. } => MetricSet::Tran,
        }
    }

    /// ngspice column name for a net under this analysis.
    fn column(&self, node: &str) -> String {
        match self.analysis.kind {
            AnalysisKind::Ac { .. } => format!("vm({node})"),
            _ => format!("v({node})"),
        }
    }
}

/// Every recipe. Circuits with more than one meaningful operating point (the
/// MOSFET switch) get more than one entry.
pub fn all() -> Vec<SimRecipe> {
    vec![
        SimRecipe {
            circuit_id: "voltage-divider",
            name: "dc operating point",
            analysis: Analysis::op(
                vec![Stimulus::Dc {
                    net: "VIN".into(),
                    volts: 12.0,
                }],
                &["VOUT"],
            ),
            output: "VOUT",
            input: None,
            expectations: vec![Expectation::new(
                "dc_voltage",
                12.0 * 10_000.0 / 49_000.0,
                1.0,
                "V",
                "VOUT = VIN x R2/(R1+R2) = 12 x 10k/49k",
            )],
            basis: "分压公式",
        },
        SimRecipe {
            circuit_id: "rc-lowpass",
            name: "ac sweep",
            analysis: Analysis::ac(
                vec![Stimulus::Ac {
                    net: "IN".into(),
                    dc: 0.0,
                    ac: 1.0,
                }],
                &["IN", "OUT"],
                20,
                10.0,
                1_000_000.0,
            ),
            output: "OUT",
            input: Some("IN"),
            expectations: vec![
                Expectation::new("fc_3db", 1.0 / (2.0 * std::f64::consts::PI * 1_000.0 * 100e-9), 5.0, "Hz", "fc = 1/(2 pi R C), R=1k, C=100n"),
                Expectation::new("gain_linear", 1.0, 2.0, "x", "通带内 RC 低通为单位增益"),
            ],
            basis: "一阶低通解析式",
        },
        SimRecipe {
            circuit_id: "rc-highpass",
            name: "ac sweep",
            analysis: Analysis::ac(
                vec![Stimulus::Ac {
                    net: "IN".into(),
                    dc: 0.0,
                    ac: 1.0,
                }],
                &["IN", "OUT"],
                20,
                10.0,
                1_000_000.0,
            ),
            output: "OUT",
            input: Some("IN"),
            expectations: vec![Expectation::new(
                "fc_3db",
                1.0 / (2.0 * std::f64::consts::PI * 1_000.0 * 100e-9),
                5.0,
                "Hz",
                "fc = 1/(2 pi R C), C=100n, R=1k",
            )],
            basis: "一阶高通解析式",
        },
        SimRecipe {
            circuit_id: "non-inverting-amp",
            name: "dc gain",
            analysis: Analysis::op(
                vec![
                    Stimulus::Dc {
                        net: "IN".into(),
                        volts: 0.1,
                    },
                    Stimulus::Dc {
                        net: "VCC".into(),
                        volts: 5.0,
                    },
                ],
                &["OUT"],
            ),
            output: "OUT",
            input: None,
            expectations: vec![Expectation::new(
                "dc_voltage",
                0.1 * (1.0 + 91_000.0 / 10_000.0),
                5.0,
                "V",
                "VOUT = VIN x (1 + R2/R1) = 0.1 x 10.1",
            )],
            basis: "同相放大器增益式（运放为行为级模型）",
        },
        SimRecipe {
            circuit_id: "inverting-amp",
            name: "dc gain",
            analysis: Analysis::op(
                vec![
                    Stimulus::Dc {
                        net: "IN".into(),
                        volts: 0.1,
                    },
                    Stimulus::Dc {
                        net: "VCC".into(),
                        volts: 5.0,
                    },
                ],
                &["OUT"],
            ),
            output: "OUT",
            input: None,
            expectations: vec![Expectation::new(
                "dc_voltage",
                0.1 * (-100_000.0 / 10_000.0),
                5.0,
                "V",
                "VOUT = -VIN x R2/R1 = -0.1 x 10",
            )],
            basis: "反相放大器增益式（运放为行为级模型）",
        },
        SimRecipe {
            circuit_id: "sallen-key-lowpass",
            name: "ac sweep",
            analysis: Analysis::ac(
                vec![Stimulus::Ac {
                    net: "IN".into(),
                    dc: 0.0,
                    ac: 1.0,
                }],
                &["IN", "OUT"],
                30,
                10.0,
                1_000_000.0,
            ),
            output: "OUT",
            input: Some("IN"),
            // f0 = 1/(2 pi sqrt(R1 R2 C1 C2)) = 1073 Hz, Q = sqrt(C1/C2)/2 = 0.742.
            // For Q = 0.742 the -3 dB point lands at ~1.047 x f0 = 1123 Hz.
            expectations: vec![Expectation::new(
                "fc_3db",
                1123.0,
                10.0,
                "Hz",
                "f0 = 1/(2 pi sqrt(R1 R2 C1 C2)) = 1.073 kHz；Q = sqrt(C1/C2)/2 = 0.742，\
                 二阶低通的 -3dB 约为 1.047 x f0 = 1.123 kHz",
            )],
            basis: "二阶 Sallen-Key 解析式（含 Q 对 -3dB 的影响）",
        },
        SimRecipe {
            circuit_id: "ldo-regulator",
            name: "dc regulation",
            analysis: Analysis::op(
                vec![Stimulus::Dc {
                    net: "VIN".into(),
                    volts: 12.0,
                }],
                &["VOUT"],
            ),
            output: "VOUT",
            input: None,
            expectations: vec![Expectation::new(
                "dc_voltage",
                5.0,
                5.0,
                "V",
                "稳压器输出被限制在 5V（行为级模型 OUT = min(VIN, 5V)）",
            )],
            basis: "稳压器标称输出",
        },
        SimRecipe {
            circuit_id: "led-driver",
            name: "dc operating point",
            analysis: Analysis::op(
                vec![Stimulus::Dc {
                    net: "VCC".into(),
                    volts: 5.0,
                }],
                &["LED_A"],
            ),
            output: "LED_A",
            input: None,
            expectations: vec![Expectation::new(
                "dc_voltage",
                2.10,
                10.0,
                "V",
                "行为级 LED 模型在约 8.8mA 下的正向压降（Is=1e-20, N=1.9, Rs=8）",
            )],
            basis: "行为级 LED 模型的直流工作点（非厂商模型）",
        },
        SimRecipe {
            circuit_id: "mosfet-switch",
            name: "off state",
            // Measure the supply side of the load, not the drain: with the switch
            // off there is no DC path from DRAIN to ground (the LED blocks), so
            // DRAIN is a floating node whose voltage is set by leakage. LED_A is
            // well defined: no current -> no drop across R3 -> LED_A = VCC.
            analysis: Analysis::op(
                vec![
                    Stimulus::Dc {
                        net: "IN".into(),
                        volts: 0.0,
                    },
                    Stimulus::Dc {
                        net: "VCC".into(),
                        volts: 5.0,
                    },
                ],
                &["LED_A"],
            ),
            output: "LED_A",
            input: None,
            expectations: vec![Expectation::new(
                "dc_voltage",
                5.0,
                1.0,
                "V",
                "栅极 0V 时 MOSFET 关断，支路无电流，R3 上无压降，故 LED_A = VCC = 5V                 （此时漏极是浮空节点，其电压由漏电决定，不适合作为判据）",
            )],
            basis: "开关关断时负载支路无电流（测供电侧节点）",
        },
        SimRecipe {
            circuit_id: "mosfet-switch",
            name: "on state",
            analysis: Analysis::op(
                vec![
                    Stimulus::Dc {
                        net: "IN".into(),
                        volts: 5.0,
                    },
                    Stimulus::Dc {
                        net: "VCC".into(),
                        volts: 5.0,
                    },
                ],
                &["DRAIN"],
            ),
            output: "DRAIN",
            input: None,
            // Analytic (triode-region) estimate with the behavioural model:
            // Vds ~ 2.32 V, Id ~ 2.13 mA, LED Vf ~ 1.98 V.
            expectations: vec![Expectation::new(
                "dc_voltage",
                2.32,
                25.0,
                "V",
                "行为级 NMOS（Vto=2V, Kp=0.5m）与 LED/330Ω 支路共同决定的漏极电压；\
                 三极管区解析估算 Vds≈2.32V, Id≈2.13mA，实测约 2.5V（模型相关）",
            )],
            basis: "行为级 MOSFET 三极管区估算（模型相关，容差放宽）",
        },
        SimRecipe {
            circuit_id: "thermistor-divider",
            name: "dc operating point",
            analysis: Analysis::op(
                vec![Stimulus::Dc {
                    net: "VCC".into(),
                    volts: 5.0,
                }],
                &["SENSE"],
            ),
            output: "SENSE",
            input: None,
            expectations: vec![Expectation::new(
                "dc_voltage",
                2.5,
                2.0,
                "V",
                "V_SENSE = VCC x R_ntc/(R1 + R_ntc) = 5 x 10k/20k（热敏电阻按 25C 标称值 10k）",
            )],
            basis: "分压公式（热敏电阻按标称值）",
        },
    ]
}

/// Recipes for one circuit.
pub fn by_circuit(id: &str) -> Vec<SimRecipe> {
    all().into_iter().filter(|r| r.circuit_id == id).collect()
}

/// Circuits covered by at least one recipe.
pub fn covered_circuits() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = all().iter().map(|r| r.circuit_id).collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Response shape for a recipe's expectations (only AC needs it).
fn shape_for(recipe: &SimRecipe) -> ResponseShape {
    if recipe.circuit_id.contains("highpass") {
        ResponseShape::Highpass
    } else {
        ResponseShape::Lowpass
    }
}

/// Result of running one recipe.
#[derive(Debug, Clone)]
pub struct SimOutcome {
    pub circuit_id: String,
    pub recipe: String,
    pub analysis_tag: String,
    pub measured: BTreeMap<String, f64>,
    pub verdict: Verdict,
    pub waveform: Waveform,
    /// Artifact paths written (empty when artifacts were not requested).
    pub artifacts: Vec<PathBuf>,
    pub behavioral_devices: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    #[error("netlist error: {0}")]
    Netlist(#[from] netlist::SimError),
    #[error("simulation error: {0}")]
    Runner(#[from] RunnerError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown circuit {0}")]
    UnknownCircuit(String),
}

/// Generate, run, extract and judge one recipe.
pub async fn run_recipe(
    ngspice: &Ngspice,
    recipe: &SimRecipe,
    ir: &NetlistIr,
    workdir: &Path,
    write_artifacts: bool,
) -> Result<SimOutcome, RecipeError> {
    const WRDATA: &str = "waveform.dat";
    let opts = DeckOptions::new(format!("{} — {}", recipe.circuit_id, recipe.name), WRDATA);
    let deck = netlist::generate(ir, &recipe.analysis, &opts)?;
    let waveform = ngspice.run(workdir, &deck.text, WRDATA).await?;

    let out_node = deck
        .node_map
        .get(recipe.output)
        .cloned()
        .unwrap_or_else(|| netlist::normalize_node(recipe.output));
    let out_col = recipe.column(&out_node);
    let in_col = match recipe.input {
        Some(net) => {
            let node = deck
                .node_map
                .get(net)
                .cloned()
                .unwrap_or_else(|| netlist::normalize_node(net));
            recipe.column(&node)
        }
        None => out_col.clone(),
    };

    let measured = analysis::extract_metrics(
        recipe.metric_set(shape_for(recipe)),
        &waveform,
        &out_col,
        &in_col,
    );
    let verdict = analysis::evaluate(&measured, &recipe.expectations);

    let mut artifacts_written = Vec::new();
    if write_artifacts {
        let tag = recipe.analysis.kind.tag();
        let (csv_name, svg_name) = artifacts::artifact_names(recipe.circuit_id, tag);
        let dir = workdir.join("artifacts");
        std::fs::create_dir_all(&dir)?;
        let csv_path = dir.join(&csv_name);
        artifacts::write_waveform_csv(&csv_path, &waveform)?;
        artifacts_written.push(csv_path);

        let title = format!("{} — {}", recipe.circuit_id, recipe.name);
        let svg = if matches!(recipe.analysis.kind, AnalysisKind::Ac { .. }) {
            artifacts::bode_svg(&waveform, &out_col, &in_col, &title)
        } else {
            artifacts::series_svg(&waveform, &out_col, &title, false)
        };
        if let Some(svg) = svg {
            let svg_path = dir.join(&svg_name);
            std::fs::write(&svg_path, svg)?;
            artifacts_written.push(svg_path);
        }
    }

    Ok(SimOutcome {
        circuit_id: recipe.circuit_id.to_string(),
        recipe: recipe.name.to_string(),
        analysis_tag: recipe.analysis.kind.tag().to_string(),
        measured,
        verdict,
        waveform,
        artifacts: artifacts_written,
        behavioral_devices: deck.behavioral_devices,
        warnings: deck.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gcode_kicad::catalog;

    #[test]
    fn every_recipe_names_a_real_circuit() {
        for r in all() {
            assert!(
                catalog::by_id(r.circuit_id).is_some(),
                "{} is not in the circuit catalog",
                r.circuit_id
            );
        }
    }

    #[test]
    fn every_catalog_circuit_is_covered() {
        let mut expected: Vec<&str> = catalog::all().iter().map(|e| e.id).collect();
        expected.sort();
        assert_eq!(covered_circuits(), expected, "recipe coverage must match the catalog");
    }

    #[test]
    fn every_recipe_has_expectations_with_a_basis() {
        for r in all() {
            assert!(!r.expectations.is_empty(), "{} has no expectations", r.circuit_id);
            assert!(!r.basis.is_empty(), "{} has no basis", r.circuit_id);
            for e in &r.expectations {
                assert!(e.tol_pct > 0.0, "{} {} has no tolerance", r.circuit_id, e.metric);
                assert!(
                    !e.rationale.is_empty(),
                    "{} {} lacks a derivation",
                    r.circuit_id,
                    e.metric
                );
                assert!(
                    analysis::METRIC_INFO.iter().any(|(k, ..)| *k == e.metric),
                    "{} uses unknown metric {}",
                    r.circuit_id,
                    e.metric
                );
            }
        }
    }

    #[test]
    fn every_recipe_probe_is_a_net_of_its_circuit() {
        for r in all() {
            let entry = catalog::by_id(r.circuit_id).unwrap();
            let nets: Vec<&str> = entry.ir.nets.iter().map(|n| n.name.as_str()).collect();
            assert!(
                nets.contains(&r.output),
                "{}: output {} is not a net ({nets:?})",
                r.circuit_id,
                r.output
            );
            if let Some(i) = r.input {
                assert!(nets.contains(&i), "{}: input {i} is not a net", r.circuit_id);
            }
            for st in &r.analysis.stimuli {
                assert!(
                    nets.contains(&st.net()),
                    "{}: stimulus net {} is not a net",
                    r.circuit_id,
                    st.net()
                );
            }
        }
    }

    #[test]
    fn every_recipe_generates_a_deck_offline() {
        // The whole recipe set must be translatable without ngspice.
        for r in all() {
            let entry = catalog::by_id(r.circuit_id).unwrap();
            let opts = DeckOptions::new("test", "waveform.dat");
            let deck = netlist::generate(&entry.ir, &r.analysis, &opts)
                .unwrap_or_else(|e| panic!("{}: {e}", r.circuit_id));
            assert!(!deck.text.is_empty());
            // The analysis card and the wrdata request must be present.
            let card = match r.analysis.kind {
                AnalysisKind::Op => ".op",
                AnalysisKind::Ac { .. } => ".ac ",
                AnalysisKind::Tran { .. } => ".tran ",
            };
            assert!(
                deck.text.contains(card),
                "{}: deck lacks {card}",
                r.circuit_id
            );
            assert!(deck.text.contains("wrdata waveform.dat"));
        }
    }

    #[test]
    fn ac_recipes_sweep_across_their_cutoff() {
        for r in all() {
            if let AnalysisKind::Ac {
                start_hz, stop_hz, ..
            } = r.analysis.kind
            {
                for e in &r.expectations {
                    if e.metric == "fc_3db" {
                        assert!(
                            start_hz < e.expected && e.expected < stop_hz,
                            "{}: cutoff {} outside sweep {}..{}",
                            r.circuit_id,
                            e.expected,
                            start_hz,
                            stop_hz
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn by_circuit_filters() {
        assert_eq!(by_circuit("mosfet-switch").len(), 2);
        assert_eq!(by_circuit("voltage-divider").len(), 1);
        assert!(by_circuit("nope").is_empty());
    }

    #[test]
    fn highpass_uses_the_highpass_shape() {
        let r = by_circuit("rc-highpass").pop().unwrap();
        assert_eq!(shape_for(&r), ResponseShape::Highpass);
        let r = by_circuit("rc-lowpass").pop().unwrap();
        assert_eq!(shape_for(&r), ResponseShape::Lowpass);
    }
}
