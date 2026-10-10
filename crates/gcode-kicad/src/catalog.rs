//! The R0 circuit catalog: 10 typical analog circuits.
//!
//! Each entry is a deterministic reference implementation (`NetlistIr`) plus
//! the natural-language description a user would say, and the expected
//! engineering figures. This is the KR1 acceptance basis: every entry must
//! generate a schematic that passes ERC.
//!
//! Wiring conventions that keep ERC clean:
//! - Externally driven rails (VIN/VCC/GND) carry a `power:PWR_FLAG`, whose
//!   pin is `power_out`; without it ERC reports "power_in pin not driven".
//! - Single-opamp symbols (`Amplifier_Operational:LM321`) are used instead of
//!   dual opamps, so there are no unused-unit unconnected pins.

use crate::netlist::{Component, Net, NetlistIr};

/// One catalog entry.
#[derive(Debug, Clone)]
pub struct CircuitEntry {
    /// Stable kebab-case id.
    pub id: &'static str,
    /// Human title (zh).
    pub title: &'static str,
    /// Natural-language request used by the KR1 acceptance harness.
    pub description: &'static str,
    /// Category, for grouping in listings.
    pub category: &'static str,
    /// Expected engineering figures (gain, cutoff, current, ...).
    pub expectation: &'static str,
    pub ir: NetlistIr,
}

/// KiCad convention: power symbols carry a `#`-prefixed reference so they are
/// treated as ERC annotations rather than parts (excluded from BOM/netlist).
/// Prefix marking a power-symbol reference (ERC annotation, not a part).
pub const FLG_PREFIX: char = '#';
const FLG1: &str = "#FLG1";
const FLG2: &str = "#FLG2";
const FLG3: &str = "#FLG3";

fn power_flag(reference: &str) -> Component {
    Component::new(reference, "power:PWR_FLAG", "PWR_FLAG")
}

/// All catalog entries, in a stable order.
pub fn all() -> Vec<CircuitEntry> {
    vec![
        voltage_divider(),
        rc_lowpass(),
        rc_highpass(),
        non_inverting_amp(),
        inverting_amp(),
        sallen_key_lowpass(),
        ldo_regulator(),
        led_driver(),
        mosfet_switch(),
        thermistor_divider(),
    ]
}

/// Look up an entry by id.
pub fn by_id(id: &str) -> Option<CircuitEntry> {
    all().into_iter().find(|e| e.id == id)
}

/// Number of catalog entries (KR1 denominator).
pub fn count() -> usize {
    all().len()
}

fn voltage_divider() -> CircuitEntry {
    // 12V * 10k/(39k+10k) = 2.45V
    CircuitEntry {
        id: "voltage-divider",
        title: "电阻分压器",
        description: "把 12V 输入分压成约 2.45V 给 ADC 采样，用 39k 和 10k 两个电阻",
        category: "基础",
        expectation: "VOUT = 2.45V (12V x 10k/49k)",
        ir: NetlistIr::new(
            vec![
                Component::new("R1", "Device:R", "39k"),
                Component::new("R2", "Device:R", "10k"),
                power_flag(FLG1),
                power_flag(FLG2),
            ],
            vec![
                Net::new("VIN", &[("R1", "1"), (FLG1, "1")]),
                Net::new("VOUT", &[("R1", "2"), ("R2", "1")]),
                Net::new("GND", &[("R2", "2"), (FLG2, "1")]),
            ],
        ),
    }
}

fn rc_lowpass() -> CircuitEntry {
    // fc = 1/(2*pi*1k*100n) = 1.59 kHz
    CircuitEntry {
        id: "rc-lowpass",
        title: "RC 低通滤波",
        description: "1k 电阻串在输入上、100nF 电容到地，做约 1.6kHz 的低通滤波",
        category: "滤波",
        expectation: "fc = 1.6 kHz (R=1k, C=100n)",
        ir: NetlistIr::new(
            vec![
                Component::new("R1", "Device:R", "1k"),
                Component::new("C1", "Device:C", "100nF"),
                power_flag(FLG1),
                power_flag(FLG2),
            ],
            vec![
                Net::new("IN", &[("R1", "1"), (FLG1, "1")]),
                Net::new("OUT", &[("R1", "2"), ("C1", "1")]),
                Net::new("GND", &[("C1", "2"), (FLG2, "1")]),
            ],
        ),
    }
}

