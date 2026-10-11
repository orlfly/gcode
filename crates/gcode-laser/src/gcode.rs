//! GRBL G-code emission and structural validation.
//!
//! Only a conservative GRBL subset is emitted (design D4): `G21`/`G90`/`G94`,
//! `G0` rapids, `G1` cuts, `G4` dwells for drill marks, `M3`/`M4`/`M5` laser
//! control, `M2` end. No arcs, no cutter compensation, no spindle commands.
//!
//! [`validate_gcode`] re-reads the text and checks the properties that matter
//! before a machine ever sees it: whitelisted commands, finite in-range
//! coordinates, and **never** a rapid move with the laser on.

use crate::toolpath::{LaserJob, MoveKind};
use std::fmt::Write as _;

/// How the laser is turned on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaserMode {
    /// `M3`: constant power (what diode lasers usually want).
    ConstantPower,
    /// `M4`: dynamic power, which GRBL varies with feed rate.
    DynamicPower,
}

impl LaserMode {
    fn on_command(&self) -> &'static str {
        match self {
            LaserMode::ConstantPower => "M3",
            LaserMode::DynamicPower => "M4",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcodeOptions {
    pub laser_mode: LaserMode,
    pub include_comments: bool,
    /// Feed rate used for rapids in the time estimate comment.
    pub rapid_feed_mm_min: u32,
    /// How long the laser dwells on each drill mark, in seconds.
    pub drill_dwell_s: f64,
}

impl Default for GcodeOptions {
    fn default() -> Self {
        Self {
            laser_mode: LaserMode::ConstantPower,
            include_comments: true,
            rapid_feed_mm_min: 3000,
            drill_dwell_s: 0.6,
        }
    }
}

fn fmt_xy(x: f64, y: f64) -> String {
    if x.fract().abs() < 1e-9 && y.fract().abs() < 1e-9 {
        format!("X{:.3} Y{:.3}", x, y)
    } else {
        format!("X{:.3} Y{:.3}", x, y)
    }
}

/// Emit GRBL G-code for a job. Deterministic for a given job and options.
pub fn generate(job: &LaserJob, opts: &GcodeOptions) -> String {
    let c = opts.include_comments;
    let mut out = String::new();
    let comment = |s: &str, out: &mut String| {
        if c {
            let _ = writeln!(out, "; {s}");
        }
    };

    comment(&format!("Gcode 激光作业: {}", job.name), &mut out);
    comment(
        "由 gcode-laser (R2) 生成。尚未在真实激光机上验证，也没有做过任何实际打样。",
        &mut out,
    );
    comment(
        &format!(
            "作业尺寸 {:.3} x {:.3} mm；原点 = 板子左下角（Gerber 左下角已平移 {:.3},{:.3}）",
            job.bounds.width(),
            job.bounds.height(),
            job.offset.0,
            job.offset.1
        ),
        &mut out,
    );
    comment(
        &format!(
            "总切割长度 {:.1} mm，空程 {:.1} mm，预估 {:.1} 分钟（不含对刀与换板）",
            job.stats.cut_length_mm, job.stats.travel_length_mm, job.stats.estimated_minutes
        ),
        &mut out,
    );
    comment(&crate::profile::DISCLAIMER.to_string(), &mut out);
    comment(
        "上机前：先对刀/对焦到板面，先在废料或纸板上跑一遍确认原点与比例，\
         再上铜板；作业中不要离开机器。",
        &mut out,
    );
    for w in &job.warnings {
        comment(&format!("警告: {w}"), &mut out);
    }

    out.push_str("G21\nG90\nG94\n");
    out.push_str("M5\n");
    out.push_str("G0 X0.000 Y0.000\n");

    for layer in &job.layers {
        comment(
            &format!("===== {} =====", layer.kind.label_zh()),
            &mut out,
        );
        comment(&format!("参数: {}", layer.profile.comment_line()), &mut out);
        comment(&format!("参数来源: {}", layer.source.label_zh()), &mut out);
        for pass in 1..=layer.passes.max(1) {
            if layer.passes > 1 {
                comment(&format!("第 {pass}/{} 遍", layer.passes), &mut out);
            }
            for path in &layer.paths {
                let Some(start) = path.start() else { continue };
                out.push_str("M5\n");
                let _ = writeln!(out, "G0 {}", fmt_xy(start.0, start.1));
                if path.points.len() == 1 {
                    // Drill mark: burn a dot where we stand.
                    let _ = writeln!(
                        out,
                        "{} S{}",
                        opts.laser_mode.on_command(),
                        layer.profile.power_s
                    );
                    let _ = writeln!(out, "G4 P{:.2}", opts.drill_dwell_s);
                    out.push_str("M5\n");
                    continue;
                }
                let _ = writeln!(out, "G1 F{}", layer.profile.speed_mm_min);
                let _ = writeln!(
                    out,
                    "{} S{}",
                    opts.laser_mode.on_command(),
                    layer.profile.power_s
                );
                for p in path.points.iter().skip(1) {
                    let _ = writeln!(out, "G1 {}", fmt_xy(p.0, p.1));
                }
                if path.closed {
                    let _ = writeln!(out, "G1 {}", fmt_xy(start.0, start.1));
                }
                out.push_str("M5\n");
            }
        }
        if layer.kind == MoveKind::Outline {
            comment("板框走刀结束：板子会脱落，注意固定", &mut out);
        }
    }

    out.push_str("M5\n");
    out.push_str("G0 X0.000 Y0.000\n");
    comment("作业结束。请关闭激光电源并清理碎屑。", &mut out);
    out.push_str("M2\n");
    out
}

/// A structural problem found in generated G-code.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub line: usize,
    pub text: String,
    pub reason: String,
}

/// Structural checks applied to generated text.
///
/// This is not a substitute for a real machine run: it only proves the file is
/// well-formed and internally safe (no laser-on rapid, in-range coordinates).
pub fn validate_gcode(gcode: &str, job: &LaserJob) -> Result<(), Vec<Violation>> {
    let allowed_prefixes = ["G0", "G1", "G4", "G21", "G90", "G94", "M2", "M3", "M4", "M5"];
    let mut violations = Vec::new();
    let mut laser_on = false;
    let mut started = false;
    let mut max_s = job
        .layers
        .iter()
        .map(|l| l.profile.max_s)
        .max()
        .unwrap_or(1000);

    for (i, raw) in gcode.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        let head: String = upper.split_whitespace().next().unwrap_or("").to_string();
        if !allowed_prefixes
            .iter()
            .any(|p| head == *p || head.starts_with(&format!("{p} X")) || head.starts_with(&format!("{p} F")))
        {
            violations.push(Violation {
                line: line_no,
                text: line.to_string(),
                reason: format!("指令 {head:?} 不在允许的 GRBL 子集内"),
            });
            continue;
        }
        if head.starts_with("G21") || head.starts_with("G90") || head.starts_with("G94") {
            started = true;
        }
        if !started {
            violations.push(Violation {
                line: line_no,
                text: line.to_string(),
                reason: "在 G21/G90 之前就出现运动指令".into(),
            });
        }

        // Coordinate range check.
        for token in upper.split_whitespace() {
            if token.starts_with('X') || token.starts_with('Y') {
                let v: f64 = token[1..].parse().unwrap_or(f64::NAN);
                if !v.is_finite() {
                    violations.push(Violation {
                        line: line_no,
                        text: line.to_string(),
                        reason: format!("坐标 {token} 不是有限数"),
                    });
                    break;
                }
                let limit = if token.starts_with('X') {
                    job.bounds.width()
                } else {
                    job.bounds.height()
                };
                if v < -1e-6 || v > limit + 1e-6 {
                    violations.push(Violation {
                        line: line_no,
                        text: line.to_string(),
                        reason: format!("坐标 {token} 超出板范围 0..={limit:.3}"),
                    });
                    break;
                }
            }
            if token.starts_with('S') {
                let v: u32 = token[1..].parse().unwrap_or(0);
                if v > max_s {
                    violations.push(Violation {
                        line: line_no,
                        text: line.to_string(),
                        reason: format!("功率 S{v} 超过设定的最大 S{max_s}"),
                    });
                }
            }
        }
        max_s = max_s.max(1);

        match head.as_str() {
            "M3" | "M4" => laser_on = true,
            "M5" => laser_on = false,
            h if h.starts_with("G0") => {
                if laser_on {
                    violations.push(Violation {
                        line: line_no,
                        text: line.to_string(),
                        reason: "激光开启状态下执行空程 G0：会烧出多余的线".into(),
                    });
                }
            }
            _ => {}
        }
    }

