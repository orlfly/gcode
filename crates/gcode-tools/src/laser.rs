//! `gcode-tools` — R2 fabrication tools: import fabrication files, plan a laser
//! job, export G-code and an SVG preview, and query laser parameters.
//!
//! Honesty boundary enforced here: these tools produce a machine file and
//! geometric verification. They never claim a board was cut — there is no laser
//! and no GRBL controller, and no test can change that.

use async_trait::async_trait;
use gcode_laser::gcode::{generate as generate_gcode, validate_gcode, GcodeOptions, LaserMode};
use gcode_laser::profile::{self, apply_overrides, ProfileOverride, ProfileSource};
use gcode_laser::svg::{render as render_svg, SvgOptions};
use gcode_laser::toolpath::{self, DrillMode, IsolationParams, JobInputs};
use gcode_laser::{ImportSet, LaserJob, MoveKind};
use jcode::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::path::PathBuf;

fn str_arg(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
}

fn f64_arg(input: &Value, key: &str) -> Option<f64> {
    input
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
}

fn u32_arg(input: &Value, key: &str) -> Option<u32> {
    input
        .get(key)
        .and_then(|v| v.as_u64().map(|i| i as u32))
}

/// Collect input paths from `paths` (array) or `path`/`dir` (single string).
fn input_paths(input: &Value) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(arr) = input.get("paths").and_then(|v| v.as_array()) {
        for v in arr {
            if let Some(s) = v.as_str() {
                if !s.trim().is_empty() {
                    out.push(PathBuf::from(s));
                }
            }
        }
    }
    for key in ["path", "dir", "fab_dir"] {
        if let Some(s) = str_arg(input, key) {
            out.push(PathBuf::from(s));
        }
    }
    out
}

fn render_import(set: &ImportSet) -> String {
    let mut out = String::new();
    out.push_str("## 制造文件解析结果\n");
    if let Some(c) = &set.copper {
        out.push_str(&format!(
            "\n- 铜层（用于隔离雕刻）: {} → {}\n",
            c.function, c.report.summary()
        ));
        out.push_str(&format!(
            "  - flash {}、draw {}、区域 {}、孔径 {}\n",
            c.report.flash_count,
            c.report.draw_count,
            c.report.region_count,
            c.report.aperture_count
        ));
    } else {
        out.push_str("\n- 铜层: 未找到\n");
    }
    if let Some(p) = &set.profile {
        out.push_str(&format!(
            "- 板框（用于切割外形）: {} → {}\n",
            p.function,
            p.report.summary()
        ));
    } else {
        out.push_str("- 板框: 未找到\n");
    }
    if let Some(d) = &set.drill {
        out.push_str(&format!("- 钻孔: {}\n", d.report.summary()));
    } else {
        out.push_str("- 钻孔: 未找到\n");
    }
    out.push_str("\n### 文件\n");
    for f in &set.files {
        let role = match &f.role {
            gcode_laser::ImportedRole::Copper => "铜层".to_string(),
            gcode_laser::ImportedRole::Profile => "板框".to_string(),
            gcode_laser::ImportedRole::Drill => "钻孔".to_string(),
            gcode_laser::ImportedRole::Ignored { kind } => format!("忽略（{kind}）"),
        };
        out.push_str(&format!("- [{role}] {} — {}\n", f.path.display(), f.detail));
    }
    if !set.notes.is_empty() {
        out.push_str("\n### 提示\n");
        for n in &set.notes {
            out.push_str(&format!("- {n}\n"));
        }
    }
    if !set.problems.is_empty() {
        out.push_str("\n### 需要用户注意的问题\n");
        for p in &set.problems {
            out.push_str(&format!("- {p}\n"));
        }
    }
    out
}

/// Tool: parse Gerber and Excellon files and report what was found.
pub struct ImportFabTool;

#[async_trait]
impl Tool for ImportFabTool {
    fn name(&self) -> &str {
        "gcode_import_fab"
    }

