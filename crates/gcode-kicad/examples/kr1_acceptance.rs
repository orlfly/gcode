//! KR1 acceptance harness.
//!
//! Walks the 10-circuit catalog and, for each circuit, records:
//!   1. generation success (offline, deterministic reference implementation),
//!   2. KiCad-loadability + netlist correctness via `kicad-cli sch export netlist`,
//!   3. ERC verdict via `kicad-cli sch erc` (only when the toolchain passes the
//!      configured minimum version).
//!
//! KR1's criterion is "NL description → ERC-passing KiCad project, >= 7/10".
//! The NL→IR step itself is performed by the agent (needs an LLM provider), so
//! this harness measures the deterministic half plus the ERC half and reports
//! which half could not run.
//!
//! Usage:
//!   cargo run -p gcode-kicad --example kr1_acceptance -- [--out report.md] [--workdir DIR]

use gcode_kicad::catalog;
use gcode_kicad::erc::{kicad_cli_path, KicadToolchain};
use gcode_kicad::schematic::{generate_with_report, Layout};
use gcode_kicad::sexpr::Sexpr;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
enum ErcState {
    Clean,
    Violations(usize),
    Unavailable(String),
}

#[derive(Debug)]
struct Row {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    expectation: &'static str,
    components: usize,
    nets: usize,
    generated: bool,
    /// False when the run had to fall back to the generic stub for any symbol.
    symbols_resolved: bool,
    kicad_loadable: Option<bool>,
    netlist_correct: Option<bool>,
    erc: ErcState,
    notes: Vec<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out_path = PathBuf::from("openspec/changes/r0-design-mvp/kr1-report.md");
    let mut workdir = std::env::temp_dir().join("gcode-kr1");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out_path = PathBuf::from(args.next().expect("--out needs a path")),
            "--workdir" => workdir = PathBuf::from(args.next().expect("--workdir needs a path")),
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    std::fs::create_dir_all(&workdir)?;

    let cli_ok = Command::new(kicad_cli_path())
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // ERC is gated on the version the project requires.
    let toolchain = tokio::runtime::Runtime::new()?.block_on(KicadToolchain::probe());

    let mut rows = Vec::new();
    for entry in catalog::all() {
        let mut notes = Vec::new();
        let report = generate_with_report(&entry.ir, Layout::default())?;
        let generated = !report.content.is_empty() && entry.ir.validate().is_ok();
        let symbols_resolved = report.missing_symbols.is_empty();
        if !report.warnings.is_empty() {
            notes.push(format!("生成警告 {} 条", report.warnings.len()));
        }
        if !symbols_resolved {
            // A stub substitution silently changes pin semantics, so ERC cannot
            // be trusted for this circuit. It must not count as accepted.
            notes.push(format!(
                "符号缺失 {:?} → 已回退通用符号，该电路的 ERC 结果不可信",
                report.missing_symbols
            ));
        }

        let path = workdir.join(format!("{}.kicad_sch", entry.id));
        std::fs::write(&path, &report.content)?;

        let (loadable, correct) = if cli_ok {
            match export_netlist(&path) {
                Some(text) => {
                    let actual = parse_netlist(&text);
                    let expected = expected_nets(&entry.ir);
                    (Some(true), Some(actual == expected))
                }
                None => (Some(false), None),
            }
        } else {
            (None, None)
        };

        let erc = match &toolchain {
            Ok(tc) => match run_erc_blocking(&path) {
                Some(clean) => {
                    if clean {
                        ErcState::Clean
                    } else {
                        let n = count_violations(&path);
                        ErcState::Violations(n)
                    }
                }
                None => ErcState::Unavailable(format!("kicad-cli {} 不支持 sch erc", tc.version)),
            },
            Err(e) => ErcState::Unavailable(e.to_string()),
        };

        rows.push(Row {
            id: entry.id,
            title: entry.title,
            description: entry.description,
            expectation: entry.expectation,
            components: entry.ir.components.len(),
            nets: entry.ir.nets.len(),
            generated,
            symbols_resolved,
            kicad_loadable: loadable,
            netlist_correct: correct,
            erc,
            notes,
        });
    }

    let md = render(&rows, cli_ok);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, &md)?;
    print!("{md}");
    eprintln!("\nwrote {}", out_path.display());

    // Non-zero exit if the measurable half regressed.
    let gen_ok = rows.iter().filter(|r| r.generated).count();
    let net_ok = rows
        .iter()
        .filter(|r| r.netlist_correct == Some(true))
        .count();
    if gen_ok < rows.len() {
        std::process::exit(2);
    }
    if cli_ok && net_ok < rows.len() {
        std::process::exit(3);
    }
    let sym_ok = rows.iter().filter(|r| r.symbols_resolved).count();
    if sym_ok < rows.len() {
        std::process::exit(4);
    }
    Ok(())
}

