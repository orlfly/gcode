//! `gcode-laser` — R2 fabrication: real Gerber/Excellon artwork to GRBL laser
//! G-code and an SVG preview.
//!
//! Pipeline: [gerber] + [excellon] parse the fabrication files → [geometry]
//! rasterises the copper, dilates by tool radius + clearance, traces and
//! simplifies the contours → [toolpath] assembles the isolation, drill and
//! outline groups with statistics → [gcode] and [svg] emit the machine file and
//! a preview. [profile] supplies parameter starting points with their sources.
//!
//! Honesty boundary: nothing here has been run on a laser or a GRBL controller.
//! The verification available is geometric and structural (clearance
//! measurement, drill position match, G-code whitelist and range checks).

pub mod excellon;
pub mod gcode;
pub mod geometry;
pub mod gerber;
pub mod profile;
pub mod svg;
pub mod toolpath;

pub use excellon::{DrillFile, Hole};
pub use gcode::{GcodeOptions, LaserMode, Violation};
pub use geometry::{ClearanceCheck, Path, Raster};
pub use gerber::{Bounds, GerberLayer, LayerKind};
pub use profile::{LaserProfile, ProfileOverride, ProfileSource};
pub use toolpath::{
    DrillMode, IsolationParams, JobError, JobLayer, JobStats, LaserJob, MoveKind,
};

use std::path::{Path as FsPath, PathBuf};

/// Which parsed file plays which role in the job.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportedRole {
    Copper,
    Profile,
    Drill,
    /// Parsed, but not used (silk, mask, and similar).
    Ignored { kind: String },
}

/// One file brought in by [`import_paths`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedFile {
    pub path: PathBuf,
    pub role: ImportedRole,
    /// One-line description for reports.
    pub detail: String,
}

/// Everything imported for a job.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportSet {
    pub copper: Option<GerberLayer>,
    /// Index in `files` of the chosen copper file (so a later, better layer can
    /// demote it).
    pub copper_file_idx: Option<usize>,
    pub profile: Option<GerberLayer>,
    pub drill: Option<DrillFile>,
    pub files: Vec<ImportedFile>,
    /// Notes about approximations and constructs we did not fully support.
    pub notes: Vec<String>,
    /// Non-fatal problems (a file that failed to parse, an ambiguity).
    pub problems: Vec<String>,
}

