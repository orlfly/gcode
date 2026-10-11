//! Laser job assembly: turn parsed artwork into ordered toolpaths and statistics.
//!
//! The job owns the three toolpath groups (isolation, drill marks, board
//! outline), the parameters with their provenance, and every caveat that must
//! reach the user. Coordinates are normalised so the board's lower-left corner
//! is the machine origin.

use crate::excellon::{DrillFile, Hole};
use crate::gerber::{Bounds, GerberLayer, LayerKind};
use crate::geometry::{self, ClearanceCheck, Path, Raster};
use crate::profile::{LaserProfile, ProfileSource};
use serde::{Deserialize, Serialize};

/// Which toolpath group a layer belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveKind {
    /// Clear the copper between traces (the board's conductive pattern).
    Isolation,
    /// Locate or cut the drill holes.
    Drill,
    /// Cut the board out along its outline.
    Outline,
}

impl MoveKind {
    pub fn label_zh(&self) -> &'static str {
        match self {
            MoveKind::Isolation => "隔离雕刻",
            MoveKind::Drill => "钻孔",
            MoveKind::Outline => "板框切割",
        }
    }

    /// Colour used in the SVG preview.
    pub fn svg_color(&self) -> &'static str {
        match self {
            MoveKind::Isolation => "#c0392b",
            MoveKind::Drill => "#2471a3",
            MoveKind::Outline => "#1e8449",
        }
    }
}

/// How drill holes are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrillMode {
    /// Mark each hole's centre at low power so it can be drilled by hand.
    Mark,
    /// Cut a circle of the hole's diameter with the laser.
    Circle,
}

/// Geometry parameters for isolation routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IsolationParams {
    /// Isolation tool (laser spot / cutter) diameter in millimetres.
    pub tool_diameter: f64,
    /// Required clearance between copper and the cut edge, in millimetres.
    pub clearance: f64,
    /// Raster resolution in millimetres (smaller = more accurate, slower).
    pub resolution: f64,
    /// Douglas-Peucker tolerance in millimetres.
    pub simplify_tolerance: f64,
}

impl Default for IsolationParams {
    fn default() -> Self {
        Self {
            tool_diameter: 0.1,
            clearance: 0.2,
            resolution: 0.05,
            simplify_tolerance: 0.02,
        }
    }
}

impl IsolationParams {
    /// The dilation radius that guarantees the requested clearance.
    ///
    /// Tool radius + clearance, plus half a pixel for raster quantisation, plus
    /// the simplification tolerance so a simplified vertex can never end up
    /// inside the requested clearance band.
    pub fn dilation_radius(&self) -> f64 {
        self.tool_diameter / 2.0 + self.clearance + self.resolution / 2.0 + self.simplify_tolerance
    }

    /// Required distance from the tool centre path to the copper.
    pub fn required_centre_clearance(&self) -> f64 {
        self.tool_diameter / 2.0 + self.clearance
    }

    pub fn validate(&self) -> Result<(), JobError> {
        if self.tool_diameter <= 0.0 {
            return Err(JobError::BadParameter("刀具直径必须大于 0".into()));
        }
        if self.clearance < 0.0 {
            return Err(JobError::BadParameter("安全间距不能为负".into()));
        }
        if self.resolution <= 0.0 {
            return Err(JobError::BadParameter("栅格分辨率必须大于 0".into()));
        }
        if self.resolution > self.clearance.max(0.05) + self.tool_diameter {
            return Err(JobError::BadParameter(format!(
                "栅格分辨率 {}mm 相对间距过大，会产生明显误差；请减小分辨率",
                self.resolution
            )));
        }
        Ok(())
    }
}

/// One group of toolpaths with its parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct JobLayer {
    pub kind: MoveKind,
    pub profile: LaserProfile,
    pub source: ProfileSource,
    pub paths: Vec<Path>,
    pub passes: u32,
}

impl JobLayer {
    pub fn cut_length(&self) -> f64 {
        self.paths.iter().map(|p| p.length()).sum()
    }
}

