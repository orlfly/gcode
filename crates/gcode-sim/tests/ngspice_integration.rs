//! Integration tests against a real ngspice.
//!
//! These are the checks that prove the generated decks actually simulate and
//! that the simulated numbers match the analytic expectations. They skip (with
//! a printed note) when ngspice is unavailable, so the offline suite still runs.

use gcode_kicad::catalog;
use gcode_sim::analysis::{self, Expectation, MetricSet, ResponseShape};
use gcode_sim::netlist::{self, Analysis, DeckOptions, Stimulus};
use gcode_sim::recipes;
use gcode_sim::runner::Ngspice;
use std::path::PathBuf;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("tokio runtime")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gcode-sim-it-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_recipe_passes_against_real_ngspice() {
    let rt = runtime();
    let Ok(ngspice) = rt.block_on(Ngspice::probe()) else {
        eprintln!("skip: ngspice not available");
        return;
    };
    let dir = scratch("recipes");
    let mut failures = Vec::new();
    let mut total = 0;

    for recipe in recipes::all() {
        let entry = catalog::by_id(recipe.circuit_id).expect("catalog entry");
        let work = dir.join(format!(
            "{}.{}",
            recipe.circuit_id,
            recipe.name.replace(' ', "_")
        ));
        match rt.block_on(recipes::run_recipe(&ngspice, &recipe, &entry.ir, &work, true)) {
            Ok(o) => {
                total += 1;
                if !o.verdict.passed {
                    failures.push(format!(
                        "{} [{}]: {}",
                        recipe.circuit_id,
                        recipe.name,
                        o.verdict.render()
                    ));
                }
                // Artifacts must exist when requested.
                assert!(
                    !o.artifacts.is_empty(),
                    "{}: no artifacts written",
                    recipe.circuit_id
                );
                for p in &o.artifacts {
                    assert!(p.is_file(), "{}: missing artifact {}", recipe.circuit_id, p.display());
                }
            }
            Err(e) => failures.push(format!(
                "{} [{}]: run failed: {e}",
                recipe.circuit_id, recipe.name
            )),
        }
    }

    assert!(
        failures.is_empty(),
        "{} recipe(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(total, recipes::all().len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dc_ac_and_tran_all_retrieve_waveforms() {
    let rt = runtime();
    let Ok(ngspice) = rt.block_on(Ngspice::probe()) else {
        eprintln!("skip: ngspice not available");
        return;
    };
    let dir = scratch("three-kinds");

    // DC operating point on the divider.
    let divider = catalog::by_id("voltage-divider").unwrap().ir;
    let op = Analysis::op(
        vec![Stimulus::Dc {
            net: "VIN".into(),
            volts: 12.0,
        }],
        &["VOUT"],
    );
    let deck = netlist::generate(&divider, &op, &DeckOptions::new("op", "w.dat")).unwrap();
    let w = rt
        .block_on(ngspice.run(&dir.join("op"), &deck.text, "w.dat"))
        .expect("op run");
    // `.op` has no real independent variable: ngspice writes a placeholder in
    // column 0 (named after the first requested vector). The invariant we rely
    // on is that probes are addressed by name, not by position.
    let scale = w.scale_name().expect("a first column exists").to_string();
    assert_ne!(scale, "v(VOUT)", "the placeholder must not shadow a probe");
    let vout = w.last_value("v(VOUT)").expect("v(VOUT) present");
    assert!((vout - 12.0 * 10.0 / 49.0).abs() < 0.01, "vout {vout}");

    // AC sweep on the RC low pass.
    let lp = catalog::by_id("rc-lowpass").unwrap().ir;
    let ac = Analysis::ac(
        vec![Stimulus::Ac {
            net: "IN".into(),
            dc: 0.0,
            ac: 1.0,
        }],
        &["IN", "OUT"],
        20,
        10.0,
        1_000_000.0,
    );
    let deck = netlist::generate(&lp, &ac, &DeckOptions::new("ac", "w.dat")).unwrap();
    let w = rt
        .block_on(ngspice.run(&dir.join("ac"), &deck.text, "w.dat"))
        .expect("ac run");
    assert!(w.column("vm(OUT)").is_some());
    assert!(w.column("vp(OUT)").is_some());
    assert_eq!(w.scale_name(), Some("frequency"));
    let m = analysis::extract_metrics(
        MetricSet::Ac {
            shape: ResponseShape::Lowpass,
        },
        &w,
        "vm(OUT)",
        "vm(IN)",
    );
    let fc = m["fc_3db"];
    assert!(
        (fc - 1591.55).abs() / 1591.55 < 0.05,
        "cutoff {fc} outside 5% of 1591.55 Hz"
    );
    // Phase comes back in radians (sanity check on the units assumption).
    let phases = w.column("vp(OUT)").unwrap();
    assert!(
        phases.iter().all(|p| p.abs() < std::f64::consts::PI),
        "phases must be radians"
    );

    // Transient on the RC low pass with a step.
    let tran = Analysis::tran(
        vec![Stimulus::Pulse {
            net: "IN".into(),
            v1: 0.0,
            v2: 1.0,
            delay_s: 0.0,
            rise_s: 1e-9,
            fall_s: 1e-9,
            width_s: 5e-3,
            period_s: 10e-3,
        }],
        &["OUT"],
        1e-5,
        5e-3,
    );
    let deck = netlist::generate(&lp, &tran, &DeckOptions::new("tran", "w.dat")).unwrap();
    let w = rt
        .block_on(ngspice.run(&dir.join("tran"), &deck.text, "w.dat"))
        .expect("tran run");
    assert_eq!(w.scale_name(), Some("time"));
    let m = analysis::extract_metrics(MetricSet::Tran, &w, "v(OUT)", "v(OUT)");
    // tau = RC = 1k * 100n = 100 us -> t10..t90 = tau*ln(9) = 219.7 us.
    let tr = m["rise_time_s"];
    let expected = 1e-4 * 9f64.ln();
    assert!(
        (tr - expected).abs() / expected < 0.15,
        "rise time {tr} vs expected {expected}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_simulation_is_reported_not_silently_empty() {
    let rt = runtime();
    let Ok(ngspice) = rt.block_on(Ngspice::probe()) else {
        eprintln!("skip: ngspice not available");
        return;
    };
    let dir = scratch("bad-deck");
    // A deck with a device referencing a model that does not exist must fail
    // loudly rather than returning an empty waveform.
    let bad = "\
.title broken
R1 a 0 1k
M1 a a 0 0 NOSUCHMODEL
V1 a 0 DC 1
.op
.control
set wr_singlescale
set wr_vecnames
run
wrdata w.dat v(a)
.endc
.end
";
    let res = rt.block_on(ngspice.run(&dir, bad, "w.dat"));
    assert!(res.is_err(), "a broken deck must not report success");
    let msg = res.unwrap_err().to_string();
    assert!(!msg.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verdict_requires_simulated_data() {
    // Offline: no data means no PASS, even with expectations present.
    let v = analysis::evaluate(
        &std::collections::BTreeMap::new(),
        &[Expectation::new("dc_voltage", 1.0, 1.0, "V", "test")],
    );
    assert!(!v.passed);
}
