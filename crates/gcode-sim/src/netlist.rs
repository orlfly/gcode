//! Deterministic SPICE netlist generation from a `NetlistIr` + analysis config.
//!
//! See `design.md` D1–D5: the deck is generated from the IR (not from KiCad's
//! SPICE export, which cannot represent our active devices), node names are
//! normalised with `GND -> 0`, stimuli come from the analysis config, and the
//! `.control` block asks ngspice for machine-readable `wrdata` output.

use crate::models::{self, CardKind};
use gcode_kicad::NetlistIr;
use std::collections::BTreeMap;

/// Frequency sweep shape for AC analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sweep {
    /// Points per decade (`dec`).
    Dec,
    /// Points per octave (`oct`).
    Oct,
    /// Total points, linear (`lin`).
    Lin,
}

impl Sweep {
    fn keyword(self) -> &'static str {
        match self {
            Sweep::Dec => "dec",
            Sweep::Oct => "oct",
            Sweep::Lin => "lin",
        }
    }
}

/// What to analyse.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AnalysisKind {
    /// DC operating point (`.op`).
    Op,
    /// AC sweep in Hz.
    Ac {
        sweep: Sweep,
        points: u32,
        start_hz: f64,
        stop_hz: f64,
    },
    /// Transient analysis in seconds.
    Tran { step_s: f64, stop_s: f64 },
}

impl AnalysisKind {
    /// Short tag used in artifact filenames.
    pub fn tag(&self) -> &'static str {
        match self {
            AnalysisKind::Op => "op",
            AnalysisKind::Ac { .. } => "ac",
            AnalysisKind::Tran { .. } => "tran",
        }
    }

    fn card(&self) -> String {
        match self {
            AnalysisKind::Op => ".op".into(),
            AnalysisKind::Ac {
                sweep,
                points,
                start_hz,
                stop_hz,
            } => format!(
                ".ac {} {} {} {}",
                sweep.keyword(),
                points,
                fmt(*start_hz),
                fmt(*stop_hz)
            ),
            AnalysisKind::Tran { step_s, stop_s } => {
                format!(".tran {} {}", fmt(*step_s), fmt(*stop_s))
            }
        }
    }

    /// Probe vector expressions for this analysis type.
    fn probe_vectors(&self, node: &str) -> Vec<String> {
        match self {
            // AC results are complex; ask for magnitude and phase directly.
            AnalysisKind::Ac { .. } => vec![format!("vm({node})"), format!("vp({node})")],
            _ => vec![format!("v({node})")],
        }
    }
}

/// An independent source placed on a net for the duration of the analysis.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Stimulus {
    /// Constant DC voltage.
    Dc { net: String, volts: f64 },
    /// DC operating point plus a unit AC amplitude (for AC sweeps).
    Ac { net: String, dc: f64, ac: f64 },
    /// Pulse train, for transient analysis.
    Pulse {
        net: String,
        v1: f64,
        v2: f64,
        delay_s: f64,
        rise_s: f64,
        fall_s: f64,
        width_s: f64,
        period_s: f64,
    },
    /// Sine wave, for transient analysis.
    Sin {
        net: String,
        offset: f64,
        amplitude: f64,
        freq_hz: f64,
    },
}

impl Stimulus {
    pub fn net(&self) -> &str {
        match self {
            Stimulus::Dc { net, .. }
            | Stimulus::Ac { net, .. }
            | Stimulus::Pulse { net, .. }
            | Stimulus::Sin { net, .. } => net,
        }
    }
}

/// A complete simulation request: what to analyse, how to excite it, what to
/// look at.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Analysis {
    pub kind: AnalysisKind,
    pub stimuli: Vec<Stimulus>,
    /// Net names from the IR to record.
    pub probes: Vec<String>,
}

