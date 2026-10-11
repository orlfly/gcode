//! gcode-tools: Gcode circuit tools exposed as jcode `Tool`s.
//!
//! The agent never writes KiCad s-expressions itself. It describes a circuit as
//! netlist IR and calls these tools, which own all parsing, generation, checking
//! and diffing:
//!
//! - `gcode_generate_schematic` — NetlistIr JSON → `.kicad_sch`
//! - `gcode_run_erc` — run ERC, explain violations in plain language
//! - `gcode_read_schematic` — read an existing `.kicad_sch` back to IR
//! - `gcode_apply_edit` — diff a change, write only after explicit confirmation
//! - `gcode_search_symbols` — component library lookup
//! - `gcode_check_toolchain` — report kicad-cli availability/version

use async_trait::async_trait;
use gcode_kicad::edit::FileEdit;
use gcode_kicad::erc::{interpret_report, run_erc, KicadToolchain, ToolchainError};
use gcode_kicad::reader::read_netlist_file;
use gcode_kicad::schematic::{generate_with_report, Layout};
use gcode_kicad::symbols::SymbolLibrary;
use gcode_kicad::NetlistIr;
use gcode_sim::analysis::Expectation;
use gcode_sim::netlist::Analysis;
use gcode_sim::recipes;
use gcode_sim::runner::Ngspice;
use jcode::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;
use std::path::Path;

fn layout_from(input: &Value) -> Layout {
    let default = Layout::default();
    let num = |key: &str, fallback: f64| -> f64 {
        input
            .get(key)
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
            .unwrap_or(fallback)
    };
    Layout {
        origin_x: num("origin_x", default.origin_x),
        origin_y: num("origin_y", default.origin_y),
        min_gap: num("min_gap", num("pitch_y", default.min_gap)),
    }
}

fn render_report(report: &gcode_kicad::schematic::GenerationReport) -> String {
    let mut out = String::new();
    for w in &report.warnings {
        out.push_str(&format!("\n警告: {w}"));
    }
    if !report.missing_symbols.is_empty() {
        out.push_str(&format!(
            "\n缺失符号（已回退通用符号，ERC 结果不可信）: {}",
            report.missing_symbols.join(", ")
        ));
    }
    out
}

/// Tool: generate a KiCad schematic from a netlist IR.
pub struct GenerateSchematicTool;

#[async_trait]
impl Tool for GenerateSchematicTool {
    fn name(&self) -> &str {
        "gcode_generate_schematic"
    }

    fn description(&self) -> &str {
        "Generate a KiCad .kicad_sch from a netlist IR (components + nets JSON). \
         Deterministic. Never write s-expressions yourself: describe the circuit as JSON. \
         Connectivity is realised with labels on each pin, and real KiCad symbol definitions \
         are embedded so ERC can check the design."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "netlist"],
            "properties": {
                "path": {"type": "string", "description": "Output .kicad_sch file path"},
                "netlist": {
                    "type": "object",
                    "description": "Netlist IR: {components: [{reference, lib_id, value, footprint?}], nets: [{name, connections: [[ref, pin], ...]}]}",
                    "properties": {
                        "components": {"type": "array"},
                        "nets": {"type": "array"}
                    }
                },
                "origin_x": {"type": "number", "description": "Layout anchor X in mm (default 76.2)"},
                "origin_y": {"type": "number", "description": "Layout anchor Y in mm (default 50.8)"},
                "min_gap": {"type": "number", "description": "Minimum vertical gap between symbols in mm (default 10.16)"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let netlist: NetlistIr =
            serde_json::from_value(input.get("netlist").cloned().unwrap_or(Value::Null))
                .map_err(|e| anyhow::anyhow!("netlist JSON invalid: {e}"))?;

        // Validation happens before any file is touched.
        let report = match generate_with_report(&netlist, layout_from(&input)) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput::new(format!(
                    "网表校验失败，未写入任何文件: {e}\n请修正网表后重试。"
                )))
            }
        };

        let abs = ctx.resolve_path(Path::new(path));
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&abs, &report.content).await?;

        Ok(ToolOutput::new(format!(
            "已写入 {path}: {} 个元件, {} 个网络。{}",
            netlist.components.len(),
            netlist.nets.len(),
            render_report(&report)
        )))
    }
}

/// Tool: run ERC and explain the result.
pub struct RunErcTool;

#[async_trait]
impl Tool for RunErcTool {
    fn name(&self) -> &str {
        "gcode_run_erc"
    }

    fn description(&self) -> &str {
        "Run KiCad ERC on a .kicad_sch via kicad-cli and return a PASS/FAIL verdict with each \
         violation explained in plain language plus an actionable fix. Requires a KiCad version \
         at or above the configured minimum (default 10); otherwise it explains what is missing."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {"type": "string", "description": "Path to the .kicad_sch file"},
                "json": {"type": "boolean", "description": "Return the machine-readable summary instead of prose"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let abs = ctx.resolve_path(Path::new(path));

        // Gate on the toolchain before doing anything.
        match KicadToolchain::probe().await {
            Ok(tc) => {
                if std::env::var_os("GCODE_VERBOSE").is_some() {
                    eprintln!("gcode: using kicad-cli {} ({:?})", tc.version, tc.path);
                }
            }
            Err(e) => {
                return Ok(ToolOutput::new(format!(
                    "无法运行 ERC：{e}\n原理图已生成，但电气检查未执行。"
                )))
            }
        }

        match run_erc(&abs).await {
            Ok(report) => {
                let want_json = input.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
                if want_json {
                    Ok(ToolOutput::new(report.to_json().to_string()))
                } else {
                    let interp = interpret_report(&report);
                    Ok(ToolOutput::new(format!(
                        "{}\n{}",
                        report.summary(),
                        interp.render()
                    )))
                }
            }
            Err(e) => Ok(ToolOutput::new(format!("ERC 执行失败：{e}"))),
        }
    }
}

/// Tool: read an existing schematic back into IR.
pub struct ReadSchematicTool;

#[async_trait]
impl Tool for ReadSchematicTool {
    fn name(&self) -> &str {
        "gcode_read_schematic"
    }

