//! gcode-sim: ngspice-driven simulation for the Gcode agent.
//!
//! Layers:
//! - [`models`]: KiCad `lib_id` → SPICE device card + behavioural macromodel
//! - [`netlist`]: deterministic SPICE deck generation from a `NetlistIr`
//! - [`runner`]: ngspice probe, execution and machine-readable waveform retrieval
//! - [`analysis`]: metric extraction, expectation checking, plain-language verdicts
//! - [`artifacts`]: waveform CSV and dependency-free SVG plots
//! - [`recipes`]: per-circuit simulation recipes and the run pipeline
//!
//! Scope (R1): DC operating point, AC sweeps and transient analysis for the
//! passive/opamp/MOSFET/LED/regulator circuits in the catalog. Active devices
//! use behavioural macromodels — see [`models::Model::behavioral`].

pub mod analysis;
pub mod artifacts;
pub mod models;
pub mod netlist;
pub mod recipes;
pub mod runner;

pub use analysis::{
    cutoff_3db, cutoff_3db_shape, evaluate, extract_metrics, Expectation, MetricSet,
    ResponseShape, Verdict,
};
pub use artifacts::{artifact_names, bode_svg, series_svg, waveform_csv};
pub use models::{lookup as lookup_model, Model};
pub use netlist::{Analysis, AnalysisKind, Deck, DeckOptions, SimError, Stimulus, Sweep};
pub use recipes::{all as all_recipes, by_circuit as recipes_for, run_recipe, SimRecipe};
pub use runner::{Ngspice, RunnerError, Waveform};
