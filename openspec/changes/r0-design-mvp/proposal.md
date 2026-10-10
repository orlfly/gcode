## Why

PRD v0.2 定义了 Gcode「想法 → 验证过的设计 → 实测数据 → 可制造的 PCB」的四阶段路线，但只有 R0 已有可落地的代码骨架（`gcode-kicad` 的 s-expr/网表 IR/生成器/ERC 桥、`gcode-tools` 的两个工具、`gcode-agent` 的隔离运行时）。缺口在于：

1. **没有可执行的开发任务分解**：PRD §8 的 R0 只有一句「NL 原理图引擎（10 类典型电路）+ ERC 解读 + diff 修改」，无法据此排期或验收。
2. **核心赌注未闭环**：PRD §7.5 假设「NL → 结构化网表 + ERC 闭环达到可用」，但当前没有 10 类电路的覆盖验证、没有 ERC 错误的「人话解读 + 修复建议」、没有对话式 diff 修改回流。
3. **环境前置未验证**：PRD 附录的 R0 技术验证清单（kicad-cli headless ERC、s-expr 解析覆盖率）尚未执行，阻塞一切后续工作。

本变更把 R0 从路线图条目变成可实施、可验收的规格与任务清单。

## What Changes

- 引入 **R0 设计 MVP** 的完整规格：自然语言 → NetlistIr → 确定性 `.kicad_sch` → ERC 解读 → 对话式 diff 修改 的自查闭环。
- 固化为 6 个新能力（capability）：网表 IR 契约、确定性原理图生成、ERC 验证解读闭环、典型电路目录、对话式编辑、agent 运行时。
- 交付 **KR1 的验收口径**：10 类典型电路（分压、放大、滤波、电源、传感器接口等）从 NL 描述到通过 ERC 的 KiCad 工程，首版成功率 ≥ 7/10（含一轮对话式修正）。
- 明确 R0 的 **Non-goals**：仿真（R1）、实测（R1.5）、激光 G-code（R2）、PCB 布局布线、GUI/KiCad 插件、非 KiCad EDA。
- 记录 R1–R3 的**路线图上下文**（见 Impact），各自在进入阶段时另开独立变更。

## Capabilities

### New Capabilities

- `netlist-ir`: 结构化电路中间表示（NetlistIr）的 schema、校验规则与确定性序列化；LLM 与确定性代码之间唯一允许的契约。
- `schematic-generation`: 从 NetlistIr 确定性生成 `.kicad_sch`（符号实例、引脚、连线、标签），并保证写出的文件可被解析器回读、可被 KiCad 打开。
- `erc-verification`: 通过 `kicad-cli` headless 运行 ERC，解析 JSON 报告，把违规翻译成人话并给出可执行的修复建议，驱动「生成 → 检查 → 修正 → 重生成」自查循环。
- `circuit-catalog`: 10 类典型电路的自然语言描述 → 网表模板/夹具，配套覆盖率与成功率验收标准（KR1）。
- `conversational-edit`: 所有电路修改以 diff 呈现，用户确认后写回；支持增量编辑既有 `.kicad_sch` 而非整体重写。
- `agent-runtime`: `gcode` CLI 二进制、`GCODE_HOME` 隔离运行时、工具注册与系统提示词契约，以及 kicad-cli 工具链探测/版本门控。

### Modified Capabilities

<!-- 无：openspec/specs/ 下当前无既有规格，本变更引入首批能力。 -->

## Impact

**受影响代码**
- `crates/gcode-kicad`：`netlist`（IR 校验扩展、模板）、`schematic`（生成器）、`erc`（解读层）、`sexpr`（覆盖率）。
- `crates/gcode-tools`：工具集从 2 个扩展（生成/校验/diff 应用/库检索）。
- `crates/gcode-agent`：系统提示词契约与工具注册。
- `docs/`：PRD 附录的 R0 技术验证清单需回填结果。

**依赖与环境**
- 外部：KiCad 10（`kicad-cli`）、ngspice（R1 才用，R0 仅需 kicad-cli）。当前本机尚未安装。
- 上游：Jcode workspace（`/opt/workspace/jcode`，path 依赖，只读）。

**路线图上下文（不属于本变更范围）**
| 阶段 | 内容 | 依赖 | 后续动作 |
|---|---|---|---|
| R1 · 仿真 | NL 仿真驱动（DC/AC/瞬态）+ 波形解读 | R0 + ngspice | R0 交付后另开变更 |
| R1.5 · 实测 | Sigrok 集成、仿真-实测对比报告 | R1 + 仪器 | 另开变更 |
| R2 · 制造 | Gerber → GRBL G-code + 走刀预览 + 参数库 | R1.5 | 另开变更 |
| R3 · 全链闭环 | 失败自动建议迭代 | R0–R2 | 另开变更 |

**风险**
- 核心假设（NL→网表可用）若在 10 类电路验收中不达标，需回到 IR schema 与提示词层重设计，R1+ 顺延。
- KiCad 10 未安装 + s-expr 版本兼容性（v10 格式）是首要技术风险，故列为第一批任务。