    fn description(&self) -> &str {
        "Read an existing .kicad_sch back into netlist IR (components and nets). Use this before \
         editing a design so your change is based on what the file actually contains."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {"type": "string", "description": "Path to the .kicad_sch file"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let abs = ctx.resolve_path(Path::new(path));
        match read_netlist_file(&abs) {
            Ok(ir) => Ok(ToolOutput::new(
                serde_json::to_string_pretty(&ir).unwrap_or_else(|_| "{}".into()),
            )),
            Err(e) => Ok(ToolOutput::new(format!("读取失败：{e}"))),
        }
    }
}

/// Tool: stage and apply an edit to an existing schematic.
pub struct ApplyEditTool;

#[async_trait]
impl Tool for ApplyEditTool {
    fn name(&self) -> &str {
        "gcode_apply_edit"
    }

    fn description(&self) -> &str {
        "Modify an existing .kicad_sch from a new netlist IR. Call once with confirm=false to get \
         the diff; show it to the user, then call again with confirm=true to write it. Never writes \
         without confirmation."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "netlist"],
            "properties": {
                "path": {"type": "string", "description": "Existing .kicad_sch file to edit"},
                "netlist": {"type": "object", "description": "The desired netlist IR after the edit"},
                "confirm": {"type": "boolean", "description": "Write the change; false only reports the diff (default false)"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let confirm = input
            .get("confirm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let netlist: NetlistIr =
            serde_json::from_value(input.get("netlist").cloned().unwrap_or(Value::Null))
                .map_err(|e| anyhow::anyhow!("netlist JSON invalid: {e}"))?;

        let abs = ctx.resolve_path(Path::new(path));
        let mut edit = match FileEdit::stage_from_file(&abs, &netlist, layout_from(&input)) {
            Ok(e) => e,
            Err(e) => return Ok(ToolOutput::new(format!("无法准备修改：{e}"))),
        };

        if edit.is_empty() {
            return Ok(ToolOutput::new("现有原理图与新网表一致，无需修改。".to_string()));
        }

        let diff = edit.render();
        if !confirm {
            return Ok(ToolOutput::new(format!(
                "{diff}\n以上改动尚未写入。请向用户确认后，用 confirm=true 再次调用以写回。"
            )));
        }
        match edit.confirm() {
            Ok(()) => Ok(ToolOutput::new(format!(
                "已写入 {path}。\n{diff}\n提示：建议随后调用 gcode_run_erc 重新做电气检查。"
            ))),
            Err(e) => Ok(ToolOutput::new(format!("写入失败：{e}"))),
        }
    }
}

/// Tool: component library lookup.
pub struct SearchSymbolsTool;

#[async_trait]
impl Tool for SearchSymbolsTool {
    fn name(&self) -> &str {
        "gcode_search_symbols"
    }

    fn description(&self) -> &str {
        "Search the local KiCad symbol library for symbols matching a name fragment \
         (e.g. 'lm358', '7805', 'opamp'). Returns full lib_id values usable in the netlist."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {"type": "string", "description": "Case-insensitive name fragment"},
                "limit": {"type": "integer", "description": "Maximum results (default 40)"}
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'query'"))?;
        let limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(40) as usize;
        let mut lib = SymbolLibrary::locate();
        if lib.dir().is_none() {
            return Ok(ToolOutput::new(
                "未找到 KiCad 符号库（可用 GCODE_KICAD_SYMBOL_DIR 指定目录）。".to_string(),
            ));
        }
        let hits = lib.search(query);
        if hits.is_empty() {
            return Ok(ToolOutput::new(format!("没有匹配 {query:?} 的符号。")));
        }
        let shown: Vec<&String> = hits.iter().take(limit).collect();
        Ok(ToolOutput::new(format!(
            "匹配 {} 个（显示 {}）：\n{}",
            hits.len(),
            shown.len(),
            shown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        )))
    }
}

/// Tool: toolchain availability/version report.
pub struct CheckToolchainTool;

#[async_trait]
impl Tool for CheckToolchainTool {
    fn name(&self) -> &str {
        "gcode_check_toolchain"
    }

    fn description(&self) -> &str {
        "Report whether kicad-cli is available, its version, the symbol-library path, and whether \
         ERC is allowed by the configured minimum version."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let lib_dir = SymbolLibrary::locate()
            .dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "未找到".to_string());
        let mut out = format!("符号库目录：{lib_dir}\n");
        match KicadToolchain::probe().await {
            Ok(tc) => out.push_str(&format!(
                "kicad-cli：{}（{}），ERC 可用。\n",
                tc.version,
                tc.path.display()
            )),
            Err(ToolchainError::NotFound) => out.push_str(
                "kicad-cli：未安装。生成原理图可用，但 ERC 不可用（安装 KiCad 或设置 GCODE_KICAD_CLI）。\n",
            ),
            Err(e @ ToolchainError::TooOld { .. }) => out.push_str(&format!(
                "{e}\n生成原理图可用；升级后即可运行 ERC（或用 GCODE_KICAD_MIN_VERSION 覆盖最低版本）。\n"
            )),
            Err(e) => out.push_str(&format!("kicad-cli：{e}\n")),
        }
        Ok(ToolOutput::new(out))
    }
}

/// Tool: list the simulation recipes available for the catalog.
pub struct ListSimRecipesTool;

#[async_trait]
impl Tool for ListSimRecipesTool {
    fn name(&self) -> &str {
        "gcode_list_sim_recipes"
    }

    fn description(&self) -> &str {
        "List the simulation recipes (analysis type, probes, analytically derived expectations and \
         tolerances) available for the circuit catalog. Use this before running a simulation so you \
         know what can be measured and what the expected values are."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "circuit_id": {"type": "string", "description": "Optional: only show recipes for this circuit"}
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let filter = input.get("circuit_id").and_then(|v| v.as_str());
        let recipes: Vec<recipes::SimRecipe> = match filter {
            Some(id) => recipes::by_circuit(id),
            None => recipes::all(),
        };
        if recipes.is_empty() {
            return Ok(ToolOutput::new(format!(
                "没有电路 {filter:?} 的仿真配方。可仿真的电路：{}",
                recipes::covered_circuits().join(", ")
            )));
        }
        let mut out = format!("共 {} 个仿真配方：\n", recipes.len());
        for r in &recipes {
            out.push_str(&format!(
                "\n- {} / {}（{}）\n  测量节点：{}\n  依据：{}",
                r.circuit_id,
                r.name,
                r.analysis.kind.tag(),
                r.output,
                r.basis
            ));
            for e in &r.expectations {
                out.push_str(&format!(
                    "\n  · {} 预期 {} {}（±{}%）：{}",
                    e.metric, e.expected, e.unit, e.tol_pct, e.rationale
                ));
            }
        }
        Ok(ToolOutput::new(out))
    }
}

/// Tool: run a simulation and report the verdict.
pub struct RunSimulationTool;

#[async_trait]
impl Tool for RunSimulationTool {
    fn name(&self) -> &str {
        "gcode_run_simulation"
    }

