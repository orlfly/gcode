//! Device model registry: KiCad `lib_id` → SPICE device card + model.
//!
//! Every model here is a **behavioural macromodel**: it reproduces the device's
//! role and order of magnitude, not the vendor's silicon. That is deliberate
//! (see `design.md` D2): the point is to make the circuit simulatable and to
//! catch topology/design errors, not to certify a part. Callers surface
//! [`Model::behavioral`] so users are never told otherwise.
//!
//! Components whose reference starts with `#` are KiCad power annotations
//! (`#FLG1`, `#PWR01`) and are skipped entirely, not modelled.

/// How a device is emitted into the SPICE deck.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    /// Two-terminal passives: `R1 n1 n2 <value>`.
    Passive,
    /// Diode: `D1 anode cathode <model>`.
    Diode,
    /// MOSFET: `Q1 drain gate source <model>`.
    Mosfet,
    /// Subcircuit instance: `XU1 <ports...> <subckt>`.
    Subckt,
}

/// A registered device model.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// KiCad symbol id this entry handles.
    pub lib_id: &'static str,
    /// SPICE card letter (`R`, `C`, `L`, `D`, `Q`, `X`).
    pub prefix: &'static str,
    pub kind: CardKind,
    /// Pin numbers in the order the SPICE card expects its nodes.
    pub pin_order: &'static [&'static str],
    /// `.subckt` definition to inject (subcircuits only).
    pub subckt: Option<&'static str>,
    /// Subcircuit name to instantiate (subcircuits only).
    pub subckt_name: Option<&'static str>,
    /// `.model` line to inject (diode/mosfet only).
    pub model_line: Option<&'static str>,
    /// Model name used by the card (diode/mosfet only).
    pub model_name: Option<&'static str>,
    /// True when this is an approximation rather than a vendor model.
    pub behavioral: bool,
    /// One-line human note about fidelity.
    pub note: &'static str,
}

/// Single-pole opamp macromodel.
///
/// Physics: open-loop DC gain `A0 = 100k` (100 dB) and a dominant pole at
/// `f_p = 1/(2*pi*Rp*Cp) = 1/(2*pi*1k*15.9155u) = 10 Hz`, giving a unity-gain
/// bandwidth of `A0 * f_p = 1 MHz`. Output stage is a unity buffer with a small
/// series resistance. No rail limiting, no slew limit.
const LM321_SUBCKT: &str = "\
.subckt LM321 INP INN VCC VEE OUT
* Behavioral single-pole opamp macromodel (approximation, not a vendor model).
* A0 = 100k, dominant pole at 10 Hz -> GBW ~ 1 MHz.
Rin INP INN 1Meg
Eg1 n1 VEE INP INN 100k
Rp1 n1 n2 1k
Cp1 n2 VEE 15.9155u
Eout OUT VEE n2 VEE 1
Rout OUT VEE 1k
* Keeps the VCC port from floating when the subcircuit body does not use it.
Rbias VCC VEE 1Meg
.ends LM321";

/// NMOS switch model: threshold 2 V, modest transconductance.
///
/// Note: level-1 MOS in ngspice accepts `CGSO`/`CGDO`/`CGBO`, **not** `Cgs`/`Cgd`
/// (passing those aborts the run with "Error on line N"). Capacitances are
/// omitted deliberately: they do not affect the DC switching checks in R1.
const NMOS_MODEL: &str = ".model QNMOD_GCODE NMOS (Level=1 Vto=2.0 Kp=0.5m)";

/// LED model: ~1.9 V forward drop, series resistance sets ~10 mA at 5 V.
const LED_MODEL: &str = ".model DLED_GCODE D (Is=1e-20 N=1.9 Rs=8)";

/// 5 V linear regulator macromodel: output follows `min(VIN, 5 V)`.
///
/// A B-source is used because the interesting behaviour for design checks is
/// the regulation ceiling, not dropout/loss detail.
const L7805_SUBCKT: &str = "\
.subckt L7805 IN GND OUT
* Behavioral 5 V regulator macromodel (approximation, not a vendor model).
* OUT tracks min(V(IN), 5 V).
B1 OUT GND V = min(V(IN,GND), 5)
.ends L7805";

