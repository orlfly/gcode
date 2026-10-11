//! SVG preview of a laser job.
//!
//! Purely a visual check: three colours for isolation / drill / outline, a
//! board border, a legend, and the same caveats as the G-code. Deterministic.

use crate::toolpath::{LaserJob, MoveKind};
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub struct SvgOptions {
    /// Extra margin around the board in millimetres.
    pub margin_mm: f64,
    /// Stroke width for cut paths in millimetres.
    pub stroke_mm: f64,
    pub include_legend: bool,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            margin_mm: 5.0,
            stroke_mm: 0.15,
            include_legend: true,
        }
    }
}

/// Render the job as a standalone SVG document.
pub fn render(job: &LaserJob, opts: &SvgOptions) -> String {
    let w = job.bounds.width() + 2.0 * opts.margin_mm;
    let h = job.bounds.height() + 2.0 * opts.margin_mm;
    let mut out = String::new();
    let _ = writeln!(
        out,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.3}mm" height="{h:.3}mm" viewBox="0 0 {w:.3} {h:.3}">"##
    );
    let _ = writeln!(out, "<title>Gcode 激光作业预览: {}</title>", escape(&job.name));
    let _ = writeln!(
        out,
        r##"<desc>由 gcode-laser 生成。仅用于几何检查，未在真实激光机上验证，未做实际打样。</desc>"##
    );
    let _ = writeln!(
        out,
        r##"<rect x="0" y="0" width="{w:.3}" height="{h:.3}" fill="#fbfbf8"/>"##
    );

    // Work area group: flip Y so Gerber's up-is-positive matches the preview.
    let _ = writeln!(
        out,
        r##"<g transform="translate({mx:.3},{my_h:.3}) scale(1,-1)" fill="none" stroke-linecap="round" stroke-linejoin="round">"##,
        mx = opts.margin_mm,
        my_h = opts.margin_mm + job.bounds.height()
    );
    // Board outline of the work area.
    let _ = writeln!(
        out,
        r##"<rect x="0" y="0" width="{bw:.3}" height="{bh:.3}" stroke="#999" stroke-width="0.05" stroke-dasharray="1 0.5"/>"##,
        bw = job.bounds.width(),
        bh = job.bounds.height()
    );

    for layer in &job.layers {
        let _ = writeln!(
            out,
            r##"<g stroke="{}" stroke-width="{:.3}">"##,
            layer.kind.svg_color(),
            opts.stroke_mm
        );
        for path in &layer.paths {
            if path.points.len() == 1 {
                if let Some((x, y)) = path.start() {
                    let _ = writeln!(
                        out,
                        r##"<circle cx="{x:.4}" cy="{y:.4}" r="{:.3}" fill="{}" />"##,
                        opts.stroke_mm,
                        layer.kind.svg_color()
                    );
                }
                continue;
            }
            let mut d = String::new();
            for (i, (x, y)) in path.points.iter().enumerate() {
                let _ = write!(d, "{}{:.4},{:.4} ", if i == 0 { "M" } else { "L" }, x, y);
            }
            if path.closed {
                d.push('Z');
            }
            let _ = writeln!(out, r##"<path d="{}" />"##, d.trim_end());
        }
        let _ = writeln!(out, "</g>");
    }
    let _ = writeln!(out, "</g>");

    if opts.include_legend {
        let mut y = 4.0;
        for (i, layer) in job.layers.iter().enumerate() {
            let _ = writeln!(
                out,
                r##"<rect x="{x:.3}" y="{y:.3}" width="3" height="1.2" fill="{c}" />"##,
                x = opts.margin_mm,
                y = opts.margin_mm + i as f64 * 2.4,
                c = layer.kind.svg_color()
            );
            let _ = writeln!(
                out,
                r##"<text x="{x:.3}" y="{y:.3}" font-size="1.6" fill="#333" font-family="monospace">{}</text>"##,
                escape(&format!(
                    "{} ({} 条, {:.1} mm)",
                    layer.kind.label_zh(),
                    layer.paths.len(),
                    layer.cut_length()
                )),
                x = opts.margin_mm + 4.0,
                y = opts.margin_mm + i as f64 * 2.4 + 1.0,
            );
            y = opts.margin_mm + i as f64 * 2.4 + 1.0;
        }
        let _ = writeln!(
            out,
            r##"<text x="{x:.3}" y="{y:.3}" font-size="1.6" fill="#333" font-family="monospace">{}</text>"##,
            escape(&format!(
                "板 {} x {} mm，切割 {:.1} mm，预估 {:.1} min（未上机验证）",
                trim(job.bounds.width()),
                trim(job.bounds.height()),
                job.stats.cut_length_mm,
                job.stats.estimated_minutes
            )),
            x = opts.margin_mm,
            y = y + 2.4,
        );
    }

    let _ = writeln!(out, "</svg>");
    out
}

fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render a compact preview of one toolpath group (used in text reports).
pub fn layer_summary(kind: MoveKind, paths: usize, length: f64, passes: u32) -> String {
    format!(
        "{}: {} 条路径, {:.1} mm, {} 遍",
        kind.label_zh(),
        paths,
        length,
        passes
    )
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
        let drill =
            crate::excellon::parse("M48\nMETRIC\nT1C0.800\n%\nT1\nX5.0Y5.0\nM30\n").unwrap();
        let prof =
            |id: &str| profile::profiles().into_iter().find(|p| p.id.contains(id)).unwrap();
        let src = |id: &str| ProfileSource::KnowledgeBase {
            id: id.into(),
            source: "test".into(),
        };
        toolpath::build_job(JobInputs {
            name: "svg-test & <board>".into(),
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
    fn svg_is_well_formed_and_layered() {
        let j = job();
        let s = render(&j, &SvgOptions::default());
        assert!(s.starts_with("<svg"));
        assert!(s.trim_end().ends_with("</svg>"));
        assert_eq!(s.matches("<svg").count(), 1);
        assert!(s.contains("#c0392b"), "isolation colour");
        assert!(s.contains("#2471a3"), "drill colour");
        assert!(s.contains("#1e8449"), "outline colour");
        assert!(s.contains("隔离雕刻"), "legend");
        // Same number of open and close tags for the groups we emit.
        assert_eq!(s.matches("<g ").count(), s.matches("</g>").count());
    }

    #[test]
    fn svg_is_deterministic() {
        let j = job();
        assert_eq!(render(&j, &SvgOptions::default()), render(&j, &SvgOptions::default()));
    }

    #[test]
    fn svg_escapes_xml_in_the_job_name() {
        let j = job();
        let s = render(&j, &SvgOptions::default());
        assert!(s.contains("&amp;"));
        assert!(!s.contains("<board>"));
    }

    #[test]
    fn svg_says_it_is_unverified() {
        let j = job();
        let s = render(&j, &SvgOptions::default());
        assert!(s.contains("未在真实激光机上验证"));
        assert!(s.contains("未上机验证"));
    }

    #[test]
    fn legend_can_be_disabled() {
        let j = job();
        let s = render(
            &j,
            &SvgOptions {
                include_legend: false,
                ..Default::default()
            },
        );
        assert!(!s.contains("隔离雕刻"));
    }
}