    fn description(&self) -> &str {
        "Run a SPICE simulation with ngspice and return measured metrics compared against \
         expectations, plus the paths of the waveform CSV and SVG plot. Either pass a circuit_id to \
         use a catalog recipe, or pass netlist (IR JSON) together with an explicit analysis. Active \
         devices use behavioural macromodels, so results verify topology and magnitude, not \
         device-level precision."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "circuit_id": {"type": "string", "description": "Catalog circuit to simulate using its recipe"},
                "recipe": {"type": "string", "description": "Recipe name when a circuit has several (e.g. 'on state')"},
                "netlist": {"type": "object", "description": "Netlist IR to simulate when circuit_id is omitted"},
                "analysis": {"type": "object", "description": "Analysis config when netlist is given: {kind, points, start_hz, stop_hz, step_s, stop_s, stimuli, probes}"},
                "expectations": {"type": "array", "description": "Optional [{metric, expected, tol_pct, unit, rationale}] to judge against"},
                "out_dir": {"type": "string", "description": "Directory for artifacts (defaults to a temp dir)"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        simulate(input, ctx, None).await
    }
}

/// Tool: report whether simulation is available.
pub struct CheckSimToolchainTool;

#[async_trait]
impl Tool for CheckSimToolchainTool {
    fn name(&self) -> &str {
        "gcode_check_sim_toolchain"
    }

