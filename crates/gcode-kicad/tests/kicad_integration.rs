//! Integration tests against a real `kicad-cli`.
//!
//! These are the only checks that prove the generated files are *actually*
//! KiCad-readable and netlist-correct. They are skipped (with a printed note)
//! when no `kicad-cli` is available, so the offline suite still runs anywhere.

use gcode_kicad::catalog;
use gcode_kicad::erc::{kicad_cli_path, KicadToolchain};
use gcode_kicad::schematic::{generate_with_report, Layout};
use gcode_kicad::sexpr::Sexpr;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn have_cli() -> bool {
    Command::new(kicad_cli_path())
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gcode-integration-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Export a netlist with kicad-cli and return the file content.
fn export_netlist(schematic: &Path) -> Option<String> {
    let out = schematic.with_extension("net");
    let status = Command::new(kicad_cli_path())
        .args([
            "sch",
            "export",
            "netlist",
            "--format",
            "kicadsexpr",
            "-o",
        ])
        .arg(&out)
        .arg(schematic)
        .output()
        .ok()?;
    if !status.status.success() {
        eprintln!(
            "kicad-cli netlist export failed: {}{}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        );
        return None;
    }
    std::fs::read_to_string(&out).ok()
}

/// net name -> set of (reference, pin) as KiCad sees it.
fn parse_netlist(text: &str) -> BTreeMap<String, BTreeSet<(String, String)>> {
    let docs = Sexpr::parse(text).expect("netlist parses");
    let root = docs.first().expect("netlist root");
    let nets_node = root.find("nets").expect("(nets ...) present");
    let mut out: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for net in nets_node.find_all("net") {
        let name = net
            .get("name")
            .and_then(Sexpr::as_str)
            .unwrap_or("")
            .trim_start_matches('/')
            .to_string();
        let mut nodes = BTreeSet::new();
        for node in net.find_all("node") {
            let r = node.get("ref").and_then(Sexpr::as_str).unwrap_or("");
            let p = node.get("pin").and_then(Sexpr::as_str).unwrap_or("");
            nodes.insert((r.to_string(), p.to_string()));
        }
        out.insert(name, nodes);
    }
    out
}

/// Expected nets from the IR, excluding power-flag symbols: KiCad deliberately
/// omits `(power)` symbols from exported netlists.
fn expected_nets(entry: &gcode_kicad::CircuitEntry) -> BTreeMap<String, BTreeSet<(String, String)>> {
    let mut out: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for net in &entry.ir.nets {
        let nodes: BTreeSet<(String, String)> = net
            .connections
            .iter()
            .filter(|(r, _)| !r.starts_with("FLG"))
            .map(|(r, p)| (r.clone(), p.clone()))
            .collect();
        out.insert(net.name.clone(), nodes);
    }
    out
}

#[test]
fn generated_schematics_are_loadable_and_netlist_correct() {
    if !have_cli() {
        eprintln!("skip: kicad-cli not installed");
        return;
    }
    let dir = scratch("netlist");
    let mut checked = 0;
    for entry in catalog::all() {
        let report = generate_with_report(&entry.ir, Layout::default()).unwrap();
        let path = dir.join(format!("{}.kicad_sch", entry.id));
        std::fs::write(&path, &report.content).unwrap();

        let Some(text) = export_netlist(&path) else {
            panic!("{}: kicad-cli could not load the generated schematic", entry.id);
        };
        let actual = parse_netlist(&text);
        let expected = expected_nets(&entry);

        for (name, nodes) in &expected {
            let got = actual.get(name).unwrap_or_else(|| {
                panic!("{}: net {} missing from KiCad netlist", entry.id, name)
            });
            assert_eq!(
                got, nodes,
                "{}: net {} nodes differ (KiCad vs IR)",
                entry.id, name
            );
        }
        // No nets beyond the expected ones (unconnected nets would show here).
        for name in actual.keys() {
            assert!(
                expected.contains_key(name),
                "{}: unexpected net {name} in KiCad netlist",
                entry.id
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 10, "all ten circuits must be verified");
}

#[test]
fn erc_passes_for_catalog_when_toolchain_allows() {
    if !have_cli() {
        eprintln!("skip: kicad-cli not installed");
        return;
    }
    // ERC is only run on a version we trust; the project minimum is KiCad 10.
    // Override with GCODE_KICAD_MIN_VERSION when validating on an older install.
    let probed = tokio::runtime::Runtime::new()
        .expect("tokio runtime")
        .block_on(KicadToolchain::probe());
    match probed {
        Ok(tc) => eprintln!("ERC integration enabled for kicad-cli {}", tc.version),
        Err(e) => {
            eprintln!("skip ERC integration: {e}");
            return;
        }
    }
    let dir = scratch("erc");
    for entry in catalog::all() {
        let report = generate_with_report(&entry.ir, Layout::default()).unwrap();
        let path = dir.join(format!("{}.kicad_sch", entry.id));
        std::fs::write(&path, &report.content).unwrap();
        let erc = run_erc_blocking(&path);
        match erc {
            Some(outcome) => {
                assert!(
                    outcome,
                    "{}: ERC reported violations (see {}.erc.json)",
                    entry.id,
                    path.display()
                );
            }
            None => {
                eprintln!("skip: ERC not runnable for {} (old kicad-cli?)", entry.id);
                return;
            }
        }
    }
}

/// Run `kicad-cli sch erc` synchronously and report whether it was clean.
fn run_erc_blocking(schematic: &Path) -> Option<bool> {
    let report = schematic.with_extension("erc.json");
    let status = Command::new(kicad_cli_path())
        .args([
            "sch",
            "erc",
            "--format",
            "json",
            "--output",
        ])
        .arg(&report)
        .arg(schematic)
        .output()
        .ok()?;
    if !status.status.success() {
        return None;
    }
    let text = std::fs::read_to_string(&report).ok()?;
    let report: gcode_kicad::ErcReport = serde_json::from_str(&text).ok()?;
    Some(report.is_clean())
}
