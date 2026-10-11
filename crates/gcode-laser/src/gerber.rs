//! RS-274X (Gerber) parser: the subset KiCad emits, plus explicit reporting of
//! everything it does not handle.
//!
//! Supported: `FS` (leading/trailing zero omission, absolute), `MO` (mm/inch),
//! `AD` apertures `C`/`R`/`O`, `LP` polarity (dark recognised, clear reported),
//! `G01` linear draws, `G02`/`G03` arcs (linearised into chords and counted),
//! `D01`/`D02`/`D03`, `G36`/`G37` regions, `TF.FileFunction` layer
//! identification, `M02`.
//!
//! Not supported (counted, never silently dropped): `AM` macro apertures and the
//! objects that use them, `SR` step-and-repeat, `LM`/`LR`/`LS`
//! mirror/rotate/scale. Macro definition bodies are skipped as a block, because
//! their statements look like Gerber data but are not.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a Gerber file describes, inferred from `TF.FileFunction`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    /// A copper layer (used for isolation routing).
    Copper,
    /// The board outline / profile (used for cutting out).
    Profile,
    /// Anything else, with the raw function string.
    Other(String),
}

impl LayerKind {
    fn from_function(value: &str) -> Self {
        let head = value.split(',').next().unwrap_or("").trim();
        match head {
            "Copper" => LayerKind::Copper,
            "Profile" => LayerKind::Profile,
            other => LayerKind::Other(other.to_string()),
        }
    }

    pub fn label_zh(&self) -> String {
        match self {
            LayerKind::Copper => "铜层".into(),
            LayerKind::Profile => "板框".into(),
            LayerKind::Other(s) => format!("其它（{s}）"),
        }
    }
}

/// A primitive inside an aperture macro, with parameters substituted and all
/// values in millimetres, relative to the aperture origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MacroPrimitive {
    /// Primitive 1: a circle.
    Circle {
        diameter: f64,
        center: (f64, f64),
        rotation: f64,
    },
    /// Primitive 4: an outline polygon.
    Outline {
        points: Vec<(f64, f64)>,
        rotation: f64,
    },
    /// Primitive 20: a stroked line with width.
    VectorLine {
        width: f64,
        from: (f64, f64),
        to: (f64, f64),
        rotation: f64,
    },
    /// Primitive 21: a rectangle centred on a point.
    CenterLine {
        width: f64,
        height: f64,
        center: (f64, f64),
        rotation: f64,
    },
}

/// An aperture definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Aperture {
    /// Circle with diameter (mm).
    Circle { diameter: f64 },
    /// Rectangle w x h (mm).
    Rect { width: f64, height: f64 },
    /// Obround w x h (mm).
    Obround { width: f64, height: f64 },
    /// A macro aperture reduced to the primitives we can build.
    Macro {
        name: String,
        primitives: Vec<MacroPrimitive>,
    },
}

impl Aperture {
    /// Largest extent, used for raster padding.
    pub fn max_extent(&self) -> f64 {
        match self {
            Aperture::Circle { diameter } => *diameter,
            Aperture::Rect { width, height } | Aperture::Obround { width, height } => {
                width.max(*height)
            }
            Aperture::Macro { primitives, .. } => {
                let (hx, hy) = macro_half_extent(primitives);
                2.0 * hx.max(hy)
            }
        }
    }
}

/// Half-extents of a macro's primitives, per axis, in millimetres.
pub fn macro_half_extent(primitives: &[MacroPrimitive]) -> (f64, f64) {
    let mut hx: f64 = 0.0;
    let mut hy: f64 = 0.0;
    let mut consider = |p: (f64, f64), ex: f64, ey: f64| {
        hx = hx.max(p.0.abs() + ex);
        hy = hy.max(p.1.abs() + ey);
    };
    for p in primitives {
        match p {
            MacroPrimitive::Circle {
                diameter, center, ..
            } => consider(*center, diameter / 2.0, diameter / 2.0),
            MacroPrimitive::Outline { points, .. } => {
                for q in points {
                    consider(*q, 0.0, 0.0);
                }
            }
            MacroPrimitive::VectorLine {
                width, from, to, ..
            } => {
                consider(*from, width / 2.0, width / 2.0);
                consider(*to, width / 2.0, width / 2.0);
            }
            MacroPrimitive::CenterLine {
                width,
                height,
                center,
                ..
            } => consider(*center, width / 2.0, height / 2.0),
        }
    }
    (hx, hy)
}

/// How far a macro's primitives reach from its origin (diagonal reach).
pub fn macro_reach(primitives: &[MacroPrimitive]) -> f64 {
    let (hx, hy) = macro_half_extent(primitives);
    hx.max(hy)
}

/// Token of a macro expression.
#[derive(Debug, Clone, Copy, PartialEq)]
enum MTok {
    Num(f64),
    Param(usize),
    Plus,
    Minus,
    Mul,
    Div,
}

fn tokenize(expr: &str) -> Option<Vec<MTok>> {
    let mut out = Vec::new();
    let mut it = expr.chars().peekable();
    while let Some(&c) = it.peek() {
        match c {
            ' ' | '\t' => {
                it.next();
            }
            '+' => {
                out.push(MTok::Plus);
                it.next();
            }
            '-' => {
                out.push(MTok::Minus);
                it.next();
            }
            'x' | 'X' => {
                out.push(MTok::Mul);
                it.next();
            }
            '/' => {
                out.push(MTok::Div);
                it.next();
            }
            '$' => {
                it.next();
                let mut digits = String::new();
                while let Some(&d) = it.peek() {
                    if d.is_ascii_digit() {
                        digits.push(d);
                        it.next();
                    } else {
                        break;
                    }
                }
                out.push(MTok::Param(digits.parse().ok()?));
            }
            _ if c.is_ascii_digit() || c == '.' => {
                let mut digits = String::new();
                while let Some(&d) = it.peek() {
                    if d.is_ascii_digit() || d == '.' {
                        digits.push(d);
                        it.next();
                    } else {
                        break;
                    }
                }
                out.push(MTok::Num(digits.parse().ok()?));
            }
            _ => return None,
        }
    }
    Some(out)
}

fn value_of(t: MTok, params: &[f64]) -> Option<f64> {
    match t {
        MTok::Num(v) => Some(v),
        MTok::Param(n) => params.get(n.checked_sub(1)?).copied(),
        _ => None,
    }
}

