//! Excellon drill-file parser.
//!
//! Handles what KiCad emits: `M48` header with a tool table (`T#C<diameter>`),
//! `METRIC`/`INCH` units, `%` body separator, `T#` tool selection, decimal
//! absolute coordinates, and plating annotations in `; #@!` comments.
//!
//! A coordinate before any tool selection is an error: guessing a diameter would
//! silently produce wrong holes.

use serde::{Deserialize, Serialize};

/// One drill hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hole {
    pub x: f64,
    pub y: f64,
    /// Tool diameter in millimetres.
    pub diameter: f64,
    /// Plated through-hole (from the tool's `TA.AperFunction` annotation).
    pub plated: bool,
    /// Tool number it came from.
    pub tool: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrillReport {
    pub units: String,
    pub tool_count: usize,
    pub hole_count: usize,
    /// Tool diameters present, sorted.
    pub diameters: Vec<f64>,
    /// Lines that looked like commands but were not recognised.
    pub unknown_commands: Vec<String>,
}

impl DrillReport {
    pub fn summary(&self) -> String {
        format!(
            "{}；刀具 {} 种（{}）、孔 {} 个",
            self.units,
            self.tool_count,
            self.diameters
                .iter()
                .map(|d| format!("{d}mm"))
                .collect::<Vec<_>>()
                .join("/"),
            self.hole_count
        )
    }
}

/// A parsed drill file.
#[derive(Debug, Clone, PartialEq)]
pub struct DrillFile {
    pub holes: Vec<Hole>,
    pub report: DrillReport,
}