/// Everything known about the job's size and cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobStats {
    pub isolation_paths: usize,
    pub isolation_length_mm: f64,
    pub drill_count: usize,
    pub drill_length_mm: f64,
    pub outline_paths: usize,
    pub outline_length_mm: f64,
    pub cut_length_mm: f64,
    pub travel_length_mm: f64,
    pub estimated_minutes: f64,
    pub board_width_mm: f64,
    pub board_height_mm: f64,
}

/// A complete laser job.
#[derive(Debug, Clone, PartialEq)]
pub struct LaserJob {
    pub name: String,
    /// Board size in the job coordinate system (lower-left at 0,0).
    pub bounds: Bounds,
    /// Translation applied to raw Gerber coordinates.
    pub offset: (f64, f64),
    pub layers: Vec<JobLayer>,
    pub stats: JobStats,
    /// Facts the user must know (missing inputs, approximations).
    pub notes: Vec<String>,
    /// Things that may ruin the board if ignored.
    pub warnings: Vec<String>,
    /// Clearance measurement for the isolation layer, when it exists.
    pub clearance: Option<ClearanceCheck>,
}

impl LaserJob {
    pub fn layer(&self, kind: MoveKind) -> Option<&JobLayer> {
        self.layers.iter().find(|l| l.kind == kind)
    }

    /// Total path count.
    pub fn path_count(&self) -> usize {
        self.layers.iter().map(|l| l.paths.len()).sum()
    }

