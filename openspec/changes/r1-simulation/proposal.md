## Why

R0 交付了「NL → 网表 → 确定性 `.kicad_sch` → ERC」闭环，KR1 达标 10/10。但 PRD §4 的 KR2
要求「仿真结果与实测数据在同一电路上误差进入工程可接受范围」，而 R0 只做电气规则检查，
**不做任何数值仿真**：用户无法知道「这个放大器的增益是不是 10」「这个低通的截止频率是不是 1.6kHz」。

PRD §7.2 第 3 项（NL 仿真驱动）与 §7.4（ngspice 驱动层）定义了 R1 要补的能力。
KiCad 10 已能直接从我们生成的原理图导出 SPICE 网表，本机 ngspice 42 可用（无需 root），
依赖已就绪。

## What Changes

- 新增 **`gcode-sim` crate**：SPICE 网表生成（从 `NetlistIr`）、ngspice 驱动、波形取回、
  指标提取与解读、波形/波特图产物。
- **DC 工作点 / AC 扫频（含波特图）/ 瞬态** 三类分析，由对话式配置（分析类型、探针节点、
  扫描范围）。
- **器件模型注册表**：无源件直接映射；有源件（LM321 运放、`Q_NMOS`、LED、L7805）用
  行为级 macromodel（明确标注非厂商模型）。原因：KiCad 的 SPICE 导出对我们这些符号只输出
  占位节点（实测 `U1 __U1`、`Q1 __Q1`、`D1 __D1`），因为符号里没有 SPICE 模型。
- **指标解读**：从波形提取工程指标（直流工作点、-3dB 截止、通带增益、相位裕度、上升时间、
  纹波），与解析预期比对并给 PASS/FAIL + 中文解释与偏差归因。
- **产物**：波形 CSV + 无依赖 SVG 图（波特图/瞬态），供用户直接查看。
- **验收**：电路目录增加「仿真配方」（分析类型、探针、解析预期值、容差），
  10 类电路的可仿真子集必须全部落在容差内（R1 版 KR2 的确定性半段）。
- Agent 暴露仿真工具并更新提示词契约（`sim-workflow` 能力）。

**Non-goals（R1 明确不做）**：数字/混合信号仿真；噪声/失真/蒙特卡洛/灵敏度分析；
自动参数优化与扫参寻优；实测对比（R1.5）；激光制造（R2）；厂商级精确器件模型。

## Capabilities

### New Capabilities

- `sim-netlist`: 从 `NetlistIr` 确定性生成 SPICE 网表：节点映射与接地归一化（`/GND` → `0`）、
  器件卡映射（R/C/L/V/I/D/Q/U）、器件模型注册表与注入、分析卡（DC/AC/瞬态）生成；
  并与 `kicad-cli sch export netlist --format spice` 的拓扑做交叉校验。
- `sim-runner`: ngspice 工具链探测与版本门控、三类分析的执行、机器可读波形取回、
  失败与收敛问题的结构化上报。
- `sim-analysis`: 波形 → 工程指标提取（工作点、-3dB、增益、相位裕度、上升时间、纹波），
  与预期/容差比对给出 PASS/FAIL 与中文解读、偏差归因建议。
- `sim-artifacts`: 波形数据落盘（CSV）与无外部依赖的 SVG 图（波特图/瞬态/直流扫描），
  文件名与内容确定性。
- `sim-catalog`: 电路目录的仿真配方（分析类型、探针、解析预期、容差）+ 离线与集成验收，
  输出可复算的仿真验收报告。
- `sim-workflow`: agent 侧的仿真流程：工具（列出配方/配置分析/跑仿真/解读结果）、
  系统提示词对仿真的约束（不得自己算结论、必须引用仿真结果）。

### Modified Capabilities

<!-- 无：openspec/specs/ 下当前无既有规格（r0-design-mvp 尚未归档）。
     待 R0 归档后，agent 工具注册与提示词可改为对 agent-runtime 的 MODIFIED 增量。 -->

## Impact

**新增代码**
- `crates/gcode-sim`：`netlist`（IR→SPICE）、`models`（器件模型注册表）、
  `runner`（ngspice 驱动）、`analysis`（指标与解读）、`plot`（SVG）、`recipes`（仿真配方）。
- `crates/gcode-tools`：新增 4 个仿真工具，注册进 `all_tools()`。
- `crates/gcode-agent`：提示词增加仿真工作流与「结论必须来自仿真」约束。
- `Cargo.toml`：workspace 增加 `gcode-sim`。

**依赖与环境**
- ngspice 42：本机已可用（`~/.local/bin/ngspice`，从 .deb 解包，无需 root）；
  驱动支持 `GCODE_NGSPICE` 覆盖路径；`libngspice.so.0` 随 KiCad 存在（备选后端，R1 不启用）。
- KiCad 10（已装）：仅用于 SPICE 导出的交叉校验，仿真本身不依赖 KiCad。

**风险**
- **模型精度**：行为级 macromodel 只保证拓扑与量级正确，不保证与厂商器件一致。
  → 文档与工具输出中明确标注；KR2 的误差指标留给 R1.5 用实测数据校准。
- **收敛性**：瞬态分析可能不收敛。→ 报告原始 ngspice 输出与失败原因，不静默失败。
- **节点命名**：KiCad 导出使用 `/NAME` 且不把 GND 映射为 0。→ 归一化层显式处理并有测试。
