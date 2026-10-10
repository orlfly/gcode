//! gcode-tools: Gcode circuit tools exposed as jcode `Tool`s.
//!
//! Two tools:
//! - `gcode_generate_schematic`: NetlistIr JSON → `.kicad_sch` file
//! - `gcode_run_erc`: `.kicad_sch` → ERC report (via kicad-cli)

use async_trait::async_trait;
use gcode_kicad::{generate_schematic, Layout, NetlistIr};
use gcode_kicad::erc::run_erc;
use jcode::tool::{Tool, ToolContext, ToolOutput};
use serde_json::Value;

/// Tool: generate a KiCad schematic from a netlist IR.
pub struct GenerateSchematicTool;

#[async_trait]
impl Tool for GenerateSchematicTool {
    fn name(&self) -> &str {
        "gcode_generate_schematic"
    }

    fn description(&self) -> &str {
        "Generate a KiCad .kicad_sch schematic file from a netlist IR (components + nets JSON). \
         Deterministic: never write s-expressions yourself, describe the circuit as JSON."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "netlist"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Output .kicad_sch file path"
                },
                "netlist": {
                    "type": "object",
                    "description": "Netlist IR: {components: [{reference, lib_id, value, footprint?}], nets: [{name, connections: [[ref, pin], ...]}]}",
                    "properties": {
                        "components": {"type": "array"},
                        "nets": {"type": "array"}
                    }
                },
                "origin_x": {"type": "integer", "description": "Layout origin X (default 100)"},
                "origin_y": {"type": "integer", "description": "Layout origin Y (default 100)"},
                "pitch_y": {"type": "integer", "description": "Vertical pitch between components (default 100)"}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let netlist: NetlistIr = serde_json::from_value(
            input.get("netlist").cloned().unwrap_or(Value::Null),
        )?;
        let layout = Layout {
            origin_x: input.get("origin_x").and_then(|v| v.as_i64()).unwrap_or(100),
            origin_y: input.get("origin_y").and_then(|v| v.as_i64()).unwrap_or(100),
            pitch_y: input.get("pitch_y").and_then(|v| v.as_i64()).unwrap_or(100),
        };

        let doc = generate_schematic(&netlist, layout)
            .map_err(|e| anyhow::anyhow!("invalid netlist: {e}"))?;

        let abs = ctx.resolve_path(std::path::Path::new(path));
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&abs, doc).await?;

        let n_comp = netlist.components.len();
        let n_net = netlist.nets.len();
        Ok(ToolOutput::new(format!(
            "Wrote {path}: {n_comp} components, {n_net} nets"
        )))
    }
}

/// Tool: run ERC on a generated schematic.
pub struct RunErcTool;

#[async_trait]
impl Tool for RunErcTool {
    fn name(&self) -> &str {
        "gcode_run_erc"
    }

    fn description(&self) -> &str {
        "Run KiCad ERC (electrical rules check) on a .kicad_sch file via kicad-cli and \
         return a PASS/FAIL report with violations."
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
        let abs = ctx.resolve_path(std::path::Path::new(path));
        let report = run_erc(&abs).await?;
        let mut out = report.summary();
        for v in &report.violations {
            out.push_str(&format!(
                "\n- [{}] {}: {}",
                v.severity,
                v.type_.as_deref().unwrap_or("unknown"),
                v.description
            ));
        }
        Ok(ToolOutput::new(out))
    }
}

/// All Gcode tools, for registration with the jcode tool registry.
pub fn all_tools() -> Vec<Box<dyn Tool>> {
    vec![Box::new(GenerateSchematicTool), Box::new(RunErcTool)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode::tool::ToolContext;

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

    #[tokio::test]
    async fn generates_voltage_divider() {
        let tool = GenerateSchematicTool;
        let input = serde_json::json!({
            "path": "/tmp/gcode-test/vdiv.kicad_sch",
            "netlist": {
                "components": [
                    {"reference": "R1", "lib_id": "Device:R", "value": "10k"},
                    {"reference": "R2", "lib_id": "Device:R", "value": "10k"}
                ],
                "nets": [
                    {"name": "VIN", "connections": [["R1", "1"]]},
                    {"name": "VOUT", "connections": [["R1", "2"], ["R2", "1"]]},
                    {"name": "GND", "connections": [["R2", "2"]]}
                ]
            }
        });
        let out = tool.execute(input, ctx()).await.unwrap();
        assert!(out.output.contains("2 components, 3 nets"));
        let text = std::fs::read_to_string("/tmp/gcode-test/vdiv.kicad_sch").unwrap();
        assert!(text.starts_with("(kicad_sch"));
    }

    #[test]
    fn tool_registry() {
        let tools = all_tools();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name(), "gcode_generate_schematic");
        assert_eq!(tools[1].name(), "gcode_run_erc");
    }
}