    fn description(&self) -> &str {
        "Report whether ngspice is available for simulation, its version, and which device models \
         the simulator knows. Schematic generation and ERC do not depend on this."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let mut out = format!(
            "可仿真的器件模型：{}\n",
            gcode_sim::models::supported_lib_ids().join(", ")
        );
        match Ngspice::probe().await {
            Ok(n) => out.push_str(&format!(
                "ngspice：{}（{}），仿真可用。\n",
                n.version,
                n.path.display()
            )),
            Err(e) => out.push_str(&format!(
                "ngspice：不可用 —— {e}\n仿真工具会给出提示；生成与 ERC 不受影响。\n"
            )),
        }
        Ok(ToolOutput::new(out))
    }
}

/// Shared implementation for the simulation tool.
///
/// `ngspice_path` exists so tests can inject a bad binary without mutating
/// process-global environment state.
async fn simulate(
    input: Value,
    ctx: ToolContext,
    ngspice_path: Option<&str>,
) -> anyhow::Result<ToolOutput> {
    let ngspice = match ngspice_path {
        Some(p) => match Ngspice::probe_at(Path::new(p)).await {
            Ok(n) => n,
            Err(e) => {
                return Ok(ToolOutput::new(format!(
                    "无法仿真：{e}\n（原理图生成与 ERC 仍可正常使用。）"
                )))
            }
        },
        None => match Ngspice::probe().await {
            Ok(n) => n,
            Err(e) => {
                return Ok(ToolOutput::new(format!(
                    "无法仿真：{e}\n（原理图生成与 ERC 仍可正常使用。）"
                )))
            }
        },
    };

    let out_dir = input
        .get("out_dir")
        .and_then(|v| v.as_str())
        .map(|s| ctx.resolve_path(Path::new(s)))
        .unwrap_or_else(|| std::env::temp_dir().join("gcode-sim-runs"));

    // Path A: a catalog recipe.
    if let Some(id) = input.get("circuit_id").and_then(|v| v.as_str()) {
        let Some(entry) = gcode_kicad::catalog::by_id(id) else {
            return Ok(ToolOutput::new(format!(
                "未知电路 {id:?}。可仿真的电路：{}",
                recipes::covered_circuits().join(", ")
            )));
        };
        let mut candidates = recipes::by_circuit(id);
        if let Some(name) = input.get("recipe").and_then(|v| v.as_str()) {
            candidates.retain(|r| r.name == name);
        }
        if candidates.is_empty() {
            return Ok(ToolOutput::new(format!(
                "电路 {id} 没有匹配的仿真配方。可用配方：{}",
                recipes::by_circuit(id)
                    .iter()
                    .map(|r| r.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let mut sections = Vec::new();
        for recipe in candidates {
            let dir = out_dir.join(format!(
                "{}.{}",
                recipe.circuit_id,
                recipe.name.replace(' ', "_")
            ));
            match recipes::run_recipe(&ngspice, &recipe, &entry.ir, &dir, true).await {
                Ok(o) => sections.push(render_outcome(&o, &recipe)),
                Err(e) => sections.push(format!("{} [{}] 运行失败：{e}", id, recipe.name)),
            }
        }
        return Ok(ToolOutput::new(sections.join("\n\n")));
    }

    // Path B: an explicit netlist + analysis.
    let Some(netlist) = input.get("netlist") else {
        return Ok(ToolOutput::new(
            "需要 circuit_id 或 netlist 之一。用 gcode_list_sim_recipes 查看可用的仿真配方。"
                .to_string(),
        ));
    };
    let ir: NetlistIr = serde_json::from_value(netlist.clone())
        .map_err(|e| anyhow::anyhow!("netlist JSON invalid: {e}"))?;
    let Some(analysis_value) = input.get("analysis") else {
        return Ok(ToolOutput::new(
            "提供 netlist 时必须同时提供 analysis（分析类型、激励与探针）。".to_string(),
        ));
    };
    let analysis: Analysis = serde_json::from_value(analysis_value.clone())
        .map_err(|e| anyhow::anyhow!("analysis JSON invalid: {e}"))?;
    let expectations: Vec<Expectation> = match input.get("expectations") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| anyhow::anyhow!("expectations JSON invalid: {e}"))?,
        None => Vec::new(),
    };

    let tag = analysis.kind.tag();
    let opts = gcode_sim::netlist::DeckOptions::new("gcode simulation", "waveform.dat");
    let deck = match gcode_sim::netlist::generate(&ir, &analysis, &opts) {
        Ok(d) => d,
        Err(e) => return Ok(ToolOutput::new(format!("无法生成仿真网表：{e}"))),
    };
    let dir = out_dir.join(format!("adhoc.{tag}"));
    let waveform = match ngspice.run(&dir, &deck.text, "waveform.dat").await {
        Ok(w) => w,
        Err(e) => return Ok(ToolOutput::new(format!("仿真失败：{e}"))),
    };

    let column = |net: &str| -> String {
        let node = deck
            .node_map
            .get(net)
            .cloned()
            .unwrap_or_else(|| gcode_sim::netlist::normalize_node(net));
        match analysis.kind {
            gcode_sim::AnalysisKind::Ac { .. } => format!("vm({node})"),
            _ => format!("v({node})"),
        }
    };
    let probe_net = analysis.probes.first().cloned().unwrap_or_default();
    let out_col = column(&probe_net);
    let in_col = analysis
        .stimuli
        .first()
        .map(|s| column(s.net()))
        .unwrap_or_else(|| out_col.clone());

    let metric_set = match analysis.kind {
        gcode_sim::AnalysisKind::Op => gcode_sim::MetricSet::Dc,
        gcode_sim::AnalysisKind::Ac { .. } => gcode_sim::MetricSet::Ac {
            shape: gcode_sim::ResponseShape::Lowpass,
        },
        gcode_sim::AnalysisKind::Tran { .. } => gcode_sim::MetricSet::Tran,
    };
    let measured = gcode_sim::extract_metrics(metric_set, &waveform, &out_col, &in_col);

    let mut out = format!("仿真完成（{}，探针 {}）。\n实测指标：\n", tag, probe_net);
    for (k, v) in &measured {
        out.push_str(&format!("- {k} = {v}\n"));
    }
    if !expectations.is_empty() {
        let verdict = gcode_sim::evaluate(&measured, &expectations);
        out.push_str(&format!("\n{}", verdict.render()));
    } else {
        out.push_str("\n（未提供预期值，因此不做 PASS/FAIL 判定。）\n");
    }

    // Artifacts.
    let (csv_name, svg_name) = gcode_sim::artifact_names("adhoc", tag);
    let art_dir = dir.join("artifacts");
    if gcode_sim::artifacts::write_waveform_csv(&art_dir.join(&csv_name), &waveform).is_ok() {
        out.push_str(&format!("\n波形数据：{}\n", art_dir.join(&csv_name).display()));
    }
    let svg = match analysis.kind {
        gcode_sim::AnalysisKind::Ac { .. } => {
            gcode_sim::bode_svg(&waveform, &out_col, &in_col, "gcode simulation")
        }
        _ => gcode_sim::series_svg(&waveform, &out_col, "gcode simulation", false),
    };
    if let Some(svg) = svg {
        let p = art_dir.join(&svg_name);
        if std::fs::create_dir_all(&art_dir).is_ok() && std::fs::write(&p, svg).is_ok() {
            out.push_str(&format!("图形：{}\n", p.display()));
        }
    }
    if !deck.behavioral_devices.is_empty() {
        out.push_str("\n本次使用行为级模型：\n");
        for d in &deck.behavioral_devices {
            out.push_str(&format!("- {d}\n"));
        }
    }
    Ok(ToolOutput::new(out))
}

fn render_outcome(o: &recipes::SimOutcome, recipe: &recipes::SimRecipe) -> String {
    let mut out = format!(
        "电路 {} / {}（{}）\n{}\n依据：{}",
        o.circuit_id,
        o.recipe,
        o.analysis_tag,
        o.verdict.render(),
        recipe.basis
    );
    if !o.artifacts.is_empty() {
        out.push_str("\n产物：");
        for p in &o.artifacts {
            out.push_str(&format!("\n- {}", p.display()));
        }
    }
    if !o.behavioral_devices.is_empty() {
        out.push_str("\n行为级模型：");
        for d in &o.behavioral_devices {
            out.push_str(&format!("\n- {d}"));
        }
    }
    out
}

/// Tool: list measurement instruments and their capabilities.
pub struct ListInstrumentsTool;

#[async_trait]
impl Tool for ListInstrumentsTool {
    fn name(&self) -> &str {
        "gcode_list_instruments"
    }

    fn description(&self) -> &str {
        "List the measurement backends available on this machine (virtual bench, sigrok-cli, serial \
         programmable supply), each with a reason when it is unavailable, plus what quantities each \
         can measure. Run this before planning a test."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let mut out = String::from("测量后端：\n");
        for s in gcode_instrument::probe_backends().await {
            out.push_str(&format!("- {}\n", s.describe()));
        }
        out.push_str("\n安全约束（硬性）：电压 ≤ 48V DC、电流 ≤ 5A；上电需要用户当轮确认；\n");
        out.push_str(&format!(
            "上电通道默认关闭（需 {} =1），且读到超过预期 2 倍的电流会立即断电。\n",
            gcode_instrument::hal::ALLOW_SOURCE_ENV
        ));
        match gcode_instrument::hal::source_enabled() {
            true => out.push_str("当前会话：允许上电。\n"),
            false => out.push_str("当前会话：不允许上电（带电步骤会被拒绝）。\n"),
        }
        Ok(ToolOutput::new(out))
    }
}

/// Tool: build a measurement test plan.
pub struct PlanTestTool;

#[async_trait]
impl Tool for PlanTestTool {
    fn name(&self) -> &str {
        "gcode_plan_test"
    }

    fn description(&self) -> &str {
        "Build a measurement test plan. Either derive it from a catalog circuit's simulation recipe \
         (circuit_id) so the expected values are simulation baselines, or supply your own steps with \
         explicit expected values. A plan with steps that have no expected value is refused: pass \
         criteria must never be invented."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "circuit_id": {"type": "string", "description": "Catalog circuit to derive the plan from"},
                "recipe": {"type": "string", "description": "Recipe name when the circuit has several"},
                "plan_id": {"type": "string", "description": "Plan id (default derived from the circuit)"},
                "steps": {
                    "type": "array",
                    "description": "Explicit steps: [{target, quantity, expected_value, tol_pct, unit, rationale, powered_voltage?, current_limit?}]"
                },
                "out_dir": {"type": "string", "description": "Where to write the plan JSON"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let plan = match build_plan(&input) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::new(format!("无法生成测试计划：{e}"))),
        };
        let json = match plan.to_json() {
            Ok(j) => j,
            Err(e) => return Ok(ToolOutput::new(format!("计划无法序列化：{e}"))),
        };

        let mut out = format!(
            "测试计划 {}：{} 个步骤（其中 {} 个需要上电）\n",
            plan.plan_id,
            plan.steps.len(),
            plan.powered_steps().len()
        );
        for s in &plan.steps {
            out.push_str(&format!(
                "\n- 步骤 {} · {}：{} {}（±{}%），预期依据 {}，{}",
                s.index,
                s.target,
                s.quantity.label_zh(),
                s.expected.value,
                s.expected.tol_pct,
                s.expected.source.describe(),
                if s.needs_power() { "需要上电" } else { "无需上电" }
            ));
        }
        for n in &plan.notes {
            out.push_str(&format!("\n说明：{n}"));
        }
        out.push_str(&format!("\n\n计划 JSON：\n{json}"));

        if let Some(dir) = input.get("out_dir").and_then(|v| v.as_str()) {
            let abs = ctx.resolve_path(Path::new(dir));
            let path = abs.join(format!("{}.plan.json", plan.plan_id));
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            if tokio::fs::write(&path, &json).await.is_ok() {
                out.push_str(&format!("\n\n已写入 {}", path.display()));
            }
        }
        Ok(ToolOutput::new(out))
    }
}

/// Tool: execute a test plan.
pub struct RunTestTool;

#[async_trait]
impl Tool for RunTestTool {
    fn name(&self) -> &str {
        "gcode_run_test"
    }

    fn description(&self) -> &str {
        "Execute a measurement test plan and return a PASS/FAIL report with deviations and \
         attribution. Steps that energise the board require explicit user confirmation \
         (user_confirmed=true); without it they are refused and recorded, never run. Measured \
         values always come from an instrument: never report a simulated or estimated number as a \
         measurement."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plan": {"type": "object", "description": "The test plan (from gcode_plan_test)"},
                "plan_path": {"type": "string", "description": "Or a path to a plan JSON file"},
                "user_confirmed": {"type": "boolean", "description": "True only when the user has approved energising the board for this run"},
                "meter": {"type": "string", "description": "Which instrument to measure with: auto | sigrok | virtual"},
                "virtual_readings": {"type": "object", "description": "For meter=virtual only: quantity -> value (e.g. {\"VoltageDc\": 2.449, \"CurrentDc\": 0.01})"},
                "virtual_error_pct": {"type": "number", "description": "For meter=virtual: inject a systematic error percentage"},
                "dc_tol_pct": {"type": "number", "description": "Override the KR2 DC tolerance (default 10%)"},
                "ac_freq_tol_pct": {"type": "number", "description": "Override the KR2 AC-frequency tolerance (default 20%)"},
                "out_dir": {"type": "string", "description": "Where to write the report"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        run_test(input, ctx, None).await
    }
}

fn build_plan(input: &Value) -> Result<gcode_instrument::TestPlan, String> {
    if let Some(id) = input.get("circuit_id").and_then(|v| v.as_str()) {
        let recipe = input.get("recipe").and_then(|v| v.as_str());
        return gcode_instrument::TestPlan::from_recipe(id, recipe).map_err(|e| e.to_string());
    }
    let Some(steps) = input.get("steps").and_then(|v| v.as_array()) else {
        return Err("需要 circuit_id 或 steps 之一".into());
    };
    let plan_id = input
        .get("plan_id")
        .and_then(|v| v.as_str())
        .unwrap_or("tp-manual");
    let mut built = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        let target = s
            .get("target")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("步骤 {} 缺少 target", i + 1))?;
        let quantity = match s.get("quantity").and_then(|v| v.as_str()) {
            Some("VoltageDc") => gcode_instrument::Quantity::VoltageDc,
            Some("CurrentDc") => gcode_instrument::Quantity::CurrentDc,
            Some("Resistance") => gcode_instrument::Quantity::Resistance,
            Some("Continuity") => gcode_instrument::Quantity::Continuity,
            Some("Frequency") => gcode_instrument::Quantity::Frequency,
            other => return Err(format!("步骤 {} 的 quantity 无效：{other:?}", i + 1)),
        };
        let expected_value = s
            .get("expected_value")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                format!(
                    "步骤 {}（{target}）没有预期值：预期值必须来自仿真或用户显式输入，不能凭空设定",
                    i + 1
                )
            })?;
        let tol_pct = s.get("tol_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let unit = s
            .get("unit")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| quantity.unit());
        let rationale = s
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("用户显式输入");
        let mut step = gcode_instrument::TestStep::new(
            i + 1,
            target,
            quantity,
            gcode_instrument::Expected::from_user(expected_value, tol_pct, unit, rationale),
        );
        if let Some(v) = s.get("powered_voltage").and_then(|v| v.as_f64()) {
            let limit = s.get("current_limit").and_then(|v| v.as_f64()).unwrap_or(0.2);
            step = step.powered(v, limit);
            if let Some(ec) = s.get("expected_current").and_then(|v| v.as_f64()) {
                step = step.with_expected_current(ec);
            }
        }
        built.push(step);
    }
    gcode_instrument::TestPlan::new(plan_id, built).map_err(|e| e.to_string())
}

/// Shared implementation, with an injectable backend for tests.
async fn run_test(
    input: Value,
    ctx: ToolContext,
    meter_override: Option<Box<dyn gcode_instrument::Instrument>>,
) -> anyhow::Result<ToolOutput> {
    // 1) Obtain the plan.
    let plan = if let Some(p) = input.get("plan") {
        match serde_json::from_value::<gcode_instrument::TestPlan>(p.clone()) {
            Ok(plan) => plan,
            Err(e) => return Ok(ToolOutput::new(format!("计划 JSON 无效：{e}"))),
        }
    } else if let Some(path) = input.get("plan_path").and_then(|v| v.as_str()) {
        let abs = ctx.resolve_path(Path::new(path));
        match tokio::fs::read_to_string(&abs).await {
            Ok(text) => match gcode_instrument::TestPlan::from_json(&text) {
                Ok(plan) => plan,
                Err(e) => return Ok(ToolOutput::new(format!("计划文件无效：{e}"))),
            },
            Err(e) => return Ok(ToolOutput::new(format!("无法读取计划文件：{e}"))),
        }
    } else {
        return Ok(ToolOutput::new(
            "需要 plan（来自 gcode_plan_test）或 plan_path。".to_string(),
        ));
    };
    if let Err(e) = plan.validate() {
        return Ok(ToolOutput::new(format!("计划校验失败：{e}")));
    }

    // 2) Confirmation: refuse loudly before attempting anything powered.
    let user_confirmed = input
        .get("user_confirmed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let token = gcode_instrument::ConfirmToken::mint(user_confirmed);
    let powered = plan.powered_steps().len();
    if powered > 0 && token.is_none() {
        let mut out = format!(
            "该计划有 {powered} 个步骤需要给板子上电，但未获得用户确认，未执行任何带电操作。\n\
             请向用户说明将要施加的电压/限流并取得明确同意，然后以 user_confirmed=true 重试。\n\n"
        );
        for s in plan.powered_steps() {
            let supply = s.supply.as_ref().expect("powered step has a supply");
            out.push_str(&format!(
                "- 步骤 {} · {}：计划施加 {}V / 限流 {}A\n",
                s.index, s.target, supply.voltage, supply.current_limit
            ));
        }
        out.push_str(&format!(
            "\n（提示：本会话还需 {} =1 才允许上电。）\n",
            gcode_instrument::hal::ALLOW_SOURCE_ENV
        ));
        return Ok(ToolOutput::new(out));
    }

    // 3) Select the instrument.
    let meter: Box<dyn gcode_instrument::Instrument> = match meter_override {
        Some(m) => m,
        None => {
            let kind = input
                .get("meter")
                .and_then(|v| v.as_str())
                .unwrap_or("auto");
            match kind {
                "virtual" => {
                    let mut vi = gcode_instrument::instruments::VirtualInstrument::new();
                    if let Some(readings) = input.get("virtual_readings").and_then(|v| v.as_object())
                    {
                        for (k, v) in readings {
                            let q = match k.as_str() {
                                "VoltageDc" => gcode_instrument::Quantity::VoltageDc,
                                "CurrentDc" => gcode_instrument::Quantity::CurrentDc,
                                "Resistance" => gcode_instrument::Quantity::Resistance,
                                "Continuity" => gcode_instrument::Quantity::Continuity,
                                "Frequency" => gcode_instrument::Quantity::Frequency,
                                _ => continue,
                            };
                            if let Some(val) = v.as_f64() {
                                vi.set_reading(q, val);
                            }
                        }
                    }
                    if let Some(e) = input.get("virtual_error_pct").and_then(|v| v.as_f64()) {
                        vi = vi.with_error_percent(e);
                    }
                    Box::new(vi)
                }
                "sigrok" | "auto" => {
                    match gcode_instrument::instruments::SigrokInstrument::probe().await {
                        Ok(s) => Box::new(s),
                        Err(e) => {
                            return Ok(ToolOutput::new(format!(
                                "没有可用的测量仪器：{e}\n\
                                 （可选：接入 sigrok 兼容设备，或用 meter=virtual 并显式给出读数。）"
                            )))
                        }
                    }
                }
                other => return Ok(ToolOutput::new(format!("未知 meter：{other}"))),
            }
        }
    };

    // 4) Execute, with the KR2 policy (optionally overridden).
    let policy = match (
        input.get("dc_tol_pct").and_then(|v| v.as_f64()),
        input.get("ac_freq_tol_pct").and_then(|v| v.as_f64()),
    ) {
        (None, None) => gcode_instrument::TolerancePolicy::kr2_default(),
        (dc, ac) => gcode_instrument::TolerancePolicy::override_with(
            dc.unwrap_or(gcode_instrument::compare::KR2_DC_PCT),
            ac.unwrap_or(gcode_instrument::compare::KR2_AC_FREQ_PCT),
        ),
    };
    let report = gcode_instrument::Executor::new(meter.as_ref(), None)
        .with_policy(policy)
        .run(&plan, token.as_ref())
        .await;

    let mut out = report.render();
    if let Some(dir) = input.get("out_dir").and_then(|v| v.as_str()) {
        let abs = ctx.resolve_path(Path::new(dir));
        match report.write(&abs, &plan.plan_id) {
            Ok((json, txt)) => out.push_str(&format!(
                "\n报告已写入：\n- {}\n- {}\n",
                json.display(),
                txt.display()
            )),
            Err(e) => out.push_str(&format!("\n报告写入失败：{e}\n")),
        }
    }
    Ok(ToolOutput::new(out))
}
/// All Gcode tools, for registration with the jcode tool registry.
pub fn all_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(GenerateSchematicTool),
        Box::new(RunErcTool),
        Box::new(ReadSchematicTool),
        Box::new(ApplyEditTool),
        Box::new(SearchSymbolsTool),
        Box::new(CheckToolchainTool),
        Box::new(ListSimRecipesTool),
        Box::new(RunSimulationTool),
        Box::new(CheckSimToolchainTool),
        Box::new(ListInstrumentsTool),
        Box::new(PlanTestTool),
        Box::new(RunTestTool),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gcode_kicad::catalog;

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

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gcode-tools-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn divider_json() -> Value {
        serde_json::to_value(&catalog::by_id("voltage-divider").unwrap().ir).unwrap()
    }

    #[test]
    fn registers_all_tools_with_unique_names() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names.len(), 12);
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "tool names must be unique");
        assert!(names.contains(&"gcode_generate_schematic"));
        assert!(names.contains(&"gcode_run_erc"));
        assert!(names.contains(&"gcode_apply_edit"));
        assert!(names.contains(&"gcode_run_simulation"));
        assert!(names.contains(&"gcode_list_sim_recipes"));
        assert!(names.contains(&"gcode_check_sim_toolchain"));
        assert!(names.contains(&"gcode_list_instruments"));
        assert!(names.contains(&"gcode_plan_test"));
        assert!(names.contains(&"gcode_run_test"));
    }

