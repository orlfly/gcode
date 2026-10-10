//! Cross-check: our generated SPICE deck must describe the same circuit as
//! KiCad's own `sch export netlist --format spice`.
//!
//! This is the `sim-netlist` spec's topology-consistency requirement. It is
//! limited to **passive-only** circuits on purpose: KiCad emits placeholder
//! nodes (`U1 __U1`, `D1 __D1`) for symbols without SPICE models, so only the
//! purely passive designs are comparable. Node *naming* differs by design (we
//! map GND to `0`, KiCad keeps a net name), so we compare structure, not text.

use gcode_kicad::catalog;
use gcode_sim::netlist::{self, Analysis, DeckOptions, Stimulus};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn have_kicad_cli() -> bool {
    Command::new("kicad-cli")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gcode-sim-xcheck-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Canonical node name: strip KiCad's `/`, upper-case, and fold ground to `0`.
fn canon_node(node: &str) -> String {
    let n = node.trim_start_matches('/');
    if n.eq_ignore_ascii_case("gnd") || n == "0" {
        return "0".into();
    }
    n.to_ascii_uppercase()
}

/// Device letter + reference, e.g. "R1" -> ('R', "R1").
fn split_device(token: &str) -> Option<(char, String)> {
    let mut chars = token.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    if !"RCLDQMXV".contains(letter) {
        return None;
    }
    Some((letter, token.to_string()))
}

/// Parse a SPICE deck into `device -> {canonical nodes}`.
fn device_nodes(deck: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in deck.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('*') || line.starts_with('.') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        // Our stimulus sources are named `V_<net>`; they are test conditions,
        // not part of the circuit, and KiCad never emits them.
        if name.starts_with("V_") || name.starts_with("I_") {
            continue;
        }
        let Some((letter, refname)) = split_device(name) else {
            continue;
        };
        // Node positions differ per card type; take the leading tokens that look
        // like nodes (not numbers-with-units, not model names).
        let nodes: BTreeSet<String> = parts
            .take_while(|t| {
                // Stop at values like "1k"; but "0" is the ground node.
                let is_value = *t != "0"
                    && t.chars().next().is_some_and(|c| c.is_ascii_digit());
                !t.starts_with('.') && !t.contains('=') && !is_value
            })
            .filter(|t| t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '/'))
            .map(|t| canon_node(t))
            .collect();
        if nodes.is_empty() {
            continue;
        }
        out.insert(format!("{letter}{}", &refname[1..]).to_ascii_uppercase(), nodes);
    }
    out
}

fn kicad_spice_export(schematic: &Path) -> Option<String> {
    let out = schematic.with_extension("spice");
    let status = Command::new("kicad-cli")
        .args(["sch", "export", "netlist", "--format", "spice", "-o"])
        .arg(&out)
        .arg(schematic)
        .output()
        .ok()?;
    status.status.success().then(|| std::fs::read_to_string(&out).ok()).flatten()
}

#[test]
fn passive_circuits_agree_with_kicads_own_spice_export() {
    if !have_kicad_cli() {
        eprintln!("skip: kicad-cli not available");
        return;
    }
    // Only circuits whose every device is a plain R/C in both worlds.
    let passive_only = ["voltage-divider", "rc-lowpass", "rc-highpass"];
    let dir = scratch("passives");
    let mut compared = 0;

    for id in passive_only {
        let entry = catalog::by_id(id).expect("catalog entry");

        // 1) Our deck.
        let non_ground: Vec<&str> = entry
            .ir
            .nets
            .iter()
            .map(|n| n.name.as_str())
            .filter(|n| *n != "GND")
            .collect();
        let analysis = Analysis::op(
            vec![Stimulus::Dc {
                net: non_ground[0].to_string(),
                volts: 1.0,
            }],
            &[non_ground[non_ground.len() - 1]],
        );
        let deck = netlist::generate(
            &entry.ir,
            &analysis,
            &DeckOptions::new("crosscheck", "w.dat"),
        )
        .unwrap();
        let ours = device_nodes(&deck.text);

        // 2) KiCad's own SPICE export of the same circuit.
        let sch = dir.join(format!("{id}.kicad_sch"));
        let generated =
            gcode_kicad::generate_with_report(&entry.ir, gcode_kicad::Layout::default()).unwrap();
        std::fs::write(&sch, &generated.content).unwrap();
        let Some(kicad) = kicad_spice_export(&sch) else {
            panic!("{id}: kicad-cli could not export a spice netlist");
        };
        let theirs = device_nodes(&kicad);

        // 3) Compare device -> node sets.
        assert_eq!(
            ours.keys().collect::<Vec<_>>(),
            theirs.keys().collect::<Vec<_>>(),
            "{id}: device sets differ\nours:\n{}\ntheirs:\n{}",
            deck.text,
            kicad
        );
        for (device, nodes) in &ours {
            assert_eq!(
                nodes,
                theirs.get(device).unwrap(),
                "{id}: {device} node set differs\nours:\n{}\ntheirs:\n{}",
                deck.text,
                kicad
            );
        }
        compared += 1;
    }
    assert_eq!(compared, passive_only.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn node_canonicalisation_folds_ground_and_kicad_prefixes() {
    assert_eq!(canon_node("/IN"), "IN");
    assert_eq!(canon_node("GND"), "0");
    assert_eq!(canon_node("/GND"), "0");
    assert_eq!(canon_node("out"), "OUT");
}

#[test]
fn device_parser_reads_a_simple_deck() {
    let deck = "R1 in out 1k\nC1 out 0 100n\n.model X D\n";
    let m = device_nodes(deck);
    assert_eq!(m["R1"], BTreeSet::from(["IN".to_string(), "OUT".to_string()]));
    assert_eq!(m["C1"], BTreeSet::from(["OUT".to_string(), "0".to_string()]));
}
