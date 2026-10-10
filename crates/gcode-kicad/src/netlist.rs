//! Netlist-level intermediate representation (IR).
//!
//! The agent layer (LLM) produces `NetlistIr`; deterministic code turns it
//! into KiCad files. LLM never touches s-expressions directly.

use crate::sexpr::Sexpr;
use std::collections::{BTreeMap, BTreeSet};

/// A component instance in the circuit.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Component {
    /// Reference designator: "R1", "C2", "U1"
    pub reference: String,
    /// KiCad symbol library id: "Device:R", "Device:C", "Amplifier_Operational:LM358"
    pub lib_id: String,
    /// Value / part name shown on the schematic ("10k", "LM358").
    pub value: String,
    /// Optional footprint: "Resistor_SMD:R_0603_1608Metric"
    pub footprint: Option<String>,
}

impl Component {
    /// Convenience constructor without a footprint.
    pub fn new(reference: &str, lib_id: &str, value: &str) -> Self {
        Self {
            reference: reference.into(),
            lib_id: lib_id.into(),
            value: value.into(),
            footprint: None,
        }
    }

    /// Convenience constructor with a footprint.
    pub fn with_footprint(
        reference: &str,
        lib_id: &str,
        value: &str,
        footprint: &str,
    ) -> Self {
        Self {
            reference: reference.into(),
            lib_id: lib_id.into(),
            value: value.into(),
            footprint: Some(footprint.into()),
        }
    }
}

/// A net connecting named pins of components. Pin format: "1", "2", "A", "VCC".
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Net {
    pub name: String,
    /// (component reference, pin number/name), sorted for determinism
    pub connections: Vec<(String, String)>,
}

impl Net {
    /// Build a net from `(reference, pin)` pairs.
    pub fn new(name: &str, connections: &[(&str, &str)]) -> Self {
        Self {
            name: name.into(),
            connections: connections
                .iter()
                .map(|(r, p)| ((*r).to_string(), (*p).to_string()))
                .collect(),
        }
    }
}

/// Netlist-level circuit IR — the contract between the LLM layer and the
/// deterministic KiCad generator.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct NetlistIr {
    pub components: Vec<Component>,
    pub nets: Vec<Net>,
}

impl NetlistIr {
    /// Construct from components and nets.
    pub fn new(components: Vec<Component>, nets: Vec<Net>) -> Self {
        Self { components, nets }
    }

    /// Reference designators in declaration order.
    pub fn references(&self) -> Vec<&str> {
        self.components.iter().map(|c| c.reference.as_str()).collect()
    }