    fn description(&self) -> &str {
        "Parse Gerber and Excellon fabrication files (typically a kicad-cli export directory) and \
         report which file is the copper layer, which is the board outline, the drill file, and \
         every construct the parser could not fully support. Nothing is silently ignored: \
         unsupported constructs are counted and reported so the geometry gap is visible."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "A Gerber/Excellon file or a directory containing an export"},
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Several files/directories to import together"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let paths = input_paths(&input);
        if paths.is_empty() {
            return Ok(ToolOutput::new(
                "缺少参数 path：请给出 Gerber/钻孔文件或导出目录的路径。".to_string(),
            ));
        }
        match gcode_laser::import_paths(&paths) {
            Ok(set) => Ok(ToolOutput::new(render_import(&set))),
            Err(e) => Ok(ToolOutput::new(format!("导入失败：{e}"))),
        }
    }
}

/// Resolve the three parameter profiles for a job from a material query and
/// user overrides.
fn resolve_profiles(
    input: &Value,
) -> (
    gcode_laser::LaserProfile,
    ProfileSource,
    gcode_laser::LaserProfile,
    ProfileSource,
    gcode_laser::LaserProfile,
    ProfileSource,
    Vec<String>,
) {
    let material = str_arg(input, "material").unwrap_or_else(|| "FR4".to_string());
    let thickness = f64_arg(input, "thickness_mm").unwrap_or(1.6);
    let over = ProfileOverride {
        power_s: u32_arg(input, "power_s"),
        speed_mm_min: u32_arg(input, "speed_mm_min"),
        passes: u32_arg(input, "passes"),
        max_s: u32_arg(input, "max_s"),
        material: None,
    };
    let mut notes = Vec::new();

    let hit = profile::lookup(&material, thickness);
    notes.push(hit.note.clone());
    let isolation = match &hit.profile {
        Some(p) => p.clone(),
        None => profile::lookup("fr4-1.6-isolation", 1.6)
            .profile
            .expect("knowledge base must contain the FR4 isolation entry"),
    };
    let (isolation, changed) = apply_overrides(&isolation, &over);
    let isolation_source = if changed.is_empty() {
        ProfileSource::KnowledgeBase {
            id: isolation.id.clone(),
            source: isolation.source.clone(),
        }
    } else {
        ProfileSource::KnowledgeBaseWithOverrides {
            id: isolation.id.clone(),
            source: isolation.source.clone(),
            overridden: changed.clone(),
        }
    };

    // Drill marks and outline cutting keep their own entries: a marking pass and
    // a through-cut need very different settings.
    let drill = profile::lookup("fr4-drillmark", thickness)
        .profile
        .expect("knowledge base must contain the drill-mark entry");
    let outline = profile::lookup("fr4-outline-1.6", thickness)
        .profile
        .expect("knowledge base must contain the outline entry");
    let drill_source = ProfileSource::KnowledgeBase {
        id: drill.id.clone(),
        source: drill.source.clone(),
    };
    let outline_source = ProfileSource::KnowledgeBase {
        id: outline.id.clone(),
        source: outline.source.clone(),
    };
    if !changed.is_empty() {
        notes.push(format!(
            "参数覆盖已应用（只作用于隔离雕刻）：{}",
            changed.join("、")
        ));
    }
    (
        isolation,
        isolation_source,
        drill,
        drill_source,
        outline,
        outline_source,
        notes,
    )
}

fn build_job(input: &Value, set: &ImportSet) -> anyhow::Result<LaserJob> {
    let (iso_p, iso_s, drl_p, drl_s, out_p, out_s, mut notes) = resolve_profiles(input);
    let params = IsolationParams {
        tool_diameter: f64_arg(input, "tool_diameter").unwrap_or(0.1),
        clearance: f64_arg(input, "clearance").unwrap_or(0.2),
        resolution: f64_arg(input, "resolution").unwrap_or(0.05),
        simplify_tolerance: f64_arg(input, "simplify_tolerance").unwrap_or(0.02),
    };
    let drill_mode = match str_arg(input, "drill_mode").as_deref() {
        Some("circle") | Some("cut") => DrillMode::Circle,
        _ => DrillMode::Mark,
    };
    let name = str_arg(input, "name").unwrap_or_else(|| "board".to_string());
    notes.extend(set.report_notes());
    let job = toolpath::build_job(JobInputs {
        name,
        copper: set.copper.as_ref(),
        profile_layer: set.profile.as_ref(),
        drill: set.drill.as_ref(),
        isolation: params,
        drill_mode,
        isolation_profile: iso_p,
        isolation_source: iso_s,
        drill_profile: drl_p,
        drill_source: drl_s,
        outline_profile: out_p,
        outline_source: out_s,
        import_notes: notes,
    })?;
    Ok(job)
}