    #[tokio::test]
    async fn list_instruments_states_the_hard_limits() {
        let out = ListInstrumentsTool
            .execute(serde_json::json!({}), ctx())
            .await
            .unwrap();
        assert!(out.output.contains("virtual"), "{}", out.output);
        assert!(out.output.contains("sigrok"), "{}", out.output);
        assert!(out.output.contains("serial-psu"), "{}", out.output);
        assert!(out.output.contains("48V"), "{}", out.output);
        assert!(out.output.contains("5A"), "{}", out.output);
    }

    #[tokio::test]
    async fn plan_test_derives_from_a_simulation_recipe() {
        let out = PlanTestTool
            .execute(serde_json::json!({"circuit_id": "voltage-divider"}), ctx())
            .await
            .unwrap();
        assert!(out.output.contains("仿真基准"), "{}", out.output);
        assert!(out.output.contains("需要上电"), "{}", out.output);
        // The plan JSON must be included so it can be handed to gcode_run_test.
        assert!(out.output.contains("\"plan_id\""), "{}", out.output);
    }

    #[tokio::test]
    async fn plan_test_refuses_a_step_without_an_expected_value() {
        let out = PlanTestTool
            .execute(
                serde_json::json!({"steps": [{"target": "3V3 rail", "quantity": "VoltageDc"}]}),
                ctx(),
            )
            .await
            .unwrap();
        assert!(out.output.contains("无法生成测试计划"), "{}", out.output);
        assert!(out.output.contains("不能凭空设定"), "{}", out.output);
    }