/// Evaluate a macro expression: `$n` references, numbers, and `+ - x /`.
///
/// Gerber uses `x` as the multiplication symbol inside macro expressions.
fn eval_expr(expr: &str, params: &[f64]) -> Option<f64> {
    let toks = tokenize(expr.trim())?;
    let mut i = 0usize;
    let mut total = 0.0;
    let mut sign = 1.0;
    let mut expect_value = true;
    while i < toks.len() {
        match toks[i] {
            MTok::Plus if !expect_value => {
                sign = 1.0;
                i += 1;
            }
            MTok::Minus if !expect_value => {
                sign = -1.0;
                i += 1;
            }
            MTok::Plus | MTok::Minus => {
                if toks[i] == MTok::Minus {
                    sign = -sign;
                }
                i += 1;
            }
            tok => {
                let mut product = value_of(tok, params)?;
                i += 1;
                while i < toks.len() {
                    match toks[i] {
                        MTok::Mul => {
                            product *= value_of(*toks.get(i + 1)?, params)?;
                            i += 2;
                        }
                        MTok::Div => {
                            let d = value_of(*toks.get(i + 1)?, params)?;
                            if d == 0.0 {
                                return None;
                            }
                            product /= d;
                            i += 2;
                        }
                        _ => break,
                    }
                }
                total += sign * product;
                sign = 1.0;
                expect_value = false;
            }
        }
    }
    Some(total)
}

/// Build macro primitives from a macro body and the aperture's parameter values.
///
/// Returns the primitives we can construct plus the number of primitives we had
/// to skip (unsupported codes, or clear-exposure subtractions).
fn instantiate_macro(body: &[String], params: &[f64]) -> (Vec<MacroPrimitive>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for line in body {
        let fields: Vec<&str> = line.split(',').map(|f| f.trim()).collect();
        let code = fields.first().and_then(|c| c.parse::<u32>().ok());
        let num = |i: usize| -> Option<f64> {
            fields.get(i).and_then(|f| eval_expr(f, params))
        };
        match code {
            Some(0) | None => {} // comment
            Some(1) => {
                // 1, exposure, diameter, x, y [, rotation]
                match (num(1), num(2), num(3), num(4)) {
                    (Some(exposure), Some(d), Some(x), Some(y)) if exposure != 0.0 => out.push(
                        MacroPrimitive::Circle {
                            diameter: d,
                            center: (x, y),
                            rotation: num(5).unwrap_or(0.0),
                        },
                    ),
                    (Some(0.0), ..) => skipped += 1,
                    _ => skipped += 1,
                }
            }
            Some(4) => {
                // 4, exposure, n, x0, y0, ... , x0, y0, rotation
                if fields.get(1).and_then(|f| eval_expr(f, params)) != Some(1.0) {
                    skipped += 1;
                    continue;
                }
                let n = fields.get(2).and_then(|f| eval_expr(f, params)).unwrap_or(0.0) as usize;
                if n < 3 || fields.len() < 3 + 2 * n {
                    skipped += 1;
                    continue;
                }
                let mut points = Vec::with_capacity(n);
                for k in 0..n {
                    match (num(3 + 2 * k), num(4 + 2 * k)) {
                        (Some(x), Some(y)) => points.push((x, y)),
                        _ => break,
                    }
                }
                let rotation = fields
                    .get(3 + 2 * n)
                    .and_then(|f| eval_expr(f, params))
                    .unwrap_or(0.0);
                if points.len() == n {
                    out.push(MacroPrimitive::Outline { points, rotation });
                } else {
                    skipped += 1;
                }
            }
            Some(20) => {
                // 20, exposure, width, x1, y1, x2, y2, rotation
                match (num(1), num(2), num(3), num(4), num(5), num(6)) {
                    (Some(e), Some(w), Some(x1), Some(y1), Some(x2), Some(y2)) if e != 0.0 => {
                        out.push(MacroPrimitive::VectorLine {
                            width: w,
                            from: (x1, y1),
                            to: (x2, y2),
                            rotation: num(7).unwrap_or(0.0),
                        })
                    }
                    _ => skipped += 1,
                }
            }
            Some(21) => {
                // 21, exposure, width, height, x, y, rotation
                match num(1) {
                    Some(e) if e != 0.0 => match (num(2), num(3), num(4), num(5)) {
                        (Some(w), Some(h), Some(x), Some(y)) => out.push(
                            MacroPrimitive::CenterLine {
                                width: w,
                                height: h,
                                center: (x, y),
                                rotation: num(6).unwrap_or(0.0),
                            },
                        ),
                        _ => skipped += 1,
                    },
                    _ => skipped += 1,
                }
            }
            Some(22) => {
                // 22, exposure, width, height, x, y, rotation (lower-left origin)
                match (num(1), num(2), num(3), num(4), num(5)) {
                    (Some(e), Some(w), Some(h), Some(x), Some(y)) if e != 0.0 => {
                        out.push(MacroPrimitive::CenterLine {
                            width: w,
                            height: h,
                            center: (x + w / 2.0, y + h / 2.0),
                            rotation: num(6).unwrap_or(0.0),
                        })
                    }
                    _ => skipped += 1,
                }
            }
            Some(_) => skipped += 1,
        }
    }
    (out, skipped)
}

/// A single-position feature (`D03`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flash {
    pub aperture: u32,
    pub x: f64,
    pub y: f64,
}

/// A stroked segment (`D02` then `D01`) drawn with an aperture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draw {
    pub aperture: u32,
    pub from: (f64, f64),
    pub to: (f64, f64),
}

/// A filled polygon from a `G36`/`G37` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub points: Vec<(f64, f64)>,
}

/// What the parser could not represent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParseReport {
    pub units: String,
    pub coordinate_format: String,
    /// construct name (e.g. "宏孔径 AM") -> occurrences
    pub unsupported: BTreeMap<String, usize>,
    /// Arcs that were linearised into chords (counted, approximated).
    pub arcs_linearised: usize,
    /// Objects skipped because their aperture is a macro we cannot build.
    pub skipped_objects: usize,
    /// True when the file declares clear (negative) polarity.
    pub clear_polarity: bool,
    /// Objects drawn while clear polarity was active.
    pub clear_objects: usize,
    pub aperture_count: usize,
    pub flash_count: usize,
    pub draw_count: usize,
    pub region_count: usize,
}

impl ParseReport {
    pub fn has_unsupported(&self) -> bool {
        !self.unsupported.is_empty() || self.clear_objects > 0 || self.skipped_objects > 0
    }

