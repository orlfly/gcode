//! Netlist-level intermediate representation (IR).
//!
//! The agent layer (LLM) produces `NetlistIR`; deterministic code turns it
//! into KiCad files. LLM never touches s-expressions directly.

use crate::sexpr::Sexpr;
use std::collections::BTreeMap;

/// A component instance in the circuit.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Component {
    /// Reference designator: "R1", "C2", "U1"
    pub reference: String,
    /// KiCad symbol library id: "Device:R", "Device:C", "Amplifier_Operational:LM358"
    pub lib_id: String,
    /// Reference prefix for auto-numbering when reference is empty
    pub value: String,
    /// Optional footprint: "Resistor_SMD:R_0603_1608Metric"
    pub footprint: Option<String>,
}

/// A net connecting named pins of components. Pin format: "1", "2", "A", "VCC".
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Net {
    pub name: String,
    /// (component reference, pin number/name), sorted for determinism
    pub connections: Vec<(String, String)>,
}

/// Netlist-level circuit IR — the contract between the LLM layer and the
/// deterministic KiCad generator.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct NetlistIr {
    pub components: Vec<Component>,
    pub nets: Vec<Net>,
}

#[derive(Debug, thiserror::Error)]
pub enum IrError {
    #[error("duplicate component reference {0}")]
    DuplicateReference(String),
    #[error("net {net} references unknown component {comp}")]
    UnknownComponent { net: String, comp: String },
    #[error("net {net} has no connections")]
    Unconnected { net: String },
    #[error("empty reference in net {0}")]
    EmptyReference(String),
}

impl NetlistIr {
    /// Validate structural consistency before generation.
    pub fn validate(&self) -> Result<(), IrError> {
        let refs: std::collections::BTreeSet<&str> = self
            .components
            .iter()
            .map(|c| c.reference.as_str())
            .collect();
        if refs.len() != self.components.len() {
            let mut seen = std::collections::BTreeSet::new();
            for c in &self.components {
                if !seen.insert(c.reference.as_str()) {
                    return Err(IrError::DuplicateReference(c.reference.clone()));
                }
            }
        }
        for net in &self.nets {
            // Named power-style nets (VIN/GND/...) may hang off a single
            // component pin; the external source injects the other side.
            if net.connections.is_empty() {
                return Err(IrError::Unconnected { net: net.name.clone() });
            }
            for (comp, _pin) in &net.connections {
                if comp.is_empty() {
                    return Err(IrError::EmptyReference(net.name.clone()));
                }
                if !refs.contains(comp.as_str()) {
                    return Err(IrError::UnknownComponent {
                        net: net.name.clone(),
                        comp: comp.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Serialize the IR to a compact s-expression form (audit/diff friendly).
    pub fn to_sexpr(&self) -> Sexpr {
        let comps: Vec<Sexpr> = self
            .components
            .iter()
            .map(|c| {
                Sexpr::List(vec![
                    Sexpr::Atom("comp".into()),
                    Sexpr::Atom(c.reference.clone()),
                    Sexpr::Atom(c.lib_id.clone()),
                    Sexpr::Str(c.value.clone()),
                ])
            })
            .collect();
        let nets: Vec<Sexpr> = self
            .nets
            .iter()
            .map(|n| {
                let conns: Vec<Sexpr> = n
                    .connections
                    .iter()
                    .map(|(c, p)| Sexpr::Atom(format!("{c}.{p}")))
                    .collect();
                let mut items = vec![Sexpr::Atom("net".into()), Sexpr::Str(n.name.clone())];
                items.extend(conns);
                Sexpr::List(items)
            })
            .collect();
        let mut root = vec![Sexpr::Atom("netlist".into())];
        root.push(Sexpr::List({
            let mut c = vec![Sexpr::Atom("components".into())];
            c.extend(comps);
            c
        }));
        root.push(Sexpr::List({
            let mut n = vec![Sexpr::Atom("nets".into())];
            n.extend(nets);
            n
        }));
        Sexpr::List(root)
    }

    /// Build a pin-name → net-name map per component, used by the schematic writer.
    pub fn pin_nets(&self) -> BTreeMap<String, BTreeMap<String, String>> {
        let mut map: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for net in &self.nets {
            for (comp, pin) in &net.connections {
                map.entry(comp.clone())
                    .or_default()
                    .insert(pin.clone(), net.name.clone());
            }
        }
        map
    }
}

/// Built-in circuit templates the deterministic layer can instantiate
/// (the first 10 typical circuits from PRD KR1 start here).
pub mod templates {
    use super::{Component, Net, NetlistIr};

    /// Voltage divider: VIN --[R1]--+--[R2]-- GND, VOUT at the junction.
    pub fn voltage_divider(r1: &str, r2: &str, vin: &str, vout: &str, gnd: &str) -> NetlistIr {
        NetlistIr {
            components: vec![
                Component {
                    reference: "R1".into(),
                    lib_id: "Device:R".into(),
                    value: r1.to_string(),
                    footprint: None,
                },
                Component {
                    reference: "R2".into(),
                    lib_id: "Device:R".into(),
                    value: r2.to_string(),
                    footprint: None,
                },
            ],
            nets: vec![
                Net {
                    name: vin.into(),
                    connections: vec![("R1".into(), "1".into())],
                },
                Net {
                    name: vout.into(),
                    connections: vec![("R1".into(), "2".into()), ("R2".into(), "1".into())],
                },
                Net {
                    name: gnd.into(),
                    connections: vec![("R2".into(), "2".into())],
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_ok() {
        let ir = templates::voltage_divider("10k", "10k", "VIN", "VOUT", "GND");
        assert!(ir.validate().is_ok());
    }

    #[test]
    fn validate_duplicate_ref() {
        let mut ir = templates::voltage_divider("10k", "10k", "VIN", "VOUT", "GND");
        ir.components[1].reference = "R1".into();
        assert!(matches!(
            ir.validate(),
            Err(IrError::DuplicateReference(_))
        ));
    }

    #[test]
    fn validate_underconnected() {
        let ir = NetlistIr {
            components: vec![Component {
                reference: "R1".into(),
                lib_id: "Device:R".into(),
                value: "10k".into(),
                footprint: None,
            }],
            nets: vec![Net {
                name: "N1".into(),
                connections: vec![],
            }],
        };
        assert!(matches!(ir.validate(), Err(IrError::Unconnected { .. })));
    }

    #[test]
    fn validate_unknown_component() {
        let mut ir = templates::voltage_divider("10k", "10k", "VIN", "VOUT", "GND");
        ir.nets[0].connections.push(("Q9".into(), "1".into()));
        assert!(matches!(
            ir.validate(),
            Err(IrError::UnknownComponent { .. })
        ));
    }

    #[test]
    fn ir_sexpr_roundtrip_through_parser() {
        let ir = templates::voltage_divider("10k", "20k", "VIN", "VOUT", "GND");
        let s = ir.to_sexpr().to_string_kicad();
        let reparsed = Sexpr::parse(&s).unwrap();
        assert_eq!(reparsed.len(), 1);
        assert_eq!(reparsed[0].tag(), Some("netlist"));
    }

    #[test]
    fn pin_nets_map() {
        let ir = templates::voltage_divider("10k", "10k", "VIN", "VOUT", "GND");
        let m = ir.pin_nets();
        assert_eq!(m["R1"]["1"], "VIN");
        assert_eq!(m["R1"]["2"], "VOUT");
        assert_eq!(m["R2"]["2"], "GND");
    }
}