    #[tokio::test]
    async fn run_test_refuses_powered_steps_without_confirmation() {
        let plan = PlanTestTool
            .execute(serde_json::json!({"circuit_id": "voltage-divider"}), ctx())
            .await
            .unwrap()
            .output;
        let json_start = plan.find('{').unwrap();
        let json = &plan[json_start..];
        // Trim any trailing prose after the JSON object.
        let json = json.trim_end();
        let value: Value = serde_json::from_str(json).expect("plan JSON parses");

        let out = run_test(
            serde_json::json!({"plan": value, "meter": "virtual"}),
            ctx(),
            None,
        )
        .await
        .unwrap();
        assert!(out.output.contains("未获得用户确认"), "{}", out.output);
        assert!(out.output.contains("未执行任何带电操作"), "{}", out.output);
    }

    #[tokio::test]
    async fn run_test_needs_a_plan() {
        let out = run_test(serde_json::json!({}), ctx(), None).await.unwrap();
        assert!(out.output.contains("需要 plan"), "{}", out.output);
    }

    #[tokio::test]
    async fn run_test_reports_pass_and_fail_for_virtual_readings() {
        // A passive step, so no confirmation is needed.
        let plan = serde_json::json!({
            "plan_id": "tp-tool",
            "steps": [{
                "index": 1, "target": "R1", "quantity": "Resistance",
                "channel": null, "samples": null, "reduce": "Mean", "supply": null,
                "expected": {"value": 220.0, "tol_pct": 5.0, "unit": "ohm",
                             "source": {"kind": "User"}, "rationale": "标称 220R"}
            }]
        });

        let pass = run_test(
            serde_json::json!({"plan": plan, "meter": "virtual",
                               "virtual_readings": {"Resistance": 219.0}}),
            ctx(),
            None,
        )
        .await
        .unwrap();
        assert!(pass.output.contains("PASS"), "{}", pass.output);

        let fail = run_test(
            serde_json::json!({"plan": plan, "meter": "virtual",
                               "virtual_readings": {"Resistance": 300.0}}),
            ctx(),
            None,
        )
        .await
        .unwrap();
        assert!(fail.output.contains("FAIL"), "{}", fail.output);
        assert!(fail.output.contains("超出容差"), "{}", fail.output);
        assert!(fail.output.contains("合成数据"), "{}", fail.output);
    }