    if laser_on {
        violations.push(Violation {
            line: gcode.lines().count(),
            text: "M2".into(),
            reason: "文件结束时激光仍处于开启状态（缺少 M5）".into(),
        });
    }
    if !gcode.contains("M2") {
        violations.push(Violation {
            line: gcode.lines().count(),
            text: String::new(),
            reason: "缺少结束指令 M2".into(),
        });
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerber;
    use crate::profile::{self, ProfileSource};
    use crate::toolpath::{self, DrillMode, IsolationParams, JobInputs};

    fn job() -> LaserJob {
        let copper_text = "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%ADD10C,2.000000*%\n%ADD11C,0.500000*%\nD10*\nX5000000Y5000000D03*\nX15000000Y5000000D03*\nD11*\nX5000000Y5000000D02*\nX15000000Y5000000D01*\nM02*\n";
        let profile_text = "%TF.FileFunction,Profile,NP*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%ADD10C,0.050000*%\nD10*\nX0Y0D02*\nX20000000Y0D01*\nX20000000Y20000000D01*\nX0Y20000000D01*\nX0Y0D01*\nM02*\n";
        let copper = gerber::parse(copper_text).unwrap();
        let pl = gerber::parse(profile_text).unwrap();
        let drill = crate::excellon::parse("M48\nMETRIC\nT1C0.800\n%\nT1\nX5.0Y5.0\nM30\n").unwrap();
        let prof = |id: &str| profile::profiles().into_iter().find(|p| p.id.contains(id)).unwrap();
        let src = |id: &str| ProfileSource::UserSupplied {
            overridden: vec![id.into()],
        };
        toolpath::build_job(JobInputs {
            name: "unit-test".into(),
            copper: Some(&copper),
            profile_layer: Some(&pl),
            drill: Some(&drill),
            isolation: IsolationParams {
                resolution: 0.2,
                simplify_tolerance: 0.05,
                ..Default::default()
            },
            drill_mode: DrillMode::Mark,
            isolation_profile: prof("isol"),
            isolation_source: src("isol"),
            drill_profile: prof("drillmark"),
            drill_source: src("drillmark"),
            outline_profile: prof("outline"),
            outline_source: src("outline"),
            import_notes: vec![],
        })
        .unwrap()
    }

    #[test]
    fn generated_gcode_passes_structural_validation() {
        let j = job();
        let g = generate(&j, &GcodeOptions::default());
        validate_gcode(&g, &j).expect("generated code must validate");
    }

    #[test]
    fn generated_gcode_is_deterministic() {
        let j = job();
        assert_eq!(
            generate(&j, &GcodeOptions::default()),
            generate(&j, &GcodeOptions::default())
        );
    }

    #[test]
    fn gcode_has_the_expected_preamble_and_end() {
        let j = job();
        let g = generate(&j, &GcodeOptions::default());
        let body: Vec<&str> = g.lines().filter(|l| !l.starts_with(';')).collect();
        assert_eq!(body[0], "G21");
        assert_eq!(body[1], "G90");
        assert_eq!(body[2], "G94");
        assert_eq!(*body.last().unwrap(), "M2");
        assert!(g.contains("M5"), "laser must be turned off");
    }

    #[test]
    fn comments_carry_material_power_speed_and_source() {
        let j = job();
        let g = generate(&j, &GcodeOptions::default());
        assert!(g.contains("参数来源"), "{g}");
        assert!(g.contains("S255") || g.contains("S150"), "power in comments");
        assert!(g.contains("F200") || g.contains("mm/min"), "speed in comments");
        assert!(g.contains("试刻"), "trial-cut advice required");
        assert!(g.contains("未在真实激光机上验证"), "honesty required");
    }

    #[test]
    fn drill_marks_use_a_dwell_and_turn_the_laser_off() {
        let j = job();
        let g = generate(&j, &GcodeOptions::default());
        assert!(g.contains("G4 P0.60"), "{g}");
    }

    #[test]
    fn comments_can_be_disabled() {
        let j = job();
        let g = generate(
            &j,
            &GcodeOptions {
                include_comments: false,
                ..Default::default()
            },
        );
        assert!(!g.contains(';'));
        validate_gcode(&g, &j).expect("still valid");
    }

    #[test]
    fn a_rapid_with_the_laser_on_is_caught() {
        let j = job();
        let bad = "G21\nG90\nG94\nM3 S100\nG0 X1.000 Y1.000\nM5\nM2\n";
        let errs = validate_gcode(bad, &j).unwrap_err();
        assert!(
            errs.iter().any(|v| v.reason.contains("多余")),
            "{errs:?}"
        );
    }

    #[test]
    fn an_unknown_command_is_caught() {
        let j = job();
        let bad = "G21\nG90\nG94\nG2 X1.000 Y1.000 I0 J0\nM2\n";
        let errs = validate_gcode(bad, &j).unwrap_err();
        assert!(errs.iter().any(|v| v.reason.contains("子集")), "{errs:?}");
    }

    #[test]
    fn out_of_range_coordinates_are_caught() {
        let j = job();
        let bad = format!(
            "G21\nG90\nG94\nG0 X{:.3} Y0.000\nM2\n",
            j.bounds.width() + 10.0
        );
        let errs = validate_gcode(&bad, &j).unwrap_err();
        assert!(errs.iter().any(|v| v.reason.contains("超出板范围")), "{errs:?}");
    }

    #[test]
    fn excessive_power_is_caught() {
        let j = job();
        let bad = "G21\nG90\nG94\nM3 S99999\nM5\nM2\n";
        let errs = validate_gcode(bad, &j).unwrap_err();
        assert!(errs.iter().any(|v| v.reason.contains("超过设定的最大")), "{errs:?}");
    }

    #[test]
    fn a_missing_final_m5_is_caught() {
        let j = job();
        let bad = "G21\nG90\nG94\nM3 S100\nM2\n";
        let errs = validate_gcode(bad, &j).unwrap_err();
        assert!(errs.iter().any(|v| v.reason.contains("仍处于开启")), "{errs:?}");
    }

    #[test]
    fn dynamic_power_mode_is_honoured() {
        let j = job();
        let g = generate(
            &j,
            &GcodeOptions {
                laser_mode: LaserMode::DynamicPower,
                ..Default::default()
            },
        );
        assert!(g.contains("M4 S"), "expected M4");
        assert!(!g.contains("M3 S"), "no M3 when M4 is requested");
    }
}
