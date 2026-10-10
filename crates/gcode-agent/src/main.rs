//! gcode: professional CLI circuit-design agent built on the jcode framework.
//!
//! Flow: NL intent → (LLM) NetlistIr JSON → deterministic .kicad_sch → ERC →
//! self-verification loop. TUI and agent loop are reused from jcode; Gcode
//! contributes its tool set and system prompt.

use anyhow::Result;
use std::sync::Arc;

mod prompt;

/// Gcode entry: provider + registry (with circuit tools) → jcode TUI app loop.
#[tokio::main]
async fn main() -> Result<()> {
    // 1) Provider + tool registry via the jcode cli layer.
    let (provider, registry) = jcode::cli::provider_init::init_provider_and_registry(
        &jcode::cli::provider_init::ProviderChoice::Auto,
        None,
    )
    .await?;

    // 2) Register Gcode circuit tools.
    for tool in gcode_tools::all_tools() {
        let name = tool.name().to_string();
        registry.register(name, Arc::from(tool)).await;
    }

    // 3) Terminal + TUI app (full jcode agent loop: streaming, tool exec,
    //    permissions, compaction, slash commands).
    let (terminal, tui_runtime) = jcode::cli::terminal::init_tui_runtime()?;
    let mut app = jcode::tui::App::new(provider, registry);
    // Full system prompt override (same mechanism as ambient mode).
    // queue_startup_message would auto-send, so queue an empty (no-op) turn:
    // override the prompt while leaving the conversation user-driven.
    app.set_ambient_mode(prompt::GCODE_SYSTEM_PROMPT.to_string(), String::new());

    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle("gcode - circuit design agent")
    )?;

    let result = app.run(terminal).await;
    tui_runtime.finish(true);
    result.map(|_| ())
}