    /// Look up a component by reference.
    pub fn component(&self, reference: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.reference == reference)
    }

    /// Set of reference designators, for quick membership checks.
    pub fn reference_set(&self) -> BTreeSet<String> {
        self.components.iter().map(|c| c.reference.clone()).collect()
    }

    /// Validate structural consistency before generation.
    ///
    /// This is the hard gate in front of every file write: an IR that fails
    /// here MUST NOT produce or modify a `.kicad_sch` file.
    pub fn validate(&self) -> Result<(), IrError> {
        let mut refs: BTreeSet<&str> = BTreeSet::new();
        for c in &self.components {
            if c.reference.trim().is_empty() {
                return Err(IrError::EmptyReference);
            }
            if c.lib_id.trim().is_empty() {
                return Err(IrError::EmptyLibId {
                    reference: c.reference.clone(),
                });
            }
            if !refs.insert(c.reference.as_str()) {
                return Err(IrError::DuplicateReference(c.reference.clone()));
            }
        }

        let mut net_names: BTreeSet<&str> = BTreeSet::new();
        // Which net claims each pin, so a pin cannot be shorted across nets.
        let mut pin_owner: BTreeMap<(&str, &str), &str> = BTreeMap::new();

        for net in &self.nets {
            if net.name.trim().is_empty() {
                return Err(IrError::EmptyNetName);
            }
            if !net_names.insert(net.name.as_str()) {
                return Err(IrError::DuplicateNet(net.name.clone()));
            }
            // Named power-style nets (VIN/GND/...) may hang off a single
            // component pin; the external source injects the other side.
            if net.connections.is_empty() {
                return Err(IrError::Unconnected {
                    net: net.name.clone(),
                });
            }
            for (comp, pin) in &net.connections {
                if comp.trim().is_empty() {
                    return Err(IrError::EmptyReference);
                }
                if pin.trim().is_empty() {
                    return Err(IrError::EmptyPin {
                        net: net.name.clone(),
                        comp: comp.clone(),
                    });
                }
                if !refs.contains(comp.as_str()) {
                    return Err(IrError::UnknownComponent {
                        net: net.name.clone(),
                        comp: comp.clone(),
                    });
                }
                if let Some(prev) = pin_owner.insert((comp.as_str(), pin.as_str()), &net.name) {
                    if prev != net.name {
                        return Err(IrError::PinInMultipleNets {
                            comp: comp.clone(),
                            pin: pin.clone(),
                            first: prev.to_string(),
                            second: net.name.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// A canonical form: components sorted by reference, nets sorted by name,
    /// each net's connections sorted. Generation and serialization go through
    /// this so equivalent IRs (any input order) produce identical output.
    pub fn normalized(&self) -> NetlistIr {
        let mut components = self.components.clone();
        components.sort_by(|a, b| {
            a.reference
                .cmp(&b.reference)
                .then_with(|| a.lib_id.cmp(&b.lib_id))
        });
        let mut nets = self.nets.clone();
        for n in &mut nets {
            n.connections.sort();
            n.connections.dedup();
        }
        nets.sort_by(|a, b| a.name.cmp(&b.name));
        NetlistIr { components, nets }
    }

    /// Serialize the IR to a compact s-expression form (audit/diff friendly).
    ///
    /// Output is canonical: independent of input ordering.
    pub fn to_sexpr(&self) -> Sexpr {
        let n = self.normalized();
        let comps: Vec<Sexpr> = n
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
        let nets: Vec<Sexpr> = n
            .nets
            .iter()
            .map(|net| {
                let conns: Vec<Sexpr> = net
                    .connections
                    .iter()
                    .map(|(c, p)| Sexpr::Atom(format!("{c}.{p}")))
                    .collect();
                let mut items = vec![Sexpr::Atom("net".into()), Sexpr::Str(net.name.clone())];
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
            let mut nn = vec![Sexpr::Atom("nets".into())];
            nn.extend(nets);
            nn
        }));
        Sexpr::List(root)
    }

    /// Build a pin-name → net-name map per component, used by the schematic
    /// writer. Order-insensitive (BTreeMap keyed by reference then pin).
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

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum IrError {
    #[error("empty component reference")]
    EmptyReference,
    #[error("component {reference} has an empty lib_id")]
    EmptyLibId { reference: String },
    #[error("duplicate component reference {0}")]
    DuplicateReference(String),
    #[error("net has an empty name")]
    EmptyNetName,
    #[error("duplicate net name {0}")]
    DuplicateNet(String),
    #[error("net {net} references unknown component {comp}")]
    UnknownComponent { net: String, comp: String },
    #[error("net {net} pin on {comp} has an empty pin number")]
    EmptyPin { net: String, comp: String },
    #[error("net {net} has no connections")]
    Unconnected { net: String },
    #[error("pin {comp}.{pin} appears in both net {first} and net {second}")]
    PinInMultipleNets {
        comp: String,
        pin: String,
        first: String,
        second: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    fn divider() -> NetlistIr {
        catalog::by_id("voltage-divider").unwrap().ir.clone()
    }

    #[test]
    fn validate_ok() {
        assert!(divider().validate().is_ok());
    }

    #[test]
    fn validate_duplicate_ref() {
        let mut ir = divider();
        ir.components[1].reference = ir.components[0].reference.clone();
        assert!(matches!(
            ir.validate(),
            Err(IrError::DuplicateReference(_))
        ));
    }

    #[test]
    fn validate_empty_reference() {
        let mut ir = divider();
        ir.components[0].reference = "  ".into();
        assert_eq!(ir.validate(), Err(IrError::EmptyReference));
    }

    #[test]
    fn validate_empty_lib_id() {
        let mut ir = divider();
        ir.components[0].lib_id = "".into();
        assert_eq!(
            ir.validate(),
            Err(IrError::EmptyLibId {
                reference: ir.components[0].reference.clone()
            })
        );
    }

    #[test]
    fn validate_empty_net_name() {
        let mut ir = divider();
        ir.nets[0].name = "".into();
        assert_eq!(ir.validate(), Err(IrError::EmptyNetName));
    }

    #[test]
    fn validate_duplicate_net() {
        let mut ir = divider();
        let dup = ir.nets[0].clone();
        ir.nets.push(dup);
        assert!(matches!(ir.validate(), Err(IrError::DuplicateNet(_))));
    }

    #[test]
    fn validate_underconnected() {
        let ir = NetlistIr::new(
            vec![Component::new("R1", "Device:R", "10k")],
            vec![Net {
                name: "N1".into(),
                connections: vec![],
            }],
        );
        assert!(matches!(ir.validate(), Err(IrError::Unconnected { .. })));
    }

    #[test]
    fn validate_unknown_component() {
        let mut ir = divider();
        ir.nets[0].connections.push(("Q9".into(), "1".into()));
        assert!(matches!(
            ir.validate(),
            Err(IrError::UnknownComponent { .. })
        ));
    }

    #[test]
    fn validate_empty_pin() {
        let mut ir = divider();
        ir.nets[0].connections.push((ir.components[0].reference.clone(), " ".into()));
        assert!(matches!(ir.validate(), Err(IrError::EmptyPin { .. })));
    }

    #[test]
    fn validate_pin_short_across_nets() {
        let ir = NetlistIr::new(
            vec![
                Component::new("R1", "Device:R", "10k"),
                Component::new("R2", "Device:R", "10k"),
            ],
            vec![
                Net::new("A", &[("R1", "1"), ("R2", "1")]),
                Net::new("B", &[("R1", "1"), ("R2", "2")]),
            ],
        );
        assert!(matches!(
            ir.validate(),
            Err(IrError::PinInMultipleNets { .. })
        ));
    }

    #[test]
    fn ir_sexpr_roundtrip_through_parser() {
        let ir = divider();
        let s = ir.to_sexpr().to_string_kicad();
        let reparsed = Sexpr::parse(&s).unwrap();
        assert_eq!(reparsed.len(), 1);
        assert_eq!(reparsed[0].tag(), Some("netlist"));
    }

    #[test]
    fn serialization_is_order_insensitive() {
        let mut a = divider();
        let mut b = divider();
        // reverse component order and rotate the first net's connections
        b.components.reverse();
        b.nets.reverse();
        b.nets[0].connections.reverse();
        assert_ne!(a.components, b.components);
        assert_eq!(
            a.to_sexpr().to_string_kicad(),
            b.to_sexpr().to_string_kicad()
        );

        // normalization also stabilizes the layout-relevant component order
        a.components.reverse();
        assert_eq!(a.normalized().components, b.normalized().components);
    }

    #[test]
    fn pin_nets_map() {
        let ir = divider();
        let m = ir.pin_nets();
        assert_eq!(m["R1"]["1"], "VIN");
        assert_eq!(m["R1"]["2"], "VOUT");
        assert_eq!(m["R2"]["2"], "GND");
    }

    #[test]
    fn catalog_entries_all_validate() {
        for entry in catalog::all() {
            entry
                .ir
                .validate()
                .unwrap_or_else(|e| panic!("{}: {e}", entry.id));
        }
    }
}