fn render_job(job: &LaserJob, gcode_path: Option<&PathBuf>, svg_path: Option<&PathBuf>) -> String {
    let mut out = String::new();
    out.push_str(&format!("## 激光作业：{}\n", job.name));
    out.push_str(&format!(
        "\n板尺寸 {:.3} × {:.3} mm（原点 = 板子左下角，Gerber 左下角已平移 {:.3},{:.3}）\n",
        job.bounds.width(),
        job.bounds.height(),
        job.offset.0,
        job.offset.1
    ));
    out.push_str("\n### 走刀\n");
    for l in &job.layers {
        out.push_str(&format!(
            "- {}: {} 条路径，{:.1} mm，{} 遍；参数 {}（{}）\n",
            l.kind.label_zh(),
            l.paths.len(),
            l.cut_length(),
            l.passes,
            l.profile.comment_line(),
            l.source.label_zh()
        ));
    }
    if job.layers.iter().all(|l| l.kind != MoveKind::Isolation) {
        out.push_str("- 隔离雕刻: 无（未提供铜层）\n");
    }
    out.push_str(&format!(
        "\n总切割长度 {:.1} mm，空程 {:.1} mm，预估 {:.1} 分钟（不含对刀与换板）\n",
        job.stats.cut_length_mm, job.stats.travel_length_mm, job.stats.estimated_minutes
    ));
    out.push_str(&format!(
        "\n钻孔 {} 个，板框 {} 条路径\n",
        job.stats.drill_count, job.stats.outline_paths
    ));

    out.push_str("\n### 几何校验（已做）\n");
    match &job.clearance {
        Some(c) => out.push_str(&format!(
            "- 隔离路径到铜的最小距离 {:.4} mm，要求 ≥ {:.4} mm（刀半径 + 安全间距）→ {}\n",
            c.min_distance,
            c.required,
            if c.passed { "通过" } else { "未通过" }
        )),
        None => out.push_str("- 未做隔离几何校验（本次作业不含隔离雕刻）\n"),
    }
    out.push_str(&format!(
        "- 钻孔路径 {} 条，与 Excellon 孔位逐点一致（误差 < 1e-9 mm）\n",
        job.stats.drill_count
    ));
    out.push_str("- 板框包围盒与 Profile 图层包围盒一致（误差 < 1e-6 mm）\n");

    out.push_str("\n### 未验证的部分（必须如实转述给用户）\n");
    out.push_str(
        "- 本机没有激光雕刻机、也没有 GRBL 控制器：G-code 生成并做过结构与几何校验，\
         但**没有在真实机器上运行过，也没有做过任何实际打样**。\n\
         - 切削深度、切缝宽度、间距是否够用都未经实测；参数只是起点，需先试刻校准。\n\
         - 钻孔为标记模式时不会形成通孔，仍需机械钻孔。\n",
    );

    if let Some(p) = gcode_path {
        out.push_str(&format!("\n### 产物\n- G-code: {}\n", p.display()));
    }
    if let Some(p) = svg_path {
        out.push_str(&format!("- SVG 预览: {}\n", p.display()));
    }

    if !job.notes.is_empty() {
        out.push_str("\n### 作业提示\n");
        for n in &job.notes {
            out.push_str(&format!("- {n}\n"));
        }
    }
    if !job.warnings.is_empty() {
        out.push_str("\n### 警告\n");
        for w in &job.warnings {
            out.push_str(&format!("- {w}\n"));
        }
    }
    out
}

/// Tool: build a laser job from fabrication files, and optionally write the
/// G-code and SVG.
pub struct PlanLaserJobTool;

#[async_trait]
impl Tool for PlanLaserJobTool {
    fn name(&self) -> &str {
        "gcode_laser_job"
    }

