//! Simulation acceptance harness (R1).
//!
//! Runs every recipe in `gcode_sim::recipes` against the real ngspice and
//! compares the simulated metrics with the analytically derived expectations.
//! Writes a report and exits non-zero if anything fails.
//!
//! Usage:
//!   cargo run -p gcode-sim --example sim_acceptance -- [--out report.md] [--workdir DIR]

use gcode_sim::recipes::{self, SimOutcome};
use gcode_sim::runner::Ngspice;
use gcode_kicad::catalog;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out_path = PathBuf::from("openspec/changes/r1-simulation/sim-report.md");
    let mut workdir = std::env::temp_dir().join("gcode-sim-acceptance");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out_path = PathBuf::from(args.next().expect("--out needs a path")),
            "--workdir" => workdir = PathBuf::from(args.next().expect("--workdir needs a path")),
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    std::fs::create_dir_all(&workdir)?;

    let rt = tokio::runtime::Runtime::new()?;
    let ngspice = match rt.block_on(Ngspice::probe()) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("skip: {e}");
            std::process::exit(0);
        }
    };
    println!("using {} ({})", ngspice.version, ngspice.path.display());

    let mut outcomes: Vec<SimOutcome> = Vec::new();
    let mut errors: Vec<(String, String)> = Vec::new();

    for recipe in recipes::all() {
        let entry = catalog::by_id(recipe.circuit_id)
            .ok_or_else(|| format!("unknown circuit {}", recipe.circuit_id))?;
        let dir = workdir.join(format!(
            "{}.{}",
            recipe.circuit_id,
            recipe.name.replace(' ', "_")
        ));
        match rt.block_on(recipes::run_recipe(
            &ngspice,
            &recipe,
            &entry.ir,
            &dir,
            true,
        )) {
            Ok(o) => outcomes.push(o),
            Err(e) => errors.push((
                format!("{} [{}]", recipe.circuit_id, recipe.name),
                e.to_string(),
            )),
        }
    }

    let md = render(&outcomes, &errors, &ngspice.version);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, &md)?;
    print!("{md}");
    eprintln!("\nwrote {}", out_path.display());

    let failed = outcomes.iter().filter(|o| !o.verdict.passed).count();
    if failed > 0 || !errors.is_empty() {
        std::process::exit(2);
    }
    Ok(())
}

fn render(outcomes: &[SimOutcome], errors: &[(String, String)], version: &str) -> String {
    let mut md = String::new();
    md.push_str("# R1 仿真验收报告\n\n");
    md.push_str(&format!(
        "> 由 `cargo run -p gcode-sim --example sim_acceptance` 生成；后端 `{version}`。\n\n"
    ));
    md.push_str(
        "对比对象是**解析推导**的预期值（公式与依据列在每项旁），不是上一次仿真的结果。\n\
         有源器件使用**行为级 macromodel**，因此容差按「拓扑与量级正确」设定。\n\n",
    );

    let total_checks: usize = outcomes.iter().map(|o| o.verdict.checks.len()).sum();
    let passed_checks: usize = outcomes.iter().map(|o| o.verdict.passed_count()).sum();
    let passed_recipes = outcomes.iter().filter(|o| o.verdict.passed).count();

    md.push_str("## 汇总\n\n");
    md.push_str("| 项 | 结果 |\n|---|---|\n");
    md.push_str(&format!(
        "| 仿真配方通过 | **{passed_recipes}/{}** |\n",
        outcomes.len()
    ));
    md.push_str(&format!(
        "| 指标项通过 | **{passed_checks}/{total_checks}** |\n"
    ));
    md.push_str(&format!(
        "| 运行失败（异常） | {} |\n\n",
        errors.len()
    ));

    md.push_str("## 逐电路结果\n\n");
    md.push_str("| 电路 | 分析 | 指标 | 实测 | 预期 | 偏差 | 判定 |\n|---|---|---|---|---|---|---|\n");
    for o in outcomes {
        for c in &o.verdict.checks {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} {} | {} | {} |\n",
                o.circuit_id,
                o.analysis_tag,
                c.label,
                c.measured
                    .map(|m| format!("{} {}", fmt(m), c.unit))
                    .unwrap_or_else(|| "—".into()),
                fmt(c.expected),
                c.unit,
                c.deviation_pct
                    .map(|p| format!("{p:+.2}%"))
                    .unwrap_or_else(|| "—".into()),
                if c.passed { "PASS" } else { "FAIL" }
            ));
        }
    }

    md.push_str("\n## 依据与产物\n\n");
    for recipe in recipes::all() {
        md.push_str(&format!(
            "- **{}** / {}：{}（{}）\n",
            recipe.circuit_id, recipe.name, recipe.basis, recipe.analysis.kind.tag()
        ));
        for e in &recipe.expectations {
            md.push_str(&format!(
                "  - {} 预期 {} {}（±{}%）：{}\n",
                e.metric,
                fmt(e.expected),
                e.unit,
                fmt(e.tol_pct),
                e.rationale
            ));
        }
    }
    if let Some(o) = outcomes.first() {
        if !o.behavioral_devices.is_empty() {
            md.push_str("\n行为级模型使用情况（示例）：\n");
            for d in o.behavioral_devices.iter().take(6) {
                md.push_str(&format!("- {d}\n"));
            }
        }
    }

    if !errors.is_empty() {
        md.push_str("\n## 运行失败\n\n");
        for (what, err) in errors {
            md.push_str(&format!("- {what}: {err}\n"));
        }
    }

    md.push_str("\n## 结论\n\n");
    if errors.is_empty() && passed_recipes == outcomes.len() {
        md.push_str(&format!(
            "- ✅ 全部 {passed_recipes} 个配方、{passed_checks} 项指标落在解析预期容差内。\n"
        ));
        md.push_str("- 说明：有源器件为行为级模型，验证的是拓扑与量级正确性，不是器件级精度。\n");
    } else {
        md.push_str(&format!(
            "- ❌ {passed_recipes}/{} 个配方通过；详见上表与失败列表。\n",
            outcomes.len()
        ));
    }
    md
}

fn fmt(v: f64) -> String {
    if !v.is_finite() {
        return "n/a".into();
    }
    let a = v.abs();
    if a != 0.0 && (a < 1e-3 || a >= 1e6) {
        return format!("{v:.4e}");
    }
    let mut s = format!("{v:.6}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}