    /// One-line summary for tools and reports.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{}；孔径 {}、flash {}、draw {}、区域 {}",
            self.coordinate_format, self.aperture_count, self.flash_count, self.draw_count,
            self.region_count
        );
        if self.arcs_linearised > 0 {
            s.push_str(&format!("；圆弧 {} 段已线性化", self.arcs_linearised));
        }
        if self.has_unsupported() {
            let items: Vec<String> = self
                .unsupported
                .iter()
                .map(|(k, n)| format!("{k}×{n}"))
                .collect();
            if !items.is_empty() {
                s.push_str(&format!("；未完整支持：{}", items.join("、")));
            }
            if self.skipped_objects > 0 {
                s.push_str(&format!(
                    "；因孔径无法构造而跳过的图元 {} 个（已计数，未静默丢弃）",
                    self.skipped_objects
                ));
            }
            if self.clear_objects > 0 {
                s.push_str(&format!("；负极性图形 {}(已忽略)", self.clear_objects));
            }
        }
        s
    }
}

/// A parsed Gerber layer.
#[derive(Debug, Clone, PartialEq)]
pub struct GerberLayer {
    pub kind: LayerKind,
    /// Raw `TF.FileFunction` value (e.g. "Copper,L1,Top"), used to pick a layer.
    pub function: String,
    pub apertures: BTreeMap<u32, Aperture>,
    pub flashes: Vec<Flash>,
    pub draws: Vec<Draw>,
    pub regions: Vec<Region>,
    pub report: ParseReport,
}

impl GerberLayer {
    /// Bounding box of everything in the layer, if anything is present.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut b: Option<Bounds> = None;
        let mut add = |x: f64, y: f64| {
            b = Some(match b {
                None => Bounds {
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                },
                Some(v) => Bounds {
                    min_x: v.min_x.min(x),
                    min_y: v.min_y.min(y),
                    max_x: v.max_x.max(x),
                    max_y: v.max_y.max(y),
                },
            });
        };
        for f in &self.flashes {
            add(f.x, f.y);
        }
        for d in &self.draws {
            add(d.from.0, d.from.1);
            add(d.to.0, d.to.1);
        }
        for r in &self.regions {
            for (x, y) in &r.points {
                add(*x, *y);
            }
        }
        b
    }

    pub fn is_empty(&self) -> bool {
        self.flashes.is_empty() && self.draws.is_empty() && self.regions.is_empty()
    }
}

/// An axis-aligned bounding box in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Bounds {
    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }
    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum GerberError {
    #[error("缺少格式声明（FS）：无法确定坐标精度")]
    MissingFormat,
    #[error("缺少单位声明（MO）：拒绝猜测单位")]
    MissingUnits,
    #[error("不支持的坐标记法：{0}（只支持绝对 LA）")]
    UnsupportedNotation(String),
    #[error("无法解析命令 {command:?}：{reason}")]
    Command { command: String, reason: String },
    #[error("数据块引用了未定义的孔径 D{0}")]
    UnknownAperture(u32),
    #[error("区域块未闭合（缺少 G37）")]
    UnclosedRegion,
}

/// Parser state.
struct Parser {
    x_int: u32,
    x_dec: u32,
    y_int: u32,
    y_dec: u32,
    units_mm: bool,
    layer: GerberLayer,
    current_aperture: Option<u32>,
    /// Current point, in Gerber coordinates (before normalisation).
    cur: Option<(f64, f64)>,
    in_region: bool,
    region_points: Vec<(f64, f64)>,
    clear_objects_are_ignored: bool,
    /// Modal interpolation mode: 1 = linear, 2 = clockwise arc, 3 = ccw arc.
    interpolation: u32,
    /// Arcs turned into chords so far.
    arcs_linearised: usize,
    /// Macro aperture definitions, still symbolic (`$n` placeholders).
    macros: BTreeMap<String, Vec<String>>,
}

/// Parse a Gerber file.
pub fn parse(text: &str) -> Result<GerberLayer, GerberError> {
    let mut p = Parser {
        x_int: 0,
        x_dec: 0,
        y_int: 0,
        y_dec: 0,
        units_mm: true,
        layer: GerberLayer {
            kind: LayerKind::Other(String::new()),
            function: String::new(),
            apertures: BTreeMap::new(),
            flashes: Vec::new(),
            draws: Vec::new(),
            regions: Vec::new(),
            report: ParseReport::default(),
        },
        current_aperture: None,
        cur: None,
        in_region: false,
        region_points: Vec::new(),
        clear_objects_are_ignored: false,
        interpolation: 1,
        arcs_linearised: 0,
        macros: BTreeMap::new(),
    };
    let mut saw_format = false;
    let mut saw_units = false;
    let mut macro_body: Vec<String> = Vec::new();

    let mut current_macro: Option<String> = None;
    for cmd in commands(text) {
        if cmd.block_end {
            // The enclosing `%...%` block ended: the macro definition is done.
            if let Some(name) = current_macro.take() {
                p.macros.insert(name, std::mem::take(&mut macro_body));
            }
            continue;
        }
        let cmd = cmd.text.trim();
        if cmd.is_empty() {
            continue;
        }
        if let Some(name) = current_macro.clone() {
            // Macro body statements ($1 placeholders, primitive templates) are
            // not Gerber data blocks: they are stored verbatim for later
            // parameter substitution instead of being parsed as geometry.
            let _ = name;
            macro_body.push(cmd.to_string());
            continue;
        }
        // Comments: G04 ... (also used for human-readable headers).
        if cmd.starts_with("G04") || cmd.starts_with("G4") {
            continue;
        }
        if let Some(rest) = cmd.strip_prefix("FS") {
            p.parse_fs(rest)?;
            saw_format = true;
            continue;
        }
        if let Some(rest) = cmd.strip_prefix("MO") {
            match rest.trim() {
                "MM" => p.units_mm = true,
                "IN" => p.units_mm = false,
                other => {
                    return Err(GerberError::Command {
                        command: cmd.to_string(),
                        reason: format!("未知单位 {other:?}"),
                    })
                }
            }
            p.layer.report.units = rest.trim().to_string();
            saw_units = true;
            continue;
        }
        if let Some(rest) = cmd.strip_prefix("TF.FileFunction,") {
            p.layer.function = rest.trim().to_string();
            p.layer.kind = LayerKind::from_function(rest);
            continue;
        }
        if cmd.starts_with("TF.")
            || cmd.starts_with("TA.")
            || cmd.starts_with("TO.")
            || cmd.starts_with("TD")
        {
            // File, aperture and object attributes are informational. `TO.C,D12`
            // must not be mistaken for an aperture select.
            continue;
        }
        if let Some(rest) = cmd.strip_prefix("ADD") {
            p.parse_aperture(rest)?;
            continue;
        }
        if let Some(rest) = cmd.strip_prefix("LP") {
            match rest.trim() {
                "D" => {
                    p.layer.report.clear_polarity = false;
                    p.clear_objects_are_ignored = false;
                }
                "C" => {
                    p.layer.report.clear_polarity = true;
                    p.clear_objects_are_ignored = true;
                }
                _ => {}
            }
            continue;
        }
        if let Some(name) = cmd.strip_prefix("AM") {
            // A new macro may start inside the same block: close the previous.
            if let Some(previous) = current_macro.take() {
                p.macros.insert(previous, std::mem::take(&mut macro_body));
            }
            let name = name.split(',').next().unwrap_or("").trim().to_string();
            macro_body.clear();
            current_macro = Some(name);
            continue;
        }
        if cmd.starts_with("SR") {
            bump(&mut p.layer.report.unsupported, "步进重复 SR");
            continue;
        }
        if cmd.starts_with("LM") || cmd.starts_with("LR") || cmd.starts_with("LS") {
            bump(&mut p.layer.report.unsupported, "镜像/旋转/缩放 LM/LR/LS");
            continue;
        }
        if cmd.starts_with("AB") || cmd.starts_with("ABD") {
            bump(&mut p.layer.report.unsupported, "孔径块 AB");
            continue;
        }
        if cmd == "M02" || cmd == "M2" {
            break;
        }
        // Otherwise: a data block, possibly with inline G codes.
        p.parse_data_block(cmd)?;
    }

    if !saw_format {
        return Err(GerberError::MissingFormat);
    }
    if !saw_units {
        return Err(GerberError::MissingUnits);
    }
    if p.in_region {
        return Err(GerberError::UnclosedRegion);
    }
    p.layer.report.arcs_linearised = p.arcs_linearised;
    p.layer.report.aperture_count = p.layer.apertures.len();
    p.layer.report.flash_count = p.layer.flashes.len();
    p.layer.report.draw_count = p.layer.draws.len();
    p.layer.report.region_count = p.layer.regions.len();
    Ok(p.layer)
}