impl DrillFile {
    pub fn bounds(&self) -> Option<super::gerber::Bounds> {
        let mut b: Option<super::gerber::Bounds> = None;
        for h in &self.holes {
            b = Some(match b {
                None => super::gerber::Bounds {
                    min_x: h.x,
                    min_y: h.y,
                    max_x: h.x,
                    max_y: h.y,
                },
                Some(v) => super::gerber::Bounds {
                    min_x: v.min_x.min(h.x),
                    min_y: v.min_y.min(h.y),
                    max_x: v.max_x.max(h.x),
                    max_y: v.max_y.max(h.y),
                },
            });
        }
        b
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DrillError {
    #[error("在未选择刀具时遇到坐标（第 {line} 行）：无法确定孔径")]
    NoToolSelected { line: usize },
    #[error("坐标无法解析（第 {line} 行）：{text:?}")]
    BadCoordinate { line: usize, text: String },
    #[error("刀具 T{0} 未在头部定义")]
    UndefinedTool(u32),
}

/// Split at the first character satisfying `pred`.
///
/// Not `str::split_once`, which on this toolchain drops the delimiter.
fn split_at<'a>(s: &'a str, pred: impl Fn(char) -> bool) -> Option<(&'a str, &'a str)> {
    for (i, c) in s.char_indices() {
        if pred(c) {
            return Some((&s[..i], &s[i..]));
        }
    }
    None
}

/// Parse an Excellon drill file.
pub fn parse(text: &str) -> Result<DrillFile, DrillError> {
    let mut tools: Vec<(u32, f64, bool)> = Vec::new(); // (number, diameter mm, plated)
    let mut in_header = false;
    let mut header_done = !text.contains("M48");
    let mut units_mm = true;
    let mut saw_units = false;
    let mut current_tool: Option<u32> = None;
    let mut holes: Vec<Hole> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut pending_plated = false;

    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // Annotations: plating is announced just before the tool definition.
        if let Some(comment) = line.strip_prefix(';') {
            let c = comment.trim();
            if c.contains("Plated") && !c.contains("NonPlated") {
                pending_plated = true;
            } else if c.contains("NonPlated") {
                pending_plated = false;
            }
            continue;
        }
        if line == "M48" {
            in_header = true;
            header_done = false;
            continue;
        }
        if line == "M95" || line == "%" {
            in_header = false;
            header_done = true;
            continue;
        }
        if line == "M30" || line == "M00" {
            break;
        }
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("METRIC") || upper.starts_with("FMAT,2") {
            if upper.starts_with("METRIC") {
                units_mm = true;
                saw_units = true;
            }
            continue;
        }
        if upper.starts_with("INCH") {
            units_mm = false;
            saw_units = true;
            continue;
        }
        if upper.starts_with("FMAT") || upper.starts_with("G90") || upper.starts_with("G05")
            || upper.starts_with("G93") || upper.starts_with("M71") || upper.starts_with("M72")
            || upper.starts_with("ICI") || upper.starts_with("VER")
        {
            continue;
        }

        // Tool definition in the header: T1C0.800
        if let Some(rest) = upper.strip_prefix('T') {
            if let Some((num_str, dia_str)) = split_at(rest, |c| c == 'C') {
                let dia_str = dia_str.trim_start_matches('C');
                if let Ok(num) = num_str.trim().parse::<u32>() {
                    if let Ok(dia) = dia_str.trim().parse::<f64>() {
                        let mm = if units_mm { dia } else { dia * 25.4 };
                        // Replace any earlier definition of the same tool.
                        tools.retain(|(n, _, _)| *n != num);
                        tools.push((num, mm, pending_plated));
                        continue;
                    }
                }
            }
            // Bare tool selection in the body: T1
            if in_header {
                // In the header a bare T# is not expected; record it as unknown.
                unknown.push(line.to_string());
                continue;
            }
            if let Ok(num) = rest.trim().parse::<u32>() {
                if !tools.iter().any(|(n, _, _)| *n == num) {
                    return Err(DrillError::UndefinedTool(num));
                }
                current_tool = Some(num);
                continue;
            }
        }

        // Coordinate line: X126.365Y-125.095
        if upper.starts_with('X') || upper.starts_with('Y') {
            let mut x: Option<f64> = None;
            let mut y: Option<f64> = None;
            let bytes = line.as_bytes();
            let mut i = 0usize;
            while i < bytes.len() {
                let c = bytes[i] as char;
                if c.eq_ignore_ascii_case(&'X') || c.eq_ignore_ascii_case(&'Y') {
                    let is_x = c.eq_ignore_ascii_case(&'X');
                    i += 1;
                    let start = i;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_digit()
                            || bytes[i] == b'-'
                            || bytes[i] == b'+'
                            || bytes[i] == b'.')
                    {
                        i += 1;
                    }
                    let raw_num = &line[start..i];
                    let v: f64 = raw_num.parse().map_err(|_| DrillError::BadCoordinate {
                        line: line_no,
                        text: line.to_string(),
                    })?;
                    let mm = if units_mm { v } else { v * 25.4 };
                    if is_x {
                        x = Some(mm);
                    } else {
                        y = Some(mm);
                    }
                } else {
                    i += 1;
                }
            }
            let Some(tool) = current_tool else {
                return Err(DrillError::NoToolSelected { line: line_no });
            };
            let (_, diameter, plated) = tools
                .iter()
                .find(|(n, _, _)| *n == tool)
                .copied()
                .ok_or(DrillError::UndefinedTool(tool))?;
            let prev = holes.last();
            // Missing axes repeat the previous value (Excellon allows this).
            let x = x.or_else(|| prev.map(|h| h.x)).unwrap_or(0.0);
            let y = y.or_else(|| prev.map(|h| h.y)).unwrap_or(0.0);
            holes.push(Hole {
                x,
                y,
                diameter,
                plated,
                tool,
            });
            continue;
        }

