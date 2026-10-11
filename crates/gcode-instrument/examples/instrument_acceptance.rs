//! R1.5 acceptance harness.
//!
//! Runs the whole chain — plan → safety gate → execute → compare → report — in
//! three configurations, and writes a report:
//!
//! A. **Virtual backend, in tolerance.** Proves the happy path and the report.
//! B. **Virtual backend, 15% error on a DC step.** Proves the KR2 verdict and
//!    attribution actually fire (a check that always passes is worthless).
//! C. **Cross-domain: ngspice simulation vs a real sigrok acquisition.** The
//!    baseline comes from an ngspice transient run; the "measurement" comes from
//!    `sigrok-cli` driving its synthetic `demo` device. This exercises real
//!    simulator and real acquisition code end to end — but both sides are
//!    synthetic, so the report says so and cannot be read as a board result.
//!
//! Usage:
//!   cargo run -p gcode-instrument --example instrument_acceptance -- [--out FILE]

use gcode_instrument::compare::TolerancePolicy;
use gcode_instrument::executor::Executor;
use gcode_instrument::hal::{
    probe_backends, Instrument, MeasureRequest, Measurement, Quantity, Reduce,
};
use gcode_instrument::instruments::{SigrokInstrument, VirtualInstrument};
use gcode_instrument::plan::{Expected, TestPlan, TestStep};
use gcode_instrument::report::TestReport;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out_path = PathBuf::from("openspec/changes/r15-instrument/instrument-report.md");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out_path = PathBuf::from(args.next().expect("--out needs a path")),
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    let rt = tokio::runtime::Runtime::new()?;

    let mut md = String::new();
    md.push_str("# R1.5 实测链路验收报告\n\n");
    md.push_str("> 由 `cargo run -p gcode-instrument --example instrument_acceptance` 生成。\n\n");

    // ---- Backend availability ----
    md.push_str("## 仪器后端探测\n\n");
    let statuses = rt.block_on(probe_backends());
    for s in &statuses {
        md.push_str(&format!("- {}\n", s.describe()));
    }
    md.push('\n');

    // ---- A: virtual, within tolerance ----
    let (report_a, notes_a) = section_a(&rt);
    md.push_str("## A. 虚拟仪器 · 容差内\n\n");
    md.push_str(&format!(
        "目的：验证「计划 → 安全闸门 → 执行 → 对比 → 报告」正常路径与 KR2 判定。\n\n\
         步骤预期值来自仿真配方（`gcode-sim`），实测值由虚拟仪器给出。\n\n```\n{}\n```\n\n",
        report_a.render()
    ));
    for n in notes_a {
        md.push_str(&format!("- {n}\n"));
    }
    md.push('\n');

    // ---- B: virtual, out of tolerance ----
    let (report_b, notes_b) = section_b(&rt);
    md.push_str("## B. 虚拟仪器 · 注入 15% 直流误差\n\n");
    md.push_str(&format!(
        "目的：证明 KR2 判定与归因真的会触发（默认直流阈值 10%）。\n\n```\n{}\n```\n\n",
        report_b.render()
    ));
    for n in notes_b {
        md.push_str(&format!("- {n}\n"));
    }
    md.push('\n');

    // ---- C: cross-domain ----
    md.push_str("## C. 跨域：ngspice 仿真 vs sigrok 采集\n\n");
    match section_c(&rt) {
        Some((report_c, detail)) => {
            md.push_str(&format!("{}\n\n```\n{}\n```\n\n", detail, report_c.render()));
        }
        None => {
            md.push_str(
                "跳过：需要 ngspice 与 sigrok-cli 同时可用，且 sigrok 需能枚举到一个设备。\n\n",
            );
        }
    }

    // ---- Verdict ----
    md.push_str("## 结论\n\n");
    let a_ok = report_a.overall == "PASS";
    let b_ok = report_b.overall == "FAIL" && report_b.failed == 1;
    md.push_str(&format!(
        "- A 正常路径：{}（{}）\n",
        if a_ok { "✅ PASS 符合预期" } else { "❌ 与预期不符" },
        report_a.summary
    ));
    md.push_str(&format!(
        "- B 容差外：{}（{}）\n",
        if b_ok {
            "✅ FAIL 且已归因，符合预期"
        } else {
            "❌ 未按预期失败"
        },
        report_b.summary
    ));
    md.push_str(
        "- 诚实边界：本机**没有真实测量硬件**（无 DMM/可编程电源）。A/B 用的是虚拟仪器，\
         C 用的是 sigrok 自带的合成 demo 设备。因此本报告验证的是**链路与判定机制**，\
         **不是**某块真实板卡的 KR2 结论。要得到真实结论，需要接入一台 sigrok 兼容 DMM\n\
         （或可编程电源）后重跑。\n",
    );

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, &md)?;
    let (json, txt) = report_a.write(out_path.parent().unwrap_or(std::path::Path::new(".")), "instrument-A")?;
    println!("{md}");
    eprintln!("\nwrote {}", out_path.display());
    eprintln!("wrote {} and {}", json.display(), txt.display());

    if !(a_ok && b_ok) {
        std::process::exit(2);
    }
    Ok(())
}

