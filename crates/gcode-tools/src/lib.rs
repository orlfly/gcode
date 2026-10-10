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

/// All Gcode tools, for registration with the jcode tool registry.
pub fn all_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(GenerateSchematicTool),
        Box::new(RunErcTool),
        Box::new(ReadSchematicTool),
        Box::new(ApplyEditTool),
        Box::new(SearchSymbolsTool),
        Box::new(CheckToolchainTool),
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
        assert_eq!(names.len(), 6);
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "tool names must be unique");
        assert!(names.contains(&"gcode_generate_schematic"));
        assert!(names.contains(&"gcode_run_erc"));
        assert!(names.contains(&"gcode_apply_edit"));
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