/// The registry. Order is irrelevant; lookup is by `lib_id`.
pub const MODELS: &[Model] = &[
    Model {
        lib_id: "Device:R",
        prefix: "R",
        kind: CardKind::Passive,
        pin_order: &["1", "2"],
        subckt: None,
        subckt_name: None,
        model_line: None,
        model_name: None,
        behavioral: false,
        note: "ideal resistor",
    },
    Model {
        lib_id: "Device:C",
        prefix: "C",
        kind: CardKind::Passive,
        pin_order: &["1", "2"],
        subckt: None,
        subckt_name: None,
        model_line: None,
        model_name: None,
        behavioral: false,
        note: "ideal capacitor",
    },
    Model {
        lib_id: "Device:L",
        prefix: "L",
        kind: CardKind::Passive,
        pin_order: &["1", "2"],
        subckt: None,
        subckt_name: None,
        model_line: None,
        model_name: None,
        behavioral: false,
        note: "ideal inductor",
    },
    Model {
        lib_id: "Device:Thermistor",
        // Modelled at its nominal resistance (the value field). Temperature
        // dependence is out of scope for R1.
        prefix: "R",
        kind: CardKind::Passive,
        pin_order: &["1", "2"],
        subckt: None,
        subckt_name: None,
        model_line: None,
        model_name: None,
        behavioral: true,
        note: "thermistor modelled as a fixed resistance at its nominal value",
    },
    Model {
        lib_id: "Device:LED",
        // Device:LED pins are 1 = K (cathode), 2 = A (anode); the D card takes
        // anode first, hence this order.
        prefix: "D",
        kind: CardKind::Diode,
        pin_order: &["2", "1"],
        subckt: None,
        subckt_name: None,
        model_line: Some(LED_MODEL),
        model_name: Some("DLED_GCODE"),
        behavioral: true,
        note: "behavioural LED (forward drop ~1.9 V, Rs 8 ohm)",
    },
    Model {
        lib_id: "Device:Q_NMOS",
        // SPICE device letters matter: `Q` is a BJT, MOSFETs use `M`.
        // Device:Q_NMOS pin numbers are letters: D, G, S, and the M card takes
        // drain, gate, source, bulk (the generator ties bulk to source).
        prefix: "M",
        kind: CardKind::Mosfet,
        pin_order: &["D", "G", "S"],
        subckt: None,
        subckt_name: None,
        model_line: Some(NMOS_MODEL),
        model_name: Some("QNMOD_GCODE"),
        behavioral: true,
        note: "behavioural level-1 NMOS (Vto 2.0 V)",
    },
    Model {
        lib_id: "Amplifier_Operational:LM321",
        prefix: "X",
        kind: CardKind::Subckt,
        // LM321 pins: 1 = +in, 2 = V-, 3 = -in, 4 = out, 5 = V+.
        // Subcircuit ports: INP INN VCC VEE OUT.
        pin_order: &["1", "3", "5", "2", "4"],
        subckt: Some(LM321_SUBCKT),
        subckt_name: Some("LM321"),
        model_line: None,
        model_name: None,
        behavioral: true,
        note: "behavioural single-pole opamp (A0 100k, GBW ~1 MHz)",
    },
    Model {
        lib_id: "Regulator_Linear:L7805",
        prefix: "X",
        kind: CardKind::Subckt,
        // L7805 pins: 1 = IN, 2 = GND, 3 = OUT. Ports: IN GND OUT.
        pin_order: &["1", "2", "3"],
        subckt: Some(L7805_SUBCKT),
        subckt_name: Some("L7805"),
        model_line: None,
        model_name: None,
        behavioral: true,
        note: "behavioural 5 V regulator (OUT = min(VIN, 5 V))",
    },
];

/// Look up a device model by KiCad `lib_id`.
pub fn lookup(lib_id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.lib_id == lib_id)
}

/// `lib_id`s the registry can simulate.
pub fn supported_lib_ids() -> Vec<&'static str> {
    MODELS.iter().map(|m| m.lib_id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_devices_resolve() {
        for id in [
            "Device:R",
            "Device:C",
            "Device:L",
            "Device:Thermistor",
            "Device:LED",
            "Device:Q_NMOS",
            "Amplifier_Operational:LM321",
            "Regulator_Linear:L7805",
        ] {
            assert!(lookup(id).is_some(), "{id} missing from registry");
        }
    }

    #[test]
    fn unknown_device_has_no_model() {
        assert!(lookup("Device:Q_NMOS_GSD").is_none());
        assert!(lookup("power:PWR_FLAG").is_none());
    }

    #[test]
    fn active_devices_are_marked_behavioral() {
        // Anything that is not an ideal passive must be labelled approximate.
        for id in [
            "Device:Thermistor",
            "Device:LED",
            "Device:Q_NMOS",
            "Amplifier_Operational:LM321",
            "Regulator_Linear:L7805",
        ] {
            let m = lookup(id).unwrap();
            assert!(m.behavioral, "{id} must be marked behavioral");
            assert!(!m.note.is_empty());
        }
        for id in ["Device:R", "Device:C", "Device:L"] {
            assert!(!lookup(id).unwrap().behavioral);
        }
    }

    #[test]
    fn subckt_entries_are_consistent() {
        for m in MODELS {
            if m.kind == CardKind::Subckt {
                assert!(m.subckt.is_some(), "{} needs a subckt body", m.lib_id);
                let name = m.subckt_name.expect("subckt name");
                assert!(
                    m.subckt.unwrap().contains(&format!(".subckt {name}")),
                    "{} subckt name must match its body",
                    m.lib_id
                );
                assert!(!m.pin_order.is_empty());
            } else {
                assert!(m.subckt.is_none());
            }
            if m.kind == CardKind::Diode || m.kind == CardKind::Mosfet {
                assert!(m.model_line.is_some(), "{} needs a .model", m.lib_id);
                let name = m.model_name.expect("model name");
                assert!(m.model_line.unwrap().contains(name));
            }
        }
    }

    #[test]
    fn mosfets_use_the_m_card_not_q() {
        // `Q` is a BJT in SPICE; using it produces "model type mismatch".
        assert_eq!(lookup("Device:Q_NMOS").unwrap().prefix, "M");
    }

    #[test]
    fn lm321_port_order_matches_its_pins() {
        let m = lookup("Amplifier_Operational:LM321").unwrap();
        // INP INN VCC VEE OUT  ->  +in(1), -in(3), V+(5), V-(2), out(4)
        assert_eq!(m.pin_order, &["1", "3", "5", "2", "4"]);
    }
}
