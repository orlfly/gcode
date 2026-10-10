//! Offline regression: the 10 catalog circuits must reproduce their committed
//! golden schematics byte-for-byte. No LLM and no kicad-cli required.
//!
//! If this fails because of an intended generator change, refresh the fixtures:
//!
//! ```text
//! cargo run -p gcode-kicad --example dump_catalog -- crates/gcode-kicad/tests/fixtures/circuits
//! ```

use gcode_kicad::catalog;
use gcode_kicad::schematic::{generate_with_report, Layout};
use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/circuits")
}

#[test]
fn every_circuit_reproduces_its_golden_schematic() {
    let dir = fixture_dir();
    assert!(dir.is_dir(), "missing fixture dir {}", dir.display());
    for entry in catalog::all() {
        let path = dir.join(format!("{}.kicad_sch", entry.id));
        let golden = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let report = generate_with_report(&entry.ir, Layout::default()).unwrap();
        assert_eq!(
            report.content, golden,
            "{} drifted from its golden fixture (regenerate it if intended)",
            entry.id
        );
        assert!(
            report.missing_symbols.is_empty(),
            "{} resolved no real symbols: {:?}",
            entry.id,
            report.missing_symbols
        );
    }
}

#[test]
fn fixture_directory_has_no_orphans() {
    let dir = fixture_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".kicad_sch"))
        .collect();
    names.sort();
    let mut expected: Vec<String> = catalog::all()
        .iter()
        .map(|e| format!("{}.kicad_sch", e.id))
        .collect();
    expected.sort();
    assert_eq!(names, expected, "fixture set and catalog have diverged");
}

#[test]
fn golden_fixtures_are_deterministic_across_runs() {
    // Regenerating twice in-process must match, independent of the fixtures.
    for entry in catalog::all() {
        let a = generate_with_report(&entry.ir, Layout::default()).unwrap().content;
        let b = generate_with_report(&entry.ir, Layout::default()).unwrap().content;
        assert_eq!(a, b, "{} is not deterministic", entry.id);
        assert!(!a.is_empty());
    }
}