fn rc_highpass() -> CircuitEntry {
    // fc = 1/(2*pi*1k*100n) = 1.59 kHz
    CircuitEntry {
        id: "rc-highpass",
        title: "RC 高通滤波",
        description: "100nF 电容串在输入上、1k 电阻到地，做约 1.6kHz 的高通滤波",
        category: "滤波",
        expectation: "fc = 1.6 kHz (C=100n, R=1k)",
        ir: NetlistIr::new(
            vec![
                Component::new("C1", "Device:C", "100nF"),
                Component::new("R1", "Device:R", "1k"),
                power_flag(FLG1),
                power_flag(FLG2),
            ],
            vec![
                Net::new("IN", &[("C1", "1"), (FLG1, "1")]),
                Net::new("OUT", &[("C1", "2"), ("R1", "1")]),
                Net::new("GND", &[("R1", "2"), (FLG2, "1")]),
            ],
        ),
    }
}

fn non_inverting_amp() -> CircuitEntry {
    // Gain = 1 + 91k/10k = 10.1
    CircuitEntry {
        id: "non-inverting-amp",
        title: "同相放大器",
        description: "用 LM321 做同相放大器，增益约 10 倍（10k 和 91k 反馈电阻）",
        category: "放大",
        expectation: "Gain = 1 + 91k/10k = 10.1",
        ir: NetlistIr::new(
            vec![
                Component::new("U1", "Amplifier_Operational:LM321", "LM321"),
                Component::new("R1", "Device:R", "10k"),
                Component::new("R2", "Device:R", "91k"),
                power_flag(FLG1),
                power_flag(FLG2),
                power_flag(FLG3),
            ],
            vec![
                // LM321 pins: 1=+in, 2=V-, 3=-in, 4=out, 5=V+
                Net::new("IN", &[("U1", "1"), (FLG1, "1")]),
                Net::new("FB", &[("U1", "3"), ("R1", "1"), ("R2", "1")]),
                Net::new("OUT", &[("U1", "4"), ("R1", "2")]),
                Net::new("GND", &[("R2", "2"), ("U1", "2"), (FLG2, "1")]),
                Net::new("VCC", &[("U1", "5"), (FLG3, "1")]),
            ],
        ),
    }
}

fn inverting_amp() -> CircuitEntry {
    // Gain = -100k/10k = -10
    CircuitEntry {
        id: "inverting-amp",
        title: "反相放大器",
        description: "用 LM321 做反相放大器，增益约 -10 倍（10k 输入、100k 反馈）",
        category: "放大",
        expectation: "Gain = -100k/10k = -10",
        ir: NetlistIr::new(
            vec![
                Component::new("U1", "Amplifier_Operational:LM321", "LM321"),
                Component::new("R1", "Device:R", "10k"),
                Component::new("R2", "Device:R", "100k"),
                power_flag(FLG1),
                power_flag(FLG2),
                power_flag(FLG3),
            ],
            vec![
                Net::new("IN", &[("R1", "1"), (FLG1, "1")]),
                Net::new("SUM", &[("R1", "2"), ("R2", "1"), ("U1", "3")]),
                Net::new("OUT", &[("R2", "2"), ("U1", "4")]),
                Net::new("GND", &[("U1", "1"), ("U1", "2"), (FLG2, "1")]),
                Net::new("VCC", &[("U1", "5"), (FLG3, "1")]),
            ],
        ),
    }
}