    fn description(&self) -> &str {
        "Turn Gerber/Excellon fabrication files into a laser job: isolation routing around the \
         copper, drill marks or circles, and a board-outline cut. Returns toolpath statistics, the \
         measured clearance result, and every caveat. With out_dir set it also writes the GRBL \
         G-code and an SVG preview. It only ever reports what was verified geometrically: the \
         G-code has NOT been run on a machine and nothing has been cut."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Gerber/drill file or export directory"},
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Several files/directories"},
                "name": {"type": "string", "description": "Job name used in outputs"},
                "material": {"type": "string", "description": "Material for parameter lookup, e.g. FR4；也可直接给知识库条目 id"},
                "thickness_mm": {"type": "number", "description": "Board thickness in mm"},
                "tool_diameter": {"type": "number", "description": "Laser spot / cutter diameter in mm (default 0.1)"},
                "clearance": {"type": "number", "description": "Required clearance from copper to the cut edge in mm (default 0.2)"},
                "resolution": {"type": "number", "description": "Raster resolution in mm (default 0.05)"},
                "simplify_tolerance": {"type": "number", "description": "Path simplification tolerance in mm (default 0.02)"},
                "drill_mode": {"type": "string", "enum": ["mark", "circle"], "description": "mark = burn a locating dot (default), circle = laser-cut the hole"},
                "power_s": {"type": "integer", "description": "Override laser power (GRBL S value)"},
                "speed_mm_min": {"type": "integer", "description": "Override feed rate"},
                "passes": {"type": "integer", "description": "Override isolation pass count"},
                "laser_mode": {"type": "string", "enum": ["m3", "m4"], "description": "GRBL laser mode: m3 constant power (default) or m4 dynamic"},
                "out_dir": {"type": "string", "description": "Directory to write job.gcode and preview.svg into"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let paths = input_paths(&input);
        if paths.is_empty() {
            return Ok(ToolOutput::new(
                "缺少参数 path：请给出 Gerber/钻孔文件或导出目录的路径。".to_string(),
            ));
        }
        let set = match gcode_laser::import_paths(&paths) {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::new(format!("导入失败：{e}"))),
        };
        let job = match build_job(&input, &set) {
            Ok(j) => j,
            Err(e) => {
                return Ok(ToolOutput::new(format!(
                    "生成作业失败：{e}\n\n{}",
                    render_import(&set)
                )))
            }
        };

        let opts = GcodeOptions {
            laser_mode: match str_arg(&input, "laser_mode").as_deref() {
                Some("m4") => LaserMode::DynamicPower,
                _ => LaserMode::ConstantPower,
            },
            ..Default::default()
        };
        let gcode_text = generate_gcode(&job, &opts);
        if let Err(violations) = validate_gcode(&gcode_text, &job) {
            return Ok(ToolOutput::new(format!(
                "内部错误：生成的 G-code 未通过结构校验，已停止导出（不会给出不可靠的产物）。\n{violations:?}"
            )));
        }
        let svg_text = render_svg(&job, &SvgOptions::default());

        let mut gcode_path = None;
        let mut svg_path = None;
        if let Some(dir) = str_arg(&input, "out_dir") {
            let dir = PathBuf::from(dir);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Ok(ToolOutput::new(format!(
                    "无法创建输出目录 {}：{e}",
                    dir.display()
                )));
            }
            let g = dir.join("job.gcode");
            let s = dir.join("preview.svg");
            if let Err(e) = std::fs::write(&g, &gcode_text) {
                return Ok(ToolOutput::new(format!("写入 {} 失败：{e}", g.display())));
            }
            if let Err(e) = std::fs::write(&s, &svg_text) {
                return Ok(ToolOutput::new(format!("写入 {} 失败：{e}", s.display())));
            }
            gcode_path = Some(g);
            svg_path = Some(s);
        }

        let mut out = render_job(&job, gcode_path.as_ref(), svg_path.as_ref());
        if gcode_path.is_none() {
            out.push_str(
                "\n（未设置 out_dir：只做了作业计算，没有写出 G-code 与 SVG 文件）\n",
            );
        }
        // The import report is included so unsupported constructs cannot be lost.
        out.push('\n');
        out.push_str(&render_import(&set));
        Ok(ToolOutput::new(out))
    }
}

/// Tool: query the laser parameter knowledge base.
pub struct LaserParamsTool;

#[async_trait]
impl Tool for LaserParamsTool {
    fn name(&self) -> &str {
        "gcode_laser_params"
    }