fn export_netlist(schematic: &Path) -> Option<String> {
    let out = schematic.with_extension("net");
    let status = Command::new(kicad_cli_path())
        .args([
            "sch",
            "export",
            "netlist",
            "--format",
            "kicadsexpr",
            "-o",
        ])
        .arg(&out)
        .arg(schematic)
        .output()
        .ok()?;
    status.status.success().then(|| std::fs::read_to_string(&out).ok()).flatten()
}

fn run_erc_blocking(schematic: &Path) -> Option<bool> {
    let report = schematic.with_extension("erc.json");
    let status = Command::new(kicad_cli_path())
        .args(["sch", "erc", "--format", "json", "--output"])
        .arg(&report)
        .arg(schematic)
        .output()
        .ok()?;
    if !status.status.success() {
        return None;
    }
    let text = std::fs::read_to_string(&report).ok()?;
    let report: gcode_kicad::ErcReport = serde_json::from_str(&text).ok()?;
    Some(report.is_clean())
}

fn count_violations(schematic: &Path) -> usize {
    let report = schematic.with_extension("erc.json");
    std::fs::read_to_string(&report)
        .ok()
        .and_then(|t| serde_json::from_str::<gcode_kicad::ErcReport>(&t).ok())
        .map(|r| r.violations.len())
        .unwrap_or(0)
}

fn parse_netlist(text: &str) -> BTreeMap<String, BTreeSet<(String, String)>> {
    let docs = Sexpr::parse(text).unwrap_or_default();
    let mut out = BTreeMap::new();
    let Some(root) = docs.first() else {
        return out;
    };
    let Some(nets) = root.find("nets") else {
        return out;
    };
    for net in nets.find_all("net") {
        let name = net
            .get("name")
            .and_then(Sexpr::as_str)
            .unwrap_or("")
            .trim_start_matches('/')
            .to_string();
        let mut nodes = BTreeSet::new();
        for node in net.find_all("node") {
            nodes.insert((
                node.get("ref").and_then(Sexpr::as_str).unwrap_or("").to_string(),
                node.get("pin").and_then(Sexpr::as_str).unwrap_or("").to_string(),
            ));
        }
        out.insert(name, nodes);
    }
    out
}

/// KiCad drops `(power)` symbols from exported netlists, so expectation
/// excludes the `#`-referenced flags; nets left with no nodes disappear.
fn expected_nets(ir: &gcode_kicad::NetlistIr) -> BTreeMap<String, BTreeSet<(String, String)>> {
    let mut out = BTreeMap::new();
    for net in &ir.nets {
        let nodes: BTreeSet<(String, String)> = net
            .connections
            .iter()
            .filter(|(r, _)| !r.starts_with('#'))
            .map(|(r, p)| (r.clone(), p.clone()))
            .collect();
        if !nodes.is_empty() {
            out.insert(net.name.clone(), nodes);
        }
    }
    out
}

