//! Simulation artifacts: waveform CSV and dependency-free SVG plots.
//!
//! Both are deterministic (`sim-artifacts` spec): the same waveform produces
//! byte-identical files, so they can be committed and diffed.

use crate::runner::Waveform;
use std::fmt::Write as _;
use std::path::Path;

/// Value formatting shared by CSV and SVG (fixed precision, no locale surprises).
fn num(v: f64) -> String {
    if !v.is_finite() {
        return "nan".into();
    }
    format!("{v:.9e}")
}

/// CSV with a header row of column names.
pub fn waveform_csv(w: &Waveform) -> String {
    let mut out = String::new();
    out.push_str(&w.columns.join(","));
    out.push('\n');
    for row in &w.data {
        let cells: Vec<String> = row.iter().map(|v| num(*v)).collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

/// Write the waveform CSV, creating parent directories.
pub fn write_waveform_csv(path: &Path, w: &Waveform) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, waveform_csv(w))
}

/// Artifact file names for a circuit + analysis tag.
///
/// Predictable by construction: `<circuit>.<tag>.csv` and `<circuit>.<tag>.svg`.
pub fn artifact_names(circuit_id: &str, tag: &str) -> (String, String) {
    (
        format!("{circuit_id}.{tag}.csv"),
        format!("{circuit_id}.{tag}.svg"),
    )
}

/// A named series to draw.
struct Series {
    label: String,
    color: &'static str,
    points: Vec<(f64, f64)>,
    /// Draw the y axis in dB (magnitude panels).
    y_db: bool,
}

/// Render an SVG plot.
///
/// `panels` are stacked vertically. Log-x is used when `x_log` is set, which is
/// what a Bode plot wants.
fn render_svg(title: &str, x_label: &str, x_log: bool, panels: &[(String, Vec<Series>)]) -> String {
    const W: f64 = 900.0;
    const PANEL_H: f64 = 260.0;
    const MARGIN: f64 = 60.0;
    let h = PANEL_H * panels.len() as f64 + 40.0;

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{W}\" height=\"{h}\" \
         viewBox=\"0 0 {W} {h}\" font-family=\"sans-serif\" font-size=\"12\">\n"
    );
    let _ = write!(
        svg,
        "<rect width=\"{W}\" height=\"{h}\" fill=\"white\"/>\n\
         <text x=\"{MARGIN}\" y=\"20\" font-size=\"15\">{}</text>\n",
        escape(title)
    );

    let x_of = |v: f64| -> f64 { if x_log { v.max(1e-12).log10() } else { v } };

    for (pi, (panel_title, series)) in panels.iter().enumerate() {
        let top = 40.0 + PANEL_H * pi as f64;
        let bottom = top + PANEL_H - MARGIN;
        let left = MARGIN;
        let right = W - 20.0;

        // Ranges.
        let mut xs: Vec<f64> = Vec::new();
        let mut ys: Vec<f64> = Vec::new();
        for s in series {
            for (x, y) in &s.points {
                if x.is_finite() {
                    xs.push(x_of(*x));
                }
                let yv = if s.y_db { 20.0 * y.max(1e-30).log10() } else { *y };
                if yv.is_finite() {
                    ys.push(yv);
                }
            }
        }
        if xs.is_empty() || ys.is_empty() {
            continue;
        }
        let (x0, x1) = min_max(&xs);
        let (mut y0, mut y1) = min_max(&ys);
        if (y1 - y0).abs() < 1e-12 {
            y0 -= 1.0;
            y1 += 1.0;
        }
        let pad = (y1 - y0) * 0.05;
        let (y0, y1) = (y0 - pad, y1 + pad);
        let (x0, x1) = if (x1 - x0).abs() < 1e-12 {
            (x0 - 0.5, x1 + 0.5)
        } else {
            (x0, x1)
        };

        let sx = |v: f64| left + (x_of(v) - x0) / (x1 - x0) * (right - left);
        let sy = |v: f64| bottom - (v - y0) / (y1 - y0) * (bottom - top);

        let _ = write!(
            svg,
            "<rect x=\"{left}\" y=\"{top}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#999\"/>\n",
            right - left,
            bottom - top
        );
        let _ = write!(
            svg,
            "<text x=\"{left}\" y=\"{}\" font-size=\"13\">{}</text>\n",
            top - 6.0,
            escape(panel_title)
        );

        // Horizontal grid + y labels.
        const Y_TICKS: usize = 5;
        for k in 0..=Y_TICKS {
            let v = y0 + (y1 - y0) * k as f64 / Y_TICKS as f64;
            let y = sy(v);
            let _ = write!(
                svg,
                "<line x1=\"{left}\" y1=\"{y:.2}\" x2=\"{right}\" y2=\"{y:.2}\" stroke=\"#eee\"/>\n\
                 <text x=\"4\" y=\"{:.2}\" font-size=\"10\">{:.4}</text>\n",
                y + 3.0,
                v
            );
        }
        // Vertical grid at the swept points (log or linear).
        if let Some(s) = series.first() {
            for (x, _) in s.points.iter().step_by((s.points.len() / 8).max(1)) {
                let px = sx(*x);
                let _ = write!(
                    svg,
                    "<line x1=\"{px:.2}\" y1=\"{top}\" x2=\"{px:.2}\" y2=\"{bottom}\" stroke=\"#f4f4f4\"/>\n"
                );
            }
        }

        for s in series {
            let mut path = String::new();
            for (i, (x, y)) in s.points.iter().enumerate() {
                let yv = if s.y_db { 20.0 * y.max(1e-30).log10() } else { *y };
                if !x.is_finite() || !yv.is_finite() {
                    continue;
                }
                let _ = write!(
                    path,
                    "{}{:.2},{:.2}",
                    if i == 0 { "M" } else { "L" },
                    sx(*x),
                    sy(yv)
                );
            }
            let _ = write!(
                svg,
                "<path d=\"{path}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.6\"/>\n",
                s.color
            );
        }

        // Legend.
        for (i, s) in series.iter().enumerate() {
            let ly = top + 14.0 + 14.0 * i as f64;
            let _ = write!(
                svg,
                "<line x1=\"{:.0}\" y1=\"{ly:.0}\" x2=\"{:.0}\" y2=\"{ly:.0}\" stroke=\"{}\" stroke-width=\"2\"/>\n\
                 <text x=\"{:.0}\" y=\"{:.0}\" font-size=\"11\">{}</text>\n",
                right - 140.0,
                right - 120.0,
                s.color,
                right - 116.0,
                ly + 4.0,
                escape(&s.label)
            );
        }
    }

    let _ = write!(
        svg,
        "<text x=\"{MARGIN}\" y=\"{}\" font-size=\"11\">x: {}</text>\n</svg>\n",
        h - 12.0,
        escape(x_label)
    );
    svg
}

