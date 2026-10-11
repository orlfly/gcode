//! R2 acceptance: real KiCad exports -> laser job -> G-code + SVG + report.
//!
//! This is *not* a hardware test: there is no laser and no GRBL controller here.
//! What it does verify against real fabrication data:
//!
//! 1. every Gerber/Excellon file exported by `kicad-cli` parses, and the
//!    unsupported-construct report is empty (or lists what was skipped);
//! 2. the isolation toolpath keeps at least the requested clearance from the
//!    copper, measured from the rasterised copper, not assumed;
//! 3. every drill path sits exactly on a hole from the Excellon file;
//! 4. the outline path matches the profile layer's bounding box;
//! 5. the G-code passes structural validation, and the SVG is well formed.
//!
//! Artifacts land in `$JCODE_SCRATCH_DIR/r2/out/<board>/` when that variable is
//! set; otherwise the test still runs but writes nothing.

use gcode_laser::gcode::{self, GcodeOptions};
use gcode_laser::gerber;
use gcode_laser::profile::{self, ProfileOverride, ProfileSource};
use gcode_laser::svg::{self, SvgOptions};
use gcode_laser::toolpath::{self, DrillMode, IsolationParams, JobInputs};
use gcode_laser::{ImportSet, MoveKind};
use std::path::{Path, PathBuf};

fn scratch() -> Option<PathBuf> {
    std::env::var_os("JCODE_SCRATCH_DIR").map(PathBuf::from)
}

fn fixture_dirs() -> Vec<PathBuf> {
    let Some(s) = scratch() else {
        return vec![];
    };
    let root = s.join("r2/gerber");
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&root) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) => vec![],
    };
    dirs.sort();
    dirs
}

fn import(dir: &Path) -> ImportSet {
    gcode_laser::import_paths(&[dir.to_path_buf()])
        .unwrap_or_else(|e| panic!("import {}: {e}", dir.display()))
}

#[test]
fn real_kicad_exports_parse_with_full_coverage() {
    let dirs = fixture_dirs();
    if dirs.is_empty() {
        eprintln!(
            "skipping: no real Gerber directory at $JCODE_SCRATCH_DIR/r2/gerber \
             (export one with `kicad-cli pcb export gerbers|drill`)"
        );
        return;
    }
    for dir in dirs {
        let set = import(&dir);
        assert!(set.copper.is_some(), "{}: no copper layer found", dir.display());
        assert!(set.profile.is_some(), "{}: no profile layer found", dir.display());
        assert!(set.drill.is_some(), "{}: no drill file found", dir.display());

        let copper_files: Vec<_> = set
            .files
            .iter()
            .filter(|f| matches!(f.role, gcode_laser::ImportedRole::Copper))
            .collect();
        assert_eq!(
            copper_files.len(),
            1,
            "exactly one file may be the copper layer: {:?}",
            set.files
                .iter()
                .map(|f| (f.path.display().to_string(), format!("{:?}", f.role)))
                .collect::<Vec<_>>()
        );

        let copper = set.copper.as_ref().unwrap();
        eprintln!(
            "{}: copper {} flash / {} draw / {} region; report: {}",
            dir.display(),
            copper.flashes.len(),
            copper.draws.len(),
            copper.regions.len(),
            copper.report.summary()
        );
        assert!(
            copper.report.coordinate_format.contains("X46Y46"),
            "KiCad exports 4.6 format: {}",
            copper.report.coordinate_format
        );
        assert!(
            !copper.report.unsupported.contains_key("圆弧 G02/G03"),
            "KiCad writes linearised arcs for these boards: {:?}",
            copper.report.unsupported
        );
        // Nothing may be silently dropped: if something is unsupported it must
        // be visible in the report, which we print above.
        for f in &set.files {
            match &f.role {
                gcode_laser::ImportedRole::Copper | gcode_laser::ImportedRole::Profile => {
                    eprintln!("  copper/profile: {} -> {}", f.path.display(), f.detail)
                }
                gcode_laser::ImportedRole::Drill => {
                    eprintln!("  drill: {} -> {}", f.path.display(), f.detail)
                }
                gcode_laser::ImportedRole::Ignored { kind } => {
                    eprintln!("  ignored ({kind}): {}", f.path.display())
                }
            }
        }
    }
}