impl ImportSet {
    /// Build the import notes string list, including unsupported constructs.
    pub fn report_notes(&self) -> Vec<String> {
        let mut notes = self.notes.clone();
        for f in &self.files {
            match (&f.role, &f.detail) {
                (ImportedRole::Copper, d) | (ImportedRole::Profile, d) => {
                    notes.push(format!("{}: {d}", f.path.display()));
                }
                (ImportedRole::Drill, d) => {
                    notes.push(format!("{}: {d}", f.path.display()));
                }
                (ImportedRole::Ignored { kind }, d) => {
                    notes.push(format!("{}: 已忽略（{kind}）{d}", f.path.display()));
                }
            }
        }
        notes
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ImportError {
    #[error("路径不存在：{0}")]
    NotFound(String),
    #[error("目录里没有 .gbr/.ger/.gtl/.drl/.xln 之类的制造文件：{0}")]
    NothingToImport(String),
    #[error("读取 {path} 失败：{reason}")]
    Io { path: String, reason: String },
}

/// Import Gerber and drill files from explicit paths or a directory.
///
/// Directories are scanned (non-recursively, sorted for determinism). Copper
/// layers are picked by `TF.FileFunction`; a file that fails to parse becomes a
/// problem entry rather than a silent skip.
pub fn import_paths(paths: &[PathBuf]) -> Result<ImportSet, ImportError> {
    let mut files_to_read: Vec<PathBuf> = Vec::new();
    for p in paths {
        if !p.exists() {
            return Err(ImportError::NotFound(p.display().to_string()));
        }
        if p.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(p)
                .map_err(|e| ImportError::Io {
                    path: p.display().to_string(),
                    reason: e.to_string(),
                })?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|f| f.is_file() && looks_like_fab_file(f))
                .collect();
            entries.sort();
            files_to_read.extend(entries);
        } else {
            files_to_read.push(p.clone());
        }
    }
    if files_to_read.is_empty() {
        return Err(ImportError::NothingToImport(
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }

    let mut set = ImportSet {
        copper: None,
        copper_file_idx: None,
        profile: None,
        drill: None,
        files: Vec::new(),
        notes: Vec::new(),
        problems: Vec::new(),
    };

    for path in files_to_read {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                set.problems
                    .push(format!("{}: 读取失败 {e}", path.display()));
                continue;
            }
        };
        if is_drill_file(&path, &text) {
            match excellon::parse(&text) {
                Ok(d) => {
                    let detail = d.report.summary();
                    if !d.report.unknown_commands.is_empty() {
                        set.notes.push(format!(
                            "{}: 有 {} 行无法识别（已跳过）：{}",
                            path.display(),
                            d.report.unknown_commands.len(),
                            d.report
                                .unknown_commands
                                .iter()
                                .take(3)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" / ")
                        ));
                    }
                    if set.drill.is_some() {
                        set.problems.push(format!(
                            "{}: 已有一个钻孔文件，此文件被忽略",
                            path.display()
                        ));
                    } else {
                        set.files.push(ImportedFile {
                            path: path.clone(),
                            role: ImportedRole::Drill,
                            detail: detail.clone(),
                        });
                        set.drill = Some(d);
                    }
                }
                Err(e) => set
                    .problems
                    .push(format!("{}: 钻孔文件解析失败：{e}", path.display())),
            }
            continue;
        }
        match gerber::parse(&text) {
            Ok(layer) => {
                let detail = layer.report.summary();
                if layer.report.has_unsupported() {
                    set.notes.push(format!(
                        "{}: {}（未完整支持的构造已计入，相关图形不会出现在走刀里）",
                        path.display(),
                        layer.report.summary()
                    ));
                }
                let role = match &layer.kind {
                    LayerKind::Copper => {
                        // A KiCad export directory holds both copper layers. Pick
                        // the top one by default and say so, rather than silently
                        // milling whichever file the directory scan hit first.
                        let existing = set.copper.as_ref().map(|c| c.function.clone());
                        match existing {
                            None => ImportedRole::Copper,
                            Some(existing_function) => {
                                let better = copper_preference(&layer.function)
                                    > copper_preference(&existing_function);
                                if better {
                                    // Demote the previous pick so that exactly one
                                    // file is ever reported as the copper layer.
                                    if let Some(idx) = set.copper_file_idx {
                                        set.files[idx].role = ImportedRole::Ignored {
                                            kind: format!("多余的铜层 {existing_function}"),
                                        };
                                    }
                                    set.problems.push(format!(
                                        "{}: 目录里有多个铜层，已改用此层（{}），原 {} 被忽略",
                                        path.display(),
                                        layer.function,
                                        existing_function
                                    ));
                                    ImportedRole::Copper
                                } else {
                                    set.problems.push(format!(
                                        "{}: 已有铜层 {}，此层被忽略（R2 一次只做一层；如需加工此层请直接指定该文件）",
                                        path.display(),
                                        existing_function
                                    ));
                                    ImportedRole::Ignored {
                                        kind: format!("多余的铜层 {}", layer.function),
                                    }
                                }
                            }
                        }
                    }
                    LayerKind::Profile => {
                        if set.profile.is_some() {
                            set.problems.push(format!(
                                "{}: 已有板框文件，此文件被忽略",
                                path.display()
                            ));
                            ImportedRole::Ignored {
                                kind: "多余的板框".into(),
                            }
                        } else {
                            ImportedRole::Profile
                        }
                    }
                    LayerKind::Other(k) => ImportedRole::Ignored { kind: k.clone() },
                };
                if matches!(role, ImportedRole::Copper) {
                    set.copper = Some(layer);
                    set.copper_file_idx = Some(set.files.len());
                } else if matches!(role, ImportedRole::Profile) {
                    set.profile = Some(layer);
                }
                set.files.push(ImportedFile { path, role, detail });
            }
            Err(e) => set
                .problems
                .push(format!("{}: Gerber 解析失败：{e}", path.display())),
        }
    }
    Ok(set)
}

/// Ranking for choosing between several copper layers: prefer the top copper,
/// which is what a single-sided isolation cut normally targets.
fn copper_preference(function: &str) -> u8 {
    let f = function.to_ascii_lowercase();
    if f.contains("top") || f.contains("l1") {
        2
    } else if f.contains("bot") || f.contains("l2") {
        1
    } else {
        0
    }
}

fn looks_like_fab_file(p: &FsPath) -> bool {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "gbr" | "ger" | "gtl" | "gbl" | "gto" | "gbo" | "gko" | "gts" | "gbs" | "gm1"
            | "drl" | "xln" | "txt" | "nc"
    )
}