    /// Translate a point from raw Gerber coordinates into job coordinates.
    pub fn to_job(&self, p: (f64, f64)) -> (f64, f64) {
        (p.0 - self.offset.0, p.1 - self.offset.1)
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum JobError {
    #[error("参数不合法：{0}")]
    BadParameter(String),
    #[error("几何处理失败：{0}")]
    Geometry(#[from] geometry::GeometryError),
    #[error("缺少 {0}：无法生成对应走刀，请先导出该文件")]
    MissingInput(String),
}

/// Build isolation toolpaths from a copper layer.
///
/// Returns the paths in *raw Gerber coordinates* plus the clearance check.
pub fn isolation_paths(
    copper: &GerberLayer,
    params: &IsolationParams,
) -> Result<(Vec<Path>, ClearanceCheck, Raster), JobError> {
    params.validate()?;
    if copper.is_empty() {
        return Err(JobError::Geometry(geometry::GeometryError::EmptyLayer));
    }
    let raster = Raster::rasterize_layer(copper, params.resolution, 1.0)?;
    let grown = raster.dilate(params.dilation_radius());
    let contours = geometry::trace_contours(&grown);
    let mut paths: Vec<Path> = contours
        .into_iter()
        .map(|p| Path {
            points: geometry::simplify(&p.points, params.simplify_tolerance),
            closed: true,
        })
        .filter(|p| p.points.len() >= 3)
        .collect();
    paths = geometry::order_paths(paths);
    let check = geometry::verify_clearance(&paths, &raster, params.required_centre_clearance())
        .unwrap_or(ClearanceCheck {
            min_distance: f64::INFINITY,
            required: params.required_centre_clearance(),
            at: (0.0, 0.0),
            passed: true,
        });
    Ok((paths, check, raster))
}

/// Build toolpaths for the board outline by chaining the profile layer's draws.
///
/// The outline is cut along the centreline of the profile artwork, so the kerf
/// (laser spot width) is the real cut width.
pub fn outline_paths(profile: &GerberLayer) -> Vec<Path> {
    let mut segments: Vec<((f64, f64), (f64, f64))> =
        profile.draws.iter().map(|d| (d.from, d.to)).collect();
    if segments.is_empty() {
        // A region-based outline: use the region polygon directly.
        return profile
            .regions
            .iter()
            .filter(|r| r.points.len() >= 3)
            .map(|r| Path {
                points: r.points.clone(),
                closed: true,
            })
            .collect();
    }
    let key = |p: (f64, f64)| ((p.0 * 1e6).round() as i64, (p.1 * 1e6).round() as i64);
    let mut polylines: Vec<Vec<(f64, f64)>> = Vec::new();
    while let Some((a, b)) = segments.pop() {
        let mut line = vec![a, b];
        loop {
            let end = *line.last().unwrap();
            if key(end) == key(line[0]) {
                break; // closed
            }
            match segments.iter().position(|(s, e)| {
                key(*s) == key(end) || key(*e) == key(end)
            }) {
                Some(i) => {
                    let (s, e) = segments.remove(i);
                    let next = if key(s) == key(end) { e } else { s };
                    line.push(next);
                }
                None => break,
            }
        }
        polylines.push(line);
    }
    geometry::order_paths(
        polylines
            .into_iter()
            .filter(|l| l.len() >= 2)
            .map(|l| {
                let closed = l.len() > 2 && key(l[0]) == key(*l.last().unwrap());
                Path {
                    points: l,
                    closed,
                }
            })
            .collect(),
    )
}

/// Build drill toolpaths.
pub fn drill_paths(holes: &[Hole], mode: DrillMode) -> Vec<Path> {
    let mut paths: Vec<Path> = holes
        .iter()
        .map(|h| match mode {
            DrillMode::Mark => Path {
                // A single point: move there, dwell, move on.
                points: vec![(h.x, h.y)],
                closed: false,
            },
            DrillMode::Circle => {
                let r = h.diameter / 2.0;
                let n = 48;
                let points = (0..n)
                    .map(|i| {
                        let a = std::f64::consts::TAU * (i as f64) / (n as f64);
                        (h.x + r * a.cos(), h.y + r * a.sin())
                    })
                    .collect();
                Path { points, closed: true }
            }
        })
        .collect();
    // Deterministic order: left-to-right, bottom-to-top.
    paths.sort_by(|a, b| {
        a.start()
            .partial_cmp(&b.start())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    paths
}

/// Inputs for [`build_job`].
pub struct JobInputs<'a> {
    pub name: String,
    /// Copper layer (required for isolation).
    pub copper: Option<&'a GerberLayer>,
    /// Profile/outline layer.
    pub profile_layer: Option<&'a GerberLayer>,
    /// Drill file.
    pub drill: Option<&'a DrillFile>,
    pub isolation: IsolationParams,
    pub drill_mode: DrillMode,
    // Profile selection for each group.
    pub isolation_profile: LaserProfile,
    pub isolation_source: ProfileSource,
    pub drill_profile: LaserProfile,
    pub drill_source: ProfileSource,
    pub outline_profile: LaserProfile,
    pub outline_source: ProfileSource,
    /// Extra notes from the import step (unsupported constructs, etc.).
    pub import_notes: Vec<String>,
}

/// Assemble a job from parsed artwork.
pub fn build_job(inputs: JobInputs<'_>) -> Result<LaserJob, JobError> {
    let mut notes = inputs.import_notes.clone();
    let mut warnings = Vec::new();
    let mut layers: Vec<JobLayer> = Vec::new();
    let mut clearance = None;

    // ---- gather raw geometry -------------------------------------------------
    let mut raw_isolation: Vec<Path> = Vec::new();
    if let Some(copper) = inputs.copper {
        let (paths, check, _raster) = isolation_paths(copper, &inputs.isolation)?;
        if !check.passed {
            warnings.push(format!(
                "隔离路径到铜的最小距离 {:.3}mm 小于要求 {:.3}mm（位置 {:.3},{:.3}）：\
                 请减小栅格分辨率或增大安全间距后重算",
                check.min_distance, check.required, check.at.0, check.at.1
            ));
        }
        if !copper.report.has_unsupported() && paths.is_empty() {
            warnings.push("铜层解析后没有得到任何走刀，请检查文件内容".into());
        }
        notes.push(format!("隔离雕刻：{} 条闭合路径", paths.len()));
        notes.push(format!(
            "隔离遍数 {} = 同一条路径重复走（用于逐遍加深切缝）；要加宽隔离缝请增大安全间距（当前 {:.3}mm）",
            inputs.isolation_profile.passes, inputs.isolation.clearance
        ));
        raw_isolation = paths;
        clearance = Some(check);
    } else {
        notes.push("没有提供铜层文件：本次作业不含隔离雕刻".into());
    }

    let mut raw_drill: Vec<Path> = Vec::new();
    if let Some(d) = inputs.drill {
        raw_drill = drill_paths(&d.holes, inputs.drill_mode);
        notes.push(format!(
            "钻孔：{} 个孔，模式 {}",
            d.holes.len(),
            match inputs.drill_mode {
                DrillMode::Mark => "标记（不做通孔，供后续手钻定位）",
                DrillMode::Circle => "激光切圆（实际孔径 = 孔径 + 光斑宽度）",
            }
        ));
        if inputs.drill_mode == DrillMode::Mark {
            warnings.push(
                "钻孔模式为标记：激光只在孔位烧出定位点，不会形成通孔，仍需机械钻孔".into(),
            );
        }
    } else {
        notes.push("没有提供钻孔文件：本次作业不含孔位标记".into());
    }

    let mut raw_outline: Vec<Path> = Vec::new();
    if let Some(pl) = inputs.profile_layer {
        raw_outline = outline_paths(pl);
        if raw_outline.is_empty() {
            notes.push("板框文件里没有可用的线段或区域：本次作业不含板框切割".into());
        } else {
            notes.push(format!(
                "板框：{} 条路径，沿板框中心线切割（实际切缝 = 光斑宽度）",
                raw_outline.len()
            ));
            warnings.push(
                "板框切完板子会脱落：请先用双面胶/夹具固定，必要时自行添加连接桥".into(),
            );
        }
    } else {
        notes.push("没有提供板框文件：本次作业不含板框切割，板子不会与板材分离".into());
    }

    // ---- normalise coordinates ----------------------------------------------
    let mut all: Vec<(f64, f64)> = Vec::new();
    for set in [&raw_isolation, &raw_drill, &raw_outline] {
        for p in set {
            all.extend(p.points.iter().copied());
        }
    }
    if all.is_empty() {
        return Err(JobError::MissingInput(
            "任何可加工图形（铜层/板框/钻孔）".into(),
        ));
    }
    let min_x = all.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let min_y = all.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_x = all.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let max_y = all.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let offset = (min_x, min_y);
    let bounds = Bounds {
        min_x: 0.0,
        min_y: 0.0,
        max_x: max_x - min_x,
        max_y: max_y - min_y,
    };
    let shift = |paths: Vec<Path>| -> Vec<Path> {
        paths
            .into_iter()
            .map(|p| Path {
                points: p
                    .points
                    .into_iter()
                    .map(|(x, y)| (x - min_x, y - min_y))
                    .collect(),
                closed: p.closed,
            })
            .collect()
    };

    if let Some(c) = &mut clearance {
        let (ax, ay) = (c.at.0 - min_x, c.at.1 - min_y);
        c.at = (ax, ay);
    }

    let iso = shift(raw_isolation);
    let drl = shift(raw_drill);
    let out = shift(raw_outline);

    if !iso.is_empty() {
        // Passes come from the parameter profile alone, so there is exactly one
        // place to change them (a user override updates the same field).
        let passes = inputs.isolation_profile.passes;
        layers.push(JobLayer {
            kind: MoveKind::Isolation,
            profile: inputs.isolation_profile,
            source: inputs.isolation_source,
            paths: iso,
            passes,
        });
    }
    if !drl.is_empty() {
        layers.push(JobLayer {
            kind: MoveKind::Drill,
            profile: inputs.drill_profile,
            source: inputs.drill_source,
            paths: drl,
            passes: 1,
        });
    }
    if !out.is_empty() {
        let passes = inputs.outline_profile.passes;
        layers.push(JobLayer {
            kind: MoveKind::Outline,
            profile: inputs.outline_profile,
            source: inputs.outline_source,
            paths: out,
            passes,
        });
    }

    notes.push(format!(
        "坐标已归一：原 Gerber 左下角 ({min_x:.3}, {min_y:.3}) 移到机床原点 (0,0)，\
         Y 轴保持 Gerber 的上为正约定"
    ));
    notes.push(format!("作业尺寸 {:.3} × {:.3} mm", bounds.width(), bounds.height()));

    let stats = compute_stats(&bounds, &layers);
    Ok(LaserJob {
        name: inputs.name,
        bounds,
        offset,
        layers,
        stats,
        notes,
        warnings,
        clearance,
    })
}

/// Compute deterministic statistics for a set of layers.
pub fn compute_stats(bounds: &Bounds, layers: &[JobLayer]) -> JobStats {
    let iso = layers.iter().find(|l| l.kind == MoveKind::Isolation);
    let drl = layers.iter().find(|l| l.kind == MoveKind::Drill);
    let out = layers.iter().find(|l| l.kind == MoveKind::Outline);

    let cut_length_mm = layers
        .iter()
        .map(|l| l.cut_length() * l.passes as f64)
        .sum::<f64>();
    let travel = travel_length(layers);

    // Time: cutting at each layer's feed, travel at a fixed rapid rate, plus a
    // dwell per drill mark.
    let mut minutes = 0.0;
    for l in layers {
        let feed = l.profile.speed_mm_min.max(1) as f64;
        minutes += l.cut_length() * l.passes as f64 / feed;
        // Drill marks are single-point paths and need a dwell each.
        if l.kind == MoveKind::Drill && l.paths.iter().all(|p| p.points.len() == 1) {
            minutes += l.paths.len() as f64 * (2.0 / 60.0); // ~2 s per mark
        }
    }
    minutes += travel / 3000.0;
    let drill_count = drl.map(|l| l.paths.len()).unwrap_or(0);

    JobStats {
        isolation_paths: iso.map(|l| l.paths.len()).unwrap_or(0),
        isolation_length_mm: iso.map(|l| l.cut_length()).unwrap_or(0.0),
        drill_count,
        drill_length_mm: drl.map(|l| l.cut_length()).unwrap_or(0.0),
        outline_paths: out.map(|l| l.paths.len()).unwrap_or(0),
        outline_length_mm: out.map(|l| l.cut_length()).unwrap_or(0.0),
        cut_length_mm,
        travel_length_mm: travel,
        estimated_minutes: minutes,
        board_width_mm: bounds.width(),
        board_height_mm: bounds.height(),
    }
}

/// Sum of the rapid moves between consecutive paths (laser off).
fn travel_length(layers: &[JobLayer]) -> f64 {
    let mut total = 0.0;
    let mut pos = (0.0, 0.0);
    for l in layers {
        for p in &l.paths {
            if let Some(s) = p.start() {
                total += ((s.0 - pos.0).powi(2) + (s.1 - pos.1).powi(2)).sqrt();
            }
            pos = p.end().or(p.start()).unwrap_or(pos);
        }
    }
    total
}

/// Convenience: infer what a layer file is from its parsed kind.
pub fn layer_role(kind: &LayerKind) -> &'static str {
    match kind {
        LayerKind::Copper => "copper",
        LayerKind::Profile => "profile",
        LayerKind::Other(_) => "ignored",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerber;

    fn copper_layer() -> GerberLayer {
        // Two pads with a trace between them, inside a 20x20 area.
        let text = "\
%TF.FileFunction,Copper,L1,Top*%
%FSLAX46Y46*%
%MOMM*%
%LPD*%
G01*
%ADD10C,2.000000*%
%ADD11C,0.500000*%
D10*
X5000000Y5000000D03*
X15000000Y5000000D03*
D11*
X5000000Y5000000D02*
X15000000Y5000000D01*
M02*
";
        gerber::parse(text).unwrap()
    }

    fn profile_layer() -> GerberLayer {
        let text = "\
%TF.FileFunction,Profile,NP*%
%FSLAX46Y46*%
%MOMM*%
%LPD*%
G01*
%ADD10C,0.050000*%
D10*
X0Y0D02*
X20000000Y0D01*
X20000000Y20000000D01*
X0Y20000000D01*
X0Y0D01*
M02*
";
        gerber::parse(text).unwrap()
    }

    fn drill() -> DrillFile {
        let text = "\
M48
METRIC
T1C0.800
%
T1
X5.0Y5.0
X15.0Y5.0
M30
";
        // Excellon uses decimal coordinates (mm), not the Gerber scaling.
        crate::excellon::parse(text).unwrap()
    }

    fn profile(kind: &str) -> LaserProfile {
        let mut p = crate::profile::profiles()
            .into_iter()
            .find(|p| p.id.contains(kind))
            .unwrap();
        p.passes = 2;
        p
    }

    fn inputs<'a>(
        copper: Option<&'a GerberLayer>,
        pl: Option<&'a GerberLayer>,
        d: Option<&'a DrillFile>,
    ) -> JobInputs<'a> {
        let src = |id: &str| ProfileSource::KnowledgeBase {
            id: id.into(),
            source: "test".into(),
        };
        JobInputs {
            name: "test-board".into(),
            copper,
            profile_layer: pl,
            drill: d,
            isolation: IsolationParams {
                resolution: 0.1,
                simplify_tolerance: 0.05,
                ..Default::default()
            },
            drill_mode: DrillMode::Mark,
            isolation_profile: profile("isol"),
            isolation_source: src("fr4-1.6-isolation"),
            drill_profile: profile("drillmark"),
            drill_source: src("fr4-drillmark"),
            outline_profile: profile("outline"),
            outline_source: src("fr4-outline-1.6"),
            import_notes: vec![],
        }
    }

    #[test]
    fn isolation_paths_keep_the_requested_clearance() {
        let layer = copper_layer();
        let params = IsolationParams {
            resolution: 0.05,
            tool_diameter: 0.1,
            clearance: 0.2,
            simplify_tolerance: 0.02,
        };
        let (paths, check, _r) = isolation_paths(&layer, &params).unwrap();
        assert!(!paths.is_empty(), "some isolation paths expected");
        assert!(
            check.passed,
            "clearance check failed: {check:?} (required {})",
            params.required_centre_clearance()
        );
        assert!(
            check.min_distance >= params.required_centre_clearance() - 0.001,
            "{check:?}"
        );
        for p in &paths {
            assert!(p.closed, "isolation contours must be closed");
        }
    }

    #[test]
    fn a_larger_clearance_pushes_paths_further_from_copper() {
        let layer = copper_layer();
        let small = isolation_paths(
            &layer,
            &IsolationParams {
                resolution: 0.05,
                clearance: 0.1,
                ..Default::default()
            },
        )
        .unwrap();
        let large = isolation_paths(
            &layer,
            &IsolationParams {
                resolution: 0.05,
                clearance: 0.6,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            large.1.min_distance > small.1.min_distance,
            "small {:?} large {:?}",
            small.1,
            large.1
        );
    }

    #[test]
    fn invalid_parameters_are_refused() {
        assert!(matches!(
            IsolationParams {
                tool_diameter: 0.0,
                ..Default::default()
            }
            .validate(),
            Err(JobError::BadParameter(_))
        ));
        assert!(matches!(
            IsolationParams {
                clearance: -1.0,
                ..Default::default()
            }
            .validate(),
            Err(JobError::BadParameter(_))
        ));
        assert!(matches!(
            IsolationParams {
                resolution: 0.0,
                ..Default::default()
            }
            .validate(),
            Err(JobError::BadParameter(_))
        ));
        assert!(matches!(
            IsolationParams {
                resolution: 5.0,
                ..Default::default()
            }
            .validate(),
            Err(JobError::BadParameter(_))
        ));
    }

    #[test]
    fn outline_chains_the_profile_segments_into_one_closed_path() {
        let paths = outline_paths(&profile_layer());
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(paths[0].closed);
        // 4 corners + the repeated start point.
        assert!(paths[0].points.len() >= 5, "{:?}", paths[0].points);
        let len = paths[0].length();
        assert!((len - 80.0).abs() < 0.001, "perimeter {len}");
    }

    #[test]
    fn drill_marks_are_single_points_in_reading_order() {
        let d = drill();
        let paths = drill_paths(&d.holes, DrillMode::Mark);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].points.len(), 1);
        assert_eq!(paths[0].length(), 0.0, "a mark has no cut length");
        assert_eq!(paths[0].start(), Some((5.0, 5.0)));
        assert_eq!(paths[1].start(), Some((15.0, 5.0)));
    }

    #[test]
    fn drill_circles_have_the_hole_diameter() {
        let d = drill();
        let paths = drill_paths(&d.holes, DrillMode::Circle);
        let p = &paths[0];
        assert!(p.closed);
        let r = p
            .points
            .iter()
            .map(|(x, y)| ((x - 5.0).powi(2) + (y - 5.0).powi(2)).sqrt())
            .fold(0.0f64, f64::max);
        assert!((r - 0.4).abs() < 1e-9, "radius {r}");
    }

    #[test]
    fn job_normalises_coordinates_to_the_origin() {
        let copper = copper_layer();
        let pl = profile_layer();
        let d = drill();
        let job = build_job(inputs(Some(&copper), Some(&pl), Some(&d))).unwrap();
        assert_eq!(job.bounds.min_x, 0.0);
        assert_eq!(job.bounds.min_y, 0.0);
        assert!((job.offset.0 - 0.0).abs() < 1e-9);
        for l in &job.layers {
            for p in &l.paths {
                for (x, y) in &p.points {
                    assert!(*x >= -1e-9 && *x <= job.bounds.width() + 1e-9, "x {x}");
                    assert!(*y >= -1e-9 && *y <= job.bounds.height() + 1e-9, "y {y}");
                }
            }
        }
        assert!((job.bounds.width() - 20.0).abs() < 0.01);
    }

    #[test]
    fn job_records_stats_and_notes() {
        let copper = copper_layer();
        let pl = profile_layer();
        let d = drill();
        let job = build_job(inputs(Some(&copper), Some(&pl), Some(&d))).unwrap();
        assert!(job.stats.isolation_paths > 0);
        assert_eq!(job.stats.drill_count, 2);
        assert_eq!(job.stats.outline_paths, 1);
        assert!(job.stats.cut_length_mm > 0.0);
        assert!(job.stats.travel_length_mm > 0.0);
        assert!(job.stats.estimated_minutes > 0.0);
        assert!(job.stats.board_width_mm > 0.0);
        assert!(job.notes.iter().any(|n| n.contains("坐标已归一")));
        // Mark mode must warn that no through-hole is made.
        assert!(job.warnings.iter().any(|w| w.contains("不会形成通孔")));
    }

    #[test]
    fn stats_are_reproducible() {
        let copper = copper_layer();
        let pl = profile_layer();
        let d = drill();
        let a = build_job(inputs(Some(&copper), Some(&pl), Some(&d))).unwrap();
        let b = build_job(inputs(Some(&copper), Some(&pl), Some(&d))).unwrap();
        assert_eq!(a.stats, b.stats);
        assert_eq!(a.layers, b.layers);
    }

    #[test]
    fn missing_inputs_are_reported_not_silently_dropped() {
        let copper = copper_layer();
        let job = build_job(inputs(Some(&copper), None, None)).unwrap();
        assert_eq!(job.layers.len(), 1);
        assert!(job.notes.iter().any(|n| n.contains("不含板框切割")));
        assert!(job.notes.iter().any(|n| n.contains("不含孔位标记")));
    }

    #[test]
    fn a_job_with_no_geometry_at_all_is_an_error() {
        assert!(matches!(
            build_job(inputs(None, None, None)),
            Err(JobError::MissingInput(_))
        ));
    }

    #[test]
    fn passes_come_from_the_profile_and_multiply_the_cut_length() {
        let copper = copper_layer();
        let mut i = inputs(Some(&copper), None, None);
        i.isolation_profile.passes = 1;
        let one = build_job(i).unwrap();
        let mut i2 = inputs(Some(&copper), None, None);
        i2.isolation_profile.passes = 3;
        let three = build_job(i2).unwrap();
        assert!((three.stats.cut_length_mm - one.stats.cut_length_mm * 3.0).abs() < 1e-9);
        assert_eq!(three.layer(MoveKind::Isolation).unwrap().passes, 3);
    }

    #[test]
    fn dilation_radius_includes_both_guard_terms() {
        let p = IsolationParams {
            tool_diameter: 0.2,
            clearance: 0.3,
            resolution: 0.05,
            simplify_tolerance: 0.02,
        };
        // 0.1 + 0.3 + 0.025 + 0.02
        assert!((p.dilation_radius() - 0.445).abs() < 1e-12);
        assert!((p.required_centre_clearance() - 0.4).abs() < 1e-12);
    }
}
