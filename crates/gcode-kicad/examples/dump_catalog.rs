//! Dev tool: write every catalog schematic to a directory.
//!
//! Usage: cargo run -p gcode-kicad --example dump_catalog -- <out-dir>
//!
//! Used by the KR1 acceptance harness and by manual `kicad-cli` verification.

use gcode_kicad::catalog;
use gcode_kicad::schematic::{generate_with_report, Layout};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "catalog-out".to_string());
    std::fs::create_dir_all(&out)?;
    for entry in catalog::all() {
        let report = generate_with_report(&entry.ir, Layout::default())?;
        let path = format!("{out}/{}.kicad_sch", entry.id);
        std::fs::write(&path, &report.content)?;
        println!(
            "{:<22} {} components, {} nets, {} warnings{}",
            entry.id,
            entry.ir.components.len(),
            entry.ir.nets.len(),
            report.warnings.len(),
            if report.missing_symbols.is_empty() {
                String::new()
            } else {
                format!(", MISSING {:?}", report.missing_symbols)
            }
        );
        for w in &report.warnings {
            println!("    warn: {w}");
        }
    }
    Ok(())
}