impl Analysis {
    pub fn op(stimuli: Vec<Stimulus>, probes: &[&str]) -> Self {
        Self {
            kind: AnalysisKind::Op,
            stimuli,
            probes: probes.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    pub fn ac(
        stimuli: Vec<Stimulus>,
        probes: &[&str],
        points: u32,
        start_hz: f64,
        stop_hz: f64,
    ) -> Self {
        Self {
            kind: AnalysisKind::Ac {
                sweep: Sweep::Dec,
                points,
                start_hz,
                stop_hz,
            },
            stimuli,
            probes: probes.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    pub fn tran(
        stimuli: Vec<Stimulus>,
        probes: &[&str],
        step_s: f64,
        stop_s: f64,
    ) -> Self {
        Self {
            kind: AnalysisKind::Tran { step_s, stop_s },
            stimuli,
            probes: probes.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

/// Generation options.
#[derive(Debug, Clone)]
pub struct DeckOptions {
    /// Circuit title written into the deck.
    pub title: String,
    /// Path passed to `wrdata` (interpreted relative to ngspice's cwd).
    pub wrdata_file: String,
}

impl DeckOptions {
    pub fn new(title: impl Into<String>, wrdata_file: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            wrdata_file: wrdata_file.into(),
        }
    }
}

/// A generated deck plus the bookkeeping the caller needs.
#[derive(Debug, Clone)]
pub struct Deck {
    pub text: String,
    /// IR net name → SPICE node name.
    pub node_map: BTreeMap<String, String>,
    /// SPICE vector expressions written to `wrdata`.
    pub probe_vectors: Vec<String>,
    /// Devices that used an approximate model.
    pub behavioral_devices: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SimError {
    #[error("invalid netlist: {0}")]
    InvalidIr(#[from] gcode_kicad::IrError),
    #[error("no SPICE model registered for {lib_id} (known: {known})")]
    UnknownDevice { lib_id: String, known: String },
    #[error("component {reference} has no connection on pin {pin}")]
    MissingPin { reference: String, pin: String },
    #[error("nets {a:?} and {b:?} both normalise to SPICE node {node:?}")]
    NodeCollision { a: String, b: String, node: String },
    #[error("probe {net:?} is not a net in this circuit (nets: {nets})")]
    UnknownProbe { net: String, nets: String },
    #[error("analysis has no probe")]
    NoProbe,
    #[error("stimulus on unknown net {0:?}")]
    UnknownStimulusNet(String),
    #[error("stimulus on the ground net is not allowed")]
    StimulusOnGround,
}

/// SPICE-safe node name for a net.
pub fn normalize_node(net: &str) -> String {
    if net.eq_ignore_ascii_case("gnd") || net.trim() == "0" {
        return "0".to_string();
    }
    let mut s: String = net
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        s = "n".into();
    }
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.insert(0, 'n');
    }
    s
}

/// KiCad-friendly number formatting shared with the rest of Gcode.
fn fmt(v: f64) -> String {
    let mut s = format!("{v:.9}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

/// Build the node map, rejecting collisions.
fn node_map(ir: &NetlistIr) -> Result<BTreeMap<String, String>, SimError> {
    let mut map = BTreeMap::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new(); // normalized -> net
    for net in &ir.nets {
        let node = normalize_node(&net.name);
        if let Some(other) = seen.get(&node) {
            if other != &net.name {
                return Err(SimError::NodeCollision {
                    a: other.clone(),
                    b: net.name.clone(),
                    node,
                });
            }
        }
        seen.insert(node.clone(), net.name.clone());
        map.insert(net.name.clone(), node);
    }
    Ok(map)
}

/// Generate a SPICE deck for `ir` under `analysis`.
pub fn generate(
    ir: &NetlistIr,
    analysis: &Analysis,
    opts: &DeckOptions,
) -> Result<Deck, SimError> {
    ir.validate()?;
    if analysis.probes.is_empty() {
        return Err(SimError::NoProbe);
    }
    let nodes = node_map(ir)?;

    for probe in &analysis.probes {
        if !nodes.contains_key(probe) {
            return Err(SimError::UnknownProbe {
                net: probe.clone(),
                nets: nodes.keys().cloned().collect::<Vec<_>>().join(", "),
            });
        }
    }
    for st in &analysis.stimuli {
        let Some(node) = nodes.get(st.net()) else {
            return Err(SimError::UnknownStimulusNet(st.net().to_string()));
        };
        if node == "0" {
            return Err(SimError::StimulusOnGround);
        }
    }

    let pin_nets = ir.pin_nets();
    let mut warnings = Vec::new();
    let mut behavioral_devices = Vec::new();
    let mut cards: Vec<String> = Vec::new();
    let mut injected_subckts: Vec<String> = Vec::new();
    let mut injected_models: Vec<String> = Vec::new();

    for c in &ir.components {
        // KiCad power annotations (#FLG1, #PWR01) are ERC bookkeeping, not parts.
        if c.reference.starts_with('#') {
            continue;
        }
        let Some(model) = models::lookup(&c.lib_id) else {
            return Err(SimError::UnknownDevice {
                lib_id: c.lib_id.clone(),
                known: models::supported_lib_ids().join(", "),
            });
        };

        let mut ports = Vec::new();
        for pin in model.pin_order {
            let net = pin_nets
                .get(&c.reference)
                .and_then(|m| m.get(*pin))
                .ok_or_else(|| SimError::MissingPin {
                    reference: c.reference.clone(),
                    pin: (*pin).to_string(),
                })?;
            ports.push(nodes.get(net).cloned().unwrap_or_else(|| normalize_node(net)));
        }

        if model.behavioral {
            behavioral_devices.push(format!("{} ({})", c.reference, model.note));
        }

        let card = match model.kind {
            CardKind::Passive => {
                format!("{}{} {} {}", model.prefix, c.reference, ports.join(" "), c.value)
            }
            CardKind::Diode => format!(
                "{}{} {} {}",
                model.prefix,
                c.reference,
                ports.join(" "),
                model.model_name.unwrap_or_default()
            ),
            CardKind::Mosfet => {
                // The M card needs four nodes; we tie the bulk to the source
                // (three-terminal usage), which is what the schematic expresses.
                let bulk = ports.get(2).cloned().unwrap_or_else(|| "0".into());
                format!(
                    "{}{} {} {} {}",
                    model.prefix,
                    c.reference,
                    ports.join(" "),
                    bulk,
                    model.model_name.unwrap_or_default()
                )
            }
            CardKind::Subckt => {
                let name = model.subckt_name.unwrap_or_default();
                if !injected_subckts.iter().any(|s| s == name) {
                    if let Some(body) = model.subckt {
                        injected_subckts.push(body.to_string());
                    }
                }
                format!("X{} {} {}", c.reference, ports.join(" "), name)
            }
        };
        cards.push(card);

        if let Some(line) = model.model_line {
            if !injected_models.iter().any(|m| m == line) {
                injected_models.push(line.to_string());
            }
        }
    }

    // Stimulus cards, in the order given (deterministic).
    let mut stim_cards = Vec::new();
    for st in &analysis.stimuli {
        let node = nodes.get(st.net()).cloned().unwrap_or_default();
        let name = format!("V_{}", normalize_node(st.net()));
        let card = match st {
            Stimulus::Dc { volts, .. } => format!("{name} {node} 0 DC {}", fmt(*volts)),
            Stimulus::Ac { dc, ac, .. } => {
                format!("{name} {node} 0 DC {} AC {}", fmt(*dc), fmt(*ac))
            }
            Stimulus::Pulse {
                v1,
                v2,
                delay_s,
                rise_s,
                fall_s,
                width_s,
                period_s,
                ..
            } => format!(
                "{name} {node} 0 DC {} PULSE({} {} {} {} {} {} {})",
                fmt(*v1),
                fmt(*v1),
                fmt(*v2),
                fmt(*delay_s),
                fmt(*rise_s),
                fmt(*fall_s),
                fmt(*width_s),
                fmt(*period_s)
            ),
            Stimulus::Sin {
                offset,
                amplitude,
                freq_hz,
                ..
            } => format!(
                "{name} {node} 0 DC {} SIN({} {} {})",
                fmt(*offset),
                fmt(*offset),
                fmt(*amplitude),
                fmt(*freq_hz)
            ),
        };
        stim_cards.push(card);
    }

    // Probe vectors: inputs first so gain can be computed, then the rest.
    let mut probe_vectors = Vec::new();
    let mut ordered: Vec<&String> = Vec::new();
    for st in &analysis.stimuli {
        if analysis.probes.iter().any(|p| p == st.net()) {
            if let Some(p) = analysis.probes.iter().find(|p| *p == st.net()) {
                if !ordered.contains(&p) {
                    ordered.push(p);
                }
            }
        }
    }
    for p in &analysis.probes {
        if !ordered.contains(&p) {
            ordered.push(p);
        }
    }
    for probe in ordered {
        let node = nodes.get(probe).cloned().unwrap_or_default();
        probe_vectors.extend(analysis.kind.probe_vectors(&node));
    }

    if behavioral_devices.is_empty() {
        warnings.push("本电路未使用行为级模型".to_string());
    }

    let mut text = String::new();
    text.push_str(&format!("* Gcode simulation deck — {}\n", opts.title));
    text.push_str(
        "* NOTE: active devices use BEHAVIOURAL macromodels (approximations, not vendor models).\n",
    );
    for d in &behavioral_devices {
        text.push_str(&format!("*   behavioural: {d}\n"));
    }
    text.push_str(&format!(".title {}\n", opts.title));
    for m in &injected_models {
        text.push_str(m);
        text.push('\n');
    }
    for s in &injected_subckts {
        text.push_str(s);
        text.push('\n');
    }
    for c in &cards {
        text.push_str(c);
        text.push('\n');
    }
    for c in &stim_cards {
        text.push_str(c);
        text.push('\n');
    }
    text.push_str(&analysis.kind.card());
    text.push('\n');
    text.push_str(".control\n");
    text.push_str("set wr_singlescale\n");
    text.push_str("set wr_vecnames\n");
    text.push_str("run\n");
    text.push_str(&format!(
        "wrdata {} {}\n",
        opts.wrdata_file,
        probe_vectors.join(" ")
    ));
    text.push_str(".endc\n");
    text.push_str(".end\n");

    Ok(Deck {
        text,
        node_map: nodes,
        probe_vectors,
        behavioral_devices,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gcode_kicad::catalog;

    fn divider_ir() -> NetlistIr {
        catalog::by_id("voltage-divider").unwrap().ir
    }

    fn opts() -> DeckOptions {
        DeckOptions::new("test", "out.dat")
    }

    #[test]
    fn ground_is_node_zero() {
        assert_eq!(normalize_node("GND"), "0");
        assert_eq!(normalize_node("gnd"), "0");
        assert_eq!(normalize_node("0"), "0");
    }

    #[test]
    fn node_names_are_sanitised() {
        assert_eq!(normalize_node("VOUT"), "VOUT");
        assert_eq!(normalize_node("V+"), "V_");
        assert_eq!(normalize_node("3V3"), "n3V3");
        assert_eq!(normalize_node("LED_A"), "LED_A");
    }

    #[test]
    fn node_collision_is_rejected() {
        let ir = NetlistIr::new(
            vec![
                gcode_kicad::Component::new("R1", "Device:R", "1k"),
                gcode_kicad::Component::new("R2", "Device:R", "1k"),
            ],
            vec![
                gcode_kicad::Net::new("A+B", &[("R1", "1"), ("R2", "1")]),
                gcode_kicad::Net::new("A_B", &[("R1", "2"), ("R2", "2")]),
            ],
        );
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: "A+B".into(),
                volts: 1.0,
            }],
            &["A+B"],
        );
        assert!(matches!(
            generate(&ir, &analysis, &opts()),
            Err(SimError::NodeCollision { .. })
        ));
    }

    #[test]
    fn passives_map_to_device_cards() {
        let ir = divider_ir();
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: "VIN".into(),
                volts: 12.0,
            }],
            &["VOUT"],
        );
        let deck = generate(&ir, &analysis, &opts()).unwrap();
        assert!(deck.text.contains("RR1 0 VOUT 39k") || deck.text.contains("RR1 VIN VOUT 39k"));
        assert!(deck.text.contains("RR2 VOUT 0 10k"));
        assert!(deck.text.contains("V_VIN VIN 0 DC 12"));
        assert!(deck.text.contains(".op"));
        // power flags are not simulated
        assert!(!deck.text.contains("FLG"));
    }

    #[test]
    fn ac_analysis_asks_for_magnitude_and_phase() {
        let ir = catalog::by_id("rc-lowpass").unwrap().ir;
        let analysis = Analysis::ac(
            vec![Stimulus::Ac {
                net: "IN".into(),
                dc: 0.0,
                ac: 1.0,
            }],
            &["OUT"],
            20,
            10.0,
            100_000.0,
        );
        let deck = generate(&ir, &analysis, &opts()).unwrap();
        assert!(deck.text.contains(".ac dec 20 10 100000"));
        assert_eq!(deck.probe_vectors, vec!["vm(OUT)", "vp(OUT)"]);
        assert!(deck.text.contains("wrdata out.dat vm(OUT) vp(OUT)"));
        assert!(deck.text.contains("set wr_singlescale"));
        assert!(deck.text.contains("set wr_vecnames"));
    }

    #[test]
    fn transient_analysis_emits_pulse() {
        let ir = catalog::by_id("rc-lowpass").unwrap().ir;
        let analysis = Analysis::tran(
            vec![Stimulus::Pulse {
                net: "IN".into(),
                v1: 0.0,
                v2: 1.0,
                delay_s: 0.0,
                rise_s: 1e-9,
                fall_s: 1e-9,
                width_s: 1e-3,
                period_s: 2e-3,
            }],
            &["OUT"],
            1e-5,
            1e-3,
        );
        let deck = generate(&ir, &analysis, &opts()).unwrap();
        assert!(deck.text.contains(".tran 0.00001 0.001"));
        assert!(deck.text.contains("PULSE(0 1 0 0.000000001"));
        assert_eq!(deck.probe_vectors, vec!["v(OUT)"]);
    }

    #[test]
    fn opamp_injects_subckt_and_is_flagged_behavioral() {
        let ir = catalog::by_id("non-inverting-amp").unwrap().ir;
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: "IN".into(),
                volts: 0.1,
            }],
            &["OUT"],
        );
        let deck = generate(&ir, &analysis, &opts()).unwrap();
        assert!(deck.text.contains(".subckt LM321 INP INN VCC VEE OUT"));
        // XU1 <+in> <-in> <V+> <V-> <out> LM321
        assert!(deck.text.contains("XU1 IN FB VCC 0 OUT LM321"), "{}", deck.text);
        assert!(deck.behavioral_devices.iter().any(|d| d.contains("U1")));
        assert!(deck.text.contains("BEHAVIOURAL macromodels"));
    }

    #[test]
    fn mosfet_uses_letter_pins_and_a_model() {
        let ir = catalog::by_id("mosfet-switch").unwrap().ir;
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: "IN".into(),
                volts: 5.0,
            }],
            &["DRAIN"],
        );
        let deck = generate(&ir, &analysis, &opts()).unwrap();
        assert!(
            deck.text.contains("MQ1 DRAIN GATE 0 0 QNMOD_GCODE"),
            "{}",
            deck.text
        );
        assert!(deck.text.contains(".model QNMOD_GCODE NMOS"));
    }

    #[test]
    fn regulator_injects_behavioral_subckt() {
        let ir = catalog::by_id("ldo-regulator").unwrap().ir;
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: "VIN".into(),
                volts: 12.0,
            }],
            &["VOUT"],
        );
        let deck = generate(&ir, &analysis, &opts()).unwrap();
        assert!(deck.text.contains(".subckt L7805 IN GND OUT"));
        assert!(deck.text.contains("XU1 VIN 0 VOUT L7805"));
    }

    #[test]
    fn unknown_device_is_rejected() {
        let ir = NetlistIr::new(
            vec![
                gcode_kicad::Component::new("R1", "Device:R", "1k"),
                gcode_kicad::Component::new("U9", "SomeLib:Unknown", "X"),
            ],
            vec![
                gcode_kicad::Net::new("A", &[("R1", "1"), ("U9", "1")]),
                gcode_kicad::Net::new("B", &[("R1", "2"), ("U9", "2")]),
            ],
        );
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: "A".into(),
                volts: 1.0,
            }],
            &["B"],
        );
        match generate(&ir, &analysis, &opts()) {
            Err(SimError::UnknownDevice { lib_id, .. }) => assert_eq!(lib_id, "SomeLib:Unknown"),
            other => panic!("expected UnknownDevice, got {other:?}"),
        }
    }

    #[test]
    fn unknown_probe_is_rejected() {
        let ir = divider_ir();
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: "VIN".into(),
                volts: 1.0,
            }],
            &["NOPE"],
        );
        assert!(matches!(
            generate(&ir, &analysis, &opts()),
            Err(SimError::UnknownProbe { .. })
        ));
    }

    #[test]
    fn stimulus_on_ground_is_rejected() {
        let ir = divider_ir();
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: "GND".into(),
                volts: 1.0,
            }],
            &["VOUT"],
        );
        assert_eq!(
            generate(&ir, &analysis, &opts()).unwrap_err(),
            SimError::StimulusOnGround
        );
    }

    #[test]
    fn generation_is_deterministic() {
        let ir = catalog::by_id("sallen-key-lowpass").unwrap().ir;
        let analysis = Analysis::ac(
            vec![Stimulus::Ac {
                net: "IN".into(),
                dc: 0.0,
                ac: 1.0,
            }],
            &["OUT"],
            30,
            10.0,
            100_000.0,
        );
        let a = generate(&ir, &analysis, &opts()).unwrap().text;
        let b = generate(&ir, &analysis, &opts()).unwrap().text;
        assert_eq!(a, b);
    }

    #[test]
    fn no_probe_is_rejected() {
        let ir = divider_ir();
        let analysis = Analysis::op(vec![], &[]);
        assert_eq!(
            generate(&ir, &analysis, &opts()).unwrap_err(),
            SimError::NoProbe
        );
    }
}
