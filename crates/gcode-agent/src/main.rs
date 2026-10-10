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

    // 3) Best-effort toolchain report: never blocks startup, and a missing or
    //    too-old kicad-cli only disables ERC, not schematic generation.
    report_toolchain().await;

    // 4) Terminal + TUI app (full jcode agent loop: streaming, tool exec,
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

/// Print a one-line toolchain status to stderr. Best effort: silence is fine.
async fn report_toolchain() {
    let symbols = gcode_kicad::SymbolLibrary::locate()
        .dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "未找到".into());
    let kicad = match gcode_kicad::erc::KicadToolchain::probe().await {
        Ok(tc) => format!("kicad-cli {} ({})", tc.version, tc.path.display()),
        Err(_) => "kicad-cli 不可用（生成可用，ERC 不可用）".to_string(),
    };
    let ngspice = match gcode_sim::runner::Ngspice::probe().await {
        Ok(n) => format!("{} ({})", n.version, n.path.display()),
        Err(_) => "ngspice 不可用（生成/ERC 可用，仿真不可用）".to_string(),
    };
    eprintln!("gcode: {kicad} · {ngspice} · 符号库 {symbols}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole runtime must be redirectable, and must never point at jcode's
    /// own user data. One test on purpose: env vars are process-global, so
    /// mutating them from several tests would race.
    #[test]
    fn isolated_home_redirects_everything_and_never_targets_jcode() {
        let dir = std::env::temp_dir().join(format!("gcode-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 1) Default (no override) is ~/.gcode, and must not sit under jcode's
        //    own data directory.
        let jcode_home = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join(".jcode");
        unsafe { std::env::remove_var("GCODE_HOME") };
        let default = gcode_home();
        assert_eq!(default.file_name().and_then(|s| s.to_str()), Some(".gcode"));
        assert!(
            !default.starts_with(&jcode_home),
            "default gcode home must not be inside {}",
            jcode_home.display()
        );

        // 2) An explicit override wins.
        // SAFETY: this test owns the env for the duration of the binary's tests.
        unsafe {
            std::env::set_var("GCODE_HOME", &dir);
            std::env::remove_var("JCODE_HOME");
            std::env::remove_var("JCODE_RUNTIME_DIR");
        }

        assert_eq!(gcode_home(), dir, "GCODE_HOME override must be honoured");
        let home = install_isolated_env().expect("install env");
        assert_eq!(home, dir);

        for sub in ["logs", "sessions", "skills", "runtime", "tmp"] {
            assert!(dir.join(sub).is_dir(), "missing {sub}");
        }

        let expect = |key: &str, want: &std::path::Path| {
            let got = std::env::var(key).unwrap_or_default();
            assert_eq!(got, want.display().to_string(), "{key} not redirected");
        };
        expect("JCODE_HOME", &dir);
        expect("JCODE_RUNTIME_DIR", &dir.join("runtime"));
        expect("TMPDIR", &dir.join("tmp"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
