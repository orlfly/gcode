## Context

见 `proposal.md - Why`。R0/R1 已交付生成/ERC（KR1 10/10）与仿真（11/11 配方）。
本节只描述 R1.5 的架构选择。已确认的环境事实：

- `sigrok-cli` 0.7.2 用户级可用，`--scan` 只找到内置 **`demo`** 设备（D0-D7 + A0-A4；
  `sigrok-cli -d demo --channels A0 --samples N` 输出形如 `A0: -10.0000 V DC`）。
- 无 `/dev/ttyUSB*`、无 pyserial/pyvisa、无真实 DMM/可编程电源。

约束：R0/R1 行为不得回归；实测必须**默认关闭带电能力**；不得让合成数据被当成真实测量。

## Goals / Non-Goals

**Goals:**

- 一套 HAL 让上层不关心设备差异；后端可探测、可显式选择、失败可解释。
- 安全是**装饰层**而不是约定：任何后端都无法绕过上限、确认令牌与熔断。
- 从仿真基准生成测试计划（预期值有来源），执行后产出符合 PRD §7.3.5 的报告。
- 用真实 sigrok 软件（demo 设备）+ 虚拟仪器把「计划 → 安全 → 执行 → 对比 → 报告」整条链路
  验证到位，并明确标注数据来源。

**Non-Goals:**

- 不做真实板卡结论（无硬件）；不追 L3 波形级；不做 >48V/>5A；不引入 Python 串口栈。

## Decisions

### D1 · HAL：trait + 能力声明 + 三后端

```rust
#[async_trait] pub trait Instrument: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    async fn measure(&self, req: &MeasureRequest) -> Result<Measurement, InstrumentError>;
    async fn source(&self, req: &SourceRequest) -> Result<(), InstrumentError>;  // 仅电源
}
```
`Capabilities` 声明可测量（`Quantity::{Voltage,Current,Resistance,Continuity,Frequency}`）、
上限（电压/电流）与是否支持源输出。
后端：`virtual`（确定性 + 误差注入）、`sigrok`（驱动真实 CLI）、`serial-psu`（SCPI-lite）。
- 备选：直接在各处调用 sigrok-cli。否决：无法测试、无法注入、无法保证安全。

### D2 · 测量结果携带来源（provenance）

`Measurement { quantity, value, unit, backend, device, synthetic: bool, timestamp }`。
`synthetic = true` 用于虚拟后端与 sigrok demo 设备。报告必须展示它。
- 这是防止「合成数据被当成真实板卡结论」的机制性保证，不只是文档约定。

### D3 · 安全用装饰层实现，而非调用方自觉

`SafeInstrument<I: Instrument>` 包裹任意后端，在**进入后端之前**做检查：
1) 电压/电流上限（≤48V / ≤5A）；
2) `source()` 必须持有 `ConfirmToken`（否则拒绝）；
3) 熔断：把「超预期 2 倍电流」判定放在执行器里（它知道预期值），触发即断电+终止。
`ConfirmToken` 只能由工具层用「用户本轮显式确认」构造，且**一次性消耗**（用后失效）。
- 备选：在提示词里要求 agent 遵守。否决：提示词不是安全边界。

### D4 · 预期值必须有来源

```rust
enum Baseline { Simulation { circuit: String, node: String, analysis: String }, User }
struct Expected { value: f64, tol_pct: f64, unit: String, source: Baseline }
```
`TestPlan` 构建时若某步没有 `Expected`，**构建失败**（不是运行时跳过）。
预期值可来自 `gcode_sim::recipes` 的仿真指标（同节点），或用户显式输入。

### D5 · 对比与 KR2 阈值

`Kr2Tolerances { dc_pct: 10.0, ac_freq_pct: 20.0 }`（PRD §4）；可覆盖，且输出必须注明
「使用默认 KR2 阈值」还是「覆盖值」。基准为 0 时只报绝对偏差。
FAIL 给归因（器件容差 / 焊接接触 / 拓扑不符 / 模型偏差 / 测量条件）。

