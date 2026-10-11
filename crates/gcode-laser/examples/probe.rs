//! Diagnostic: parse one file or a directory and print what was found.
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: probe <file-or-dir>...");
        std::process::exit(2);
    }
    let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let target: PathBuf = if paths[0].is_dir() {
        paths[0].clone()
    } else {
        paths[0].parent().unwrap().to_path_buf()
    };
    match gcode_laser::import_paths(&[target]) {
        Ok(set) => {
            println!("files:");
            for f in &set.files {
                println!("  {:?} {}", f.role, f.path.display());
                println!("    {}", f.detail);
            }
            println!("notes:");
            for n in &set.notes {
                println!("  - {n}");
            }
            println!("problems:");
            for p in &set.problems {
                println!("  ! {p}");
            }
            println!(
                "copper={} profile={} drill={}",
                set.copper.is_some(),
                set.profile.is_some(),
                set.drill.is_some()
            );
        }
        Err(e) => println!("import error: {e}"),
    }
}
