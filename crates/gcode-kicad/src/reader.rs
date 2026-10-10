//! Read an existing `.kicad_sch` back into `NetlistIr`.
//!
//! This is the inverse of [`crate::schematic`] and the enabler for editing a
//! design the user (or KiCad itself) already has: components are recovered from
//! the symbol instances, and connectivity from labels matched against the pin
//! connection points computed from the embedded symbol definitions.
//!
//! Only the subset Gcode generates (and KiCad preserves) is understood:
//! `lib_symbols`, top-level `symbol` instances with `lib_id`, and `label`s.
//! Anything else is ignored rather than failing the read.

use crate::netlist::{Component, Net, NetlistIr};
use crate::sexpr::Sexpr;
use crate::symbols::pins_of;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("s-expression parse error: {0}")]
    Parse(#[from] crate::sexpr::ParseError),
    #[error("document root is not (kicad_sch ...)")]
    NotASchematic,
    #[error("no symbol definitions or instances found")]
    Empty,
}

/// Coordinate key with millimetre-level tolerance.
fn coord_key(x: f64, y: f64) -> (i64, i64) {
    ((x * 1000.0).round() as i64, (y * 1000.0).round() as i64)
}

fn num_at(node: &Sexpr, idx: usize) -> Option<f64> {
    node.as_list()?.get(idx)?.as_str()?.parse().ok()
}

fn prop(node: &Sexpr, name: &str) -> Option<String> {
    node.find_all("property")
        .into_iter()
        .find(|p| p.as_list().and_then(|l| l.get(1)).and_then(Sexpr::as_str) == Some(name))
        .and_then(|p| p.as_list()?.get(2))
        .and_then(Sexpr::as_str)
        .map(str::to_string)
}

/// Recover a `NetlistIr` from schematic text.
pub fn read_netlist(text: &str) -> Result<NetlistIr, ReadError> {
    let docs = Sexpr::parse(text)?;
    let root = docs.first().ok_or(ReadError::Empty)?;
    if root.tag() != Some("kicad_sch") {
        return Err(ReadError::NotASchematic);
    }

    // lib_id -> pins (from the embedded definitions)
    let mut lib_pins: BTreeMap<String, Vec<crate::symbols::PinInfo>> = BTreeMap::new();
    if let Some(libs) = root.find("lib_symbols") {
        for sym in libs.find_all("symbol") {
            if let Some(name) = sym.as_list().and_then(|l| l.get(1)).and_then(Sexpr::as_str) {
                lib_pins.insert(name.to_string(), pins_of(sym));
            }
        }
    }

    // Labels: coordinate -> net name
    let mut label_at: BTreeMap<(i64, i64), String> = BTreeMap::new();
    for label in root.find_all("label") {
        let name = label
            .as_list()
            .and_then(|l| l.get(1))
            .and_then(Sexpr::as_str)
            .unwrap_or("")
            .to_string();
        let at = label.find("at");
        let (Some(x), Some(y)) = (
            at.and_then(|a| num_at(a, 1)),
            at.and_then(|a| num_at(a, 2)),
        ) else {
            continue;
        };
        label_at.entry(coord_key(x, y)).or_insert(name);
    }

    let mut components = Vec::new();
    let mut net_conns: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();

    for (idx, sym) in root.find_all("symbol").into_iter().enumerate() {
        let Some(lib_id) = sym.get("lib_id").and_then(Sexpr::as_str) else {
            continue;
        };
        let lib_id = lib_id.to_string();
        let reference = prop(sym, "Reference").unwrap_or_else(|| format!("U{idx}"));
        let value = prop(sym, "Value").unwrap_or_default();
        let footprint = prop(sym, "Footprint").filter(|s| !s.is_empty());

        // NB: `Sexpr::get` returns the first argument, so use `find` to reach
        // the `(at x y rot)` list itself.
        let sym_at = sym.find("at");
        let sym_x = sym_at.and_then(|a| num_at(a, 1)).unwrap_or(0.0);
        let sym_y = sym_at.and_then(|a| num_at(a, 2)).unwrap_or(0.0);

        components.push(Component {
            reference: reference.clone(),
            lib_id: lib_id.clone(),
            value,
            footprint,
        });

        // Match each pin's connection point against the labels.
        if let Some(pins) = lib_pins.get(&lib_id) {
            for p in pins {
                // Same transform as the writer: library Y is up, sheet Y is down.
                let key = coord_key(sym_x + p.x, sym_y - p.y);
                if let Some(net) = label_at.get(&key) {
                    // Power annotations (#FLG, #PWR...) are recovered too: the
                    // reader must report what the file contains, so that an edit
                    // staged against it round-trips exactly.
                    net_conns
                        .entry(net.clone())
                        .or_default()
                        .push((reference.clone(), p.number.clone()));
                }
            }
        }
    }

    if components.is_empty() {
        return Err(ReadError::Empty);
    }

    let nets: Vec<Net> = net_conns
        .into_iter()
        .map(|(name, mut connections)| {
            connections.sort();
            connections.dedup();
            Net { name, connections }
        })
        .collect();

    Ok(NetlistIr::new(components, nets))
}

/// Read a `.kicad_sch` file into a netlist.
pub fn read_netlist_file(path: &std::path::Path) -> Result<NetlistIr, ReadFileError> {
    let text = std::fs::read_to_string(path)?;
    Ok(read_netlist(&text)?)
}

#[derive(Debug, thiserror::Error)]
pub enum ReadFileError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Read(#[from] ReadError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;
    use crate::schematic::{generate_with_report, Layout};

    #[test]
    fn round_trips_every_catalog_circuit() {
        for entry in catalog::all() {
            let text = generate_with_report(&entry.ir, Layout::default())
                .unwrap()
                .content;
            let read = read_netlist(&text).unwrap_or_else(|e| panic!("{}: {e}", entry.id));

            let mut want_refs: Vec<&str> = entry.ir.references();
            want_refs.sort();
            let mut got_refs: Vec<&str> = read.references();
            got_refs.sort();
            assert_eq!(got_refs, want_refs, "{}: components differ", entry.id);

            // The read must reproduce the IR exactly, power annotations included.
            for want in &entry.ir.nets {
                let expected: Vec<(String, String)> = want.connections.clone();
                let got = read
                    .nets
                    .iter()
                    .find(|n| n.name == want.name)
                    .map(|n| {
                        let mut c = n.connections.clone();
                        c.sort();
                        c
                    })
                    .unwrap_or_default();
                let mut expected = expected;
                expected.sort();
                assert_eq!(got, expected, "{}: net {} differs", entry.id, want.name);
            }
        }
    }

    #[test]
    fn recovers_component_fields() {
        let ir = catalog::by_id("voltage-divider").unwrap().ir;
        let text = generate_with_report(&ir, Layout::default()).unwrap().content;
        let read = read_netlist(&text).unwrap();
        let r1 = read.component("R1").expect("R1 present");
        assert_eq!(r1.lib_id, "Device:R");
        assert_eq!(r1.value, "39k");
    }

    #[test]
    fn rejects_non_schematic() {
        assert!(matches!(
            read_netlist("(netlist (components))"),
            Err(ReadError::NotASchematic)
        ));
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(read_netlist("(kicad_sch"), Err(ReadError::Parse(_))));
    }
}