    #[tokio::test]
    async fn run_test_writes_a_report_when_asked() {
        let dir = tmp("instr-report");
        let plan = serde_json::json!({
            "plan_id": "tp-write",
            "steps": [{
                "index": 1, "target": "R1", "quantity": "Resistance",
                "channel": null, "samples": null, "reduce": "Mean", "supply": null,
                "expected": {"value": 220.0, "tol_pct": 5.0, "unit": "ohm",
                             "source": {"kind": "User"}, "rationale": "标称 220R"}
            }]
        });
        let out = run_test(
            serde_json::json!({"plan": plan, "meter": "virtual",
                               "virtual_readings": {"Resistance": 220.0},
                               "out_dir": dir.to_str().unwrap()}),
            ctx(),
            None,
        )
        .await
        .unwrap();
        assert!(out.output.contains("报告已写入"), "{}", out.output);
        let mut found = 0;
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().into_string().unwrap();
            if name.ends_with(".report.json") || name.ends_with(".report.txt") {
                found += 1;
            }
        }
        assert_eq!(found, 2, "both report artifacts must exist");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn lists_sim_recipes_for_every_circuit() {
        let all = ListSimRecipesTool
            .execute(serde_json::json!({}), ctx())
            .await
            .unwrap();
        assert!(all.output.contains("mosfet-switch"));
        assert!(all.output.contains("依据"), "{}", all.output);

        let one = ListSimRecipesTool
            .execute(serde_json::json!({"circuit_id": "rc-lowpass"}), ctx())
            .await
            .unwrap();
        assert!(one.output.contains("fc_3db"));
        assert!(!one.output.contains("mosfet-switch"));

        let none = ListSimRecipesTool
            .execute(serde_json::json!({"circuit_id": "nope"}), ctx())
            .await
            .unwrap();
        assert!(none.output.contains("没有电路"));
    }

    #[tokio::test]
    async fn simulation_reports_missing_ngspice_clearly() {
        // Inject a non-existent binary instead of mutating the environment.
        let out = simulate(
            serde_json::json!({"circuit_id": "voltage-divider"}),
            ctx(),
            Some("/nonexistent/ngspice"),
        )
        .await
        .unwrap();
        assert!(out.output.contains("无法仿真"), "{}", out.output);
        assert!(out.output.contains("ERC"), "{}", out.output);
    }

    #[tokio::test]
    async fn simulation_requires_an_input() {
        let out = simulate(serde_json::json!({}), ctx(), None).await.unwrap();
        assert!(
            out.output.contains("circuit_id") || out.output.contains("无法仿真"),
            "{}",
            out.output
        );
    }

    #[tokio::test]
    async fn simulation_rejects_an_unknown_circuit() {
        let out = simulate(
            serde_json::json!({"circuit_id": "does-not-exist"}),
            ctx(),
            None,
        )
        .await
        .unwrap();
        assert!(
            out.output.contains("未知电路") || out.output.contains("无法仿真"),
            "{}",
            out.output
        );
    }

    #[tokio::test]
    async fn adhoc_simulation_needs_an_analysis() {
        let ir = catalog::by_id("voltage-divider").unwrap().ir;
        let out = simulate(
            serde_json::json!({"netlist": serde_json::to_value(&ir).unwrap()}),
            ctx(),
            None,
        )
        .await
        .unwrap();
        assert!(
            out.output.contains("analysis") || out.output.contains("无法仿真"),
            "{}",
            out.output
        );
    }