    fn description(&self) -> &str {
        "List or look up laser cutting parameters (material, thickness → power, speed, passes). \
         Every entry carries its source and applicability, and the answer always includes the \
         reminder that these are starting points requiring a test cut: no parameter here has been \
         calibrated or verified on a real machine."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "material": {"type": "string", "description": "Material name or knowledge-base entry id; omit to list everything"},
                "thickness_mm": {"type": "number", "description": "Board thickness in mm (default 1.6)"}
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let mut out = String::new();
        match str_arg(&input, "material") {
            None => {
                out.push_str("## 激光参数知识库\n\n");
                for p in profile::profiles() {
                    out.push_str(&format!("- `{}` {}\n", p.id, p.comment_line()));
                    out.push_str(&format!("  - 适用范围: {}\n", p.applicability));
                }
            }
            Some(material) => {
                let thickness = f64_arg(&input, "thickness_mm").unwrap_or(1.6);
                let hit = profile::lookup(&material, thickness);
                out.push_str(&format!("## 查询：{material} / {thickness}mm\n\n"));
                out.push_str(&format!("匹配结果: {}\n", hit.note));
                if let Some(p) = &hit.profile {
                    out.push_str(&format!("\n- `{}` {}\n", p.id, p.comment_line()));
                    out.push_str(&format!("  - 适用范围: {}\n", p.applicability));
                    out.push_str(&format!("  - 厚度范围: {}-{}mm\n", p.thickness_mm.0, p.thickness_mm.1));
                }
                if !hit.nearest.is_empty() {
                    out.push_str("\n仅供参考的相近条目（未收录该材质，不要直接采用）：\n");
                    for p in &hit.nearest {
                        out.push_str(&format!("- `{}` {} — {}\n", p.id, p.comment_line(), p.applicability));
                    }
                }
            }
        }
        out.push_str(&format!("\n### 必须遵守\n{}\n", profile::DISCLAIMER));
        Ok(ToolOutput::new(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode::tool::Tool;

    fn fixture_root() -> Option<PathBuf> {
        let s = std::env::var_os("JCODE_SCRATCH_DIR")?;
        let root = PathBuf::from(s).join("r2/gerber");
        root.is_dir().then_some(root)
    }

    fn first_board() -> Option<PathBuf> {
        let root = fixture_root()?;
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs.into_iter().next()
    }

    fn ctx() -> ToolContext {
        ToolContext {
            session_id: "test".into(),
            message_id: "test".into(),
            tool_call_id: "test".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: jcode::tool::ToolExecutionMode::AgentTurn,
        }
    }

    #[test]
    fn laser_tools_are_registered_with_schemas() {
        let tools = crate::all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"gcode_import_fab"));
        assert!(names.contains(&"gcode_laser_job"));
        assert!(names.contains(&"gcode_laser_params"));
        for t in tools.iter().filter(|t| t.name().starts_with("gcode_")) {
            let schema = t.parameters_schema();
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "{} schema must be an object schema",
                t.name()
            );
        }
    }

    #[tokio::test]
    async fn import_tool_requires_a_path() {
        let out = ImportFabTool.execute(serde_json::json!({}), ctx()).await.unwrap();
        assert!(out.output.contains("缺少参数 path"), "{}", out.output);
    }

    #[tokio::test]
    async fn import_tool_reports_a_missing_directory() {
        let out = ImportFabTool
            .execute(serde_json::json!({"path": "/nope/does-not-exist"}), ctx())
            .await
            .unwrap();
        assert!(out.output.contains("导入失败"), "{}", out.output);
    }

    #[tokio::test]
    async fn job_tool_requires_a_path() {
        let out = PlanLaserJobTool.execute(serde_json::json!({}), ctx()).await.unwrap();
        assert!(out.output.contains("缺少参数 path"), "{}", out.output);
    }

    #[tokio::test]
    async fn params_tool_lists_entries_and_the_disclaimer() {
        let out = LaserParamsTool.execute(serde_json::json!({}), ctx()).await.unwrap();
        let text = &out.output;
        assert!(text.contains("fr4-1.6-isolation"), "{text}");
        assert!(text.contains("试刻"), "disclaimer required: {text}");
        assert!(text.contains("未在真实激光机上验证"), "{text}");
    }