fn min_max(v: &[f64]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for x in v {
        lo = lo.min(*x);
        hi = hi.max(*x);
    }
    (lo, hi)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Bode plot: magnitude (dB) over phase (degrees).
pub fn bode_svg(w: &Waveform, out_col: &str, in_col: &str, title: &str) -> Option<String> {
    let f = w.column(w.scale_name()?)?;
    let out = w.column(out_col)?;
    let inp = w.column(in_col)?;
    let ratio: Vec<(f64, f64)> = f
        .iter()
        .zip(out.iter().zip(inp.iter()))
        .filter(|(_, (_, i))| **i != 0.0)
        .map(|(f, (o, i))| (*f, o / i))
        .collect();
    let node = out_col
        .strip_prefix("vm(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(out_col);
    let phase: Vec<(f64, f64)> = f
        .iter()
        .zip(
            w.column(&format!("vp({node})"))?
                .iter()
                .zip(w.column(&format!("vp({})", in_node(in_col)))?.iter()),
        )
        // ngspice vp() is radians; plot degrees.
        .map(|(f, (po, pi))| (*f, (po - pi).to_degrees()))
        .collect();

    let panels = vec![
        (
            format!("幅频 |{node}|"),
            vec![Series {
                label: format!("{out_col}/{in_col}"),
                color: "#1f77b4",
                points: ratio,
                y_db: true,
            }],
        ),
        (
            format!("相频 ∠{node}"),
            vec![Series {
                label: "phase (deg)".into(),
                color: "#d62728",
                points: phase,
                y_db: false,
            }],
        ),
    ];
    Some(render_svg(title, "frequency (Hz)", true, &panels))
}

fn in_node(col: &str) -> String {
    col.strip_prefix("vm(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(col)
        .to_string()
}

/// Time-domain or sweep plot of one column.
pub fn series_svg(w: &Waveform, col: &str, title: &str, x_log: bool) -> Option<String> {
    let x = w.column(w.scale_name()?)?;
    let y = w.column(col)?;
    let points: Vec<(f64, f64)> = x.into_iter().zip(y).collect();
    let panels = vec![(
        format!("{col}"),
        vec![Series {
            label: col.to_string(),
            color: "#2ca02c",
            points,
            y_db: false,
        }],
    )];
    Some(render_svg(title, "x", x_log, &panels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave() -> Waveform {
        Waveform {
            columns: vec!["frequency".into(), "vm(OUT)".into(), "vp(OUT)".into()],
            data: vec![
                vec![10.0, 1.0, 0.0],
                vec![100.0, 0.99, -5.7],
                vec![1000.0, 0.7, -45.0],
                vec![10000.0, 0.1, -84.3],
            ],
            raw_output: String::new(),
        }
    }

    #[test]
    fn csv_has_a_header_and_one_row_per_sample() {
        let csv = waveform_csv(&wave());
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "frequency,vm(OUT),vp(OUT)");
        assert_eq!(lines[1].split(',').count(), 3);
    }

    #[test]
    fn csv_is_deterministic() {
        assert_eq!(waveform_csv(&wave()), waveform_csv(&wave()));
    }

    #[test]
    fn artifact_names_are_predictable() {
        let (csv, svg) = artifact_names("rc-lowpass", "ac");
        assert_eq!(csv, "rc-lowpass.ac.csv");
        assert_eq!(svg, "rc-lowpass.ac.svg");
    }

    #[test]
    fn bode_svg_has_two_panels_and_axes() {
        let svg = bode_svg(&wave(), "vm(OUT)", "vm(OUT)", "rc-lowpass AC").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("幅频"));
        assert!(svg.contains("相频"));
        assert!(svg.contains("<path"));
        assert!(svg.contains("frequency (Hz)"));
        assert!(svg.ends_with("</svg>\n"));
    }

    #[test]
    fn svg_is_deterministic() {
        let a = bode_svg(&wave(), "vm(OUT)", "vm(OUT)", "t").unwrap();
        let b = bode_svg(&wave(), "vm(OUT)", "vm(OUT)", "t").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn series_svg_renders() {
        let svg = series_svg(&wave(), "vm(OUT)", "step", false).unwrap();
        assert!(svg.contains("<path"));
        assert!(svg.contains("vm(OUT)"));
    }
}