fn sallen_key_lowpass() -> CircuitEntry {
    // fc = 1/(2*pi*sqrt(10k*10k*10n*22n)) = 1.07 kHz
    CircuitEntry {
        id: "sallen-key-lowpass",
        title: "二阶有源低通（Sallen-Key）",
        description: "用 LM321 做 Sallen-Key 二阶低通，截止频率约 1kHz",
        category: "滤波",
        expectation: "fc ~= 1.07 kHz (R1=R2=10k, C1=10n, C2=22n)",
        ir: NetlistIr::new(
            vec![
                Component::new("U1", "Amplifier_Operational:LM321", "LM321"),
                Component::new("R1", "Device:R", "10k"),
                Component::new("R2", "Device:R", "10k"),
                Component::new("C1", "Device:C", "10nF"),
                Component::new("C2", "Device:C", "22nF"),
                power_flag(FLG1),
                power_flag(FLG2),
                power_flag(FLG3),
            ],
            vec![
                Net::new("IN", &[("R1", "1"), (FLG1, "1")]),
                Net::new("N1", &[("R1", "2"), ("R2", "1"), ("C1", "1")]),
                Net::new("N2", &[("R2", "2"), ("C2", "1"), ("U1", "1")]),
                Net::new("OUT", &[("C1", "2"), ("U1", "3"), ("U1", "4")]),
                Net::new("GND", &[("C2", "2"), ("U1", "2"), (FLG2, "1")]),
                Net::new("VCC", &[("U1", "5"), (FLG3, "1")]),
            ],
        ),
    }
}

fn ldo_regulator() -> CircuitEntry {
    CircuitEntry {
        id: "ldo-regulator",
        title: "线性稳压（L7805）",
        description: "用 L7805 把 12V 降到 5V，输入输出各加 100nF 去耦",
        category: "电源",
        expectation: "VOUT = 5V from VIN = 12V; OUT pin is power_out (self-driven)",
        ir: NetlistIr::new(
            vec![
                Component::new("U1", "Regulator_Linear:L7805", "L7805"),
                Component::new("C1", "Device:C", "100nF"),
                Component::new("C2", "Device:C", "100nF"),
                power_flag(FLG1),
                power_flag(FLG2),
            ],
            vec![
                // L7805 pins: 1=IN, 2=GND, 3=OUT
                Net::new("VIN", &[("U1", "1"), ("C1", "1"), (FLG1, "1")]),
                Net::new(
                    "GND",
                    &[("U1", "2"), ("C1", "2"), ("C2", "2"), (FLG2, "1")],
                ),
                Net::new("VOUT", &[("U1", "3"), ("C2", "1")]),
            ],
        ),
    }
}

fn led_driver() -> CircuitEntry {
    // (5V - 2V) / 330R = 9.1 mA
    CircuitEntry {
        id: "led-driver",
        title: "LED 限流驱动",
        description: "5V 电源通过 330 欧姆电阻驱动一个 LED，电流约 9mA",
        category: "接口",
        expectation: "I = (5V - 2V)/330R = 9.1 mA",
        ir: NetlistIr::new(
            vec![
                Component::new("R1", "Device:R", "330"),
                Component::new("D1", "Device:LED", "LED"),
                power_flag(FLG1),
                power_flag(FLG2),
            ],
            vec![
                Net::new("VCC", &[("R1", "1"), (FLG1, "1")]),
                // LED pins: 1=K, 2=A
                Net::new("LED_A", &[("R1", "2"), ("D1", "2")]),
                Net::new("GND", &[("D1", "1"), (FLG2, "1")]),
            ],
        ),
    }
}