### D6 · TestReport 严格对齐 PRD §7.3.5

```json
{"plan_id":"tp-001","items":[{"step":1,"target":"3V3 rail",
 "expected":{"value":3.3,"tol":0.1},"measured":3.31,"unit":"V","verdict":"PASS"}],
 "summary":"2/3 PASS，1 FAIL 已归因"}
```
额外字段（`source`、`synthetic`、`deviation`）是**补充**，不替代必需字段。

### D7 · 执行器负责时序与熔断

`Executor::run(plan, meter, supply?) -> TestReport`：
逐步骤执行；带电步骤先检查令牌再上电；读到电流超 2× 预期立即断电并终止；
被拒步骤写入报告（`verdict = REJECTED`）而不是静默跳过。
执行器不直接碰进程——它只依赖 `Instrument`，因此可用虚拟后端完整测试。

### D8 · sigrok 后端

命令：`sigrok-cli -d <driver> --channels <ch> --samples <n> [--config samplerate=..]`。
解析：`<ch>: <value> <unit> <type>`（如 `A0: -10.0000 V DC`）。
必须区分「sigrok-cli 缺失」「设备不存在」「解析失败」三类错误并给出不同提示。
`GCODE_SIGROK_CLI` 覆盖可执行文件；默认 `sigrok-cli`。

### D9 · serial-psu 后端不引入串口库

仅用 `tokio::fs` 打开 tty（`O_RDWR`）读写，实现最小 SCPI-lite 子集：
`MEAS:VOLT?` / `MEAS:CURR?` / `VOLT <v>` / `CURR <c>` / `OUT 0|1`。
仅当 `GCODE_PSU_PORT` 设置时可用。解析逻辑用录制样本测试（真实设备行为待接入验证）。

### D10 · 依赖方向

`gcode-kicad`（目录/IR）与 `gcode-sim`（仿真基准）← `gcode-instrument` ← `gcode-tools` ← `gcode-agent`。

## Risks / Trade-offs

- [无真实硬件，KR2 物理结论无法得出] → 报告与文档严格区分「机制已验证」与「物理未定」；
  所有 demo/虚拟数据带 `synthetic` 标记；提供环境变量插槽，插入设备即可跑。
- [demo 设备 ±10V 方波容易被误当成真信号] → D2 的来源标记 + 报告显式标注 + 文档说明。
- [安全实现有漏洞会损坏硬件] → 装饰层 + 一次性令牌 + 熔断，三者都有测试；源输出默认不启用。
- [串口协议差异] → 只做最小子集 + 录制样本测试 + 明确「真实设备待验证」。
- [sigrok 版本/驱动差异] → 后端只依赖 `--version`/`--scan`/逐行读数三个稳定面；错误分类明确。
- [执行器时序错误导致带电状态残留] → 执行器在结束（含熔断/拒绝/异常路径）时统一关闭输出，
  并有「异常路径也必须断电」的测试。

## Migration Plan

1. 加 `gcode-instrument` crate（纯新增，不动 R0/R1）。
2. HAL + 安全装饰层 + 虚拟后端（可离线测）。
3. 计划/对比/报告 IR（可离线测）。
4. 执行器（熔断与拒绝路径，用虚拟后端测）。
5. sigrok 后端（对真实 sigrok-cli 的 demo 设备做集成验证）。
6. serial-psu 后端 + 录制样本解析测试。
7. 工具与提示词接入；跑全量测试。
8. 回滚：删除 workspace 成员即回到 R1 状态。

## Open Questions

- `Continuity`（通断）的量值语义：用电阻阈值判定还是布尔？倾向「电阻值 + 阈值判定」，
  实现时定并可后续调整。
- 是否需要把对比结果反写回 `gcode-sim` 的模型参数（校准）？属 R2+，R1.5 只做判定与归因。