/// Split at the first character satisfying `pred`, keeping that character at
/// the start of the second half.
///
/// Deliberately not `str::split_once`: on this toolchain `split_once` returns the
/// text *after* the delimiter, which would silently drop part of the payload.
fn split_at<'a>(s: &'a str, pred: impl Fn(char) -> bool) -> Option<(&'a str, &'a str)> {
    for (i, c) in s.char_indices() {
        if pred(c) {
            return Some((&s[..i], &s[i..]));
        }
    }
    None
}

/// Linearise an arc into chords.
///
/// `center` is absolute; `clockwise` selects G02. The chord tolerance is 0.01mm,
/// which is well inside the raster's own half-pixel guard. Arcs are counted in
/// the parse report as approximated, not silently replaced.
fn sample_arc(
    from: (f64, f64),
    to: (f64, f64),
    center: (f64, f64),
    clockwise: bool,
) -> Vec<(f64, f64)> {
    const TOL: f64 = 0.01;
    let r = ((from.0 - center.0).powi(2) + (from.1 - center.1).powi(2)).sqrt();
    if r <= 1e-9 {
        return vec![to];
    }
    let a0 = (from.1 - center.1).atan2(from.0 - center.0);
    let a1 = (to.1 - center.1).atan2(to.0 - center.0);
    let mut sweep = a1 - a0;
    let full_turn = std::f64::consts::TAU;
    if from == to {
        sweep = if clockwise { -full_turn } else { full_turn };
    } else if clockwise {
        while sweep > 0.0 {
            sweep -= full_turn;
        }
    } else {
        while sweep < 0.0 {
            sweep += full_turn;
        }
    }
    // Sagitta step: 2*acos(1 - tol/r), clamped to 5..45 degrees.
    let ratio = (1.0 - TOL / r).clamp(-1.0, 1.0);
    let step = (2.0 * ratio.acos()).clamp(full_turn / 72.0, full_turn / 8.0);
    let n = ((sweep.abs() / step).ceil() as usize).clamp(2, 720);
    let mut out = Vec::with_capacity(n);
    for k in 1..=n {
        let a = a0 + sweep * (k as f64) / (n as f64);
        out.push((center.0 + r * a.cos(), center.1 + r * a.sin()));
    }
    if let Some(last) = out.last_mut() {
        *last = to;
    }
    out
}

fn bump(map: &mut BTreeMap<String, usize>, key: &str) {
    *map.entry(key.to_string()).or_insert(0) += 1;
}

/// One command from a Gerber stream.
struct Cmd {
    text: String,
    /// True for `%...%` extended commands.
    #[allow(dead_code)]
    extended: bool,
    /// True for the synthetic entry that marks the end of a `%...%` block.
    block_end: bool,
}

/// Split a Gerber file into commands (excluding the `*` and `%` delimiters).
///
/// Block boundaries are preserved because aperture macro bodies (`%AM...`) run
/// until the closing `%` and must not be parsed as data blocks.
fn commands(text: &str) -> Vec<Cmd> {
    let mut out: Vec<Cmd> = Vec::new();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '%' => {
                while let Some(c) = chars.next() {
                    match c {
                        '%' => {
                            if !buf.trim().is_empty() {
                                out.push(Cmd {
                                    text: std::mem::take(&mut buf),
                                    extended: true,
                                    block_end: false,
                                });
                            }
                            out.push(Cmd {
                                text: String::new(),
                                extended: true,
                                block_end: true,
                            });
                            break;
                        }
                        '*' => {
                            if !buf.trim().is_empty() {
                                out.push(Cmd {
                                    text: std::mem::take(&mut buf),
                                    extended: true,
                                    block_end: false,
                                });
                            } else {
                                buf.clear();
                            }
                        }
                        _ => buf.push(c),
                    }
                }
            }
            '*' => {
                if !buf.trim().is_empty() {
                    out.push(Cmd {
                        text: std::mem::take(&mut buf),
                        extended: false,
                        block_end: false,
                    });
                } else {
                    buf.clear();
                }
            }
            '\n' | '\r' => {}
            _ => buf.push(c),
        }
    }
    if !buf.trim().is_empty() {
        out.push(Cmd {
            text: buf,
            extended: false,
            block_end: false,
        });
    }
    out
}

