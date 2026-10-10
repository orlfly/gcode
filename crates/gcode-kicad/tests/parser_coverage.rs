//! Parser coverage against *real* KiCad files.
//!
//! The PRD's R0 checklist asks for the s-expression parser to be measured on
//! real projects. By default this walks `/usr/share/kicad` (demos + symbol
//! libraries). Point `GCODE_KICAD_SAMPLE_DIR` at any KiCad checkout to widen it.
//!
//! Run with `--nocapture` to see the coverage summary.

use gcode_kicad::sexpr::Sexpr;
use std::path::{Path, PathBuf};

fn sample_roots() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("GCODE_KICAD_SAMPLE_DIR") {
        return vec![PathBuf::from(dir)];
    }
    vec![
        PathBuf::from("/usr/share/kicad/demos"),
        PathBuf::from("/usr/share/kicad/symbols"),
        PathBuf::from("/usr/share/kicad/footprints"),
    ]
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if matches!(
            p.extension().and_then(|s| s.to_str()),
            Some("kicad_sch") | Some("kicad_sym") | Some("kicad_pcb") | Some("kicad_pro")
        ) {
            out.push(p);
        }
    }
}

#[test]
fn parses_every_real_kicad_file_available() {
    let mut files = Vec::new();
    for root in sample_roots() {
        if root.exists() {
            collect(&root, &mut files);
        }
    }
    if files.is_empty() {
        eprintln!("skip: no KiCad sample files found (set GCODE_KICAD_SAMPLE_DIR)");
        return;
    }
    files.sort();

    let mut ok = 0usize;
    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    let mut total_bytes = 0usize;

    for path in &files {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                failures.push((path.clone(), format!("read error: {e}")));
                continue;
            }
        };
        total_bytes += text.len();
        match Sexpr::parse(&text) {
            Ok(_) => ok += 1,
            Err(e) => failures.push((path.clone(), e.to_string())),
        }
    }

    let rate = 100.0 * ok as f64 / files.len() as f64;
    eprintln!(
        "\nsexpr coverage: {ok}/{} files parsed ({rate:.1}%), {:.1} MiB",
        files.len(),
        total_bytes as f64 / (1024.0 * 1024.0)
    );
    if !failures.is_empty() {
        eprintln!("failures ({}):", failures.len());
        for (p, e) in failures.iter().take(20) {
            eprintln!("  {} :: {e}", p.display());
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} real KiCad files failed to parse",
        failures.len(),
        files.len()
    );
    assert!(files.len() >= 100, "expected a meaningful sample, got {}", files.len());
}