/// Build a job from a real import set with the knowledge-base parameters.
fn job_from(set: &ImportSet, name: &str, clearance: f64) -> toolpath::LaserJob {
    let kb = |id: &str| {
        profile::profiles()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("profile {id} missing"))
    };
    let src = |id: &str| ProfileSource::KnowledgeBase {
        id: id.into(),
        source: kb(id).source.clone(),
    };
    let isolation = kb("fr4-1.6-isolation");
    let params = IsolationParams {
        tool_diameter: 0.1,
        clearance,
        resolution: 0.05,
        simplify_tolerance: 0.02,
    };
    toolpath::build_job(JobInputs {
        name: name.to_string(),
        copper: set.copper.as_ref(),
        profile_layer: set.profile.as_ref(),
        drill: set.drill.as_ref(),
        isolation: params,
        drill_mode: DrillMode::Mark,
        isolation_profile: isolation,
        isolation_source: src("fr4-1.6-isolation"),
        drill_profile: kb("fr4-drillmark"),
        drill_source: src("fr4-drillmark"),
        outline_profile: kb("fr4-outline-1.6"),
        outline_source: src("fr4-outline-1.6"),
        import_notes: set.report_notes(),
    })
    .unwrap_or_else(|e| panic!("build job {name}: {e}"))
}