/// A: simulation-baselined plan on a virtual bench.
///
/// Two sub-cases: a passive measurement (no energising), and a powered one that
/// runs through the confirmation gate and ends with the output off.
fn section_a(_rt: &tokio::runtime::Runtime) -> (TestReport, Vec<String>) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let baseline = 12.0 * 10_000.0 / 49_000.0;

    // The opt-in is an operator decision; this harness is the operator.
    // SAFETY: single-threaded main thread of this example.
    unsafe { std::env::set_var(gcode_instrument::hal::ALLOW_SOURCE_ENV, "1") };

    let meter = VirtualInstrument::new()
        .with_reading(Quantity::VoltageDc, baseline)
        .with_reading(Quantity::CurrentDc, 0.004);

    let plan = TestPlan::new(
        "tp-a-passive",
        vec![TestStep::new(
            1,
            "VOUT（无源测量）",
            Quantity::VoltageDc,
            Expected::from_simulation(
                "voltage-divider",
                "VOUT",
                "op",
                baseline,
                1.0,
                "V",
                "VOUT = VIN x R2/(R1+R2)，与 gcode-sim 的 .op 结果一致",
            ),
        )],
    )
    .expect("plan");

    let mut report = rt.block_on(Executor::new(&meter, None).run(&plan, None));

    // Powered sub-case: same expected value, but the rail must be energised. The
    // virtual bench can source, and this run carries the operator's confirmation.
    let powered_plan = TestPlan::new(
        "tp-a-powered",
        vec![TestStep::new(
            2,
            "VOUT（带电验证 3V3 轨）",
            Quantity::VoltageDc,
            Expected::from_user(3.3, 2.0, "V", "用户要求 3.3V 轨"),
        )
        .powered(5.0, 0.2)
        .with_expected_current(0.02)],
    )
    .expect("powered plan");
    let meter2 = VirtualInstrument::new()
        .with_reading(Quantity::VoltageDc, 3.3)
        .with_reading(Quantity::CurrentDc, 0.004);
    let token = gcode_instrument::safety::ConfirmToken::mint(true).expect("operator confirmed");
    let powered = rt.block_on(Executor::new(&meter2, None).run(&powered_plan, Some(&token)));

    // Present both in one report by concatenating the renderings.
    let mut text = report.render();
    text.push_str("\n—— 带电子用例 ——\n");
    text.push_str(&powered.render());
    report.events.extend(powered.events.clone());

    let notes = vec![
        "无源子用例：不带电直接测量，预期值来自仿真基准（gcode-sim 的 .op 结果）。".to_string(),
        "带电子用例：3.3V 轨，携带用户确认令牌；虚拟台可测也可供电，结束后自动断电。"
            .to_string(),
        format!("两次判定：无源 {} / 带电 {}", report.overall, powered.overall),
    ];
    (report, notes)
}

/// B: virtual instrument with a 15% systematic error.
fn section_b(_rt: &tokio::runtime::Runtime) -> (TestReport, Vec<String>) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let baseline = 12.0 * 10_000.0 / 49_000.0;
    let meter = VirtualInstrument::new()
        .with_reading(Quantity::VoltageDc, baseline)
        .with_error_percent(15.0);
    let plan = TestPlan::new(
        "tp-b-out-of-tolerance",
        vec![TestStep::new(
            1,
            "VOUT",
            Quantity::VoltageDc,
            Expected::from_simulation(
                "voltage-divider",
                "VOUT",
                "op",
                baseline,
                0.0,
                "V",
                "VOUT = VIN x R2/(R1+R2)；容差留空即使用 KR2 默认值",
            ),
        )],
    )
    .expect("plan");
    let report = rt.block_on(Executor::new(&meter, None).run(&plan, None));
    let notes = vec![format!(
        "注入 +15% 系统误差（基准 {baseline:.4} V，步骤未指定容差 → 使用默认 KR2 直流阈值 10%）"
    )];
    (report, notes)
}