        if !header_done || !in_header {
            unknown.push(line.to_string());
        }
    }

    let mut diameters: Vec<f64> = tools.iter().map(|(_, d, _)| *d).collect();
    diameters.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    diameters.dedup();

    Ok(DrillFile {
        report: DrillReport {
            units: if saw_units {
                if units_mm { "METRIC".into() } else { "INCH".into() }
            } else {
                "未声明（按毫米处理）".into()
            },
            tool_count: tools.len(),
            hole_count: holes.len(),
            diameters,
            unknown_commands: unknown,
        },
        holes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape captured from a real `kicad-cli pcb export drill` output.
    const REAL: &str = "\
M48
; DRILL file KiCad 10.0.7
; FORMAT={-:-/ absolute / metric / decimal}
FMAT,2
METRIC
; #@! TA.AperFunction,Plated,PTH,ComponentDrill
T1C0.800
; #@! TA.AperFunction,Plated,PTH,ComponentDrill
T2C1.000
; #@! TA.AperFunction,NonPlated,NPTH,ComponentDrill
T3C3.200
%
G90
G05
T1
X126.365Y-125.095
X133.985Y-125.095
T2
X136.271Y-107.95
T3
X148.59Y-115.57
M30
";

    #[test]
    fn parses_a_real_kicad_drill_file() {
        let d = parse(REAL).unwrap();
        assert_eq!(d.holes.len(), 4);
        assert_eq!(d.report.tool_count, 3);
        assert_eq!(d.report.units, "METRIC");
        assert_eq!(d.report.diameters, vec![0.8, 1.0, 3.2]);
        let h = &d.holes[0];
        assert!((h.x - 126.365).abs() < 1e-9);
        assert!((h.y + 125.095).abs() < 1e-9);
        assert!((h.diameter - 0.8).abs() < 1e-9);
        assert!(h.plated);
    }

    #[test]
    fn tracks_plating_per_tool() {
        let d = parse(REAL).unwrap();
        let npth = d.holes.iter().find(|h| h.tool == 3).unwrap();
        assert!(!npth.plated, "T3 was annotated NonPlated");
        assert!(d.holes.iter().filter(|h| h.tool == 1).all(|h| h.plated));
    }

    #[test]
    fn coordinates_before_a_tool_are_refused() {
        let text = "M48\nMETRIC\nT1C0.8\n%\nX1.0Y1.0\nM30\n";
        // T1 was defined but not selected in the body.
        assert_eq!(
            parse(text).unwrap_err(),
            DrillError::NoToolSelected { line: 5 }
        );
    }

    #[test]
    fn undefined_tool_selection_is_refused() {
        let text = "M48\nMETRIC\nT1C0.8\n%\nT9\nX1.0Y1.0\nM30\n";
        assert_eq!(parse(text).unwrap_err(), DrillError::UndefinedTool(9));
    }

    #[test]
    fn inches_are_converted() {
        let text = "M48\nINCH\nT1C0.04\n%\nT1\nX1.0Y0.5\nM30\n";
        let d = parse(text).unwrap();
        assert!((d.holes[0].x - 25.4).abs() < 1e-9);
        assert!((d.holes[0].y - 12.7).abs() < 1e-9);
        assert!((d.holes[0].diameter - 1.016).abs() < 1e-9);
    }

    #[test]
    fn missing_axis_repeats_the_previous_value() {
        let text = "M48\nMETRIC\nT1C0.8\n%\nT1\nX1.0Y2.0\nX3.0\nM30\n";
        let d = parse(text).unwrap();
        assert_eq!(d.holes.len(), 2);
        assert!((d.holes[1].x - 3.0).abs() < 1e-9);
        assert!((d.holes[1].y - 2.0).abs() < 1e-9, "Y should repeat");
    }

    #[test]
    fn report_summary_lists_diameters() {
        let d = parse(REAL).unwrap();
        let s = d.report.summary();
        assert!(s.contains("0.8mm"), "{s}");
        assert!(s.contains("孔 4 个"), "{s}");
    }

    #[test]
    fn bounds_cover_holes() {
        let d = parse(REAL).unwrap();
        let b = d.bounds().unwrap();
        assert!((b.min_x - 126.365).abs() < 1e-9);
        assert!((b.max_x - 148.59).abs() < 1e-9);
    }
}
