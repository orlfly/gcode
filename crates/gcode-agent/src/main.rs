//! gcode: professional CLI circuit-design agent built on the jcode framework.
//!
//! Flow: NL intent → (LLM) NetlistIr JSON → deterministic .kicad_sch → ERC →
//! self-verification loop. TUI and agent loop are reused from jcode; Gcode
//! contributes its tool set, system prompt, and a fully isolated runtime
//! (config, sessions, logs, temp) so it never touches jcode's user data.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

mod prompt;

/// Isolated Gcode home: `~/.gcode` (overridable via `GCODE_HOME`).
///
/// Set as `JCODE_HOME` before any jcode code runs, so every jcode subsystem
/// (config.toml, sessions, auth, logs, skills, memory) resolves under the
/// Gcode directory and jcode's own `~/.jcode` data is never read or written.
fn gcode_home() -> PathBuf {
    if let Some(p) = std::env::var_os("GCODE_HOME") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gcode")
}

/// Install the isolated runtime environment. Must run before any jcode call.
fn install_isolated_env() -> Result<PathBuf> {
    let home = gcode_home();
    std::fs::create_dir_all(&home)
        .with_context(|| format!("failed to create Gcode home {}", home.display()))?;
    // Gcode-specific subdirs jcode will populate on demand.
    for sub in ["logs", "sessions", "skills"] {
        let _ = std::fs::create_dir_all(home.join(sub));
    }

    // 1) Redirect all jcode storage (config, sessions, auth, logs).
    // SAFETY: single-threaded process startup; no other thread has read env yet.
    unsafe { std::env::set_var("JCODE_HOME", &home) };

    // 2) Redirect sockets/ephemeral runtime state.
    let runtime = home.join("runtime");
    let _ = std::fs::create_dir_all(&runtime);
    unsafe { std::env::set_var("JCODE_RUNTIME_DIR", &runtime) };

    // 3) Redirect std::env::temp_dir()-based temp files to Gcode's tmp.
    let tmp = home.join("tmp");
    std::fs::create_dir_all(&tmp)
        .with_context(|| format!("failed to create Gcode tmp {}", tmp.display()))?;
    unsafe { std::env::set_var("TMPDIR", &tmp) };

    Ok(home)
}

/// Gcode entry: isolated env → provider + registry (circuit tools) → TUI loop.
#[tokio::main]
async fn main() -> Result<()> {
    let home = install_isolated_env()?;

    // 1) Provider + tool registry via the jcode cli layer (reads Gcode's own
    //    config, not jcode's).
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
    eprintln!("gcode: isolated home at {}", home.display());
    result.map(|_| ())
}