    #[tokio::test]
    async fn sim_toolchain_report_names_models() {
        let out = CheckSimToolchainTool
            .execute(serde_json::json!({}), ctx())
            .await
            .unwrap();
        assert!(
            out.output.contains("Amplifier_Operational:LM321"),
            "{}",
            out.output
        );
        assert!(out.output.contains("ngspice"), "{}", out.output);
    }

    #[tokio::test]
    async fn run_simulation_produces_a_verdict_and_artifacts() {
        // Skips silently when ngspice is not installed.
        if Ngspice::probe().await.is_err() {
            eprintln!("skip: ngspice not available");
            return;
        }
        let dir = tmp("simrun");
        let out = RunSimulationTool
            .execute(
                serde_json::json!({
                    "circuit_id": "voltage-divider",
                    "out_dir": dir.to_str().unwrap()
                }),
                ctx(),
            )
            .await
            .unwrap();
        assert!(out.output.contains("PASS"), "{}", out.output);
        assert!(out.output.contains("产物"), "{}", out.output);
        assert!(out.output.contains(".svg"), "{}", out.output);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn generates_divider_with_real_symbols() {
        let tool = GenerateSchematicTool;
        let dir = tmp("gen");
        let path = dir.join("vdiv.kicad_sch");
        let out = tool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "netlist": divider_json()}),
                ctx(),
            )
            .await
            .unwrap();
        assert!(out.output.contains("4 个元件"), "{}", out.output);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("(kicad_sch"));
        assert!(text.contains("Device:R"), "real symbol must be embedded");
        assert!(!out.output.contains("缺失符号"), "{}", out.output);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn invalid_netlist_is_rejected_without_writing_a_file() {
        let tool = GenerateSchematicTool;
        let dir = tmp("invalid");
        let path = dir.join("bad.kicad_sch");
        let mut ir: Value = divider_json();
        // reference a component that does not exist
        ir["nets"][0]["connections"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(["Q9", "1"]));
        let out = tool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "netlist": ir}),
                ctx(),
            )
            .await
            .unwrap();
        assert!(out.output.contains("校验失败"), "{}", out.output);
        assert!(!path.exists(), "no file may be created for invalid IR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn missing_required_parameter_errors_without_panicking() {
        let tool = GenerateSchematicTool;
        let err = tool.execute(serde_json::json!({}), ctx()).await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("path"));

        let tool = SearchSymbolsTool;
        let err = tool.execute(serde_json::json!({}), ctx()).await;
        assert!(err.is_err());

        let tool = ApplyEditTool;
        let err = tool.execute(serde_json::json!({}), ctx()).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn malformed_netlist_json_reports_an_error_not_a_panic() {
        let tool = GenerateSchematicTool;
        let out = tool
            .execute(
                serde_json::json!({"path": "/tmp/x.kicad_sch", "netlist": {"components": "nope"}}),
                ctx(),
            )
            .await;
        assert!(out.is_err());
    }

    #[tokio::test]
    async fn edit_requires_confirmation_before_writing() {
        let dir = tmp("edit");
        let path = dir.join("e.kicad_sch");
        let ir = catalog::by_id("voltage-divider").unwrap().ir;
        GenerateSchematicTool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "netlist": serde_json::to_value(&ir).unwrap()}),
                ctx(),
            )
            .await
            .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        let mut changed = ir.clone();
        for c in &mut changed.components {
            if c.reference == "R2" {
                c.value = "4.7k".into();
            }
        }
        let payload = serde_json::to_value(&changed).unwrap();

        // Stage only: the file must not change.
        let staged = ApplyEditTool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "netlist": payload, "confirm": false}),
                ctx(),
            )
            .await
            .unwrap();
        assert!(staged.output.contains("R2"), "{}", staged.output);
        assert!(staged.output.contains("尚未写入"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // Confirm: now it writes.
        let applied = ApplyEditTool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "netlist": payload, "confirm": true}),
                ctx(),
            )
            .await
            .unwrap();
        assert!(applied.output.contains("已写入"), "{}", applied.output);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_ne!(after, before);
        assert!(after.contains("4.7k"));

        // Re-applying the same netlist is a no-op on the actual file content.
        let again = ApplyEditTool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "netlist": payload, "confirm": false}),
                ctx(),
            )
            .await
            .unwrap();
        assert!(again.output.contains("一致"), "{}", again.output);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reads_back_a_generated_schematic() {
        let dir = tmp("read");
        let path = dir.join("r.kicad_sch");
        let ir = catalog::by_id("led-driver").unwrap().ir;
        GenerateSchematicTool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "netlist": serde_json::to_value(&ir).unwrap()}),
                ctx(),
            )
            .await
            .unwrap();
        let out = ReadSchematicTool
            .execute(serde_json::json!({"path": path.to_str().unwrap()}), ctx())
            .await
            .unwrap();
        let parsed: NetlistIr = serde_json::from_str(&out.output).unwrap();
        assert!(parsed.reference_set().contains(&"D1".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn erc_tool_reports_the_toolchain_gate_clearly() {
        let dir = tmp("erc");
        let path = dir.join("x.kicad_sch");
        let ir = catalog::by_id("voltage-divider").unwrap().ir;
        GenerateSchematicTool
            .execute(
                serde_json::json!({"path": path.to_str().unwrap(), "netlist": serde_json::to_value(&ir).unwrap()}),
                ctx(),
            )
            .await
            .unwrap();
        let out = RunErcTool
            .execute(serde_json::json!({"path": path.to_str().unwrap()}), ctx())
            .await
            .unwrap();
        // Either ERC ran, or we got an actionable explanation (never a panic).
        assert!(
            out.output.contains("ERC") || out.output.contains("无法运行"),
            "{}",
            out.output
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn toolchain_report_is_informative() {
        let out = CheckToolchainTool
            .execute(serde_json::json!({}), ctx())
            .await
            .unwrap();
        assert!(out.output.contains("符号库"), "{}", out.output);
        assert!(out.output.contains("kicad-cli"), "{}", out.output);
    }

    #[tokio::test]
    async fn symbol_search_finds_a_known_part() {
        let out = SearchSymbolsTool
            .execute(serde_json::json!({"query": "lm358"}), ctx())
            .await
            .unwrap();
        assert!(
            out.output.contains("Amplifier_Operational:LM358") || out.output.contains("未找到"),
            "{}",
            out.output
        );
    }
}