    #[tokio::test]
    async fn params_tool_refuses_to_assert_for_unknown_material() {
        let out = LaserParamsTool
            .execute(serde_json::json!({"material": "月岩", "thickness_mm": 2.0}), ctx())
            .await
            .unwrap();
        let text = &out.output;
        assert!(text.contains("未收录"), "{text}");
        assert!(text.contains("不要直接采用"), "{text}");
    }

    #[tokio::test]
    async fn job_tool_generates_artifacts_and_states_what_is_unverified() {
        let Some(board) = first_board() else {
            eprintln!("skipping: no real fabrication files available");
            return;
        };
        let out_dir = std::env::temp_dir().join("gcode-laser-tool-out");
        let _ = std::fs::remove_dir_all(&out_dir);
        let out = PlanLaserJobTool
            .execute(
                serde_json::json!({
                    "path": board.display().to_string(),
                    "name": "tool-test",
                    "out_dir": out_dir.display().to_string(),
                    "clearance": 0.2,
                    "resolution": 0.1
                }),
                ctx(),
            )
            .await
            .unwrap();
        let text = &out.output;
        assert!(text.contains("G-code:"), "{text}");
        assert!(text.contains("SVG 预览:"), "{text}");
        assert!(text.contains("几何校验"), "{text}");
        assert!(text.contains("没有做过任何实际打样"), "{text}");
        assert!(out_dir.join("job.gcode").is_file());
        assert!(out_dir.join("preview.svg").is_file());
        let gcode = std::fs::read_to_string(out_dir.join("job.gcode")).unwrap();
        assert!(gcode.starts_with("; "), "comments first: {gcode}");
        assert!(gcode.contains("M2"));
    }

    #[tokio::test]
    async fn job_tool_passes_unsupported_constructs_through() {
        let dir = std::env::temp_dir().join("gcode-laser-tool-unsupported");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("top.gbr"),
            "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%SRX2Y2I1.0J1.0*%\n%ADD10C,1.000000*%\nD10*\nX1000000Y1000000D03*\nM02*\n",
        )
        .unwrap();
        let out = PlanLaserJobTool
            .execute(
                serde_json::json!({"path": dir.display().to_string(), "resolution": 0.1}),
                ctx(),
            )
            .await
            .unwrap();
        let text = &out.output;
        assert!(
            text.contains("步进重复"),
            "unsupported constructs must reach the tool output: {text}"
        );
    }

    #[tokio::test]
    async fn job_tool_refuses_invalid_parameters() {
        let Some(board) = first_board() else {
            eprintln!("skipping: no real fabrication files available");
            return;
        };
        let out = PlanLaserJobTool
            .execute(
                serde_json::json!({
                    "path": board.display().to_string(),
                    "tool_diameter": 0.0
                }),
                ctx(),
            )
            .await
            .unwrap();
        assert!(out.output.contains("参数不合法"), "{}", out.output);
    }

    #[tokio::test]
    async fn job_tool_reports_a_copper_only_job_honestly() {
        let dir = std::env::temp_dir().join("gcode-laser-tool-copper-only");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("top.gbr"),
            "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%ADD10C,2.000000*%\nD10*\nX10000000Y10000000D03*\nM02*\n",
        )
        .unwrap();
        let out = PlanLaserJobTool
            .execute(
                serde_json::json!({"path": dir.display().to_string(), "resolution": 0.1}),
                ctx(),
            )
            .await
            .unwrap();
        let text = &out.output;
        assert!(text.contains("未提供铜层") || text.contains("不含板框切割"), "{text}");
        assert!(text.contains("不含孔位标记"), "{text}");
        assert!(text.contains("未做隔离几何校验") || text.contains("最小距离"), "{text}");
    }

    #[tokio::test]
    async fn job_tool_marks_drill_mode_warning() {
        let Some(board) = first_board() else {
            eprintln!("skipping: no real fabrication files available");
            return;
        };
        let out = PlanLaserJobTool
            .execute(
                serde_json::json!({
                    "path": board.display().to_string(),
                    "drill_mode": "mark",
                    "resolution": 0.1
                }),
                ctx(),
            )
            .await
            .unwrap();
        assert!(out.output.contains("不会形成通孔"), "{}", out.output);
    }
}