/// C: ngspice baseline vs sigrok demo acquisition.
fn section_c(rt: &tokio::runtime::Runtime) -> Option<(TestReport, String)> {
    // Baseline: an ngspice transient of a 0->10 V pulse through a series
    // resistor (no load, so the node follows the source).
    let ngspice = rt.block_on(gcode_sim::runner::Ngspice::probe()).ok()?;
    let ir = gcode_kicad::NetlistIr::new(
        vec![gcode_kicad::Component::new("R1", "Device:R", "1k")],
        vec![
            gcode_kicad::Net::new("IN", &[("R1", "1")]),
            gcode_kicad::Net::new("OUT", &[("R1", "2")]),
        ],
    );
    let analysis = gcode_sim::netlist::Analysis::tran(
        vec![gcode_sim::netlist::Stimulus::Pulse {
            net: "IN".into(),
            v1: 0.0,
            v2: 10.0,
            delay_s: 0.0,
            rise_s: 1e-6,
            fall_s: 1e-6,
            width_s: 1e-3,
            period_s: 2e-3,
        }],
        &["OUT"],
        1e-5,
        1e-3,
    );
    let opts = gcode_sim::netlist::DeckOptions::new("r15-cross-domain", "waveform.dat");
    let deck = gcode_sim::netlist::generate(&ir, &analysis, &opts).ok()?;
    let dir = std::env::temp_dir().join("gcode-instrument-acceptance");
    let waveform = rt
        .block_on(ngspice.run(&dir, &deck.text, "waveform.dat"))
        .ok()?;
    let metrics = gcode_sim::analysis::extract_metrics(
        gcode_sim::analysis::MetricSet::Tran,
        &waveform,
        "v(OUT)",
        "v(OUT)",
    );
    let simulated_peak = *metrics.get("max_v")?;

    // Measurement: the sigrok demo device's A0, peak-reduced.
    let sigrok = rt.block_on(SigrokInstrument::probe()).ok()?;
    let req = MeasureRequest::on_channel(Quantity::VoltageDc, "A0")
        .with_samples(64)
        .with_reduce(Reduce::Peak);
    let measured: Measurement = rt.block_on(sigrok.measure(&req)).ok()?;

    let plan = TestPlan::new(
        "tp-cross-domain",
        vec![
            TestStep::new(
                1,
                "demo A0 峰值",
                Quantity::VoltageDc,
                Expected::from_simulation(
                    "sigrok-demo-ref",
                    "OUT",
                    "tran",
                    simulated_peak,
                    20.0,
                    "V",
                    "ngspice 瞬态仿真 0→10V 脉冲的峰值",
                ),
            )
            .on_channel("A0")
            .with_samples(64)
            .with_reduce(Reduce::Peak),
        ],
    )
    .ok()?;

    // A dedicated single-measurement meter so the executor's own measurement call
    // reuses the same request shape.
    struct OneShot(Measurement);
    #[async_trait::async_trait]
    impl Instrument for OneShot {
        fn id(&self) -> String {
            format!("{}:{}", self.0.backend, self.0.device)
        }
        fn capabilities(&self) -> gcode_instrument::hal::Capabilities {
            gcode_instrument::hal::Capabilities {
                quantities: vec![Quantity::VoltageDc],
                max_voltage: 48.0,
                max_current: 5.0,
                can_source: false,
            }
        }
        async fn measure(
            &self,
            _req: &MeasureRequest,
        ) -> Result<Measurement, gcode_instrument::hal::InstrumentError> {
            Ok(self.0.clone())
        }
    }

    let meter = OneShot(measured.clone());
    let report = rt.block_on(
        Executor::new(&meter, None)
            .with_policy(TolerancePolicy::kr2_default())
            .run(&plan, None),
    );

    let detail = format!(
        "基线：ngspice 瞬态仿真峰值 **{simulated_peak:.4} V**（电路 {}）；\n\
         实测：sigrok `{}` 通道 A0 峰值 **{:.4} V**（来源 {}，合成数据：{}）。\n\
         两侧都不是真实板卡数据：基线来自我们自己的仿真激励，\"实测\"来自 sigrok 的合成 demo 驱动。\n\
         这验证的是「仿真基准 ↔ 采集读数」的比较与判定链路。",
        deck.node_map.len(),
        sigrok.device().id,
        measured.value,
        measured.source_label(),
        measured.synthetic
    );
    Some((report, detail))
}