fn render(rows: &[Row], cli_ok: bool) -> String {
    let gen_ok = rows.iter().filter(|r| r.generated).count();
    let net_ok = rows.iter().filter(|r| r.netlist_correct == Some(true)).count();
    // A circuit only counts if the pins are the real ones *and* it passed ERC.
    let accepted = |r: &Row| {
        r.generated && r.symbols_resolved && r.netlist_correct != Some(false) && r.erc == ErcState::Clean
    };
    let accepted_n = rows.iter().filter(|r| accepted(r)).count();
    let sym_ok = rows.iter().filter(|r| r.symbols_resolved).count();
    let erc_measured: Vec<&Row> = rows
        .iter()
        .filter(|r| matches!(r.erc, ErcState::Clean | ErcState::Violations(_)))
        .collect();
    let erc_ok = erc_measured
        .iter()
        .filter(|r| r.erc == ErcState::Clean)
        .count();

    let mut md = String::new();
    md.push_str("# KR1 验收报告（R0 设计 MVP）\n\n");
    md.push_str("> 由 `cargo run -p gcode-kicad --example kr1_acceptance` 生成。\n\n");
    md.push_str("KR1 原文：10 个典型电路从自然语言描述到通过 ERC + 仿真的 KiCad 工程，");
    md.push_str("首版目标成功率 ≥ 7/10（含一轮对话式修正）。\n\n");

    md.push_str("## 汇总\n\n");
    md.push_str("| 轨道 | 结果 |\n|---|---|\n");
    md.push_str(&format!(
        "| ① 生成（网表 IR → .kicad_sch，离线、确定性） | **{gen_ok}/{}** |\n",
        rows.len()
    ));
    if cli_ok {
        md.push_str(&format!(
            "| ② KiCad 可读 + 网表与 IR 一致（真实 kicad-cli netlist 导出） | **{net_ok}/{}** |\n",
            rows.len()
        ));
    } else {
        md.push_str("| ② KiCad 可读 + 网表一致 | 未测量（未安装 kicad-cli） |\n");
    }
    if erc_measured.is_empty() {
        let reason = rows
            .iter()
            .find_map(|r| match &r.erc {
                ErcState::Unavailable(m) => Some(m.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "未知".into());
        md.push_str(&format!(
            "| ③ ERC（KR1 的判定轨道） | **未测量** —— {reason} |\n"
        ));
    } else {
        md.push_str(&format!(
            "| ③ ERC PASS（KR1 判定） | **{erc_ok}/{}**（目标 ≥ 7） |\n",
            erc_measured.len()
        ));
    }
    md.push_str(&format!(
        "| ④ 符号解析（未回退通用 stub） | **{sym_ok}/{}** |\n",
        rows.len()
    ));
    md.push_str("| ⑤ NL → 网表（需要 LLM provider） | 本脚本不执行；由 agent 会话产生 |\n\n");
    md.push_str(&format!(
        "**KR1 判定（①②③④ 全部满足的电路数）**：**{accepted_n}/{}**（目标 ≥ 7）\n\n",
        rows.len()
    ));

    md.push_str("## 逐电路结果\n\n");
    md.push_str("| 电路 | 元件/网络 | 生成 | 符号解析 | KiCad 可读 | 网表一致 | ERC | 接受 |\n|---|---|---|---|---|---|---|---|\n");
    for r in rows {
        let yn = |b: Option<bool>| match b {
            Some(true) => "✓",
            Some(false) => "✗",
            None => "—",
        };
        let erc = match &r.erc {
            ErcState::Clean => "PASS".to_string(),
            ErcState::Violations(n) => format!("FAIL({n})"),
            ErcState::Unavailable(_) => "未测".to_string(),
        };
        md.push_str(&format!(
            "| {} ({}) | {}/{} | {} | {} | {} | {} | {} | {} |\n",
            r.title,
            r.id,
            r.components,
            r.nets,
            if r.generated { "✓" } else { "✗" },
            yn(Some(r.symbols_resolved)),
            yn(r.kicad_loadable),
            yn(r.netlist_correct),
            erc,
            if accepted(r) { "✓" } else { "✗" }
        ));
    }

    md.push_str("\n## 电路清单与预期\n\n");
    for r in rows {
        md.push_str(&format!(
            "- **{}**（`{}`）：{}\n  - 预期：{}\n",
            r.title, r.id, r.description, r.expectation
        ));
        for n in &r.notes {
            md.push_str(&format!("  - 备注：{n}\n"));
        }
    }

    md.push_str("\n## 结论与遗留\n\n");
    if erc_measured.is_empty() {
        md.push_str("- ⚠️ **KR1 尚未判定**：kicad-cli 不支持 `sch erc`，ERC 轨道无法测量。\n");
        md.push_str("- 遗留：安装 KiCad ≥ 10 后重跑本脚本。\n");
    } else if accepted_n * 10 >= 7 * rows.len() {
        md.push_str(&format!(
            "- ✅ **KR1 达标**：①生成 + ②符号解析 + ③KiCad 网表一致 + ④ERC PASS 全满足 {accepted_n}/{} ≥ 7/10。\n",
            rows.len()
        ));
        md.push_str("- 说明：ERC 由 kicad-cli 判定；符号必须来自真实库（回退 stub 的电路不计入）。\n");
    } else {
        md.push_str(&format!(
            "- ❌ **KR1 未达标**：全满足的电路 {accepted_n}/{} < 7/10，需回到生成/ERC 迭代。\n",
            rows.len()
        ));
    }
    md.push_str("- 遗留：NL → 网表 轨道需要在配置了 provider 的 agent 会话里跑 10 条自然语言描述（本脚本只覆盖确定性半段）。\n");
    md
}