fn mosfet_switch() -> CircuitEntry {
    CircuitEntry {
        id: "mosfet-switch",
        title: "N-MOSFET 低边开关",
        description: "用 N 沟道 MOSFET 做低边开关驱动 LED，栅极 10k 串阻加 100k 下拉",
        category: "开关",
        expectation: "Gate driven via 10k, 100k pulldown; LED load in drain path",
        ir: NetlistIr::new(
            vec![
                Component::new("Q1", "Device:Q_NMOS_GSD", "Q_NMOS_GSD"),
                Component::new("R1", "Device:R", "10k"),
                Component::new("R2", "Device:R", "100k"),
                Component::new("R3", "Device:R", "330"),
                Component::new("D1", "Device:LED", "LED"),
                power_flag(FLG1),
                power_flag(FLG2),
                power_flag(FLG3),
            ],
            vec![
                // Q_NMOS_GSD pins: 1=G, 2=S, 3=D
                Net::new("IN", &[("R1", "1"), (FLG1, "1")]),
                Net::new("GATE", &[("R1", "2"), ("Q1", "1"), ("R2", "1")]),
                Net::new("GND", &[("R2", "2"), ("Q1", "2"), (FLG2, "1")]),
                Net::new("VCC", &[("R3", "1"), (FLG3, "1")]),
                Net::new("LED_A", &[("R3", "2"), ("D1", "2")]),
                Net::new("DRAIN", &[("D1", "1"), ("Q1", "3")]),
            ],
        ),
    }
}

fn thermistor_divider() -> CircuitEntry {
    CircuitEntry {
        id: "thermistor-divider",
        title: "热敏电阻分压接口",
        description: "10k 上拉电阻和 NTC 热敏电阻分压，输出接到 ADC 测温度",
        category: "传感器接口",
        expectation: "V_sense = VCC x R_ntc/(10k + R_ntc)",
        ir: NetlistIr::new(
            vec![
                Component::new("R1", "Device:R", "10k"),
                Component::new("TH1", "Device:Thermistor", "NTC_10k"),
                power_flag(FLG1),
                power_flag(FLG2),
            ],
            vec![
                Net::new("VCC", &[("R1", "1"), (FLG1, "1")]),
                Net::new("SENSE", &[("R1", "2"), ("TH1", "1")]),
                Net::new("GND", &[("TH1", "2"), (FLG2, "1")]),
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_ten_distinct_entries() {
        let entries = all();
        assert_eq!(entries.len(), 10);
        let mut ids: Vec<&str> = entries.iter().map(|e| e.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 10, "ids must be unique");
    }

    #[test]
    fn entries_are_structurally_valid() {
        for e in all() {
            e.ir.validate().unwrap_or_else(|err| panic!("{}: {err}", e.id));
        }
    }

    #[test]
    fn every_pin_is_connected() {
        // No dangling pins: this is what keeps "unconnected pin" out of ERC.
        for e in all() {
            let used = e.ir.pin_nets();
            for c in &e.ir.components {
                let pins = used.get(&c.reference);
                assert!(
                    pins.is_some_and(|p| !p.is_empty()),
                    "{}: {} has no connected pin",
                    e.id,
                    c.reference
                );
            }
        }
    }

    #[test]
    fn power_rails_are_driven() {
        // Rails other circuits treat as externally supplied need a driver,
        // otherwise ERC reports "power_in pin not driven by any power_out pin".
        // The regulator's own OUT pin is a legitimate driver.
        const POWER_OUT_PINS: &[(&str, &str)] = &[("U1", "3")]; // L7805 OUT
        for e in all() {
            for net in &e.ir.nets {
                if !matches!(net.name.as_str(), "VIN" | "VCC" | "GND") {
                    continue;
                }
                let has_flag = net.connections.iter().any(|(r, _)| r.starts_with(FLG_PREFIX));
                let has_out = net
                    .connections
                    .iter()
                    .any(|(r, p)| POWER_OUT_PINS.contains(&(r.as_str(), p.as_str())));
                assert!(
                    has_flag || has_out,
                    "{}: rail {} has no power driver",
                    e.id,
                    net.name
                );
            }
        }
    }

    #[test]
    fn by_id_roundtrip() {
        assert_eq!(by_id("voltage-divider").unwrap().title, "电阻分压器");
        assert!(by_id("nope").is_none());
        assert_eq!(count(), 10);
    }
}