#[test]
fn acceptance_isolation_clearance_drill_positions_and_outline() {
    let dirs = fixture_dirs();
    if dirs.is_empty() {
        eprintln!("skipping: no real Gerber fixtures available");
        return;
    }
    let mut report = String::new();
    for dir in dirs {
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "board".into());
        let set = import(&dir);
        let clearance = 0.2;
        let job = job_from(&set, &name, clearance);

        // ---- 1. clearance actually measured, not assumed -------------------
        let check = job
            .clearance
            .as_ref()
            .expect("isolation layer built, so a clearance check exists");
        let required = 0.1 / 2.0 + clearance;
        assert!(
            check.passed,
            "{name}: clearance check failed: {check:?} (required {required})"
        );
        assert!(
            check.min_distance + 1e-9 >= required,
            "{name}: measured min distance {} < required {required}",
            check.min_distance
        );

        // ---- 2. every drill path is exactly on an Excellon hole ------------
        let holes = &set.drill.as_ref().unwrap().holes;
        let drill_layer = job
            .layer(MoveKind::Drill)
            .expect("drill layer present when a drill file was imported");
        assert_eq!(drill_layer.paths.len(), holes.len(), "{name}: hole count");
        for hole in holes {
            let (hx, hy) = job.to_job((hole.x, hole.y));
            let found = drill_layer.paths.iter().any(|p| {
                p.start()
                    .map(|(x, y)| (x - hx).abs() < 1e-9 && (y - hy).abs() < 1e-9)
                    .unwrap_or(false)
            });
            assert!(
                found,
                "{name}: no drill path exactly at the Excellon hole ({hx},{hy})"
            );
        }

        // ---- 3. outline matches the profile layer's bounding box -----------
        let pl = set.profile.as_ref().unwrap();
        let pb = pl.bounds().unwrap();
        let outline = job
            .layer(MoveKind::Outline)
            .expect("outline layer present when a profile was imported");
        // The job origin is the lower-left of *everything*, which may sit
        // outside the outline (copper can reach the board edge), so compare in
        // job coordinates rather than assuming the outline starts at (0,0).
        let expect_min = job.to_job((pb.min_x, pb.min_y));
        let expect_max = job.to_job((pb.max_x, pb.max_y));
        let mut min = (f64::INFINITY, f64::INFINITY);
        let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in &outline.paths {
            for (x, y) in &p.points {
                min.0 = min.0.min(*x);
                min.1 = min.1.min(*y);
                max.0 = max.0.max(*x);
                max.1 = max.1.max(*y);
            }
        }
        assert!(
            (min.0 - expect_min.0).abs() < 1e-6 && (min.1 - expect_min.1).abs() < 1e-6,
            "{name}: outline min {min:?} != profile min {expect_min:?}"
        );
        assert!(
            (max.0 - expect_max.0).abs() < 1e-6 && (max.1 - expect_max.1).abs() < 1e-6,
            "{name}: outline max {max:?} != profile max {expect_max:?}"
        );
        eprintln!(
            "{name}: outline {} 条路径（闭合 {} 条），范围 {min:?}..{max:?}",
            outline.paths.len(),
            outline.paths.iter().filter(|p| p.closed).count()
        );

        // ---- 4. G-code and SVG --------------------------------------------
        let gcode_text = gcode::generate(&job, &GcodeOptions::default());
        if let Err(v) = gcode::validate_gcode(&gcode_text, &job) {
            panic!("{name}: G-code validation failed: {v:?}");
        }
        let svg_text = svg::render(&job, &SvgOptions::default());
        assert!(svg_text.starts_with("<svg"));
        assert!(svg_text.trim_end().ends_with("</svg>"));

        // ---- artifacts -----------------------------------------------------
        let mut lines = vec![
            format!("# R2 验收报告 — {name}"),
            String::new(),
            format!("数据来源: {}（kicad-cli pcb export 真实输出）", dir.display()),
            format!(
                "板尺寸: {:.3} x {:.3} mm，切割长度 {:.1} mm，空程 {:.1} mm，预估 {:.1} 分钟",
                job.bounds.width(),
                job.bounds.height(),
                job.stats.cut_length_mm,
                job.stats.travel_length_mm,
                job.stats.estimated_minutes
            ),
            format!(
                "隔离: {} 条路径，{:.1} mm；钻孔: {} 个；板框: {} 条路径，{:.1} mm",
                job.stats.isolation_paths,
                job.stats.isolation_length_mm,
                job.stats.drill_count,
                job.stats.outline_paths,
                job.stats.outline_length_mm
            ),
            format!(
                "几何校验: 隔离路径到铜的最小距离 {:.4} mm（要求 ≥ {:.4} mm，即刀半径 {:.3} + 安全间距 {:.3}）-> {}",
                check.min_distance,
                required,
                check.required - clearance,
                clearance,
                if check.passed { "通过" } else { "未通过" }
            ),
            format!(
                "几何校验: 钻孔路径 {} 条，全部与 Excellon 孔位逐点一致（误差 < 1e-9 mm）",
                drill_layer.paths.len()
            ),
            format!(
                "几何校验: 板框包围盒 {:.3} x {:.3} mm，与 Profile 图层包围盒一致（误差 < 1e-6 mm）",
                max.0, max.1
            ),
            format!(
                "结构校验: G-code 指令白名单/坐标范围/激光开关状态检查通过（{} 行）",
                gcode_text.lines().count()
            ),
            String::new(),
            "## 未验证的部分（必须如实说明）".into(),
            "- 本机没有激光雕刻机，也没有 GRBL 控制器：G-code **没有**在真实机器上跑过，".into(),
            "  没有做过任何实际打样，切削深度/切缝宽度/间距是否够用都未经实测。".into(),
            "- 参数来自知识库条目，未经本机试刻校准。".into(),
            "- 钻孔为标记模式：不会形成通孔，仍需机械钻孔。".into(),
            String::new(),
            "## 作业提示".into(),
        ];
        for n in &job.notes {
            lines.push(format!("- {n}"));
        }
        lines.push(String::new());
        lines.push("## 警告".into());
        for w in &job.warnings {
            lines.push(format!("- {w}"));
        }
        lines.push(String::new());
        lines.push("## G-code 头 20 行".into());
        lines.push("```".into());
        for l in gcode_text.lines().take(20) {
            lines.push(l.to_string());
        }
        lines.push("```".into());
        report.push_str(&lines.join("\n"));
        report.push_str("\n\n");

        if let Some(s) = scratch() {
            let out = s.join("r2/out").join(&name);
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("job.gcode"), &gcode_text).unwrap();
            std::fs::write(out.join("preview.svg"), &svg_text).unwrap();
            std::fs::write(out.join("report.md"), lines.join("\n")).unwrap();
            eprintln!("wrote artifacts to {}", out.display());
        }
    }

    if let Some(s) = scratch() {
        let out = s.join("r2/out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("ACCEPTANCE.md"), &report).unwrap();
        eprintln!("wrote {}", out.join("ACCEPTANCE.md").display());
    }
}