fn is_drill_file(path: &FsPath, text: &str) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "drl" | "xln") {
        return true;
    }
    // Excellon bodies rarely contain '%FS'; Gerber always does.
    text.contains("M48") || (text.contains("METRIC") && !text.contains("%FS"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir() -> Option<PathBuf> {
        // Real KiCad exports, one subdirectory per board: create them with
        // `kicad-cli pcb export gerbers|drill`. Skipped when absent.
        let dir = dirs_scratch()?.join("r2/gerber");
        dir.is_dir().then_some(dir)
    }

    fn dirs_scratch() -> Option<PathBuf> {
        std::env::var_os("JCODE_SCRATCH_DIR").map(PathBuf::from)
    }

    #[test]
    fn missing_path_is_an_error() {
        let err = import_paths(&[PathBuf::from("/definitely/not/here.gbr")]).unwrap_err();
        assert!(matches!(err, ImportError::NotFound(_)));
    }

    #[test]
    fn a_directory_without_fab_files_is_an_error() {
        let dir = std::env::temp_dir().join("gcode-laser-empty-import");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("readme.txt.md"), "nothing").unwrap();
        let err = import_paths(&[dir]).unwrap_err();
        assert!(matches!(err, ImportError::NothingToImport(_)));
    }

    #[test]
    fn imports_a_synthetic_set() {
        let dir = std::env::temp_dir().join("gcode-laser-import-synth");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("top.gbr"),
            "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%ADD10C,1.000000*%\nD10*\nX1000000Y1000000D03*\nM02*\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("edge.gbr"),
            "%TF.FileFunction,Profile,NP*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%ADD10C,0.050000*%\nD10*\nX0Y0D02*\nX1000000Y0D01*\nM02*\n",
        )
        .unwrap();
        std::fs::write(dir.join("drill.drl"), "M48\nMETRIC\nT1C0.800\n%\nT1\nX1.0Y1.0\nM30\n")
            .unwrap();
        let set = import_paths(&[dir]).unwrap();
        assert!(set.copper.is_some());
        assert!(set.profile.is_some());
        assert!(set.drill.is_some());
        assert_eq!(set.files.len(), 3);
        assert!(set.problems.is_empty(), "{:?}", set.problems);
    }

    #[test]
    fn a_second_copper_layer_is_reported_not_silently_merged() {
        let dir = std::env::temp_dir().join("gcode-laser-import-two-copper");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.gbr", "b.gbr"] {
            std::fs::write(
                dir.join(name),
                "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%ADD10C,1.000000*%\nD10*\nX1000000Y1000000D03*\nM02*\n",
            )
            .unwrap();
        }
        let set = import_paths(&[dir]).unwrap();
        assert!(set.copper.is_some());
        assert_eq!(set.problems.len(), 1, "{:?}", set.problems);
        assert!(set.problems[0].contains("忽略"));
    }

    #[test]
    fn unsupported_constructs_become_notes() {
        let dir = std::env::temp_dir().join("gcode-laser-import-unsupported");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("top.gbr"),
            "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%SRX2Y2I1.0J1.0*%\n%ADD10C,1.000000*%\nD10*\nX1000000Y1000000D03*\nM02*\n",
        )
        .unwrap();
        let set = import_paths(&[dir]).unwrap();
        assert!(
            set.notes.iter().any(|n| n.contains("步进重复")),
            "{:?}",
            set.notes
        );
        assert!(set.copper.unwrap().report.has_unsupported());
    }

    #[test]
    fn real_kicad_exports_import_when_available() {
        let Some(root) = fixture_dir() else {
            eprintln!("skipping: no real Gerber directory at $JCODE_SCRATCH_DIR/r2/gerber");
            return;
        };
        let mut boards: Vec<PathBuf> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        boards.sort();
        if boards.is_empty() {
            eprintln!("skipping: {} has no board subdirectories", root.display());
            return;
        }
        for dir in boards {
            let set = import_paths(&[dir.clone()]).unwrap();
            assert!(set.copper.is_some(), "{}: expected a copper layer", dir.display());
            assert!(set.drill.is_some(), "{}: expected a drill file", dir.display());
            eprintln!(
                "{}: copper={} profile={} drill={} problems={:?}",
                dir.display(),
                set.copper.is_some(),
                set.profile.is_some(),
                set.drill.is_some(),
                set.problems
            );
        }
    }
}