impl Parser {
    fn parse_fs(&mut self, rest: &str) -> Result<(), GerberError> {
        // e.g. "LAX46Y46" (leading zeros omitted, absolute, 4.6)
        let rest = rest.trim();
        let mut it = rest.chars().peekable();
        let zero = it.next().unwrap_or(' ');
        let nota = it.next().unwrap_or(' ');
        if zero != 'L' && zero != 'T' {
            return Err(GerberError::UnsupportedNotation(format!(
                "零省略策略 {zero:?}"
            )));
        }
        if nota != 'A' {
            return Err(GerberError::UnsupportedNotation(format!(
                "坐标记法 {nota:?}（只支持绝对）"
            )));
        }
        let tail: String = it.collect();
        // e.g. tail = "X46Y46"
        let tail = tail.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        let mut parts = tail.split('Y');
        let xs = parts.next().unwrap_or("");
        let ys = parts.next().unwrap_or("");
        let (xi, xd) = parse_axis(xs)?;
        let (yi, yd) = parse_axis(ys)?;
        self.x_int = xi;
        self.x_dec = xd;
        self.y_int = yi;
        self.y_dec = yd;
        self.layer.report.coordinate_format =
            format!("FS{zero}{nota}X{xi}{xd}Y{yi}{yd}");
        Ok(())
    }

    fn parse_aperture(&mut self, rest: &str) -> Result<(), GerberError> {
        // "10R,2.000000X2.000000" or "11C,2.000000"
        let rest = rest.trim_start_matches('D');
        let (num_str, spec) = split_at(rest, |c| c.is_ascii_alphabetic()).ok_or_else(|| {
            GerberError::Command {
                command: format!("ADD{rest}"),
                reason: "缺少孔径形状".into(),
            }
        })?;
        let code: u32 = num_str.trim().parse().map_err(|_| GerberError::Command {
            command: format!("ADD{rest}"),
            reason: "孔径号不是整数".into(),
        })?;
        let shape = spec.trim();
        // A macro aperture may be referenced without any parameters.
        let (kind, params) = match split_at(shape, |c| c == ',') {
            Some((kind, params)) => (kind.trim(), params.trim_start_matches(',')),
            None => (shape.trim(), ""),
        };
        let dims: Vec<f64> = params
            .trim_start_matches(',')
            .split(['X', 'x'])
            .filter_map(|s| s.trim().parse::<f64>().ok())
            .collect();
        let aperture = match kind.to_ascii_uppercase().as_str() {
            "C" => Aperture::Circle {
                diameter: self.mm(*dims.first().unwrap_or(&0.0)),
            },
            "R" => Aperture::Rect {
                width: self.mm(*dims.first().unwrap_or(&0.0)),
                height: self.mm(*dims.get(1).unwrap_or(&0.0)),
            },
            "O" => Aperture::Obround {
                width: self.mm(*dims.first().unwrap_or(&0.0)),
                height: self.mm(*dims.get(1).unwrap_or(&0.0)),
            },
            other => {
                // A macro aperture: instantiate it from its definition.
                match self
                    .macros
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(other))
                    .map(|(name, body)| (name.clone(), body.clone()))
                {
                    Some((name, body)) => {
                        let values: Vec<f64> = params
                            .trim_start_matches(',')
                            .split(['X', 'x'])
                            .filter_map(|v| v.trim().parse::<f64>().ok())
                            .collect();
                        let (primitives, skipped) = instantiate_macro(&body, &values);
                        if skipped > 0 {
                            bump(
                                &mut self.layer.report.unsupported,
                                &format!("宏 {name} 内 {skipped} 个图元未支持"),
                            );
                        }
                        if primitives.is_empty() {
                            bump(
                                &mut self.layer.report.unsupported,
                                &format!("宏孔径 {name}（无法构造）"),
                            );
                            return Ok(());
                        }
                        Aperture::Macro { name, primitives }
                    }
                    None => {
                        bump(
                            &mut self.layer.report.unsupported,
                            &format!("孔径类型 {other}（未定义宏）"),
                        );
                        return Ok(());
                    }
                }
            }
        };
        self.layer.apertures.insert(code, aperture);
        Ok(())
    }

    /// Convert a file-unit length to millimetres.
    fn mm(&self, v: f64) -> f64 {
        if self.units_mm { v } else { v * 25.4 }
    }

    fn parse_data_block(&mut self, cmd: &str) -> Result<(), GerberError> {
        let mut x: Option<f64> = None;
        let mut y: Option<f64> = None;
        let mut i_off: Option<f64> = None;
        let mut j_off: Option<f64> = None;
        let mut dcode: Option<u32> = None;
        let mut arc_requested = false;

        let bytes = cmd.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            let c = bytes[i] as char;
            match c.to_ascii_uppercase() {
                'X' | 'Y' | 'I' | 'J' => {
                    let upper = c.to_ascii_uppercase();
                    i += 1;
                    let start = i;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_digit() || bytes[i] == b'-' || bytes[i] == b'+')
                    {
                        i += 1;
                    }
                    let raw = &cmd[start..i];
                    // Offsets (I/J) share the coordinate format.
                    let is_x_axis = upper == 'X' || upper == 'I';
                    let value = self.coord(raw, is_x_axis).map_err(|_| GerberError::Command {
                        command: cmd.to_string(),
                        reason: format!("坐标 {raw:?} 无法解析"),
                    })?;
                    match upper {
                        'X' => x = Some(value),
                        'Y' => y = Some(value),
                        'I' => i_off = Some(value),
                        _ => j_off = Some(value),
                    }
                }
                'D' => {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let n: u32 = cmd[start..i].parse().unwrap_or(0);
                    if (1..=3).contains(&n) {
                        dcode = Some(n);
                    }
                    // D10+ are aperture selects
                    else if n >= 10 {
                        self.current_aperture = Some(n);
                    }
                }
                'G' => {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let n: i32 = cmd[start..i].parse().unwrap_or(0);
                    match n {
                        // Modal interpolation: persists across blocks, which is
                        // how KiCad emits `G75*` `G02*` `X..Y..I..J..D01*`.
                        1 | 2 | 3 => {
                            self.interpolation = n as u32;
                            if n != 1 {
                                arc_requested = true;
                            }
                        }
                        36 => {
                            self.in_region = true;
                            self.region_points.clear();
                        }
                        37 => {
                            self.in_region = false;
                            if self.region_points.len() >= 3 {
                                if self.clear_objects_are_ignored {
                                    self.layer.report.clear_objects += 1;
                                } else {
                                    self.layer.regions.push(Region {
                                        points: std::mem::take(&mut self.region_points),
                                    });
                                }
                            }
                            self.region_points.clear();
                        }
                        70 | 71 | 90 | 91 => {}
                        _ => {}
                    }
                }
                _ => i += 1,
            }
        }

        let target = match (x, y) {
            (None, None) => None,
            _ => {
                let cur = self.cur.unwrap_or((0.0, 0.0));
                Some((x.unwrap_or(cur.0), y.unwrap_or(cur.1)))
            }
        };

        let Some(d) = dcode else {
            // A bare G01/G02/G03 or aperture select: only the mode changed.
            if let Some(t) = target {
                self.cur = Some(t);
                if self.in_region {
                    self.region_points.push(t);
                }
            }
            return Ok(());
        };

        // Arc mode is modal: `G75*` `G02*` then a data block with I/J and no G
        // code is how KiCad emits an arc, so the mode alone decides.
        let arc_mode = self.interpolation == 2 || self.interpolation == 3;
        let is_arc = (arc_requested || arc_mode) && (i_off.is_some() || j_off.is_some());

        // Points this operation adds, in order.
        let points: Vec<(f64, f64)> = match (d, is_arc) {
            (1, true) => {
                let from = self.cur.unwrap_or((0.0, 0.0));
                let to = target.unwrap_or(from);
                let center = (
                    from.0 + i_off.unwrap_or(0.0),
                    from.1 + j_off.unwrap_or(0.0),
                );
                let pts = sample_arc(from, to, center, self.interpolation == 2);
                self.arcs_linearised += 1;
                pts
            }
            (1, false) => vec![target.unwrap_or_else(|| self.cur.unwrap_or((0.0, 0.0)))],
            (3, _) => vec![target.unwrap_or_else(|| self.cur.unwrap_or((0.0, 0.0)))],
            _ => vec![],
        };

        match d {
            2 => {
                if let Some(t) = target {
                    self.cur = Some(t);
                    if self.in_region {
                        self.region_points.push(t);
                    }
                }
            }
            1 => {
                let from = self.cur.unwrap_or((0.0, 0.0));
                let mut previous = from;
                for p in points {
                    if self.in_region {
                        self.region_points.push(p);
                    } else if self.clear_objects_are_ignored {
                        self.layer.report.clear_objects += 1;
                    } else {
                        match self.current_aperture {
                            None => {
                                return Err(GerberError::Command {
                                    command: cmd.to_string(),
                                    reason: "绘制前没有选择孔径".into(),
                                })
                            }
                            Some(aperture) => {
                                if self.layer.apertures.contains_key(&aperture) {
                                    self.layer.draws.push(Draw {
                                        aperture,
                                        from: previous,
                                        to: p,
                                    });
                                } else {
                                    // A macro aperture we cannot build: counted,
                                    // never silently dropped.
                                    bump(
                                        &mut self.layer.report.unsupported,
                                        &format!("孔径 D{aperture} 未支持（图元已跳过）"),
                                    );
                                    self.layer.report.skipped_objects += 1;
                                }
                            }
                        }
                    }
                    previous = p;
                }
                self.cur = Some(previous);
            }
            3 => {
                let to = target.unwrap_or_else(|| self.cur.unwrap_or((0.0, 0.0)));
                if self.clear_objects_are_ignored {
                    self.layer.report.clear_objects += 1;
                } else {
                    match self.current_aperture {
                        None => {
                            return Err(GerberError::Command {
                                command: cmd.to_string(),
                                reason: "闪光前没有选择孔径".into(),
                            })
                        }
                        Some(aperture) => {
                            if self.layer.apertures.contains_key(&aperture) {
                                self.layer.flashes.push(Flash {
                                    aperture,
                                    x: to.0,
                                    y: to.1,
                                });
                            } else {
                                bump(
                                    &mut self.layer.report.unsupported,
                                    &format!("孔径 D{aperture} 未支持（图元已跳过）"),
                                );
                                self.layer.report.skipped_objects += 1;
                            }
                        }
                    }
                }
                self.cur = Some(to);
            }
            _ => {}
        }
        Ok(())
    }

    /// Convert a raw coordinate string to millimetres using the FS declaration.
    fn coord(&self, raw: &str, is_x: bool) -> Result<f64, GerberError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(GerberError::Command {
                command: raw.to_string(),
                reason: "空坐标".into(),
            });
        }
        let (int_digits, dec_digits) = if is_x {
            (self.x_int, self.x_dec)
        } else {
            (self.y_int, self.y_dec)
        };
        let negative = raw.starts_with('-');
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        // Leading zeros omitted: pad to the declared total width.
        let total = (int_digits + dec_digits) as usize;
        let padded = if digits.len() < total {
            format!("{}{}", "0".repeat(total - digits.len()), digits)
        } else {
            digits
        };
        let value: f64 = padded.parse().map_err(|_| GerberError::Command {
            command: raw.to_string(),
            reason: "坐标不是数字".into(),
        })?;
        let scaled = value / 10f64.powi(dec_digits as i32);
        let mm = self.mm(scaled);
        Ok(if negative { -mm } else { mm })
    }
}