#[test]
fn parameters_can_be_overridden_and_the_change_is_recorded() {
    let dirs = fixture_dirs();
    if dirs.is_empty() {
        eprintln!("skipping: no real Gerber fixtures available");
        return;
    }
    let set = import(&dirs[0]);
    let base = profile::profiles()
        .into_iter()
        .find(|p| p.id == "fr4-1.6-isolation")
        .unwrap();
    let over = ProfileOverride {
        power_s: Some(200),
        speed_mm_min: Some(300),
        passes: Some(3),
        ..Default::default()
    };
    let (p, changed) = profile::apply_overrides(&base, &over);
    assert_eq!(changed.len(), 3);
    let mut job = job_from(&set, "override-test", 0.2);
    job.layers[0].profile = p;
    job.layers[0].source = ProfileSource::KnowledgeBaseWithOverrides {
        id: base.id.clone(),
        source: base.source.clone(),
        overridden: changed.clone(),
    };
    let g = gcode::generate(&job, &GcodeOptions::default());
    assert!(g.contains("用户覆盖"), "{g}");
    assert!(g.contains("S200"), "overridden power must appear");
    assert!(g.contains("F300"), "overridden speed must appear");
    gcode::validate_gcode(&g, &job).expect("overridden job must still validate");
}

#[test]
fn a_copper_only_export_still_produces_a_usable_job() {
    let dirs = fixture_dirs();
    if dirs.is_empty() {
        eprintln!("skipping: no real Gerber fixtures available");
        return;
    }
    let set = import(&dirs[0]);
    let copper_only = ImportSet {
        copper: set.copper.clone(),
        copper_file_idx: None,
        profile: None,
        drill: None,
        files: vec![],
        notes: vec![],
        problems: vec![],
    };
    let job = job_from(&copper_only, "copper-only", 0.2);
    assert!(job.layer(MoveKind::Outline).is_none());
    assert!(job.layer(MoveKind::Drill).is_none());
    assert!(
        job.notes.iter().any(|n| n.contains("不含板框切割")),
        "{:?}",
        job.notes
    );
}

#[test]
fn an_empty_or_wrong_directory_is_refused_with_a_clear_error() {
    let empty = std::env::temp_dir().join("gcode-laser-accept-empty");
    std::fs::create_dir_all(&empty).unwrap();
    let err = gcode_laser::import_paths(&[empty]).unwrap_err();
    assert!(err.to_string().contains("制造文件"), "{err}");
    let missing = gcode_laser::import_paths(&[PathBuf::from("/nope/nothing.gbr")]).unwrap_err();
    assert!(missing.to_string().contains("不存在"), "{missing}");
}

#[test]
fn gerber_parser_refuses_files_it_cannot_trust() {
    // No FS: we refuse to guess the coordinate precision.
    assert!(gerber::parse("%MOMM*%\nM02*\n").is_err());
    // Relative coordinates are not supported.
    assert!(gerber::parse("%FSLIX46Y46*%\n%MOMM*%\nM02*\n").is_err());
}