fn parse_axis(s: &str) -> Result<(u32, u32), GerberError> {
    let s = s.trim();
    let mut it = s.chars();
    let int = it
        .next()
        .and_then(|c| c.to_digit(10))
        .ok_or_else(|| GerberError::UnsupportedNotation(s.to_string()))?;
    let dec = it
        .next()
        .and_then(|c| c.to_digit(10))
        .ok_or_else(|| GerberError::UnsupportedNotation(s.to_string()))?;
    Ok((int, dec))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n";

    fn parse_with(body: &str) -> GerberLayer {
        parse(&format!("{HEADER}{body}")).unwrap()
    }

    #[test]
    fn parses_coordinates_with_leading_zero_omission() {
        let layer = parse_with("%ADD10C,1.000000*%\nD10*\nX141605000Y-99695000D03*\nM02*\n");
        assert_eq!(layer.flashes.len(), 1);
        let f = &layer.flashes[0];
        assert!((f.x - 141.605).abs() < 1e-9, "x={}", f.x);
        assert!((f.y + 99.695).abs() < 1e-9, "y={}", f.y);
    }

    #[test]
    fn parses_flashes_and_draws() {
        let layer = parse_with(
            "%ADD10C,0.5*%\n%ADD11R,2.0X2.0*%\nD10*\nX1000000Y2000000D03*\nD11*\nX0Y0D02*\nX5000000Y0D01*\nM02*\n",
        );
        assert_eq!(layer.flashes.len(), 1);
        assert_eq!(layer.draws.len(), 1);
        let d = &layer.draws[0];
        assert_eq!(d.aperture, 11);
        assert!((d.to.0 - 5.0).abs() < 1e-9);
    }

    #[test]
    fn parses_aperture_shapes() {
        let layer = parse_with("%ADD10C,1.5*%\n%ADD11R,2.0X3.0*%\n%ADD12O,1.0X1.6*%\nM02*\n");
        assert_eq!(layer.apertures[&10], Aperture::Circle { diameter: 1.5 });
        assert_eq!(
            layer.apertures[&11],
            Aperture::Rect {
                width: 2.0,
                height: 3.0
            }
        );
        assert_eq!(
            layer.apertures[&12],
            Aperture::Obround {
                width: 1.0,
                height: 1.6
            }
        );
    }

    #[test]
    fn recognises_layer_kinds() {
        let copper = parse_with("%ADD10C,1.0*%\nM02*\n");
        assert_eq!(copper.kind, LayerKind::Copper);
        let profile = parse(&format!(
            "%TF.FileFunction,Profile,NP*%\n%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nX0Y0D02*\nX1000000Y0D01*\nM02*\n"
        ))
        .unwrap();
        assert_eq!(profile.kind, LayerKind::Profile);
        assert_eq!(profile.kind.label_zh(), "板框");
    }

    #[test]
    fn parses_regions() {
        let layer = parse_with(
            "%ADD10C,0.1*%\nD10*\nG36*\nX0Y0D02*\nX1000000Y0D01*\nX1000000Y1000000D01*\nX0Y1000000D01*\nG37*\nM02*\n",
        );
        assert_eq!(layer.regions.len(), 1);
        assert_eq!(layer.regions[0].points.len(), 4);
    }

    #[test]
    fn arcs_are_linearised_into_chords_and_counted() {
        let layer = parse_with("%ADD10C,0.5*%\nD10*\nX0Y0D02*\nG03X1000000Y0I500000J0D01*\nM02*\n");
        assert!(layer.draws.len() > 4, "an arc becomes several chords");
        assert_eq!(layer.report.arcs_linearised, 1);
        assert!(!layer.report.has_unsupported(), "arcs are approximated, not unsupported");
        assert!(layer.report.summary().contains("圆弧 1 段已线性化"));
        // Every chord endpoint stays on a 0.5mm radius semicircle around (0.5, 0).
        for d in &layer.draws {
            for p in [d.from, d.to] {
                let r = ((p.0 - 0.5).powi(2) + p.1.powi(2)).sqrt();
                assert!((r - 0.5).abs() < 0.02, "point {p:?} radius {r}");
            }
        }
        // The chord sagitta must stay inside the 0.01mm tolerance.
        for d in &layer.draws {
            let dx = d.to.0 - d.from.0;
            let dy = d.to.1 - d.from.1;
            let chord = (dx * dx + dy * dy).sqrt();
            if chord > 1e-9 {
                let sagitta = 0.5 - (0.25f64 - chord * chord / 4.0).max(0.0).sqrt();
                assert!(sagitta < 0.011, "sagitta {sagitta} too large");
            }
        }
    }

    #[test]
    fn modal_arc_mode_persists_across_blocks() {
        // How KiCad emits arcs: G75, G02, then a data block without a G code.
        let layer = parse_with("%ADD10C,0.5*%\nD10*\nX0Y0D02*\nG75*\nG02*\nX1000000Y0I500000J0D01*\nG01*\nM02*\n");
        assert_eq!(layer.report.arcs_linearised, 1);
        assert!(layer.draws.len() > 4);
    }

    #[test]
    fn a_full_circle_arc_returns_to_its_start() {
        let layer = parse_with("%ADD10C,0.2*%\nD10*\nX1000000Y0D02*\nG75*\nG03*\nX1000000Y0I-1000000J0D01*\nM02*\n");
        let last = layer.draws.last().unwrap();
        assert!((last.to.0 - 1.0).abs() < 1e-9 && last.to.1.abs() < 1e-9);
        assert!(layer.draws.len() >= 8, "a full circle needs many chords");
    }

    #[test]
    fn macro_definition_bodies_are_skipped_as_a_block() {
        // A real macro body contains statements that look like data blocks.
        let text = "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%AMROUNDEDRECT*\n0 Rectangle with rounded corners*\n0 $2 $3 $4 $5 $6 $7 $8 $9 X,Y pos of 4 corners*\n21,1,$1+$1,$2,$3,$4,$5,0*%\n%ADD10C,0.5*%\nD10*\nX1000000Y1000000D03*\nM02*\n";
        let layer = parse(text).expect("macro bodies must not break parsing");
        assert_eq!(layer.flashes.len(), 1, "the real flash still parses");
        assert_eq!(layer.report.skipped_objects, 0);
        assert!(
            matches!(layer.apertures[&10], Aperture::Circle { .. }),
            "the standard aperture is unaffected by the macro body"
        );
    }

    #[test]
    fn object_attributes_are_not_aperture_selects() {
        // `%TO.C,D15*%` names a component; it must not select aperture 15.
        let layer = parse_with(
            "%ADD10C,0.5*%\n%TO.C,D15*%\nD10*\nX1000000Y1000000D03*\nM02*\n",
        );
        assert_eq!(layer.flashes.len(), 1);
        assert_eq!(layer.flashes[0].aperture, 10);
        assert!(!layer.report.has_unsupported(), "{:?}", layer.report);
    }

    #[test]
    fn counts_step_repeat_and_undefined_macros() {
        let layer = parse_with("%SRX2Y2I1.0J1.0*%\n%ADD10C,1.0*%\n%ADD12NOPE,1.0*%\nM02*\n");
        assert_eq!(layer.report.unsupported["步进重复 SR"], 1);
        assert!(
            layer
                .report
                .unsupported
                .keys()
                .any(|k| k.contains("NOPE")),
            "{:?}",
            layer.report.unsupported
        );
        assert!(layer.report.summary().contains("步进重复"), "{}", layer.report.summary());
    }

    #[test]
    fn reports_clear_polarity_objects() {
        let layer = parse_with(
            "%ADD10C,1.0*%\n%LPC*%\nD10*\nX1000000Y1000000D03*\nM02*\n",
        );
        assert!(layer.report.clear_polarity);
        assert_eq!(layer.report.clear_objects, 1);
        assert!(layer.flashes.is_empty(), "clear objects are not treated as copper");
    }

    #[test]
    fn inches_are_converted() {
        let layer = parse(
            "%FSLAX24Y24*%\n%MOIN*%\n%ADD10C,0.010000*%\nD10*\nX10000Y0D03*\nM02*\n",
        )
        .unwrap();
        // 0.01 in aperture = 0.254 mm; X=1.0000 in = 25.4 mm
        assert!((layer.apertures[&10].max_extent() - 0.254).abs() < 1e-9);
        assert!((layer.flashes[0].x - 25.4).abs() < 1e-9);
    }

    #[test]
    fn missing_format_or_units_is_an_error() {
        assert_eq!(
            parse("%MOMM*%\nM02*\n").unwrap_err(),
            GerberError::MissingFormat
        );
        assert_eq!(
            parse("%FSLAX46Y46*%\nM02*\n").unwrap_err(),
            GerberError::MissingUnits
        );
    }

    #[test]
    fn relative_notation_is_refused() {
        assert!(matches!(
            parse("%FSLIX46Y46*%\n%MOMM*%\nM02*\n").unwrap_err(),
            GerberError::UnsupportedNotation(_)
        ));
    }

    /// The macro body KiCad writes for rounded-rectangle pads (verbatim).
    const ROUNDRECT: &str = "\
%AMRoundRect*
0 Rectangle with rounded corners*
0 $1 Rounding radius*
0 $2 $3 $4 $5 $6 $7 $8 $9 X,Y pos of 4 corners*
0 Add a 4 corners polygon primitive as box body*
4,1,4,$2,$3,$4,$5,$6,$7,$8,$9,$2,$3,0*
0 Add four circle primitives for the rounded corners*
1,1,$1+$1,$2,$3*
1,1,$1+$1,$4,$5*
1,1,$1+$1,$6,$7*
1,1,$1+$1,$8,$9*
0 Add four rect primitives between the rounded corners*
20,1,$1+$1,$2,$3,$4,$5,0*
20,1,$1+$1,$4,$5,$6,$7,0*
20,1,$1+$1,$6,$7,$8,$9,0*
20,1,$1+$1,$8,$9,$2,$3,0*%
";

    #[test]
    fn kicad_roundrect_macro_becomes_a_buildable_aperture() {
        let text = format!(
            "{ROUNDRECT}%FSLAX46Y46*%\n%MOMM*%\n%ADD10RoundRect,0.050000X-0.250000X0.150000X-0.250000X-0.150000X0.250000X-0.150000X0.250000X0.150000X0*%\nD10*\nX0Y0D03*\nM02*\n"
        );
        let layer = parse(&text).unwrap();
        assert_eq!(layer.flashes.len(), 1, "the pad must be drawn, not skipped");
        assert_eq!(layer.report.skipped_objects, 0);
        let Aperture::Macro { name, primitives } = &layer.apertures[&10] else {
            panic!("expected a macro aperture, got {:?}", layer.apertures[&10]);
        };
        assert_eq!(name, "RoundRect");
        // 1 outline + 4 circles + 4 vector lines.
        assert_eq!(primitives.len(), 9);
        // Corner circles get diameter $1+$1 = 0.1mm.
        let circles = primitives
            .iter()
            .filter(|p| matches!(p, MacroPrimitive::Circle { .. }))
            .count();
        assert_eq!(circles, 4);
        assert!((layer.apertures[&10].max_extent() - 0.6).abs() < 1e-9);

        // Area check: a 0.6 x 0.4 rounded rect with r = 0.05 is
        // 0.6*0.4 - (4 - pi)*0.05^2 = 0.237854 mm^2.
        let mut raster = crate::geometry::Raster::for_bounds(
            Bounds {
                min_x: -0.5,
                min_y: -0.5,
                max_x: 0.5,
                max_y: 0.5,
            },
            0.005,
            0.1,
        );
        raster.rasterize_into(&layer, 0.005).unwrap();
        let area = raster.occupied_count() as f64 * 0.005 * 0.005;
        let expected =
            0.6 * 0.4 - (4.0 - std::f64::consts::PI) * 0.05 * 0.05;
        assert!(
            (area - expected).abs() / expected < 0.02,
            "area {area} vs expected {expected}"
        );
    }

    #[test]
    fn macro_expressions_are_evaluated() {
        let params = [2.5, 4.0];
        assert_eq!(eval_expr("$1+$1", &params), Some(5.0));
        assert_eq!(eval_expr("$2-$1", &params), Some(1.5));
        assert_eq!(eval_expr("2x$1", &params), Some(5.0));
        assert_eq!(eval_expr("$2/2", &params), Some(2.0));
        assert_eq!(eval_expr("-1.25", &params), Some(-1.25));
        assert_eq!(eval_expr("1+2x3", &params), Some(7.0));
        assert_eq!(eval_expr("$9", &params), None, "out-of-range parameter");
        assert_eq!(eval_expr("banana", &params), None);
    }

    #[test]
    fn clear_exposure_inside_a_macro_is_counted() {
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%AMHOLE*\n1,1,1.0,0,0*\n1,0,0.5,0,0*%\n%ADD10HOLE*%\nD10*\nX0Y0D03*\nM02*\n";
        let layer = parse(text).unwrap();
        assert_eq!(layer.flashes.len(), 1);
        assert!(
            layer
                .report
                .unsupported
                .keys()
                .any(|k| k.contains("宏 HOLE") && k.contains("未支持")),
            "{:?}",
            layer.report.unsupported
        );
    }

    #[test]
    fn objects_with_an_unknown_aperture_are_counted_not_dropped_silently() {
        let layer = parse(&format!("{HEADER}D10*\nX0Y0D03*\nM02*\n"))
            .expect("an unsupported aperture must not abort the whole file");
        assert!(layer.flashes.is_empty());
        assert_eq!(layer.report.skipped_objects, 1);
        assert!(layer.report.unsupported.keys().any(|k| k.contains("D10")));
        assert!(layer.report.summary().contains("跳过的图元 1 个"));
    }

    #[test]
    fn drawing_without_any_aperture_is_still_an_error() {
        let err = parse(&format!("{HEADER}X0Y0D01*\nM02*\n")).unwrap_err();
        assert!(matches!(err, GerberError::Command { .. }), "{err:?}");
    }

    #[test]
    fn bounds_cover_all_primitives() {
        let layer = parse_with(
            "%ADD10C,1.0*%\nD10*\nX0Y0D03*\nX10000000Y0D02*\nX10000000Y5000000D01*\nM02*\n",
        );
        let b = layer.bounds().unwrap();
        assert!((b.width() - 10.0).abs() < 1e-9);
        assert!((b.height() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn report_summary_mentions_unsupported_constructs() {
        let layer = parse_with("%SRX2Y2I1.0J1.0*%\n%ADD10C,1.0*%\nM02*\n");
        let s = layer.report.summary();
        assert!(s.contains("步进重复"), "{s}");
    }
}

